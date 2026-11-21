#!/bin/sh
# Test of board/iyakki/modcompress-config.sh on made-up defconfigs (F-PKG-11). No build, no kernel.
set -eu
HERE="$(cd "$(dirname "$0")" && pwd)"
S="$HERE/board/iyakki/modcompress-config.sh"
W="$(mktemp -d)"; trap 'rm -rf "$W"' EXIT
fail() { echo "FAIL: $1" >&2; exit 1; }
cat > "$W/d" <<'EOT'
BR2_x86_64=y
BR2_LINUX_KERNEL_CONFIG_FRAGMENT_FILES="$(BR2_EXTERNAL_IYAKKI_PATH)/board/iyakki/linux.fragment"
BR2_LINUX_KERNEL_CONFIG_FRAGMENT_FILES="$(BR2_EXTERNAL_IYAKKI_PATH)/board/iyakki/linux.fragment $(BR2_EXTERNAL_IYAKKI_PATH)/board/iyakki/linux-mac.fragment"
BR2_TOOLCHAIN=y
EOT
cp "$W/d" "$W/off"
sh "$S" "$W/off"; cmp -s "$W/d" "$W/off" || fail "changed the file without IYAKKI_MODULE_COMPRESS=1"
cp "$W/d" "$W/on"
IYAKKI_MODULE_COMPRESS=1 sh "$S" "$W/on"
[ "$(grep -c 'linux-modcompress.fragment"$' "$W/on")" = 2 ] || fail "both fragment lines should end with the compress fragment"
grep -q '^BR2_x86_64=y$' "$W/on" && grep -q '^BR2_TOOLCHAIN=y$' "$W/on" || fail "another line was changed"
grep -q 'linux.fragment $(BR2_EXTERNAL_IYAKKI_PATH)/board/iyakki/linux-mac.fragment $(BR2_EXTERNAL_IYAKKI_PATH)/board/iyakki/linux-modcompress.fragment"' "$W/on" || fail "the Mac list lost an entry"
cp "$W/on" "$W/twice"; IYAKKI_MODULE_COMPRESS=1 sh "$S" "$W/twice"; cmp -s "$W/on" "$W/twice" || fail "a second run changed the file"
printf 'BR2_x86_64=y\n' > "$W/none"
if IYAKKI_MODULE_COMPRESS=1 sh "$S" "$W/none" 2>/dev/null; then fail "a defconfig with no fragment line was accepted"; fi
# the fragment itself asks for what post-build.sh produces: gzip modules, and a kernel that reads them
F="$HERE/board/iyakki/linux-modcompress.fragment"
grep -q '^CONFIG_MODULE_COMPRESS_GZIP=y$' "$F" && grep -q '^CONFIG_MODULE_DECOMPRESS=y$' "$F" || fail "the fragment does not ask for gzip modules and decompression"
echo "ok: the compressed-module fragment is added only when asked, once, to every fragment list"
