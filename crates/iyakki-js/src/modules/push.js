(function (g) {
  'use strict';
  if (!g.isSecureContext || !g.navigator) return;
  const call = (op, a) => JSON.parse(__host('push', op, JSON.stringify(a)));
  const A = 'ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_';
  const enc = (b) => {
    let o = '';
    for (let i = 0; i < b.length; i += 3) {
      const n = (b[i] << 16) | ((b[i + 1] || 0) << 8) | (b[i + 2] || 0);
      o += A[(n >> 18) & 63] + A[(n >> 12) & 63] + (i + 1 < b.length ? A[(n >> 6) & 63] : '') + (i + 2 < b.length ? A[n & 63] : '');
    }
    return o;
  };
  const dec = (s) => {
    const out = [];
    let acc = 0, bits = 0;
    for (const ch of String(s)) {
      const v = A.indexOf(ch);
      if (v < 0) continue;
      acc = (acc << 6) | v; bits += 6;
      if (bits >= 8) { bits -= 8; out.push((acc >> bits) & 255); acc &= (1 << bits) - 1; }
    }
    return new Uint8Array(out).buffer;
  };
  const keyBytes = (v) => {
    if (typeof v === 'string') return new Uint8Array(dec(v));
    if (v instanceof ArrayBuffer) return new Uint8Array(v);
    if (ArrayBuffer.isView(v)) return new Uint8Array(v.buffer, v.byteOffset, v.byteLength);
    throw new TypeError('applicationServerKey must be an ArrayBuffer, a typed array or a base64url string');
  };
  const fail = (m) => {
    m = String(m);
    const k = m.indexOf(':');
    const names = ['NotAllowedError', 'SecurityError', 'InvalidStateError', 'NotSupportedError', 'InvalidAccessError', 'AbortError', 'TypeError'];
    if (k > 0 && names.indexOf(m.slice(0, k)) >= 0) return m.slice(0, k) === 'TypeError' ? new TypeError(m.slice(k + 1).trim()) : new DOMException(m.slice(k + 1).trim(), m.slice(0, k));
    return new DOMException(m, 'NotAllowedError');
  };
  const settle = (fn) => new Promise((resolve, reject) => {
    try { resolve(fn()); } catch (e) { reject(e && e.message !== undefined && !(e instanceof DOMException) && !(e instanceof TypeError) ? fail(e.message) : e); }
  });
  const ask = (op, a) => { const r = call(op, a); if (r.error) throw fail(r.error); return r; };

  const ET = g.EventTarget || class {};
  class PushSubscriptionOptions {
    constructor(userVisibleOnly, key) { this.userVisibleOnly = userVisibleOnly; Object.defineProperty(this, '_k', { value: key }); }
    get applicationServerKey() { return dec(this._k); }
  }
  class PushSubscription {
    constructor(r, scope) {
      this.endpoint = r.endpoint;
      this.expirationTime = r.expirationTime === undefined ? null : r.expirationTime;
      this.options = new PushSubscriptionOptions(true, r.key);
      Object.defineProperty(this, '_r', { value: r });
      Object.defineProperty(this, '_scope', { value: scope });
    }
    getKey(name) { return name === 'p256dh' ? dec(this._r.p256dh) : name === 'auth' ? dec(this._r.auth) : null; }
    toJSON() { return { endpoint: this.endpoint, expirationTime: this.expirationTime, keys: { p256dh: this._r.p256dh, auth: this._r.auth } }; }
    unsubscribe() { return settle(() => ask('unsubscribe', { scope: this._scope }).done === true); }
  }
  class PushManager {
    constructor(scope) { Object.defineProperty(this, '_scope', { value: scope }); }
    static get supportedContentEncodings() { return ['aes128gcm']; }
    permissionState(o) { return settle(() => ask('permission', {}).state); }
    getSubscription() { return settle(() => { const r = ask('get', { scope: this._scope }); return r.sub ? new PushSubscription(r.sub, this._scope) : null; }); }
    subscribe(o) {
      return settle(() => {
        o = o || {};
        if (o.userVisibleOnly !== true) throw new DOMException('Only user-visible push is supported: a notification must be shown for every push (set userVisibleOnly to true).', 'NotSupportedError');
        if (o.applicationServerKey === undefined || o.applicationServerKey === null) throw new DOMException('An applicationServerKey is needed (the sender\'s VAPID public key).', 'InvalidAccessError');
        const k = keyBytes(o.applicationServerKey);
        if (k.length !== 65 || k[0] !== 4) throw new DOMException('The applicationServerKey is not an uncompressed P-256 public key (65 bytes starting with 4).', 'InvalidAccessError');
        const r = ask('subscribe', { scope: this._scope, key: enc(k) });
        return new PushSubscription(r.sub, this._scope);
      });
    }
  }
  class ServiceWorker extends ET {
    constructor(script) { super(); this.scriptURL = script; this.state = 'activated'; this.onstatechange = null; }
    postMessage() {}
  }
  class ServiceWorkerRegistration extends ET {
    constructor(scope, script) {
      super();
      this.scope = scope; this.updateViaCache = 'imports'; this.installing = null; this.waiting = null;
      this.active = new ServiceWorker(script);
      this.pushManager = new PushManager(scope);
    }
    update() { return Promise.resolve(this); }
    unregister() { return settle(() => ask('unregister', { scope: this.scope }).done === true); }
    showNotification() { return Promise.reject(new DOMException('Notifications from a page are not shown here; a push is shown by the system.', 'NotSupportedError')); }
    getNotifications() { return Promise.resolve([]); }
  }
  class ServiceWorkerContainer extends ET {
    constructor() { super(); this.controller = null; this._regs = []; this._waiters = []; }
    get ready() { return this._regs.length ? Promise.resolve(this._regs[0]) : new Promise((r) => this._waiters.push(r)); }
    register(url, opts) {
      return settle(() => {
        const u = new URL(String(url), g.location.href);
        if (u.origin !== g.location.origin) throw new DOMException('A service worker script must come from the page\'s own origin.', 'SecurityError');
        const dir = u.pathname.slice(0, u.pathname.lastIndexOf('/') + 1);
        const s = new URL(opts && opts.scope !== undefined ? String(opts.scope) : dir, u);
        if (s.origin !== u.origin || !s.pathname.startsWith(dir)) throw new DOMException('The scope is wider than the folder of the script.', 'SecurityError');
        ask('register', { script: u.href, scope: s.href });
        let r = this._regs.find((x) => x.scope === s.href);
        if (!r) { r = new ServiceWorkerRegistration(s.href, u.href); this._regs.push(r); this._waiters.splice(0).forEach((w) => w(r)); }
        return r;
      });
    }
    getRegistration(u) { return settle(() => { const p = new URL(u === undefined ? '' : String(u), g.location.href).href; return this._regs.filter((r) => p.startsWith(r.scope)).sort((a, b) => b.scope.length - a.scope.length)[0]; }); }
    getRegistrations() { return Promise.resolve(this._regs.slice()); }
    startMessages() {}
  }
  g.PushManager = PushManager; g.PushSubscription = PushSubscription; g.PushSubscriptionOptions = PushSubscriptionOptions;
  g.ServiceWorker = ServiceWorker; g.ServiceWorkerRegistration = ServiceWorkerRegistration; g.ServiceWorkerContainer = ServiceWorkerContainer;
  Object.defineProperty(g.navigator, 'serviceWorker', { value: new ServiceWorkerContainer(), configurable: true, enumerable: true });
})(globalThis);
