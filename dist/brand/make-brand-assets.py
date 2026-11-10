#!/usr/bin/env python3
"""Draws IYAKKI's own pictures from nothing, with fixed numbers (the same files every run): the emblem (logo.png), the GRUB
background and two small test fixtures. No third-party picture, font file or model is used
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


def cut_owner_logo(path):
    """The owner's logo picture (brushed-steel arrow heads with blue circuit light, on a dark grey wall) as an RGBA picture: the wall is the part of the
    picture that the corners reach through pixels that change by only a few levels from one to the next and stay dark; the steel and the circuit light
    stop it. Deterministic (no model, no random numbers)."""
    from collections import deque
    src = Image.open(path).convert("RGB")
    a = np.asarray(src).astype(np.int16)
    lum = a.mean(axis=2)
    h, w = lum.shape
    wall = np.zeros((h, w), bool)
    q = deque()
    for x0, y0 in ((0, 0), (w - 1, 0), (0, h - 1), (w - 1, h - 1)):
        wall[y0, x0] = True
        q.append((y0, x0))
    while q:
        y, x = q.popleft()
        for dy, dx in ((1, 0), (-1, 0), (0, 1), (0, -1)):
            ny, nx = y + dy, x + dx
            if 0 <= ny < h and 0 <= nx < w and not wall[ny, nx] and lum[ny, nx] < 100 and abs(lum[ny, nx] - lum[y, x]) <= 3.5:
                wall[ny, nx] = True
                q.append((ny, nx))
    alpha = Image.fromarray(((~wall) * 255).astype(np.uint8))
    alpha = alpha.filter(ImageFilter.MinFilter(3)).filter(ImageFilter.GaussianBlur(0.9))
    out = src.convert("RGBA")
    out.putalpha(alpha)
    return out


def emblem(size=384, circle=378):
    """The logo: the owner's picture (dist/brand/owner-logo.png, supplied 2026-10-31), cut out of its background, centred in a transparent square.
    `circle` is the widest the picture may be (the old round emblem filled 378 of 384 pixels)."""
    cut = cut_owner_logo(P("dist/brand/owner-logo.png"))
    cut = cut.crop(cut.getchannel("A").point(lambda v: 255 if v > 24 else 0).getbbox())
    k = min(circle * 0.94 / cut.width, circle * 0.94 / cut.height)
    cut = cut.resize((round(cut.width * k), round(cut.height * k)), Image.LANCZOS)
    im = Image.new("RGBA", (size, size), (0, 0, 0, 0))
    im.alpha_composite(cut, ((size - cut.width) // 2, (size - cut.height) // 2))
    return im


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
    logo(); grub_bg(); fixtures()
    print("written")
