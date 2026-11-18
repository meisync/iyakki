#!/bin/sh
# Image content gate (F-QA-05): scan every finished image file against legal/image-rules.txt, the package licences and the asset list.
#   dist/release/image-gate.sh [FILE...]        default: the images of target/buildroot-*/images (rootfs.cpio, *.cpio.zst, *.iso, *.vfat)
# A .zst file is unpacked by the build host's own `zstd -dc` into the scanner's standard input (nothing is written to disk); ISO 9660
# (Joliet names), FAT12/16/32 volume images and newc cpio files are read by iyakki-relcheck itself. Exit 1 when any image fails;
# exit 0 with a loud line when there is nothing to scan (so a CI job on a machine without images does not fail, and does not pass silently).
set -u
ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
cd "$ROOT" || exit 2
CHK="${IYAKKI_RELCHECK:-$ROOT/target/debug/iyakki-relcheck}"
[ -x "$CHK" ] || { cargo build -q --locked -p iyakki-relcheck || exit 2; }
if [ "$#" -eq 0 ]; then
  set --
  for f in target/buildroot-*/images/rootfs.cpio target/buildroot-*/images/*.cpio.zst target/buildroot-*/images/*.iso target/buildroot-*/images/*.vfat; do
    [ -f "$f" ] && set -- "$@" "$f"
  done
fi
if [ "$#" -eq 0 ]; then echo "image-gate: SKIPPED, no image found (the scan runs where the images are built)"; exit 0; fi
bad=0
for f in "$@"; do
  echo "== $f"
  # the Buildroot build folder the image came from (images/ is inside it) holds the package file list and the licence manifest; without it every
  # file of a rootfs read from standard input would be "unattributed" (the default only knows target/buildroot-mac and target/buildroot-web)
  BASE="$(cd "$(dirname "$f")/.." 2>/dev/null && pwd)"; BASEARG=""; [ -f "$BASE/build/packages-file-list.txt" ] && BASEARG="--base $BASE"
  case "$f" in
    *.zst)
      command -v zstd >/dev/null 2>&1 || { echo "image-gate: FAIL $f needs zstd on this machine"; bad=1; continue; }
      zstd -dc -- "$f" | "$CHK" image-scan "$ROOT" --cpio - $BASEARG || bad=1 ;;
    *) "$CHK" image-scan "$ROOT" --file "$f" $BASEARG || bad=1 ;;
  esac
done
# the builder's home folder must not be inside a root file system image (found 2026-11-13: build paths in programs and text files; board/iyakki/scrub-paths.sh and the
# rustc path remapping take it out). Checked on the build machine's own HOME, byte for byte, in every rootfs given.
HOMEPAT="${HOME:-}"
if [ "${#HOMEPAT}" -ge 5 ]; then
  for f in "$@"; do
    case "$f" in
      *.zst) n="$(zstd -dc -- "$f" 2>/dev/null | LC_ALL=C grep -a -c -F -- "$HOMEPAT")" ;;
      *rootfs.cpio) n="$(LC_ALL=C grep -a -c -F -- "$HOMEPAT" "$f")" ;;
      *) continue ;;
    esac
    [ "${n:-0}" -eq 0 ] || { echo "image-gate: FAIL $f holds the build machine's home folder ($n line(s) match $HOMEPAT)"; bad=1; }
  done
fi
[ "$bad" -eq 0 ] && echo "image-gate: PASS ($# image(s))" || echo "image-gate: FAIL"
exit "$bad"
