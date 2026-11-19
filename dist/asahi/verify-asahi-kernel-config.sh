#!/bin/sh
# Checks board/iyakki-apple/linux-asahi.fragment against a linux-asahi source tree (or any kernel tree, to see what is mainline).
#   dist/asahi/verify-asahi-kernel-config.sh <kernel-source-dir> [--symbols-only]
# --symbols-only (read-only, safe on a shared tree): lists symbols that no Kconfig file of the tree defines.
# Without it: runs kconfig in a COPY of the tree's config area (needs a writable tree you own) and also reports options that
# kconfig dropped because a dependency was unmet. Exit 0 only when nothing is missing or dropped.
set -eu
HERE="$(cd "$(dirname "$0")/.." && pwd)"
FRAG="$HERE/buildroot/board/iyakki-apple/linux-asahi.fragment"
BASE="$HERE/buildroot/board/iyakki-aarch64/linux.fragment"
SRC="${1:?usage: $0 <kernel-source-dir> [--symbols-only]}"
[ -d "$SRC/arch/arm64" ] || { echo "not a kernel tree: $SRC" >&2; exit 2; }
missing=0
for s in $(grep -oE '^CONFIG_[A-Z0-9_]+=' "$FRAG" | sed 's/^CONFIG_//; s/=$//'); do
  if ! grep -rqE "^[[:space:]]*(menu)?config $s\$" --include='Kconfig*' "$SRC" 2>/dev/null; then
    echo "NOT IN TREE: CONFIG_$s"; missing=$((missing+1))
  fi
done
if [ "${2:-}" = "--symbols-only" ]; then
  [ "$missing" = 0 ] && echo "OK: every symbol exists in this tree" || echo "$missing symbol(s) are not defined by this tree"
  [ "$missing" = 0 ]; exit
fi
cd "$SRC"
make -s ARCH=arm64 mrproper
make -s ARCH=arm64 defconfig
[ -f arch/arm64/configs/asahi.config ] && make -s ARCH=arm64 asahi.config || true
ARCH=arm64 KCONFIG_CONFIG=.config scripts/kconfig/merge_config.sh -m -O . .config "$BASE" "$FRAG" >/dev/null
make -s ARCH=arm64 olddefconfig
dropped=0
for l in $(cat "$BASE" "$FRAG" | grep -E '^CONFIG_[A-Za-z0-9_]+=' ); do
  grep -qxF -- "$l" .config || { echo "NOT APPLIED: $l"; dropped=$((dropped+1)); }
done
[ "$dropped" = 0 ] && [ "$missing" = 0 ] && { echo "OK: all fragment options applied"; exit 0; }
echo "FAIL: $missing missing, $dropped dropped"; exit 1
