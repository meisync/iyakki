#!/usr/bin/env python3
"""Boots the image in QEMU with a virtual screen and checks that the boot screen (logo, name, progress bar, hint) is
really drawn, then presses L and checks that the loading log appears. Saves the screenshots.

  sudo python3 dist/buildroot/qemu-splash-test.py [build-output-dir] [screenshot-dir]
  python3 dist/buildroot/qemu-splash-test.py --grub [build-output-dir] [screenshot-dir]
      boots the ISO through GRUB (BIOS, and UEFI when OVMF is found) and checks the IYAKKI logo is on the screen at the
      menu, before the kernel starts (needs an image built with the graphical GRUB menu; STRICT_LOG=1 below as well).
The boot log is checked too: the "splash:" lines (when the framebuffer appeared, its geometry, why the splash stopped)
must be in the serial log; set STRICT_LOG=1 to make a missing line a failure (an image older than this check has none).
  sudo python3 dist/buildroot/qemu-splash-test.py --boot [build-output-dir or ISO] [screenshot-dir]
      boots the ISO's normal GRUB entry (BIOS and UEFI) and takes a screenshot every ~200 ms from the GRUB menu to the
      sign-in screen: after GRUB only the boot screen (logo + progress bar) may show until the shell; see the end of the file
      for INITRD=, SPLASH_MIN= and FIRMWARE=.
(root only because QEMU may need it for KVM/io_uring limits; pass QEMU=qemu-system-x86_64 to override.)
"""
import json, os, socket, subprocess, sys, tempfile, time

here = os.path.dirname(os.path.abspath(__file__))
GRUB = "--grub" in sys.argv
BOOT = "--boot" in sys.argv
sys.argv = [a for a in sys.argv if a not in ("--grub", "--boot")]
build = sys.argv[1] if len(sys.argv) > 1 else os.path.join(here, "../../target/buildroot-libre")
shots = sys.argv[2] if len(sys.argv) > 2 else tempfile.mkdtemp(prefix="iyakki-splash-")
qemu = os.environ.get("QEMU", "qemu-system-x86_64")
work = tempfile.mkdtemp()
if not BOOT: os.makedirs(shots, exist_ok=True)


def grub_check():
    """Boot the ISO and look at the screen while GRUB's menu is up (its timeout is 3 s): the logo must be there."""
    from PIL import Image
    iso = f"{build}/images/iyakki-x86_64.iso"
    firmwares = [("bios", [])]
    for ovmf in ("/usr/share/OVMF/OVMF_CODE.fd", "/usr/share/ovmf/OVMF.fd", "/usr/share/qemu/OVMF.fd"):
        if os.path.exists(ovmf):
            firmwares.append(("uefi", ["-bios", ovmf])); break
    problems = []
    for name, extra in firmwares:
        w = tempfile.mkdtemp()
        p = subprocess.Popen([qemu, "-m", "1024", "-accel", "kvm", "-accel", "tcg", "-cdrom", iso, "-boot", "d", "-vga", "std", "-display", "none",
                              "-qmp", f"unix:{w}/q.sock,server,nowait", *extra])
        try:
            s = socket.socket(socket.AF_UNIX)
            for _ in range(100):
                try:
                    s.connect(f"{w}/q.sock"); break
                except OSError:
                    time.sleep(0.1)
            f = s.makefile("rw"); f.readline()
            def q(c, a=None):
                f.write(json.dumps({"execute": c, "arguments": a or {}}) + "\n"); f.flush()
                while True:
                    r = json.loads(f.readline())
                    if "return" in r or "error" in r: return r
            q("qmp_capabilities")
            best = 0.0
            for i in range(24):
                time.sleep(0.25)
                path = f"{w}/g.ppm"; q("screendump", {"filename": path})
                im = Image.open(path).convert("RGB"); im.save(f"{shots}/grub-{name}-{i}.png")
                raw = im.crop((int(im.size[0]*0.35), int(im.size[1]*0.15), int(im.size[0]*0.65), int(im.size[1]*0.5))).tobytes()
                px = [raw[j:j+3] for j in range(0, len(raw), 3)]
                best = max(best, sum(1 for c in px if c[1] > 150 and c[2] > 150) / max(1, len(px)))
            print(f"grub on {name}: logo cyan {best:.3f}")
            if best < 0.02: problems.append(f"the IYAKKI logo is not on screen at the {name} GRUB menu")
            q("quit")
        finally:
            p.kill() if p.poll() is None else None
            p.wait()
    return "; ".join(problems) or None

def classify(im):
    """What a screenshot shows: 'grub' (the menu over the logo), 'splash' (the logo with the progress bar), 'panel' (the bug:
    GRUB's black terminal box over the middle), 'black', 'shell' (anything else that is drawn: the sign-in screen)."""
    w, h = im.size
    px = im.load()
    def frac(x0, y0, x1, y1, pred, step=4):
        n = hit = 0
        for y in range(int(y0 * h), int(y1 * h), step):
            for x in range(int(x0 * w), int(x1 * w), step):
                n += 1; hit += 1 if pred(px[x, y]) else 0
        return hit / max(1, n)
    cyan = frac(0.35, 0.15, 0.65, 0.5, lambda p: p[1] > 150 and p[2] > 150)
    # GRUB's terminal box: a block in the middle where every pixel is black
    panel = frac(0.3, 0.3, 0.7, 0.6, lambda p: max(p) < 4)
    lit = frac(0, 0, 1, 1, lambda p: max(p) > 24, 8)
    # the progress bar: a row below the logo, most of whose middle is the bar's blue or its dark blue track
    is_bar = lambda p: (p[2] > 180 and p[0] < 140 and p[1] > 150) or (abs(p[0] - 24) < 7 and abs(p[1] - 34) < 7 and abs(p[2] - 64) < 9)
    xs = range(int(w * 0.36), int(w * 0.64), 2)
    rows = [sum(1 for x in xs if is_bar(px[x, y])) / len(xs) for y in range(int(h * 0.5), int(h * 0.95))]
    # only a thin band counts (a few pixels high), not a big block of a similar colour
    bar, run = 0.0, []
    for r in rows + [0.0]:
        if r > 0.5:
            run.append(r)
        else:
            if run and len(run) <= max(3, int(h * 0.02)): bar = max(bar, min(run))
            run = []
    # GRUB's menu: light grey-blue text (#9aa6c8) in the menu area
    menu = frac(0.14, 0.61, 0.86, 0.95, lambda p: abs(p[0] - 154) < 30 and abs(p[1] - 166) < 30 and abs(p[2] - 200) < 30, 1)
    if panel > 0.97 and lit >= 0.002: kind = "panel"
    elif lit < 0.01: kind = "black"
    # the emblem fills much of the middle box (0.15 or more); the sign-in screen has only thin cyan lines there (about 0.06)
    elif cyan > 0.1 and bar > 0.5: kind = "splash"
    elif cyan > 0.1 and menu > 0.003: kind = "grub"
    # the emblem alone: GRUB loading the kernel (its terminal window below the logo), or a frame being written
    elif cyan > 0.1: kind = "logo"
    else: kind = "shell"
    return kind, {"cyan": round(cyan, 3), "panel": round(panel, 3), "bar": round(bar, 3), "menu": round(menu, 4), "lit": round(lit, 3)}


def repack_iso(iso, out_dir, initrd=None, splash_min=None, grub_from_repo=False, gfxmode=None):
    """A copy of the ISO with: another initrd (a freshly built iyakki appended to the image's own, see --boot below); the
    boot menu and GRUB theme of this checkout (written the way post-image.sh writes them); a preferred GRUB screen mode
    (`set gfxmode=WxH,auto`: with gfxpayload=keep the kernel keeps it, so 1920x1080 is what the Mac mini shows); and/or
    `iyakki.splash=N` on the normal entry (a VM starts too fast to look at the boot screen otherwise)."""
    import re
    out = os.path.join(out_dir, "test.iso")
    args = ["xorriso", "-indev", iso, "-outdev", out, "-boot_image", "any", "replay"]
    if initrd:
        args += ["-map", initrd, "/boot/initrd"]
    if grub_from_repo:
        args += ["-map", os.path.join(here, "board/iyakki/grub/theme.txt"), "/boot/grub/themes/iyakki/theme.txt"]
    if splash_min or grub_from_repo or gfxmode:
        if grub_from_repo:
            text = open(os.path.join(here, "board/iyakki/grub.cfg")).read().replace("__KERNEL_PATH__", "/boot/bzImage").replace("__INITRD_PATH__", "/boot/initrd")
        else:
            subprocess.run(["xorriso", "-osirrox", "on", "-indev", iso, "-extract", "/boot/grub/grub.cfg", os.path.join(out_dir, "grub.cfg")], check=True, capture_output=True)
            text = open(os.path.join(out_dir, "grub.cfg")).read()
        if gfxmode:
            text = text.replace("set gfxmode=auto", f"set gfxmode={gfxmode},auto", 1)
        if splash_min:
            text = re.sub(r'(menuentry "IYAKKI" \{\s*linux\s+[^\n]*)', lambda m: m.group(1) + f" iyakki.splash={splash_min}", text, count=1)
        open(os.path.join(out_dir, "grub-test.cfg"), "w").write(text)
        args += ["-map", os.path.join(out_dir, "grub-test.cfg"), "/boot/grub/grub.cfg"]
    subprocess.run(args + ["-commit"], check=True, capture_output=True)
    return out


def boot_check(iso, keep):
    """Boot the ISO's normal GRUB entry (BIOS, and UEFI when OVMF is found) and take a screenshot every ~200 ms from the GRUB
    menu until a moment after the sign-in screen's first picture ('shell: first picture on screen' on the serial port, or
    the screen settling on the shell). Between the last GRUB frame and the shell every frame must be the boot screen (logo +
    progress bar): never GRUB's black terminal box, never GRUB's menu left over."""
    from PIL import Image
    firmwares = [("bios", [])]
    for ovmf in ("/usr/share/OVMF/OVMF_CODE.fd", "/usr/share/ovmf/OVMF.fd", "/usr/share/qemu/OVMF.fd"):
        if os.path.exists(ovmf):
            firmwares.append(("uefi", ["-bios", ovmf])); break
    only = os.environ.get("FIRMWARE")
    problems = []
    for name, extra in firmwares:
        if only and name != only:
            continue
        w = tempfile.mkdtemp()
        p = subprocess.Popen([qemu, "-m", "2048", "-smp", "4", "-accel", "kvm", "-accel", "tcg", "-cdrom", iso, "-boot", "d", "-vga", "std", "-display", "none",
                              "-serial", f"file:{w}/serial.log", "-qmp", f"unix:{w}/q.sock,server,nowait",
                              "-netdev", "user,id=n", "-device", "virtio-net-pci,netdev=n", *extra])
        kinds = []
        try:
            s = socket.socket(socket.AF_UNIX)
            for _ in range(100):
                try:
                    s.connect(f"{w}/q.sock"); break
                except OSError:
                    time.sleep(0.1)
            f = s.makefile("rw"); f.readline()
            def q(c, a=None):
                f.write(json.dumps({"execute": c, "arguments": a or {}}) + "\n"); f.flush()
                while True:
                    r = json.loads(f.readline())
                    if "return" in r or "error" in r: return r
            q("qmp_capabilities")
            t0 = time.time()
            shell_at = None
            i = 0
            while time.time() - t0 < 120:
                time.sleep(0.2)
                path = f"{w}/g.ppm"; q("screendump", {"filename": path})
                try:
                    im = Image.open(path).convert("RGB")
                except OSError:
                    continue
                kind, nums = classify(im)
                kinds.append(kind)
                if keep:
                    im.save(f"{shots}/boot-{name}-{i:03d}-{kind}.png")
                print(f"{name} {time.time() - t0:6.2f}s #{i:03d} {im.size[0]}x{im.size[1]} {kind:6s} {nums}", flush=True)
                i += 1
                serial = open(f"{w}/serial.log", errors="replace").read() if os.path.exists(f"{w}/serial.log") else ""
                # the end: the shell's first picture (in the serial log when the kernel prints it), or the screen settled on
                # the shell after the boot screen (or long after GRUB, when no boot screen ever came)
                settled = "grub" in kinds and (kinds[-6:] == ["shell"] * 6 and "splash" in kinds or kinds[-40:] == ["shell"] * 40)
                if shell_at is None and ("first picture on screen" in serial or settled):
                    shell_at = i
                if shell_at is not None and i > shell_at + 8:
                    break
            q("quit")
            open(f"{shots}/serial-{name}.log", "w").write(open(f"{w}/serial.log", errors="replace").read())
        finally:
            p.kill() if p.poll() is None else None
            p.wait()
        seq = [k for j, k in enumerate(kinds) if j == 0 or k != kinds[j - 1]]
        print(f"{name}: {' -> '.join(seq)}")
        if "grub" not in kinds:
            problems.append(f"{name}: the GRUB menu was never seen"); continue
        last_grub = len(kinds) - 1 - kinds[::-1].index("grub")
        after = kinds[last_grub + 1:]
        # the shell: the first of three shell frames in a row (a single odd frame is a GPU driver changing the mode, which
        # QEMU's screen shows as a frame of noise)
        first_shell = next((j for j in range(len(after)) if after[j:j + 3] == ["shell"] * 3), len(after))
        between = after[:first_shell]
        splash = between.count("splash")
        print(f"{name}: {splash} boot screen frame(s) between GRUB and the shell; then {len(after) - first_shell} shell frame(s)")
        if splash == 0: problems.append(f"{name}: no boot screen (logo + progress bar) between GRUB and the shell")
        # GRUB's own terminal box while it loads the kernel comes before the kernel runs; after the first boot screen frame
        # nothing but the boot screen may show
        first_splash = between.index("splash") if splash else len(between)
        bad = [k for k in between[first_splash:] if k != "splash"]
        if bad: problems.append(f"{name}: {len(bad)} frame(s) of {sorted(set(bad))} after the boot screen appeared, before the shell")
        if "shell" not in after: problems.append(f"{name}: the sign-in screen never came")
    return "; ".join(problems) or None


if BOOT:
    # sudo python3 qemu-splash-test.py --boot [build-dir or ISO] [screenshot-dir]
    #   INITRD=path    boot this initrd instead of the ISO's (for example the image's rootfs.cpio.zst with a cpio holding a
    #                  newly built usr/bin/iyakki appended: the kernel unpacks both, the later file wins)
    #   SPLASH_MIN=N   add iyakki.splash=N to the normal entry (a VM boots too fast to see the boot screen otherwise)
    #   GRUB_FROM_REPO=1   use this checkout's board/iyakki/grub.cfg and grub/theme.txt instead of the ISO's
    #   GFXMODE=1920x1080  the GRUB (and so the kernel's) screen mode to prefer
    #   FIRMWARE=bios|uefi   only that one
    iso = sys.argv[1] if len(sys.argv) > 1 else f"{build}/images/iyakki-x86_64.iso"
    if os.path.isdir(iso): iso = f"{iso}/images/iyakki-x86_64.iso"
    shots = sys.argv[2] if len(sys.argv) > 2 else tempfile.mkdtemp(prefix="iyakki-boot-")
    os.makedirs(shots, exist_ok=True)
    env = os.environ.get
    if env("INITRD") or env("SPLASH_MIN") or env("GRUB_FROM_REPO") or env("GFXMODE"):
        iso = repack_iso(iso, work, env("INITRD"), env("SPLASH_MIN"), env("GRUB_FROM_REPO") == "1", env("GFXMODE"))
        print(f"test ISO: {iso}")
    r = boot_check(iso, True)
    print(f"screenshots in {shots}")
    print("FAIL: " + r if r else "PASS")
    sys.exit(1 if r else 0)

if GRUB:
    shots = sys.argv[2] if len(sys.argv) > 2 else tempfile.mkdtemp(prefix="iyakki-grub-")
    os.makedirs(shots, exist_ok=True)
    r = grub_check()
    print(f"screenshots in {shots}")
    print("FAIL: " + r if r else "PASS")
    sys.exit(1 if r else 0)

def normal_cmdline():
    """The kernel command line of the boot menu's normal "IYAKKI" entry (grub.cfg), so this test boots with exactly the options a
    real start uses; the serial console comes last so the log still reaches serial.log."""
    import re
    cfg = open(os.path.join(here, "board/iyakki/grub.cfg")).read()
    m = re.search(r'menuentry "IYAKKI" \{\s*linux\s+\S+\s+([^\n]*)', cfg)
    args = [a for a in (m.group(1).split() if m else ["quiet", "loglevel=3"]) if not a.startswith("console=")]
    return " ".join(["console=tty0", *args, "console=ttyS0", "iyakki.splash=8"])

CMDLINE = normal_cmdline()
print(f"kernel command line: {CMDLINE}")
proc = subprocess.Popen(
    [qemu, "-m", "1024", "-smp", "4", "-accel", "kvm", "-accel", "tcg", "-kernel", f"{build}/images/bzImage",
     "-initrd", f"{build}/images/rootfs.cpio.zst", "-append", CMDLINE,
     "-vga", "std", "-display", "none", "-serial", f"file:{work}/serial.log", "-qmp", f"unix:{work}/q.sock,server,nowait",
     "-netdev", "user,id=n", "-device", "virtio-net-pci,netdev=n"])
fail = None
try:
    s = socket.socket(socket.AF_UNIX)
    for _ in range(100):
        try:
            s.connect(f"{work}/q.sock"); break
        except OSError:
            time.sleep(0.1)
    f = s.makefile("rw")
    f.readline()

    def cmd(c, a=None):
        f.write(json.dumps({"execute": c, "arguments": a or {}}) + "\n"); f.flush()
        while True:
            r = json.loads(f.readline())
            if "return" in r or "error" in r:
                return r

    cmd("qmp_capabilities")

    def shot(name):
        path = f"{work}/{name}.ppm"
        cmd("screendump", {"filename": path})
        from PIL import Image
        im = Image.open(path).convert("RGB")
        im.save(f"{shots}/{name}.png")
        return im

    def bright(im, box, pred):
        w, h = im.size
        x0, y0, x1, y1 = [int(v) for v in (box[0] * w, box[1] * h, box[2] * w, box[3] * h)]
        raw = im.crop((x0, y0, x1, y1)).tobytes()
        px = [raw[i:i + 3] for i in range(0, len(raw), 3)]
        return sum(1 for p in px if pred(p)) / max(1, len(px))

    # wait for a 1280-wide frame whose middle is not black (the kernel's text mode is 720 wide)
    im = None
    for _ in range(60):
        time.sleep(0.25)
        im = shot("splash")
        if im.size[0] >= 1024 and bright(im, (0.35, 0.2, 0.65, 0.5), lambda p: p[1] > 150 and p[2] > 150) > 0.02:
            break
    cyan = bright(im, (0.35, 0.2, 0.65, 0.5), lambda p: p[1] > 150 and p[2] > 150)       # the logo's cyan
    white = bright(im, (0.4, 0.58, 0.6, 0.7), lambda p: min(p) > 200)                    # the name, "IYAKKI"
    bar = bright(im, (0.28, 0.72, 0.72, 0.76), lambda p: p[2] > 180 and p[0] < 140)       # the progress bar
    stars = bright(im, (0.0, 0.0, 0.25, 1.0), lambda p: max(p) > 80)                      # stars at the left side
    print(f"screen {im.size}: logo cyan {cyan:.3f}, name {white:.3f}, bar {bar:.3f}, stars {stars:.4f}")
    if im.size[0] < 1024: fail = "the screen never switched to the graphics mode"
    elif cyan < 0.02: fail = "the logo is not on screen"
    elif white < 0.005: fail = "the name IYAKKI is not on screen"
    elif bar < 0.01: fail = "the progress bar is not on screen"
    elif stars < 0.0003: fail = "the galaxy is not drawn"
    # the animation moves: two frames a moment apart differ
    time.sleep(0.8)
    im2 = shot("splash2")
    if not fail and im.tobytes() == im2.tobytes(): fail = "the picture is frozen (no animation)"
    # L shows the loading log
    cmd("send-key", {"keys": [{"type": "qcode", "data": "l"}]})
    time.sleep(1.0)
    im3 = shot("splash-log")
    panel = bright(im3, (0.1, 0.12, 0.9, 0.85), lambda p: p[0] > 150 and p[1] > 160 and p[2] > 170)  # light text
    print(f"log panel text {panel:.4f}")
    if not fail and panel < 0.003: fail = "pressing L did not show the loading log"
    cmd("quit")
    # the boot log must tell the story of the screen (dmesg | grep -i splash)
    try:
        serial = open(f"{work}/serial.log", errors="replace").read()
    except OSError:
        serial = ""
    lines = [l for l in serial.splitlines() if "splash:" in l]
    print("\n".join(lines[:12]) or "no splash: lines in the serial log")
    if not fail and os.environ.get("STRICT_LOG") == "1":
        if not any("appeared" in l for l in lines): fail = "the log does not say when the framebuffer appeared"
        elif not any("drawing on" in l and "x" in l for l in lines): fail = "the log does not give the screen geometry"
        elif not any("stopped after" in l for l in lines): fail = "the log does not say why the splash stopped"
finally:
    proc.kill() if proc.poll() is None else None
    proc.wait()
print(f"screenshots in {shots}")
print("FAIL: " + fail if fail else "PASS")
sys.exit(1 if fail else 0)
