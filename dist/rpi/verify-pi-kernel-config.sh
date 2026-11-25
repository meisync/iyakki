#!/bin/sh
# Checks that every option in the Pi 5 kernel fragment (and the shared aarch64 one, for the options the Pi needs from it) survives kconfig
# resolution unchanged: kconfig silently drops unknown symbols and ones whose dependencies are unmet (the same check verify-kernel-config.sh does
# for the PC kernels). Resolves bcm2712_defconfig + the fragments in KERNEL_SRC; builds nothing.
#   dist/rpi/verify-pi-kernel-config.sh KERNEL_SRC_DIR [CROSS_COMPILE_PREFIX]
set -eu
HERE="$(cd "$(dirname "$0")" && pwd)"; ROOT="$(cd "$HERE/../.." && pwd)"
SRC="${1:?kernel source dir (extracted Raspberry Pi tree)}"; CROSS="${2:-}"
FR_BASE="$ROOT/dist/buildroot/board/iyakki-aarch64/linux.fragment"; FR_PI="$ROOT/dist/buildroot/board/iyakki-rpi5/linux.fragment"
. "$HERE/kernel.pin"
cd "$SRC"
M="make -s ARCH=arm64 ${CROSS:+CROSS_COMPILE=$CROSS}"
$M mrproper
$M "$DEFCONFIG"
ARCH=arm64 KCONFIG_CONFIG=.config scripts/kconfig/merge_config.sh -m -O . .config "$FR_BASE" "$FR_PI" >/dev/null
$M olddefconfig
bad=0
# every `CONFIG_X=v` line and every `# CONFIG_X is not set` line of the Pi fragment must be in .config as written
grep -E '^(CONFIG_[A-Za-z0-9_]+=.*|# CONFIG_[A-Za-z0-9_]+ is not set)$' "$FR_PI" | while IFS= read -r line; do
  name="${line#\# }"; name="${name%%[= ]*}"
  if ! grep -qxF "$line" .config; then
    got="$(grep -E "^(# )?$name[= ]" .config | head -n1)"
    echo "DROPPED $line   (kernel has: ${got:-nothing})"
    echo x >> "$SRC/.verify-bad"
  fi
done
if [ -f "$SRC/.verify-bad" ]; then rm -f "$SRC/.verify-bad"; echo "FAIL: options of board/iyakki-rpi5/linux.fragment did not survive kconfig"; exit 1; fi
# the things the lean start cannot do without, and the Pi's pages
for need in CONFIG_SQUASHFS=y CONFIG_SQUASHFS_ZSTD=y CONFIG_OVERLAY_FS=y CONFIG_BLK_DEV_LOOP=y CONFIG_RD_ZSTD=y CONFIG_MMC_BLOCK=y CONFIG_VFAT_FS=y CONFIG_EXT4_FS=y CONFIG_ARM64_4K_PAGES=y CONFIG_DRM_VC4=y CONFIG_MFD_RP1=y CONFIG_PCIE_BRCMSTB=y CONFIG_MACB=y; do
  grep -qx "$need" .config || { echo "FAIL: $need is not in the resolved config"; exit 1; }
done
! grep -q '^CONFIG_ARM64_16K_PAGES=y' .config || { echo "FAIL: 16 KiB pages are still on"; exit 1; }
echo "ok: the Pi 5 kernel fragment resolves as written ($(grep -c '^CONFIG_' .config) options set)"
