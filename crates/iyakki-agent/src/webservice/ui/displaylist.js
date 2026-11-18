/*
  SPDX-License-Identifier: Apache-2.0
  Copyright IYAKKI contributors. Licensed under the Apache License, Version 2.0 (see LICENSE at the root of the repository).

  The browser renderer of the UI SDK's display list (docs/development/UI-SDK.md; `iyakki-ui-sdk::list`). It reads the plain-text wire
  format (`iyakki-display-list 1 W H`, one operation per line), draws it locally on a Canvas2D context with the browser's own fonts (so
  text is sharp and scrolling is local), builds the accessibility tree as ARIA elements from the hit regions, and sends the person's
  input back as the SDK's input lines (`wire` in `iyakki-ui-sdk`): `ptr`, `key`, `text`, `action`, `resize`, `tick`, `wheel`.

  The rules of the Rust reader are kept: a list newer than VERSION is refused by name, an unknown or malformed operation is an error that
  names its line (never skipped), a clip that is never closed is an error, limits of 100000 operations and 8 MB. A refused list draws
  nothing and `connect` shows the reason where the picture would be. No outside library; loads under node for the tests (module.exports).
*/
"use strict";
(function (root) {
  const VERSION = 2;
  const FAMILIES = { default: "'DejaVu Sans', 'Bitstream Vera Sans', sans-serif", dejavu: "'DejaVu Sans', 'Bitstream Vera Sans', sans-serif", liberation: "'Liberation Sans', Arimo, Arial, sans-serif" };
  const MONO = { default: "'DejaVu Sans Mono', ui-monospace, monospace", dejavu: "'DejaVu Sans Mono', ui-monospace, monospace", liberation: "'Liberation Mono', Cousine, ui-monospace, monospace" };
  const MAGIC = "iyakki-display-list";
  const MAX_OPS = 100000;
  const MAX_BYTES = 8 * 1024 * 1024;
  const ROLES = ["button", "link", "item", "tab", "field", "text", "image"];
  const ARIA_ROLE = { button: "button", link: "link", item: "listitem", tab: "tab", field: "textbox", text: "text", image: "img" };
  const STYLES = ["regular", "bold", "mono"];

  function num(s, line) {
    const v = Number(s);
    if (s === "" || s === undefined || !Number.isFinite(v)) throw new Error("line " + line + ": bad number '" + s + "'");
    return v;
  }
  function color(s, line) {
    if (typeof s !== "string" || !/^[0-9a-fA-F]{8}$/.test(s)) throw new Error("line " + line + ": bad colour '" + s + "'");
    return [parseInt(s.slice(0, 2), 16), parseInt(s.slice(2, 4), 16), parseInt(s.slice(4, 6), 16), parseInt(s.slice(6, 8), 16)];
  }
  function unesc(s, line) {
    if (s === "~") return "";
    const bytes = [];
    for (let i = 0; i < s.length; i++) {
      const c = s.charCodeAt(i);
      if (c === 37) {
        const h = s.slice(i + 1, i + 3);
        if (h.length !== 2 || !/^[0-9a-fA-F]{2}$/.test(h)) throw new Error("line " + line + ": bad escape");
        bytes.push(parseInt(h, 16));
        i += 2;
      } else if (c < 128) bytes.push(c);
      else throw new Error("line " + line + ": bad escape");
    }
    try { return new TextDecoder("utf-8", { fatal: true }).decode(Uint8Array.from(bytes)); } catch (e) { throw new Error("line " + line + ": text is not UTF-8"); }
  }
  function esc(s) {
    const out = [];
    for (const b of new TextEncoder().encode(s)) {
      if ((b >= 48 && b <= 57) || (b >= 65 && b <= 90) || (b >= 97 && b <= 122) || b === 46 || b === 95 || b === 45) out.push(String.fromCharCode(b));
      else out.push("%" + b.toString(16).toUpperCase().padStart(2, "0"));
    }
    return out.length ? out.join("") : "~";
  }

  // text -> { version, width, height, ops }; throws an Error whose message is the Rust reader's
  function parse(text) {
    if (typeof text !== "string") throw new Error("not a display list");
    if (text.length > MAX_BYTES) throw new Error("the display list is too big");
    const lines = text.split("\n");
    if (lines.length && lines[lines.length - 1] === "") lines.pop();
    if (!lines.length) throw new Error("empty display list");
    const head = lines[0].split(" ");
    if (head.length !== 4 || head[0] !== MAGIC) throw new Error("not a display list");
    if (!/^[0-9]+$/.test(head[1])) throw new Error("bad version");
    const version = Number(head[1]);
    if (version > VERSION) throw new Error("display list version " + version + " is newer than " + VERSION);
    const width = num(head[2], 1), height = num(head[3], 1);
    const ops = [];
    let depth = 0;
    for (let i = 1; i < lines.length; i++) {
      const n = i + 1;
      if (ops.length >= MAX_OPS) throw new Error("line " + n + ": too many operations");
      const line = lines[i];
      if (line === "") continue;
      const f = line.split(" ");
      const g = (k) => num(f[k], n);
      const r = (k) => ({ x: g(k), y: g(k + 1), w: g(k + 2), h: g(k + 3) });
      const key = f[0] + "/" + f.length;
      let op;
      switch (key) {
        case "rect/6": op = { op: "rect", r: r(1), c: color(f[5], n) }; break;
        case "rrect/7": op = { op: "rrect", r: r(1), radius: g(5), c: color(f[6], n) }; break;
        case "outline/8": op = { op: "outline", r: r(1), radius: g(5), width: g(6), c: color(f[7], n) }; break;
        case "line/7": op = { op: "line", p: [g(1), g(2), g(3), g(4)], width: g(5), c: color(f[6], n) }; break;
        case "grad/7": op = { op: "grad", r: r(1), top: color(f[5], n), bottom: color(f[6], n) }; break;
        case "shadow/8": op = { op: "shadow", r: r(1), radius: g(5), spread: g(6), c: color(f[7], n) }; break;
        case "text/7":
          if (!STYLES.includes(f[5])) throw new Error("line " + n + ": unknown text style");
          op = { op: "text", x: g(1), y: g(2), size: g(3), c: color(f[4], n), style: f[5], text: unesc(f[6], n) }; break;
        case "textx/10":
          if (!STYLES.includes(f[5])) throw new Error("line " + n + ": unknown text style");
          if (!(f[6] in FAMILIES)) throw new Error("line " + n + ": unknown font family");
          if (f[8] !== "0" && f[8] !== "1") throw new Error("line " + n + ": bad tabular flag");
          op = { op: "textx", x: g(1), y: g(2), size: g(3), c: color(f[4], n), style: f[5], family: f[6], tracking: Math.min(0.2, Math.max(0, g(7))), tabular: f[8] === "1", text: unesc(f[9], n) }; break;
        case "blur/6": op = { op: "blur", r: r(1), radius: g(5) }; break;
        case "clip/5": depth++; op = { op: "clip", r: r(1) }; break;
        case "endclip/1":
          if (depth === 0) throw new Error("line " + n + ": endclip without clip");
          depth--; op = { op: "endclip" }; break;
        case "image/10": {
          if (f[5] !== "rgba" && f[5] !== "jpeg") throw new Error("line " + n + ": unknown image format");
          if (!/^[0-9]+$/.test(f[6]) || !/^[0-9]+$/.test(f[7]) || !/^[0-9]+$/.test(f[8])) throw new Error("line " + n + ": bad number");
          const pw = Number(f[6]), ph = Number(f[7]);
          if (pw < 1 || ph < 1 || pw > 4096 || ph > 4096 || f[9].length > 1398104 || !/^[A-Za-z0-9+\/]*={0,2}$/.test(f[9]) || f[9].length % 4 !== 0) throw new Error("line " + n + ": bad image data");
          op = { op: "image", r: r(1), format: f[5], pw, ph, seq: Number(f[8]), data: f[9] }; break;
        }
        case "hit/8":
          if (!/^[0-9]+$/.test(f[5])) throw new Error("line " + n + ": bad action");
          if (!ROLES.includes(f[6])) throw new Error("line " + n + ": unknown role");
          op = { op: "hit", r: r(1), action: Number(f[5]), role: f[6], name: unesc(f[7], n) }; break;
        default: throw new Error("line " + n + ": unknown or malformed operation '" + f[0] + "'");
      }
      ops.push(op);
    }
    if (depth !== 0) throw new Error("a clip is never closed");
    return { version, width, height, ops };
  }

  const rgba = (c) => "rgba(" + c[0] + "," + c[1] + "," + c[2] + "," + (c[3] / 255) + ")";
  const contains = (r, x, y) => x >= r.x && y >= r.y && x < r.x + r.w && y < r.y + r.h;
  function font(style, size, family) {
    if (family) {
      if (style === "bold") return "bold " + size + "px " + FAMILIES[family];
      if (style === "mono") return size + "px " + MONO[family];
      return size + "px " + FAMILIES[family];
    }
    if (style === "bold") return "bold " + size + "px system-ui, sans-serif";
    if (style === "mono") return size + "px ui-monospace, monospace";
    return size + "px system-ui, sans-serif";
  }
  // A run with the person's family, letter spacing and tabular figures. Canvas2D has no font-variant-numeric, so tabular figures are
  // drawn one digit at a time at the width of the widest digit; letter spacing is the canvas's own `letterSpacing` where it exists,
  // else one letter at a time (connected scripts are drawn whole, as a browser does).
  function drawRun(ctx, o) {
    ctx.font = font(o.style, o.size, o.family); ctx.fillStyle = rgba(o.c);
    const connected = /[\u0600-\u06FF\u0750-\u077F\uFB50-\uFEFF]/.test(o.text);
    const gap = connected ? 0 : o.tracking * o.size;
    if (!o.tabular && !gap) { ctx.fillText(o.text, o.x, o.y); return; }
    let digit = 0;
    if (o.tabular) for (const d of "0123456789") digit = Math.max(digit, ctx.measureText(d).width);
    let x = o.x;
    for (const ch of o.text) {
      const w = ctx.measureText(ch).width;
      const slot = o.tabular && ch >= "0" && ch <= "9" ? digit : w;
      ctx.fillText(ch, x + (slot - w) / 2, o.y);
      x += slot + gap;
    }
  }
  function rrectPath(ctx, r, radius) {
    const k = Math.max(0, Math.min(radius, r.w / 2, r.h / 2));
    ctx.beginPath();
    ctx.moveTo(r.x + k, r.y);
    ctx.lineTo(r.x + r.w - k, r.y); ctx.arcTo(r.x + r.w, r.y, r.x + r.w, r.y + k, k);
    ctx.lineTo(r.x + r.w, r.y + r.h - k); ctx.arcTo(r.x + r.w, r.y + r.h, r.x + r.w - k, r.y + r.h, k);
    ctx.lineTo(r.x + k, r.y + r.h); ctx.arcTo(r.x, r.y + r.h, r.x, r.y + r.h - k, k);
    ctx.lineTo(r.x, r.y + k); ctx.arcTo(r.x, r.y, r.x + k, r.y, k);
    ctx.closePath();
  }

  // Pictures: decoded once per (data, seq) and kept on the op; a JPEG goes through the browser's own decoder (an image element), RGBA through
  // ImageData. A picture that has not decoded yet draws nothing this frame and asks `onImage` (set on the list) to redraw when it has.
  function b64bytes(s) {
    const bin = root.atob(s), out = new Uint8Array(bin.length);
    for (let i = 0; i < bin.length; i++) out[i] = bin.charCodeAt(i);
    return out;
  }
  function drawImage(ctx, o, list) {
    if (!o.bitmap && !o.pending) {
      try {
        const bytes = b64bytes(o.data);
        if (o.format === "rgba") {
          if (bytes.length !== o.pw * o.ph * 4) return;
          const cv = (root.document || {}).createElement ? root.document.createElement("canvas") : null;
          if (!cv) return;
          cv.width = o.pw; cv.height = o.ph;
          cv.getContext("2d").putImageData(new root.ImageData(new Uint8ClampedArray(bytes.buffer), o.pw, o.ph), 0, 0);
          o.bitmap = cv;
        } else if (root.createImageBitmap) {
          o.pending = true;
          root.createImageBitmap(new root.Blob([bytes], { type: "image/jpeg" })).then((bm) => { o.bitmap = bm; o.pending = false; if (list.onImage) list.onImage(); }, () => { o.pending = false; o.failed = true; });
        }
      } catch (e) { o.failed = true; }
    }
    if (o.bitmap) ctx.drawImage(o.bitmap, o.r.x, o.r.y, o.r.w, o.r.h);
  }

  // Draw a parsed list on a canvas; `scale` is device pixels per logical unit (default devicePixelRatio or 1). The canvas is sized to the list.
  function draw(canvas, list, scale) {
    const k = scale || (root.devicePixelRatio || 1);
    canvas.width = Math.max(1, Math.round(list.width * k));
    canvas.height = Math.max(1, Math.round(list.height * k));
    if (canvas.style) { canvas.style.width = list.width + "px"; canvas.style.height = list.height + "px"; }
    const ctx = canvas.getContext("2d");
    ctx.setTransform(k, 0, 0, k, 0, 0);
    ctx.clearRect(0, 0, list.width, list.height);
    ctx.textBaseline = "top";
    let clips = 0;
    for (const o of list.ops) {
      switch (o.op) {
        case "rect": ctx.fillStyle = rgba(o.c); ctx.fillRect(o.r.x, o.r.y, o.r.w, o.r.h); break;
        case "rrect": ctx.fillStyle = rgba(o.c); rrectPath(ctx, o.r, o.radius); ctx.fill(); break;
        case "outline": {
          const h = o.width / 2;
          ctx.strokeStyle = rgba(o.c); ctx.lineWidth = o.width;
          rrectPath(ctx, { x: o.r.x + h, y: o.r.y + h, w: Math.max(0, o.r.w - o.width), h: Math.max(0, o.r.h - o.width) }, Math.max(0, o.radius - h)); ctx.stroke(); break;
        }
        case "line": ctx.strokeStyle = rgba(o.c); ctx.lineWidth = o.width; ctx.beginPath(); ctx.moveTo(o.p[0], o.p[1]); ctx.lineTo(o.p[2], o.p[3]); ctx.stroke(); break;
        case "grad": {
          const gr = ctx.createLinearGradient(0, o.r.y, 0, o.r.y + o.r.h);
          gr.addColorStop(0, rgba(o.top)); gr.addColorStop(1, rgba(o.bottom));
          ctx.fillStyle = gr; ctx.fillRect(o.r.x, o.r.y, o.r.w, o.r.h); break;
        }
        case "shadow": {
          // a soft glow around the rectangle: blur the colour outwards, draw nothing inside the rectangle itself
          ctx.save();
          ctx.beginPath(); ctx.rect(0, 0, list.width, list.height); rrectPath(ctx, o.r, o.radius); ctx.clip("evenodd");
          ctx.shadowColor = rgba(o.c); ctx.shadowBlur = Math.max(0, o.spread) * k; ctx.fillStyle = rgba(o.c);
          rrectPath(ctx, o.r, o.radius); ctx.fill();
          ctx.restore(); break;
        }
        case "textx": drawRun(ctx, o); break;
        case "blur": {
          // frost what is drawn so far inside the rectangle: the canvas drawn onto itself through a blur filter, clipped to the rectangle
          if (!("filter" in ctx) || o.radius <= 0) break;
          ctx.save();
          ctx.beginPath(); ctx.rect(o.r.x, o.r.y, o.r.w, o.r.h); ctx.clip();
          ctx.filter = "blur(" + o.radius + "px)";
          ctx.drawImage(canvas, o.r.x * k, o.r.y * k, o.r.w * k, o.r.h * k, o.r.x, o.r.y, o.r.w, o.r.h);
          ctx.restore(); break;
        }
        case "text": ctx.font = font(o.style, o.size); ctx.fillStyle = rgba(o.c); ctx.fillText(o.text, o.x, o.y); break;
        case "clip": ctx.save(); clips++; ctx.beginPath(); ctx.rect(o.r.x, o.r.y, o.r.w, o.r.h); ctx.clip(); break;
        case "endclip": if (clips > 0) { ctx.restore(); clips--; } break;
        case "image": drawImage(ctx, o, list); break;
        default: break; // hit regions are not drawn
      }
    }
    while (clips-- > 0) ctx.restore();
  }

  // The accessibility tree in drawing order: the Rust `DisplayList::a11y`.
  function a11y(list) { return list.ops.filter((o) => o.op === "hit").map((o) => ({ name: o.name, role: o.role, rect: o.r, action: o.action })); }

  // The topmost hit at a point, respecting clips (the Rust `hit_test`: the last one drawn wins).
  function hitAt(list, x, y) {
    const clips = []; let found = null;
    for (const o of list.ops) {
      if (o.op === "clip") clips.push(o.r);
      else if (o.op === "endclip") clips.pop();
      else if (o.op === "hit" && contains(o.r, x, y) && clips.every((c) => contains(c, x, y))) found = o;
    }
    return found;
  }

  // Put the accessibility tree into `host` (a positioned element over the canvas): one element per hit region with its name, ARIA role
  // and place. `onAction(action)` runs when the element is used (click, Enter or Space on a focused one).
  function aria(host, list, onAction) {
    const doc = host.ownerDocument;
    while (host.firstChild) host.removeChild(host.firstChild);
    host.setAttribute("role", "group");
    for (const n of a11y(list)) {
      const el = doc.createElement("div");
      el.setAttribute("role", ARIA_ROLE[n.role] || "button");
      el.setAttribute("aria-label", n.name);
      el.setAttribute("data-action", String(n.action));
      el.tabIndex = n.role === "text" || n.role === "image" ? -1 : 0;
      el.style.cssText = "position:absolute;left:" + n.rect.x + "px;top:" + n.rect.y + "px;width:" + n.rect.w + "px;height:" + n.rect.h + "px;background:transparent;";
      el.addEventListener("click", (ev) => { ev.stopPropagation(); onAction(n.action); });
      el.addEventListener("keydown", (ev) => { if (ev.key === "Enter" || ev.key === " ") { ev.preventDefault(); ev.stopPropagation(); onAction(n.action); } });
      host.appendChild(el);
    }
    return host.children.length;
  }

  // ---- the input lines (the Rust `wire` module reads exactly these)
  const KEYS = { Enter: "enter", Backspace: "backspace", Delete: "delete", Escape: "esc", Tab: "tab", ArrowUp: "up", ArrowDown: "down", ArrowLeft: "left", ArrowRight: "right" };
  const input = {
    pointer: (id, phase, x, y, t) => "ptr " + (id >>> 0) + " " + phase + " " + (Math.round(x * 100) / 100) + " " + (Math.round(y * 100) / 100) + " " + Math.max(0, Math.round(t)),
    key: (k) => KEYS[k] ? "key " + KEYS[k] : (Array.from(k).length === 1 ? "key char " + esc(k) : null),
    text: (s) => "text " + esc(s),
    action: (n) => "action " + (n >>> 0),
    resize: (w, h) => "resize " + Math.round(w) + " " + Math.round(h),
    tick: (t) => "tick " + Math.max(0, Math.round(t)),
    wheel: (n) => "wheel " + Math.round(n),
  };

  // Connect a canvas to a frame source and an input sink. `io.frame()` returns a promise of the list text; `io.send(lines)` posts input
  // lines and returns a promise of the new list text. Returns { redraw, last, error }.
  function connect(opts) {
    const canvas = opts.canvas, host = opts.ariaHost, io = opts.io;
    const state = { list: null, error: null, frames: 0, t0: Date.now() };
    const show = (text) => {
      try {
        const list = parse(text);
        state.list = list; state.error = null; state.frames++;
        draw(canvas, list, opts.scale);
        if (host) aria(host, list, (a) => send([input.action(a)]));
      } catch (e) {
        state.error = String(e.message || e);
        if (opts.onError) opts.onError(state.error);
      }
    };
    const send = (lines) => io.send(lines.filter(Boolean)).then(show);
    const at = (ev) => { const b = canvas.getBoundingClientRect(); return [ev.clientX - b.left, ev.clientY - b.top]; };
    const now = () => Date.now() - state.t0;
    canvas.addEventListener("pointerdown", (ev) => { const [x, y] = at(ev); send([input.pointer(ev.pointerId === 1 ? 0 : ev.pointerId, "down", x, y, now())]); });
    canvas.addEventListener("pointermove", (ev) => { if (ev.buttons) { const [x, y] = at(ev); send([input.pointer(ev.pointerId === 1 ? 0 : ev.pointerId, "move", x, y, now())]); } });
    canvas.addEventListener("pointerup", (ev) => { const [x, y] = at(ev); send([input.pointer(ev.pointerId === 1 ? 0 : ev.pointerId, "up", x, y, now())]); });
    canvas.addEventListener("pointercancel", (ev) => { const [x, y] = at(ev); send([input.pointer(ev.pointerId === 1 ? 0 : ev.pointerId, "cancel", x, y, now())]); });
    canvas.addEventListener("wheel", (ev) => { ev.preventDefault(); send([input.wheel(ev.deltaY > 0 ? -1 : 1)]); }, { passive: false });
    const target = opts.keyTarget || canvas;
    target.addEventListener("keydown", (ev) => {
      if (ev.ctrlKey || ev.metaKey || ev.altKey) return;
      const l = input.key(ev.key);
      if (l) { ev.preventDefault(); send([l]); }
    });
    target.addEventListener("paste", (ev) => { const t = ev.clipboardData && ev.clipboardData.getData("text"); if (t) { ev.preventDefault(); send([input.text(t)]); } });
    return { send, show, state, start: () => io.frame().then(show) };
  }

  const api = { VERSION, parse, draw, a11y, hitAt, aria, input, connect, esc, unesc };
  if (typeof module !== "undefined" && module.exports) module.exports = api;
  root.IyakkiDisplayList = api;
})(typeof window !== "undefined" ? window : globalThis);
