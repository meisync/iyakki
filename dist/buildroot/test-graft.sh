#!/usr/bin/env bash
# Test of graft.sh (repack.sh --graft) on fake folders: no Buildroot, no image, nothing real is touched. Run: dist/buildroot/test-graft.sh
set -eu
HERE="$(cd "$(dirname "$0")" && pwd)"
W="$(mktemp -d "${TMPDIR:-/tmp}/iyakki-graft-test.XXXXXX")"
trap 'rm -rf "$W"' EXIT
pass=0; fail=0
ok()   { pass=$((pass+1)); echo "ok   - $1"; }
bad()  { fail=$((fail+1)); echo "FAIL - $1"; }
check() { if eval "$2"; then ok "$1"; else bad "$1"; fi; }

SRC="$W/src/target"; DEST="$W/root"
mkdir -p "$SRC/usr/bin" "$SRC/usr/libexec/git-core/mergetools" "$SRC/usr/libexec/iyakki" "$SRC/usr/lib" "$SRC/usr/share/git-core/templates/hooks" "$DEST/usr/lib" "$DEST/etc/ssl/certs"
printf '\177ELFgit' > "$SRC/usr/bin/git"; chmod 755 "$SRC/usr/bin/git"
ln "$SRC/usr/bin/git" "$SRC/usr/libexec/git-core/git"            # hard links, like Buildroot's install
ln "$SRC/usr/bin/git" "$SRC/usr/libexec/git-core/git-add"
ln "$SRC/usr/bin/git" "$SRC/usr/libexec/git-core/git-cvsserver"  # named in the list's exclusions
printf '#!/usr/bin/perl\nprint 1;\n' > "$SRC/usr/libexec/git-core/git-weird"; chmod 755 "$SRC/usr/libexec/git-core/git-weird"
printf '#!/bin/sh\necho hi\n' > "$SRC/usr/libexec/git-core/git-sh-setup"; chmod 755 "$SRC/usr/libexec/git-core/git-sh-setup"
echo m > "$SRC/usr/libexec/git-core/mergetools/vimdiff"
printf '\177ELFssh' > "$SRC/usr/libexec/iyakki/git-ssh"; chmod 755 "$SRC/usr/libexec/iyakki/git-ssh"
echo c > "$SRC/usr/lib/libcurl.so.4.8.0"; ln -s libcurl.so.4.8.0 "$SRC/usr/lib/libcurl.so.4"; ln -s libcurl.so.4 "$SRC/usr/lib/libcurl.so"
echo s > "$SRC/usr/share/git-core/templates/description"; echo h > "$SRC/usr/share/git-core/templates/hooks/pre-push.sample"

# a fake readelf: every program needs libcurl.so.4 and libz.so.1
FAKE="$W/readelf"; printf '#!/bin/sh\necho " 0x1 (NEEDED)  Shared library: [libcurl.so.4]"\necho " 0x1 (NEEDED)  Shared library: [libz.so.1]"\necho " 0x1 (NEEDED)  Shared library: [libc.so]"\n' > "$FAKE"; chmod +x "$FAKE"

# 1 libraries missing in the image: the graft fails and names the one
if READELF="$FAKE" sh "$HERE/graft.sh" git "$SRC" "$DEST" > "$W/out1" 2> "$W/err1"; then bad "a missing shared library fails the graft"; else ok "a missing shared library fails the graft"; fi
check "the failure names the library" "grep -q 'needs libz.so.1, which is not in the image' '$W/err1'"
# 2 with the library in place
touch "$DEST/usr/lib/libz.so.1"
check "the graft passes when every library is there" "READELF='$FAKE' sh '$HERE/graft.sh' git '$SRC' '$DEST' > '$W/out2' 2> '$W/err2'"
check "git is in the image" "[ -x '$DEST/usr/bin/git' ]"
check "hard links stay hard links (one inode)" "[ \"\$(stat -c %i '$DEST/usr/bin/git')\" = \"\$(stat -c %i '$DEST/usr/libexec/git-core/git-add')\" ]"
check "the git-core commands are there" "[ -x '$DEST/usr/libexec/git-core/git-sh-setup' ]"
check "perl scripts are skipped" "[ ! -e '$DEST/usr/libexec/git-core/git-weird' ] && grep -q 'skipped usr/libexec/git-core/git-weird' '$W/err2'"
check "files named as exclusions are not copied" "[ ! -e '$DEST/usr/libexec/git-core/git-cvsserver' ] && [ ! -e '$DEST/usr/libexec/git-core/mergetools' ] && [ ! -e '$DEST/usr/share/git-core/templates/hooks/pre-push.sample' ]"
check "the ssh for git is installed as a libexec helper, not as /usr/bin/ssh" "[ -x '$DEST/usr/libexec/iyakki/git-ssh' ] && [ ! -e '$DEST/usr/bin/ssh' ]"
check "libcurl and its links are copied as links" "[ -L '$DEST/usr/lib/libcurl.so.4' ] && [ \"\$(readlink '$DEST/usr/lib/libcurl.so.4')\" = libcurl.so.4.8.0 ]"
check "the templates are there" "[ -f '$DEST/usr/share/git-core/templates/description' ]"
check "a summary line with the size is printed" "grep -q 'graft: git: [0-9]* files, [0-9]* bytes' '$W/out2'"
# 3 idempotent
check "running it again works" "READELF='$FAKE' sh '$HERE/graft.sh' git '$SRC' '$DEST' >/dev/null 2>&1"
# 4 dry run writes nothing
D2="$W/root2"; mkdir -p "$D2"
check "dry run lists and writes nothing" "sh '$HERE/graft.sh' git '$SRC' '$D2' --dry-run 2>/dev/null | grep -q 'dry run' && [ -z \"\$(ls -A '$D2')\" ]"
# 5 a package that was not built
EMPTY="$W/empty"; mkdir -p "$EMPTY"
if sh "$HERE/graft.sh" git "$EMPTY" "$D2" > /dev/null 2> "$W/err5"; then bad "an unbuilt package fails"; else ok "an unbuilt package fails"; fi
check "the message says to build the package" "grep -q 'was the package built' '$W/err5'"
# 6 unknown list
if sh "$HERE/graft.sh" nosuch "$SRC" "$D2" > /dev/null 2> "$W/err6"; then bad "an unknown graft name fails"; else ok "an unknown graft name fails"; fi
# 7 repack.sh knows the option
check "repack.sh parses cleanly" "bash -n '$HERE/repack.sh'"
check "repack.sh documents --graft" "grep -q -- '--graft NAME' '$HERE/repack.sh'"
if bash "$HERE/repack.sh" web "$W/out" --graft > /dev/null 2> "$W/err7"; then bad "--graft without a name is refused"; else ok "--graft without a name is refused"; fi
check "the real list names git, libcurl and the ssh helper" "grep -q '^usr/bin/git' '$HERE/graft/git.list' && grep -q 'libcurl' '$HERE/graft/git.list' && grep -q 'iyakki/git-ssh' '$HERE/graft/git.list'"

echo "$pass passed, $fail failed"
[ "$fail" = 0 ]
