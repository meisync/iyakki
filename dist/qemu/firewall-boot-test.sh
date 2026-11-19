#!/bin/sh
# The firewall at boot on a real kernel (F-FW-09).
#   dist/qemu/firewall-boot-test.sh [build-output-dir]
# Needs: the built image, qemu-system-x86_64 (KVM recommended; without KVM raise TIMEOUT), python3 is not needed.
# Two guest boots, each ending by itself after TIMEOUT seconds (the guest is only watched, nothing is typed into it):
#   1. default boot: the console shows "firewall: on (incoming connections blocked unless allowed)" and never "NOT PROTECTED";
#      the firewall line comes before the first line that says a network service is listening (the order F-FW-09 promises).
#   2. `iyakki.firewall=off` on the kernel line: the console shows "firewall: OFF for this boot" and no "NOT PROTECTED".
# PASS = both boots show what they should. Not covered: the F8 emergency list, a boot from an installed disk with a saved rule set, and a real NIC.
set -eu
. "$(dirname "$0")/lib.sh"
check_images
TIMEOUT="${TIMEOUT:-90}"
echo "=== guest 1: default boot (about $TIMEOUT seconds)"
boot "$WORK/default.log" "" -netdev user,id=n0 -device virtio-net-pci,netdev=n0
want "$WORK/default.log" "firewall: on (incoming connections blocked" "the default boot applies the firewall"
unwanted "$WORK/default.log" "NOT PROTECTED" "the default boot is protected"
fw=$(tr -d '\r' < "$WORK/default.log" | grep -an "firewall: on" | head -1 | cut -d: -f1)
listen=$(tr -d '\r' < "$WORK/default.log" | grep -ani "listening on\|listening at" | head -1 | cut -d: -f1)
if [ -n "$listen" ]; then
  [ "$fw" -lt "$listen" ] || fail "a service listened (line $listen) before the firewall was applied (line $fw) (log: $WORK/default.log)"
  echo "ok: the firewall (line $fw) comes before the first listener (line $listen)"
else
  echo "note: no 'listening' line in the console, the order could not be compared (the firewall line is there)"
fi
echo "=== guest 2: iyakki.firewall=off"
boot "$WORK/off.log" "iyakki.firewall=off" -netdev user,id=n0 -device virtio-net-pci,netdev=n0
want "$WORK/off.log" "firewall: OFF for this boot" "the boot option switches it off for this boot only"
unwanted "$WORK/off.log" "NOT PROTECTED" "switching it off on purpose is not reported as a failure"
echo "PASS (firewall-boot)"
