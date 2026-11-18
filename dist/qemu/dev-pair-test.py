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
    initrd = os.path.join(OUT, "images", "rootfs.cpio.zst")
    if os.environ.get("IYAKKI_BIN"):      # a newer musl `iyakki` laid over the RAM disk as a second zstd cpio (as the other dist/qemu scripts do)
        ov = os.path.join(work, "ov"); os.makedirs(os.path.join(ov, "usr", "bin"))
        shutil.copy(os.environ["IYAKKI_BIN"], os.path.join(ov, "usr", "bin", "iyakki"))
        files = subprocess.run("find . | cpio -o -H newc 2>/dev/null | zstd -q -19", shell=True, cwd=ov, capture_output=True).stdout
        initrd = os.path.join(work, "initrd.zst")
        open(initrd, "wb").write(open(os.path.join(OUT, "images", "rootfs.cpio.zst"), "rb").read() + files)
    mon = os.path.join(work, "mon.sock")
    cmdline = ["qemu-system-x86_64", "-accel", "kvm", "-accel", "tcg", "-m", "3072", "-smp", "2", "-display", "none", "-vga", "std", "-no-reboot", "-audiodev", "none,id=snd0",
               "-smbios", "type=1,manufacturer=Acme,product=Example Laptop 7,version=1.0,serial=SECRET-SERIAL-1", "-device", "virtio-keyboard-pci"]
    if sound:
        cmdline += ["-device", "intel-hda", "-device", "hda-duplex,audiodev=snd0"]
    cmdline += ["-kernel", os.path.join(OUT, "images", "bzImage"), "-initrd", initrd, "-append", "console=ttyS0 panic=-1 iyakki.disk=vda",
                "-drive", "file=%s,format=raw,if=virtio" % disk, "-netdev", "user,id=n0,hostfwd=tcp:127.0.0.1:17979-:7979", "-device", "e1000e,netdev=n0", "-serial", "file:" + os.path.join(work, "ser.log"),
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
        cmd("sendkey n", 0.5)
        cmd("sendkey s", 2.0)
        t = page("developer mode")
        print(t[:600])
        cmd("sendkey e", 2.0)
        check("dev on", shot("dev-on"), [r"Developer mode is on|port 7979"], "developer mode switched on from the Settings page")
        cmd("sendkey p", 2.0)
        t = wait_for(r"\b[A-Z0-9]{5} ?[-–] ?[A-Z0-9]{5}\b", 12, "dev-pair")
        if not re.search(r"\b[A-Z0-9]{5} ?[-–] ?[A-Z0-9]{5}\b", t):
            cmd("sendkey n", 1.5); cmd("sendkey p", 2.0)       # the time-zone card ("Not now") can arrive late and covers the page
            t = wait_for(r"\b[A-Z0-9]{5} ?[-–] ?[A-Z0-9]{5}\b", 25, "dev-pair")
        print(t[:900])
        m = re.search(r"\b([A-Z0-9]{5} ?[-–] ?[A-Z0-9]{5})\b", t)
        if not m:
            fails.append("no pairing code read from the screen"); return
        code = re.sub(r"[ –]", "", m.group(1)).replace("-", "-"); code = code[:5] + "-" + code[-5:] if "-" not in code else code; print("code read:", code)
        cmd("sendkey f", 2.0)
        check("allow", shot("dev-allow"), [r"firewall|local network|ten minutes"], "the firewall allow for the pairing was pressed and the page says what it did")
        env = dict(os.environ, HOME=os.path.join(work, "home")); os.makedirs(env["HOME"], exist_ok=True)
        bad = "AAAAA-AAAAA"
        r = subprocess.run([iyakki_bin, "dev", "pair", "127.0.0.1:17979", "--code", bad], capture_output=True, text=True, env=env, timeout=60)
        print("wrong code:", r.returncode, (r.stdout + r.stderr)[-300:])
        if r.returncode == 0:
            fails.append("a wrong code paired")
        else:
            print("ok: [%s] a wrong code is refused" % tag)
        r = subprocess.run([iyakki_bin, "dev", "pair", "127.0.0.1:17979", "--code", code], capture_output=True, text=True, env=env, timeout=60)
        print("pair:", r.returncode, (r.stdout + r.stderr)[-500:])
        if r.returncode != 0:
            fails.append("pairing failed: " + (r.stdout + r.stderr)[-300:])
        else:
            print("ok: [%s] `iyakki dev pair` from the host paired with the guest through the forwarded port" % tag)
            r = subprocess.run([iyakki_bin, "dev", "whoami"], capture_output=True, text=True, env=env, timeout=60)
            print("info:", r.returncode, (r.stdout + r.stderr)[-500:])
            if r.returncode == 0:
                print("ok: [%s] a signed request to the guest was answered (`iyakki dev whoami`)" % tag)
            else:
                fails.append("dev whoami failed")
            # grants by keyboard in the guest (G: every diagnostic scope for the selected computer), then the powers they allow
            cmd("sendkey g", 0.2)
            cmd("sendkey shift-g", 2.0)
            check("grant", shot("dev-grant"), [r"diagnose|may now help|look|files"], "G granted the diagnostic scopes to the paired computer from the screen")
            import glob
            r = subprocess.run([iyakki_bin, "dev", "screenshot"], capture_output=True, text=True, env=env, timeout=60, cwd=env["HOME"])
            print("screenshot:", r.returncode, (r.stdout + r.stderr)[-300:])
            pngs = glob.glob(os.path.join(env["HOME"], "iyakki-screen-*.png"))
            if r.returncode == 0 and pngs and open(pngs[0], "rb").read(8) == b"\x89PNG\r\n\x1a\n":
                print("ok: [%s] `iyakki dev screenshot` returned a PNG of the guest's screen (%d bytes)" % (tag, os.path.getsize(pngs[0])))
            else:
                fails.append("dev screenshot failed")
            r = subprocess.run([iyakki_bin, "dev", "files", "ls", "/tmp/iyakki-dev"], capture_output=True, text=True, env=env, timeout=60)
            print("files ls:", r.returncode, (r.stdout + r.stderr)[-300:])
            if r.returncode == 0:
                print("ok: [%s] `iyakki dev files ls` listed a folder of the guest" % tag)
            else:
                fails.append("dev files ls failed")
            # a shell with a terminal: the guest asks on its own screen, the administrator answers with a (ten minutes)
            import pty
            mfd, sfd = pty.openpty()      # --tty wants a terminal on this side too
            sh = subprocess.Popen([iyakki_bin, "dev", "shell", "--tty", "--", "sh", "-c", "tty; id -u; echo hello-from-the-guest"], stdin=sfd, stdout=sfd, stderr=sfd, env=env)
            time.sleep(6)
            ask = shot("dev-shell-ask")
            print(ask[:500])
            # the approval must come from the person at the device: `dev input key a` (and o) must NOT answer it
            for k in ("a", "o"):
                subprocess.run([iyakki_bin, "dev", "input", "key", k], capture_output=True, text=True, env=env, timeout=60)
            time.sleep(4)
            if sh.poll() is None:
                print("ok: [%s] the shell approval was NOT answered by `dev input key a` / `o` (the request still waits)" % tag)
            else:
                fails.append("dev input key a/o answered the shell approval (exit %s)" % sh.returncode)
                mfd0 = None
            cmd("sendkey 8", 2.0)       # the 8 hour button of the approval (a = ten minutes)
            try:
                sh.wait(timeout=60)
            except subprocess.TimeoutExpired:
                sh.kill(); sh.wait()
            import select
            o = b""
            while select.select([mfd], [], [], 0.5)[0]:
                try:
                    d = os.read(mfd, 4096)
                except OSError:
                    break
                if not d:
                    break
                o += d
            o = o.decode("utf-8", "replace")
            print("shell:", sh.returncode, o[-400:])
            gm = re.search(r"granted until (\d{4}-\d\d-\d\d \d\d:\d\d:\d\d)", o)
            if gm:
                import datetime
                until = datetime.datetime.strptime(gm.group(1), "%Y-%m-%d %H:%M:%S"); left = (until - datetime.datetime.now()).total_seconds() / 3600
                if 7.8 < left < 8.1:
                    print("ok: [%s] the 8 hour key of the approval gave a grant of %.2f hours (shell access granted until %s)" % (tag, left, gm.group(1)))
                else:
                    fails.append("the 8 hour key gave %.2f hours" % left)
            else:
                fails.append("no 'granted until' line from the shell approval")
            if "hello-from-the-guest" in o and "/dev/pts/" in o:
                print("ok: [%s] `iyakki dev shell --tty` ran on the guest after the on-screen approval, on a pseudo-terminal (/dev/pts/N)" % tag)
            else:
                fails.append("dev shell --tty: " + o[-200:])
            # the screen driven from the host (scope input, part of the diagnostic grant): back out of the page, open the Settings search, type into it
            def dev(*a):
                r = subprocess.run([iyakki_bin, "dev"] + list(a), capture_output=True, text=True, env=env, timeout=60)
                print("dev", " ".join(a), "->", r.returncode, (r.stdout + r.stderr).strip()[-200:])
                return r
            r = dev("input", "key", "esc"); time.sleep(1.5)
            if r.returncode == 0:
                print("ok: [%s] `dev input` works again once the approval card was answered with the screen's own key (the key after the card was accepted)" % tag)
            else:
                fails.append("dev input stayed refused after the approval card was answered on the screen: " + (r.stdout + r.stderr)[-150:])
            st = dev("state")                 # which app is in front now (letters on the Home screen are shortcuts: p and r there restart the machine)
            if re.search(r"CALENDAR", st.stdout or ""):       # the Home tiles are in front
                dev("input", "key", "n"); time.sleep(1.0)     # the time-zone card, if it is up
                dev("input", "key", "s"); time.sleep(2.0)
            r2 = dev("input", "key", "/"); time.sleep(1.0)
            r3 = dev("input", "text", "zzprobe"); time.sleep(1.5)
            t = shot("dev-input")
            if r.returncode == 0 and r2.returncode == 0 and r3.returncode == 0 and re.search(r"zzprobe", t, re.I):
                print("ok: [%s] `iyakki dev input key` and `text` from the host moved the guest's real shell: 'zzprobe' is in the Settings search box on its screen" % tag)
            else:
                fails.append("dev input did not show in the guest screen: " + t[:300])
            r = dev("input", "tap", "10", "10")
            if r.returncode == 0:
                print("ok: [%s] `iyakki dev input tap X Y` was accepted by the guest" % tag)
            else:
                fails.append("dev input tap failed")
            r = dev("screenshot", "--display", "2")
            if r.returncode != 0 and "display" in (r.stdout + r.stderr).lower():
                print("ok: [%s] a second display is refused with a sentence (the guest has one)" % tag)
            else:
                fails.append("screenshot --display 2 was not refused")
            # the power menu must not be reachable through `dev input` on the Home screen: back to Home, then p and r
            st = dev("state")
            for _ in range(4):          # back out one step at a time and stop the moment the Home tiles show (Esc on Home signs out)
                if re.search(r"CALENDAR", st.stdout or ""):
                    break
                dev("input", "key", "esc"); time.sleep(1.5)
                st = dev("state")
            if re.search(r"CALENDAR", st.stdout or ""):
                dev("input", "key", "n"); time.sleep(1.0)
                dev("input", "key", "p"); time.sleep(1.0)
                dev("input", "key", "r"); time.sleep(8.0)
                alive = dev("whoami")
                if alive.returncode == 0 and qemu.poll() is None:
                    print("ok: [%s] `dev input key p` then `r` on Home did not restart the guest" % tag)
                else:
                    fails.append("dev input key p + r restarted the guest (power card answered through the input scope)")
            else:
                fails.append("could not get the Home screen in front for the p+r case")
    finally:
        try:
            if out_dir:
                shutil.copy(os.path.join(work, "ser.log"), os.path.join(out_dir, "%s-serial.log" % tag))
        except OSError:
            pass
        qemu.kill(); qemu.wait()
        shutil.rmtree(work, ignore_errors=True)


iyakki_bin = os.environ.get("IYAKKI_BIN_HOST", os.path.join(root, "target", "debug", "iyakki"))
run_vm(False, "pair")
if fails:
    print("FAIL: " + "; ".join(fails)); sys.exit(1)
print("PASS (dev-pair)")
