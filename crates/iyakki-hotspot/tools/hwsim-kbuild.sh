#!/bin/bash
set -e
# Builds the small test kernel (cfg80211, mac80211, mac80211_hwsim built in) from a 6.12 source tree into ~/iyakki-work/hwsim/kbuild.
# SRC defaults to the tree used for the images; run under flock and systemd-run like every build (see hwsim-test.sh).
SRC=$HOME/.cache/iyakki-tmp/sb/linux-6.12.112
O=$HOME/iyakki-work/hwsim/kbuild
cd $SRC
make O=$O tinyconfig
KCONFIG_CONFIG=$O/.config $SRC/scripts/kconfig/merge_config.sh -m -O $O $O/.config $(dirname "$0")/hwsim-kernel.frag
make O=$O olddefconfig
grep -E "CONFIG_(MAC80211_HWSIM|CFG80211|MAC80211|PACKET|NET_NS|RD_ZSTD|SERIAL_8250_CONSOLE|CRYPTO_CCM)=" $O/.config
make O=$O -j2 bzImage
ls -l $O/arch/x86/boot/bzImage
