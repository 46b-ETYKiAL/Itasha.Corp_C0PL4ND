# C0PL4ND launch fixes — result

Branch `fix/launch-flash-foreground` → **PR #271** (base `master`).
Repo: `Itasha.Corp_C0PL4ND`. Two commits (`d6ea1dc`, `f254195`).

## Task 1 — kill the console/terminal flash on launch

Root cause: the GUI binary is `windows_subsystem = "windows"` (owns no console),
but it spawned **console** child processes without `CREATE_NO_WINDOW`, so Windows
allocated a fresh console for each → a terminal window flashed for a few frames.

Spawn sites fixed (all routed through one new shared helper):
- `crates/core/src/reduced_motion.rs` — `reg query` (OS reduce-motion probe; hit
  on the FIRST frame via `reduced_motion()` → this was the actual on-launch flash).
- `crates/core/src/fs_perms.rs` — `icacls` (owner-only ACL tighten on config save).
- `crates/app/src/update_engine/updater.rs` — the self-relaunch spawn (uniformity).

Shared helper: `crates/core/src/win_process.rs` — a `NoConsoleWindow` extension
trait on `std::process::Command` whose `.no_console_window()` applies
`CREATE_NO_WINDOW` (0x0800_0000) on Windows and is a no-op elsewhere. Single
choke point so no future spawn can regress the flash.

Not touched (correctly): the ConPTY shell spawn in `session.rs` (a pseudoconsole,
never a visible window — the whole point of the terminal); `settings.rs`
`explorer`/`open`/`xdg-open` reveal (GUI apps, not console, and out of scope);
the legacy `window.rs` spawns (behind default-off `legacy-winit`, and already
flagged).

## Task 2 — foreground the window on first launch

Root cause: Windows 11 foreground-lock ignores a raw focus request from a
process that doesn't already own the foreground → window opened behind others.

Fix, weakest → strongest:
- `crates/app/src/egui_main.rs` — `ViewportBuilder::with_active(true)`.
- `crates/app/src/egui_app/mod.rs` — on the first rendered frame of a real window,
  send `egui::ViewportCommand::Focus`.
- `crates/app/src/egui_app/win_foreground.rs` (new) — `#[cfg(windows)]`
  AttachThreadInput backstop: attach our input queue to the current foreground
  thread → `ShowWindow(SW_SHOW)` + `BringWindowToTop` + `SetForegroundWindow` →
  detach. Primed with the same main-window HWND `caption_close` already extracts
  from eframe's `CreationContext`.

Discipline: runs EXACTLY once — latched by a new `foreground_done` field and
gated on `live_window` (so headless test harnesses never issue it). Never runs on
later frames (won't steal focus back), never lowers the global foreground-lock
timeout. Follows the audited-unsafe pattern of `caption_close`/`job_object` under
the binary's `#![deny(unsafe_code)]` (each Win32 call in its own `unsafe` block
with a `// SAFETY:` note; needed no new Cargo `windows` features — AttachThreadInput
/ GetCurrentThreadId are under the already-enabled `Win32_System_Threading`).

## Files changed
- `crates/core/src/win_process.rs` (new) + `crates/core/src/lib.rs` (export)
- `crates/core/src/reduced_motion.rs`, `crates/core/src/fs_perms.rs`
- `crates/app/src/update_engine/updater.rs`
- `crates/app/src/egui_main.rs`
- `crates/app/src/egui_app/win_foreground.rs` (new)
- `crates/app/src/egui_app/mod.rs` (module decl, struct field, HWND prime, first-frame raise)

## Verification
- `cargo build --release` — clean.
- `cargo clippy --release -p c0pl4nd-core -p c0pl4nd` — clean under `-D warnings`
  (fixed `multiple_unsafe_ops_per_block` by splitting the three raise calls into
  one-op unsafe blocks).
- `cargo test -p c0pl4nd-core --lib` — 813 passed, 0 failed (incl. new
  `win_process` tests and unchanged `reduced_motion`/`fs_perms`).
- `cargo test -p c0pl4nd --test egui_window_mgmt` — 411 passed, 0 failed
  (the new `foreground_done` field + first-frame logic don't disturb the
  headless harness; `live_window` is false there so the raise never fires).
- Binary smoke: `c0pl4nd.exe --version` → `C0PL4ND 0.4.16`.

## Caveat
The "no console flash" and "window in front" outcomes still need one real
interactive launch on Windows 11 to confirm visually — an automated agent can't
observe the desktop foreground/flash. Both fixes are the standard, documented
Win32 remedies (CREATE_NO_WINDOW; AttachThreadInput + with_active + Focus) and
compile/test clean.
