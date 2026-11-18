#!/bin/sh
# The mouse pointer speed (F-KBD-20) with a REAL relative mouse device: the QEMU mouse in a guest, moved through the QEMU monitor.
#   dist/qemu/pointer-speed-test.sh [build-output-dir]
# Needs: the built image (images/bzImage, rootfs.cpio.zst; the boot stage `pointer` is in images built after 2026-11-14, otherwise IYAKKI_BIN=path of a newer `iyakki` program (musl) is laid over the
# RAM disk as a second zstd cpio), qemu-system-x86_64 (KVM recommended), python3, zstd and cpio for IYAKKI_BIN. The guest has a `virtio-mouse-pci`; the stage opens the input devices like the shell, sets
# the pointer speed from `iyakki.ptrspeed=NN` through `request_pointer_speed` and prints the pointer after each move. Three guest boots (100, 50 and 200 percent): the script moves the mouse by
# 40 units to the right five times (200 units), then 30 units up twice and presses and releases the left button. PASS = the pointer moved 200 x pct/100 pixels sideways and 60 x pct/100 up, the
# button events came at that position. Not covered: a touchpad (its own gain), acceleration of a real mouse, 4K screens, the Settings page.
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
guest() { # pct
  pct="$1"; MON="$WORK/mon$pct.sock"; LOG="$WORK/g$pct.log"
  $QEMU -accel kvm -accel tcg -m 2048 -smp 2 -display none -no-reboot -device virtio-mouse-pci -kernel "$OUT/images/bzImage" -initrd "$INITRD" \
    -append "console=ttyS0 panic=-1 iyakki.selftest=pointer iyakki.ptrspeed=$pct" -serial "file:$LOG" -monitor "unix:$MON,server,nowait" >/dev/null 2>&1 &
  QP=$!
  CLEANUP="${CLEANUP:+$CLEANUP; }kill $QP 2>/dev/null || true"
  python3 - "$MON" "$LOG" <<'PY' || true
import socket, sys, time
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
while time.time() - t0 < 120 and "pointer READY" not in text():
    time.sleep(0.5)
time.sleep(1)
for _ in range(5):
    s.send(b"mouse_move 40 0\n"); time.sleep(0.3)
for _ in range(2):
    s.send(b"mouse_move 0 -30\n"); time.sleep(0.3)
s.send(b"mouse_button 1\n"); time.sleep(0.4); s.send(b"mouse_button 0\n"); time.sleep(1.5)
s.send(b"quit\n"); time.sleep(0.5)
PY
  kill "$QP" 2>/dev/null || true; wait "$QP" 2>/dev/null || true
  tr -d '\r' < "$LOG" | grep -a "IYAKKI-SELFTEST pointer" | grep -av " at " | sed 's/^/  /' | cut -c1-160
  want "$LOG" "pointer READY" "speed $pct: the stage opened the mouse"
}
for p in 100 50 200; do echo "=== speed $p %"; guest $p; done
python3 - "$WORK" <<'PY' || fail "the pointer did not move by the speed asked for (see the lines above)"
import re, sys
w = sys.argv[1]
for pct in (100, 50, 200):
    t = open("%s/g%d.log" % (w, pct), errors="replace").read().replace("\r", "")
    pts = [(int(a), int(b)) for a, b in re.findall(r"pointer at (-?\d+) (-?\d+)", t)]
    assert pts, "speed %d: no pointer events" % pct
    x0, y0 = 512, 384                                  # InputState::new(1024, 768) starts in the middle
    xe, ye = pts[-1]
    dx, dy = xe - x0, ye - y0
    wx, wy = 200 * pct // 100, -60 * pct // 100
    assert abs(dx - wx) <= 2 and abs(dy - wy) <= 2, "speed %d: moved %d,%d, wanted about %d,%d (events %s...)" % (pct, dx, dy, wx, wy, pts[:6])
    d = re.search(r"pointer down (-?\d+) (-?\d+)", t)
    assert d and (int(d.group(1)), int(d.group(2))) == (xe, ye), "speed %d: the click came at %s, the pointer was at %s,%s" % (pct, d and d.groups(), xe, ye)
    print("ok: speed %d %%: the pointer moved %d,%d (asked 200,-60 units), the click came where it was" % (pct, dx, dy))
PY
echo "PASS (pointer-speed)"
