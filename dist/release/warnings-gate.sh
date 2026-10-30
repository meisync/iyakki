#!/usr/bin/env bash
# The warnings gate: `cargo check --workspace --all-targets` must print no compiler warning at all.
# Not RUSTFLAGS="-D warnings": that changes the flags of every crate and rebuilds the whole tree; this reuses the normal cache
# (cargo replays the warnings of cached crates) and reports file:line for each one. Exit 1 on any warning or error.
set -uo pipefail
RELDIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
. "$RELDIR/env.sh"
cd "$ROOT"
out="$(mktemp "$TMPDIR/iyakki-warnings.XXXXXX")"
trap 'rm -f "$out"' EXIT
cargo check --workspace --all-targets --message-format short >"$out" 2>&1
rc=$?
n="$(grep -cE '(^|: )warning(\[|:)' "$out" || true)"
if [ "$rc" -ne 0 ]; then grep -E 'error' "$out" | head -40; echo "warnings gate: FAIL (cargo check failed)"; exit 1; fi
if [ "${n:-0}" -ne 0 ]; then grep -E '(^|: )warning(\[|:)' "$out" | head -80; echo "warnings gate: FAIL ($n warnings)"; exit 1; fi
echo "warnings gate: PASS (0 warnings, all targets)"
