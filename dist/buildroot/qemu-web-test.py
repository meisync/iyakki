#!/usr/bin/env python3
"""Boot the optional "web" image in QEMU and show pages with the embedded full web engine (WPE WebKit helper), saving screenshots.

  [QEMU="sudo qemu-system-x86_64"] python3 qemu-web-test.py [build-dir] [out-dir]

What it does: boots kernel + initrd directly (4 GB RAM, 4 CPUs, KVM when there is one, user networking), creates the first
account through the sign-in screen with real key presses (QEMU monitor `sendkey`), opens the BROWSER tile, loads each address in the
built-in reader, presses F (full engine) and takes a screenshot after WAIT seconds. Read the PNGs: that is the test.
Environment: URLS (space separated; default: the local test page, google.com, youtube.com), WAIT (default 40), PORT (8731),
SSH_KEY (a public key file: adds it to the initrd as /etc/iyakki/authorized_keys and forwards host port 2222 to the guest's SSH,
which is how the helper's log and `ps` can be looked at: `ssh -p 2222 root@127.0.0.1`).
The local test page (canvas animation, CSS grid, fonts) is served from this machine, reached from the guest as http://10.0.2.2:PORT/ .
The tile position assumes the launcher's current layout (the BROWSER tile is one row down, one to the right of CHAT).
"""
import os, shlex, shutil, socket, struct, subprocess, sys, tempfile, time, zlib

here = os.path.dirname(os.path.abspath(__file__))
build = sys.argv[1] if len(sys.argv) > 1 else os.path.join(here, "../../target/buildroot-web")
out = sys.argv[2] if len(sys.argv) > 2 else "/tmp/iyakki-webtest"
port = os.environ.get("PORT", "8731")
wait = int(os.environ.get("WAIT", "40"))
qemu = shlex.split(os.environ.get("QEMU", "qemu-system-x86_64"))
urls = os.environ.get("URLS", f"http://10.0.2.2:{port}/ https://www.google.com/ https://www.youtube.com/").split()
os.makedirs(out, exist_ok=True)

initrd = f"{build}/images/rootfs.cpio.zst"
key = os.environ.get("SSH_KEY")
if key:
    # a second cpio appended to the first (padded to 4 bytes) adds the key without rebuilding the image
    tmp = tempfile.mkdtemp()
    os.makedirs(f"{tmp}/etc/iyakki")
    shutil.copy(key, f"{tmp}/etc/iyakki/authorized_keys")
    cpio = subprocess.run("find . | cpio -o -H newc --owner=0:0 2>/dev/null", shell=True, cwd=tmp, capture_output=True).stdout
    data = open(initrd, "rb").read()
    data += b"\0" * (-len(data) % 4) + cpio
    initrd = f"{out}/initrd-with-key.img"
    open(initrd, "wb").write(data)

server = subprocess.Popen([sys.executable, f"{here}/qemu-web-test/server.py", port, f"{out}/server.log"])
sock = f"{out}/mon.sock"
if os.path.exists(sock):
    os.remove(sock)
args = qemu + ["-accel", "kvm", "-accel", "tcg", "-m", "4096", "-smp", "4", "-no-reboot", "-display", "none", "-device", "virtio-vga",
               "-kernel", f"{build}/images/bzImage", "-initrd", initrd,
               "-append", "console=ttyS0 panic=-1" + (" iyakki.ssh=1" if key else ""),
               "-serial", f"file:{out}/serial.log", "-monitor", f"unix:{sock},server,nowait",
               "-device", "qemu-xhci", "-device", "usb-kbd", "-device", "usb-mouse",
               "-netdev", "user,id=n0" + (",hostfwd=tcp:127.0.0.1:2222-:22" if key else ""), "-device", "virtio-net-pci,netdev=n0",
               "-audiodev", f"wav,id=snd0,path={out}/audio.wav", "-device", "intel-hda", "-device", "hda-duplex,audiodev=snd0"]
q = subprocess.Popen(args, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
for _ in range(100):
    if os.path.exists(sock):
        break
    time.sleep(0.1)
time.sleep(0.5)
m = socket.socket(socket.AF_UNIX)
try:
    m.connect(sock)
except PermissionError:           # QEMU started with sudo made the socket root's
    subprocess.run(['sudo', 'chmod', '666', sock])
    m.connect(sock)
m.settimeout(5)
NAMES = {" ": "spc", ".": "dot", "-": "minus", "/": "slash", "_": "shift-minus", ":": "shift-semicolon", "?": "shift-slash", "=": "equal", "&": "shift-7", "%": "shift-5", "@": "shift-2", ",": "comma", ";": "semicolon"}

def mon(cmd, wait=0.12):
    m.sendall((cmd + "\n").encode())
    time.sleep(wait)
    try:
        m.recv(65536)
    except Exception:
        pass

def typ(text):
    for ch in text:
        mon("sendkey " + NAMES.get(ch, "shift-" + ch.lower() if ch.isupper() else ch), 0.05)

def keys(*ks):
    for k in ks:
        mon("sendkey " + k, 0.15)

def shot(name):
    ppm = f"{out}/{name}.ppm"
    mon(f"screendump {ppm}", 1.0)
    d = open(ppm, "rb").read()
    parts = d.split(b"\n", 3)
    w, h = map(int, parts[1].split())
    raw = parts[3]
    rows = b"".join(b"\x00" + raw[y * w * 3:(y + 1) * w * 3] for y in range(h))
    def chunk(t, c):
        return struct.pack(">I", len(c)) + t + c + struct.pack(">I", zlib.crc32(t + c) & 0xffffffff)
    open(f"{out}/{name}.png", "wb").write(b"\x89PNG\r\n\x1a\n" + chunk(b"IHDR", struct.pack(">IIBBBBB", w, h, 8, 2, 0, 0, 0)) + chunk(b"IDAT", zlib.compress(rows, 1)) + chunk(b"IEND", b""))
    os.remove(ppm)
    colours = len(set(raw[i:i + 3] for i in range(0, len(raw), 3 * 97)))
    print(f"{out}/{name}.png: {w}x{h}, ~{colours} colours sampled")
    return colours

ok = False
try:
    for _ in range(240):
        log = open(f"{out}/serial.log", errors="replace").read() if os.path.exists(f"{out}/serial.log") else ""
        if "shell: " in log and "frames in" in log:
            break
        time.sleep(1)
    time.sleep(3)
    typ("admin"); keys("tab"); typ("adminpass1"); keys("tab"); typ("adminpass1"); keys("ret")
    time.sleep(8)
    keys("down", "right", "ret")        # the BROWSER tile
    time.sleep(3)
    ok = True
    for n, url in enumerate(urls, 1):
        keys("ctrl-l") if n > 1 else None
        typ(url); keys("ret")
        time.sleep(6)
        if n == 1:
            keys("shift-f")             # the full engine (the second and later addresses are sent to it by Ctrl+L)
        time.sleep(wait)
        ok &= shot(f"{n}") > 30
    print("OK (read the PNGs in %s)" % out if ok else "FAIL: a screenshot looks blank")
finally:
    try:
        mon("quit")
    except Exception:
        pass
    q.terminate()
    server.terminate()
sys.exit(0 if ok else 1)
