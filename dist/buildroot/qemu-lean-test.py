#!/usr/bin/env python3
"""The lean boot (docs/BOOT-SPEED.md) through the real chain: UEFI firmware, GRUB menu, kernel, first stage, the real root, over a THROTTLED virtual
USB disk, with the entry chosen by key presses (QEMU monitor `sendkey`). Nothing is ever written to the image (`snapshot=on`).

    python3 dist/buildroot/qemu-lean-test.py IMAGE.img [--entry N] [--ssh-key PRIVATE_KEY] [--mbps N] [--wait SECONDS] [--shot FILE.ppm]
                                             [--serial-only] [--cmd 'sh command'] [--lean-check SCRIPT]

  IMAGE       an image made by `lean-image.sh IMAGES OUT --root stick` (the normal boot volume plus the lean files and the lean menu entries)
  --entry N   which menu entry to press Enter on (0 = the first; the lean entries are 1 and 2; entry 2 has remote diagnostics, iyakki.ssh=1)
  --ssh-key   with entry 2 and a tree built with IYAKKI_AUTH_KEYS=KEY.pub: runs --cmd / --lean-check over SSH (port 2222 of this computer)
  --serial-only  do not wait for SSH: stop after --wait seconds and print what the serial console showed (failure messages)
  --break-tree  work on a private copy of the image whose iyakki/root/current pointer is deleted (the "tree is missing" case)
Exit code 0 when the guest answered (SSH) or, with --serial-only, always (the caller looks at the printed lines).
"""
import os, re, shutil, socket, subprocess, sys, tempfile, time

args = sys.argv[1:]
def opt(name, default=None, flag=False):
    if name in args:
        i = args.index(name)
        if flag:
            del args[i]; return True
        v = args[i + 1]; del args[i:i + 2]; return v
    return False if flag else default
entry = int(opt("--entry", "0")); key = opt("--ssh-key"); mbps = int(opt("--mbps", "0")); wait = float(opt("--wait", "90"))
shot = opt("--shot"); cmd = opt("--cmd"); check = opt("--lean-check"); serial_only = opt("--serial-only", flag=True); brk = opt("--break-tree", flag=True)
no_serial = opt("--no-serial", flag=True)   # no serial port at all, like a Mac: /dev/console is then not the serial port the command line names last
keep = opt("--keep-writes", flag=True)      # write to a private copy of the image (not snapshot) and keep it in --keep-to
keep_to = opt("--keep-to")
delay = float(opt("--disk-delay", "0"))     # the USB disk is plugged in this many seconds after power-on (via the monitor)
lat = opt("--iops", None)                   # limit random reads per second (latency of a slow stick)
if not args:
    sys.exit(__doc__)
img = os.path.abspath(args[0])
code = next((p for p in ("/usr/share/OVMF/OVMF_CODE_4M.fd", "/usr/share/OVMF/OVMF_CODE.fd") if os.path.exists(p)), None)
vars_src = next((p for p in ("/usr/share/OVMF/OVMF_VARS_4M.fd", "/usr/share/OVMF/OVMF_VARS.fd") if os.path.exists(p)), None)
if not code or not vars_src or not os.path.exists(img):
    sys.exit("need OVMF and the image")
w = tempfile.mkdtemp(prefix="rt-lt-", dir=os.environ.get("TMPDIR", "/tmp"))
brk_data = opt("--break-data", flag=True)    # the data partition's label is changed (the "no data volume" case)
if brk_data:
    cp = f"{w}/disk.img"; shutil.copyfile(img, cp)
    out = subprocess.run(["sfdisk", "-d", cp], capture_output=True, text=True).stdout
    start = [int(m.group(1)) for m in re.finditer(r"start=\s*(\d+)", out)][-1]
    r = subprocess.run(["debugfs", "-w", "-R", "ssv volume_name NOTOURS", f"{cp}?offset={start * 512}"], capture_output=True, text=True)
    img = cp
if brk:
    # a private copy; the pointer file is removed with debugfs (the data partition is the second one)
    cp = f"{w}/disk.img"; shutil.copyfile(img, cp)
    out = subprocess.run(["sfdisk", "-d", cp], capture_output=True, text=True).stdout
    start = [int(m.group(1)) for m in re.finditer(r"start=\s*(\d+)", out)][-1]
    r = subprocess.run(["debugfs", "-w", "-R", "rm /iyakki/root/current", f"{cp}?offset={start * 512}"], capture_output=True, text=True)
    print("debugfs:", (r.stdout + r.stderr).strip()[-200:])
    img = cp
shutil.copy(vars_src, f"{w}/vars.fd")
if keep and not brk:
    cp = f"{w}/disk.img"; shutil.copyfile(img, cp); img = cp
sock = f"{w}/mon.sock"
thr = (f",throttling.bps-read={mbps * 1000000}" if mbps else "") + (f",throttling.iops-read={lat}" if lat else "")
accel = ["-accel", "kvm"] if os.path.exists("/dev/kvm") else ["-accel", "tcg"]
snap = "off" if keep else "on"
direct = opt("--direct")                  # kernel + lean initrd handed to QEMU directly (no firmware, no GRUB): the only way to plug the disk in late
disk = ["-drive", f"if=none,id=d0,file={img},format=raw,snapshot={snap}{thr}"]
if direct:
    base = os.path.dirname(os.path.abspath(args[0]))
    cmdline = "console=tty0 console=ttyS0,115200 panic=20 quiet loglevel=3 vt.global_cursor_default=0 iyakki.ssh=1"
    boot = ["-kernel", direct, "-initrd", opt("--initrd", ""), "-append", cmdline, "-device", "qemu-xhci,id=x"]
else:
    boot = ["-drive", f"if=pflash,format=raw,readonly=on,file={code}", "-drive", f"if=pflash,format=raw,file={w}/vars.fd", "-device", "qemu-xhci"]
    if not delay:
        boot += ["-device", "usb-storage,drive=d0,bootindex=0"]
q = subprocess.Popen(["qemu-system-x86_64", *accel, "-m", "3072", "-smp", "2", "-display", "none", "-no-reboot", "-vga", "std", *boot, *disk,
                      *([] if direct or not delay else []),
                      *(["-device", "usb-storage,drive=d0,bus=x.0"] if (direct and not delay) else []),
                      "-netdev", "user,id=n0,hostfwd=tcp:127.0.0.1:2222-:22", "-device", "virtio-net-pci,netdev=n0",
                      *([] if no_serial else ["-serial", f"file:{w}/serial.log"]), "-monitor", f"unix:{sock},server,nowait"], stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
t0 = time.monotonic()
for _ in range(100):
    if os.path.exists(sock):
        break
    time.sleep(0.1)
m = socket.socket(socket.AF_UNIX); m.connect(sock); m.settimeout(5)
def mon(c, pause=0.2):
    m.sendall((c + "\n").encode()); time.sleep(pause)
    try: m.recv(65536)
    except Exception: pass
def serial():
    try: return open(f"{w}/serial.log", "rb").read().decode("latin1")
    except OSError: return ""
rc = 1
try:
    if direct:
        t_grub = 0.0
        t_enter = time.monotonic() - t0
        print(f"direct kernel boot (no firmware, no GRUB){f'; the disk is plugged in {delay} s after power-on' if delay else ''}")
        if delay:
            time.sleep(delay)
            mon("device_add usb-storage,drive=d0,bus=x.0", 0.5)
    else:
        if no_serial:
            time.sleep(3.0)
        else:
            while "BdsDxe: starting" not in serial() and time.monotonic() - t0 < 60:
                time.sleep(0.1)
        t_grub = time.monotonic() - t0
        time.sleep(1.6)                       # the menu is up (it waits 3 s)
        for _ in range(entry):
            mon("sendkey down", 0.25)
        mon("sendkey ret", 0.1)
        t_enter = time.monotonic() - t0
        print(f"grub at {t_grub:.1f} s, Enter on entry {entry} at {t_enter:.1f} s (QEMU wall clock)")
    ssh = ["ssh", "-i", key, "-p", "2222", "-o", "StrictHostKeyChecking=no", "-o", "UserKnownHostsFile=/dev/null", "-o", "BatchMode=yes", "-o", "ConnectTimeout=3", "-o", "LogLevel=ERROR", "root@127.0.0.1"] if key else None
    up = None
    if serial_only:
        # a failing lean boot: look at the screen 15 s after Enter, then see when the machine restarts by itself (QEMU stops at the reset)
        time.sleep(float(opt("--shot-at", "15")))
        if shot:
            mon(f"screendump {shot}", 1.0)
        snap = serial()
        while q.poll() is None and time.monotonic() - t0 < t_enter + wait:
            time.sleep(1)
        print(f"machine restarted itself {time.monotonic() - t0 - t_enter:.0f} s after Enter" if q.poll() is not None else "machine did NOT restart within the wait")
        lines = [re.sub(r"\x1b\[[0-9;?]*[A-Za-z]", "", l) for l in snap.splitlines()]
        print("--- serial console (the same text goes to the screen) ---")
        print("\n".join(l for l in lines if re.search(r"stage1|lean boot|Restart|restarts|panic", l)))
        sys.exit(0)
    while time.monotonic() - t0 < t_enter + wait:
        if ssh:
            r = subprocess.run(ssh + ["true"], capture_output=True, timeout=20)
            if r.returncode == 0:
                up = time.monotonic() - t0; break
        time.sleep(1)
    if up:
        print(f"SSH answered at {up:.1f} s ({up - t_enter:.1f} s after Enter)")
        time.sleep(12)                    # let the screen come up before looking
        if shot:
            mon(f"screendump {shot}", 1.0)
        hc = opt("--host-cmd")
        if hc:
            r = subprocess.run(hc, shell=True, capture_output=True, timeout=300)
            print(f"--- on this computer: {hc} ---"); print(r.stdout.decode("latin1") + r.stderr.decode("latin1"))
        for c in ([cmd] if cmd else []) + ([f"sh -s"] if check else []):
            r = subprocess.run(ssh + [c], input=open(check, "rb").read() if (check and c == "sh -s") else None, capture_output=True, timeout=120)
            print(f"--- {c} ---"); print(r.stdout.decode("latin1") + r.stderr.decode("latin1"))
        rc = 0
    else:
        if shot:
            mon(f"screendump {shot}", 1.0)
        print("--- serial console (what an owner sees on the screen if the lean boot stops) ---")
        lines = [re.sub(r"\x1b\[[0-9;?]*[A-Za-z]", "", l) for l in serial().splitlines()]
        print("\n".join(l for l in lines if re.search(r"stage1|lean|IYAKKI|panic|Kernel", l))[-3000:])
        rc = 0 if serial_only else 1
finally:
    q.kill(); q.wait()
    if keep and keep_to:
        shutil.copyfile(img, keep_to)
    shutil.rmtree(w, ignore_errors=True)
sys.exit(rc)
