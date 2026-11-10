#!/usr/bin/env python3
"""Draws the IYAKKI emblem from nothing: three brushed-steel arrow heads, the first two with a window of blue circuit light.

Every number below is ours (the shape, the colours, the random seed of the circuit); no picture is read. The same file comes out on every run.
Used by make-brand-assets.py; run it alone to see the emblem:

    python3 dist/brand/draw-logo.py [out.png]      # 1024 x 572, on a dark grey wall

Dedicated by the project to the public under Apache-2.0 like the rest.
"""
import math, random, sys
import numpy as np
from PIL import Image, ImageDraw, ImageFilter

W, H = 1024, 572
SS = 3  # supersampling for the shapes

# Each arrow head: back tip top, back tip bottom, notch (the point of its concave back) and apex (the point of its convex front), all at the same height H/2.
CHEVRONS = [
    # name, tip top, tip bottom, notch, apex, window (offset at the centre, shrink per pixel of height, width at the centre, narrowing per pixel, reach)
    ("C", (495, 22), (490, 548), (680, 285), (791, 288), None),
    ("B", (385, 35), (380, 535), (520, 285), (711, 285), (78, 0.25, 56, 0.12, 200)),
    ("A", (222, 10), (212, 552), (357, 285), (640, 285), (83, 0.276, 55, 0.12, 215)),
]


def poly_mask(points):
    im = Image.new("L", (W * SS, H * SS), 0)
    ImageDraw.Draw(im).polygon([(x * SS, y * SS) for x, y in points], fill=255)
    return np.asarray(im.resize((W, H), Image.LANCZOS)).astype(np.float32) / 255.0


def over(dst, rgb, a):
    """dst (h, w, 4 premultiplied-free float RGBA); paint rgb (h, w, 3) with alpha a (h, w) over it."""
    da = dst[..., 3:4]
    oa = a[..., None] + da * (1 - a[..., None])
    out_rgb = (rgb * a[..., None] + dst[..., :3] * da * (1 - a[..., None])) / np.maximum(oa, 1e-6)
    dst[..., :3] = out_rgb
    dst[..., 3:4] = oa


def brushed(seed, scale=1.0):
    """Fine streaks along the picture's horizontal: the brushed-steel grain (a different seed per face)."""
    rnd = np.random.RandomState(seed)
    rows = rnd.randn(H * 2).astype(np.float32)
    rows = np.convolve(rows, np.ones(3) / 3.0, mode="same")
    g = rows[:H, None] * np.ones((1, W), np.float32)
    cols = np.convolve(rnd.randn(W).astype(np.float32), np.ones(41) / 41.0, mode="same")
    return (g * 0.7 + cols[None, :] * 14.0) * scale


def facet_color(xx, yy, base, top_boost, seed, axis):
    """Steel: a light that falls from the upper left, a darker lower edge, the grain on top. axis (dx, dy) is the direction the facet lightens along."""
    t = ((xx - W / 2) * axis[0] + (yy - H / 2) * axis[1]) / (W * 0.5)
    lum = base + top_boost * t + 9.0 * brushed(seed)
    lum = np.clip(lum, 0, 255)
    cool = np.stack([lum * 0.97, lum * 0.99, lum * 1.03], axis=-1)  # steel leans a little blue
    return np.clip(cool, 0, 255)


def circuit_layer(mask, seed):
    """Blue light on a dark board: bent traces at 45 and 90 degrees, with a glow, kept inside `mask`."""
    rnd = random.Random(seed)
    ys, xs = np.nonzero(mask > 0.5)
    if len(xs) == 0:
        return np.zeros((H, W, 3), np.float32)
    x0, x1, y0, y1 = xs.min(), xs.max(), ys.min(), ys.max()
    sheet = Image.new("L", (W, H), 0)
    d = ImageDraw.Draw(sheet)
    lines = []
    for _ in range(int(len(xs) / 260)):
        x, y = rnd.uniform(x0, x1), rnd.uniform(y0, y1)
        dirn = rnd.choice([(1, 0), (0, 1), (-1, 0), (0, -1), (1, 1), (1, -1), (-1, 1), (-1, -1)])
        pts = [(x, y)]
        for _ in range(rnd.randint(2, 5)):
            run = rnd.uniform(8, 30)
            x, y = x + dirn[0] * run, y + dirn[1] * run
            pts.append((x, y))
            dirn = rnd.choice([(1, 0), (0, 1), (-1, 0), (0, -1), (1, 1), (1, -1), (-1, 1), (-1, -1)])
        lines.append(pts)
    for pts in lines:
        d.line(pts, fill=255, width=1)
        x, y = pts[-1]
        d.ellipse([x - 3, y - 3, x + 3, y + 3], outline=255, width=1)
    core = np.asarray(sheet).astype(np.float32) / 255.0
    glow = np.asarray(sheet.filter(ImageFilter.GaussianBlur(5))).astype(np.float32) / 255.0
    glow2 = np.asarray(sheet.filter(ImageFilter.GaussianBlur(14))).astype(np.float32) / 255.0
    board_t = (np.arange(H, dtype=np.float32)[:, None] / H) * np.ones((1, W), np.float32)
    rgb = np.zeros((H, W, 3), np.float32)
    mid = 1.0 - np.abs(board_t - 0.5) * 1.4
    rgb[..., 0] = 4 + 6 * mid + core * 150 + glow * 20
    rgb[..., 1] = 8 + 16 * mid + core * 200 + glow * 60 + glow2 * 20
    rgb[..., 2] = 24 + 46 * mid + core * 255 + glow * 150 + glow2 * 90
    return np.clip(rgb, 0, 255) * mask[..., None]


def draw_emblem(wall=None):
    """RGBA picture (W x H, float 0..255 inside, returned as a PIL image). With `wall` (an RGB tuple) the background is that colour."""
    yy, xx = np.mgrid[0:H, 0:W].astype(np.float32)
    canvas = np.zeros((H, W, 4), np.float32)
    for n, (name, tt, tb, nt, ap, win) in enumerate(CHEVRONS):
        cy = (nt[1] + ap[1]) / 2.0
        ridge_end = ((nt[0] + ap[0]) / 2.0, cy)
        outer = poly_mask([tt, ap, tb, nt])
        # four facets: back and front of the upper arm, back and front of the lower arm
        upper_back = poly_mask([tt, nt, ridge_end])
        upper_front = poly_mask([tt, ridge_end, ap])
        lower_back = poly_mask([tb, nt, ridge_end])
        lower_front = poly_mask([tb, ridge_end, ap])
        seed = 100 + n * 10
        # soft shadow of this arrow head on the one behind it
        if n > 0:
            shadow = Image.fromarray((outer * 255).astype(np.uint8)).filter(ImageFilter.GaussianBlur(9))
            sh = np.asarray(shadow).astype(np.float32) / 255.0 * 0.0
        for mask, base, boost, ax, sd in [
            (upper_back, 150, -40, (0.4, 1), seed + 1),
            (upper_front, 205, -45, (0.6, 1), seed + 2),
            (lower_back, 120, 35, (0.4, 1), seed + 3),
            (lower_front, 160, 30, (0.6, 1), seed + 4),
        ]:
            over(canvas, facet_color(xx, yy, base, boost, sd, ax), mask)
        # the sharp light along the edges of the arms (the bevel)
        for a, b in [(tt, ap), (tb, ap), (tt, nt), (tb, nt), (tt, ridge_end), (tb, ridge_end)]:
            edge = Image.new("L", (W * SS, H * SS), 0)
            ImageDraw.Draw(edge).line([(a[0] * SS, a[1] * SS), (b[0] * SS, b[1] * SS)], fill=255, width=int(1.6 * SS))
            e = np.asarray(edge.resize((W, H), Image.LANCZOS)).astype(np.float32) / 255.0 * outer
            over(canvas, np.full((H, W, 3), 235.0, np.float32), e * 0.45)
        # the window of blue light, set into the back of the arrow head
        if win:
            off0, shrink, w0, narrow, reach = win
            pts_l, pts_r = [], []
            for dy in range(-int(reach), int(reach) + 1, 6):
                a = abs(dy)
                back_x = nt[0] - (nt[0] - tt[0]) * a / (cy - tt[1])
                left = back_x + off0 - shrink * a
                right = left + max(5.0, w0 * (1.0 - (a / (reach + 10.0)) ** 1.6) + 4.0)
                pts_l.append((left, cy + dy))
                pts_r.append((right, cy + dy))
            window = poly_mask(pts_l + pts_r[::-1])
            glow_edge = Image.fromarray((window * 255).astype(np.uint8)).filter(ImageFilter.GaussianBlur(3))
            rim = np.clip(np.asarray(glow_edge).astype(np.float32) / 255.0 - window * 0.6, 0, 1)
            over(canvas, np.full((H, W, 3), 25.0, np.float32), window)  # the dark wall of the slot
            over(canvas, circuit_layer(window, 7 + n), window * 0.96)
            inner = np.asarray(Image.fromarray((window * 255).astype(np.uint8)).filter(ImageFilter.GaussianBlur(4))).astype(np.float32) / 255.0
            over(canvas, np.zeros((H, W, 3), np.float32), np.clip(window - inner, 0, 1) * 0.7)
            over(canvas, np.stack([np.full((H, W), 90.0), np.full((H, W), 160.0), np.full((H, W), 255.0)], -1).astype(np.float32), rim * 0.55)
    if wall is not None:
        bg = np.zeros((H, W, 4), np.float32)
        gy = np.linspace(0, 1, H, dtype=np.float32)[:, None] * np.ones((1, W), np.float32)
        for c in range(3):
            bg[..., c] = wall[c] * (1.06 - 0.12 * gy)
        bg[..., 3] = 1.0
        over(bg, canvas[..., :3], canvas[..., 3])
        canvas = bg
    rgba = np.concatenate([np.clip(canvas[..., :3], 0, 255), np.clip(canvas[..., 3:4], 0, 1) * 255.0], axis=-1)
    return Image.fromarray(rgba.astype(np.uint8), "RGBA")


if __name__ == "__main__":
    out = sys.argv[1] if len(sys.argv) > 1 else "iyakki-logo.png"
    draw_emblem(wall=(36, 38, 42)).save(out, optimize=True)
    print("wrote", out)
