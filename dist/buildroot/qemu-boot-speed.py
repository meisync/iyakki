#!/usr/bin/env python3
"""Boot speed through the real boot chain (UEFI firmware, GRUB, kernel, initramfs) with a THROTTLED virtual USB disk.

    python3 dist/buildroot/qemu-boot-speed.py [IMAGE.img] [--mbps 40,120,0] [--runs 1] [--entry N]

IMAGE defaults to target/buildroot-macweb/images/iyakki-install-x86_64.img (the install template: GRUB + boot volume + marker).
The image is opened with `snapshot=on`: nothing is ever written to it. --mbps lists the read speeds of the virtual USB stick in
MB/s (0 = unthrottled); 40 is a slow USB 3 stick through firmware, 120 a good one. KVM is used when /dev/kvm exists.

What is stamped (wall clock since QEMU started, from the serial console):
  grub      the firmware handed over to GRUB ("BdsDxe: starting")
  output    the first line the system itself prints (kernel + initramfs are loaded and unpacked, IYAKKI runs: GRUB's
            read time + kernel unpack are inside the span grub -> output)
  data      "data: persistent volume ... mounted" (storage up)
  super     "supervisor up"
Cut-off the pure firmware part: it is the same for every image. Compare the span grub -> output between images and speeds.
The simulated disk is a block-level throttle in QEMU, not a Mac's firmware USB driver: it tells what the SIZE of what GRUB reads
costs at a given sustained speed. The Mac firmware can be slower than the stick (measure on the device: dist/buildroot/boot-timeline.sh).
"""
import os, re, shutil, subprocess, sys, tempfile, threading, time, statistics

here = os.path.dirname(os.path.abspath(__file__))
args = sys.argv[1:]
def opt(name, default):
    if name in args:
        i = args.index(name); v = args[i + 1]; del args[i:i + 2]; return v
    return default
speeds = [int(x) for x in opt("--mbps", "40,120,0").split(",")]
runs = int(opt("--runs", "1"))
entry = int(opt("--entry", "0"))      # 0 = default entry (after the 3 s menu timeout)
img = args[0] if args else os.path.join(here, "../../target/buildroot-macweb/images/iyakki-install-x86_64.img")
code = next((p for p in ("/usr/share/OVMF/OVMF_CODE_4M.fd", "/usr/share/OVMF/OVMF_CODE.fd", "/usr/share/ovmf/OVMF.fd") if os.path.exists(p)), None)
vars_src = next((p for p in ("/usr/share/OVMF/OVMF_VARS_4M.fd", "/usr/share/OVMF/OVMF_VARS.fd") if os.path.exists(p)), None)
if not code or not vars_src or not os.path.exists(img):
    sys.exit("need OVMF (ovmf package) and the image: " + img)
accel = ["-accel", "kvm"] if os.path.exists("/dev/kvm") else ["-accel", "tcg"]
MARKS = [("grub", re.compile(r"BdsDxe: starting")), ("output", re.compile(r"\x1b\[0m\x1b\[30m\x1b\[40m|IYAKKI-BOOT|splash:")),
         ("data", re.compile(r"data: persistent volume")), ("super", re.compile(r"supervisor up"))]


def one(mbps):
    w = tempfile.mkdtemp(prefix="rt-boot-", dir=os.environ.get("TMPDIR", "/tmp"))
    try:
        shutil.copy(vars_src, f"{w}/vars.fd")
        thr = f",throttling.bps-read={mbps * 1000000}" if mbps else ""
        cmd = ["qemu-system-x86_64", *accel, "-m", "3072", "-smp", "2", "-display", "none", "-serial", "stdio", "-no-reboot", "-vga", "std",
               "-drive", f"if=pflash,format=raw,readonly=on,file={code}", "-drive", f"if=pflash,format=raw,file={w}/vars.fd",
               "-drive", f"if=none,id=d0,file={img},format=raw,snapshot=on{thr}", "-device", "qemu-xhci", "-device", "usb-storage,drive=d0,bootindex=0",
               "-netdev", "user,id=n0", "-device", "virtio-net-pci,netdev=n0"]
        t0 = time.monotonic()
        p = subprocess.Popen(cmd, stdout=subprocess.PIPE, stderr=subprocess.STDOUT, stdin=subprocess.DEVNULL)
        timer = threading.Timer(240, p.kill); timer.start()
        seen, grub_t = {}, None
        try:
            for raw in iter(p.stdout.readline, b""):
                t = time.monotonic() - t0
                line = raw.decode("latin1")
                for name, rx in MARKS:
                    if name not in seen and rx.search(line):
                        if name != "grub" and "grub" not in seen: continue
                        seen[name] = t
                if "super" in seen: break
        finally:
            timer.cancel(); p.kill(); p.wait()
        return seen
    finally:
        shutil.rmtree(w, ignore_errors=True)


size = os.path.getsize(img) / 1e6
print(f"image {os.path.basename(img)} ({size:.0f} MB), {runs} run(s) per speed; seconds since QEMU start (span = since the firmware started GRUB)")
for s in speeds:
    rs = [one(s) for _ in range(runs)]
    row = []
    for name in ("grub", "output", "data", "super"):
        v = [r[name] for r in rs if name in r]
        row.append(f"{name} {statistics.median(v):6.1f}" if v else f"{name}    n/a")
    g = [r["grub"] for r in rs if "grub" in r]; o = [r["output"] for r in rs if "output" in r]
    span = f"  grub->output {statistics.median(o) - statistics.median(g):5.1f} s" if g and o else ""
    print(f"{('unthrottled' if not s else str(s) + ' MB/s'):>12}: " + "  ".join(row) + span, flush=True)
