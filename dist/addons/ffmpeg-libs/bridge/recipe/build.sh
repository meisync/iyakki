#!/bin/sh
# Runs INSIDE the build container (docker.io/library/alpine@sha256:..., see versions.map). Builds the six FFmpeg libraries the IYAKKI browser bridge
# plugin needs, LGPL only, and packs them as ffmpeg-libs-<version>-linux-musl-<arch>.tar.gz.
#   build.sh <ffmpeg-version> <source-sha256> <sonames comma separated>      (the source tarball is downloaded here; /out receives the package)
# Exit codes: 2 usage, 3 checksum differs, 4 not LGPL-only, 5 an expected library is missing or has unexpected dependencies.
set -eu
VER="${1:?ffmpeg version}"; SHA="${2:?source sha256}"; SONAMES="$(printf '%s' "${3:?sonames}" | tr ',' ' ')"
HERE="$(cd "$(dirname "$0")" && pwd)"
WORK="${WORK:-/work}"; OUT="${OUT:-/out}"
export SOURCE_DATE_EPOCH="${SOURCE_DATE_EPOCH:-1767225600}"
ARCH="$(uname -m)"
URL="https://ffmpeg.org/releases/ffmpeg-$VER.tar.xz"

mkdir -p "$WORK" "$OUT"
apk add --no-cache build-base nasm xz zlib-dev curl binutils tar gzip >/dev/null
cd "$WORK"
echo "[1/5] downloading $URL"
curl -fsSL --proto '=https' --tlsv1.2 -o "ffmpeg-$VER.tar.xz" "$URL"
got="$(sha256sum "ffmpeg-$VER.tar.xz" | cut -d' ' -f1)"
if [ "$got" != "$SHA" ]; then
  echo "checksum of ffmpeg-$VER.tar.xz differs: expected $SHA, got $got" >&2; exit 3
fi
echo "[2/5] unpacking"
tar xf "ffmpeg-$VER.tar.xz"
cd "ffmpeg-$VER"
if [ -d "$HERE/patches" ]; then for p in "$HERE"/patches/*.patch; do [ -f "$p" ] && patch -p1 < "$p"; done; fi
echo "[3/5] configuring (LGPL only)"
# shellcheck disable=SC2046
./configure --prefix=/usr $(cat "$HERE/configure-args.txt") >/dev/null
grep -q '^#define CONFIG_GPL 0' config.h && grep -q '^#define CONFIG_NONFREE 0' config.h && grep -q '^#define CONFIG_VERSION3 0' config.h \
  || { echo "this build is not LGPL-2.1-only; refusing" >&2; exit 4; }
echo "[4/5] building"
make -j"$(nproc)" >/dev/null
make DESTDIR="$WORK/inst" install >/dev/null

echo "[5/5] packing"
NAME="ffmpeg-libs-$VER"; STAGE="$WORK/stage/$NAME"
rm -rf "$WORK/stage"; mkdir -p "$STAGE/lib" "$STAGE/bin" "$STAGE/source"
for s in $SONAMES; do
  real="$(readlink -f "$WORK/inst/usr/lib/$s")"
  [ -f "$real" ] || { echo "$s was not built" >&2; exit 5; }
  cp "$real" "$STAGE/lib/$s"; strip --strip-unneeded "$STAGE/lib/$s"; chmod 0644 "$STAGE/lib/$s"
  got="$(readelf -d "$STAGE/lib/$s" | sed -n 's/.*SONAME.*\[\(.*\)\]/\1/p')"
  [ "$got" = "$s" ] || { echo "$s has the soname $got" >&2; exit 5; }
done
bad="$(for s in $SONAMES; do readelf -d "$STAGE/lib/$s" | sed -n 's/.*NEEDED.*\[\(.*\)\]/\1/p'; done | sort -u | grep -v -E '^(libc\.musl-[a-z0-9_]+\.so\.1|libc\.so|libz\.so\.1|libav(codec|format|util|filter)\.so\.[0-9]+|libsw(scale|resample)\.so\.[0-9]+)$' || true)"
[ -z "$bad" ] || { echo "unexpected dependencies: $bad" >&2; exit 5; }
{ echo "# The FFmpeg shared libraries (sonames) this add-on provides; the image's bridge plugin needs these (usr/share/iyakki/av-addon-abi)."; for s in $SONAMES; do echo "$s"; done; } > "$STAGE/av-addon-abi"
cp "$HERE/files/iyakki-av-check" "$STAGE/bin/iyakki-av-check"; chmod 0755 "$STAGE/bin/iyakki-av-check"
cp COPYING.LGPLv2.1 LICENSE.md "$STAGE/"
if ls "$HERE"/patches/*.patch >/dev/null 2>&1; then cp "$HERE"/patches/*.patch "$STAGE/source/"; fi
cat > "$STAGE/NOTICE" <<EON
FFmpeg version: $VER
Licence: GNU Lesser General Public License, version 2.1 or later (LGPL-2.1+). FFmpeg was configured WITHOUT --enable-gpl, --enable-nonfree,
--enable-version3, libx264, libx265 or libfdk-aac. The libraries are shared (dynamically linked), so you may replace them with your own build
of the same FFmpeg version (the same sonames, see av-addon-abi).
Built on this machine by iyakki-media-bridge (musl C library, Alpine builder container), the x86-64 baseline; FFmpeg's own assembly is used for the CPU found at run time.
Decoders compiled in: h264, aac, aac_latm, mp3, mp3float (no HEVC, no encoders, no muxers, no network access).

Configure line:
  ./configure --prefix=/usr $(tr '\n' ' ' < "$HERE/configure-args.txt")

Source: FFmpeg $VER, $URL (sha256 $SHA)$(if ls "$HERE"/patches/*.patch >/dev/null 2>&1; then echo ", plus the patches in source/"; else echo ", unpatched"; fi).
See SOURCE-OFFER.txt. The licence texts are COPYING.LGPLv2.1 and LICENSE.md.

Patents: H.264 and AAC are formats covered by patents in some countries. These libraries were built because you pressed Build; decoding with them is your own choice and responsibility.
Remove them in Store -> Setup.
EON
cat > "$STAGE/SOURCE-OFFER.txt" <<EOO
The corresponding source of the FFmpeg libraries in this folder is the FFmpeg $VER release tarball from the FFmpeg project
($URL, sha256 $SHA), configured with the line in NOTICE. These libraries were built on your own machine from that tarball, which your machine
downloaded from the FFmpeg project at your request; the iyakki-media-bridge repository holds the build recipe and the version map.
EOO
PKG="$OUT/$NAME-linux-musl-$ARCH.tar.gz"
( cd "$WORK/stage" && tar --sort=name --owner=0 --group=0 --numeric-owner --mtime="@$SOURCE_DATE_EPOCH" -cf - "$NAME" ) | gzip -n -9 > "$PKG"
( cd "$OUT" && sha256sum "$(basename "$PKG")" > "$(basename "$PKG").sha256" )
echo "done: $PKG"
