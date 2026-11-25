"""One QEMU boot of an existing IYAKKI image, shared by many named checks (dist/qemu/session-checks.py).

The guest is started like a machine, a scratch account is made by typing, developer mode is switched on from Settings, this computer is
paired (the code read off the guest's screen) and the shell approval is answered on the guest's own screen with the 8 hour key. Then a check
can look at the guest three ways: the serial console log (`ctx.serial()`), the developer-mode API (`ctx.dev(...)`, the `iyakki dev` client)
and commands run in the guest (`ctx.sh(...)`, `iyakki dev shell`). It reuses dist/devtest/qemu_guest.py for the boot and the pairing.
Nothing here touches a real machine: every address is 127.0.0.1 and the guest is deleted at the end.
"""
import os, re, select, shutil, socket, subprocess, sys, time, pty

HERE = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, os.path.join(HERE, "..", "devtest"))
import devui, qemu_guest  # noqa: E402


class SessionGuest(qemu_guest.Guest):
    """The devtest guest, with an optional extra QEMU argument list and no data disk when iyakki-usb is not at hand."""

    # 4096 MB (was 3072): the unpacked root of a pc image is 1.3 GB and the 2026-11 program is 150 MB; the scratch account then ran out of space ("storage error: No space left", Q1)
    def __init__(self, build_dir, client, extra_qemu=None, append="", data_disk=False, ram=4096, disk_gb=2, audio_wav=None):
        super().__init__(build_dir, client)
        self.extra, self.append, self.data_disk, self.ram, self.disk_gb = extra_qemu or [], append, data_disk, ram, disk_gb
        self.disk = False
        # audio_wav: a path, then QEMU records the guest's playback there (-audiodev wav, the sound verbs of dist/autotest judge it);
        # None keeps the default, the sound device with no output (-audiodev none)
        self.audio_wav = audio_wav

    def _start(self):
        usb = os.environ.get("IYAKKI_USB") or os.path.join(os.path.dirname(self.client), "iyakki-usb")
        out = self.out
        for f in ("bzImage", "rootfs.cpio.zst"):
            if not os.path.isfile(os.path.join(out, "images", f)):
                raise devui.DevError("no %s in %s/images" % (f, out))
        mon = os.path.join(self.work, "mon.sock")
        drive = []
        if self.data_disk:
            # an ext4 volume labelled IYAKKI-DATA on a virtio disk: the core mounts it as the data volume (kept across a restart of the guest)
            disk = os.path.join(self.work, "data.img")
            if not os.path.exists(disk):
                subprocess.run(["truncate", "-s", "%dG" % self.disk_gb, disk], check=True)
                subprocess.run(["mkfs.ext4", "-q", "-F", "-L", "IYAKKI-DATA", disk], check=True)
            drive = ["-drive", "file=%s,format=raw,if=virtio" % disk]
            self.disk = True
        elif os.environ.get("IYAKKI_SESSION_INSTALL_DISK") and os.access(usb, os.X_OK) and os.path.isfile(os.path.join(out, "images", "iyakki-install-x86_64.img")):
            disk = os.path.join(self.work, "disk.img")
            subprocess.run(["truncate", "-s", "1G", disk], check=True)
            r = subprocess.run([usb, os.path.join(out, "images", "iyakki-install-x86_64.img"), "--target-file", disk, "--erase", "testfile", "--table", "mbr"], capture_output=True, text=True)
            if r.returncode:
                raise devui.DevError("iyakki-usb: " + r.stderr[-300:])
            drive = ["-drive", "file=%s,format=raw,if=virtio" % disk]
            self.disk = True
        else:
            self.disk = False  # no data volume: the session lives in RAM (the checks that need a data volume say so and are skipped)
        cmd = ["qemu-system-x86_64", "-accel", "kvm", "-accel", "tcg", "-m", str(self.ram), "-smp", "2", "-display", "none", "-vga", "std", "-no-reboot", "-audiodev", ("wav,id=snd0,path=" + self.audio_wav) if self.audio_wav else "none,id=snd0",
               "-smbios", "type=1,manufacturer=Acme,product=Example Laptop 7,version=1.0,serial=SECRET-SERIAL-1", "-device", "virtio-keyboard-pci",
               "-device", "intel-hda", "-device", "hda-duplex,audiodev=snd0",
               "-kernel", os.path.join(out, "images", "bzImage"), "-initrd", os.path.join(out, "images", "rootfs.cpio.zst"),
               "-append", ("console=ttyS0 panic=-1 " + ("iyakki.disk=vda " if self.disk and not self.data_disk else "") + self.append).strip()] + drive + [
               "-netdev", "user,id=n0,hostfwd=tcp:127.0.0.1:%d-:7979" % self.port, "-device", "e1000e,netdev=n0",
               "-serial", "file:" + os.path.join(self.work, "ser.log"), "-monitor", "unix:%s,server,nowait" % mon] + self.extra
        # the shared Vm (dist/qemu/dev-pair-test.py) runs this command line, so the keys, screenshots and pairing steps of qemu_guest.Guest work on it
        if self.vm is None:
            self.pair_script = qemu_guest.load_pair_script()
            self.vm = self.pair_script.Vm(out, usb, sound=True, tag="qemu-guest", port=self.port, tmpdir=self.work)
            shutil.rmtree(self.vm.work, ignore_errors=True)
            self.vm.work = self.work
        self.vm.cmdline_fn = lambda: cmd
        self.vm.boot()
        if self.vm.mon is None:
            raise devui.DevError("QEMU monitor did not come up")

    @property
    def mon(self):
        return self.vm.mon if self.vm else None


CODE_ALPHABET = "ABCDEFGHJKMNPQRSTUVWXYZ23456789"   # crates/iyakki-agent/src/devmode/auth.rs: no I, L, O, 0, 1
_LOOKALIKE = {"I": "JT", "1": "7J", "L": "E", "O": "DQ", "0": "DQ", "5": "S", "8": "B", "J": "T", "U": "V", "V": "U", "M": "N", "N": "M", "S": "5", "B": "8", "Z": "2", "2": "Z", "D": "Q", "Q": "D"}


def _variants(code, limit=5):
    """The pairing code read by OCR may mix up look-alike characters. The device allows five tries in all, so only the likeliest readings are returned: the reading itself
    when every character is in the code alphabet, then the readings that change the characters outside the alphabet, then a single look-alike swap, nearest first."""
    code = code.upper()
    out = []

    def add(c):
        if c not in out and len(out) < limit and all(ch in CODE_ALPHABET or ch == "-" for ch in c):
            out.append(c)
    base = [i for i, ch in enumerate(code) if ch not in CODE_ALPHABET and ch != "-"]
    first = list(code)
    for i in base:                       # a character the device never prints: its most likely look-alike
        first[i] = (_LOOKALIKE.get(code[i], "") + "?")[0]
    add("".join(first))
    for i in base:
        for alt in _LOOKALIKE.get(code[i], ""):
            c = list(first); c[i] = alt; add("".join(c))
    for i, ch in enumerate(code):
        for alt in _LOOKALIKE.get(ch, ""):
            c = list(first); c[i] = alt; add("".join(c))
    return out


def _read_code_zoomed(png):
    """The pairing code read twice as carefully: find the word that looks like a code, cut it out, enlarge it three times, and read that line alone with only the code alphabet allowed.
    None when no such word is on the screen (the caller falls back to the plain reading)."""
    try:
        from PIL import Image, ImageOps
    except ImportError:
        return None
    tsv = subprocess.run(["tesseract", png, "-", "--psm", "11", "tsv"], capture_output=True, text=True).stdout.splitlines()
    for line in tsv[1:]:
        f = line.split("\t")
        if len(f) < 12 or not re.fullmatch(r"[A-Z0-9]{4,5}[-\u2013][A-Z0-9]{4,5}", f[11].strip()):
            continue
        l, t, w, h = (int(x) for x in f[6:10])
        im = Image.open(png).convert("L").crop((max(0, l - 12), max(0, t - 8), l + w + 12, t + h + 8))
        im = ImageOps.autocontrast(ImageOps.invert(im.resize((im.width * 3, im.height * 3), Image.LANCZOS)))
        z = os.path.join(os.path.dirname(png), "code-zoom.png")
        im.save(z)
        said = subprocess.run(["tesseract", z, "-", "--psm", "7", "-c", "tessedit_char_whitelist=%s-" % CODE_ALPHABET], capture_output=True, text=True).stdout
        m = re.search(r"([A-Z0-9]{5})-([A-Z0-9]{5})", said.replace(" ", ""))
        if m:
            return m.group(1) + "-" + m.group(2)
    return None


def _pair_by_screen_retrying(self):
    """qemu_guest.Guest._pair_by_screen with retries: the code is read off the screen by OCR, a wrong reading is tried again with look-alike characters."""
    g = self
    g._cmd("sendkey s", 2.0)
    g._cmd("sendkey slash", 0.5); g._cmd("sendkey ctrl-a", 0.2); g.vm.typ("developer mode"); g._cmd("sendkey down", 0.4); g._cmd("sendkey ret", 1.8)
    for _try in range(4):
        g._cmd("sendkey n", 1.0)               # the time-zone card, if it is up, would swallow the next key
        g._cmd("sendkey e", 2.5)
        png = g.shot("pair-on")
        said = subprocess.run(["tesseract", png, "-", "--psm", "11"], capture_output=True, text=True).stdout
        if re.search(r"Developer mode: On|switches off in", said, re.I):
            break
    g._cmd("sendkey p", 2.0)
    tried, allowed = set(), False
    for attempt in range(8):
        found = None
        for psm in ("11", "6"):
            png = g.shot("pair-code")
            if os.environ.get("SESSION_DEBUG_DIR"):
                shutil.copy(png, os.path.join(os.environ["SESSION_DEBUG_DIR"], "pair-%d-%s.png" % (attempt, psm)))
            txt = subprocess.run(["tesseract", png, "-", "--psm", psm], capture_output=True, text=True).stdout
            if os.environ.get("SESSION_DEBUG_DIR"):
                open(os.path.join(os.environ["SESSION_DEBUG_DIR"], "pair-%d-%s.txt" % (attempt, psm)), "w").write(txt)
            m = re.search(r"\b([A-Z0-9]{4,5}) ?[-\u2013] ?([A-Z0-9]{4,5})\b", txt)
            if m:
                found = m.group(1) + "-" + m.group(2)
                found = _read_code_zoomed(png) or found
                break
        if not found:
            g._cmd("sendkey n", 1.5)          # the time-zone card, if it covers the page
            if attempt % 3 == 2:
                g._cmd("sendkey p", 2.0)      # `p` toggles the pairing code: only after two empty looks
            continue
        if not allowed:
            for _try in range(3):
                g._cmd("sendkey f", 3.0)      # the page's "allow this computer through the firewall" button, before pairing
                png = g.shot("pair-allow")
                said = subprocess.run(["tesseract", png, "-", "--psm", "11"], capture_output=True, text=True).stdout
                if re.search(r"lets the local network|for ten minutes|until a computer has paired", said, re.I):
                    break
                g._cmd("sendkey n", 1.0)      # the time-zone card may have taken the key
            allowed = True
        for cand in _variants(found):
            if cand in tried:
                continue
            tried.add(cand)
            for _wait in range(10):
                r = subprocess.run([g.client, "dev", "pair", "127.0.0.1:%d" % g.port, "--code", cand, "--as", g.device_name, "--home", g.home], capture_output=True, text=True, timeout=90)
                if r.returncode and re.search(r"temporarily unavailable|refused|timed out|reset", r.stdout + r.stderr):
                    if _wait in (2, 5):
                        g._cmd("sendkey f", 3.0)   # the firewall allow did not take: press it again (the code is still on the page)
                    time.sleep(4)   # the developer service (or its firewall rule) is not up yet
                    continue
                break
            if r.returncode == 0:
                g._cmd("sendkey g", 0.2)
                g._cmd("sendkey shift-g", 2.0)
                g.paired = True
                for _ in range(3):
                    g._cmd("sendkey f11", 1.0)
                return
            if "no pairing code is open" in (r.stdout + r.stderr):
                g._cmd("sendkey p", 2.0)
                tried.discard(cand)
                break
            if "wrong code" not in (r.stdout + r.stderr) and "tries left" not in (r.stdout + r.stderr):
                raise devui.DevError("pairing failed: " + (r.stdout + r.stderr)[-300:])
            if len(tried) % 4 == 0:
                break
    raise devui.DevError("pairing code could not be read or accepted")


SessionGuest._pair_by_screen = _pair_by_screen_retrying


class Session:
    """What a check gets: serial log, dev API, shell commands in the guest."""

    def __init__(self, build_dir, client, log_dir, keep=False, ui_process=False, **kw):
        # ui_process: the UI runs as its own program (F-SPLIT-03, docs/development/UI-BOUNDARY.md "Booted proof"). The Settings page that switches developer
        # mode on and shows the pairing code is not served over the UI socket, so the guest is paired in the usual mode on a data volume and then restarted with
        # `iyakki.ui=process` on the SAME volume (the pairing, the grant and developer mode are kept); `iyakki.ssh=1` gives the checks a root shell (key in
        # $IYAKKI_QEMU_ROOT_KEY, its public half in the image's /etc/iyakki/authorized_keys) for what the dev shell's unprivileged account cannot do.
        if ui_process:
            kw["data_disk"] = True
        self.ui_process = ui_process
        self.ssh_port = None
        self.guest = SessionGuest(build_dir, client, **kw)
        self.client, self.log_dir = client, log_dir
        self.shell_ok = False
        if keep:
            os.environ["IYAKKI_DEVTEST_KEEP"] = "1"

    def boot_serial_only(self, wait_for="supervisor up", secs=120):
        """Start the guest and only watch its serial console (no sign-in, no pairing): for checks that read the boot log."""
        g = self.guest
        g._start()
        end = time.time() + secs
        while time.time() < end:
            if wait_for in self.serial():
                time.sleep(15)       # let the first services log their lines
                return self
            time.sleep(2)
        raise devui.DevError("the guest console never said %r" % wait_for)

    def boot(self):
        g = self.guest
        g.boot_and_pair()
        if not g.paired:
            raise devui.DevError("the guest booted but could not be paired (tesseract missing or the pairing code was not read)")
        self.approve_shell()
        if self.ui_process:
            self.switch_to_ui_process()
        return self

    def switch_to_ui_process(self):
        """Restart the guest (same data volume) with the UI as its own program and a root SSH door for the checks that need one."""
        g = self.guest
        g.append = (g.append + " iyakki.ui=process iyakki.ssh=1").strip()
        if not self.restart_guest(qemu_guest.SCRATCH_PASSWORD):
            raise devui.DevError("the guest did not come back with developer mode on after the restart into iyakki.ui=process")
        # restart_guest leaves Settings open on the Developer mode page with its search text (the same clean-up as the end of approve_shell)
        g._cmd("sendkey n", 1.0); g._cmd("sendkey slash", 1.0); g._cmd("sendkey ctrl-a", 0.5); g._cmd("sendkey delete", 1.0); g._cmd("sendkey esc", 0.5)
        for _ in range(3):
            g._cmd("sendkey f11", 1.0)
        self.shell_ok = True            # the 8 hour grant is on the data volume
        if os.environ.get("IYAKKI_QEMU_ROOT_KEY"):
            self.ssh_port = g.port + 1
            g._cmd("hostfwd_add tcp:127.0.0.1:%d-:22" % self.ssh_port, 0.5)
            end = time.time() + 90
            while time.time() < end and self.root("true")[0] != 0:
                time.sleep(3)

    def root(self, command, timeout=60):
        """One command line as root in the guest over SSH (only with $IYAKKI_QEMU_ROOT_KEY and a guest booted with iyakki.ssh=1); returns (exit code, output)."""
        key = os.environ.get("IYAKKI_QEMU_ROOT_KEY")
        if not key or not self.ssh_port:
            raise devui.Skip("no root door: set IYAKKI_QEMU_ROOT_KEY (private key whose public half is the image's /etc/iyakki/authorized_keys) and run with ui_process")
        try:
            r = subprocess.run(["ssh", "-p", str(self.ssh_port), "-i", key, "-o", "BatchMode=yes", "-o", "StrictHostKeyChecking=no", "-o", "UserKnownHostsFile=/dev/null", "-o", "LogLevel=ERROR",
                                "-o", "ConnectTimeout=5", "root@127.0.0.1", command], capture_output=True, text=True, timeout=timeout, stdin=subprocess.DEVNULL)
        except subprocess.TimeoutExpired:
            return 124, "timed out after %ss" % timeout
        return r.returncode, r.stdout + r.stderr

    @property
    def home(self):
        return self.guest.home

    def serial(self):
        try:
            return open(os.path.join(self.guest.work, "ser.log"), "rb").read().decode("utf-8", "replace").replace("\r", "")
        except OSError:
            return ""

    def ui(self):
        """The screen driver of dist/devtest/devui.py for this guest (state, key, text, click, tap_text, wait_for, go_home, screenshot)."""
        if not hasattr(self, "_ui"):
            self._ui = devui.Device(self.guest.device_name, run_dir=self.log_dir, client=self.client, home=self.home, kind="qemu")
        return self._ui

    def key(self, *names, wait=0.4):
        for n in names:
            self.guest._cmd("sendkey " + n, wait)

    def dev(self, *args, timeout=60):
        try:
            r = subprocess.run([self.client, "dev"] + list(args) + ["--home", self.home], capture_output=True, text=True, timeout=timeout)
        except subprocess.TimeoutExpired:
            return 124, "timed out after %ss" % timeout
        return r.returncode, (r.stdout + r.stderr)

    def approve_shell(self):
        """The shell scope asks on the guest's own screen. A request is sent from the host while the guest shows Home (the banner is read with
        `dev state`), then Settings, Developer mode is opened and the request is answered with the 8 hour key."""
        g = self.guest
        g._cmd("sendkey n", 0.6)
        mfd, sfd = pty.openpty()
        p = subprocess.Popen([self.client, "dev", "shell", "--tty", "--home", self.home, "--", "sh", "-c", "echo shell-approved"], stdin=sfd, stdout=sfd, stderr=sfd)
        self.banner = ""
        for _ in range(8):
            time.sleep(2.5)
            rc, st = self.dev("state")
            if "SHELL REQUEST" in st:
                self.banner = st
                break
        g._cmd("sendkey s", 2.0)
        g._cmd("sendkey slash", 0.5); g._cmd("sendkey ctrl-a", 0.2); g.vm.typ("developer mode"); g._cmd("sendkey down", 0.4); g._cmd("sendkey ret", 1.8)
        deadline = time.time() + 90
        while time.time() < deadline and p.poll() is None:
            g._cmd("sendkey 8", 1.0)
            time.sleep(3)
        # Settings keeps its search text ("developer mode", typed above): empty the box and go Home, or every scenario that opens Settings afterwards sees only the
        # Developer mode page ("no button X in this build", Q1 2026-11-21)
        g._cmd("sendkey n", 1.0); g._cmd("sendkey slash", 1.0); g._cmd("sendkey ctrl-a", 0.5); g._cmd("sendkey delete", 1.0); g._cmd("sendkey esc", 0.5)
        for _ in range(3):
            g._cmd("sendkey f11", 1.0)
        out = b""
        while select.select([mfd], [], [], 0.3)[0]:
            try:
                d = os.read(mfd, 65536)
            except OSError:
                break
            if not d:
                break
            out += d
        if p.poll() is None:
            p.kill()
        self.shell_ok = b"shell-approved" in out
        self.approve_log = out.decode("utf-8", "replace")[-400:]
        return self.shell_ok

    def restart_guest(self, password):
        """A hard restart of the guest on the same data volume (QEMU is stopped and started again), then sign in by typing the scratch
        account's password at the sign-in screen and switch developer mode on again from Settings. The paired computer is kept on the data volume."""
        g = self.guest
        self.dev("files", "ls", "/tmp/iyakki-dev")
        self.sh("sync")
        g.qemu.kill(); g.qemu.wait()
        try:
            g.mon.close()
        except OSError:
            pass
        g.n = 0
        g._start()
        time.sleep(1)
        deadline = time.time() + 150
        while time.time() < deadline:
            png = g.shot("restart-wait")
            ocr = subprocess.run(["tesseract", png, "-", "--psm", "11"], capture_output=True, text=True).stdout
            if re.search(r"password|sign in|Welcome", ocr, re.I):
                break
            time.sleep(5)
        g.vm.typ(password)
        g._cmd("sendkey ret", 6.0)
        for _ in range(3):
            g._cmd("sendkey n", 1.0)
        g._cmd("sendkey s", 2.0)
        g._cmd("sendkey slash", 0.5); g._cmd("sendkey ctrl-a", 0.2); g.vm.typ("developer mode"); g._cmd("sendkey down", 0.4); g._cmd("sendkey ret", 1.8)
        for _ in range(4):
            g._cmd("sendkey e", 2.5)
            rc, out = self.dev("whoami", timeout=20)
            if rc == 0 and "developer mode On" in out:
                return True
            g._cmd("sendkey n", 1.0)
        return False

    def sh(self, command, timeout=90):
        """Run one command line in the guest as the account (through `iyakki dev shell`, approved for 8 hours); returns (exit code, output)."""
        if not self.shell_ok:
            raise devui.Skip("the shell approval was not given")
        p = subprocess.Popen([self.client, "dev", "shell", "--home", self.home, "--", "sh", "-c", command], stdin=subprocess.DEVNULL, stdout=subprocess.PIPE, stderr=subprocess.STDOUT)
        try:
            out, _ = p.communicate(timeout=timeout)
        except subprocess.TimeoutExpired:
            p.kill(); out, _ = p.communicate()
            return 124, out.decode("utf-8", "replace")
        return p.returncode, out.decode("utf-8", "replace")

    def stop(self):
        self.guest.stop()
