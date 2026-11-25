#!/bin/sh
# Buildroot post-image hook of the KV260 variant: stage what genimage.cfg names (the board file, the device trees, extlinux.conf and
# the data marker), then genimage (called by Buildroot's support/scripts/genimage.sh with the same arguments as the other boards).
set -eu
BOARD_DIR="$(dirname "$0")"
cp -f "$BOARD_DIR/extlinux.conf" "$BINARIES_DIR/extlinux.conf"
# the boot partition's `board` file: the one line stage 1 and the installer compare with /proc/device-tree/model, so a KV260 image
# never runs on another board by mistake
printf 'kv260\n' > "$BINARIES_DIR/board-kv260"
rm -rf "$BINARIES_DIR/kv260-dtb"; mkdir -p "$BINARIES_DIR/kv260-dtb"
for d in "$BINARIES_DIR"/zynqmp-smk-k26-revA-sck-kv-g-rev*.dtb; do [ -f "$d" ] && cp -f "$d" "$BINARIES_DIR/kv260-dtb/"; done
# the data partition starts with our marker; the rest is zeros (IYAKKI formats it on first boot). Made fresh every time, never reused:
# nothing from a test run may reach a card (KI-1032). No filesystem here on purpose: first boot finds the marker, and a filesystem
# without it would be classed as foreign data and never used.
rm -f "$BINARIES_DIR/data-marker.img"
dd if=/dev/zero of="$BINARIES_DIR/data-marker.img" bs=1M count=1 2>/dev/null
printf 'IYAKKI-FORMAT-ME-v1\0' | dd of="$BINARIES_DIR/data-marker.img" conv=notrunc 2>/dev/null
