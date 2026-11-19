# SPDX-License-Identifier: Apache-2.0
# Copyright IYAKKI contributors. Licensed under the Apache License, Version 2.0 (see LICENSE at the root of the repository).
"""F-WASM-08/09/10 in real Chrome: `ui/tilerender.js` draws the display lists the Rust reference renderer drew (the goldens), on a real
canvas, and the result is compared twice: the canvas pixels read back (exact) and a real screenshot of the page (a tolerance, because
the screenshot goes through the browser's compositor). Then the Tiles page of the real web service is driven with a real mouse and
keyboard and compared with the Rust pictures of the same states."""
import json
import os

import harness
import png

TOLERANCE = 2  # screenshot, per channel (0-255)
BG = [200, 210, 220, 255]

PAGE = r"""<!doctype html><meta charset="utf-8"><title>tile</title>
<style>html,body{margin:0;padding:0;background:#c8d2dc}canvas{display:block}</style>
<canvas id="c"></canvas>
<script src="/ui/tilerender.js"></script>
<script>
async function show(name, scale) {
  const frame = await (await fetch('/golden/' + name + '.json')).json();
  const c = document.getElementById('c');
  const sz = IyakkiTileRender.draw(c, frame, scale, [200, 210, 220, 255]);
  return { w: sz.w, h: sz.h, cw: c.width, ch: c.height };
}
// compare the canvas's own pixels (read back) with the Rust reference RGBA; at scale > 1 each reference pixel is a scale x scale block
async function readback(name, scale) {
  const want = new Uint8Array(await (await fetch('/golden/' + name + '.rgba')).arrayBuffer());
  const c = document.getElementById('c');
  const got = c.getContext('2d').getImageData(0, 0, c.width, c.height).data;
  const w = c.width / scale, h = c.height / scale;
  let differ = 0, worst = 0;
  for (let y = 0; y < c.height; y++) for (let x = 0; x < c.width; x++) {
    const gi = (y * c.width + x) * 4, wi = (((y / scale) | 0) * w + ((x / scale) | 0)) * 4;
    let d = 0; for (let k = 0; k < 4; k++) d = Math.max(d, Math.abs(got[gi + k] - want[wi + k]));
    if (d) differ++; if (d > worst) worst = d;
  }
  return { differ, worst, cw: c.width, ch: c.height };
}
</script>"""


def suite(rep, chrome, ui_dir, tiles_bin, work):
    gdir = os.path.join(work, "golden")
    import jsdrivers
    cases = jsdrivers.write_goldens(tiles_bin, gdir)
    srv = harness.Static({"/ui/": ui_dir, "/golden/": gdir}, {"/tile.html": ("text/html", PAGE.encode())})
    try:
        for scale in (1, 2):
            page = chrome.new_page(400, 300, scale)  # device pixel ratio = scale
            try:
                page.goto(srv.base + "/tile.html")
                for name in cases:
                    meta = json.loads(open(os.path.join(gdir, name + ".json")).read())
                    w, h = meta["width"], meta["height"]
                    page.set_viewport(max(w, 1), max(h, 1), scale)
                    page.evaluate("show(%s, %d)" % (json.dumps(name), scale))
                    rb = page.evaluate("readback(%s, %d)" % (json.dumps(name), scale))
                    rep.check("%s at device ratio %d: canvas pixels equal the Rust reference picture (tolerance 0; %d differ, worst %d)" % (name, scale, rb["differ"], rb["worst"]), rb["differ"] == 0 and rb["cw"] == w * scale and rb["ch"] == h * scale, rb)
                    shot = page.screenshot(rep.shot("tile-%s-x%d.png" % (name, scale)))
                    sw, sh, rgba = png.decode(shot)
                    want = open(os.path.join(gdir, name + ".rgba"), "rb").read()
                    # the reference at the same scale: every pixel repeated scale x scale times
                    if scale > 1:
                        rows = []
                        for y in range(h):
                            row = b"".join(want[(y * w + x) * 4:(y * w + x) * 4 + 4] * scale for x in range(w))
                            rows.extend([row] * scale)
                        ref = b"".join(rows)
                    else:
                        ref = want
                    worst, n = png.compare(rgba, ref, w * scale, h * scale, sw, w * scale)
                    rep.check("%s at device ratio %d: real screenshot within %d per channel of the reference (%d pixels differ at all, worst %d, screenshot %dx%d)" % (name, scale, TOLERANCE, n, worst, sw, sh), worst <= TOLERANCE and sw >= w * scale and sh >= h * scale, (worst, n, sw, sh))
                    if name != "empty":
                        nonbg = sum(1 for i in range(0, len(ref), 4) if ref[i:i + 3] != bytes(BG[:3]))
                        rep.check("%s: the reference is not just the background (%d pixels drawn)" % (name, nonbg), nonbg > 0)
                    with open(rep.shot("tile-%s-x%d-reference.png" % (name, scale)), "wb") as f:
                        f.write(png.encode(w * scale, h * scale, ref))
                rep.check("no console errors at device ratio %d" % scale, page.errors() == [], page.errors())
            finally:
                page.close()
    finally:
        srv.stop()


READ_CANVAS = """(function(){var c=document.querySelector('canvas.tile-canvas');if(!c)return null;var d=c.getContext('2d').getImageData(0,0,c.width,c.height).data;var s='',i;var b=new Uint8Array(d.buffer,d.byteOffset,d.length);var chunk=[];for(i=0;i<b.length;i+=4096){chunk.push(String.fromCharCode.apply(null,b.subarray(i,i+4096)));}return {w:c.width,h:c.height,b64:btoa(chunk.join(''))};})()"""


def _canvas(page):
    import base64
    r = page.evaluate(READ_CANVAS)
    return r["w"], r["h"], base64.b64decode(r["b64"]) if r else None


def service(rep, chrome, fx, work):
    """The Tiles page of the real service with a real mouse and keyboard: the example counter tile (a real tile running in the service's
    tile host) must draw the same pictures as the Rust reference renderer drew for the same states (the goldens)."""
    import base64  # noqa: F401
    import pagesuite
    gdir = os.path.join(work, "golden")
    page = chrome.new_page(1000, 800, 1)
    try:
        pagesuite.login(page, fx.base, "root", fx.info["admin_password"])
        pagesuite.goto_page(page, "Tiles")
        page.wait_for("!!document.querySelector('canvas.tile-canvas')", 10, "tile canvas")
        want0 = open(os.path.join(gdir, "counter-0.rgba"), "rb").read()
        meta = json.load(open(os.path.join(gdir, "counter-0.json")))

        def same(want, tries=60):
            import time
            last = None
            for _ in range(tries):
                w, h, got = _canvas(page)
                if got is not None and len(got) == len(want):
                    worst, n = png.compare(got, want, w, h)
                    last = (worst, n)
                    if n == 0:
                        return True, last
                time.sleep(0.1)
            return False, last

        ok, last = same(want0)
        rep.check("the Tiles page draws the example counter tile as the Rust reference does (counter-0, tolerance 0)", ok, last)
        page.screenshot(rep.shot("service-tile-counter-0.png"))
        hit = next(o for o in meta["ops"] if o.get("op") == "hit" and o.get("id") == 1)
        cx, cy = hit["x"] + hit["w"] // 2, hit["y"] + hit["h"] // 2
        box = page.evaluate("(function(){var r=document.querySelector('canvas.tile-canvas').getBoundingClientRect();return [r.x,r.y];})()")
        cursor = page.evaluate("(function(){var c=document.querySelector('canvas.tile-canvas');return c.className;})()")
        for _ in range(3):
            page.click_at(box[0] + cx, box[1] + cy)
            page.evaluate("new Promise(function(r){setTimeout(r,250)})")
        page.type_text("Hello, tile! 42?")
        want = open(os.path.join(gdir, "counter-typed.rgba"), "rb").read()
        ok, last = same(want)
        rep.check("three real clicks on the hit region and typed text give the Rust picture of the same state (counter-typed, tolerance 0)", ok, last)
        page.screenshot(rep.shot("service-tile-counter-typed.png"))
        rep.check("the Tiles page with a running tile: no console errors (the 401 of the whoami probe is expected)", page.errors(ignore=["api/v1/whoami"]) == [], page.errors(ignore=["api/v1/whoami"]))
    finally:
        page.close()
