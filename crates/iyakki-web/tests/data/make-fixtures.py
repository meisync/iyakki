#!/usr/bin/env python3
"""Makes the decoder test pictures of crates/iyakki-web/tests/images.rs from nothing: a synthetic test card drawn here (fixed numbers,
no photograph, no third-party picture), encoded by libjpeg, giflib-style GIF and libwebp (through Pillow and ImageMagick), and the
expected pixels as decoded by Pillow, an independent decoder. Dedicated by the project to the public under Apache-2.0.

    cd crates/iyakki-web/tests/data && python3 make-fixtures.py
Needs: python3, numpy, Pillow with WebP, ImageMagick `convert` with WebP (a developer tool only; nothing here is shipped).
"""
import io, math, os, subprocess, tempfile
import numpy as np
from PIL import Image, ImageDraw

HERE = os.path.dirname(os.path.abspath(__file__))


def card(w, h, alpha=False):
    """A test card: smooth colour ramps, flat blocks, a disc and a diagonal; alpha: a transparent corner and a soft ramp."""
    x = np.linspace(0, 1, w, dtype=np.float32)[None, :]
    y = np.linspace(0, 1, h, dtype=np.float32)[:, None]
    a = np.stack([255 * x + 0 * y, 255 * (1 - x) * (0.35 + 0.65 * y), 255 * y * (0.45 + 0.55 * x)], -1)
    im = Image.fromarray(np.clip(a, 0, 255).astype(np.uint8), "RGB").convert("RGBA")
    d = ImageDraw.Draw(im)
    d.rectangle([w // 10, h // 8, w // 3, h // 2], fill=(240, 220, 40, 255))
    d.rectangle([w // 2, h // 2, w - w // 8, h - h // 8], fill=(30, 90, 200, 255))
    d.ellipse([w // 3, h // 4, w // 3 + h // 2, h // 4 + h // 2], fill=(20, 160, 90, 255))
    d.line([(0, h - 1), (w - 1, 0)], fill=(255, 255, 255, 255), width=1)
    if alpha:
        al = np.clip(255 * (0.15 + 0.85 * np.hypot(x - 0.5, y - 0.5) * 1.6), 0, 255).astype(np.uint8)
        al = np.broadcast_to(al, (h, w)).copy()
        al[: h // 5, : w // 5] = 0
        im.putalpha(Image.fromarray(al, "L"))
    return im


def raw(path, im, channels):
    im = im.convert("RGBA" if channels == 4 else "RGB")
    open(path, "wb").write(im.tobytes())


def out(name):
    return os.path.join(HERE, name)


def fnv1a(b):
    h = 0xcbf29ce484222325
    for c in b:
        h = ((h ^ c) * 0x100000001b3) & 0xFFFFFFFF_FFFFFFFF
    return h


def main():
    rgb = card(67, 45).convert("RGB")
    rgb.save(out("rgb.png"))
    for name, kw in [("base444", dict(subsampling=0)), ("base422", dict(subsampling=1)), ("base420", dict(subsampling=2)),
                     ("opt420", dict(subsampling=2, optimize=True)), ("prog444", dict(subsampling=0, progressive=True)),
                     ("prog420", dict(subsampling=2, progressive=True))]:
        rgb.save(out(name + ".jpg"), quality=90, **kw)
        raw(out(name + ".rgb"), Image.open(out(name + ".jpg")), 3)
    g = rgb.convert("L")
    g.save(out("gray.jpg"), quality=90)
    g.save(out("gray_prog.jpg"), quality=90, progressive=True)
    for n in ("gray", "gray_prog"):
        raw(out(n + ".rgb"), Image.open(out(n + ".jpg")), 3)
    # GIFs: a 16-colour palette, colour 0 transparent, plain and interlaced
    c = card(67, 45, alpha=True)
    pal = c.convert("RGB").quantize(15, method=Image.Quantize.MEDIANCUT)
    p = Image.new("P", c.size)
    p.putpalette([0, 0, 0] + pal.getpalette()[: 15 * 3])
    idx = np.array(pal, dtype=np.uint8) + 1
    idx[np.array(c.getchannel("A")) < 128] = 0
    p.putdata(idx.flatten().tolist())
    p.save(out("pal.gif"), transparency=0, interlace=False)
    p.save(out("pal_interlaced.gif"), interlace=True)
    raw(out("pal_gif_expected.rgba"), Image.open(out("pal.gif")), 4)
    raw(out("pal_interlaced_expected.rgba"), Image.open(out("pal_interlaced.gif")), 4)
    # WebP
    files = []

    def webp(name, im, **kw):
        im.save(out(name), "WEBP", **kw)
        files.append(name)

    webp("webp_lossy.webp", rgb, quality=80, method=4)
    webp("webp_lossy_big.webp", card(160, 120).convert("RGB"), quality=75, method=4)
    webp("webp_lossy_1x1.webp", Image.new("RGB", (1, 1), (200, 60, 90)), quality=80)
    webp("webp_lossless.webp", card(57, 41, alpha=True), lossless=True)
    webp("webp_lossless_opaque.webp", card(48, 36).convert("RGB"), lossless=True)
    pc = Image.new("RGB", (50, 40), (250, 250, 250))
    dr = ImageDraw.Draw(pc)
    for i, col in enumerate([(200, 30, 30), (30, 160, 60), (30, 60, 200), (240, 200, 20)]):
        dr.rectangle([i * 12, 4 + i * 6, i * 12 + 11, 30], fill=col)
    webp("webp_lossless_palette.webp", pc, lossless=True)
    webp("webp_lossy_alpha_lossless.webp", card(61, 43, alpha=True), quality=75, alpha_quality=100, method=4)
    # the encoder switches that Pillow does not expose: through ImageMagick
    with tempfile.TemporaryDirectory() as t:
        card(70, 50).convert("RGB").save(os.path.join(t, "a.png"))
        subprocess.run(["convert", os.path.join(t, "a.png"), "-quality", "75", "-define", "webp:filter-type=0", "-define", "webp:filter-strength=60",
                        out("webp_lossy_simple_filter.webp")], check=True)
        files.append("webp_lossy_simple_filter.webp")
        card(33, 27, alpha=True).save(os.path.join(t, "b.png"))
        subprocess.run(["convert", os.path.join(t, "b.png"), "-quality", "75", "-define", "webp:alpha-compression=0", "-define", "webp:alpha-filtering=2",
                        out("webp_lossy_alpha_raw_gradient.webp")], check=True)
        files.append("webp_lossy_alpha_raw_gradient.webp")
    # a two-frame animation
    f1, f2 = card(40, 30, alpha=True), card(40, 30, alpha=True).transpose(Image.FLIP_LEFT_RIGHT)
    f1.save(out("webp_anim.webp"), "WEBP", save_all=True, append_images=[f2], duration=100, loop=0, lossless=True)
    files.append("webp_anim.webp")
    order = ["webp_lossy.webp", "webp_lossy_simple_filter.webp", "webp_lossy_alpha_lossless.webp", "webp_lossy_alpha_raw_gradient.webp", "webp_lossy_1x1.webp",
             "webp_lossy_big.webp", "webp_lossless.webp", "webp_lossless_opaque.webp", "webp_lossless_palette.webp", "webp_anim.webp"]
    lines = []
    for n in order:
        im = Image.open(out(n))
        im.seek(0)
        im = im.convert("RGBA")
        lines.append(f"{n} {im.width} {im.height} {fnv1a(im.tobytes()):016x}")
    open(out("webp_expected.txt"), "w").write("\n".join(lines) + "\n")
    print("written", len(lines), "webp lines")


main()
