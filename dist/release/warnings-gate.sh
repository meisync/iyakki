#!/usr/bin/env bash
# The warnings gate: `cargo check --workspace --all-targets` must print no compiler warning at all.
# Not RUSTFLAGS="-D warnings": that changes the flags of every crate and rebuilds the whole tree; this reuses the normal cache
# (cargo replays the warnings of cached crates) and reports file:line for each one. Exit 1 on any warning or error.
set -uo pipefail
RELDIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
. "$RELDIR/env.sh"
cd "$ROOT"
# `--mac` (KI-063): the same check on an Apple Silicon Mac through mcargo, so warnings of code that is compiled only off Linux are counted too.
# A few Linux-only targets cannot compile on a Mac at all (errors, not warnings); `--keep-going` checks everything else, and only warnings count.
if [ "${1:-}" = "--mac" ]; then
  MC="${MCARGO:-$HOME/iyakki-work/bin/mcargo}"
  [ -x "$MC" ] || { echo "warnings gate (mac): SKIPPED (no mcargo at $MC)"; exit 0; }
  out="$(mktemp "$TMPDIR/iyakki-warnings-mac.XXXXXX")"; trap 'rm -f "$out"' EXIT
  "$MC" "$ROOT" check --workspace --all-targets --keep-going --message-format short >"$out" 2>&1
  w="$(grep -E '(^|: )warning(\[|:)' "$out" | grep -vE 'linker|generated [0-9]+ warning|build failed' || true)"
  if [ -n "$w" ]; then echo "$w" | sort -u | head -80; echo "warnings gate (mac): FAIL"; exit 1; fi
  echo "warnings gate (mac): PASS (0 warnings on macOS; Linux-only targets that do not compile there are not counted)"; exit 0
fi
out="$(mktemp "$TMPDIR/iyakki-warnings.XXXXXX")"
trap 'rm -f "$out"' EXIT
cargo check --workspace --all-targets --message-format short >"$out" 2>&1
rc=$?
n="$(grep -cE '(^|: )warning(\[|:)' "$out" || true)"
if [ "$rc" -ne 0 ]; then grep -E 'error' "$out" | head -40; echo "warnings gate: FAIL (cargo check failed)"; exit 1; fi
if [ "${n:-0}" -ne 0 ]; then grep -E '(^|: )warning(\[|:)' "$out" | head -80; echo "warnings gate: FAIL ($n warnings)"; exit 1; fi
echo "warnings gate: PASS (0 warnings, all targets)"
