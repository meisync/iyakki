#!/bin/sh
# Measures the size of the headless web service (F-WEBSVC-15) against the full program, both in the release profile (fat LTO, as the
# images and the container are built), and prints the numbers; with an image folder as the first argument, also the image files.
#   dist/buildroot/headless-size.sh [target/buildroot-headless]
# Builds only what is missing (cargo is incremental), one cargo at a time under the shared build lock and the memory cap of the other
# build scripts. Never run it while another build runs on the same machine.
set -eu
HERE="$(cd "$(dirname "$0")" && pwd)"; ROOT="$(cd "$HERE/../.." && pwd)"
T=x86_64-unknown-linux-musl
capped() { flock "$HOME/.cache/iyakki-build.lock" systemd-run --user --scope -q -p MemoryMax=9G -p MemorySwapMax=6G nice env CARGO_BUILD_JOBS="${CARGO_BUILD_JOBS:-2}" "$@"; }
. "$HERE/linker.sh"; iyakki_export_rustflags "$T"
HL="$ROOT/target/headless/$T/release/iyakki"; FULL="$ROOT/target/$T/release/iyakki"
[ -x "$HL" ] || (cd "$ROOT" && CARGO_TARGET_DIR="$ROOT/target/headless" capped cargo build --release --locked --target "$T" -p iyakki-core --no-default-features --features headless)
[ -x "$FULL" ] || (cd "$ROOT" && capped cargo build --release --locked --target "$T" -p iyakki-core)
size() { stat -c %s "$1"; }
h=$(size "$HL"); f=$(size "$FULL")
echo "headless program: $h bytes ($((h / 1048576)) MiB)"
echo "full program:     $f bytes ($((f / 1048576)) MiB)"
echo "headless is $((100 * h / f)) percent of the full program"
if [ -n "${1:-}" ] && [ -d "$1/images" ]; then
  for i in "$1"/images/*.iso "$1"/images/*.img "$1"/images/*.raw "$1"/images/rootfs.*; do [ -f "$i" ] && echo "image $(basename "$i"): $(size "$i") bytes"; done
fi
