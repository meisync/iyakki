// The picture decoder of protocol version 2 (docs/REMOTE-PROTOCOL.md). Served at /decoder.js and used by the page on its main
// thread, or inside a Worker drawing on an OffscreenCanvas when the browser has both. It also loads under node (for the
// crate's tests), where a stand-in canvas can be passed in.
"use strict";
(function (root) {
  const KIND_FRAME = 2, HEADER = 18, MAX_SIDE = 4096, MAX_BODY = 48 * 1024 * 1024;
  const T_SOLID = 0, T_RAW = 1, T_PALETTE = 2, T_PNG = 3, T_YC = 4;

  function canInflate() {
    try { new DecompressionStream("deflate-raw"); return true; } catch (e) { return false; }
  }

  async function inflateRaw(bytes, expected) {
    const stream = new Blob([bytes]).stream().pipeThrough(new DecompressionStream("deflate-raw"));
    const out = new Uint8Array(await new Response(stream).arrayBuffer());
    if (out.length !== expected) throw new Error("body length");
    return out;
  }

  function paeth(a, b, c) {
    const p = a + b - c, pa = Math.abs(p - a), pb = Math.abs(p - b), pc = Math.abs(p - c);
    return pa <= pb && pa <= pc ? a : pb <= pc ? b : c;
  }

  // rows of [filter byte, row bytes] -> plain rows; `pos` is where the data starts in `src`
  function unfilter(src, pos, rowLen, rows, bpp) {
    if (pos + rows * (rowLen + 1) > src.length) throw new Error("short tile");
    const out = new Uint8Array(rows * rowLen);
    for (let y = 0; y < rows; y++) {
      const f = src[pos + y * (rowLen + 1)], s = pos + y * (rowLen + 1) + 1, o = y * rowLen;
      if (f > 3) throw new Error("bad filter");
      for (let i = 0; i < rowLen; i++) {
        const a = i >= bpp ? out[o + i - bpp] : 0, b = y > 0 ? out[o - rowLen + i] : 0, c = (y > 0 && i >= bpp) ? out[o - rowLen + i - bpp] : 0;
        const p = f === 0 ? 0 : f === 1 ? a : f === 2 ? b : paeth(a, b, c);
        out[o + i] = (src[s + i] + p) & 255;
      }
    }
    return out;
  }

  const bitsFor = (n) => (n <= 2 ? 1 : n <= 4 ? 2 : n <= 16 ? 4 : 8);
  const clamp8 = (v) => (v < 0 ? 0 : v > 255 ? 255 : Math.round(v));

  class Painter {
    // canvas: the page's canvas or an OffscreenCanvas; makeCanvas(w, h): makes the half-size back canvas
    constructor(canvas, makeCanvas) {
      this.cv = canvas;
      this.ctx = canvas.getContext("2d", { alpha: false });
      this.makeCanvas = makeCanvas || ((w, h) => new OffscreenCanvas(w, h));
      this.back = null; this.bctx = null; this.bw = 0; this.bh = 0;
    }

    resize(w, h) {
      if (this.cv.width !== w || this.cv.height !== h) { this.cv.width = w; this.cv.height = h; }
    }

    // buf: ArrayBuffer or Uint8Array holding one frame message. Resolves with {seq, tiles, bytes}.
    async paint(buf) {
      const m = buf instanceof Uint8Array ? buf : new Uint8Array(buf);
      if (m.length < HEADER || m[0] !== KIND_FRAME) throw new Error("not a frame");
      const dv = new DataView(m.buffer, m.byteOffset, m.byteLength);
      const flags = m[1], seq = dv.getUint32(2, true), screenW = dv.getUint16(6, true), screenH = dv.getUint16(8, true);
      const scale = m[10], ntiles = dv.getUint16(12, true), bodyLen = dv.getUint32(14, true);
      if (scale < 1 || scale > 4 || !screenW || !screenH || bodyLen > MAX_BODY) throw new Error("bad header");
      const sw = Math.ceil(screenW / scale), sh = Math.ceil(screenH / scale);
      if (sw > MAX_SIDE || sh > MAX_SIDE) throw new Error("too big");
      const body = (flags & 1) ? await inflateRaw(m.subarray(HEADER), bodyLen) : m.subarray(HEADER);
      if (body.length !== bodyLen) throw new Error("body length");
      let target = this.ctx;
      if (scale > 1) {
        if (!this.back || this.bw !== sw || this.bh !== sh) { this.back = this.makeCanvas(sw, sh); this.bctx = this.back.getContext("2d", { alpha: false }); this.bw = sw; this.bh = sh; }
        target = this.bctx;
      }
      const bv = new DataView(body.buffer, body.byteOffset, body.byteLength);
      const pending = [];
      let p = 0;
      for (let t = 0; t < ntiles; t++) {
        if (p + 7 > body.length) throw new Error("short body");
        const x = bv.getUint16(p, true), y = bv.getUint16(p + 2, true), w = body[p + 4], h = body[p + 5], ty = body[p + 6];
        p += 7;
        if (!w || !h || x + w > sw || y + h > sh) throw new Error("tile outside");
        const img = target.createImageData(w, h), d = img.data;
        const put = (i, r, g, b) => { d[i] = r; d[i + 1] = g; d[i + 2] = b; d[i + 3] = 255; };
        if (ty === T_SOLID) {
          if (p + 3 > body.length) throw new Error("short body");
          for (let i = 0; i < w * h; i++) put(i * 4, body[p], body[p + 1], body[p + 2]);
          p += 3;
        } else if (ty === T_RAW) {
          const rows = unfilter(body, p, w * 3, h, 3);
          p += h * (w * 3 + 1);
          for (let i = 0; i < w * h; i++) put(i * 4, rows[i * 3], rows[i * 3 + 1], rows[i * 3 + 2]);
        } else if (ty === T_PALETTE) {
          if (p >= body.length) throw new Error("short body");
          const n = body[p++] + 1, pal = p;
          p += n * 3;
          const bits = bitsFor(n), rb = Math.ceil(w * bits / 8);
          if (p + rb * h > body.length) throw new Error("short body");
          for (let yy = 0; yy < h; yy++) for (let xx = 0; xx < w; xx++) {
            const row = p + yy * rb;
            const i = bits === 8 ? body[row + xx] : (body[row + ((xx * bits) >> 3)] >> (8 - bits - ((xx * bits) & 7))) & ((1 << bits) - 1);
            if (i >= n) throw new Error("palette index");
            put((yy * w + xx) * 4, body[pal + i * 3], body[pal + i * 3 + 1], body[pal + i * 3 + 2]);
          }
          p += rb * h;
        } else if (ty === T_YC) {
          const cw = (w + 1) >> 1, ch = (h + 1) >> 1;
          const yp = unfilter(body, p, w, h, 1); p += h * (w + 1);
          const cg = unfilter(body, p, cw, ch, 1); p += ch * (cw + 1);
          const co = unfilter(body, p, cw, ch, 1); p += ch * (cw + 1);
          for (let yy = 0; yy < h; yy++) for (let xx = 0; xx < w; xx++) {
            const k = (yy >> 1) * cw + (xx >> 1), Y = yp[yy * w + xx], G = cg[k] - 128, O = co[k] - 128;
            put((yy * w + xx) * 4, clamp8(Y - G + O), clamp8(Y + G), clamp8(Y - G - O));
          }
        } else if (ty === T_PNG) {
          if (p + 4 > body.length) throw new Error("short body");
          const len = bv.getUint32(p, true);
          p += 4;
          if (p + len > body.length) throw new Error("short body");
          const png = body.slice(p, p + len);
          p += len;
          pending.push(createImageBitmap(new Blob([png], { type: "image/png" })).then((bm) => { target.drawImage(bm, x, y); if (bm.close) bm.close(); }));
          continue;
        } else throw new Error("unknown tile type");
        target.putImageData(img, x, y);
      }
      if (p !== body.length) throw new Error("trailing bytes");
      if (pending.length) await Promise.all(pending);
      if (scale > 1) {
        this.ctx.imageSmoothingEnabled = true;
        this.ctx.drawImage(this.back, 0, 0, sw, sh, 0, 0, this.cv.width, this.cv.height);
      }
      return { seq, tiles: ntiles, bytes: m.length };
    }
  }

  const api = { Painter, canInflate };
  if (typeof module !== "undefined" && module.exports) module.exports = api;
  else root.IyakkiDecoder = api;

  // inside a Worker: draw on the OffscreenCanvas the page handed over, in order, and say when each message is drawn
  if (typeof document === "undefined" && typeof importScripts === "function") {
    let painter = null, chain = Promise.resolve();
    root.onmessage = (e) => {
      const m = e.data;
      if (m.t === "init") { painter = new Painter(m.canvas); root.postMessage({ t: "ready" }); }
      else if (m.t === "size" && painter) painter.resize(m.w, m.h);
      else if (m.t === "frame" && painter) {
        chain = chain.then(() => painter.paint(m.buf)).then(
          (r) => root.postMessage({ id: m.id, seq: r.seq, tiles: r.tiles }),
          (err) => root.postMessage({ id: m.id, err: String(err && err.message || err) }));
      }
    };
  }
})(typeof self !== "undefined" ? self : globalThis);
