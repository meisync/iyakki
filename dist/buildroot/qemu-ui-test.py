#!/usr/bin/env python3
"""Boot an IYAKKI build in QEMU with a virtual display, then drive the shell with real keystrokes
through the QEMU monitor and save screenshots. Checks the framebuffer shows non-trivial content.

  sudo python3 qemu-ui-test.py [build-dir] [out-dir]      (root only because of the QEMU memlock/io_uring limit)
"""
import os, socket, struct, subprocess, sys, time, zlib

build = sys.argv[1] if len(sys.argv) > 1 else os.path.join(os.path.dirname(__file__), "../../target/buildroot-libre")
out = sys.argv[2] if len(sys.argv) > 2 else "/tmp/iyakki-qemu"
os.makedirs(out, exist_ok=True)
sock = os.path.join(out, "mon.sock")
if os.path.exists(sock):
    os.remove(sock)

q = subprocess.Popen(
    ["qemu-system-x86_64", "-accel", "kvm", "-accel", "tcg", "-m", "1024", "-smp", "2", "-display", "none", "-vga", "std",
     "-kernel", f"{build}/images/bzImage", "-initrd", f"{build}/images/rootfs.cpio.zst",
     "-append", "console=ttyS0 panic=-1", "-serial", f"file:{out}/serial.log",
     "-monitor", f"unix:{sock},server,nowait", "-no-reboot",
     "-netdev", "user,id=n0", "-device", "virtio-net-pci,netdev=n0"],
    stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
for _ in range(100):
    if os.path.exists(sock):
        break
    time.sleep(0.1)
m = socket.socket(socket.AF_UNIX)
m.connect(sock)
m.settimeout(5)

def mon(cmd, wait=0.15):
    m.sendall((cmd + "\n").encode())
    time.sleep(wait)
    try:
        m.recv(65536)
    except Exception:
        pass

def typ(text):
    names = {" ": "spc", ".": "dot", "-": "minus", "/": "slash", "_": "shift-minus"}
    for ch in text:
        mon("sendkey " + names.get(ch, ch), 0.06)

def key(k):
    mon("sendkey " + k, 0.1)

def ppm_to_png(ppm, png):
    d = open(ppm, "rb").read()
    parts = d.split(b"\n", 3)
    w, h = map(int, parts[1].split())
    raw = parts[3]
    rows = b"".join(b"\x00" + raw[y * w * 3:(y + 1) * w * 3] for y in range(h))
    def chunk(t, c):
        return struct.pack(">I", len(c)) + t + c + struct.pack(">I", zlib.crc32(t + c) & 0xffffffff)
    open(png, "wb").write(b"\x89PNG\r\n\x1a\n" + chunk(b"IHDR", struct.pack(">IIBBBBB", w, h, 8, 2, 0, 0, 0)) + chunk(b"IDAT", zlib.compress(rows)) + chunk(b"IEND", b""))
    colors = len(set(raw[i:i + 3] for i in range(0, len(raw), 3 * 97)))
    return w, h, colors

def shot(name):
    ppm = f"{out}/{name}.ppm"
    mon(f"screendump {ppm}", 0.8)
    w, h, colors = ppm_to_png(ppm, f"{out}/{name}.png")
    os.remove(ppm)
    print(f"{name}: {w}x{h}, ~{colors} distinct colours sampled")
    return colors

try:
    deadline = time.time() + 90
    seen = False
    while time.time() < deadline:
        time.sleep(1)
        log = open(f"{out}/serial.log", errors="replace").read() if os.path.exists(f"{out}/serial.log") else ""
        if "supervisor up" in log:
            seen = True
            break
    assert seen, "supervisor did not start:\n" + log[-800:]
    time.sleep(6)  # display driver + shell start
    assert shot("1-first-run-setup") > 20, "setup screen looks blank"
    typ("admin"); key("tab"); typ("adminpass1"); key("tab"); typ("adminpass1"); key("ret")
    time.sleep(3)
    assert shot("2-launcher") > 20
    key("4"); time.sleep(1)
    typ("whoami"); key("ret"); typ("detect"); key("ret"); typ("net"); key("ret")
    time.sleep(1)
    shot("3-cli")
    key("esc"); time.sleep(0.5)
    key("5"); time.sleep(1)  # EDIT
    typ("fn main"); key("ret"); typ("let x"); time.sleep(0.5)
    assert shot("4-edit") > 20
    key("esc"); time.sleep(0.5)
    key("tab"); key("esc"); time.sleep(0.5)  # (an unsaved buffer asks before closing; any other key cancels)
    key("7"); time.sleep(1)  # USERS (admin)
    shot("5-users")
    key("esc"); time.sleep(0.5)
    key("8"); time.sleep(1.5)  # SETTINGS
    assert shot("6-settings") > 20
    print("OK")
finally:
    q.terminate()
    time.sleep(1)
    q.kill()
