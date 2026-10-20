#!/usr/bin/env bash
# ./run.sh check-macos   (or: dist/release/check-macos.sh [aarch64-apple-darwin x86_64-apple-darwin ...])
# Does the code still COMPILE for macOS? `cargo check` (no linking, nothing runs) of the crates the desktop window is made
# of, for each installed macOS Rust target, so Linux-only code that slipped in without a `cfg` is found on any Linux machine.
#   iyakki-agent, iyakki-shell, iyakki-core (library, the window's feature set, and the default set),
#   and the window crate itself (crates/iyakki-gui, its own workspace and lock file): the iyakki-desktop binary.
# Result lines end with "check-macos: PASS", "check-macos: SKIP <why>" or "check-macos: FAIL <targets>". The exit code is 1
# only for FAIL. A missing target is a SKIP (rustup target add aarch64-apple-darwin needs the network).
# This proves the code compiles for macOS. It does NOT prove it runs: nothing is linked or started here.
# On a Linux machine there is no Apple C compiler, and a few dependencies (ring, wasmtime...) compile C in their build
# scripts. `check` never uses that object code, so unless a real cross compiler is configured (CC_<target>) a stand-in
# that writes empty object files is used. It is created in the build folder and is only ever used for `cargo check`.
set -uo pipefail
. "$(dirname "${BASH_SOURCE[0]}")/env.sh"
cd "$ROOT"
command -v cargo >/dev/null || { echo "check-macos: SKIP cargo not found"; exit 0; }
WANT=("$@"); [ ${#WANT[@]} = 0 ] && WANT=(aarch64-apple-darwin x86_64-apple-darwin)
HAVE="$(rustup target list --installed 2>/dev/null || true)"
TARGETS=(); for t in "${WANT[@]}"; do if grep -qx "$t" <<<"$HAVE"; then TARGETS+=("$t"); else echo "check-macos: target $t is not installed (rustup target add $t), skipped"; fi; done
[ ${#TARGETS[@]} = 0 ] && { echo "check-macos: SKIP no macOS Rust target installed (rustup target add aarch64-apple-darwin)"; exit 0; }

OUT="$ROOT/target/macos-check"; mkdir -p "$OUT"
export CARGO_TARGET_DIR="$OUT/target"
if [ "$(uname -s)" != Darwin ]; then
  cat >"$OUT/check-cc" <<'SH'
#!/bin/sh
# stand-in C compiler for `cargo check` only: answers cc-rs's probes, writes empty object files
case "$1" in -E) echo '"gcc"'; exit 0;; -\?) exit 1;; esac
out=""; prev=""; for a in "$@"; do [ "$prev" = "-o" ] && out="$a"; prev="$a"; done
case " $* " in *" --version "*|*" -dumpmachine "*|*" -print-search-dirs "*) echo check-cc; exit 0;; esac
[ -n "$out" ] && { mkdir -p "$(dirname "$out")"; : > "$out"; }
exit 0
SH
  chmod +x "$OUT/check-cc"
fi

fail=()
for t in "${TARGETS[@]}"; do
  u="${t//-/_}"
  if [ "$(uname -s)" != Darwin ] && [ -z "$(printenv "CC_$u")" ]; then export "CC_$u=$OUT/check-cc" "AR_$u=${AR:-ar}"; fi
  step() { # name, command...
    local name="$1" log; shift; log="$OUT/$t-$(printf %s "$name" | tr -c 'A-Za-z0-9.-' _).log"
    if "$@" >"$log" 2>&1; then echo "  ok    $t  $name"; else echo "  FAIL  $t  $name  (see $log)"; fail+=("$t:$name"); fi
  }
  step "iyakki-agent" cargo check --locked --target "$t" -p iyakki-agent
  step "iyakki-shell lib" cargo check --locked --target "$t" -p iyakki-shell --lib
  step "iyakki-core lib (window features)" cargo check --locked --target "$t" -p iyakki-core --lib --no-default-features --features shell
  step "iyakki-core lib+bin (default features)" cargo check --locked --target "$t" -p iyakki-core
  step "iyakki-desktop (window)" cargo check --locked --target "$t" --manifest-path crates/iyakki-gui/Cargo.toml --bin iyakki-desktop
done
if [ ${#fail[@]} = 0 ]; then echo "check-macos: PASS (${TARGETS[*]}; compiles, never run on a Mac)"; exit 0; fi
echo "check-macos: FAIL ${fail[*]}"; exit 1
