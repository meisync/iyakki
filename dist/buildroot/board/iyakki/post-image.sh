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
for want in "set gfxpayload=keep" "themes/iyakki/theme.txt" "IYAKKI - remote diagnostics" "IYAKKI - safe graphics" "IYAKKI - Install to a disk"; do
  grep -qF "$want" "$TREE/boot/grub/grub.cfg" || { echo "post-image: $TREE/boot/grub/grub.cfg lacks '$want'" >&2; exit 1; }
done
# The boot menu's looks: the logo picture, the theme and a font (GRUB's menu falls back to plain text when any is missing).
mkdir -p "$TREE/boot/grub/fonts" "$TREE/boot/grub/themes/iyakki"
cp "$BOARD/grub/iyakki-bg.png" "$TREE/boot/grub/themes/iyakki/iyakki-bg.png"
cp "$BOARD/grub/theme.txt" "$TREE/boot/grub/themes/iyakki/theme.txt"
# Fonts for the menu and the text under the logo, made here when grub-mkfont is on the build machine (Latin only, about 40 KB each):
# the menu rows in DejaVu Sans Bold 30 (no hinting: the hinted advances are rounded up and look wider spaced), and the loading text /
# terminal in DejaVu Sans Mono 24, which GRUB needs to be monospaced (a terminal is a grid; -c/-d make the line as tall as the letters,
# or the tails of g and y are cut off). These are the ONLY fonts of the boot menu (GRUB's stock Unicode font is not shipped;
# decision 2026-11-06). The theme names them in full because GRUB finds a font by the NAME inside the .pf2 and silently uses the one
# loaded last when no font has the asked name. Characters outside Latin show GRUB's own missing-glyph box in the menu.
MENUFONT=""; TERMFONT=""
# The fonts are made from the DejaVu files IN THIS REPOSITORY (crates/iyakki-shell/assets, licence text DEJAVU-LICENSE.txt, listed in
# legal/assets.txt): a licence that is recorded, not whatever the build machine has. The build machine's DejaVu is only a fallback for
# the development images; the RELEASE image (IYAKKI_IMAGE_PROFILE=release) never uses it.
ASSETS="$BOARD/../../../../crates/iyakki-shell/assets"
[ -f "$ASSETS/DejaVuSans-Bold.ttf" ] && MENUFONT="$ASSETS/DejaVuSans-Bold.ttf"
[ -f "$ASSETS/DejaVuSansMono.ttf" ] && TERMFONT="$ASSETS/DejaVuSansMono.ttf"
if [ -z "$MENUFONT" ] && [ "${IYAKKI_IMAGE_PROFILE:-}" != release ]; then
  for f in /usr/share/fonts/truetype/dejavu/DejaVuSans-Bold.ttf /usr/share/fonts/dejavu/DejaVuSans-Bold.ttf /usr/share/fonts/TTF/DejaVuSans-Bold.ttf; do [ -f "$f" ] && MENUFONT="$f" && break; done
fi
if [ -z "$TERMFONT" ] && [ "${IYAKKI_IMAGE_PROFILE:-}" != release ]; then
  for f in /usr/share/fonts/truetype/dejavu/DejaVuSansMono.ttf /usr/share/fonts/dejavu/DejaVuSansMono.ttf /usr/share/fonts/TTF/DejaVuSansMono.ttf; do [ -f "$f" ] && TERMFONT="$f" && break; done
fi
GMF="$(command -v grub-mkfont || command -v grub2-mkfont || true)"
THEME="$TREE/boot/grub/themes/iyakki/theme.txt"
if [ -n "$MENUFONT" ] && [ -n "$TERMFONT" ] && [ -n "$GMF" ] \
   && "$GMF" -s 30 --no-hinting --range=0x20-0x7e,0xa0-0x24f -o "$TREE/boot/grub/fonts/iyakki-menu.pf2" "$MENUFONT" 2>/dev/null \
   && "$GMF" -s 24 -c 18 -d 6 --range=0x20-0x7e,0xa0-0xff -o "$TREE/boot/grub/fonts/iyakki-term.pf2" "$TERMFONT" 2>/dev/null; then
  grep -q 'DejaVu Sans Bold 30' "$THEME" && grep -q 'DejaVu Sans Mono Regular 24' "$THEME" || { echo "post-image: the menu fonts are built but the theme does not name them" >&2; exit 1; }
else
  rm -f "$TREE/boot/grub/fonts/iyakki-menu.pf2" "$TREE/boot/grub/fonts/iyakki-term.pf2"
  echo "post-image: no grub-mkfont or DejaVu Sans Bold/Mono: the boot menu will be plain text" >&2
  [ "${IYAKKI_IMAGE_PROFILE:-}" != release ] || { echo "post-image: release image: the boot menu fonts could not be built" >&2; exit 1; }
fi
# Where the boot fonts came from and under what licence (legal/image-rules.txt `own` row): the text travels with the fonts, on the ISO and in
# the install template. The DejaVu licence is the repository's own copy; these are the only boot fonts.
FL="$TREE/boot/grub/fonts/FONTS-LICENSE.txt"
{
  cat "$BOARD/grub/FONTS-LICENSE.txt"
  echo
  echo "Built with this image:"
  if [ -f "$TREE/boot/grub/fonts/iyakki-menu.pf2" ]; then
    echo "  iyakki-menu.pf2   grub-mkfont of $(basename "$MENUFONT") (sha256 $(sha256sum "$MENUFONT" | cut -d' ' -f1))"
    echo "  iyakki-term.pf2   grub-mkfont of $(basename "$TERMFONT") (sha256 $(sha256sum "$TERMFONT" | cut -d' ' -f1))"
  else echo "  iyakki-menu.pf2, iyakki-term.pf2   not present"; fi
  echo
  echo "---- DejaVu fonts licence (copy of crates/iyakki-shell/assets/DEJAVU-LICENSE.txt) ----"
  cat "$ASSETS/DEJAVU-LICENSE.txt" 2>/dev/null || echo "(the repository's DEJAVU-LICENSE.txt was not found)"
} > "$FL"
if [ "${IYAKKI_IMAGE_PROFILE:-}" = release ]; then
  [ -f "$ASSETS/DEJAVU-LICENSE.txt" ] || { echo "post-image: release image: crates/iyakki-shell/assets/DEJAVU-LICENSE.txt is missing" >&2; exit 1; }
  if [ -n "$MENUFONT$TERMFONT" ] && ! { [ "$MENUFONT" = "$ASSETS/DejaVuSans-Bold.ttf" ] && [ "$TERMFONT" = "$ASSETS/DejaVuSansMono.ttf" ]; }; then
    echo "post-image: release image: the menu fonts must come from the repository's DejaVu files" >&2; exit 1
  fi
fi
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
cmp -s "$TREE/boot/grub/themes/iyakki/theme.txt" "$ROOT/boot/grub/themes/iyakki/theme.txt" || { echo "post-image: the install template has no GRUB theme" >&2; exit 1; }
cp "$BINARIES_DIR/bzImage" "$ROOT/boot/bzImage"
cp "$BINARIES_DIR/rootfs.cpio.zst" "$ROOT/boot/initrd"
# which IYAKKI these boot files are: `iyakki dev update-pack` reads it (version, variant, build date) and a system update writes it
# next to the new files, so the next update can tell what is installed even before the device is restarted
[ -f "$TARGET_DIR/etc/iyakki-release" ] && cp "$TARGET_DIR/etc/iyakki-release" "$ROOT/boot/iyakki-release"
cp "$(ls "$BUILD_DIR"/grub2-*/build-i386-pc/grub-core/boot.img | head -n1)" "$BINARIES_DIR/boot.img"
if [ "$SB" = 1 ]; then
  # the install template: signed kernel, shim as EFI/BOOT/BOOTX64.EFI, signed GRUB beside it, MokManager, the certificate
  rm -f "$ROOT/EFI/BOOT/bootx64.efi"
  sh "$BOARD/../../secureboot.sh" image - "$ROOT" || { echo "post-image: the Secure Boot signing step failed" >&2; exit 1; }
fi
# the extra boot loader (iyakki-boot): EFI/iyakki-boot/{loader.efi,boot.cfg,splash.bmp} when the loader program was built (see the script's header);
# GRUB's "IYAKKI - new boot loader (test)" entry shows only then. Not signed here: under Secure Boot sign loader.efi with iyakki-boot/tools/sign.sh.
sh "$BOARD/../../boot-loader-stage.sh" "$ROOT" /boot/initrd || echo "post-image: the extra boot loader was not staged (image is fine without it)" >&2
# IYAKKI_LOADER_DEFAULT=1 (default 0: GRUB stays the default way in): the loader becomes shim's second stage (EFI/BOOT/grubx64.efi) or, without
# shim, EFI/BOOT/BOOTX64.EFI, GRUB is kept as EFI/BOOT/grub-recovery.efi (the loader's Recovery row) and GRUB's own "new boot loader (test)"
# entry is dropped. Signed with the image key when IYAKKI_SB_KEY / IYAKKI_SB_CERT are set, else UNSIGNED (Secure Boot off only).
# docs/development/GRUB-REPLACEMENT-PLAN.md step 2. The live ISO is not touched: it keeps GRUB (it must start on BIOS too).
if [ "${IYAKKI_LOADER_DEFAULT:-0}" = 1 ]; then
  if sh "$BOARD/../../loader-default.sh" esp "$ROOT"; then
    sh "$BOARD/../../loader-default.sh" grubcfg "$ROOT/boot/grub/grub.cfg"
  else
    echo "post-image: IYAKKI_LOADER_DEFAULT=1 but the loader could not be made the default: this image keeps GRUB as the default" >&2
  fi
fi
# partition 2 starts with our marker; the rest is zeros (IYAKKI formats it on first boot)
dd if=/dev/zero of="$BINARIES_DIR/data-marker.img" bs=1M count=1 2>/dev/null
printf 'IYAKKI-FORMAT-ME-v1\0' | dd of="$BINARIES_DIR/data-marker.img" conv=notrunc 2>/dev/null
