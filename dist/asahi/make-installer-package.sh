#!/bin/sh
# Packs a built IYAKKI Apple Silicon image for the Asahi installer (alx.sh style), WITHOUT any Apple firmware.
#   dist/asahi/make-installer-package.sh <esp-dir> <data.img> <out-dir> [version]
# <esp-dir> holds what goes on the EFI partition: m1n1/boot.bin (m1n1 stage 2 + device trees + U-Boot payload),
# EFI/BOOT/BOOTAA64.EFI (GRUB), Image, rootfs.cpio.gz, EFI/BOOT/grub.cfg.  <data.img> is an ext4 image (multiple of 4 KiB).
# Output: <out-dir>/os/iyakki-apple-<version>.zip and <out-dir>/installer_data.json (check it with  asahi_package.py validate).
# Serve <out-dir> and run the installer with  INSTALLER_DATA=<url>/installer_data.json REPO_BASE=<url>  (docs/APPLE-SILICON.md).
set -eu
HERE="$(cd "$(dirname "$0")" && pwd)"
[ $# -ge 3 ] || { sed -n 2,8p "$0" | sed 's/^# \{0,1\}//'; exit 2; }
exec python3 -I "$HERE/asahi_package.py" make --esp "$1" --data-img "$2" --out "$3" --version "${4:-0.0.0}"
