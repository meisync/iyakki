#!/usr/bin/env python3
# SPDX-License-Identifier: Apache-2.0
"""Make the splash picture of the extra boot loader (iyakki-boot) from the IYAKKI emblem.

  boot-splash.py LOGO.png OUT.bmp [--width 800] [--height 450] [--logo-height 324]

Writes an uncompressed 24-bit BMP, black background, the emblem centred (324 px tall = 30% of a 1080p screen; the loader never
enlarges a picture, so on a bigger screen it stays small and centred). Python standard library only (own PNG reader: 8-bit
greyscale/RGB/palette/+alpha, not interlaced). The emblem is the IYAKKI artwork (crates/iyakki-shell/assets/logo.png), the owner's
logo; this script is licensed like the rest of the repository and says nothing about the artwork's rights (see LICENSING.md).
"""
import struct, sys, zlib


def read_png(path):
    d = open(path, "rb").read()
    if d[:8] != b"\x89PNG\r\n\x1a\n":
        raise SystemExit("boot-splash: not a PNG: " + path)
    pos, idat, plte = 8, b"", b""
    while pos < len(d):
        n, t = struct.unpack(">I4s", d[pos:pos + 8])
        body = d[pos + 8:pos + 8 + n]
        pos += 12 + n
        if t == b"IHDR":
            w, h, depth, ctype, _, _, inter = struct.unpack(">IIBBBBB", body)
        elif t == b"PLTE":
            plte = body
        elif t == b"IDAT":
            idat += body
    if depth != 8 or inter != 0 or ctype not in (0, 2, 3, 4, 6):
        raise SystemExit("boot-splash: only 8-bit, non-interlaced PNG files are read")
    ch = {0: 1, 2: 3, 3: 1, 4: 2, 6: 4}[ctype]
    raw = zlib.decompress(idat)
    stride = w * ch
    rows, prev = [], bytearray(stride)
    for y in range(h):
        f = raw[y * (stride + 1)]
        cur = bytearray(raw[y * (stride + 1) + 1:(y + 1) * (stride + 1)])
        for i in range(stride):
            a = cur[i - ch] if i >= ch else 0
            b = prev[i]
            c = prev[i - ch] if i >= ch else 0
            if f == 1: cur[i] = (cur[i] + a) & 255
            elif f == 2: cur[i] = (cur[i] + b) & 255
            elif f == 3: cur[i] = (cur[i] + (a + b) // 2) & 255
            elif f == 4:
                p = a + b - c
                pa, pb, pc = abs(p - a), abs(p - b), abs(p - c)
                cur[i] = (cur[i] + (a if pa <= pb and pa <= pc else b if pb <= pc else c)) & 255
            elif f != 0:
                raise SystemExit("boot-splash: bad PNG filter")
        rows.append(cur)
        prev = cur
    out = []  # rows of (r, g, b, a) tuples
    for cur in rows:
        row = []
        for x in range(w):
            p = cur[x * ch:(x + 1) * ch]
            if ctype == 0: row.append((p[0], p[0], p[0], 255))
            elif ctype == 2: row.append((p[0], p[1], p[2], 255))
            elif ctype == 3: row.append((plte[p[0] * 3], plte[p[0] * 3 + 1], plte[p[0] * 3 + 2], 255))
            elif ctype == 4: row.append((p[0], p[0], p[0], p[1]))
            else: row.append((p[0], p[1], p[2], p[3]))
        out.append(row)
    return w, h, out


def scale_over_black(w, h, px, nw, nh):
    """Box-filter the picture to nw x nh, premultiplying by alpha so transparent edges blend into black."""
    res = []
    for y in range(nh):
        y0, y1 = y * h / nh, (y + 1) * h / nh
        row = []
        for x in range(nw):
            x0, x1 = x * w / nw, (x + 1) * w / nw
            acc, wt = [0.0, 0.0, 0.0], 0.0
            for sy in range(int(y0), min(h, int(y1) + 1)):
                fy = min(y1, sy + 1) - max(y0, sy)
                for sx in range(int(x0), min(w, int(x1) + 1)):
                    f = fy * (min(x1, sx + 1) - max(x0, sx))
                    if f <= 0: continue
                    r, g, b, a = px[sy][sx]
                    k = f * a / 255.0
                    acc[0] += r * k; acc[1] += g * k; acc[2] += b * k
                    wt += f
            row.append(tuple(int(round(v / wt)) if wt else 0 for v in acc))
        res.append(row)
    return res


def write_bmp(path, w, h, rows):
    stride = (w * 3 + 3) & ~3
    body = b"".join(bytes(c for (r, g, b) in row for c in (b, g, r)) + b"\0" * (stride - w * 3) for row in reversed(rows))
    hdr = b"BM" + struct.pack("<IHHI", 54 + len(body), 0, 0, 54) + struct.pack("<IiiHHIIiiII", 40, w, h, 1, 24, 0, len(body), 2835, 2835, 0, 0)
    open(path, "wb").write(hdr + body)


def main(argv):
    a = [x for x in argv[1:]]
    opt = {"--width": 800, "--height": 450, "--logo-height": 324}
    pos = []
    while a:
        x = a.pop(0)
        if x in opt and a: opt[x] = int(a.pop(0))
        else: pos.append(x)
    if len(pos) != 2:
        raise SystemExit(__doc__)
    w, h, px = read_png(pos[0])
    lh = opt["--logo-height"]
    lw = max(1, round(w * lh / h))
    logo = scale_over_black(w, h, px, lw, lh)
    cw, ch = opt["--width"], opt["--height"]
    if lw > cw or lh > ch:
        raise SystemExit("boot-splash: the emblem does not fit the canvas")
    ox, oy = (cw - lw) // 2, (ch - lh) // 2
    canvas = [[(0, 0, 0)] * cw for _ in range(ch)]
    for y in range(lh):
        canvas[oy + y][ox:ox + lw] = logo[y]
    write_bmp(pos[1], cw, ch, canvas)


if __name__ == "__main__":
    main(sys.argv)
