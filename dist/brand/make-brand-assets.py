#!/usr/bin/env python3
"""Draws IYAKKI's own pictures from nothing, with fixed numbers (the same files every run): the emblem (logo.png), the GRUB
background, the SRE PILOT whiteboard picture and two small test fixtures. No third-party picture, font file or model is used
except the DejaVu font already shipped with the shell. Dedicated by the project to the public under Apache-2.0 like the rest.

    python3 dist/brand/make-brand-assets.py            # run from the repository root; rewrites the files named below
Needs: python3, numpy, Pillow (a developer tool only; nothing here is shipped or run in an image).
"""
import math, os, random
import numpy as np
from PIL import Image, ImageDraw, ImageFont, ImageFilter

ROOT = os.path.abspath(os.path.join(os.path.dirname(__file__), "..", ".."))
P = lambda *a: os.path.join(ROOT, *a)
SS = 4  # supersampling


def emblem(size=384, circle=378):
    """The round emblem: an infinity loop (continuous operation), an I at the top, a compass star below, rings round it."""
    n = size * SS
    c = n / 2.0
    k = SS
    R = circle / 2.0 * k
    yy, xx = np.mgrid[0:n, 0:n].astype(np.float32)
    r = np.hypot(xx + 0.5 - c, yy + 0.5 - c)
    ang = np.arctan2(yy + 0.5 - c, xx + 0.5 - c)
    img = np.zeros((n, n, 4), np.float32)
    # disc: deep navy, lighter towards the middle
    t = np.clip(r / R, 0, 1)[..., None]
    disc = np.array([14, 48, 78], np.float32) * (1 - t) + np.array([5, 20, 40], np.float32) * t
    inside = r <= R
    img[..., :3] = np.where(inside[..., None], disc, 0)
    img[..., 3] = inside * 255
    # outer ring, cyan, with eight slits
    ring = (r >= 0.845 * R) & (r <= R)
    slit = np.zeros_like(ring)
    for i in range(8):
        a = math.radians(22.5 + 45 * i)
        d = np.abs(np.sin(ang - a)) * r
        slit |= (d < 3.2 * k) & (np.cos(ang - a) > 0)
    ring &= ~slit
    tr = np.clip((r - 0.845 * R) / (0.155 * R), 0, 1)[..., None]
    rc = np.array([88, 232, 246], np.float32) * (1 - tr) + np.array([30, 150, 200], np.float32) * tr
    img[..., :3] = np.where(ring[..., None], rc, img[..., :3])
    # thin gold ring
    gold = (r >= 0.775 * R) & (r <= 0.79 * R)
    img[..., :3] = np.where(gold[..., None], np.array([212, 169, 90], np.float32), img[..., :3])
    im = Image.fromarray(np.clip(img, 0, 255).astype(np.uint8), "RGBA")
    d = ImageDraw.Draw(im)
    # the infinity loop (lemniscate of Bernoulli), glowing
    a = 0.60 * R
    pts = []
    for i in range(0, 721):
        th = 2 * math.pi * i / 720
        s = 1 + math.sin(th) ** 2
        pts.append((c + a * math.cos(th) / s * 1.0, c + 0.02 * R + a * math.sin(th) * math.cos(th) / s * 1.6))
    glow = Image.new("RGBA", im.size, (0, 0, 0, 0))
    ImageDraw.Draw(glow).line(pts, fill=(60, 220, 240, 190), width=int(15 * k), joint="curve")
    glow = glow.filter(ImageFilter.GaussianBlur(7 * k))
    im = Image.alpha_composite(im, glow)
    d = ImageDraw.Draw(im)
    d.line(pts, fill=(96, 236, 250, 255), width=int(8 * k), joint="curve")
    d.line(pts, fill=(200, 250, 255, 255), width=int(2.5 * k), joint="curve")
    for i in range(0, 720, 90):
        x, y = pts[i]
        rr = 6.5 * k
        d.ellipse([x - rr, y - rr, x + rr, y + rr], fill=(230, 252, 255, 255))
    # the letter I, with serifs, at the top
    ix, top, bot = c, c - 0.74 * R, c - 0.36 * R
    col = (222, 247, 255, 255)
    d.rounded_rectangle([ix - 7 * k, top, ix + 7 * k, bot], radius=2 * k, fill=col)
    d.rounded_rectangle([ix - 22 * k, top, ix + 22 * k, top + 9 * k], radius=2 * k, fill=col)
    d.rounded_rectangle([ix - 22 * k, bot - 9 * k, ix + 22 * k, bot], radius=2 * k, fill=col)
    # the compass star at the bottom
    sx, sy = c, c + 0.58 * R
    star = []
    for i in range(16):
        rad = (0.17 * R if i % 4 == 0 else 0.10 * R if i % 2 == 0 else 0.045 * R)
        th = math.pi * 2 * i / 16 - math.pi / 2
        star.append((sx + rad * math.cos(th), sy + rad * math.sin(th)))
    d.polygon(star, fill=(232, 252, 255, 255))
    d.ellipse([sx - 4 * k, sy - 4 * k, sx + 4 * k, sy + 4 * k], fill=(30, 130, 180, 255))
    return im.resize((size, size), Image.LANCZOS)


def logo():
    emblem().save(P("crates/iyakki-shell/assets/logo.png"), optimize=True)


def grub_bg():
    """1280x720: a dark gradient with a soft glow; the emblem sits at (640, 259), 300 px across (splash.rs GRUB_LOGO_IN_BG)."""
    w, h = 1280, 720
    y = np.linspace(0, 1, h, dtype=np.float32)[:, None]
    x = np.arange(w, dtype=np.float32)[None, :]
    base = np.zeros((h, w, 3), np.float32)
    for ch, (a, b) in enumerate([(7, 3), (13, 5), (26, 12)]):
        base[..., ch] = a + (b - a) * y + 0 * x
    rr = np.hypot((x - 640) / 520.0, (np.arange(h, dtype=np.float32)[:, None] - 259) / 330.0)
    glow = np.clip(1 - rr, 0, 1) ** 2
    for ch, v in enumerate([6, 34, 52]):  # keeps every glow channel under 70 outside the emblem
        base[..., ch] += glow * v
    bg = Image.fromarray(np.clip(base, 0, 255).astype(np.uint8), "RGB").convert("RGBA")
    e = emblem().resize((300, 300), Image.LANCZOS)
    bg.alpha_composite(e, (640 - 150, 259 - 150))
    bg.convert("RGB").save(P("dist/buildroot/board/iyakki/grub/iyakki-bg.png"), optimize=True)


# ---- the SRE PILOT whiteboard, 1280 x 960; shapes follow PHOTO_BOXES / PHOTO_PATHS (2000 x 1500) of crates/iyakki-shell/src/apps/srepilot.rs
BOXES = [(1310, 170, 1660, 290), (1330, 325, 1610, 415), (1320, 585, 1665, 680), (900, 350, 1090, 480), (565, 470, 805, 730), (650, 860, 780, 925),
         (720, 935, 990, 1000), (850, 1015, 1140, 1110), (265, 810, 445, 950), (1225, 725, 1475, 830), (1250, 880, 1525, 1285)]
LABELS = ["SOURCE 1", "SOURCE 2", "SOURCE 3", "Aggregator", "Optimized store", "map", "pilot assist", "auto pilot", "Front end", "Site infra", "Chat / Teams / phone"]
PATHS = [[(1310, 255), (1100, 400)], [(1335, 395), (1100, 405)], [(1340, 605), (1100, 420)], [(900, 425), (725, 550)], [(655, 760), (670, 800), (675, 880)],
         [(735, 760), (750, 850), (755, 940)], [(780, 760), (835, 880), (875, 1025)], [(560, 740), (455, 845)], [(662, 912), (640, 917), (455, 845)],
         [(727, 975), (690, 970), (645, 925), (455, 845)], [(782, 962), (835, 880), (1230, 775)], [(875, 1025), (835, 880), (1230, 775)],
         [(782, 962), (835, 880), (1252, 915)], [(875, 1025), (835, 880), (1252, 915)], [(1380, 722), (1385, 688)]]


def whiteboard():
    W, H, k = 1280, 960, 0.64
    rnd = random.Random(11)
    yy, xx = np.mgrid[0:H, 0:W].astype(np.float32)
    v = 1 - 0.10 * (np.hypot(xx - W / 2, yy - H / 2) / 800) ** 2
    img = np.stack([236 * v, 239 * v, 241 * v], -1)
    im = Image.fromarray(np.clip(img, 0, 255).astype(np.uint8), "RGB")
    d = ImageDraw.Draw(im)
    ink = (28, 58, 168)
    font = ImageFont.truetype(P("crates/iyakki-shell/assets/DejaVuSans.ttf"), 22)

    def wob(p):
        return (p[0] * k + rnd.uniform(-2.2, 2.2), p[1] * k + rnd.uniform(-2.2, 2.2))

    def stroke(pts, w=4):
        # a hand-ish line: subdivide and jitter
        out = []
        for a, b in zip(pts, pts[1:]):
            for i in range(12):
                t = i / 12
                out.append((a[0] + (b[0] - a[0]) * t + rnd.uniform(-1.2, 1.2), a[1] + (b[1] - a[1]) * t + rnd.uniform(-1.2, 1.2)))
        out.append(pts[-1])
        d.line(out, fill=ink, width=w, joint="curve")

    for (x0, y0, x1, y1), label in zip(BOXES, LABELS):
        c = [wob((x0, y0)), wob((x1, y0)), wob((x1, y1)), wob((x0, y1))]
        stroke(c + [c[0]])
        tw = d.textlength(label, font=font)
        cx, cy = (c[0][0] + c[2][0]) / 2, (c[0][1] + c[2][1]) / 2
        d.text((cx - tw / 2, cy - 13), label, fill=ink, font=font)
    for path in PATHS:
        pts = [(p[0] * k, p[1] * k) for p in path]
        stroke(pts, 3)
        (x0, y0), (x1, y1) = pts[-2], pts[-1]
        a = math.atan2(y1 - y0, x1 - x0)
        for da in (2.6, -2.6):
            d.line([(x1, y1), (x1 + 16 * math.cos(a + da), y1 + 16 * math.sin(a + da))], fill=ink, width=3)
    d.text((30, 520), "Front end", fill=ink, font=font) if False else None
    d.ellipse([28, 52, 62, 86], fill=(200, 202, 206), outline=(170, 172, 176))
    im.save(P("crates/iyakki-shell/assets/sre-whiteboard.jpg"), quality=86, optimize=True)


def test_gradient(w, h):
    x = np.linspace(0, 1, w, dtype=np.float32)[None, :]
    y = np.linspace(0, 1, h, dtype=np.float32)[:, None]
    a = np.stack([255 * x + 0 * y, 255 * (1 - x) * (0.4 + 0.6 * y), 255 * y * (0.5 + 0.5 * x)], -1)
    return Image.fromarray(np.clip(a, 0, 255).astype(np.uint8), "RGB")


def fixtures():
    # a 48 x 32 JPEG written by libjpeg through Pillow (quality 90, 4:2:0, standard tables): camera/jpeg.rs compares our tables with it
    test_gradient(48, 32).save(P("crates/iyakki-shell/src/camera/testdata/pil-q90.jpg"), quality=90, subsampling=2)
    with open(P("crates/iyakki-shell/src/apps/testdata/hello.svg"), "w") as f:
        f.write('<svg xmlns="http://www.w3.org/2000/svg" width="200" height="100" viewBox="0 0 200 100">\n'
                '  <rect width="200" height="100" fill="blue"/>\n'
                '  <text x="100" y="58" font-size="22" text-anchor="middle" fill="white" font-family="Arial">Hello SVG</text>\n</svg>\n')


if __name__ == "__main__":
    logo(); grub_bg(); whiteboard(); fixtures()
    print("written")
