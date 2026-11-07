#!/usr/bin/env python3
# SPDX-License-Identifier: MIT OR Apache-2.0
"""Boot a disk image under QEMU + OVMF and take screenshots / press keys / read the serial console.  Used to try a loader-default image
(docs/BOOT-LOADER.md, `IYAKKI_LOADER_DEFAULT=1`) without hardware.  The disk is attached with snapshot=on: the image file is never written.

    qemu_default_test.py DISK.img --out DIR [--secure VARS.fd] [--mem MB] [--step wait:SECONDS|key:NAME|spam:KEY:SECONDS|shot:NAME|expect:REGEX|until:REGEX:SECONDS] ...

  --secure VARS.fd  use the Secure Boot firmware (OVMF_CODE_4M.secboot.fd) with that variable store (made with virt-fw-vars); without it
                    the plain firmware with Secure Boot off.
  steps run in order; `shot:NAME` writes DIR/NAME.ppm and DIR/NAME.png; `expect:` and `until:` read the serial log (DIR/serial.log).
Exit 0 when every expect / until step matched.  Only files in DIR are written.
"""
import argparse, os, re, shutil, socket, subprocess, sys, time


def clean(t):
    return re.sub(r"\x1b\[[0-9;?]*[A-Za-z]", "", t).replace("\r", "")


def ppm_to_png(src, dst):
    try:
        from PIL import Image
        Image.open(src).save(dst)
    except Exception as e:  # PIL is optional
        print(f"      (no PNG: {e})")


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("disk")
    ap.add_argument("--out", required=True)
    ap.add_argument("--secure", default=None)
    ap.add_argument("--mem", type=int, default=3072)
    ap.add_argument("--ovmf", default="/usr/share/OVMF")
    ap.add_argument("--step", action="append", default=[])
    a = ap.parse_args()
    os.makedirs(a.out, exist_ok=True)
    log = os.path.join(a.out, "serial.log")
    open(log, "w").close()
    sock = os.path.join(a.out, "mon.sock")
    if os.path.exists(sock):
        os.unlink(sock)
    if a.secure:
        code = os.path.join(a.ovmf, "OVMF_CODE_4M.secboot.fd")
        vars_ = os.path.join(a.out, "vars.fd")
        shutil.copyfile(a.secure, vars_)
    else:
        code = os.path.join(a.ovmf, "OVMF_CODE_4M.fd")
        vars_ = os.path.join(a.out, "vars.fd")
        shutil.copyfile(os.path.join(a.ovmf, "OVMF_VARS_4M.fd"), vars_)
    kvm = os.access("/dev/kvm", os.R_OK | os.W_OK)
    cmd = ["qemu-system-x86_64", "-machine", "q35,smm=on" if a.secure else "q35", "-m", str(a.mem), "-smp", "2", "-display", "none",
           "-accel", "kvm" if kvm else "tcg", "-cpu", "host" if kvm else "max", "-vga", "std", "-no-reboot",
           "-drive", f"if=pflash,format=raw,unit=0,readonly=on,file={code}", "-drive", f"if=pflash,format=raw,unit=1,file={vars_}",
           "-serial", f"file:{log}", "-monitor", f"unix:{sock},server,nowait",
           "-drive", f"file={a.disk},format=raw,if=none,id=d0,snapshot=on", "-device", "virtio-blk-pci,drive=d0,bootindex=1",
           "-device", "qemu-xhci", "-device", "usb-kbd", "-device", "usb-mouse"]
    if a.secure:
        cmd += ["-global", "driver=cfi.pflash01,property=secure,value=on"]
    p = subprocess.Popen(cmd, stdin=subprocess.DEVNULL, stdout=subprocess.DEVNULL, stderr=subprocess.PIPE)
    mon = None
    for _ in range(100):
        try:
            mon = socket.socket(socket.AF_UNIX)
            mon.connect(sock)
            mon.setblocking(False)
            break
        except OSError:
            mon = None
            time.sleep(0.1)
    if not mon:
        print("FAIL: no QEMU monitor", file=sys.stderr)
        p.kill()
        sys.exit(1)

    def monitor(line, wait=0.05):
        try:
            mon.send((line + "\n").encode())
            time.sleep(wait)
            mon.recv(65536)
        except (OSError, BlockingIOError):
            pass

    def text():
        return clean(open(log, "rb").read().decode("utf-8", "replace"))

    ok = True
    t0 = time.time()
    for st in a.step:
        kind, _, rest = st.partition(":")
        if kind == "wait":
            time.sleep(float(rest))
        elif kind == "key":
            for k in rest.split(","):
                monitor("sendkey " + k, 0.4)
        elif kind == "spam":  # spam:KEY:SECONDS: press KEY every 100 ms (the loader opens its menu on a key held at start)
            k, _, secs = rest.partition(":")
            end = time.time() + float(secs)
            while time.time() < end:
                monitor("sendkey " + k, 0.1)
        elif kind == "shot":
            ppm = os.path.join(a.out, rest + ".ppm")
            monitor("screendump " + ppm, 1.5)
            time.sleep(1)
            ppm_to_png(ppm, os.path.join(a.out, rest + ".png"))
            print(f"      shot {rest} at {time.time() - t0:.0f}s")
        elif kind == "expect":
            m = re.search(rest, text())
            print(("PASS  " if m else "FAIL  ") + f"serial has /{rest}/")
            ok &= bool(m)
        elif kind == "until":
            rx, _, secs = rest.rpartition(":")
            end = time.time() + float(secs)
            m = None
            while time.time() < end and p.poll() is None:
                m = re.search(rx, text())
                if m:
                    break
                time.sleep(1)
            print(("PASS  " if m else "FAIL  ") + f"serial got /{rx}/ after {time.time() - t0:.0f}s")
            ok &= bool(m)
        if p.poll() is not None:
            print(f"      QEMU exited early (code {p.returncode})")
            break
    p.kill()
    p.wait()
    print("PASS" if ok else "FAIL")
    sys.exit(0 if ok else 1)


main()
