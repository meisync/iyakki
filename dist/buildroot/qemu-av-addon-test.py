#!/usr/bin/env python3
"""Boots the web image in QEMU and checks the audio and video add-on (docs/development/AV-ADDON-BUILD.md). No stick, no Mac.

  SSH_KEY=~/.ssh/some.pub [ADDON=/path/ffmpeg-libs-....tar.gz] [BRIDGE=hooktree] python3 qemu-av-addon-test.py [build-dir] [out-dir]

The base image is build-dir/images/rootfs.cpio.zst. A base built before WEB_AV_ADDON has no bridge plugin, so BRIDGE (a folder made by
running web-post-build.sh with IYAKKI_WEB_AV_ADDON=1 on a copy of the libraries) is appended to the initrd as a second cpio together
with the SSH key: that is what a repack of a finished image would have. With ADDON the archive is unpacked over SSH into the data
folder the way the installer does (<data>/addons/ffmpeg); without it the add-on is absent. Then the BROWSER tile opens the local
test page (qemu-web-test/media/av-test.html: an H.264 + AAC mp4 in a <video>), the full engine is started (Shift+F) and the page's
own reports (GET /report, written to out-dir/server.log) say whether the video reached `playing`. A screenshot and the
engine's environment are saved too.
"""
import os, shlex, shutil, socket, struct, subprocess, sys, tempfile, time, zlib, glob

here = os.path.dirname(os.path.abspath(__file__))
build = sys.argv[1] if len(sys.argv) > 1 else os.path.join(here, "../../target/buildroot-web")
out = sys.argv[2] if len(sys.argv) > 2 else os.path.expanduser("~/iyakki-work/addons/qemu")
port = os.environ.get("PORT", "8741")
wait = int(os.environ.get("WAIT", "45"))
key = os.environ["SSH_KEY"]
addon = os.environ.get("ADDON")
bridge = os.environ.get("BRIDGE")
qemu = shlex.split(os.environ.get("QEMU", "qemu-system-x86_64"))
os.makedirs(out, exist_ok=True)
for f in glob.glob(f"{out}/*.log") + glob.glob(f"{out}/*.png"):
    os.remove(f)

tmp = tempfile.mkdtemp(dir=os.path.expanduser("~/.cache/iyakki-tmp"))
os.makedirs(f"{tmp}/etc/iyakki")
shutil.copy(key, f"{tmp}/etc/iyakki/authorized_keys")
if bridge:
    subprocess.run(["cp", "-a", f"{bridge}/usr", f"{tmp}/"], check=True)
    for junk in ("fonts", "web-gl-drivers"):
        shutil.rmtree(f"{tmp}/usr/share/fonts", ignore_errors=True)
        try: os.remove(f"{tmp}/usr/share/iyakki/web-gl-drivers")
        except FileNotFoundError: pass
cpio = subprocess.run("find . | cpio -o -H newc --owner=0:0 2>/dev/null", shell=True, cwd=tmp, capture_output=True).stdout
data = open(f"{build}/images/rootfs.cpio.zst", "rb").read()
data += b"\0" * (-len(data) % 4) + cpio
initrd = f"{out}/initrd-av.img"
open(initrd, "wb").write(data)

here_srv = f"{here}/qemu-web-test/server.py"
server = subprocess.Popen([sys.executable, here_srv, port, f"{out}/server.log"])
sock = f"{out}/mon.sock"
if os.path.exists(sock): os.remove(sock)
args = qemu + ["-accel", "kvm", "-accel", "tcg", "-m", "4096", "-smp", "4", "-no-reboot", "-display", "none", "-device", os.environ.get("QEMU_VGA", "VGA"),
               "-kernel", f"{build}/images/bzImage", "-initrd", initrd, "-append", "console=ttyS0 panic=-1 iyakki.ssh=1",
               "-serial", f"file:{out}/serial.log", "-monitor", f"unix:{sock},server,nowait",
               "-device", "qemu-xhci", "-device", "usb-kbd", "-device", "usb-mouse",
               "-netdev", "user,id=n0,hostfwd=tcp:127.0.0.1:2223-:22", "-device", "virtio-net-pci,netdev=n0",
               "-audiodev", f"wav,id=snd0,path={out}/audio.wav", "-device", "intel-hda", "-device", "hda-duplex,audiodev=snd0"]
q = subprocess.Popen(args, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
for _ in range(100):
    if os.path.exists(sock): break
    time.sleep(0.1)
time.sleep(0.5)
m = socket.socket(socket.AF_UNIX); m.connect(sock); m.settimeout(5)
NAMES = {" ": "spc", ".": "dot", "-": "minus", "/": "slash", "_": "shift-minus", ":": "shift-semicolon", "?": "shift-slash", "=": "equal"}
def mon(cmd, w=0.12):
    m.sendall((cmd + "\n").encode()); time.sleep(w)
    try: m.recv(65536)
    except Exception: pass
def typ(text):
    for ch in text: mon("sendkey " + NAMES.get(ch, "shift-" + ch.lower() if ch.isupper() else ch), 0.05)
def keys(*ks):
    for k in ks: mon("sendkey " + k, 0.15)
def ssh(cmd, t=60):
    r = subprocess.run(["ssh", "-p", "2223", "-o", "StrictHostKeyChecking=no", "-o", "UserKnownHostsFile=/dev/null", "-o", "LogLevel=ERROR", "-o", "ConnectTimeout=5",
                        "-i", key[:-4], "root@127.0.0.1", cmd], capture_output=True, text=True, timeout=t)
    return (r.stdout + r.stderr).strip()
def upload(src, dst):   # the guest's SSH has no sftp server: a pipe through the shell
    with open(src, "rb") as f:
        subprocess.run(["ssh", "-p", "2223", "-o", "StrictHostKeyChecking=no", "-o", "UserKnownHostsFile=/dev/null", "-o", "LogLevel=ERROR", "-i", key[:-4], "root@127.0.0.1", f"cat > {dst}"], stdin=f, check=True)
def shot(name):
    ppm = f"{out}/{name}.ppm"; mon(f"screendump {ppm}", 1.0)
    d = open(ppm, "rb").read(); parts = d.split(b"\n", 3); w, h = map(int, parts[1].split()); raw = parts[3]
    rows = b"".join(b"\x00" + raw[y * w * 3:(y + 1) * w * 3] for y in range(h))
    def chunk(t, c): return struct.pack(">I", len(c)) + t + c + struct.pack(">I", zlib.crc32(t + c) & 0xffffffff)
    open(f"{out}/{name}.png", "wb").write(b"\x89PNG\r\n\x1a\n" + chunk(b"IHDR", struct.pack(">IIBBBBB", w, h, 8, 2, 0, 0, 0)) + chunk(b"IDAT", zlib.compress(rows, 1)) + chunk(b"IEND", b""))
    os.remove(ppm)

try:
    for _ in range(300):
        log = open(f"{out}/serial.log", errors="replace").read() if os.path.exists(f"{out}/serial.log") else ""
        if "shell: " in log and "frames in" in log: break
        time.sleep(1)
    time.sleep(3)
    typ("admin"); keys("tab"); typ("adminpass1"); keys("tab"); typ("adminpass1"); keys("ret")
    time.sleep(10)
    print("guest bridge files:", ssh("ls -l /usr/lib/iyakki-av/gstreamer-1.0/ ; cat /usr/share/iyakki/av-addon-abi | grep -v '^#' | tr '\\n' ' '; echo; ls /var/lib/iyakki | head -20"))
    if addon:
        upload(addon, "/tmp/addon.tar.gz")
        print("install:", ssh("set -e; D=/var/lib/iyakki/addons; mkdir -p $D/x && gunzip -c /tmp/addon.tar.gz | tar x -C $D/x && rm -rf $D/ffmpeg && mv $D/x/ffmpeg-libs-* $D/ffmpeg && rmdir $D/x && ls $D/ffmpeg/lib && $D/ffmpeg/bin/iyakki-av-check | tail -2"))
    print("gst-inspect avdec_h264 (plain engine env):", ssh("GST_REGISTRY_1_0=/tmp/r1 gst-inspect-1.0 avdec_h264 2>&1 | head -3"))
    shot("home")
    keys(*os.environ.get("TILE_KEYS", "tab,tab,ret").split(",")); time.sleep(3)
    shot("tile")
    if os.environ.get("STOP"): raise SystemExit
    typ(f"http://10.0.2.2:{port}/media/av-test.html"); keys("ret"); time.sleep(6)
    keys("shift-f"); time.sleep(wait // 2)
    keys("ret"); time.sleep(wait // 2)   # the page's autofocused Play button: a key press is the user gesture the engine's autoplay policy (deny here: no shared sound device in QEMU) needs
    shot("av")
    print("engine log (tail):", ssh("tail -30 /var/lib/iyakki/log/engine.log 2>&1; echo ---; tail -5 /run/iyakki-web.log 2>/dev/null"))
    print("engine processes:", ssh("ps | grep -i -E 'WPE|webview' | grep -v grep | head -5"))
    print("engine env (GST/LD):", ssh("for p in $(pidof iyakki-webview-helper WPEWebProcess 2>/dev/null); do tr '\\0' '\\n' < /proc/$p/environ | grep -E '^(GST_PLUGIN_PATH_1_0|LD_LIBRARY_PATH|GST_REGISTRY_1_0|GST_PLUGIN_FEATURE_RANK)=' | sed \"s/^/$p /\"; done | head -12"))
    if addon:
        A = "/var/lib/iyakki/addons/ffmpeg/lib"
        print("gst-inspect avdec_h264 (add-on env):", ssh(f"GST_PLUGIN_PATH_1_0=/usr/lib/iyakki-av/gstreamer-1.0 LD_LIBRARY_PATH={A} GST_REGISTRY_1_0=/tmp/r2 gst-inspect-1.0 avdec_h264 2>&1 | head -4"))
        print("gst-inspect avdec_aac (add-on env):", ssh(f"GST_PLUGIN_PATH_1_0=/usr/lib/iyakki-av/gstreamer-1.0 LD_LIBRARY_PATH={A} GST_REGISTRY_1_0=/tmp/r2 gst-inspect-1.0 avdec_aac 2>&1 | head -2"))
        upload(f"{here}/qemu-web-test/media/av-test.mp4", "/tmp/av-test.mp4")
        env = f"GST_PLUGIN_PATH_1_0=/usr/lib/iyakki-av/gstreamer-1.0 LD_LIBRARY_PATH={A} GST_REGISTRY_1_0=/tmp/r2"
        print("gst-launch video:", ssh(f"{env} gst-launch-1.0 -v -e filesrc location=/tmp/av-test.mp4 ! qtdemux ! h264parse ! avdec_h264 ! identity silent=false ! fakesink sync=false > /tmp/gl1.log 2>&1; echo frames decoded by avdec_h264: $(grep -c 'chain ' /tmp/gl1.log); tail -3 /tmp/gl1.log", 90))
        print("gst-launch audio:", ssh(f"{env} gst-launch-1.0 -v -e filesrc location=/tmp/av-test.mp4 ! qtdemux ! aacparse ! avdec_aac ! identity silent=false ! fakesink sync=false > /tmp/gl2.log 2>&1; echo buffers decoded by avdec_aac: $(grep -c 'chain ' /tmp/gl2.log); tail -3 /tmp/gl2.log", 90))
    if os.environ.get("HOLD"):   # keep the guest for manual work over SSH (port 2223) until the file named by HOLD exists
        print("HOLDING: ssh -p 2223 root@127.0.0.1; release by creating", os.environ["HOLD"], flush=True)
        while not os.path.exists(os.environ["HOLD"]): time.sleep(2)
finally:
    try: mon("quit")
    except Exception: pass
    q.terminate(); server.terminate(); shutil.rmtree(tmp, ignore_errors=True)
rep = [l for l in open(f"{out}/server.log").read().splitlines() if l.startswith("REPORT") and '"test":"av"' in l] if os.path.exists(f"{out}/server.log") else []
print("page reports:", len(rep)); print("last report:", rep[-1] if rep else "none")
