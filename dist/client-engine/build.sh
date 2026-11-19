#!/bin/sh
# Build the client engine for the browser: crates/iyakki-client-engine compiled to WebAssembly (wasm32-unknown-unknown, no wasm-bindgen).
#   sh dist/client-engine/build.sh           writes crates/iyakki-client-engine/assets/iyakki-client-engine.wasm
#   sh dist/client-engine/build.sh --check   builds to a temporary place and fails if the committed file differs in its exports or imports
#
# The result is committed (about 200 KB, built from our own code and crates the workspace already uses: serde_json, miniz_oxide,
# ed25519-dalek, sha2), so normal builds do not need the wasm32 target. Run it after changing the engine; the gate runs --check.
# Needs: rustup target add wasm32-unknown-unknown
set -eu
HERE="$(cd "$(dirname "$0")" && pwd)"
ROOT="$(cd "$HERE/../.." && pwd)"
OUT="$ROOT/crates/iyakki-client-engine/assets/iyakki-client-engine.wasm"
cd "$ROOT"
. "$ROOT/dist/buildroot/linker.sh"; _f="$(iyakki_remap_flags)"   # no builder path (home folder, user name) in the committed module
[ -z "$_f" ] || export CARGO_TARGET_WASM32_UNKNOWN_UNKNOWN_RUSTFLAGS="$_f"
CARGO_BUILD_JOBS="${CARGO_BUILD_JOBS:-3}" cargo build --release --locked --target wasm32-unknown-unknown -p iyakki-client-engine
BUILT="$(cargo metadata --format-version 1 --no-deps | sed -n 's/.*"target_directory":"\([^"]*\)".*/\1/p')/wasm32-unknown-unknown/release/iyakki_client_engine.wasm"
if [ "${1:-}" = "--check" ]; then
  node "$HERE/check.mjs" "$BUILT" "$OUT"
else
  cp "$BUILT" "$OUT"
  node "$HERE/check.mjs" "$OUT"
  ls -l "$OUT"
  sha256sum "$OUT"
fi
