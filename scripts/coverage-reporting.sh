#!/usr/bin/env bash
# Per-file coverage gate for the W1TN3SS reporting INTEGRATION module
# (crates/app/src/reporting.rs). `cargo llvm-cov --fail-under-lines` gates a
# whole package, not one file, so we emit JSON and assert reporting.rs's LINE
# coverage stays at or above the floor. A regression below it fails the gate
# (exit 1). See docs/coverage-reporting.md for the posture + the documented
# genuinely-uncoverable lines.
#
# Measure the LIB, not the bin. `reporting` is declared by src/lib.rs; the
# `c0pl4nd` bin (src/egui_main.rs) only `use c0pl4nd::reporting`s it. So
# `--bin c0pl4nd` runs the BIN's unit tests, which do not include reporting's own
# `#[cfg(test)] mod tests` — the file reads as barely covered even though its
# tests pass. Measured on the same tree:
#
#     --bin c0pl4nd  ->  12.00% (33/275)     <- wrong target, gate fails
#     --lib          ->  98.45% (1080/1097)  <- reporting's tests actually run
#
# Note the DENOMINATOR, not just the percentage: the bin sees 275 lines of a
# 1097-line file. A per-file gate whose line count silently collapses is
# measuring a fragment of the file and cannot be trusted either way, so the
# assertion below also pins a minimum line count.
set -euo pipefail

FLOOR="${REPORTING_COVERAGE_FLOOR:-95}"
TARGET_SUBSTR="reporting.rs"
JSON="${REPORTING_COVERAGE_JSON:-reporting-cov.json}"

PY="$(command -v python3 || command -v python)"
if [[ -z "${PY}" ]]; then
  echo "coverage-reporting: no python interpreter found on PATH" >&2
  exit 2
fi

echo "coverage-reporting: measuring crates/app/src/${TARGET_SUBSTR} (floor=${FLOOR}%)"
cargo llvm-cov -p c0pl4nd --lib --json --output-path "${JSON}"

"${PY}" - "${JSON}" "${TARGET_SUBSTR}" "${FLOOR}" <<'PYEOF'
import json
import sys

json_path, target_substr, floor = sys.argv[1], sys.argv[2], float(sys.argv[3])
data = json.load(open(json_path))

match = None
for f in data["data"][0]["files"]:
    name = f["filename"].replace("\\", "/")
    if name.endswith("/" + target_substr) or name.endswith(target_substr):
        match = f
        break

if match is None:
    print(f"coverage-reporting: FAIL — {target_substr} not found in coverage report")
    sys.exit(1)

lines = match["summary"]["lines"]
pct = lines["percent"]
covered, count = lines["covered"], lines["count"]
print(
    f"coverage-reporting: {target_substr} line coverage = {pct:.2f}% "
    f"({covered}/{count}); floor = {floor:.2f}%"
)

# Guard the DENOMINATOR before trusting the percentage. If the wrong target is
# measured, llvm-cov still reports a real-looking percentage over a fragment of
# the file: `--bin c0pl4nd` sees 275 lines of this 1097-line file, and a fragment
# that happened to be well covered would sail past the floor while the file's own
# tests never ran. A percentage over the wrong denominator is not a measurement.
MIN_LINES = int(__import__("os").environ.get("REPORTING_COVERAGE_MIN_LINES", "900"))
if count < MIN_LINES:
    print(
        f"coverage-reporting: FAIL — only {count} lines of {target_substr} were "
        f"measured (expected >= {MIN_LINES}). The wrong target is being measured "
        f"(reporting is declared by src/lib.rs, so this needs --lib, not --bin), "
        f"or the file shrank drastically. Refusing to read a percentage over a "
        f"fragment."
    )
    sys.exit(1)

if pct + 1e-9 < floor:
    print(f"coverage-reporting: FAIL — {pct:.2f}% is below the {floor:.2f}% floor")
    sys.exit(1)
print("coverage-reporting: PASS")
PYEOF
