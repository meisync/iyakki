#!/bin/sh
# The boot counter of a system update (F-UPD-02) read and written by the REAL GRUB of an install stick, in QEMU.
#   dist/qemu/boot-counter-test.sh [build-output-dir]     (GRUB_FROM_DEFCONFIG=1: UEFI with a GRUB made from the defconfig's module list)
# Needs: the built install stick image (images/iyakki-install-x86_64.img), qemu-system-x86_64 (KVM recommended), mtools (mcopy, mtype), sfdisk, python3, cargo. The stick is
# copied first; nothing real is written. What it does: (1) takes boot/grub/grub.cfg from the copy and adds an entry "IYAKKI (previous version)" (the normal entry again with
# `iyakki.entry=previous` added: a real update puts the old kernel on the data partition, here only the choice is tested) and makes the first entry print its command line;
# (2) the REAL guard code of the agent (`devmode::bootcount`, the ignored test `boot_counter_steps_on_a_folder_for_a_real_grub_run`) writes the guard into that menu and arms the
# counter (`iyakki_trial=1` in boot/grub/grubenv); (3) the copy is booted in QEMU again and again and the kernel's own command line says which entry GRUB started:
#   start 1  counter 1: the normal entry starts, GRUB (load_env, save_env on the FAT volume) moves the counter to 2
#   start 2  counter 2 (nobody marked the start good): the previous-version entry starts by itself, the counter becomes 0
#   start 3  counter 0: the normal entry starts again, the counter stays 0
#   then armed again: start 4 normal (counter 2); the system marks it good (the real `mark_good`); start 5 normal again, counter 0 (no fall back after a good start)
# PASS = all of that. Not covered: a real GRUB on real hardware or under Secure Boot, a start that really fails (the counter is not marked on purpose), the update engine writing the
# menu entry with a real data partition.
set -eu
. "$(dirname "$0")/lib.sh"
check_images; need python3; need mtype; need mcopy; need sfdisk; need cargo
STICK="$OUT/images/iyakki-install-x86_64.img"
[ -f "$STICK" ] || { echo "no $STICK (the install image of the build)" >&2; exit 2; }
IMG="$WORK/stick.img"; cp --sparse=always "$STICK" "$IMG"
OFF=$(sfdisk -d "$IMG" | sed -n 's/^[^ ]*1 *: *start= *\([0-9]*\),.*/\1/p' | head -1); [ -n "$OFF" ] || fail "no boot partition on the stick image"
V="$IMG@@$((OFF * 512))"
# GRUB can only run load_env and save_env when its `loadenv` module is in the image (the boot partition carries no module folder). Run as built, this test shows whether the
# BUILT image's GRUB (BIOS, from the boot sector) has it. GRUB_FROM_DEFCONFIG=1 instead makes a UEFI GRUB with the HOST's grub-mkimage and the module list that
# dist/buildroot/configs/iyakki_x86_64_defconfig asks for (BR2_TARGET_GRUB2_BUILTIN_MODULES_EFI), puts it on the stick copy as EFI/BOOT/BOOTX64.EFI and boots it in OVMF: this checks
# the defconfig's list and the guard, not the built image (the host's GRUB version may differ from the image's). Needs OVMF (/usr/share/OVMF) then.
FW=bios
if [ "${GRUB_FROM_DEFCONFIG:-0}" = 1 ]; then
  need grub-mkimage; FW=uefi
  [ -f /usr/share/OVMF/OVMF_VARS_4M.fd ] || { echo "no OVMF" >&2; exit 2; }
  cp /usr/share/OVMF/OVMF_VARS_4M.fd "$WORK/vars.fd"
  MODS=$(sed -n 's/^BR2_TARGET_GRUB2_BUILTIN_MODULES_EFI="\(.*\)"/\1/p' "$ROOT/dist/buildroot/configs/iyakki_x86_64_defconfig")
  grub-mkimage -O x86_64-efi -o "$WORK/grub.efi" -p /boot/grub -c "$ROOT/dist/buildroot/board/iyakki/grub-efi-builtin.cfg" $MODS 2>/dev/null || fail "grub-mkimage failed"
  mcopy -o -i "$V" "$WORK/grub.efi" ::EFI/BOOT/BOOTX64.EFI
  echo "note: the stick copy now starts a UEFI GRUB made with the defconfig's module list: $MODS"
fi
mkdir -p "$WORK/boot/boot/grub"
mtype -i "$V" ::boot/grub/grub.cfg > "$WORK/boot/boot/grub/grub.cfg" || fail "no boot/grub/grub.cfg"
python3 - "$WORK/boot/boot/grub/grub.cfg" <<'PY'
import re, sys
p = sys.argv[1]; s = open(p).read()
m = re.search(r'^menuentry "IYAKKI" \{.*?^\}\n', s, re.S | re.M)
assert m, "no normal entry"
normal = m.group(0)
first = normal.replace("quiet loglevel=3", "loglevel=7")                      # the kernel prints its command line
prev = normal.replace('menuentry "IYAKKI"', 'menuentry "IYAKKI (previous version)"').replace("quiet loglevel=3", "loglevel=7 iyakki.entry=previous")
s = s.replace(normal, first + "\n" + prev, 1).replace('set timeout="3"', 'set timeout="1"')
open(p, "w").write(s)
PY
step() { # mode
  ( cd "$ROOT" && IYAKKI_BOOTCOUNT_DIR="$WORK/boot" IYAKKI_BOOTCOUNT_MODE="$1" cargo test --locked -q -p iyakki-agent --lib -- --ignored boot_counter_steps_on_a_folder_for_a_real_grub_run >"$WORK/step.log" 2>&1 ) || { cat "$WORK/step.log" | tail -20; fail "the agent's boot counter step $1 failed"; }
}
put() { mcopy -o -i "$V" "$WORK/boot/boot/grub/grub.cfg" ::boot/grub/grub.cfg; mcopy -o -i "$V" "$WORK/boot/boot/grub/grubenv" ::boot/grub/grubenv; }
pull() { mtype -i "$V" ::boot/grub/grubenv > "$WORK/boot/boot/grub/grubenv" 2>/dev/null || true; }
trial() { mtype -i "$V" ::boot/grub/grubenv 2>/dev/null | tr -d '\0' | sed -n 's/^iyakki_trial=//p' | head -1; }
mode_args() { if [ "${1:-bios}" = uefi ]; then echo "-drive if=pflash,format=raw,readonly=on,file=/usr/share/OVMF/OVMF_CODE_4M.fd -drive if=pflash,format=raw,file=$WORK/vars.fd"; fi; }
start() { # label expected-entry(normal|previous) expected-counter-after firmware
  n=0; log="$WORK/$1.log"; : > "$log"
  $QEMU -accel kvm -accel tcg -m 2048 -smp 2 -display none -no-reboot $(mode_args "$4") -drive "file=$IMG,format=raw,if=none,id=d" -device usb-ehci -device usb-storage,drive=d,bootindex=0 \
    -serial "file:$log" >/dev/null 2>&1 &
  qp=$!
  CLEANUP="${CLEANUP:+$CLEANUP; }kill $qp 2>/dev/null || true"
  t=0; while [ $t -lt "${TIMEOUT:-240}" ]; do
    grep -aq "Kernel command line:" "$log" && break
    kill -0 $qp 2>/dev/null || break
    sleep 1; t=$((t + 1))
  done
  sleep 1; kill $qp 2>/dev/null || true; wait $qp 2>/dev/null || true
  grep -aq "Kernel command line:" "$log" || { tail -5 "$log"; fail "$1: the kernel never printed its command line (GRUB did not start an entry)"; }
  line=$(tr -d '\r' < "$log" | grep -a "Kernel command line:" | head -1)
  case "$2" in
    normal) echo "$line" | grep -aq "iyakki.entry=previous" && fail "$1: the previous-version entry started, the normal one was expected" ;;
    previous) echo "$line" | grep -aq "iyakki.entry=previous" || fail "$1: the normal entry started, the previous-version entry was expected" ;;
  esac
  got=$(trial)
  [ "$got" = "$3" ] || fail "$1: the counter in grubenv is '$got', expected '$3'"
  echo "ok: $1: GRUB started the $2 entry in ${t} s and the counter is $got (firmware $4)"
}
run_all() { # firmware
  fw="$1"
  step prepare; put
  [ "$(trial)" = 1 ] || fail "the counter was not armed on the stick copy"
  start "$fw-start1" normal 2 "$fw"
  start "$fw-start2" previous 0 "$fw"
  start "$fw-start3" normal 0 "$fw"
  pull; step arm; put
  start "$fw-start4" normal 2 "$fw"
  pull; step good; put
  [ "$(trial)" = 0 ] || fail "mark_good did not clear the counter"
  start "$fw-start5" normal 0 "$fw"
}
echo "=== $FW"
run_all "$FW"
echo "PASS (boot-counter, $FW)"
