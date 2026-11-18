#!/bin/sh
# The USB controllers kept awake (F-NET-15) on a REAL kernel's sysfs: emulated PCI USB host controllers in a QEMU guest.
#   dist/qemu/usb-awake-test.sh [build-output-dir]
# Needs: the built image (images/bzImage, rootfs.cpio.zst; the boot stage `usbawake` is in images built after 2026-11-14, otherwise IYAKKI_BIN=path of a newer `iyakki` program (musl) is laid
# over the RAM disk as a second zstd cpio), qemu-system-x86_64 (KVM recommended), zstd and cpio for IYAKKI_BIN. The guest has an xHCI and an EHCI controller (PCI class 0x0c03); the stage lists their
# `power/control` files (the emulated ones start as `on`, so the stage puts them in `auto` first, as the Mac mini's Thunderbolt-side controller was), takes the sleeping ones through `usbawake::sleeping_controllers` (the machine must have a /sys/bus/thunderbolt, which this kernel has), writes `on` through `keep_awake`
# and reads the files again. PASS = the kernel took `auto` for the controllers, `sleeping_controllers` found them, every one that was listed says `on` after, and the step reports each as switched.
# Not covered: the Mac mini's Thunderbolt-side xHCI and whether this fixes a stick plugged into USB-C after the start (the cause is a hypothesis, docs/development/USB-DETECT-AND-STARTUP-DISK-PLAN.md).
set -eu
. "$(dirname "$0")/lib.sh"
check_images
INITRD="$OUT/images/rootfs.cpio.zst"
if [ -n "${IYAKKI_BIN:-}" ]; then
  need zstd; need cpio
  mkdir -p "$WORK/ov/usr/bin"; cp "$IYAKKI_BIN" "$WORK/ov/usr/bin/iyakki"
  ( cd "$WORK/ov" && find . | cpio -o -H newc 2>/dev/null | zstd -q -19 ) > "$WORK/overlay.zst"
  cat "$OUT/images/rootfs.cpio.zst" "$WORK/overlay.zst" > "$WORK/initrd.zst"; INITRD="$WORK/initrd.zst"
fi
timeout "${TIMEOUT:-240}" $QEMU -accel kvm -accel tcg -m 2048 -smp 2 -nographic -no-reboot -device qemu-xhci,id=xhci -device usb-ehci,id=ehci \
  -kernel "$OUT/images/bzImage" -initrd "$INITRD" -append "console=ttyS0 panic=-1 iyakki.selftest=usbawake" >"$WORK/g.log" 2>&1 || true
tr -d '\r' < "$WORK/g.log" | grep -a "IYAKKI-SELFTEST usbawake" | sed 's/^/  /' | cut -c1-200
want "$WORK/g.log" "usbawake end" "the stage ran to its end"
python3 - "$WORK/g.log" <<'PY' || fail "the controllers were not kept awake as described (see the lines above)"
import re, sys
t = open(sys.argv[1], errors="replace").read().replace("\r", "")
assert "thunderbolt-bus true" in t, "this kernel has no /sys/bus/thunderbolt, the step would do nothing"
before = dict(re.findall(r"set-auto (\S+) control=(\w+)", t))
assert before and all(v == "auto" for v in before.values()), "the kernel did not take `auto`: %s" % before
sleeping = int(re.search(r"sleeping (\d+)", t).group(1))
switched = int(re.search(r"switched (\d+)", t).group(1))
after = dict(re.findall(r"after (\S+) control=(\w+)", t))
assert before and sleeping >= 1, "no USB controller was in automatic power management: %s" % before
assert switched == sleeping and len(after) == sleeping and all(v == "on" for v in after.values()), "after: %s switched %d of %d" % (after, switched, sleeping)
print("ok: %d of %d USB controllers were in `auto` (%s) and the step set them to `on`" % (sleeping, len(before), before))
PY
echo "PASS (usb-awake)"
