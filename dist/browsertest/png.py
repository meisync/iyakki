# SPDX-License-Identifier: Apache-2.0
# Copyright IYAKKI contributors. Licensed under the Apache License, Version 2.0 (see LICENSE at the root of the repository).
"""PNG reading (8-bit grey, RGB, RGBA, no interlace; every filter) and writing (RGBA) with the standard library, and a picture compare."""
import struct
import zlib


def _chunks(data):
    if data[:8] != b"\x89PNG\r\n\x1a\n":
        raise ValueError("not a PNG")
    p = 8
    while p + 8 <= len(data):
        n, t = struct.unpack("!I4s", data[p:p + 8])
        yield t, data[p + 8:p + 8 + n]
        p += 12 + n


def decode(data):
    """Returns (width, height, rgba bytes)."""
    w = h = ct = 0
    idat = []
    for t, d in _chunks(data):
        if t == b"IHDR":
            w, h, depth, ct, _, _, il = struct.unpack("!IIBBBBB", d)
            if depth != 8 or il != 0 or ct not in (0, 2, 6):
                raise ValueError("unsupported PNG (depth %d, colour %d, interlace %d)" % (depth, ct, il))
        elif t == b"IDAT":
            idat.append(d)
    bpp = {0: 1, 2: 3, 6: 4}[ct]
    raw = zlib.decompress(b"".join(idat))
    stride = w * bpp
    out = bytearray(stride * h)
    prev = bytearray(stride)
    pos = 0
    for y in range(h):
        f = raw[pos]
        line = bytearray(raw[pos + 1:pos + 1 + stride])
        pos += 1 + stride
        if f == 1:
            for x in range(bpp, stride):
                line[x] = (line[x] + line[x - bpp]) & 255
        elif f == 2:
            for x in range(stride):
                line[x] = (line[x] + prev[x]) & 255
        elif f == 3:
            for x in range(stride):
                a = line[x - bpp] if x >= bpp else 0
                line[x] = (line[x] + ((a + prev[x]) >> 1)) & 255
        elif f == 4:
            for x in range(stride):
                a = line[x - bpp] if x >= bpp else 0
                b = prev[x]
                c = prev[x - bpp] if x >= bpp else 0
                pa, pb, pc = abs(b - c), abs(a - c), abs(a + b - 2 * c)
                pr = a if (pa <= pb and pa <= pc) else (b if pb <= pc else c)
                line[x] = (line[x] + pr) & 255
        elif f != 0:
            raise ValueError("bad filter")
        out[y * stride:(y + 1) * stride] = line
        prev = line
    if bpp == 4:
        return w, h, bytes(out)
    rgba = bytearray(w * h * 4)
    for i in range(w * h):
        if bpp == 3:
            rgba[i * 4:i * 4 + 3] = out[i * 3:i * 3 + 3]
        else:
            rgba[i * 4:i * 4 + 3] = bytes([out[i]]) * 3
        rgba[i * 4 + 3] = 255
    return w, h, bytes(rgba)


def encode(w, h, rgba):
    def chunk(t, d):
        return struct.pack("!I", len(d)) + t + d + struct.pack("!I", zlib.crc32(t + d) & 0xFFFFFFFF)
    rows = b"".join(b"\x00" + rgba[y * w * 4:(y + 1) * w * 4] for y in range(h))
    return b"\x89PNG\r\n\x1a\n" + chunk(b"IHDR", struct.pack("!IIBBBBB", w, h, 8, 6, 0, 0, 0)) + chunk(b"IDAT", zlib.compress(rows, 6)) + chunk(b"IEND", b"")


def compare(a, b, w, h, aw=None, bw=None):
    """Compare two RGBA buffers over the top-left w x h. Returns (max channel difference, number of pixels that differ at all)."""
    aw = aw or w
    bw = bw or w
    worst = n = 0
    for y in range(h):
        ra = a[y * aw * 4:(y * aw + w) * 4]
        rb = b[y * bw * 4:(y * bw + w) * 4]
        if ra == rb:
            continue
        for x in range(w):
            pa, pb = ra[x * 4:x * 4 + 4], rb[x * 4:x * 4 + 4]
            if pa != pb:
                n += 1
                worst = max(worst, max(abs(pa[k] - pb[k]) for k in range(4)))
    return worst, n
