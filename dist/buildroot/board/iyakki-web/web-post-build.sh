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

# Codec policy (docs/FULL-BROWSER.md): gst-libav (GStreamer's LGPL glue to FFmpeg: H.264, AAC, MP3...) is pulled in by WPE's multimedia
# option, so it is always BUILT. What the image carries depends on the switches (build.sh exports them):
#   WEB_H264=1       gst-libav and FFmpeg's libraries ship and are active (the old, whole-codec image).
#   WEB_AV_ADDON=1   (the default) only the INACTIVE bridge ships: libgstlibav.so moves to /usr/lib/iyakki-av/gstreamer-1.0, a folder GStreamer
#                    does not scan, and every FFmpeg library and program is removed. The person downloads the libraries from their vendor
#                    (Store -> Setup); then the shell adds the folder and the library path to the web engine's environment (avaddon.rs).
#                    /usr/share/iyakki/av-addon-abi lists the sonames the plugin was linked against: the download must provide them.
#   neither          gst-libav and FFmpeg are removed (VP9, AV1, Opus and Vorbis come from libvpx, dav1d, opus, vorbis).
AVDIR="$TARGET_DIR/usr/lib/iyakki-av/gstreamer-1.0"
PLUGIN="$TARGET_DIR/usr/lib/gstreamer-1.0/libgstlibav.so"
remove_ffmpeg() {
  # a half-removed FFmpeg is worse than none: /usr/bin/ffmpeg without its libraries made the MEDIA tile pick it and fail with
  # "Error relocating ...: symbol not found". Remove the programs, every library and the data with them.
  rm -f "$TARGET_DIR"/usr/lib/libavcodec.so* "$TARGET_DIR"/usr/lib/libavformat.so* "$TARGET_DIR"/usr/lib/libavutil.so* \
        "$TARGET_DIR"/usr/lib/libavfilter.so* "$TARGET_DIR"/usr/lib/libswscale.so* "$TARGET_DIR"/usr/lib/libswresample.so* \
        "$TARGET_DIR"/usr/lib/libpostproc.so* "$TARGET_DIR"/usr/lib/libavdevice.so* \
        "$TARGET_DIR"/usr/bin/ffmpeg "$TARGET_DIR"/usr/bin/ffprobe "$TARGET_DIR"/usr/bin/ffplay
  rm -rf "$TARGET_DIR"/usr/share/ffmpeg "$TARGET_DIR"/usr/lib/pkgconfig/libav*.pc "$TARGET_DIR"/usr/lib/pkgconfig/libsw*.pc
}
if [ "${IYAKKI_WEB_H264:-0}" = 1 ]; then
  echo "web-post-build: H.264/AAC (gst-libav, FFmpeg) KEPT because WEB_H264=1"
elif [ "${IYAKKI_WEB_AV_ADDON:-0}" = 1 ]; then
  mkdir -p "$TARGET_DIR/usr/share/iyakki"
  if [ -f "$PLUGIN" ]; then
    # the sonames the plugin needs: FFmpeg's libraries of this build (libavcodec.so.60 ...), read before they are removed
    {
      echo "# The FFmpeg shared libraries (sonames) the bridge plugin was linked against; the audio and video add-on must provide them."
      echo "# Written by web-post-build.sh from the libraries of this build."
      ls "$TARGET_DIR"/usr/lib 2>/dev/null | grep -E '^lib(avcodec|avformat|avutil|avfilter|swscale|swresample|postproc)\.so\.[0-9]+$' | LC_ALL=C sort -u
    } > "$TARGET_DIR/usr/share/iyakki/av-addon-abi"
    mkdir -p "$AVDIR"
    mv -f "$PLUGIN" "$AVDIR/libgstlibav.so"
    echo "web-post-build: gst-libav moved to /usr/lib/iyakki-av (inactive); sonames: $(grep -v '^#' "$TARGET_DIR/usr/share/iyakki/av-addon-abi" | tr '\n' ' ')"
  elif [ -f "$AVDIR/libgstlibav.so" ]; then
    echo "web-post-build: gst-libav already in /usr/lib/iyakki-av (repack)"
  else
    # a rootfs that never had the plugin (a base built before WEB_AV_ADDON, or a repack of one): it cannot be added here
    rm -f "$TARGET_DIR/usr/share/iyakki/av-addon-abi"
    echo "web-post-build: WARNING WEB_AV_ADDON=1 but no gst-libav in this rootfs; the image has no audio and video bridge. Do a real build (make gst1-libav-reinstall first, docs/FULL-BROWSER.md)"
  fi
  if [ "${WITH_FFMPEG:-0}" = 1 ]; then
    echo "web-post-build: ffmpeg and its libraries KEPT because WITH_FFMPEG=1"
  else
    remove_ffmpeg
  fi
else
  rm -f "$PLUGIN"
  if [ "${WITH_FFMPEG:-0}" = 1 ]; then
    # the media fragment asked for ffmpeg/ffprobe (the MEDIA tile's own player): they need their libraries, keep them
    echo "web-post-build: gst-libav removed; ffmpeg and its libraries KEPT because WITH_FFMPEG=1"
  else
    remove_ffmpeg
    echo "web-post-build: H.264/AAC (gst-libav, FFmpeg programs and libraries) removed from the image (WEB_AV_ADDON=1 keeps an inactive bridge, WEB_H264=1 the whole codec)"
  fi
fi

# The add-on RECIPE (the image never carries FFmpeg; the administrator supplies the libraries, docs/development/AV-ADDON-BUILD.md): the script
# that makes the archive, the Buildroot option fragment, the check tool and a README. /local is a link to the data volume's local folder,
# where the installer looks for ffmpeg-libs-*-linux-musl-<arch>.tar.gz (the agent creates local/packages at first use).
if [ "${IYAKKI_WEB_AV_ADDON:-0}" = 1 ]; then
  RECIPE="$TARGET_DIR/usr/share/iyakki/av-addon"
  mkdir -p "$RECIPE" "$TARGET_DIR/usr/bin"
  install -m 0755 "$REPO/dist/buildroot/package-av-addon.sh" "$RECIPE/package-av-addon.sh"
  install -m 0755 "$REPO/dist/addons/ffmpeg-libs/iyakki-av-check" "$RECIPE/iyakki-av-check"
  install -m 0644 "$REPO/dist/buildroot/configs/iyakki_web_av_addon_libs.fragment" "$RECIPE/iyakki_web_av_addon_libs.fragment"
  install -m 0644 "$REPO/dist/addons/ffmpeg-libs/README" "$RECIPE/README"
  install -m 0644 "$REPO/dist/addons/ffmpeg-libs/LICENSES" "$RECIPE/LICENSES"
  ln -sf ../share/iyakki/av-addon/iyakki-av-check "$TARGET_DIR/usr/bin/iyakki-av-check"
  # the version map and the build recipe of the iyakki-media-bridge repository (copied there by dist/addons/ffmpeg-libs/sync-bridge.sh, never fetched
  # here): the agent's "Build it here (k3s)" reads versions.map for this image's IYAKKI version and hands recipe/ to the build Job after checking RECIPE.SHA256
  mkdir -p "$RECIPE/bridge/recipe/files"
  install -m 0644 "$REPO/dist/addons/ffmpeg-libs/bridge/versions.map" "$RECIPE/bridge/versions.map"
  install -m 0644 "$REPO/dist/addons/ffmpeg-libs/bridge/RECIPE.SHA256" "$RECIPE/bridge/RECIPE.SHA256"
  install -m 0644 "$REPO/dist/addons/ffmpeg-libs/bridge/BRIDGE-SOURCE" "$RECIPE/bridge/BRIDGE-SOURCE"
  install -m 0644 "$REPO/dist/addons/ffmpeg-libs/bridge/recipe/build.sh" "$RECIPE/bridge/recipe/build.sh"
  install -m 0644 "$REPO/dist/addons/ffmpeg-libs/bridge/recipe/configure-args.txt" "$RECIPE/bridge/recipe/configure-args.txt"
  install -m 0644 "$REPO/dist/addons/ffmpeg-libs/bridge/recipe/files/iyakki-av-check" "$RECIPE/bridge/recipe/files/iyakki-av-check"
  ln -sfn var/lib/iyakki/local "$TARGET_DIR/local"
fi
# nothing of FFmpeg's own may remain in the image unless a switch asked for it (checked by dist/buildroot/test-av-addon.sh)

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
