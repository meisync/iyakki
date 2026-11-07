#!/usr/bin/env bash
# What has to be rebuilt for an IYAKKI image variant?  docs/BUILD-INCREMENTAL.md
#
#   changed.sh <variant> [OUT_DIR]            print the cheapest action (never builds anything, never writes)
#   changed.sh <variant> --record [OUT_DIR]   remember the current inputs as "what the images in OUT_DIR were built from"
#   changed.sh <variant> --adopt  [OUT_DIR]   same, for images that were built before this script existed (you vouch they are current)
#   changed.sh <variant> --explain            also print every input group with its hash
#
# <variant> is what build.sh takes: libre|hw|mac|web|macweb (ARCH=aarch64 for the ARM image, like build.sh). The same environment
# switches count (WEB_H264, WEB_AV_ADDON, WEB_MEDIA, WEB_IRIS, WEB_CROCUS, WITH_FFMPEG, IYAKKI_PROFILE), because they change the composed defconfig.
#
# First line of the output is exactly one of
#   NOTHING             the images are up to date: reuse them
#   IYAKKI-ONLY         only the iyakki program / overlay / release texts / boot menu changed: dist/buildroot/repack.sh <variant>
#   PACKAGES a b c      only these Buildroot packages changed (new, or their directory/options changed): dirclean + make
#   KERNEL              the kernel (patches, fragments, version) or the Mac out-of-tree modules changed; PACKAGES may follow in the detail
#   FULL: reason        no usable earlier build, or a change Buildroot cannot apply incrementally (toolchain, architecture, removed package)
# followed by detail lines that start with two spaces. Exit status is 0 for every decision, 2 for a usage error.
#
# The record lives in OUT_DIR/.iyakki-changed/ (one file per input group, plus a copy of the composed defconfig).
set -eu

HERE="$(cd "$(dirname "$0")" && pwd)"
ROOT="${IYAKKI_ROOT:-$(cd "$HERE/../.." && pwd)}"
VARIANT=""; MODE=check; OUT=""; EXPLAIN=0
for a in "$@"; do
  case "$a" in
    --record) MODE=record ;;
    --adopt) MODE=adopt ;;
    --explain) EXPLAIN=1 ;;
    -*) echo "changed.sh: unknown option $a" >&2; exit 2 ;;
    *) if [ -z "$VARIANT" ]; then VARIANT="$a"; elif [ -z "$OUT" ]; then OUT="$a"; else echo "changed.sh: too many arguments" >&2; exit 2; fi ;;
  esac
done
[ -n "$VARIANT" ] || { sed -n '2,20p' "$0" >&2; exit 2; }
case "$VARIANT" in libre|hw|mac|web|macweb) ;; *) echo "changed.sh: unknown variant $VARIANT" >&2; exit 2 ;; esac
ARCH="${ARCH:-x86_64}"
case "$ARCH" in
  x86_64) RUST_TARGET=x86_64-unknown-linux-musl; : "${OUT:=$ROOT/target/buildroot-$VARIANT}" ;;
  aarch64) RUST_TARGET=aarch64-unknown-linux-musl; : "${OUT:=$ROOT/target/buildroot-$VARIANT-aarch64}" ;;
  *) echo "changed.sh: ARCH must be x86_64 or aarch64" >&2; exit 2 ;;
esac
PROFILE="${IYAKKI_PROFILE:-release}"
BR_VERSION="${BR_VERSION:-2026.02.3}"
BR_DIR="${BR_DIR:-$ROOT/target/buildroot-src/buildroot-$BR_VERSION}"
CFG="$HERE/configs"; BOARD="$HERE/board"
REC="$OUT/.iyakki-changed"
WORK="$(mktemp -d)"; trap 'rm -rf "$WORK"' EXIT

# ---- the defconfig build.sh would compose (keep in step with build.sh; the test compares it with a real OUT/defconfig) ----
compose() {
  cat "$CFG/iyakki_${ARCH}_defconfig"
  [ "$VARIANT" = hw ] && cat "$CFG/iyakki_firmware.fragment"
  if [ "$VARIANT" = mac ] || [ "$VARIANT" = macweb ]; then cat "$CFG/iyakki_firmware.fragment" "$CFG/iyakki_mac.fragment"; fi
  if [ "$VARIANT" = web ] || [ "$VARIANT" = macweb ]; then
    cat "$CFG/iyakki_web.fragment"
    if [ "${WEB_IRIS:-0}" = 1 ] || [ "${WEB_CROCUS:-0}" = 1 ]; then cat "$CFG/iyakki_web_iris.fragment"; fi
    [ "${WEB_CROCUS:-0}" = 1 ] && cat "$CFG/iyakki_web_crocus.fragment"
    if [ "${WEB_H264:-0}" = 1 ]; then cat "$CFG/iyakki_web_h264.fragment"
    elif [ "${WEB_AV_ADDON:-1}" = 1 ]; then
      cat "$CFG/iyakki_web_av_addon.fragment"
      [ "${WITH_FFMPEG:-0}" = 1 ] || cat "$CFG/iyakki_web_av_addon_libs.fragment"
    fi
    if [ "${WEB_MEDIA:-0}" = 1 ]; then cat "$CFG/iyakki_web_media.fragment"; fi
  fi
  [ "${WITH_FFMPEG:-0}" = 1 ] && cat "$CFG/iyakki_media.fragment"
  return 0
}
compose > "$WORK/defconfig"; [ -z "${IYAKKI_DUMP_DEFCONFIG:-}" ] || cp "$WORK/defconfig" "$IYAKKI_DUMP_DEFCONFIG"; [ -z "${IYAKKI_DUMP_DEFCONFIG:-}" ] || cp "$WORK/defconfig" "$IYAKKI_DUMP_DEFCONFIG"; [ -z "${IYAKKI_DUMP_DEFCONFIG:-}" ] || cp "$WORK/defconfig" "$IYAKKI_DUMP_DEFCONFIG"; [ -z "${IYAKKI_DUMP_DEFCONFIG:-}" ] || cp "$WORK/defconfig" "$IYAKKI_DUMP_DEFCONFIG"
# only real settings: comments and blank lines (and their edits) must not trigger anything
grep -E '^BR2_' "$WORK/defconfig" | LC_ALL=C sort -u > "$WORK/settings"

# ---- hashing helpers ----
hash_files() { # files or dirs (missing ones count as empty)
  for p in "$@"; do [ -e "$p" ] && find "$p" -type f -print0; done | LC_ALL=C sort -z | xargs -0 -r sha256sum 2>/dev/null \
    | sed "s# $ROOT/# #" | sha256sum | cut -c1-16
}
hash_text() { sha256sum | cut -c1-16; }
has() { grep -q "^$1=y" "$WORK/settings"; }

# ---- input groups ----
# toolchain: architecture, libc, compiler, kernel headers, C++/wchar: anything here forces a clean tree
grep -E '^BR2_(x86_64|aarch64|TOOLCHAIN|PACKAGE_HOST_LINUX_HEADERS|GCC|BINUTILS|INIT|ROOTFS_DEVICE|SYSTEM|TARGET_GENERIC|TARGET_ROOTFS|ROOTFS_SKELETON|ENABLE_|OPTIMIZE|STATIC|SHARED|PER_PACKAGE|REPRODUCIBLE)' "$WORK/settings" > "$WORK/g.toolchain" || true
# kernel: version/defconfig/fragments/patch dirs/signing keys + the files they name + apple-bce (out-of-tree module of the Mac images)
{ grep -E '^BR2_(LINUX|GLOBAL_PATCH_DIR)' "$WORK/settings" || true; } > "$WORK/g.kernel.cfg"
kfiles=("$BOARD/iyakki/linux.fragment" "$BOARD/iyakki/patches-sb" "$BOARD/iyakki/secureboot-test.fragment")
if grep -q 'patches ' "$WORK/g.kernel.cfg" || grep -q 'board/iyakki/patches"' "$WORK/g.kernel.cfg" || grep -q 'linux-mac.fragment' "$WORK/g.kernel.cfg"; then
  kfiles+=("$BOARD/iyakki/patches" "$BOARD/iyakki/linux-mac.fragment")
fi
[ "$ARCH" = aarch64 ] && kfiles=("$BOARD/iyakki-aarch64/linux.fragment")
K_SUM="$( { cat "$WORK/g.kernel.cfg"; hash_files "${kfiles[@]}"; } | hash_text)"
# GRUB (built-in modules + config): the same state build.sh's rebuild_if_changed grub2 watches
G_SUM="$( { grep '^BR2_TARGET_GRUB2' "$WORK/settings" || true; hash_files "$BOARD/iyakki/grub-pc-builtin.cfg" "$BOARD/iyakki/grub-efi-builtin.cfg"; } | hash_text)"
# the packages that live in this repository (one hash per directory); only those the variant enables
pkgdir_sums() {
  for d in "$HERE"/package/*/; do
    n="$(basename "$d")"; sym="BR2_PACKAGE_$(echo "$n" | tr 'a-z-' 'A-Z_')"
    has "$sym" && echo "$n $(hash_files "$d")"
  done
  return 0
}
pkgdir_sums > "$WORK/g.pkgdirs"
# everything that only lands in the root file system or the boot menu: a repack covers it
{
  echo "overlay $(hash_files "$BOARD/iyakki/overlay" $( [ "$VARIANT" = web ] || [ "$VARIANT" = macweb ] && echo "$BOARD/iyakki-web/overlay" ))"
  echo "hooks $(hash_files "$BOARD/iyakki/post-build.sh" "$BOARD/iyakki/prune-firmware.sh" "$BOARD/iyakki/mac-firmware.keep" "$BOARD/iyakki/pc-firmware.keep" "$BOARD/iyakki/restricted-firmware.sh" "$BOARD/iyakki/restricted-firmware.table" "$BOARD/iyakki/release-profile.sh" "$BOARD/iyakki/release-firmware.keep" "$BOARD/iyakki/release" $( [ "$VARIANT" = web ] || [ "$VARIANT" = macweb ] && echo "$BOARD/iyakki-web/web-post-build.sh" ))"
  echo "boot $(hash_files "$BOARD/iyakki/grub.cfg" "$BOARD/iyakki/grub" "$BOARD/iyakki/post-image.sh" "$BOARD/iyakki/genimage-install.cfg" "$HERE/secureboot.sh" "$BOARD/iyakki-aarch64/grub.cfg" "$BOARD/iyakki-aarch64/genimage.cfg" "$BOARD/iyakki-aarch64/post-image.sh")"
  echo "docs $(hash_files "$ROOT/LICENSE" "$ROOT/NOTICE" "$ROOT/LICENSING.md" "$ROOT/THIRD_PARTY_LICENSES.txt")"
  echo "samples $(hash_files "$ROOT/dist/samples/Music" "$ROOT/dist/samples/Videos" "$ROOT/dist/samples/README.md" "$ROOT/dist/samples/LICENSE")"
  if [ "$VARIANT" = web ] || [ "$VARIANT" = macweb ]; then echo "fonts $(hash_files "$ROOT"/crates/iyakki-shell/assets/NotoSans*.ttf "$ROOT/crates/iyakki-shell/assets/NOTO-OFL-LICENSE.txt")"; fi
  echo "env h264=${WEB_H264:-0} av=${WEB_AV_ADDON:-1} media=${WEB_MEDIA:-0} intel=${WEB_IRIS:-0}${WEB_CROCUS:-0} restricted=${IYAKKI_INCLUDE_RESTRICTED_FIRMWARE:-0} profile=${IYAKKI_IMAGE_PROFILE:-}"
} | sed 's/ /\t/' > "$WORK/g.rootfs"
# the iyakki program and the private store server (cargo output; absent when cargo has not run yet)
BIN="$ROOT/target/$RUST_TARGET/$PROFILE/iyakki"; SBIN="$ROOT/target/$RUST_TARGET/$PROFILE/iyakki-store-server"
{ [ -f "$BIN" ] && echo "iyakki $(sha256sum < "$BIN" | cut -c1-16)" || echo "iyakki missing"
  [ -f "$SBIN" ] && echo "store-server $(sha256sum < "$SBIN" | cut -c1-16)" || echo "store-server missing"
  echo "version $(sed -n 's/^version *= *"\(.*\)"/\1/p' "$ROOT/Cargo.toml" | head -n1)"; } | sed 's/ /\t/' > "$WORK/g.bin"
[ -n "${IYAKKI_SB_KEY:-}" ] && [ -n "${IYAKKI_SB_CERT:-}" ] && echo "SECURE_BOOT=signed" >> "$WORK/g.toolchain" || true
{ printf 'toolchain\t%s\n' "$(hash_text < "$WORK/g.toolchain")"
  printf 'kernel\t%s\n' "$K_SUM"
  printf 'grub\t%s\n' "$G_SUM"
  sed 's/^/pkgdir:/;s/ /\t/' "$WORK/g.pkgdirs"
  sed 's/^/rootfs:/' "$WORK/g.rootfs"
  sed 's/^/bin:/' "$WORK/g.bin"
  printf 'external\t%s\n' "$(hash_files "$HERE/external.mk" "$HERE/Config.in" "$HERE/external.desc")"
} > "$WORK/now"

if [ "$EXPLAIN" = 1 ]; then echo "inputs for $VARIANT ($ARCH):" >&2; sed 's/^/  /' "$WORK/now" >&2; fi

# ---- record / adopt ----
if [ "$MODE" != check ]; then
  [ -d "$OUT" ] || { echo "changed.sh: $OUT does not exist" >&2; exit 2; }
  if [ "$MODE" = record ] && [ ! -s "$OUT/images/rootfs.cpio.zst" ] && [ "$ARCH" = x86_64 ]; then
    echo "changed.sh: no $OUT/images/rootfs.cpio.zst: refusing to record a build that has no image" >&2; exit 2
  fi
  mkdir -p "$REC"
  cp "$WORK/now" "$REC/inputs"; cp "$WORK/settings" "$REC/settings"
  echo "$MODE $(date -u +%Y-%m-%dT%H:%M:%SZ)" > "$REC/when"
  echo "recorded: $REC"
  exit 0
fi

# ---- decide ----
reasons=()
full() { echo "FULL: $1"; shift; for r in "$@"; do echo "  - $r"; done; exit 0; }
if [ "$ARCH" = x86_64 ]; then IMG="$OUT/images/rootfs.cpio.zst"; else IMG="$OUT/images/iyakki-aarch64.img"; fi
[ -d "$OUT/build" ] || full "no earlier build in $OUT"
[ -s "$IMG" ] || full "no finished image ($IMG)"
[ -s "$REC/inputs" ] || full "no record of what the images were built from" "if the existing images are up to date: changed.sh $VARIANT --adopt"
old() { awk -F'\t' -v k="$1" '$1==k { sub(/^[^\t]*\t/, ""); print; exit }' "$REC/inputs"; }
new() { awk -F'\t' -v k="$1" '$1==k { sub(/^[^\t]*\t/, ""); print; exit }' "$WORK/now"; }
diffs() { # keys of one prefix whose value differs (or exists on one side only)
  { cut -f1 "$REC/inputs"; cut -f1 "$WORK/now"; } | LC_ALL=C sort -u | grep -E "^$1" | while IFS= read -r k; do
    [ "$(old "$k")" = "$(new "$k")" ] || echo "$k"
  done
}
# toolchain / architecture / libc / init / skeleton / ccache change: Buildroot would rebuild everything anyway
[ -z "$(diffs toolchain)" ] || full "toolchain, architecture or core system options changed" \
  "$(diff <(grep -E '^BR2_(x86_64|aarch64|TOOLCHAIN|PACKAGE_HOST_LINUX_HEADERS|GCC|BINUTILS|INIT|ROOTFS_DEVICE|SYSTEM|TARGET_GENERIC|TARGET_ROOTFS|ROOTFS_SKELETON|ENABLE_|OPTIMIZE|STATIC|SHARED|PER_PACKAGE|REPRODUCIBLE)' "$REC/settings" || true) "$WORK/g.toolchain" | grep '^[<>]' | head -n 6 | tr '\n' ' ')"
[ -z "$(diffs external)" ] || full "external.mk / Config.in changed (the package list of the tree may have changed)"

# package symbols that were added or removed since the record
grep -E '^BR2_PACKAGE_[A-Z0-9_]+=' "$REC/settings" | LC_ALL=C sort > "$WORK/old.pk" || true
grep -E '^BR2_PACKAGE_[A-Z0-9_]+=' "$WORK/settings" | LC_ALL=C sort > "$WORK/new.pk" || true
LC_ALL=C comm -23 "$WORK/old.pk" "$WORK/new.pk" > "$WORK/pk.gone" || true
LC_ALL=C comm -13 "$WORK/old.pk" "$WORK/new.pk" > "$WORK/pk.new" || true
# a symbol name -> the package it belongs to: the longest prefix that is a package directory of Buildroot or of this tree
known="$WORK/known"; { ls "$BR_DIR/package" 2>/dev/null; ls "$HERE/package" 2>/dev/null; } | tr 'A-Z-' 'a-z_' | LC_ALL=C sort -u > "$known"
pkg_of() { # BR2_PACKAGE_FOO_BAR_BAZ=y -> foo-bar (longest known prefix) or the first word
  s="$(echo "${1%%=*}" | sed 's/^BR2_PACKAGE_//' | tr 'A-Z' 'a-z')"; best=""
  host=""; case "$s" in host_*) host=host-; s="${s#host_}" ;; esac
  p="$s"; while [ -n "$p" ]; do
    if grep -qx -- "$p" "$known"; then best="$p"; break; fi
    case "$p" in *_*) p="${p%_*}" ;; *) p="" ;; esac
  done
  [ -n "$best" ] || best="${s%%_*}"
  echo "$host$best" | tr '_' '-'
}
# removed packages: Buildroot leaves their files in target/ until it is cleaned
gone_pk=""
while IFS= read -r l; do [ -n "$l" ] || continue; case "$l" in *=y) gone_pk="$gone_pk $(pkg_of "$l")" ;; esac; done < "$WORK/pk.gone"
gone_only=""; still=""
for p in $gone_pk; do grep -qE "^BR2_PACKAGE_$(echo "$p" | tr 'a-z-' 'A-Z_')=y" "$WORK/settings" || gone_only="$gone_only $p"; done
# a sub-option switched off while its package stays on: that package is reconfigured
for p in $gone_pk; do case " $gone_only " in *" $p "*) ;; *) still="$still $p" ;; esac; done
[ -z "$gone_only" ] || full "package(s) switched off:$(echo "$gone_only" | tr ' ' '\n' | LC_ALL=C sort -u | tr '\n' ' ')" "Buildroot keeps the files of a removed package in target/ until a clean build"

pk=""; added_pk=""
while IFS= read -r l; do [ -n "$l" ] || continue; added_pk="$added_pk $(pkg_of "$l")"; done < "$WORK/pk.new"
pk="$added_pk $still"
# a package whose directory in this tree changed: dirclean and build again; apple-bce goes with the kernel
for k in $(diffs pkgdir:); do pk="$pk ${k#pkgdir:}"; done
# GRUB options (modules, built-in config)
[ -z "$(diffs grub)" ] || pk="$pk grub2"
# other option changes inside the settings (not package presence): kernel lines are handled by the kernel group, the rest by name
{ diff "$REC/settings" "$WORK/settings" || true; } | grep -E '^[<>] BR2_' | sed 's/^[<>] //' | LC_ALL=C sort -u > "$WORK/optdiff"
while IFS= read -r l; do
  case "$l" in BR2_LINUX*|BR2_GLOBAL_PATCH_DIR*|BR2_TARGET_GRUB2*|BR2_ROOTFS_OVERLAY*|BR2_ROOTFS_POST*|BR2_TARGET_ROOTFS*|BR2_PACKAGE_*=y) continue ;; esac
  case "$l" in BR2_PACKAGE_*) pk="$pk $(pkg_of "$l")" ;; esac
done < "$WORK/optdiff"
pk="$(echo $pk | tr ' ' '\n' | grep -v '^$' | LC_ALL=C sort -u | tr '\n' ' ' | sed 's/ $//')"

kernel=0
[ -z "$(diffs kernel)" ] || kernel=1
case " $pk " in *" apple-bce "*) kernel=1; pk="$(echo " $pk " | sed 's/ apple-bce / /;s/^ //;s/ $//')" ;; esac

root_changed="$(diffs 'rootfs:'; diffs 'bin:' )"
overlay_cfg=0
{ diff <(grep -E '^BR2_(ROOTFS_OVERLAY|ROOTFS_POST)' "$REC/settings" || true) <(grep -E '^BR2_(ROOTFS_OVERLAY|ROOTFS_POST)' "$WORK/settings" || true) >/dev/null; } || overlay_cfg=1
# the missing-binary case: nothing to compare, nothing to repack with
note_bin=""; grep -q '^bin:iyakki	missing' "$WORK/now" && note_bin="iyakki is not built (cargo build --release --target $RUST_TARGET -p iyakki-core) so it was not compared"

if [ "$kernel" = 1 ]; then
  echo "KERNEL"
  echo "  - kernel patches / fragments / version, or the Mac out-of-tree modules changed: make linux-dirclean (build.sh does it) then make"
  [ -z "$pk" ] || echo "  - PACKAGES: $pk"
  [ -z "$root_changed" ] || echo "  - also changed (picked up by the same make): $(echo $root_changed | tr '\n' ' ')"
  [ -z "$note_bin" ] || echo "  - $note_bin"
  exit 0
fi
if [ -n "$pk" ]; then
  echo "PACKAGES $pk"
  echo "  - for each: make O=$OUT <pkg>-dirclean (or -reconfigure for option-only changes), then make"
  [ -z "$root_changed" ] || echo "  - also changed (picked up by the same make): $(echo $root_changed | tr '\n' ' ')"
  [ -z "$note_bin" ] || echo "  - $note_bin"
  exit 0
fi
if [ -n "$root_changed" ] || [ "$overlay_cfg" = 1 ]; then
  echo "IYAKKI-ONLY"
  for k in $root_changed; do echo "  - changed: $k"; done
  [ "$overlay_cfg" = 0 ] || echo "  - overlay / post-build hook list changed in the defconfig"
  [ -z "$note_bin" ] || echo "  - $note_bin"
  exit 0
fi
echo "NOTHING"
echo "  - the images in $OUT/images are up to date; reuse them"
[ -z "$note_bin" ] || echo "  - $note_bin"
exit 0
