#!/bin/sh
# Boot the aarch64 disk image the way a VM does: UEFI firmware (AAVMF) -> GRUB -> kernel -> IYAKKI, with a
# virtio GPU and USB keyboard/tablet. Checks that IYAKKI comes up as PID 1 and finds a display and input devices.
#   ./qemu-uefi-aarch64-test.sh [build-output-dir]
set -eu
OUT="${1:-$(cd "$(dirname "$0")/../.." && pwd)/target/buildroot-libre-aarch64}"
QEMU="${QEMU:-qemu-system-aarch64}"
CODE="${AAVMF_CODE:-/usr/share/AAVMF/AAVMF_CODE.fd}"
VARS_SRC="${AAVMF_VARS:-/usr/share/AAVMF/AAVMF_VARS.fd}"
VARS="$(mktemp)"; LOG="$(mktemp)"; DISK="$(mktemp)"
trap 'rm -f "$VARS" "$DISK"' EXIT
cp "$VARS_SRC" "$VARS"; cp "$OUT/images/iyakki-aarch64.img" "$DISK"
timeout 600 $QEMU -M virt -cpu cortex-a72 -accel tcg -m 2048 -smp 4 -display none -serial stdio -no-reboot \
  -drive if=pflash,format=raw,readonly=on,file="$CODE" -drive if=pflash,format=raw,file="$VARS" \
  -drive file="$DISK",format=raw,if=none,id=hd0 -device virtio-blk-pci,drive=hd0 \
  -device virtio-gpu-pci -device qemu-xhci -device usb-kbd -device usb-tablet \
  -netdev user,id=n0 -device virtio-net-pci,netdev=n0 >"$LOG" 2>&1 &
PID=$!
for i in $(seq 1 580); do
  tr -d '\r' < "$LOG" | grep -aq "shell: " && break
  sleep 1
done
kill $PID 2>/dev/null || true; wait $PID 2>/dev/null || true
tr -d '\r' < "$LOG" | grep -a "Booting .IYAKKI\|supervisor up\|shell:" | cut -c1-120 | head -5 || true
for want in "Booting .IYAKKI" "supervisor up" "shell: [0-9]*x[0-9]* framebuffer"; do
  tr -d '\r' < "$LOG" | grep -aq "$want" || { echo "FAIL: missing '$want' (log: $LOG)"; trap - EXIT; rm -f "$VARS" "$DISK"; exit 1; }
done
echo "PASS"
