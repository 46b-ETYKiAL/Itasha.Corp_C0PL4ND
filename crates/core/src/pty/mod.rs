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
        apply_prompt_env(&mut cmd, program);
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

/// The default `PROMPT` we inject for a `cmd.exe`-family shell so there is a
/// single space between the prompt and the cursor.
///
/// `cmd.exe`'s built-in default prompt is `$P$G` — the path followed by `>`
/// with NO trailing space — so the cursor renders flush against the `>`
/// (`C:\Users\.46b_>`). Setting `PROMPT=$P$G ` (note the trailing space) makes
/// cmd render `C:\Users\.46b_> ` with the cursor one cell clear of the `>`.
/// PowerShell and POSIX shells (bash/zsh/fish) already end their default prompt
/// with a space via their own `prompt` function / `PS1`, so they need no env.
#[cfg(windows)]
const CMD_PROMPT_WITH_TRAILING_SPACE: &str = "$P$G ";

/// Whether `program` names a `cmd.exe`-family shell (case-insensitive on the
/// file stem). Matches `cmd`, `cmd.exe`, and an absolute/relative path ending in
/// one of those (e.g. `C:\Windows\System32\cmd.exe`). Does NOT match
/// `powershell.exe`, `pwsh.exe`, `wsl.exe`, or `bash` — those carry their own
/// space-terminated default prompt and must not be touched.
#[cfg(windows)]
fn is_cmd_shell(program: &str) -> bool {
    std::path::Path::new(program)
        .file_stem()
        .and_then(|s| s.to_str())
        .is_some_and(|stem| stem.eq_ignore_ascii_case("cmd"))
}

/// Guarantee the `cmd.exe`-family prompt ends with a trailing space, so the
/// cursor always sits one cell clear of the `>`.
///
/// cmd reads its prompt from the `PROMPT` environment variable and defaults to
/// `$P$G` (no trailing space) when it is unset. The first fix only injected our
/// spaced prompt when `PROMPT` was *unset* — but many environments INHERIT a
/// bare `PROMPT=$P$G` (set by the system, a parent shell, or a prior session),
/// which is not None, so the injection was skipped and the prompt rendered with
/// no space (the reported "cursor directly next to `>`"). We now normalise the
/// effective prompt instead of conditioning on presence:
///   * cmd-family shell only (PowerShell/pwsh/WSL/bash own their space-terminated
///     prompt and are never touched), AND
///   * unset / empty → use `$P$G ` (default with the space), OR
///   * inherited value already ending in whitespace → kept verbatim, OR
///   * inherited value without a trailing space → a single space is appended
///     (preserves a customised prompt's content while guaranteeing the space).
///
/// On non-Windows targets this is a no-op (POSIX shells own their `PS1`).
#[cfg_attr(not(windows), allow(unused_variables))]
fn apply_prompt_env(cmd: &mut CommandBuilder, program: &str) {
    #[cfg(windows)]
    {
        if is_cmd_shell(program) {
            let prompt = match std::env::var_os("PROMPT") {
                Some(v) => {
                    let s = v.to_string_lossy();
                    if s.is_empty() {
                        CMD_PROMPT_WITH_TRAILING_SPACE.to_string()
                    } else if s.ends_with([' ', '\t']) {
                        s.into_owned()
                    } else {
                        format!("{s} ")
                    }
                }
                None => CMD_PROMPT_WITH_TRAILING_SPACE.to_string(),
            };
            cmd.env("PROMPT", prompt);
        }
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

    /// Serializes the `apply_prompt_env_*` tests: they mutate the process-wide
    /// `PROMPT` env var, so they must not run concurrently with each other.
    /// `Mutex<()>` poisons if a test panics while holding it; the helper below
    /// recovers the guard so one assertion failure does not cascade into
    /// `PoisonError` failures in the sibling tests.
    #[cfg(windows)]
    static PROMPT_ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    /// Lock `PROMPT_ENV_LOCK`, ignoring a prior panic's poison (we only use the
    /// lock to serialise, not to protect invariant state).
    #[cfg(windows)]
    fn lock_prompt_env() -> std::sync::MutexGuard<'static, ()> {
        PROMPT_ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// The `PROMPT` value `apply_prompt_env` INJECTED (i.e. set via `env()`),
    /// independent of any inherited base-environment `PROMPT`. `None` means no
    /// injection happened. `CommandBuilder::new` seeds the base env with the
    /// parent's vars, so `get_env` would also report an inherited `PROMPT`;
    /// `iter_extra_env_as_str` reports ONLY caller-set vars, which is exactly
    /// what we want to assert here.
    #[cfg(windows)]
    fn injected_prompt(cmd: &CommandBuilder) -> Option<String> {
        cmd.iter_extra_env_as_str()
            .find(|(k, _)| k.eq_ignore_ascii_case("PROMPT"))
            .map(|(_, v)| v.to_string())
    }

    #[test]
    fn default_shell_is_nonempty() {
        assert!(!default_shell().is_empty());
    }

    /// `is_cmd_shell` matches the cmd-family by file stem (case-insensitive),
    /// including a full path, and rejects the space-terminated-prompt shells.
    #[cfg(windows)]
    #[test]
    fn is_cmd_shell_matches_only_cmd_family() {
        assert!(super::is_cmd_shell("cmd"));
        assert!(super::is_cmd_shell("cmd.exe"));
        assert!(super::is_cmd_shell("CMD.EXE"));
        assert!(super::is_cmd_shell(r"C:\Windows\System32\cmd.exe"));
        // These shells already end their default prompt with a space — never touch.
        assert!(!super::is_cmd_shell("powershell.exe"));
        assert!(!super::is_cmd_shell("pwsh.exe"));
        assert!(!super::is_cmd_shell("wsl.exe"));
        assert!(!super::is_cmd_shell("bash"));
        assert!(!super::is_cmd_shell("command.com"));
    }

    /// The reported bug: a cmd shell spawned with no user `PROMPT` must carry the
    /// injected `PROMPT=$P$G ` (trailing space) so the cursor clears the `>`.
    /// We guard the env var so the assertion is deterministic regardless of how
    /// the test runner was launched.
    #[cfg(windows)]
    #[test]
    fn apply_prompt_env_sets_trailing_space_prompt_for_cmd_when_unset() {
        let _guard = lock_prompt_env();
        let saved = std::env::var_os("PROMPT");
        // SAFETY: serialized by PROMPT_ENV_LOCK; we restore the prior value.
        unsafe { std::env::remove_var("PROMPT") };
        let mut cmd = CommandBuilder::new("cmd.exe");
        super::apply_prompt_env(&mut cmd, "cmd.exe");
        let got = injected_prompt(&cmd);
        // Restore before asserting so a failure cannot leak state.
        if let Some(v) = saved {
            unsafe { std::env::set_var("PROMPT", v) };
        }
        assert_eq!(
            got.as_deref(),
            Some(super::CMD_PROMPT_WITH_TRAILING_SPACE),
            "cmd with no user PROMPT must get PROMPT=$P$G with a trailing space"
        );
        assert!(
            super::CMD_PROMPT_WITH_TRAILING_SPACE.ends_with(' '),
            "the injected prompt MUST end with a space (this is the whole fix)"
        );
    }

    /// An inherited `PROMPT` that already ends in a space is kept verbatim
    /// (content preserved), but one WITHOUT a trailing space gets a single space
    /// appended — the bare inherited `$P$G` (the reported bug) is normalised so
    /// the cursor still sits one cell clear of the `>`.
    #[cfg(windows)]
    #[test]
    fn apply_prompt_env_guarantees_trailing_space_on_inherited_prompt() {
        let _guard = lock_prompt_env();
        let saved = std::env::var_os("PROMPT");

        // Case 1: inherited bare "$P$G" (no trailing space) — the exact reported
        // failure — must be normalised to "$P$G ".
        // SAFETY: serialized by PROMPT_ENV_LOCK; we restore the prior value.
        unsafe { std::env::set_var("PROMPT", "$P$G") };
        let mut cmd1 = CommandBuilder::new("cmd.exe");
        super::apply_prompt_env(&mut cmd1, "cmd.exe");
        let got1 = injected_prompt(&cmd1);

        // Case 2: an inherited prompt that already ends in a space is untouched.
        unsafe { std::env::set_var("PROMPT", "$P$_$G ") };
        let mut cmd2 = CommandBuilder::new("cmd.exe");
        super::apply_prompt_env(&mut cmd2, "cmd.exe");
        let got2 = injected_prompt(&cmd2);

        // Restore before asserting so a failure cannot leak state.
        match saved {
            Some(v) => unsafe { std::env::set_var("PROMPT", v) },
            None => unsafe { std::env::remove_var("PROMPT") },
        }
        assert_eq!(
            got1.as_deref(),
            Some("$P$G "),
            "inherited bare $P$G must gain a trailing space (the whole fix)"
        );
        assert_eq!(
            got2.as_deref(),
            Some("$P$_$G "),
            "an inherited prompt already ending in a space is preserved verbatim"
        );
    }

    /// PowerShell / WSL must NOT receive a `PROMPT` injection — they own their
    /// own space-terminated prompt.
    #[cfg(windows)]
    #[test]
    fn apply_prompt_env_does_not_touch_non_cmd_shells() {
        let _guard = lock_prompt_env();
        let saved = std::env::var_os("PROMPT");
        // SAFETY: serialized by PROMPT_ENV_LOCK; we restore the prior value.
        unsafe { std::env::remove_var("PROMPT") };
        let injected: Vec<_> = ["powershell.exe", "pwsh.exe", "wsl.exe"]
            .into_iter()
            .map(|prog| {
                let mut cmd = CommandBuilder::new(prog);
                super::apply_prompt_env(&mut cmd, prog);
                (prog, injected_prompt(&cmd))
            })
            .collect();
        if let Some(v) = saved {
            unsafe { std::env::set_var("PROMPT", v) };
        }
        for (prog, got) in injected {
            assert_eq!(got, None, "{prog} must not get a PROMPT injection");
        }
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
