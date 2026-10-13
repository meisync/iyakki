#!/usr/bin/env python3
"""Boots the image in QEMU with a virtual screen and checks that the boot screen (logo, name, progress bar, hint) is
really drawn, then presses L and checks that the loading log appears. Saves the screenshots.

  sudo python3 dist/buildroot/qemu-splash-test.py [build-output-dir] [screenshot-dir]
(root only because QEMU may need it for KVM/io_uring limits; pass QEMU=qemu-system-x86_64 to override.)
"""
import json, os, socket, subprocess, sys, tempfile, time

here = os.path.dirname(os.path.abspath(__file__))
build = sys.argv[1] if len(sys.argv) > 1 else os.path.join(here, "../../target/buildroot-libre")
shots = sys.argv[2] if len(sys.argv) > 2 else tempfile.mkdtemp(prefix="iyakki-splash-")
qemu = os.environ.get("QEMU", "qemu-system-x86_64")
work = tempfile.mkdtemp()
os.makedirs(shots, exist_ok=True)

proc = subprocess.Popen(
    [qemu, "-m", "1024", "-smp", "4", "-accel", "kvm", "-accel", "tcg", "-kernel", f"{build}/images/bzImage",
     "-initrd", f"{build}/images/rootfs.cpio.zst", "-append", "console=ttyS0 quiet loglevel=3 iyakki.splash=8",
     "-vga", "std", "-display", "none", "-serial", f"file:{work}/serial.log", "-qmp", f"unix:{work}/q.sock,server,nowait",
     "-netdev", "user,id=n", "-device", "virtio-net-pci,netdev=n"])
fail = None
try:
    s = socket.socket(socket.AF_UNIX)
    for _ in range(100):
        try:
            s.connect(f"{work}/q.sock"); break
        except OSError:
            time.sleep(0.1)
    f = s.makefile("rw")
    f.readline()

    def cmd(c, a=None):
        f.write(json.dumps({"execute": c, "arguments": a or {}}) + "\n"); f.flush()
        while True:
            r = json.loads(f.readline())
            if "return" in r or "error" in r:
                return r

    cmd("qmp_capabilities")

    def shot(name):
        path = f"{work}/{name}.ppm"
        cmd("screendump", {"filename": path})
        from PIL import Image
        im = Image.open(path).convert("RGB")
        im.save(f"{shots}/{name}.png")
        return im

    def bright(im, box, pred):
        w, h = im.size
        x0, y0, x1, y1 = [int(v) for v in (box[0] * w, box[1] * h, box[2] * w, box[3] * h)]
        raw = im.crop((x0, y0, x1, y1)).tobytes()
        px = [raw[i:i + 3] for i in range(0, len(raw), 3)]
        return sum(1 for p in px if pred(p)) / max(1, len(px))

    # wait for a 1280-wide frame whose middle is not black (the kernel's text mode is 720 wide)
    im = None
    for _ in range(60):
        time.sleep(0.25)
        im = shot("splash")
        if im.size[0] >= 1024 and bright(im, (0.35, 0.2, 0.65, 0.5), lambda p: p[1] > 150 and p[2] > 150) > 0.02:
            break
    cyan = bright(im, (0.35, 0.2, 0.65, 0.5), lambda p: p[1] > 150 and p[2] > 150)       # the logo's cyan
    white = bright(im, (0.4, 0.58, 0.6, 0.7), lambda p: min(p) > 200)                    # the name, "IYAKKI"
    bar = bright(im, (0.28, 0.72, 0.72, 0.76), lambda p: p[2] > 180 and p[0] < 140)       # the progress bar
    stars = bright(im, (0.0, 0.0, 0.25, 1.0), lambda p: max(p) > 80)                      # stars at the left side
    print(f"screen {im.size}: logo cyan {cyan:.3f}, name {white:.3f}, bar {bar:.3f}, stars {stars:.4f}")
    if im.size[0] < 1024: fail = "the screen never switched to the graphics mode"
    elif cyan < 0.02: fail = "the logo is not on screen"
    elif white < 0.005: fail = "the name IYAKKI is not on screen"
    elif bar < 0.01: fail = "the progress bar is not on screen"
    elif stars < 0.0003: fail = "the galaxy is not drawn"
    # the animation moves: two frames a moment apart differ
    time.sleep(0.8)
    im2 = shot("splash2")
    if not fail and im.tobytes() == im2.tobytes(): fail = "the picture is frozen (no animation)"
    # L shows the loading log
    cmd("send-key", {"keys": [{"type": "qcode", "data": "l"}]})
    time.sleep(1.0)
    im3 = shot("splash-log")
    panel = bright(im3, (0.1, 0.12, 0.9, 0.85), lambda p: p[0] > 150 and p[1] > 160 and p[2] > 170)  # light text
    print(f"log panel text {panel:.4f}")
    if not fail and panel < 0.003: fail = "pressing L did not show the loading log"
    cmd("quit")
finally:
    proc.kill() if proc.poll() is None else None
    proc.wait()
print(f"screenshots in {shots}")
print("FAIL: " + fail if fail else "PASS")
sys.exit(1 if fail else 0)
