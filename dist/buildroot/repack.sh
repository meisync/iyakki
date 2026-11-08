#!/usr/bin/env bash
# IYAKKI-ONLY fast path: put a new iyakki program (and changed overlay / release texts / boot menu) into the image a variant
# already has, WITHOUT running Buildroot's make.   docs/BUILD-INCREMENTAL.md
#
#   repack.sh <variant> [OUT_DIR] [--dry-run] [--cargo] [--fast-zstd] [--base BASE_OUT] [--kernel-from KERNEL_OUT] [--graft NAME]... [--firmware-from LINUX_FIRMWARE_DIR]
#
# What it does (x86_64 only; minutes instead of hours):
#   1. unpacks OUT/images/rootfs.cpio.zst under fakeroot (owners and device nodes survive),
#   2. copies the rootfs overlays over it (like Buildroot) and runs the variant's own post-build hooks (the ones named in
#      BASE/.config: install /usr/bin/iyakki, /etc/iyakki-release, samples, licences, fonts, firmware pruning ...),
#   3. packs it again exactly like Buildroot does (find | sort | cpio -H newc, zstd -19),
#   4. runs the board's post-image.sh and genimage on a staging copy (ISO, install template, data-marker partition: the layout is
#      whatever post-image.sh and genimage-install.cfg say, nothing is re-implemented here),
#   5. only when all of that worked, moves the new files into OUT/images and updates the ISO tree in OUT/build.
# Nothing is written outside OUT. Nothing is written to a drive. The old images stay untouched until step 5.
# --dry-run: check the prerequisites and print the plan only.  --cargo: run cargo build first (like build.sh).
# --base BASE_OUT: take the rootfs, host tools and ISO tree from ANOTHER finished build (default: OUT itself); the result goes to OUT.
# --kernel-from KERNEL_OUT: graft that build's kernel (images/bzImage), /lib/modules and /lib/firmware into the rootfs. Together:
#     repack.sh macweb --base target/buildroot-web --kernel-from target/buildroot-mac
#   makes the macweb image from the web and mac builds with no compile (docs/BUILD-INCREMENTAL.md, "layered macweb").
# --graft NAME (repeatable, or IYAKKI_GRAFT="git ..."): copy the files of dist/buildroot/graft/NAME.list from BASE/target (a package built
#     alone with `make -C BASE <package>`, e.g. git: `make -C target/buildroot-web git iyakki-git-ssh`) into the unpacked root, with graft.sh
#     (hard links kept, perl/python scripts skipped, shared libraries checked). Nothing else of the base's target/ is used.
# --firmware-from DIR (variant "pc"; default: the linux-firmware tree of the --kernel-from build): put the WHOLE linux-firmware set (DIR's own
#     copy-firmware.sh, uncompressed) into /lib/firmware instead of the kernel donor's pruned one; post-build.sh then prunes it with
#     board/iyakki/pc-firmware.keep (all GPUs, common PC Wi-Fi/Bluetooth/Ethernet). ./run.sh build-pc-layered does it (docs/GPU-K3S.md).
# --fast-zstd: zstd -3 instead of -19 (quick tests; the file is bigger, do not release it).
# IYAKKI_IMAGE_PROFILE=release (docs/development/RELEASE-IMAGE-PROFILE.md): the RELEASE image. No vendor firmware of any kind (the donor's
#     /lib/firmware contributes only regulatory.db; --firmware-from is refused; board/iyakki/pc-firmware.keep and mac-firmware.keep are not
#     used, release-firmware.keep is), no NVIDIA GSP, no Apple/Broadcom blobs, no H.264/AAC codec libraries; the GPU drivers are held back
#     until firmware exists (gpu-fw-guard); /etc/iyakki-release says IMAGE_PROFILE=release, FIRMWARE=none; the person installs what the
#     computer needs with "Hardware firmware". The output goes to target/buildroot-VARIANT-release (the development images stay untouched).
#     (IYAKKI_PROFILE below is the CARGO profile; it has nothing to do with this.)
# IYAKKI_PROFILE=device (default release): the CARGO profile of the program. `device` (Cargo.toml [profile.device]) has no LTO, 16 code-generation
#     units and incremental compilation: a one-line change builds in seconds to a minute instead of 12 minutes, the binary is bigger and slower.
#     For test images and `dev update` pushes ONLY; a release image is always built with the release profile. The binary is then taken from
#     target/x86_64-unknown-linux-musl/device/ (IYAKKI_PROFILE=fast works the same way).
# The pruned, signed modules + firmware are cached (dist/buildroot/repack-cache.sh, in target/repack-cache, IYAKKI_REPACK_CACHE=0 turns it off):
#     the first run with a given donor, firmware set, keep lists and signing key prunes and signs as always and saves the result; the next
#     runs with the same inputs restore it and skip the copy of linux-firmware, the signing of every module and the 1 280 MiB prune.
# IYAKKI_REPACK_BIN=FILE: install this program instead of target/.../iyakki (dist/buildroot/quick-push.sh uses it).
#       IYAKKI_IMAGE_PROFILE=release ./dist/buildroot/repack.sh pc --base target/buildroot-web --kernel-from target/buildroot-mac
# Secure Boot: run it with the same IYAKKI_SB_KEY / IYAKKI_SB_CERT as the build (a change from signed to unsigned is FULL for
# changed.sh); every kernel module that has no signature yet is signed with the key (Secure Boot lockdown refuses unsigned ones), the kernel and the EFI image are signed as in post-image.sh.
set -eu

HERE="$(cd "$(dirname "$0")" && pwd)"
ROOT="${IYAKKI_ROOT:-$(cd "$HERE/../.." && pwd)}"
VARIANT=""; OUT=""; DRY=0; CARGO=0; ZL=19; BASE=""; KFROM=""; NEXT=""; GRAFTS="${IYAKKI_GRAFT:-}"; FWFROM=""
for a in "$@"; do
  case "$NEXT" in base) BASE="$a"; NEXT=""; continue ;; kernel) KFROM="$a"; NEXT=""; continue ;; graft) GRAFTS="$GRAFTS $a"; NEXT=""; continue ;; fwfrom) FWFROM="$a"; NEXT=""; continue ;; esac
  case "$a" in
    --base) NEXT=base ;; --kernel-from) NEXT=kernel ;; --graft) NEXT=graft ;; --firmware-from) NEXT=fwfrom ;;
    --dry-run) DRY=1 ;; --cargo) CARGO=1 ;; --fast-zstd) ZL=3 ;;
    -*) echo "repack.sh: unknown option $a" >&2; exit 2 ;;
    *) if [ -z "$VARIANT" ]; then VARIANT="$a"; elif [ -z "$OUT" ]; then OUT="$a"; else echo "repack.sh: too many arguments" >&2; exit 2; fi ;;
  esac
done
[ -n "$VARIANT" ] || { sed -n '2,8p' "$0" >&2; exit 2; }
[ "$NEXT" = "" ] || { echo "repack.sh: --$NEXT needs a value" >&2; exit 2; }
# a relative --base / --kernel-from means relative to the repository (the steps below change directory)
case "$BASE" in ""|/*) ;; *) BASE="$ROOT/$BASE" ;; esac
case "$KFROM" in ""|/*) ;; *) KFROM="$ROOT/$KFROM" ;; esac
ARCH="${ARCH:-x86_64}"
[ "$ARCH" = x86_64 ] || { echo "repack.sh: x86_64 only (the ARM disk image is built by its own post-image step: use the full build)" >&2; exit 2; }
RUST_TARGET=x86_64-unknown-linux-musl
. "$HERE/repack-cache.sh"
IMAGE_PROFILE="${IYAKKI_IMAGE_PROFILE:-}"
case "$IMAGE_PROFILE" in ""|release) ;; *) echo "repack.sh: IYAKKI_IMAGE_PROFILE must be empty or release (got '$IMAGE_PROFILE')" >&2; exit 2 ;; esac
if [ "$IMAGE_PROFILE" = release ]; then
  : "${OUT:=$ROOT/target/buildroot-$VARIANT-release}"
  sh "$HERE/board/iyakki/release-profile.sh" env || exit 2
  [ -z "$FWFROM" ] || { echo "repack.sh: --firmware-from puts vendor firmware in the image; a release image (IYAKKI_IMAGE_PROFILE=release) has none" >&2; exit 2; }
else
  : "${OUT:=$ROOT/target/buildroot-$VARIANT}"
fi
PROFILE="${IYAKKI_PROFILE:-release}"
if [ "$PROFILE" = release ]; then PFLAG=--release; else PFLAG="--profile $PROFILE"; fi
: "${BASE:=$OUT}"
IMG="$BASE/images"; BLD="$BASE/build"; HOSTD="$BASE/host"; IMGD="$OUT/images"
if [ -n "$KFROM" ]; then
  [ -s "$KFROM/images/rootfs.cpio" ] && [ -s "$KFROM/images/bzImage" ] || { echo "repack.sh: $KFROM has no finished images" >&2; exit 1; }
fi
BR_VERSION="${BR_VERSION:-2026.02.3}"
BR_DIR="${BR_DIR:-$ROOT/target/buildroot-src/buildroot-$BR_VERSION}"
STAGE="$OUT/.repack"

say() { echo "repack: $*"; }
die() { echo "repack: $*" >&2; exit 1; }

# ---- prerequisites ----
[ -s "$IMG/rootfs.cpio.zst" ] || die "no $IMG/rootfs.cpio.zst: nothing to repack, do a full build$( [ "$IMAGE_PROFILE" = release ] && echo " (a release image is repacked from a finished build: give --base, e.g. --base target/buildroot-web --kernel-from target/buildroot-mac)" )"
[ -d "$BLD/buildroot-fs/rootfs.iso9660.tmp/boot" ] || die "no ISO tree in $BLD/buildroot-fs: do a full build"
[ -f "$BASE/.config" ] || die "no $BASE/.config"
ls "$BLD"/grub2-*/build-i386-pc/grub-core/boot_hybrid.img >/dev/null 2>&1 || die "no grub2 build tree in $BLD (post-image needs boot_hybrid.img)"
for t in xorriso genimage zstd; do [ -x "$HOSTD/bin/$t" ] || die "missing $HOSTD/bin/$t"; done
FAKEROOT="$HOSTD/bin/fakeroot"; [ -x "$FAKEROOT" ] || FAKEROOT="$(command -v fakeroot || true)"
[ -n "$FAKEROOT" ] || die "fakeroot not found"
command -v cpio >/dev/null || die "cpio not found"; command -v rsync >/dev/null || die "rsync not found"
# the PC image: the full linux-firmware set comes from the source tree of the kernel donor (its copy-firmware.sh), post-build.sh prunes it
if [ "$VARIANT" = pc ] && [ -z "$FWFROM" ] && [ -n "$KFROM" ] && [ "$IMAGE_PROFILE" != release ]; then FWFROM="$(ls -d "$KFROM"/build/linux-firmware-* 2>/dev/null | head -n1 || true)"; fi
case "$FWFROM" in ""|/*) ;; *) FWFROM="$ROOT/$FWFROM" ;; esac
if { [ "$VARIANT" = pc ] && [ "$IMAGE_PROFILE" != release ]; } || [ -n "$FWFROM" ]; then
  [ -f "$FWFROM/copy-firmware.sh" ] && [ -f "$FWFROM/WHENCE" ] || die "variant pc needs the linux-firmware source tree (copy-firmware.sh, WHENCE): give --firmware-from DIR or --kernel-from a build that has build/linux-firmware-*"
  [ -f "$HERE/board/iyakki/pc-firmware.keep" ] || die "no board/iyakki/pc-firmware.keep"
fi
BIN="${IYAKKI_REPACK_BIN:-$ROOT/target/$RUST_TARGET/$PROFILE/iyakki}"; SBIN="$ROOT/target/$RUST_TARGET/$PROFILE/iyakki-store-server"
cfgval() { sed -n "s/^$1=\"\(.*\)\"\$/\1/p" "$BASE/.config" | head -n1 | sed "s#\$(BR2_EXTERNAL_IYAKKI_PATH)#$HERE#g"; }
OVERLAYS="$(cfgval BR2_ROOTFS_OVERLAY)"; HOOKS="$(cfgval BR2_ROOTFS_POST_BUILD_SCRIPT)"
POSTIMG="$(cfgval BR2_ROOTFS_POST_IMAGE_SCRIPT)"; PIARGS="$(cfgval BR2_ROOTFS_POST_SCRIPT_ARGS)"
[ -n "$HOOKS" ] || die "no BR2_ROOTFS_POST_BUILD_SCRIPT in $BASE/.config"
case "$POSTIMG" in *post-image.sh*genimage.sh*) ;; *) die "unexpected BR2_ROOTFS_POST_IMAGE_SCRIPT: $POSTIMG" ;; esac
set -- $POSTIMG; PI_SCRIPT="$1"; GI_SCRIPT="$BR_DIR/$2"
# the PC image: its initial RAM disk (the whole GPU and Wi-Fi firmware set) does not fit the 256 MB boot volume of the other variants:
# the install template gets a 512 MB one (iyakki-usb takes the size from the image; a 4096-byte-sector drive gets its own fixed 320 MiB)
PC_GI_CFG=""
if [ "$VARIANT" = pc ] && [ "$IMAGE_PROFILE" != release ]; then
  case "$PIARGS" in *genimage-install.cfg*) PC_GI_CFG="$OUT/.repack/genimage-install-pc.cfg"; PIARGS="$(printf '%s' "$PIARGS" | sed "s#[^ ]*genimage-install\.cfg#$PC_GI_CFG#")" ;; *) die "unexpected genimage arguments for the pc variant: $PIARGS" ;; esac
fi
[ "$CARGO" = 1 ] || [ -f "$BIN" ] || die "$BIN is missing: cargo build $PFLAG --target $RUST_TARGET -p iyakki-core (or use --cargo)"

# every graft must be possible before anything is unpacked (the package built in BASE, the list known)
GERR="$(mktemp)"
for g in $GRAFTS; do
  [ -f "$HERE/graft/$g.list" ] || die "no graft list $HERE/graft/$g.list"
  sh "$HERE/graft.sh" "$g" "$BASE/target" "$BASE" --dry-run >/dev/null 2>"$GERR" || { cat "$GERR" >&2; rm -f "$GERR"; die "graft $g is not possible from $BASE/target"; }
done
rm -f "$GERR"

# the base build must have been made with the same WEB_* switches as this run asks for (they change packages, not just files)
for pair in "WEB_IRIS:iyakki_web_iris" "WEB_CROCUS:iyakki_web_crocus"; do
  flag="${pair%%:*}"; frag="$HERE/configs/${pair#*:}.fragment"; [ -f "$frag" ] || continue
  sym="$(grep -E '^BR2_[A-Z0-9_]+=y' "$frag" | head -n1 || true)"; [ -n "$sym" ] || continue
  have=0; grep -qx "$sym" "$BASE/.config" && have=1
  want="$(eval echo "\${$flag:-0}")"; [ "$flag" = WEB_IRIS ] && [ "${WEB_CROCUS:-0}" = 1 ] && want=1
  [ "$have" = "$want" ] || die "$BASE was built with $flag=$have but this run has $flag=$want: that needs a real build of that option set"
done
# H.264 is not a package switch (gst-libav is always built and web-post-build.sh strips or parks it): look at what the base image carries.
# WEB_H264=1: the plugin is active in /usr/lib/gstreamer-1.0. WEB_AV_ADDON=1 (default): it is parked in /usr/lib/iyakki-av (inactive).
if grep -q 'iyakki_web' "$BASE/.config" 2>/dev/null || grep -q '^BR2_PACKAGE_IYAKKI_WEBVIEW_HELPER=y' "$BASE/.config" 2>/dev/null; then
  have=0; have_av=0
  listing="$(zstd -dc "$IMG/rootfs.cpio.zst" | cpio -t --quiet 2>/dev/null || true)"
  echo "$listing" | grep -q 'usr/lib/gstreamer-1.0/libgstlibav' && have=1 || true
  echo "$listing" | grep -q 'usr/lib/iyakki-av/gstreamer-1.0/libgstlibav' && have_av=1 || true
  [ "$have" = "${WEB_H264:-0}" ] || die "$BASE was built with WEB_H264=$have but this run has WEB_H264=${WEB_H264:-0}: the codecs differ, do a real build"
  want_av=0; [ "${WEB_H264:-0}" != 1 ] && [ "${WEB_AV_ADDON:-1}" = 1 ] && want_av=1
  if [ "$have_av" = 1 ] && [ "$want_av" = 0 ]; then die "$BASE carries the inactive audio and video bridge but this run has WEB_AV_ADDON=0 / WEB_H264=1: a plugin cannot be taken from a finished rootfs, do a real build"; fi
  # (a base built before WEB_AV_ADDON has no plugin to park: the repack goes on without the bridge and the hook says so; a real build adds it, docs/FULL-BROWSER.md)
  if [ "$have_av" = 0 ] && [ "$want_av" = 1 ]; then echo "repack: WARNING $BASE has no audio and video bridge (built before WEB_AV_ADDON); this image will not have it either" >&2; fi
fi
# the environment build.sh gives the hooks
export IYAKKI_ARCH=x86_64 BR2_EXTERNAL_IYAKKI_PATH="$HERE" BR2_CONFIG="$BASE/.config"
export IYAKKI_BIN="$BIN"
export IYAKKI_VERSION="$(sed -n 's/^version *= *"\(.*\)"/\1/p' "$ROOT/Cargo.toml" | head -n1)"
export IYAKKI_BUILD_DATE="$(date -u +%Y-%m-%d)"
export IYAKKI_GIT_DESCRIBE="$(cd "$ROOT" && git describe --tags --always --dirty 2>/dev/null || echo unknown)"
export IYAKKI_VARIANT="$VARIANT-x86_64"
if [ "$IMAGE_PROFILE" = release ]; then export IYAKKI_IMAGE_PROFILE=release; else unset IYAKKI_IMAGE_PROFILE || true; fi
export IYAKKI_WEB_H264="${WEB_H264:-0}"
if [ "${WEB_H264:-0}" != 1 ] && [ "${WEB_AV_ADDON:-1}" = 1 ]; then export IYAKKI_WEB_AV_ADDON=1; else export IYAKKI_WEB_AV_ADDON=0; fi
if [ "${WEB_IRIS:-0}" = 1 ] || [ "${WEB_CROCUS:-0}" = 1 ]; then export IYAKKI_WEB_INTEL_GL=1; else export IYAKKI_WEB_INTEL_GL=0; fi
[ -f "$SBIN" ] && export IYAKKI_STORE_SERVER_BIN="$SBIN"

say "variant      $VARIANT ($OUT)"
[ "$BASE" = "$OUT" ] || say "base         $BASE"
[ -z "$KFROM" ] || say "kernel from  $KFROM (bzImage, /lib/modules, /lib/firmware)"
[ -z "$GRAFTS" ] || say "grafts       $GRAFTS (from $BASE/target)"
[ -z "$FWFROM" ] || say "firmware     the whole set from $FWFROM, pruned by board/iyakki/pc-firmware.keep"
[ "$IMAGE_PROFILE" != release ] || say "profile      RELEASE: no vendor firmware (only regulatory.db, board/iyakki/release-firmware.keep), no codec libraries, GPU drivers held until firmware exists; AV bridge $( [ "${IYAKKI_RELEASE_AV_BRIDGE:-1}" = 0 ] && echo dropped || echo 'parked (inactive)' )"
say "overlays     $OVERLAYS"
say "post-build   $HOOKS"
say "post-image   $PI_SCRIPT, then $GI_SCRIPT $PIARGS"
say "binary       $BIN$( [ "$CARGO" = 1 ] && echo ' (cargo build first)')"
say "zstd level   $ZL$( [ "$ZL" != 19 ] && echo '  (NOT a release image)')"
say "secure boot  $( [ -n "${IYAKKI_SB_KEY:-}" ] && [ -n "${IYAKKI_SB_CERT:-}" ] && echo signed || echo unsigned )"
say "cargo profile $PROFILE$( [ "$PROFILE" != release ] && echo '  (NOT a release binary)')"
FWC_KEY="$(rc_key 2>/dev/null || true)"; FWC_FILE=""; FWC_LOAD=""
if [ -n "$FWC_KEY" ]; then
  FWC_FILE="$(rc_file "$FWC_KEY")"
  if [ -s "$FWC_FILE" ]; then FWC_LOAD="$FWC_FILE"; say "mod+fw cache hit  $FWC_FILE (no firmware copy, module signing or prune)"; else say "mod+fw cache miss: will be saved to $FWC_FILE"; fi
else
  say "mod+fw cache off ($( [ "$IMAGE_PROFILE" = release ] && echo 'release image' || echo 'IYAKKI_REPACK_CACHE=0 or an overlay with firmware' ))"
fi
if [ "$DRY" = 1 ]; then
  say "plan: unpack $IMG/rootfs.cpio.zst (fakeroot) -> overlays -> hooks -> cpio+zstd -> post-image + genimage on $STAGE -> move into $IMGD"
  say "dry run: nothing was written"; exit 0
fi

if [ "$CARGO" = 1 ]; then
  # ONE cargo call for both programs: two calls made cargo build the shared third-party crates twice with different feature sets (and so
  # iyakki-proxy and iyakki-agent twice: 3 m 49 s in the 2026-11-07 log). The feature sets of the two programs were compared before this
  # was merged (docs/development/COMPILE-SPEED-PLAN.md, "Measured results"). If the store server cannot be built the main program still is:
  # the old two-step build is the fallback, and the image then goes without the store server, as before.
  if (cd "$ROOT" && cargo build $PFLAG --target "$RUST_TARGET" -p iyakki-core -p iyakki-store-server); then
    export IYAKKI_STORE_SERVER_BIN="$SBIN"
  else
    echo "repack: WARNING: the joint build failed; building the main program alone" >&2
    (cd "$ROOT" && cargo build $PFLAG --target "$RUST_TARGET" -p iyakki-core)
    (cd "$ROOT" && cargo build $PFLAG --target "$RUST_TARGET" -p iyakki-store-server) && export IYAKKI_STORE_SERVER_BIN="$SBIN" \
      || echo "repack: WARNING: iyakki-store-server did not build; the image will not carry it" >&2
  fi
fi
[ -f "$BIN" ] || die "$BIN is missing"

mkdir -p "$OUT/images"; rm -rf "$STAGE"; mkdir -p "$STAGE/root" "$STAGE/bin" "$STAGE/build/buildroot-fs"
if [ -n "$PC_GI_CFG" ]; then sed 's/^\tsize = 256M$/\tsize = 512M/' "$HERE/board/iyakki/genimage-install.cfg" > "$PC_GI_CFG"; grep -q 'size = 512M' "$PC_GI_CFG" || die "the genimage configuration has no 256M boot volume to enlarge"; fi
trap 'rm -rf "$STAGE/root"' EXIT   # the staging folder itself stays on failure for a look; the unpacked root is big
t0=$(date +%s)

# ---- 1-3: unpack, update, pack (one fakeroot session) ----
cat > "$STAGE/session.sh" <<'EOS'
set -eu
cd "$STAGE/root"
zstd -dc "$IMG/rootfs.cpio.zst" | cpio -idm --quiet --no-absolute-filenames
OLD_SB="$(sed -n 's/^\(SECURE_BOOT_CERT_SHA1=.*\)$/\1/p' etc/iyakki-release 2>/dev/null || true)"
# the pruned and signed modules + firmware of an earlier run with the same inputs (repack-cache.sh): restore them and skip the staging below
FWC_HIT=0
if [ -n "${FWC_LOAD:-}" ] && [ -s "$FWC_LOAD" ]; then
  rm -rf lib/modules lib/firmware
  zstd -dc "$FWC_LOAD" | tar --numeric-owner -xpf - || { echo "repack: the cache $FWC_LOAD is damaged: remove it and run again" >&2; exit 1; }
  [ -d lib/modules ] && [ -d lib/firmware ] || { echo "repack: the cache $FWC_LOAD has no modules or firmware: remove it and run again" >&2; exit 1; }
  FWC_HIT=1; export IYAKKI_FW_PRUNED=1
  echo "repack: modules and firmware restored from the cache $FWC_LOAD (not copied, signed or pruned again)"
fi
if [ "$FWC_HIT" != 1 ]; then
if [ -n "$KFROM" ]; then
  rm -rf lib/modules lib/firmware
  if [ "${IYAKKI_IMAGE_PROFILE:-}" = release ]; then
    # release: the donor's modules, and of its firmware folder only the regulatory database (wireless-regdb, ISC); the rest is vendor firmware
    cpio -idm --quiet --no-absolute-filenames 'lib/modules' 'lib/modules/*' 'lib/firmware/regulatory.db' 'lib/firmware/regulatory.db.p7s' < "$KFROM/images/rootfs.cpio"
    mkdir -p lib/firmware
    [ -d lib/modules ] || { echo "graft: the kernel build has no /lib/modules" >&2; exit 1; }
  else
    cpio -idm --quiet --no-absolute-filenames 'lib/modules' 'lib/modules/*' 'lib/firmware' 'lib/firmware/*' < "$KFROM/images/rootfs.cpio"
    [ -d lib/modules ] && [ -d lib/firmware ] || { echo "graft: the kernel build has no /lib/modules or /lib/firmware" >&2; exit 1; }
  fi
  [ "$(ls lib/modules)" = "$(basename "$(ls -d lib/modules/*/ | head -n1)")" ] || { echo "graft: more than one kernel version in /lib/modules" >&2; exit 1; }
fi
# out-of-tree modules built alone in the kernel donor (`make -C target/buildroot-mac snd-hda-macbookpro`, docs/MAC-HARDWARE.md): a .ko in the
# donor's target/lib/modules/<ver>/updates that its packed rootfs.cpio does not have yet (or has in an older build) is added, signed when a Secure Boot key is given,
# and modules.dep/alias are rebuilt with the donor's depmod (the HD-audio bus finds snd-hda-codec-cs8409 by its alias hdaudio:v10138409; the PCI core finds facetimehd by pci:v000014E4d00001570)
if [ -n "$KFROM" ]; then
  KVER="$(ls lib/modules)"; XUP="$KFROM/target/lib/modules/$KVER/updates"; added=""
  # the list of out-of-tree modules: the sound codec, the FaceTime HD camera and the videobuf2 module the camera driver needs (package/facetimehd)
  OOT_MODULES="snd-hda-codec-cs8409 facetimehd videobuf2-dma-sg apple-bce"
  for m in $OOT_MODULES; do
    ko="$XUP/$m.ko"
    [ -f "$ko" ] || continue
    # a module built alone AFTER the donor's rootfs was packed (the sound driver was rebuilt with new log lines and the image kept the
    # old copy): the donor's target copy wins when it differs from the packed one
    [ -f "lib/modules/$KVER/updates/$(basename "$ko")" ] && cmp -s "$ko" "lib/modules/$KVER/updates/$(basename "$ko")" && continue
    mkdir -p "lib/modules/$KVER/updates"; cp -f "$ko" "lib/modules/$KVER/updates/"; chmod 0644 "lib/modules/$KVER/updates/$(basename "$ko")"
    added="$added $(basename "$ko")"
  done
  if [ -n "$added" ]; then
    if [ -n "${IYAKKI_SB_KEY:-}" ] && [ -n "${IYAKKI_SB_CERT:-}" ]; then
      SF="${IYAKKI_SIGN_FILE:-$(ls "$KFROM"/build/linux-[0-9]*/scripts/sign-file 2>/dev/null | head -n1 || true)}"
      [ -n "$SF" ] && [ -x "$SF" ] || { echo "repack: no scripts/sign-file to sign$added (set IYAKKI_SIGN_FILE)" >&2; exit 1; }
      for m in $added; do "$SF" sha256 "$IYAKKI_SB_KEY" "$IYAKKI_SB_CERT" "lib/modules/$KVER/updates/$m" || { echo "repack: could not sign $m" >&2; exit 1; }; done
    fi
    DEPMOD="$BASE/host/sbin/depmod"; [ -x "$DEPMOD" ] || DEPMOD="$(command -v depmod || true)"
    [ -n "$DEPMOD" ] || { echo "repack: depmod not found (needed for$added)" >&2; exit 1; }
    "$DEPMOD" -b "$STAGE/root" "$KVER" || { echo "repack: depmod failed" >&2; exit 1; }
    echo "repack: modules added from $KFROM:$added (depmod run)"
  fi
fi
# Secure Boot ON locks the kernel down: it loads only modules signed with a key it trusts (the enrolled MOK). A kernel build made without the
# key leaves its modules unsigned, so on a PC with Secure Boot every driver that is a module (network, Wi-Fi, sound, touchpad, camera) would be
# refused ("Lockdown: modprobe: unsigned module loading is restricted"). Sign every module that carries no signature yet; modules already
# signed (the out-of-tree ones above, or a build made with the key) are left as they are.
if [ -n "${IYAKKI_SB_KEY:-}" ] && [ -n "${IYAKKI_SB_CERT:-}" ] && [ -d lib/modules ]; then
  SF="${IYAKKI_SIGN_FILE:-$(ls "${KFROM:-$BASE}"/build/linux-[0-9]*/scripts/sign-file "$BASE"/build/linux-[0-9]*/scripts/sign-file 2>/dev/null | head -n1 || true)}"
  [ -n "$SF" ] && [ -x "$SF" ] || { echo "repack: no scripts/sign-file to sign the kernel modules (set IYAKKI_SIGN_FILE)" >&2; exit 1; }
  nsigned=0; nalready=0
  while IFS= read -r ko; do
    if tail -c 28 "$ko" | grep -q '~Module signature appended~'; then nalready=$((nalready + 1)); continue; fi
    "$SF" sha256 "$IYAKKI_SB_KEY" "$IYAKKI_SB_CERT" "$ko" || { echo "repack: could not sign $ko" >&2; exit 1; }
    nsigned=$((nsigned + 1))
  done <<EOF_MODS
$(find lib/modules -name '*.ko' | sort)
EOF_MODS
  echo "repack: Secure Boot: $nsigned kernel module(s) signed with the key, $nalready already signed"
fi
# the PC image: all of linux-firmware (uncompressed, as the kernel of this image reads it), pruned later by the post-build hook
if [ -n "$FWFROM" ]; then
  # the wireless regulatory database is not part of linux-firmware (it is the wireless-regdb package, ISC licence): keep the donor's copy,
  # or cfg80211 logs "failed to load regulatory.db" and every Wi-Fi card runs with the world regulatory domain
  mkdir -p "$STAGE/regdb"
  for f in regulatory.db regulatory.db.p7s; do [ -f "lib/firmware/$f" ] && cp -f "lib/firmware/$f" "$STAGE/regdb/$f"; done
  rm -rf lib/firmware; mkdir -p lib/firmware
  (cd "$FWFROM" && sh ./copy-firmware.sh "$STAGE/root/lib/firmware") >/dev/null || { echo "firmware: copy-firmware.sh failed" >&2; exit 1; }
  for f in regulatory.db regulatory.db.p7s; do [ -f "$STAGE/regdb/$f" ] && [ ! -f "lib/firmware/$f" ] && cp -f "$STAGE/regdb/$f" "lib/firmware/$f"; done
  echo "repack: firmware: the full set from $FWFROM is in place (post-build prunes it)"
fi
fi   # FWC_HIT != 1
# a web engine helper rebuilt alone (`make iyakki-webview-helper` in the base tree) is newer than the packed image: take it
NEWH="$BASE/target/usr/libexec/iyakki/webview-helper"
if [ -f "$NEWH" ] && [ -f usr/libexec/iyakki/webview-helper ] && [ "$NEWH" -nt "$IMG/rootfs.cpio.zst" ]; then
  cp -f "$NEWH" usr/libexec/iyakki/webview-helper; chmod 0755 usr/libexec/iyakki/webview-helper
  echo "repack: web engine helper refreshed from $NEWH"
fi
# the shared camera plugin (dist/webview/wpe-helper/camshare.c) travels with the helper: copy it when it is newer or the image has none
NEWP="$BASE/target/usr/lib/gstreamer-1.0/libgstiyakkicamshare.so"
if [ -f "$NEWP" ] && [ -d usr/lib/gstreamer-1.0 ] && { [ ! -f usr/lib/gstreamer-1.0/libgstiyakkicamshare.so ] || [ "$NEWP" -nt "$IMG/rootfs.cpio.zst" ]; }; then
  cp -f "$NEWP" usr/lib/gstreamer-1.0/libgstiyakkicamshare.so; chmod 0755 usr/lib/gstreamer-1.0/libgstiyakkicamshare.so
  echo "repack: shared camera plugin refreshed from $NEWP"
fi
# the microphone plugin (dist/webview/wpe-helper/micshare.c) the same way
NEWM="$BASE/target/usr/lib/gstreamer-1.0/libgstiyakkimic.so"
if [ -f "$NEWM" ] && [ -d usr/lib/gstreamer-1.0 ] && { [ ! -f usr/lib/gstreamer-1.0/libgstiyakkimic.so ] || [ "$NEWM" -nt "$IMG/rootfs.cpio.zst" ]; }; then
  cp -f "$NEWM" usr/lib/gstreamer-1.0/libgstiyakkimic.so; chmod 0755 usr/lib/gstreamer-1.0/libgstiyakkimic.so
  echo "repack: microphone plugin refreshed from $NEWM"
fi
# the MP3 decoder plugin (dist/webview/wpe-helper/mp3dec.c) the same way
NEW3="$BASE/target/usr/lib/gstreamer-1.0/libgstiyakkimp3.so"
if [ -f "$NEW3" ] && [ -d usr/lib/gstreamer-1.0 ] && { [ ! -f usr/lib/gstreamer-1.0/libgstiyakkimp3.so ] || [ "$NEW3" -nt "$IMG/rootfs.cpio.zst" ]; }; then
  cp -f "$NEW3" usr/lib/gstreamer-1.0/libgstiyakkimp3.so; chmod 0755 usr/lib/gstreamer-1.0/libgstiyakkimp3.so
  echo "repack: MP3 decoder plugin refreshed from $NEW3"
fi
# the engine's never-failing sound output plugin (dist/webview/wpe-helper/websink.c) the same way
NEW4="$BASE/target/usr/lib/gstreamer-1.0/libgstiyakkiwebsink.so"
if [ -f "$NEW4" ] && [ -d usr/lib/gstreamer-1.0 ] && { [ ! -f usr/lib/gstreamer-1.0/libgstiyakkiwebsink.so ] || [ "$NEW4" -nt "$IMG/rootfs.cpio.zst" ]; }; then
  cp -f "$NEW4" usr/lib/gstreamer-1.0/libgstiyakkiwebsink.so; chmod 0755 usr/lib/gstreamer-1.0/libgstiyakkiwebsink.so
  echo "repack: web sound output plugin refreshed from $NEW4"
fi
# GStreamer plugins the Buildroot packages gained after the packed image was made: the MediaRecorder pipeline of the engine needs
# `encodebin` (gst-plugins-base, plugin "encoding") and `uritranscodebin` (gst-plugins-bad, plugin "transcode"): copy when newer or missing
for P in libgstencoding libgsttranscode; do
  NEWG="$BASE/target/usr/lib/gstreamer-1.0/$P.so"
  if [ -f "$NEWG" ] && [ -d usr/lib/gstreamer-1.0 ] && { [ ! -f usr/lib/gstreamer-1.0/$P.so ] || [ "$NEWG" -nt "$IMG/rootfs.cpio.zst" ]; }; then
    cp -f "$NEWG" usr/lib/gstreamer-1.0/$P.so; chmod 0755 usr/lib/gstreamer-1.0/$P.so
    echo "repack: recorder plugin $P refreshed from $NEWG"
  fi
done
for g in $GRAFTS; do
  sh "$HERE/graft.sh" "$g" "$BASE/target" "$STAGE/root" | tail -n 1
done
for d in $OVERLAYS; do
  rsync -a --ignore-times --keep-dirlinks --chmod=u=rwX,go=rX --exclude .empty --exclude '*~' "$d/" "$STAGE/root/"
done
# files of packages the base build no longer selects (Buildroot keeps them in target/ after a rebuild without the package): pruned in a release
# image, with no legal-info row they would be unexplained; IYAKKI_PRUNE_STALE=1 does it for a development image too, 0 never
if [ "${IYAKKI_PRUNE_STALE:-$( [ "${IYAKKI_IMAGE_PROFILE:-}" = release ] && echo 1 || echo 0 )}" = 1 ]; then
  if [ -f "$BASE/build/packages-file-list.txt" ] && [ -f "$BASE/.config" ]; then
    sh "$HERE/board/iyakki/prune-stale-packages.sh" "$BASE/build/packages-file-list.txt" "$BASE/.config" "$BR_DIR/package" "$STAGE/root"
  else
    echo "repack: no package list or .config in $BASE: stale packages not pruned" >&2
  fi
fi
# modules are already signed in the old image; the hooks must not sign them again (the certificate line is put back below)
SB_KEY_SAVE="${IYAKKI_SB_KEY:-}"; unset IYAKKI_SB_KEY IYAKKI_SB_CERT || true
for s in $HOOKS; do sh "$s" "$STAGE/root"; done
[ -z "$OLD_SB" ] || echo "$OLD_SB" >> etc/iyakki-release
find . | LC_ALL=C sort | cpio --quiet -o -H newc > "$STAGE/bin/rootfs.cpio"
EOS
# the cache: a hit is restored in the session; a miss asks post-build.sh (right after the prune) to save the result under the key
if [ -n "$FWC_FILE" ] && [ -z "$FWC_LOAD" ]; then export IYAKKI_FW_CACHE_SAVE="$FWC_FILE"; fi
export STAGE IMG OVERLAYS HOOKS KFROM BASE FWFROM GRAFTS HERE IMAGE_PROFILE BR_DIR FWC_LOAD
say "unpacking, applying overlays and hooks, packing (fakeroot)"
# Buildroot's fakeroot script has the absolute path of the tree it was built in baked in (here a folder that has since been renamed:
# "libfakeroot.so not found, aborting"): when it is the host tree's own script, name the library and the daemon of THIS tree
FRARGS=""
if [ "$FAKEROOT" = "$HOSTD/bin/fakeroot" ] && [ -f "$HOSTD/lib/libfakeroot.so" ] && [ -x "$HOSTD/bin/faked" ]; then FRARGS="-l $HOSTD/lib/libfakeroot.so -f $HOSTD/bin/faked"; fi
# shellcheck disable=SC2086
"$FAKEROOT" $FRARGS -- sh "$STAGE/session.sh" >"$STAGE/session.log" 2>&1 || { tail -n 20 "$STAGE/session.log" >&2; die "the unpack/hook/pack step failed (log: $STAGE/session.log)"; }
[ -z "${IYAKKI_FW_CACHE_SAVE:-}" ] || rc_trim
sed -n '/^post-build\|^web-post-build\|^prune-firmware\|^repack: web engine helper\|^repack: modules and firmware restored/p' "$STAGE/session.log" | head -n 10 || true
say "compressing (zstd -$ZL)"
"$HOSTD/bin/zstd" -$ZL -z -f -T0 -q "$STAGE/bin/rootfs.cpio" -o "$STAGE/bin/rootfs.cpio.zst"
# the listing must match the old image except for what the hooks changed
zstd -dc "$IMG/rootfs.cpio.zst" | cpio -t --quiet 2>/dev/null | LC_ALL=C sort > "$STAGE/old.list"
cpio -t --quiet < "$STAGE/bin/rootfs.cpio" 2>/dev/null | LC_ALL=C sort > "$STAGE/new.list"
LC_ALL=C comm -3 "$STAGE/old.list" "$STAGE/new.list" > "$STAGE/list.diff" || true
say "file list: $(wc -l < "$STAGE/new.list") entries; $(wc -l < "$STAGE/list.diff") differ from the old image"
head -n 12 "$STAGE/list.diff" | sed 's/^/repack:   /'
rm -rf "$STAGE/root"

# ---- 4: post-image on a staging copy ----
for f in bzImage grub.img grub-eltorito.img; do cp -a "$IMG/$f" "$STAGE/bin/"; done
[ -z "$KFROM" ] || cp -f "$KFROM/images/bzImage" "$STAGE/bin/bzImage"
cp -a "$IMG/efi-part" "$STAGE/bin/"
# GRUB comes from the kernel donor too: its core images (grub.img, grub-eltorito.img, the EFI loader) carry the boot menu's modules
# (graphics, PNG, fonts, theme); a base built before that module list changed would show plain "file not found" errors instead
if [ -n "$KFROM" ]; then
  for f in grub.img grub-eltorito.img; do [ -f "$KFROM/images/$f" ] && cp -f "$KFROM/images/$f" "$STAGE/bin/$f"; done
  if [ -d "$KFROM/images/efi-part" ]; then rm -rf "$STAGE/bin/efi-part"; cp -a "$KFROM/images/efi-part" "$STAGE/bin/"; fi
fi
cp -a "$BLD/buildroot-fs/rootfs.iso9660.tmp" "$STAGE/build/buildroot-fs/"
# the ISO's 1 MB EFI partition image (fat.efi) is the same GRUB build as the donor's
if [ -n "$KFROM" ] && [ -f "$KFROM/build/buildroot-fs/rootfs.iso9660.tmp/boot/fat.efi" ]; then
  cp -f "$KFROM/build/buildroot-fs/rootfs.iso9660.tmp/boot/fat.efi" "$STAGE/build/buildroot-fs/rootfs.iso9660.tmp/boot/fat.efi"
fi
for g in "$BLD"/grub2-*; do ln -s "$g" "$STAGE/build/$(basename "$g")"; done
# the tree's kernel is the Buildroot one again (a signed copy from the last build must not be signed twice)
cp -f "$STAGE/bin/bzImage" "$STAGE/build/buildroot-fs/rootfs.iso9660.tmp/boot/bzImage"
cp -f "$STAGE/bin/rootfs.cpio.zst" "$STAGE/build/buildroot-fs/rootfs.iso9660.tmp/boot/initrd"
export BUILD_DIR="$STAGE/build" BINARIES_DIR="$STAGE/bin" HOST_DIR="$HOSTD" TARGET_DIR="$OUT/target"
say "post-image (ISO, install template)"
sh "$PI_SCRIPT" "$BINARIES_DIR" $PIARGS
say "genimage"
PATH="$HOSTD/bin:$PATH" bash "$GI_SCRIPT" $PIARGS
for f in iyakki-x86_64.iso iyakki-install-x86_64.img iyakki-boot.vfat; do [ -s "$STAGE/bin/$f" ] || die "$f was not produced; the old images are untouched"; done

# ---- 5: publish ----
say "replacing the images in $IMGD"
for f in rootfs.cpio rootfs.cpio.zst iyakki-x86_64.iso iyakki-install-x86_64.img iyakki-boot.vfat boot.img data-marker.img; do
  mv -f "$STAGE/bin/$f" "$IMGD/$f.new" && mv -f "$IMGD/$f.new" "$IMGD/$f"
done
rm -rf "$IMGD/install-root"; mv "$STAGE/bin/install-root" "$IMGD/install-root"
cp -a "$STAGE/bin/bzImage" "$IMGD/bzImage"
if [ "$BASE" = "$OUT" ]; then rsync -a "$STAGE/build/buildroot-fs/rootfs.iso9660.tmp/" "$BLD/buildroot-fs/rootfs.iso9660.tmp/"; fi
rm -rf "$STAGE"
say "done in $(( $(date +%s) - t0 )) s: $IMGD/iyakki-x86_64.iso, iyakki-install-x86_64.img"
say "note: rootfs.iso9660 and rootfs.tar in images/ are Buildroot's intermediate files and are NOT refreshed (a full make does)"
if [ "$BASE" = "$OUT" ]; then say "next: dist/buildroot/changed.sh $VARIANT --record"; else say "assembled image: it is not a Buildroot build of $OUT, so nothing is recorded; a later full build there starts from scratch"; fi
