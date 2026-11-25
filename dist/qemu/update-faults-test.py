#!/usr/bin/env python3
"""Fault injection for a system update, on a copy of the real install stick, started by the REAL GRUB in QEMU (F-UPD-28).

    python3 dist/qemu/update-faults-test.py [build-output-dir] [--out DIR] [--only control,done,...] [--uefi] [--grub-core FILE]

Needs: the built install stick image (images/iyakki-install-x86_64.img), qemu-system-x86_64 (KVM recommended), mtools (mcopy, mtype), sfdisk, cargo, python3. Runs ONLY on the
Linux build host, one at a time, never inside the release gate. The stick is copied first; nothing real is written.

For each case the boot partition of a fresh copy is put into the state a fault leaves: the REAL update engine (`devmode::update::apply`, the ignored test
`update_fault_state_on_a_folder_for_a_real_grub_run` of iyakki-agent, with its power-cut knob) runs on a folder with the stick's boot files, and the folder is copied back.
Then the copy is started three times by the real GRUB, and the kernel's own command line says which entry GRUB chose:
  control     no update: normal, normal, normal
  done        a finished update, armed by the update centre, nobody marks it good: normal (counter 2), previous version by itself, normal
  cut-initrd  power cut while the initramfs is replaced     \\
  cut-kernel  power cut while the kernel is replaced         > the counter was armed before the first file changed: normal, previous version by itself, normal
  cut-menu    power cut while the menu is rewritten         /
PASS = every start reached the kernel (the machine boots) and the entries are the ones in the table. The previous-version entry of the copy is the normal entry plus
`iyakki.entry=previous` (the saved files live on the data volume, which a stick does not have; only the choice is tested, as in boot-counter-test.sh).
`--grub-core FILE` writes that BIOS GRUB core (grub.img, from `dist/buildroot/grub-refresh.sh DONOR OUTDIR`) into the copy of the stick first, for a stick whose own GRUB was made
before `loadenv` was in the module lists (every image repacked from a donor older than 2026-11-14: its GRUB says "can't find command `load_env'" and the counter never steps; Q6).
Without it the script runs the stick as it is, and a counter that stays at 1 is reported with that hint. The report is written in the autotest result format (dist/autotest/results.py), level `qemu`, under OUT/F-UPD-28/qemu/ (default ~/iyakki-work/autotest/<date-time>/).
LOADER LAYOUT (`--loader`, LDR2, F-UPD-31): the same engine on the same stick, but the stick is the loader-default one (EFI/BOOT/BOOTX64.EFI = iyakki-boot, grub-recovery.efi, EFI/iyakki-boot/loader.efi,
boot.cfg and, after an update, prev/ with a marker), started by the loader under OVMF with a vars file of its own per case, so the loader's tries (UEFI variable BootTries) and its
BootConfirm survive from one start to the next. Each case is started until the loader falls back to the previous version by itself, or three times when it has none:
  control          no update: current x3, no previous-version row
  done             a finished update (new set), nobody confirms: current x tries (9), then the previous version at start tries+1
  cut-initrd       power cut while the initramfs is replaced (old set whole, previous copy whole)     \\
  cut-kernel       power cut while the kernel is replaced (new initramfs + old kernel = a mixed set)    > the same sequence as done
  cut-menu         power cut while the menu is rewritten (new set whole)                                /
  cut-prev-kernel  power cut while the loader's copy of the old kernel is written   \\
  cut-prev-initrd  ... of the old initramfs                                          > old set still stands, NO marker: no previous row is offered over a half copy; current x3
  cut-prev-marker  ... at the marker (both copies whole, marker missing)            /
PASS = the first start runs the set the case names (kernel command line printed), the fall-back comes at start tries+1 with the previous kernel running (`iyakki.entry=previous` on its command
line, added by this script to the copy's boot.cfg together with loglevel=7), the copy in prev/ is byte for byte the old set, and a cut copy never has a marker. The loader's EFI/iyakki-boot/last-boot.log
(F-LDR-09) of every start is kept as evidence. Needs the QEMU lock `~/iyakki-work/.qemu.lock` (held for the starts of one case, not for the whole run).
Not covered: a power cut on real hardware (dist/autotest/checklists/F-UPD-29.json), the data volume's roots and pointers (unit tests, `tests_update_faults`), a signed update
over the network, the shell after the start.
"""
import fcntl, hashlib, os, re, shutil, subprocess, sys, tempfile, time

here = os.path.dirname(os.path.abspath(__file__)); root = os.path.abspath(os.path.join(here, "..", ".."))
sys.path.insert(0, os.path.join(root, "dist", "autotest"))
import results as R  # noqa: E402

CASES = {  # name: (counter state the engine leaves, GRUB entries of three starts)
    "control": ("Idle", ["normal", "normal", "normal"]),
    "done": ("Armed", ["normal", "previous", "normal"]),
    "cut-initrd": ("Armed", ["normal", "previous", "normal"]),
    "cut-kernel": ("Armed", ["normal", "previous", "normal"]),
    "cut-menu": ("Armed", ["normal", "previous", "normal"]),
}
TIMEOUT = int(os.environ.get("TIMEOUT", "240"))
# the loader layout (--loader): name: (files of the boot volume after the engine, does the loader find a previous-version marker, expected prev/ listing kind)
LOADER_CASES = {
    "control": ("old", False, "none"),
    "done": ("new", True, "whole"),
    "cut-initrd": ("old", True, "whole"),
    "cut-kernel": ("mixed", True, "whole"),
    "cut-menu": ("new", True, "whole"),
    "cut-prev-kernel": ("old", False, "kernel-cut"),
    "cut-prev-initrd": ("old", False, "initrd-cut"),
    "cut-prev-marker": ("old", False, "marker-cut"),
}
QEMU_LOCK = os.path.join(os.path.expanduser("~"), "iyakki-work", ".qemu.lock")


def need(tool):
    if not shutil.which(tool):
        sys.exit("needs %s (see dist/qemu/README.md)" % tool)


ENGINE_TEST = "update_fault_state_on_a_folder_for_a_real_grub_run"


def run_engine(env):
    """The ignored test of iyakki-agent that runs the update engine. With IYAKKI_FAULT_TEST_BIN (the test binary of `cargo test -p iyakki-agent --lib --no-run`, made on the build host and
    copied here) it runs that binary; without it cargo builds and runs it in this checkout."""
    binary = os.environ.get("IYAKKI_FAULT_TEST_BIN")
    if binary:
        return run([binary, "--ignored", "--nocapture", ENGINE_TEST], cwd=root, env=env)
    return run(["cargo", "test", "--locked", "-q", "-p", "iyakki-agent", "--lib", "--", "--ignored", "--nocapture", ENGINE_TEST], cwd=root, env=env)


def run(cmd, **kw):
    return subprocess.run(cmd, capture_output=True, text=True, **kw)


def boot_offset(img):
    out = run(["sfdisk", "-d", img]).stdout
    m = re.search(r"^[^ ]*1 *: *start= *(\d+),", out, re.M)
    if not m:
        sys.exit("no boot partition on the stick image")
    return int(m.group(1)) * 512


def fix_menu(cfg):
    """The previous-version entry of the copy: the normal entry with `iyakki.entry=previous` (the engine's own entry names files on the data volume, which a stick
    does not have); the first entry prints its command line. The guard of the engine stays as it is."""
    m = re.search(r'^menuentry "IYAKKI" \{.*?^\}\n', cfg, re.S | re.M)
    if not m:
        sys.exit("no normal entry in the stick's menu")
    normal = m.group(0)
    first = normal.replace("quiet loglevel=3", "loglevel=7")
    prev = normal.replace('menuentry "IYAKKI"', 'menuentry "IYAKKI (previous version)"').replace("quiet loglevel=3", "loglevel=7 iyakki.entry=previous")
    old = re.search(r'^menuentry "IYAKKI \(previous version\)[^\n]*\{.*?^\}\n', cfg, re.S | re.M)
    cfg = cfg.replace(normal, first + ("" if old else "\n" + prev), 1)
    if old:
        cfg = cfg.replace(old.group(0), prev, 1)
    return cfg.replace('set timeout="3"', 'set timeout="1"')


def put_grub_core(img, core):
    """The BIOS core image goes where genimage puts it, at sector 1 of the stick (the gap before the first partition)."""
    if os.path.getsize(core) > 2047 * 512:
        sys.exit("the GRUB core %s does not fit before the first partition" % core)
    r = run(["dd", "if=" + core, "of=" + img, "bs=512", "seek=1", "conv=notrunc"])
    if r.returncode:
        sys.exit("cannot write the GRUB core: " + r.stderr)


def engine(case, work, vol):
    """Run the real update engine on the folder, copy the folder back; returns the FAULT-STATE line."""
    part = os.path.join(work, "part")
    os.makedirs(os.path.join(part, "boot/grub"), exist_ok=True)
    for f in ("bzImage", "initrd", "iyakki-release", "grub/grub.cfg"):
        r = run(["mcopy", "-o", "-i", vol, "::boot/" + f, os.path.join(part, "boot", f)])
        if r.returncode and f == "iyakki-release":
            # the sticks of the 2026-11 builds carry no boot/iyakki-release (Q1 found it: the first real run stopped here); the engine only compares the two versions, so the stick gets a
            # plain old version. The file is written back with the other boot files below.
            open(os.path.join(part, "boot", f), "w").write("VERSION=0.0.0-stick\nVARIANT=pc\nBUILD_DATE=\n")
            continue
        if r.returncode:
            sys.exit("cannot read boot/%s from the stick: %s" % (f, r.stderr))
    env = dict(os.environ, IYAKKI_FAULT_DIR=part, IYAKKI_FAULT_CASE=case)
    r = run_engine(env)
    line = next((l for l in (r.stdout + r.stderr).splitlines() if l.startswith("FAULT-STATE")), "")
    if r.returncode or not line:
        sys.exit("the engine step for %s failed:\n%s" % (case, (r.stdout + r.stderr)[-1500:]))
    cfgp = os.path.join(part, "boot/grub/grub.cfg")
    fixed = fix_menu(open(cfgp).read())      # read BEFORE the write opens (and empties) the file: `open(p, "w").write(f(open(p).read()))` handed fix_menu an empty text (Q1)
    open(cfgp, "w").write(fixed)
    for rel in ("boot/bzImage", "boot/initrd", "boot/iyakki-release", "boot/grub/grub.cfg", "boot/grub/grubenv"):
        p = os.path.join(part, rel)
        if os.path.isfile(p):
            rc = run(["mcopy", "-o", "-i", vol, p, "::" + rel])
            if rc.returncode:
                sys.exit("cannot write %s back: %s" % (rel, rc.stderr))
    return line


def counter(vol):
    out = run(["mtype", "-i", vol, "::boot/grub/grubenv"]).stdout.replace("\0", "")
    m = re.search(r"^iyakki_trial=(\d)", out, re.M)
    return m.group(1) if m else ""


def start(img, log, uefi, work):
    cmd = ["qemu-system-x86_64", "-accel", "kvm", "-accel", "tcg", "-m", "2048", "-smp", "2", "-display", "none", "-no-reboot",
           "-drive", "file=%s,format=raw,if=none,id=d" % img, "-device", "usb-ehci", "-device", "usb-storage,drive=d,bootindex=0", "-serial", "file:" + log]
    if uefi:
        cmd += ["-drive", "if=pflash,format=raw,readonly=on,file=/usr/share/OVMF/OVMF_CODE_4M.fd", "-drive", "if=pflash,format=raw,file=" + os.path.join(work, "vars.fd")]
    open(log, "w").close()
    p = subprocess.Popen(cmd, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
    t0 = time.time()
    try:
        while time.time() - t0 < TIMEOUT:
            if "Kernel command line:" in open(log, errors="replace").read():
                break
            if p.poll() is not None:
                break
            time.sleep(1)
        time.sleep(1)
    finally:
        p.kill(); p.wait()
    text = open(log, errors="replace").read().replace("\r", "")
    line = next((l for l in text.splitlines() if "Kernel command line:" in l), None)
    if line is None:
        return None
    return "previous" if "iyakki.entry=previous" in line else "normal"


# ----------------------------------------------------------------------------------------------------------------------------- the loader layout (LDR2)

def fix_loader_cfg(cfg):
    """The copy's boot.cfg: the kernel prints its own command line (loglevel=7 instead of quiet), and the previous-version rows say `iyakki.entry=previous` on it, so the serial log
    names the set that really ran. Nothing else changes (tries, requires, the order of the entries stay as the image has them)."""
    out, cur = [], None
    for line in cfg.split("\n"):
        m = re.match(r"entry (\S+)", line)
        if m:
            cur = m.group(1)
        if cur in ("iyakki", "prev-plain", "prev-oldroot") and line.strip().startswith("cmdline "):
            line = line.replace("quiet loglevel=3", "loglevel=7")
            if cur != "iyakki":
                line += " iyakki.entry=previous"
        out.append(line)
    return "\n".join(out)


def fat_sha(vol, rel, size=False):
    """SHA-256 of a file of the FAT volume (and its size with `size=True`); empty when the file is not there."""
    p = subprocess.Popen(["mtype", "-i", vol, "::" + rel], stdout=subprocess.PIPE, stderr=subprocess.DEVNULL)
    h, n = hashlib.sha256(), 0
    for chunk in iter(lambda: p.stdout.read(1 << 20), b""):
        h.update(chunk)
        n += len(chunk)
    p.wait()
    d = h.hexdigest() if p.returncode == 0 else ""
    return (d, n) if size else d


def engine_loader(case, work, vol, capacity):
    """The real update engine on a folder with the stick's boot files and its `EFI/iyakki-boot/boot.cfg` (so the engine sees a loader layout and keeps the old set in prev/ itself);
    everything the engine left in the folder (also the half files of a cut) is written back into the FAT volume. Returns the FAULT-STATE line."""
    part = os.path.join(work, "part")
    os.makedirs(os.path.join(part, "boot/grub"), exist_ok=True)
    os.makedirs(os.path.join(part, "EFI/iyakki-boot"), exist_ok=True)
    for f in ("boot/bzImage", "boot/initrd", "boot/iyakki-release", "boot/grub/grub.cfg", "EFI/iyakki-boot/boot.cfg"):
        r = run(["mcopy", "-o", "-i", vol, "::" + f, os.path.join(part, f)])
        if r.returncode and f == "boot/iyakki-release":
            open(os.path.join(part, f), "w").write("VERSION=0.0.0-stick\nVARIANT=pc\nBUILD_DATE=\n")
            continue
        if r.returncode:
            sys.exit("cannot read %s from the stick: %s" % (f, r.stderr))
    env = dict(os.environ, IYAKKI_FAULT_DIR=part, IYAKKI_FAULT_CASE=case, IYAKKI_FAULT_CAPACITY=str(capacity))
    r = run_engine(env)
    line = next((l for l in (r.stdout + r.stderr).splitlines() if l.startswith("FAULT-STATE")), "")
    if r.returncode or not line:
        sys.exit("the engine step for %s failed:\n%s" % (case, (r.stdout + r.stderr)[-1500:]))
    cfgp = os.path.join(part, "EFI/iyakki-boot/boot.cfg")
    fixed = fix_loader_cfg(open(cfgp).read())
    open(cfgp, "w").write(fixed)
    for dirpath, dirs, files in os.walk(part):
        rel_dir = os.path.relpath(dirpath, part)
        if rel_dir != ".":
            run(["mmd", "-i", vol, "::" + rel_dir])      # exists already: refused, fine
        for f in files:
            rel = os.path.normpath(os.path.join(rel_dir, f))
            rc = run(["mcopy", "-o", "-i", vol, os.path.join(dirpath, f), "::" + rel])
            if rc.returncode:
                sys.exit("cannot write %s back: %s" % (rel, rc.stderr))
    return line


def parse_state(line):
    d = dict(re.findall(r'(\w+)=(\[[^\]]*\]|"[^"]*"|\S+)', line))
    prev = {}
    for item in d.get("prev", "[]").strip("[]").split(","):
        if ":" in item:
            k, v = item.rsplit(":", 1)
            prev[k] = int(v)
    d["prev_files"] = prev
    d["marker_text"] = d.get("marker", '""').strip('"')
    return d


def start_loader(img, vars_fd, log, vol, keep):
    """One start of the stick by the loader under OVMF (persistent variables in vars_fd); the caller holds the QEMU lock. Ends when the kernel prints its
    command line. Returns what the loader logged: the entry, the tree, the tries left, whether the kernel ran, and the entries it offered."""
    cmd = ["qemu-system-x86_64", "-accel", "kvm", "-accel", "tcg", "-m", "2048", "-smp", "2", "-display", "none", "-no-reboot",
           "-drive", "file=%s,format=raw,if=none,id=d" % img, "-device", "usb-ehci", "-device", "usb-storage,drive=d,bootindex=0", "-serial", "file:" + log,
           "-drive", "if=pflash,format=raw,readonly=on,file=/usr/share/OVMF/OVMF_CODE_4M.fd", "-drive", "if=pflash,format=raw,file=" + vars_fd]
    open(log, "w").close()
    p = subprocess.Popen(cmd, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
    t0 = time.time()
    try:
        while time.time() - t0 < TIMEOUT:
            if "Kernel command line:" in open(log, errors="replace").read() or p.poll() is not None:
                break
            time.sleep(1)
        time.sleep(1)
    finally:
        p.kill(); p.wait()
    r = run(["mtype", "-i", vol, "::EFI/iyakki-boot/last-boot.log"])
    open(keep, "w").write(r.stdout if r.returncode == 0 else "(no last-boot.log on the volume: %s)\n" % r.stderr.strip())
    text = open(log, errors="replace").read().replace("\r", "")
    starts = [l for l in text.splitlines() if l.startswith("start: ")]
    if not starts:
        return None
    m = re.match(r"start: (\S+) kernel=(\S+) tree=(\S+)", starts[-1])
    st = [l for l in text.splitlines() if l.startswith("state:")]
    tl = re.search(r"tries_left=(\d+)", st[-1]) if st else None
    cmdline = next((l for l in text.splitlines() if "Kernel command line:" in l), "")
    return {"entry": m.group(1), "kernel": m.group(2), "tree": m.group(3), "tries_left": int(tl.group(1)) if tl else -1, "ran": bool(cmdline),
            "previous_cmdline": "iyakki.entry=previous" in cmdline, "items": [l for l in text.splitlines() if l.startswith("item: ")],
            "state": st[-1] if st else "", "decide": next((l for l in text.splitlines() if l.startswith("decide:")), "")}


def run_loader_case(case, stick, work, ev_dir, orig):
    files_want, marker_want, kind = LOADER_CASES[case]
    img, vars_fd = os.path.join(work, case + ".img"), os.path.join(work, case + ".vars.fd")
    subprocess.run(["cp", "--sparse=always", stick, img], check=True)
    shutil.copy("/usr/share/OVMF/OVMF_VARS_4M.fd", vars_fd)
    vol = "%s@@%d" % (img, boot_offset(img))
    shutil.rmtree(os.path.join(work, "part"), ignore_errors=True)
    # the update's room check sees the real volume: its size (the boot partition of the stick) less what the folder does not hold (the EFI files, the splash picture)
    part_sectors = int(re.search(r"^[^ ]*1 *: *start= *\d+, *size= *(\d+),", run(["sfdisk", "-d", img]).stdout, re.M).group(1))
    line = engine_loader(case, work, vol, part_sectors * 512 - (4 << 20))
    st = parse_state(line)
    bad = []
    # 1. the files the engine left on the boot volume
    if st.get("files") != files_want:
        bad.append("the boot files are %s, expected %s" % (st.get("files"), files_want))
    pf = st["prev_files"]
    marker = st["marker_text"]
    if bool(marker) != marker_want:
        bad.append("the previous-version marker is %s, expected %s" % ("there" if marker else "missing", "there" if marker_want else "missing"))
    old_bz, old_ird = orig["bz_size"], orig["ird_size"]
    if kind == "none" and pf:
        bad.append("prev/ is not empty after a run without an update: %s" % pf)
    if kind == "whole":
        if pf.get("bzImage") != old_bz or pf.get("initrd") != old_ird or any(k.endswith(".tmp") for k in pf):
            bad.append("prev/ is not the whole old set: %s" % pf)
        for name, want in (("bzImage", orig["bz"]), ("initrd", orig["ird"])):
            if fat_sha(vol, "EFI/iyakki-boot/prev/" + name) != want:
                bad.append("prev/%s on the volume is not byte for byte the old file" % name)
        if "stamp 1" not in marker:
            bad.append("the marker has no stamp 1: %r" % marker)
    if kind == "kernel-cut" and not (0 < pf.get("bzImage.tmp", 0) < old_bz and "bzImage" not in pf and "initrd" not in pf):
        bad.append("the cut did not land inside the kernel copy: %s" % pf)
    if kind == "initrd-cut" and not (pf.get("bzImage") == old_bz and 0 < pf.get("initrd.tmp", 0) < old_ird and "initrd" not in pf):
        bad.append("the cut did not land inside the initramfs copy: %s" % pf)
    if kind == "marker-cut" and not (pf.get("bzImage") == old_bz and pf.get("initrd") == old_ird and not any(k.endswith(".tmp") for k in pf)):
        bad.append("the cut did not land at the marker: %s" % pf)
    # 2. the starts
    tries = int(re.search(r"^tries (\d+)", subprocess.run(["mtype", "-i", vol, "::EFI/iyakki-boot/boot.cfg"], capture_output=True, text=True).stdout, re.M).group(1))
    limit = tries + 2 if marker_want else 3
    seq, evidence = [], []
    with open(QEMU_LOCK, "a+") as lk:        # one QEMU job at a time on this host: held for the starts of this case only
        fcntl.flock(lk, fcntl.LOCK_EX)
        for n in range(1, limit + 1):
            log = os.path.join(ev_dir, "loader-%s-start%02d.serial.log" % (case, n))
            keep = os.path.join(ev_dir, "loader-%s-start%02d.last-boot.log" % (case, n))
            r = start_loader(img, vars_fd, log, vol, keep)
            evidence += [log, keep]
            seq.append(r)
            if r is None or r["tree"] == "previous":
                break
    names = [("%s/%s" % (r["entry"], r["tree"])) if r else "no start" for r in seq]
    if any(r is None or not r["ran"] for r in seq):
        bad.append("a start never reached the kernel: %s" % names)
    elif marker_want:
        # `tries` counted starts on the current set, then the previous version at start tries+1, and its kernel runs
        cur_ok = all(r["tree"] == "current" and r["entry"] == "iyakki" for r in seq[:tries])
        last = seq[-1]
        if len(seq) != tries + 1 or not cur_ok or last["tree"] != "previous" or not last["entry"].startswith("prev-") or not last["previous_cmdline"]:
            bad.append("expected %d starts of the current set then the previous version, got %s" % (tries, names))
        left = [r["tries_left"] for r in seq]
        if left[:tries] != list(range(tries - 1, -1, -1)):
            bad.append("the loader's tries did not count down one by one: %s" % left)
    else:
        if len(seq) != 3 or any(r["tree"] != "current" or r["entry"] != "iyakki" for r in seq):
            bad.append("expected three starts of the current set, got %s" % names)
        if any(re.search(r'prev-(plain|oldroot) ', i) for r in seq for i in r["items"]):
            bad.append("the loader offers a previous-version row without a marker")
    if marker_want and seq and seq[0] and not any("prev-" in i for i in seq[0]["items"]):
        bad.append("the loader's list has no previous-version row although the marker is there")
    if marker_want:
        summary = "current(%s set) x%d, then %s at start %d" % (files_want, sum(1 for r in seq if r and r["tree"] == "current"), names[-1], len(seq))
    else:
        summary = "current x%d, no previous-version row (marker %s)" % (len(seq), "missing" if not marker else "there")
    line_out = "%s: %s | files=%s prev=%s marker=%r | starts %s | tries_left %s" % (case, summary, st.get("files"), pf, marker, names, [r["tries_left"] for r in seq if r])
    shutil.rmtree(os.path.join(work, "part"), ignore_errors=True)
    os.remove(img)
    return bad, line_out, evidence


def main_loader(args, out_dir, only):
    build = args[0] if args else os.path.join(root, "target", "buildroot-libre")
    stick = os.path.join(build, "images", "iyakki-install-x86_64.img")
    for t in ("qemu-system-x86_64", "mtype", "mcopy", "mmd", "sfdisk") + (() if os.environ.get("IYAKKI_FAULT_TEST_BIN") else ("cargo",)):
        need(t)
    if not os.path.isfile(stick):
        sys.exit("no %s (the install image of the build)" % stick)
    off = boot_offset(stick)
    vol = "%s@@%d" % (stick, off)
    ls = run(["mdir", "-/", "-b", "-i", vol, "::EFI"]).stdout.lower()
    if "grub-recovery.efi" not in ls or "loader.efi" not in ls:
        sys.exit("%s is not a loader-default stick (EFI/BOOT/grub-recovery.efi and EFI/iyakki-boot/loader.efi are needed)" % stick)
    if fat_sha(vol, "EFI/BOOT/BOOTX64.EFI") != fat_sha(vol, "EFI/iyakki-boot/loader.efi"):
        sys.exit("EFI/BOOT/BOOTX64.EFI is not the loader (the stick is GRUB-default)")
    if R.gate_lock_held() and not os.environ.get("IYAKKI_FAULT_IGNORE_GATE"):
        sys.exit("the release gate holds its lock: not now")
    (bz, bz_size), (ird, ird_size) = fat_sha(vol, "boot/bzImage", True), fat_sha(vol, "boot/initrd", True)
    orig = {"bz": bz, "ird": ird, "bz_size": bz_size, "ird_size": ird_size}
    work = tempfile.mkdtemp(prefix="iyakki-faults.", dir=os.environ.get("TMPDIR", "/tmp"))
    evidence_dir = os.path.join(out_dir, "F-UPD-31", "qemu")
    os.makedirs(evidence_dir, exist_ok=True)
    fails, lines, ev = [], [], []
    try:
        for case in only:
            bad, line, evidence = run_loader_case(case, stick, work, evidence_dir, orig)
            lines.append(line)
            ev += [os.path.relpath(e, out_dir) for e in evidence]
            print(("ok: " if not bad else "FAIL: ") + line + "".join("\n   problem: " + b for b in bad))
            lines += ["   problem: " + b for b in bad]
            if bad:
                fails.append(case)
    finally:
        shutil.rmtree(work, ignore_errors=True)
    txt = os.path.join(evidence_dir, "update-faults-loader.txt")
    open(txt, "w").write("\n".join(lines) + "\n")
    ev.append(os.path.relpath(txt, out_dir))
    status = "FAIL" if fails else "PASS"
    rec = R.new_result("F-UPD-31", "update-faults-test.py --loader", "qemu", status, evidence=ev, covered=only, reason=("failed cases: " + ", ".join(fails)) if fails else "",
                       not_covered=["a power cut on real hardware (checklist F-UPD-30)", "the data volume roots and pointers (unit tests)", "a signed update over the network",
                                    "the shell confirming a good start (BootConfirm) in the started guest", "Secure Boot (shim and MOK)", "the cut between the system image pointer and the kernel (unit test: the stick has no data volume)"])
    R.write_result(out_dir, rec)
    print("%s (update-faults --loader, %d cases) report: %s" % (status, len(only), out_dir))
    sys.exit(1 if fails else 0)


def main():
    args = [a for i, a in enumerate(sys.argv[1:]) if not a.startswith("--") and (i == 0 or sys.argv[i] not in ("--out", "--only", "--grub-core"))]
    out_dir = sys.argv[sys.argv.index("--out") + 1] if "--out" in sys.argv else os.path.join(os.path.expanduser("~"), "iyakki-work", "autotest", time.strftime("%Y%m%d-%H%M%S"))
    loader = "--loader" in sys.argv
    only = sys.argv[sys.argv.index("--only") + 1].split(",") if "--only" in sys.argv else list(LOADER_CASES if loader else CASES)
    if loader:
        return main_loader(args, out_dir, only)
    uefi = "--uefi" in sys.argv
    core = sys.argv[sys.argv.index("--grub-core") + 1] if "--grub-core" in sys.argv else None
    build = args[0] if args else os.path.join(root, "target", "buildroot-libre")
    stick = os.path.join(build, "images", "iyakki-install-x86_64.img")
    for t in ("qemu-system-x86_64", "mtype", "mcopy", "sfdisk", "cargo"):
        need(t)
    if not os.path.isfile(stick):
        sys.exit("no %s (the install image of the build)" % stick)
    if R.gate_lock_held() and not os.environ.get("IYAKKI_FAULT_IGNORE_GATE"):
        sys.exit("the release gate holds its lock: not now")
    work = tempfile.mkdtemp(prefix="iyakki-faults.", dir=os.environ.get("TMPDIR", "/tmp"))
    if uefi:
        shutil.copy("/usr/share/OVMF/OVMF_VARS_4M.fd", os.path.join(work, "vars.fd"))
    evidence_dir = os.path.join(out_dir, "F-UPD-28", "qemu")
    os.makedirs(evidence_dir, exist_ok=True)
    fails, lines, ev = [], [], []
    try:
        for case in only:
            want_counter, want = CASES[case]
            img = os.path.join(work, case + ".img")
            subprocess.run(["cp", "--sparse=always", stick, img], check=True)
            if core:
                put_grub_core(img, core)
            vol = "%s@@%d" % (img, boot_offset(img))
            shutil.rmtree(os.path.join(work, "part"), ignore_errors=True)
            state = engine(case, work, vol)
            got = []
            for n in range(3):
                log = os.path.join(evidence_dir, "%s-start%d.serial.log" % (case, n + 1))
                got.append(start(img, log, uefi, work))
                ev.append(os.path.relpath(log, out_dir))
            ok = got == want and all(got)
            now, n0 = counter(vol), len(lines)
            lines.append("%s: %s | starts %s | expected %s | counter now %r" % (case, state, got, want, now))
            if not ok and want_counter == "Armed" and now == "1":
                lines.append("   hint: the counter never stepped, so GRUB did not run the guard: look for \"can't find command `load_env'\" (a GRUB core made before `loadenv` was in the module lists; use --grub-core, see the header)")
            print(("ok: " if ok else "FAIL: ") + "\n".join(lines[n0:]))
            if not ok:
                fails.append(case)
            shutil.rmtree(os.path.join(work, "part"), ignore_errors=True)
            os.remove(img)
    finally:
        shutil.rmtree(work, ignore_errors=True)
    txt = os.path.join(evidence_dir, "update-faults.txt")
    open(txt, "w").write("\n".join(lines) + "\n")
    ev.append(os.path.relpath(txt, out_dir))
    status = "FAIL" if fails else "PASS"
    rec = R.new_result("F-UPD-28", "update-faults-test.py", "qemu", status, evidence=ev, covered=only, reason=("failed cases: " + ", ".join(fails)) if fails else "",
                       not_covered=["a power cut on real hardware (checklist F-UPD-29)", "the data volume roots and pointers (unit tests)", "a signed update over the network",
                                    "the shell after the start"])
    R.write_result(out_dir, rec)
    print("%s (update-faults, %d cases) report: %s" % (status, len(only), out_dir))
    sys.exit(1 if fails else 0)


if __name__ == "__main__":
    main()
