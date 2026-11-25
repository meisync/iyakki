#!/bin/sh
# grub-refresh.sh BUILD_DIR IMAGES_DIR [DEFCONFIG]
#
# Makes the BIOS and UEFI GRUB images of an install image again from the GRUB build tree of BUILD_DIR (a Buildroot output folder: host/bin/grub-mkimage,
# build/grub2-*/build-*/grub-core, .config), with the module lists of the DEFCONFIG (default: dist/buildroot/configs/iyakki_x86_64_defconfig), and puts them
# into IMAGES_DIR (grub.img, grub-eltorito.img, efi-part/EFI/BOOT/bootx64.efi). It is the same grub-mkimage call as Buildroot's grub2 package makes (the
# result is byte for byte the donor's own image when the lists are the same: Q6 compared them), so nothing is built from source again: seconds, not hours.
#
# Why: `repack.sh` takes GRUB from the kernel donor. A donor built before the `loadenv` module was added to the lists (2026-11-14) put a BIOS GRUB without
# `load_env` and `save_env` into every image repacked from it, and the boot counter of a system update (F-UPD-02) never stepped: the first real GRUB run of
# dist/qemu/update-faults-test.py showed it ("error: can't find command `load_env'"). repack.sh runs this script on the staged images, so the lists of the
# defconfig, not the age of the donor, decide.
#
# The UEFI file is replaced only when the one in IMAGES_DIR is the plain Buildroot GRUB (it equals what the donor's own .config lists make); a signed or
# shim-chained file is left alone (secureboot.sh builds those from the tree's .config; refresh that tree first). Says what it did on stdout, one line each.
# Test: crates/iyakki-core/tests/grub_refresh.rs (a stand-in grub-mkimage, GRUB_MKIMAGE).
set -eu
B="${1:?usage: grub-refresh.sh BUILD_DIR IMAGES_DIR [DEFCONFIG]}"; IMGS="${2:?usage: grub-refresh.sh BUILD_DIR IMAGES_DIR [DEFCONFIG]}"
HERE="$(cd "$(dirname "$0")" && pwd)"
DEF="${3:-$HERE/configs/iyakki_x86_64_defconfig}"
MK="${GRUB_MKIMAGE:-$B/host/bin/grub-mkimage}"
say() { echo "grub-refresh: $*"; }
die() { echo "grub-refresh: $*" >&2; exit 1; }
[ -x "$MK" ] || die "no grub-mkimage at $MK"
[ -f "$DEF" ] || die "no $DEF"
TREE="$(ls -d "$B"/build/grub2-* 2>/dev/null | head -n1 || true)"; [ -d "$TREE" ] || die "no GRUB build tree in $B/build"
val() { sed -n "s/^$2=\"\(.*\)\"\$/\1/p" "$1" | head -n1 | sed "s#\$(BR2_EXTERNAL_IYAKKI_PATH)#$HERE#g"; }
W="$(mktemp -d "${TMPDIR:-/tmp}/iyakki-grub-refresh.XXXXXX")"; trap 'rm -rf "$W"' EXIT
# mkgrub TUPLE PLATFORM PREFIX CONFIG-FILE MODULES OUT
mkgrub() {
  # shellcheck disable=SC2086
  "$MK" -d "$TREE/build-$1/grub-core/" -O "$1" -o "$6" -p "$3" ${4:+-c "$4"} $5 || die "grub-mkimage failed for $1"
}
PART="$(val "$DEF" BR2_TARGET_GRUB2_BOOT_PARTITION)"
# ---- BIOS
mods="$(val "$DEF" BR2_TARGET_GRUB2_BUILTIN_MODULES_PC)"; [ -n "$mods" ] || die "no BR2_TARGET_GRUB2_BUILTIN_MODULES_PC in $DEF"
case " $mods " in *" loadenv "*) ;; *) die "the BIOS module list of $DEF has no loadenv: the boot counter cannot run" ;; esac
mkgrub i386-pc pc "(${PART:-cd})/boot/grub" "$(val "$DEF" BR2_TARGET_GRUB2_BUILTIN_CONFIG_PC)" "$mods" "$W/grub.img"
mkdir -p "$IMGS"
if [ -f "$IMGS/grub.img" ] && cmp -s "$W/grub.img" "$IMGS/grub.img"; then
  say "grub.img already has the defconfig's modules"
else
  cp -f "$W/grub.img" "$IMGS/grub.img"
  cat "$TREE/build-i386-pc/grub-core/cdboot.img" "$W/grub.img" > "$IMGS/grub-eltorito.img"
  say "grub.img and grub-eltorito.img made again with the defconfig's modules (loadenv included)"
fi
# ---- UEFI
efi="$IMGS/efi-part/EFI/BOOT/bootx64.efi"
if [ -f "$efi" ] && [ -d "$TREE/build-x86_64-efi/grub-core" ]; then
  emods="$(val "$DEF" BR2_TARGET_GRUB2_BUILTIN_MODULES_EFI)"; [ -n "$emods" ] || die "no BR2_TARGET_GRUB2_BUILTIN_MODULES_EFI in $DEF"
  case " $emods " in *" loadenv "*) ;; *) die "the UEFI module list of $DEF has no loadenv: the boot counter cannot run" ;; esac
  ecfg="$(val "$DEF" BR2_TARGET_GRUB2_BUILTIN_CONFIG_EFI)"
  mkgrub x86_64-efi efi /EFI/BOOT "$ecfg" "$emods" "$W/new.efi"
  if cmp -s "$W/new.efi" "$efi"; then
    say "bootx64.efi already has the defconfig's modules"
  elif [ -f "$B/.config" ]; then
    own="$(val "$B/.config" BR2_TARGET_GRUB2_BUILTIN_MODULES_EFI)"
    mkgrub x86_64-efi efi /EFI/BOOT "$(val "$B/.config" BR2_TARGET_GRUB2_BUILTIN_CONFIG_EFI)" "$own" "$W/old.efi"
    if cmp -s "$W/old.efi" "$efi"; then
      cp -f "$W/new.efi" "$efi"; say "bootx64.efi made again with the defconfig's modules (loadenv included)"
    else
      say "bootx64.efi is not the plain Buildroot GRUB (signed or shim-chained): left as it is; refresh the tree it is made from"
    fi
  else
    say "bootx64.efi left as it is (no .config in $B to tell whether it is the plain Buildroot GRUB)"
  fi
fi
