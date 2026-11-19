#!/bin/sh
# OUTLINE, NOT RUN YET: the boot side of hot patches (docs/development/REMOTE-PATCH.md) through the real chain, on a PRIVATE COPY of a lean
# image (never the original, never a drive). It plants a patch layer on the data partition with debugfs, boots the lean entry with
# qemu-lean-test.py and checks over SSH that the patch is in the root, then does the same with a corrupt layer.txt and with boot-count=3.
#
#   dist/buildroot/qemu-patch-test.sh IMAGE.img SSH_PRIVATE_KEY
#
# Expected: case A prints "patched"; case B (layer.txt damaged) boots without the patch and `skipped` on the data volume names it;
# case C (iyakki/patches/boot-count = 3) boots without the patch and writes `disabled`.
set -eu
IMG=${1:?image}; KEY=${2:?ssh key}
HERE=$(dirname "$0")
W=$(mktemp -d)
trap 'rm -rf "$W"' EXIT
START=$(sfdisk -d "$IMG" | sed -n 's/.*start= *\([0-9]*\),.*/\1/p' | tail -1)
plant() { # plant COPY LAYER_TXT_CONTENT BOOT_COUNT
  printf 'p1\n' > "$W/active"
  printf '%s\n' "$2" > "$W/layer.txt"
  printf 'patched\n' > "$W/marker"
  {
    echo "mkdir iyakki/patches"; echo "mkdir iyakki/patches/p1"; echo "mkdir iyakki/patches/p1/layer"; echo "mkdir iyakki/patches/p1/layer/etc"
    echo "write $W/active iyakki/patches/active"
    echo "write $W/layer.txt iyakki/patches/p1/layer.txt"
    echo "write $W/marker iyakki/patches/p1/layer/etc/iyakki-patch-test"
    [ -n "${3:-}" ] && { printf '%s\n' "$3" > "$W/count"; echo "write $W/count iyakki/patches/boot-count"; }
  } | debugfs -w -f - "$1?offset=$((START * 512))" >/dev/null
}
GOOD='iyakki-patch-layer 1
id=p1
base=*
file 8 /etc/iyakki-patch-test'
run() { python3 "$HERE/qemu-lean-test.py" "$1" --entry 2 --ssh-key "$KEY" --keep-writes --keep-to "$W/after-$2.img" --cmd 'cat /etc/iyakki-patch-test 2>&1; ls /var/lib/iyakki/iyakki/patches' ; }
for c in A B C; do cp "$IMG" "$W/$c.img"; done
plant "$W/A.img" "$GOOD"
plant "$W/B.img" "garbage"
plant "$W/C.img" "$GOOD" 3
run "$W/A.img" A; run "$W/B.img" B; run "$W/C.img" C
