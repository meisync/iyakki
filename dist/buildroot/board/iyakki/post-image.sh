#!/bin/sh
# Buildroot post-image hook: turn the El Torito ISO into a real hybrid image that boots when written to a USB
# stick, on legacy BIOS (MBR boot code that loads GRUB from the ISO) and on UEFI (a GPT with a FAT EFI
# partition), as well as from a CD. Output: $BINARIES_DIR/iyakki-x86_64.iso (also used by `iyakki-usb`).
set -eu
TREE="$BUILD_DIR/buildroot-fs/rootfs.iso9660.tmp"
HYB="$(ls "$BUILD_DIR"/grub2-*/build-i386-pc/grub-core/boot_hybrid.img | head -n1)"
OUT="$BINARIES_DIR/iyakki-x86_64.iso"
BOARD="$(dirname "$0")"
[ -d "$TREE" ] || { echo "post-image: ISO tree $TREE missing" >&2; exit 1; }
# The boot menu itself: written here from the board's grub.cfg (same substitutions as Buildroot's ISO step), so the ISO and the
# install template below can never carry an older copy of the menu than the one in the tree. A build that still ends up without
# the graphical menu (no gfxpayload=keep, no theme) stops here instead of producing a stick that flickers and has no logo.
sed -e "s%__KERNEL_PATH__%/boot/bzImage%" -e "s%__INITRD_PATH__%/boot/initrd%" "$BOARD/grub.cfg" > "$TREE/boot/grub/grub.cfg"
for want in "set gfxpayload=keep" "themes/iyakki/theme.txt" "IYAKKI - remote diagnostics" "IYAKKI - safe graphics"; do
  grep -qF "$want" "$TREE/boot/grub/grub.cfg" || { echo "post-image: $TREE/boot/grub/grub.cfg lacks '$want'" >&2; exit 1; }
done
# The boot menu's looks: the logo picture, the theme and a font (GRUB's menu falls back to plain text when any is missing).
mkdir -p "$TREE/boot/grub/fonts" "$TREE/boot/grub/themes/iyakki"
cp "$BOARD/grub/iyakki-bg.png" "$TREE/boot/grub/themes/iyakki/iyakki-bg.png"
cp "$BOARD/grub/theme.txt" "$TREE/boot/grub/themes/iyakki/theme.txt"
for f in "$HOST_DIR/share/grub/unicode.pf2" "$BUILD_DIR"/grub2-*/unicode.pf2 /usr/share/grub/unicode.pf2 /usr/share/grub2/unicode.pf2 /boot/grub/fonts/unicode.pf2; do
  if [ -f "$f" ]; then cp "$f" "$TREE/boot/grub/fonts/unicode.pf2"; break; fi
done
[ -f "$TREE/boot/grub/fonts/unicode.pf2" ] || echo "post-image: no unicode.pf2 found; the boot menu will be plain text (install grub-common on the build machine)" >&2
# Secure Boot (docs/SECURE-BOOT.md): with IYAKKI_SB_KEY and IYAKKI_SB_CERT set, the kernel is signed and the ISO's EFI partition is
# rebuilt with shim + MokManager + the signed GRUB + the certificate. Without them nothing changes: the image is the plain one.
SB=0
if [ -n "${IYAKKI_SB_KEY:-}" ] && [ -n "${IYAKKI_SB_CERT:-}" ]; then
  SB=1
  sh "$BOARD/../../secureboot.sh" image "$TREE" - || { echo "post-image: the Secure Boot signing step failed" >&2; exit 1; }
else
  echo "post-image: IYAKKI_SB_KEY / IYAKKI_SB_CERT not set: building the UNSIGNED image (Secure Boot must be off to start it)" >&2
fi
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
cp -r "$TREE/boot/grub/fonts" "$TREE/boot/grub/themes" "$ROOT/boot/grub/"
cmp -s "$BOARD/grub/theme.txt" "$ROOT/boot/grub/themes/iyakki/theme.txt" || { echo "post-image: the install template has no GRUB theme" >&2; exit 1; }
cp "$BINARIES_DIR/bzImage" "$ROOT/boot/bzImage"
cp "$BINARIES_DIR/rootfs.cpio.zst" "$ROOT/boot/initrd"
cp "$(ls "$BUILD_DIR"/grub2-*/build-i386-pc/grub-core/boot.img | head -n1)" "$BINARIES_DIR/boot.img"
if [ "$SB" = 1 ]; then
  # the install template: signed kernel, shim as EFI/BOOT/BOOTX64.EFI, signed GRUB beside it, MokManager, the certificate
  rm -f "$ROOT/EFI/BOOT/bootx64.efi"
  sh "$BOARD/../../secureboot.sh" image - "$ROOT" || { echo "post-image: the Secure Boot signing step failed" >&2; exit 1; }
fi
# partition 2 starts with our marker; the rest is zeros (IYAKKI formats it on first boot)
dd if=/dev/zero of="$BINARIES_DIR/data-marker.img" bs=1M count=1 2>/dev/null
printf 'IYAKKI-FORMAT-ME-v1\0' | dd of="$BINARIES_DIR/data-marker.img" conv=notrunc 2>/dev/null
