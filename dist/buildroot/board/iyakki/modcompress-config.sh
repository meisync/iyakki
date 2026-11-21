#!/bin/sh
# F-PKG-11: add the compressed-module kernel fragment to the kernel fragment list of a Buildroot defconfig, only when asked
# (IYAKKI_MODULE_COMPRESS=1). Usage: modcompress-config.sh DEFCONFIG
# The list is the value of BR2_LINUX_KERNEL_CONFIG_FRAGMENT_FILES (a later line of the file wins in Buildroot, so every such line is
# changed). The fragment is added once (a second run changes nothing), and a defconfig with no such line is an error: the kernel would
# be built without the option while post-build.sh compresses the modules, and a kernel that cannot read .ko.gz cannot load any module.
set -eu
F="${1:?usage: modcompress-config.sh DEFCONFIG}"
[ "${IYAKKI_MODULE_COMPRESS:-0}" = 1 ] || exit 0
[ -f "$F" ] || { echo "modcompress-config: no $F" >&2; exit 2; }
grep -q '^BR2_LINUX_KERNEL_CONFIG_FRAGMENT_FILES="' "$F" || { echo "modcompress-config: $F has no BR2_LINUX_KERNEL_CONFIG_FRAGMENT_FILES line; compressed modules would not load" >&2; exit 2; }
ADD='$(BR2_EXTERNAL_IYAKKI_PATH)/board/iyakki/linux-modcompress.fragment'
tmp="$F.mc.$$"
while IFS= read -r line || [ -n "$line" ]; do
  case "$line" in
    BR2_LINUX_KERNEL_CONFIG_FRAGMENT_FILES=\"*)
      case "$line" in
        *linux-modcompress.fragment*) printf '%s\n' "$line" ;;
        *) printf '%s %s"\n' "${line%\"}" "$ADD" ;;
      esac ;;
    *) printf '%s\n' "$line" ;;
  esac
done < "$F" > "$tmp"
mv "$tmp" "$F"
