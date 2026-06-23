#!/usr/bin/env bash
# Per-file coverage gate for the W1TN3SS reporting INTEGRATION module
# (crates/app/src/reporting.rs). `cargo llvm-cov --fail-under-lines` gates a
# whole package, not one file, and the app crate also holds the egui shell
# (display-gated, deliberately not covered by this gate). So we run llvm-cov on
# the `c0pl4nd` bin (which declares `mod reporting`), emit JSON, and assert the
# reporting.rs LINE coverage stays at or above the floor. A regression below the
# floor fails the gate (exit 1). See docs/coverage-reporting.md for the posture
# + the documented genuinely-uncoverable lines.
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
cargo llvm-cov -p c0pl4nd --bin c0pl4nd --json --output-path "${JSON}"

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
if pct + 1e-9 < floor:
    print(f"coverage-reporting: FAIL — {pct:.2f}% is below the {floor:.2f}% floor")
    sys.exit(1)
print("coverage-reporting: PASS")
PYEOF
