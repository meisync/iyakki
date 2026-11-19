#!/bin/sh
# Buildroot post-image hook: put our GRUB menu next to the EFI loader before genimage builds the disk image.
set -eu
BOARD_DIR="$(dirname "$0")"
cp -f "$BOARD_DIR/grub.cfg" "$BINARIES_DIR/efi-part/EFI/BOOT/grub.cfg"
# the install template's data partition starts with our marker; the rest is zeros (IYAKKI formats it on first boot)
dd if=/dev/zero of="$BINARIES_DIR/data-marker.img" bs=1M count=1 2>/dev/null
printf 'IYAKKI-FORMAT-ME-v1\0' | dd of="$BINARIES_DIR/data-marker.img" conv=notrunc 2>/dev/null
