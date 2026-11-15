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

  /** The public key (hex) of a 32-byte seed (hex): the fallback key of a browser without Ed25519 in WebCrypto. */
  publicFromSeed(seedHex) { const r = this.cmd({ c: "public_from_seed", seed: seedHex }); return r ? r.public : null; }
}

export const hex = (u8) => [...u8].map((b) => b.toString(16).padStart(2, "0")).join("");
export const unhex = (s) => Uint8Array.from(s.match(/../g) || [], (h) => parseInt(h, 16));
export const randomHex = (n) => hex(crypto.getRandomValues(new Uint8Array(n)));
