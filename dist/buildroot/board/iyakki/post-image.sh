#!/bin/sh
# Buildroot post-image hook: turn the El Torito ISO into a real hybrid image that boots when written to a USB
# stick, on legacy BIOS (MBR boot code that loads GRUB from the ISO) and on UEFI (a GPT with a FAT EFI
# partition), as well as from a CD. Output: $BINARIES_DIR/iyakki-x86_64.iso (also used by `iyakki-usb`).
set -eu
TREE="$BUILD_DIR/buildroot-fs/rootfs.iso9660.tmp"
HYB="$(ls "$BUILD_DIR"/grub2-*/build-i386-pc/grub-core/boot_hybrid.img | head -n1)"
OUT="$BINARIES_DIR/iyakki-x86_64.iso"
[ -d "$TREE" ] || { echo "post-image: ISO tree $TREE missing" >&2; exit 1; }
"$HOST_DIR/bin/xorriso" -as mkisofs -r -J -V IYAKKI \
  -b boot/grub/grub-eltorito.img -no-emul-boot -boot-load-size 4 -boot-info-table --grub2-boot-info \
  --grub2-mbr "$HYB" -partition_offset 16 \
  -append_partition 2 0xef "$TREE/boot/fat.efi" -appended_part_as_gpt -iso_mbr_part_type 0x00 \
  -eltorito-alt-boot -e --interval:appended_partition_2:all:: -no-emul-boot \
  -o "$OUT" "$TREE" 2>&1 | tail -3
echo "post-image: wrote $OUT"

# ---- the install template: stage what genimage (run next) puts into the FAT boot partition ----
ROOT="$BINARIES_DIR/install-root"
rm -rf "$ROOT"
mkdir -p "$ROOT/EFI/BOOT" "$ROOT/boot/grub"
cp "$BINARIES_DIR/efi-part/EFI/BOOT/bootx64.efi" "$ROOT/EFI/BOOT/bootx64.efi"
cp "$TREE/boot/grub/grub.cfg" "$ROOT/boot/grub/grub.cfg"
cp "$BINARIES_DIR/bzImage" "$ROOT/boot/bzImage"
cp "$BINARIES_DIR/rootfs.cpio.zst" "$ROOT/boot/initrd"
cp "$(ls "$BUILD_DIR"/grub2-*/build-i386-pc/grub-core/boot.img | head -n1)" "$BINARIES_DIR/boot.img"
# partition 2 starts with our marker; the rest is zeros (IYAKKI formats it on first boot)
dd if=/dev/zero of="$BINARIES_DIR/data-marker.img" bs=1M count=1 2>/dev/null
printf 'IYAKKI-FORMAT-ME-v1\0' | dd of="$BINARIES_DIR/data-marker.img" conv=notrunc 2>/dev/null
