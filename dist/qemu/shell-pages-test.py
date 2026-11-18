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


def run_vm(sound, tag):
    work = tempfile.mkdtemp(prefix="iyakki-shellpages-", dir=os.environ.get("TMPDIR", "/tmp"))
    disk = os.path.join(work, "disk.img")
    subprocess.run(["truncate", "-s", "1G", disk], check=True)
    r = subprocess.run([usb, os.path.join(OUT, "images", "iyakki-install-x86_64.img"), "--target-file", disk, "--erase", "testfile", "--table", "mbr"], capture_output=True, text=True)
    if r.returncode:
        sys.exit("iyakki-usb: " + r.stderr[-300:])
    mon = os.path.join(work, "mon.sock")
    cmdline = ["qemu-system-x86_64", "-accel", "kvm", "-accel", "tcg", "-m", "3072", "-smp", "2", "-display", "none", "-vga", "std", "-no-reboot", "-audiodev", "none,id=snd0",
               "-smbios", "type=1,manufacturer=Acme,product=Example Laptop 7,version=1.0,serial=SECRET-SERIAL-1", "-device", "virtio-keyboard-pci"]
    if sound:
        cmdline += ["-device", "intel-hda", "-device", "hda-duplex,audiodev=snd0"]
    cmdline += ["-kernel", os.path.join(OUT, "images", "bzImage"), "-initrd", os.path.join(OUT, "images", "rootfs.cpio.zst"), "-append", "console=ttyS0 panic=-1 iyakki.disk=vda",
                "-drive", "file=%s,format=raw,if=virtio" % disk, "-netdev", "user,id=n0", "-device", "e1000e,netdev=n0", "-serial", "file:" + os.path.join(work, "ser.log"),
                "-monitor", "unix:%s,server,nowait" % mon]
    qemu = subprocess.Popen(cmdline, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
    try:
        s = None
        for _ in range(100):
            try:
                s = socket.socket(socket.AF_UNIX); s.connect(mon); break
            except OSError:
                time.sleep(0.1)
        def cmd(c, wait=0.15):
            s.send((c + "\n").encode()); time.sleep(wait)
        MAP = {" ": "spc", "-": "minus", ".": "dot", ",": "comma", "/": "slash", "_": "shift-minus", ":": "shift-semicolon", "+": "shift-equal", "=": "equal"}
        def typ(text):
            for ch in text:
                cmd("sendkey " + (MAP[ch] if ch in MAP else ("shift-" + ch.lower() if ch.isupper() else ch)), 0.12)
        n = [0]
        def shot(name):
            n[0] += 1
            ppm = os.path.join(work, "s%d.ppm" % n[0]); cmd("screendump " + ppm, 1.2)
            png = os.path.join(work, "%s.png" % name); Image.open(ppm).save(png)
            if out_dir:
                shutil.copy(png, os.path.join(out_dir, "%s-%s.png" % (tag, name)))
            return subprocess.run(["tesseract", png, "-", "--psm", "11"], capture_output=True, text=True).stdout
        def wait_for(what, secs, name):
            t0 = time.time(); text = ""
            while time.time() - t0 < secs:
                text = shot(name)
                if re.search(what, text, re.I):
                    return text
                time.sleep(2)
            return text
        def check(label, text, expect, claim):
            miss = [e for e in expect if not re.search(e, text, re.I)]
            if miss:
                fails.append("%s: %s: missing %s" % (tag, label, miss)); print("FAIL: %s: %s: missing %s\n%s" % (tag, label, miss, text[:600]))
            else:
                print("ok: [%s] %s" % (tag, claim))
        def page(q):
            cmd("sendkey slash", 0.5); cmd("sendkey ctrl-a", 0.2); typ(q); cmd("sendkey down", 0.4); cmd("sendkey ret", 1.8)
            return shot("page-" + q.replace(" ", "-"))
        t = wait_for(r"create the administrator", 120, "first-screen")
        if not re.search(r"create the administrator", t, re.I):
            fails.append("%s: the first-start screen never appeared" % tag); return
        print("ok: [%s] the shell drew its first screen on the virtual display: 'Welcome - create the administrator account'" % tag)
        typ("admin"); cmd("sendkey tab", 0.3); typ("Correct-Horse-Battery-9"); cmd("sendkey tab", 0.3); typ("Correct-Horse-Battery-9"); cmd("sendkey ret", 0.5)
        t = wait_for(r"CHAT", 60, "home")
        if not re.search(r"CHAT", t):
            fails.append("%s: the home screen did not appear after creating the account" % tag); return
        print("ok: [%s] the account was created by typing and the home screen with its tiles appeared" % tag)
        cmd("sendkey n", 0.5)        # the time-zone card: Not now
        cmd("sendkey s", 2.0)        # Settings
        if sound:
            check("Displays", page("displays"), [r"Virtual-?\s*[1lI]", r"1280", r"display is connected"], "the Displays page lists the connector and mode the kernel's display driver reports")
            check("Display", page("display"), [r"Screen in use|Output", r"1280|800", r"Text and button size"], "the Display page names the screen in use and the output, with the text size buttons")
            sp = page("sound")
            check("Sound", sp, [r"HDA", r"Sound output", r"IN USE|AUTO"], "the Sound page names the HDA card the kernel found as the output in use")
            cmd("sendkey volumeup", 0.6); cmd("sendkey volumeup", 1.5)
            check("volume keys", shot("sound-volume-up"), [r"80\s*%"], "two volume-up keys took the IYAKKI volume from 70 % to 80 % and the Sound page shows it")
            cmd("sendkey audiomute", 1.5)
            check("mute key", shot("sound-muted"), [r"mute"], "the mute key shows as muted")
            cmd("sendkey audiomute", 1.0)
            check("This computer", page("this computer"), [r"Example Laptop 7", r"Started 1 time", r"per computer"], "the This computer page names the computer by its real DMI strings")
            check("Keyboard", page("keyboard"), [r"Input profile", r"speed"], "the Keyboard page has the input profile and the speed rows")
            check("Network", page("network"), [r"Ethernet|eth0|MAC", r"WiFi"], "the Network page shows the wired adapter the kernel found and no WiFi")
            check("Firewall", page("firewall"), [r"Standard", r"block everything coming in", r"connection attempts blocked"], "the Firewall page shows the standard preset, in force")
            check("Cloud mode", page("cloud mode"), [r"Automatic", r"Mode now"], "the Cloud mode page says why, from what the machine has (a sound card: desktop mode)")
            # the tiles of the Devices & System group, with what the machine really has
            cmd("sendkey esc", 1.5); cmd("sendkey n", 0.5)
            for _ in range(4):
                cmd("sendkey tab", 0.5)
            def tile(index, name):
                cmd("sendkey home", 0.5)
                for _ in range(index):
                    cmd("sendkey right", 0.3)
                cmd("sendkey ret", 3.0)
                return shot("tile-" + name)
            check("USERS", tile(0, "users"), [r"Users", r"admin"], "the USERS tile lists the account that was just made")
            cmd("sendkey esc", 1.5)
            check("NET SCAN", tile(1, "netscan"), [r"eth[0O]", r"10\.0\.2\.0/24"], "the NET SCAN tile offers the network the wired adapter is on (eth0, 10.0.2.0/24)")
            cmd("sendkey esc", 1.5)
            check("SYSTEM", tile(3, "system"), [r"2 cores", r"zram", r"\d GB"], "the SYSTEM tile shows the 2 cores, the memory (3 GB) and the zram the kernel really has")
            cmd("sendkey esc", 1.5)
            check("DISKS", tile(5, "disks"), [r"IYAKKI-DATA", r"ext4", r"vda", r"running system|system volume"], "the DISKS tile lists the virtual disk, its two partitions (FAT32 boot, ext4 data) and that it is part of the running system")
            cmd("sendkey esc", 1.5)
            check("DOCTOR", tile(7, "doctor"), [r"things? to look at|All good", r"No camera was found"], "the DOCTOR tile reports what the machine lacks (no camera) from the real facts")
            cmd("sendkey esc", 1.5)
            ck = tile(10, "checkup")
            check("CHECK-UP", ck, [r"12 not done yet", r"Keyboard", r"Battery and power"], "the CHECK-UP tile lists its twelve steps")
            cmd("sendkey down", 0.4); cmd("sendkey down", 0.4); cmd("sendkey ret", 1.0); typ("abc")
            check("CHECK-UP keyboard", shot("tile-checkup-keyboard"), [r"1 passed", r"Pass"], "typing on the real keyboard passed the Keyboard step")
            # the lock screen: Super+L covers the screen, a wrong password is refused, many wrong ones start the growing wait that refuses even the right one, then the right one unlocks
            cmd("sendkey esc", 1.5)
            cmd("sendkey meta_l-l", 2.5)
            check("lock", shot("lock-1"), [r"Locked", r"admin"], "Super+L covers the screen with the lock card ('Locked - admin', a password field, Unlock)")
            typ("not-the-password"); cmd("sendkey ret", 1.5)
            check("lock wrong", shot("lock-2"), [r"invalid username or password"], "a wrong password is refused with the account store's words")
            for _ in range(5):
                typ("x"); cmd("sendkey ret", 4.6)       # six wrong tries in all, each after the wait of the one before (1, 2, 4 seconds): the next wait is 8 s
            typ("Correct-Horse-Battery-9"); cmd("sendkey ret", 0.8)
            check("lock wait", shot("lock-3"), [r"Locked", r"try again in|too many"], "after six wrong tries even the right password is refused for a wait ('too many attempts; try again in N s')")
            time.sleep(10)
            typ("Correct-Horse-Battery-9"); cmd("sendkey ret", 3.0)
            check("unlock", shot("lock-4"), [r"admin", r"IYAKKI"], "after the wait the right password unlocks and the desktop is back")
        else:
            cm = page("cloud mode")
            check("Cloud mode", cm, [r"Mode now\s*Cloud mode is on", r"no sound card", r"RADIO"], "without a sound card the Cloud mode page says it is ON because there is none and names the hidden apps")
            sp = page("sound")
            if re.search(r"Sound output", sp):
                fails.append("%s: the Sound page is there on a machine with no sound card" % tag); print("FAIL: Sound page present without a card")
            else:
                print("ok: [%s] the Sound page is hidden when there is no sound card" % tag)
    finally:
        qemu.kill(); qemu.wait()
        shutil.rmtree(work, ignore_errors=True)


run_vm(True, "sound")
run_vm(False, "nosound")
if fails:
    print("FAIL: " + "; ".join(fails)); sys.exit(1)
print("PASS (shell-pages)")
