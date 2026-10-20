#!/usr/bin/env bash
# Decision-table test for changed.sh, on a COPY of dist/buildroot with fake variant output directories (no Buildroot, no real
# target/ is touched). Run: dist/buildroot/test-changed.sh
set -eu
HERE="$(cd "$(dirname "$0")" && pwd)"
REAL_ROOT="$(cd "$HERE/../.." && pwd)"
W="$(mktemp -d "${TMPDIR:-/tmp}/iyakki-changed-test.XXXXXX")"
trap 'rm -rf "$W"' EXIT
pass=0; fail=0

mkdir -p "$W/dist" "$W/target/x86_64-unknown-linux-musl/release" "$W/crates/iyakki-shell/assets"
cp -a "$HERE" "$W/dist/buildroot"
printf '[workspace.package]\nversion = "0.0.1"\n' > "$W/Cargo.toml"
echo lic > "$W/LICENSE"; echo n > "$W/NOTICE"; echo l > "$W/LICENSING.md"; echo t > "$W/THIRD_PARTY_LICENSES.txt"
echo bin1 > "$W/target/x86_64-unknown-linux-musl/release/iyakki"
echo sbin1 > "$W/target/x86_64-unknown-linux-musl/release/iyakki-store-server"
# a package list: use Buildroot's real one when it is there, else a tiny fake
if [ -d "$REAL_ROOT/target/buildroot-src/buildroot-2026.02.3/package/mesa3d" ]; then export BR_DIR="$REAL_ROOT/target/buildroot-src/buildroot-2026.02.3"
else export BR_DIR="$W/br"; mkdir -p "$BR_DIR/package/mesa3d" "$BR_DIR/package/jq" "$BR_DIR/package/wpewebkit"; fi
CH="$W/dist/buildroot/changed.sh"
R="$W/dist/buildroot"

mkvariant() { # name: a finished build
  local o="$W/target/buildroot-$1"; rm -rf "$o"; mkdir -p "$o/build" "$o/images"; echo z > "$o/images/rootfs.cpio.zst"
}
first() { bash "$CH" "$@" 2>/dev/null | head -n1; }
expect() { # description, expected-first-line-prefix, variant, extra args...
  local d="$1" want="$2"; shift 2
  local got; got="$(first "$@")"
  case "$got" in "$want"*) pass=$((pass+1)); echo "ok   - $d: $got" ;; *) fail=$((fail+1)); echo "FAIL - $d: wanted '$want' got '$got'" ;; esac
}
reset_tree() { rm -rf "$R/configs" "$R/board" "$R/package"; cp -a "$HERE/configs" "$HERE/board" "$HERE/package" "$R/"; echo bin1 > "$W/target/x86_64-unknown-linux-musl/release/iyakki"; }

# 1-3 no build, no record, adopted
rm -rf "$W/target/buildroot-web"
expect "no build dir" "FULL: no earlier build" web
mkvariant web
expect "images but no record" "FULL: no record" web
bash "$CH" web --adopt >/dev/null
expect "adopted baseline" "NOTHING" web
expect "same inputs twice" "NOTHING" web

# 4 the program changed
echo bin2 > "$W/target/x86_64-unknown-linux-musl/release/iyakki"
expect "new iyakki binary" "IYAKKI-ONLY" web
bash "$CH" web --record >/dev/null; expect "after --record" "NOTHING" web
echo sbin2 > "$W/target/x86_64-unknown-linux-musl/release/iyakki-store-server"
expect "new store server binary" "IYAKKI-ONLY" web
bash "$CH" web --record >/dev/null

# 5 overlay, hook, boot menu, licence texts
echo x >> "$R/board/iyakki/overlay/etc/$(ls "$R/board/iyakki/overlay/etc" | head -n1)" 2>/dev/null || echo x > "$R/board/iyakki/overlay/etc/new"
expect "overlay file edited" "IYAKKI-ONLY" web; bash "$CH" web --record >/dev/null
echo "# x" >> "$R/board/iyakki-web/web-post-build.sh"
expect "web post-build hook edited" "IYAKKI-ONLY" web; bash "$CH" web --record >/dev/null
echo "# x" >> "$R/board/iyakki/grub.cfg"
expect "grub.cfg edited" "IYAKKI-ONLY" web; bash "$CH" web --record >/dev/null
echo more >> "$W/THIRD_PARTY_LICENSES.txt"
expect "licence file edited" "IYAKKI-ONLY" web; bash "$CH" web --record >/dev/null

# 6 things that must NOT trigger anything
mkdir -p "$W/docs"; echo d > "$W/docs/X.md"; echo r > "$W/README.md"; echo c >> "$R/configs/iyakki_web.fragment.bak"
expect "docs and README ignored" "NOTHING" web
cp "$R/configs/iyakki_web.fragment" "$W/frag.keep"; { echo "# just a comment"; echo; cat "$W/frag.keep"; } > "$R/configs/iyakki_web.fragment"
expect "comment/blank lines in a fragment ignored" "NOTHING" web
echo "touch" >> "$R/board/iyakki/post-image.sh.unrelated"; expect "unrelated file in board ignored" "NOTHING" web
rm -f "$R/configs/iyakki_web.fragment.bak" "$R/board/iyakki/post-image.sh.unrelated"

# 7 kernel
mkdir -p "$R/board/iyakki/patches-sb/linux/6.12.112"; echo p > "$R/board/iyakki/patches-sb/linux/6.12.112/9999-x.patch"
expect "kernel patch added" "KERNEL" web; bash "$CH" web --record >/dev/null
echo "CONFIG_X=y" >> "$R/board/iyakki/linux.fragment"
expect "kernel fragment edited" "KERNEL" web; bash "$CH" web --record >/dev/null

# 8 packages
echo "# x" >> "$R/package/iyakki-webview-helper/iyakki-webview-helper.mk"
expect "package dir edited" "PACKAGES iyakki-webview-helper" web; bash "$CH" web --record >/dev/null
echo "BR2_PACKAGE_JQ=y" >> "$R/configs/iyakki_web.fragment"
expect "package switched on" "PACKAGES jq" web; bash "$CH" web --record >/dev/null
grep -v 'BR2_PACKAGE_MESA3D_GALLIUM_DRIVER_RADEONSI' "$R/configs/iyakki_web.fragment" > "$W/f"; cp "$W/f" "$R/configs/iyakki_web.fragment"
expect "sub-option off, package stays on" "PACKAGES mesa3d" web; bash "$CH" web --record >/dev/null
echo 'BR2_TARGET_GRUB2_BUILTIN_MODULES_PC="boot linux"' >> "$R/configs/iyakki_web.fragment"
expect "GRUB module list changed" "PACKAGES grub2" web; bash "$CH" web --record >/dev/null

# 9 things Buildroot cannot do incrementally
grep -v '^BR2_PACKAGE_JQ=y' "$R/configs/iyakki_web.fragment" > "$W/f"; cp "$W/f" "$R/configs/iyakki_web.fragment"
expect "package switched off" "FULL: package(s) switched off" web
cp "$R/configs/iyakki_web.fragment" "$W/f"; bash "$CH" web --record >/dev/null
sed -i 's/^BR2_TOOLCHAIN_BUILDROOT_CXX=y/# BR2_TOOLCHAIN_BUILDROOT_CXX is not set/' "$R/configs/iyakki_web.fragment"
expect "C++ toolchain option changed" "FULL: toolchain" web
reset_tree; bash "$CH" web --adopt >/dev/null
WEB_H264=1 expect "H.264 switched on (new fragment)" "PACKAGES" web
echo "# x" >> "$R/external.mk"
expect "external.mk edited" "FULL: external.mk" web
echo "# x" >> "$R/external.mk" # keep dirty
rm -f "$W/target/buildroot-web/images/rootfs.cpio.zst"
expect "image deleted" "FULL: no finished image" web

# 10 mac: out-of-tree module patches belong to the kernel
reset_tree; mkvariant mac; bash "$CH" mac --adopt >/dev/null
expect "mac baseline" "NOTHING" mac
echo "x" >> "$R/package/apple-bce/0001-apple-bce-work-with-one-MSI-vector.patch"
expect "apple-bce patch edited" "KERNEL" mac; bash "$CH" mac --record >/dev/null
echo "x" >> "$R/board/iyakki/linux-mac.fragment"
expect "linux-mac.fragment edited" "KERNEL" mac; bash "$CH" mac --record >/dev/null
expect "libre variant sees no Mac inputs" "FULL: no earlier build" libre

# 11 the web and mac records are separate; mac fragment does not move web
mkvariant web; bash "$CH" web --adopt >/dev/null
echo "x" >> "$R/board/iyakki/linux-mac.fragment"
expect "web ignores linux-mac.fragment" "NOTHING" web
expect "mac sees linux-mac.fragment" "KERNEL" mac

# 12 --record refuses a build without an image
rm -f "$W/target/buildroot-web/images/rootfs.cpio.zst"
if bash "$CH" web --record >/dev/null 2>&1; then fail=$((fail+1)); echo "FAIL - record without image should be refused"; else pass=$((pass+1)); echo "ok   - record without image refused"; fi

# 13 usage
if bash "$CH" nosuch >/dev/null 2>&1; then fail=$((fail+1)); echo "FAIL - unknown variant accepted"; else pass=$((pass+1)); echo "ok   - unknown variant refused"; fi

echo "changed.sh decision table: $pass passed, $fail failed"
[ "$fail" = 0 ]
