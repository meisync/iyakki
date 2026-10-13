#!/bin/sh
# Build the JavaScript engine for the browser: QuickJS-ng plus our bridge, compiled to WebAssembly.
#   sh dist/js/build.sh        writes crates/iyakki-js/assets/iyakki-js.wasm
#
# The result is committed, so normal builds do not need this. Run it after changing crates/iyakki-js/glue or when
# upgrading QuickJS. The compiler is the WASI SDK (LLVM, Apache-2.0 with LLVM exceptions); it is downloaded once into
# target/toolchains and checked against a pinned SHA-256. The engine source is vendored in crates/iyakki-js/vendor.
set -eu
HERE="$(cd "$(dirname "$0")" && pwd)"
ROOT="$(cd "$HERE/../.." && pwd)"
SDK_VERSION=34
SDK_URL="https://github.com/WebAssembly/wasi-sdk/releases/download/wasi-sdk-$SDK_VERSION/wasi-sdk-$SDK_VERSION.0-x86_64-linux.tar.gz"
SDK_SHA256=b761e3a0721dbae9c09a0059e5fdb2bf917d1b4a8a7b430fb3b5aafb0984b2c4
TOOLS="$ROOT/target/toolchains"
SDK="$TOOLS/wasi-sdk-$SDK_VERSION.0-x86_64-linux"
QJS="$ROOT/crates/iyakki-js/vendor/quickjs-ng-0.17.0"
OUT="$ROOT/crates/iyakki-js/assets/iyakki-js.wasm"

if [ ! -x "$SDK/bin/clang" ]; then
  mkdir -p "$TOOLS"
  TMP="$(mktemp -d)"
  echo "downloading the WASI SDK $SDK_VERSION ..."
  curl -fL "$SDK_URL" -o "$TMP/sdk.tar.gz"
  echo "$SDK_SHA256  $TMP/sdk.tar.gz" | sha256sum -c - || { echo "the WASI SDK download does not match its pinned checksum" >&2; exit 1; }
  tar xzf "$TMP/sdk.tar.gz" -C "$TOOLS"
fi

# -O3 and no assertions (NDEBUG): measurably faster than -O2 with them; LTO, bulk-memory and the like made no difference
CFLAGS="-O3 -DNDEBUG -D_WASI_EMULATED_SIGNAL -D_WASI_EMULATED_PROCESS_CLOCKS -D_GNU_SOURCE -I$QJS"
WORK="$(mktemp -d)"
"$SDK/bin/clang" --target=wasm32-wasip1 $CFLAGS -c "$QJS/quickjs-amalgam.c" -o "$WORK/quickjs.o"
"$SDK/bin/clang" --target=wasm32-wasip1 $CFLAGS -c "$ROOT/crates/iyakki-js/glue/jsbridge.c" -o "$WORK/bridge.o"
"$SDK/bin/clang" --target=wasm32-wasip1 -mexec-model=reactor -O3 "$WORK/quickjs.o" "$WORK/bridge.o" -o "$OUT" \
  -lwasi-emulated-signal -lwasi-emulated-process-clocks \
  -Wl,--strip-all -Wl,--gc-sections -Wl,-z,stack-size=2097152
ls -l "$OUT"
sha256sum "$OUT"
