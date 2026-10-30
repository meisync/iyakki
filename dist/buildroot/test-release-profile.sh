#!/bin/sh
# Static test of the RELEASE image profile's shell side: no build, no network, nothing outside a temporary folder.
#   dist/buildroot/test-release-profile.sh
# Runs board/iyakki/post-build.sh (the real hook chain: prune, restricted-firmware, firmware-licences, release-profile.sh) on a fake root file
# system that holds vendor firmware, FFmpeg and a parked codec bridge, and checks what is left; then the GPU firmware guard with a fake
# modprobe. crates/iyakki-relcheck/tests/release_image.rs runs this script, so `cargo test -p iyakki-relcheck` covers it.
set -eu
HERE="$(cd "$(dirname "$0")" && pwd)"
ROOT="$(cd "$HERE/../.." && pwd)"
BOARD="$HERE/board/iyakki"
W="$(mktemp -d)"; trap 'rm -rf "$W"' EXIT
fail() { echo "test-release-profile: FAIL: $*" >&2; exit 1; }
ok() { echo "test-release-profile: ok: $*"; }

mkroot() { # fresh fake root in $1: vendor firmware, codecs, the tools post-build.sh insists on
  R="$1"; rm -rf "$R"; mkdir -p "$R/lib/firmware/amdgpu" "$R/lib/firmware/nvidia/ga102/gsp" "$R/lib/firmware/brcm" "$R/lib/firmware/i915" \
    "$R/usr/share/doc/linux-firmware" "$R/usr/lib/iyakki-av/gstreamer-1.0" "$R/usr/lib/gstreamer-1.0" "$R/usr/bin" "$R/sbin" "$R/etc"
  : > "$R/lib/firmware/regulatory.db"; : > "$R/lib/firmware/regulatory.db.p7s"
  echo x > "$R/lib/firmware/amdgpu/psp_13_0_0_sos.bin"; echo x > "$R/lib/firmware/nvidia/ga102/gsp/gsp-535.113.01.bin"
  echo x > "$R/lib/firmware/brcm/brcmfmac4364b2-pcie.apple,j140k.bin"; echo x > "$R/lib/firmware/i915/tgl_dmc_ver2_12.bin"
  echo x > "$R/usr/share/doc/linux-firmware/WHENCE"
  echo x > "$R/usr/lib/libavcodec.so.60"; echo x > "$R/usr/bin/ffmpeg"; echo x > "$R/usr/lib/iyakki-av/gstreamer-1.0/libgstlibav.so"
  for p in ssh scp dbclient dropbearkey; do echo x > "$R/usr/bin/$p"; done
}
hook() { # run the real post-build hook on $1 with the profile switches in the environment
  R="$1"; shift
  : > "$W/fake-iyakki"
  env IYAKKI_BIN="$W/fake-iyakki" BR2_EXTERNAL_IYAKKI_PATH="$HERE" BR2_CONFIG=/nonexistent IYAKKI_VERSION=0.0.0 IYAKKI_BUILD_DATE=2026-10-27 \
      IYAKKI_VARIANT=pc-x86_64 IYAKKI_ARCH=x86_64 "$@" sh "$BOARD/post-build.sh" "$R" > "$W/hook.log" 2>&1
}

# ---- 1. the release hook chain on a variant that normally keeps the whole GPU/Wi-Fi set
mkroot "$W/r1"
hook "$W/r1" IYAKKI_IMAGE_PROFILE=release || { cat "$W/hook.log" >&2; fail "post-build.sh failed in the release profile"; }
left="$(find "$W/r1/lib/firmware" \( -type f -o -type l \) | sed "s#^$W/r1/lib/firmware/##" | LC_ALL=C sort | tr '\n' ' ')"
[ "$left" = "regulatory.db regulatory.db.p7s " ] || fail "firmware left in the image: $left"
[ ! -e "$W/r1/usr/share/doc/linux-firmware" ] || fail "linux-firmware licence texts left without firmware"
[ ! -e "$W/r1/usr/lib/libavcodec.so.60" ] && [ ! -e "$W/r1/usr/bin/ffmpeg" ] || fail "FFmpeg left in the image"
[ -f "$W/r1/usr/lib/iyakki-av/gstreamer-1.0/libgstlibav.so" ] || fail "the parked bridge was removed although IYAKKI_RELEASE_AV_BRIDGE is not 0"
[ -x "$W/r1/usr/libexec/iyakki/gpu-fw-guard" ] && [ -f "$W/r1/etc/modprobe.d/iyakki-release.conf" ] || fail "release files missing"
grep -qx 'IMAGE_PROFILE=release' "$W/r1/etc/iyakki-release" && grep -qx 'FIRMWARE=none' "$W/r1/etc/iyakki-release" && grep -qx 'AV_BRIDGE=parked' "$W/r1/etc/iyakki-release" || fail "stamp wrong: $(cat "$W/r1/etc/iyakki-release")"
grep -q '^VERSION=0.0.0' "$W/r1/etc/iyakki-release" || fail "the original stamp lines were lost"
sh "$BOARD/release-profile.sh" verify "$W/r1" >/dev/null || fail "verify refuses the tree apply made"
ok "post-build.sh in the release profile leaves only regulatory.db, no FFmpeg, the parked bridge, the stamp and the guard"

# ---- 2. the same chain WITHOUT the profile keeps its old behaviour (pc-firmware.keep applies): firmware is not wiped
mkroot "$W/r2"; echo x > "$W/r2/lib/firmware/amdgpu/psp_13_0_0_toc.bin"
mkdir -p "$W/fwsrc"; echo whence > "$W/fwsrc/WHENCE"; echo licence > "$W/fwsrc/LICENCE.amdgpu"
hook "$W/r2" FWFROM="$W/fwsrc" || { cat "$W/hook.log" >&2; fail "post-build.sh failed without the profile"; }
[ -f "$W/r2/lib/firmware/amdgpu/psp_13_0_0_sos.bin" ] && [ ! -e "$W/r2/usr/libexec/iyakki/gpu-fw-guard" ] || fail "the development image changed"
ok "without IYAKKI_IMAGE_PROFILE the development behaviour is unchanged (pc keep list, no guard)"

# ---- 3. verify catches what apply would have removed
mkroot "$W/r3"
if sh "$BOARD/release-profile.sh" verify "$W/r3" >/dev/null 2>"$W/v.log"; then fail "verify accepted a tree with firmware"; fi
grep -q 'vendor firmware file' "$W/v.log" && grep -q 'codec' "$W/v.log" || fail "verify did not name the problems: $(cat "$W/v.log")"
ok "verify names vendor firmware and codec leftovers"

# ---- 4. switches a release must refuse
for sw in IYAKKI_INCLUDE_RESTRICTED_FIRMWARE=1 WEB_H264=1 IYAKKI_WEB_H264=1 WITH_FFMPEG=1; do
  mkroot "$W/r4"
  if hook "$W/r4" IYAKKI_IMAGE_PROFILE=release "$sw"; then fail "the release profile accepted $sw"; fi
  grep -q 'release-profile' "$W/hook.log" || fail "no explanation for $sw: $(tail -n 3 "$W/hook.log")"
done
ok "IYAKKI_INCLUDE_RESTRICTED_FIRMWARE, WEB_H264 and WITH_FFMPEG are refused"
mkroot "$W/r5"; hook "$W/r5" IYAKKI_IMAGE_PROFILE=release IYAKKI_RELEASE_AV_BRIDGE=0 || fail "bridge-less release failed"
[ ! -e "$W/r5/usr/lib/iyakki-av" ] && grep -qx 'AV_BRIDGE=none' "$W/r5/etc/iyakki-release" || fail "IYAKKI_RELEASE_AV_BRIDGE=0 did not drop the bridge"
ok "IYAKKI_RELEASE_AV_BRIDGE=0 drops the parked bridge"

# ---- 5. repack.sh and build.sh refuse what a release must not have (before doing anything)
if IYAKKI_IMAGE_PROFILE=release sh "$HERE/repack.sh" pc --dry-run --firmware-from /nonexistent >/dev/null 2>"$W/rp.log"; then fail "repack.sh accepted --firmware-from in the release profile"; fi
grep -q 'release' "$W/rp.log" || fail "repack.sh gave no reason: $(cat "$W/rp.log")"
if IYAKKI_IMAGE_PROFILE=bogus sh "$HERE/repack.sh" pc --dry-run >/dev/null 2>"$W/rp2.log"; then fail "repack.sh accepted a bogus profile"; fi
if IYAKKI_IMAGE_PROFILE=release WITH_FFMPEG=1 sh "$HERE/build.sh" web "$W/out" >/dev/null 2>"$W/bs.log"; then fail "build.sh accepted WITH_FFMPEG=1 in the release profile"; fi
[ ! -e "$W/out" ] || fail "build.sh created $W/out before refusing"
if IYAKKI_IMAGE_PROFILE=release sh "$HERE/build.sh" hw "$W/out" >/dev/null 2>&1; then fail "build.sh accepted the hw variant in the release profile"; fi
ok "repack.sh and build.sh refuse --firmware-from, a bogus profile, WITH_FFMPEG=1 and the hw variant"

# ---- 6. the GPU firmware guard
G="$BOARD/release/usr/libexec/iyakki/gpu-fw-guard"
printf '#!/bin/sh\necho "$@" >> "%s/modprobe.calls"\n' "$W" > "$W/fake-modprobe"; chmod +x "$W/fake-modprobe"
guard() { env IYAKKI_FW_DIR="$W/fw" IYAKKI_MODPROBE="$W/fake-modprobe" IYAKKI_CMDLINE_FILE="$W/cmdline" IYAKKI_GUARD_LOG="$W/guard.log" sh "$G" "$@"; }
rm -rf "$W/fw" "$W/modprobe.calls" "$W/guard.log"; mkdir -p "$W/fw"; echo "quiet" > "$W/cmdline"
for m in amdgpu radeon nouveau i915 xe; do guard "$m" || fail "guard exited non-zero for $m without firmware"; done
[ ! -e "$W/modprobe.calls" ] || fail "the guard loaded a GPU driver without firmware: $(cat "$W/modprobe.calls")"
[ "$(grep -c 'not loaded' "$W/guard.log")" = 5 ] && grep -q 'Hardware firmware' "$W/guard.log" || fail "guard log: $(cat "$W/guard.log")"
mkdir -p "$W/fw/amdgpu" "$W/fw/nvidia/ga102" "$W/fw/i915"; echo x > "$W/fw/amdgpu/psp.bin"; echo x > "$W/fw/nvidia/ga102/x.bin"; echo x > "$W/fw/i915/dmc.bin"
guard amdgpu; guard nouveau; guard xe; guard i915; guard radeon
[ "$(cat "$W/modprobe.calls")" = "--ignore-install amdgpu
--ignore-install nouveau
--ignore-install xe
--ignore-install i915" ] || fail "guard loads with firmware wrong: $(cat "$W/modprobe.calls")"
rm -f "$W/modprobe.calls"; rm -rf "$W/fw"; mkdir -p "$W/fw"; echo "quiet iyakki.gpu_fw_guard=0" > "$W/cmdline"; guard amdgpu
[ "$(cat "$W/modprobe.calls")" = "--ignore-install amdgpu" ] || fail "iyakki.gpu_fw_guard=0 did not load the driver"
rm -f "$W/modprobe.calls"; guard snd_hda_intel; [ "$(cat "$W/modprobe.calls")" = "--ignore-install snd_hda_intel" ] || fail "an unguarded module was not passed through"
if guard 2>/dev/null; then fail "guard without a module name exited 0"; fi
# every driver the modprobe rules name is handled by the guard, and the other way round
for m in $(sed -n 's/^install \([a-z0-9_]*\) .*/\1/p' "$BOARD/release/etc/modprobe.d/iyakki-release.conf"); do
  grep -q "^  $m)" "$G" || fail "modprobe rule for $m but the guard has no case for it"
done
for m in $(sed -n 's/^  \([a-z0-9]*\)) .*DIRS=.*/\1/p' "$G"); do
  grep -q "^install $m /usr/libexec/iyakki/gpu-fw-guard $m\$" "$BOARD/release/etc/modprobe.d/iyakki-release.conf" || fail "the guard handles $m but no modprobe rule calls it"
done
ok "the GPU firmware guard loads a driver only when its firmware folder has files (or the command line switches it off)"

# ---- 7. the first-boot notice says what the owner asked for
grep -q 'Optional hardware firmware is not included (licence)' "$BOARD/release/usr/share/iyakki/firmware-notice.txt" || fail "notice text"
grep -q 'Hardware firmware' "$BOARD/release/usr/share/iyakki/firmware-notice.txt" || fail "notice does not name the item"
ok "the first-boot notice text is in place"
# ---- 8. files of packages the build no longer selects are pruned (fixture: a package folder, a package list and a .config)
P="$W/prune"; mkdir -p "$P/pkg/cage" "$P/pkg/wlroots" "$P/pkg/libfoo" "$P/pkg/skeleton" "$P/root/usr/bin" "$P/root/usr/lib" "$P/root/etc"
for n in cage wlroots libfoo; do printf 'config BR2_PACKAGE_%s\n\tbool "x"\n' "$(echo $n | tr a-z A-Z)" > "$P/pkg/$n/Config.in"; done
echo 'config BR2_PACKAGE_SKELETON_X' > "$P/pkg/skeleton/Config.in"
printf 'BR2_PACKAGE_LIBFOO=y\n# BR2_PACKAGE_CAGE is not set\n' > "$P/config"
printf 'cage,./usr/bin/cage\nwlroots,./usr/lib/libwlroots.so.12\nwlroots,./usr/lib/libshared.so\nlibfoo,./usr/lib/libshared.so\nlibfoo,./usr/lib/libfoo.so\nskeleton,./etc/os-release\nhost-gcc,./usr/bin/gcc\n' > "$P/list"
for f in usr/bin/cage usr/lib/libwlroots.so.12 usr/lib/libshared.so usr/lib/libfoo.so etc/os-release; do echo x > "$P/root/$f"; done
sh "$BOARD/prune-stale-packages.sh" --dry-run "$P/list" "$P/config" "$P/pkg" "$P/root" | grep -q '2 deselected package(s)' || fail "dry run did not name the two deselected packages"
[ -e "$P/root/usr/bin/cage" ] || fail "the dry run removed a file"
sh "$BOARD/prune-stale-packages.sh" "$P/list" "$P/config" "$P/pkg" "$P/root" > "$P/out.log" || fail "prune-stale-packages failed"
[ ! -e "$P/root/usr/bin/cage" ] && [ ! -e "$P/root/usr/lib/libwlroots.so.12" ] || fail "a deselected package's file is still there"
[ -e "$P/root/usr/lib/libshared.so" ] && [ -e "$P/root/usr/lib/libfoo.so" ] && [ -e "$P/root/etc/os-release" ] || fail "a selected or symbol-less package's file was removed"
mkdir -p "$P/root/usr/lib/gstreamer-1.0" "$P/root/usr/lib/gst-plugins-bad/wpe-extension"; echo x > "$P/root/usr/lib/gstreamer-1.0/libgstwpe.so"; echo x > "$P/root/usr/lib/gst-plugins-bad/wpe-extension/libgstwpeextension.so"; echo x > "$P/root/usr/lib/gstreamer-1.0/libgstkeep.so"
sh "$BOARD/prune-stale-packages.sh" "$P/list" "$P/config" "$P/pkg" "$P/root" > "$P/out2.log" || fail "prune-stale-packages failed (plugin option)"
[ ! -e "$P/root/usr/lib/gstreamer-1.0/libgstwpe.so" ] && [ ! -e "$P/root/usr/lib/gst-plugins-bad/wpe-extension/libgstwpeextension.so" ] || fail "the switched-off WPE plugin files are still there"
[ -e "$P/root/usr/lib/gstreamer-1.0/libgstkeep.so" ] || fail "another GStreamer plugin was removed"
ok "files of deselected packages are pruned; shared files and packages without a symbol stay; the switched-off gst WPE plugin goes"
echo "test-release-profile: all checks passed"
