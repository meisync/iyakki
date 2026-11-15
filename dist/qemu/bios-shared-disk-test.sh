#!/bin/sh
# Legacy BIOS boot of a shared disk (F-INST-13): apply --bios to a disk image file, boot it under QEMU's BIOS (SeaBIOS), undo it.
#   env IYAKKI_USB=target/debug/iyakki-usb dist/qemu/bios-shared-disk-test.sh [build-output-dir]
# Needs: the built image and its install stick (images/iyakki-install-x86_64.img), target/debug/iyakki-usb (cargo build -p iyakki-usb),
# qemu-system-x86_64 (KVM recommended), sfdisk (util-linux), sha256sum. No real drive is touched.
# Steps: a 4 GiB MBR disk file with one "other system" partition (1 GiB from 1 MiB, with foreign boot code in sector 0);
#   1. control: that disk alone does not start IYAKKI under BIOS;
#   2. iyakki-usb --plan ... --mode free-space --bios --bios-save S --bios-replace-boot-code writes IYAKKI beside it; the other partition's first MiB is
#      byte for byte the same, the save file exists, sector 0 now holds GRUB's boot code;
#   3. the disk, booted under BIOS, starts IYAKKI (supervisor up, the data volume mounted);
#   4. iyakki-usb --plan --bios-restore S --target-file DISK puts the old boot code back (sector 0 bytes 0-439 as before, the gap empty again).
# PASS = every "ok:" line and the last line PASS.
set -eu
. "$(dirname "$0")/lib.sh"
check_images; need sfdisk; need sha256sum
IYAKKI_USB="${IYAKKI_USB:-$ROOT/target/debug/iyakki-usb}"
STICKIMG="$OUT/images/iyakki-install-x86_64.img"
[ -x "$IYAKKI_USB" ] && [ -f "$STICKIMG" ] || { echo "needs iyakki-usb ($IYAKKI_USB) and $STICKIMG" >&2; exit 2; }
DISK="$WORK/shared.img"; SAVE="$WORK/bios-save.bin"
truncate -s 4G "$DISK"
printf 'label: dos\nstart=2048, size=1G, type=83, bootable\n' | sfdisk -q "$DISK" >/dev/null || fail "sfdisk could not make the disk"
head -c 440 /dev/zero | tr '\0' '\220' | dd of="$DISK" conv=notrunc 2>/dev/null
dd if=/dev/urandom of="$DISK" bs=1M seek=1 count=1 conv=notrunc 2>/dev/null
OLD_CODE=$(dd if="$DISK" bs=1 count=440 2>/dev/null | sha256sum | cut -d' ' -f1)
OLD_PART=$(dd if="$DISK" bs=1M skip=1 count=1 2>/dev/null | sha256sum | cut -d' ' -f1)
bios_boot() { # LOG SECONDS: boot the disk under the BIOS, copy-on-write so the disk is not changed
  : > "$1"
  $QEMU -accel kvm -accel tcg -m 2048 -smp 2 -display none -serial file:"$1" -no-reboot \
    -drive file="$DISK",format=raw,if=virtio,snapshot=on >"$1.err" 2>&1 &
  pid=$!
  n=0; while [ $n -lt "$2" ]; do tr -d '\r' < "$1" 2>/dev/null | grep -aq "supervisor up" && break; sleep 1; n=$((n + 1)); done
  kill $pid 2>/dev/null || true; wait $pid 2>/dev/null || true
}
echo "=== 1. control: the disk alone does not start IYAKKI"
bios_boot "$WORK/before.log" 25
unwanted "$WORK/before.log" "supervisor up" "the other system's disk does not start IYAKKI"
echo "=== 2. write IYAKKI beside it with --bios"
"$IYAKKI_USB" --plan --disk testfile --mode free-space --bios --bios-save "$SAVE" --bios-replace-boot-code --yes --confirm testfile --target-file "$DISK" "$STICKIMG" >"$WORK/apply.log" 2>&1 || { tail -20 "$WORK/apply.log"; fail "the install with --bios failed"; }
want "$WORK/apply.log" "legacy BIOS boot set up" "the install reports legacy BIOS boot"
[ -s "$SAVE" ] && echo "ok: the save file exists" || fail "no save file"
[ "$(dd if="$DISK" bs=1M skip=1 count=1 2>/dev/null | sha256sum | cut -d' ' -f1)" = "$OLD_PART" ] && echo "ok: the other partition's first MiB is unchanged" || fail "the other system's partition changed"
NEW_CODE=$(dd if="$DISK" bs=1 count=440 2>/dev/null | sha256sum | cut -d' ' -f1)
[ "$NEW_CODE" != "$OLD_CODE" ] && echo "ok: sector 0 holds new boot code" || fail "sector 0 boot code did not change"
[ "$(sfdisk -d "$DISK" | grep -c '^/')" -ge 3 ] && echo "ok: the disk has the other partition plus IYAKKI's two" || fail "unexpected partition table"
echo "=== 3. boot under legacy BIOS"
bios_boot "$WORK/after.log" 150
tr -d '\r' < "$WORK/after.log" | grep -a "GRUB\|supervisor up\|persistent volume" | head -5 | sed 's/^/  /' || true
want "$WORK/after.log" "supervisor up" "IYAKKI started from the shared disk through GRUB under BIOS"
want "$WORK/after.log" "persistent volume .* mounted" "the data volume was mounted"
echo "=== 4. undo with the save file"
"$IYAKKI_USB" --plan --bios-restore "$SAVE" --target-file "$DISK" >"$WORK/restore.log" 2>&1 || { cat "$WORK/restore.log"; fail "the restore failed"; }
[ "$(dd if="$DISK" bs=1 count=440 2>/dev/null | sha256sum | cut -d' ' -f1)" = "$OLD_CODE" ] && echo "ok: the old boot code is back" || fail "the old boot code did not come back"
[ "$(dd if="$DISK" bs=512 skip=1 count=8 2>/dev/null | tr -d '\0' | wc -c)" = 0 ] && echo "ok: the gap behind sector 0 is empty again" || fail "the gap still holds data"
[ "$(sfdisk -d "$DISK" | grep -c '^/')" -ge 3 ] && echo "ok: IYAKKI's partitions are still there" || fail "the partitions went"
echo "PASS (bios shared disk)"
