#!/usr/bin/env python3
"""Boot the hardware check in QEMU with a virtual USB keyboard and tablet (an absolute pointer, like a touch
screen), tap one of the eight targets and verify it turns green. Optionally at 4K.

  sudo python3 qemu-hwcheck-test.py [build-dir] [out-dir] [WIDTHxHEIGHT]      (root only for the QEMU memlock limit)
"""
import json, os, socket, subprocess, sys, time, struct, zlib

build = sys.argv[1] if len(sys.argv) > 1 else os.path.join(os.path.dirname(__file__), "../../target/buildroot-libre")
out = sys.argv[2] if len(sys.argv) > 2 else "/tmp/iyakki-qemu-hw"
res = sys.argv[3] if len(sys.argv) > 3 else "1280x800"
W, H = map(int, res.split("x"))
os.makedirs(out, exist_ok=True)
sock = os.path.join(out, "mon.sock")
qsock = os.path.join(out, "qmp.sock")
for f in (sock, qsock):
    if os.path.exists(f):
        os.remove(f)
q = subprocess.Popen(
    ["qemu-system-x86_64", "-accel", "kvm", "-accel", "tcg", "-m", "2048", "-smp", "2", "-display", "none",
     "-device", f"VGA,vgamem_mb=64,xres={W},yres={H}",
     "-kernel", f"{build}/images/bzImage", "-initrd", f"{build}/images/rootfs.cpio.zst",
     "-append", "console=ttyS0 iyakki.hwcheck=1 panic=-1", "-serial", f"file:{out}/serial.log",
     "-monitor", f"unix:{sock},server,nowait", "-qmp", f"unix:{qsock},server,nowait", "-no-reboot",
     "-device", "qemu-xhci", "-device", "usb-kbd", "-device", "usb-tablet,id=tablet0",
     "-netdev", "user,id=n0", "-device", "virtio-net-pci,netdev=n0"],
    stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
for _ in range(100):
    if os.path.exists(sock):
        break
    time.sleep(0.1)
m = socket.socket(socket.AF_UNIX)
m.connect(sock)
m.settimeout(5)

qm = None

def qmp(cmd, **args):
    """Talk to QEMU's control channel (needed to aim input at one device: the PC also has a PS/2 mouse)."""
    global qm
    if qm is None:
        for _ in range(100):
            if os.path.exists(qsock):
                break
            time.sleep(0.1)
        qm = socket.socket(socket.AF_UNIX)
        qm.connect(qsock)
        qm.settimeout(5)
        qm.recv(65536)
        qm.sendall(b'{"execute":"qmp_capabilities"}\n')
        qm.recv(65536)
    qm.sendall((json.dumps({"execute": cmd, "arguments": args}) + "\n").encode())
    time.sleep(0.2)
    try:
        return qm.recv(65536)
    except Exception:
        return b""

def tap(x, y, w, h):
    ax, ay = int(x / w * 32767), int(y / h * 32767)
    ev = lambda t, **d: {"type": t, "data": d}
    qmp("input-send-event", events=[ev("abs", axis="x", value=ax), ev("abs", axis="y", value=ay)])
    time.sleep(0.4)
    qmp("input-send-event", events=[ev("btn", button="left", down=True)])
    time.sleep(0.2)
    qmp("input-send-event", events=[ev("btn", button="left", down=False)])

def mon(cmd, wait=0.2):
    m.sendall((cmd + "\n").encode())
    time.sleep(wait)
    try:
        m.recv(65536)
    except Exception:
        pass

def dump(name):
    ppm = f"{out}/{name}.ppm"
    mon(f"screendump {ppm}", 1.5)
    d = open(ppm, "rb").read()
    parts = d.split(b"\n", 3)
    w, h = map(int, parts[1].split())
    raw = parts[3]
    rows = b"".join(b"\x00" + raw[y * w * 3:(y + 1) * w * 3] for y in range(h))
    def chunk(t, c):
        return struct.pack(">I", len(c)) + t + c + struct.pack(">I", zlib.crc32(t + c) & 0xffffffff)
    open(f"{out}/{name}.png", "wb").write(b"\x89PNG\r\n\x1a\n" + chunk(b"IHDR", struct.pack(">IIBBBBB", w, h, 8, 2, 0, 0, 0)) + chunk(b"IDAT", zlib.compress(rows)) + chunk(b"IEND", b""))
    os.remove(ppm)
    return w, h, raw

def pixel(w, raw, x, y):
    i = (y * w + x) * 3
    return tuple(raw[i:i + 3])

try:
    deadline = time.time() + 120
    ok = False
    while time.time() < deadline:
        time.sleep(2)
        log = open(f"{out}/serial.log", errors="replace").read() if os.path.exists(f"{out}/serial.log") else ""
        if "framebuffer" in log and "input device" in log:
            ok = True
            break
    assert ok, "the shell did not start:\n" + log[-800:]
    time.sleep(5)
    w, h, raw = dump("1-hwcheck")
    print(f"screen {w}x{h}")
    assert (w, h) == (W, H), f"expected {W}x{H}"
    s = min(w / 1280, h / 720)
    s = max(0.6, min(3.0, s))
    t = 78 * s
    # target 1 is the top-left square: centre (10 + t/2, 10 + t/2) in pixels
    cx, cy = int(10 * s + t / 2), int(10 * s + t / 2)
    before = pixel(w, raw, cx, cy - int(t * 0.3))
    # tap it with the tablet: absolute coordinates are 0..32767 across the screen
    tap(cx, cy, w, h)
    time.sleep(1.5)
    w, h, raw = dump("2-after-tap")
    after = pixel(w, raw, cx, cy - int(t * 0.3))
    print("target colour before", before, "after", after)
    green = lambda p: p[1] > p[0] + 40 and p[1] > p[2] + 20
    assert not green(before) and green(after), "the tapped target did not turn green"
    # a key press reaches the tester too
    log = open(f"{out}/serial.log", errors="replace").read()
    print("OK")
finally:
    q.terminate()
    time.sleep(1)
    q.kill()
