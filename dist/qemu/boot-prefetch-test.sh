#!/bin/sh
# Boot-order recording and prefetch on a real kernel (F-BOOT-30).
#   dist/qemu/boot-prefetch-test.sh [build-output-dir]
# Needs: the built image, qemu-system-x86_64 (KVM recommended; without KVM raise TIMEOUT to 600), python3.
# The guest (iyakki.selftest=bootlist iyakki.bootrecord=1) lets the fanotify recorder watch the first 90 seconds of the boot, then checks: the list
# file was written, it holds at least 20 boot files and none under /proc /sys /dev /run /tmp, a prefetch of that list runs to the end ("ended
# Done"), and with only 100 MiB reported free it stops at once ("MemoryShort", 0 batches). The run takes about two minutes.
# PASS = RESULT PASS with every check PASS. Not covered: that the boot option iyakki.prefetch=on starts it by itself on a second boot, and whether
# prefetching makes a boot faster (that needs the real machine).
set -eu
. "$(dirname "$0")/lib.sh"
check_images; need python3
echo "=== guest: iyakki.selftest=bootlist iyakki.bootrecord=1 (about two minutes)"
TIMEOUT="${TIMEOUT:-300}" boot "$WORK/list.log" "iyakki.selftest=bootlist iyakki.bootrecord=1" -netdev user,id=n0 -device virtio-net-pci,netdev=n0
tr -d '\r' < "$WORK/list.log" | grep -a "IYAKKI-SELFTEST bootlist\|boot order:\|boot prefetch:" | sed 's/^/  /' || true
result "$WORK/list.log" bootlist
echo "PASS (bootlist)"
