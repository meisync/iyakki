#!/usr/bin/env bash
# ./run.sh check-linux-cfg   (or: dist/release/check-linux-cfg.sh [target ...])
# Does the Linux-only code (cfg(target_os = "linux")) COMPILE without warnings? Meant for a Mac (or any machine) that cannot build it
# natively: `cargo check` for a Linux Rust target, no linking and nothing run. Default targets: aarch64-unknown-linux-gnu
# x86_64-unknown-linux-musl (the images use musl). A target that is not installed is skipped with a hint.
# Checks, per target: `cargo check --locked --workspace --all-targets` (lib, bins, tests, examples, benches) and the window crate
# crates/iyakki-gui (its own workspace and lock file), all bins and tests. Then counts compiler warnings (repo rule: ZERO).
# Needs: rustup with the target(s) (rustup target add <target>), cargo. On a non-Linux host a stand-in C compiler is used, because build
# scripts of dependencies (ring, wasmtime...) compile C; `check` never uses that object code. A real cross compiler is used when
# CC_<target> is already set. Output: target/linux-cfg/<target>.log; the warnings and errors are printed with file:line.
# Exit: 0 clean, 1 errors or warnings, 2 "nothing was checked" (cargo, target or compiler missing).
# It cannot see: tests/code run on Linux (nothing runs), build.rs output that differs by host, #[cfg(feature)] sets other than the
# default (--all-features is not used), and link errors (no linking happens).
set -uo pipefail
. "$(dirname "${BASH_SOURCE[0]}")/env.sh"
cd "$ROOT"
command -v cargo >/dev/null && command -v rustup >/dev/null || { echo "check-linux-cfg: nothing was checked (cargo or rustup not found)"; exit 2; }
WANT=("$@"); [ ${#WANT[@]} = 0 ] && WANT=(aarch64-unknown-linux-gnu x86_64-unknown-linux-musl)
HAVE="$(rustup target list --installed 2>/dev/null || true)"
TARGETS=(); for t in "${WANT[@]}"; do if grep -qx "$t" <<<"$HAVE"; then TARGETS+=("$t"); else echo "check-linux-cfg: target $t is not installed (rustup target add $t), skipped"; fi; done
[ ${#TARGETS[@]} = 0 ] && { echo "check-linux-cfg: nothing was checked (no Linux Rust target installed)"; exit 2; }
OUT="$ROOT/target/linux-cfg"; mkdir -p "$OUT"
if [ "$(uname -s)" != Linux ]; then
  command -v cc >/dev/null || { echo "check-linux-cfg: nothing was checked (no C compiler to stand in for build scripts)"; exit 2; }
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
bad=0
for t in "${TARGETS[@]}"; do
  u="${t//-/_}"
  if [ "$(uname -s)" != Linux ] && [ -z "$(printenv "CC_$u")" ]; then export "CC_$u=$OUT/check-cc" "AR_$u=${AR:-ar}"; fi
  log="$OUT/$t.log"; : >"$log"; rc=0
  CARGO_TARGET_DIR="$OUT/target" cargo check --locked --target "$t" --workspace --all-targets --message-format short >>"$log" 2>&1 || rc=1
  CARGO_TARGET_DIR="$OUT/target-gui" cargo check --locked --target "$t" --manifest-path crates/iyakki-gui/Cargo.toml --bins --tests --message-format short >>"$log" 2>&1 || rc=1
  # not counted: cargo summary lines ("generated N warning") and the musl-only "dropping unsupported crate type cdylib" note (a wasm tile crate)
  wl="$(grep -E '(^|: )warning(\[|:)' "$log" | grep -vE 'generated [0-9]+ warning|dropping unsupported crate type' || true)"
  n="$(printf '%s' "$wl" | grep -c . || true)"
  if [ "$rc" != 0 ]; then grep -E '(^|: )error' "$log" | head -60; fi
  if [ "${n:-0}" != 0 ]; then printf '%s\n' "$wl" | sort -u | head -80; fi
  if [ "$rc" != 0 ] || [ "${n:-0}" != 0 ]; then echo "check-linux-cfg: $t FAIL (errors: $rc, warnings: ${n:-0}; $log)"; bad=1; else echo "check-linux-cfg: $t PASS (0 warnings)"; fi
done
exit $bad
