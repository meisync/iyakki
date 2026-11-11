#!/bin/sh
# Build IYAKKI's own boot loader (the iyakki-boot repository, a git submodule) so an image build needs nothing made by hand.
#   loader-build.sh            prints the path of the built loader on stdout; progress and problems go to stderr
# The commit built is the one the submodule is pinned to (a build is the same for everyone). For development, IYAKKI_LOADER_REF=<branch|tag|sha>
# fetches that ref of the submodule's remote first (needs network). Incremental: cargo rebuilds only what changed. Not signed here (see boot-loader-stage.sh).
# Exit 0 with the path, or 1 with a message that says what is missing and how to fix it.
set -eu
HERE="$(cd "$(dirname "$0")" && pwd)"; REPO="$(cd "$HERE/../.." && pwd)"
SUB="$REPO/iyakki-boot"
OUT="$SUB/target/x86_64-unknown-uefi/release/boot-uefi.efi"
die() { echo "loader-build: $*" >&2; exit 1; }

if [ ! -f "$SUB/Cargo.toml" ]; then
  if [ -e "$REPO/.git" ] && [ -f "$REPO/.gitmodules" ]; then
    echo "loader-build: fetching the boot loader submodule (git submodule update --init iyakki-boot)" >&2
    git -C "$REPO" submodule update --init iyakki-boot >&2 || die "could not fetch the submodule; clone with --recursive or put the iyakki-boot sources in $SUB"
  fi
  [ -f "$SUB/Cargo.toml" ] || die "no boot loader sources in $SUB (clone with --recursive, or git submodule update --init)"
fi
if [ -n "${IYAKKI_LOADER_REF:-}" ]; then
  git -C "$SUB" fetch -q origin "$IYAKKI_LOADER_REF" >&2 && git -C "$SUB" checkout -q FETCH_HEAD >&2 || die "could not fetch $IYAKKI_LOADER_REF"
  echo "loader-build: using $IYAKKI_LOADER_REF ($(git -C "$SUB" rev-parse --short HEAD)); the submodule pointer in the main repository is NOT changed" >&2
fi
command -v cargo >/dev/null 2>&1 || die "cargo not found (install Rust from rustup.rs)"
if command -v rustup >/dev/null 2>&1 && ! rustup target list --installed 2>/dev/null | grep -qx x86_64-unknown-uefi; then
  echo "loader-build: adding the Rust target x86_64-unknown-uefi" >&2
  rustup target add x86_64-unknown-uefi >&2 || die "could not add the Rust target x86_64-unknown-uefi (rustup target add x86_64-unknown-uefi)"
fi
# the config file name is compiled in: boot-loader-stage.sh refuses a loader built without it
( cd "$SUB" && BOOT_CONFIG_PATH=/EFI/iyakki-boot/boot.cfg cargo build --release --locked -p boot-uefi --target x86_64-unknown-uefi >&2 ) || die "the loader did not build (see the cargo output above)"
[ -s "$OUT" ] || die "the build finished but $OUT is missing"
grep -aq '/EFI/iyakki-boot/boot\.cfg' "$OUT" || die "$OUT was built without BOOT_CONFIG_PATH"
echo "$OUT"
