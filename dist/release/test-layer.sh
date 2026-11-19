#!/usr/bin/env bash
# One test layer of the gate: ./run.sh test-core | test-apps   (dist/release/test-layer.sh core|apps [--no-stamps] [--out DIR] [-- extra cargo test args])
#   core  = the crates of the layers base, core, agent and tools;  apps = the crates of the layers apps and ui   (dist/scripts/crate-layers.txt, read by dist/scripts/testlayers.py)
# Order: (1) every workspace crate must have a layer; (2) when a green stamp matches the inputs of the layer the tests are skipped ("SKIP stamp <commit>"),
# unless --no-stamps; (3) the layer's tests run through dist/release/test-with-rerun.sh (failed tests are run again alone); (4) only a COMPLETE PASSING run
# with no extra cargo arguments writes the stamp (target/alpha-check/stamps/<layer>) for the input hash taken BEFORE the run.
# Exit: 0 passed, 3 passed with a known flaky test (no stamp), 10 skipped because of a green stamp, 1 failed or broke, 2 usage.
set -uo pipefail
. "$(dirname "${BASH_SOURCE[0]}")/env.sh"
cd "$ROOT"
LAYER="${1:-}"; [ "$LAYER" = core ] || [ "$LAYER" = apps ] || { sed -n 2,3p "$0" >&2; exit 2; }
shift
STAMPS=1; OUT="$ROOT/target/alpha-check"; EXTRA=()
while [ $# -gt 0 ]; do case "$1" in
  --no-stamps) STAMPS=0 ;; --out) OUT="$2"; shift ;; --) shift; EXTRA=("$@"); break ;;
  *) echo "unknown option $1" >&2; exit 2 ;; esac; shift; done
TL="python3 $ROOT/dist/scripts/testlayers.py"
$TL check || exit 1
if [ "$STAMPS" = 1 ]; then
  if s="$($TL verify "$LAYER" 2>&1)"; then echo "test-layer $LAYER: SKIP $s (inputs unchanged since that green run; --no-stamps forces the tests)"; exit 10; fi
  echo "test-layer $LAYER: no usable stamp ($s)"
fi
BEFORE="$($TL hash "$LAYER" 2>/dev/null || true)"
LOUT="$OUT/layer-$LAYER"; mkdir -p "$LOUT"
n="$($TL crates "$LAYER" | wc -l)"
echo "test-layer $LAYER: testing $n crates:"; $TL crates "$LAYER" | tr '\n' ' '; echo
PKGS="$($TL crates "$LAYER" --p)"
if [ "${#EXTRA[@]}" -gt 0 ]; then
  # a filtered run is a partial run: plain cargo test, no rerun logic, never a stamp
  read -r -a PA <<<"$PKGS"; cargo test "${PA[@]}" --locked "${EXTRA[@]}" 2>&1 | tee "$LOUT/test.log"; rc=${PIPESTATUS[0]}
else
  IYAKKI_TEST_PACKAGES="$PKGS" dist/release/test-with-rerun.sh "$LOUT"; rc=$?
fi
if [ "$rc" = 0 ]; then
  if [ "${#EXTRA[@]}" -gt 0 ]; then echo "test-layer $LAYER: passed, but with extra cargo arguments: partial run, no stamp"
  elif [ -z "$BEFORE" ]; then echo "test-layer $LAYER: passed, no stamp: the layer's files have uncommitted changes (commit first for a stamp)"
  else $TL stamp "$LAYER" --expect "$BEFORE" || echo "test-layer $LAYER: passed, no stamp written"; fi
fi
exit "$rc"
