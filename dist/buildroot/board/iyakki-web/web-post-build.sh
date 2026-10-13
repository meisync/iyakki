#!/bin/sh
# Extra Buildroot post-build hook for the optional "web" variant only. $1 = target rootfs dir.
set -eu
TARGET_DIR="$1"
REPO="${BR2_EXTERNAL_IYAKKI_PATH}/../.."

# Scripts the shell already ships (SIL OFL Noto fonts) so the full browser can show them too.
FONTS="$TARGET_DIR/usr/share/fonts/noto"
mkdir -p "$FONTS"
for f in "$REPO"/crates/iyakki-shell/assets/NotoSans*.ttf; do
  install -m 0644 "$f" "$FONTS/"
done
install -m 0644 "$REPO/crates/iyakki-shell/assets/NOTO-OFL-LICENSE.txt" "$FONTS/"

# Codec policy: gst-libav (FFmpeg: H.264, AAC, MP3...) is pulled in by WPE's multimedia option, but it ships
# only when the build was asked for it (WEB_H264=1). VP9, AV1, Opus and Vorbis come from libvpx, dav1d, opus, vorbis.
if [ "${IYAKKI_WEB_H264:-0}" != 1 ]; then
  rm -f "$TARGET_DIR"/usr/lib/gstreamer-1.0/libgstlibav.so
  rm -f "$TARGET_DIR"/usr/lib/libavcodec.so* "$TARGET_DIR"/usr/lib/libavformat.so* "$TARGET_DIR"/usr/lib/libavutil.so* \
        "$TARGET_DIR"/usr/lib/libavfilter.so* "$TARGET_DIR"/usr/lib/libswscale.so* "$TARGET_DIR"/usr/lib/libswresample.so* \
        "$TARGET_DIR"/usr/lib/libpostproc.so*
  echo "web-post-build: H.264/AAC (gst-libav, FFmpeg) removed from the image (WEB_H264=1 keeps it)"
else
  echo "web-post-build: H.264/AAC (gst-libav, FFmpeg) KEPT because WEB_H264=1"
fi

# The browser's persistent profiles live on the data volume (the launcher creates /var/lib/iyakki/web-profile/<user>).
mkdir -p "$TARGET_DIR/var/lib/iyakki/web-profile"
chmod 0700 "$TARGET_DIR/var/lib/iyakki/web-profile"

# WebKit's processes ask D-Bus for a machine id (a missing one only logs a warning, but keep the log clean).
if [ ! -s "$TARGET_DIR/etc/machine-id" ]; then
  echo "0d1e2f3a4b5c4d6e8f90a1b2c3d4e5f6" > "$TARGET_DIR/etc/machine-id"
fi

# A <video src=...> (the MEDIA app's video player, any site's plain video) is buffered by GStreamer's downloadbuffer into
# /var/tmp/WebKit-Media-XXXXXX; without the folder every plain video fails with "Could not create temp file" (code 4 in the page).
# It is in RAM on the live image, so a video larger than the free memory cannot be played that way.
mkdir -p "$TARGET_DIR/var/tmp"
chmod 1777 "$TARGET_DIR/var/tmp"
