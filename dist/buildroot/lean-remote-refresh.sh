#!/usr/bin/env bash
# Refresh the BOOT-volume part of the lean experiment (boot/initrd-lean and the lean entries of boot/grub/grub.cfg) on a stick that is
# plugged into ANOTHER machine you can reach with ssh, typically a Mac that is running IYAKKI from that very stick (normal boot with
# remote diagnostics). The tree on the data volume is not touched (use lean-install.sh with the stick in a PC for a new tree).
#
#   lean-remote-refresh.sh --ssh 'root@ADDRESS' [-i KEY] [-p PORT] --lean-out DIR [--menu-from DIR] [--yes]
#
# --menu-from DIR (an image's install-root/boot/grub: grub.cfg already filled in, fonts/, themes/) also puts that menu on the stick: the whole
# grub.cfg (the new first screen, with the lean entries in it), fonts/iyakki-menu.pf2 and themes/iyakki/theme.txt. Without it only the lean block is merged.
#
# Needs on the remote side only what IYAKKI's BusyBox has (dd, mount, cp, mv, sha256sum, sync). It finds the FAT partition by its label
# bytes (IYAKKI-BOOT), refuses anything else, mounts it read-write for the copy and puts the previous state back (read-only or unmounted).
# Without --yes it only shows what it found. The merge of grub.cfg is done HERE (python3), the remote machine only receives the file.
set -eu
HERE="$(cd "$(dirname "$0")" && pwd)"
SSH=""; KEY=""; OUT=""; YES=0; PORT=22; MENU=""
while [ $# -gt 0 ]; do case "$1" in --ssh) SSH="$2"; shift 2 ;; -i) KEY="$2"; shift 2 ;; -p) PORT="$2"; shift 2 ;; --lean-out) OUT="$2"; shift 2 ;; --menu-from) MENU="$2"; shift 2 ;; --yes) YES=1; shift ;; *) sed -n '2,16p' "$0" >&2; exit 2 ;; esac; done
[ -n "$SSH" ] && [ -d "$OUT" ] || { sed -n '2,16p' "$0" >&2; exit 2; }
for f in initrd-lean grub-lean-block.cfg; do [ -s "$OUT/$f" ] || { echo "lean-remote-refresh: $OUT/$f missing" >&2; exit 1; }; done
SO=(-p "$PORT" -o StrictHostKeyChecking=no -o UserKnownHostsFile=/dev/null -o BatchMode=yes -o LogLevel=ERROR); [ -z "$KEY" ] || SO+=(-i "$KEY")
rs() { ssh "${SO[@]}" "$SSH" "$@"; }
T="$(mktemp -d "${TMPDIR:-/tmp}/lean-rr-XXXXXX")"; trap 'rm -rf "$T"' EXIT
# 1. find the boot partition by the label bytes in its boot sector (FAT32: offset 71; FAT12/16: offset 43), on sd* partitions only
DEV="$(rs 'for d in /dev/sd[a-z][0-9]*; do [ -b "$d" ] || continue; for o in 71 43; do l=$(dd if=$d bs=1 skip=$o count=11 2>/dev/null); [ "$l" = "IYAKKI-BOOT" ] && { echo $d; break 2; }; done; done')"
[ -n "$DEV" ] || { echo "lean-remote-refresh: no partition with the label IYAKKI-BOOT on the remote machine" >&2; exit 1; }
echo "remote boot partition: $DEV ($(rs "cat /sys/class/block/$(basename "$DEV")/size" | awk '{printf "%.0f MB", $1/2048}'))"
MP="$(rs "grep \"^$DEV \" /proc/mounts | head -n1 | cut -d' ' -f2" || true)"
echo "mounted at: ${MP:-nowhere}"
[ "$YES" = 1 ] || { echo "lean-remote-refresh: nothing was changed; add --yes to refresh boot/initrd-lean and the lean entries on $DEV"; exit 3; }
# 2. mount read-write (remember how it was)
if [ -n "$MP" ]; then rs "mount -o remount,rw $MP"; WAS=mounted; else MP=/mnt/lean-boot; rs "mkdir -p $MP && mount -t vfat -o nosuid,nodev,noexec $DEV $MP"; WAS=unmounted; fi
restore() { if [ "$WAS" = mounted ]; then rs "sync; mount -o remount,ro $MP" || true; else rs "sync; umount $MP" || true; fi; }
trap 'restore; rm -rf "$T"' EXIT
rs "test -d $MP/boot/grub && test -f $MP/boot/grub/grub.cfg" || { echo "lean-remote-refresh: $MP is not an IYAKKI boot volume" >&2; exit 1; }
# 3. grub.cfg: fetched, merged here, sent back (the first time the old file is kept as grub.cfg.before-lean)
rs "cat $MP/boot/grub/grub.cfg" > "$T/grub.cfg"
if [ -n "$MENU" ]; then
  [ -s "$MENU/grub.cfg" ] && grep -q '^menuentry "IYAKKI" {' "$MENU/grub.cfg" && ! grep -q '__INITRD_PATH__' "$MENU/grub.cfg" || { echo "lean-remote-refresh: $MENU/grub.cfg is missing or not filled in" >&2; exit 1; }
  # what the updater added to the stick's menu (the previous-version entry) is kept: it is copied in front of the normal entry again
  cp "$MENU/grub.cfg" "$T/grub.new"
  python3 -I - "$T/grub.cfg" "$T/grub.new" <<'PY'
import re, sys
old = open(sys.argv[1]).read(); new = open(sys.argv[2]).read()
m = re.search(r'(# previous version:[^\n]*\nmenuentry "[^"]*" \{.*?\n\}\n)', old, re.S)
if m and "# previous version:" not in new:
    i = new.index('menuentry "IYAKKI" {')
    new = new[:i] + m.group(1) + "\n" + new[i:]
open(sys.argv[2], "w").write(new)
PY
  cp "$T/grub.new" "$T/grub.cfg"
else
  python3 -I "$HERE/lean-sync.py" grubmerge "$T/grub.cfg" "$OUT/grub-lean-block.cfg"
fi
# 4. send, check, move into place (initrd first, grub.cfg last; a file is renamed only after its checksum matched)
send() { # local remote
  ssh "${SO[@]}" "$SSH" "cat > $2.tmp" < "$1"
  want="$(sha256sum < "$1" | cut -d' ' -f1)"; got="$(rs "sync; sha256sum < $2.tmp" | cut -d' ' -f1)"
  [ "$want" = "$got" ] || { rs "rm -f $2.tmp"; echo "lean-remote-refresh: checksum mismatch for $2" >&2; exit 1; }
  rs "mv -f $2.tmp $2"
}
send "$OUT/initrd-lean" "$MP/boot/initrd-lean"
if [ -n "$MENU" ]; then
  [ -s "$MENU/fonts/iyakki-menu.pf2" ] && rs "mkdir -p $MP/boot/grub/fonts" && send "$MENU/fonts/iyakki-menu.pf2" "$MP/boot/grub/fonts/iyakki-menu.pf2"
  [ -s "$MENU/fonts/iyakki-term.pf2" ] && rs "mkdir -p $MP/boot/grub/fonts" && send "$MENU/fonts/iyakki-term.pf2" "$MP/boot/grub/fonts/iyakki-term.pf2"
  [ -s "$MENU/themes/iyakki/theme.txt" ] && rs "mkdir -p $MP/boot/grub/themes/iyakki" && send "$MENU/themes/iyakki/theme.txt" "$MP/boot/grub/themes/iyakki/theme.txt"
fi
rs "test -f $MP/boot/grub/grub.cfg.before-lean || cp $MP/boot/grub/grub.cfg $MP/boot/grub/grub.cfg.before-lean"
send "$T/grub.cfg" "$MP/boot/grub/grub.cfg"
rs sync
# 5. read back from the stick itself (the cache is dropped first when the remote kernel allows it)
rs "echo 3 > /proc/sys/vm/drop_caches" 2>/dev/null || true
[ "$(rs "sha256sum < $MP/boot/initrd-lean" | cut -d' ' -f1)" = "$(sha256sum < "$OUT/initrd-lean" | cut -d' ' -f1)" ] || { echo "lean-remote-refresh: read-back of initrd-lean differs" >&2; exit 1; }
[ "$(rs "sha256sum < $MP/boot/grub/grub.cfg" | cut -d' ' -f1)" = "$(sha256sum < "$T/grub.cfg" | cut -d' ' -f1)" ] || { echo "lean-remote-refresh: read-back of grub.cfg differs" >&2; exit 1; }
echo "lean-remote-refresh: done and read back ($DEV). The normal entry is unchanged; the stage-1 log lands in boot/stage1.log on the stick."
