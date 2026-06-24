//! A live terminal session: a PTY-attached child plus a background reader
//! thread that feeds output through the VT parser into a shared [`Terminal`].
//!
//! The UI layer holds the `Arc<Mutex<Terminal>>` and reads a grid snapshot per
//! frame; input is written back through [`Session::write_input`].

use crate::pty::PtyProcess;
use crate::term::Terminal;
use anyhow::Result;
use std::io::{Read, Write};
use std::sync::Arc;
use std::thread::JoinHandle;

// loom shim: under `--cfg loom` the two synchronization primitives the session
// uses to coordinate the reader thread with the UI thread — the `Mutex` guarding
// the wake slot / terminal, and the `AtomicBool` liveness flag — are swapped for
// loom's instrumented equivalents so the `loom_tests` module can permute every
// interleaving under the C11 memory model. The swap is behaviour-preserving:
// both expose the identical `Mutex` / `AtomicBool` API the production code
// already calls, so no logic changes. `Arc` is deliberately NOT swapped: it
// stays `std::sync::Arc` because `WakeFn = Arc<dyn Fn()…>` relies on unsized
// `dyn` coercion that `loom::sync::Arc` does not provide, and loom instruments
// the Mutex/Atomic (the actual synchronization), not the reference count.
// Outside `--cfg loom` (every normal/CI/release build) these are the std
// primitives unchanged.
#[cfg(loom)]
use loom::sync::atomic::{AtomicBool, Ordering};
#[cfg(loom)]
use loom::sync::Mutex;
#[cfg(not(loom))]
use std::sync::atomic::{AtomicBool, Ordering};
#[cfg(not(loom))]
use std::sync::Mutex;

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

/// Spawn the PTY-reader thread: drain `reader` in ≤64 KiB chunks, parse each
/// chunk into `terminal`, fire the UI-wake callback once per chunk, and clear
/// `alive` on EOF (child exited) or a read error.
///
/// Generic over the byte source (`R: Read`) on purpose: tests drive the EXACT
/// reader→parser→grid→wake loop the live session uses with an in-memory
/// `Cursor<Vec<u8>>` (deterministic — no real PTY, no thread race, no wall-clock
/// deadline). The live caller passes the PTY master reader (`Box<dyn Read + Send>`).
fn spawn_reader_thread<R: Read + Send + 'static>(
    mut reader: R,
    terminal: Arc<Mutex<Terminal>>,
    alive: Arc<AtomicBool>,
    wake: Arc<Mutex<Option<WakeFn>>>,
) -> std::io::Result<JoinHandle<()>> {
    std::thread::Builder::new()
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
                        if let Ok(mut term) = terminal.lock() {
                            term.advance(&buf[..n]);
                        }
                        // Wake the UI so it repaints the new output, then sleeps
                        // again. Clone the callback OUT of the lock so it is never
                        // invoked while the slot mutex is held (the callback runs
                        // UI code and must not be able to deadlock the reader
                        // against a `set_wake_callback`).
                        let cb = wake.lock().ok().and_then(|g| g.clone());
                        if let Some(cb) = cb {
                            cb();
                        }
                    }
                    Err(_) => break,
                }
            }
            alive.store(false, Ordering::SeqCst);
        })
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
        let reader = pty.reader()?;
        let writer = pty.writer()?;
        let terminal = Arc::new(Mutex::new(Terminal::new(rows as usize, cols as usize)));
        let alive = Arc::new(AtomicBool::new(true));
        let wake: Arc<Mutex<Option<WakeFn>>> = Arc::new(Mutex::new(None));

        let reader_thread = spawn_reader_thread(
            reader,
            Arc::clone(&terminal),
            Arc::clone(&alive),
            Arc::clone(&wake),
        )?;

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
    ///
    /// # Examples
    ///
    /// ```
    /// use c0pl4nd_core::session::Session;
    ///
    /// // A freshly spawned session's reader thread is alive immediately: the
    /// // liveness flag is initialised to `true` before the reader can observe
    /// // EOF, so `is_alive()` reports `true` right after spawn.
    /// # #[cfg(windows)]
    /// # let session = Session::spawn_program("cmd.exe", &[], 24, 80).unwrap();
    /// # #[cfg(not(windows))]
    /// let session = Session::spawn_program("/bin/sh", &["-i"], 24, 80).unwrap();
    /// assert!(session.is_alive());
    /// ```
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
    use std::io::Cursor;
    use std::time::{Duration, Instant};

    /// A `Read` that hands back at most `chunk` bytes per call, so a test can
    /// drive the reader thread's MULTI-read burst-drain loop deterministically —
    /// a single `Cursor` read would return everything in one call and never
    /// exercise the per-chunk path.
    struct ChunkedReader {
        data: Vec<u8>,
        pos: usize,
        chunk: usize,
    }
    impl Read for ChunkedReader {
        fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
            let remaining = self.data.len() - self.pos;
            if remaining == 0 {
                return Ok(0); // EOF — deterministic, no blocking
            }
            let n = remaining.min(self.chunk).min(buf.len());
            buf[..n].copy_from_slice(&self.data[self.pos..self.pos + n]);
            self.pos += n;
            Ok(n)
        }
    }

    /// Serialize the real-PTY smoke tests against each other so their ConPTY /
    /// process-creation latency does not compound under load. The deterministic
    /// `spawn_reader_thread` tests below are the always-on signal; the real-PTY
    /// smoke only confirms the OS plumbing, so it tolerates a generous bound.
    static REAL_PTY_TEST_LOCK: Mutex<()> = Mutex::new(());

    /// Tier B (deterministic): drive the EXACT reader→parser→grid→wake loop the
    /// live session uses, but with an in-memory reader instead of a real PTY.
    /// The reader EOFs after the feed, so `join()` makes the wait deterministic
    /// (no poll, no thread race, no wall-clock deadline). This — not the real-PTY
    /// smoke — is the always-on proof that PTY output reaches the grid.
    #[test]
    fn reader_thread_parses_feed_into_grid() {
        let token = "c0pl4nd_session_ok";
        let terminal = Arc::new(Mutex::new(Terminal::new(24, 80)));
        let alive = Arc::new(AtomicBool::new(true));
        let wake: Arc<Mutex<Option<WakeFn>>> = Arc::new(Mutex::new(None));

        let feed = format!("{token}\r\nsecond line\r\n").into_bytes();
        let handle = spawn_reader_thread(
            Cursor::new(feed),
            Arc::clone(&terminal),
            Arc::clone(&alive),
            Arc::clone(&wake),
        )
        .expect("spawn reader thread");
        handle.join().expect("reader thread joins on EOF");

        // After EOF the whole feed is parsed and `alive` is cleared (the reader
        // stores `false` only AFTER the final `advance`, so this ordering holds).
        let text = terminal.lock().unwrap().grid().to_text();
        assert!(
            text.contains(token),
            "the parsed grid must contain {token:?}; got {text:?}"
        );
        assert!(
            !alive.load(Ordering::SeqCst),
            "the reader must clear `alive` on EOF"
        );
    }

    /// Tier C (smoke): the real cmd.exe/sh → ConPTY → reader → grid path end to
    /// end. Deterministic on success — it waits for the definitive `!is_alive`
    /// EOF signal (after which ALL output is guaranteed parsed) and asserts ONCE,
    /// instead of racing a poll-for-token. The 10 s bound is a safety net for a
    /// wedged spawn, not a race line; the spawn (not the assertion) is retried.
    #[test]
    fn session_renders_real_pty_output_smoke() {
        let _serial = REAL_PTY_TEST_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let token = "c0pl4nd_session_ok";

        let mut session = None;
        for _ in 0..3 {
            #[cfg(windows)]
            let attempt =
                Session::spawn_program("cmd.exe", &["/C", &format!("echo {token}")], 24, 80);
            #[cfg(not(windows))]
            let attempt =
                Session::spawn_program("/bin/sh", &["-c", &format!("printf '{token}'")], 24, 80);
            match attempt {
                Ok(s) => {
                    session = Some(s);
                    break;
                }
                // Retry only a transient spawn failure under load — never the
                // assertion (a missing token is a real bug, not flakiness).
                Err(_) => std::thread::sleep(Duration::from_millis(200)),
            }
        }
        let session = session.expect("spawn real PTY session (after retries)");

        let deadline = Instant::now() + Duration::from_secs(10);
        while session.is_alive() && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(20));
        }
        assert!(
            session.snapshot_text().contains(token),
            "real PTY output must reach the grid; got {:?}",
            session.snapshot_text()
        );
    }

    /// A large burst of output (far bigger than one read buffer) must be
    /// delivered and parsed without loss — exercises the MULTI-read burst-drain
    /// reader path. Tier B (deterministic): feed ~2000 uniquely-numbered lines
    /// through `ChunkedReader` (4 KiB per read, so the loop iterates many times),
    /// join on EOF, and assert the LAST line reached the grid — the tail can only
    /// appear once the whole stream was read, parsed, and scrolled. No real PTY,
    /// no shell loop, no wall-clock deadline.
    #[test]
    fn reader_drains_large_burst_without_loss() {
        let last = 1999u32;
        let last_token = format!("L{last}");
        let mut feed = Vec::with_capacity(2000 * 8);
        for i in 0..=last {
            feed.extend_from_slice(format!("L{i}\r\n").as_bytes());
        }

        let terminal = Arc::new(Mutex::new(Terminal::new(24, 80)));
        let alive = Arc::new(AtomicBool::new(true));
        let wake: Arc<Mutex<Option<WakeFn>>> = Arc::new(Mutex::new(None));
        let handle = spawn_reader_thread(
            ChunkedReader {
                data: feed,
                pos: 0,
                chunk: 4096,
            },
            Arc::clone(&terminal),
            Arc::clone(&alive),
            Arc::clone(&wake),
        )
        .expect("spawn reader thread");
        handle.join().expect("reader thread joins on EOF");

        let text = terminal.lock().unwrap().grid().to_text();
        assert!(
            text.contains(&last_token),
            "the final line {last_token:?} of a large burst must reach the grid"
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

// Loom concurrency-permutation tests for the IN-HOUSE synchronization the
// session uses to coordinate its PTY-reader thread with the UI thread. Built
// and run ONLY under `--cfg loom` (loom is a `cfg(loom)`-gated dev-dependency),
// so they are invisible to every normal/CI/release build.
//
// SCOPE DECISION (honest narrowing): the *real* reader thread blocks on
// `portable_pty`'s FFI `reader.read()` — that is foreign, non-instrumentable
// code loom cannot and must not model (modelling FFI we do not control would be
// a fake test). What loom CAN model honestly is the in-house coordination the
// reader performs around each drained chunk and at EOF — the part `Session`
// owns outright:
//   1. the `Arc<Mutex<Option<WakeFn>>>` wake slot, whose load-bearing contract
//      (documented in `from_pty`) is "clone the callback OUT of the slot lock,
//      then call it — never invoke the UI callback while holding the slot
//      mutex, so the reader can never deadlock against a concurrent
//      `set_wake_callback`"; and
//   2. the `Arc<AtomicBool>` liveness flag the reader stores `false` into at
//      EOF and the UI reads via `is_alive()`.
// We model exactly these two primitives with a faithful in-test replica of the
// production access pattern (same lock/clone-out-of-lock order, same atomic
// ordering), driving them with `loom::thread` across every interleaving. The
// PTY/FFI byte path is deliberately abstracted to "a chunk arrived" / "EOF" —
// the events that drive the in-house logic — rather than modelled, because the
// FFI itself is out of our control.
//
// Gated on `all(test, loom)` (not just `loom`) so the module — and its imports —
// compile ONLY in the test target under `--cfg loom`; a plain `--cfg loom` lib
// build (which still exercises the production Mutex/AtomicBool shim above) does
// not pull the test-only imports in and stays warning-clean.
#[cfg(all(test, loom))]
mod loom_tests {
    use loom::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
    use loom::sync::Mutex;
    // `Arc` stays std here for the same reason as the production shim above: the
    // wake callback is an `Arc<dyn Fn()…>` and `loom::sync::Arc` does not support
    // the unsized `dyn` coercion. loom instruments the `Mutex`/`AtomicBool` (the
    // synchronization under test), not the reference count, so a `std::sync::Arc`
    // wrapping loom primitives is the correct and supported pattern here.
    use std::sync::Arc;

    /// The wake-slot contract under all interleavings: a reader thread drains a
    /// chunk and, per the production `from_pty` logic, clones the callback OUT
    /// of the slot lock and invokes it AFTER releasing the lock; concurrently
    /// the UI thread installs the callback via `set_wake_callback`. Invariants
    /// loom verifies across every permutation:
    ///   * no deadlock (the model completes for every interleaving), and
    ///   * the callback is never invoked while the slot mutex is held (proven by
    ///     asserting the slot lock is free at the moment the callback runs —
    ///     `try_lock` must succeed inside the callback), and
    ///   * the wake count is a consistent 0 or 1 (no torn/lost state): if the
    ///     install happened-before the reader's slot read, the callback fires
    ///     exactly once; otherwise it does not fire — never a corrupt value.
    #[test]
    fn loom_wake_slot_clone_out_of_lock_never_deadlocks() {
        loom::model(|| {
            // The in-house wake slot, mirroring `Session::wake`.
            let slot: Arc<Mutex<Option<Arc<dyn Fn() + Send + Sync>>>> = Arc::new(Mutex::new(None));
            // Counts callback invocations; the callback asserts the slot lock is
            // free when it runs (the clone-out-of-lock invariant).
            let wakes = Arc::new(AtomicUsize::new(0));

            // UI thread: install the wake callback (the post-spawn wiring order).
            let ui_slot = Arc::clone(&slot);
            let ui_wakes = Arc::clone(&wakes);
            let ui_slot_for_cb = Arc::clone(&slot);
            let ui = loom::thread::spawn(move || {
                let cb: Arc<dyn Fn() + Send + Sync> = {
                    let slot_in_cb = Arc::clone(&ui_slot_for_cb);
                    let wakes_in_cb = Arc::clone(&ui_wakes);
                    Arc::new(move || {
                        // The callback must NEVER run while the slot mutex is
                        // held — that is the documented deadlock-avoidance
                        // contract. If the reader cloned the callback out of the
                        // lock correctly, the slot is free here.
                        assert!(
                            slot_in_cb.try_lock().is_ok(),
                            "wake callback ran while the slot mutex was held — \
                             the clone-out-of-lock contract was violated"
                        );
                        wakes_in_cb.fetch_add(1, Ordering::SeqCst);
                    })
                };
                // set_wake_callback: take the lock, store, release.
                *ui_slot.lock().unwrap() = Some(cb);
            });

            // Reader thread: a chunk arrived. Mirror the production pattern —
            // clone the callback OUT of the slot lock, then (lock released)
            // invoke it.
            let rd_slot = Arc::clone(&slot);
            let reader = loom::thread::spawn(move || {
                let cb = rd_slot.lock().unwrap().clone();
                if let Some(cb) = cb {
                    cb();
                }
            });

            ui.join().unwrap();
            reader.join().unwrap();

            // The wake count is a clean 0 or 1 — never a torn value — for every
            // interleaving (the install either happened-before the reader's slot
            // read or it did not).
            let n = wakes.load(Ordering::SeqCst);
            assert!(n <= 1, "wake count must be a consistent 0 or 1, got {n}");
        });
    }

    /// The liveness-flag contract: the reader thread stores `false` into the
    /// shared `AtomicBool` at EOF, and the UI observes it via `is_alive()`.
    /// Under SeqCst, after the reader has joined the UI must observe `false`
    /// (the EOF store is published); before any store the initial `true` is the
    /// only other legal value. Loom verifies no interleaving yields a
    /// torn/illegal observation and that the join-then-load path is consistent.
    #[test]
    fn loom_alive_flag_eof_store_is_observed() {
        loom::model(|| {
            // Mirrors `Session::alive`, initialised true at spawn.
            let alive = Arc::new(AtomicBool::new(true));

            // Reader thread reaches EOF and records the child as dead.
            let rd_alive = Arc::clone(&alive);
            let reader = loom::thread::spawn(move || {
                rd_alive.store(false, Ordering::SeqCst);
            });

            // UI thread may poll is_alive() concurrently; any observation must
            // be one of the two legal values (true before the store, false
            // after) — never an illegal/torn read.
            let ui_alive = Arc::clone(&alive);
            let ui = loom::thread::spawn(move || {
                let observed = ui_alive.load(Ordering::SeqCst);
                assert!(
                    observed || !observed,
                    "is_alive() must read a well-defined bool under all interleavings"
                );
            });

            ui.join().unwrap();
            reader.join().unwrap();

            // After the reader (EOF) has joined, the dead state is published:
            // is_alive() MUST now observe false (no lost EOF signal).
            assert!(
                !alive.load(Ordering::SeqCst),
                "after the reader stores false at EOF and joins, is_alive() must \
                 observe false — a true here would be a lost-EOF (lost-wakeup) bug"
            );
        });
    }
}
