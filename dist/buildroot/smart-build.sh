#!/usr/bin/env bash
# Build an IYAKKI image the cheapest way:   smart-build.sh <variant> [--force-full] [--clean] [--dry-run]
# Asks changed.sh what changed since the last recorded build of the variant, then
#   NOTHING      -> reuses the images (prints where they are)
#   IYAKKI-ONLY  -> repack.sh --cargo (minutes)
#   PACKAGES/KERNEL/FULL (nothing to reuse or an incremental change) -> build.sh (Buildroot rebuilds only what its stamps say;
#                   the listed packages are dirclean-ed first)
#   FULL because of a toolchain / removed-package change -> stops and says so; add --clean to delete the output dir and rebuild
# --force-full: skip the decision and run build.sh.  --dry-run: print the decision and the commands, run nothing.
# Same environment as build.sh (ARCH, WEB_H264, WEB_AV_ADDON, WEB_MEDIA, WEB_IRIS, WEB_CROCUS, WITH_FFMPEG, JOBS, IYAKKI_PROFILE). docs/BUILD-INCREMENTAL.md
set -eu
HERE="$(cd "$(dirname "$0")" && pwd)"; ROOT="$(cd "$HERE/../.." && pwd)"
V=""; FORCE=0; CLEAN=0; DRY=0
for a in "$@"; do case "$a" in --force-full) FORCE=1 ;; --clean) CLEAN=1 ;; --dry-run) DRY=1 ;; -*) echo "unknown option $a" >&2; exit 2 ;; *) V="$a" ;; esac; done
[ -n "$V" ] || { sed -n '2,9p' "$0" >&2; exit 2; }
ARCH="${ARCH:-x86_64}"; if [ "$ARCH" = aarch64 ]; then OUT="$ROOT/target/buildroot-$V-aarch64"; else OUT="$ROOT/target/buildroot-$V"; fi
BR_DIR="${BR_DIR:-$ROOT/target/buildroot-src/buildroot-${BR_VERSION:-2026.02.3}}"
run() { echo "+ $*"; [ "$DRY" = 1 ] || "$@"; }
if [ "$FORCE" = 1 ]; then DEC="FULL: --force-full"; else DEC="$(bash "$HERE/changed.sh" "$V" 2>&1)"; fi
echo "$DEC"; KIND="$(echo "$DEC" | head -n1)"
case "$KIND" in
  NOTHING) echo "smart-build: nothing changed; reusing $OUT/images" ;;
  IYAKKI-ONLY)
    [ "$ARCH" = x86_64 ] || { run "$HERE/build.sh" "$V"; exit $?; }
    ARGS=(); [ "$DRY" = 1 ] && ARGS+=(--dry-run)
    bash "$HERE/repack.sh" "$V" --cargo "${ARGS[@]}"
    [ "$DRY" = 1 ] || bash "$HERE/changed.sh" "$V" --record ;;
  *)
    case "$DEC" in
      *"toolchain, architecture"*|*"switched off"*|*"external.mk"*)
        if [ "$CLEAN" = 1 ]; then run rm -rf "$OUT"; else
          echo "smart-build: Buildroot cannot apply this change to the existing tree; run again with --clean (deletes $OUT, a full rebuild)" >&2; exit 3; fi ;;
    esac
    if [ "$KIND" = "${KIND#PACKAGES}" ] || [ "$KIND" = FULL ]; then :; else
      for p in ${KIND#PACKAGES}; do
        ls -d "$OUT/build/$p-"[0-9]* >/dev/null 2>&1 && run make -C "$BR_DIR" O="$OUT" "$p-dirclean"
      done
    fi
    run "$HERE/build.sh" "$V"
    [ "$DRY" = 1 ] || bash "$HERE/changed.sh" "$V" --record ;;
esac
