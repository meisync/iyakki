#!/bin/sh
# End to end test of zram and of the on-device installer ("install to this computer"), all in QEMU with image files:
#   A. zram comes up by default (/proc/swaps, comp_algorithm, vm settings) and the kernel options change it,
#   B. a stick (the install template written with iyakki-usb) boots with a second blank virtual disk; `iyakki install`
#      (reached through the self-test boot option iyakki.selftest=installhere) installs onto that disk,
#   C. the new disk boots directly: first boot formats the data volume, the installer's choices became system.conf and took
#      effect (zram size/algorithm, swap file, host name), and a second boot keeps the data,
#   D. the new disk boots through GRUB under legacy BIOS and UEFI (OVMF) and sets zram up.
# Never touches a real drive: every disk is a file under a private work directory.
# Partition table and sector size of the INSTALLED disk (the stick is always the MBR template with 512-byte sectors):
#   TABLE=mbr (default) | gpt | auto       SECTOR=512 (default) | 4096   (4096: a virtio disk with 4096-byte logical sectors;
#   it starts through UEFI only, so the legacy BIOS boot is skipped; auto then picks GPT by itself)
#   sudo env TMPDIR=$HOME/.cache/iyakki-tmp IYAKKI_USB=target/debug/iyakki-usb ./qemu-memory-install-test.sh [build-output-dir]
set -eu
TABLE="${TABLE:-mbr}"; SECTOR="${SECTOR:-512}"
if [ "$TABLE" = gpt ] || { [ "$TABLE" = auto ] && [ "$SECTOR" = 4096 ]; }; then EXPECT=gpt; DATAPART=vda3; else EXPECT=mbr; DATAPART=vda2; fi
OUT="${1:-$(cd "$(dirname "$0")/../.." && pwd)/target/buildroot-libre}"
QEMU="${QEMU:-qemu-system-x86_64}"
IYAKKI_USB="${IYAKKI_USB:-$(cd "$(dirname "$0")/../.." && pwd)/target/debug/iyakki-usb}"
OVMF="${OVMF:-/usr/share/OVMF/OVMF_CODE_4M.fd}"
WORK="${WORK:-$(mktemp -d "${TMPDIR:-/tmp}/iyakki-meminst.XXXXXX")}"
mkdir -p "$WORK"
STICK="$WORK/stick.img"; TARGET="$WORK/target.img"
trap 'rm -rf "$WORK" 2>/dev/null || true' EXIT
fail() { echo "FAIL: $1"; trap - EXIT; echo "(work directory kept: $WORK)"; exit 1; }
want() { tr -d '\r' < "$1" | grep -aq -- "$2" || fail "$3 (log: $1, wanted: $2)"; echo "ok: $3"; }
unwanted() { tr -d '\r' < "$1" | grep -aq -- "$2" && fail "$3 (log: $1, found: $2)"; echo "ok: $3"; return 0; }
# the number after "$2" in log $1 must lie in [$3, $4]
within() {
  v=$(tr -d '\r' < "$1" | sed -n "s/.*$2 \([0-9][0-9]*\).*/\1/p" | head -1)
  [ -n "$v" ] && [ "$v" -ge "$3" ] && [ "$v" -le "$4" ] || fail "$5 (log: $1, found: ${v:-nothing})"
  echo "ok: $5 ($v)"
}
direct() { # $1 log, $2 kernel args, $3.. drives
  log="$1"; args="$2"; shift 2
  timeout 240 $QEMU -accel kvm -accel tcg -m "${MEM:-2048}" -smp 2 -nographic -no-reboot -kernel "$OUT/images/bzImage" -initrd "$OUT/images/rootfs.cpio.zst" \
    -append "console=ttyS0 panic=-1 $args" "$@" >"$log" 2>&1 || true
}

# a virtio disk with the given logical sector size: dev ID FILE SECTOR [drive options]
dev() { echo "-drive file=$2,format=raw,if=none,id=$1${4:+,$4} -device virtio-blk-pci,drive=$1,logical_block_size=$3,physical_block_size=$3"; }
echo "=== A. zram by default and through the boot options (no disk)"
MEM=4096 direct "$WORK/a1.log" "iyakki.selftest=memory"
want "$WORK/a1.log" "swaps /dev/zram0 .*100" "zram0 is swap with priority 100"
want "$WORK/a1.log" "zram algo .*\[zstd\]" "zstd is the compressor"
within "$WORK/a1.log" "zram disksize" 1800000000 2150000000 "4 GiB RAM gives about a 2 GiB zram (50 % of RAM)"
want "$WORK/a1.log" "page-cluster 0" "vm.page-cluster is 0"
want "$WORK/a1.log" "swappiness 100" "vm.swappiness is 100"
MEM=2048 direct "$WORK/a2.log" "iyakki.selftest=memory iyakki.zram=512M,lz4 iyakki.swappiness=60"
want "$WORK/a2.log" "zram disksize 536870912" "iyakki.zram=512M sets the size"
want "$WORK/a2.log" "zram algo .*\[lz4\]" "iyakki.zram=...,lz4 sets the compressor"
want "$WORK/a2.log" "swappiness 60" "iyakki.swappiness=60"
direct "$WORK/a3.log" "iyakki.selftest=memory iyakki.zram=off"
unwanted "$WORK/a3.log" "swaps /dev/zram0" "iyakki.zram=off leaves no zram swap"
MEM=1024 direct "$WORK/a4.log" "iyakki.selftest=memory"
within "$WORK/a4.log" "zram disksize" 850000000 1073741824 "1 GiB RAM gives a zram as big as RAM (100 %)"

echo "=== B. on-device install from a stick onto a blank disk"
truncate -s 2G "$STICK"
"$IYAKKI_USB" "$OUT/images/iyakki-install-x86_64.img" --target-file "$STICK" --erase testfile --table mbr >/dev/null 2>&1 || fail "iyakki-usb could not write the stick"
truncate -s 6G "$TARGET"
direct "$WORK/b.log" "iyakki.selftest=installhere iyakki.testinstall=vdb,zram=2G,swapfile=256M,hostname=qinstall,table=$TABLE" \
  $(dev s0 "$STICK" 512 snapshot=on) $(dev t0 "$TARGET" "$SECTOR")
tr -d '\r' < "$WORK/b.log" | grep -a "Copying from\|Done\.\|IYAKKI-SELFTEST installhere\|iyakki install:" || true
want "$WORK/b.log" "IYAKKI-SELFTEST installhere exit 0" "the installer finished"
if [ "$EXPECT" = gpt ]; then
  SF=$(sfdisk --sector-size "$SECTOR" -d "$TARGET" 2>&1 || true)
  echo "$SF" | grep -q "label: gpt" || fail "the target has no GPT: $SF"
  echo "$SF" | grep -q "type=21686148-6449-6E6F-744E-656564454649" || fail "target partition 1 is not a BIOS boot partition"
  echo "$SF" | grep -q 'type=C12A7328-F81F-11D2-BA4B-00A0C93EC93B.*name="IYAKKI-BOOT"' || fail "target partition 2 is not the EFI System Partition"
  echo "$SF" | grep -q 'type=0FC63DAF-8483-4772-8E79-3D69D8477DE4.*name="IYAKKI-DATA"' || fail "target partition 3 is not the data partition"
  [ "$SECTOR" != 512 ] || ! command -v sgdisk >/dev/null || sgdisk -v "$TARGET" | grep -q "No problems found" || fail "sgdisk -v found problems with the installed GPT"
  echo "ok: the target has a GPT with BIOS boot, EFI System and data partitions (${SECTOR}-byte sectors)"
else
  fdisk -l "$TARGET" | grep -q "^$TARGET.*1 .*\*.*ef" || fail "target partition 1 is not the active EFI/FAT partition"
  fdisk -l "$TARGET" | grep -q "^${TARGET}2 .*83" || fail "target partition 2 is not the data partition"
  echo "ok: the target has the boot and data partitions"
fi
cmp -n 1048576 "$STICK" "$TARGET" >/dev/null 2>&1 && fail "the disk signature should differ from the stick's" || echo "ok: fresh disk signature"

echo "=== C. the installed disk, kernel booted directly: first boot, then a second"
direct "$WORK/c1.log" "iyakki.selftest=install1 iyakki.disk=vda" $(dev t0 "$TARGET" "$SECTOR")
want "$WORK/c1.log" "first boot, formatted $DATAPART" "first boot formatted the data partition"
want "$WORK/c1.log" "installer settings saved" "the installer's choices were saved"
want "$WORK/c1.log" "install1 system.conf zram=2G" "system.conf has zram=2G"
want "$WORK/c1.log" "install1 system.conf hostname=qinstall" "system.conf has the host name"
want "$WORK/c1.log" "install1 hostname qinstall" "the host name is applied"
want "$WORK/c1.log" "install1 zram disksize 2147483648" "zram is 2 GiB from the installer's choice"
want "$WORK/c1.log" "install1 swaps /var/lib/iyakki/swapfile" "the swap file is active"
direct "$WORK/c2.log" "iyakki.selftest=install2" $(dev t0 "$TARGET" "$SECTOR")
want "$WORK/c2.log" 'install2 marker "iyakki-install-ok"' "data written in the first boot is still there"
unwanted "$WORK/c2.log" "first boot, formatted" "the data partition was not formatted again"
want "$WORK/c2.log" "install2 system.conf zram=2G" "settings persisted"
want "$WORK/c2.log" "install2 zram disksize 2147483648" "zram came up from system.conf on the second boot"

echo "=== D. the installed disk through GRUB: BIOS and UEFI"
for mode in bios uefi; do
  if [ "$mode" = bios ] && [ "$SECTOR" = 4096 ]; then echo "(bios skipped: 4096-byte sectors start through UEFI only)"; continue; fi
  if [ "$mode" = uefi ]; then FW="-drive if=pflash,format=raw,readonly=on,file=$OVMF"; else FW=""; fi
  LOG="$WORK/grub-$mode.log"; : > "$LOG"
  $QEMU -accel kvm -accel tcg -m 2048 -smp 2 -display none -serial file:"$LOG" -no-reboot $FW $(dev t0 "$TARGET" "$SECTOR" snapshot=on) >"$LOG.err" 2>&1 &
  PID=$!
  for i in $(seq 1 120); do tr -d '\r' < "$LOG" 2>/dev/null | grep -aq "zram: zram0" && break; sleep 1; done
  kill $PID 2>/dev/null || true; wait $PID 2>/dev/null || true
  tr -d '\r' < "$LOG" | grep -a "zram:\|data:\|supervisor up" | head -6
  want "$LOG" "supervisor up" "$mode: IYAKKI started from the installed disk"
  want "$LOG" "persistent volume $DATAPART mounted" "$mode: the data volume was mounted"
  want "$LOG" "zram: zram0 2.0 GiB" "$mode: zram is 2 GiB (from system.conf)"
done
echo "PASS (installed table: $EXPECT, ${SECTOR}-byte sectors)"
