#!/bin/sh
# Checks that every option in board/iyakki/linux.fragment survives kconfig resolution unchanged
# (kconfig silently drops unknown symbols or ones whose dependencies are unmet).
#   [ARCH=x86_64|aarch64] ./verify-kernel-config.sh [kernel-source-dir]
set -eu
HERE="$(cd "$(dirname "$0")" && pwd)"
KVER="${KVER:-6.12.112}"
SRC="${1:-${TMPDIR:-/tmp}/linux-$KVER}"
if [ ! -d "$SRC" ]; then
  mkdir -p "$(dirname "$SRC")"
  curl -fL "https://cdn.kernel.org/pub/linux/kernel/v6.x/linux-$KVER.tar.xz" | tar xJ -C "$(dirname "$SRC")"
fi
ARCH_NAME="${ARCH:-x86_64}"
case "$ARCH_NAME" in
  x86_64)  KARCH=x86_64; FRAGMENT="$HERE/board/iyakki/linux.fragment" ;;
  aarch64) KARCH=arm64;  FRAGMENT="$HERE/board/iyakki-aarch64/linux.fragment" ;;
  *) echo "ARCH must be x86_64 or aarch64" >&2; exit 2 ;;
esac
cd "$SRC"
make -s ARCH=$KARCH mrproper
make -s ARCH=$KARCH defconfig
ARCH=$KARCH KCONFIG_CONFIG=.config scripts/kconfig/merge_config.sh -m -O . .config "$FRAGMENT" >/dev/null
make -s ARCH=$KARCH olddefconfig
bad=0
grep -E '^CONFIG_[A-Za-z0-9_]+=' "$FRAGMENT" | while IFS= read -r l; do
  grep -qxF -- "$l" .config || { echo "NOT APPLIED: $l"; echo x >> .iyakki-bad; }
done
if [ -f .iyakki-bad ]; then rm .iyakki-bad; echo "FAIL: some options were not applied"; exit 1; fi
echo "OK: all fragment options applied"
