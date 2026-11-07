#!/bin/sh
# SPDX-License-Identifier: MIT OR Apache-2.0
# Secure Boot test of the loader in the shim chain (QEMU + OVMF with Secure Boot on, throwaway keys). See the header of qemu_sb_test.py.
#   qemu-secureboot-test.sh --kernel BZIMAGE [--shim SHIM.efi] [--mmx MM.efi] [--boot-efi LOADER.efi] [--keep]
# The loader must be built without BOOT_CONFIG_PATH (it then reads /boot.cfg, which the test writes). Only a temp directory is written.
set -eu
HERE=$(cd "$(dirname "$0")" && pwd)
PATH=$PATH:/usr/sbin
export PATH
exec python3 -I "$HERE/qemu_sb_test.py" "$@"
