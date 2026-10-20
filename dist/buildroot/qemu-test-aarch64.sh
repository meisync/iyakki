#!/bin/sh
# Boot the aarch64 build in QEMU (virt machine, software emulation: slow, give it minutes) with the self-test
# flag and check the result. Boots the kernel directly; qemu-uefi-aarch64-test.sh covers GRUB and UEFI.
#   ./qemu-test-aarch64.sh [build-output-dir]
set -eu
OUT="${1:-$(cd "$(dirname "$0")/../.." && pwd)/target/buildroot-libre-aarch64}"
QEMU="${QEMU:-qemu-system-aarch64}"
LOG="$(mktemp)"
timeout 420 $QEMU -M virt -cpu cortex-a72 -accel tcg -m 1536 -smp 4 -nographic -no-reboot \
  -kernel "$OUT/images/Image" -initrd "$OUT/images/rootfs.cpio.gz" \
  -append "console=ttyAMA0 iyakki.selftest=1 panic=-1" \
  -netdev user,id=n0 -device virtio-net-pci,netdev=n0 >"$LOG" 2>&1 || true
tr -d '\r' < "$LOG" | grep -a "IYAKKI-SELFTEST\|IYAKKI-HWREPORT" || true
for want in "IYAKKI-SELFTEST begin" "IYAKKI-SELFTEST ssh Dropbear v" "IYAKKI-SELFTEST wasm guest-ok" "IYAKKI-SELFTEST end" "IYAKKI-HWREPORT   architecture: aarch64" "iface eth0 Ethernet" "addr eth0 .* 10\\.0\\.2\\."; do
  tr -d '\r' < "$LOG" | grep -aq "$want" || { echo "FAIL: missing '$want' (log: $LOG)"; exit 1; }
done
echo "PASS"
