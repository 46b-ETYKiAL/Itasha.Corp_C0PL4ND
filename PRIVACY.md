# Privacy

C0PL4ND is a local-first terminal emulator. This document explains, in plain
language, exactly what data stays on your machine, the one network connection the
app can make, and the controls you have over it. Every statement here is grounded
in the source — the relevant files are named so you can check.

---

## The short version

- **C0PL4ND is telemetry-free by default.** There is no telemetry, no
  analytics, no ads, and no account or login. Nothing about how you use the app
  is collected or transmitted in the background — ever.
- **Crash/issue reporting is opt-in and default-OFF.** C0PL4ND now ships the
  in-house **W1TN3SS** reporting SDK, but **nothing is ever sent without you
  first enabling a stream and consenting to the specific report.** With every
  stream off (the default), no report ever leaves your machine. See
  *Opt-in crash & issue reporting* below.
- **Your shell input and output never leave your device.** Keystrokes, command
  output, and session contents are not transmitted anywhere.
- **The only network connection the app makes on its own is a version check**
  against the public GitHub Releases API. By default it runs once per launch
  (the `notify` mode) to tell you when a newer version exists; it is
  **read-only, sends zero identifiers**, and you can switch it to on-demand
  (`manual`) or off (`off`). See below.

The codebase enforces this structurally: a CI gate (`no-network-gate.yml`) fails
the build if any network call site appears anywhere in C0PL4ND's own source
outside the two opt-in updater modules. The terminal core and UI contain no
network code at all. Crash/issue reporting carries no network code of its own
either — its transport lives entirely in the W1TN3SS SDK dependency and runs
**only** after you enable a stream and consent to a report (see below).

---

## The update check

C0PL4ND checks whether a newer release exists. This is the **only outbound
network feature that runs on its own** (crash/issue reporting, below, only runs
when you opt in), and it is designed to be minimal and privacy-respecting:

- The default update mode is **`notify`** (`crates/core/src/config/mod.rs`,
  `UpdateMode::Notify`). At most once per launch — throttled to one check per
  `check_interval_hours` (default 24h), recorded in a small `last-update-check`
  timestamp file next to your config — the app reads the public GitHub Releases
  API and shows a passive toast if a newer version exists. The check is
  **read-only and sends zero identifiers** (see "What the update check sends"
  below); it never downloads or installs anything on its own.
- For a fully local-first, **no-network-on-launch** experience, set **`manual`**
  (`UpdateMode::Manual`) — the app then contacts the network only when *you*
  press **"Check for updates"** in Settings (or run the update command).
- You can turn it off entirely with the **`off`** mode (`UpdateMode::Off`):
  C0PL4ND then never checks and never touches the network for updates.
- The **`auto`** mode additionally downloads and applies a cryptographically
  verified update when one is found.

### What the update check sends

When a check runs, the app makes a single unauthenticated `GET` request to the
public GitHub Releases API:

```
GET https://api.github.com/repos/{owner}/{repo}/releases/latest
```

It sends **no identifiers** — no account, no install ID, no machine fingerprint,
no OS fingerprint, no usage data. The only header it sets that identifies the
caller is a generic `User-Agent` of the form `c0pl4nd-updater/<version>`
(`crates/app/src/update_engine/net.rs`). If an update is found and downloaded,
the request fetches the release archive and its `.sha256` / `.minisig` sidecars
over `https://` only — again with no identifiers attached. The download is
verified (SHA-256 then minisign signature) before anything runs; see
[SECURITY.md](SECURITY.md) for the verification details.

GitHub may, like any web service, observe the IP address that makes the request.
That is inherent to making an HTTPS request to GitHub at all; C0PL4ND adds
nothing to it.

---

## Opt-in crash & issue reporting (W1TN3SS)

C0PL4ND integrates the in-house **W1TN3SS** reporting SDK
(`crates/app/src/reporting.rs`, `panic_hook.rs`, `issue_intake.rs`). It exists
so that, *if you choose to*, you can help fix crashes and bugs. It is **opt-in
and default-OFF**, and the design follows one rule: **nothing is transmitted
without your explicit consent.**

- **Telemetry-free by default; opt-in reporting available.** Every reporting
  stream defaults to **OFF** (`ReportingMode::Off`). With the defaults, C0PL4ND
  transmits **no reports of any kind**. An upgrade never silently turns
  reporting on.
- **Consent-gated, per report.** A report is only ever sent after you enable a
  stream *and* agree in the consent dialog (or you previously chose "Always" for
  that stream). At the type level, no consent token means no send.
- **Previewable and editable.** Before anything leaves your machine, the consent
  dialog shows you the exact text that would be sent, and you can edit or cancel
  it. Reports are captured locally first and spooled — capture transmits
  nothing.
- **Two independent consent streams**, each separately opt-in:
  1. a **crash-report** stream — a scrubbed, stack-only text backtrace. C0PL4ND
     captures only a panic's `&'static str` message plus its `file:line` site; a
     free-form `String` payload that could embed a path or environment fragment
     is deliberately suppressed at capture, and the SDK's sanitizer normalizes
     your home directory to `<HOME>`. Because a backtrace can still be tied back
     to a single report, this stream is honestly **pseudonymous, not
     anonymous**, under GDPR — we label it that way on purpose; and
  2. a **manual "Report an issue"** stream — covered below.
- **No persistent identifier.** Reports carry no account, install ID, or machine
  fingerprint — only an ephemeral per-report nonce on the consent token.
- **Self-hosted ingest only — no third parties.** A crash report is delivered by
  a hardened HTTPS transport (no redirects, bounded timeout) to a **single,
  config-injected self-hosted** W1TN3SS endpoint, and only when one is
  configured. There is no default endpoint, and no data is ever sent to any
  third-party crash or analytics vendor.
- **Optional sender-anonymous Tor transport (opt-in, default-OFF).** By default a
  configured report is sent over clearnet HTTPS as described above — your IP is
  visible to your own self-hosted ingest endpoint. If you additionally configure
  a v3 `.onion` ingest address in the `C0PL4ND_REPORT_ONION` environment
  variable, a consented send is instead routed over an embedded pure-Rust Tor
  client (Arti) to that hidden service, so the ingest server never learns your IP
  (sender anonymity). This is strictly opt-in: with `C0PL4ND_REPORT_ONION` unset
  (the default) nothing changes and the clearnet path is used. A malformed onion
  value can never silently downgrade you to clearnet under a false sense of
  anonymity — it falls back to the explicit clearnet path. The Tor option changes
  only the *network path*; it does not alter the consent gate, the default-OFF
  posture, or what data a report contains.
- **Manual "Report an issue" stays manual.** The issue-intake helper only
  pre-fills a GitHub issue when *you* invoke it; it never sends anything in the
  background.

In short: C0PL4ND is **telemetry-free by default**, and offers **opt-in,
default-OFF** crash/issue reporting that never sends without your consent.

---

## Data-at-rest inventory: what is written to disk

**This table is the source of truth for everything C0PL4ND writes to disk.** It
enumerates *every* file the app creates, grounded in the write call sites in the
source (`std::fs::write`, `atomic_write*`, `File::create`, and eframe's
persistence store). C0PL4ND writes very little, and **nothing that contains your
shell session contents** — no scrollback, no command output, no keystrokes.

### What IS written

| Data | Path (Windows) | Path (Linux) | Format | Contents | Sensitivity | Protection |
| --- | --- | --- | --- | --- | --- | --- |
| **Settings** | `%APPDATA%\c0pl4nd\config.toml` | `~/.config/c0pl4nd/config.toml` (or `$XDG_CONFIG_HOME/c0pl4nd/config.toml`) | TOML | Font, theme, update mode, keybindings, window geometry, transparency/effects toggles. No shell content. | Low — reflects your preferences and last window position. | Owner-only: `0600` on Unix; on Windows inheritance is stripped to grant only your account (best-effort `icacls`). See `restrict_to_owner` → `crates/core/src/fs_perms.rs`; written by `Config::save_to` in `crates/core/src/config/mod.rs`. |
| **Window/UI state** (eframe persistence) | `%APPDATA%\com.itashacorp.c0pl4nd\data\app.ron` | `~/.local/share/com.itashacorp.c0pl4nd/app.ron` | RON | **Native window geometry only** (position + size, via eframe `persist_window`). **egui in-memory UI state is NOT persisted** — `App::persist_egui_memory()` returns `false` (`crates/app/src/egui_app/mod.rs`), so widget undo stacks / typed-text fragments (find overlay, command palette, settings search) stay in memory and never reach this file. | Low — window geometry only. | Stored in your per-user application-data directory under the stable app-id `com.itashacorp.c0pl4nd`. A "Clear saved window/UI state" control in **Settings → Privacy** deletes this file (`clear_saved_ui_state` in `crates/app/src/egui_app/settings.rs`). |
| **Updater high-water record** | `<install-dir>\.c0pl4nd-installed-version` (next to `c0pl4nd.exe`) | `<install-dir>/.c0pl4nd-installed-version` (next to the binary) | plain text (one line) | A single semver string: the highest version ever installed (anti-rollback floor). **No identifiers, no PII** — just a version number. | None. | Written next to the executable, only ever advanced upward (monotonic). See `INSTALLED_VERSION_FILE` / `record_installed` in `crates/app/src/update_engine/rollback_guard.rs`. |
| **Saved workspace layouts** (opt-in) | `%APPDATA%\c0pl4nd\workspaces\<name>.layout.json` | `~/.config/c0pl4nd/workspaces/<name>.layout.json` | JSON | Pane/tab tree geometry plus **each pane's working directory (cwd)** and shell profile, so a restored workspace relaunches where you left it. Created only when you use "Save Layout / Save Workspace"; absent otherwise. | Medium — a saved cwd can reveal a project/path on your machine (no command content, no scrollback). | Owner-only via `atomic_write_owner_only` (`0600` on Unix; owner-only ACL on Windows). See `save_workspace` / `workspaces_dir` in `crates/app/src/window.rs` and `crates/core/src/layout_persist.rs`. |
| **Update download** (transient) | a per-run temporary staging directory | a per-run temporary staging directory (`0700` on Unix) | binary archive | The verified release archive being installed. | None — deleted after apply. | Created fresh per download (`tempfile`), removed once the update is applied. See `crates/app/src/update_engine/net.rs`. |

> **Note on screenshots:** the `c0pl4nd screenshot` path writes a PNG to a
> path **you supply explicitly** on the command line. It is a user-directed
> output, not background data-at-rest — C0PL4ND never writes a screenshot on
> its own. (`crates/app/src/screenshot.rs`.)

### What is NOT written

- **Command history is memory-only.** The command history used by the command
  palette is bounded to 200 entries (`DEFAULT_CAP` in
  `crates/core/src/command_history.rs`) and is **never persisted to disk** — it
  lives only in memory for the duration of the session and is gone when the app
  closes.
- **Scrollback is not persisted.** Terminal scrollback exists only in memory
  during the session.
- **Your shell's bytes are never logged.** Diagnostic logging uses `tracing`
  configured to write only to **stderr** (`crates/app/src/egui_main.rs`,
  `crates/app/src/main.rs`); there is **no file log appender**, and PTY
  input/output is not routed into the logs.
- **egui UI memory is not persisted.** Widget undo stacks and in-progress text
  in the find overlay, command palette, and settings search live only in memory.
  `App::persist_egui_memory()` returns `false`, so none of that typed text is
  written to `app.ron` — only native window geometry is.

> **Keeping this inventory honest.** The table above is meant to list *every*
> write target. If you are auditing or contributing, you can re-derive the set
> of write sinks with:
>
> ```
> grep -rn "fs::write\|atomic_write\|File::create" crates/*/src
> ```
>
> Every hit should map to a row in the inventory table — the config writer, the
> workspace writer, the high-water record, and the updater archive read-back in
> `net.rs` — or to the user-directed screenshot note above it. A new write
> target that maps to none of these means the inventory needs updating.

---

## Your controls

- **Disable the update check.** Set the update mode to **`off`** (Settings →
  Updates, or `mode = "off"` in `config.toml`). C0PL4ND will then make no network
  connection at all. To keep update checks but make **no on-launch network
  connection** (check only when you press the button), set **`manual`** instead
  of the default `notify`.
- **Clearing / disabling command history.** Command history is already
  memory-only and is discarded when the app closes; nothing is written to disk to
  clear. *(In-app controls to clear or disable history during a session are
  surfaced in app settings as those controls ship; see the app's Settings.)*
- **Delete your settings.** Removing `config.toml` (and the eframe persistence
  folder) resets the app to defaults; nothing else about you is stored.
- **Clear saved window/UI state from inside the app.** **Settings → Privacy**
  has a "Clear saved window/UI state" button that deletes `app.ron` for you
  (`clear_saved_ui_state`).

### Removing all data on uninstall

The Windows installer removes the program files but, by design, **does not delete
your per-user settings** — so reinstalling or upgrading never wipes your
preferences. To remove *everything* C0PL4ND has written, delete these paths after
uninstalling. (You can paste a path into the Explorer address bar or a shell.)

**Windows:**

| What | Path |
| --- | --- |
| Settings + saved workspaces | `%APPDATA%\c0pl4nd\` |
| Window/UI state (`app.ron`) | `%APPDATA%\com.itashacorp.c0pl4nd\` |

**Linux:**

| What | Path |
| --- | --- |
| Settings + saved workspaces | `~/.config/c0pl4nd/` (or `$XDG_CONFIG_HOME/c0pl4nd/`) |
| Window/UI state (`app.ron`) | `~/.local/share/com.itashacorp.c0pl4nd/` |

The updater high-water record (`.c0pl4nd-installed-version`) lives next to the
installed binary and is removed when you uninstall the program files. The update
staging directory is transient and already gone. None of these files contain
your shell session contents.

---

## A note on terminal-inherent exposure

A terminal displays whatever the program running inside it prints. That has
privacy implications that are inherent to *any* terminal, not specific to
C0PL4ND:

- **Titles and hyperlinks come from running programs.** OSC title sequences and
  OSC 8 hyperlinks are emitted by the shell or the programs you run, not by
  C0PL4ND. C0PL4ND captures a title only as the in-app tab label and never
  forwards it to the OS window title or back to the shell (see
  [SECURITY.md](SECURITY.md)).
- **Clipboard.** A program inside the terminal can place text on your clipboard
  via OSC 52 — this is a normal terminal feature. C0PL4ND treats the inverse
  (a program *reading* your clipboard via OSC 52) as an exfiltration vector and
  refuses it by default. Clipboard text that C0PL4ND handles in memory is wiped
  (`zeroize`) when it is dropped, so it does not linger in freed memory
  (`crates/core/src/term/osc.rs`).
- **What you type and what programs print is yours to manage.** C0PL4ND keeps it
  on your machine; how you use the shell is up to you.

---

## Questions or concerns

Privacy or security questions can be raised through the repository's
**Security → Advisories** form for anything sensitive, or as a regular GitHub
issue for general questions. See [SECURITY.md](SECURITY.md) for the full security
posture and reporting process.
