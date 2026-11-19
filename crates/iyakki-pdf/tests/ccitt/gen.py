#!/usr/bin/env python3
"""Regenerates the CCITT fixtures with Pillow's real Group 3 / Group 4 encoder (libtiff): for each picture a raw CCITT stream
(<name>.g4 / <name>.g3) and the picture's samples (<name>.pbm: width, height, then rows of 1 bit per pixel, 1 = black).
Run: python3 gen.py   (needs Pillow; only for regenerating, the committed files are what the test reads)."""
import io, random, struct, sys, os
from PIL import Image, ImageDraw, ImageOps

random.seed(7)
here = os.path.dirname(os.path.abspath(__file__))

def pictures():
    # 1: text-like page
    im = Image.new("1", (300, 90), 1)
    d = ImageDraw.Draw(im)
    d.text((6, 6), "Hello CCITT fax 12345", fill=0)
    d.text((6, 30), "The quick brown fox jumps", fill=0)
    d.rectangle((10, 60, 120, 80), outline=0)
    yield "text", im
    # 2: every run length from 0 to 70 and the long ones (make-up codes up to 2560 and the extended ones)
    w = 2700
    im = Image.new("1", (w, 40), 1)
    px = im.load()
    for y in range(36):
        x = 0; col = 1 if y % 2 == 0 else 0
        runs = [y * 3 % 70 + 1, 63, 64, 65, 127, 128, 129, 1728, 1792, 1856, 2560] if y % 4 < 2 else [random.randint(1, 400) for _ in range(40)]
        for r in runs:
            for i in range(r):
                if x < w:
                    px[x, y] = col; x += 1
            col ^= 1
        while x < w:
            px[x, y] = col; x += 1
    yield "runs", im
    # 3: noise with structure, odd width not a multiple of 8
    im = Image.new("1", (203, 61), 1)
    px = im.load()
    for y in range(61):
        for x in range(203):
            px[x, y] = 0 if ((x // 7 + y // 5) % 3 == 0) ^ (random.random() < 0.03) else 1
    yield "noise", im
    # 4: blank and all black rows
    im = Image.new("1", (64, 8), 1)
    d = ImageDraw.Draw(im); d.rectangle((0, 4, 63, 7), fill=0)
    yield "blank", im

def strip(im, comp):
    b = io.BytesIO()
    # the encoder codes 1 bits as black runs; our pictures have black = 0, so the bits are turned over first
    ImageOps.invert(im.convert("L")).convert("1", dither=Image.Dither.NONE).save(b, format="TIFF", compression=comp)
    t = Image.open(io.BytesIO(b.getvalue()))
    off = t.tag_v2[273]; cnt = t.tag_v2[279]
    off = off[0] if isinstance(off, tuple) else off; cnt = cnt[0] if isinstance(cnt, tuple) else cnt
    return b.getvalue()[off:off + cnt]

for name, im in pictures():
    w, h = im.size
    rows = bytearray()
    for y in range(h):
        bits = [1 if im.getpixel((x, y)) == 0 else 0 for x in range(w)]
        for i in range(0, w, 8):
            v = 0
            for j in range(8):
                v = (v << 1) | (bits[i + j] if i + j < w else 0)
            rows.append(v)
    open(os.path.join(here, name + ".pbm"), "wb").write(struct.pack(">II", w, h) + bytes(rows))
    open(os.path.join(here, name + ".g4"), "wb").write(strip(im, "group4"))
    if name != "runs" or True:
        open(os.path.join(here, name + ".g3"), "wb").write(strip(im, "group3"))
print("ok")
