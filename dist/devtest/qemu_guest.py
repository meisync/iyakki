#!/usr/bin/env python3
"""Boot the lean image in QEMU like a machine, make a SCRATCH account by typing (the same throw-away guest as
dist/qemu/dev-pair-test.py; the password is the scratch one from that script and the disk is deleted at the end),
and pair this computer with the guest so the scenarios can run through developer mode.

The boot recipe and the pairing steps are not copied: they are the `Vm` class of dist/qemu/dev-pair-test.py (boot, keys, screenshots,
`make_account`, `read_pair_code`, `pair_from_host`), loaded from that file, so the two cannot drift. Needs: the build folder with images/bzImage,
rootfs.cpio.zst, iyakki-install-x86_64.img; iyakki-usb; qemu-system-x86_64; Pillow. Pairing reads the one-time code
off the guest's screen with tesseract; without tesseract only the smoke shots (first screen, Home) are taken through
the QEMU monitor and `paired` stays False. Real devices are never touched by this module.
"""
import importlib.util
import os
import socket
import tempfile
import time

import devui

SCRATCH_PASSWORD = "Correct-Horse-Battery-9"  # the throw-away guest's own account, the same string dist/qemu/dev-pair-test.py uses


def load_pair_script():
    """dist/qemu/dev-pair-test.py as a module (its file name has a hyphen, so it is loaded by path). Nothing runs at import."""
    path = os.path.join(os.path.dirname(os.path.abspath(__file__)), "..", "qemu", "dev-pair-test.py")
    spec = importlib.util.spec_from_file_location("dev_pair_test", path)
    m = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(m)
    m.Image = devui.Image
    return m


class Guest:
    def __init__(self, build_dir, client, port=None):
        self.out, self.client = build_dir, client
        self.work = tempfile.mkdtemp(prefix="iyakki-devtest-", dir=os.environ.get("TMPDIR", "/tmp"))
        self.home = os.path.join(self.work, "home")
        os.makedirs(self.home)
        self.device_name = "qemu-guest"
        self.paired = False
        self.smoke = []  # (label, png path)
        self.vm = None
        self.port = port or self._free_port()
        self.pair_script = None

    @staticmethod
    def _free_port():
        s = socket.socket()
        s.bind(("127.0.0.1", 0))
        p = s.getsockname()[1]
        s.close()
        return p

    @property
    def qemu(self):
        return self.vm.qemu if self.vm else None

    def _start(self):
        for f in ("bzImage", "rootfs.cpio.zst", "iyakki-install-x86_64.img"):
            if not os.path.isfile(os.path.join(self.out, "images", f)):
                raise devui.DevError("no %s in %s/images" % (f, self.out))
        usb = os.environ.get("IYAKKI_USB") or os.path.join(os.path.dirname(self.client), "iyakki-usb")
        if not os.access(usb, os.X_OK):
            raise devui.DevError("needs iyakki-usb (%s; set IYAKKI_USB)" % usb)
        self.pair_script = load_pair_script()
        self.vm = self.pair_script.Vm(self.out, usb, sound=True, tag="qemu-guest", port=self.port, tmpdir=self.work)
        try:
            self.vm.boot()
        except SystemExit as e:  # the shared recipe says "sys.exit(sentence)" for a failed disk write
            raise devui.DevError(str(e))
        if self.vm.mon is None:
            raise devui.DevError("QEMU monitor did not come up")

    def _cmd(self, c, wait=0.15):
        self.vm.cmd(c, wait)

    def shot(self, label):
        return self.vm.snap(label)

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
        # the first-start screen: create the administrator (scratch account of this throw-away guest). 2.5 s after each Tab: at 0.3 s (and 1 s) the shell
        # of alpha.10 (device build, boot services still starting) handled the typed letters of a frame before the Tab and the second password landed in the first field (Q1, 2026-11-21).
        self.vm.typ("admin"); self._cmd("sendkey tab", 2.5); self.vm.typ(SCRATCH_PASSWORD); self._cmd("sendkey tab", 2.5); self.vm.typ(SCRATCH_PASSWORD); self._cmd("sendkey ret", 0.5)
        home = self._wait_screen_change(first, 90, "home")
        time.sleep(8)
        self._cmd("sendkey n", 0.5)
        self._cmd("sendkey n", 0.5)
        home = self.shot("home")
        self.smoke.append(("home", home))
        if not devui.HAVE_TESSERACT:
            return  # cannot read the pairing code off the screen
        self._pair_by_screen()

    def _pair_by_screen(self):
        """The shared steps: developer mode on, the code off the screen, the firewall allow; then pair from here and grant the diagnostic scopes."""
        code = self.vm.read_pair_code()
        if not code:
            return
        r = self.vm.pair_from_host(self.client, code, self.home, name=self.device_name)
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
            if self.vm:
                self.vm.close()
        finally:
            import shutil
            shutil.rmtree(self.work, ignore_errors=True) if not os.environ.get("IYAKKI_DEVTEST_KEEP") else None
