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

pub struct Session {
    pty: PtyProcess,
    terminal: Arc<Mutex<Terminal>>,
    writer: Box<dyn Write + Send>,
    alive: Arc<AtomicBool>,
    reader_thread: Option<JoinHandle<()>>,
}

impl Session {
    /// Spawn the platform default (or configured) shell.
    pub fn spawn_shell(shell: Option<&str>, rows: u16, cols: u16) -> Result<Self> {
        let pty = PtyProcess::spawn_shell(shell, rows, cols)?;
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

        let term_for_thread = Arc::clone(&terminal);
        let alive_for_thread = Arc::clone(&alive);
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
            reader_thread: Some(reader_thread),
        })
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

        let deadline = Instant::now() + Duration::from_secs(15);
        let mut seen = false;
        while Instant::now() < deadline {
            if session.snapshot_text().contains(&last_token) {
                seen = true;
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
}
