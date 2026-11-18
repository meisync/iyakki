#!/bin/sh
# Media and Fn keys from REAL evdev devices (F-KBD-01), on a real kernel: a virtual keyboard in a QEMU guest and keys pressed through the QEMU monitor.
#   dist/qemu/fnkeys-test.sh [build-output-dir]
# Needs: the built image (images/bzImage, rootfs.cpio.zst; the boot stage `fnkeys` is in images built after 2026-11-14, otherwise IYAKKI_BIN=path of a newer `iyakki` program (musl) is laid over the RAM
# disk as a second zstd cpio), qemu-system-x86_64 (KVM recommended), python3, zstd and cpio for IYAKKI_BIN. The guest has a `virtio-keyboard-pci` (the kernel's `virtio_input` makes /dev/input/eventN).
# The stage opens the input devices with the shell's own `scan_devices`, reads them with `poll_devices` and feeds `InputState::feed`, which turns recognised codes into `Event::Fn`. The script waits for
# READY, then presses `audiomute`, `volumeup`, `volumedown`, `audionext`, `audioprev`, `audiostop`, `audioplay`, the ACPI power button (`system_powerdown`) and, last, the letter `a`. PASS = every media key came out as the Fn key the table in
# `fnkeys.rs` says (mute, volume up, volume down, next, previous, stop, play/pause, power), each as a down and an up, in order, and the letter came out as an ordinary key, not as an Fn key.
# Not covered: a real laptop's Fn row and its brightness and keyboard-light keys, a Consumer Control or Video Bus device (only the virtual keyboard and the ACPI Power Button exist), one press reported by two devices.
set -eu
. "$(dirname "$0")/lib.sh"
check_images; need python3
INITRD="$OUT/images/rootfs.cpio.zst"
if [ -n "${IYAKKI_BIN:-}" ]; then
  need zstd; need cpio
  mkdir -p "$WORK/ov/usr/bin"; cp "$IYAKKI_BIN" "$WORK/ov/usr/bin/iyakki"
  ( cd "$WORK/ov" && find . | cpio -o -H newc 2>/dev/null | zstd -q -19 ) > "$WORK/overlay.zst"
  cat "$OUT/images/rootfs.cpio.zst" "$WORK/overlay.zst" > "$WORK/initrd.zst"; INITRD="$WORK/initrd.zst"
fi
MON="$WORK/mon.sock"; LOG="$WORK/g.log"
$QEMU -accel kvm -accel tcg -m 2048 -smp 2 -display none -no-reboot -device virtio-keyboard-pci -kernel "$OUT/images/bzImage" -initrd "$INITRD" \
  -append "console=ttyS0 panic=-1 iyakki.selftest=fnkeys" -serial "file:$LOG" -monitor "unix:$MON,server,nowait" >/dev/null 2>&1 &
QP=$!
CLEANUP="${CLEANUP:+$CLEANUP; }kill $QP 2>/dev/null || true"
python3 - "$MON" "$LOG" <<'PY' || true
import socket, sys, time, re
mon, log = sys.argv[1], sys.argv[2]
s = None
for _ in range(100):
    try:
        s = socket.socket(socket.AF_UNIX); s.connect(mon); break
    except OSError:
        time.sleep(0.1)
def text():
    try:
        return open(log, errors="replace").read()
    except OSError:
        return ""
t0 = time.time()
while time.time() - t0 < 120 and "fnkeys READY" not in text():
    time.sleep(0.5)
time.sleep(1)
for k in ["audiomute", "volumeup", "volumedown", "audionext", "audioprev", "audiostop", "audioplay", "POWER", "a"]:
    # POWER: the ACPI power button (the monitor's system_powerdown), a device of its own that carries only KEY_POWER
    s.send(("system_powerdown\n" if k == "POWER" else "sendkey %s\n" % k).encode()); time.sleep(0.8)
t0 = time.time()
while time.time() - t0 < 60 and "fnkeys end" not in text():
    time.sleep(0.5)
    if time.time() - t0 > 6 and re.search(r"fnkeys key", text()):
        break
s.send(b"quit\n"); time.sleep(0.5)
PY
kill "$QP" 2>/dev/null || true; wait "$QP" 2>/dev/null || true
tr -d '\r' < "$LOG" | grep -a "IYAKKI-SELFTEST fnkeys" | sed 's/^/  /' | cut -c1-200
want "$LOG" "fnkeys READY" "the stage opened the input devices"
python3 - "$LOG" <<'PY' || fail "the media keys did not come out as the table says (see the lines above)"
import re, sys
t = open(sys.argv[1], errors="replace").read().replace("\r", "")
fn = re.findall(r"fnkeys fn (\w+) (down|up)", t)
want = ["VolumeMute", "VolumeUp", "VolumeDown", "Next", "Prev", "Stop", "PlayPause", "Power"]
got = [k for k, d in fn if d == "down"]
assert got == want, "down events %s, wanted %s" % (got, want)
assert [k for k, d in fn if d == "up"] == want, "up events %s" % [k for k, d in fn if d == "up"]
assert "fnkeys key" in t, "the letter did not come out as an ordinary key"
assert not any(k in ("a",) for k, _ in fn)
print("ok: mute, volume up, volume down, next, previous, stop, play/pause and the ACPI power button arrived as Fn keys (down and up each); the letter arrived as an ordinary key")
PY
echo "PASS (fnkeys)"
