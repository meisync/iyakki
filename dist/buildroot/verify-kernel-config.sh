#!/bin/sh
# Checks that every option in board/iyakki/linux.fragment survives kconfig resolution unchanged
# (kconfig silently drops unknown symbols or ones whose dependencies are unmet).
#   [ARCH=x86_64|aarch64] [VARIANT=mac] ./verify-kernel-config.sh [kernel-source-dir]
#   VARIANT=mac or macweb (x86_64; the same kernel fragments, macweb only adds userspace) also checks board/iyakki/linux-mac.fragment, the optional Intel Mac drivers.
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
FRAGMENTS="$FRAGMENT"
if case "${VARIANT:-}" in mac|macweb) ;; *) false ;; esac; then
  [ "$KARCH" = x86_64 ] || { echo "VARIANT=mac/macweb is x86_64 only" >&2; exit 2; }
  FRAGMENTS="$FRAGMENT $HERE/board/iyakki/linux-mac.fragment"
fi
cd "$SRC"
# The Mac variant's kernel patches add drivers (Touch Bar) whose options only exist once applied: apply them to this source
# tree first (each one once: a patch that already reverses cleanly is taken as applied). Buildroot does the same at build time.
if [ "$FRAGMENTS" != "$FRAGMENT" ]; then
  for p in "$HERE"/board/iyakki/patches/linux/"$KVER"/*.patch; do
    [ -f "$p" ] || continue
    if patch -p1 -R -s -f --dry-run < "$p" >/dev/null 2>&1; then continue; fi
    patch -p1 -s -N --no-backup-if-mismatch < "$p" || { echo "FAIL: $(basename "$p") does not apply to $SRC"; exit 1; }
    echo "applied $(basename "$p")"
  done
fi
make -s ARCH=$KARCH mrproper
make -s ARCH=$KARCH defconfig
ARCH=$KARCH KCONFIG_CONFIG=.config scripts/kconfig/merge_config.sh -m -O . .config $FRAGMENTS >/dev/null
make -s ARCH=$KARCH olddefconfig
bad=0
cat $FRAGMENTS | grep -E '^CONFIG_[A-Za-z0-9_]+=' | while IFS= read -r l; do
  grep -qxF -- "$l" .config || { echo "NOT APPLIED: $l"; echo x >> .iyakki-bad; }
done
if [ -f .iyakki-bad ]; then rm .iyakki-bad; echo "FAIL: some options were not applied"; exit 1; fi
echo "OK: all fragment options applied"
