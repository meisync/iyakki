#!/usr/bin/env bash
# PUSH MODE for development updates: the shortest path from a changed program to an update pack, WITHOUT a release build, and (full mode)
# WITHOUT the lean image (docs/development/COMPILE-SPEED-PLAN.md, "Measured results"). --lean makes the pack from a lean image (below).
#
#   quick-push.sh [--cargo] [--bin FILE] [--variant pc] [--base BASE_OUT] [--kernel-from KERNEL_OUT] [--out OUT] [--sign KEYFILE]
#                 [--version V] [--lean | --full] [--lean-level N] [--layers [--against DIR | --no-against]] -o PACKDIR [-- extra repack.sh arguments, e.g. --graft git --firmware-from DIR]
#
#   1. --cargo: one `cargo build --profile device` call for iyakki-core and iyakki-store-server (incremental, no LTO; IYAKKI_PROFILE=x to
#      choose another profile). Without --cargo the program already built there (or --bin FILE, any musl iyakki program) is used.
#   2. repack.sh <variant> --fast-zstd: the pruned and signed modules + firmware come from the cache (repack-cache.sh), so only the program,
#      the overlays and the packing are redone. The first run with a new donor / key / firmware set fills the cache and is as slow as before.
#   3. `iyakki dev update-pack` (the program just built, it is a static file and runs on the build computer) makes the payload from the install image.
#   3b. --lean (see below): lean-image.sh --root sfs --level N on the repacked image, and the pack comes from the LEAN image.
# NOTHING is sent to a device and no drive is written: the last line prints the command the person runs when ready (`iyakki dev update PACKDIR`,
# which needs the paired computer and the on-device approval). The image produced is a development image (fast zstd, device profile): it is
# never a release and the boot-test rule still applies (boot it in QEMU before the first push of a day).
# LEAN MODE (2026-11-11). A full-initrd pack (the 420 MB payload above) does NOT fit a lean install: its boot partition is 457 MB with about
# 21 MB free, because the system lives in boot/root.sfs there. For such a target the pack must come from the lean image. `--lean` (or
# IYAKKI_PUSH_TARGET=lean in the environment; `--full` overrides it) runs `lean-image.sh REPACKED_IMAGES LEANDIR --root sfs --level N` after the
# repack (N from `--lean-level`, default 3 for development speed; 19 for a nightly, see nightly-push.sh) and makes the pack from
# LEANDIR/iyakki-lean-x86_64.img. This script never asks the device what it is (that would be talking to it): the person says which kind of
# install the target is, with the flag or the environment variable. A lean pack is SIGNED: IYAKKI_SB_KEY and IYAKKI_SB_CERT must be set (and
# readable) or the script stops before building anything: without them the repack is silently unsigned and `iyakki dev update` refuses it
# (docs/development/COMPILE-SPEED-PLAN.md, "Lean push"). Needs the first stage built: target/x86_64-unknown-linux-musl/release/iyakki-stage1
# (IYAKKI_STAGE1=FILE names another). The boot-test rule still applies: nightly-push.sh boots the lean image in QEMU before it prints the commands.
# LAYERS MODE (2026-11-11, docs/development/LAYERED-ROOT-DESIGN.md). `--layers` (implies --lean) makes the lean image with base/core/ui layers
# (lean-image.sh --layers) and a v2 pack that carries only the layers that changed against the last build that was offered: `--against DIR` names
# that build (a lean-image layers folder: layers.json + the .sfs files; default $IYAKKI_PUSHED_DIR/VARIANT, IYAKKI_PUSHED_DIR defaulting to
# ~/iyakki-work/pushed-layers, outside the repository). With no such folder, or --no-against, every layer is carried whole. This script never
# records anything; nightly-push.sh records the build after its boot tests passed. The pack is for devices that have layers (a layered
# install): a device with a single root.sfs refuses a pack that keeps layers, in plain words, before anything is written.
# Defaults: variant pc, OUT = target/buildroot-VARIANT-push (a separate folder: the finished builds in target/ stay as they are), BASE =
# target/buildroot-web, KERNEL = target/buildroot-mac (the pc recipe of ./run.sh build-pc-layered).
set -eu
HERE="$(cd "$(dirname "$0")" && pwd)"
ROOT="${IYAKKI_ROOT:-$(cd "$HERE/../.." && pwd)}"
LEAN="${IYAKKI_PUSH_TARGET:-}"; case "$LEAN" in lean) LEAN=1 ;; ""|full) LEAN=0 ;; *) echo "quick-push.sh: IYAKKI_PUSH_TARGET must be lean or full (found: $LEAN)" >&2; exit 2 ;; esac
LEVEL=3; LAYERS=0; AGAINST=""; NOAGAINST=0
VARIANT=pc; BASE="$ROOT/target/buildroot-web"; KFROM="$ROOT/target/buildroot-mac"; OUT=""; PACK=""; SIGN=""; VER=""; BINF=""; CARGO=0
while [ $# -gt 0 ]; do
  case "$1" in
    --cargo) CARGO=1 ;;
    --lean) LEAN=1 ;;
    --full) LEAN=0 ;;
    --lean-level) LEVEL="$2"; shift ;;
    --layers) LAYERS=1; LEAN=1 ;;
    --against) AGAINST="$2"; shift ;;
    --no-against) NOAGAINST=1 ;;
    --bin) BINF="$2"; shift ;;
    --variant) VARIANT="$2"; shift ;;
    --base) BASE="$2"; shift ;;
    --kernel-from) KFROM="$2"; shift ;;
    --out) OUT="$2"; shift ;;
    --sign) SIGN="$2"; shift ;;
    --version) VER="$2"; shift ;;
    -o) PACK="$2"; shift ;;
    --) shift; break ;;
    -h|--help) sed -n '2,/^set -eu/p' "$0" | sed '$d'; exit 0 ;;
    *) echo "quick-push.sh: unknown option $1" >&2; exit 2 ;;
  esac
  shift
done
[ -n "$PACK" ] || { echo "quick-push.sh: -o PACKDIR is required (where the update payload goes)" >&2; exit 2; }
case "$LEVEL" in ''|*[!0-9]*) echo "quick-push.sh: --lean-level must be a number (3 to 19)" >&2; exit 2 ;; esac
if [ "$LEAN" = 1 ]; then
  # fail loudly BEFORE any build: an unsigned lean pack is refused by the device (feedback: signed builds need the SB environment)
  [ -n "${IYAKKI_SB_KEY:-}" ] && [ -n "${IYAKKI_SB_CERT:-}" ] || { echo "quick-push.sh: --lean needs IYAKKI_SB_KEY and IYAKKI_SB_CERT (the Secure Boot signing key and certificate); without them the image is unsigned and the device refuses the update" >&2; exit 2; }
  [ -r "$IYAKKI_SB_KEY" ] && [ -r "$IYAKKI_SB_CERT" ] || { echo "quick-push.sh: IYAKKI_SB_KEY / IYAKKI_SB_CERT name a file that is not readable" >&2; exit 2; }
  export IYAKKI_SB_KEY IYAKKI_SB_CERT
fi
case "${IYAKKI_IMAGE_PROFILE:-}" in release) echo "quick-push.sh: a release image is not a push-mode image (use repack.sh without --fast-zstd)" >&2; exit 2 ;; esac
: "${OUT:=$ROOT/target/buildroot-$VARIANT-push}"
export IYAKKI_PROFILE="${IYAKKI_PROFILE:-device}"
T=x86_64-unknown-linux-musl
if [ "$IYAKKI_PROFILE" = release ]; then PFLAG=--release; else PFLAG="--profile $IYAKKI_PROFILE"; fi
t0=$(date +%s)
step() { echo "quick-push: [$(( $(date +%s) - t0 )) s] $*"; }

if [ "$CARGO" = 1 ]; then
  step "cargo build $PFLAG (one call)"
  . "$HERE/linker.sh"; iyakki_export_rustflags "$T"; LDENV="$(iyakki_linker_env "$T" "$IYAKKI_PROFILE")"   # optional IYAKKI_LINKER=lld|mold|auto
  (cd "$ROOT" && env $LDENV cargo build $PFLAG --target "$T" -p iyakki-core -p iyakki-store-server)
fi
BIN="${BINF:-$ROOT/target/$T/$IYAKKI_PROFILE/iyakki}"
[ -x "$BIN" ] || { echo "quick-push.sh: $BIN is missing: use --cargo or --bin FILE" >&2; exit 1; }
export IYAKKI_REPACK_BIN="$BIN"
SBIN="$ROOT/target/$T/$IYAKKI_PROFILE/iyakki-store-server"; [ ! -x "$SBIN" ] || export IYAKKI_STORE_SERVER_BIN="$SBIN"

step "repack $VARIANT (cache for modules + firmware, zstd -3)"
"$HERE/repack.sh" "$VARIANT" "$OUT" --fast-zstd --base "$BASE" --kernel-from "$KFROM" "$@"

IMG="$OUT/images/iyakki-install-x86_64.img"
[ -s "$IMG" ] || { echo "quick-push.sh: $IMG was not produced" >&2; exit 1; }
if [ "$LEAN" = 1 ]; then
  LEANDIR="$OUT/lean"
  step "lean image (--root sfs --level $LEVEL) into $LEANDIR"
  LI=(--root sfs --level "$LEVEL"); [ -z "${IYAKKI_STAGE1:-}" ] || LI+=(--stage1 "$IYAKKI_STAGE1"); [ "$LAYERS" = 0 ] || LI+=(--layers)
  "$HERE/lean-image.sh" "$OUT/images" "$LEANDIR" "${LI[@]}"
  IMG="$LEANDIR/iyakki-lean-x86_64.img"
  [ -s "$IMG" ] || { echo "quick-push.sh: $IMG was not produced by lean-image.sh" >&2; exit 1; }
  # best effort: the kernel on the lean boot volume must carry the signature (sbverify is optional; its absence is a note)
  K="$OUT/images/install-root/boot/bzImage"
  if [ -s "$K" ] && command -v sbverify >/dev/null; then
    sbverify --cert "$IYAKKI_SB_CERT" "$K" >/dev/null 2>&1 || { echo "quick-push.sh: the kernel $K is not signed with IYAKKI_SB_CERT: stop (the device would refuse the update)" >&2; exit 1; }
  else
    echo "quick-push.sh: note: signature of the kernel not checked (sbverify missing or no install-root kernel); check the repack log for 'secure boot  signed'" >&2
  fi
fi
step "update-pack"
mkdir -p "$PACK"
UP=(dev update-pack "$IMG" -o "$PACK" --variant "$VARIANT")
[ -z "$VER" ] || UP+=(--version "$VER")
[ -z "$SIGN" ] || UP+=(--sign "$SIGN")
if [ "$LAYERS" = 1 ] && [ "$NOAGAINST" = 0 ]; then
  : "${AGAINST:=${IYAKKI_PUSHED_DIR:-$HOME/iyakki-work/pushed-layers}/$VARIANT}"
  if [ -s "$AGAINST/layers.json" ]; then UP+=(--against "$AGAINST"); step "layered pack against the last offered build: $AGAINST"
  else step "no earlier build in $AGAINST: every layer is carried whole"; fi
fi
"$BIN" "${UP[@]}"
if [ -n "${IYAKKI_QUICK_PUSH_NO_HINT:-}" ]; then   # nightly-push.sh: the update command is printed only after the boot test passed
  step "pack ready in $PACK (nothing was sent; not yet boot-tested)"
else
  step "done: payload in $PACK (nothing was sent; $([ "$LEAN" = 1 ] && echo "lean image, level $LEVEL" || echo "full-initrd image")). To install it on a paired device, the person runs:  iyakki dev update $PACK"
fi
