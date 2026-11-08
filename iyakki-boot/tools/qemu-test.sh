#!/bin/sh
# SPDX-License-Identifier: MIT OR Apache-2.0
# Boot the UEFI loader under QEMU + OVMF and check the menu, the hold key, the fallback, boot counting and chain loading.
#   qemu-test.sh [--only a|a2|b|c|d|e|f|g|h|i|j|k|l] [--keep] [--lean-image IMAGE.img]
# Settings (environment; the defaults are relative to this repository):
#   CARGO_TARGET_DIR   where `cargo build --release --target x86_64-unknown-uefi -p boot-uefi -p test-efi` put its output
#   BOOT_EFI, TEST_EFI the two programs (default: under CARGO_TARGET_DIR, else ../target)
#   OVMF_CODE, OVMF_VARS  firmware (default /usr/share/OVMF/OVMF_CODE_4M.fd and OVMF_VARS_4M.fd)
#   BOOT_SCRATCH       directory for the throwaway disks (default: $XDG_CACHE_HOME or ~/.cache, then boot-loader-test)
#   LEAN_IMAGE         a disk image whose first partition has /boot/bzImage and /boot/initrd-lean, for the real-kernel test
#                      (read only; that test is skipped without it)
#   INITRD_MARKER      regex of a line that init in that initrd prints (checked after the kernel runs /init)
# Only files in the scratch directory are written; no disk or device is touched.
set -eu
HERE=$(cd "$(dirname "$0")" && pwd)
REPO=$(cd "$HERE/.." && pwd)
TARGET=${CARGO_TARGET_DIR:-$REPO/target}
BOOT_EFI=${BOOT_EFI:-$TARGET/x86_64-unknown-uefi/release/boot-uefi.efi}
TEST_EFI=${TEST_EFI:-$TARGET/x86_64-unknown-uefi/release/test-efi.efi}
SCRATCH=${BOOT_SCRATCH:-${XDG_CACHE_HOME:-$HOME/.cache}/boot-loader-test}
for t in qemu-system-x86_64 python3 sgdisk mkfs.vfat; do
	command -v "$t" >/dev/null 2>&1 || [ -x "/usr/sbin/$t" ] || { echo "qemu-test: $t is not installed" >&2; exit 1; }
done
LEAN=${LEAN_IMAGE:-}
EXTRA=""
while [ $# -gt 0 ]; do
	case $1 in
		--lean-image) LEAN=$2; shift 2 ;;
		--only) EXTRA="$EXTRA --only $2"; shift 2 ;;
		--keep) EXTRA="$EXTRA --keep"; shift ;;
		*) echo "usage: $0 [--only a|a2|b|c|d|e|f|g|h|i|j|k|l] [--keep] [--lean-image IMAGE.img]" >&2; exit 2 ;;
	esac
done
PATH=$PATH:/usr/sbin
export PATH
# shellcheck disable=SC2086
exec python3 -I "$HERE/qemu_test.py" --boot-efi "$BOOT_EFI" --test-efi "$TEST_EFI" \
	--ovmf-code "${OVMF_CODE:-/usr/share/OVMF/OVMF_CODE_4M.fd}" --ovmf-vars "${OVMF_VARS:-/usr/share/OVMF/OVMF_VARS_4M.fd}" \
	--scratch "$SCRATCH" --lean-image "$LEAN" --initrd-marker "${INITRD_MARKER:-}" $EXTRA
