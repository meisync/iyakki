"""Golden screens: a masked tolerance diff (Pillow, in-house maths) and the review pictures.

A golden is dist/autotest/golden/<ROW>/<step>.png. compare() ignores the mask boxes (clock, addresses, battery), counts the pixels whose
channels differ by more than `tolerance`, and passes when that share is below `budget` percent. A missing golden is NEW (a candidate, never a pass).
The OCR text expectation of the step carries the proof; this diff is a regression alarm (docs/development/AUTOTEST.md).
"""
import os
import re

try:
    from PIL import Image, ImageChops
    HAVE_PIL = True
except ImportError:  # pragma: no cover
    HAVE_PIL = False

DEFAULT_BUDGET = 0.5      # percent of pixels
DEFAULT_TOLERANCE = 24    # per channel, 0..255


def parse_masks(words):
    """['mask=10,20,30,40', 'mask=0,0,5,5'] -> [(x, y, w, h), ...]; a bad box raises ValueError."""
    out = []
    for w in words:
        m = re.match(r"^mask=(\d+),(\d+),(\d+),(\d+)$", w)
        if not m:
            raise ValueError("a mask is mask=x,y,w,h, not %r" % w)
        out.append(tuple(int(g) for g in m.groups()))
    return out


def golden_path(golden_root, row, step):
    if not re.match(r"^[A-Za-z0-9._-]+$", step) or not re.match(r"^F-[A-Z0-9]+-\d+$", row):
        raise ValueError("bad golden name %s/%s" % (row, step))
    return os.path.join(golden_root, row, step + ".png")


def compare(golden, actual, masks=(), budget=DEFAULT_BUDGET, tolerance=DEFAULT_TOLERANCE):
    """-> dict(ok, changed_percent, size_ok, counted). Both arguments are PNG paths. A different size is a failure, never a pass."""
    if not HAVE_PIL:
        raise RuntimeError("golden screens need Pillow")
    a = Image.open(golden).convert("RGB")
    b = Image.open(actual).convert("RGB")
    if a.size != b.size:
        return {"ok": False, "changed_percent": 100.0, "size_ok": False, "counted": a.size[0] * a.size[1]}
    diff = ImageChops.difference(a, b)
    # per-pixel: the largest channel difference
    r, g, bl = diff.split()
    mx = ImageChops.lighter(ImageChops.lighter(r, g), bl)
    hot = mx.point(lambda v: 255 if v > tolerance else 0)
    w, h = a.size
    keep = Image.new("L", (w, h), 255)
    for x, y, mw, mh in masks:
        keep.paste(0, (x, y, min(w, x + mw), min(h, y + mh)))
    hot = ImageChops.multiply(hot, keep)
    changed = sum(1 for v in hot.getdata() if v)
    counted = sum(1 for v in keep.getdata() if v)
    pct = 100.0 * changed / counted if counted else 0.0
    return {"ok": pct <= budget, "changed_percent": round(pct, 4), "size_ok": True, "counted": counted, "_hot": hot}


def write_diff_picture(golden, actual, hot, path):
    """Side by side: golden | actual | changed pixels in red. For the review page."""
    a = Image.open(golden).convert("RGB")
    b = Image.open(actual).convert("RGB")
    w, h = a.size
    red = Image.new("RGB", (w, h), (0, 0, 0))
    red.paste((255, 40, 40), (0, 0, w, h), hot)
    sheet = Image.new("RGB", (w * 3, h), (30, 30, 30))
    sheet.paste(a, (0, 0))
    sheet.paste(b, (w, 0))
    sheet.paste(red, (2 * w, 0))
    sheet.save(path)
    return path
