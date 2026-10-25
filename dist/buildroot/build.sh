#!/bin/sh
# Build an IYAKKI image.   [ARCH=x86_64|aarch64] ./build.sh [libre|hw|mac|web|macweb] [output-dir]
#   libre: no proprietary firmware (default).  hw: adds linux-firmware blobs for WiFi/NIC/GPU.
#   mac: x86_64 only, OPTIONAL, not built by default: hw + the Intel Mac drivers (applesmc, hid_apple, Cirrus audio, ...). docs/MAC-HARDWARE.md.
#   macweb: x86_64 only, OPTIONAL: mac + web in one image (Intel Macs that should run YouTube and modern sites). Output in
#        target/buildroot-macweb. Honours WEB_H264=1 and WEB_IRIS=1 like web, and WEB_CROCUS=1 (Mesa crocus for Intel gen4-7, e.g. Haswell).
#   web: x86_64 only, OPTIONAL: adds the full web engine (WPE WebKit + the iyakki-webview-helper, Mesa, GStreamer).
#        Output in target/buildroot-web. WEB_H264=1 also ships H.264/AAC. See docs/FULL-BROWSER.md.
#   WITH_FFMPEG=1 (any variant): also build ffmpeg + ffprobe (LGPL, no ffplay) into the image: configs/iyakki_media.fragment.
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
if [ "$VARIANT" = "mac" ]; then
  [ "$ARCH" = "x86_64" ] || { echo "the mac variant is x86_64 only (Apple Silicon is a separate effort: docs/MAC-HARDWARE.md)" >&2; exit 2; }
  cat "$HERE/configs/iyakki_firmware.fragment" "$HERE/configs/iyakki_mac.fragment" >> "$OUT/defconfig"
fi
if [ "$VARIANT" = "macweb" ]; then
  [ "$ARCH" = "x86_64" ] || { echo "the macweb variant is x86_64 only" >&2; exit 2; }
  # firmware + Mac drivers first, then the web engine: the Mac fragment sets the kernel fragment list, the web fragment the rootfs
  # overlay and post-build hooks; no option is set by both, so the order does not change the result
  cat "$HERE/configs/iyakki_firmware.fragment" "$HERE/configs/iyakki_mac.fragment" >> "$OUT/defconfig"
fi
if [ "$VARIANT" = "web" ] || [ "$VARIANT" = "macweb" ]; then
  [ "$ARCH" = "x86_64" ] || { echo "the $VARIANT variant is x86_64 only for now" >&2; exit 2; }
  cat "$HERE/configs/iyakki_web.fragment" >> "$OUT/defconfig"
  # crocus (Intel gen4-7: Haswell Macs) needs the same precompiled-compiler pieces as iris, so WEB_CROCUS=1 implies the iris fragment
  if [ "${WEB_IRIS:-0}" = 1 ] || [ "${WEB_CROCUS:-0}" = 1 ]; then cat "$HERE/configs/iyakki_web_iris.fragment" >> "$OUT/defconfig"; fi
  if [ "${WEB_CROCUS:-0}" = 1 ]; then cat "$HERE/configs/iyakki_web_crocus.fragment" >> "$OUT/defconfig"; fi
  if [ "${WEB_H264:-0}" = 1 ]; then cat "$HERE/configs/iyakki_web_h264.fragment" >> "$OUT/defconfig"; fi
  export IYAKKI_WEB_H264="${WEB_H264:-0}"
  if [ "${WEB_IRIS:-0}" = 1 ] || [ "${WEB_CROCUS:-0}" = 1 ]; then export IYAKKI_WEB_INTEL_GL=1; else export IYAKKI_WEB_INTEL_GL=0; fi
fi
# OPTIONAL, any variant: ffmpeg and ffprobe (LGPL build) for IYAKKI's own media player. docs/MEDIA-PLAYER.md.
if [ "${WITH_FFMPEG:-0}" = 1 ]; then cat "$HERE/configs/iyakki_media.fragment" >> "$OUT/defconfig"; fi
# OPTIONAL, x86_64 mac/macweb (the kernel donor of the PC image): NVIDIA's open GPU kernel modules, built out of tree (package/nvidia-open).
# Nothing proprietary: the vendor libraries and firmware are downloaded by the installer (docs/GPU-K3S.md).
if [ "${IYAKKI_NVIDIA_OPEN:-0}" = 1 ]; then
  [ "$ARCH" = "x86_64" ] || { echo "IYAKKI_NVIDIA_OPEN=1 is x86_64 only" >&2; exit 2; }
  cat "$HERE/configs/iyakki_nvidia_open.fragment" >> "$OUT/defconfig"
fi
# Shared compiler cache (docs/BUILD-INCREMENTAL.md): only for an output directory that has no build yet, or when asked for
# (IYAKKI_CCACHE=1); IYAKKI_CCACHE=0 switches it off. A finished tree is not changed behind its back.
if [ "${IYAKKI_CCACHE:-auto}" = 1 ] || { [ "${IYAKKI_CCACHE:-auto}" = auto ] && [ ! -d "$OUT/build" ]; }; then
  cat >> "$OUT/defconfig" <<EOF_CC
BR2_CCACHE=y
BR2_CCACHE_DIR="${IYAKKI_CCACHE_DIR:-$HOME/.cache/iyakki-ccache}"
BR2_CCACHE_INITIAL_SETUP="--max-size=${IYAKKI_CCACHE_SIZE:-30G}"
BR2_CCACHE_USE_BASEDIR=y
EOF_CC
elif [ -f "$OUT/.config" ] && grep -q '^BR2_CCACHE=y' "$OUT/.config"; then
  # keep it on for a tree that already has it (the defconfig is composed again every run)
  grep '^BR2_CCACHE' "$OUT/.config" | grep -v '^# ' >> "$OUT/defconfig"
fi
make -C "$BR_DIR" O="$OUT" BR2_EXTERNAL="$HERE" BR2_DEFCONFIG="$OUT/defconfig" defconfig

# Buildroot does not rebuild a package when only its options or its patches change. GRUB's built-in modules (the graphical menu)
# and the kernel's patches (board/iyakki/patches, the Mac variant) decide what the screen does at boot, so a change to either
# cleans that package for a fresh build. $1 package, $2 what must match the last build, $3 "new": also rebuild an existing build
# that has no record yet (made before this check existed).
rebuild_if_changed() {
  sum="$(printf '%s' "$2" | cksum)"
  stamp="$OUT/.iyakki-$1.stamp"
  built=no
  ls -d "$OUT/build/$1-"[0-9]* >/dev/null 2>&1 && built=yes
  if [ "$built" = yes ] && { { [ -f "$stamp" ] && [ "$(cat "$stamp")" != "$sum" ]; } || { [ ! -f "$stamp" ] && [ "$3" = new ]; }; }; then
    echo "build.sh: the $1 options or patches changed since the last build; rebuilding $1"
    make -C "$BR_DIR" O="$OUT" "$1-dirclean"
  fi
  printf '%s' "$sum" > "$stamp"
}
rebuild_if_changed grub2 "$(grep '^BR2_TARGET_GRUB2' "$OUT/.config")" new
PATCH_DIR_LINE="$(grep '^BR2_GLOBAL_PATCH_DIR' "$OUT/.config" || true)"
KPATCHES="$(cat "$HERE"/board/iyakki/patches/linux/*/*.patch 2>/dev/null || true)"
case "$PATCH_DIR_LINE" in *board/iyakki/patches*) LNEW=new ;; *) LNEW=no; KPATCHES="" ;; esac
# The Mac variant's kernel fragment counts too (Buildroot does not reconfigure the kernel when a fragment changes either).
# (Only appended for that variant, so the record of the other variants stays what it was and they are not rebuilt.)
KSTATE="$PATCH_DIR_LINE
$KPATCHES"
case "$PATCH_DIR_LINE" in *board/iyakki/patches*) KSTATE="$KSTATE
$(cat "$HERE/board/iyakki/linux-mac.fragment")" ;; esac
rebuild_if_changed linux "$KSTATE" "$LNEW"
# An out-of-tree module is built against the kernel tree: when the kernel is rebuilt, so is apple-bce (Mac variant).
if grep -q '^BR2_PACKAGE_APPLE_BCE=y' "$OUT/.config"; then
  rebuild_if_changed apple-bce "$KSTATE
$(cat "$HERE"/package/apple-bce/*.patch "$HERE"/package/apple-bce/apple-bce.mk)" no
fi

# And snd-hda-macbookpro (CS8409 sound of MacBookPro13/14): built from the kernel tree's own patch_cs8409 files, so any kernel change rebuilds it.
if grep -q '^BR2_PACKAGE_SND_HDA_MACBOOKPRO=y' "$OUT/.config"; then
  rebuild_if_changed snd-hda-macbookpro "$KSTATE
$(cat "$HERE"/package/snd-hda-macbookpro/snd-hda-macbookpro.mk "$HERE"/package/snd-hda-macbookpro/Kbuild)" no
fi

# And facetimehd (FaceTime HD camera): needs the kernel tree's videobuf2-dma-sg.c, so a kernel change rebuilds it.
if grep -q '^BR2_PACKAGE_FACETIMEHD=y' "$OUT/.config"; then
  rebuild_if_changed facetimehd "$KSTATE
$(cat "$HERE"/package/facetimehd/facetimehd.mk "$HERE"/package/facetimehd/Kbuild)" no
fi

# Same for NVIDIA's open modules: a different kernel or a new package version rebuilds them.
if grep -q '^BR2_PACKAGE_NVIDIA_OPEN=y' "$OUT/.config"; then
  rebuild_if_changed nvidia-open "$KSTATE
$(cat "$HERE"/package/nvidia-open/nvidia-open.mk)" no
fi

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
# The small static private store server goes into the image too (/usr/share/iyakki/store-server/<arch>/), so the Private store wizard
# finds it without building anything. A build that fails only costs that file: the image build goes on (post-build.sh warns).
if (cd "$ROOT" && cargo build $PFLAG --target "$RUST_TARGET" -p iyakki-store-server); then
  export IYAKKI_STORE_SERVER_BIN="$ROOT/target/$RUST_TARGET/$PROFILE/iyakki-store-server"
else
  echo "build.sh: WARNING: iyakki-store-server did not build; the image will not carry it" >&2
fi
export IYAKKI_ARCH="$ARCH"
# written to /etc/iyakki-release by the post-build hook (SYSTEM -> About)
export IYAKKI_VERSION="$(sed -n 's/^version *= *"\(.*\)"/\1/p' "$ROOT/Cargo.toml" | head -n1)"
export IYAKKI_BUILD_DATE="$(date -u +%Y-%m-%d)"
export IYAKKI_GIT_DESCRIBE="$(cd "$ROOT" && git describe --tags --always --dirty 2>/dev/null || echo unknown)"
export IYAKKI_VARIANT="$VARIANT-$ARCH"

# JOBS also caps the compilers started inside packages (Buildroot reads BR2_JLEVEL for those): WebKit needs ~2-3 GB per compiler
JL="${JOBS:-$(nproc)}"
make -C "$BR_DIR" O="$OUT" BR2_JLEVEL="$JL" -j"$JL"
# Corresponding source + license manifest for every GPL/LGPL component in the image:
make -C "$BR_DIR" O="$OUT" legal-info
if [ "$ARCH" = "aarch64" ]; then echo "disk image: $OUT/images/iyakki-aarch64.img"; else echo "ISO:        $OUT/images/rootfs.iso9660"; fi
echo "legal info: $OUT/legal-info/"
