#!/usr/bin/env bash
# Boots the Raspberry Pi 5 image's files in QEMU's aarch64 `virt` machine, the way the Pi firmware hands them over (kernel + the RAM disk named in
# iyakki-boot.txt + the command line of cmdline.txt, no UEFI, no GRUB), and checks what QEMU CAN prove: stage 1 finds IYAKKI-BOOT and IYAKKI-DATA by
# label on the MBR card layout, mounts the seed root, switches root, the data partition is formatted on the first boot and kept on the second, the
# agent starts (boot self-test), and a failed fast start rewrites iyakki-boot.txt to the full RAM disk so the next start still works.
# It does NOT prove: the EEPROM, config.txt, the bcm2712 device tree, RP1 (USB, Ethernet), vc4/v3d, the SD controller, HDMI, Wi-Fi, thermals. Those
# are the owner's first-boot checklist (docs/BOARD-RPI5-FIRST-BOOT.md).
#   dist/rpi/qemu-pi-boot-test.sh OUT_DIR [EVIDENCE_DIR]      OUT_DIR is make-pi-image.sh's output (iyakki-rpi5.img and boot/)
# The image is copied first; the original is never written. One QEMU at a time. Exit 0 only when every check passed.
set -u
OUT="${1:?make-pi-image.sh output dir}"; OUT="$(cd "$OUT" && pwd)"
EV="${2:-$OUT/qemu-evidence}"; mkdir -p "$EV"
QEMU="${QEMU:-qemu-system-aarch64}"; IMG0="$OUT/iyakki-rpi5.img"; BOOT="$OUT/boot"
W="$(mktemp -d "${TMPDIR:-/var/tmp}/pi-qemu-XXXXXX")"; trap 'rm -rf "$W"' EXIT
IMG="$W/card.img"; cp --reflink=auto "$IMG0" "$IMG"
OFF=$(( 8192 * 512 )); export MTOOLS_SKIP_CHECK=1
FAILS=0
ok() { echo "ok   $*"; }
bad() { echo "FAIL $*"; FAILS=$((FAILS + 1)); }
for f in "$IMG0" "$BOOT/kernel8.img" "$BOOT/initrd-lean" "$BOOT/cmdline.txt" "$BOOT/iyakki-boot.txt"; do [ -s "$f" ] || { echo "no $f" >&2; exit 2; }; done

# the RAM disk the firmware would load: the one named in iyakki-boot.txt on the card (read from the card image, like the firmware does)
ramdisk() { mtype -i "$IMG@@$OFF" ::iyakki-boot.txt | sed -n 's/^initramfs \([^ ]*\) followkernel.*/\1/p'; }
boot_once() { # LOGFILE SECONDS EXTRA_APPEND
  local log="$1" secs="$2" extra="$3" rd; rd="$(ramdisk)"; mcopy -n -i "$IMG@@$OFF" "::$rd" "$W/ramdisk" 2>/dev/null || { echo "no ramdisk $rd on the card"; return 2; }
  mcopy -n -i "$IMG@@$OFF" ::kernel8.img "$W/kernel8.img"; local cl; cl="$(mtype -i "$IMG@@$OFF" ::cmdline.txt | tr -d '\n' | sed 's/console=ttyAMA10,115200 //')"   # the Pi 5 debug UART does not exist in virt, and naming it silences the console there: the ONLY word of the line that differs from the board's
  echo "RAMDISK $rd" > "$log"
  timeout "$secs" "$QEMU" -M virt -cpu cortex-a76 -accel tcg -m 2048 -smp 4 -nographic -no-reboot \
    -kernel "$W/kernel8.img" -initrd "$W/ramdisk" -append "$cl console=ttyAMA0,115200 $extra" \
    -drive "if=none,id=d0,file=$IMG,format=raw" -device virtio-blk-pci,drive=d0 \
    -netdev user,id=n0 -device virtio-net-pci,netdev=n0 >> "$log" 2>&1
  tr -d '\r' < "$log" > "$log.clean"; mv "$log.clean" "$log"
}
has() { grep -aq -- "$2" "$1"; }

echo "== 1. first start: lean RAM disk, the card as the firmware lays it out (MBR, FAT IYAKKI-BOOT, marked data partition)"
boot_once "$EV/boot1.log" 600 "iyakki.selftest=1 panic=-1"
L="$EV/boot1.log"
has "$L" "RAMDISK initrd-lean" && ok "the RAM disk named in iyakki-boot.txt is initrd-lean" || bad "iyakki-boot.txt does not name initrd-lean"
has "$L" "stage1: started" && ok "stage 1 ran as /init" || bad "no stage 1 line in the serial log"
has "$L" "stage1: volumes found" && ok "stage 1 found the volumes by label" || bad "stage 1 did not find the volumes"
has "$L" "stage1: boot volume mounted" && ok "boot volume mounted (FAT32 IYAKKI-BOOT)" || bad "boot volume not mounted"
grep -aE "stage1: (root boot/root.sfs mounted|root .*mounted|root layers mounted)" "$L" | head -2 | sed 's/^/     /'
has "$L" "stage1: switching root" && ok "stage 1 switched to the seed root" || bad "no switch to the root"
has "$L" "IYAKKI-SELFTEST begin" && has "$L" "IYAKKI-SELFTEST end" && ok "the agent started and the boot self-test ran to its end" || bad "the self-test did not finish"
has "$L" "IYAKKI-HWREPORT   architecture: aarch64" && ok "the system reports aarch64" || bad "no aarch64 hardware report"
has "$L" "Raspberry Pi" && echo "     (a Pi device tree: unexpected in QEMU)" || ok "not a Pi device tree (QEMU's virt machine): the board gate let the Pi volume through"
# the data partition: formatted by the first boot, read back from the card image
SECT=$(sfdisk -d "$IMG" 2>/dev/null | sed -n 's/.*iyakki-rpi5.img2 *: *start= *\([0-9]*\),.*/\1/p'); [ -n "$SECT" ] || SECT=$(sfdisk -d "$IMG" | awk '/start=/ {gsub(/[^0-9]/,"",$4); s=$4} END {print s}')
LABEL="$(dd if="$IMG" bs=512 skip=$((SECT)) count=4 2>/dev/null | dd bs=1 skip=$((1024 + 120)) count=16 2>/dev/null | tr -d '\0')"
[ "$LABEL" = "IYAKKI-DATA" ] && ok "the data partition was formatted by the first start: ext4 labelled IYAKKI-DATA" || bad "the data partition label is '$LABEL', not IYAKKI-DATA"

echo "== 2. second start: the data volume is kept"
boot_once "$EV/boot2.log" 600 "iyakki.selftest=1 panic=-1"
has "$EV/boot2.log" "stage1: data volume mounted" && ok "stage 1 mounted the data volume formatted by the first start" || bad "the second start did not mount the data volume"
has "$EV/boot2.log" "IYAKKI-SELFTEST end" && ok "the second start also reached the end of the self-test" || bad "the second start did not finish"

echo "== 3. a failed fast start: no seed root on the card"
mdel -i "$IMG@@$OFF" ::boot/root.sfs
rm -f "$W/ramdisk"
# (the data volume still holds the root tree only after an update; this card has none, so stage 1 finds no root at all)
boot_once "$EV/boot3.log" 150 "panic=-1"
has "$EV/boot3.log" "no usable system root on the drive" && ok "stage 1 said, in words, that it found no usable root" || bad "no 'no usable system root' line"
has "$EV/boot3.log" "iyakki-boot.txt rewritten" && ok "stage 1 rewrote iyakki-boot.txt for the next start" || bad "iyakki-boot.txt was not rewritten"
NEXT="$(ramdisk)"
[ "$NEXT" = "initrd" ] && ok "the card now names the full RAM disk (initrd) for the next start" || bad "the card names '$NEXT'"
mtype -i "$IMG@@$OFF" ::iyakki-lean-failed >/dev/null 2>&1 && ok "the marker file iyakki-lean-failed is on the volume" || bad "no iyakki-lean-failed marker"

echo "== 4. the next start takes the full RAM disk by itself"
boot_once "$EV/boot4.log" 600 "iyakki.selftest=1 panic=-1"
has "$EV/boot4.log" "RAMDISK initrd$" && ok "the firmware's choice is the full RAM disk" || bad "the full RAM disk was not chosen"
has "$EV/boot4.log" "IYAKKI-SELFTEST end" && ok "the full RAM disk boots to the end of the self-test without the seed root" || bad "the full start did not finish"

echo
if [ "$FAILS" = 0 ]; then echo "PASS: every check that QEMU can make passed (logs in $EV)"; else echo "FAILED: $FAILS check(s) (logs in $EV)"; fi
exit $(( FAILS > 0 ))
