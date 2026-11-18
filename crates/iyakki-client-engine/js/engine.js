// The glue between a host (a page, or node in the tests) and the client engine module (iyakki-client-engine.wasm). EXPERIMENTAL.
// Plain WebAssembly calls, no generated bindings: strings go in and out of the module's memory as UTF-8, records come back as JSON lines.
// Nothing here touches the network, the DOM or storage; the page does that and hands messages in.

const enc = new TextEncoder();
const dec = new TextDecoder();

export class IyakkiEngine {
  constructor(instance) {
    this.x = instance.exports;
    this.mem = () => this.x.memory.buffer;
  }

  /** Compile and start the module from its bytes (an ArrayBuffer or a Uint8Array). */
  static async load(bytes) {
    const { instance } = await WebAssembly.instantiate(bytes, {});
    return new IyakkiEngine(instance);
  }

  // text in: write into a fresh buffer, call, wipe the buffer (it may hold a password or a seed), free it
  _with(text, f) {
    const b = enc.encode(text);
    const p = this.x.iy_alloc(b.length);
    new Uint8Array(this.mem(), p, b.length).set(b);
    try { return f(p, b.length); } finally { new Uint8Array(this.mem(), p, b.length).fill(0); this.x.iy_free(p, b.length); }
  }

  _reply(n) { return n > 0 ? dec.decode(new Uint8Array(this.mem(), this.x.iy_reply_ptr(), n)) : ""; }

  /** cfg: { token, public, name, device, client }. Returns false when the public key is not 64 hex digits. */
  init(cfg) { return this._with(JSON.stringify(cfg), (p, n) => this.x.iy_init(p, n)) === 1; }

  /** One command object, for example { c: "pointer", kind: "down", x: 1, y: 2 }. Returns the parsed reply, true when there is none, false when refused. */
  cmd(o) {
    const n = this._with(JSON.stringify(o), (p, l) => this.x.iy_cmd(p, l));
    if (n < 0) return false;
    return n === 0 ? true : JSON.parse(this._reply(n));
  }

  onText(s) { this._with(s, (p, n) => this.x.iy_on_text(p, n)); }

  onBinary(buf) {
    const u = buf instanceof Uint8Array ? buf : new Uint8Array(buf);
    const p = this.x.iy_alloc(u.length);
    new Uint8Array(this.mem(), p, u.length).set(u);
    try { this.x.iy_on_binary(p, u.length); } finally { this.x.iy_free(p, u.length); }
  }

  /** The records the engine wants acted on, in order: { k: "send", text } and so on. */
  drain() {
    const out = [];
    for (let n = this.x.iy_next_out(); n > 0; n = this.x.iy_next_out()) out.push(JSON.parse(this._reply(n)));
    return out;
  }

  /** The picture as { data: Uint8ClampedArray (a view of the module's memory: draw it before the next call), w, h, version }. */
  framebuffer() {
    const w = this.x.iy_fb_w(), h = this.x.iy_fb_h();
    if (!w || !h) return null;
    return { data: new Uint8ClampedArray(this.mem(), this.x.iy_fb_ptr(), w * h * 4), w, h, version: this.x.iy_fb_version() };
  }

  // ---- the UI SDK's display list (F-SDK-22): draw a list, find the action at a place, make an input line ----

  /**
   * Read a display list (text) and draw its shapes at `scale` pixels per unit. Returns { w, h, version, lw, lh, scale, a11y, texts }:
   * the picture size, the accessibility tree (name, role, action, place in pixels) and the text runs the page draws with its own fonts
   * (the module has none). Returns { error } when the list is refused (newer, unknown operation, damaged); the old picture stays.
   */
  uiLoad(text, scale = 1) {
    const n = this._with(JSON.stringify({ c: "ui_load", text, scale }), (p, l) => this.x.iy_cmd(p, l));
    const r = JSON.parse(this._reply(Math.abs(n)));
    return n < 0 ? { error: r.error || "refused" } : r;
  }

  /** Draw the list again after glyphs were handed in; the same reply as uiLoad. */
  uiRedraw() {
    const n = this._with(JSON.stringify({ c: "ui_redraw" }), (p, l) => this.x.iy_cmd(p, l));
    const r = JSON.parse(this._reply(Math.abs(n)));
    return n < 0 ? { error: r.error || "refused" } : r;
  }

  /**
   * The text of the module's own pictures (F-SDK-28): the module has no fonts, so the page draws the glyphs it lacks, once each, with its
   * own fonts and hands them in as alpha bitmaps; the module then composes every run whose glyphs it has. `fonts` is { sans, mono } (CSS
   * font families). Returns the last reply (uiRedraw's) or null when nothing was missing; `r.texts` holds the runs still left to the page.
   * `canvasFor(w, h)` makes a 2D canvas (an OffscreenCanvas by default).
   */
  uiGlyphs(fonts = { sans: "system-ui, sans-serif", mono: "ui-monospace, monospace" }, canvasFor = (w, h) => new OffscreenCanvas(w, h)) {
    let last = null;
    for (let round = 0; round < 4; round++) {
      const need = this.cmd({ c: "ui_needs" });
      if (!need || !need.glyphs || !need.glyphs.length) break;
      for (const g of need.glyphs) {
        const pad = Math.ceil(g.size * 0.35);
        const font = (g.style === "bold" ? "bold " : "") + g.size + "px " + (g.style === "mono" ? fonts.mono : fonts.sans);
        const probe = canvasFor(8, 8).getContext("2d");
        probe.font = font;
        const adv = probe.measureText(g.ch).width;
        const w = Math.max(1, Math.min(512, Math.ceil(adv) + 2 * pad)), h = Math.max(1, Math.min(512, Math.ceil(g.size * 1.4) + 2 * pad));
        const c = canvasFor(w, h), x = c.getContext("2d", { willReadFrequently: true });
        x.font = font; x.fillStyle = "#fff";
        // the same place the page draws a run: the baseline at 0.8 of the size under the run's top
        x.fillText(g.ch, pad, pad + g.size * 0.8);
        const px = x.getImageData(0, 0, w, h).data, a = new Uint8Array(w * h);
        for (let i = 0; i < a.length; i++) a[i] = px[i * 4 + 3];
        this.cmd({ c: "ui_glyph", ch: g.ch, size: g.size, style: g.style, adv, w, h, ox: -pad, oy: -pad, alpha: hex(a) });
      }
      last = this.uiRedraw();
      if (last.error) break;
    }
    return last;
  }

  /** The action of the hit region at a place in pixels, or null. Returns { action, line } with the SDK's `action N` line. */
  uiHit(x, y) { return this.cmd({ c: "ui_hit", x, y }); }

  /** An input line for the SDK's session (`ptr ...`, `key ...`, `text ...`, `action N`, `wheel N`, `resize W H`, `tick T`), or null when refused. */
  uiLine(o) { const r = this.cmd({ c: "ui_line", ...o }); return r ? r.line : null; }

  /** The display-list picture, like framebuffer(): { data, w, h, version }. */
  uiFramebuffer() {
    const w = this.x.iy_ui_fb_w(), h = this.x.iy_ui_fb_h();
    if (!w || !h) return null;
    return { data: new Uint8ClampedArray(this.mem(), this.x.iy_ui_fb_ptr(), w * h * 4), w, h, version: this.x.iy_ui_fb_version() };
  }

  /** The public key (hex) of a 32-byte seed (hex): the fallback key of a browser without Ed25519 in WebCrypto. */
  publicFromSeed(seedHex) { const r = this.cmd({ c: "public_from_seed", seed: seedHex }); return r ? r.public : null; }
}

export const hex = (u8) => [...u8].map((b) => b.toString(16).padStart(2, "0")).join("");
export const unhex = (s) => Uint8Array.from(s.match(/../g) || [], (h) => parseInt(h, 16));
export const randomHex = (n) => hex(crypto.getRandomValues(new Uint8Array(n)));
