#!/usr/bin/env bash
# Tests of lean-install.sh and lean-sync.py with made-up mount points, labels and drives (LEAN_TEST_MAP): nothing real is touched.
#   dist/buildroot/lean-install-test.sh        (needs bash, python3, sha256sum; runs in a few seconds; exits non-zero on the first failure)
set -u
HERE="$(cd "$(dirname "$0")" && pwd)"; BIN="$HERE/lean-install.sh"
W="$(mktemp -d "${TMPDIR:-/tmp}/lean-it-XXXXXX")"; trap 'chmod -R u+w "$W" 2>/dev/null; rm -rf "$W"' EXIT
fail() { echo "FAIL: $*" >&2; exit 1; }
ok() { echo "ok: $*"; }

# ---- a made-up lean-out, boot volume, data volume --------------------------------------------------------------------------------------
mkout() { # dir id marker
  rm -rf "$1"; mkdir -p "$1/tree/bin" "$1/tree/etc" "$1/tree/usr/lib"
  head -c 5000 /dev/urandom > "$1/tree/usr/lib/big.so"; printf 'same everywhere\n' > "$1/tree/etc/stable"
  printf 'version %s\n' "$3" > "$1/tree/etc/release"; printf '#!/bin/sh\necho hi\n' > "$1/tree/bin/tool"; chmod 755 "$1/tree/bin/tool"
  ln -s tool "$1/tree/bin/alias"; ln "$1/tree/bin/tool" "$1/tree/bin/tool2"; ln "$1/tree/bin/tool" "$1/tree/bin/tool3"
  head -c 100000 /dev/urandom > "$1/initrd-lean"; echo "$2" > "$1/root-id"
  python3 -I "$HERE/lean-sync.py" manifest "$1/tree" > "$1/tree.manifest"
  sed -n '/^# BEGIN lean-boot/,/^# END lean-boot/p' "$HERE/board/iyakki/grub.cfg" | sed -e 's%__KERNEL_PATH__%/boot/bzImage%' -e 's%__INITRD_PATH__%/boot/initrd%' > "$1/grub-lean-block.cfg"
}
OUT="$W/out1"; mkout "$OUT" "id1" one
BOOT="$W/boot"; DATA="$W/data"; mkdir -p "$BOOT/boot/grub" "$DATA/home" "$DATA/vault"
# the stick's old menu: the board's menu without the lean block, paths filled in
sed -e '/^# BEGIN lean-boot/,/^# END lean-boot/d' -e 's%__KERNEL_PATH__%/boot/bzImage%' -e 's%__INITRD_PATH__%/boot/initrd%' "$HERE/board/iyakki/grub.cfg" > "$BOOT/boot/grub/grub.cfg"
echo "user data" > "$DATA/home/file"
cp "$BOOT/boot/grub/grub.cfg" "$W/grub.orig"
BOOTP="$(cd "$BOOT" && pwd -P)"; DATAP="$(cd "$DATA" && pwd -P)"
MAP="$W/map"; export LEAN_TEST_MAP="$MAP"
setmap() { printf '%s\n' "$BOOTP /dev/sdx1 ${1:-IYAKKI-BOOT} ${3:-/dev/sdx} 64G SER123" "$DATAP /dev/sdx2 ${2:-IYAKKI-DATA} ${4:-/dev/sdx} 64G SER123" > "$MAP"; }
snapshot() { (cd "$W" && find boot data -type f -o -type l | sort | xargs -r sha256sum 2>/dev/null | sha256sum); }
run() { "$BIN" "$@" 2>&1; }
setmap
unset LEAN_TEST_ROOT_DEV

# ---- refusals ----------------------------------------------------------------------------------------------------------------------------
S0="$(snapshot)"
printf '' > "$MAP"; o="$(run --boot "$BOOT" --data "$DATA" --lean-out "$OUT" --yes)" && fail "not a mount point accepted"; echo "$o" | grep -q "not a mount point" || fail "wrong message: $o"; ok "refuses folders that are not mount points"
setmap WRONG; o="$(run --boot "$BOOT" --data "$DATA" --lean-out "$OUT" --yes)" && fail "wrong boot label accepted"; echo "$o" | grep -q "not labelled IYAKKI-BOOT" || fail "$o"; ok "refuses a wrong boot label"
setmap IYAKKI-BOOT OTHER; o="$(run --boot "$BOOT" --data "$DATA" --lean-out "$OUT" --yes)" && fail "wrong data label accepted"; echo "$o" | grep -q "not labelled IYAKKI-DATA" || fail "$o"; ok "refuses a wrong data label"
setmap IYAKKI-BOOT IYAKKI-DATA /dev/nvme0n1 /dev/nvme0n1; sed -i 's#/dev/sdx1#/dev/nvme0n1p1#;s#/dev/sdx2#/dev/nvme0n1p2#' "$MAP"; o="$(run --boot "$BOOT" --data "$DATA" --lean-out "$OUT" --yes)" && fail "nvme accepted"; echo "$o" | grep -qi "nvme" || fail "$o"; ok "refuses an NVMe disk"
setmap IYAKKI-BOOT IYAKKI-DATA /dev/sdx /dev/sdy; o="$(run --boot "$BOOT" --data "$DATA" --lean-out "$OUT" --yes)" && fail "two drives accepted"; echo "$o" | grep -q "different drives" || fail "$o"; ok "refuses boot and data on different drives"
setmap; LEAN_TEST_ROOT_DEV=/dev/sdx2 o="$(LEAN_TEST_ROOT_DEV=/dev/sdx2 run --boot "$BOOT" --data "$DATA" --lean-out "$OUT" --yes)" && fail "the running system's root accepted"; echo "$o" | grep -q "running system's root" || fail "$o"; ok "refuses the drive the running system's root is on"
o="$(LEAN_TEST_ROOT_DEV=/dev/sdx run --boot "$BOOT" --data "$DATA" --lean-out "$OUT" --yes)" && fail "root's drive accepted"; ok "refuses a partition of the root's drive"
setmap; o="$(run --boot "$BOOT" --data "$DATA" --lean-out "$W/missing" --yes)" && fail "missing lean-out accepted"; ok "refuses a missing lean-out"
[ "$(snapshot)" = "$S0" ] || fail "a refused run changed something"; ok "nothing was written by any refused run"

# ---- without --yes: shows the drive, writes nothing ----------------------------------------------------------------------------
o="$(run --boot "$BOOT" --data "$DATA" --lean-out "$OUT")"; rc=$?
echo "$o" | grep -q "SER123" && echo "$o" | grep -q "/dev/sdx" && echo "$o" | grep -q "64G" || fail "no device/size/serial printed: $o"
echo "$o" | grep -q -- "--yes" || fail "does not ask for --yes"
[ "$(snapshot)" = "$S0" ] || fail "run without --yes wrote something"; ok "prints device, size, serial and writes nothing without --yes"

# ---- install ---------------------------------------------------------------------------------------------------------------------------
o="$(run --boot "$BOOT" --data "$DATA" --lean-out "$OUT" --yes)" || fail "install failed: $o"
[ -f "$BOOT/boot/initrd-lean" ] && cmp -s "$BOOT/boot/initrd-lean" "$OUT/initrd-lean" || fail "initrd-lean not copied"
[ -f "$BOOT/boot/grub/grub.cfg.before-lean" ] && cmp -s "$BOOT/boot/grub/grub.cfg.before-lean" "$W/grub.orig" || fail "old grub.cfg not kept"
grep -q 'menuentry "IYAKKI - fast start (lean boot)"' "$BOOT/boot/grub/grub.cfg" && grep -q "fast start + remote diagnostics" "$BOOT/boot/grub/grub.cfg" || fail "entries not in grub.cfg"
# every old line is still there, in the same order (only the block was added)
diff <(cat "$W/grub.orig") <(sed '/^# BEGIN lean-boot/,/^# END lean-boot/d' "$BOOT/boot/grub/grub.cfg" | cat -s) >/dev/null || diff "$W/grub.orig" <(sed '/^# BEGIN lean-boot/,/^# END lean-boot/d' "$BOOT/boot/grub/grub.cfg") | head -5
[ "$(grep -c '^menuentry "IYAKKI"' "$BOOT/boot/grub/grub.cfg")" = 1 ] || fail "normal entry changed"
first_entry="$(grep -m1 -n 'menuentry' "$BOOT/boot/grub/grub.cfg" | cut -d: -f1)"; normal_line="$(grep -n '^menuentry "IYAKKI" {' "$BOOT/boot/grub/grub.cfg" | head -1 | cut -d: -f1)"
[ "$first_entry" -lt "$normal_line" ] && grep -m1 'menuentry' "$BOOT/boot/grub/grub.cfg" | grep -q 'fast start (lean boot)' || fail "the fast start is not the first entry"
grep -q 'set default="0"' "$BOOT/boot/grub/grub.cfg" || fail "default changed"
R="$DATA/iyakki/root"; [ "$(cat "$R/current")" = id1 ] && [ -f "$R/id1/.lean-complete" ] && [ -x "$R/id1/tree/bin/tool" ] && [ "$(readlink "$R/id1/tree/bin/alias")" = tool ] || fail "tree not in place"
[ ! -e "$R/previous" ] || fail "previous pointer on a first install"
[ "$(cat "$DATA/home/file")" = "user data" ] || fail "user data touched"
ls -A "$R" | grep -q '\.tmp' && fail "temporary folder left"
[ "$(stat -c %i "$R/id1/tree/bin/tool")" = "$(stat -c %i "$R/id1/tree/bin/tool3")" ] && [ "$(stat -c %i "$R/id1/tree/bin/tool")" = "$(stat -c %i "$R/id1/tree/bin/tool2")" ] || fail "hard links were copied apart"
ok "install: initrd, menu entries (before the normal one, so the first and the default), tree, pointer, old menu kept, user data untouched"

# ---- idempotent re-run ------------------------------------------------------------------------------------------------------------------
S1="$(snapshot)"; o="$(run --boot "$BOOT" --data "$DATA" --lean-out "$OUT" --yes)" || fail "re-run failed: $o"
echo "$o" | grep -q "already" || fail "no 'already' in re-run: $o"; [ "$(snapshot)" = "$S1" ] || fail "re-run changed files"; ok "re-run with the same tree changes nothing"

# ---- newer tree: only changed files are copied, previous is kept -------------------------------------------------------------------
OUT2="$W/out2"; mkout "$OUT2" "id2" two; cp "$OUT/tree/usr/lib/big.so" "$OUT2/tree/usr/lib/big.so"; python3 -I "$HERE/lean-sync.py" manifest "$OUT2/tree" > "$OUT2/tree.manifest"
cp "$OUT/initrd-lean" "$OUT2/initrd-lean"
o="$(run --boot "$BOOT" --data "$DATA" --lean-out "$OUT2" --yes)" || fail "update failed: $o"
echo "$o" | grep -q "files written" || fail "$o"
[ "$(cat "$R/current")" = id2 ] && [ "$(cat "$R/previous")" = id1 ] || fail "pointers: $(cat "$R/current") $(cat "$R/previous" 2>/dev/null)"
grep -q "version one" "$R/id1/tree/etc/release" && grep -q "version two" "$R/id2/tree/etc/release" || fail "old tree must stay, new one must differ"
[ "$(stat -c %i "$R/id1/tree/usr/lib/big.so")" = "$(stat -c %i "$R/id2/tree/usr/lib/big.so")" ] || fail "unchanged file was copied, not linked"
[ "$(stat -c %i "$R/id1/tree/etc/release")" != "$(stat -c %i "$R/id2/tree/etc/release")" ] || fail "changed file shares the old inode (the old tree would change)"
ok "newer tree: only changed files written, unchanged ones hard-linked, previous tree kept intact, pointers switched last"

# ---- a damaged copy is noticed ----------------------------------------------------------------------------------------------------------
echo corrupt >> "$R/id2/tree/etc/stable"
python3 -I "$HERE/lean-sync.py" verify "$R/id2/tree" "$OUT2/tree.manifest" --no-chown >/dev/null && fail "verify missed a changed file"; ok "the fresh-read check finds a changed file"
o="$(run --boot "$BOOT" --data "$DATA" --lean-out "$OUT2" --yes)" && fail "re-run accepted a damaged tree"; ok "a re-run does not accept a damaged tree"
rm -rf "$R/id2" ; printf 'id1\n' > "$R/current"; rm -f "$R/previous"

# ---- an interrupted copy: a leftover temporary folder is reused, never used as a root ---------------------------------------------
mkdir -p "$R/.id2.tmp/tree/etc"; echo junk > "$R/.id2.tmp/tree/etc/junk"
o="$(run --boot "$BOOT" --data "$DATA" --lean-out "$OUT2" --yes)" || fail "after an interrupted copy: $o"
[ ! -e "$R/id2/tree/etc/junk" ] && [ ! -e "$R/.id2.tmp" ] && [ "$(cat "$R/current")" = id2 ] || fail "interrupted copy not cleaned"; ok "an interrupted earlier copy is completed cleanly"

# ---- third tree: the oldest made by this tool goes ---------------------------------------------------------------------------------
OUT3="$W/out3"; mkout "$OUT3" "id3" three; cp "$OUT/initrd-lean" "$OUT3/initrd-lean"
o="$(run --boot "$BOOT" --data "$DATA" --lean-out "$OUT3" --yes)" || fail "$o"
[ -d "$R/id3" ] && [ -d "$R/id2" ] && [ ! -d "$R/id1" ] || fail "pruning: $(ls "$R")"; [ "$(cat "$R/previous")" = id2 ] || fail "previous"
mkdir "$R/foreign"; touch "$R/foreign/keep"; python3 -I "$HERE/lean-sync.py" prune "$R" >/dev/null; [ -f "$R/foreign/keep" ] || fail "pruned a folder it did not make"
ok "older trees made by the tool are removed, a foreign folder never"

# ---- remove --------------------------------------------------------------------------------------------------------------------------------
o="$(run --boot "$BOOT" --data "$DATA" --remove)"; [ -d "$R/id3" ] || fail "--remove without --yes removed"
o="$(run --boot "$BOOT" --data "$DATA" --remove --yes)" || fail "$o"
[ ! -e "$BOOT/boot/initrd-lean" ] && [ ! -d "$DATA/iyakki" ] && ! grep -q 'menuentry "IYAKKI - lean' "$BOOT/boot/grub/grub.cfg" || fail "remove incomplete"
diff -q "$W/grub.orig" "$BOOT/boot/grub/grub.cfg" >/dev/null || { diff "$W/grub.orig" "$BOOT/boot/grub/grub.cfg" | head; fail "grub.cfg is not as before the install"; }
[ "$(cat "$DATA/home/file")" = "user data" ] || fail "user data touched by remove"
ok "--remove takes the lean files and entries off; grub.cfg is byte-identical to the old one; user data untouched"
echo "all lean-install tests passed"
