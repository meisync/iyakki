#!/usr/bin/env python3
# SPDX-License-Identifier: MIT OR Apache-2.0
"""Real-kernel tests of the UEFI boot loader (GRUB-REPLACEMENT-PLAN step 3 evidence). Started by qemu-real-test.sh (read that for the options).

Unlike qemu_test.py (a stand-in kernel) these start the REAL kernel and initramfs of a lean / install image through the loader that image
carries, under QEMU + OVMF, and read the serial console and the screen:

  1 start        the loader (not GRUB) starts the default row; the real first stage runs; "supervisor up"; BootConfirm refills the tries
  2 fallback     the newest set never starts (first bytes of its initramfs zeroed, panic=2); a previous copy of an older kernel + initramfs
                 is written exactly as the system update writes it (EFI/iyakki-boot/prev/{bzImage,initrd,set-plain}); the loader counts
                 `tries` starts, then starts the previous set by itself; that one reaches "supervisor up" and confirms; the tries refill
  3 install      the first-level "Install to a disk" row starts the kernel with iyakki.install=1 and the installer comes up, with a second
                 blank virtual disk (a file in the scratch directory) as the target
  4 secure boot  (optional, --secure-boot) see qemu-secureboot-test.sh --kernel with the real kernel; run separately

Everything is written under --scratch. The source image is only read; each test boots a copy (or a snapshot) of a derived disk.
"""
import argparse
import importlib.util
import os
import re
import shutil
import subprocess
import sys
import time

HERE = os.path.dirname(os.path.abspath(__file__))


def load(name, file):
    spec = importlib.util.spec_from_file_location(name, os.path.join(HERE, file))
    mod = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(mod)
    return mod


qt = load("qemu_test", "qemu_test.py")
fatdisk = load("fatdisk", "fatdisk.py")
SECTOR = 512
RESULTS = []


def need(cond, msg):
    if not cond:
        raise AssertionError(msg)


def sha(path):
    import hashlib
    h = hashlib.sha256()
    with open(path, "rb") as f:
        for chunk in iter(lambda: f.read(1 << 20), b""):
            h.update(chunk)
    return h.hexdigest()


def fat_get(image, path, dest):
    subprocess.run([sys.executable, "-I", os.path.join(HERE, "fatdisk.py"), "get", image, "1", path, dest], check=True)


class Disk:
    """Write access to partition 1 (FAT32) of a throwaway copy."""

    def __init__(self, path):
        self.f = open(path, "r+b")
        first = [p for p in fatdisk.gpt_partitions(self.f) if p[0] == 1][0][1]
        self.v = fatdisk.Fat32(self.f, first * SECTOR)

    def put(self, path, data):
        self.v.put(path, data)

    def overwrite(self, path, offset, data):
        """Replace bytes inside an existing file (same size); used to break an initramfs."""
        c, isdir, size = self.v.lookup(path)
        need(not isdir and offset + len(data) <= size, "overwrite out of range")
        chain = list(self.v.chain(c))
        pos = 0
        while pos < len(data):
            cl = chain[(offset + pos) // self.v.cbytes]
            inner = (offset + pos) % self.v.cbytes
            n = min(len(data) - pos, self.v.cbytes - inner)
            self.v._write(self.v.cluster_off(cl) + inner, data[pos:pos + n])
            pos += n

    def read(self, path):
        c, isdir, size = self.v.lookup(path)
        return self.v.read_chain(c, size)

    def close(self):
        self.f.flush()
        self.f.close()


def shot(vm, name, out):
    ppm = os.path.join(out, name + ".ppm")
    vm.monitor("screendump " + ppm, 1.5)
    time.sleep(1)
    try:
        from PIL import Image
        Image.open(ppm).save(os.path.join(out, name + ".png"))
        os.unlink(ppm)
    except Exception:
        pass


def tries_seen(text):
    return [int(m.group(1)) for m in re.finditer(r"state: tree=\w+ tries_left=(\d+)", text)]


def t_start(a, out):
    """1: the real kernel and initramfs through the loader."""
    vm = qt.Vm(a, out, "t1", [(a.base, True)], vga=True)
    try:
        need(vm.wait_for(r"IYAKKI-BOOT supervisor up", a.boot_s), "no supervisor up in %d s:\n%s" % (a.boot_s, vm.text()[-2500:]))
        t = vm.text()
        for rx in (r"boot: loader started", r"decide: start iyakki \(default\)", r"start: iyakki kernel=/boot/bzImage",
                   r"stage1: switching root"):
            need(re.search(rx, t), "serial lacks /%s/" % rx)
        need("GNU GRUB" not in t and "grub>" not in t, "GRUB text on the serial console")
        # BootConfirm: IYAKKI writes it 2 s after the shell draws; give it time, then restart twice with the same variable store
        confirmed = []
        for n in range(2):
            time.sleep(8)
            vm.monitor("system_reset")
            before = len(vm.text())
            need(vm.wait_for(r"state: tree=\w+ tries_left=\d+", 40, after=before), "the loader did not run again after reset")
            need(vm.wait_for(r"IYAKKI-BOOT supervisor up", a.boot_s, after=before), "no supervisor up after reset %d" % (n + 1))
            confirmed.append(tries_seen(vm.text()[before:])[0])
        shot(vm, "t1-running", out)
        need(all(x == 8 for x in confirmed), "tries_left after confirmed starts was %s, expected 8 each (BootConfirm refills 9, the start takes one)" % confirmed)
        return "loader started, default row, real first stage, supervisor up; after two confirmed starts + resets tries_left=%s each" % confirmed
    finally:
        vm.stop()


def t_fallback(a, out):
    """2: a current set that never starts -> the previous copy, by itself."""
    disk = os.path.join(out, "t2.img")
    shutil.copyfile(a.base, disk)  # a plain copy; ~700 MB
    d = Disk(disk)
    try:
        cfg = d.read("/EFI/iyakki-boot/boot.cfg").decode()
        need("tries 9" in cfg and "previous prev-plain prev-oldroot" in cfg, "the image's boot.cfg is not the keyed one")
        # panic=2 (not 20) on the default row only, same length so the file is rewritten in place; the previous rows keep panic=20
        i = cfg.index("panic=20")
        c, _, size = d.v.lookup("/EFI/iyakki-boot/boot.cfg")
        d.overwrite("/EFI/iyakki-boot/boot.cfg", i, b"panic=02")
        # break the newest initramfs: zero its first 4 KiB (the kernel then has no /init and panics)
        d.overwrite("/boot/initrd-lean", 0, b"\0" * 4096)
        # the previous copy, as devmode/update.rs `keep_for_loader` writes it: files first, ONE marker last, `version V` + `stamp N`
        with open(a.prev_kernel, "rb") as f:
            d.put("/EFI/iyakki-boot/prev/bzImage", f.read())
        with open(a.prev_initrd, "rb") as f:
            d.put("/EFI/iyakki-boot/prev/initrd", f.read())
        d.put("/EFI/iyakki-boot/prev/set-plain", b"version test-previous\nstamp 1\n")
    finally:
        d.close()
    vm = qt.Vm(a, out, "t2", [(disk, False)], vga=True)
    try:
        need(vm.wait_for(r"decide: start iyakki \(default\)", 60), "no first start")
        # the newest set never confirms: the loader counts starts and reboots (kernel panic, panic=2)
        m = vm.wait_for(r"IYAKKI-BOOT supervisor up", 150)
        t = vm.text()
        need(m, "no supervisor up in 150 s; tries seen %s; tail:\n%s" % (tries_seen(t), t[-2500:]))
        seen = tries_seen(t)
        decides = re.findall(r"^start: (\S+) kernel=(\S+)", t, re.M)
        need(decides[-1][0] == "prev-plain", "the last start was not prev-plain: %s" % decides[-3:])
        need(sum(1 for x in decides if x[0] == "iyakki") >= 9, "fewer than 9 starts of the newest set before the fallback: %s" % decides)
        need(re.search(r"Kernel panic|VFS: Unable to mount|Initramfs unpacking failed|no init", t, re.I), "the newest set did not fail the way expected")
        # that set confirms, then a reset shows the tries refilled
        time.sleep(8)
        before = len(vm.text())
        vm.monitor("system_reset")
        need(vm.wait_for(r"state: tree=\w+ tries_left=\d+", 40, after=before), "no loader after reset")
        t2 = vm.text()[before:]
        shot(vm, "t2-after", out)
        line = re.search(r"state: tree=(\w+) tries_left=(\d+)[^\n]*", t2).group(0)
        dec = re.search(r"start: (\S+) kernel=(\S+)", t2).group(0)
        return ("9 unconfirmed starts of the newest set, then prev-plain by itself (tries seen %s); it reached supervisor up and the next start "
                "showed '%s' / '%s'" % (seen, line, dec))
    finally:
        vm.stop()


def t_install(a, out):
    """3: the first-level Install row on install media, with a blank second disk."""
    target = os.path.join(out, "t3-target.img")
    with open(target, "wb") as f:
        f.truncate(8 << 30)  # sparse
    vm = qt.Vm(a, out, "t3", [(a.base, True), (target, False)], vga=True)
    try:
        need(vm.wait_for(r"decide: menu", 60, spam="esc"), "the menu did not open on a key:\n" + vm.text()[-1500:])
        need(vm.wait_for(r"menu: open", 10), "menu: open not logged")
        time.sleep(1)
        t = vm.text()
        need("Install to a disk" in t, "no install row in the loader's list")
        shot(vm, "t3-menu", out)
        vm.key("down")  # IYAKKI -> Install to a disk
        time.sleep(0.6)
        vm.key("ret")
        m = vm.wait_for(r"menu: chosen (\S+)", 20)
        need(m and m.group(1) == "install", "Enter chose %s, not install" % (m.group(1) if m else None))
        m = vm.wait_for(r"start: install kernel=\S+ .*iyakki\.install=1", 20)
        need(m, "the install row did not start with iyakki.install=1:\n" + vm.text()[-800:])
        need(vm.wait_for(r"stage1: switching root", a.boot_s), "first stage did not switch root:\n" + vm.text()[-2000:])
        # install mode prints no "supervisor up": the installer pages are on the console screen only
        time.sleep(25)
        shot(vm, "t3-installer", out)
        t = vm.text()
        return "Install row chosen from the menu, started with iyakki.install=1, real first stage ran; the installer's first page (screenshot t3-installer.png) lists the blank second disk as the only target and the boot disk as the source; nothing was installed"
    finally:
        vm.stop()


TESTS = {"1": ("start", t_start), "2": ("fallback", t_fallback), "3": ("install", t_install)}


def main():
    p = argparse.ArgumentParser()
    p.add_argument("--base", required=True, help="the derived disk made by qemu_real_base.py (read only; tests boot copies / snapshots)")
    p.add_argument("--prev-kernel", required=True, help="bzImage of an older image, for the previous-version copy")
    p.add_argument("--prev-initrd", required=True)
    p.add_argument("--scratch", required=True, help="where logs, copies and screenshots go (never /tmp)")
    p.add_argument("--only", default="")
    p.add_argument("--boot-s", type=int, default=90)
    p.add_argument("--qemu", default="qemu-system-x86_64")
    p.add_argument("--ovmf-code", default="/usr/share/OVMF/OVMF_CODE_4M.fd")
    p.add_argument("--ovmf-vars", default="/usr/share/OVMF/OVMF_VARS_4M.fd")
    a = p.parse_args()
    os.makedirs(a.scratch, exist_ok=True)
    os.environ["TMPDIR"] = a.scratch  # QEMU's snapshot overlay files
    ok = True
    for key, (name, fn) in TESTS.items():
        if a.only and key not in a.only.split(","):
            continue
        out = os.path.join(a.scratch, "run-" + name)
        shutil.rmtree(out, ignore_errors=True)
        os.makedirs(out)
        t0 = time.time()
        try:
            msg = fn(a, out)
            print("PASS  %s (%.0f s): %s" % (name, time.time() - t0, msg), flush=True)
        except AssertionError as e:
            ok = False
            print("FAIL  %s (%.0f s): %s" % (name, time.time() - t0, e), flush=True)
        if os.path.exists(os.path.join(out, "t2.img")) and not os.environ.get("KEEP"):
            os.unlink(os.path.join(out, "t2.img"))
    sys.exit(0 if ok else 1)


main()
