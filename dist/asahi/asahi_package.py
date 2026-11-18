#!/usr/bin/env python3
"""Builds and checks an OS package for the Asahi installer (github.com/AsahiLinux/asahi-installer, MIT).

  asahi_package.py make     --esp DIR --data-img FILE --out DIR [--version V] [--name NAME] [--fw 13.5,14.8.3 ...]
  asahi_package.py validate --zip FILE --entry FILE.json

Writes  <out>/os/iyakki-apple-<V>.zip  and  <out>/installer_data.json  (one os_list entry).  Stdlib only.
The package contains no Apple firmware: the installer extracts that from the user's own macOS onto the ESP (vendorfw/).
Format as read from asahi-installer src/osinstall.py and the reference installer_data.json (see docs/APPLE-SILICON.md)."""
import argparse, json, os, re, sys, zipfile

ALIGN = 16384            # Apple Silicon page size; partitions are aligned to it
SECTOR = 4096            # the installer refuses an image whose size is not a multiple of 4 KiB
ESP_REQUIRED = ["m1n1/boot.bin", "EFI/BOOT/BOOTAA64.EFI"]
PART_TYPES = {"EFI", "Linux", "APFS"}
# macOS versions the reference installer data lists today; an entry may only name versions the installer knows.
DEFAULT_FW = ["13.5", "14.8.3"]

def up(v, a=ALIGN):
    return (v + a - 1) // a * a

def make(a):
    esp = os.path.abspath(a.esp)
    missing = [p for p in ESP_REQUIRED if not os.path.isfile(os.path.join(esp, p))]
    if missing:
        sys.exit("ESP tree is missing: " + ", ".join(missing))
    size = os.path.getsize(a.data_img)
    if size % SECTOR:
        sys.exit(f"{a.data_img}: size {size} is not a multiple of 4096")
    os.makedirs(os.path.join(a.out, "os"), exist_ok=True)
    zname = f"iyakki-apple-{a.version}.zip"
    zpath = os.path.join(a.out, "os", zname)
    esp_bytes = 0
    with zipfile.ZipFile(zpath, "w", zipfile.ZIP_DEFLATED) as z:
        for root, dirs, files in os.walk(esp):
            dirs.sort()
            for f in sorted(files):
                full = os.path.join(root, f)
                rel = os.path.relpath(full, esp).replace(os.sep, "/")
                z.write(full, "esp/" + rel)
                esp_bytes += os.path.getsize(full)
        z.write(a.data_img, "data.img", compress_type=zipfile.ZIP_STORED)
    # 512 MB ESP like the reference entries, or bigger when the kernel and initramfs need more (room for vendorfw/ ~ 70 MB)
    esp_size = max(524288000, up(esp_bytes + 128 * 1024 * 1024))
    entry = {
        "name": a.name,
        "default_os_name": "IYAKKI",
        "boot_object": "m1n1.bin",
        "next_object": "m1n1/boot.bin",
        "package": zname,
        "supported_fw": a.fw,
        "partitions": [
            {"name": "EFI", "type": "EFI", "size": f"{esp_size}B", "format": "fat", "volume_id": a.volume_id,
             "copy_firmware": True, "copy_installer_data": True, "source": "esp"},
            {"name": "Data", "type": "Linux", "size": f"{up(size)}B", "expand": True, "image": "data.img"},
        ],
    }
    with open(os.path.join(a.out, "installer_data.json"), "w") as f:
        json.dump({"os_list": [entry]}, f, indent=2)
        f.write("\n")
    problems = check(zpath, entry)
    if problems:
        sys.exit("\n".join("PROBLEM: " + p for p in problems))
    print(f"wrote {zpath} and installer_data.json")

def psize(s):
    m = re.fullmatch(r"(\d+)B", s)
    return int(m.group(1)) if m else None

def check(zip_path, entry):
    """Returns the list of problems with an os_list entry and its package (empty = fine)."""
    p = []
    for k in ("name", "default_os_name", "boot_object", "package", "supported_fw", "partitions"):
        if k not in entry:
            p.append(f"missing key {k}")
    if p:
        return p
    if entry["boot_object"] != "m1n1.bin":
        p.append("boot_object must be m1n1.bin (the installer ships it in its own boot/ folder)")
    if entry.get("next_object") and entry["next_object"] != "m1n1/boot.bin":
        p.append("next_object must be m1n1/boot.bin inside the ESP (m1n1 stage 2 + device trees + U-Boot)")
    if not entry["supported_fw"] or not all(re.fullmatch(r"\d+(\.\d+){1,2}", v) for v in entry["supported_fw"]):
        p.append("supported_fw must be a non-empty list of macOS versions such as 13.5")
    if re.search(r"UNPINNED", json.dumps(entry)):
        p.append("entry still contains an UNPINNED marker")
    try:
        z = zipfile.ZipFile(zip_path)
    except Exception as e:
        return p + [f"cannot open the package: {e}"]
    names = set(z.namelist())
    parts = entry["partitions"]
    efi = [x for x in parts if x.get("type") == "EFI"]
    if len(efi) != 1:
        p.append("exactly one EFI partition is required (m1n1 chainloads from it)")
    expand = [x for x in parts if x.get("expand")]
    if len(expand) != 1:
        p.append("exactly one partition must have expand: true")
    for x in parts:
        for k in ("name", "type", "size"):
            if k not in x:
                p.append(f"partition {x.get('name', '?')}: missing {k}")
        if x.get("type") not in PART_TYPES:
            p.append(f"partition {x.get('name')}: type {x.get('type')!r} unknown")
        sz = psize(str(x.get("size", "")))
        if sz is None:
            p.append(f"partition {x.get('name')}: size must look like 524288000B")
            continue
        if x.get("format") not in (None, "fat"):
            p.append(f"partition {x.get('name')}: only format fat is supported by the installer")
        if x.get("format") == "fat" and "volume_id" in x and not re.fullmatch(r"0x[0-9a-fA-F]{1,8}", x["volume_id"]):
            p.append("volume_id must look like 0x264e2b13")
        img = x.get("image")
        if img:
            if img not in names:
                p.append(f"image {img} is not in the package")
            else:
                isz = z.getinfo(img).file_size
                if isz % SECTOR:
                    p.append(f"image {img}: size {isz} is not a multiple of 4096")
                if isz > sz:
                    p.append(f"image {img} ({isz}) is larger than its partition ({sz})")
        src = x.get("source")
        if src:
            pre = src.rstrip("/") + "/"
            for need in ESP_REQUIRED:
                if pre + need not in names:
                    p.append(f"{pre}{need} is not in the package")
            if x.get("type") != "EFI":
                p.append("source: is expected on the EFI partition")
    for n in names:
        if n.startswith("/") or ".." in n.split("/"):
            p.append(f"unsafe path in the package: {n}")
        if re.search(r"(^|/)(vendorfw|firmware\.cpio|firmware\.tar)(/|$)|AppleCameraInterface|\.ipsw$", n):
            p.append(f"Apple firmware must not be shipped in the package: {n}")
    return p

def validate(a):
    with open(a.entry) as f:
        data = json.load(f)
    problems = []
    for e in data.get("os_list", []):
        problems += check(a.zip, e)
    if not data.get("os_list"):
        problems.append("os_list is empty")
    if problems:
        print("\n".join("PROBLEM: " + x for x in problems))
        return 1
    print("OK")
    return 0

if __name__ == "__main__":
    ap = argparse.ArgumentParser()
    sub = ap.add_subparsers(dest="cmd", required=True)
    m = sub.add_parser("make")
    m.add_argument("--esp", required=True); m.add_argument("--data-img", required=True); m.add_argument("--out", required=True)
    m.add_argument("--version", default="0.0.0"); m.add_argument("--name", default="IYAKKI (alpha)")
    m.add_argument("--fw", type=lambda s: s.split(","), default=DEFAULT_FW)
    m.add_argument("--volume-id", default="0x1a4c61b0")
    v = sub.add_parser("validate")
    v.add_argument("--zip", required=True); v.add_argument("--entry", required=True)
    a = ap.parse_args()
    if a.cmd == "make":
        make(a)
    else:
        sys.exit(validate(a))
