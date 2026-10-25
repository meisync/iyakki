#!/usr/bin/env bash
# Lean boot layout (docs/BOOT-SPEED.md): split a finished image's initial RAM disk into a tiny first stage and the real root.
#
#   lean-image.sh IMAGES_DIR OUT_DIR [--root sfs|tree|both|export] [--level N] [--fat-mb N] [--stage1 BIN] [--busybox-from DIR]
#
#   IMAGES_DIR  a finished build's images/ (needs rootfs.cpio.zst, bzImage, install-root/ and iyakki-install-x86_64.img)
#   OUT_DIR     everything is written here and nowhere else (the IMAGES_DIR is only read; no drive is ever written)
#
# Makes:
#   initrd-lean          the first-stage RAM disk: iyakki-stage1 as /init + BusyBox (emergency shell) + empty folders (about 1.5 MB)
#   root.sfs             the whole system as a zstd squashfs (--root sfs|both). NEEDS a kernel with CONFIG_SQUASHFS + CONFIG_SQUASHFS_ZSTD
#   data-tree.ext4       an ext4 volume labelled IYAKKI-DATA holding the extracted tree (iyakki/root/<id>/tree, pointer `current`)
#                        (--root tree|both). Works with the kernels built so far (ext4 is built in); the test route of this script
#   iyakki-lean-x86_64.img   the install template with the lean layout: a bigger FAT boot volume holding the lean initrd and (for
#                        sfs) boot/root.sfs; the data partition is the marker (sfs) or data-tree.ext4 (tree)
#   (every tree mode also writes tree/ (the extracted root), tree.manifest (owners, modes, checksums: files in tree/ belong to the builder, the
#   manifest has the real ones), root-id and grub-lean-block.cfg: the input of lean-install.sh, which puts the experiment on an installed stick;
#   `--root export` writes only these and initrd-lean, no image)
# Not touched: the Buildroot build, the ISO (the live ISO keeps the full initrd: it has no data disk, docs/BOOT-SPEED.md),
# repack.sh/post-image.sh (integration into the normal build is a later step).
set -eu
HERE="$(cd "$(dirname "$0")" && pwd)"; ROOT="$(cd "$HERE/../.." && pwd)"
IMG="${1:-}"; OUT="${2:-}"; shift 2 2>/dev/null || true
[ -d "$IMG" ] && [ -n "$OUT" ] || { sed -n '2,10p' "$0" >&2; exit 2; }
MODE=sfs; LEVEL=3; FATMB=0; S1="$ROOT/target/x86_64-unknown-linux-musl/release/iyakki-stage1"
while [ $# -gt 0 ]; do
  case "$1" in --root) MODE="$2"; shift 2 ;; --level) LEVEL="$2"; shift 2 ;; --fat-mb) FATMB="$2"; shift 2 ;; --stage1) S1="$2"; shift 2 ;;
    *) echo "lean-image: unknown option $1" >&2; exit 2 ;; esac
done
die() { echo "lean-image: $*" >&2; exit 1; }
for t in fakeroot cpio zstd python3; do command -v "$t" >/dev/null || die "missing $t"; done
case "$MODE" in sfs|tree|both|export|stick) ;; *) die "--root must be sfs, tree, both, export or stick" ;; esac
MCOPY="$(command -v mcopy || ls "$ROOT"/target/buildroot-*/host/bin/mcopy 2>/dev/null | head -n1 || true)"; [ "$MODE" = export ] || [ -n "$MCOPY" ] || die "no mcopy (mtools)"
MMD="$(dirname "${MCOPY:-/nonexistent/x}")/mmd"; [ -x "$MMD" ] || MMD="$(command -v mmd || true)"
if [ "$MODE" != export ]; then for t in mkfs.vfat sfdisk; do command -v "$t" >/dev/null || die "missing $t"; done; fi
case "$MODE" in sfs|both) command -v mksquashfs >/dev/null || die "missing mksquashfs" ;; esac
case "$MODE" in tree|both|stick) command -v mke2fs >/dev/null || [ -x /usr/sbin/mke2fs ] || die "missing mke2fs" ;; esac
MKE2FS="$(command -v mke2fs || echo /usr/sbin/mke2fs)"
[ -x "$S1" ] || die "no $S1: cargo build --release --target x86_64-unknown-linux-musl -p iyakki-stage1"
for f in rootfs.cpio.zst bzImage iyakki-install-x86_64.img; do [ -s "$IMG/$f" ] || die "no $IMG/$f"; done
[ -d "$IMG/install-root/EFI" ] || die "no $IMG/install-root"
IMG="$(cd "$IMG" && pwd)"; mkdir -p "$OUT"; OUT="$(cd "$OUT" && pwd)"; W="$(mktemp -d "${TMPDIR:-/tmp}/lean-XXXXXX")"; trap 'rm -rf "$W"' EXIT
ID="${IYAKKI_ROOT_ID:-$(date -u +%Y%m%d%H%M%S)}"
export W IMG OUT MODE LEVEL S1 ID MKE2FS HERE

cat > "$W/session.sh" <<'EOS'
set -eu
mkdir "$W/root"; cd "$W/root"
zstd -dc "$IMG/rootfs.cpio.zst" | cpio -idm --quiet --no-absolute-filenames
# the kernel copy inside the old initrd (12.7 MB) is useless in the root: GRUB loads the kernel from the boot volume
rm -f boot/bzImage
# a newer iyakki program (test builds): IYAKKI_BIN=/path/to/iyakki replaces /usr/bin/iyakki
[ -z "${IYAKKI_BIN:-}" ] || { cp "$IYAKKI_BIN" usr/bin/iyakki; chmod 0755 usr/bin/iyakki; }
# the keys that may sign in over SSH on the diagnostics entries are baked into the image at /etc/iyakki/authorized_keys; IYAKKI_AUTH_KEYS=FILE
# replaces that file in the lean tree (public keys only), so the machine that will connect to the stick can be named at build time
[ -z "${IYAKKI_AUTH_KEYS:-}" ] || { cp "$IYAKKI_AUTH_KEYS" etc/iyakki/authorized_keys; chmod 0644 etc/iyakki/authorized_keys; }
BUSY="bin/busybox"; MUSL="lib/libc.so"
# first stage
mkdir "$W/s1"; cd "$W/s1"
mkdir -p bin lib dev proc sys s newroot lower upper
cp "$S1" init; chmod 0755 init
cp "$W/root/$BUSY" bin/busybox; chmod 0755 bin/busybox
cp "$W/root/$MUSL" lib/libc.so; ln -s libc.so lib/ld-musl-x86_64.so.1; ln -s lib lib64
find . | LC_ALL=C sort | cpio --quiet -o -H newc | zstd -19 -T0 -q -f -o "$OUT/initrd-lean"
cd "$W/root"
if [ "$MODE" = sfs ] || [ "$MODE" = both ]; then
  mksquashfs "$W/root" "$OUT/root.sfs" -comp zstd -Xcompression-level "$LEVEL" -b 256K -noappend -quiet -no-progress -all-time 0 >/dev/null
fi
if [ "$MODE" != sfs ]; then
  # the input of lean-install.sh: the tree, its manifest (real owners and modes, made inside fakeroot), the id
  chmod -R u+w "$OUT/tree" 2>/dev/null || true; rm -rf "$OUT/tree"
  cp -a "$W/root" "$OUT/tree"
  python3 -I "$HERE/lean-sync.py" manifest "$W/root" > "$OUT/tree.manifest"
  printf '%s\n' "$ID" > "$OUT/root-id"
fi
if [ "$MODE" = tree ] || [ "$MODE" = both ] || [ "$MODE" = stick ]; then
  mkdir -p "$W/data/iyakki/root/$ID"
  mv "$W/root" "$W/data/iyakki/root/$ID/tree"
  printf '%s\n' "$ID" > "$W/data/iyakki/root/current"
  KB=$(du -sk "$W/data" | cut -f1); MB=$(( KB / 1024 * 12 / 10 + 64 ))
  rm -f "$OUT/data-tree.ext4"
  "$MKE2FS" -q -t ext4 -L IYAKKI-DATA -m 0 -d "$W/data" "$OUT/data-tree.ext4" "${MB}M"
fi
EOS
echo "lean-image: unpacking $IMG/rootfs.cpio.zst (fakeroot)"
fakeroot -- bash "$W/session.sh"
[ -s "$OUT/initrd-lean" ] || die "no lean initrd was made"
SZ() { echo "$(( $(stat -c %s "$1") / 1048576 )) MB"; }
echo "lean-image: initrd-lean $(SZ "$OUT/initrd-lean") (was $(SZ "$IMG/rootfs.cpio.zst"))"
[ ! -f "$OUT/root.sfs" ] || echo "lean-image: root.sfs $(SZ "$OUT/root.sfs") (zstd -$LEVEL)"
[ ! -f "$OUT/data-tree.ext4" ] || echo "lean-image: data-tree.ext4 $(SZ "$OUT/data-tree.ext4")"

# the GRUB block lean-install.sh inserts into the stick's grub.cfg: the markers' range of the board's menu, with the usual paths filled in
sed -n '/^# BEGIN lean-boot/,/^# END lean-boot/p' "$HERE/board/iyakki/grub.cfg" | sed -e 's%__KERNEL_PATH__%/boot/bzImage%' -e 's%__INITRD_PATH__%/boot/initrd%' > "$OUT/grub-lean-block.cfg"
[ -s "$OUT/grub-lean-block.cfg" ] || die "no lean block in board/iyakki/grub.cfg"
[ "$MODE" != export ] || { echo "lean-image: exported tree, manifest, grub block and initrd-lean to $OUT"; exit 0; }

# ---- the install template: bigger FAT volume with the lean files ----
BOOTFILES=$(( $(stat -c %s "$OUT/initrd-lean") + $(stat -c %s "$IMG/bzImage") + $( [ -f "$OUT/root.sfs" ] && stat -c %s "$OUT/root.sfs" || echo 0 ) ))
[ "$FATMB" -gt 0 ] || FATMB=$(( BOOTFILES / 1048576 + 96 ))
[ "$FATMB" -ge 64 ] || FATMB=64
# default cluster size: GRUB (and the firmware) take a FAT32 volume with fewer than 65525 clusters for FAT16, so bigger clusters need a bigger volume
FAT="$W/boot.vfat"; rm -f "$FAT"
[ "$MODE" = stick ] || mkfs.vfat -F 32 -n IYAKKI-BOOT -C "$FAT" $(( FATMB * 1024 )) >/dev/null
export MTOOLS_SKIP_CHECK=1
if [ "$MODE" = stick ]; then
  # a stick as the normal install makes it (full initrd as boot/initrd, normal menu), plus what lean-install.sh adds: boot/initrd-lean and the lean entries
  cp "$IMG/iyakki-boot.vfat" "$FAT"
  "$MCOPY" -i "$FAT" "$OUT/initrd-lean" ::boot/initrd-lean
  "$MCOPY" -i "$FAT" ::boot/grub/grub.cfg "$W/grub.cfg"
  python3 -I "$HERE/lean-sync.py" grubmerge "$W/grub.cfg" "$OUT/grub-lean-block.cfg"
  "$MCOPY" -o -i "$FAT" "$W/grub.cfg" ::boot/grub/grub.cfg
else
"$MMD" -i "$FAT" ::boot ::EFI >/dev/null
"$MCOPY" -i "$FAT" -s "$IMG/install-root/EFI" ::/ >/dev/null
"$MCOPY" -i "$FAT" -s "$IMG/install-root/boot/grub" ::boot/ >/dev/null
"$MCOPY" -i "$FAT" "$IMG/bzImage" ::boot/bzImage
"$MCOPY" -i "$FAT" "$OUT/initrd-lean" ::boot/initrd
[ ! -f "$OUT/root.sfs" ] || "$MCOPY" -i "$FAT" "$OUT/root.sfs" ::boot/root.sfs
fi
# disk: boot sector + GRUB core (first MiB) from the full template, then our partition table (FAT at 1 MiB, data after it)
DISK="$OUT/iyakki-lean-x86_64.img"; rm -f "$DISK"
DATA="$W/data-marker.img"; if [ -f "$OUT/data-tree.ext4" ] && [ "$MODE" != sfs ]; then DATA="$OUT/data-tree.ext4"; else dd if="$IMG/data-marker.img" of="$DATA" bs=1M 2>/dev/null || dd if=/dev/zero of="$DATA" bs=1M count=1 2>/dev/null; fi
FATB=$(stat -c %s "$FAT"); DATAB=$(stat -c %s "$DATA")
TOTAL=$(( 1048576 + FATB + DATAB + 1048576 ))
dd if="$IMG/iyakki-install-x86_64.img" of="$DISK" bs=1M count=1 2>/dev/null
truncate -s "$TOTAL" "$DISK"
dd if="$FAT" of="$DISK" bs=1M seek=1 conv=notrunc 2>/dev/null
dd if="$DATA" of="$DISK" bs=1M seek=$(( 1 + FATB / 1048576 )) conv=notrunc 2>/dev/null
sfdisk -q "$DISK" >/dev/null <<EOF
label: dos
label-id: 0x41465300
unit: sectors
start=2048, size=$(( FATB / 512 )), type=ef, bootable
start=$(( 2048 + FATB / 512 )), size=$(( DATAB / 512 )), type=83
EOF
echo "lean-image: $DISK $(SZ "$DISK") (boot volume $FATMB MB)"
