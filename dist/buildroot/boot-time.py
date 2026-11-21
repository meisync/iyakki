#!/usr/bin/env python3
"""Measure how long an IYAKKI image takes from power-on to the sign-in screen, in QEMU.

    sudo ./boot-time.py [build-output-dir] [runs]            (or: QEMU="sudo qemu-system-x86_64" ./boot-time.py)

Boots the kernel and initramfs directly (no GRUB) with a virtual display and the serial console on a pipe, and stamps every
serial line with the wall clock. The markers:

  kernel     "Linux version" (the kernel starts; before it come the virtual machine's firmware and the loader)
  init       "Run /init as init process" (the kernel hands over to IYAKKI)
  supervisor "supervisor up" (IYAKKI is running)
  login      "shell: first picture on screen" (the sign-in screen is drawn: the end of the measurement)
  ram        "shell: memory in use at the sign-in screen: N MiB of M MiB" (logged right after the first picture; read, not timed)

Prints, for each run, the wall time since QEMU started and the kernel's own clock at each marker, then the median of the runs.
Real hardware adds its firmware time (often 5 to 20 seconds on a PC), which this cannot see.
"""
import os, re, shlex, subprocess, sys, threading, time, statistics

out = sys.argv[1] if len(sys.argv) > 1 else os.path.join(os.path.dirname(os.path.abspath(__file__)), "../../target/buildroot-libre")
runs = int(sys.argv[2]) if len(sys.argv) > 2 else 3
qemu = shlex.split(os.environ.get("QEMU", "qemu-system-x86_64"))
images = os.path.join(out, "images")
initrd = next((os.path.join(images, n) for n in ("rootfs.cpio.zst", "rootfs.cpio.gz") if os.path.exists(os.path.join(images, n))), None)
if initrd is None or not os.path.exists(os.path.join(images, "bzImage")):
    sys.exit(f"no image in {images}; build one first (./run.sh build)")

MARKS = [("kernel", "Linux version"), ("init", "Run /init as init process"), ("supervisor", "supervisor up"), ("login", "shell: first picture on screen")]
KCLOCK = re.compile(r"\[\s*(\d+\.\d+)\]")
RAM = re.compile(r"memory in use at the sign-in screen: (\d+) MiB of (\d+) MiB")


def one_run():
    cmd = qemu + ["-accel", "kvm", "-m", "2048", "-smp", "2", "-no-reboot", "-display", "none", "-vga", "std", "-serial", "stdio",
                  "-kernel", os.path.join(images, "bzImage"), "-initrd", initrd, "-append", "console=ttyS0 quiet loglevel=7 panic=-1",
                  "-netdev", "user,id=n0", "-device", "virtio-net-pci,netdev=n0", "-monitor", "none"]
    t0 = time.monotonic()
    p = subprocess.Popen(cmd, stdout=subprocess.PIPE, stderr=subprocess.STDOUT, stdin=subprocess.DEVNULL)
    seen = {}
    timer = threading.Timer(90, p.kill)
    timer.start()
    try:
        for raw in p.stdout:
            now = time.monotonic() - t0
            line = raw.decode("utf-8", "replace")
            for name, pat in MARKS:
                if name not in seen and pat in line:
                    k = KCLOCK.search(line)
                    seen[name] = (now, float(k.group(1)) if k else None)
            m = RAM.search(line)
            if m and "ram" not in seen:
                seen["ram"] = (now, int(m.group(1)))
            # the memory line follows the first picture at once; wait for it, the 90 s timer ends a run that never prints it
            if "login" in seen and "ram" in seen:
                break
    finally:
        timer.cancel()
        p.kill()
        p.wait()
    return seen


results = []
for i in range(runs):
    r = one_run()
    results.append(r)
    if "ram" in r:
        print(f"run {i + 1}: RAM in use at the sign-in screen {r['ram'][1]} MiB")
    row = "  ".join(f"{n} {r[n][0]:5.2f}s" + (f" (kernel clock {r[n][1]:.2f}s)" if r[n][1] is not None else "") for n, _ in MARKS if n in r)
    print(f"run {i + 1}: {row if row else 'no markers seen'}")
    if "login" not in r:
        print("        the sign-in screen never appeared (the shell needs a display device; check the image)")
done = [r for r in results if "login" in r]
if done:
    print()
    for n, _ in MARKS:
        vals = [r[n][0] for r in done if n in r]
        if vals:
            print(f"median wall time to {n:<10}: {statistics.median(vals):5.2f} s since QEMU started")
    k = [r["login"][1] for r in done if r["login"][1] is not None]
    if k:
        print(f"median kernel clock at login     : {statistics.median(k):5.2f} s (the system's own time since the kernel started)")
    out_json = os.environ.get("BOOT_TIME_JSON")
    if out_json:
        import json
        res = {"timings": {"login_wall_s": round(statistics.median([r["login"][0] for r in done]), 2)}, "runs": len(done)}
        rams = [r["ram"][1] for r in done if "ram" in r]
        if rams:
            res["memory"] = {"idle_ram_mib": int(statistics.median(rams))}
        json.dump(res, open(out_json, "w"))
