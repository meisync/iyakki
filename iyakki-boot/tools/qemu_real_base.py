#!/usr/bin/env python3
# SPDX-License-Identifier: MIT OR Apache-2.0
"""Build the base disk of the real-kernel tests: a copy of a lean/install image's boot volume (everything under / of partition 1) on a
bigger GPT + FAT32 disk, so that a copy of the previous version (about 17 MB) fits beside the 420 MB system image. Standard library only
(plus sgdisk and mkfs.vfat, as fatdisk.py). The source image is only read.

    qemu_real_base.py SOURCE.img OUT.img [--size-mb 700]
"""
import argparse
import importlib.util
import os
import subprocess
import sys
import tempfile

HERE = os.path.dirname(os.path.abspath(__file__))
spec = importlib.util.spec_from_file_location("fatdisk", os.path.join(HERE, "fatdisk.py"))
fatdisk = importlib.util.module_from_spec(spec)
spec.loader.exec_module(fatdisk)
SECTOR = 512


def walk(v, cluster, path=""):
    for name, isdir, cl, size, _ in v.entries(cluster):
        if name in (".", ".."):
            continue
        if isdir:
            yield path + "/" + name, True, cl, 0
            yield from walk(v, cl, path + "/" + name)
        else:
            yield path + "/" + name, False, cl, size


def main():
    p = argparse.ArgumentParser()
    p.add_argument("source")
    p.add_argument("out")
    p.add_argument("--size-mb", type=int, default=700)
    a = p.parse_args()
    size = a.size_mb << 20
    start = 2048
    fat_bytes = size - (start + 4096) * SECTOR  # room at the end for a 1 MiB second partition (the installer needs one) and the GPT copy
    with tempfile.TemporaryDirectory(dir=os.path.dirname(os.path.abspath(a.out))) as tmp:
        fat = os.path.join(tmp, "esp.fat")
        with open(fat, "wb") as f:
            f.truncate(fat_bytes)
        # 4 KiB clusters (the source image's volume has 8 sectors per cluster too); the label is what the initramfs looks for
        subprocess.run(["mkfs.vfat", "-F", "32", "-s", "8", "-S", "512", "-n", "IYAKKI-BOOT", fat], check=True, stdout=subprocess.DEVNULL)
        src, v_src = fatdisk.open_part(a.source, 1)
        with open(fat, "r+b") as f:
            v = fatdisk.Fat32(f)
            # directories first (so that files land in them), in walk order; big files are the last so a failure shows early
            items = list(walk(v_src, v_src.root))
            for path, isdir, cl, sz in items:
                if isdir:
                    parent, _, name = path.rpartition("/")
                    hit = v.lookup(parent or "/")
                    v.mkdir(hit[0] if hit else v.root, name)
            for path, isdir, cl, sz in items:
                if not isdir:
                    print("copy", path, sz, flush=True)
                    v.put(path, v_src.read_chain(cl, sz))
        with open(a.out, "wb") as img:
            img.truncate(size)
        last = start + fat_bytes // SECTOR - 1
        subprocess.run(["sgdisk", "-o", "-n", f"1:{start}:{last}", "-t", "1:EF00", "-c", "1:IYAKKI-BOOT",
                        "-n", f"2:{last + 1}:{last + 2048}", "-t", "2:8300", "-c", "2:IYAKKI-DATA", a.out], check=True, stdout=subprocess.DEVNULL)
        with open(a.out, "r+b") as img, open(fat, "rb") as f:
            img.seek(start * SECTOR)
            while True:
                chunk = f.read(1 << 20)
                if not chunk:
                    break
                img.write(chunk)


main()
