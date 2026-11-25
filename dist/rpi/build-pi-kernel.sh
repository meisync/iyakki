#!/bin/sh
# Builds the Raspberry Pi 5 kernel from the vendor tree at the pinned tag (dist/rpi/kernel.pin) with the IYAKKI fragments, WITHOUT a full Buildroot
# run (the aarch64 userland is reused as it is: incremental builds only). Linux build host only; the kernel source is fetched here at build time
# and is never added to the repository.
#   dist/rpi/build-pi-kernel.sh OUT_DIR [--tarball FILE] [--cross PREFIX] [--jobs N] [--work DIR]
#   --cross  the cross compiler prefix, for example host/bin/aarch64-buildroot-linux-musl- of an existing aarch64 Buildroot build
#   --work   where the source is unpacked and built (default: $TMPDIR or /var/tmp; a real disk, not a RAM-backed /tmp)
# Writes to OUT_DIR: kernel8.img (the arm64 Image), bcm2712-rpi-5-b.dtb, overlays/vc4-kms-v3d-pi5.dtbo, modroot/ (lib/modules/<release>, stripped),
# kernel.config (the resolved options), kernel.release, kernel.sha256.
set -eu
HERE="$(cd "$(dirname "$0")" && pwd)"
. "$HERE/kernel.pin"
OUT="${1:?output dir}"; shift
TARBALL=""; CROSS="${CROSS_COMPILE:-}"; JOBS="$(nproc)"; WORK="${TMPDIR:-/var/tmp}"
while [ $# -gt 0 ]; do
  case "$1" in --tarball) TARBALL="$2"; shift 2 ;; --cross) CROSS="$2"; shift 2 ;; --jobs) JOBS="$2"; shift 2 ;; --work) WORK="$2"; shift 2 ;;
    *) echo "build-pi-kernel: unknown option $1" >&2; exit 2 ;; esac
done
[ "$JOBS" -le 16 ] || JOBS=16   # the build host runs other agents: never more than 16 compilers
mkdir -p "$OUT" "$WORK"; OUT="$(cd "$OUT" && pwd)"; WORK="$(cd "$WORK" && pwd)"
if [ -z "$TARBALL" ]; then
  TARBALL="$WORK/linux-$TAG.tar.gz"
  [ -s "$TARBALL" ] || curl -fsSL --retry 3 -o "$TARBALL" "$URL"
fi
got="$(sha256sum "$TARBALL" | cut -d' ' -f1)"
[ "$got" = "$SHA256" ] || { echo "build-pi-kernel: $TARBALL does not match the pinned checksum ($got, pinned $SHA256); not building" >&2; exit 1; }
SRC="$WORK/linux-$TAG"
if [ ! -f "$SRC/.iyakki-unpacked" ]; then
  rm -rf "$SRC"; mkdir -p "$SRC"; tar -xzf "$TARBALL" -C "$SRC" --strip-components=1; : > "$SRC/.iyakki-unpacked"
fi
export KBUILD_BUILD_USER=iyakki KBUILD_BUILD_HOST=iyakki-build   # never a person's name in the version banner
sh "$HERE/verify-pi-kernel-config.sh" "$SRC" "$CROSS"
cd "$SRC"
M="make ARCH=arm64 ${CROSS:+CROSS_COMPILE=$CROSS}"
$M -j"$JOBS" Image dtbs modules
REL="$($M -s kernelrelease)"
cp arch/arm64/boot/Image "$OUT/kernel8.img"
cp "arch/arm64/boot/dts/$DTB" "$OUT/"
mkdir -p "$OUT/overlays"; cp "arch/arm64/boot/dts/overlays/$OVERLAY" "$OUT/overlays/"
rm -rf "$OUT/modroot"; $M -s modules_install INSTALL_MOD_PATH="$OUT/modroot" INSTALL_MOD_STRIP=1
rm -f "$OUT"/modroot/lib/modules/*/build "$OUT"/modroot/lib/modules/*/source   # symlinks into this work folder
cp .config "$OUT/kernel.config"; printf '%s\n' "$REL" > "$OUT/kernel.release"
( cd "$OUT" && sha256sum kernel8.img "$(basename "$DTB")" "overlays/$OVERLAY" > kernel.sha256 )
echo "build-pi-kernel: $REL: kernel8.img $(( $(stat -c %s "$OUT/kernel8.img") / 1048576 )) MB, modules $(du -sm "$OUT/modroot" | cut -f1) MB, in $OUT"
