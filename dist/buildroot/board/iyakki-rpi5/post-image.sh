#!/bin/sh
# Buildroot post-image hook of the rpi5 variant: no GRUB, no genimage. It only puts the kernel pieces where dist/rpi/make-pi-image.sh looks for them
# (BINARIES_DIR/rpi5/: kernel8.img, the device tree, the display overlay), so the image script can run from a finished build without Buildroot.
set -eu
OUT="$BINARIES_DIR/rpi5"
mkdir -p "$OUT/overlays"
cp -f "$BINARIES_DIR/Image" "$OUT/kernel8.img"
cp -f "$BINARIES_DIR/bcm2712-rpi-5-b.dtb" "$OUT/"
[ ! -f "$BINARIES_DIR/vc4-kms-v3d-pi5.dtbo" ] || cp -f "$BINARIES_DIR/vc4-kms-v3d-pi5.dtbo" "$OUT/overlays/"
echo "rpi5: kernel pieces in $OUT (next: dist/rpi/make-pi-image.sh)"
