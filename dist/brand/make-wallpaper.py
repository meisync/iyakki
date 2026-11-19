#!/usr/bin/env python3
"""Draws the wallpaper shipped with IYAKKI (F-LAUNCH-22): a dusk sky over four layers of ridges, 1280 x 720, written as a PNG with the
standard library only. It is our own drawing from numbers (Apache-2.0 like the rest of the repository), not a photograph, so no picture
licence is involved; the file it writes, dist/brand/wallpaper-default.png, is what the shell draws when `wallpaper = picture` is chosen and
the person has put no picture of their own in their themes folder. Run: python3 dist/brand/make-wallpaper.py [out.png]"""
import math, struct, sys, zlib

W, H = 1280, 720

def mix(a, b, t):
    return tuple(a[i] + (b[i] - a[i]) * t for i in range(3))

SKY = [(0.0, (20, 24, 52)), (0.45, (62, 52, 110)), (0.7, (176, 98, 120)), (1.0, (244, 178, 120))]

def sky(y):
    t = y / (H * 0.78)
    t = min(max(t, 0.0), 1.0)
    for (t0, c0), (t1, c1) in zip(SKY, SKY[1:]):
        if t <= t1:
            return mix(c0, c1, (t - t0) / (t1 - t0))
    return SKY[-1][1]

def ridge(x, base, amp, seed):
    return base + amp * (0.55 * math.sin(x * 0.0061 + seed) + 0.3 * math.sin(x * 0.0153 + seed * 2.3) + 0.15 * math.sin(x * 0.0377 + seed * 5.1))

LAYERS = [(500, 70, 1.0, (92, 70, 118)), (565, 60, 2.7, (62, 50, 96)), (620, 52, 4.1, (38, 34, 72)), (676, 40, 6.3, (18, 20, 44))]

def pixel(x, y):
    c = sky(y)
    # the low sun: a soft glow near the horizon on the left third
    d = math.hypot((x - 400) / 420.0, (y - 470) / 150.0)
    glow = max(0.0, 1.0 - d) ** 2
    c = mix(c, (255, 214, 150), glow * 0.8)
    for base, amp, seed, col in LAYERS:
        if y >= ridge(x, base, amp, seed):
            haze = max(0.0, (y - base) / 220.0)
            c = mix(col, (28, 26, 60), min(0.35, haze * 0.25))
    return c

def main(out):
    rows = []
    for y in range(H):
        row = bytearray([1])  # PNG filter 1 (Sub) keeps smooth gradients small
        prev = (0, 0, 0)
        for x in range(W):
            r, g, b = (int(round(v)) for v in pixel(x, y))
            cur = (r, g, b)
            row += bytes((cur[i] - prev[i]) & 255 for i in range(3))
            prev = cur
        rows.append(bytes(row))
    raw = b"".join(rows)
    def chunk(tag, data):
        c = struct.pack(">I", len(data)) + tag + data
        return c + struct.pack(">I", zlib.crc32(tag + data) & 0xFFFFFFFF)
    png = b"\x89PNG\r\n\x1a\n" + chunk(b"IHDR", struct.pack(">IIBBBBB", W, H, 8, 2, 0, 0, 0)) + chunk(b"IDAT", zlib.compress(raw, 9)) + chunk(b"IEND", b"")
    open(out, "wb").write(png)
    print(f"{out}: {len(png)} bytes")

if __name__ == "__main__":
    main(sys.argv[1] if len(sys.argv) > 1 else "dist/brand/wallpaper-default.png")
