#!/usr/bin/env bash
# PUSH MODE for development updates: the shortest path from a changed program to an update pack, WITHOUT the lean image and WITHOUT a release
# build (docs/development/COMPILE-SPEED-PLAN.md, "Measured results").
#
#   quick-push.sh [--cargo] [--bin FILE] [--variant pc] [--base BASE_OUT] [--kernel-from KERNEL_OUT] [--out OUT] [--sign KEYFILE]
#                 [--version V] -o PACKDIR [-- extra repack.sh arguments, e.g. --graft git --firmware-from DIR]
#
#   1. --cargo: one `cargo build --profile device` call for iyakki-core and iyakki-store-server (incremental, no LTO; IYAKKI_PROFILE=x to
#      choose another profile). Without --cargo the program already built there (or --bin FILE, any musl iyakki program) is used.
#   2. repack.sh <variant> --fast-zstd: the pruned and signed modules + firmware come from the cache (repack-cache.sh), so only the program,
#      the overlays and the packing are redone. The first run with a new donor / key / firmware set fills the cache and is as slow as before.
#   3. `iyakki dev update-pack` (the program just built, it is a static file and runs on the build computer) makes the payload from the install image.
# NOTHING is sent to a device and no drive is written: the last line prints the command the person runs when ready (`iyakki dev update PACKDIR`,
# which needs the paired computer and the on-device approval). The image produced is a development image (fast zstd, device profile): it is
# never a release and the boot-test rule still applies (boot it in QEMU before the first push of a day).
# Defaults: variant pc, OUT = target/buildroot-VARIANT-push (a separate folder: the finished builds in target/ stay as they are), BASE =
# target/buildroot-web, KERNEL = target/buildroot-mac (the pc recipe of ./run.sh build-pc-layered).
set -eu
HERE="$(cd "$(dirname "$0")" && pwd)"
ROOT="${IYAKKI_ROOT:-$(cd "$HERE/../.." && pwd)}"
VARIANT=pc; BASE="$ROOT/target/buildroot-web"; KFROM="$ROOT/target/buildroot-mac"; OUT=""; PACK=""; SIGN=""; VER=""; BINF=""; CARGO=0
while [ $# -gt 0 ]; do
  case "$1" in
    --cargo) CARGO=1 ;;
    --bin) BINF="$2"; shift ;;
    --variant) VARIANT="$2"; shift ;;
    --base) BASE="$2"; shift ;;
    --kernel-from) KFROM="$2"; shift ;;
    --out) OUT="$2"; shift ;;
    --sign) SIGN="$2"; shift ;;
    --version) VER="$2"; shift ;;
    -o) PACK="$2"; shift ;;
    --) shift; break ;;
    -h|--help) sed -n '2,19p' "$0"; exit 0 ;;
    *) echo "quick-push.sh: unknown option $1" >&2; exit 2 ;;
  esac
  shift
done
[ -n "$PACK" ] || { echo "quick-push.sh: -o PACKDIR is required (where the update payload goes)" >&2; exit 2; }
case "${IYAKKI_IMAGE_PROFILE:-}" in release) echo "quick-push.sh: a release image is not a push-mode image (use repack.sh without --fast-zstd)" >&2; exit 2 ;; esac
: "${OUT:=$ROOT/target/buildroot-$VARIANT-push}"
export IYAKKI_PROFILE="${IYAKKI_PROFILE:-device}"
T=x86_64-unknown-linux-musl
if [ "$IYAKKI_PROFILE" = release ]; then PFLAG=--release; else PFLAG="--profile $IYAKKI_PROFILE"; fi
t0=$(date +%s)
step() { echo "quick-push: [$(( $(date +%s) - t0 )) s] $*"; }

if [ "$CARGO" = 1 ]; then
  step "cargo build $PFLAG (one call)"
  (cd "$ROOT" && cargo build $PFLAG --target "$T" -p iyakki-core -p iyakki-store-server)
fi
BIN="${BINF:-$ROOT/target/$T/$IYAKKI_PROFILE/iyakki}"
[ -x "$BIN" ] || { echo "quick-push.sh: $BIN is missing: use --cargo or --bin FILE" >&2; exit 1; }
export IYAKKI_REPACK_BIN="$BIN"
SBIN="$ROOT/target/$T/$IYAKKI_PROFILE/iyakki-store-server"; [ ! -x "$SBIN" ] || export IYAKKI_STORE_SERVER_BIN="$SBIN"

step "repack $VARIANT (cache for modules + firmware, zstd -3)"
"$HERE/repack.sh" "$VARIANT" "$OUT" --fast-zstd --base "$BASE" --kernel-from "$KFROM" "$@"

IMG="$OUT/images/iyakki-install-x86_64.img"
[ -s "$IMG" ] || { echo "quick-push.sh: $IMG was not produced" >&2; exit 1; }
step "update-pack"
mkdir -p "$PACK"
UP=(dev update-pack "$IMG" -o "$PACK")
[ -z "$VER" ] || UP+=(--version "$VER")
[ -z "$SIGN" ] || UP+=(--sign "$SIGN")
"$BIN" "${UP[@]}"
step "done: payload in $PACK (nothing was sent). To install it on a paired device, the person runs:  iyakki dev update $PACK"
