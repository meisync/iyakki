# SPDX-License-Identifier: Apache-2.0
# Copyright IYAKKI contributors. Licensed under the Apache License, Version 2.0 (see LICENSE at the root of the repository).
"""F-SDK-20 in real Chrome: the browser renderer of the UI SDK's display list (`ui/displaylist.js`) against the real preview server
(`iyakki-ui-preview`, the SAMPLE app of `iyakki-app-sample` driven by the SDK host). It checks what the renderer draws (canvas pixels read
back against the list), the accessibility tree (ARIA elements against the hit regions), real mouse and keyboard input that goes back to the
app and comes back as a new frame, and the refusal of a newer or damaged list."""
import json
import subprocess
import time
import urllib.request

import cdp

PROBE = r"""
window.__probe = function () {
  const D = window.IyakkiDisplayList, st = window.app.state, list = st.list, c = document.getElementById('c');
  const ctx = c.getContext('2d'), k = c.width / list.width;
  const px = (x, y) => Array.from(ctx.getImageData(Math.round(x * k), Math.round(y * k), 1, 1).data);
  const covered = (i, x, y) => {
    let clips = [];
    for (let j = i + 1; j < list.ops.length; j++) {
      const o = list.ops[j];
      if (o.op === 'clip') clips.push(o.r); else if (o.op === 'endclip') clips.pop();
      else if (['rect', 'rrect', 'grad', 'outline', 'shadow'].includes(o.op) && x >= o.r.x - 2 && y >= o.r.y - 2 && x < o.r.x + o.r.w + 2 && y < o.r.y + o.r.h + 2) return true;
      else if (o.op === 'text' && x >= o.x - 2 && x < o.x + o.size * o.text.length && y >= o.y - 2 && y < o.y + o.size * 1.5) return true;
    }
    return false;
  };
  let tested = 0, bad = [];
  list.ops.forEach((o, i) => {
    if ((o.op !== 'rect' && o.op !== 'rrect') || o.c[3] !== 255 || o.r.w < 6 || o.r.h < 6) return;
    const r = o.op === 'rrect' ? Math.min(o.radius, o.r.w / 2, o.r.h / 2) : 0;
    const x = o.r.x + r + 1.5, y = o.r.y + 2.5;
    if (covered(i, x, y)) return;
    // inside every clip that is open at this op
    tested++;
    const got = px(x, y);
    if (Math.max(Math.abs(got[0] - o.c[0]), Math.abs(got[1] - o.c[1]), Math.abs(got[2] - o.c[2])) > 1 || got[3] !== 255) bad.push({ i, op: o.op, want: o.c, got, x, y });
  });
  let texts = 0, textBad = [];
  list.ops.forEach((o, i) => {
    if (o.op !== 'text' || !o.text.trim()) return;
    ctx.font = (o.style === 'bold' ? 'bold ' : '') + o.size + 'px ' + (o.style === 'mono' ? 'ui-monospace, monospace' : 'system-ui, sans-serif');
    const w = Math.min(Math.ceil(ctx.measureText(o.text).width), list.width - o.x);
    if (w < 1) return;
    texts++;
    const d = ctx.getImageData(Math.round(o.x * k), Math.round(o.y * k), Math.max(1, Math.round(w * k)), Math.max(1, Math.round(o.size * k))).data;
    let near = 0;
    for (let p = 0; p < d.length; p += 4) if (Math.abs(d[p] - o.c[0]) < 40 && Math.abs(d[p + 1] - o.c[1]) < 40 && Math.abs(d[p + 2] - o.c[2]) < 40) near++;
    if (near < 3) textBad.push({ i, text: o.text, near });
  });
  const els = Array.from(document.querySelectorAll('#aria > div')).map((e) => ({ role: e.getAttribute('role'), name: e.getAttribute('aria-label'), action: e.getAttribute('data-action'), l: parseFloat(e.style.left), t: parseFloat(e.style.top), w: parseFloat(e.style.width), h: parseFloat(e.style.height) }));
  return { w: c.width, h: c.height, lw: list.width, lh: list.height, k, tested, bad, texts, textBad, els, a11y: D.a11y(list), frames: st.frames, error: st.error, names: list.ops.filter((o) => o.op === 'text').map((o) => o.text) };
};
"""

ROLE = {"button": "button", "link": "link", "item": "listitem", "tab": "tab", "field": "textbox", "text": "text", "image": "img"}


def start_preview(binary, size):
    p = subprocess.Popen([binary, "--size", size], stdout=subprocess.PIPE, stderr=subprocess.STDOUT, text=True)
    line = p.stdout.readline().strip()
    if not line.startswith("http://127.0.0.1:"):
        p.kill()
        raise RuntimeError("the preview did not start: " + line)
    return p, line


def suite(rep, chrome, preview_bin):
    proc, base = start_preview(preview_bin, "800x480")
    try:
        page = chrome.new_page(900, 560, 1)
        page.goto(base)
        page.wait_for("window.app && window.app.state.frames > 0", what="the first frame")
        page.evaluate(PROBE + "; 1")
        p = page.evaluate("window.__probe()")
        rep.check("the first frame is drawn and the canvas is the size of the list (%sx%s)" % (p["lw"], p["lh"]), p["error"] is None and p["w"] == p["lw"] * p["k"] and p["h"] == p["lh"] * p["k"] and p["frames"] == 1, p)
        rep.check("canvas pixels equal the list's colour at %d opaque rectangles that nothing covers (read back, tolerance 1)" % p["tested"], p["tested"] >= 5 and not p["bad"], p["bad"][:3])
        rep.check("every one of %d texts has pixels of its colour on the canvas" % p["texts"], p["texts"] >= 8 and not p["textBad"], p["textBad"][:3])
        want = [(n["name"], ROLE[n["role"]], str(n["action"])) for n in p["a11y"]]
        got = [(e["name"], e["role"], e["action"]) for e in p["els"]]
        rep.check("the accessibility tree is %d ARIA elements with the name, role and action of every hit region" % len(want), want == got and len(want) > 5, (want[:3], got[:3]))
        place = all(abs(e["l"] - n["rect"]["x"]) < 0.01 and abs(e["t"] - n["rect"]["y"]) < 0.01 and abs(e["w"] - n["rect"]["w"]) < 0.01 and abs(e["h"] - n["rect"]["h"]) < 0.01 for e, n in zip(p["els"], p["a11y"]))
        rep.check("each ARIA element sits where its hit region is", place)
        page.screenshot(rep.shot("uisdk-first-frame.png"))

        # a real mouse: click the field, type with real key events, press Enter -> the app adds the note and a new frame comes back
        def rect_of(name):
            return page.evaluate("(()=>{const e=Array.from(document.querySelectorAll('#aria > div')).find(e=>e.getAttribute('aria-label')==%s);const b=document.getElementById('c').getBoundingClientRect();return e?[b.left+parseFloat(e.style.left)+parseFloat(e.style.width)/2,b.top+parseFloat(e.style.top)+parseFloat(e.style.height)/2]:null})()" % json.dumps(name))

        frames0 = page.evaluate("window.app.state.frames")
        # the aria element sits over the canvas, so a click lands on it; the canvas pointer events are what a mouse on the picture sends: use the canvas directly
        page.evaluate("document.getElementById('aria').style.pointerEvents='none'")
        x, y = rect_of("New note")
        page.click_at(x, y)
        page.wait_for("window.app.state.frames > %d" % frames0, what="a frame after the click")
        for ch in "buy milk":
            page.press(ch, "Key" + ch.upper() if ch.isalpha() else "Space", ord(ch.upper()), 0, ch)
        page.press("Enter", "Enter", 13)
        page.wait_for("window.app.state.list.ops.some(o=>o.op==='text'&&o.text==='[ ] buy milk')", what="the typed note in the list")
        p2 = page.evaluate("window.__probe()")
        rep.check("a real click and real key presses went to the app and the new frame shows the note (frames %d -> %d)" % (frames0, p2["frames"]), "[ ] buy milk" in p2["names"] and p2["frames"] > frames0, p2["names"])
        rep.check("after the new frame the pixels still equal the list (%d rectangles) and the tree is rebuilt" % p2["tested"], not p2["bad"] and len(p2["els"]) == len(p2["a11y"]), p2["bad"][:3])
        page.screenshot(rep.shot("uisdk-after-typing.png"))
        # a click on the row checks it off
        x, y = rect_of("[ ] buy milk")
        page.click_at(x, y)
        page.wait_for("window.app.state.list.ops.some(o=>o.op==='text'&&o.text==='[x] buy milk')", what="the checked row")
        rep.check("a click on the row toggles it ([x] buy milk)", True)

        # a screen reader's way: the ARIA element is focused and Enter is pressed (no pointer at all)
        page.evaluate("document.getElementById('aria').style.pointerEvents='auto'")
        page.evaluate("Array.from(document.querySelectorAll('#aria > div')).find(e=>(e.getAttribute('aria-label')||'').startsWith('Clear done')).focus()")
        page.press("Enter", "Enter", 13)
        page.wait_for("!window.app.state.list.ops.some(o=>o.op==='text'&&o.text.includes('buy milk'))", what="the cleared note")
        rep.check("Enter on a focused ARIA element runs its action (Clear done removed the note)", True)
        page.screenshot(rep.shot("uisdk-after-clear.png"))

        # the layout follows the pane: the size goes back as a line and the phone layout comes back
        page.evaluate("window.app.send([IyakkiDisplayList.input.resize(360, 740)])")
        page.wait_for("window.app.state.list.width === 360", what="the phone-sized frame")
        p3 = page.evaluate("window.__probe()")
        rep.check("a resize line gives a 360x740 frame in the compact layout, drawn at that size", p3["lw"] == 360 and p3["lh"] == 740 and p3["w"] == 360 * p3["k"] and any("compact pane" in t for t in p3["names"]) and not p3["bad"], p3["names"][:4])
        page.screenshot(rep.shot("uisdk-phone.png"))
        page.evaluate("window.app.send([IyakkiDisplayList.input.resize(1280, 720)])")
        page.wait_for("window.app.state.list.width === 1280", what="the wide frame")
        p4 = page.evaluate("window.__probe()")
        rep.check("at 1280x720 the wide layout has the side panel and the pixels equal the list", "This screen" in p4["names"] and not p4["bad"] and p4["tested"] > 5, p4["bad"][:3])
        page.screenshot(rep.shot("uisdk-wide.png"))

        # the high-density case: the canvas has 2 device pixels per unit and is still sharp and the same size in CSS units
        page.set_viewport(1300, 760, 2)
        text = urllib.request.urlopen(base.rstrip("/") + "/frame").read().decode()
        r = page.evaluate("(()=>{const D=window.IyakkiDisplayList;const c=document.createElement('canvas');D.draw(c,D.parse(%s),2);return [c.width,c.height,c.style.width,c.style.height]})()" % json.dumps(text))
        rep.check("at 2 device pixels per unit the canvas has twice the pixels and the same CSS size (%s)" % r, r[0] == 2560 and r[1] == 1440 and r[2] == "1280px" and r[3] == "720px", r)

        # refusals, in the same page: a newer list, an unknown operation, a clip never closed, a hostile string
        res = page.evaluate("""(()=>{const D=window.IyakkiDisplayList, out={};
          const t=(k,s)=>{try{D.parse(s);out[k]='accepted'}catch(e){out[k]=e.message}};
          t('newer','iyakki-display-list 3 10 10\\n'); t('unknown','iyakki-display-list 1 10 10\\nrect 0 0 1 1 ffffffff\\nsparkle 1 2\\n');
          t('clip','iyakki-display-list 1 10 10\\nclip 0 0 5 5\\n'); t('endclip','iyakki-display-list 1 10 10\\nendclip\\n');
          t('colour','iyakki-display-list 1 10 10\\nrect 0 0 1 1 zzzzzzzz\\n'); t('nan','iyakki-display-list 1 10 10\\nrect 0 NaN 1 1 ffffffff\\n');
          t('role','iyakki-display-list 1 10 10\\nhit 0 0 1 1 1 wizard X\\n'); t('empty',''); t('magic','hello 1 2 3\\n');
          t('esc','iyakki-display-list 1 10 10\\ntext 0 0 10 ffffffff regular %zz\\n');
          const ok=D.parse('iyakki-display-list 1 10 10\\ntext 0 0 10 ffffffff bold %3Cscript%3E%0A%E2%82%AC\\n'); out.text=ok.ops[0].text; return out;})()""")
        rep.check("a newer list is refused by name", "newer than 1" in res["newer"], res["newer"])
        rep.check("an unknown operation is refused with its line, not skipped", res["unknown"].startswith("line 3:") and "sparkle" in res["unknown"], res["unknown"])
        rep.check("an unclosed clip, a stray endclip, a bad colour, NaN, an unknown role, an empty list, a wrong magic and a bad escape are each refused", all(res[k] != "accepted" for k in ("clip", "endclip", "colour", "nan", "role", "empty", "magic", "esc")), res)
        rep.check("awkward text comes through as text, never markup (%r)" % res["text"], res["text"] == "<script>\n€")
        # a refused list on a live page draws nothing and says why
        before = page.evaluate("window.app.state.frames")
        page.evaluate("window.app.show('iyakki-display-list 9 10 10\\n')")
        st = page.evaluate("({e: window.app.state.error, f: window.app.state.frames, msg: document.getElementById('err').textContent, w: document.getElementById('c').width})")
        rep.check("a newer list that arrives on the live page is refused: the old picture stays, the reason is shown", "newer than 1" in (st["e"] or "") and st["f"] == before and "refused" in st["msg"] and st["w"] > 0, st)
        rep.check("no console errors", page.errors() == [], page.errors())
    finally:
        proc.kill()
        proc.wait()
