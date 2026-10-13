#!/bin/sh
# Buildroot post-image hook: put our GRUB menu next to the EFI loader before genimage builds the disk image.
set -eu
BOARD_DIR="$(dirname "$0")"
cp -f "$BOARD_DIR/grub.cfg" "$BINARIES_DIR/efi-part/EFI/BOOT/grub.cfg"
