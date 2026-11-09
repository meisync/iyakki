#!/bin/sh
# SPDX-License-Identifier: MIT OR Apache-2.0
# Real-kernel tests of the UEFI loader (start, previous-version fallback, install row). See the header of qemu_real_test.py.
#   qemu-real-test.sh [--only 1,2,3] [--image LEAN.img] [--prev-image OLDER-LEAN.img] [--scratch DIR]
# --image: a lean or install image whose boot volume carries the loader (read only). --prev-image: an older image whose kernel and initramfs
# become the "previous version" copy. --scratch defaults to ~/iyakki-work/boot-qemu/real (never /tmp). Memory per VM: 2 GB. One VM at a time.
set -eu
HERE=$(cd "$(dirname "$0")" && pwd)
SCRATCH=${BOOT_REAL_SCRATCH:-$HOME/iyakki-work/boot-qemu/real}
IMAGE=""; PREV=""; ONLY=""
while [ $# -gt 0 ]; do
	case $1 in
		--image) IMAGE=$2; shift 2 ;;
		--prev-image) PREV=$2; shift 2 ;;
		--scratch) SCRATCH=$2; shift 2 ;;
		--only) ONLY="--only $2"; shift 2 ;;
		*) echo "usage: $0 [--only 1,2,3] --image LEAN.img --prev-image OLDER.img [--scratch DIR]" >&2; exit 2 ;;
	esac
done
[ -n "$IMAGE" ] && [ -n "$PREV" ] || { echo "qemu-real-test: --image and --prev-image are needed" >&2; exit 2; }
PATH=$PATH:/usr/sbin
export PATH
mkdir -p "$SCRATCH/src"
BASE=$SCRATCH/base.img
[ -s "$BASE" ] && [ "$BASE" -nt "$IMAGE" ] || python3 -I "$HERE/qemu_real_base.py" "$IMAGE" "$BASE" >/dev/null
python3 -I "$HERE/fatdisk.py" get "$PREV" 1 /boot/bzImage "$SCRATCH/src/prev-bzImage"
python3 -I "$HERE/fatdisk.py" get "$PREV" 1 /boot/initrd-lean "$SCRATCH/src/prev-initrd"
# shellcheck disable=SC2086
exec python3 -I "$HERE/qemu_real_test.py" --base "$BASE" --prev-kernel "$SCRATCH/src/prev-bzImage" --prev-initrd "$SCRATCH/src/prev-initrd" \
	--scratch "$SCRATCH" $ONLY
