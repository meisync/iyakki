#!/usr/bin/env python3
"""Checks IYAKKI's GRUB menu draws over the logo, without building an image: makes a small GRUB EFI program from this
PC's GRUB modules (the same module list as the image's defconfig), a FAT folder with the real grub.cfg, theme, picture and a
font, boots it in QEMU with OVMF and looks at the screen. Needs grub-mkimage (grub-efi-amd64-bin), qemu-system-x86_64,
OVMF and Pillow. Saves the screenshot.

  python3 dist/buildroot/qemu-grub-menu-test.py [screenshot-dir]

This does not test Buildroot's own GRUB build or the BIOS (PC) GRUB; qemu-splash-test.py --grub does, on a built ISO.
"""
import json, os, re, shutil, socket, subprocess, sys, tempfile, time

here = os.path.dirname(os.path.abspath(__file__))
board = os.path.join(here, "board/iyakki")
shots = sys.argv[1] if len(sys.argv) > 1 else tempfile.mkdtemp(prefix="iyakki-grubmenu-")
os.makedirs(shots, exist_ok=True)
work = tempfile.mkdtemp(prefix="iyakki-gm-")
sock = os.path.join(tempfile.gettempdir(), f"iyakki-gm-{os.getpid()}.sock")

# the module list comes from the defconfig, so the test follows what the image builds in
cfg = open(os.path.join(here, "configs/iyakki_x86_64_defconfig")).read()
mods = re.search(r'BUILTIN_MODULES_EFI="([^"]*)"', cfg).group(1).split()
esp = os.path.join(work, "esp")
for d in ("EFI/BOOT", "boot/grub/fonts", "boot/grub/themes/iyakki"):
    os.makedirs(os.path.join(esp, d))
open(os.path.join(work, "builtin.cfg"), "w").write("search --no-floppy --file --set=root /boot/grub/grub.cfg\nset prefix=($root)/boot/grub\nconfigfile $prefix/grub.cfg\n")
subprocess.run(["grub-mkimage", "-O", "x86_64-efi", "-o", os.path.join(esp, "EFI/BOOT/BOOTX64.EFI"), "-c", os.path.join(work, "builtin.cfg"), "-p", "/boot/grub", *mods], check=True)
open(os.path.join(esp, "boot/grub/grub.cfg"), "w").write(open(os.path.join(board, "grub.cfg")).read().replace("__KERNEL_PATH__", "/boot/bzImage").replace("__INITRD_PATH__", "/boot/initrd"))
for f in ("iyakki-bg.png", "theme.txt"):
    shutil.copy(os.path.join(board, "grub", f), os.path.join(esp, "boot/grub/themes/iyakki", f))
font = next((p for p in ("/usr/share/grub/unicode.pf2", "/boot/grub/fonts/unicode.pf2", "/usr/share/grub2/unicode.pf2") if os.path.exists(p)), None)
if not font:
    sys.exit("FAIL: no unicode.pf2 on this PC (install grub-common)")
shutil.copy(font, os.path.join(esp, "boot/grub/fonts/unicode.pf2"))
code = next((p for p in ("/usr/share/OVMF/OVMF_CODE_4M.fd", "/usr/share/OVMF/OVMF_CODE.fd", "/usr/share/ovmf/OVMF.fd") if os.path.exists(p)), None)
vars_src = next((p for p in ("/usr/share/OVMF/OVMF_VARS_4M.fd", "/usr/share/OVMF/OVMF_VARS.fd") if os.path.exists(p)), None)
if not (code and vars_src):
    sys.exit("FAIL: OVMF not found")
shutil.copy(vars_src, os.path.join(work, "vars.fd"))

proc = subprocess.Popen(["qemu-system-x86_64", "-m", "512", "-accel", "kvm", "-accel", "tcg", "-machine", "q35",
                         "-drive", f"if=pflash,format=raw,readonly=on,file={code}", "-drive", f"if=pflash,format=raw,file={work}/vars.fd",
                         "-drive", f"file=fat:rw:{esp},format=raw,if=none,id=d", "-device", "virtio-blk-pci,drive=d",
                         "-vga", "std", "-display", "none", "-nic", "none", "-qmp", f"unix:{sock},server,nowait"])
fail = None
try:
    s = socket.socket(socket.AF_UNIX)
    for _ in range(100):
        try:
            s.connect(sock); break
        except OSError:
            time.sleep(0.1)
    f = s.makefile("rw"); f.readline()
    def cmd(c, a=None):
        f.write(json.dumps({"execute": c, "arguments": a or {}}) + "\n"); f.flush()
        while True:
            r = json.loads(f.readline())
            if "return" in r or "error" in r:
                return r
    cmd("qmp_capabilities")
    from PIL import Image
    best, im = 0.0, None
    for i in range(30):
        time.sleep(0.5)
        cmd("screendump", {"filename": f"{work}/g.ppm"})
        im = Image.open(f"{work}/g.ppm").convert("RGB")
        w, h = im.size
        raw = im.crop((int(w * .35), int(h * .15), int(w * .65), int(h * .5))).tobytes()
        px = [raw[j:j + 3] for j in range(0, len(raw), 3)]
        best = max(best, sum(1 for c in px if c[1] > 150 and c[2] > 150) / max(1, len(px)))
        if best > 0.02:
            break
    im.save(os.path.join(shots, "grub-menu.png"))
    print(f"logo cyan {best:.3f}")
    if best < 0.02:
        fail = "the IYAKKI logo is not on screen at the GRUB menu"
    cmd("quit")
finally:
    proc.kill() if proc.poll() is None else None
    proc.wait()
    for p in (sock,):
        if os.path.exists(p): os.remove(p)
    shutil.rmtree(work, ignore_errors=True)
print(f"screenshot in {shots}")
print("FAIL: " + fail if fail else "PASS")
sys.exit(1 if fail else 0)
