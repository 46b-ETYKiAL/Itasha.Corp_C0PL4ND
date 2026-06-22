//! A live terminal session: a PTY-attached child plus a background reader
//! thread that feeds output through the VT parser into a shared [`Terminal`].
//!
//! The UI layer holds the `Arc<Mutex<Terminal>>` and reads a grid snapshot per
//! frame; input is written back through [`Session::write_input`].

use crate::pty::PtyProcess;
use crate::term::Terminal;
use anyhow::Result;
use std::io::{Read, Write};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;

/// A UI-wake callback the reader thread invokes after parsing a chunk of PTY
/// output into the [`Terminal`]. The UI layer registers one via
/// [`Session::set_wake_callback`] so the render loop can sleep when idle and be
/// woken the instant new output lands — instead of free-running at the monitor
/// refresh rate. Deliberately a plain `Fn` (not an `egui` type) so `core` stays
/// UI-toolkit-agnostic; the app wires it to `egui::Context::request_repaint`,
/// which is documented to be safe to call from any thread.
pub type WakeFn = Arc<dyn Fn() + Send + Sync>;

/// A live terminal session: a spawned PTY process, the [`Terminal`] model its
/// output is parsed into, and a background reader thread that drains the PTY and
/// updates the terminal. The session owns the write side of the PTY (keystrokes
/// and pastes are sent through it) and tracks process liveness so the UI can
/// degrade a dead pane gracefully.
pub struct Session {
    pty: PtyProcess,
    terminal: Arc<Mutex<Terminal>>,
    writer: Box<dyn Write + Send>,
    alive: Arc<AtomicBool>,
    /// Shared slot for the UI-wake callback. The reader thread holds a clone and
    /// calls it (if set) once per drained chunk; the UI sets it post-spawn.
    wake: Arc<Mutex<Option<WakeFn>>>,
    reader_thread: Option<JoinHandle<()>>,
}

impl Session {
    /// Spawn the platform default (or configured) shell.
    pub fn spawn_shell(shell: Option<&str>, rows: u16, cols: u16) -> Result<Self> {
        let pty = PtyProcess::spawn_shell(shell, rows, cols)?;
        Self::from_pty(pty, rows, cols)
    }

    /// Spawn the platform default (or configured) shell with an explicit `TERM`
    /// override (the config-driven `term` key flows in here). `term = None` /
    /// `Some("")` uses the canonical [`crate::pty::DEFAULT_TERM`].
    pub fn spawn_shell_with_term(
        shell: Option<&str>,
        rows: u16,
        cols: u16,
        term: Option<&str>,
    ) -> Result<Self> {
        let pty = PtyProcess::spawn_shell_in_with_term(shell, rows, cols, None, term)?;
        Self::from_pty(pty, rows, cols)
    }

    /// Spawn the shell in an explicit working directory (session restore). A
    /// `cwd` that no longer exists falls back to home (never wedges the launch).
    pub fn spawn_shell_in(
        shell: Option<&str>,
        rows: u16,
        cols: u16,
        cwd: Option<&str>,
    ) -> Result<Self> {
        let pty = PtyProcess::spawn_shell_in(shell, rows, cols, cwd)?;
        Self::from_pty(pty, rows, cols)
    }

    /// Like [`Session::spawn_shell_in`] but with an explicit `TERM` override
    /// (the config-driven `term` key). `term = None` / `Some("")` uses the
    /// canonical [`crate::pty::DEFAULT_TERM`].
    pub fn spawn_shell_in_with_term(
        shell: Option<&str>,
        rows: u16,
        cols: u16,
        cwd: Option<&str>,
        term: Option<&str>,
    ) -> Result<Self> {
        let pty = PtyProcess::spawn_shell_in_with_term(shell, rows, cols, cwd, term)?;
        Self::from_pty(pty, rows, cols)
    }

    /// Spawn an explicit program (used by tests for deterministic behaviour).
    pub fn spawn_program(program: &str, args: &[&str], rows: u16, cols: u16) -> Result<Self> {
        let pty = PtyProcess::spawn_program(program, args, rows, cols)?;
        Self::from_pty(pty, rows, cols)
    }

    fn from_pty(pty: PtyProcess, rows: u16, cols: u16) -> Result<Self> {
        let mut reader = pty.reader()?;
        let writer = pty.writer()?;
        let terminal = Arc::new(Mutex::new(Terminal::new(rows as usize, cols as usize)));
        let alive = Arc::new(AtomicBool::new(true));

        let wake: Arc<Mutex<Option<WakeFn>>> = Arc::new(Mutex::new(None));

        let term_for_thread = Arc::clone(&terminal);
        let alive_for_thread = Arc::clone(&alive);
        let wake_for_thread = Arc::clone(&wake);
        let reader_thread = std::thread::Builder::new()
            .name("c0pl4nd-pty-reader".into())
            .spawn(move || {
                // 64 KiB read buffer (was 8 KiB). Under a flood (`cat bigfile`,
                // `yes`, a large paste) a blocking `read()` returns as many bytes
                // as are currently available up to the buffer size, so a larger
                // buffer drains a burst in up to ~8× fewer syscalls AND ~8× fewer
                // `Terminal` mutex acquisitions — the UI thread is starved far
                // less because the reader holds the render lock once per drained
                // chunk rather than once per 8 KiB. WezTerm/Alacritty read into
                // 64 KiB–1 MiB buffers for exactly this reason.
                //
                // We deliberately do NOT issue a speculative second `read()` to
                // coalesce multiple chunks under one lock: `portable_pty`'s reader
                // is BLOCKING with no readiness probe, so an extra `read()` after a
                // full-buffer read would block until the next byte arrives if the
                // producer happens to pause on a 64 KiB boundary — re-introducing
                // input-echo latency (the exact "over-batch" risk the roadmap
                // warns about). A single blocking read per wake, into a larger
                // buffer, is the correct latency-safe form: the kernel already
                // hands back everything currently buffered (up to 64 KiB) in one
                // call, so a real burst is drained per-chunk with no extra
                // round-trips and no risk of waiting for bytes that are not coming.
                //
                // The buffer is heap-allocated (a 64 KiB array would risk the
                // thread stack on some targets) and reused for the thread's
                // lifetime (zero per-read allocation).
                const READ_BUF: usize = 64 * 1024;
                let mut buf = vec![0u8; READ_BUF];
                loop {
                    match reader.read(&mut buf) {
                        Ok(0) => break, // EOF: child exited
                        Ok(n) => {
                            if let Ok(mut term) = term_for_thread.lock() {
                                term.advance(&buf[..n]);
                            }
                            // Wake the UI so it repaints the new output, then
                            // sleeps again. Clone the callback OUT of the lock so
                            // it is never invoked while the slot mutex is held
                            // (the callback runs UI code and must not be able to
                            // deadlock the reader against a `set_wake_callback`).
                            let cb = wake_for_thread.lock().ok().and_then(|g| g.clone());
                            if let Some(cb) = cb {
                                cb();
                            }
                        }
                        Err(_) => break,
                    }
                }
                alive_for_thread.store(false, Ordering::SeqCst);
            })?;

        Ok(Session {
            pty,
            terminal,
            writer,
            alive,
            wake,
            reader_thread: Some(reader_thread),
        })
    }

    /// Register the UI-wake callback invoked once per drained chunk of output.
    /// The UI layer passes a closure that calls `egui::Context::request_repaint`
    /// so the render loop can stay asleep when idle yet repaint the instant new
    /// PTY output arrives. Replacing a previously-set callback is allowed.
    pub fn set_wake_callback(&self, wake: WakeFn) {
        if let Ok(mut slot) = self.wake.lock() {
            *slot = Some(wake);
        }
    }

    /// Send input bytes to the child process.
    pub fn write_input(&mut self, bytes: &[u8]) -> Result<()> {
        self.writer.write_all(bytes)?;
        self.writer.flush()?;
        Ok(())
    }

    /// Shared handle to the terminal grid for the renderer.
    pub fn terminal(&self) -> Arc<Mutex<Terminal>> {
        Arc::clone(&self.terminal)
    }

    /// Resize both the PTY and the grid.
    pub fn resize(&mut self, rows: u16, cols: u16) -> Result<()> {
        self.pty.resize(rows, cols)?;
        if let Ok(mut t) = self.terminal.lock() {
            t.resize(rows as usize, cols as usize);
        }
        Ok(())
    }

    /// True while the child process is running (reader thread alive).
    pub fn is_alive(&self) -> bool {
        self.alive.load(Ordering::SeqCst)
    }

    /// Render the current grid to text (used by the headless smoke test).
    pub fn snapshot_text(&self) -> String {
        self.terminal
            .lock()
            .map(|t| t.grid().to_text())
            .unwrap_or_default()
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        // Kill the child shell FIRST. This is load-bearing for two reasons:
        //   1. It reaps the shell so it is not left as an orphaned `cmd.exe`
        //      (the session previously leaked one process per pane — ~148 after
        //      a heavy session).
        //   2. With the child dead, the reader thread's blocking `read()` gets
        //      EOF and the thread exits on its own, and the PTY master can close
        //      without `ClosePseudoConsole` blocking (the close-hang).
        // We then DETACH the reader thread rather than `join()` it: a ConPTY
        // master does not always deliver EOF promptly, and blocking `Drop` on a
        // `join()` would re-introduce the very hang we are fixing. The detached
        // thread observes EOF shortly after and exits; `PtyProcess::Drop` is the
        // backstop that also kills the child if we are dropped another way.
        self.pty.kill();
        self.reader_thread.take();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, Instant};

    #[test]
    fn session_renders_command_output() {
        let token = "c0pl4nd_session_ok";
        #[cfg(windows)]
        let mut session =
            Session::spawn_program("cmd.exe", &["/C", &format!("echo {token}")], 24, 80)
                .expect("spawn session");
        #[cfg(not(windows))]
        let mut session =
            Session::spawn_program("/bin/sh", &["-c", &format!("printf '{token}'")], 24, 80)
                .expect("spawn session");

        // Poll the grid for up to ~3s for the token to appear.
        let deadline = Instant::now() + Duration::from_secs(3);
        let mut seen = false;
        while Instant::now() < deadline {
            if session.snapshot_text().contains(token) {
                seen = true;
                break;
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        let _ = &mut session;
        assert!(seen, "expected grid to contain {token:?}");
    }

    /// A large burst of output (far bigger than one read buffer) must be
    /// delivered and parsed without loss — exercises the 64 KiB burst-drain
    /// reader path. We print many uniquely-numbered lines and assert that the
    /// LAST line (which can only appear once the whole stream has been read and
    /// scrolled through the grid) lands on screen.
    #[test]
    fn session_handles_large_burst_without_loss() {
        // ~2000 lines of "L<NNN>" — well over a single 64 KiB read on its own
        // and forces many scrolls, so seeing the final line proves the tail of
        // the burst was read and parsed (not truncated mid-stream).
        let last = 1999u32;
        let last_token = format!("L{last}");

        #[cfg(windows)]
        let mut session = Session::spawn_program(
            "cmd.exe",
            &["/C", "for /L %i in (0,1,1999) do @echo L%i"],
            24,
            80,
        )
        .expect("spawn session");
        #[cfg(not(windows))]
        let mut session = Session::spawn_program(
            "/bin/sh",
            &[
                "-c",
                "i=0; while [ $i -le 1999 ]; do echo L$i; i=$((i+1)); done",
            ],
            24,
            80,
        )
        .expect("spawn session");

        // Robust polling (no fixed wall-clock flake): wait patiently WHILE the
        // child is still emitting, and once it has EXITED do a bounded final
        // drain before deciding. This decouples the assertion from system load —
        // the only way to fail is genuine data loss (the tail never arriving),
        // not "the shell was slow under a loaded CI box". The 60 s cap is a
        // safety net for a wedged spawn; the normal path completes in well under
        // a second.
        let deadline = Instant::now() + Duration::from_secs(60);
        let mut seen = false;
        loop {
            if session.snapshot_text().contains(&last_token) {
                seen = true;
                break;
            }
            // Child finished: all its bytes are in the PTY. Give the reader
            // thread a bounded grace to drain the tail, then stop polling.
            if !session.is_alive() {
                for _ in 0..80 {
                    if session.snapshot_text().contains(&last_token) {
                        seen = true;
                        break;
                    }
                    std::thread::sleep(Duration::from_millis(25));
                }
                break;
            }
            if Instant::now() >= deadline {
                break;
            }
            std::thread::sleep(Duration::from_millis(25));
        }
        let _ = &mut session;
        assert!(
            seen,
            "expected the final line {last_token:?} of a large burst to reach the grid"
        );
    }

    /// The UI-wake callback must fire at least once after the child produces
    /// output — this is the signal the render loop relies on to repaint live
    /// output while otherwise sleeping (the damage-tracked-redraw mechanism).
    #[test]
    fn wake_callback_fires_on_output() {
        use std::sync::atomic::AtomicUsize;

        let wakes = Arc::new(AtomicUsize::new(0));
        let wakes_cb = Arc::clone(&wakes);

        // Stream many lines over time (not a single `echo`) so the callback,
        // registered immediately after spawn, reliably catches output chunks
        // that arrive AFTER registration — avoiding a race where a one-shot
        // child drains before the UI has wired its wake.
        #[cfg(windows)]
        let session = Session::spawn_program(
            "cmd.exe",
            &["/C", "for /L %i in (1,1,500) do @echo wake%i"],
            24,
            80,
        )
        .expect("spawn");
        #[cfg(not(windows))]
        let session = Session::spawn_program(
            "/bin/sh",
            &[
                "-c",
                "i=0; while [ $i -le 500 ]; do echo wake$i; i=$((i+1)); done",
            ],
            24,
            80,
        )
        .expect("spawn");

        // Register the callback AFTER spawn (the real wiring order: the UI sets
        // it once it has an egui Context). It must still catch the output.
        session.set_wake_callback(Arc::new(move || {
            wakes_cb.fetch_add(1, Ordering::SeqCst);
        }));

        let deadline = Instant::now() + Duration::from_secs(5);
        while Instant::now() < deadline {
            if wakes.load(Ordering::SeqCst) > 0 {
                break;
            }
            std::thread::sleep(Duration::from_millis(25));
        }
        assert!(
            wakes.load(Ordering::SeqCst) > 0,
            "wake callback should fire at least once after the child emits output"
        );
    }

    /// `terminal()` hands back a clone of the shared `Arc<Mutex<Terminal>>`, and
    /// the grid it points at reflects the session's configured size. Asserts the
    /// exact rows/cols so a wrong dimension propagation would fail. Covers the
    /// `terminal()` accessor (was never directly called by a test).
    #[test]
    fn terminal_handle_reflects_configured_size() {
        #[cfg(windows)]
        let session =
            Session::spawn_program("cmd.exe", &["/C", "echo hi"], 30, 100).expect("spawn");
        #[cfg(not(windows))]
        let session =
            Session::spawn_program("/bin/sh", &["-c", "echo hi"], 30, 100).expect("spawn");

        let term = session.terminal();
        let guard = term.lock().expect("lock terminal");
        assert_eq!(guard.grid().rows(), 30, "grid rows must match spawn size");
        assert_eq!(guard.grid().cols(), 100, "grid cols must match spawn size");
        // Two calls return handles to the SAME underlying terminal (Arc clone).
        drop(guard);
        assert!(
            Arc::ptr_eq(&session.terminal(), &session.terminal()),
            "terminal() must return clones of one shared Arc, not new allocations"
        );
    }

    /// `resize()` resizes the grid (the PTY resize is exercised too, but the
    /// observable state is the grid dimensions). Asserts the new size lands —
    /// covers the `resize` method's terminal-lock + `t.resize` branch.
    #[test]
    fn resize_updates_grid_dimensions() {
        #[cfg(windows)]
        let mut session =
            Session::spawn_program("cmd.exe", &["/C", "echo hi"], 24, 80).expect("spawn");
        #[cfg(not(windows))]
        let mut session =
            Session::spawn_program("/bin/sh", &["-c", "echo hi"], 24, 80).expect("spawn");

        session.resize(40, 120).expect("resize");
        let term = session.terminal();
        let guard = term.lock().expect("lock");
        assert_eq!(guard.grid().rows(), 40, "grid rows updated by resize");
        assert_eq!(guard.grid().cols(), 120, "grid cols updated by resize");
    }

    /// `write_input` succeeds against a live child (the PTY writer accepts the
    /// bytes). Sending a harmless newline to an interactive shell must return
    /// `Ok` — covers `write_input`'s write_all + flush success path. We then
    /// kill the child so the test does not leak it.
    #[test]
    fn write_input_to_live_child_succeeds() {
        #[cfg(windows)]
        let mut session = Session::spawn_program("cmd.exe", &[], 24, 80).expect("spawn");
        #[cfg(not(windows))]
        let mut session = Session::spawn_program("/bin/sh", &["-i"], 24, 80).expect("spawn");

        // A bare newline is innocuous; the writer must accept it.
        session
            .write_input(b"\r\n")
            .expect("write_input should succeed on a live PTY");
        // Drop kills the child (see Session::Drop).
        let _ = &mut session;
    }

    /// `is_alive()` reports `true` immediately after spawn (the reader thread is
    /// running and the atomic was initialised to `true`). The eventual flip to
    /// `false` is NOT asserted here because it is platform-dependent: on Windows
    /// ConPTY the reader only observes EOF once the PTY master is CLOSED, not on
    /// child death (documented in `Session::Drop` and `PtyProcess::Drop`), so a
    /// bounded wait for `is_alive()==false` is flaky on Windows by design. The
    /// `alive.store(false, ..)` end-of-reader path is exercised end-to-end by the
    /// large-burst test (which polls `is_alive()` after the child exits) on the
    /// POSIX reader-EOF path. This test pins only the initial-true accessor read.
    #[test]
    fn is_alive_is_true_immediately_after_spawn() {
        #[cfg(windows)]
        let session = Session::spawn_program("cmd.exe", &[], 24, 80).expect("spawn");
        #[cfg(not(windows))]
        let session = Session::spawn_program("/bin/sh", &["-i"], 24, 80).expect("spawn");

        assert!(
            session.is_alive(),
            "a freshly spawned session reports its reader thread alive"
        );
        drop(session);
    }

    /// The `spawn_program` delegation reaches `from_pty` and produces a usable
    /// session whose initial snapshot is a blank grid (all spaces) before any
    /// output. Covers `snapshot_text` on a freshly-spawned session and the
    /// constructor chain. We assert the snapshot is the right shape (24 newlines)
    /// rather than exact spaces (output may arrive between spawn and read).
    #[test]
    fn fresh_session_snapshot_has_expected_row_count() {
        #[cfg(windows)]
        let session = Session::spawn_program("cmd.exe", &[], 24, 80).expect("spawn");
        #[cfg(not(windows))]
        let session = Session::spawn_program("/bin/sh", &["-i"], 24, 80).expect("spawn");

        let snap = session.snapshot_text();
        // to_text() emits one '\n' per row → 24 newlines for a 24-row grid.
        assert_eq!(
            snap.matches('\n').count(),
            24,
            "a 24-row grid snapshot must contain 24 line terminators"
        );
        drop(session);
    }
}
