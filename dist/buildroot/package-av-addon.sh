#!/bin/bash
# Makes the audio and video add-on archive (store item ffmpeg-libs) from the FFmpeg libraries of a finished or half-finished web build.
# Never put into an image: the archive is for the person's click in Store -> Setup (docs/development/AV-ADDON-BUILD.md).
#   dist/buildroot/package-av-addon.sh [BUILDROOT_OUT] [OUT_DIR]
#   BUILDROOT_OUT  default target/buildroot-web (its target/usr/lib must hold the 6 libraries built LGPL-only: configs/iyakki_web_av_addon_libs.fragment)
#   OUT_DIR        default ~/iyakki-work/addons
# Writes OUT_DIR/ffmpeg-libs-<ffmpeg version>-linux-musl-<arch>.tar.gz (+ .sha256) and leaves the unpacked tree in OUT_DIR/staging/.
set -eu
HERE="$(cd "$(dirname "$0")" && pwd)"
ROOT="$(cd "$HERE/../.." && pwd)"
OUT="${1:-$ROOT/target/buildroot-web}"
DEST="${2:-$HOME/iyakki-work/addons}"
ARCH="${ARCH:-x86_64}"
FF="$(ls -d "$OUT"/build/ffmpeg-[0-9]* | head -n1)"
VER="${FF##*/ffmpeg-}"
TOOL="$(ls "$OUT"/host/bin/*-linux-musl-strip | head -n1)"
READELF="${TOOL%strip}readelf"
LIBS="libavcodec.so.60 libavformat.so.60 libavutil.so.58 libavfilter.so.9 libswresample.so.4 libswscale.so.7"
NAME="ffmpeg-libs-$VER"
STAGE="$DEST/staging/$NAME"
rm -rf "$DEST/staging"; mkdir -p "$STAGE/lib" "$STAGE/bin" "$STAGE/source"

# 0. refuse anything that is not LGPL-only
cfg="$FF/config.h"
grep -q '^#define CONFIG_GPL 0' "$cfg" && grep -q '^#define CONFIG_NONFREE 0' "$cfg" && grep -q '^#define CONFIG_VERSION3 0' "$cfg" \
  || { echo "package-av-addon: this FFmpeg build is not LGPL-2.1-only (config.h); refusing" >&2; exit 1; }
if grep -E 'enable-(gpl|nonfree|version3|libx264|libx265|libfdk-aac)' "$FF/ffbuild/config.log" | head -n1 | grep -q .; then
  echo "package-av-addon: the configure line has a GPL, nonfree or encoder-library option; refusing" >&2; exit 1
fi
# 1. the libraries: plain files named by soname, stripped
for s in $LIBS; do
  real="$(readlink -f "$OUT/target/usr/lib/$s")"
  [ -f "$real" ] || { echo "package-av-addon: $s is not in $OUT/target/usr/lib (build ffmpeg first)" >&2; exit 1; }
  cp "$real" "$STAGE/lib/$s"
  "$TOOL" --strip-unneeded "$STAGE/lib/$s"
  got="$("$READELF" -d "$STAGE/lib/$s" | sed -n 's/.*SONAME.*\[\(.*\)\]/\1/p')"
  [ "$got" = "$s" ] || { echo "package-av-addon: $s has soname $got" >&2; exit 1; }
  chmod 0644 "$STAGE/lib/$s"
done
# nothing but musl, zlib and the add-on's own libraries may be needed
bad="$(for s in $LIBS; do "$READELF" -d "$STAGE/lib/$s" | sed -n 's/.*NEEDED.*\[\(.*\)\]/\1/p'; done | sort -u | grep -v -E '^(libc\.so|libz\.so\.1|libav(codec|format|util|filter)\.so\.[0-9]+|libsw(scale|resample)\.so\.[0-9]+)$' || true)"
[ -z "$bad" ] || { echo "package-av-addon: unexpected dependencies: $bad" >&2; exit 1; }
# 2. the list, the check script, the licence texts
printf '%s\n' "# The FFmpeg shared libraries (sonames) this add-on provides; the image's bridge plugin needs these (usr/share/iyakki/av-addon-abi)." $LIBS > "$STAGE/av-addon-abi"
CHK="$HERE/../addons/ffmpeg-libs/iyakki-av-check"; [ -f "$CHK" ] || CHK="$HERE/iyakki-av-check"   # in the repo, or flat in /usr/share/iyakki/av-addon
cp "$CHK" "$STAGE/bin/iyakki-av-check"; chmod 0755 "$STAGE/bin/iyakki-av-check"
cp "$FF/COPYING.LGPLv2.1" "$FF/LICENSE.md" "$STAGE/"
CONF="$(sed -n "s/^FFMPEG_CONFIGURATION='\(.*\)'$/\1/p" "$FF/ffbuild/config.mak" 2>/dev/null | head -n1)"
[ -n "$CONF" ] || CONF="$(sed -n '1s/^# //p' "$FF/ffbuild/config.log")"
CONF="$(printf '%s' "$CONF" | sed "s|$OUT/host/bin/||g; s|--sysroot=[^ ]*||; s|--pkg-config=[^ ]*||; s|$ROOT|<tree>|g; s|^\./configure ||")"
PATCHES="$(ls "$ROOT"/target/buildroot-src/buildroot-*/package/ffmpeg/*.patch 2>/dev/null || true)"
for p in $PATCHES; do cp "$p" "$STAGE/source/"; done
SHA="$(sed -n 's/^sha256  \([0-9a-f]*\)  ffmpeg-'"$VER"'.tar.xz$/\1/p' "$ROOT"/target/buildroot-src/buildroot-*/package/ffmpeg/ffmpeg.hash | head -n1)"
cat > "$STAGE/NOTICE" <<EON
FFmpeg version: $VER
Licence: GNU Lesser General Public License, version 2.1 or later (LGPL-2.1+). FFmpeg was configured WITHOUT --enable-gpl, --enable-nonfree,
--enable-version3, libx264, libx265 or libfdk-aac. The libraries are shared (dynamically linked), so you may replace them with your own build
of the same FFmpeg version (the same sonames, see av-addon-abi).
Built with: Buildroot (musl C library), the x86-64 baseline; FFmpeg's own assembly is used for the CPU found at run time.
Decoders compiled in: h264, aac, aac_latm, mp3, mp3float (no HEVC, no encoders, no muxers, no network access).

Configure line (paths shortened):
  ./configure $CONF

Source: FFmpeg $VER, https://ffmpeg.org/releases/ffmpeg-$VER.tar.xz (sha256 $SHA), plus the patches of Buildroot's package in source/.
See SOURCE-OFFER.txt. The licence texts are COPYING.LGPLv2.1 and LICENSE.md.

Patents: H.264 and AAC are formats covered by patents in some countries. IYAKKI does not ship these libraries in any image; they are
downloaded because you pressed Install, and decoding with them is your own choice and responsibility. Remove them in Store -> Setup.
EON
cat > "$STAGE/SOURCE-OFFER.txt" <<EOS
Written offer for the corresponding source of the FFmpeg libraries in this folder (LGPL-2.1+ section 6).

The complete source of the libraries is the FFmpeg $VER release tarball from the FFmpeg project (https://ffmpeg.org/releases/ffmpeg-$VER.tar.xz,
sha256 $SHA) with the patches in the source/ folder applied (they come from Buildroot's ffmpeg package and fix build problems of other
configurations), configured with the line in NOTICE and built with the Buildroot toolchain (GCC, musl). The IYAKKI project will give
the exact tarball, the patches and its build scripts (dist/buildroot/ in the IYAKKI source tree) on request to anyone who received
these libraries, for at least three years after the release that carried them, for the cost of delivering them.
EOS
find "$STAGE" -type d -exec chmod 0755 {} +; find "$STAGE" -type f ! -path "*/bin/*" -exec chmod 0644 {} +
# 3. the archive: fixed order, fixed times, no owners, so the same input gives the same bytes
(cd "$DEST/staging" && find "$NAME" -type f -o -type d | LC_ALL=C sort > ../.list && tar --owner=0 --group=0 --numeric-owner --mtime='2026-01-01 00:00:00' --no-recursion -T ../.list -cf - | gzip -n -9 > "$DEST/$NAME-linux-musl-$ARCH.tar.gz")
rm -f "$DEST/.list"
( cd "$DEST" && sha256sum "$NAME-linux-musl-$ARCH.tar.gz" > "$NAME-linux-musl-$ARCH.tar.gz.sha256" )
ls -l "$DEST/$NAME-linux-musl-$ARCH.tar.gz"; cat "$DEST/$NAME-linux-musl-$ARCH.tar.gz.sha256"
