#!/usr/bin/env bash
# KI-1034: make-pi-image.sh refuses a userland root whose etc/iyakki-release is not this workspace's version, and passes one that is
# (it then stops at the boot-volume step, because the USB writer here is a stub). Nothing is built: the fixtures are a few text files.
# Run: dist/rpi/test-make-pi-image.sh
set -euo pipefail
HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT="$(cd "$HERE/../.." && pwd)"
IMG="$HERE/make-pi-image.sh"
WANT="$(sed -n 's/^version *= *"\(.*\)"/\1/p' "$ROOT/Cargo.toml" | head -n1)"
T="$(mktemp -d)"; trap 'rm -rf "${T:?}"' EXIT
fail() { echo "FAIL $*" >&2; exit 1; }

# fixtures: a kernel folder, a stage 1 file, a failing USB writer stub, and a userland root with the given release VERSION
mkdir -p "$T/k/modroot/lib/modules/6.12.0-test/kernel" "$T/out"
echo kernel > "$T/k/kernel8.img"; echo dtb > "$T/k/bcm2712-rpi-5-b.dtb"; echo 6.12.0-test > "$T/k/kernel.release"
echo init > "$T/stage1"; printf '#!/bin/sh\nexit 1\n' > "$T/usb"; chmod 0755 "$T/usb"
root() {  # root VERSION -> $T/u/rootfs.cpio.gz
  rm -rf "$T/r" && mkdir -p "$T/r/etc" "$T/r/bin" "$T/u"
  printf 'VERSION=%s\nVARIANT=rpi5\n' "$1" > "$T/r/etc/iyakki-release"
  (cd "$T/r" && find . | cpio --quiet -o -H newc | gzip -n > "$T/u/rootfs.cpio.gz")
}
run() { "$IMG" --kernel "$T/k" --userland "$T/u" --stage1 "$T/stage1" --usb "$T/usb" --out "$T/out" 2>&1; }

# 1. a stale root (an alpha.1 release file) is refused with the version named
root 0.1.0-alpha.1
if out="$(run)"; then fail "a stale root was accepted"; fi
grep -q "workspace's $WANT" <<<"$out" || fail "the refusal does not name the workspace version: $out"
grep -q "0.1.0-alpha.1" <<<"$out" || fail "the refusal does not name the root's version: $out"

# 2. a root with the workspace version passes the version gate (it fails later, at the stub USB writer, not at the version check)
root "$WANT"
if out="$(run)"; then fail "the USB stub should stop the run"; fi
grep -q "this workspace" <<<"$out" && fail "a current root was refused: $out"

# 3. no arguments: refused with the usage text
if "$IMG" >/dev/null 2>&1; then fail "no arguments was accepted"; fi

echo "ok   make-pi-image refuses a stale root (workspace $WANT) and passes a current one"
