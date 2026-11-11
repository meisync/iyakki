#!/usr/bin/env bash
# Runs the workspace tests, then runs only the tests that failed again, one at a time, and says which kind of failure each was.
#   dist/release/test-with-rerun.sh OUT_DIR       writes OUT_DIR/test.log, test-rerun.log, test-flaky.txt (names that failed in parallel and passed alone)
# Exit 0: everything passed the first time. Exit 3: some tests failed in parallel and passed alone (races or shared state: each is listed and counts as a
# flaky test; the gate allows no more than the number of names in dist/release/known-flaky.txt). Exit 1: a test also failed alone (a real failure), or the build broke.
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
allowed=$(grep -vc '^\(#\|$\)' "$KNOWN" 2>/dev/null || true); allowed=${allowed:-0}
echo "test-with-rerun: FLAKY (failed in parallel, passed alone; fix the cause):" >&2; printf '  %s\n' "${FAILED[@]}" >&2
if [ "${#FAILED[@]}" -gt "$allowed" ]; then echo "test-with-rerun: ${#FAILED[@]} flaky test(s), only $allowed allowed by $KNOWN" >&2; exit 1; fi
exit 3
