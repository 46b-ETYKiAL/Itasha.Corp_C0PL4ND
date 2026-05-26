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

Thank you for helping keep C0PL4ND and its users safe.
