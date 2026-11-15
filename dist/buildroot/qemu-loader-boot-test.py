#!/usr/bin/env python3
"""Boot a lean image through IYAKKI's OWN boot loader (iyakki-boot), not GRUB, in three boot-volume layouts, under QEMU + OVMF (KVM when there is one).

    python3 dist/buildroot/qemu-loader-boot-test.py IMAGE.img [--layout lean|sfs|sibling|all] [--loader BOOT-UEFI.EFI] [--wait SECONDS] [--keep]
    python3 dist/buildroot/qemu-loader-boot-test.py --synthetic [--kernel BZIMAGE] [same options]    (a small fixture made from this computer's kernel; tests the script and the loader only)

  IMAGE      a disk image whose first partition is the FAT boot volume with /boot/bzImage, a RAM disk (/boot/initrd-lean and/or /boot/initrd) and
             EFI/iyakki-boot/{loader.efi,boot.cfg}: an `iyakki-lean-x86_64.img` of lean-image.sh, or an installed stick (docs/development/LOADER-QEMU-TESTS.md)
  --layout   lean     the lean layout: /boot/initrd-lean is there, /boot/initrd is not, boot.cfg names /boot/initrd-lean (seed and stick images)
             sfs      the sfs layout: /boot/initrd is there, /boot/initrd-lean is not, boot.cfg names /boot/initrd (loader-default sfs images)
             sibling  the volume only has /boot/initrd-lean but boot.cfg names /boot/initrd: the loader must use the sibling and say so
                      (needs a loader with that rule: pass --loader with a build of iyakki-boot that has it)
             all      lean and sfs, and sibling when --loader is given (the default)
  --loader   a boot-uefi.efi built with BOOT_CONFIG_PATH=/EFI/iyakki-boot/boot.cfg; it replaces the loader in the private copy (default: the one in the image)
  --wait     seconds to wait for the system's first stage to print that it switched root (default 120)
  --keep     keep the scratch folder (disk copies and serial logs) and print its path

Each layout is made on a PRIVATE COPY of the image (never the image itself): the loader is copied to EFI/BOOT/BOOTX64.EFI (and to the shim's second-stage
name when the volume has one), so the firmware starts the loader directly and GRUB is never involved; then boot.cfg and the RAM disk names are set for the
layout. The serial console must show the loader's own lines (it printed its menu or its start, no GRUB), then the kernel's first stage `switching root`
(or `root layers mounted` / `root boot/root.sfs mounted` first). Exit 0 when every layout booted, 1 otherwise, 77 when a needed tool is missing.
"""
import argparse, json, os, re, shutil, socket, subprocess, sys, tempfile, time

def tool(name):
    p = shutil.which(name) or (f"/usr/sbin/{name}" if os.access(f"/usr/sbin/{name}", os.X_OK) else None)
    if not p:
        print(f"qemu-loader-boot-test: {name} is not installed (dist/release/setup-linux-host.sh installs what the tests need)", file=sys.stderr)
        sys.exit(77)
    return p

def run(cmd, **kw):
    return subprocess.run(cmd, capture_output=True, text=True, **kw)

def first_partition(img):
    out = run([tool("sfdisk"), "-J", img])
    if out.returncode:
        sys.exit(f"qemu-loader-boot-test: {img} has no partition table: {out.stderr.strip()}")
    parts = json.loads(out.stdout)["partitiontable"]["partitions"]
    return parts[0]["start"] * 512

class Vol:
    """The FAT boot volume inside a disk image, edited with mtools (no mount, no root)."""
    def __init__(self, img):
        self.spec = f"{img}@@{first_partition(img)}"
    def mt(self, prog, *a, check=True):
        r = run([tool(prog), "-i", self.spec, *a])
        if check and r.returncode:
            raise RuntimeError(f"{prog} {' '.join(a)}: {r.stderr.strip() or r.stdout.strip()}")
        return r
    def has(self, path):
        return self.mt("mdir", f"::{path}", check=False).returncode == 0
    def read(self, path):
        return self.mt("mtype", f"::{path}").stdout
    def put(self, local, path):
        self.mt("mcopy", "-o", local, f"::{path}")
    def get(self, path, local):
        self.mt("mcopy", "-o", f"::{path}", local)
    def delete(self, path):
        if self.has(path):
            self.mt("mdel", f"::{path}")
    def mkdir(self, path):
        self.mt("mmd", f"::{path}", check=False)

def make_synthetic(work, kernel=None):
    """A tiny disk: this computer's kernel (EFI stub), a static first stage that prints what the real one prints, the loader's config."""
    kernel = kernel or next((p for p in ("/boot/vmlinuz", "/boot/bzImage") if os.access(p, os.R_OK)), None)
    if not kernel or not os.access(kernel, os.R_OK):
        sys.exit("qemu-loader-boot-test: --synthetic needs a readable EFI-stub kernel: pass --kernel PATH (a copy of /boot/vmlinuz-* the person running this may read), or give a real image")
    cc = tool("gcc"); cpio = tool("cpio")
    src = os.path.join(work, "init.c")
    with open(src, "w") as f:
        f.write('#include <stdio.h>\n#include <unistd.h>\n#include <sys/reboot.h>\nint main(void){puts("stage1: synthetic first stage");puts("switching root");fflush(stdout);sync();reboot(RB_POWER_OFF);for(;;)pause();}\n')
    tree = os.path.join(work, "rd"); os.makedirs(tree)
    r = run([cc, "-static", "-O2", "-o", os.path.join(tree, "init"), src])
    if r.returncode:
        sys.exit("qemu-loader-boot-test: could not build the static test init: " + r.stderr)
    lean = os.path.join(work, "initrd-lean")
    with open(lean, "wb") as o:
        p1 = subprocess.run("echo init | " + cpio + " -o -H newc --quiet", shell=True, cwd=tree, stdout=subprocess.PIPE)
        import gzip
        o.write(gzip.compress(p1.stdout))
    img = os.path.join(work, "synthetic.img")
    subprocess.run([tool("truncate"), "-s", "700M", img], check=True)
    subprocess.run([tool("sgdisk"), "-n", "1:2048:0", "-t", "1:ef00", img], check=True, capture_output=True)
    off = 2048 * 512
    subprocess.run([tool("mkfs.vfat"), "-F", "32", "--offset", "2048", img], check=True, capture_output=True)
    v = Vol(img)
    for d in ("EFI", "EFI/BOOT", "EFI/iyakki-boot", "boot"):
        v.mkdir(d)
    kp = os.path.join(work, "kernel"); shutil.copyfile(kernel, kp)
    v.put(kp, "boot/bzImage"); v.put(lean, "boot/initrd-lean")
    cfg = os.path.join(work, "boot.cfg")
    with open(cfg, "w") as f:
        f.write("timeout 0\nentry iyakki\n  title IYAKKI\n  kernel /boot/bzImage\n  initrd /boot/initrd-lean\n  cmdline console=ttyS0,115200 panic=5 rdinit=/init\n  default\n")
    v.put(cfg, "EFI/iyakki-boot/boot.cfg")
    return img

def set_layout(v, layout, workdir):
    """Make the volume look like `layout` and return the RAM disk name boot.cfg names."""
    have_lean, have_full = v.has("boot/initrd-lean"), v.has("boot/initrd")
    if not have_lean and not have_full:
        raise RuntimeError("the volume has neither /boot/initrd-lean nor /boot/initrd")
    src = os.path.join(workdir, "initrd.src")
    v.get("boot/initrd-lean" if have_lean else "boot/initrd", src)
    named = {"lean": "/boot/initrd-lean", "sfs": "/boot/initrd", "sibling": "/boot/initrd"}[layout]
    present = "/boot/initrd" if layout == "sfs" else "/boot/initrd-lean"
    absent = "/boot/initrd-lean" if layout == "sfs" else "/boot/initrd"
    v.put(src, present.lstrip("/")); v.delete(absent.lstrip("/"))
    cfg = v.read("EFI/iyakki-boot/boot.cfg")
    new, n = re.subn(r"(?m)^(\s*initrd\s+)/boot/initrd(-lean)?\s*$", lambda m: m.group(1) + named, cfg.replace("\r\n", "\n"))
    if n == 0:
        raise RuntimeError("boot.cfg has no `initrd /boot/initrd...` row to set")
    cp = os.path.join(workdir, "boot.cfg"); open(cp, "w", newline="\n").write(new)
    v.put(cp, "EFI/iyakki-boot/boot.cfg")
    return named, present, n

def boot(img, serial, a, name, until=r"switching root"):
    code = next((p for p in ("/usr/share/OVMF/OVMF_CODE_4M.fd", "/usr/share/OVMF/OVMF_CODE.fd") if os.path.exists(p)), None)
    vars_src = next((p for p in ("/usr/share/OVMF/OVMF_VARS_4M.fd", "/usr/share/OVMF/OVMF_VARS.fd") if os.path.exists(p)), None)
    if not code or not vars_src:
        print("qemu-loader-boot-test: OVMF firmware is not installed (apt install ovmf)", file=sys.stderr); sys.exit(77)
    vars_fd = os.path.join(os.path.dirname(serial), name + ".vars.fd"); shutil.copyfile(vars_src, vars_fd)
    kvm = os.access("/dev/kvm", os.R_OK | os.W_OK)
    cmd = [tool("qemu-system-x86_64"), "-machine", "q35", "-accel", "kvm" if kvm else "tcg", "-cpu", "host" if kvm else "max", "-m", "2048", "-smp", "2",
           "-display", "none", "-no-reboot", "-no-user-config", "-nodefaults", "-vga", "std",
           "-drive", f"if=pflash,format=raw,readonly=on,file={code}", "-drive", f"if=pflash,format=raw,file={vars_fd}",
           "-drive", f"if=none,id=d0,file={img},format=raw", "-device", "virtio-blk-pci,drive=d0,bootindex=0",
           "-serial", f"file:{serial}"]
    q = subprocess.Popen(cmd, stdin=subprocess.DEVNULL, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
    t0 = time.monotonic(); text = ""
    try:
        while time.monotonic() - t0 < a.wait and q.poll() is None:
            time.sleep(0.5)
            text = open(serial, "rb").read().decode("latin1")
            if re.search(until, text):
                break
        time.sleep(0.5); text = open(serial, "rb").read().decode("latin1")
    finally:
        if q.poll() is None:
            q.terminate()
            try: q.wait(10)
            except subprocess.TimeoutExpired: q.kill()
    return re.sub(r"\x1b\[[0-9;?]*[A-Za-z]", "", text), time.monotonic() - t0, kvm

def main():
    ap = argparse.ArgumentParser(description="boot an image through iyakki-boot (not GRUB) in lean, sfs and sibling layouts")
    ap.add_argument("image", nargs="?")
    ap.add_argument("--synthetic", action="store_true")
    ap.add_argument("--layout", default="all", choices=("lean", "sfs", "sibling", "all"))
    ap.add_argument("--kernel", help="--synthetic: the kernel to boot (a bzImage with the EFI stub)"); ap.add_argument("--loader"); ap.add_argument("--wait", type=float, default=120); ap.add_argument("--keep", action="store_true")
    a = ap.parse_args()
    if not a.synthetic and not (a.image and os.path.isfile(a.image)):
        ap.error("give IMAGE.img (a lean image) or --synthetic")
    if a.loader and not os.path.isfile(a.loader):
        ap.error(f"no such loader: {a.loader}")
    for t in ("mcopy", "mdir", "mtype", "mdel", "mmd", "sfdisk", "qemu-system-x86_64"):
        tool(t)
    base = os.environ.get("TMPDIR", "/tmp")
    work = tempfile.mkdtemp(prefix="iyakki-loader-boot-", dir=base)
    layouts = ["lean", "sfs"] + (["sibling"] if a.loader else []) if a.layout == "all" else [a.layout]
    if "sibling" in layouts and not a.loader:
        sys.exit("qemu-loader-boot-test: --layout sibling needs --loader (a loader build that has the sibling rule)")
    results = []
    try:
        src = make_synthetic(work, a.kernel) if a.synthetic else os.path.abspath(a.image)
        for layout in layouts:
            img = os.path.join(work, f"{layout}.img")
            # sparse-aware copy: the image is only ever read
            subprocess.run(["cp", "--sparse=always", src, img], check=True)
            try:
                v = Vol(img)
                loader = os.path.join(work, "loader.efi")
                if a.loader:
                    shutil.copyfile(a.loader, loader)
                elif v.has("EFI/iyakki-boot/loader.efi"):
                    v.get("EFI/iyakki-boot/loader.efi", loader)
                else:
                    raise RuntimeError("the image has no EFI/iyakki-boot/loader.efi (build it with the loader staged, or pass --loader)")
                v.mkdir("EFI/BOOT")
                v.put(loader, "EFI/BOOT/BOOTX64.EFI")
                if v.has("EFI/BOOT/grubx64.efi"):
                    v.put(loader, "EFI/BOOT/grubx64.efi")   # the shim path: shim's second stage by name (Secure Boot is off in this test)
                v.put(loader, "EFI/iyakki-boot/loader.efi")
                named, present, _ = set_layout(v, layout, work)
                text, secs, kvm = boot(img, os.path.join(work, layout + ".serial"), a, layout)
                bad = []
                if re.search(r"GNU GRUB|Booting `|grub>", text):
                    bad.append("GRUB showed up: the loader was not what started")
                if not re.search(r"switching root", text):
                    bad.append("the first stage never printed `switching root`")
                sib = re.search(rf"initrd: {re.escape(named)} cannot be read; {re.escape(present)} is used instead", text)
                if layout == "sibling" and not sib:
                    bad.append("the loader did not log the sibling line")
                if layout != "sibling" and re.search(r"cannot be read", text):
                    bad.append("the loader could not read a file the layout has")
                results.append((layout, not bad, "; ".join(bad), secs, kvm))
                if bad:
                    print(f"--- {layout}: last serial lines ---\n" + "\n".join(text.splitlines()[-25:]))
            except Exception as e:
                results.append((layout, False, f"{type(e).__name__}: {e}", 0.0, False))
            finally:
                if os.path.exists(img) and not a.keep:
                    os.unlink(img)
    finally:
        if a.keep:
            print("kept", work)
        else:
            shutil.rmtree(work, ignore_errors=True)
    for layout, ok, why, secs, kvm in results:
        print(f"{'PASS' if ok else 'FAIL'}  {layout} layout through iyakki-boot ({secs:.0f} s, {'KVM' if kvm else 'TCG'}){'  -> ' + why if why else ''}")
    sys.exit(0 if results and all(r[1] for r in results) else 1)

if __name__ == "__main__":
    main()
