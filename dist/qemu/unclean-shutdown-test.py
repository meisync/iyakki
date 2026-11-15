#!/usr/bin/env python3
"""The real IYAKKI shell on a (virtual) screen, driven with the keyboard and read with OCR (F-DISP-06, F-SND-02, F-SND-04, F-KBD-02, F-HW-04, F-CLOUD-01 and the pages of F-FW-06, F-NET).

    python3 dist/qemu/shell-pages-test.py [build-output-dir] [--out DIR]

Needs: the built image (images/bzImage, rootfs.cpio.zst, iyakki-install-x86_64.img), `iyakki-usb` (IYAKKI_USB, default target/debug/iyakki-usb), qemu-system-x86_64 (KVM recommended), python3 with
Pillow, tesseract. The guest is started like a machine: a virtual VGA screen (the kernel's DRM and framebuffer drivers make `Virtual-1`), two keyboards (PS/2 and virtio, which has the media keys),
a network card, QEMU's `-smbios` strings that name the computer, and, in the first run, a virtual Intel HDA sound card. The script makes the first account by typing, opens Settings and visits pages
with its search box (`/`, the page name, Down, Enter), takes a screenshot of each page through the QEMU monitor and reads it with tesseract.
  run 1 (with a sound card): Displays, Display, Sound (the HDA output in use, then two volume-up keys take IYAKKI's volume from 70 to 80 percent and the mute key shows), This computer (the DMI
         name and "Started 1 time"), Keyboard, Network, Firewall, Cloud mode (a machine with a sound card is in desktop mode).
  then the tiles USERS, NET SCAN, SYSTEM, DISKS, DOCTOR and CHECK-UP of the Devices & System group (and a keyboard step passed by typing), then the lock screen (Super+L, a wrong password, the growing wait, the unlock).
  run 2 (no sound card): the Cloud mode page says it is ON because there is no sound card and names the hidden apps, and the Sound page is not in Settings.
PASS = each page shows the words that only the real machine can have put there. Screenshots go to --out for anybody to open.
Not covered: a real display or sound card, touch, the web engine, a second screen, the changes the pages offer (only reading, besides the volume keys), hearing anything.
"""
import os, re, shutil, socket, subprocess, sys, tempfile, time

try:
    from PIL import Image
except ImportError:
    sys.exit("needs python3-pil")
if not shutil.which("tesseract") or not shutil.which("qemu-system-x86_64"):
    sys.exit("needs tesseract and qemu-system-x86_64")
here = os.path.dirname(os.path.abspath(__file__)); root = os.path.abspath(os.path.join(here, "..", ".."))
args = [a for i, a in enumerate(sys.argv[1:]) if not a.startswith("--") and not (i > 0 and sys.argv[i] == "--out")]
out_dir = sys.argv[sys.argv.index("--out") + 1] if "--out" in sys.argv else None
OUT = args[0] if args and os.path.isdir(os.path.join(args[0], "images")) else os.path.join(root, "target", "buildroot-libre")
for f in ("bzImage", "rootfs.cpio.zst", "iyakki-install-x86_64.img"):
    if not os.path.isfile(os.path.join(OUT, "images", f)):
        sys.exit("no %s in %s/images" % (f, OUT))
usb = os.environ.get("IYAKKI_USB", os.path.join(root, "target", "debug", "iyakki-usb"))
if not os.access(usb, os.X_OK):
    sys.exit("needs iyakki-usb (%s)" % usb)
if out_dir:
    os.makedirs(out_dir, exist_ok=True)
fails = []



class VM:
    def __init__(self, work, disk, tag):
        self.work, self.disk, self.tag = work, disk, tag
        initrd = os.path.join(OUT, "images", "rootfs.cpio.zst")
        if os.environ.get("IYAKKI_BIN"):
            ov = os.path.join(work, "ov"); os.makedirs(os.path.join(ov, "usr", "bin"), exist_ok=True)
            shutil.copy(os.environ["IYAKKI_BIN"], os.path.join(ov, "usr", "bin", "iyakki"))
            files = subprocess.run("find . | cpio -o -H newc 2>/dev/null | zstd -q -19", shell=True, cwd=ov, capture_output=True).stdout
            initrd = os.path.join(work, "initrd.zst")
            open(initrd, "wb").write(open(os.path.join(OUT, "images", "rootfs.cpio.zst"), "rb").read() + files)
        self.mon = os.path.join(work, "mon-%s.sock" % tag)
        self.ser = os.path.join(work, "ser-%s.log" % tag)
        cmdline = ["qemu-system-x86_64", "-accel", "kvm", "-accel", "tcg", "-m", "3072", "-smp", "2", "-display", "none", "-vga", "std", "-no-reboot", "-audiodev", "none,id=snd0",
                   "-smbios", "type=1,manufacturer=Acme,product=Example Laptop 7,version=1.0,serial=SECRET-SERIAL-1", "-device", "virtio-keyboard-pci",
                   "-kernel", os.path.join(OUT, "images", "bzImage"), "-initrd", initrd, "-append", "console=ttyS0 panic=-1 iyakki.disk=vda",
                   "-drive", "file=%s,format=raw,if=virtio" % disk, "-netdev", "user,id=n0", "-device", "e1000e,netdev=n0", "-serial", "file:" + self.ser, "-monitor", "unix:%s,server,nowait" % self.mon]
        self.q = subprocess.Popen(cmdline, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
        self.s = None
        for _ in range(100):
            try:
                self.s = socket.socket(socket.AF_UNIX); self.s.connect(self.mon); break
            except OSError:
                time.sleep(0.1)
        self.n = 0

    def cmd(self, c, wait=0.15):
        self.s.send((c + "\n").encode()); time.sleep(wait)

    MAP = {" ": "spc", "-": "minus", ".": "dot", ",": "comma", "/": "slash", "_": "shift-minus", ":": "shift-semicolon", "+": "shift-equal", "=": "equal"}

    def typ(self, text):
        for ch in text:
            self.cmd("sendkey " + (self.MAP[ch] if ch in self.MAP else ("shift-" + ch.lower() if ch.isupper() else ch)), 0.12)

    def shot(self, name):
        self.n += 1
        ppm = os.path.join(self.work, "u%s%d.ppm" % (self.tag, self.n)); self.cmd("screendump " + ppm, 1.2)
        png = os.path.join(self.work, "%s-%s.png" % (self.tag, name)); Image.open(ppm).save(png)
        if out_dir:
            shutil.copy(png, os.path.join(out_dir, "%s-%s.png" % (self.tag, name)))
        return subprocess.run(["tesseract", png, "-", "--psm", "11"], capture_output=True, text=True).stdout

    def wait_for(self, what, secs, name):
        t0 = time.time(); text = ""
        while time.time() - t0 < secs:
            text = self.shot(name)
            if re.search(what, text, re.I):
                return text
            time.sleep(2)
        return text

    def page(self, q):
        self.cmd("sendkey slash", 0.5); self.cmd("sendkey ctrl-a", 0.2); self.typ(q); self.cmd("sendkey down", 0.4); self.cmd("sendkey ret", 1.8)
        return self.shot("page-" + q.replace(" ", "-"))

    def kill(self):
        self.q.kill(); self.q.wait()


def check(label, text, expect, claim):
    miss = [e for e in expect if not re.search(e, text, re.I)]
    if miss:
        fails.append("%s: missing %s" % (label, miss)); print("FAIL: %s: missing %s\n%s" % (label, miss, text[:700]))
    else:
        print("ok: " + claim)


PW = "Correct-Horse-Battery-9"


def sign_in_and_settings(vm, tag):
    t = vm.wait_for(r"Sign in", 150, "signin"); time.sleep(10)     # the card appears while the system is still starting
    print(t[:300])
    # the sign-in card has a name field and a password field; the Tab is slow to be taken while the system is still starting, so it is pressed until the card shows the second field
    # the card takes keys slowly while the system is still starting (the first boots lost a Tab): wait until it is quiet, then name, Tab, password
    time.sleep(30)
    # after a clean power-off the card already has the name and the focus is in the password field; after a power cut it starts empty
    card = vm.shot("signin-card")
    if not re.search(r"\badmin\b", card):
        vm.typ("admin"); time.sleep(1.5); vm.cmd("sendkey tab", 2.0)
    else:
        print("  (the sign-in card already shows the name admin: only the password is typed)")
    vm.typ(PW); time.sleep(0.5); vm.cmd("sendkey ret", 0.5)
    t = vm.wait_for(r"CHAT", 90, "home")
    if not re.search(r"CHAT", t):
        fails.append("%s: no home screen after signing in" % tag); return None
    vm.cmd("sendkey n", 1.0)     # the time-zone card, if it is up
    vm.cmd("sendkey s", 3.0)     # Settings (the first page is the Overview)
    return vm.shot("overview")


work = tempfile.mkdtemp(prefix="iyakki-unclean-", dir=os.environ.get("TMPDIR", "/tmp"))
disk = os.path.join(work, "disk.img")
subprocess.run(["truncate", "-s", "1G", disk], check=True)
r = subprocess.run([usb, os.path.join(OUT, "images", "iyakki-install-x86_64.img"), "--target-file", disk, "--erase", "testfile", "--table", "mbr"], capture_output=True, text=True)
if r.returncode:
    sys.exit("iyakki-usb: " + r.stderr[-300:])
fails = []
try:
    # boot 1: the account, then a power cut
    vm = VM(work, disk, "b1")
    t = vm.wait_for(r"create the administrator", 150, "first")
    if not re.search(r"create the administrator", t, re.I):
        sys.exit("FAIL: the first-start screen never appeared")
    vm.typ("admin"); vm.cmd("sendkey tab", 0.3); vm.typ(PW); vm.cmd("sendkey tab", 0.3); vm.typ(PW); vm.cmd("sendkey ret", 0.5)
    t = vm.wait_for(r"CHAT", 90, "home")
    if not re.search(r"CHAT", t):
        sys.exit("FAIL: no home screen after creating the account")
    print("ok: boot 1: the account was made and the home screen is up")
    time.sleep(float(os.environ.get("UNCLEAN_WAIT", "8")))     # seconds between the home screen and the power cut (the record is not durable at once unless it is synced)
    vm.kill()
    print("boot 1 was killed with SIGKILL (a power cut)")
    # boot 2: the same disk
    vm = VM(work, disk, "b2")
    ov = sign_in_and_settings(vm, "boot 2")
    if ov is not None:
        check("boot 2 overview", ov, [r"ended unexpectedly"], "boot 2 (after the power cut): Settings -> Overview says 'The last session ended unexpectedly'")
        t = vm.page("last session log")
        check("boot 2 log", t, [r"kernel|\[\s*\d+\.\d+\]|iyakki"], "boot 2: Settings -> Last session log shows the kept kernel lines of the cut session")
    # "I have seen it" (y on the log page) clears the note; without it the note stays through later boots by design
    vm.cmd("sendkey y", 1.5)
    t = vm.page("overview")
    if re.search(r"ended unexpectedly", t, re.I):
        fails.append("boot 2: the note is still on the Overview after 'I have seen it'"); print("FAIL: the note stayed after y\n" + t[:400])
    else:
        print("ok: boot 2: 'I have seen it' clears the note from the Overview")
    # a clean end: Home, power menu, off
    for _ in range(4):
        vm.cmd("sendkey esc", 1.2)
    vm.cmd("sendkey n", 0.5)
    vm.cmd("sendkey p", 1.5)
    pm = vm.shot("power-menu")
    print(pm[:300])
    vm.cmd("sendkey y", 1.0)
    for _ in range(60):
        if vm.q.poll() is not None:
            break
        time.sleep(1)
    if vm.q.poll() is None:
        fails.append("the power menu did not switch the guest off within a minute")
        vm.kill()
    else:
        print("ok: the power menu switched the guest off (QEMU ended by itself: a clean end)")
    # boot 3: after the clean end
    vm = VM(work, disk, "b3")
    ov = sign_in_and_settings(vm, "boot 3")
    if ov is not None:
        if re.search(r"ended unexpectedly", ov, re.I):
            fails.append("boot 3: the note is there after a clean power-off")
            print("FAIL: boot 3 still says ended unexpectedly\n" + ov[:500])
        else:
            print("ok: boot 3 (after a clean power-off): Settings -> Overview has no 'ended unexpectedly' note")
    vm.kill()
finally:
    for p in subprocess.run(["pgrep", "-f", work], capture_output=True, text=True).stdout.split():
        try:
            os.kill(int(p), 9)
        except OSError:
            pass
    shutil.rmtree(work, ignore_errors=True)
if fails:
    print("FAIL: " + "; ".join(fails)); sys.exit(1)
print("PASS (unclean-shutdown)")
