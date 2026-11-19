#!/usr/bin/env bash
# Flake hunter (owner order 2026-11-18, docs/development/FLAKY-TESTS.md): runs the tests of one crate (or a filter) N times at the same time,
# with every run using many test threads, under artificial CPU load, and lists every test that failed at least once.
#
#   dist/scripts/flake-hunt.sh -p CRATE [-t lib|all] [-n RUNS] [-j PARALLEL] [-T TEST_THREADS] [-l BURNERS] [-f FILTER] [-- extra test-binary args]
#
#   -p CRATE        the package (required), e.g. iyakki-shared
#   -t lib|all      the unit tests only (default) or every test target of the crate
#   -n RUNS         how many runs (default 10)
#   -j PARALLEL     how many runs at the same time (default 4)
#   -T THREADS      --test-threads of every run (default 64, far above the core count: that is the point)
#   -l BURNERS      CPU burner processes (default: the number of cores)
#   -f FILTER       only tests whose name contains FILTER (as cargo test does)
# Output: a table of tests that failed and in how many runs, plus the log folder (HUNT_OUT, default $TMPDIR/flake-hunt-PID); exit 1 when any
# test failed at least once. Runs on Linux and macOS (use it on the build hosts: copy of the worktree, `cargo` on the PATH). The burners
# and test processes are always stopped when the script ends. Never run it on a host that is running the release gate.
set -uo pipefail
PKG=""; TYPE=lib; RUNS=10; PAR=4; THREADS=64; BURN=""; FILTER=""
while getopts "p:t:n:j:T:l:f:" o; do case $o in p) PKG=$OPTARG;; t) TYPE=$OPTARG;; n) RUNS=$OPTARG;; j) PAR=$OPTARG;; T) THREADS=$OPTARG;; l) BURN=$OPTARG;; f) FILTER=$OPTARG;; *) sed -n 2,18p "$0"; exit 2;; esac; done
shift $((OPTIND-1))
[ -z "$PKG" ] && { sed -n 2,18p "$0"; exit 2; }
CORES=$( (nproc 2>/dev/null || sysctl -n hw.ncpu 2>/dev/null || echo 4) | head -1 )
[ -z "$BURN" ] && BURN=$CORES
OUT="${HUNT_OUT:-${TMPDIR:-/tmp}/flake-hunt-$$}"; mkdir -p "$OUT"
BURNERS=()
cleanup() { for p in ${BURNERS[@]+"${BURNERS[@]}"}; do kill "$p" 2>/dev/null; done; for p in $(jobs -p); do kill "$p" 2>/dev/null; done; wait 2>/dev/null; }
trap cleanup EXIT INT TERM
ROOT="$(cd "$(dirname "$0")/../.." && pwd)"; PKGDIR="$ROOT/crates/$PKG"; [ -d "$PKGDIR" ] || { echo "flake-hunt: no crate folder $PKGDIR" >&2; exit 2; }
TARGETS=(--lib); [ "$TYPE" = all ] && TARGETS=(--lib --tests)
echo "flake-hunt: building the test binaries of $PKG ..."
cargo test -p "$PKG" "${TARGETS[@]}" --locked --no-run >"$OUT/build.log" 2>&1 || { echo "flake-hunt: build failed, see $OUT/build.log" >&2; tail -20 "$OUT/build.log" >&2; exit 3; }
BINS=(); while IFS= read -r l; do BINS+=("$l"); done < <(grep -oE 'Executable .*\((.*)\)' "$OUT/build.log" | sed -E 's/.*\((.*)\)/\1/' | sort -u)
for i in "${!BINS[@]}"; do case "${BINS[$i]}" in /*) ;; *) BINS[$i]="$ROOT/${BINS[$i]}";; esac; done
[ "${#BINS[@]}" = 0 ] && { echo "flake-hunt: no test binary found in $OUT/build.log" >&2; exit 3; }
echo "flake-hunt: ${#BINS[@]} test binar(ies), $RUNS runs, $PAR at a time, $THREADS test threads each, $BURN CPU burners"
i=0; while [ "$i" -lt "$BURN" ]; do i=$((i+1)); ( while :; do :; done ) & BURNERS+=("$!"); done
LIMIT="${HUNT_RUN_SECONDS:-1800}"
# cargo sets the [env] of .cargo/config.toml for a test run; a binary started directly needs the same big test-thread stack
export RUST_MIN_STACK="${RUST_MIN_STACK:-33554432}"
one() { # run number, binary index
  local n=$1 b=$2 log="$OUT/run-$1-$2.log"; shift 2
  # cargo runs a test binary with the package folder as its current folder and sets CARGO_MANIFEST_DIR: do the same
  ( cd "$PKGDIR" && CARGO_MANIFEST_DIR="$PKGDIR" perl -e 'alarm shift; exec @ARGV' "$LIMIT" "${BINS[$b]}" --test-threads="$THREADS" ${FILTER:+"$FILTER"} "$@" >"$log" 2>&1 )
  echo $? >"$log.rc"
}
# a batch of PAR runs at a time (portable: bash 3.2 of macOS has no `wait -n`)
n=0
while [ "$n" -lt "$RUNS" ]; do
  k=0
  while [ "$k" -lt "$PAR" ] && [ "$n" -lt "$RUNS" ]; do
    n=$((n+1)); k=$((k+1))
    for b in "${!BINS[@]}"; do one "$n" "$b" "$@" & done
  done
  for p in $(jobs -p); do case " ${BURNERS[*]:-} " in *" $p "*) ;; *) wait "$p";; esac; done
done
# only the test processes ended: now the burners
cleanup; BURNERS=()
grep -hE '^test .* \.\.\. FAILED$' "$OUT"/run-*.log 2>/dev/null | sed -E 's/^test (.*) \.\.\. FAILED$/\1/' | sort | uniq -c | sort -rn >"$OUT/failed.txt"
# a run that died without naming a test (a crash, a timeout) is a failure too
bad=0; for f in "$OUT"/run-*.log.rc; do [ "$(cat "$f")" != 0 ] && bad=$((bad+1)); done
if [ -s "$OUT/failed.txt" ]; then
  echo "flake-hunt: FAILED at least once ($PKG, $RUNS runs):"; sed 's/^/  /' "$OUT/failed.txt"; echo "logs: $OUT"; exit 1
fi
if [ "$bad" -gt 0 ]; then echo "flake-hunt: $bad run(s) ended with a non-zero status but named no failed test (crash or timeout): see $OUT/run-*.log"; exit 1; fi
echo "flake-hunt: no test failed in $RUNS runs of $PKG under load"; rm -rf "$OUT"
