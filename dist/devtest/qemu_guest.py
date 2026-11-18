#!/usr/bin/env python3
"""Boot the lean image in QEMU like a machine, make a SCRATCH account by typing (the same throw-away guest as
dist/qemu/dev-pair-test.py; the password is the scratch one from that script and the disk is deleted at the end),
and pair this computer with the guest so the scenarios can run through developer mode.

The boot recipe mirrors dist/qemu/dev-pair-test.py `run_vm` (that script is one monolithic file with no importable
function; keep the two in step, see docs/development/DEVICE-UI-TESTS.md). Needs: the build folder with images/bzImage,
rootfs.cpio.zst, iyakki-install-x86_64.img; iyakki-usb; qemu-system-x86_64; Pillow. Pairing reads the one-time code
off the guest's screen with tesseract; without tesseract only the smoke shots (first screen, Home) are taken through
the QEMU monitor and `paired` stays False. Real devices are never touched by this module.
"""
import glob
import os
import re
import shutil
import socket
import subprocess
import tempfile
import time

import devui

MAP = {" ": "spc", "-": "minus", ".": "dot", ",": "comma", "/": "slash", "_": "shift-minus", ":": "shift-semicolon", "+": "shift-equal", "=": "equal"}
SCRATCH_PASSWORD = "Correct-Horse-Battery-9"  # the throw-away guest's own account, the same string dist/qemu/dev-pair-test.py uses


class Guest:
    def __init__(self, build_dir, client, port=None):
        self.out, self.client = build_dir, client
        self.work = tempfile.mkdtemp(prefix="iyakki-devtest-", dir=os.environ.get("TMPDIR", "/tmp"))
        self.home = os.path.join(self.work, "home")
        os.makedirs(self.home)
        self.device_name = "qemu-guest"
        self.paired = False
        self.smoke = []  # (label, png path)
        self.qemu = None
        self.mon = None
        self.port = port or self._free_port()
        self.n = 0

    @staticmethod
    def _free_port():
        s = socket.socket()
        s.bind(("127.0.0.1", 0))
        p = s.getsockname()[1]
        s.close()
        return p

    # -- the QEMU recipe (as dev-pair-test.py)
    def _start(self):
        for f in ("bzImage", "rootfs.cpio.zst", "iyakki-install-x86_64.img"):
            if not os.path.isfile(os.path.join(self.out, "images", f)):
                raise devui.DevError("no %s in %s/images" % (f, self.out))
        usb = os.environ.get("IYAKKI_USB") or os.path.join(os.path.dirname(self.client), "iyakki-usb")
        if not os.access(usb, os.X_OK):
            raise devui.DevError("needs iyakki-usb (%s; set IYAKKI_USB)" % usb)
        disk = os.path.join(self.work, "disk.img")
        subprocess.run(["truncate", "-s", "1G", disk], check=True)
        r = subprocess.run([usb, os.path.join(self.out, "images", "iyakki-install-x86_64.img"), "--target-file", disk, "--erase", "testfile", "--table", "mbr"], capture_output=True, text=True)
        if r.returncode:
            raise devui.DevError("iyakki-usb: " + r.stderr[-300:])
        mon = os.path.join(self.work, "mon.sock")
        cmd = ["qemu-system-x86_64", "-accel", "kvm", "-accel", "tcg", "-m", "3072", "-smp", "2", "-display", "none", "-vga", "std", "-no-reboot", "-audiodev", "none,id=snd0",
               "-smbios", "type=1,manufacturer=Acme,product=Example Laptop 7,version=1.0,serial=SECRET-SERIAL-1", "-device", "virtio-keyboard-pci",
               "-device", "intel-hda", "-device", "hda-duplex,audiodev=snd0",
               "-kernel", os.path.join(self.out, "images", "bzImage"), "-initrd", os.path.join(self.out, "images", "rootfs.cpio.zst"),
               "-append", "console=ttyS0 panic=-1 iyakki.disk=vda", "-drive", "file=%s,format=raw,if=virtio" % disk,
               "-netdev", "user,id=n0,hostfwd=tcp:127.0.0.1:%d-:7979" % self.port, "-device", "e1000e,netdev=n0",
               "-serial", "file:" + os.path.join(self.work, "ser.log"), "-monitor", "unix:%s,server,nowait" % mon]
        self.qemu = subprocess.Popen(cmd, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
        for _ in range(100):
            try:
                self.mon = socket.socket(socket.AF_UNIX)
                self.mon.connect(mon)
                return
            except OSError:
                time.sleep(0.1)
        raise devui.DevError("QEMU monitor did not come up")

    def _cmd(self, c, wait=0.15):
        self.mon.send((c + "\n").encode())
        time.sleep(wait)

    def _type(self, text):
        for ch in text:
            self._cmd("sendkey " + (MAP[ch] if ch in MAP else ("shift-" + ch.lower() if ch.isupper() else ch)), 0.12)

    def shot(self, label):
        self.n += 1
        ppm = os.path.join(self.work, "s%d.ppm" % self.n)
        self._cmd("screendump " + ppm, 1.2)
        png = os.path.join(self.work, "%02d-%s.png" % (self.n, label))
        devui.Image.open(ppm).save(png)
        os.remove(ppm)
        return png

    def _wait_screen_change(self, since, secs, label):
        """Without OCR: wait for a screen that is not blank and differs from `since` (a PNG path or None)."""
        t0 = time.time()
        png = None
        while time.time() - t0 < secs:
            png = self.shot(label)
            im = devui.Image.open(png)
            if not devui.is_blank(devui.blank_stats(devui.image_pixels(im))):
                if since is None:
                    return png
                d = devui.ImageChops.difference(im.convert("RGB"), devui.Image.open(since).convert("RGB"))
                if sum(devui.ImageStat.Stat(d).mean) / 3.0 > 3.0:
                    return png
            time.sleep(3)
        return png

    def boot_and_pair(self):
        if not devui.HAVE_PIL:
            raise devui.DevError("QEMU mode needs Pillow (python3-pil)")
        self._start()
        first = self._wait_screen_change(None, 180, "first-screen")
        time.sleep(4)
        first = self.shot("first-screen")
        self.smoke.append(("first-screen", first))
        # the first-start screen: create the administrator (scratch account of this throw-away guest)
        self._type("admin"); self._cmd("sendkey tab", 0.3); self._type(SCRATCH_PASSWORD); self._cmd("sendkey tab", 0.3); self._type(SCRATCH_PASSWORD); self._cmd("sendkey ret", 0.5)
        home = self._wait_screen_change(first, 90, "home")
        time.sleep(8)
        self._cmd("sendkey n", 0.5)
        self._cmd("sendkey n", 0.5)
        home = self.shot("home")
        self.smoke.append(("home", home))
        if not devui.HAVE_TESSERACT:
            return  # cannot read the pairing code off the screen
        self._pair_by_screen()

    def _ocr(self, png):
        return subprocess.run(["tesseract", png, "-", "--psm", "11"], capture_output=True, text=True).stdout

    def _pair_by_screen(self):
        self._cmd("sendkey s", 2.0)
        self._cmd("sendkey slash", 0.5); self._cmd("sendkey ctrl-a", 0.2); self._type("developer mode"); self._cmd("sendkey down", 0.4); self._cmd("sendkey ret", 1.8)
        self._cmd("sendkey e", 2.0)
        self._cmd("sendkey p", 2.0)
        code = None
        for _ in range(10):
            m = re.search(r"\b([A-Z0-9]{5}) ?[-–] ?([A-Z0-9]{5})\b", self._ocr(self.shot("pair-code")))
            if m:
                code = m.group(1) + "-" + m.group(2)
                break
            self._cmd("sendkey n", 1.0); self._cmd("sendkey p", 2.0)
        if not code:
            return
        self._cmd("sendkey f", 2.0)
        r = subprocess.run([self.client, "dev", "pair", "127.0.0.1:%d" % self.port, "--code", code, "--as", self.device_name, "--home", self.home], capture_output=True, text=True, timeout=90)
        if r.returncode:
            raise devui.DevError("pairing failed: " + (r.stdout + r.stderr)[-300:])
        self._cmd("sendkey g", 0.2)
        self._cmd("sendkey shift-g", 2.0)  # the diagnostic scopes (state, screenshot, input) for the selected computer
        self.paired = True
        # back to Home: the scenarios start there
        for _ in range(3):
            self._cmd("sendkey f11", 1.0)

    def stop(self):
        try:
            if self.qemu:
                self.qemu.kill()
                self.qemu.wait()
        finally:
            shutil.rmtree(self.work, ignore_errors=True) if not os.environ.get("IYAKKI_DEVTEST_KEEP") else None
