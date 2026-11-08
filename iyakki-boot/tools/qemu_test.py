#!/usr/bin/env python3
# SPDX-License-Identifier: MIT OR Apache-2.0
"""QEMU/OVMF tests of the UEFI boot loader. Standard library only; started by qemu-test.sh (read that for the options).

Each test builds a throwaway raw disk (GPT + FAT32 ESP) in the scratch directory, boots it under OVMF with a fresh copy of the
variable store, drives the menu through the QEMU monitor (sendkey) and reads the serial console. Only the process this script
started is ever stopped. Nothing outside the scratch directory is written; the real-kernel test only reads the image it is given.
"""
import argparse
import hashlib
import os
import re
import shutil
import socket
import subprocess
import sys
import tempfile
import time

HERE = os.path.dirname(os.path.abspath(__file__))
ANSI = re.compile(r"\x1b\[[0-9;?]*[ -/]*[@-~]|\x1b[()][0-9A-B]")


def clean(text):
    return ANSI.sub("", text).replace("\r", "")


def initrd_bytes(tag, n):
    out = bytearray()
    x = int.from_bytes(hashlib.sha256(tag.encode()).digest()[:4], "little")
    while len(out) < n:
        x = (x * 1103515245 + 12345) & 0xFFFFFFFF
        out += (x >> 8 & 0xFF).to_bytes(1, "little")
    return bytes(out)


def hash31(data):
    s = 0
    for b in data:
        s = (s * 31 + b) & 0xFFFFFFFF
    return s


class Skip(Exception):
    """A test that cannot run on this machine (not a failure)."""


class Vm:
    def __init__(self, a, scratch, name, disks, vga=False, extra=None, vars_path=None, machine="q35"):
        self.a, self.name = a, name
        self.log = os.path.join(scratch, name + ".serial")
        self.sock = os.path.join(scratch, name + ".mon")
        self.vars = vars_path or os.path.join(scratch, name + ".vars.fd")
        if not vars_path:
            shutil.copyfile(a.ovmf_vars, self.vars)  # a fresh copy: the original is never written
        open(self.log, "w").close()
        kvm = os.access("/dev/kvm", os.R_OK | os.W_OK)
        cmd = [a.qemu, "-machine", machine, "-m", "2048", "-smp", "2", "-display", "none", "-no-user-config", "-nodefaults",
               "-accel", "kvm" if kvm else "tcg", "-cpu", "host" if kvm else "max",
               "-drive", f"if=pflash,format=raw,unit=0,readonly=on,file={a.ovmf_code}",
               "-drive", f"if=pflash,format=raw,unit=1,file={self.vars}",
               "-serial", f"file:{self.log}", "-monitor", f"unix:{self.sock},server,nowait",
               "-vga", "std" if vga else "none"]
        for i, (path, snap) in enumerate(disks):
            cmd += ["-drive", f"file={path},format=raw,if=none,id=d{i},snapshot={'on' if snap else 'off'}",
                    "-device", f"virtio-blk-pci,drive=d{i}" + (",bootindex=1" if i == 0 else "")]
        cmd += extra or []
        self.p = subprocess.Popen(cmd, stdin=subprocess.DEVNULL, stdout=subprocess.DEVNULL, stderr=subprocess.PIPE)
        self.mon = None
        for _ in range(100):
            try:
                self.mon = socket.socket(socket.AF_UNIX)
                self.mon.connect(self.sock)
                self.mon.setblocking(False)
                break
            except OSError:
                self.mon = None
                time.sleep(0.1)

    def text(self):
        with open(self.log, "rb") as f:
            return clean(f.read().decode("utf-8", "replace"))

    def monitor(self, line, wait=0.02):
        reply = b""
        if self.mon:
            try:
                self.mon.send((line + "\n").encode())
                time.sleep(wait)
                try:
                    reply = self.mon.recv(65536)
                except BlockingIOError:
                    pass
            except OSError:
                pass
        return clean(reply.decode("utf-8", "replace"))

    def key(self, k):
        self.monitor("sendkey " + k)

    def wait_for(self, pattern, timeout, spam=None, after=0):
        """Wait until the (cleaned) serial text has `pattern` after offset `after`; optionally send a key every 100 ms meanwhile."""
        end = time.time() + timeout
        rx = re.compile(pattern)
        while time.time() < end:
            t = self.text()
            m = rx.search(t, after)
            if m:
                return m
            if self.p.poll() is not None:
                return None
            if spam:
                self.key(spam)
            time.sleep(0.1 if spam else 0.25)
        return None

    def stop(self):
        if self.p.poll() is None:
            self.p.terminate()
            try:
                self.p.wait(10)
            except subprocess.TimeoutExpired:
                self.p.kill()
                self.p.wait()
        if self.mon:
            self.mon.close()


class Suite:
    def __init__(self, a):
        self.a = a
        base = a.scratch
        os.makedirs(base, exist_ok=True)
        self.dir = tempfile.mkdtemp(prefix="qemu-", dir=base)
        self.results = []
        self.keep = a.keep

    def disk(self, name, files, text=None, size_mb=48):
        out = os.path.join(self.dir, name + ".img")
        cmd = [sys.executable, "-I", os.path.join(HERE, "fatdisk.py"), "make", out, "--size-mb", str(size_mb)]
        for src, dest in files:
            cmd += ["--put", f"{src}:{dest}"]
        for t, dest in text or []:
            cmd += ["--text", f"{t}:{dest}"]
        subprocess.run(cmd, check=True)
        return out

    def trees(self, names=("current", "previous", "seed", "alt", "rescue")):
        files = []
        self.initrds = {}
        for n in names:
            files.append((self.a.test_efi, f"/{n}/vmlinuz"))
            data = initrd_bytes(n, 4096 * 3 + 100 * len(n) // 4 * 4)
            data = data[: len(data) // 4 * 4]
            p = os.path.join(self.dir, f"initrd-{n}.img")
            with open(p, "wb") as f:
                f.write(data)
            files.append((p, f"/{n}/initrd.img"))
            self.initrds[n] = (len(data), hash31(data))
        return files

    def boot_files(self):
        return [(self.a.boot_efi, "/EFI/BOOT/BOOTX64.EFI")]

    def run(self, name, fn):
        t0 = time.time()
        try:
            fn()
            ok, why = True, ""
        except Skip as e:
            self.results.append((name, None, str(e)))
            print(f"SKIP  {name}  -> {e}", flush=True)
            return
        except AssertionError as e:
            ok, why = False, str(e)
        except Exception as e:  # a broken test is a failed test, never a silent pass
            ok, why = False, f"{type(e).__name__}: {e}"
        self.results.append((name, ok, why))
        print(f"{'PASS' if ok else 'FAIL'}  {name}  ({time.time() - t0:.0f}s){'  -> ' + why if why else ''}", flush=True)

    def vm(self, name, disks, **kw):
        v = Vm(self.a, self.dir, name, disks, **kw)
        self.vms = getattr(self, "vms", []) + [v]
        return v

    def done(self):
        for v in getattr(self, "vms", []):
            v.stop()
        if not self.keep:
            shutil.rmtree(self.dir, ignore_errors=True)
        else:
            print("kept", self.dir)


def need(cond, msg):
    if not cond:
        raise AssertionError(msg)


CFG_BASIC = """timeout 0
entry current
  title Current system
  kernel /{tree}/vmlinuz
  initrd /{tree}/initrd.img
  cmdline console=ttyS0 test=1
  default
entry seed
  title Seed system
  kernel /seed/vmlinuz
  initrd /seed/initrd.img
  cmdline console=ttyS0 seed=1
"""


def t_default(s):
    files = s.boot_files() + s.trees()
    # the log directory of the real images exists here too (EFI/iyakki-boot), so the log lands on its real path
    d = s.disk("a", files, [(CFG_BASIC, "/boot.cfg"), ("log directory", "/EFI/iyakki-boot/README.txt")])
    vm = s.vm("a", [(d, False)])  # no snapshot: the loader's log file must land in this throwaway disk so it can be read back
    try:
        m = vm.wait_for(r"TEST-EFI started path=(\S+) options=\[([^\]]*)\] initrd=(\d+) sum=([0-9a-f]+)", 90)
        need(m, "the default entry did not start:\n" + vm.text()[-1500:])
        size, h = s.initrds["current"]
        need(m.group(1) == "\\current\\vmlinuz", f"wrong kernel path {m.group(1)}")
        need(m.group(2) == "console=ttyS0 test=1", f"wrong command line {m.group(2)!r}")
        need(int(m.group(3)) == size and int(m.group(4), 16) == h, f"initrd differs: {m.group(3)} {m.group(4)} vs {size} {h:08x}")
        need("start: current kernel=/current/vmlinuz tree=current" in vm.text(), "start line missing")
    finally:
        vm.stop()
    # the boot log on the volume: the device summary, the entries and the start, written before the kernel ran
    out = os.path.join(s.dir, "a-last-boot.log")
    subprocess.run([sys.executable, "-I", os.path.join(HERE, "fatdisk.py"), "get", d, "1", "/EFI/iyakki-boot/last-boot.log", out], check=True)
    with open(out, "rb") as f:
        log = f.read().decode("utf-8", "replace")
    for want in ("boot: loader started", "hw: firmware ", "hw: memory ", "hw: pci ", "hw: disk ", "hw: screen", "hw: input: ",
                 "config: /boot.cfg read", "item: 0 current", "start: current kernel=/current/vmlinuz tree=current",
                 "log: about to start; this log is in /EFI/iyakki-boot/last-boot.log"):
        need(want in log, f"the boot log lacks {want!r}:\n{log[-2000:]}")
    need("could not be written" not in log, "the log reports a failed write:\n" + log[-800:])
    # the pre-start write comes after the line that names the kernel: a kernel that hangs still leaves that line in the file
    need(log.find("start: current kernel=") < log.find("log: about to start"), "the about-to-start write precedes the start line:\n" + log[-800:])
    need(log.rstrip().endswith("on the boot volume"), "the last line of the file is not the 'about to start' line:\n" + log[-300:])


def t_real_kernel(s):
    a = s.a
    if not a.lean_image:
        raise Skip("no --lean-image")
    k = os.path.join(s.dir, "bzImage")
    i = os.path.join(s.dir, "initrd-lean")
    for src, dst in (("/boot/bzImage", k), ("/boot/initrd-lean", i)):
        subprocess.run([sys.executable, "-I", os.path.join(HERE, "fatdisk.py"), "get", a.lean_image, "1", src, dst], check=True)
    cfg = "timeout 0\nentry real\n  title Real kernel\n  kernel /real/vmlinuz\n  initrd /real/initrd.img\n  cmdline console=ttyS0 loglevel=7 panic=-1\n  default\n"
    d = s.disk("real", s.boot_files() + [(k, "/real/vmlinuz"), (i, "/real/initrd.img")], [(cfg, "/boot.cfg")], size_mb=64)
    vm = s.vm("real", [(d, True)])
    try:
        need(vm.wait_for(r"start: real kernel=/real/vmlinuz", 90), "loader did not start the entry")
        m = vm.wait_for(r"Run /init as init process", 150)
        t = vm.text()
        need(m, "the kernel did not reach /init:\n" + t[-2500:])
        need(re.search(r"Linux version", t), "no kernel banner")
        # the initramfs the loader handed over was unpacked and its init ran (not just the kernel): a line from the first stage
        if a.initrd_marker:
            m2 = vm.wait_for(a.initrd_marker, 60, after=m.end())
            need(m2, f"no output matching {a.initrd_marker!r} from the initrd's init after 'Run /init':\n" + t[-1500:])
            print("      initrd output seen: " + repr(m2.group(0)), flush=True)
    finally:
        vm.stop()


def t_menu_and_hold(s):
    cfg = """timeout 30
entry current
  title Current system
  kernel /{tree}/vmlinuz
  initrd /{tree}/initrd.img
  cmdline console=ttyS0
  default
entry alt
  title Alternative system
  kernel /alt/vmlinuz
  initrd /alt/initrd.img
  cmdline console=ttyS0 alt=1
entry rescue
  title Rescue system
  kernel /rescue/vmlinuz
  initrd /rescue/initrd.img
  cmdline console=ttyS0 rescue=1
  hold r
  hidden
"""
    d = s.disk("b", s.boot_files() + s.trees(), [(cfg, "/boot.cfg")])
    # menu: Down, Enter picks the second entry
    vm = s.vm("b1", [(d, True)])
    try:
        need(vm.wait_for(r"menu: open", 60), "menu did not open:\n" + vm.text()[-1200:])
        need(re.search(r"Current system", vm.text()) and "Choose what to start" in vm.text(), "menu text not drawn")
        need("Alternative system" in vm.text(), "second row not drawn")
        vm.key("down")
        time.sleep(0.5)
        vm.key("ret")
        need(vm.wait_for(r"menu: chosen alt\b", 20), "Enter did not choose the second entry")
        m = vm.wait_for(r"TEST-EFI started path=(\S+) options=\[([^\]]*)\]", 30)
        need(m and m.group(1) == "\\alt\\vmlinuz" and "alt=1" in m.group(2), "alt kernel did not start")
    finally:
        vm.stop()
    # countdown: no key at all, the default is chosen when the time is up (timeout shortened to 3 s)
    cfg3 = cfg.replace("timeout 30", "timeout 3")
    d3 = s.disk("b3", s.boot_files() + s.trees(), [(cfg3, "/boot.cfg")])
    vm = s.vm("b3", [(d3, True)])
    try:
        m = vm.wait_for(r"menu: chosen (\w+) \(countdown\)", 60)
        need(m and m.group(1) == "current", "countdown did not choose the default")
        need(vm.wait_for(r"state: tree=current tries_left=2", 10), "a try was not spent for the automatic start")
    finally:
        vm.stop()
    # hold key: 'r' down from the very start picks the hidden rescue entry without a menu
    vm = s.vm("b2", [(d, True)])
    try:
        m = vm.wait_for(r"decide: start rescue \(hold key\)", 60, spam="r")
        need(m, "hold key r did not pick the rescue entry:\n" + vm.text()[-1200:])
        m = vm.wait_for(r"TEST-EFI started path=(\S+) options=\[([^\]]*)\]", 30)
        need(m and m.group(1) == "\\rescue\\vmlinuz", "rescue kernel did not start")
        need("tries_left=3" in vm.text() or "tries_left=2" not in vm.text().split("decide: start rescue")[1][:200], "a hold-key start spent a try")
    finally:
        vm.stop()


def t_fallback(s):
    # no config file at all
    d = s.disk("c1", s.boot_files() + s.trees(), [])
    vm = s.vm("c1", [(d, True)])
    try:
        need(vm.wait_for(r"config: /boot.cfg not found; built-in entries used", 60), "missing config not reported:\n" + vm.text()[-800:])
        m = vm.wait_for(r"TEST-EFI started path=(\S+) options=\[([^\]]*)\] initrd=(\d+)", 60)
        need(m and m.group(1) == "\\current\\vmlinuz", "built-in current entry did not start")
        need(int(m.group(3)) == s.initrds["current"][0], "built-in entry did not pass its initrd")
    finally:
        vm.stop()
    # broken config
    d = s.disk("c2", s.boot_files() + s.trees(), [("timeout 5\\nbogus-key 1\\nentry x\\n  kernel /x\\n", "/boot.cfg")])
    vm = s.vm("c2", [(d, True)])
    try:
        m = vm.wait_for(r"config: /boot.cfg is broken \(([^)]*)\); built-in entries used", 60)
        need(m and "bogus-key" in m.group(1), "broken config not reported with its reason:\n" + vm.text()[-800:])
        m = vm.wait_for(r"TEST-EFI started path=(\S+)", 60)
        need(m and m.group(1) == "\\current\\vmlinuz", "built-in current entry did not start after a broken config")
    finally:
        vm.stop()


def t_tries(s):
    cfg = """timeout 0
tries 3
entry current
  title Current system
  kernel /{tree}/vmlinuz
  initrd /{tree}/initrd.img
  cmdline console=ttyS0 reset
  default
entry seed
  title Seed system
  kernel /seed/vmlinuz
  cmdline console=ttyS0 seed=1
"""
    d = s.disk("d", s.boot_files() + s.trees(), [(cfg, "/boot.cfg")])
    vm = s.vm("d", [(d, True)])
    try:
        m = vm.wait_for(r"decide: menu \(waiting\)", 240)
        t = vm.text()
        need(m, "the loader never stopped to wait:\n" + t[-1500:])
        trees = re.findall(r"state: tree=(\w+) tries_left=(\d+)", t)
        need(trees[:6] == [("current", "2"), ("current", "1"), ("current", "0"), ("previous", "2"), ("previous", "1"), ("previous", "0")],
             f"wrong sequence of starts: {trees}")
        need(len(re.findall(r"TEST-EFI started path=\\current", t)) == 3 and len(re.findall(r"TEST-EFI started path=\\previous", t)) == 3,
             "kernel starts were not 3 current + 3 previous")
        need("notice: the newest and the previous system both failed" in t, "the reason was not shown")
        mo = vm.wait_for(r"menu: open \(waiting, no timeout\)", 10)
        need(mo, "the menu did not open in waiting mode")
        need(vm.wait_for(r"both failed to start several times", 10, after=mo.end()), "the reason is not on the menu screen")
        # no timeout: wait well past any countdown and check that nothing starts by itself
        n = len(re.findall(r"TEST-EFI started", vm.text()))
        time.sleep(12)
        need(len(re.findall(r"TEST-EFI started", vm.text())) == n, "something started by itself while the menu should wait")
        # the person can still choose: Down moves to the seed entry, Enter starts it
        vm.key("down")
        time.sleep(0.5)
        vm.key("ret")
        m = vm.wait_for(r"TEST-EFI started path=\\seed\\vmlinuz", 30)
        need(m, "a manual choice after the stop did not start")
    finally:
        vm.stop()


def t_chain(s):
    cfg = "timeout 30\nscan yes\nentry current\n  title Current system\n  kernel /{tree}/vmlinuz\n  initrd /{tree}/initrd.img\n  cmdline console=ttyS0\n  default\n"
    d1 = s.disk("e1", s.boot_files() + s.trees(), [(cfg, "/boot.cfg")])
    osr = "NAME=\"Fake Windows\"\\nPRETTY_NAME=\"Fake Windows Test\"\\n"
    d2 = s.disk("e2", [(s.a.test_efi, "/EFI/Microsoft/Boot/bootmgfw.efi"), (s.a.test_efi, "/EFI/Other/loader.efi"), (s.a.test_efi, "/EFI/BOOT/BOOTX64.EFI")],
                [(osr, "/etc/os-release")])
    before = hashlib.sha256(open(d2, "rb").read()).hexdigest()
    vm = s.vm("e", [(d1, True), (d2, False)])
    try:
        need(vm.wait_for(r"menu: open", 60), "menu did not open:\n" + vm.text()[-1500:])
        t = vm.text()
        items = re.findall(r"item: (\d+) (\S+) \"([^\"]*)\"", t)
        ids = [i[1] for i in items]
        need(any("bootmgfw.efi" in i for i in ids), f"bootmgfw.efi not found by the scan: {items}")
        need(any("/efi/other/loader.efi" in i for i in ids), f"EFI/Other/loader.efi not found: {items}")
        need(any("/efi/boot/bootx64.efi" in i for i in ids), f"fallback loader of the other disk not found: {items}")
        need(not any(i.endswith("efi:boot:/efi/boot/bootx64.efi") for i in ids), f"the loader listed itself: {items}")
        need(len(items) == 4, f"expected 4 items (1 configured + 3 found), got {items}")
        idx = [n for n, i, _ in items if "bootmgfw.efi" in i][0]
        titles = {i: ti for _, i, ti in items}
        print("      items: " + "; ".join(f"{i} = {ti}" for _, i, ti in items), flush=True)
        # other systems live under "More options": Down to it, Enter, then (the first found system is selected) Down to ours, Enter
        vm.key("down")
        time.sleep(0.3)
        vm.key("ret")
        need(vm.wait_for(r"menu: more options", 10), "Enter on More options did not open the second level")
        for _ in range(int(idx) - 1):
            vm.key("down")
            time.sleep(0.3)
        vm.key("ret")
        m = vm.wait_for(r"TEST-EFI started path=(\S+) options=\[([^\]]*)\]", 40)
        need(m and m.group(1).lower() == "\\efi\\microsoft\\boot\\bootmgfw.efi", f"chain load did not run the fake bootmgfw: {m and m.group(0)}")
        need(vm.wait_for(r"returned: success", 20), "the loader did not notice the chained image returning")
        need(vm.wait_for(r"menu: open \(waiting", 10), "the loader did not return to a waiting menu")
    finally:
        vm.stop()
    after = hashlib.sha256(open(d2, "rb").read()).hexdigest()
    need(before == after, "the other disk was modified")


def t_secure_boot(s):
    """Negative check only: with the Secure Boot firmware and its enrolled variable store, the unsigned loader is refused."""
    a = s.a
    code, vars_ = a.ovmf_code.replace("_4M.fd", "_4M.snakeoil.fd"), a.ovmf_vars.replace("_4M.fd", "_4M.snakeoil.fd")
    if not (os.path.isfile(code) and os.path.isfile(vars_)):
        raise Skip("no Secure Boot firmware files with enrolled keys on this machine")
    d = s.disk("sb", s.boot_files(), [("timeout 0\\nentry x\\n  kernel /x/vmlinuz\\n", "/boot.cfg")])
    a2 = argparse.Namespace(**vars(a))
    a2.ovmf_code, a2.ovmf_vars = code, vars_
    v = Vm(a2, s.dir, "sb", [(d, True)], extra=["-global", "driver=cfi.pflash01,property=secure,value=on"], machine="q35,smm=on")
    s.vms = getattr(s, "vms", []) + [v]
    try:
        need(v.wait_for(r"Access Denied -- rejected probably by Secure Boot", 40), "the unsigned loader was not refused:\n" + v.text()[-800:])
        need("boot: loader started" not in v.text(), "the unsigned loader ran with Secure Boot on")
    finally:
        v.stop()


def read_ppm(path):
    with open(path, "rb") as f:
        data = f.read()
    m = re.match(rb"P6\s+(\d+)\s+(\d+)\s+(\d+)\s", data)
    w, h = int(m.group(1)), int(m.group(2))
    return w, h, data[m.end():]


def t_graphics(s):
    cfg = "timeout 60\nentry current\n  title Current system\n  kernel /{tree}/vmlinuz\n  initrd /{tree}/initrd.img\n  default\nentry seed\n  title Seed system\n  kernel /seed/vmlinuz\n"
    d = s.disk("g", s.boot_files() + s.trees(), [(cfg, "/boot.cfg")])
    vm = s.vm("g", [(d, True)], vga=True, extra=["-device", "qemu-xhci", "-device", "usb-mouse"])
    try:
        need(vm.wait_for(r"menu: open", 60), "menu did not open")
        time.sleep(2)
        shot = os.path.join(s.dir, "g.ppm")
        vm.monitor("screendump " + shot)
        time.sleep(1)
        w, h, px = read_ppm(shot)
        need(w >= 640 and h >= 480, f"screen too small {w}x{h}")
        sel = bg = row = 0
        for i in range(0, len(px) - 2, 3):
            c = (px[i], px[i + 1], px[i + 2])
            if c == (0x2a, 0x6f, 0xd6):
                sel += 1
            elif c == (0x10, 0x12, 0x1c):
                bg += 1
            elif c == (0x1e, 0x22, 0x32):
                row += 1
        print(f"      screen {w}x{h}: selected-row pixels {sel}, normal-row pixels {row}, background {bg}", flush=True)
        need(sel > w * 40 and row > w * 40 and bg > w * h // 3, "the graphical menu does not look drawn (colour counts)")
        # keyboard on the graphical menu
        vm.key("down")
        time.sleep(0.7)
        vm.monitor("screendump " + shot + "2")
        time.sleep(1)
        _, _, px2 = read_ppm(shot + "2")
        need(px2 != px, "the screen did not change after Down")
        # mouse: move the pointer onto the second row and click it (best effort, relative mouse)
        vm.key("up")
        time.sleep(0.5)
        mice = vm.monitor("info mice", 0.5)
        usb = re.search(r"Mouse #(\d+): QEMU HID Mouse", mice)
        need(usb, f"no USB mouse in the VM: {mice!r}")
        vm.monitor(f"mouse_set {usb.group(1)}")  # the monitor drives the PS/2 mouse unless told otherwise
        scale = 4 if w >= 1920 else 3 if w >= 1280 else 2
        header, row_h = 8 * scale * 5, 8 * scale * 4
        for _ in range(100):
            vm.monitor("mouse_move 0 -20")  # to the top edge (the pointer starts in the middle of the screen)
        for _ in range((header + row_h + row_h // 2) // 10):
            vm.monitor("mouse_move 0 10")  # down into the second row
        time.sleep(0.3)
        vm.monitor("mouse_button 1")
        time.sleep(0.2)
        vm.monitor("mouse_button 0")
        m = vm.wait_for(r"menu: chosen (\w+)", 10)
        if not m:
            # Seen with OVMF 4M: the firmware lists a pointer device but delivers no events from the monitor's mouse_move, with
            # a PS/2, xHCI, EHCI or UHCI mouse alike. So the click path (Tap -> row) is covered by boot-core's unit tests only.
            print("      mouse: NOT VERIFIED (the firmware pointer produced no events under QEMU)", flush=True)
        else:
            need(m.group(1) == "seed", f"the click chose {m.group(1)} instead of the second row (seed)")
    finally:
        vm.stop()


def make_bmp(path, w, h, f, bits=24, top_down=False):
    """Write an uncompressed BMP; f(x, y) -> (r, g, b) with y counted from the top."""
    bp = bits // 8
    stride = (w * bits + 31) // 32 * 4
    rows = []
    for y in range(h):
        row = bytearray()
        for x in range(w):
            r, g, b = f(x, y)
            row += bytes([b, g, r]) + (b"\x00" if bp == 4 else b"")
        row += b"\x00" * (stride - len(row))
        rows.append(bytes(row))
    if not top_down:
        rows.reverse()
    body = b"".join(rows)
    hdr = (b"BM" + (54 + len(body)).to_bytes(4, "little") + bytes(4) + (54).to_bytes(4, "little") + (40).to_bytes(4, "little")
           + w.to_bytes(4, "little") + ((-h if top_down else h) & 0xFFFFFFFF).to_bytes(4, "little") + (1).to_bytes(2, "little")
           + bits.to_bytes(2, "little") + bytes(24))
    with open(path, "wb") as fh:
        fh.write(hdr + body)


SPLASH_BG = (0x20, 0x40, 0x80)
MENU_BG = (0x10, 0x12, 0x1c)
MENU_SEL, MENU_ROW, MENU_FG = (0x2a, 0x6f, 0xd6), (0x1e, 0x22, 0x32), (0xe6, 0xe8, 0xf0)
RED, GREEN, YELLOW = (255, 0, 0), (0, 255, 0), (255, 255, 0)


class Shot:
    """A screendump: size, colour counts and the bounding box of everything that is not `bg`."""

    def __init__(self, vm, path, bg=SPLASH_BG):
        vm.monitor("screendump " + path)
        time.sleep(1)
        self.w, self.h, px = read_ppm(path)
        pix = list(zip(px[0::3], px[1::3], px[2::3]))
        self.pix = pix
        self.count = {}
        for c in pix:
            self.count[c] = self.count.get(c, 0) + 1
        xs = [i % self.w for i, c in enumerate(pix) if c != bg]
        ys = [i // self.w for i, c in enumerate(pix) if c != bg]
        self.box = (min(xs), min(ys), max(xs), max(ys)) if xs else None

    def n(self, c):
        return self.count.get(c, 0)

    def bbox(self, *colours):
        """(x0, y0, x1, y1) of all pixels of these colours, or None."""
        idx = [i for i, c in enumerate(self.pix) if c in colours]
        if not idx:
            return None
        xs = [i % self.w for i in idx]
        ys = [i // self.w for i in idx]
        return (min(xs), min(ys), max(xs), max(ys))

    def menu_pixels(self):
        return self.n(MENU_SEL) + self.n(MENU_ROW) + self.n(MENU_FG)


def splash_cfg(extra, timeout=0):
    return (f"timeout {timeout}\n{extra}\nentry current\n  title Current system\n  kernel /{{tree}}/vmlinuz\n  initrd /{{tree}}/initrd.img\n"
            "  cmdline console=ttyS0\n  default\nentry seed\n  title Seed system\n  kernel /seed/vmlinuz\n")


def t_splash(s):
    logo = os.path.join(s.dir, "logo.bmp")
    make_bmp(logo, 200, 100, lambda x, y: RED if x < 100 else GREEN)
    big = os.path.join(s.dir, "big.bmp")
    make_bmp(big, 2000, 1000, lambda x, y: YELLOW, bits=32, top_down=True)
    junk = os.path.join(s.dir, "junk.bmp")
    with open(junk, "wb") as f:
        f.write(b"BM" + bytes(range(256)) * 3)
    cut = os.path.join(s.dir, "cut.bmp")
    with open(logo, "rb") as f, open(cut, "wb") as g:
        g.write(f.read()[:5000])
    bg = "splash-bg #204080\n"
    files = s.boot_files() + s.trees() + [(logo, "/logo.bmp"), (big, "/big.bmp"), (junk, "/junk.bmp"), (cut, "/cut.bmp")]
    gfx = dict(vga=True, extra=["-device", "qemu-xhci", "-device", "usb-mouse"])

    def disk(name, cfg):
        return s.disk(name, files, [(cfg, "/boot.cfg")])

    # 1. normal timed start: only the splash, centred, no menu text, held for splash-ms, then the kernel starts
    vm = s.vm("h1", [(disk("h1", splash_cfg("splash /logo.bmp\n" + bg + "splash-ms 4000")), True)], **gfx)
    try:
        need(vm.wait_for(r"splash: drawn /logo.bmp 200x100 as 200x100 at (\d+),(\d+) on (\d+)x(\d+)", 60), "splash not drawn:\n" + vm.text()[-1200:])
        sh = Shot(vm, os.path.join(s.dir, "h1.ppm"))
        x0, y0 = (sh.w - 200) // 2, (sh.h - 100) // 2
        print(f"      screen {sh.w}x{sh.h}: red {sh.n(RED)}, green {sh.n(GREEN)}, box {sh.box}", flush=True)
        need(sh.box == (x0, y0, x0 + 199, y0 + 99), f"image not centred: box {sh.box}, wanted {(x0, y0, x0 + 199, y0 + 99)}")
        need(sh.n(RED) == 10000 and sh.n(GREEN) == 10000, "image pixels wrong (bottom-up BMP read upside down or scaled?)")
        need(sh.n(SPLASH_BG) == sh.w * sh.h - 20000, "something else is on the screen besides background and image")
        need(sh.menu_pixels() == 0, "menu pixels on the screen during a normal start")
        t = vm.text()
        need("menu: open" not in t and "start: current" not in t, "menu opened or the kernel started during the hold:\n" + t[-800:])
        need(vm.wait_for(r"TEST-EFI started path=\\current", 30), "kernel did not start after the splash")
        t = vm.text()
        # (the stand-in kernel returns and the loader then opens its "came back" menu; only what came before the kernel counts)
        need("splash: holding for" in t and "menu: open" not in t.split("TEST-EFI started")[0], "no hold line, or a menu opened before the kernel")
        need(len(re.findall(r"decide: start current \(default\)", t)) == 1, "no default decision")
    finally:
        vm.stop()
    # 2. countdown (timeout 30): splash only; a key shows the menu on the same background; the key itself is not an action
    vm = s.vm("h2", [(disk("h2", splash_cfg("splash /logo.bmp\n" + bg, timeout=30)), True)], **gfx)
    try:
        need(vm.wait_for(r"menu: open", 60), "menu object not opened:\n" + vm.text()[-1200:])
        sh = Shot(vm, os.path.join(s.dir, "h2a.ppm"))
        need(sh.menu_pixels() == 0 and sh.n(RED) == 10000, "the countdown showed the menu instead of the splash")
        vm.key("down")
        need(vm.wait_for(r"menu: shown by a key", 10), "a key did not show the menu")
        time.sleep(0.7)
        sh = Shot(vm, os.path.join(s.dir, "h2b.ppm"))
        print(f"      menu over splash: selected {sh.n(MENU_SEL)}, text {sh.n(MENU_FG)}, background {sh.n(SPLASH_BG)}, red {sh.n(RED)}", flush=True)
        need(sh.n(MENU_SEL) > sh.w * 20 and sh.n(MENU_FG) > 100, "menu not drawn after the key")
        need(sh.n(SPLASH_BG) > sh.w * sh.h // 3 and sh.n(MENU_BG) == 0, "menu not drawn on the splash background")
        logo, sel = sh.bbox(RED, GREEN), sh.bbox(MENU_SEL)
        need(sh.n(RED) == 10000 and sh.n(GREEN) == 10000, "the logo is not whole on the screen next to the menu")
        need(logo[3] < sel[1], f"the menu is not below the logo: logo {logo}, selected row {sel}")
        need("menu: chosen" not in vm.text(), "the revealing key also chose something")
        vm.key("down")
        time.sleep(0.5)
        vm.key("ret")
        need(vm.wait_for(r"menu: chosen seed\b", 20), "the menu does not work after being revealed")
        need(vm.wait_for(r"TEST-EFI started path=\\seed", 30), "seed did not start")
    finally:
        vm.stop()
    # 3. countdown with no key: never drawn, default starts
    vm = s.vm("h3", [(disk("h3", splash_cfg("splash /logo.bmp\n" + bg, timeout=3)), True)], **gfx)
    try:
        need(vm.wait_for(r"splash: drawn", 60), "no splash")
        sh = Shot(vm, os.path.join(s.dir, "h3.ppm"))
        need(sh.menu_pixels() == 0 and sh.n(RED) == 10000, "menu drawn during a silent countdown")
        need(vm.wait_for(r"menu: chosen current \(countdown\)", 30), "countdown did not choose the default")
        need(vm.wait_for(r"TEST-EFI started path=\\current", 30), "kernel did not start")
    finally:
        vm.stop()
    # 4. menu requested by a key at start (Esc): menu at once, over the splash background
    vm = s.vm("h4", [(disk("h4", splash_cfg("splash /logo.bmp\n" + bg, timeout=0)), True)], **gfx)
    try:
        need(vm.wait_for(r"decide: menu", 60, spam="esc"), "Esc did not ask for the menu:\n" + vm.text()[-800:])
        time.sleep(1)
        sh = Shot(vm, os.path.join(s.dir, "h4.ppm"))
        need(sh.n(MENU_SEL) > sh.w * 20 and sh.n(SPLASH_BG) > sh.w * sh.h // 3 and sh.n(MENU_BG) == 0, "requested menu not on the splash background")
    finally:
        vm.stop()
    # 5. a picture bigger than the screen is scaled down to fit, aspect kept, centred
    vm = s.vm("h5", [(disk("h5", splash_cfg("splash /big.bmp\n" + bg + "splash-ms 3000")), True)], **gfx)
    try:
        m = vm.wait_for(r"splash: drawn /big.bmp 2000x1000 as (\d+)x(\d+) at (\d+),(\d+) on (\d+)x(\d+)", 60)
        need(m, "big splash not drawn:\n" + vm.text()[-1200:])
        nw, nh, x, y, sw, sh_ = map(int, m.groups())
        mg = min(sw, sh_) // 16  # the picture keeps a margin of 1/16 of the shorter side on every edge (Image::margin_box)
        mw, mh = sw - 2 * mg, sh_ - 2 * mg
        need((nw, nh) == ((mw, mw // 2) if mw * 1000 <= mh * 2000 else (mh * 2, mh)), f"wrong fitted size {nw}x{nh} on {sw}x{sh_}")
        sh = Shot(vm, os.path.join(s.dir, "h5.ppm"))
        need(sh.box == (x, y, x + nw - 1, y + nh - 1) and sh.n(YELLOW) == nw * nh, f"scaled picture wrong: box {sh.box}, {sh.n(YELLOW)} pixels")
        need((sw - nw) // 2 == x and (sh_ - nh) // 2 == y, "scaled picture not centred")
    finally:
        vm.stop()
    # 6. missing, junk and cut-short files: no crash, loader goes on as without a splash (menu in the normal colours)
    for tag, path, why in (("h6a", "/nope.bmp", "cannot be read"), ("h6b", "/junk.bmp", "unsupported BMP|not a BMP|cut short|out of range"),
                           ("h6c", "/cut.bmp", "cut short")):
        vm = s.vm(tag, [(disk(tag, splash_cfg(f"splash {path}\n" + bg, timeout=30)), True)], **gfx)
        try:
            need(vm.wait_for(rf"splash: {re.escape(path)}.*({why}).*skipped", 60), f"{path}: no skip line:\n" + vm.text()[-900:])
            need(vm.wait_for(r"menu: open", 30), f"{path}: menu did not open")
            time.sleep(1.5)
            sh = Shot(vm, os.path.join(s.dir, tag + ".ppm"), bg=MENU_BG)
            need(sh.n(MENU_SEL) > sh.w * 40 and sh.n(MENU_BG) > sh.w * sh.h // 3 and sh.n(SPLASH_BG) == 0, f"{path}: menu is not the normal one")
            vm.key("ret")
            need(vm.wait_for(r"TEST-EFI started path=\\current", 30), f"{path}: kernel did not start after the fallback")
        finally:
            vm.stop()
    # 7. no graphics output at all: splash skipped, text behaviour as before
    vm = s.vm("h7", [(disk("h7", splash_cfg("splash /logo.bmp\n" + bg)), True)])
    try:
        need(vm.wait_for(r"splash: no graphics output; skipped", 60), "no skip line without a graphics output:\n" + vm.text()[-800:])
        need(vm.wait_for(r"TEST-EFI started path=\\current", 30), "kernel did not start without a graphics output")
    finally:
        vm.stop()


def t_levels(s):
    """The menu under the logo at 1280x800 and, when the firmware offers the mode, at 1920x1080 and 640x480."""
    t_levels_at(s, "i", None)
    for w, h in ((1920, 1080), (640, 480)):
        try:
            t_levels_at(s, f"i{w}", (w, h))
        except Skip as e:
            print(f"      {e}", flush=True)


def t_levels_at(s, name, res):
    """The menu under the logo: first level (default + More options), second level, Esc back, countdown line, no key = logo only."""
    logo = os.path.join(s.dir, "logo2.bmp")
    make_bmp(logo, 600, 300, lambda x, y: RED if (100 <= x < 300 and 100 <= y < 200) else (GREEN if (300 <= x < 500 and 100 <= y < 200) else SPLASH_BG))
    cfg = ("timeout 30\nsplash /logo.bmp\nsplash-bg #204080\nscan no\n"
           "entry current\n  title IYAKKI\n  kernel /{tree}/vmlinuz\n  initrd /{tree}/initrd.img\n  cmdline console=ttyS0\n  default\n"
           "entry safe\n  title IYAKKI (safe graphics)\n  kernel /seed/vmlinuz\n  more\n"
           "entry prev\n  title Previous version\n  kind chain\n  kernel /EFI/BOOT/BOOTX64.EFI\n  requires /EFI/nothing/loader.prev.efi\n  more\n"
           "entry rescue\n  title Recovery\n  kernel /rescue/vmlinuz\n  more\n")
    d = s.disk(name, s.boot_files() + s.trees() + [(logo, "/logo.bmp")], [(cfg, "/boot.cfg")])
    if res:
        vm = s.vm(name, [(d, True)], vga=False, extra=["-device", f"VGA,edid=on,xres={res[0]},yres={res[1]}", "-device", "qemu-xhci", "-device", "usb-mouse"])
    else:
        vm = s.vm(name, [(d, True)], vga=True, extra=["-device", "qemu-xhci", "-device", "usb-mouse"])
    shots = os.path.join(s.dir, "levels-" + name)
    os.makedirs(shots, exist_ok=True)
    try:
        m = vm.wait_for(r"splash: drawn .* on (\d+)x(\d+)", 60)
        need(m, "no splash")
        if res and (int(m.group(1)), int(m.group(2))) != res:
            raise Skip(f"the firmware did not offer {res[0]}x{res[1]} (it chose {m.group(1)}x{m.group(2)})")
        need(vm.wait_for(r"menu: open \(under the logo\)", 30), "the menu did not open in the under-the-logo layout:\n" + vm.text()[-800:])
        need("entries left out" in vm.text(), "the entry whose `requires` file is missing was not left out")
        # 1. silent countdown: the logo only (this is what a person sees when no key is pressed)
        whole = res is None or res[0] >= 1280  # on a small screen the picture is scaled down, so only "is there" can be checked
        full = lambda sh: (sh.n(RED) == 200 * 100 and sh.n(GREEN) == 200 * 100) if whole else (sh.n(RED) > 1000 and sh.n(GREEN) > 1000)
        sh0 = Shot(vm, os.path.join(shots, "1-logo-only.ppm"))
        need(sh0.menu_pixels() == 0 and full(sh0), "countdown without a key did not show the logo only")
        # 2. a key: first level, logo on top, rows below it
        vm.key("down")
        need(vm.wait_for(r"menu: shown by a key", 10), "a key did not show the menu")
        time.sleep(0.8)
        sh1 = Shot(vm, os.path.join(shots, "2-first-level.ppm"))
        logo_box, sel = sh1.bbox(RED, GREEN), sh1.bbox(MENU_SEL)
        print(f"      first level on {sh1.w}x{sh1.h}: logo {logo_box}, selected row {sel}", flush=True)
        need(full(sh1), "the logo is not whole next to the menu")
        need(logo_box[3] < sel[1], "the first level is not below the logo")
        need(logo_box[1] < sh1.h // 4 and logo_box[1] < (sh1.h - 100) // 2, "the logo did not move up for the menu")
        text = sh1.bbox(MENU_FG)
        need(text is not None and sh1.n(MENU_FG) > 300, "no text on the first level")
        # 3. the countdown stopped on the key; Down -> More options, Enter opens the second level
        vm.key("down")
        time.sleep(0.5)
        sh1b = Shot(vm, os.path.join(shots, "3-more-selected.ppm"))
        need(sh1b.bbox(MENU_SEL)[1] > sel[1], "Down did not move the highlight to the second row")
        vm.key("ret")
        need(vm.wait_for(r"menu: more options", 10), "Enter on More options did not open the second level")
        time.sleep(0.8)
        sh2 = Shot(vm, os.path.join(shots, "4-more-level.ppm"))
        need(sh2.bbox(RED, GREEN) == logo_box, "the logo moved between the two levels")
        need(sh2.bbox(MENU_SEL)[1] > logo_box[3], "second level not below the logo")
        # 4. Esc comes back, Enter again, choose "IYAKKI (safe graphics)" (Back is row 0, safe is row 1)
        vm.key("esc")
        need(vm.wait_for(r"menu: back", 10), "Esc did not go back")
        time.sleep(0.5)
        seen = len(vm.text())
        vm.key("ret")
        need(vm.wait_for(r"menu: more options", 10, after=seen), "could not open the second level a second time")
        time.sleep(0.3)
        vm.key("ret")
        need(vm.wait_for(r"menu: chosen safe\b", 20), "Enter on the first entry of the second level did not choose it:\n" + vm.text()[-600:])
        need(vm.wait_for(r"TEST-EFI started path=\\seed", 30), "the chosen entry did not start")
    finally:
        vm.stop()


def t_confirm(s):
    """A start the running system confirms (it writes BootConfirm) never spends the tries; one it does not confirm does."""
    cfg = "timeout 0\ntries 3\nentry current\n  title Current system\n  kernel /{tree}/vmlinuz\n  initrd /{tree}/initrd.img\n  cmdline console=ttyS0 reset confirm\n  default\n"
    d = s.disk("j", s.boot_files() + s.trees(), [(cfg, "/boot.cfg")])
    vm = s.vm("j", [(d, True)])
    try:
        end = time.time() + 150
        while time.time() < end and vm.text().count("state: tree=") < 8:
            time.sleep(0.5)
        t = vm.text()
        need(t.count("state: tree=") >= 8, "fewer than 8 starts in time:\n" + t[-800:])
        trees = re.findall(r"state: tree=(\w+) tries_left=(\d+)", t)
        print(f"      starts: {trees[:8]}", flush=True)
        need("TEST-EFI confirm written: ok" in t, "the stand-in kernel could not write the confirm variable")
        need(trees[0] == ("current", "2") and all(x == ("current", "2") for x in trees[1:8]),
             f"a confirmed start spent a try (without confirmation 3 starts would use them up): {trees[:8]}")
        need(t.count("the last start was confirmed good") >= 7, "the loader did not read the confirmation")
        need("decide: menu (waiting)" not in t, "the loader stopped although every start was confirmed")
    finally:
        vm.stop()


def main():
    p = argparse.ArgumentParser()
    p.add_argument("--boot-efi", required=True)
    p.add_argument("--test-efi", required=True)
    p.add_argument("--qemu", default="qemu-system-x86_64")
    p.add_argument("--ovmf-code", default="/usr/share/OVMF/OVMF_CODE_4M.fd")
    p.add_argument("--ovmf-vars", default="/usr/share/OVMF/OVMF_VARS_4M.fd")
    p.add_argument("--scratch", required=True)
    p.add_argument("--lean-image", default="")
    p.add_argument("--initrd-marker", default="", help="regex of a line the initrd's init prints (real-kernel test)")
    p.add_argument("--only", default="")
    p.add_argument("--keep", action="store_true")
    a = p.parse_args()
    for f in (a.boot_efi, a.test_efi, a.ovmf_code, a.ovmf_vars):
        if not os.path.isfile(f):
            raise SystemExit(f"missing file: {f}")
    s = Suite(a)
    tests = [("a. default entry starts, command line and initrd arrive", t_default),
             ("a2. real kernel + initrd start under the loader", t_real_kernel),
             ("b. menu (Down+Enter), countdown and hold key choose entries", t_menu_and_hold),
             ("c. missing / broken config falls back to built-in entries", t_fallback),
             ("d. boot tries: 3 current, 3 previous, then wait with the reason", t_tries),
             ("e. second disk: bootmgfw.efi found in the menu and chain-loaded", t_chain),
             ("f. graphics menu draws, keyboard and mouse work", t_graphics),
             ("g. Secure Boot firmware refuses the unsigned loader (negative check)", t_secure_boot),
             ("h. splash: centred, no menu on a normal start, menu on the same background, bad files fall back", t_splash),
             ("j. a start the running system confirms never spends the tries", t_confirm),
             ("i. menu under the logo: first level, More options, Esc back, countdown, logo only without a key", t_levels)]
    try:
        for name, fn in tests:
            if a.only and not name.startswith(a.only + "."):
                continue
            s.run(name, lambda fn=fn: fn(s))
    finally:
        s.done()
    bad = [r for r in s.results if r[1] is False]
    skipped = [r for r in s.results if r[1] is None]
    print(f"\n{len(s.results) - len(bad) - len(skipped)} passed, {len(bad)} failed, {len(skipped)} skipped")
    sys.exit(1 if bad else 0)


if __name__ == "__main__":
    main()
