#!/bin/sh
# The "Install to a disk" boot entry of an install stick (F-INST-10), on a real kernel.
#   dist/qemu/install-entry-test.sh [build-output-dir]
# Needs: the built image (images/bzImage, rootfs.cpio.zst and iyakki-install-x86_64.img), qemu-system-x86_64 (KVM recommended), python3, mtools (mtype), sfdisk, tesseract
# (to read the text console's screen; `apt install tesseract-ocr`). Nothing is written to the image (the stick is a snapshot) or to a real disk.
# What it does: (1) reads boot/grub/grub.cfg from the stick image and takes the kernel line of the entry "IYAKKI - Install to a disk": it must carry iyakki.install=1;
# (2) boots that kernel and RAM disk exactly as GRUB would (same command line), with the stick as a USB drive and a second, empty 6 GB NVMe drive, and takes a screenshot of the
# console after TIMEOUT seconds (default 30); (3) reads the screenshot: the installer's first page is on the text console ("IYAKKI installer step 1 of 6: choose the drive"),
# the source is the stick ("Source image: /dev/sda"), the empty drive is offered ("internal", "no table", "6.4 GB free") and the stick itself is not offered as a target;
# the screenshot shows a few grey levels only (black, the console text and its anti-aliasing): no desktop was started.
# PASS = every check. Not covered: GRUB itself choosing the entry (the kernel line is the same text), the keyboard on a real USB keyboard, the install being run to its end
# (dist/buildroot/qemu-install-test.sh does the install), a GPU's text console on real hardware.
set -eu
. "$(dirname "$0")/lib.sh"
check_images; need python3; need mtype; need sfdisk; need tesseract
STICK="$OUT/images/iyakki-install-x86_64.img"
[ -f "$STICK" ] || { echo "no $STICK (the install image of the build)" >&2; exit 2; }
echo "=== 1. the menu entry on the stick"
OFF=$(sfdisk -d "$STICK" | sed -n 's/^[^ ]*1 *: *start= *\([0-9]*\),.*/\1/p' | head -1)
[ -n "$OFF" ] || fail "could not find the boot partition of the stick image"
mtype -i "$STICK@@$((OFF * 512))" ::boot/grub/grub.cfg > "$WORK/grub.cfg" 2>/dev/null || fail "no boot/grub/grub.cfg on the stick image"
LINE=$(awk '/menuentry "IYAKKI - Install to a disk"/ {f=1} f && /^[ \t]*linux / {print; exit}' "$WORK/grub.cfg")
[ -n "$LINE" ] || fail "the stick's menu has no entry \"IYAKKI - Install to a disk\""
echo "$LINE" | grep -q "iyakki.install=1" && echo "ok: the entry's kernel line has iyakki.install=1" || fail "the install entry does not set iyakki.install=1: $LINE"
ARGS=$(echo "$LINE" | sed 's/^[ \t]*linux [^ ]* //; s/\$iyakki_usbq//')
echo "=== 2. boot it as GRUB would (about ${TIMEOUT:-30} seconds)"
truncate -s 6G "$WORK/target.img"
MON="$WORK/mon.sock"
$QEMU -accel kvm -accel tcg -m 2048 -smp 2 -display none -vga std -no-reboot -kernel "$OUT/images/bzImage" -initrd "$OUT/images/rootfs.cpio.zst" -append "$ARGS" \
  -drive "file=$STICK,format=raw,if=none,id=s,snapshot=on" -device usb-ehci -device usb-storage,drive=s \
  -drive "file=$WORK/target.img,format=raw,if=none,id=t" -device nvme,serial=m80target,drive=t \
  -serial "file:$WORK/serial.log" -monitor "unix:$MON,server,nowait" >/dev/null 2>&1 &
QPID=$!
CLEANUP="${CLEANUP:+$CLEANUP; }kill $QPID 2>/dev/null || true"
python3 - "$MON" "$WORK/screen.ppm" "${TIMEOUT:-30}" <<'EOF'
import socket, sys, time
mon, out, wait = sys.argv[1], sys.argv[2], float(sys.argv[3])
for _ in range(100):
    try:
        s = socket.socket(socket.AF_UNIX); s.connect(mon); break
    except OSError:
        time.sleep(0.1)
time.sleep(wait)
s.send(("screendump %s\n" % out).encode()); time.sleep(2); s.send(b"quit\n"); time.sleep(0.5)
EOF
wait "$QPID" 2>/dev/null || true
[ -s "$WORK/screen.ppm" ] || fail "no screenshot was taken"
python3 - "$WORK/screen.ppm" "$WORK/screen.png" <<'EOF' || fail "the screen shows more than the text console: the desktop or something else was started"
import collections, sys
d = open(sys.argv[1], "rb").read().split(b"\n", 3)
w, h = map(int, d[1].split()); raw = d[3]
c = collections.Counter(raw[i:i + 3] for i in range(0, len(raw), 3 * 5))
print("screen %dx%d, %d colours: %s" % (w, h, len(c), c.most_common(3)))
try:
    from PIL import Image
    Image.frombytes("RGB", (w, h), raw).save(sys.argv[2])
except ImportError:
    sys.exit(0 if len(c) <= 4 else 1)
sys.exit(0 if len(c) <= 4 else 1)
EOF
echo "ok: the console shows text in black and grey only (no desktop)"
echo "=== 3. what the installer says"
[ -f "$WORK/screen.png" ] || fail "python3 needs Pillow to read the screen (apt install python3-pil)"
tesseract "$WORK/screen.png" "$WORK/ocr" >/dev/null 2>&1
tr -d '\r' < "$WORK/ocr.txt" | sed 's/^/  /'
want "$WORK/ocr.txt" "installer step 1 of 6" "the installer's first page is on the console"
want "$WORK/ocr.txt" "choose the drive" "it asks which drive"
want "$WORK/ocr.txt" "Source image: /dev/sda" "the source is the stick"
want "$WORK/ocr.txt" "internal" "the empty NVMe drive is offered"
want "$WORK/ocr.txt" "no table" "it is shown as having no partition table"
want "$WORK/ocr.txt" "6.4 GB" "with its size"
n=$(tr -d '\r' < "$WORK/ocr.txt" | grep -c '/dev/' || true)
[ "$n" -le 2 ] && echo "ok: only the empty drive is offered next to the source line (the stick is not a target)" || fail "more drives are listed than expected ($n lines with /dev/)"
echo "PASS (install-entry)"
