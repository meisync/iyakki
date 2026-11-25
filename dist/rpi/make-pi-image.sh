#!/usr/bin/env bash
# Assembles the Raspberry Pi 5 SD-card image from pieces that already exist (incremental: no Buildroot run): the kernel pieces of build-pi-kernel.sh,
# the userland of a finished aarch64 build (its rootfs.cpio.gz), the aarch64 first stage, and the boot-volume texts that `iyakki-usb rpi files` writes.
# Linux build host only. Writes ONLY into OUT_DIR; never touches a drive.
#   dist/rpi/make-pi-image.sh --kernel DIR --userland IMAGES_DIR --stage1 BIN --usb BIN --out DIR [--iyakki BIN] [--level N] [--no-full-initrd]
#     --kernel     OUT_DIR of build-pi-kernel.sh (kernel8.img, the dtb, overlays/, modroot/)
#     --userland   a finished aarch64 build's images/ (needs rootfs.cpio.gz)
#     --stage1     the aarch64 musl build of iyakki-stage1
#     --usb        a host build of iyakki-usb (writes config.txt, cmdline*.txt, iyakki-boot.txt, board, the card's table and checks the volume)
#     --iyakki     replace /usr/bin/iyakki in the root with this (aarch64 musl) build
# Makes in OUT_DIR: iyakki-rpi5.img (the card image: the install template the writer stretches to the card), boot/ (the boot volume's files),
# initrd-lean, initrd, root.sfs, image.sha256, sizes.txt, boot-files.txt.
# Nothing from Raspberry Pi's firmware set is in the image: a Pi 5 needs none (its EEPROM has the VideoCore firmware); `iyakki-usb rpi check` fails the build if
# bootcode.bin, start*.elf, fixup*.dat or an EEPROM image would be on the volume.
set -eu
HERE="$(cd "$(dirname "$0")" && pwd)"
K=""; U=""; S1=""; USB=""; OUT=""; IYAKKI=""; LEVEL=3; NOFULL=0
while [ $# -gt 0 ]; do
  case "$1" in --kernel) K="$2"; shift 2 ;; --userland) U="$2"; shift 2 ;; --stage1) S1="$2"; shift 2 ;; --usb) USB="$2"; shift 2 ;; --out) OUT="$2"; shift 2 ;;
    --iyakki) IYAKKI="$2"; shift 2 ;; --level) LEVEL="$2"; shift 2 ;; --no-full-initrd) NOFULL=1; shift ;;
    *) echo "make-pi-image: unknown option $1" >&2; exit 2 ;; esac
done
die() { echo "make-pi-image: $*" >&2; exit 1; }
[ -n "$K" ] && [ -n "$U" ] && [ -n "$S1" ] && [ -n "$USB" ] && [ -n "$OUT" ] || { sed -n '2,16p' "$0" >&2; exit 2; }
for t in fakeroot cpio zstd mksquashfs mkfs.vfat mcopy mmd mdir mke2fs xz depmod; do command -v "$t" >/dev/null 2>&1 || [ -x "/usr/sbin/$t" ] || die "missing $t"; done
for f in "$K/kernel8.img" "$K/bcm2712-rpi-5-b.dtb" "$K/kernel.release" "$U/rootfs.cpio.gz" "$S1" "$USB"; do [ -s "$f" ] || die "no $f"; done
[ -d "$K/modroot/lib/modules" ] || die "no $K/modroot/lib/modules"
REL="$(cat "$K/kernel.release")"
# KI-1034: the root must be a build of THIS workspace version (the build writes it to etc/iyakki-release from Cargo.toml); an old root is refused
WANT="$(sed -n 's/^version *= *"\(.*\)"/\1/p' "$HERE/../../Cargo.toml" | head -n1)"
[ -n "$WANT" ] || die "no workspace version in $HERE/../../Cargo.toml"
mkdir -p "$OUT"; OUT="$(cd "$OUT" && pwd)"; K="$(cd "$K" && pwd)"; U="$(cd "$U" && pwd)"
W="$(mktemp -d "${TMPDIR:-/var/tmp}/pi-image-XXXXXX")"; trap 'rm -rf "$W"' EXIT
export W K U S1 OUT LEVEL NOFULL IYAKKI REL WANT

cat > "$W/session.sh" <<'EOS'
set -eu
mkdir "$W/root"; cd "$W/root"
gzip -dc "$U/rootfs.cpio.gz" | cpio -idm --quiet --no-absolute-filenames
# the old build's kernel modules and kernel copy are for another kernel: the root carries this kernel's modules only
rm -rf lib/modules boot/Image boot/bzImage
# KI-1034: refuse a root that is not this workspace's build (an old userland was put on the card on 2026-11-24)
GOT="$(sed -n 's/^VERSION=//p' etc/iyakki-release 2>/dev/null | head -n1)"
[ "$GOT" = "$WANT" ] || { echo "make-pi-image: the root in $U is $GOT, not this workspace's $WANT: build the aarch64 userland again (it must carry etc/iyakki-release VERSION=$WANT)" >&2; exit 1; }
mkdir -p lib/modules; cp -a "$K/modroot/lib/modules/$REL" "lib/modules/$REL"
# KI-1039: the card's BusyBox modprobe cannot read xz-compressed modules, so they are decompressed here (xz -d removes each .ko.xz) and depmod
# names the .ko files in modules.dep; the copy is the root's own, the kernel build's modroot is not touched
find "lib/modules/$REL" -name '*.ko.xz' -exec xz -d {} +
depmod -b "$W/root" "$REL"
[ -z "$IYAKKI" ] || { cp "$IYAKKI" usr/bin/iyakki; chmod 0755 usr/bin/iyakki; }
# the first stage: stage 1 as /init, BusyBox for the emergency shell, the musl loader
mkdir "$W/s1"; cd "$W/s1"
mkdir -p bin lib dev proc sys s newroot lower upper
cp "$S1" init; chmod 0755 init
cp "$W/root/bin/busybox" bin/busybox; chmod 0755 bin/busybox
cp "$W/root/lib/libc.so" lib/libc.so; ln -s libc.so lib/ld-musl-aarch64.so.1; ln -s lib lib64
find . | LC_ALL=C sort | cpio --quiet -o -H newc | zstd -19 -T0 -q -f -o "$OUT/initrd-lean"
cd "$W/root"
mksquashfs "$W/root" "$OUT/root.sfs" -comp zstd -Xcompression-level "$LEVEL" -b 256K -noappend -quiet -no-progress -all-time 0 >/dev/null
# the full RAM disk (the fallback after a failed fast start): the whole root as a cpio
if [ "$NOFULL" = 0 ]; then find . | LC_ALL=C sort | cpio --quiet -o -H newc | zstd -9 -T0 -q -f -o "$OUT/initrd"; fi
mkdir -p "$OUT/boot-extra"; cp etc/iyakki-release "$OUT/boot-extra/iyakki-release" 2>/dev/null || true
EOS
echo "make-pi-image: unpacking the userland and putting this kernel's modules in (fakeroot)"
fakeroot -- bash "$W/session.sh"
[ -s "$OUT/initrd-lean" ] && [ -s "$OUT/root.sfs" ] || die "the RAM disk or the root was not made"

# ---- the boot volume's files ----
B="$OUT/boot"; rm -rf "$B"; mkdir -p "$B/boot" "$B/overlays"
"$USB" rpi files --board pi5 --out "$B" >/dev/null
cp "$K/kernel8.img" "$K/bcm2712-rpi-5-b.dtb" "$B/"
[ ! -f "$K/overlays/vc4-kms-v3d-pi5.dtbo" ] || cp "$K/overlays/vc4-kms-v3d-pi5.dtbo" "$B/overlays/"
cp "$OUT/initrd-lean" "$B/initrd-lean"; cp "$OUT/root.sfs" "$B/boot/root.sfs"
[ ! -f "$OUT/initrd" ] || cp "$OUT/initrd" "$B/initrd"
[ ! -f "$OUT/boot-extra/iyakki-release" ] || cp "$OUT/boot-extra/iyakki-release" "$B/iyakki-release"
rmdir "$B/overlays" 2>/dev/null || true
PAYLOAD=$(find "$B" -type f -printf '%s\n' | paste -sd+ | bc)
FATMB="$("$USB" rpi size --payload-bytes "$PAYLOAD")"
echo "make-pi-image: boot volume payload $((PAYLOAD / 1048576)) MB, FAT32 volume $FATMB MB"

# ---- the FAT32 boot volume ----
FAT="$W/boot.vfat"; rm -f "$FAT"
mkfs.vfat -F 32 -n IYAKKI-BOOT -C "$FAT" $(( FATMB * 1024 )) >/dev/null
export MTOOLS_SKIP_CHECK=1
mcopy -s -i "$FAT" "$B"/* ::/ >/dev/null
( cd "$B" && find . -type f | sed 's#^\./##' | LC_ALL=C sort ) > "$OUT/boot-files.txt"
mdir -/ -b -i "$FAT" 2>/dev/null | sed 's#^::/##' | grep -v '/$' > "$W/on-volume.txt" || true
"$USB" rpi check --board pi5 --list "$OUT/boot-files.txt" >/dev/null || die "the boot volume check failed (see above)"

# ---- the card image: table, boot volume, marked data partition ----
"$USB" rpi mbr --boot-mib "$FATMB" --out "$W/mbr.bin" >/dev/null
IMG="$OUT/iyakki-rpi5.img"; rm -f "$IMG"
MARK="$W/marker.img"; dd if=/dev/zero of="$MARK" bs=1M count=1 2>/dev/null; printf 'IYAKKI-FORMAT-ME-v1\0' | dd of="$MARK" conv=notrunc 2>/dev/null
dd if="$W/mbr.bin" of="$IMG" bs=512 count=1 conv=notrunc 2>/dev/null
dd if="$FAT" of="$IMG" bs=512 seek=8192 conv=notrunc 2>/dev/null
dd if="$MARK" of="$IMG" bs=512 seek=$(( 8192 + FATMB * 2048 )) conv=notrunc 2>/dev/null
truncate -s $(( (8192 + FATMB * 2048 + 2048) * 512 )) "$IMG"
( cd "$OUT" && sha256sum iyakki-rpi5.img > image.sha256 )
{
  echo "image $(stat -c %s "$IMG") bytes ($(( $(stat -c %s "$IMG") / 1048576 )) MB)"
  for f in initrd-lean initrd root.sfs; do [ ! -f "$OUT/$f" ] || echo "$f $(stat -c %s "$OUT/$f") bytes"; done
  echo "kernel8.img $(stat -c %s "$K/kernel8.img") bytes, kernel $REL"
  echo "boot volume $FATMB MB FAT32 IYAKKI-BOOT, data marker partition 1 MB (the writer stretches it to the card)"
} | tee "$OUT/sizes.txt"
sfdisk -d "$IMG" 2>/dev/null | sed 's#^/[^ ]*iyakki-rpi5.img#iyakki-rpi5.img#' | tee "$OUT/partition-table.txt"
echo "make-pi-image: $IMG"
