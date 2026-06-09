# Privacy

C0PL4ND is a local-first terminal emulator. This document explains, in plain
language, exactly what data stays on your machine, the one network connection the
app can make, and the controls you have over it. Every statement here is grounded
in the source — the relevant files are named so you can check.

---

## The short version

- **C0PL4ND does not phone home.** There is no telemetry, no analytics, no ads,
  no crash reporting to a server, and no account or login.
- **Your shell input and output never leave your device.** Keystrokes, command
  output, and session contents are not transmitted anywhere.
- **The only network connection the app can make is an opt-in update check** to
  GitHub Releases — and even that is **not** the default behavior (see below).

The codebase enforces this structurally: a CI gate (`no-network-gate.yml`) fails
the build if any network call site appears anywhere outside the two opt-in
updater modules. The terminal core and UI contain no network code at all.

---

## The only network connection: the update check

C0PL4ND can check whether a newer release exists. This is the **sole** outbound
network feature, and it is **opt-in by default in the most conservative sense**:

- The default update mode is **`manual`** (`crates/core/src/config/mod.rs`,
  `UpdateMode::Manual`). In manual mode the app makes **no automatic network
  connection** — it contacts the network only when *you* press **"Check for
  updates"** in Settings (or run the update command).
- You can turn it off entirely with the **`off`** mode (`UpdateMode::Off`):
  C0PL4ND then never checks and never touches the network for updates.
- The `notify` and `auto` modes are explicit opt-ins to an on-launch check;
  neither is the default.

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

## Data-at-rest inventory: what is written to disk

C0PL4ND writes very little to disk, and nothing that contains your shell session
contents.

### What IS written

| Data | Where | Format | Protection |
| --- | --- | --- | --- |
| Settings (font, theme, update mode, etc.) | `~/.config/c0pl4nd/config.toml` (Unix), `%APPDATA%\c0pl4nd\config.toml` (Windows) | TOML | Owner-only: `0600` on Unix; on Windows, the file lives in your per-user profile and inheritance is stripped to grant only your account (best-effort `icacls`). See `restrict_to_owner` in `crates/core/src/config/mod.rs`. |
| Window geometry + egui UI state | the eframe `persistence` store under the app's stable app-id folder (`com.itashacorp.c0pl4nd`) | RON | Stored in your per-user application-data directory. |
| A verified update download (transient) | a per-run, owner-only temporary staging directory (`tempfile`, `0700` on Unix) | binary archive | Created fresh per download and deleted after the update is applied. |

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

---

## Your controls

- **Disable the update check.** Set the update mode to **`off`** (Settings →
  Updates, or `mode = "off"` in `config.toml`). C0PL4ND will then make no network
  connection at all. The default mode (`manual`) already makes no automatic
  connection — it only checks when you press the button.
- **Clearing / disabling command history.** Command history is already
  memory-only and is discarded when the app closes; nothing is written to disk to
  clear. *(In-app controls to clear or disable history during a session are
  surfaced in app settings as those controls ship; see the app's Settings.)*
- **Delete your settings.** Removing `config.toml` (and the eframe persistence
  folder) resets the app to defaults; nothing else about you is stored.

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
