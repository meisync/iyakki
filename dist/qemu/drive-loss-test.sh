#!/bin/sh
# The boot drive pulled and plugged in again (F-LOSS-01, F-LOSS-02, F-LOSS-04), on a real kernel, as the plan in docs/STORAGE-LOSS.md describes for a USB-C cable.
#   dist/qemu/drive-loss-test.sh [build-output-dir]
# Needs: the built image (images/bzImage, rootfs.cpio.zst; the boot stage `driveloss` is in images built after 2026-11-14), qemu-system-x86_64 (KVM recommended), python3 with
# Pillow, tesseract (reads the screen). The data volume is a virtual USB disk (xHCI, `usb-storage`) that the script removes with the QEMU monitor's `device_del` and adds
# again with `device_add`; nothing touches a real disk.
#   1. a normal boot with the data disk on USB: the watcher says nothing until the disk is pulled; then the console shows `storage: drive lost at uptime N s` and the screen
#      shows the persistent card "The IYAKKI drive was disconnected" with the advice about the cable (screenshot read with tesseract).
#   2. `iyakki.selftest=driveloss`: the guest writes a file, prints READY, the script pulls the disk, waits 6 seconds and plugs it in again; the guest must see the loss, recognise
#      the same file system (UUID and label) on its new node, reattach it (the automatic step or the Reconnect request, real mount, MS_MOVE and umount2 calls), read the file it wrote
#      before, write a new one and say that the data folder is now mounted from the new node. The guest's own checks decide (selftest-result.py).
# PASS = both parts. Not covered: a real USB-C cable and hub, programs whose working folder sat on the old mount (they keep seeing the dead device until restarted: see the docs),
# k3s stopped and started around the reattach, the apps' own error lines.
set -eu
. "$(dirname "$0")/lib.sh"
check_images; need python3; need tesseract
python3 -c "import PIL" 2>/dev/null || { echo "needs python3-pil (apt install python3-pil)" >&2; exit 2; }
DISK="$WORK/data.img"; truncate -s 512M "$DISK"
echo "=== making the data volume (a first boot: iyakki.selftest=persist1)"
boot "$WORK/p1.log" "iyakki.selftest=persist1 iyakki.disk=vda" -drive "file=$DISK,format=raw,if=virtio"
want "$WORK/p1.log" "IYAKKI-SELFTEST persist1 ok" "the disk became the IYAKKI data volume"
BLK="-blockdev driver=file,node-name=f0,filename=$DISK -blockdev driver=raw,node-name=n0,file=f0 -device qemu-xhci,id=x -device usb-storage,drive=n0,id=u1,bus=x.0"
# run_guest LOG "kernel args" SCRIPT: starts QEMU with the monitor on a socket, runs the python SCRIPT (monitor socket path, serial log path), then stops QEMU
run_guest() {
  log="$1"; args="$2"; script="$3"; mon="$WORK/mon.$$.sock"; rm -f "$mon"
  $QEMU -accel kvm -accel tcg -m 2048 -smp 2 -display none -vga std -no-reboot -kernel "$OUT/images/bzImage" -initrd "$OUT/images/rootfs.cpio.zst" -append "console=tty0 console=ttyS0,115200 panic=-1 $args" \
    $BLK -serial "file:$log" -monitor "unix:$mon,server,nowait" >/dev/null 2>&1 &
  qp=$!
  CLEANUP="${CLEANUP:+$CLEANUP; }kill $qp 2>/dev/null || true"
  python3 - "$mon" "$log" "$WORK" <<EOF || true
import socket, sys, time, re
mon, log, work = sys.argv[1], sys.argv[2], sys.argv[3]
s = None
for _ in range(100):
    try:
        s = socket.socket(socket.AF_UNIX); s.connect(mon); break
    except OSError:
        time.sleep(0.1)
def cmd(c, wait=0.5):
    try:
        s.send((c + "\n").encode())
    except OSError:
        return
    time.sleep(wait)
def text():
    try:
        return open(log, errors="replace").read()
    except OSError:
        return ""
def wait_for(pat, secs):
    t0 = time.time()
    while time.time() - t0 < secs:
        if re.search(pat, text()):
            return True
        time.sleep(0.5)
    return False
$script
EOF
  kill "$qp" 2>/dev/null || true; wait "$qp" 2>/dev/null || true
}
echo "=== 1. a normal boot, then the disk is pulled"
run_guest "$WORK/normal.log" "" '
wait_for(r"supervisor up", 60)
time.sleep(4)
cmd("device_del u1", 10)
cmd("screendump %s/loss.ppm" % work, 2)
cmd("quit", 0.5)
'
want "$WORK/normal.log" "persistent volume sd. mounted" "the data disk (USB) was the data volume"
want "$WORK/normal.log" "storage: drive lost at uptime" "the console says the drive was lost"
want "$WORK/normal.log" "storage: drive on USB port" "and names the port the drive was on"
[ -s "$WORK/loss.ppm" ] || fail "no screenshot"
python3 - "$WORK/loss.ppm" "$WORK/loss.png" <<'EOF'
import sys
from PIL import Image
d = open(sys.argv[1], "rb").read().split(b"\n", 3)
w, h = map(int, d[1].split())
Image.frombytes("RGB", (w, h), d[3]).save(sys.argv[2])
EOF
tesseract "$WORK/loss.png" "$WORK/loss" >/dev/null 2>&1
tr -d '\r' < "$WORK/loss.txt" | sed 's/^/  screen: /' | head -8
want "$WORK/loss.txt" "drive was disconnected" "the screen shows the persistent card"
want "$WORK/loss.txt" "USB-C" "with the advice about the cable"
echo "=== 2. pull and plug in again, the guest decides (iyakki.selftest=driveloss)"
run_guest "$WORK/loss.log" "iyakki.selftest=driveloss" '
if wait_for(r"driveloss READY", 90):
    time.sleep(2)
    cmd("device_del u1", 6)
    cmd("device_add usb-storage,drive=n0,id=u2,bus=x.0", 1)
    wait_for(r"driveloss RESULT", 150)
cmd("quit", 0.5)
'
tr -d '\r' < "$WORK/loss.log" | grep -a "IYAKKI-SELFTEST driveloss\|storage:" | sed 's/^/  /' || true
result "$WORK/loss.log" driveloss
echo "PASS (drive-loss)"
