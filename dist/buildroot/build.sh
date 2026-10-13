#!/bin/sh
# Build an IYAKKI image.   [ARCH=x86_64|aarch64] ./build.sh [libre|hw|web] [output-dir]
#   libre: no proprietary firmware (default).  hw: adds linux-firmware blobs for WiFi/NIC/GPU.
#   web: x86_64 only, OPTIONAL: adds the full web engine (WPE WebKit + the iyakki-webview-helper, Mesa, GStreamer).
#        Output in target/buildroot-web. WEB_H264=1 also ships H.264/AAC. See docs/FULL-BROWSER.md.
#   x86_64 (default): BIOS+UEFI hybrid ISO.  aarch64: UEFI disk image for VMs on Apple Silicon Macs / ARM64 boards.
# Needs: rustup + musl-tools (static iyakki), and Buildroot's host deps (see its manual).
set -eu
VARIANT="${1:-libre}"
ARCH="${ARCH:-x86_64}"
HERE="$(cd "$(dirname "$0")" && pwd)"
ROOT="$(cd "$HERE/../.." && pwd)"
case "$ARCH" in
  x86_64)  RUST_TARGET=x86_64-unknown-linux-musl ;  OUT="${2:-$ROOT/target/buildroot-$VARIANT}" ;;
  aarch64) RUST_TARGET=aarch64-unknown-linux-musl ; OUT="${2:-$ROOT/target/buildroot-$VARIANT-aarch64}" ;;
  *) echo "ARCH must be x86_64 or aarch64" >&2; exit 2 ;;
esac
BR_VERSION="${BR_VERSION:-2026.02.3}"
BR_DIR="${BR_DIR:-$ROOT/target/buildroot-src/buildroot-$BR_VERSION}"

if [ ! -d "$BR_DIR" ]; then
  mkdir -p "$(dirname "$BR_DIR")"
  curl -fL "https://buildroot.org/downloads/buildroot-$BR_VERSION.tar.gz" | tar xz -C "$(dirname "$BR_DIR")"
fi

mkdir -p "$OUT"
cp "$HERE/configs/iyakki_${ARCH}_defconfig" "$OUT/defconfig"
[ "$VARIANT" = "hw" ] && cat "$HERE/configs/iyakki_firmware.fragment" >> "$OUT/defconfig"
if [ "$VARIANT" = "web" ]; then
  [ "$ARCH" = "x86_64" ] || { echo "the web variant is x86_64 only for now" >&2; exit 2; }
  cat "$HERE/configs/iyakki_web.fragment" >> "$OUT/defconfig"
  if [ "${WEB_IRIS:-0}" = 1 ]; then cat "$HERE/configs/iyakki_web_iris.fragment" >> "$OUT/defconfig"; fi
  if [ "${WEB_H264:-0}" = 1 ]; then cat "$HERE/configs/iyakki_web_h264.fragment" >> "$OUT/defconfig"; fi
  export IYAKKI_WEB_H264="${WEB_H264:-0}"
fi
make -C "$BR_DIR" O="$OUT" BR2_EXTERNAL="$HERE" BR2_DEFCONFIG="$OUT/defconfig" defconfig

rustup target add "$RUST_TARGET"
if [ "$ARCH" = "aarch64" ]; then
  # cross-compile with the toolchain Buildroot just built (the same musl the image will use)
  make -C "$BR_DIR" O="$OUT" toolchain
  CROSS="$(ls "$OUT"/host/bin/aarch64-*-linux-musl-gcc | head -n1)"
  PREFIX="${CROSS%gcc}"
  export CARGO_TARGET_AARCH64_UNKNOWN_LINUX_MUSL_LINKER="$CROSS"
  export CC_aarch64_unknown_linux_musl="$CROSS"
  export AR_aarch64_unknown_linux_musl="${PREFIX}ar"
fi
# IYAKKI_PROFILE=fast: quicker build for testing (uses every core, bigger binary); the default is the full release build
PROFILE="${IYAKKI_PROFILE:-release}"
if [ "$PROFILE" = release ]; then PFLAG=--release; else PFLAG="--profile $PROFILE"; fi
(cd "$ROOT" && cargo build $PFLAG --target "$RUST_TARGET" -p iyakki-core)
export IYAKKI_BIN="$ROOT/target/$RUST_TARGET/$PROFILE/iyakki"

# JOBS also caps the compilers started inside packages (Buildroot reads BR2_JLEVEL for those): WebKit needs ~2-3 GB per compiler
JL="${JOBS:-$(nproc)}"
make -C "$BR_DIR" O="$OUT" BR2_JLEVEL="$JL" -j"$JL"
# Corresponding source + license manifest for every GPL/LGPL component in the image:
make -C "$BR_DIR" O="$OUT" legal-info
if [ "$ARCH" = "aarch64" ]; then echo "disk image: $OUT/images/iyakki-aarch64.img"; else echo "ISO:        $OUT/images/rootfs.iso9660"; fi
echo "legal info: $OUT/legal-info/"
