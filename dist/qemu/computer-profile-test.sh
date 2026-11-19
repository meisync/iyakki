#!/bin/sh
# One install on several computers (F-HW-04): the computer is recognised from the REAL DMI strings of the (virtual) machine and its settings are kept apart.
#   dist/qemu/computer-profile-test.sh [build-output-dir]
# Needs: the built image (images/bzImage, rootfs.cpio.zst, iyakki-install-x86_64.img; the boot stage `computer` is in images built after 2026-11-14, otherwise IYAKKI_BIN=path of a newer `iyakki`
# program (musl) is laid over the RAM disk as a second zstd cpio), `iyakki-usb` (IYAKKI_USB, default target/debug/iyakki-usb), qemu-system-x86_64 (KVM recommended), zstd and cpio for IYAKKI_BIN.
# One virtual data disk (made by `iyakki-usb --target-file`, the data volume is formatted by the first start) is started four times; QEMU's `-smbios` makes the DMI strings that the kernel
# shows in /sys/class/dmi/id, which the settings store of the stage reads through `iyakki-hwinfo`:
#   1. computer A (an Apple MacBookPro14,1, serial SERIAL-ONE), `iyakki.hwset=40`: the first computer an install sees adopts the global values; its sound volume is set to 40.
#   2. computer B (an HP Pavilion): a NEW computer: it starts from the defaults (not 40), `iyakki.hwset=55`.
#   3. computer A again with ANOTHER serial number (SERIAL-TWO): the same id (serial numbers are not part of it), volume 40 restored, not new.
#   4. computer B again: volume 55. Two profiles in all. The serial numbers never appear in what the stage prints.
# Not covered: a real Mac, Legion or HP; the other per-computer settings (display, keyboard, pointer, microphone, GPU); display size, which applies at the next sign-in.
set -eu
. "$(dirname "$0")/lib.sh"
check_images; need python3
USB="${IYAKKI_USB:-$ROOT/target/debug/iyakki-usb}"
[ -x "$USB" ] && [ -f "$OUT/images/iyakki-install-x86_64.img" ] || { echo "needs iyakki-usb ($USB) and $OUT/images/iyakki-install-x86_64.img" >&2; exit 2; }
INITRD="$OUT/images/rootfs.cpio.zst"
if [ -n "${IYAKKI_BIN:-}" ]; then
  need zstd; need cpio
  mkdir -p "$WORK/ov/usr/bin"; cp "$IYAKKI_BIN" "$WORK/ov/usr/bin/iyakki"
  ( cd "$WORK/ov" && find . | cpio -o -H newc 2>/dev/null | zstd -q -19 ) > "$WORK/overlay.zst"
  cat "$OUT/images/rootfs.cpio.zst" "$WORK/overlay.zst" > "$WORK/initrd.zst"; INITRD="$WORK/initrd.zst"
fi
DISK="$WORK/disk.img"; truncate -s 1G "$DISK"
"$USB" "$OUT/images/iyakki-install-x86_64.img" --target-file "$DISK" --erase testfile --table mbr >"$WORK/usb.log" 2>&1 || { cat "$WORK/usb.log"; fail "iyakki-usb could not write the test disk"; }
A1="type=1,manufacturer=Apple Inc.,product=MacBookPro14,,1,version=1.0,serial=SERIAL-ONE"
A2="type=1,manufacturer=Apple Inc.,product=MacBookPro14,,1,version=1.0,serial=SERIAL-TWO"
B="type=1,manufacturer=HP,product=HP Pavilion Laptop 16-af0xxx,version=Type1ProductConfigId,serial=SERIAL-THREE"
guest() { # log smbios args
  log="$1"; smb="$2"; args="$3"
  timeout "${TIMEOUT:-240}" $QEMU -accel kvm -accel tcg -m 2048 -smp 2 -nographic -no-reboot -smbios "$smb" \
    -kernel "$OUT/images/bzImage" -initrd "$INITRD" -append "console=ttyS0 panic=-1 iyakki.selftest=computer iyakki.disk=vda $args" -drive "file=$DISK,format=raw,if=virtio" >"$log" 2>&1 || true
  tr -d '\r' < "$log" | grep -a "IYAKKI-SELFTEST computer" | sed 's/^/  /' | cut -c1-200
  want "$log" "computer end" "the stage ran to its end"
}
echo "=== 1. computer A, volume set to 40"; guest "$WORK/a1.log" "$A1" "iyakki.hwset=40"
echo "=== 2. computer B, a new computer, volume set to 55"; guest "$WORK/b1.log" "$B" "iyakki.hwset=55"
echo "=== 3. computer A again with another serial number"; guest "$WORK/a2.log" "$A2" ""
echo "=== 4. computer B again"; guest "$WORK/b2.log" "$B" ""
python3 - "$WORK" <<'PY' || fail "the per-computer profiles did not behave as expected (see the lines above)"
import re, sys
w = sys.argv[1]
def read(n):
    t = open("%s/%s.log" % (w, n), errors="replace").read().replace("\r", "")
    d = {}
    d["id"] = re.search(r"computer id=(\w+) label=\"([^\"]*)\"", t).groups()
    m = re.search(r"computer new=(\w+) volume=(\d+) profiles=(\d+)", t)
    d["new"], d["vol"], d["profiles"] = m.group(1) == "true", int(m.group(2)), int(m.group(3))
    d["text"] = t
    return d
a1, b1, a2, b2 = (read(n) for n in ("a1", "b1", "a2", "b2"))
assert a1["id"][0] == a2["id"][0], "the same model is the same computer whatever the serial number: %s %s" % (a1["id"], a2["id"])
assert a1["id"][0] != b1["id"][0], "another model is another computer"
assert b1["id"][0] == b2["id"][0]
assert a1["id"][1] == "Apple MacBookPro14,1" and "Pavilion" in b1["id"][1], "labels: %s, %s" % (a1["id"][1], b1["id"][1])
assert not a1["new"], "the first computer adopts the existing settings"
assert b1["new"] and b1["vol"] != 40, "computer B is new and starts from the defaults, not A's 40: %s" % b1
assert not a2["new"] and a2["vol"] == 40, "back on A: its 40 is restored: %s" % a2
assert not b2["new"] and b2["vol"] == 55, "back on B: its 55 is restored: %s" % b2
assert a2["profiles"] == 2 and b2["profiles"] == 2
for d in (a1, b1, a2, b2):
    for s in ("SERIAL-ONE", "SERIAL-TWO", "SERIAL-THREE"):
        assert s not in d["text"].split("IYAKKI-SELFTEST computer")[1] if "IYAKKI-SELFTEST computer" in d["text"] else True
print("ok: A=%s B=%s; A adopted, B new (volume %d), A restored 40, B restored 55, serial numbers never shown" % (a1["id"][0], b1["id"][0], b1["vol"]))
PY
echo "PASS (computer-profile)"
