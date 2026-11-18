#!/usr/bin/env bash
# Build the private store server as one static (musl) file:  dist/store-server/build-static.sh [x86_64|aarch64]
# Prints the path of the finished file on the last line. Needs `rustup target add <arch>-unknown-linux-musl` and a linker:
#   x86_64:  the system C compiler (musl-gcc is fine too); the Rust toolchain brings its own musl start-up files.
#   aarch64: a cross C compiler. Used in this order: $IYAKKI_AARCH64_CC, the Buildroot toolchain of `./run.sh build-arm`
#            (target/buildroot-libre-aarch64/host/bin/aarch64-*-linux-musl-gcc), aarch64-linux-musl-gcc, aarch64-linux-gnu-gcc.
# The same build is what the wizard's "Build the store server now" runs (docs/STORE.md, section 13).
set -euo pipefail
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
ARCH="${1:-$(uname -m)}"
case "$ARCH" in
  x86_64|amd64) ARCH=x86_64 ;;
  aarch64|arm64) ARCH=aarch64 ;;
  *) echo "build-static: arch must be x86_64 or aarch64" >&2; exit 2 ;;
esac
TARGET="$ARCH-unknown-linux-musl"
rustup target list --installed 2>/dev/null | grep -qx "$TARGET" || { echo "build-static: the Rust target is missing: rustup target add $TARGET" >&2; exit 1; }
if [ "$ARCH" = aarch64 ] && [ "$(uname -m)" != aarch64 ]; then
  CROSS="${IYAKKI_AARCH64_CC:-}"
  [ -n "$CROSS" ] || CROSS="$(ls "$ROOT"/target/buildroot-libre-aarch64/host/bin/aarch64-*-linux-musl-gcc 2>/dev/null | head -n1 || true)"
  [ -n "$CROSS" ] || CROSS="$(command -v aarch64-linux-musl-gcc || command -v aarch64-linux-gnu-gcc || true)"
  [ -n "$CROSS" ] || { echo "build-static: no aarch64 cross compiler (set IYAKKI_AARCH64_CC, or build the arm image once: ./run.sh build-arm)" >&2; exit 1; }
  export CARGO_TARGET_AARCH64_UNKNOWN_LINUX_MUSL_LINKER="$CROSS" CC_aarch64_unknown_linux_musl="$CROSS"
fi
cd "$ROOT"
cargo build --release --locked --target "$TARGET" -p iyakki-store-server
OUT="$ROOT/target/$TARGET/release/iyakki-store-server"
[ -f "$OUT" ] || { echo "build-static: $OUT was not produced" >&2; exit 1; }
echo "$OUT"
