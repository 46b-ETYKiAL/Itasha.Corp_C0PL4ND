//! Pseudo-terminal abstraction.
//!
//! Built on `portable-pty` so the same API drives ConPTY on Windows and a
//! POSIX PTY on Linux/macOS. The rest of the engine depends only on the
//! [`PtyProcess`] surface, keeping platform specifics in one place.

use anyhow::{Context, Result};
use portable_pty::{native_pty_system, Child, CommandBuilder, MasterPty, PtySize};
use std::io::{Read, Write};

/// A spawned child process attached to a PTY master.
pub struct PtyProcess {
    master: Box<dyn MasterPty + Send>,
    child: Box<dyn Child + Send + Sync>,
}

impl PtyProcess {
    /// Spawn the platform default (or configured) shell, attached to a PTY of
    /// the given size.
    pub fn spawn_shell(shell: Option<&str>, rows: u16, cols: u16) -> Result<Self> {
        Self::spawn_shell_in(shell, rows, cols, None)
    }

    /// Spawn the shell with an explicit working directory (session restore). The
    /// `cwd` is used only when it names an existing directory; otherwise the
    /// spawn falls back to the home directory (a restored cwd that no longer
    /// exists must not wedge the launch).
    pub fn spawn_shell_in(
        shell: Option<&str>,
        rows: u16,
        cols: u16,
        cwd: Option<&str>,
    ) -> Result<Self> {
        let program = shell.map(str::to_string).unwrap_or_else(default_shell);
        Self::spawn_program_in(&program, &[], rows, cols, cwd)
    }

    /// Spawn an explicit program (used by tests for deterministic one-shots).
    pub fn spawn_program(program: &str, args: &[&str], rows: u16, cols: u16) -> Result<Self> {
        Self::spawn_program_in(program, args, rows, cols, None)
    }

    /// Spawn an explicit program with an optional working directory.
    pub fn spawn_program_in(
        program: &str,
        args: &[&str],
        rows: u16,
        cols: u16,
        cwd: Option<&str>,
    ) -> Result<Self> {
        let pty_system = native_pty_system();
        let pair = pty_system
            .openpty(PtySize {
                rows,
                cols,
                pixel_width: 0,
                pixel_height: 0,
            })
            .context("openpty failed")?;

        let mut cmd = CommandBuilder::new(program);
        for a in args {
            cmd.arg(a);
        }
        // Prefer the requested cwd when it exists; else fall back to home.
        let dir = cwd
            .map(std::path::PathBuf::from)
            .filter(|p| p.is_dir())
            .or_else(dirs_home);
        if let Some(d) = dir {
            cmd.cwd(d);
        }

        let child = pair
            .slave
            .spawn_command(cmd)
            .context("failed to spawn shell in PTY")?;
        // The slave handle is dropped here; the master keeps the PTY alive.
        Ok(PtyProcess {
            master: pair.master,
            child,
        })
    }

    /// A cloned reader for the PTY output stream (move to a reader thread).
    pub fn reader(&self) -> Result<Box<dyn Read + Send>> {
        self.master
            .try_clone_reader()
            .context("failed to clone PTY reader")
    }

    /// The writer for sending input to the child.
    pub fn writer(&self) -> Result<Box<dyn Write + Send>> {
        self.master
            .take_writer()
            .context("failed to take PTY writer")
    }

    /// Resize the PTY (call on window resize).
    pub fn resize(&self, rows: u16, cols: u16) -> Result<()> {
        self.master
            .resize(PtySize {
                rows,
                cols,
                pixel_width: 0,
                pixel_height: 0,
            })
            .context("PTY resize failed")
    }

    /// Wait for the child to exit, returning whether it exited successfully.
    pub fn wait(&mut self) -> Result<bool> {
        let status = self.child.wait().context("wait failed")?;
        Ok(status.success())
    }

    /// The OS process id of the child shell, if still known.
    pub fn child_pid(&self) -> Option<u32> {
        self.child.process_id()
    }

    /// Terminate the child shell. Best-effort and idempotent (a child that has
    /// already exited returns an error, which is ignored). Killing the child is
    /// what makes [`Drop`] safe: see the `Drop` impl for why the order matters.
    pub fn kill(&mut self) {
        let _ = self.child.kill();
    }
}

impl Drop for PtyProcess {
    fn drop(&mut self) {
        // Kill the child BEFORE the `master` field is dropped. On Windows the
        // master's `Drop` calls `ClosePseudoConsole`, which BLOCKS until the
        // attached process exits — so dropping the master while the shell is
        // still alive hangs the whole app (the "close → not responding" bug).
        // Killing first (a) makes that close return promptly and (b) reaps the
        // shell so it is not left orphaned (the leaked-`cmd.exe` bug). Struct
        // fields drop in declaration order (`master` then `child`) AFTER this
        // body runs, so by the time `ClosePseudoConsole` fires the child is gone.
        let _ = self.child.kill();
    }
}

fn dirs_home() -> Option<std::path::PathBuf> {
    #[cfg(windows)]
    {
        std::env::var_os("USERPROFILE").map(std::path::PathBuf::from)
    }
    #[cfg(not(windows))]
    {
        std::env::var_os("HOME").map(std::path::PathBuf::from)
    }
}

/// The platform default interactive shell.
pub fn default_shell() -> String {
    #[cfg(windows)]
    {
        // Prefer PowerShell if present via COMSPEC fallback to cmd.exe.
        std::env::var("COMSPEC").unwrap_or_else(|_| "cmd.exe".to_string())
    }
    #[cfg(not(windows))]
    {
        std::env::var("SHELL").unwrap_or_else(|_| "/bin/sh".to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_shell_is_nonempty() {
        assert!(!default_shell().is_empty());
    }

    /// `kill()` must actually terminate an otherwise-forever-running interactive
    /// shell. This is the behavioural proof of the leaked-shell fix: `wait()` on
    /// an interactive `cmd.exe`/`sh` blocks forever UNLESS the child was killed,
    /// so a bounded `wait()` that returns proves the shell was reaped (no orphan).
    /// (On Windows ConPTY the *reader* only EOFs once the master is closed, not
    /// on child death — so we assert child termination via `wait()`, which is the
    /// property that prevents the orphaned-`cmd.exe` leak and lets
    /// `ClosePseudoConsole` return instead of hanging the app on close.)
    #[test]
    fn kill_terminates_interactive_child() {
        // An interactive shell with no `-c`/`/C` waits on stdin forever.
        #[cfg(windows)]
        let mut proc = PtyProcess::spawn_program("cmd.exe", &[], 24, 80).expect("spawn cmd");
        #[cfg(not(windows))]
        let mut proc = PtyProcess::spawn_program("/bin/sh", &["-i"], 24, 80).expect("spawn sh");

        assert!(proc.child_pid().is_some(), "child should have a pid");

        proc.kill();

        // `wait()` on a worker thread, bounded: it returns only once the child is
        // dead. Without the kill, an interactive shell never exits → timeout.
        let (tx, rx) = std::sync::mpsc::channel::<()>();
        std::thread::spawn(move || {
            let _ = proc.wait();
            let _ = tx.send(());
        });
        let reaped = rx.recv_timeout(std::time::Duration::from_secs(5)).is_ok();
        assert!(
            reaped,
            "after kill(), the child shell must terminate (wait() returns) — \
             a timeout here means the shell outlived kill() (the orphan-leak bug)"
        );
    }

    #[test]
    fn spawn_one_shot_echo_round_trips() {
        // Deterministic, cross-platform one-shot: echo a token and exit.
        let token = "c0pl4nd_pty_ok";
        #[cfg(windows)]
        let mut proc =
            PtyProcess::spawn_program("cmd.exe", &["/C", &format!("echo {token}")], 24, 80)
                .expect("spawn cmd echo");
        #[cfg(not(windows))]
        let mut proc =
            PtyProcess::spawn_program("/bin/sh", &["-c", &format!("echo {token}")], 24, 80)
                .expect("spawn sh echo");

        let mut reader = proc.reader().expect("reader");
        // Read on a worker thread so a ConPTY master that does not EOF
        // promptly after the child exits can never hang the test.
        let (tx, rx) = std::sync::mpsc::channel::<Vec<u8>>();
        std::thread::spawn(move || {
            let mut buf = Vec::new();
            let mut chunk = [0u8; 4096];
            loop {
                match reader.read(&mut chunk) {
                    Ok(0) => break,
                    Ok(n) => {
                        buf.extend_from_slice(&chunk[..n]);
                        if String::from_utf8_lossy(&buf).contains("c0pl4nd_pty_ok") {
                            break;
                        }
                    }
                    Err(_) => break,
                }
            }
            let _ = tx.send(buf);
        });
        let buf = rx
            .recv_timeout(std::time::Duration::from_secs(5))
            .unwrap_or_default();
        let _ = proc.wait();
        let out = String::from_utf8_lossy(&buf);
        assert!(
            out.contains(token),
            "expected PTY output to contain {token:?}, got: {out:?}"
        );
    }
}
