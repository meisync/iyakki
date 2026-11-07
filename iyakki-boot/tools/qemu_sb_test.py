#!/usr/bin/env python3
# SPDX-License-Identifier: MIT OR Apache-2.0
"""Secure Boot test of the loader in the shim chain, QEMU + OVMF with Secure Boot ENABLED.   See docs/SIGNING.md, section 5.

    qemu-secureboot-test.sh --kernel BZIMAGE [--shim SHIM.efi] [--mmx MM.efi] [--boot-efi LOADER.efi] [--keep]

Chain under test: firmware (Secure Boot on, Microsoft keys) -> shim (EFI/BOOT/BOOTX64.EFI) -> our loader as shim's second stage
(EFI/BOOT/grubx64.efi: the file NAME shim looks for, no GRUB involved) -> Linux kernel started by the loader with LoadImage/StartImage
(shim's security override checks it) -> a tiny initramfs that prints a marker.  The signing key is a THROWAWAY key made in a temp directory
and put into the firmware's MOK list with virt-fw-vars (what MokManager's "Enroll key" does); nothing real is read.

Cases: signed loader with SBAT + signed kernel boots; unsigned loader, loader signed by a key that is not enrolled, kernel signed by an
enrolled-nothing key, and an unsigned kernel are all refused; a signed loader WITHOUT .sbat is run and the outcome is only printed
(it depends on the shim version).  Missing tools are named and the test exits 2.

Needs: qemu-system-x86_64, OVMF 4M files with the Secure Boot code (OVMF_CODE_4M.secboot.fd, OVMF_VARS_4M.ms.fd), sbsign, openssl,
objcopy, cpio, gzip, a static busybox, a Microsoft-signed shim + MokManager (Debian/Ubuntu: /usr/lib/shim/), virt-fw-vars
(`python3 -m venv v && v/bin/pip install virt-firmware`; test tool only), and an EFI-stub kernel (the kernel is signed here with the test key).
"""
import argparse, os, re, shutil, subprocess, sys, tempfile, time

HERE = os.path.dirname(os.path.abspath(__file__))
REPO = os.path.dirname(HERE)
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


def clean(t):
    return re.sub(r"\x1b\[[0-9;?]*[A-Za-z]", "", t).replace("\r", "")


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--kernel", required=True, help="an x86_64 kernel with the EFI stub (unsigned; it is signed here with the test key)")
    ap.add_argument("--boot-efi", default=os.path.join(REPO, "target/x86_64-unknown-uefi/release/boot-uefi.efi"),
                    help="the loader built WITHOUT BOOT_CONFIG_PATH (it reads /boot.cfg)")
    ap.add_argument("--shim", default="/usr/lib/shim/shimx64.efi.signed.latest")
    ap.add_argument("--mmx", default="/usr/lib/shim/mmx64.efi")
    ap.add_argument("--ovmf", default="/usr/share/OVMF")
    ap.add_argument("--virt-fw-vars", default=shutil.which("virt-fw-vars") or os.path.expanduser("~/.cache/iyakki-tmp/sb/venv/bin/virt-fw-vars"))
    ap.add_argument("--keep", action="store_true")
    ap.add_argument("--timeout", type=int, default=90)
    a = ap.parse_args()
    qemu = need("qemu-system-x86_64")
    for t in ("sbsign", "sbverify", "openssl", "objcopy", "cpio", "gzip"):
        need(t)
    busybox = need("busybox", "(a static one: package busybox-static)")
    for f in (a.kernel, a.boot_efi, a.shim, a.mmx, a.virt_fw_vars):
        if not os.path.exists(f):
            print(f"SKIP: {f} is missing", file=sys.stderr)
            sys.exit(2)
    code = os.path.join(a.ovmf, "OVMF_CODE_4M.secboot.fd")
    vars_in = os.path.join(a.ovmf, "OVMF_VARS_4M.ms.fd")
    for f in (code, vars_in):
        if not os.path.exists(f):
            print(f"SKIP: OVMF file {f} is missing (package ovmf)", file=sys.stderr)
            sys.exit(2)

    work = tempfile.mkdtemp(prefix="boot-sbtest-", dir=os.environ.get("TMPDIR"))
    print(f"work directory: {work}")
    # ---- throwaway keys, the loader in four states, the kernel in three ----------------------------------------------------
    for k in ("good", "other"):
        os.makedirs(f"{work}/{k}")
        subprocess.run(["openssl", "req", "-x509", "-newkey", "rsa:2048", "-nodes", "-days", "2", "-subj", f"/CN=boot-loader test key {k}",
                        "-keyout", f"{work}/{k}/key.pem", "-out", f"{work}/{k}/cert.pem"], check=True, capture_output=True)
    sbat = f"{work}/sbat.csv"
    with open(sbat, "w") as f:
        f.write("sbat,1,SBAT Version,sbat,1,https://github.com/rhboot/shim/blob/main/SBAT.md\n"
                "testdistro.loader,1,Test Distribution,test-loader,0.1,https://example.invalid\n")
    sign = lambda src, dst, k: run(["sh", f"{HERE}/sign.sh", src, dst, "--key", f"{work}/{k}/key.pem", "--cert", f"{work}/{k}/cert.pem"])
    run(["sh", f"{HERE}/add-sbat.sh", a.boot_efi, f"{work}/loader-sbat.efi", sbat])
    shutil.copy(a.boot_efi, f"{work}/loader-nosbat.efi")
    loaders = {"ok": f"{work}/L-ok.efi", "nosbat": f"{work}/L-nosbat.efi", "unsigned": f"{work}/loader-sbat.efi", "other": f"{work}/L-other.efi"}
    sign(f"{work}/loader-sbat.efi", loaders["ok"], "good")
    sign(f"{work}/loader-nosbat.efi", loaders["nosbat"], "good")
    sign(f"{work}/loader-sbat.efi", loaders["other"], "other")
    kernels = {"ok": f"{work}/K-ok", "other": f"{work}/K-other", "unsigned": a.kernel}
    sign(a.kernel, kernels["ok"], "good")
    sign(a.kernel, kernels["other"], "other")
    # the signature checks the test relies on, on the host (these must pass before QEMU is blamed for anything)
    run(["sbverify", "--cert", f"{work}/good/cert.pem", loaders["ok"]])
    assert subprocess.run(["sbverify", "--cert", f"{work}/good/cert.pem", loaders["other"]], capture_output=True).returncode != 0
    assert subprocess.run(["sbverify", "--cert", f"{work}/good/cert.pem", loaders["unsigned"]], capture_output=True).returncode != 0
    assert b".sbat" in subprocess.run(["objdump", "-h", loaders["ok"]], capture_output=True).stdout

    # ---- a tiny initramfs: the kernel got here ----------------------------------------------------------------------------
    d = f"{work}/rd"
    for sub in ("bin", "proc", "sys", "dev"):
        os.makedirs(f"{d}/{sub}")
    shutil.copy(busybox, f"{d}/bin/busybox")
    with open(f"{d}/init", "w") as f:
        f.write("#!/bin/busybox sh\n/bin/busybox --install -s /bin\nmount -t proc proc /proc; mount -t sysfs sys /sys\n"
                "echo \"LOADER-SBTEST begin\"\ndmesg | grep -i -E 'secure boot|locked down' | sed 's/^/LOADER-SBTEST dmesg: /'\n"
                "echo \"LOADER-SBTEST cmdline: $(cat /proc/cmdline)\"\necho \"LOADER-SBTEST end\"\npoweroff -f\n")
    os.chmod(f"{d}/init", 0o755)
    initrd = f"{work}/initrd"
    run(f"cd '{d}' && find . | cpio -o -H newc --quiet | gzip -1 > '{initrd}'", shell=True)

    def esp(name, loader, kernel):
        e = f"{work}/esp-{name}"
        os.makedirs(f"{e}/EFI/BOOT")
        os.makedirs(f"{e}/boot")
        shutil.copy(a.shim, f"{e}/EFI/BOOT/BOOTX64.EFI")
        shutil.copy(a.mmx, f"{e}/EFI/BOOT/mmx64.efi")
        shutil.copy(loader, f"{e}/EFI/BOOT/grubx64.efi")   # the second stage shim loads: our loader under that name
        shutil.copy(kernel, f"{e}/boot/bzImage")
        shutil.copy(initrd, f"{e}/boot/initrd")
        with open(f"{e}/boot.cfg", "w") as f:
            f.write("timeout 0\nscan no\nentry t\n  title Test\n  kernel /boot/bzImage\n  initrd /boot/initrd\n  cmdline console=ttyS0 panic=-1\n  default\n")
        return e

    def boot(name, e, mok_cert, until, timeout):
        vf = f"{work}/vars-{name}.fd"
        cmd = [a.virt_fw_vars, "-i", vars_in, "-o", vf, "--secure-boot"] + (["--add-mok", OWNER_GUID, mok_cert] if mok_cert else [])
        run(cmd)
        log = f"{work}/{name}.log"
        open(log, "w").close()
        q = [qemu, "-accel", "kvm", "-accel", "tcg", "-machine", "q35,smm=on", "-m", "1024", "-smp", "2", "-display", "none",
             "-serial", f"file:{log}", "-no-reboot", "-drive", f"if=pflash,format=raw,unit=0,file={code},readonly=on",
             "-drive", f"if=pflash,format=raw,unit=1,file={vf}", "-drive", f"file=fat:rw:{e},format=raw,if=virtio",
             "-global", "driver=cfi.pflash01,property=secure,value=on"]
        p = subprocess.Popen(q, stdout=subprocess.DEVNULL, stderr=subprocess.PIPE)
        end = time.time() + timeout
        text = ""
        while time.time() < end:
            time.sleep(1)
            text = clean(open(log, "rb").read().decode("latin-1"))
            if until(text) or p.poll() is not None:
                break
        time.sleep(1)
        p.kill()
        p.wait()
        return clean(open(log, "rb").read().decode("latin-1"))

    results = []

    def check(label, ok, detail=""):
        results.append(ok)
        print(("PASS  " if ok else "FAIL  ") + label + (f"\n      {detail[-700:]!r}" if detail and not ok else ""), flush=True)

    good = f"{work}/good/cert.pem"
    end = lambda t: "LOADER-SBTEST end" in t
    refused = lambda t: end(t) or "was not started" in t or "Security Violation" in t or "Access Denied" in t or "Verification failed" in t

    print("--- 1. Secure Boot ON, MOK = test key: shim -> signed loader (with SBAT) -> signed kernel")
    t = boot("s1", esp("s1", loaders["ok"], kernels["ok"]), good, end, a.timeout)
    check("the loader ran as shim's second stage (secure boot on)", "boot: loader started, secure boot on" in t, t)
    check("the loader started the signed kernel", "LOADER-SBTEST begin" in t and "start: t kernel=/boot/bzImage" in t, t)
    check("the kernel says Secure boot enabled", re.search(r"Secure boot enabled", t) is not None, t)
    check("the command line reached the kernel", "LOADER-SBTEST cmdline: console=ttyS0 panic=-1" in t, t)

    print("--- 2. Secure Boot ON, the loader is NOT signed")
    t = boot("s2", esp("s2", loaders["unsigned"], kernels["ok"]), good, refused, 40)
    check("the unsigned loader never ran and the kernel did not start", "boot: loader started" not in t and "LOADER-SBTEST begin" not in t, t)

    print("--- 3. Secure Boot ON, the loader is signed by a key that is NOT enrolled")
    t = boot("s3", esp("s3", loaders["other"], kernels["ok"]), good, refused, 40)
    check("the loader signed by another key never ran", "boot: loader started" not in t and "LOADER-SBTEST begin" not in t, t)

    print("--- 4. Secure Boot ON, enrolled loader, kernel signed by ANOTHER key")
    t = boot("s4", esp("s4", loaders["ok"], kernels["other"]), good, refused, 60)
    check("the loader ran", "boot: loader started, secure boot on" in t, t)
    check("the kernel with a bad signature was refused and did not start", "LOADER-SBTEST begin" not in t and re.search(r"was not started: LoadImage .*(security violation|access denied)", t, re.I) is not None, t)

    print("--- 5. Secure Boot ON, enrolled loader, UNSIGNED kernel")
    t = boot("s5", esp("s5", loaders["ok"], kernels["unsigned"]), good, refused, 60)
    check("the unsigned kernel was refused and did not start", "LOADER-SBTEST begin" not in t and re.search(r"was not started: LoadImage .*(security violation|access denied)", t, re.I) is not None, t)

    print("--- 6. Secure Boot ON, nothing enrolled (a fresh machine)")
    t = boot("s6", esp("s6", loaders["ok"], kernels["ok"]), None, lambda x: "Security Violation" in x or end(x), 40)
    check("the loader did not run (shim stops, MokManager is next)", "boot: loader started" not in t and "LOADER-SBTEST begin" not in t, t)

    print("--- 7. informational: signed loader WITHOUT an .sbat section, MOK enrolled")
    t = boot("s7", esp("s7", loaders["nosbat"], kernels["ok"]), good, lambda x: end(x) or "Security Violation" in x, 60)
    print("      result with this shim: " + ("the loader ran and the kernel started" if "LOADER-SBTEST begin" in t else
          "loader ran" if "boot: loader started" in t else "REFUSED (shim wants .sbat)"))

    print(f"{sum(results)}/{len(results)} checks passed" + ("" if a.keep else f" (work directory {work} removed; --keep keeps it)"))
    ok = all(results)
    if not ok:
        a.keep = True
        print(f"FAIL (logs kept in {work})")
    else:
        print("PASS")
    if not a.keep:
        shutil.rmtree(work, ignore_errors=True)
    sys.exit(0 if ok else 1)


main()
