#!/usr/bin/env python3
"""Secure Boot test for IYAKKI's boot chain in QEMU with OVMF's Secure Boot firmware.  docs/SECURE-BOOT.md, docs/BOOT-TEST.md.

    qemu-secureboot-test.py --kit KITDIR [--buildroot target/buildroot-mac] [--keep]

It makes throwaway keys, stages the real boot files (IYAKKI's GRUB 2.12 built the way the image build does it, the board's real
grub.cfg and theme, shim and MokManager from Debian's pinned packages through `secureboot.sh`), boots them and checks, from the
serial console, each link of the chain:

  1. Secure Boot OFF, plain unsigned files: the default image still boots, nothing is locked down, an unsigned module loads.
  2. Secure Boot ON, the key enrolled as a MOK: shim loads, our signed GRUB loads, the signed kernel boots, "Secure boot enabled"
     and "Kernel is locked down", a module signed with the key loads, an unsigned one and one signed by another key are refused.
  3. Secure Boot ON, MOK list holds ANOTHER key: the boot stops before the kernel, with shim's "Security Violation" message.
  4. Secure Boot ON, nothing enrolled (a fresh machine): same, and MokManager (the blue enrolment screen) is what comes up.
  5. Secure Boot ON, unsigned files (an old stick): the firmware itself refuses.
  6. The real enrolment: nothing enrolled, MokManager is driven through the QEMU monitor's key presses exactly as a person
     does it ("Enroll key from disk" -> EFI -> IYAKKI -> iyakki-mok.cer -> Continue -> Yes -> Reboot), then the next boot
     must reach the kernel, locked down, with the signed module loading.

KITDIR holds what `secureboot-testkit.sh` builds: a small kernel (bzImage, the patch of patches-sb applied, the Secure Boot
options of linux.fragment) and a test module (sbtest.ko), plus the kernel's sign-file.  The image's own kernel is not used because
this PC has no way to build one in minutes; the options that matter are the same.

Tools: qemu-system-x86_64, OVMF 4M files (OVMF_CODE_4M.secboot.fd, OVMF_VARS_4M.ms.fd, OVMF_CODE_4M.fd, OVMF_VARS_4M.fd), sbsign,
openssl, cpio, gzip, a static busybox, and virt-fw-vars (pip: virt-firmware; test only, not shipped) to put the MOK into the
firmware's variable store the way MokManager would.  Missing tools are named and the test exits 2.
"""
import argparse, glob, os, re, shutil, subprocess, sys, tempfile, time

HERE = os.path.dirname(os.path.abspath(__file__))
ROOT = os.path.abspath(os.path.join(HERE, "..", ".."))
SB = os.path.join(HERE, "secureboot.sh")
SB_VAR = "8be4df61-93ca-11d2-aa0d-00e098032b8c"
OWNER_GUID = "0e1b9d4c-7f6a-4e0b-9a55-1c0ffee15b00"  # any GUID names the owner of a MOK entry


def need(tool, hint=""):
    p = shutil.which(tool)
    if not p:
        print(f"SKIP: {tool} is needed {hint}", file=sys.stderr)
        sys.exit(2)
    return p


def run(cmd, **kw):
    r = subprocess.run(cmd, text=True, capture_output=True, **kw)
    if r.returncode != 0:
        print(f"FAIL: {' '.join(map(str, cmd))}\n{r.stdout}{r.stderr}", file=sys.stderr)
        sys.exit(1)
    return r.stdout


def find_first(paths):
    for p in paths:
        for m in sorted(glob.glob(p)):
            if os.path.exists(m):
                return m
    return None


def initramfs(work, name, kit, mods, busybox):
    """A tiny initramfs: busybox, the test modules, an init that reports what the kernel says and tries each module."""
    d = os.path.join(work, "rd-" + name)
    shutil.rmtree(d, ignore_errors=True)
    for sub in ("bin", "proc", "sys", "dev", "m", "tmp", "mnt"):
        os.makedirs(os.path.join(d, sub))
    shutil.copy(busybox, os.path.join(d, "bin", "busybox"))
    for n, path in mods.items():
        shutil.copy(path, os.path.join(d, "m", n + ".ko"))
    init = f"""#!/bin/busybox sh
/bin/busybox --install -s /bin
mount -t proc proc /proc; mount -t sysfs sys /sys; mount -t devtmpfs dev /dev
mount -t securityfs none /sys/kernel/security 2>/dev/null
mount -t efivarfs efivarfs /sys/firmware/efi/efivars 2>/dev/null
echo "IYAKKI-SBTEST begin"
echo "IYAKKI-SBTEST lockdown: $(cat /sys/kernel/security/lockdown 2>&1)"
echo "IYAKKI-SBTEST SecureBoot variable: $(od -An -tx1 /sys/firmware/efi/efivars/SecureBoot-{SB_VAR} 2>&1 | tr -s ' ')"
dmesg | grep -i -E "secure boot|locked down|machine keyring|platform keyring|\\.machine|MOK" | sed 's/^/IYAKKI-SBTEST dmesg: /'
for m in good unsigned other; do
  if insmod /m/$m.ko 2>/tmp/err; then echo "IYAKKI-SBTEST module $m: loaded"; rmmod sbtest 2>/dev/null
  else echo "IYAKKI-SBTEST module $m: refused ($(cat /tmp/err))"; fi
done
if head -c 16 /dev/mem >/dev/null 2>/tmp/err; then echo "IYAKKI-SBTEST /dev/mem: readable"; else echo "IYAKKI-SBTEST /dev/mem: refused ($(cat /tmp/err))"; fi
echo "IYAKKI-SBTEST end"
poweroff -f
"""
    with open(os.path.join(d, "init"), "w") as f:
        f.write(init)
    os.chmod(os.path.join(d, "init"), 0o755)
    out = os.path.join(work, name + ".initrd")
    run(f"cd '{d}' && find . | cpio -o -H newc --quiet | gzip -1 > '{out}'", shell=True)
    return out


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--kit", required=True, help="directory made by secureboot-testkit.sh (bzImage, sbtest.ko, sign-file)")
    ap.add_argument("--buildroot", default=None, help="a Buildroot output directory with grub2 built (default: target/buildroot-*)")
    ap.add_argument("--only", default="", help="comma-separated scenario numbers, e.g. 1,2")
    ap.add_argument("--keep", action="store_true", help="keep the work directory (logs, ESP files)")
    ap.add_argument("--timeout", type=int, default=90)
    a = ap.parse_args()

    qemu = need("qemu-system-x86_64")
    need("sbsign", "(package sbsigntool)"); need("openssl"); need("cpio"); need("gzip")
    busybox = need("busybox", "(a static one: the Debian/Ubuntu busybox-static package)")
    vfv = shutil.which("virt-fw-vars") or find_first([os.path.expanduser("~/.cache/iyakki-tmp/sb/venv/bin/virt-fw-vars")])
    if not vfv:
        print("SKIP: virt-fw-vars is needed (python3 -m venv v && v/bin/pip install virt-firmware)", file=sys.stderr)
        sys.exit(2)
    ovmf = "/usr/share/OVMF"
    fw = {k: os.path.join(ovmf, k) for k in ("OVMF_CODE_4M.secboot.fd", "OVMF_VARS_4M.ms.fd", "OVMF_CODE_4M.fd", "OVMF_VARS_4M.fd")}
    for k, p in fw.items():
        if not os.path.exists(p):
            print(f"SKIP: OVMF file {p} is missing (package ovmf)", file=sys.stderr)
            sys.exit(2)
    kit = os.path.abspath(a.kit)
    for f in ("bzImage", "sbtest.ko", "sign-file"):
        if not os.path.exists(os.path.join(kit, f)):
            print(f"SKIP: {kit}/{f} is missing: run secureboot-testkit.sh", file=sys.stderr)
            sys.exit(2)
    br = a.buildroot or find_first([os.path.join(ROOT, "target/buildroot-*")])
    core = find_first([os.path.join(br or "", "build/grub2-*/build-x86_64-efi/grub-core")]) if br else None
    mkimage = find_first([os.path.join(br or "", "host/bin/grub-mkimage")]) if br else None
    defcfg = os.path.join(br or "", ".config")
    if not (core and mkimage and os.path.exists(defcfg)):
        print("SKIP: a Buildroot output directory with grub2 built is needed (--buildroot); GRUB is built from its modules", file=sys.stderr)
        sys.exit(2)
    mods_efi = re.search(r'^BR2_TARGET_GRUB2_BUILTIN_MODULES_EFI="(.*)"$', open(defcfg).read(), re.M).group(1)

    work = tempfile.mkdtemp(prefix="iyakki-sbtest-", dir=os.environ.get("TMPDIR"))
    env = dict(os.environ, GRUB_CORE_DIR=core, GRUB_EFI_MODULES=mods_efi, HOST_DIR=os.path.join(br, "host"))
    env["IYAKKI_SB_CACHE"] = os.environ.get("IYAKKI_SB_CACHE", os.path.join(work, "dl"))
    print(f"work directory: {work}")
    # ---- keys, GRUB, the kernel and the modules -----------------------------------------------------------------------
    run(["sh", SB, "genkey", f"{work}/good", "IYAKKI test key"]); run(["sh", SB, "genkey", f"{work}/other", "Another key"])
    gk, gc = f"{work}/good/iyakki-sb.key", f"{work}/good/iyakki-sb.crt"
    ok_env = dict(env, IYAKKI_SB_KEY=gk, IYAKKI_SB_CERT=gc)
    grub_unsigned = f"{work}/grub-unsigned.efi"
    run(["sh", SB, "grub-image", grub_unsigned], env=ok_env)
    signed_kernel = f"{work}/bzImage.signed"
    run(["sh", SB, "sign-pe", f"{kit}/bzImage", signed_kernel], env=ok_env)
    mdir = f"{work}/mods"; os.makedirs(mdir)
    shutil.copy(f"{kit}/sbtest.ko", f"{mdir}/unsigned.ko")
    for n, (k, c) in {"good": (gk, gc), "other": (f"{work}/other/iyakki-sb.key", f"{work}/other/iyakki-sb.crt")}.items():
        shutil.copy(f"{kit}/sbtest.ko", f"{mdir}/{n}.ko")
        run([f"{kit}/sign-file", "sha256", k, c, f"{mdir}/{n}.ko"])
    mods = {n: f"{mdir}/{n}.ko" for n in ("good", "unsigned", "other")}
    initrd = initramfs(work, "t", kit, mods, busybox)

    def esp(name, signed):
        """An EFI System Partition directory: the board's real grub.cfg and theme, the kernel, the initramfs, the boot chain."""
        d = f"{work}/esp-{name}"
        shutil.rmtree(d, ignore_errors=True)
        os.makedirs(f"{d}/boot/grub/fonts"); os.makedirs(f"{d}/EFI/BOOT")
        board = os.path.join(HERE, "board/iyakki")
        cfg = open(f"{board}/grub.cfg").read().replace("__KERNEL_PATH__", "/boot/bzImage").replace("__INITRD_PATH__", "/boot/initrd")
        open(f"{d}/boot/grub/grub.cfg", "w").write(cfg)
        shutil.copytree(f"{board}/grub", f"{d}/boot/grub/themes/iyakki", ignore=shutil.ignore_patterns("iyakki-bg.png.x"))
        mk = shutil.which("grub-mkfont") or shutil.which("grub2-mkfont")
        if mk:
            assets = os.path.join(HERE, "../../crates/iyakki-shell/assets")
            run([mk, "-s", "30", "--no-hinting", "--range=0x20-0x7e,0xa0-0x24f", "-o", f"{d}/boot/grub/fonts/iyakki-menu.pf2", f"{assets}/DejaVuSans-Bold.ttf"])
            run([mk, "-s", "24", "-c", "18", "-d", "6", "--range=0x20-0x7e,0xa0-0xff", "-o", f"{d}/boot/grub/fonts/iyakki-term.pf2", f"{assets}/DejaVuSansMono.ttf"])
        shutil.copy(initrd, f"{d}/boot/initrd")
        if signed:
            shutil.copy(signed_kernel, f"{d}/boot/bzImage")
            run(["sh", SB, "stage-esp", d, grub_unsigned], env=ok_env)
            run(["sh", SB, "verify", d, f"{d}/boot/bzImage"], env=ok_env)
        else:
            shutil.copy(f"{kit}/bzImage", f"{d}/boot/bzImage")
            shutil.copy(grub_unsigned, f"{d}/EFI/BOOT/bootx64.efi")
        return d

    def vars_file(name, sb, mok_cert):
        out = f"{work}/vars-{name}.fd"
        if not sb:
            shutil.copy(fw["OVMF_VARS_4M.fd"], out)
        else:
            cmd = [vfv, "-i", fw["OVMF_VARS_4M.ms.fd"], "-o", out, "--secure-boot"]
            if mok_cert:
                cmd += ["--add-mok", OWNER_GUID, mok_cert]
            run(cmd)
        return out

    def boot(name, d, sb, mok_cert, until, timeout):
        log = f"{work}/{name}.log"
        vf = vars_file(name, sb, mok_cert)
        code = fw["OVMF_CODE_4M.secboot.fd"] if sb else fw["OVMF_CODE_4M.fd"]
        cmd = [qemu, "-accel", "kvm", "-accel", "tcg", "-machine", "q35,smm=on" if sb else "q35", "-m", "768", "-smp", "2",
               "-display", "none", "-serial", f"file:{log}", "-no-reboot",
               "-drive", f"if=pflash,format=raw,unit=0,file={code},readonly=on", "-drive", f"if=pflash,format=raw,unit=1,file={vf}",
               "-drive", f"file=fat:rw:{d},format=raw,if=virtio"]
        if sb:
            cmd += ["-global", "driver=cfi.pflash01,property=secure,value=on"]  # the variable store is writable only from firmware (SMM)
        p = subprocess.Popen(cmd, stdout=subprocess.DEVNULL, stderr=subprocess.PIPE)
        end = time.time() + timeout
        text = ""
        while time.time() < end:
            time.sleep(1)
            try:
                text = open(log, "rb").read().decode("latin-1")
            except OSError:
                text = ""
            if until(text) or p.poll() is not None:
                break
        p.kill(); p.wait()
        err = p.stderr.read().decode(errors="replace") if p.stderr else ""
        text = re.sub(r"\x1b\[[0-9;?]*[A-Za-z]", "", text).replace("\r", "")
        if err.strip():
            text += "\n[qemu stderr] " + err
        return text

    def enrol_by_keys(name, d, timeout=120):
        """Drive MokManager with sendkey through a QEMU monitor socket. Returns True when it ended with the reboot."""
        import socket
        log = f"{work}/{name}.log"; open(log, "w").close()
        vf = vars_file(name, True, None); mon = f"{work}/{name}.mon"
        cmd = [qemu, "-accel", "kvm", "-accel", "tcg", "-machine", "q35,smm=on", "-m", "768", "-display", "none", "-serial", f"file:{log}",
               "-no-reboot", "-drive", f"if=pflash,format=raw,unit=0,file={fw['OVMF_CODE_4M.secboot.fd']},readonly=on",
               "-drive", f"if=pflash,format=raw,unit=1,file={vf}", "-drive", f"file=fat:rw:{d},format=raw,if=virtio",
               "-global", "driver=cfi.pflash01,property=secure,value=on", "-monitor", f"unix:{mon},server,nowait"]
        p = subprocess.Popen(cmd, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
        sock = socket.socket(socket.AF_UNIX)
        for _ in range(40):
            try:
                sock.connect(mon); break
            except OSError:
                time.sleep(0.5)

        def screen():
            t = open(log, "rb").read().decode("latin-1")
            return re.sub(r"\s+", " ", re.sub(r"[^\x20-\x7e\n]+", " ", re.sub(r"\x1b\[[0-9;?]*[A-Za-z]", "", t)))

        def wait(text, secs=30):
            end = time.time() + secs
            while time.time() < end:
                if text in screen():
                    return True
                time.sleep(0.5)
            return False

        def keys(*ks):
            for k in ks:
                sock.send(f"sendkey {k}\n".encode()); time.sleep(0.7)
        ok = wait("Security Violation", 60)
        if ok:
            keys("ret")
        steps = [("Press any key to perform MOK management", ["ret"]), ("Enroll key from disk", ["down", "ret"]), ("Select Key", ["ret"]), ("EFI/", ["ret"]),
                 ("IYAKKI/", ["down", "down", "ret"]), ("iyakki-mok.cer", ["down", "ret"]), ("View key 0", ["down", "ret"]),
                 ("Enroll the key(s)?", ["down", "ret"]), ("Reboot", ["ret"])]
        for text, ks in steps:
            if not ok:
                break
            ok = wait(text, 25)
            if not ok:
                print(f"      enrolment stopped waiting for {text!r}; screen: ...{screen()[-300:]}")
            if ok:
                time.sleep(1.5); keys(*ks)
        if ok:
            try:
                p.wait(timeout=30)   # -no-reboot: QEMU ends when the firmware resets
            except subprocess.TimeoutExpired:
                ok = False
        p.kill(); p.wait()
        return ok, vf

    done = lambda t: "IYAKKI-SBTEST end" in t
    results = []

    def check(label, ok, detail=""):
        results.append(ok)
        print(("PASS  " if ok else "FAIL  ") + label + (f"   [{detail}]" if detail and not ok else ""))

    want = {int(x) for x in a.only.split(",") if x} or {1, 2, 3, 4, 5, 6}
    good_cert, other_cert = gc, f"{work}/other/iyakki-sb.crt"
    if 1 in want:
        print("--- 1. Secure Boot OFF, plain unsigned image files (the default image)")
        t = boot("s1", esp("plain", False), False, None, done, a.timeout)
        check("kernel booted", "IYAKKI-SBTEST begin" in t, t[-600:])
        check("Secure boot disabled, no lockdown", "Secure boot disabled" in t and "[none]" in t, t[-600:])
        check("unsigned module loads (driver packs keep working)", "module unsigned: loaded" in t, t[-600:])
    if 2 in want:
        print("--- 2. Secure Boot ON, key enrolled as a MOK: shim -> signed GRUB -> signed kernel")
        t = boot("s2", esp("signed", True), True, good_cert, done, a.timeout)
        check("shim started", "Booting `IYAKKI'" in t or "IYAKKI-SBTEST begin" in t, t[-800:])
        check("kernel booted, firmware says Secure boot enabled", "Secure boot enabled" in t, t[-800:])
        check("kernel locked down (integrity)", re.search(r"lockdown: .*none \[integrity\]|lockdown: none \[integrity\]", t) is not None or "[integrity]" in t, t[-800:])
        check("module signed with the enrolled key loads", "module good: loaded" in t, t[-800:])
        check("unsigned module refused", re.search(r"module unsigned: refused", t) is not None, t[-800:])
        check("module signed with another key refused", re.search(r"module other: refused", t) is not None, t[-800:])
        check("/dev/mem refused under lockdown", "/dev/mem: refused" in t, t[-800:])
        open(f"{work}/s2.summary", "w").write("\n".join(l for l in t.splitlines() if "IYAKKI-SBTEST" in l or "Secure boot" in l or "locked down" in l))
    if 3 in want:
        print("--- 3. Secure Boot ON, a different key is enrolled")
        t = boot("s3", esp("signed", True), True, other_cert, lambda x: "Security Violation" in x or done(x), 40)
        check("the kernel did not start", "IYAKKI-SBTEST begin" not in t, t[-600:])
        check("shim reports a security violation", "Security Violation" in t or "Verification failed" in t or "MokManager" in t or "Perform MOK" in t, t[-600:])
    if 4 in want:
        print("--- 4. Secure Boot ON, nothing enrolled (a fresh machine)")
        t = boot("s4", esp("signed", True), True, None, lambda x: "Security Violation" in x or done(x), 40)
        check("the kernel did not start", "IYAKKI-SBTEST begin" not in t, t[-600:])
        check("shim reports a security violation (then MokManager)", "Security Violation" in t or "Verification failed" in t or "MokManager" in t or "Perform MOK" in t, t[-600:])
    if 5 in want:
        print("--- 5. Secure Boot ON, the old unsigned files")
        t = boot("s5", esp("plain", False), True, good_cert, done, 25)
        check("the kernel did not start", "IYAKKI-SBTEST begin" not in t, t[-600:])
        check("nothing unsigned ran", "Booting `IYAKKI'" not in t, t[-600:])

    if 6 in want:
        print("--- 6. Secure Boot ON, enrolment through MokManager by key presses, then boot")
        d = esp("signed", True)
        ok, vf = enrol_by_keys("s6a", d)
        check("MokManager: Enroll key from disk, Yes, Reboot (no password asked)", ok)
        if ok:
            shutil.copy(vf, f"{work}/vars-s6b.fd")
            # boot again with the variable store MokManager just wrote (vars_file would make a fresh one)
            orig = vars_file
            vars_file = lambda n, sb, c: f"{work}/vars-s6b.fd"
            t = boot("s6b", d, True, None, done, a.timeout)
            check("after the enrolment the signed chain boots, locked down", "Secure boot enabled" in t and "[integrity]" in t, t[-600:])
            check("signed module loads, unsigned refused", "module good: loaded" in t and "module unsigned: refused" in t, t[-600:])
    print(f"{sum(results)}/{len(results)} checks passed" + ("" if a.keep else f" (work directory {work} removed; --keep keeps it)"))
    if all(results):
        print("PASS")
    else:
        a.keep = True
        print(f"FAIL (logs kept in {work})")
    if not a.keep:
        shutil.rmtree(work, ignore_errors=True)
    sys.exit(0 if all(results) else 1)


main()
