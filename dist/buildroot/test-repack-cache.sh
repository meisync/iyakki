#!/usr/bin/env bash
# Test of repack-cache.sh (the pruned + signed modules/firmware cache of repack.sh) on fake folders: no Buildroot, no image, no cargo,
# nothing real is touched. Run: dist/buildroot/test-repack-cache.sh
set -eu
REAL="$(cd "$(dirname "$0")" && pwd)"
W="$(mktemp -d "${TMPDIR:-$HOME/.cache}/iyakki-repack-cache-test.XXXXXX")"
trap 'rm -rf "$W"' EXIT
pass=0; fail=0
ok()   { pass=$((pass+1)); echo "ok   - $1"; }
bad()  { fail=$((fail+1)); echo "FAIL - $1"; }
check() { if eval "$2"; then ok "$1"; else bad "$1"; fi; }

# a private copy of the dist/buildroot files the key reads, so a test can change them
HERE="$W/here"; mkdir -p "$HERE/board/iyakki"
for f in prune-firmware.sh iwlwifi-keep.sh pc-firmware.keep mac-firmware.keep post-build.sh; do cp "$REAL/board/iyakki/$f" "$HERE/board/iyakki/"; done
ROOT="$W/repo"; mkdir -p "$ROOT"
KFROM="$W/mac"; mkdir -p "$KFROM/images" "$KFROM/target/lib/modules/6.1/updates"; echo cpio > "$KFROM/images/rootfs.cpio"
FWFROM="$W/fw"; mkdir -p "$FWFROM"; echo whence > "$FWFROM/WHENCE"; echo copy > "$FWFROM/copy-firmware.sh"
BASE="$W/web"; mkdir -p "$BASE/images"; echo zst > "$BASE/images/rootfs.cpio.zst"
OVL="$W/overlay"; mkdir -p "$OVL/etc"; echo x > "$OVL/etc/motd"
printf 'cert1' > "$W/sb.crt"; printf 'key1' > "$W/sb.key"
export HERE ROOT KFROM FWFROM BASE IMG="$BASE/images" OVERLAYS="$OVL" VARIANT=pc IMAGE_PROFILE=""
unset IYAKKI_SB_KEY IYAKKI_SB_CERT IYAKKI_SIGN_FILE IYAKKI_REPACK_CACHE || true
# shellcheck disable=SC1091
. "$REAL/repack-cache.sh"

k0="$(rc_key)"
check "the key is a 24-character hash" "[ \${#k0} = 24 ]"
check "the same inputs give the same key" "[ \"\$(rc_key)\" = '$k0' ]"
check "the cache lives under target/repack-cache of the repository (not /tmp)" "[ \"\$(rc_dir)\" = '$ROOT/target/repack-cache' ] && case \"\$(rc_file $k0)\" in $ROOT/target/repack-cache/fwmod-pc-*.tar.zst) true ;; *) false ;; esac"
echo '# more' >> "$HERE/board/iyakki/pc-firmware.keep"
k1="$(rc_key)"; check "a changed keep list changes the key" "[ '$k1' != '$k0' ]"
echo '# more' >> "$HERE/board/iyakki/prune-firmware.sh"
k2="$(rc_key)"; check "a changed prune script changes the key" "[ '$k2' != '$k1' ]"
echo 'more cpio' >> "$KFROM/images/rootfs.cpio"
k3="$(rc_key)"; check "a new kernel donor rootfs changes the key" "[ '$k3' != '$k2' ]"
touch "$KFROM/target/lib/modules/6.1/updates/facetimehd.ko"
k4="$(rc_key)"; check "a new out-of-tree module in the donor changes the key" "[ '$k4' != '$k3' ]"
echo 'more whence' >> "$FWFROM/WHENCE"
k5="$(rc_key)"; check "a changed linux-firmware tree changes the key" "[ '$k5' != '$k4' ]"
export IYAKKI_SB_KEY="$W/sb.key" IYAKKI_SB_CERT="$W/sb.crt"
k6="$(rc_key)"; check "signing with a key changes the key" "[ '$k6' != '$k5' ]"
printf 'cert2' > "$W/sb.crt"
k7="$(rc_key)"; check "another certificate changes the key" "[ '$k7' != '$k6' ]"
unset IYAKKI_SB_KEY IYAKKI_SB_CERT
# what must NOT change the key: the IYAKKI program, the overlay's ordinary files, the zstd level are not inputs
echo y >> "$OVL/etc/motd"
check "an ordinary overlay change keeps the key" "[ \"\$(rc_key)\" = '$k5' ]"
# a cache that cannot be trusted is switched off
mkdir -p "$OVL/lib/firmware"; echo f > "$OVL/lib/firmware/x.bin"
check "an overlay with its own firmware turns the cache off" "! rc_key >/dev/null"
rm -rf "$OVL/lib"
check "a release image has no cache" "! IMAGE_PROFILE=release rc_key >/dev/null"
check "IYAKKI_REPACK_CACHE=0 turns the cache off" "! IYAKKI_REPACK_CACHE=0 rc_key >/dev/null"
check "IYAKKI_REPACK_CACHE=DIR moves the cache" "[ \"\$(IYAKKI_REPACK_CACHE=$W/c rc_dir)\" = '$W/c' ]"

# trim keeps the two newest files of the variant and leaves other variants alone
export IYAKKI_REPACK_CACHE="$W/cache"; mkdir -p "$W/cache"
for i in 1 2 3 4; do touch -d "2026-01-0$i" "$W/cache/fwmod-pc-k$i.tar.zst"; done
touch "$W/cache/fwmod-macweb-a.tar.zst"
rc_trim
check "trim keeps the newest two files of the variant" "[ -f '$W/cache/fwmod-pc-k4.tar.zst' ] && [ -f '$W/cache/fwmod-pc-k3.tar.zst' ] && [ ! -e '$W/cache/fwmod-pc-k1.tar.zst' ] && [ ! -e '$W/cache/fwmod-pc-k2.tar.zst' ]"
check "trim leaves another variant alone" "[ -f '$W/cache/fwmod-macweb-a.tar.zst' ]"

# the round trip: prune a fake tree, snapshot it the way post-build.sh does, restore into a fresh tree: identical (links, modes, content)
T="$W/tree"; mkdir -p "$T/lib/firmware/intel/iwlwifi" "$T/lib/firmware/other" "$T/lib/modules/6.1/kernel"
echo a > "$T/lib/firmware/intel/iwlwifi/keep.ucode"; ln -s intel/iwlwifi/keep.ucode "$T/lib/firmware/link.ucode"; echo b > "$T/lib/firmware/other/drop.bin"
echo m > "$T/lib/modules/6.1/kernel/m.ko"; chmod 600 "$T/lib/modules/6.1/kernel/m.ko"
printf 'link.ucode\n' > "$W/keep.list"
sh "$REAL/board/iyakki/prune-firmware.sh" "$W/keep.list" "$T/lib/firmware" > "$W/prune.out"
check "the prune dropped the unlisted firmware and kept the link target" "[ ! -e '$T/lib/firmware/other' ] && [ -f '$T/lib/firmware/intel/iwlwifi/keep.ucode' ]"
(cd "$T" && tar --numeric-owner -cf - lib/modules lib/firmware | zstd -3 -T0 -q -f -o "$W/snap.tar.zst")
R="$W/restored"; mkdir -p "$R"; (cd "$R" && zstd -dc "$W/snap.tar.zst" | tar --numeric-owner -xpf -)
lst() { (cd "$1" && find . -printf '%p %y %m %l %s\n' | LC_ALL=C sort); }
check "a restored tree equals the pruned one (names, types, modes, link targets, sizes)" "[ \"\$(lst '$T')\" = \"\$(lst '$R')\" ]"
check "restored file content is identical" "diff -r '$T/lib' '$R/lib' >/dev/null"

# the hooks honour the flag
check "post-build.sh skips the prune when the cache was restored" "grep -q 'IYAKKI_FW_PRUNED' '$REAL/board/iyakki/post-build.sh' && grep -q 'PRUNED\" = 0 \]' '$REAL/board/iyakki/post-build.sh'"
check "repack.sh syntax is valid" "bash -n '$REAL/repack.sh'"
check "repack.sh builds both programs in one cargo call" "grep -q -- '-p iyakki-core -p iyakki-store-server' '$REAL/repack.sh'"

echo "$pass passed, $fail failed"
[ "$fail" = 0 ]
