#!/usr/bin/env bash
# Put the EXPERIMENTAL lean boot (docs/BOOT-SPEED.md) on an installed IYAKKI stick, next to the normal boot, which stays as it is.
#
#   sudo lean-install.sh --boot MOUNT_OF_IYAKKI-BOOT --data MOUNT_OF_IYAKKI-DATA --lean-out DIR [--yes]
#   sudo lean-install.sh --boot ... --data ... --remove [--yes]       take the lean files off again
#
#   --lean-out  the OUT of `lean-image.sh IMAGES OUT --root export` (initrd-lean, grub-lean-block.cfg, tree/, tree.manifest, root-id)
#
# Without --yes nothing is written: the script only checks and prints the drive it would touch. It refuses unless
#   * both folders are mount points whose devices are labelled IYAKKI-BOOT and IYAKKI-DATA, on the SAME drive,
#   * that drive is not an NVMe disk and not the drive the running system's root is on.
# What it writes (and nothing else, on that drive):
#   BOOT: boot/initrd-lean (new file), and boot/grub/grub.cfg gets the lean entries inserted right before the normal entry (the old file is
#         kept once as boot/grub/grub.cfg.before-lean; every existing entry stays exactly as it was)
#   DATA: iyakki/root/<id>/tree (written as a temporary folder, verified, renamed), then the pointers iyakki/root/previous and
#         iyakki/root/current (switched last). The previous tree is kept; older trees made by this script are removed.
# Safe to run again (same or newer tree): only files that differ are copied. At the end everything it wrote is read back and compared.
#
# Test hooks (never needed on a real run): LEAN_TEST_MAP=FILE (lines: MOUNT DEVICE LABEL DISK SIZE SERIAL), LEAN_TEST_ROOT_DEV=DEVICE.
set -eu
HERE="$(cd "$(dirname "$0")" && pwd)"
die() { echo "lean-install: $*" >&2; exit 1; }
BOOT=""; DATA=""; OUT=""; YES=0; REMOVE=0
while [ $# -gt 0 ]; do
  case "$1" in
    --boot) BOOT="${2:-}"; shift 2 ;; --data) DATA="${2:-}"; shift 2 ;; --lean-out) OUT="${2:-}"; shift 2 ;;
    --yes) YES=1; shift ;; --remove) REMOVE=1; shift ;;
    *) sed -n '2,22p' "$0" >&2; exit 2 ;;
  esac
done
[ -n "$BOOT" ] && [ -n "$DATA" ] || { sed -n '2,22p' "$0" >&2; exit 2; }
[ "$REMOVE" = 1 ] || [ -n "$OUT" ] || die "--lean-out is needed"
command -v python3 >/dev/null || die "python3 is needed"
TEST="${LEAN_TEST_MAP:-}"
[ -n "$TEST" ] || [ "$(id -u)" = 0 ] || die "run it with sudo (the files of the system keep their owner root)"
CHOWN=""; [ -n "$TEST" ] && CHOWN="--no-chown"

# ---- which device is behind a mount point -----------------------------------------------------------------------------------
tfield() { awk -v m="$1" -v c="$2" '$1==m { print $c; exit }' "$TEST"; }
dev_of() {
  if [ -n "$TEST" ]; then tfield "$1" 2; else mountpoint -q -- "$1" || return 0; findmnt -n -o SOURCE --target "$1" 2>/dev/null | head -n1; fi
}
label_of() {
  if [ -n "$TEST" ]; then tfield "$1" 3; else lsblk -ndo LABEL "$2" 2>/dev/null | head -n1 || blkid -o value -s LABEL "$2"; fi
}
disk_of() {  # the whole drive of a partition
  if [ -n "$TEST" ]; then tfield "$1" 4; else p="$(lsblk -ndo PKNAME "$2" 2>/dev/null | head -n1)"; if [ -n "$p" ]; then echo "/dev/$p"; else echo "$2"; fi; fi
}
size_of() { if [ -n "$TEST" ]; then tfield "$1" 5; else lsblk -ndo SIZE "$2" 2>/dev/null | head -n1; fi; }
serial_of() { if [ -n "$TEST" ]; then tfield "$1" 6; else lsblk -ndo SERIAL "$2" 2>/dev/null | head -n1; fi; }
model_of() { if [ -n "$TEST" ]; then echo "(test)"; else lsblk -ndo MODEL "$1" 2>/dev/null | head -n1; fi; }

BOOT="$(cd "$BOOT" 2>/dev/null && pwd -P)" || die "no such folder: boot"
DATA="$(cd "$DATA" 2>/dev/null && pwd -P)" || die "no such folder: data"
[ "$BOOT" != "$DATA" ] || die "--boot and --data are the same folder"
BDEV="$(dev_of "$BOOT")"; DDEV="$(dev_of "$DATA")"
[ -n "$BDEV" ] || die "$BOOT is not a mount point"
[ -n "$DDEV" ] || die "$DATA is not a mount point"
[ "$(label_of "$BOOT" "$BDEV")" = "IYAKKI-BOOT" ] || die "the device of $BOOT ($BDEV) is not labelled IYAKKI-BOOT"
[ "$(label_of "$DATA" "$DDEV")" = "IYAKKI-DATA" ] || die "the device of $DATA ($DDEV) is not labelled IYAKKI-DATA"
BDISK="$(disk_of "$BOOT" "$BDEV")"; DDISK="$(disk_of "$DATA" "$DDEV")"
[ "$BDISK" = "$DDISK" ] || die "the boot volume ($BDEV on $BDISK) and the data volume ($DDEV on $DDISK) are on different drives"
case "$BDEV $BDISK $DDEV $DDISK" in */nvme*) die "refusing an NVMe disk ($BDISK): this experiment is for the USB stick only" ;; esac
if [ -n "$TEST" ]; then ROOTDEV="${LEAN_TEST_ROOT_DEV:-}"; else ROOTDEV="$(findmnt -n -o SOURCE / 2>/dev/null | head -n1 || true)"; fi
if [ -n "$ROOTDEV" ] && [ -b "$ROOTDEV" -o -n "$TEST" ]; then
  if [ -n "$TEST" ]; then RDISK="$ROOTDEV"; else RDISK="$(disk_of / "$ROOTDEV")"; fi
  for d in "$BDEV" "$DDEV" "$BDISK"; do
    [ "$d" != "$ROOTDEV" ] && [ "$d" != "$RDISK" ] || die "refusing $d: it is the drive the running system's root is on"
  done
fi
echo "lean-install: the drive this will touch"
echo "  drive   $BDISK  $(model_of "$BDISK")  size $(size_of "$BOOT" "$BDISK")  serial $(serial_of "$BOOT" "$BDISK")"
echo "  boot    $BDEV  label IYAKKI-BOOT  mounted at $BOOT  (free $(df -Pk "$BOOT" | awk 'NR==2 { printf "%.0f MB", $4/1024 }'))"
echo "  data    $DDEV  label IYAKKI-DATA  mounted at $DATA  (free $(df -Pk "$DATA" | awk 'NR==2 { printf "%.0f MB", $4/1024 }'))"

GRUB="$BOOT/boot/grub/grub.cfg"; ROOTS="$DATA/iyakki/root"
[ -f "$GRUB" ] || die "$GRUB not found: not an IYAKKI boot volume"

# ---- remove -----------------------------------------------------------------------------------------------------------------
if [ "$REMOVE" = 1 ]; then
  echo "lean-install: would remove boot/initrd-lean, the lean entries of grub.cfg and $ROOTS"
  [ "$YES" = 1 ] || { echo "lean-install: nothing was changed; add --yes to do it"; exit 3; }
  python3 -I "$HERE/lean-sync.py" grubstrip "$GRUB"
  rm -f "$BOOT/boot/initrd-lean" "$BOOT/boot/initrd-lean.tmp" "$GRUB.before-lean"
  rm -rf "$ROOTS"; rmdir "$DATA/iyakki" 2>/dev/null || true
  sync; echo "lean-install: removed. The normal entry was never changed."; exit 0
fi

# ---- inputs -----------------------------------------------------------------------------------------------------------------
OUT="$(cd "$OUT" 2>/dev/null && pwd -P)" || die "no such folder: lean-out"
for f in initrd-lean grub-lean-block.cfg tree.manifest root-id; do [ -s "$OUT/$f" ] || die "$OUT/$f is missing (build with: lean-image.sh IMAGES $OUT --root export)"; done
[ -d "$OUT/tree" ] || die "$OUT/tree is missing"
ID="$(tr -d ' \n' < "$OUT/root-id")"
grep -q 'function iyakki_loading' "$GRUB" || die "the stick's grub.cfg has no iyakki_loading function (an older image): update it first"
grep -q '^menuentry "IYAKKI" {' "$GRUB" || die "the stick's grub.cfg has no normal entry named IYAKKI"
need_kb=$(( $(stat -c %s "$OUT/initrd-lean") / 1024 + 1024 ))
have_kb="$(df -Pk "$BOOT" | awk 'NR==2 { print $4 }')"
[ "$have_kb" -ge "$need_kb" ] || die "the boot volume has $have_kb KB free, $need_kb KB needed"
TREE_KB="$(du -sk "$OUT/tree" | cut -f1)"; HAVE_DATA="$(df -Pk "$DATA" | awk 'NR==2 { print $4 }')"
[ "$HAVE_DATA" -ge $(( TREE_KB * 12 / 10 )) ] || [ -d "$ROOTS/.$ID.tmp" ] || [ -e "$ROOTS/current" ] || die "the data volume has $HAVE_DATA KB free; a new tree needs up to $(( TREE_KB * 12 / 10 )) KB"
echo "lean-install: will install root id $ID ($(( TREE_KB / 1024 )) MB tree, lean initrd $(( $(stat -c %s "$OUT/initrd-lean") / 1024 )) KB)"
[ "$YES" = 1 ] || { echo "lean-install: nothing was written. Check the drive above (serial, size), then run again with --yes"; exit 3; }

# ---- boot volume: initrd first (the entries appear only when it exists), grub.cfg last ----------------------------------------
want="$(sha256sum < "$OUT/initrd-lean" | cut -d' ' -f1)"
have=""; [ -f "$BOOT/boot/initrd-lean" ] && have="$(sha256sum < "$BOOT/boot/initrd-lean" | cut -d' ' -f1)"
if [ "$want" != "$have" ]; then
  cp -- "$OUT/initrd-lean" "$BOOT/boot/initrd-lean.tmp"; sync
  [ "$(sha256sum < "$BOOT/boot/initrd-lean.tmp" | cut -d' ' -f1)" = "$want" ] || { rm -f "$BOOT/boot/initrd-lean.tmp"; die "the copy of initrd-lean does not match"; }
  mv -f -- "$BOOT/boot/initrd-lean.tmp" "$BOOT/boot/initrd-lean"
  echo "lean-install: boot/initrd-lean written"
else
  echo "lean-install: boot/initrd-lean is already there"
fi
[ -f "$GRUB.before-lean" ] || cp -- "$GRUB" "$GRUB.before-lean"
python3 -I "$HERE/lean-sync.py" grubmerge "$GRUB" "$OUT/grub-lean-block.cfg"
sync

# ---- data volume: the tree ----------------------------------------------------------------------------------------------------
mkdir -p -- "$ROOTS"
python3 -I "$HERE/lean-sync.py" sync "$OUT/tree" "$OUT/tree.manifest" "$ROOTS" "$ID" $CHOWN
sync

# ---- final read-back of everything written (fresh reads) ----------------------------------------------------------------------
[ "$(sha256sum < "$BOOT/boot/initrd-lean" | cut -d' ' -f1)" = "$want" ] || die "read-back: boot/initrd-lean differs"
python3 -I - "$GRUB" "$OUT/grub-lean-block.cfg" <<'PY'
import sys
t = open(sys.argv[1]).read(); b = open(sys.argv[2]).read().rstrip("\n")
sys.exit(0 if b in t else "lean-install: read-back: grub.cfg lacks the lean entries")
PY
[ "$(tr -d ' \n' < "$ROOTS/current")" = "$ID" ] || die "read-back: iyakki/root/current is not $ID"
python3 -I "$HERE/lean-sync.py" verify "$ROOTS/$ID/tree" "$OUT/tree.manifest" $CHOWN || die "read-back: the tree differs"
echo "lean-install: done and verified. Restart, press and hold Option (Mac) to pick the stick, and the first entry, 'IYAKKI - fast start (lean boot)', is the default."
echo "lean-install: the normal entry 'IYAKKI' is unchanged (the full boot, the fallback if a fast start fails)."
