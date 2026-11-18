# SPDX-License-Identifier: Apache-2.0
# Copyright IYAKKI contributors. Licensed under the Apache License, Version 2.0 (see LICENSE at the root of the repository).
"""F-SDK-22 in real Chrome: the client WASM engine (the committed `iyakki-client-engine.wasm` and its glue `js/engine.js`) draws the UI SDK's
display list. The frames come from the real preview server (`iyakki-ui-preview`, the SAMPLE app); the page loads the module, hands it the list,
paints the module's framebuffer on a canvas and the text runs with the browser's fonts, builds the accessibility tree from the module's answer,
and turns a real mouse click and real key presses into the input lines the module makes; this script carries those lines to the preview server
and the next frame back, so the loop engine -> app -> engine is the real one. The same frame is also drawn by the JavaScript renderer
(`ui/displaylist.js`) on a second canvas and the two are compared."""
import json
import os
import urllib.request

import harness

PAGE = r"""<!doctype html><meta charset=utf-8><title>engine</title>
<style>body{margin:0;background:#222}#wrap{position:relative;display:inline-block}canvas{display:block}#aria{position:absolute;left:0;top:0;pointer-events:none}#aria>div{position:absolute}</style>
<div id=wrap><canvas id=c></canvas><div id=aria></div></div><canvas id=js style="display:none"></canvas>
<script src="/ui/displaylist.js"></script>
<script type="module">
import { IyakkiEngine } from "/engine/js/engine.js";
const e = await IyakkiEngine.load(await (await fetch("/engine/assets/iyakki-client-engine.wasm")).arrayBuffer());
const S = window.S = { e, lines: [], info: null, error: null, frames: 0, scale: 1 };
const c = document.getElementById("c"), ctx = c.getContext("2d");
const font = (st, px) => (st === "bold" ? "bold " : "") + px + "px " + (st === "mono" ? "ui-monospace, monospace" : "system-ui, sans-serif");
window.show = function (text, scale) {
  const r = e.uiLoad(text, scale || S.scale);
  if (r.error) { S.error = r.error; return r; }
  S.error = null; S.info = r; S.scale = scale || S.scale; S.frames++;
  const fb = e.uiFramebuffer();
  c.width = fb.w; c.height = fb.h; c.style.width = (fb.w / S.scale) + "px"; c.style.height = (fb.h / S.scale) + "px";
  ctx.putImageData(new ImageData(new Uint8ClampedArray(fb.data), fb.w, fb.h), 0, 0);
  for (const t of r.texts) {
    ctx.save();
    if (t.clip) { ctx.beginPath(); ctx.rect(t.clip[0], t.clip[1], t.clip[2] - t.clip[0], t.clip[3] - t.clip[1]); ctx.clip(); }
    ctx.font = font(t.style, t.size); ctx.fillStyle = "rgba(" + t.color[0] + "," + t.color[1] + "," + t.color[2] + "," + t.color[3] / 255 + ")";
    ctx.fillText(t.text, t.x, t.y + t.size * 0.8); ctx.restore();
  }
  const aria = document.getElementById("aria"); aria.textContent = "";
  for (const n of r.a11y) {
    const d = document.createElement("div"); d.setAttribute("role", { item: "listitem", field: "textbox", image: "img" }[n.role] || n.role);
    d.setAttribute("aria-label", n.name); d.setAttribute("data-action", n.action); d.tabIndex = 0;
    d.style.left = n.x / S.scale + "px"; d.style.top = n.y / S.scale + "px"; d.style.width = n.w / S.scale + "px"; d.style.height = n.h / S.scale + "px";
    d.onkeydown = (ev) => { if (ev.key === "Enter" || ev.key === " ") { ev.preventDefault(); S.lines.push("action " + n.action); } };
    aria.appendChild(d);
  }
  // the JavaScript renderer draws the same list at the same scale for the comparison
  const j = document.getElementById("js"); const D = window.IyakkiDisplayList; D.draw(j, D.parse(text), S.scale);
  return r;
};
c.addEventListener("pointerdown", (ev) => { const b = c.getBoundingClientRect(); const x = (ev.clientX - b.left) * S.scale, y = (ev.clientY - b.top) * S.scale;
  S.lines.push(e.uiLine({ kind: "pointer", phase: "down", x, y, t: Math.round(ev.timeStamp) })); });
c.addEventListener("pointerup", (ev) => { const b = c.getBoundingClientRect(); const x = (ev.clientX - b.left) * S.scale, y = (ev.clientY - b.top) * S.scale;
  S.lines.push(e.uiLine({ kind: "pointer", phase: "up", x, y, t: Math.round(ev.timeStamp) })); });
addEventListener("keydown", (ev) => {
  if (ev.target && ev.target.getAttribute && ev.target.getAttribute("role")) return;
  const names = { Enter: "enter", Backspace: "backspace", Delete: "delete", Escape: "esc", Tab: "tab", ArrowUp: "up", ArrowDown: "down", ArrowLeft: "left", ArrowRight: "right" };
  let l = null;
  if (names[ev.key]) l = e.uiLine({ kind: "key", name: names[ev.key] });
  else if (ev.key.length === 1 && !ev.ctrlKey && !ev.metaKey) l = e.uiLine({ kind: "key", name: "char", char: ev.key });
  if (l) { S.lines.push(l); ev.preventDefault(); }
});
window.ready = true;
</script>"""

COMPARE = r"""(() => {
  const a = document.getElementById('c'), b = document.getElementById('js');
  if (a.width !== b.width || a.height !== b.height) return { size: [a.width, a.height, b.width, b.height] };
  // shapes only: the two renderers draw text with the same browser font, so compare where neither has text (mask every text run)
  const S = window.S, w = a.width, h = a.height;
  const mask = new Uint8Array(w * h);
  const ctx = a.getContext('2d');
  const m = document.createElement('canvas').getContext('2d');
  for (const t of S.info.texts) {
    m.font = (t.style === 'bold' ? 'bold ' : '') + t.size + 'px ' + (t.style === 'mono' ? 'ui-monospace, monospace' : 'system-ui, sans-serif');
    const tw = Math.ceil(m.measureText(t.text).width) + 3;
    for (let y = Math.max(0, Math.floor(t.y - 2)); y < Math.min(h, Math.ceil(t.y + t.size * 1.4)); y++) for (let x = Math.max(0, Math.floor(t.x - 2)); x < Math.min(w, t.x + tw); x++) mask[y * w + x] = 1;
  }
  const da = ctx.getImageData(0, 0, w, h).data, db = b.getContext('2d').getImageData(0, 0, w, h).data;
  let total = 0, off = 0, worst = 0;
  for (let i = 0; i < w * h; i++) {
    if (mask[i]) continue;
    total++;
    const d = Math.max(Math.abs(da[i * 4] - db[i * 4]), Math.abs(da[i * 4 + 1] - db[i * 4 + 1]), Math.abs(da[i * 4 + 2] - db[i * 4 + 2]));
    worst = Math.max(worst, d);
    if (d > 24) off++;
  }
  return { total, off, worst };
})()"""


def post(base, lines):
    req = urllib.request.Request(base.rstrip("/") + "/event", data=("\n".join(lines) + "\n").encode(), method="POST", headers={"X-Iyakki-Preview": "1"})
    return urllib.request.urlopen(req).read().decode()


def suite(rep, chrome, preview_bin, root):
    import uisdksuite
    eng = os.path.join(root, "crates/iyakki-client-engine")
    ui = os.path.join(root, "crates/iyakki-agent/src/webservice/ui")
    if not os.path.isfile(os.path.join(eng, "assets/iyakki-client-engine.wasm")):
        rep.check("the committed client engine module exists", False)
        return
    proc, base = uisdksuite.start_preview(preview_bin, "800x480")
    srv = harness.Static({"/engine/": eng, "/ui/": ui}, {"/p.html": ("text/html", PAGE.encode())})
    page = chrome.new_page(900, 560, 1)
    try:
        page.goto(srv.base + "/p.html")
        page.wait_for("window.ready === true", what="the engine module loaded in the page")
        n = page.evaluate("fetch('/engine/assets/iyakki-client-engine.wasm').then(r => r.arrayBuffer()).then(b => WebAssembly.Module.imports(new WebAssembly.Module(b)).length)")
        rep.check("the committed module has no imports and instantiated in Chrome through the page's own glue (js/engine.js)", n == 0)

        def frame():
            return urllib.request.urlopen(base.rstrip("/") + "/frame").read().decode()

        text = frame()
        r = page.evaluate("(() => { const r = window.show(%s, 1); return { w: r.w, h: r.h, lw: r.lw, lh: r.lh, n: r.a11y.length, t: r.texts.length, e: r.error || null } })()" % json.dumps(text))
        rep.check("the module draws the sample app's list: a %dx%d picture, %d accessibility nodes, %d text runs" % (r["w"], r["h"], r["n"], r["t"]), r["e"] is None and (r["w"], r["h"]) == (800, 480) and r["n"] > 5 and r["t"] > 8, r)
        cmp1 = page.evaluate(COMPARE)
        rep.check("the module's picture equals the JavaScript renderer's on the same list: %s of %s non-text pixels differ by more than 24 (worst %s)" % (cmp1.get("off"), cmp1.get("total"), cmp1.get("worst")), "off" in cmp1 and cmp1["off"] * 100 <= cmp1["total"], cmp1)
        # the opaque rectangles read back from the canvas are the list's colour (same probe the JavaScript renderer gets)
        res = page.evaluate("""(() => { const D = window.IyakkiDisplayList, list = D.parse(%s), ctx = document.getElementById('c').getContext('2d'); let tested = 0, bad = [];
          list.ops.forEach((o, i) => { if ((o.op !== 'rect' && o.op !== 'rrect') || o.c[3] !== 255 || o.r.w < 6 || o.r.h < 6) return; const rr = o.op === 'rrect' ? Math.min(o.radius, o.r.w / 2, o.r.h / 2) : 0; const x = o.r.x + rr + 1.5, y = o.r.y + 2.5; let cov = false;
            for (let j = i + 1; j < list.ops.length; j++) { const q = list.ops[j]; if (['rect','rrect','grad','outline','shadow'].includes(q.op) && x >= q.r.x - 2 && y >= q.r.y - 2 && x < q.r.x + q.r.w + 2 && y < q.r.y + q.r.h + 2) cov = true; if (q.op === 'text' && x >= q.x - 2 && x < q.x + q.size * q.text.length && y >= q.y - 2 && y < q.y + q.size * 1.5) cov = true; }
            if (cov) return; tested++; const g = Array.from(ctx.getImageData(Math.round(x), Math.round(y), 1, 1).data); if (Math.max(Math.abs(g[0]-o.c[0]), Math.abs(g[1]-o.c[1]), Math.abs(g[2]-o.c[2])) > 1) bad.push({ i, want: o.c, g }); });
          return { tested, bad }; })()""" % json.dumps(text))
        rep.check("canvas pixels equal the list's colour at %d opaque rectangles that nothing covers" % res["tested"], res["tested"] >= 3 and not res["bad"], res["bad"][:3])
        info = page.evaluate("window.S.info")
        want = [(n["name"], uisdksuite.ROLE[n["role"]], str(n["action"])) for n in page.evaluate("IyakkiDisplayList.a11y(IyakkiDisplayList.parse(%s)).map(n=>({name:n.name,role:n.role,action:n.action}))" % json.dumps(text))]
        got = [(e["name"], e["role"], e["action"]) for e in page.evaluate("Array.from(document.querySelectorAll('#aria > div')).map(e=>({role:e.getAttribute('role'),name:e.getAttribute('aria-label'),action:e.getAttribute('data-action')}))")]
        rep.check("the ARIA tree built from the module's answer has the name, role and action of the %d hit regions the JavaScript reader finds" % len(want), want == got and len(want) > 5, (want[:3], got[:3]))
        page.screenshot(rep.shot("uiengine-first-frame.png"))

        # real mouse and keyboard: lines made by the module go to the real app and the next frame comes back to the module
        def send_pending():
            lines = page.evaluate("(() => { const l = window.S.lines.splice(0); return l; })()")
            if not lines:
                return None
            nxt = post(base, lines)
            page.evaluate("window.show(%s)" % json.dumps(nxt))
            return lines

        def centre(name):
            return page.evaluate("(()=>{const e=Array.from(document.querySelectorAll('#aria > div')).find(e=>e.getAttribute('aria-label')==%s);const b=document.getElementById('c').getBoundingClientRect();return e?[b.left+parseFloat(e.style.left)+parseFloat(e.style.width)/2,b.top+parseFloat(e.style.top)+parseFloat(e.style.height)/2]:null})()" % json.dumps(name))

        x, y = centre("New note")
        page.click_at(x, y)
        lines = send_pending()
        rep.check("a real click on the field makes the module's pointer lines (%s) that the app accepted" % lines, lines is not None and lines[0].startswith("ptr 0 down ") and lines[-1].startswith("ptr 0 up "), lines)
        for ch in "buy milk":
            page.press(ch, "Key" + ch.upper() if ch.isalpha() else "Space", ord(ch.upper()), 0, ch)
        page.press("Enter", "Enter", 13)
        lines = send_pending()
        texts = page.evaluate("window.S.info.texts.map(t => t.text)")
        rep.check("real key presses become key lines (%d) and the new frame the module draws shows the note" % len(lines or []), lines is not None and "[ ] buy milk" in texts, texts[:8])
        x, y = centre("[ ] buy milk")
        page.click_at(x, y)
        send_pending()
        rep.check("a click on the row toggles it in the app and the module draws [x] buy milk", "[x] buy milk" in page.evaluate("window.S.info.texts.map(t => t.text)"))
        # a screen reader's way: focus the ARIA element and press Enter -> `action N`
        page.evaluate("Array.from(document.querySelectorAll('#aria > div')).find(e=>(e.getAttribute('aria-label')||'').startsWith('Clear done')).focus()")
        page.evaluate("document.getElementById('aria').style.pointerEvents='auto'")
        page.press("Enter", "Enter", 13)
        lines = send_pending()
        rep.check("Enter on a focused ARIA element sends %s and the app clears the note" % lines, lines is not None and lines[0].startswith("action ") and not any("buy milk" in t for t in page.evaluate("window.S.info.texts.map(t => t.text)")), lines)
        page.screenshot(rep.shot("uiengine-after-clear.png"))

        # layouts: the resize line the module made gives the phone layout
        page.evaluate("window.S.lines.push(window.S.e.uiLine({kind:'resize', w: 360, h: 740}))")
        send_pending()
        p = page.evaluate("({w: document.getElementById('c').width, h: document.getElementById('c').height, t: window.S.info.texts.map(t => t.text)})")
        rep.check("a resize line from the module gives the 360x740 compact layout drawn at that size", (p["w"], p["h"]) == (360, 740) and any("compact pane" in t for t in p["t"]), p["t"][:4])
        cmp2 = page.evaluate(COMPARE)
        rep.check("the phone frame also equals the JavaScript renderer's (%s of %s differ, worst %s)" % (cmp2.get("off"), cmp2.get("total"), cmp2.get("worst")), "off" in cmp2 and cmp2["off"] * 100 <= cmp2["total"], cmp2)
        page.screenshot(rep.shot("uiengine-phone.png"))
        # two device pixels per unit
        page.evaluate("window.S.scale = 2; window.show(%s, 2)" % json.dumps(frame()))
        d = page.evaluate("({w: document.getElementById('c').width, css: document.getElementById('c').style.width})")
        cmp3 = page.evaluate(COMPARE)
        rep.check("at 2 pixels per unit the picture is 720 wide and shown 360 CSS pixels wide (%s) and equals the JavaScript renderer's (%s of %s differ)" % (d, cmp3.get("off"), cmp3.get("total")), d["w"] == 720 and d["css"] == "360px" and "off" in cmp3 and cmp3["off"] * 100 <= cmp3["total"], (d, cmp3))
        # refusals through the module: the same rules as the Rust reader; the old picture stays
        before = page.evaluate("window.S.frames")
        res = page.evaluate("""(() => { const e = window.S.e, out = {}; const t = (k, s) => { out[k] = e.uiLoad(s, 1).error || 'accepted'; };
          t('newer','iyakki-display-list 3 10 10\\n'); t('unknown','iyakki-display-list 1 10 10\\nrect 0 0 1 1 ffffffff\\nsparkle 1 2\\n'); t('clip','iyakki-display-list 1 10 10\\nclip 0 0 5 5\\n');
          t('endclip','iyakki-display-list 1 10 10\\nendclip\\n'); t('colour','iyakki-display-list 1 10 10\\nrect 0 0 1 1 zzzzzzzz\\n'); t('nan','iyakki-display-list 1 10 10\\nrect 0 NaN 1 1 ffffffff\\n');
          t('role','iyakki-display-list 1 10 10\\nhit 0 0 1 1 1 wizard X\\n'); t('empty',''); t('magic','hello 1 2 3\\n'); t('esc','iyakki-display-list 1 10 10\\ntext 0 0 10 ffffffff regular %zz\\n'); return out; })()""")
        rep.check("a newer list is refused by name", "newer than 1" in res["newer"], res["newer"])
        rep.check("an unknown operation is refused with its line, not skipped", res["unknown"].startswith("line 3:") and "sparkle" in res["unknown"], res["unknown"])
        rep.check("an unclosed clip, a stray endclip, a bad colour, NaN, an unknown role, an empty list, a wrong magic and a bad escape are each refused", all(res[k] != "accepted" for k in ("clip", "endclip", "colour", "nan", "role", "empty", "magic", "esc")), res)
        rep.check("after the refusals the old picture is still on the canvas", page.evaluate("window.S.e.uiFramebuffer().w") == 720 and page.evaluate("window.S.frames") == before)
        # F-SDK-28: the text composed INSIDE the module from glyphs the page handed in equals the page drawing the same runs itself
        res = page.evaluate("""(async () => {
          const text = %s, E = window.S.e.constructor;
          const bytes = await (await fetch('/engine/assets/iyakki-client-engine.wasm')).arrayBuffer();
          const mk = (fb) => { const c = document.createElement('canvas'); c.width = fb.w; c.height = fb.h; c.getContext('2d').putImageData(new ImageData(new Uint8ClampedArray(fb.data), fb.w, fb.h), 0, 0); return c; };
          // the page's own way: the module draws the shapes, the page draws every text run
          const e1 = await E.load(bytes), r1 = e1.uiLoad(text, 1), pageCanvas = mk(e1.uiFramebuffer()), px = pageCanvas.getContext('2d');
          const font = (st, s) => (st === 'bold' ? 'bold ' : '') + s + 'px ' + (st === 'mono' ? 'ui-monospace, monospace' : 'system-ui, sans-serif');
          for (const t of r1.texts) { px.save(); if (t.clip) { px.beginPath(); px.rect(t.clip[0], t.clip[1], t.clip[2] - t.clip[0], t.clip[3] - t.clip[1]); px.clip(); }
            px.font = font(t.style, t.size); px.fillStyle = 'rgba(' + t.color[0] + ',' + t.color[1] + ',' + t.color[2] + ',' + t.color[3] / 255 + ')'; px.fillText(t.text, t.x, t.y + t.size * 0.8); px.restore(); }
          // the new way: the page hands glyphs to the module and the module composes the text
          const e2 = await E.load(bytes); e2.uiLoad(text, 1);
          const done = e2.uiGlyphs(); const modCanvas = mk(e2.uiFramebuffer());
          const A = px.getImageData(0, 0, pageCanvas.width, pageCanvas.height).data, B = modCanvas.getContext('2d').getImageData(0, 0, modCanvas.width, modCanvas.height).data;
          let far = 0, textPx = 0;
          for (let i = 0; i < A.length; i += 4) { const d = Math.max(Math.abs(A[i] - B[i]), Math.abs(A[i+1] - B[i+1]), Math.abs(A[i+2] - B[i+2])); if (d > 90) far++; }
          return { runs: r1.texts.length, left: done ? done.texts.length : -1, far, total: A.length / 4, glyphs: e2.cmd({ c: 'ui_needs' }).have };
        })()""" % json.dumps(frame()))
        rep.check("the module composes the text itself from %d glyphs the page handed in: %d runs, none left to the page (%s)" % (res["glyphs"], res["runs"], res["left"]), res["runs"] > 5 and res["left"] == 0 and res["glyphs"] > 10, res)
        rep.check("that picture equals the page drawing the same runs itself: %s of %s pixels differ by more than 90" % (res["far"], res["total"]), res["far"] * 100 <= res["total"] * 3, res)
        # F-SDK-29: the preview PAGE itself in `?engine=wasm` mode uses the module: shapes, glyph text, ARIA tree and input lines
        pg = chrome.new_page(900, 560, 1)
        try:
            pg.goto(base.rstrip("/") + "/?engine=wasm")
            try:
                pg.wait_for("window.engineReady === true", what="the preview page in engine mode")
            except BaseException:
                try:
                    rep.info("engine-mode page errors", str(pg.errors())[:600])
                    rep.info("engine-mode state", str(pg.evaluate("JSON.stringify(window.engineState || null) + ' ' + document.getElementById('err').textContent"))[:300])
                except BaseException as ee:
                    rep.info("engine-mode debug failed", str(ee)[:200])
                raise
            st = pg.evaluate("({frames: window.engineState.frames, left: window.engineState.left, err: window.engineState.error, w: document.getElementById('c').width, aria: document.querySelectorAll('#aria > div').length})")
            rep.check("the served preview page in engine mode draws the app's list with the module: a %dx%d picture, %d ARIA elements, no text left to the page" % (st["w"], 480, st["aria"]), st["err"] is None and st["left"] == 0 and st["aria"] > 5 and st["w"] > 0, st)
            before = pg.evaluate("window.engineState.frames")
            r = pg.evaluate("(() => { const n = Array.from(document.querySelectorAll('#aria > div')).find(e => e.getAttribute('role') === 'textbox'); const a = n.getBoundingClientRect(); return [a.x + a.width / 2, a.y + a.height / 2]; })()")
            pg.click_at(r[0], r[1])
            for ch in "hi":
                pg.press(ch, "Key" + ch.upper(), ord(ch.upper()), 0, ch)
            pg.press("Enter", "Enter", 13)
            try:
                pg.wait_for("window.engineState.frames > %d" % (before + 2), what="the module's input lines reached the app and a frame came back")
            except BaseException:
                rep.info("engine-mode after typing", str(pg.evaluate("JSON.stringify(window.engineState) + ' ' + document.getElementById('err').textContent + ' ' + Array.from(document.querySelectorAll('#aria > div')).map(e => e.getAttribute('role') + ':' + e.getAttribute('aria-label')).join('|')"))[:500])
                raise
            rep.check("a real click and key presses on the page make the module's input lines and the next frame (%d -> %d frames) is drawn by the module" % (before, pg.evaluate("window.engineState.frames")), pg.evaluate("window.engineState.left") == 0 and pg.evaluate("window.engineState.error") is None)
            rep.check("the engine-mode page has no console errors", pg.errors() == [], pg.errors())
        finally:
            pg.close()
        rep.check("no console errors", page.errors() == [], page.errors())
    finally:
        page.close()
        srv.stop()
        proc.kill()
        proc.wait()
