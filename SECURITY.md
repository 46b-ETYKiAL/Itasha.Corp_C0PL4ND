# Security Policy

## Our local-first posture

C0PL4ND is **local-first by design**. This is a core product principle, not an afterthought:

- **No account.** There is no login wall. You never create or sign into an account to use the terminal.
- **No telemetry by default.** C0PL4ND ships with telemetry **off**. We do not collect usage analytics out of the box.
- **No egress of your shell I/O.** Your shell input and output **never leave your device**. C0PL4ND does not transmit keystrokes, command output, or session contents to any server.
- **No required cloud.** The terminal is fully functional offline. There is no mandatory cloud sync and no server-side component that your sessions depend on.
- **No coupling of features to analytics or accounts.** Core functionality is never gated behind enabling tracking or signing in.

If a future optional feature ever involves a network call, it will be **off by default**, **clearly disclosed**, and **never required** for normal terminal use.

---

## Supported versions

Security fixes are provided for the latest released minor version. We recommend always running the most recent release.

| Version | Supported |
| --- | --- |
| Latest release | ✅ |
| Previous minor | ⚠️ Critical fixes only |
| Older | ❌ |

---

## Reporting a vulnerability

**Please do not report security vulnerabilities through public GitHub issues, discussions, or pull requests.**

Instead, report privately using one of:

1. **GitHub Security Advisories** — use the repository's **Security → Report a vulnerability** ("Report a vulnerability" / private advisory) button. This is the preferred channel.
2. **Email** — send details to `security@c0pl4nd.dev` (PGP key available on request).

Please include, as far as you can:

- A description of the issue and its potential impact.
- Steps to reproduce, or a proof-of-concept.
- Affected version(s) and platform(s) (Windows / Linux / macOS).
- Any suggested mitigation.

### What to expect

- **Acknowledgement** within a few business days.
- A good-faith effort to validate, triage, and develop a fix.
- Coordinated disclosure: we'll work with you on timing and credit you in the advisory (unless you prefer to remain anonymous).

We ask that you give us a reasonable opportunity to release a fix before any public disclosure. We will not pursue or support legal action against researchers who report in good faith and avoid privacy violations, data destruction, and service disruption.

---

## Scope

In scope:

- Memory-safety issues, crashes, or undefined behavior in the terminal core.
- Escape-sequence handling that can lead to code execution, file access, or data exfiltration.
- Any path by which terminal contents or input could leave the device unexpectedly.
- Privilege-escalation or sandbox-escape vectors introduced by C0PL4ND.

Out of scope:

- Vulnerabilities in third-party shells, programs, or operating-system components run inside the terminal.
- Social-engineering or physical-access attacks.

---

## Fuzzing

The highest-value untrusted-input surface in a terminal emulator is the
**VT / ANSI / OSC escape-sequence parser**: it consumes arbitrary bytes
produced by any program running inside the terminal. C0PL4ND continuously
fuzzes that parser.

- **Harness**: [`cargo-fuzz`](https://github.com/rust-fuzz/cargo-fuzz) (libFuzzer back end). The fuzz crate lives in [`fuzz/`](fuzz/).
- **Target**: `vt_parser` ([`fuzz/fuzz_targets/vt_parser.rs`](fuzz/fuzz_targets/vt_parser.rs)) drives `c0pl4nd_core::Terminal::advance` — the same entry point the live PTY reader uses for shell output — then reads back the public surface to catch any state inconsistency.
- **CI**: [`.github/workflows/fuzz.yml`](.github/workflows/fuzz.yml) builds the target on every change to the parser path (regression guard) and runs a short time-boxed campaign on a nightly schedule and on demand.
- **Deterministic regression**: a seed corpus of malformed/hostile sequences runs in the normal `cargo test` suite on every platform (`parser_survives_adversarial_escape_sequences` in `crates/core/src/term.rs`), so a crash regression is caught even without the nightly fuzz toolchain.

Run a campaign locally (requires a nightly toolchain):

```sh
cargo install cargo-fuzz
cargo +nightly fuzz run vt_parser
```

### OSS-Fuzz

We intend to onboard C0PL4ND to [OSS-Fuzz](https://github.com/google/oss-fuzz)
for free continuous distributed fuzzing of the parser target. The `vt_parser`
target is structured to be OSS-Fuzz-compatible (single `fuzz_target!` over a
`&[u8]`). Until acceptance, the scheduled CI campaign above provides ongoing
coverage.

---

Thank you for helping keep C0PL4ND and its users safe.
