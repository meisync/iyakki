#!/bin/sh
# Two boots with one virtual disk: the first turns the disk into the IYAKKI data volume (and carries a
# marker file over from the RAM session); the second must find the volume, mount it over the data
# directory and see the marker. Proves persistence end to end, with the real kernel, mke2fs and mount.
#   ./qemu-persist-test.sh [build-output-dir]
set -eu
OUT="${1:-$(cd "$(dirname "$0")/../.." && pwd)/target/buildroot-libre}"
QEMU="${QEMU:-qemu-system-x86_64}"
IMG="$(mktemp)"; L1="$(mktemp)"; L2="$(mktemp)"
trap 'rm -f "$IMG" "$L1" "$L2"' EXIT
truncate -s 128M "$IMG"
boot() { # $1 = selftest stage, $2 = log
  timeout 120 $QEMU -accel kvm -accel tcg -m 1024 -smp 2 -nographic -no-reboot \
    -kernel "$OUT/images/bzImage" -initrd "$OUT/images/rootfs.cpio.zst" \
    -append "console=ttyS0 iyakki.selftest=$1 iyakki.disk=vda panic=-1" \
    -drive "file=$IMG,format=raw,if=virtio" >"$2" 2>&1 || true
  tr -d '\r' < "$2" | grep -a "IYAKKI-SELFTEST\|^vda\|^data:" || true
}
echo "--- boot 1: create the data disk"; boot persist1 "$L1"
tr -d '\r' < "$L1" | grep -aq "IYAKKI-SELFTEST persist1 location-before Memory" || { echo "FAIL: first boot was not RAM-only (log: $L1)"; trap - EXIT; exit 1; }
tr -d '\r' < "$L1" | grep -aq "IYAKKI-SELFTEST persist1 ok" || { echo "FAIL: persist failed (log: $L1)"; trap - EXIT; exit 1; }
echo "--- boot 2: the data must still be there"; boot persist2 "$L2"
tr -d '\r' < "$L2" | grep -aq 'IYAKKI-SELFTEST persist2 location Disk("/dev/vda")' || { echo "FAIL: data dir not on the disk (log: $L2)"; trap - EXIT; exit 1; }
tr -d '\r' < "$L2" | grep -aq 'IYAKKI-SELFTEST persist2 marker "iyakki-persist-ok"' || { echo "FAIL: marker file lost (log: $L2)"; trap - EXIT; exit 1; }
echo PASS
