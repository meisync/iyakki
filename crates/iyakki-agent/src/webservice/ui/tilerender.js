/*
  SPDX-License-Identifier: Apache-2.0
  Copyright IYAKKI contributors. Licensed under the Apache License, Version 2.0 (see LICENSE at the root of the repository).

  The tile renderer of the web UI: draws a tile's display list (the JSON form the service sends, `iyakki_tile_host::export::to_json`)
  on a canvas. It is the same integer algorithm as the Rust reference renderer (`iyakki-tile-host/src/render.rs`: rectangles, rounded
  rectangles, outlines, lines, text in the built-in 5x7 font, clips, images as grey boxes), so the picture is byte for byte what the
  device draws for the same list; a test compares the two. Text advances are the ABI's (6/7 of the size), never the browser's font.
  Production font tables replace the 5x7 table later; the contract (TILE-ABI.md, section 8) does not change.
  Hit regions are not drawn; `hitAt` tells the page where a click would land (for the pointer cursor). No outside library.
  Loads under node for the tests (module.exports).
*/
"use strict";
(function (root) {
  // Columns of the classic 5x7 font for 0x20..0x5A; bit 0 is the top row (copied from the Rust table; a test keeps them equal).
  const FONT = [
    [0x00, 0x00, 0x00, 0x00, 0x00], [0x00, 0x00, 0x5F, 0x00, 0x00], [0x00, 0x07, 0x00, 0x07, 0x00], [0x14, 0x7F, 0x14, 0x7F, 0x14],
    [0x24, 0x2A, 0x7F, 0x2A, 0x12], [0x23, 0x13, 0x08, 0x64, 0x62], [0x36, 0x49, 0x55, 0x22, 0x50], [0x00, 0x05, 0x03, 0x00, 0x00],
    [0x00, 0x1C, 0x22, 0x41, 0x00], [0x00, 0x41, 0x22, 0x1C, 0x00], [0x14, 0x08, 0x3E, 0x08, 0x14], [0x08, 0x08, 0x3E, 0x08, 0x08],
    [0x00, 0x50, 0x30, 0x00, 0x00], [0x08, 0x08, 0x08, 0x08, 0x08], [0x00, 0x60, 0x60, 0x00, 0x00], [0x20, 0x10, 0x08, 0x04, 0x02],
    [0x3E, 0x51, 0x49, 0x45, 0x3E], [0x00, 0x42, 0x7F, 0x40, 0x00], [0x42, 0x61, 0x51, 0x49, 0x46], [0x21, 0x41, 0x45, 0x4B, 0x31],
    [0x18, 0x14, 0x12, 0x7F, 0x10], [0x27, 0x45, 0x45, 0x45, 0x39], [0x3C, 0x4A, 0x49, 0x49, 0x30], [0x01, 0x71, 0x09, 0x05, 0x03],
    [0x36, 0x49, 0x49, 0x49, 0x36], [0x06, 0x49, 0x49, 0x29, 0x1E], [0x00, 0x36, 0x36, 0x00, 0x00], [0x00, 0x56, 0x36, 0x00, 0x00],
    [0x08, 0x14, 0x22, 0x41, 0x00], [0x14, 0x14, 0x14, 0x14, 0x14], [0x00, 0x41, 0x22, 0x14, 0x08], [0x02, 0x01, 0x51, 0x09, 0x06],
    [0x32, 0x49, 0x79, 0x41, 0x3E], [0x7E, 0x11, 0x11, 0x11, 0x7E], [0x7F, 0x49, 0x49, 0x49, 0x36], [0x3E, 0x41, 0x41, 0x41, 0x22],
    [0x7F, 0x41, 0x41, 0x22, 0x1C], [0x7F, 0x49, 0x49, 0x49, 0x41], [0x7F, 0x09, 0x09, 0x09, 0x01], [0x3E, 0x41, 0x49, 0x49, 0x7A],
    [0x7F, 0x08, 0x08, 0x08, 0x7F], [0x00, 0x41, 0x7F, 0x41, 0x00], [0x20, 0x40, 0x41, 0x3F, 0x01], [0x7F, 0x08, 0x14, 0x22, 0x41],
    [0x7F, 0x40, 0x40, 0x40, 0x40], [0x7F, 0x02, 0x0C, 0x02, 0x7F], [0x7F, 0x04, 0x08, 0x10, 0x7F], [0x3E, 0x41, 0x41, 0x41, 0x3E],
    [0x7F, 0x09, 0x09, 0x09, 0x06], [0x3E, 0x41, 0x51, 0x21, 0x5E], [0x7F, 0x09, 0x19, 0x29, 0x46], [0x46, 0x49, 0x49, 0x49, 0x31],
    [0x01, 0x01, 0x7F, 0x01, 0x01], [0x3F, 0x40, 0x40, 0x40, 0x3F], [0x1F, 0x20, 0x40, 0x20, 0x1F], [0x3F, 0x40, 0x38, 0x40, 0x3F],
    [0x63, 0x14, 0x08, 0x14, 0x63], [0x07, 0x08, 0x70, 0x08, 0x07], [0x61, 0x51, 0x49, 0x45, 0x43],
  ];

  function glyph(cp) {
    const u = cp >= 97 && cp <= 122 ? cp - 32 : cp;
    const i = u - 0x20;
    return FONT[i >= 0 && i < FONT.length ? i : 0x3f - 0x20];
  }

  // "#rrggbb" or "#rrggbbaa" -> [r, g, b, a]; anything else is null
  function parseColor(s) {
    if (typeof s !== "string" || !/^#[0-9a-fA-F]{6}([0-9a-fA-F]{2})?$/.test(s)) return null;
    const n = (i) => parseInt(s.slice(i, i + 2), 16);
    return [n(1), n(3), n(5), s.length === 9 ? n(7) : 255];
  }

  function intersect(a, b) {
    const x0 = Math.max(a.x, b.x), y0 = Math.max(a.y, b.y);
    const x1 = Math.min(a.x + a.w, b.x + b.w), y1 = Math.min(a.y + a.h, b.y + b.h);
    return x1 <= x0 || y1 <= y0 ? { x: x0, y: y0, w: 0, h: 0 } : { x: x0, y: y0, w: x1 - x0, h: y1 - y0 };
  }
  const contains = (r, x, y) => x >= r.x && y >= r.y && x < r.x + r.w && y < r.y + r.h;

  function newImage(w, h, bg) {
    const rgba = new Uint8ClampedArray(w * h * 4);
    for (let i = 0; i < w * h; i++) { rgba[i * 4] = bg[0]; rgba[i * 4 + 1] = bg[1]; rgba[i * 4 + 2] = bg[2]; rgba[i * 4 + 3] = bg[3]; }
    return { w, h, rgba };
  }

  function blend(img, x, y, c) {
    const i = (y * img.w + x) * 4, a = c[3];
    for (let k = 0; k < 3; k++) img.rgba[i + k] = Math.floor((c[k] * a + img.rgba[i + k] * (255 - a) + 127) / 255);
    img.rgba[i + 3] = 255;
  }

  // Draw the list into an RGBA image of w x h on the background colour bg ([r, g, b, a]). A bad op is skipped, never thrown on.
  function render(ops, w, h, bg) {
    w = Math.max(0, w | 0); h = Math.max(0, h | 0);
    const img = newImage(w, h, bg || [255, 255, 255, 255]);
    const surface = { x: 0, y: 0, w, h };
    const clips = [surface];
    const put = (clip, x, y, c) => { if (contains(clip, x, y)) blend(img, x, y, c); };
    const fill = (clip, r, c) => {
      const v = intersect(r, clip);
      for (let y = v.y; y < v.y + v.h; y++) for (let x = v.x; x < v.x + v.w; x++) blend(img, x, y, c);
    };
    for (const op of Array.isArray(ops) ? ops : []) {
      if (!op || typeof op !== "object") continue;
      const clip = clips[clips.length - 1];
      const col = parseColor(op.color);
      const r = { x: op.x | 0, y: op.y | 0, w: op.w | 0, h: op.h | 0 };
      switch (op.op) {
        case "rect": if (col) fill(clip, r, col); break;
        case "round_rect": {
          if (!col) break;
          const rad = Math.max(0, Math.min(op.radius | 0, Math.trunc(r.w / 2), Math.trunc(r.h / 2)));
          for (let ly = 0; ly < r.h; ly++) {
            for (let lx = 0; lx < r.w; lx++) {
              const cx = lx < rad ? 2 * rad - (2 * lx + 1) : lx >= r.w - rad ? (2 * lx + 1) - 2 * (r.w - rad) : 0;
              const cy = ly < rad ? 2 * rad - (2 * ly + 1) : ly >= r.h - rad ? (2 * ly + 1) - 2 * (r.h - rad) : 0;
              if (cx * cx + cy * cy <= 4 * rad * rad) put(clip, r.x + lx, r.y + ly, col);
            }
          }
          break;
        }
        case "outline": {
          if (!col) break;
          const t = Math.max(0, Math.min(op.thickness | 0, Math.min(r.w, r.h)));
          fill(clip, { x: r.x, y: r.y, w: r.w, h: t }, col);
          fill(clip, { x: r.x, y: r.y + r.h - t, w: r.w, h: t }, col);
          fill(clip, { x: r.x, y: r.y + t, w: t, h: r.h - 2 * t }, col);
          fill(clip, { x: r.x + r.w - t, y: r.y + t, w: t, h: r.h - 2 * t }, col);
          break;
        }
        case "line": {
          if (!col) break;
          const t = Math.max(op.thickness | 0, 1);
          const dx = (op.x2 | 0) - (op.x1 | 0), dy = (op.y2 | 0) - (op.y1 | 0);
          const n = Math.max(Math.abs(dx), Math.abs(dy), 1);
          for (let i = 0; i <= n; i++) {
            const x = (op.x1 | 0) + Math.trunc((dx * i) / n), y = (op.y1 | 0) + Math.trunc((dy * i) / n);
            fill(clip, { x: x - Math.trunc(t / 2), y: y - Math.trunc(t / 2), w: t, h: t }, col);
          }
          break;
        }
        case "text": {
          if (!col || typeof op.text !== "string") break;
          const size = op.size | 0;
          if (size <= 0 || size > 512) break;
          const adv = Math.max(1, Math.trunc((size * 6) / 7));
          let n = 0;
          for (const ch of op.text) {
            const g = glyph(ch.codePointAt(0));
            const gx = (op.x | 0) + n * adv;
            for (let dy = 0; dy < size; dy++) {
              const row = Math.trunc((dy * 7) / size);
              for (let dx = 0; dx < adv; dx++) {
                const colm = Math.trunc((dx * 6) / adv);
                if (colm < 5 && (g[colm] >> row) & 1) put(clip, gx + dx, (op.y | 0) + dy, col);
              }
            }
            n++;
          }
          break;
        }
        case "image": fill(clip, r, [128, 128, 128, 255]); break;
        case "push_clip": clips.push(intersect(r, clip)); break;
        case "pop_clip": if (clips.length > 1) clips.pop(); break;
        default: break; // "hit" and anything unknown draw nothing
      }
    }
    return img;
  }

  // Nearest-neighbour enlargement by a whole factor: the picture stays crisp on a high-density screen.
  function upscale(img, s) {
    if (s <= 1) return img;
    const out = new Uint8ClampedArray(img.w * s * img.h * s * 4);
    const W = img.w * s;
    for (let y = 0; y < img.h; y++) {
      for (let x = 0; x < img.w; x++) {
        const si = (y * img.w + x) * 4;
        for (let j = 0; j < s; j++) {
          for (let i = 0; i < s; i++) {
            const di = ((y * s + j) * W + x * s + i) * 4;
            out[di] = img.rgba[si]; out[di + 1] = img.rgba[si + 1]; out[di + 2] = img.rgba[si + 2]; out[di + 3] = img.rgba[si + 3];
          }
        }
      }
    }
    return { w: W, h: img.h * s, rgba: out };
  }

  // The id of the topmost hit region at a point (draw order, clips ignored, like the ABI's hit_test), or null.
  function hitAt(ops, x, y) {
    for (let i = (ops || []).length - 1; i >= 0; i--) {
      const o = ops[i];
      if (o && o.op === "hit" && contains({ x: o.x | 0, y: o.y | 0, w: o.w | 0, h: o.h | 0 }, x, y)) return o.id;
    }
    return null;
  }

  // Draw a frame ({width, height, ops}) on a canvas. `scale` is the whole-number device pixel ratio (1, 2, 3). The canvas keeps its
  // logical size in CSS pixels. `bg` is the theme background. Returns the logical size drawn.
  function draw(canvas, frame, scale, bg) {
    const w = Math.max(1, Math.min(4096, frame.width | 0)), h = Math.max(1, Math.min(4096, frame.height | 0));
    const s = Math.max(1, Math.min(4, Math.round(scale || 1)));
    const img = upscale(render(frame.ops, w, h, bg), s);
    canvas.width = img.w;
    canvas.height = img.h;
    canvas.setAttribute("data-logical", w + "x" + h);
    // the canvas shows at its logical size; the extra pixels are the sharper picture on a dense screen
    if (canvas.style) { canvas.style.width = w + "px"; canvas.style.height = h + "px"; }
    const ctx = canvas.getContext("2d");
    ctx.putImageData(new root.ImageData(img.rgba, img.w, img.h), 0, 0);
    return { w, h };
  }

  const exported = { FONT, glyph, parseColor, render, upscale, hitAt, draw };
  if (typeof module !== "undefined" && module.exports) module.exports = exported;
  else root.IyakkiTileRender = exported;
})(typeof window !== "undefined" ? window : globalThis);
