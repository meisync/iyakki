#!/usr/bin/env bash
# The whole green gate on one commit, in one command:   ./run.sh release-green [--commit REV] [--image] [--out DIR]
#   1. a worktree of REV (default: the checkout you run it in, as it is: nothing is merged here)
#   2. ./run.sh test-core --no-stamps, then ./run.sh test-apps --no-stamps (the contract tests of the gate live in the core layer: boundary,
#      dependency_ratchet, ui_agent_uses, feature_matrix, kernel_single_reader, layer_split, release_version, and the iyakki-core-api tests)
#   3. a test that failed in parallel is run again alone by test-with-rerun.sh: it is listed as a FLAKE when it passes alone and as FAIL when not
#   4. with --image: after a green run, the stage1 build and dist/buildroot/nightly-push.sh (signed lean image, then its QEMU boot test inside the
#      script); --image-dry-run prints the commands without running them; missing inputs are a FAIL, never a pass
#   5. OUT/RESULT.txt (PASS or FAIL, commit, failing and flaky test names, durations, `./run.sh release-check --status`) and OUT/DONE
# One run at a time (a lock under ~/iyakki-work); cargo goes through the build-host wrapper when IYAKKI_CARGO_SHIM names its folder.
# Read only RESULT.txt: the logs are in OUT/ for after a failure.
set -uo pipefail
HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO="$(cd "$HERE/../.." && pwd)"
REV=""; IMAGE=0; OUT=""
while [ $# -gt 0 ]; do case "$1" in
  --commit) REV="$2"; shift ;; --image) IMAGE=1 ;; --image-dry-run) IMAGE=dry ;; --out) OUT="$2"; shift ;;
  -h|--help) sed -n 2,13p "$0"; exit 0 ;; *) echo "unknown option $1" >&2; exit 2 ;; esac; shift; done
LOCK="${IYAKKI_GREEN_LOCK:-$HOME/iyakki-work/.release-green.lock}"
exec 9>"$LOCK" || exit 2
flock -n 9 || { echo "release-green: another run holds $LOCK" >&2; exit 3; }
WT="$REPO"; MADE=0
if [ -n "$REV" ]; then
  WT="$HOME/iyakki-work/green-wt-$(git -C "$REPO" rev-parse --short "$REV")"
  [ -d "$WT" ] || { git -C "$REPO" worktree add --detach "$WT" "$REV" >/dev/null 2>&1 || { echo "release-green: no worktree for $REV" >&2; exit 2; }; MADE=1; }
fi
COMMIT="$(git -C "$WT" rev-parse HEAD)"
OUT="${OUT:-$HOME/iyakki-work/green/run-$(date +%Y%m%d-%H%M%S)-${COMMIT:0:8}}"
mkdir -p "$OUT"; rm -f "$OUT/DONE"
[ -n "${IYAKKI_CARGO_SHIM:-}" ] && export PATH="$IYAKKI_CARGO_SHIM:$PATH" IYAKKI_RCARGO_WT="$WT"
cd "$WT" || exit 2
DIRTY="$(git status --porcelain | head -3)"
FAILS=(); FLAKES=(); NOTES=(); T0=$SECONDS; DUR=""
layer() { # core|apps
  local t=$SECONDS rc
  ./run.sh "test-$1" --no-stamps >"$OUT/$1.log" 2>&1; rc=$?
  DUR="$DUR$1 $((SECONDS - t))s; "
  if grep -q '^test-with-rerun: REAL failure' "$OUT/$1.log"; then
    while read -r n; do FAILS+=("$1: $n"); done < <(awk '/^test-with-rerun: REAL failure/ {f=1; next} f && /^  / {sub(/^  /, ""); print; next} {f=0}' "$OUT/$1.log")
  fi
  [ -s "target/alpha-check/layer-$1/test-flaky.txt" ] && while read -r n; do FLAKES+=("$1: $n"); done < "target/alpha-check/layer-$1/test-flaky.txt"
  if [ "$rc" != 0 ] && [ "$rc" != 3 ] && ! grep -q 'REAL failure' "$OUT/$1.log"; then
    if grep -q 'flaky and NOT in' "$OUT/$1.log"; then NOTES+=("$1: the layer failed only because of the flakes listed above (a flake not in known-flaky.txt fails the gate)")
    else FAILS+=("$1: the layer did not run to the end (exit $rc, see $OUT/$1.log)"); fi
  fi
  return 0
}
if [ "${IYAKKI_GREEN_SKIP_TESTS:-}" = 1 ]; then NOTES+=("the test layers were SKIPPED (IYAKKI_GREEN_SKIP_TESTS=1: only for trying the --image hook)"); else
  layer core
  layer apps
fi
VERDICT=PASS; [ "${#FAILS[@]}" -gt 0 ] && VERDICT=FAIL
[ "${#FLAKES[@]}" -gt 0 ] && [ "$VERDICT" = PASS ] && VERDICT="PASS-WITH-FLAKES (not green: flakes must be fixed)"
if [ "$IMAGE" != 0 ]; then
  # the signed lean-image build and its QEMU boot test (dist/buildroot/nightly-push.sh boots the image itself and refuses to pass without
  # `root boot/root.sfs mounted` and `switching root`). It needs the machine's trees and keys, named by variables (nothing personal in the repository):
  #   IYAKKI_MAIN  the checkout that holds target/buildroot-src, target/buildroot-web and target/buildroot-mac
  #   IYAKKI_SB_KEY / IYAKKI_SB_CERT  (default ~/.config/iyakki/secureboot/iyakki-sb.key and .crt)
  # `--image-dry-run` only prints the commands (and checks that every input exists); it is how the hook is tested.
  VER="${IYAKKI_IMAGE_VERSION:-$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -1)-green-${COMMIT:0:8}}"
  KEY="${IYAKKI_SB_KEY:-$HOME/.config/iyakki/secureboot/iyakki-sb.key}"; CRT="${IYAKKI_SB_CERT:-$HOME/.config/iyakki/secureboot/iyakki-sb.crt}"
  MAIN="${IYAKKI_MAIN:-}"; PACK="$OUT/pack"
  STAGE1=(cargo build --release --target x86_64-unknown-linux-musl -p iyakki-stage1)
  PUSH=(env WEB_IRIS=1 "IYAKKI_SB_KEY=$KEY" "IYAKKI_SB_CERT=$CRT" "BR_DIR=$MAIN/target/buildroot-src/buildroot-2026.02.3" dist/buildroot/nightly-push.sh -o "$PACK" --version "$VER" -- --base "$MAIN/target/buildroot-web" --kernel-from "$MAIN/target/buildroot-mac")
  MISSING=()
  [ -n "$MAIN" ] || MISSING+=("IYAKKI_MAIN is not set")
  for f in "$KEY" "$CRT" "$MAIN/target/buildroot-web" "$MAIN/target/buildroot-mac" "$MAIN/target/buildroot-src/buildroot-2026.02.3" dist/buildroot/nightly-push.sh; do [ -e "$f" ] || MISSING+=("missing: $f"); done
  if [ "$VERDICT" != PASS ]; then NOTES+=("image: not built, the tests are not green")
  elif [ "${#MISSING[@]}" -gt 0 ]; then VERDICT=FAIL; FAILS+=("image: inputs missing: ${MISSING[*]}")
  elif [ "$IMAGE" = dry ]; then NOTES+=("image (dry run, nothing executed): ${STAGE1[*]} ; ${PUSH[*]}")
  else
    if "${STAGE1[@]}" >"$OUT/stage1.log" 2>&1 && "${PUSH[@]}" >"$OUT/nightly-push.log" 2>&1; then NOTES+=("image: nightly-push ok and the lean image booted in QEMU: pack in $PACK, version $VER")
    else VERDICT=FAIL; FAILS+=("image: stage1 build or nightly-push failed (see $OUT/stage1.log, $OUT/nightly-push.log)"); fi
  fi
fi
{
  echo "RESULT: $VERDICT"
  echo "commit: $COMMIT${DIRTY:+  (the tree has uncommitted changes: no stamp can be written)}"
  echo "durations: ${DUR}total $((SECONDS - T0))s"
  echo "failing tests (also failed alone):"; if [ "${#FAILS[@]}" = 0 ]; then echo "  none"; else printf '  %s\n' "${FAILS[@]}"; fi
  echo "load flakes (failed in parallel, passed alone):"; if [ "${#FLAKES[@]}" = 0 ]; then echo "  none"; else printf '  %s\n' "${FLAKES[@]}"; fi
  [ "${#NOTES[@]}" -gt 0 ] && { echo "notes:"; printf '  %s\n' "${NOTES[@]}"; }
  echo "stamps (./run.sh release-check --status):"; ./run.sh release-check --status 2>&1 | sed 's/^/  /'
} >"$OUT/RESULT.txt"
echo "$OUT" > "$HOME/iyakki-work/green/LAST-RUN"
touch "$OUT/DONE"
[ "$MADE" = 1 ] && true
[ "$VERDICT" = PASS ]
