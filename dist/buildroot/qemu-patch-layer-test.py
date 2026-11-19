#!/usr/bin/env python3
"""Hot-patch layers over the root at boot (F-DEVMODE-09), on a real lean boot through IYAKKI's own boot loader and stage 1.

    python3 dist/buildroot/qemu-patch-layer-test.py LEAN-IMAGE.img [--loader BOOT-UEFI.EFI] [--wait SECONDS]

LEAN-IMAGE is an `iyakki-lean-x86_64.img` of lean-image.sh (seed root: boot/root.sfs on the boot volume, a small data partition that the first start formats). Every case works on its OWN
PRIVATE COPY of the image (never the image itself), in this order: (1) boot once so the data partition becomes the IYAKKI data volume; (2) plant a patch layer on it with `debugfs`
(`iyakki/patches/active`, `p1/layer.txt`, a file under `p1/layer/`, optionally `boot-count`); (3) boot again and read stage 1's own lines on the serial console.
  A  a whole layer           -> stage 1 says `1 patch layer(s) over the root` and the system comes up (`supervisor up`)
  B  a damaged layer.txt     -> stage 1 says `patch p1 skipped: ...`, no overlay, the system still comes up
  C  boot-count = 3          -> stage 1 writes `disabled`, uses no patch, the system still comes up
Exit 0 when all three behave so, 1 otherwise, 77 when a tool is missing. Not covered: the content of the patched file inside the running system (that needs a shell into it), a patch
made by `iyakki dev patch` itself, the 30 second good-start mark that clears boot-count.
"""
import importlib.util, os, re, shutil, subprocess, sys, tempfile, time

here = os.path.dirname(os.path.abspath(__file__))
spec = importlib.util.spec_from_file_location("loadertest", os.path.join(here, "qemu-loader-boot-test.py"))
lt = importlib.util.module_from_spec(spec)
sys.modules["loadertest"] = lt
spec.loader.exec_module(lt)


class A:  # the option object boot() reads
    wait = 150.0


def main():
    args = sys.argv[1:]
    img = next((a for a in args if a.endswith(".img")), None)
    if not img or not os.path.isfile(img):
        sys.exit(__doc__)
    loader = args[args.index("--loader") + 1] if "--loader" in args else None
    if "--wait" in args:
        A.wait = float(args[args.index("--wait") + 1])
    for t in ("debugfs", "sfdisk", "mcopy", "qemu-system-x86_64"):
        lt.tool(t)
    work = tempfile.mkdtemp(prefix="iyakki-patch-layer-", dir=os.environ.get("TMPDIR", "/tmp"))
    results = []
    GOOD = "iyakki-patch-layer 1\nid=p1\nbase=*\nfile 8 /etc/iyakki-patch-test\n"
    try:
        out = subprocess.run(["sfdisk", "-d", img], capture_output=True, text=True).stdout
        starts = [int(m.group(1)) for m in re.finditer(r"start=\s*(\d+)", out)]
        data_off = starts[-1] * 512
        for case, layer, count, expect, forbid in (
            ("A whole layer", GOOD, None, [r"1 patch layer\(s\) over the root", r"supervisor up"], [r"patch p1 skipped"]),
            ("B damaged layer.txt", "garbage\n", None, [r"patch p1 skipped", r"supervisor up"], [r"patch layer\(s\) over the root"]),
            ("C boot-count 3", GOOD, "3", [r"supervisor up"], [r"patch layer\(s\) over the root"]),
        ):
            name = case.split()[0]
            cp = os.path.join(work, f"{name}.img")
            subprocess.run(["cp", "--sparse=always", img, cp], check=True)
            v = lt.Vol(cp)
            ldr = os.path.join(work, "loader.efi")
            if loader:
                shutil.copyfile(loader, ldr)
            else:
                v.get("EFI/iyakki-boot/loader.efi", ldr)
            v.mkdir("EFI/BOOT"); v.put(ldr, "EFI/BOOT/BOOTX64.EFI")
            lt.set_layout(v, "lean", work)
            ser1 = os.path.join(work, f"{name}-1.serial")
            text1, secs1, _ = lt.boot(cp, ser1, A, f"{name}1", until=r"persistent volume .* mounted|no data")
            ok1 = bool(re.search(r"persistent volume .* mounted", text1))
            files = {"active": "p1\n", "layer.txt": layer, "marker": "patched\n"}
            for k, t in files.items():
                open(os.path.join(work, k), "w").write(t)
            if count:
                open(os.path.join(work, "count"), "w").write(count + "\n")
            cmds = ["mkdir iyakki", "mkdir iyakki/patches", "mkdir iyakki/patches/p1", "mkdir iyakki/patches/p1/layer", "mkdir iyakki/patches/p1/layer/etc",
                    f"write {work}/active iyakki/patches/active", f"write {work}/layer.txt iyakki/patches/p1/layer.txt", f"write {work}/marker iyakki/patches/p1/layer/etc/iyakki-patch-test"]
            if count:
                cmds.append(f"write {work}/count iyakki/patches/boot-count")
            r = subprocess.run(["debugfs", "-w", "-f", "-", f"{cp}?offset={data_off}"], input="\n".join(cmds) + "\n", capture_output=True, text=True)
            ser2 = os.path.join(work, f"{name}-2.serial")
            text2, secs2, _ = lt.boot(cp, ser2, A, f"{name}2", until=r"supervisor up")
            text2 = re.sub(r"\x1b\[[0-9;?]*[A-Za-z]", "", text2).replace("\r", "")
            lines = [l for l in text2.splitlines() if "patch" in l.lower() or "stage1: root" in l or "supervisor up" in l]
            bad = [] if ok1 else ["the first boot did not make a data volume"]
            bad += [f"missing /{e}/" for e in expect if not re.search(e, text2)]
            bad += [f"found /{f}/" for f in forbid if re.search(f, text2)]
            results.append((case, not bad, "; ".join(bad), lines))
            for l in lines:
                print(f"    [{name}] {l.strip()[:150]}")
            if not bad:
                print(f"PASS  {case} ({secs1:.0f} s + {secs2:.0f} s)")
            else:
                print(f"FAIL  {case}: {'; '.join(bad)}\n" + "\n".join(text2.splitlines()[-20:]))
    finally:
        shutil.rmtree(work, ignore_errors=True)
    sys.exit(0 if results and all(r[1] for r in results) else 1)


if __name__ == "__main__":
    main()
