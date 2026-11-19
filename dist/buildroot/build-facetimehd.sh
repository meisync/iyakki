#!/bin/sh
# SPDX-License-Identifier: GPL-2.0-or-later
# Builds the FaceTime HD camera modules (facetimehd.ko + videobuf2-dma-sg.ko) against the finished Mac kernel tree WITHOUT a kernel rebuild and
# puts them where dist/buildroot/repack.sh picks them up (<donor>/target/lib/modules/<ver>/updates), so `./run.sh build-macweb-layered`
# includes them. Same source, hash and Kbuild as the Buildroot package dist/buildroot/package/facetimehd (that package is the reference recipe).
#   dist/buildroot/build-facetimehd.sh [--donor target/buildroot-mac] [--check]
# --check builds nothing: it verifies the modules already in the donor (alias, vermagic, depends) and exits non-zero when wrong.
# Writes only under the donor's target/ and a temp dir. Needs the donor's toolchain (host/bin) and its configured kernel tree.
set -eu
ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
DONOR="$ROOT/target/buildroot-mac"; CHECK=0
while [ $# -gt 0 ]; do case "$1" in --donor) DONOR="$2"; shift ;; --check) CHECK=1 ;; *) echo "usage: $0 [--donor DIR] [--check]" >&2; exit 2 ;; esac; shift; done
case "$DONOR" in /*) ;; *) DONOR="$ROOT/$DONOR" ;; esac
PKG="$ROOT/dist/buildroot/package/facetimehd"
COMMIT="$(sed -n 's/^FACETIMEHD_VERSION *= *//p' "$PKG/facetimehd.mk")"
SHA="$(sed -n "s/^sha256  \([0-9a-f]*\)  facetimehd-.*\.tar\.gz\$/\1/p" "$PKG/facetimehd.hash")"
KDIR="$(ls -d "$DONOR"/build/linux-[0-9]* 2>/dev/null | head -n1 || true)"
[ -n "$KDIR" ] && [ -f "$KDIR/Module.symvers" ] || { echo "facetimehd: no built kernel tree in $DONOR/build" >&2; exit 1; }
KVER="$(ls "$DONOR/target/lib/modules" | head -n1)"
UP="$DONOR/target/lib/modules/$KVER/updates"
verify() {
  fail=0
  for m in facetimehd videobuf2-dma-sg; do
    ko="$UP/$m.ko"; [ -f "$ko" ] || { echo "MISSING $ko"; fail=1; continue; }
    vm="$(strings "$ko" | sed -n 's/^vermagic=//p' | head -n1)"
    echo "$m: vermagic=$vm"
    case "$vm" in "$KVER SMP preempt mod_unload"*) ;; *) echo "BAD vermagic for $m (want $KVER SMP preempt mod_unload)"; fail=1 ;; esac
  done
  strings "$UP/facetimehd.ko" 2>/dev/null | grep -q '^alias=pci:v000014E4d00001570' || { echo "BAD: facetimehd has no alias pci:v000014E4d00001570*"; fail=1; }
  dep="$(strings "$UP/facetimehd.ko" 2>/dev/null | sed -n 's/^depends=//p' | head -n1)"
  echo "facetimehd: depends=$dep"
  for d in videobuf2-v4l2 videobuf2-common videobuf2-dma-sg videodev; do
    case ",$dep," in *",$d,"*) ;; *) [ "$d" = videodev ] || { echo "BAD: depends lacks $d"; fail=1; } ;; esac
  done
  return $fail
}
if [ "$CHECK" = 1 ]; then verify && echo "facetimehd: modules OK"; exit $?; fi
W="$(mktemp -d "${TMPDIR:-/tmp}/fthd.XXXXXX")"; trap 'rm -rf "$W"' EXIT
TGZ="${IYAKKI_FTHD_TARBALL:-$ROOT/dl/facetimehd/facetimehd-$COMMIT.tar.gz}"
if [ ! -f "$TGZ" ]; then
  mkdir -p "$(dirname "$TGZ")"
  curl -fsSL -o "$TGZ.part" "https://github.com/patjak/facetimehd/archive/$COMMIT.tar.gz" && mv "$TGZ.part" "$TGZ"
fi
[ "$(sha256sum "$TGZ" | cut -d' ' -f1)" = "$SHA" ] || { echo "facetimehd: tarball sha256 differs from facetimehd.hash" >&2; exit 1; }
mkdir -p "$W/src"; tar -xzf "$TGZ" -C "$W/src" --strip-components=1
cp "$KDIR/drivers/media/common/videobuf2/videobuf2-dma-sg.c" "$W/src/videobuf2-dma-sg.c"
cp "$PKG/Kbuild" "$W/src/Kbuild"; rm -f "$W/src/Makefile"
make -C "$KDIR" M="$W/src" ARCH=x86_64 CROSS_COMPILE="$DONOR/host/bin/x86_64-linux-" HOSTCC="${HOSTCC:-gcc}" modules >"$W/build.log" 2>&1 || { tail -30 "$W/build.log" >&2; echo "facetimehd: build failed" >&2; exit 1; }
mkdir -p "$UP"
for m in facetimehd videobuf2-dma-sg; do cp -f "$W/src/$m.ko" "$UP/$m.ko"; chmod 0644 "$UP/$m.ko"; done
verify
echo "facetimehd: modules installed in $UP (the next repack adds them and runs depmod)"
