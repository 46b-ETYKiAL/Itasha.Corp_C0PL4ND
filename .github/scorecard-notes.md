# OpenSSF Scorecard — remediation notes

This file documents how each OpenSSF Scorecard check is addressed for
`Itasha.Corp_C0PL4ND`, including the findings that are resolved in code, the
ones that resolve automatically, and the ones that require a manual external
action.

> Scorecard findings are produced by the scheduled `scorecard.yml` workflow and
> uploaded to GitHub code scanning. **Alerts do not close instantly** — they
> auto-close on the next scheduled Scorecard scan after the fix lands on the
> default branch (typically within ~1 week, or sooner if the workflow is
> dispatched manually).

## Findings addressed in code

### `Vulnerabilities` (RUSTSEC-2017-0008)

- **What**: The unmaintained [`serial`](https://rustsec.org/advisories/RUSTSEC-2017-0008.html)
  crate was present in the dependency tree.
- **Root cause**: It was pulled in transitively by `portable-pty 0.8.1`
  (Windows serial-port handling).
- **Fix**: Upgrade `portable-pty` to `0.9.0`, which replaced the unmaintained
  `serial` crate with the maintained `serial2`. This is a **real upstream fix**,
  not an ignore — `serial` is fully removed from `Cargo.lock`. The
  `[advisories] ignore = ["RUSTSEC-2017-0008"]` entry in `deny.toml` was
  consequently removed; `cargo audit` and `cargo deny check` are clean with no
  ignores. (See dependency PR bumping `portable-pty`.)

### `Fuzzing`

- **What**: Scorecard reports no fuzzing integration.
- **Fix**: Added a `cargo-fuzz` crate (`fuzz/`) with a `vt_parser` target on the
  VT/ANSI/OSC escape-sequence parser — the highest-value untrusted-input
  surface for a terminal emulator. Wired `.github/workflows/fuzz.yml`
  (build-on-change + nightly campaign + manual dispatch) and a deterministic
  adversarial-seed regression test in the stable `cargo test` suite. OSS-Fuzz
  onboarding is documented in `SECURITY.md`.

### `Code-Review` / `Branch-Protection`

- **What**: Scorecard rewards enforced PR review on the default branch.
- **Fix**: Branch protection is configured on `main` requiring **1 approving
  review**, with **admin/owner bypass enabled** (this is a solo-maintainer
  project; enforce-on-admins is intentionally left off so the maintainer is not
  locked out). See "Branch protection state" below for the exact applied
  settings. If the GitHub API rejects the configuration (e.g. on a plan that
  does not support branch protection for the repo visibility), the fallback is
  to rely on the required status checks already enforced by the `CI Gate`
  aggregating job.

## Findings that resolve automatically

### `Maintained`

- **What**: An activity heuristic — Scorecard scores a project higher when it
  has had commits / released activity within the trailing 90 days.
- **Action**: **None required.** This project is under active development; the
  ongoing commit and PR activity (including this remediation) satisfies the
  heuristic. The score updates on the next scheduled scan. No code change can
  or should be made for this check.

## Findings deferred to an external manual action

### `CII-Best-Practices` (OpenSSF Best Practices badge)

- **What**: Scorecard awards points when the project holds an
  [OpenSSF Best Practices](https://www.bestpractices.dev/) badge (formerly the
  CII Best Practices badge).
- **Why deferred**: Obtaining the badge requires a **manual, interactive
  self-certification** on an external site that cannot be automated from code or
  CI. The maintainer must sign in and complete the questionnaire.
- **Action (manual, maintainer)**:
  1. Go to <https://www.bestpractices.dev/> and sign in with GitHub.
  2. Add a new project: <https://www.bestpractices.dev/en/projects/new>
  3. Enter the repository URL: `https://github.com/46b-ETYKiAL/Itasha.Corp_C0PL4ND`
  4. Complete the "passing" criteria (most are already satisfied: OSS license,
     `SECURITY.md`, public VCS, automated test suite, static analysis via
     CodeQL + clippy, dependency vetting via `cargo deny`/`cargo audit`).
  5. Once awarded, add the badge markdown to `README.md`.
- **Status**: **Deferred-external.** No in-repo change closes this check; it is
  tracked here so the requirement is not lost.
