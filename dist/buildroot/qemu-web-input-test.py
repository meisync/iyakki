#!/usr/bin/env python3
"""Typing and clicking in the full web engine, in QEMU: boots an image, opens BROWSER, leaves the machine running and lets you drive it.

  python3 qemu-web-input-test.py boot  BUILD_DIR OUT_DIR [EXTRA.cpio]   boot, create the account, open BROWSER (QEMU keeps running)
  python3 qemu-web-input-test.py ctl OUT_DIR shot NAME | click X Y | move X Y | drag X Y X2 Y2 | wheel N | type TEXT | keys K... | quit

`boot` serves qemu-web-test/input.html (text input, textarea, password, JS log of focus/keydown/input/mouse/wheel events that the page
reports to the server log OUT_DIR/server.log as `REPORT ...` lines) at http://10.0.2.2:PORT/input.html. EXTRA.cpio (optional) is appended to
the initrd, e.g. a newer /usr/libexec/iyakki/webview-helper. The pointer is `usb-tablet` (absolute); X Y are screenshot pixels.
Environment: PORT (8731), QEMU.
"""
import os, shlex, socket, struct, subprocess, sys, time, zlib

here = os.path.dirname(os.path.abspath(__file__))
NAMES = {" ": "spc", ".": "dot", "-": "minus", "/": "slash", "_": "shift-minus", ":": "shift-semicolon", "?": "shift-slash", "=": "equal", "&": "shift-7", "%": "shift-5", "@": "shift-2", ",": "comma", ";": "semicolon",
         "!": "shift-1", "#": "shift-3", "$": "shift-4", "^": "shift-6", "*": "shift-8", "(": "shift-9", ")": "shift-0", "+": "shift-equal", "'": "apostrophe", '"': "shift-apostrophe", "<": "shift-comma", ">": "shift-dot", "~": "shift-grave_accent", "\\": "backslash", "|": "shift-backslash", "[": "bracket_left", "]": "bracket_right", "{": "shift-bracket_left", "}": "shift-bracket_right", "`": "grave_accent"}

def connect(out):
    m = socket.socket(socket.AF_UNIX)
    m.connect(f"{out}/mon.sock")
    m.settimeout(5)
    return m

def mon(m, cmd, wait=0.12):
    m.sendall((cmd + "\n").encode())
    time.sleep(wait)
    try:
        m.recv(65536)
    except Exception:
        pass

def typ(m, text):
    for ch in text:
        mon(m, "sendkey " + NAMES.get(ch, "shift-" + ch.lower() if ch.isupper() else ch), 0.06)

def keys(m, *ks):
    for k in ks:
        mon(m, "sendkey " + k, 0.15)

def shot(m, out, name):
    ppm = f"{out}/{name}.ppm"
    mon(m, f"screendump {ppm}", 1.0)
    d = open(ppm, "rb").read()
    parts = d.split(b"\n", 3)
    w, h = map(int, parts[1].split())
    raw = parts[3]
    rows = b"".join(b"\x00" + raw[y * w * 3:(y + 1) * w * 3] for y in range(h))
    def chunk(t, c):
        return struct.pack(">I", len(c)) + t + c + struct.pack(">I", zlib.crc32(t + c) & 0xffffffff)
    open(f"{out}/{name}.png", "wb").write(b"\x89PNG\r\n\x1a\n" + chunk(b"IHDR", struct.pack(">IIBBBBB", w, h, 8, 2, 0, 0, 0)) + chunk(b"IDAT", zlib.compress(rows, 1)) + chunk(b"IEND", b""))
    os.remove(ppm)
    open(f"{out}/size", "w").write(f"{w} {h}")
    print(f"{out}/{name}.png {w}x{h}")

def size(out):
    return tuple(map(int, open(f"{out}/size").read().split()))

REL = os.environ.get("POINTER") == "rel"      # a relative USB mouse (like a real one) instead of the absolute tablet

def point(m, out, x, y):
    w, h = size(out)
    if REL:
        for _ in range(40):
            mon(m, "mouse_move -127 -127", 0.03)
        dx, dy = int(x), int(y)
        while dx or dy:
            sx, sy = max(-127, min(127, dx)), max(-127, min(127, dy))
            mon(m, f"mouse_move {sx} {sy}", 0.03)
            dx -= sx; dy -= sy
        time.sleep(0.2)
        return
    mon(m, f"mouse_move {int(x * 32767 / (w - 1))} {int(y * 32767 / (h - 1))}", 0.2)

def boot(build, out, extra):
    port = os.environ.get("PORT", "8731")
    qemu = shlex.split(os.environ.get("QEMU", "qemu-system-x86_64"))
    os.makedirs(out, exist_ok=True)
    initrd = f"{build}/images/rootfs.cpio.zst"
    if extra:
        data = open(initrd, "rb").read()
        data += b"\0" * (-len(data) % 4) + open(extra, "rb").read()
        initrd = f"{out}/initrd-extra.img"
        open(initrd, "wb").write(data)
    server = subprocess.Popen([sys.executable, f"{here}/qemu-web-test/server.py", port, f"{out}/server.log"])
    sock = f"{out}/mon.sock"
    if os.path.exists(sock):
        os.remove(sock)
    args = qemu + ["-accel", "kvm", "-accel", "tcg", "-m", "4096", "-smp", "4", "-no-reboot", "-display", "none", "-device", "virtio-vga" + os.environ.get("VGA", ""),
                   "-kernel", f"{build}/images/bzImage", "-initrd", initrd, "-append", "console=ttyS0 panic=-1",
                   "-serial", f"file:{out}/serial.log", "-monitor", f"unix:{sock},server,nowait",
                   "-device", "qemu-xhci", "-device", "usb-kbd", "-device", ("usb-mouse" if REL else "usb-tablet"),
                   "-netdev", "user,id=n0", "-device", "virtio-net-pci,netdev=n0"]
    q = subprocess.Popen(args, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
    open(f"{out}/pids", "w").write(f"{q.pid} {server.pid}")
    for _ in range(100):
        if os.path.exists(sock):
            break
        time.sleep(0.1)
    time.sleep(0.5)
    m = connect(out)
    for _ in range(240):
        log = open(f"{out}/serial.log", errors="replace").read() if os.path.exists(f"{out}/serial.log") else ""
        if "shell: " in log and "frames in" in log:
            break
        time.sleep(1)
    time.sleep(3)
    typ(m, "admin"); keys(m, "tab"); typ(m, "adminpass1"); keys(m, "tab"); typ(m, "adminpass1"); keys(m, "ret")
    time.sleep(8)
    keys(m, "tab", "ret")
    time.sleep(3)
    shot(m, out, "booted")
    print("ready; qemu pid", q.pid)

def ctl(out, cmd, a):
    m = connect(out)
    if cmd == "shot":
        shot(m, out, a[0])
    elif cmd in ("click", "move"):
        point(m, out, int(a[0]), int(a[1]))
        if cmd == "click":
            mon(m, "mouse_button 1", 0.12); mon(m, "mouse_button 0", 0.3)
    elif cmd == "drag":
        point(m, out, int(a[0]), int(a[1])); mon(m, "mouse_button 1", 0.15)
        point(m, out, int(a[2]), int(a[3])); mon(m, "mouse_button 0", 0.3)
    elif cmd == "wheel":
        for _ in range(abs(int(a[0]))):
            mon(m, "mouse_button %d" % (16 if int(a[0]) < 0 else 8), 0.05)   # QEMU monitor: 8 = wheel up? see below
            mon(m, "mouse_button 0", 0.05)
    elif cmd == "type":
        typ(m, " ".join(a))
    elif cmd == "keys":
        keys(m, *a)
    elif cmd == "quit":
        mon(m, "quit")
        try:
            for p in open(f"{out}/pids").read().split():
                os.kill(int(p), 15)
        except Exception:
            pass

if __name__ == "__main__":
    if sys.argv[1] == "boot":
        boot(sys.argv[2], sys.argv[3], sys.argv[4] if len(sys.argv) > 4 else None)
    else:
        ctl(sys.argv[2], sys.argv[3], sys.argv[4:])
