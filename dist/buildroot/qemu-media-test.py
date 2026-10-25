#!/usr/bin/env python3
"""Boot the "web" image in QEMU and drive the MEDIA app with real key presses: the built-in samples, the in-house MP3 player,
the download queue (two addresses from a local server) and the sample video played by the embedded web engine. Saves PNGs and
prints what it measured; read the PNGs: that is the test.

  [QEMU="sudo qemu-system-x86_64"] python3 qemu-media-test.py [build-dir] [out-dir]

Environment: PORT (8741, the local server of the sample files, reached from the guest as http://10.0.2.2:PORT/), WAIT (video
seconds, default 12), SSH_KEY (a public key file: adds it to the initrd and forwards host port 2222 to the guest's SSH).
The key sequences assume the launcher's current layout (Web & Media group: BROWSER, RADIO, MEDIA, CAMERA).
"""
import os, shlex, shutil, socket, struct, subprocess, sys, tempfile, time, zlib

here = os.path.dirname(os.path.abspath(__file__))
build = sys.argv[1] if len(sys.argv) > 1 else os.path.join(here, "../../target/buildroot-web")
out = sys.argv[2] if len(sys.argv) > 2 else "/tmp/iyakki-mediatest"
port = os.environ.get("PORT", "8741")
wait = int(os.environ.get("WAIT", "12"))
qemu = shlex.split(os.environ.get("QEMU", "qemu-system-x86_64"))
os.makedirs(out, exist_ok=True)

initrd = f"{build}/images/rootfs.cpio.zst"
key = os.environ.get("SSH_KEY")
if key:
    tmp = tempfile.mkdtemp()
    os.makedirs(f"{tmp}/etc/iyakki")
    shutil.copy(key, f"{tmp}/etc/iyakki/authorized_keys")
    cpio = subprocess.run("find . | cpio -o -H newc --owner=0:0 2>/dev/null", shell=True, cwd=tmp, capture_output=True).stdout
    data = open(initrd, "rb").read()
    data += b"\0" * (-len(data) % 4) + cpio
    initrd = f"{out}/initrd-with-key.img"
    open(initrd, "wb").write(data)

# the files the guest downloads: short names, copies of the shipped samples
www = tempfile.mkdtemp()
shutil.copy(f"{here}/../samples/Music/IYAKKI Sample Beats.mp3", f"{www}/beat.mp3")
shutil.copy(f"{here}/../samples/Videos/IYAKKI Sample Video.webm", f"{www}/clip.webm")
server = subprocess.Popen([sys.executable, "-m", "http.server", port, "--bind", "0.0.0.0", "-d", www], stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
sock = f"{out}/mon.sock"
if os.path.exists(sock):
    os.remove(sock)
audio = f"{out}/audio.wav"
args = qemu + ["-accel", "kvm", "-accel", "tcg", "-m", "4096", "-smp", "4", "-no-reboot", "-display", "none", "-device", "virtio-vga",
               "-kernel", f"{build}/images/bzImage", "-initrd", initrd,
               "-append", "console=ttyS0 panic=-1" + (" iyakki.ssh=1" if key else ""),
               "-serial", f"file:{out}/serial.log", "-monitor", f"unix:{sock},server,nowait",
               "-device", "qemu-xhci", "-device", "usb-kbd", "-device", "usb-mouse",
               "-netdev", "user,id=n0" + (",hostfwd=tcp:127.0.0.1:2222-:22" if key else ""), "-device", "virtio-net-pci,netdev=n0",
               "-audiodev", f"wav,id=snd0,path={audio}", "-device", "intel-hda", "-device", "hda-duplex,audiodev=snd0"]
q = subprocess.Popen(args, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
for _ in range(100):
    if os.path.exists(sock):
        break
    time.sleep(0.1)
time.sleep(0.5)
m = socket.socket(socket.AF_UNIX)
try:
    m.connect(sock)
except PermissionError:
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


def keys(*ks, pause=0.2):
    for k in ks:
        mon("sendkey " + k, pause)


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
    print(f"{out}/{name}.png: {w}x{h}, ~{colours} colours sampled", flush=True)
    return raw


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
    keys("tab", "right", "right", "ret")                 # the Web & Media group (one Tab from Home), third tile: MEDIA
    time.sleep(6)
    shot("1-tracks")                                      # the samples in a fresh library
    keys("ret")                                           # play the first track (the beats sample): MP3 inside IYAKKI
    time.sleep(5)
    shot("2-playing-mp3")
    size_a = os.path.getsize(audio) if os.path.exists(audio) else 0
    time.sleep(3)
    size_b = os.path.getsize(audio) if os.path.exists(audio) else 0
    print(f"sound written by QEMU's sound card: {size_a} then {size_b} bytes ({'growing' if size_b > size_a > 0 else 'NOT growing'})", flush=True)
    keys("s")                                             # stop
    keys("tab", "tab", "tab")                             # Albums... Videos page
    time.sleep(1)
    shot("3-videos")
    keys("ret")                                           # play the sample video in the embedded engine
    time.sleep(wait)
    a = shot("4-video-a")
    time.sleep(3)
    b = shot("4-video-b")
    print("the two video pictures differ: %s" % (a != b), flush=True)
    keys("spc")                                           # Space pauses (the video started by itself)
    time.sleep(2)
    shot("5-video-paused")
    keys("esc")                                           # stop and back to the lists
    time.sleep(2)
    shot("6-after-video")
    keys("shift-d")                                       # the download form
    time.sleep(1)
    typ(f"http://10.0.2.2:{port}/beat.mp3 http://10.0.2.2:{port}/clip.webm")
    keys("tab"); typ("my test")
    time.sleep(1)
    shot("7-download-form")
    keys("ctrl-s")
    time.sleep(1)
    shot("8-downloads-queued")
    time.sleep(8)
    shot("9-downloads-done")
    ok = True
    print("OK (read the PNGs in %s)" % out)
finally:
    try:
        mon("quit")
    except Exception:
        pass
    q.terminate()
    server.terminate()
sys.exit(0 if ok else 1)
