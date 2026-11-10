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


def emblem(size=384, circle=378):
    """The logo: three brushed-steel arrow heads with blue circuit light, drawn from nothing by dist/brand/draw-logo.py (fixed numbers, the same
    picture every run), cropped to its shape and centred in a transparent square. `circle` is the widest the picture may be."""
    import importlib.util
    spec = importlib.util.spec_from_file_location("draw_logo", P("dist/brand/draw-logo.py"))
    mod = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(mod)
    cut = mod.draw_emblem()
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
