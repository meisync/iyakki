#!/bin/sh
# The RELEASE image profile (IYAKKI_IMAGE_PROFILE=release): what the base image must NOT contain, applied to a root file system tree
# and checked afterwards.   docs/development/RELEASE-IMAGE-PROFILE.md
#
#   release-profile.sh apply  TARGET_DIR   remove vendor firmware and codec leftovers, install the release files, stamp the image, verify
#   release-profile.sh verify TARGET_DIR   only check (also works on an unpacked rootfs.cpio); exit 1 and list every problem
#   release-profile.sh env                 only check the environment switches (exit 1 when one asks for something a release must not have)
#
# Called by post-build.sh (so by Buildroot, by repack.sh and by build.sh). No network, nothing written outside TARGET_DIR.
# Switches (all optional):
#   IYAKKI_RELEASE_AV_BRIDGE=0   also drop the INACTIVE gst-libav bridge (/usr/lib/iyakki-av); default 1 keeps it parked (owner decision
#                                2026-10-23, `except` row in legal/image-rules.txt): it is LGPL glue code without any codec.
set -eu
HERE="$(cd "$(dirname "$0")" && pwd)"
cmd="${1:-}"; [ $# -eq 0 ] || shift

env_check() {
  bad=0
  [ "${IYAKKI_INCLUDE_RESTRICTED_FIRMWARE:-0}" != 1 ] || { echo "release-profile: IYAKKI_INCLUDE_RESTRICTED_FIRMWARE=1 is a build for the builder's own use and cannot be a release image" >&2; bad=1; }
  [ "${IYAKKI_WEB_H264:-${WEB_H264:-0}}" != 1 ] || { echo "release-profile: WEB_H264=1 ships H.264/AAC (gst-libav active, FFmpeg libraries); a release image does not" >&2; bad=1; }
  [ "${WITH_FFMPEG:-0}" != 1 ] || { echo "release-profile: WITH_FFMPEG=1 ships ffmpeg and ffprobe; a release image does not (the person downloads them)" >&2; bad=1; }
  [ "${WEB_MEDIA:-0}" != 1 ] || echo "release-profile: note: WEB_MEDIA=1 adds camera/WebRTC plugins; their licences are not reviewed for a release (docs/development/RELEASE-IMAGE-PROFILE.md)" >&2
  [ "$bad" = 0 ] || exit 1
}

# every problem on its own line; none printed means the tree is a release tree
problems() {
  T="$1"
  # 1. firmware: nothing but the regulatory database
  if [ -d "$T/lib/firmware" ]; then
    find "$T/lib/firmware" \( -type f -o -type l \) 2>/dev/null | sed "s#^$T/lib/firmware/##" | grep -v -x -e 'regulatory.db' -e 'regulatory.db.p7s' | sed 's#^#vendor firmware file in the image: lib/firmware/#' | head -n 20
  fi
  [ ! -e "$T/usr/share/doc/linux-firmware" ] || echo "linux-firmware licence texts without firmware: usr/share/doc/linux-firmware"
  [ ! -e "$T/usr/share/doc/linux-firmware-restricted" ] || echo "restricted-firmware stamp in the image: usr/share/doc/linux-firmware-restricted"
  # 2. codecs: no FFmpeg, no encoder libraries, no active codec bridge
  for p in usr/bin/ffmpeg usr/bin/ffprobe usr/bin/ffplay; do [ ! -e "$T/$p" ] || echo "codec program in the image: $p"; done
  for g in libavcodec libavformat libavutil libavfilter libswscale libswresample libpostproc libavdevice libx264 libx265 libopenh264 libde265 libfdk-aac libfaad libwidevine; do
    ls "$T"/usr/lib/$g.so* >/dev/null 2>&1 && echo "codec library in the image: usr/lib/$g.so*"
  done
  ls "$T"/usr/lib/gstreamer-1.0/libgstlibav* >/dev/null 2>&1 && echo "active codec bridge: usr/lib/gstreamer-1.0/libgstlibav.so (must be parked in usr/lib/iyakki-av or absent)"
  for g in libgstx264 libgstx265 libgstopenh264 libgstfaad libgstfdkaac libgstmpegpsmux libgstmpeg2enc libgstdvb; do
    ls "$T"/usr/lib/gstreamer-1.0/$g* >/dev/null 2>&1 && echo "unreviewed or encumbered GStreamer plugin: usr/lib/gstreamer-1.0/$g*"
  done
  if [ "${IYAKKI_RELEASE_AV_BRIDGE:-1}" = 0 ]; then [ ! -e "$T/usr/lib/iyakki-av" ] || echo "usr/lib/iyakki-av is present but IYAKKI_RELEASE_AV_BRIDGE=0"; fi
  # 3. the release files and the stamp
  for f in usr/libexec/iyakki/gpu-fw-guard etc/modprobe.d/iyakki-release.conf usr/share/iyakki/firmware-notice.txt usr/share/iyakki/firmware-notice.long.txt; do
    [ -e "$T/$f" ] || echo "release file missing: $f"
  done
  [ -x "$T/usr/libexec/iyakki/gpu-fw-guard" ] || echo "release file not executable: usr/libexec/iyakki/gpu-fw-guard"
  grep -qx 'IMAGE_PROFILE=release' "$T/etc/iyakki-release" 2>/dev/null || echo "etc/iyakki-release lacks IMAGE_PROFILE=release"
  grep -qx 'FIRMWARE=none' "$T/etc/iyakki-release" 2>/dev/null || echo "etc/iyakki-release lacks FIRMWARE=none"
  return 0
}

verify() {
  T="$1"; [ -d "$T" ] || { echo "release-profile: no tree $T" >&2; exit 2; }
  out="$(problems "$T")"
  if [ -n "$out" ]; then echo "$out" | sed 's/^/release-profile: PROBLEM: /' >&2; exit 1; fi
  echo "release-profile: $T is a release tree (no vendor firmware, no codec libraries, bridge $( [ -d "$T/usr/lib/iyakki-av" ] && echo parked || echo absent ))"
}

case "$cmd" in
  env) env_check ;;
  verify) verify "${1:?usage: release-profile.sh verify TARGET_DIR}" ;;
  apply)
    T="${1:?usage: release-profile.sh apply TARGET_DIR}"
    env_check
    # vendor firmware: only the keep list's files stay (default: the regulatory database of the wireless-regdb package)
    sh "$HERE/prune-firmware.sh" "$HERE/release-firmware.keep" "$T/lib/firmware"
    rm -rf "$T/usr/share/doc/linux-firmware" "$T/usr/share/doc/linux-firmware-restricted"
    # codec leftovers (web-post-build.sh removes FFmpeg for a web image; this makes the release independent of that and of the donor)
    rm -f "$T"/usr/lib/libavcodec.so* "$T"/usr/lib/libavformat.so* "$T"/usr/lib/libavutil.so* "$T"/usr/lib/libavfilter.so* \
          "$T"/usr/lib/libswscale.so* "$T"/usr/lib/libswresample.so* "$T"/usr/lib/libpostproc.so* "$T"/usr/lib/libavdevice.so* \
          "$T"/usr/bin/ffmpeg "$T"/usr/bin/ffprobe "$T"/usr/bin/ffplay
    if [ "${IYAKKI_RELEASE_AV_BRIDGE:-1}" = 0 ]; then rm -rf "$T/usr/lib/iyakki-av" "$T/usr/share/iyakki/av-addon-abi"; fi
    # the release files: GPU firmware guard, its modprobe rules, the first-boot notice texts
    cp -a "$HERE/release/." "$T/"
    chmod 0755 "$T/usr/libexec/iyakki/gpu-fw-guard"
    # the stamp the shell and the About page read (/etc/iyakki-release): FIRMWARE=none -> the first-boot notice
    REL="$T/etc/iyakki-release"
    if [ -f "$REL" ]; then
      grep -v -e '^IMAGE_PROFILE=' -e '^FIRMWARE=' -e '^CODECS=' -e '^AV_BRIDGE=' "$REL" > "$REL.new" || true
    else
      : > "$REL.new"
    fi
    {
      echo "IMAGE_PROFILE=release"
      echo "FIRMWARE=none"
      echo "CODECS=no-h264-aac"
      if [ -d "$T/usr/lib/iyakki-av" ]; then echo "AV_BRIDGE=parked"; else echo "AV_BRIDGE=none"; fi
    } >> "$REL.new"
    cat "$REL.new" > "$REL"; rm -f "$REL.new"; chmod 0644 "$REL"
    verify "$T"
    ;;
  *) echo "usage: release-profile.sh apply TARGET_DIR | verify TARGET_DIR | env" >&2; exit 2 ;;
esac
