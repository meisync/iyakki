#!/usr/bin/env bash
# Runs the workspace tests, then runs only the tests that failed again, one at a time, and says which kind of failure each was.
#   dist/release/test-with-rerun.sh OUT_DIR       writes OUT_DIR/test.log, test-rerun.log, test-flaky.txt (names that failed in parallel and passed alone)
# Exit 0: everything passed the first time. Exit 3: some tests failed in parallel and passed alone (races or shared state: each is listed and counts as a
# flaky test; only the names listed in dist/release/known-flaky.txt are tolerated). Exit 1: a test also failed alone (a real failure), or the build broke.
# A retry never turns a failure into a pass silently: the names are printed and kept.
set -uo pipefail
OUT="${1:?usage: test-with-rerun.sh OUT_DIR}"; mkdir -p "$OUT"
cd "$(git rev-parse --show-toplevel)"
KNOWN="dist/release/known-flaky.txt"
: > "$OUT/test-flaky.txt"
cargo test --workspace --locked --no-fail-fast >"$OUT/test.log" 2>&1; rc=$?
if [ "$rc" = 0 ]; then echo "test-with-rerun: all passed in the first run"; exit 0; fi
if grep -q '^error\(\[E[0-9]*\]\)\?: could not compile\|^error: failed to' "$OUT/test.log"; then echo "test-with-rerun: the build failed (see $OUT/test.log)" >&2; exit 1; fi
mapfile -t FAILED < <(grep -E '^test .* \.\.\. FAILED$' "$OUT/test.log" | sed -E 's/^test (.*) \.\.\. FAILED$/\1/' | sort -u)
if [ "${#FAILED[@]}" = 0 ]; then echo "test-with-rerun: cargo test failed but no failed test was found (see $OUT/test.log)" >&2; exit 1; fi
echo "test-with-rerun: ${#FAILED[@]} test(s) failed in parallel; running them again one at a time"
cargo test --workspace --locked --no-fail-fast -- --exact --test-threads=1 "${FAILED[@]}" >"$OUT/test-rerun.log" 2>&1
mapfile -t STILL < <(grep -E '^test .* \.\.\. FAILED$' "$OUT/test-rerun.log" | sed -E 's/^test (.*) \.\.\. FAILED$/\1/' | sort -u)
if [ "${#STILL[@]}" -gt 0 ]; then
  echo "test-with-rerun: REAL failure (also failed alone):" >&2; printf '  %s\n' "${STILL[@]}" >&2; exit 1
fi
# passed alone: a race or shared state. Make sure the rerun really ran them (a name that matches nothing would "pass" by running nothing).
ran=$(grep -E '^test .* \.\.\. ok$' "$OUT/test-rerun.log" | wc -l)
if [ "$ran" -lt "${#FAILED[@]}" ]; then echo "test-with-rerun: only $ran of ${#FAILED[@]} failed tests ran again (see $OUT/test-rerun.log)" >&2; exit 1; fi
printf '%s\n' "${FAILED[@]}" > "$OUT/test-flaky.txt"
echo "test-with-rerun: FLAKY (failed in parallel, passed alone; fix the cause):" >&2; printf '  %s\n' "${FAILED[@]}" >&2
# only tests named in the known list are tolerated; a flaky test that is not listed fails the gate
unknown=()
for t in "${FAILED[@]}"; do grep -qxF "$t" "$KNOWN" 2>/dev/null || unknown+=("$t"); done
if [ "${#unknown[@]}" -gt 0 ]; then echo "test-with-rerun: flaky and NOT in $KNOWN (fix it, or list it with a KNOWN-ISSUES entry):" >&2; printf '  %s\n' "${unknown[@]}" >&2; exit 1; fi
exit 3
