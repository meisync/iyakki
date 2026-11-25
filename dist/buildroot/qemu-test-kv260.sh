#!/bin/sh
# Boot the KV260 build's kernel and initramfs in QEMU's ZynqMP machine (xlnx-zcu102: quad Cortex-A53, Cadence GEM, Arasan SD, DWC3 USB, PS UART)
# with the self-test flag and check the result. QEMU is given the kernel directly: the board's own firmware (U-Boot in QSPI) is NOT emulated,
# so this proves the kernel and the userspace start on the ZynqMP platform, not the board's boot path, DisplayPort, USB hub or real SD slot.
#   ./qemu-test-kv260.sh [build-output-dir] [zcu102-dtb]
# The zcu102 device tree comes from the kernel tree of the build (arch/arm64/boot/dts/xilinx/zynqmp-zcu102-rev1.0.dtb, built with
# `make ARCH=arm64 CROSS_COMPILE=... xilinx/zynqmp-zcu102-rev1.0.dtb` in build/linux-*), or is given as the second argument.
set -eu
OUT="${1:-$(cd "$(dirname "$0")/../.." && pwd)/target/buildroot-kv260-aarch64}"
QEMU="${QEMU:-qemu-system-aarch64}"
DTB="${2:-$(ls "$OUT"/build/linux-*/arch/arm64/boot/dts/xilinx/zynqmp-zcu102-rev1.0.dtb 2>/dev/null | head -n1)}"
[ -f "${DTB:-}" ] || [ "${KV260_QEMU_MACHINE:-}" = virt ] || { echo "FAIL: no zcu102 device tree (give it as the second argument, or set KV260_QEMU_MACHINE=virt)"; exit 1; }
LOG="$(mktemp)"
run_zcu102() {
  timeout "${KV260_QEMU_TIMEOUT:-600}" $QEMU -M xlnx-zcu102 -accel tcg -m 4G -display none -serial mon:stdio -no-reboot \
    -kernel "$OUT/images/Image" -initrd "$OUT/images/rootfs.cpio.gz" -dtb "$DTB" \
    -append "console=ttyPS0,115200 earlycon iyakki.selftest=1 panic=-1" \
    -nic user >"$LOG" 2>&1 || true
}
# The same kernel and userspace on QEMU's generic `virt` machine (PL011 console, virtio network): used when the ZynqMP machine model does not
# start (QEMU 10.2.1 aborts at start-up on xlnx-zcu102 with an efuse object error, before any kernel runs) or when KV260_QEMU_MACHINE=virt.
run_virt() {
  timeout "${KV260_QEMU_TIMEOUT:-600}" $QEMU -M virt -cpu cortex-a53 -accel tcg -m 2048 -smp 4 -display none -serial mon:stdio -no-reboot \
    -kernel "$OUT/images/Image" -initrd "$OUT/images/rootfs.cpio.gz" \
    -append "console=ttyAMA0 iyakki.selftest=1 panic=-1" \
    -netdev user,id=n0 -device virtio-net-pci,netdev=n0 >"$LOG" 2>&1 || true
}
MACHINE=xlnx-zcu102
if [ "${KV260_QEMU_MACHINE:-}" = virt ]; then MACHINE=virt; run_virt
else
  run_zcu102
  if ! tr -d '\r' < "$LOG" | grep -aq "Linux version"; then echo "note: the xlnx-zcu102 machine did not run the kernel ($(tr -d '\r' < "$LOG" | grep -a -m1 -i 'abort\|not an instance\|error' | cut -c1-100)); using the virt machine"; MACHINE=virt; run_virt; fi
fi
tr -d '\r' < "$LOG" | grep -a "Machine model\|IYAKKI-SELFTEST\|IYAKKI-HWREPORT\|Kernel panic" | head -30 || true
for want in "Linux version" "IYAKKI-SELFTEST begin" "IYAKKI-SELFTEST end" "IYAKKI-HWREPORT   architecture: aarch64"; do
  tr -d '\r' < "$LOG" | grep -aq "$want" || { echo "FAIL: missing '$want' (machine $MACHINE, log: $LOG)"; exit 1; }
done
echo "PASS on $MACHINE (log: $LOG)"
