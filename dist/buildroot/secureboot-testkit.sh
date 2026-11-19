#!/bin/sh
# Build the small test kernel and test module the Secure Boot test (qemu-secureboot-test.py) boots.  Takes a few minutes, writes only
# into KITDIR.     secureboot-testkit.sh KITDIR [path to linux-6.12.112.tar.xz]
# The kernel is 6.12.112 + the patch of board/iyakki/patches-sb + board/iyakki/secureboot-test.fragment. Needs gcc, make, flex, bison,
# libelf and openssl headers (the same as a kernel build). No system change.
set -eu
KIT="${1:?usage: secureboot-testkit.sh KITDIR [linux-6.12.112.tar.xz]}"; TAR="${2:-}"
HERE="$(cd "$(dirname "$0")" && pwd)"
KVER=6.12.112
mkdir -p "$KIT"; KIT="$(cd "$KIT" && pwd)"
WORK="$KIT/work"; mkdir -p "$WORK"
if [ ! -d "$WORK/linux-$KVER" ]; then
  if [ -z "$TAR" ]; then
    TAR="$KIT/linux-$KVER.tar.xz"
    [ -f "$TAR" ] || curl -fL -o "$TAR" "https://cdn.kernel.org/pub/linux/kernel/v6.x/linux-$KVER.tar.xz"
  fi
  tar xJf "$TAR" -C "$WORK"
  (cd "$WORK/linux-$KVER" && patch -p1 -s < "$HERE"/board/iyakki/patches-sb/linux/$KVER/0001-x86-lock-down-the-kernel-when-booted-with-UEFI-Secure-Boot.patch)
fi
SRC="$WORK/linux-$KVER"; O="$WORK/build"; mkdir -p "$O"
make -s -C "$SRC" O="$O" ARCH=x86_64 tinyconfig
"$SRC/scripts/kconfig/merge_config.sh" -m -O "$O" "$O/.config" "$HERE/board/iyakki/secureboot-test.fragment" >/dev/null 2>&1
make -s -C "$SRC" O="$O" ARCH=x86_64 olddefconfig
for want in CONFIG_MODULE_SIG=y CONFIG_SECURITY_LOCKDOWN_LSM=y CONFIG_INTEGRITY_MACHINE_KEYRING=y CONFIG_LOAD_UEFI_KEYS=y CONFIG_EFI_STUB=y; do
  grep -qx "$want" "$O/.config" || { echo "secureboot-testkit: $want was dropped by kconfig" >&2; exit 1; }
done
make -C "$SRC" O="$O" ARCH=x86_64 -j"${JOBS:-6}" bzImage modules scripts >"$WORK/build.log" 2>&1 || { tail -20 "$WORK/build.log" >&2; exit 1; }
mkdir -p "$WORK/mod"
cat > "$WORK/mod/sbtest.c" <<'EOC'
#include <linux/module.h>
#include <linux/init.h>
static int __init sbtest_init(void) { pr_info("IYAKKI-SB-TEST module code is running\n"); return 0; }
static void __exit sbtest_exit(void) {}
module_init(sbtest_init);
module_exit(sbtest_exit);
MODULE_LICENSE("GPL");
MODULE_DESCRIPTION("Secure Boot test module");
EOC
echo 'obj-m += sbtest.o' > "$WORK/mod/Kbuild"
make -s -C "$SRC" O="$O" M="$WORK/mod" ARCH=x86_64 modules
cp "$O/arch/x86/boot/bzImage" "$KIT/bzImage"
cp "$WORK/mod/sbtest.ko" "$KIT/sbtest.ko"
cp "$O/scripts/sign-file" "$KIT/sign-file"
echo "secureboot-testkit: kit ready in $KIT (bzImage, sbtest.ko, sign-file); you may delete $WORK"
