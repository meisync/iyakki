(function (g) {
  'use strict';
  const E = (op, a) => JSON.parse(__host('window', op, JSON.stringify(a || {})));
  let L = E('location');
  const nav = (u) => { E('navigate', { url: String(u) }); };
  const resolve = (u) => { try { return typeof URL === 'function' ? new URL(String(u), L.href).href : String(u); } catch (e) { return L.href; } };
  const location = {
    get href() { return L.href; }, set href(v) { nav(v); },
    get protocol() { return L.protocol; }, get host() { return L.host; }, get hostname() { return L.hostname; }, get port() { return L.port; },
    get pathname() { return L.pathname; }, get search() { return L.search; }, get hash() { return L.hash; }, get origin() { return L.origin; },
    assign: nav, replace: nav, reload() {}, toString() { return L.href; }, ancestorOrigins: [],
  };
  g.window = g; g.self = g; g.top = g; g.parent = g; g.frames = g; g.location = location;
  g.navigator = {
    userAgent: 'Mozilla/5.0 (X11; Linux x86_64) Iyakki-Reader/0.1', appName: 'Netscape', appVersion: '5.0', language: 'en-US', languages: ['en-US', 'en'], platform: 'Linux x86_64',
    cookieEnabled: typeof __cookies === 'object', onLine: true, hardwareConcurrency: 4, deviceMemory: 4, maxTouchPoints: 0, vendor: '', webdriver: false, doNotTrack: '1',
    sendBeacon() { return false; }, javaEnabled() { return false; },
    clipboard: { writeText() { return Promise.resolve(); }, readText() { return Promise.resolve(''); } },
    permissions: { query() { return Promise.resolve({ state: 'denied' }); } },
  };
  g.history = {
    length: 1, state: null, scrollRestoration: 'auto',
    pushState(s, t, u) { this.state = s; if (u !== undefined && u !== null) { const h = resolve(u); try { const p = new URL(h); L = Object.assign({}, L, { href: p.href, pathname: p.pathname, search: p.search, hash: p.hash }); } catch (e) {} } },
    replaceState(s, t, u) { this.pushState(s, t, u); },
    back() {}, forward() {}, go() {},
  };
  g.screen = { width: 1280, height: 720, availWidth: 1280, availHeight: 720, colorDepth: 24, pixelDepth: 24, orientation: { type: 'landscape-primary', angle: 0 } };
  Object.assign(g, { innerWidth: 1280, innerHeight: 720, outerWidth: 1280, outerHeight: 720, devicePixelRatio: 1, scrollX: 0, scrollY: 0, pageXOffset: 0, pageYOffset: 0, name: '', closed: false, isSecureContext: L.protocol === 'https:', origin: L.origin });
  g.matchMedia = (q) => ({ matches: false, media: String(q), onchange: null, addListener() {}, removeListener() {}, addEventListener() {}, removeEventListener() {}, dispatchEvent() { return false; } });
  g.getComputedStyle = (el) => new Proxy({}, { get: (t, p) => { if (p === 'getPropertyValue') return (k) => (el && el.style ? el.style.getPropertyValue(k) : ''); if (typeof p !== 'string') return undefined; return el && el.style ? el.style[p] || '' : ''; } });
  g.alert = (m) => console.log('alert: ' + m); g.confirm = () => false; g.prompt = () => null;
  for (const f of ['print', 'focus', 'blur', 'scrollTo', 'scrollBy', 'scroll', 'stop', 'close', 'moveTo', 'resizeTo']) g[f] = () => {};
  g.open = () => null;
  g.postMessage = (data) => { Promise.resolve().then(() => { if (typeof MessageEvent === 'function') g.dispatchEvent(new MessageEvent('message', { data })); }); };
  g.structuredClone = (v) => JSON.parse(JSON.stringify(v));
  g.reportError = (e) => console.error(String(e));
  g.crypto = {
    getRandomValues(a) { const b = E('random', { n: a.byteLength }); new Uint8Array(a.buffer, a.byteOffset, a.byteLength).set(b); return a; },
    randomUUID() { const b = E('random', { n: 16 }); b[6] = (b[6] & 0x0f) | 0x40; b[8] = (b[8] & 0x3f) | 0x80; const h = b.map((x) => x.toString(16).padStart(2, '0')); return h.slice(0, 4).join('') + '-' + h.slice(4, 6).join('') + '-' + h.slice(6, 8).join('') + '-' + h.slice(8, 10).join('') + '-' + h.slice(10).join(''); },
  };
  g.CSS = { supports: () => false, escape: (s) => String(s).replace(/[^a-zA-Z0-9_-]/g, '\\$&') };
  class AbortSignal {
    constructor() { this.aborted = false; this.reason = undefined; this._l = []; this.onabort = null; }
    addEventListener(t, f) { if (t === 'abort') this._l.push(f); }
    removeEventListener(t, f) { this._l = this._l.filter((x) => x !== f); }
    throwIfAborted() { if (this.aborted) throw this.reason; }
    static timeout(ms) { const s = new AbortSignal(); setTimeout(() => { if (!s.aborted) { s.aborted = true; s.reason = new DOMException('The operation timed out.', 'TimeoutError'); if (typeof s.dispatchEvent === 'function') s.dispatchEvent(new Event('abort')); } }, Number(ms) || 0); return s; }
    static any(list) { const s = new AbortSignal(); for (const x of list) { if (x.aborted) { s.aborted = true; s.reason = x.reason; break; } } return s; }
    static abort(r) { const s = new AbortSignal(); s.aborted = true; s.reason = r; return s; }
  }
  class AbortController {
    constructor() { this.signal = new AbortSignal(); }
    abort(reason) {
      const s = this.signal; if (s.aborted) return;
      s.aborted = true; s.reason = reason === undefined ? new Error('AbortError: the operation was aborted') : reason;
      for (const f of s._l.slice()) { try { f({ type: 'abort', target: s }); } catch (e) {} }
      if (typeof s.onabort === 'function') { try { s.onabort({ type: 'abort', target: s }); } catch (e) {} }
    }
  }
  g.AbortController = AbortController; g.AbortSignal = AbortSignal;
})(globalThis);
