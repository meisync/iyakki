#!/bin/sh
# The experimental compressed-memory plan on a real kernel (F-BOOT-31).
#   sudo env IYAKKI_USB=target/debug/iyakki-usb dist/qemu/compressed-memory-test.sh [build-output-dir]
# Needs: the built image and its install stick image (images/iyakki-install-x86_64.img), target/debug/iyakki-usb (cargo build -p iyakki-usb),
# qemu-system-x86_64 (KVM recommended), a few GB of space in $TMPDIR. Every disk is a file; no real drive is touched.
# Boots: A. no disk and iyakki.zram=exp -> zram swap, zswap N, no swap file ("no data disk, zram only");
#        B. no disk, setting off (control) -> zram swap, zswap N, no swap file;
#        C. an installed disk (first boot formats it) with iyakki.zram=exp -> zswap Y, /var/lib/iyakki/swapfile in /proc/swaps and 536870912
#           bytes (a quarter of 2 GiB), no zram swap;
#        D. the same disk, setting off -> zram swap, zswap N, no swap file.
# PASS = every "ok:" line and the last line PASS. The settings page and the chat tools write the same system.conf key; this checks the boot side.
set -eu
. "$(dirname "$0")/lib.sh"
check_images
IYAKKI_USB="${IYAKKI_USB:-$ROOT/target/debug/iyakki-usb}"
[ -x "$IYAKKI_USB" ] && [ -f "$OUT/images/iyakki-install-x86_64.img" ] || { echo "needs iyakki-usb ($IYAKKI_USB) and $OUT/images/iyakki-install-x86_64.img" >&2; exit 2; }
dev() { echo "-drive file=$2,format=raw,if=none,id=$1${3:+,$3} -device virtio-blk-pci,drive=$1"; }
echo "=== A and B. no data disk"
MEM=2048 boot "$WORK/a.log" "iyakki.selftest=memory iyakki.zram=exp"
want "$WORK/a.log" "memory swaps /dev/zram0" "A: zram is the swap (no data disk)"
want "$WORK/a.log" "memory zswap enabled N" "A: zswap is off"
want "$WORK/a.log" "memory swapfile none" "A: no swap file"
MEM=2048 boot "$WORK/b.log" "iyakki.selftest=memory"
want "$WORK/b.log" "memory swaps /dev/zram0" "B: zram is the swap by default"
want "$WORK/b.log" "memory zswap enabled N" "B: zswap is off by default"
want "$WORK/b.log" "memory swapfile none" "B: no swap file by default"
echo "=== an installed disk"
STICK="$WORK/stick.img"; TARGET="$WORK/target.img"
truncate -s 2G "$STICK"; truncate -s 6G "$TARGET"
"$IYAKKI_USB" "$OUT/images/iyakki-install-x86_64.img" --target-file "$STICK" --erase testfile --table mbr >/dev/null 2>&1 || fail "iyakki-usb could not write the stick"
boot "$WORK/inst.log" "iyakki.selftest=installhere iyakki.testinstall=vdb,hostname=qcompressed,table=mbr" $(dev s0 "$STICK" snapshot=on) $(dev t0 "$TARGET")
want "$WORK/inst.log" "IYAKKI-SELFTEST installhere exit 0" "the installer finished"
boot "$WORK/first.log" "iyakki.selftest=install1 iyakki.disk=vda" $(dev t0 "$TARGET")
want "$WORK/first.log" "first boot, formatted" "first boot formatted the data partition"
echo "=== C. the disk with iyakki.zram=exp"
boot "$WORK/c.log" "iyakki.selftest=install2 iyakki.zram=exp" $(dev t0 "$TARGET")
want "$WORK/c.log" "install2 zswap enabled Y" "C: zswap is on"
want "$WORK/c.log" "install2 swaps /var/lib/iyakki/swapfile" "C: the swap file is in use"
want "$WORK/c.log" "install2 swapfile 536870912 bytes" "C: it is a quarter of 2 GiB"
unwanted "$WORK/c.log" "install2 swaps /dev/zram0" "C: no zram swap"
echo "=== D. the same disk, setting off"
boot "$WORK/d.log" "iyakki.selftest=install2" $(dev t0 "$TARGET")
want "$WORK/d.log" "install2 swaps /dev/zram0" "D: zram is the swap"
want "$WORK/d.log" "install2 zswap enabled N" "D: zswap is off"
# run C made the file on this same disk and turning the setting off does not delete it: what matters is that it is not in use
unwanted "$WORK/d.log" "install2 swaps /var/lib/iyakki/swapfile" "D: the swap file C left behind is not in use"
echo "PASS (compressed memory)"
