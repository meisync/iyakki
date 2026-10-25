#!/usr/bin/env bash
# Checks what web-post-build.sh leaves in a fake rootfs for each codec switch (no Buildroot, no image build):
#   ./test-av-addon.sh        needs a scratch folder under $TMPDIR (kept tiny, removed at the end)
set -eu
HERE="$(cd "$(dirname "$0")" && pwd)"
HOOK="$HERE/board/iyakki-web/web-post-build.sh"
W="$(mktemp -d "${TMPDIR:-/tmp}/iyakki-av-test.XXXXXX")"
trap 'rm -rf "$W"' EXIT
pass=0; fail=0
ok() { pass=$((pass+1)); echo "ok   - $1"; }
bad() { fail=$((fail+1)); echo "FAIL - $1"; }
fakeroot_tree() {  # a rootfs as Buildroot leaves it before the hook
  local t="$1"; rm -rf "$t"; mkdir -p "$t/usr/lib/gstreamer-1.0" "$t/usr/bin" "$t/usr/share/ffmpeg" "$t/usr/lib/pkgconfig" "$t/etc"
  echo plugin > "$t/usr/lib/gstreamer-1.0/libgstlibav.so"; echo vp > "$t/usr/lib/gstreamer-1.0/libgstvpx.so"
  for l in avcodec.so.60 avformat.so.60 avutil.so.58 avfilter.so.9 swscale.so.7 swresample.so.4 postproc.so.57 avdevice.so.60; do
    echo lib > "$t/usr/lib/lib${l}.3.100"; ln -s "lib${l}.3.100" "$t/usr/lib/lib${l}"; ln -s "lib${l}.3.100" "$t/usr/lib/lib${l%%.so*}.so"
  done
  echo x > "$t/usr/bin/ffmpeg"; echo x > "$t/usr/bin/ffprobe"; echo x > "$t/usr/lib/pkgconfig/libavcodec.pc"
}
run() { # tree, then NAME=VALUE switches
  local t="$1"; shift
  env BR2_EXTERNAL_IYAKKI_PATH="$HERE" "$@" sh "$HOOK" "$t" > "$W/log" 2>&1 || { cat "$W/log"; bad "hook failed"; }
}
no_ffmpeg() { ! ls "$1"/usr/lib/libav* "$1"/usr/lib/libsw* "$1"/usr/lib/libpostproc* "$1"/usr/bin/ff* "$1"/usr/share/ffmpeg 2>/dev/null | grep -q .; }

# default bridge: the plugin is parked, no FFmpeg anywhere, the ABI file names the sonames
T="$W/a"; fakeroot_tree "$T"; run "$T" IYAKKI_WEB_AV_ADDON=1
[ -f "$T/usr/lib/iyakki-av/gstreamer-1.0/libgstlibav.so" ] && [ ! -e "$T/usr/lib/gstreamer-1.0/libgstlibav.so" ] && ok "bridge parked, not on the scanned path" || bad "bridge parked"
[ -f "$T/usr/lib/gstreamer-1.0/libgstvpx.so" ] && ok "other plugins untouched" || bad "other plugins untouched"
no_ffmpeg "$T" && ok "no FFmpeg library or program shipped" || bad "FFmpeg left in the image"
A="$T/usr/share/iyakki/av-addon-abi"
[ "$(grep -vc '^#' "$A")" = 7 ] && grep -qx 'libavcodec.so.60' "$A" && grep -qx 'libpostproc.so.57' "$A" && ! grep -q avdevice "$A" && ! grep -q '\.3\.100' "$A" && ok "ABI file lists the 7 sonames only" || { cat "$A"; bad "ABI file"; }
# a second run (repack) keeps it
run "$T" IYAKKI_WEB_AV_ADDON=1
[ -f "$T/usr/lib/iyakki-av/gstreamer-1.0/libgstlibav.so" ] && [ "$(grep -vc '^#' "$A")" = 7 ] && ok "idempotent for a repack" || bad "second run"
# no plugin in the rootfs: says so, no ABI file, no failure
T="$W/b"; fakeroot_tree "$T"; rm "$T/usr/lib/gstreamer-1.0/libgstlibav.so"; run "$T" IYAKKI_WEB_AV_ADDON=1
grep -q WARNING "$W/log" && [ ! -e "$T/usr/share/iyakki/av-addon-abi" ] && ok "missing plugin is reported" || bad "missing plugin"
# off: nothing of it
T="$W/c"; fakeroot_tree "$T"; run "$T" IYAKKI_WEB_AV_ADDON=0
[ ! -e "$T/usr/lib/gstreamer-1.0/libgstlibav.so" ] && [ ! -e "$T/usr/lib/iyakki-av" ] && no_ffmpeg "$T" && ok "switched off: no bridge, no FFmpeg" || bad "switched off"
# the whole codec (the old switch)
T="$W/d"; fakeroot_tree "$T"; run "$T" IYAKKI_WEB_H264=1 IYAKKI_WEB_AV_ADDON=0
[ -f "$T/usr/lib/gstreamer-1.0/libgstlibav.so" ] && ls "$T"/usr/lib/libavcodec.so* >/dev/null && ok "WEB_H264=1 keeps the active codec" || bad "WEB_H264"
# the media player's ffmpeg is kept on request, the bridge still parked
T="$W/e"; fakeroot_tree "$T"; run "$T" IYAKKI_WEB_AV_ADDON=1 WITH_FFMPEG=1
[ -f "$T/usr/lib/iyakki-av/gstreamer-1.0/libgstlibav.so" ] && [ -f "$T/usr/bin/ffmpeg" ] && ok "WITH_FFMPEG=1 keeps the player's ffmpeg" || bad "WITH_FFMPEG"
echo "av-addon hook: $pass passed, $fail failed"
[ "$fail" = 0 ]
