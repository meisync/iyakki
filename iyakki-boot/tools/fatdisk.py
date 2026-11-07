#!/usr/bin/env python3
# SPDX-License-Identifier: MIT OR Apache-2.0
"""Make small GPT + FAT32 test disks and read files out of existing FAT32 images, with only the Python standard library and the
system's sgdisk and mkfs.vfat (no mount, no root, no mtools).

  fatdisk.py make OUT.img --size-mb 96 [--label TEST] [--put SRC:/DEST/PATH ...] [--text TEXT:/DEST/PATH ...]
  fatdisk.py ls IMAGE PARTITION /DIR              (PARTITION is the GPT number, 1 based; read-only)
  fatdisk.py get IMAGE PARTITION /SRC DEST_FILE   (read-only)

`make` writes only the file OUT.img. `ls` and `get` open their image read-only.
"""
import argparse
import io
import os
import struct
import subprocess
import sys
import tempfile

SECTOR = 512


def gpt_partitions(f):
    f.seek(SECTOR)
    hdr = f.read(92)
    if hdr[:8] != b"EFI PART":
        f.seek(446)  # no GPT: the four MBR entries
        mbr = f.read(64)
        out = []
        for i in range(4):
            e = mbr[i * 16:(i + 1) * 16]
            start, count = struct.unpack_from("<II", e, 8)
            if e[4] and count:
                out.append((i + 1, start, start + count - 1))
        if not out:
            raise SystemExit("no partition table")
        return out
    entries_lba, count, size = struct.unpack_from("<QII", hdr, 72)
    f.seek(entries_lba * SECTOR)
    raw = f.read(count * size)
    out = []
    for i in range(count):
        e = raw[i * size:(i + 1) * size]
        if e[:16] == b"\0" * 16:
            continue
        first, last = struct.unpack_from("<QQ", e, 32)
        out.append((i + 1, first, last))
    return out


class Fat32:
    """A FAT32 volume inside a file-like object, starting at `base` bytes."""

    def __init__(self, f, base=0):
        self.f, self.base = f, base
        b = self._read(0, 512)
        (self.bps, self.spc, self.reserved, self.nfats) = struct.unpack_from("<HBHB", b, 11)
        self.fat_sectors = struct.unpack_from("<I", b, 36)[0]
        self.root = struct.unpack_from("<I", b, 44)[0]
        self.total = struct.unpack_from("<I", b, 32)[0] or struct.unpack_from("<H", b, 19)[0]
        if struct.unpack_from("<H", b, 22)[0] != 0:
            raise SystemExit("not FAT32")
        self.fat_off = self.reserved * self.bps
        self.data_off = (self.reserved + self.nfats * self.fat_sectors) * self.bps
        self.cbytes = self.bps * self.spc
        self.nclusters = (self.total * self.bps - self.data_off) // self.cbytes
        self._fat = None

    def _read(self, off, n):
        self.f.seek(self.base + off)
        return self.f.read(n)

    def _write(self, off, data):
        self.f.seek(self.base + off)
        self.f.write(data)

    def fat_get(self, c):
        return struct.unpack("<I", self._read(self.fat_off + 4 * c, 4))[0] & 0x0FFFFFFF

    def fat_set(self, c, v):
        for k in range(self.nfats):
            self._write(self.fat_off + k * self.fat_sectors * self.bps + 4 * c, struct.pack("<I", v))

    def chain(self, c):
        while 2 <= c < 0x0FFFFFF8:
            yield c
            c = self.fat_get(c)

    def cluster_off(self, c):
        return self.data_off + (c - 2) * self.cbytes

    def read_chain(self, c, size=None):
        out = bytearray()
        for cl in self.chain(c):
            out += self._read(self.cluster_off(cl), self.cbytes)
        return bytes(out if size is None else out[:size])

    # ---- directories --------------------------------------------------------------------------------------------------
    def entries(self, c):
        """Yield (name, is_dir, first_cluster, size, entry_offset) for a directory chain, with long names joined."""
        lfn = []
        for cl in self.chain(c):
            base = self.cluster_off(cl)
            data = self._read(base, self.cbytes)
            for i in range(0, self.cbytes, 32):
                e = data[i:i + 32]
                if e[0] == 0:
                    return
                if e[0] == 0xE5:
                    lfn = []
                    continue
                if e[11] == 0x0F:
                    part = e[1:11] + e[14:26] + e[28:32]
                    lfn.insert(0, part.decode("utf-16-le").split("\0")[0])
                    continue
                if e[11] & 0x08:
                    lfn = []
                    continue
                name = "".join(lfn) if lfn else self._short(e)
                lfn = []
                hi, lo = struct.unpack_from("<H", e, 20)[0], struct.unpack_from("<H", e, 26)[0]
                size = struct.unpack_from("<I", e, 28)[0]
                yield name, bool(e[11] & 0x10), (hi << 16) | lo, size, base + i

    @staticmethod
    def _short(e):
        n, x = e[:8].decode("latin-1").rstrip(), e[8:11].decode("latin-1").rstrip()
        return n + ("." + x if x else "")

    def lookup(self, path):
        c, d, size = self.root, True, 0
        for part in [p for p in path.replace("\\", "/").split("/") if p]:
            if not d:
                return None
            for name, isdir, cl, sz, _ in self.entries(c):
                if name.lower() == part.lower():
                    c, d, size = cl, isdir, sz
                    break
            else:
                return None
        return c, d, size

    # ---- writing (used on freshly formatted images only) -----------------------------------------------------------------
    def alloc(self, count):
        got = []
        c = 3
        while len(got) < count:
            if c >= self.nclusters + 2:
                raise SystemExit("image full")
            if self.fat_get(c) == 0:
                got.append(c)
            c += 1
        for a, b in zip(got, got[1:]):
            self.fat_set(a, b)
        self.fat_set(got[-1], 0x0FFFFFFF)
        for cl in got:
            self._write(self.cluster_off(cl), b"\0" * self.cbytes)
        return got

    def write_data(self, data):
        if not data:
            return 0
        cl = self.alloc(-(-len(data) // self.cbytes))
        for i, c in enumerate(cl):
            self._write(self.cluster_off(c), data[i * self.cbytes:(i + 1) * self.cbytes].ljust(self.cbytes, b"\0"))
        return cl[0]

    def _short_name(self, name, taken):
        up = name.upper()
        base, _, ext = up.rpartition(".") if "." in up else (up, "", "")
        ok = lambda s: s and all(ch.isalnum() or ch in "!#$%&'()-@^_`{}~" for ch in s)
        if ok(base) and len(base) <= 8 and (not ext or ok(ext)) and len(ext) <= 3 and name == up:
            return (base.ljust(8) + ext.ljust(3)).encode("latin-1"), False
        b = "".join(ch for ch in base if ok(ch))[:6] or "FILE"
        e = "".join(ch for ch in ext if ok(ch))[:3]
        n = 1
        while True:
            cand = (b + "~" + str(n)).ljust(8) + e.ljust(3)
            if cand not in taken:
                return cand.encode("latin-1"), True
            n += 1

    def add_entry(self, dir_cluster, name, is_dir, first, size):
        existing = {}
        for nm, _, _, _, off in self.entries(dir_cluster):
            existing[nm.lower()] = off
        short, need_lfn = self._short_name(name, {self._read(o, 11).decode("latin-1") for o in existing.values()})
        records = b""
        if need_lfn:
            csum = 0
            for ch in short:
                csum = (((csum & 1) << 7) + (csum >> 1) + ch) & 0xFF
            units = name.encode("utf-16-le") + (b"\0\0" if len(name) % 13 else b"")
            units = units.ljust(-(-len(units) // 26) * 26, b"\xff")
            n = len(units) // 26
            for k in range(n, 0, -1):
                chunk = units[(k - 1) * 26:k * 26]
                seq = k | (0x40 if k == n else 0)
                records += bytes([seq]) + chunk[:10] + bytes([0x0F, 0, csum]) + chunk[10:22] + b"\0\0" + chunk[22:26]
        attr = 0x10 if is_dir else 0x20
        records += short + bytes([attr]) + b"\0" * 8 + struct.pack("<H", first >> 16) + b"\0" * 4 + struct.pack("<HI", first & 0xFFFF, size)
        # find a free run in the directory chain
        need = len(records) // 32
        slots = []
        for cl in self.chain(dir_cluster):
            data = self._read(self.cluster_off(cl), self.cbytes)
            for i in range(0, self.cbytes, 32):
                slots.append(self.cluster_off(cl) + i if data[i] in (0, 0xE5) else None)
        run = None
        for i in range(len(slots) - need + 1):
            if all(s is not None for s in slots[i:i + need]):
                run = slots[i:i + need]
                break
        if run is None:
            last = list(self.chain(dir_cluster))[-1]
            new = self.alloc(1)[0]
            self.fat_set(last, new)
            self.fat_set(new, 0x0FFFFFFF)
            run = [self.cluster_off(new) + 32 * i for i in range(need)]
        for i, off in enumerate(run):
            self._write(off, records[i * 32:(i + 1) * 32])

    def mkdir(self, parent, name):
        c = self.alloc(1)[0]
        self._write(self.cluster_off(c), self._dot(".", c) + self._dot("..", 0 if parent == self.root else parent))
        self.add_entry(parent, name, True, c, 0)
        return c

    @staticmethod
    def _dot(name, c):
        return name.ljust(11).encode() + bytes([0x10]) + b"\0" * 8 + struct.pack("<H", c >> 16) + b"\0" * 4 + struct.pack("<HI", c & 0xFFFF, 0)

    def put(self, path, data):
        parts = [p for p in path.replace("\\", "/").split("/") if p]
        c = self.root
        for part in parts[:-1]:
            hit = next((e for e in self.entries(c) if e[0].lower() == part.lower()), None)
            c = hit[2] if hit else self.mkdir(c, part)
        first = self.write_data(data)
        self.add_entry(c, parts[-1], False, first, len(data))


def need(tool):
    from shutil import which
    if not which(tool) and not os.path.exists("/usr/sbin/" + tool):
        raise SystemExit(f"{tool} is not installed")


def cmd_make(a):
    need("sgdisk"), need("mkfs.vfat")
    size = a.size_mb * 1024 * 1024
    start = 2048  # sectors (1 MiB)
    with tempfile.TemporaryDirectory(dir=os.path.dirname(os.path.abspath(a.out))) as tmp:
        fat = os.path.join(tmp, "esp.fat")
        fat_bytes = size - (start + 2048) * SECTOR
        with open(fat, "wb") as f:
            f.truncate(fat_bytes)
        subprocess.run(["mkfs.vfat", "-F", "32", "-s", "1", "-S", "512", "-n", a.label[:11], fat], check=True, stdout=subprocess.DEVNULL)
        with open(fat, "r+b") as f:
            v = Fat32(f)
            for spec in a.put or []:
                src, dest = spec.rsplit(":", 1)
                with open(src, "rb") as s:
                    v.put(dest, s.read())
            for spec in a.text or []:
                text, dest = spec.rsplit(":", 1)
                v.put(dest, text.encode().decode("unicode_escape").encode())
        with open(a.out, "wb") as img:
            img.truncate(size)
        last = start + fat_bytes // SECTOR - 1
        subprocess.run(["sgdisk", "-o", "-n", f"1:{start}:{last}", "-t", "1:EF00", "-c", "1:EFI system partition", a.out], check=True, stdout=subprocess.DEVNULL)
        with open(a.out, "r+b") as img, open(fat, "rb") as f:
            img.seek(start * SECTOR)
            while True:
                chunk = f.read(1 << 20)
                if not chunk:
                    break
                img.write(chunk)


def open_part(image, number):
    f = open(image, "rb")  # read-only
    for n, first, last in gpt_partitions(f):
        if n == number:
            return f, Fat32(f, first * SECTOR)
    raise SystemExit("no such partition")


def cmd_ls(a):
    f, v = open_part(a.image, a.part)
    hit = v.lookup(a.path)
    if not hit or not hit[1]:
        raise SystemExit("not a directory")
    for name, d, _, sz, _ in v.entries(hit[0]):
        print(("d " if d else "- ") + f"{sz:10d} {name}")


def cmd_get(a):
    f, v = open_part(a.image, a.part)
    hit = v.lookup(a.path)
    if not hit or hit[1]:
        raise SystemExit("no such file")
    with open(a.dest, "wb") as out:
        out.write(v.read_chain(hit[0], hit[2]))


def main():
    p = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    sub = p.add_subparsers(dest="cmd", required=True)
    m = sub.add_parser("make")
    m.add_argument("out")
    m.add_argument("--size-mb", type=int, default=96)
    m.add_argument("--label", default="TESTESP")
    m.add_argument("--put", action="append")
    m.add_argument("--text", action="append")
    m.set_defaults(fn=cmd_make)
    l = sub.add_parser("ls")
    l.add_argument("image"), l.add_argument("part", type=int), l.add_argument("path")
    l.set_defaults(fn=cmd_ls)
    g = sub.add_parser("get")
    g.add_argument("image"), g.add_argument("part", type=int), g.add_argument("path"), g.add_argument("dest")
    g.set_defaults(fn=cmd_get)
    a = p.parse_args()
    a.fn(a)


if __name__ == "__main__":
    main()
