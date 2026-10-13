#!/bin/sh
# End to end test of the installer. Writes the install template to a virtual 2 GiB disk with `iyakki-usb`, then:
#   1. boots with the kernel directly: the first boot must format the data partition and mount it,
#   2. boots again: what the first boot wrote must still be there,
#   3. boots through GRUB under legacy BIOS and under UEFI (OVMF): IYAKKI must start and mount the data volume.
#   IYAKKI_USB=target/debug/iyakki-usb QEMU="sudo qemu-system-x86_64" ./qemu-install-test.sh [build-output-dir]
set -eu
OUT="${1:-$(cd "$(dirname "$0")/../.." && pwd)/target/buildroot-libre}"
QEMU="${QEMU:-qemu-system-x86_64}"
IYAKKI_USB="${IYAKKI_USB:-$(cd "$(dirname "$0")/../.." && pwd)/target/debug/iyakki-usb}"
OVMF="${OVMF:-/usr/share/OVMF/OVMF_CODE_4M.fd}"
# a private work directory: root-run QEMU may not open files that someone else created in the shared /tmp
WORK="$(mktemp -d)"; DISK="$WORK/disk.img"; L1="$WORK/boot1.log"; L2="$WORK/boot2.log"; LB="$WORK/bios.log"; LU="$WORK/uefi.log"
trap 'rm -rf "$WORK" 2>/dev/null || true' EXIT
fail() { echo "FAIL: $1"; trap - EXIT; exit 1; }
truncate -s 2G "$DISK"
"$IYAKKI_USB" "$OUT/images/iyakki-install-x86_64.img" --target-file "$DISK" --erase testfile >/dev/null 2>&1 || fail "iyakki-usb could not write the template"
fdisk -l "$DISK" | grep -q "^$DISK.*1 .*\*.*ef" || fail "partition 1 is not the active EFI/FAT partition"
fdisk -l "$DISK" | grep -q "^${DISK}2 .*83" || fail "partition 2 is not the data partition"
direct() { # $1 stage, $2 log
  timeout 150 $QEMU -accel kvm -accel tcg -m 1024 -smp 2 -nographic -no-reboot -kernel "$OUT/images/bzImage" -initrd "$OUT/images/rootfs.cpio.zst" \
    -append "console=ttyS0 iyakki.selftest=$1 panic=-1" -drive "file=$DISK,format=raw,if=virtio" >"$2" 2>&1 || true
}
echo "--- boot 1: first boot formats the data partition"; direct install1 "$L1"
tr -d '\r' < "$L1" | grep -a "first boot, formatted\|IYAKKI-SELFTEST install1" || true
tr -d '\r' < "$L1" | grep -aq "first boot, formatted vda2" || fail "the data partition was not formatted on first boot (log: $L1)"
tr -d '\r' < "$L1" | grep -aq 'IYAKKI-SELFTEST install1 location Disk("/dev/vda2")' || fail "data directory is not on the new partition (log: $L1)"
echo "--- boot 2: the data must still be there"; direct install2 "$L2"
tr -d '\r' < "$L2" | grep -a "IYAKKI-SELFTEST install2" || true
tr -d '\r' < "$L2" | grep -aq 'IYAKKI-SELFTEST install2 marker "iyakki-install-ok"' || fail "data written in boot 1 is gone (log: $L2)"
tr -d '\r' < "$L2" | grep -aq "first boot, formatted" && fail "the partition was formatted again (data would be lost)"
for mode in bios uefi; do
  if [ "$mode" = uefi ]; then FW="-drive if=pflash,format=raw,readonly=on,file=$OVMF"; LOG="$LU"; else FW=""; LOG="$LB"; fi
  echo "--- boot through GRUB: $mode"
  $QEMU -accel kvm -accel tcg -m 1536 -smp 2 -display none -serial file:"$LOG" -no-reboot $FW -drive "file=$DISK,format=raw,if=virtio,snapshot=on" >"$LOG.err" 2>&1 &
  PID=$!
  for i in $(seq 1 100); do tr -d '\r' < "$LOG" 2>/dev/null | grep -aq "supervisor up" && break; sleep 1; done
  kill $PID 2>/dev/null || true; wait $PID 2>/dev/null || true
  tr -d '\r' < "$LOG" | grep -a "data:\|supervisor up" | head -3
  tr -d '\r' < "$LOG" | grep -aq "supervisor up" || { cat "$LOG.err" 2>/dev/null | head -3; fail "$mode: IYAKKI did not start from the installed disk (log: $LOG)"; }
  tr -d '\r' < "$LOG" | grep -aq "persistent volume vda2 mounted" || fail "$mode: the data volume was not mounted"
done
echo PASS
