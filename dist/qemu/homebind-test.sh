#!/bin/sh
# /home on the data volume survives a power-off (F-STORE-09), on a real kernel.
#   dist/qemu/homebind-test.sh [build-output-dir]
# Needs: the built image (images/bzImage, rootfs.cpio.zst), qemu-system-x86_64 (KVM recommended). Three guest boots on one virtual disk:
#   1. `iyakki.selftest=persist1`: the disk becomes the IYAKKI data volume (real mke2fs, real mount).
#   2. `iyakki.selftest=homebind`: /proc/mounts must show a bind mount of the data volume's home folder at /home (a /dev/vda mount, not the RAM disk); the stage writes /home/m80-note.txt.
#   3. the same again after a power-off: the note must be read back.
# PASS = the three steps. Not covered: file permissions of existing homes (a Mac mini report), the IDE panel's and voice tools' HOME, a real disk.
set -eu
. "$(dirname "$0")/lib.sh"
check_images
DISK="$WORK/data.img"; truncate -s 256M "$DISK"
dev() { echo "-drive file=$DISK,format=raw,if=virtio"; }
echo "=== 1. the disk becomes the data volume"
boot "$WORK/p1.log" "iyakki.selftest=persist1 iyakki.disk=vda" $(dev)
want "$WORK/p1.log" "IYAKKI-SELFTEST persist1 ok" "the data volume was made"
echo "=== 2. first boot with the data volume: /home is on it"
boot "$WORK/h1.log" "iyakki.selftest=homebind iyakki.disk=vda" $(dev)
tr -d '\r' < "$WORK/h1.log" | grep -a "IYAKKI-SELFTEST homebind" | sed 's/^/  /' || true
want "$WORK/h1.log" "homebind mount /dev/vda" "/home is a mount of the data disk (a bind mount of its home folder)"
want "$WORK/h1.log" "homebind note written" "a note was written into /home"
echo "=== 3. after a power-off the note is still there"
boot "$WORK/h2.log" "iyakki.selftest=homebind iyakki.disk=vda" $(dev)
tr -d '\r' < "$WORK/h2.log" | grep -a "IYAKKI-SELFTEST homebind" | sed 's/^/  /' || true
want "$WORK/h2.log" "homebind mount /dev/vda" "/home is still on the data disk"
want "$WORK/h2.log" "homebind note home-survives-the-reboot" "the note written before the power-off is read back"
echo "PASS (homebind)"
