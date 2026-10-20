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
  if [ "${WITH_FFMPEG:-0}" = 1 ]; then
    # the media fragment asked for ffmpeg/ffprobe (the MEDIA tile's own player): they need their libraries, keep them
    echo "web-post-build: gst-libav removed; ffmpeg and its libraries KEPT because WITH_FFMPEG=1"
  else
    # a half-removed FFmpeg is worse than none: /usr/bin/ffmpeg without its libraries made the MEDIA tile pick it and fail with
    # "Error relocating ...: symbol not found". Remove the programs, every library and the data with them.
    rm -f "$TARGET_DIR"/usr/lib/libavcodec.so* "$TARGET_DIR"/usr/lib/libavformat.so* "$TARGET_DIR"/usr/lib/libavutil.so* \
          "$TARGET_DIR"/usr/lib/libavfilter.so* "$TARGET_DIR"/usr/lib/libswscale.so* "$TARGET_DIR"/usr/lib/libswresample.so* \
          "$TARGET_DIR"/usr/lib/libpostproc.so* "$TARGET_DIR"/usr/lib/libavdevice.so* \
          "$TARGET_DIR"/usr/bin/ffmpeg "$TARGET_DIR"/usr/bin/ffprobe "$TARGET_DIR"/usr/bin/ffplay
    rm -rf "$TARGET_DIR"/usr/share/ffmpeg "$TARGET_DIR"/usr/lib/pkgconfig/libav*.pc "$TARGET_DIR"/usr/lib/pkgconfig/libsw*.pc
    echo "web-post-build: H.264/AAC (gst-libav, FFmpeg programs and libraries) removed from the image (WEB_H264=1 keeps them)"
  fi
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

# Tells the shell whether Mesa has an Intel GPU driver (iris/crocus, WEB_IRIS=1 or WEB_CROCUS=1). Without one, Intel machines (the Mac mini
# 2014: Haswell, which only crocus supports) run WebGL and GStreamer GL on llvmpipe, and the shell sets LIBGL_ALWAYS_SOFTWARE=1 for the
# engine (webengine.rs force_software_gl) instead of letting EGL fail on a render node without a driver.
mkdir -p "$TARGET_DIR/usr/share/iyakki"
if [ "${IYAKKI_WEB_INTEL_GL:-0}" = 1 ]; then echo intel; else echo software; fi > "$TARGET_DIR/usr/share/iyakki/web-gl-drivers"
