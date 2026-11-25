#!/bin/sh
# Write the KV260 SD-card image to a card, with the same care the installer takes (F-KV260-03).
#   dist/buildroot/kv260-write-sd.sh IMAGE DEVICE      (run as root; DEVICE like /dev/sdX or /dev/mmcblkN, the whole card, not a partition)
# Refuses: anything that is not a whole block device, a device that is mounted or holds the running system, loop/ram/dm/md/mtd/nvme
# devices, a card larger than 256 GB (KV260_ALLOW_LARGE=1 lifts that one), an image that is not a KV260 SD image. Asks you to type the device
# name; writes with fsync; reads the written bytes back and compares them; then stretches the data partition to the end of the card.
# KV260_WRITE_TEST=1 (tests only) lets DEVICE be an ordinary file and takes the confirmation from KV260_CONFIRM.
set -eu
die() { echo "kv260-write-sd: $*" >&2; exit 1; }
IMG="${1:-}"; DEV="${2:-}"
[ -n "$IMG" ] && [ -n "$DEV" ] || die "usage: $0 IMAGE DEVICE"
[ -f "$IMG" ] || die "$IMG is not a file"
IMGSIZE="$(stat -c %s "$IMG")"
# the image must be ours: an MBR with a bootable FAT32 first partition (type 0x0C) and the data partition (0x83)
sig="$(dd if="$IMG" bs=1 skip=510 count=2 2>/dev/null | od -An -tx1 | tr -d ' \n')"
[ "$sig" = "55aa" ] || die "$IMG has no MBR signature: not a KV260 SD image"
p1="$(dd if="$IMG" bs=1 skip=$((446 + 4)) count=1 2>/dev/null | od -An -tx1 | tr -d ' \n')"
p1b="$(dd if="$IMG" bs=1 skip=446 count=1 2>/dev/null | od -An -tx1 | tr -d ' \n')"
p2="$(dd if="$IMG" bs=1 skip=$((462 + 4)) count=1 2>/dev/null | od -An -tx1 | tr -d ' \n')"
[ "$p1" = "0c" ] && [ "$p1b" = "80" ] && [ "$p2" = "83" ] || die "$IMG is not a KV260 SD image (partition 1 must be bootable FAT32 0x0c, partition 2 Linux 0x83)"
NAME="$(basename "$DEV")"
if [ "${KV260_WRITE_TEST:-0}" = 1 ]; then
  [ -f "$DEV" ] || die "test mode: $DEV must be an ordinary file"
  SIZE="$(stat -c %s "$DEV")"
else
  case "$NAME" in
    loop*|ram*|zram*|dm-*|md*|mtd*|mtdblock*|nvme*|sr*|fd*) die "$DEV: refused (that kind of device is never written)" ;;
    sd[a-z]|mmcblk[0-9]|mmcblk[0-9][0-9]) ;;
    *) die "$DEV: give the whole card (sdX or mmcblkN), not a partition or another kind of device" ;;
  esac
  [ -b "$DEV" ] || die "$DEV is not a block device"
  [ "$(id -u)" = 0 ] || die "run as root"
  # mounted or holding the running system?
  if grep -E "^/dev/${NAME}p?[0-9]* " /proc/mounts >/dev/null 2>&1; then die "$DEV (or a partition of it) is mounted: unmount it first"; fi
  ROOTSRC="$(findmnt -n -o SOURCE / 2>/dev/null || true)"
  case "$ROOTSRC" in /dev/"$NAME"*) die "$DEV holds the running system" ;; esac
  SIZE="$(blockdev --getsize64 "$DEV")"
  [ "$(cat /sys/block/"$NAME"/size 2>/dev/null || echo 0)" != 0 ] || die "$DEV has no medium"
  [ "$SIZE" -le $((256 * 1024 * 1024 * 1024)) ] || [ "${KV260_ALLOW_LARGE:-0}" = 1 ] || die "$DEV is $((SIZE / 1000000000)) GB: larger than a card; set KV260_ALLOW_LARGE=1 if you are sure"
fi
[ "$SIZE" -ge $((IMGSIZE + 1048576)) ] || die "$DEV ($SIZE bytes) is smaller than the image ($IMGSIZE bytes)"
echo "Image:  $IMG ($((IMGSIZE / 1048576)) MiB)"
echo "Target: $DEV ($((SIZE / 1000000)) MB): EVERYTHING ON IT WILL BE ERASED"
if [ "${KV260_WRITE_TEST:-0}" = 1 ]; then ANS="${KV260_CONFIRM:-}"; else printf 'Type the device name (%s) to continue: ' "$NAME"; read -r ANS; fi
[ "$ANS" = "$NAME" ] || die "not confirmed; nothing was written"
dd if="$IMG" of="$DEV" bs=4M conv=fsync,notrunc status=none
sync
# read back exactly the bytes written
cmp -n "$IMGSIZE" "$IMG" "$DEV" || die "READ-BACK DIFFERS: the card is not good; try another card or reader"
echo "read-back: the first $IMGSIZE bytes match"
# stretch partition 2 (the data partition) to the end of the card; the first 1 MiB of it holds the marker IYAKKI formats on first boot
if command -v sfdisk >/dev/null 2>&1; then
  START2="$(sfdisk -d "$DEV" | sed -n 's/.*start= *\([0-9]*\),.*type=83.*/\1/p' | head -n1)"
  if [ -n "$START2" ]; then
    if echo "$START2,+" | sfdisk --no-reread -N 2 "$DEV" >/dev/null 2>&1; then echo "data partition stretched to the end of the card"
    else echo "kv260-write-sd: could not stretch the data partition (the card still boots; the system uses the small partition)" >&2; fi
  fi
else
  echo "kv260-write-sd: sfdisk not found; the data partition keeps the image's size" >&2
fi
echo "Done. Put the card in the KV260 (its microSD slot is on the carrier card; AMD's Getting Started guide shows where), see docs/BOARD-KV260-FIRST-BOOT.md."
