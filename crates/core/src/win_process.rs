//! Shared child-process spawn helper: suppress the transient console-window
//! flash on Windows.
//!
//! The C0PL4ND GUI binary is built with `windows_subsystem = "windows"`, so it
//! owns no console of its own. But when a GUI-subsystem process spawns a
//! **console** child (`reg`, `icacls`, `powershell`, the updater relaunch, …)
//! *without* an explicit creation flag, Windows allocates a brand-new console
//! for that child — which flashes on screen for a few frames before the child
//! exits. That is the "a terminal window briefly pops up on launch" bug.
//!
//! Applying `CREATE_NO_WINDOW` tells the OS to run the child with **no console
//! window at all**. It is preferred over `DETACHED_PROCESS` because the child's
//! `stdout`/`stderr` stay capturable (we still read `reg`/clipboard output).
//!
//! This helper is the shared way to suppress that flash: route a
//! **console-child** `std::process::Command` through
//! [`NoConsoleWindow::no_console_window`]. The method is a no-op off Windows
//! (there is no console to suppress), so callers use it unconditionally without
//! any `#[cfg]` of their own. It is not universal, and does not need to be: a
//! few call sites already set their own `CREATE_NO_WINDOW` inline via
//! `CommandExt::creation_flags`, and a spawn of another **GUI-subsystem**
//! program (which owns no console) needs no suppression at all.

use std::process::Command;

/// `CREATE_NO_WINDOW` (winbase.h): run the child process with no console window.
/// The child still inherits/keeps capturable `stdout`/`stderr` handles (unlike
/// `DETACHED_PROCESS`).
#[cfg(windows)]
pub const CREATE_NO_WINDOW: u32 = 0x0800_0000;

/// Apply the no-console-window creation flag to a [`Command`] so a
/// GUI-subsystem process never flashes a console for a console child.
///
/// A **no-op on non-Windows platforms** (nothing to suppress), so callers can
/// invoke [`no_console_window`](NoConsoleWindow::no_console_window)
/// unconditionally in cross-platform spawn code and keep the builder chain.
pub trait NoConsoleWindow {
    /// Suppress the transient console window Windows would otherwise allocate
    /// for a console child process. Returns `&mut Self` for chaining. No-op off
    /// Windows.
    fn no_console_window(&mut self) -> &mut Self;
}

impl NoConsoleWindow for Command {
    #[cfg(windows)]
    fn no_console_window(&mut self) -> &mut Self {
        use std::os::windows::process::CommandExt;
        self.creation_flags(CREATE_NO_WINDOW)
    }

    #[cfg(not(windows))]
    fn no_console_window(&mut self) -> &mut Self {
        // No console exists to suppress on non-Windows targets.
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The extension method must be chainable in the builder style AND leave the
    /// command runnable (i.e. it only sets a creation flag, never breaks spawn).
    /// A trivial, always-present OS command exercises the whole path on every
    /// platform so the no-op (non-Windows) and flag-setting (Windows) branches
    /// are both covered by the standard test run.
    #[test]
    fn no_console_window_is_chainable_and_still_spawns() {
        #[cfg(windows)]
        let status = {
            let mut cmd = Command::new("cmd");
            cmd.args(["/C", "exit", "0"]);
            cmd.no_console_window().status().expect("spawn cmd")
        };
        #[cfg(not(windows))]
        let status = Command::new("true")
            .no_console_window()
            .status()
            .expect("spawn true");
        assert!(
            status.success(),
            "the wrapped command must still run cleanly"
        );
    }

    /// The flag constant is exactly `CREATE_NO_WINDOW` (0x0800_0000) — a wrong
    /// value would silently re-introduce the console flash, so pin it.
    #[cfg(windows)]
    #[test]
    fn create_no_window_constant_is_correct() {
        assert_eq!(CREATE_NO_WINDOW, 0x0800_0000);
    }
}
