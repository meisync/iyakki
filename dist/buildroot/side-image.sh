#!/bin/sh
# side-image.sh - the build side of the side image (F-BOOT-25; device side: iyakki_core::sideimage; plan: docs/MAC-HARDWARE.md).
#
# GRUB reads the kernel and the initial RAM disk through the firmware's slow USB driver (about 6 MB/s on a Mac). The big folders
# (lib/firmware, lib/modules) are moved out of the RAM disk into one squashfs file on the FAT boot partition, which Linux's own USB
# driver reads at tens of MB/s once the kernel runs. The ISO keeps the full RAM disk (no side file there: the device side does nothing
# without `iyakki.side=1`), only the install image carries the pair.
#
#   side-image.sh split ROOTFS-TREE OUTDIR    OUTDIR/side-<kernel release>.sqfs (lib/firmware + lib/modules, zstd), OUTDIR/small-root (the
#                                             rest of the tree) and OUTDIR/initrd-small (a gzip cpio of small-root, same format as boot/initrd)
#   side-image.sh grubcfg BOOTDIR             BOOTDIR/boot/grub/iyakki-side.cfg: the one file that switches the boot menu to the side start
#   side-image.sh bootsize BOOTDIR            the boot volume size in MiB this tree needs (at least 256, in steps of 64, 25 percent spare)
#
# Tools at build time only (never shipped): mksquashfs (squashfs-tools, GPL-2.0+), cpio, gzip, find.
set -eu

die() { echo "side-image: $*" >&2; exit 1; }

release_of() { # the kernel release: the one folder under lib/modules
  n=0; rel=""
  for d in "$1"/lib/modules/*/; do [ -d "$d" ] || continue; rel="$(basename "$d")"; n=$((n + 1)); done
  [ "$n" -eq 1 ] || die "expected exactly one kernel under $1/lib/modules, found $n"
  case "$rel" in *[!A-Za-z0-9._+-]*|"") die "the kernel release '$rel' has characters a file name on the boot volume may not carry" ;; esac
  [ "${#rel}" -le 64 ] || die "the kernel release is longer than 64 characters"
  printf '%s' "$rel"
}

cmd_split() {
  tree="${1:?usage: split ROOTFS-TREE OUTDIR}"; out="${2:?usage: split ROOTFS-TREE OUTDIR}"
  [ -d "$tree/lib/modules" ] && [ -d "$tree/lib/firmware" ] || die "$tree has no lib/modules and lib/firmware"
  command -v mksquashfs >/dev/null 2>&1 || die "mksquashfs is not installed (squashfs-tools)"
  rel="$(release_of "$tree")"
  [ -s "$tree/lib/modules/$rel/modules.dep" ] || die "$tree/lib/modules/$rel/modules.dep is missing (run depmod before splitting)"
  rm -rf "$out"; mkdir -p "$out/small-root"
  side="$out/side-$rel.sqfs"
  # the squashfs root holds lib/firmware and lib/modules exactly as the device side lays them under /lib
  stage="$out/.stage"; mkdir -p "$stage/lib"
  cp -a "$tree/lib/firmware" "$stage/lib/firmware"; cp -a "$tree/lib/modules" "$stage/lib/modules"
  mksquashfs "$stage" "$side" -comp zstd -Xcompression-level 15 -noappend -no-xattrs -all-root -quiet >/dev/null || die "mksquashfs failed"
  rm -rf "$stage"
  # the small tree: everything but the two folders (their parents stay, the device side overlays them)
  ( cd "$tree" && find . -path ./lib/firmware -prune -o -path ./lib/modules -prune -o -print ) | ( cd "$tree" && cpio -pdm --quiet "$out/small-root" ) || die "copying the small tree failed"
  mkdir -p "$out/small-root/lib/firmware" "$out/small-root/lib/modules"
  ( cd "$out/small-root" && find . | cpio -o -H newc --quiet 2>/dev/null | gzip -9 > "$out/initrd-small" ) || die "making the small RAM disk failed"
  echo "side-image: $side ($(wc -c < "$side" | tr -d ' ') bytes) and $out/initrd-small ($(wc -c < "$out/initrd-small" | tr -d ' ') bytes) for kernel $rel"
}

cmd_grubcfg() {
  boot="${1:?usage: grubcfg BOOTDIR}"
  [ -d "$boot/boot/grub" ] || die "$boot/boot/grub does not exist"
  ls "$boot"/boot/side-*.sqfs >/dev/null 2>&1 || die "no boot/side-<release>.sqfs under $boot: the menu is switched only for a volume that carries it"
  [ -s "$boot/boot/initrd-small" ] || die "no boot/initrd-small under $boot"
  cat > "$boot/boot/grub/iyakki-side.cfg" <<'EOF'
# The side image (F-BOOT-25): written by side-image.sh into an install image that carries boot/side-<release>.sqfs and boot/initrd-small.
# GRUB reads only the small RAM disk; the kernel mounts the side file with its own USB driver (iyakki.side=1, added to the variable every
# entry already passes after the kernel path). If the small RAM disk is missing the menu keeps the full one.
if [ -f /boot/initrd-small ]; then
	if [ -z "$iyakki_usbq" ]; then
		set iyakki_usbq="iyakki.side=1"
	else
		set iyakki_usbq="$iyakki_usbq iyakki.side=1"
	fi
	set iyakki_initrd=/boot/initrd-small
	set iyakki_fast=""
	export iyakki_usbq
	export iyakki_initrd
	export iyakki_fast
fi
EOF
  echo "side-image: $boot/boot/grub/iyakki-side.cfg"
}

cmd_bootsize() {
  boot="${1:?usage: bootsize BOOTDIR}"
  [ -d "$boot" ] || die "$boot is not a folder"
  bytes="$(find "$boot" -type f -exec wc -c {} + | awk '{ if ($2 != "total") s += $1; else t = $1 } END { print s ? s : t }')"
  mib=$(( (bytes * 125 / 100 + 1048575) / 1048576 ))
  size=$(( (mib + 63) / 64 * 64 )); [ "$size" -ge 256 ] || size=256
  echo "$size"
}

case "${1:-}" in
  split) shift; cmd_split "$@" ;;
  grubcfg) shift; cmd_grubcfg "$@" ;;
  bootsize) shift; cmd_bootsize "$@" ;;
  *) die "usage: side-image.sh split|grubcfg|bootsize ..." ;;
esac
