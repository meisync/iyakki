#!/usr/bin/env bash
# One-time setup of a compressed build disk on a Linux build host (run ON the host, needs sudo):
#   dist/buildhost/setup-compressed-disk.sh [IMAGE_FILE] [SIZE] [MOUNT_POINT]     defaults: ~/iyakki-build.img  300G  /mnt/iyakki-build
# A sparse image file formatted as btrfs and mounted with zstd compression: build folders shrink 3 to 5 times and the builds can never fill the root disk.
# It adds an fstab line with nofail (a missing image never stops the machine from booting) after saving a copy of /etc/fstab.
set -euo pipefail
IMG="${1:-$HOME/iyakki-build.img}"; SIZE="${2:-300G}"; MNT="${3:-/mnt/iyakki-build}"
[ ! -e "$IMG" ] || { echo "$IMG exists: not touching it" >&2; exit 1; }
command -v mkfs.btrfs >/dev/null || sudo apt-get install -y btrfs-progs
truncate -s "$SIZE" "$IMG"
sudo mkfs.btrfs -q -L iyakki-build "$IMG"
sudo mkdir -p "$MNT"
sudo mount -o loop,compress=zstd:1,noatime "$IMG" "$MNT"
sudo chown "$(id -un):$(id -gn)" "$MNT"
mkdir -p "$MNT/src" "$MNT/target"
sudo cp /etc/fstab "/etc/fstab.bak-iyakki-build-$(date +%F)"
grep -q "$IMG" /etc/fstab || echo "$IMG $MNT btrfs loop,compress=zstd:1,noatime,nofail,x-systemd.device-timeout=10 0 0" | sudo tee -a /etc/fstab >/dev/null
df -h "$MNT" | tail -1
