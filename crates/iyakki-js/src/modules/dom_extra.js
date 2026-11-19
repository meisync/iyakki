(function (g) {
  'use strict';
  // Added after the page object: sizes from the estimated layout, computed styles, real media queries and observers, a canvas
  // that accepts drawing (and draws nothing), attribute nodes, and the many small members pages probe for.
  const D = (op, a) => JSON.parse(__host('dom', op, JSON.stringify(a || {})));
  const hide = (o, k, v) => Object.defineProperty(o, k, { value: v, writable: true, enumerable: false, configurable: true });
  const def = (o, k, d) => Object.defineProperty(o, k, Object.assign({ configurable: true }, d));
  const own = (proto, k) => { while (proto && !Object.prototype.hasOwnProperty.call(proto, k)) proto = Object.getPrototypeOf(proto); return proto; };
  const El = g.Element.prototype, doc = g.document;
  const mkRect = (x, y, w, h) => new g.DOMRect(x, y, w, h);

  // ------------------------------------------------------------------ sizes
  // [x, y, width, height, border-left, border-top, border-right, border-bottom, inline, has a box]
  // in the page, counting the content of shadow roots (a root is a fragment that hangs on its host)
  const inPage = (el) => { for (let n = el, i = 0; n && i < 400; i++) { if (n === doc) return true; const p = n.parentNode; if (p) n = p; else if (n._type === 11 && n.host) n = n.host; else return false; } return false; };
  const R = (el) => (el._id !== undefined && inPage(el) ? D('rect', { n: el._id }) : [0, 0, 0, 0, 0, 0, 0, 0, 0, 0]);
  const vw = () => g.innerWidth, vh = () => g.innerHeight;
  const isRoot = (el) => el === doc.documentElement;
  // the boxes around an element that were scrolled (`scrollTop`, `scrollLeft`, `scrollTo`) carry their content with them: what an element
  // reports is moved back by the sum of their positions (a fixed box is not in their content; the window's own scroll is taken off apart)
  const scrolledBy = (el) => {
    let dx = 0, dy = 0, any = false;
    for (let n = el.parentNode || (el._type === 11 && el.host) || null, i = 0; n && i < 400; i++) {
      if (n !== doc && n !== doc.documentElement && n._sp) { dx += n._sp[0]; dy += n._sp[1]; any = true; }
      n = n.parentNode || (n._type === 11 && n.host) || null;
    }
    if (!any || (dx === 0 && dy === 0)) return [0, 0];
    return D('computed', { n: el._id }).position === 'fixed' ? [0, 0] : [dx, dy];
  };
  const viewRect = (el, r) => { const s = scrolledBy(el); return mkRect(r[0] - g.scrollX - s[0], r[1] - g.scrollY - s[1], r[2], r[3]); };
  def(El, 'getBoundingClientRect', { writable: true, value() { return viewRect(this, R(this)); } });
  def(El, 'getClientRects', { writable: true, value() { const r = R(this); const a = r[2] || r[3] ? [viewRect(this, r)] : []; a.item = (i) => a[i] || null; return a; } });
  const box = (k) => (el) => { const r = R(el); return k === 'w' ? r[2] : r[3]; };
  def(El, 'offsetWidth', { get() { return Math.round(box('w')(this)); } });
  def(El, 'offsetHeight', { get() { return Math.round(box('h')(this)); } });
  // the offset parent is the body, and for the body (or no parent) the position is measured from the page's own corner, margins of the body included
  def(El, 'offsetLeft', { get() { return Math.round(R(this)[0]); } });
  def(El, 'offsetTop', { get() { return Math.round(R(this)[1]); } });
  def(El, 'offsetParent', { get() { return this !== doc.body && !isRoot(this) && R(this)[9] ? doc.body : null; } });
  def(El, 'clientWidth', { get() { if (isRoot(this)) return vw(); const r = R(this); return r[8] ? 0 : Math.round(r[2] - r[4] - r[6]); } });
  def(El, 'clientHeight', { get() { if (isRoot(this)) return vh(); const r = R(this); return r[8] ? 0 : Math.round(r[3] - r[5] - r[7]); } });
  def(El, 'clientLeft', { get() { return Math.round(R(this)[4]); } });
  def(El, 'clientTop', { get() { return Math.round(R(this)[5]); } });
  // the size of the content: a box whose children reach past it scrolls
  const content = (el) => (el._id !== undefined && inPage(el) ? D('scrollsize', { n: el._id }) : [0, 0]);
  def(El, 'scrollWidth', { get() { return isRoot(this) ? Math.max(vw(), this.clientWidth) : Math.max(Math.ceil(content(this)[0]) - Math.round(R(this)[4] + R(this)[6]), this.clientWidth); } });
  def(El, 'scrollHeight', { get() { return isRoot(this) || this === doc.body ? Math.max(Math.ceil(D('docheight')), this.clientHeight) : Math.max(Math.ceil(content(this)[1]) - Math.round(R(this)[5] + R(this)[7]), this.clientHeight); } });
  // ------------------------------------------------------------------ scrolling
  // The window and every box keep a scroll position, kept between 0 and what the content allows; a change fires one `scroll` event
  // (a task later, however many calls made it), on the document for the window and on the box otherwise. Nothing is drawn from it:
  // the page is read once, so this is for scripts that measure, lazy-load or restore a position.
  const num = (v) => { v = Number(v); return Number.isFinite(v) ? v : 0; };
  const maxX = () => Math.max(0, Math.ceil(D('docwidth')) - vw()), maxY = () => Math.max(0, Math.ceil(D('docheight')) - vh());
  const pending = new Set();
  const fire = (target) => {
    if (pending.has(target)) return;
    pending.add(target);
    g.setTimeout(() => { pending.delete(target); target.dispatchEvent(new g.Event('scroll', { bubbles: target === doc })); }, 0);
  };
  const winTo = (x, y) => {
    const nx = Math.min(Math.max(0, num(x)), maxX()), ny = Math.min(Math.max(0, num(y)), maxY());
    const moved = nx !== g.scrollX || ny !== g.scrollY;
    g.scrollX = g.pageXOffset = nx; g.scrollY = g.pageYOffset = ny;
    if (moved) fire(doc);
  };
  const args = (a, b, curX, curY) => {
    const o = a;
    if (o !== null && typeof o === 'object') return [o.left === undefined ? curX : num(o.left), o.top === undefined ? curY : num(o.top)];
    return b === undefined && a === undefined ? [curX, curY] : [num(a), num(b)];
  };
  g.scrollTo = g.scroll = (a, b) => { const p = args(a, b, g.scrollX, g.scrollY); winTo(p[0], p[1]); };
  g.scrollBy = (a, b) => { const p = args(a, b, 0, 0); winTo(g.scrollX + p[0], g.scrollY + p[1]); };
  const pos = (el) => (el._sp || [0, 0]);
  const elTo = (el, x, y) => {
    if (isRoot(el)) return winTo(x, y);
    const mx = Math.max(0, el.scrollWidth - el.clientWidth), my = Math.max(0, el.scrollHeight - el.clientHeight);
    const nx = Math.min(Math.max(0, num(x)), mx), ny = Math.min(Math.max(0, num(y)), my);
    const was = pos(el);
    hide(el, '_sp', [nx, ny]);
    if (nx !== was[0] || ny !== was[1]) fire(el);
  };
  def(El, 'scrollTo', { writable: true, value(a, b) { const p = pos(this), c = isRoot(this) ? [g.scrollX, g.scrollY] : p; const q = args(a, b, c[0], c[1]); elTo(this, q[0], q[1]); } });
  def(El, 'scroll', { writable: true, value(a, b) { this.scrollTo(a, b); } });
  def(El, 'scrollBy', { writable: true, value(a, b) { const c = isRoot(this) ? [g.scrollX, g.scrollY] : pos(this), q = args(a, b, 0, 0); elTo(this, c[0] + q[0], c[1] + q[1]); } });
  def(El, 'scrollTop', { get() { return isRoot(this) ? g.scrollY : pos(this)[1]; }, set(v) { elTo(this, isRoot(this) ? g.scrollX : pos(this)[0], v); } });
  def(El, 'scrollLeft', { get() { return isRoot(this) ? g.scrollX : pos(this)[0]; }, set(v) { elTo(this, v, isRoot(this) ? g.scrollY : pos(this)[1]); } });
  def(El, 'scrollIntoView', { writable: true, value(o) {
    const r = R(this); if (!r[9]) return;
    const blk = o && typeof o === 'object' && o.block ? o.block : (o === false ? 'end' : 'start');
    const y = blk === 'end' ? r[1] + r[3] - vh() : blk === 'center' ? r[1] - (vh() - r[3]) / 2 : r[1];
    winTo(g.scrollX, y);
  } });
  def(El, 'scrollIntoViewIfNeeded', { writable: true, value() { const r = R(this); if (!r[9]) return; if (r[1] < g.scrollY || r[1] + r[3] > g.scrollY + vh()) winTo(g.scrollX, r[1] - (vh() - r[3]) / 2); } });
  // what lies under a point of the window, the top one first (stacking order: see `LayoutState::hit` in the crate)
  const under = (x, y) => (Number.isFinite(Number(x)) && Number.isFinite(Number(y)) ? g.__wrapPairs(D('point', { x: Number(x) + g.scrollX, y: Number(y) + g.scrollY })) : []);
  def(g.Document.prototype, 'elementFromPoint', { writable: true, value(x, y) { if (num(x) < 0 || num(y) < 0 || num(x) >= vw() || num(y) >= vh()) return null; return under(x, y)[0] || null; } });
  def(g.Document.prototype, 'elementsFromPoint', { writable: true, value(x, y) { if (num(x) < 0 || num(y) < 0 || num(x) >= vw() || num(y) >= vh()) return []; return under(x, y); } });
  def(El, 'checkVisibility', { writable: true, value() { if (!inPage(this)) return false; const c = D('computed', { n: this._id }); return c.display !== 'none' && c.visibility !== 'hidden'; } });
  def(g.Document.prototype, 'scrollingElement', { get() { return this.documentElement; } });
  if (g.visualViewport) Object.assign(g.visualViewport, { width: vw(), height: vh() });
  if (g.screen) Object.assign(g.screen, { width: vw(), height: vh(), availWidth: vw(), availHeight: vh() });

  // ------------------------------------------------------------------ computed styles
  // a number says its thousands the way the page's language does (QuickJS alone writes none)
  if (typeof Intl !== 'undefined' && Intl.NumberFormat) { const orig = Number.prototype.toLocaleString; Object.defineProperty(Number.prototype, 'toLocaleString', { value: function (l, o) { try { return new Intl.NumberFormat(l, o).format(this.valueOf()); } catch (x) { return orig.call(this, l, o); } }, configurable: true, writable: true }); }
  g.getComputedStyle = (el) => {
    const live = el && el._id !== undefined && inPage(el);
    const map = live ? D('computed', { n: el._id }) : {};
    const kebab = (s) => s.replace(/[A-Z]/g, (c) => '-' + c.toLowerCase());
    // the engine's computed value when it has one (the cascade, units resolved); the element's own text only for what the engine does not compute
    const read = (k) => { const own = el && el.style ? el.style.getPropertyValue(k) : ''; return map[k] || own || ''; };
    const names = Object.keys(map);
    return new Proxy({}, {
      get: (t, p) => {
        if (p === 'getPropertyValue') return (k) => read(String(k).toLowerCase());
        if (p === 'getPropertyPriority') return () => '';
        if (p === 'length') return names.length;
        if (p === 'item') return (i) => names[i] || '';
        if (p === 'cssText') return names.map((k) => k + ': ' + map[k] + ';').join(' ');
        if (typeof p !== 'string') return undefined;
        return read(p.startsWith('--') ? p : kebab(p));
      },
      has: (t, p) => typeof p === 'string' && (p in map || kebab(p) in map),
    });
  };

  // ------------------------------------------------------------------ media queries
  const mqls = new Set();
  class MediaQueryList extends g.EventTarget {
    constructor(media) { super(); hide(this, '_m', String(media)); this.onchange = null; hide(this, '_last', this.matches); mqls.add(new WeakRef(this)); if (mqls.size % 256 === 0) for (const r of mqls) if (r.deref() === undefined) mqls.delete(r); }
    get media() { return this._m; }
    get matches() { return !!JSON.parse(__host('window', 'media', JSON.stringify({ q: this._m }))); }
    addListener(f) { this.addEventListener('change', f); }
    removeListener(f) { this.removeEventListener('change', f); }
  }
  g.MediaQueryList = MediaQueryList;
  g.matchMedia = (q) => new MediaQueryList(q);

  // ------------------------------------------------------------------ observers
  const observers = new Set();
  const area = (r) => Math.max(0, r.width) * Math.max(0, r.height);
  const ioSet = new Set();
  class IntersectionObserver {
    constructor(cb, opts) {
      opts = opts || {};
      this._cb = cb; this.root = opts.root || null; this.rootMargin = opts.rootMargin || '0px';
      this.thresholds = [].concat(opts.threshold === undefined ? 0 : opts.threshold);
      hide(this, '_t', new Map()); hide(this, '_q', []);
    }
    observe(t) { if (this._t.has(t)) return; this._t.set(t, null); ioSet.add(this); Promise.resolve().then(() => this._check(true)); }
    unobserve(t) { this._t.delete(t); }
    disconnect() { this._t.clear(); ioSet.delete(this); }
    takeRecords() { const q = this._q; this._q = []; return q; }
    _check(first) {
      const rb = this.root ? this.root.getBoundingClientRect() : mkRect(0, 0, vw(), vh());
      const out = [];
      for (const [t, prev] of this._t) {
        const r = t.getBoundingClientRect();
        const x0 = Math.max(r.left, rb.left), y0 = Math.max(r.top, rb.top), x1 = Math.min(r.right, rb.right), y1 = Math.min(r.bottom, rb.bottom);
        const touching = x1 >= x0 && y1 >= y0 && t.isConnected;
        const ia = touching ? mkRect(x0, y0, x1 - x0, y1 - y0) : mkRect(0, 0, 0, 0);
        const ratio = area(r) > 0 ? area(ia) / area(r) : touching ? 1 : 0;
        const step = this.thresholds.filter((th) => ratio >= th).length;
        const inter = touching && (ratio > 0 || area(r) === 0 || this.thresholds.includes(0));
        const key = (inter ? 1 : 0) + ':' + step;
        if (prev === key) continue;
        this._t.set(t, key);
        out.push({ target: t, isIntersecting: inter, intersectionRatio: Math.min(1, ratio), boundingClientRect: r, intersectionRect: ia, rootBounds: rb, time: g.performance ? g.performance.now() : 0 });
      }
      if (out.length) { this._q.push(...out); const q = this.takeRecords(); try { this._cb(q, this); } catch (e) { g.reportError && g.reportError(e); } }
    }
  }
  const watchers = new Set();
  class ResizeObserver {
    constructor(cb) { this._cb = cb; hide(this, '_t', new Map()); }
    observe(t) {
      if (this._t.has(t)) return; this._t.set(t, null); watchers.add(this);
      Promise.resolve().then(() => this._look(t, true));
    }
    _look(t, force) {
      {
        if (!this._t.has(t)) return;
        const r = R(t), w = r[2] - r[4] - r[6], h = r[3] - r[5] - r[7];
        if (!force && this._t.get(t) === w + 'x' + h) return;
        this._t.set(t, w + 'x' + h);
        if (!w && !h) return;
        const cr = mkRect(r[4], r[5], w, h);
        const sz = [{ inlineSize: w, blockSize: h }];
        try { this._cb([{ target: t, contentRect: cr, borderBoxSize: [{ inlineSize: r[2], blockSize: r[3] }], contentBoxSize: sz, devicePixelContentBoxSize: sz }], this); } catch (e) { g.reportError && g.reportError(e); }
      }
    }
    unobserve(t) { this._t.delete(t); }
    disconnect() { this._t.clear(); }
  }
  g.IntersectionObserver = IntersectionObserver; g.ResizeObserver = ResizeObserver;
  // the embedder changed the window: the page's numbers, media query lists, observers and `resize` follow (see iyakki_js::resize)
  hide(g, '__iyakkiResize', () => {
    const V = JSON.parse(__host('window', 'viewport', '{}'));
    Object.assign(g, { innerWidth: V.w, innerHeight: V.h, outerWidth: V.w, outerHeight: V.h, devicePixelRatio: V.d > 0 ? V.d : 1 });
    if (g.visualViewport) Object.assign(g.visualViewport, { width: V.w, height: V.h });
    if (g.screen) Object.assign(g.screen, { width: V.w, height: V.h, availWidth: V.w, availHeight: V.h });
    for (const ref of Array.from(mqls)) {
      const m = ref.deref(); if (!m) { mqls.delete(ref); continue; }
      const now = m.matches; if (now === m._last) continue;
      m._last = now;
      const ev = new g.Event('change'); ev.matches = now; ev.media = m.media;
      m.dispatchEvent(ev);
    }
    g.dispatchEvent(new g.Event('resize'));
    for (const io of Array.from(ioSet)) io._check(false);
    for (const ro of watchers) for (const t of Array.from(ro._t.keys())) ro._look(t, false);
  });

  // MutationObserver: every change made through the DOM methods below is turned into a record for the observers that watch it
  class MutationObserver {
    constructor(cb) { this._cb = cb; hide(this, '_targets', []); hide(this, '_rec', []); hide(this, '_pending', false); }
    observe(node, opts) {
      opts = opts || {};
      if (!opts.childList && !opts.attributes && !opts.characterData && !(opts.attributeFilter || opts.attributeOldValue)) throw new TypeError('The options object must set at least one of childList, attributes, or characterData');
      const o = { childList: !!opts.childList, subtree: !!opts.subtree, attributes: opts.attributes === undefined ? !!(opts.attributeFilter || opts.attributeOldValue) : !!opts.attributes, characterData: opts.characterData === undefined ? !!opts.characterDataOldValue : !!opts.characterData, attributeOldValue: !!opts.attributeOldValue, characterDataOldValue: !!opts.characterDataOldValue, attributeFilter: opts.attributeFilter ? opts.attributeFilter.map(String) : null };
      const i = this._targets.findIndex((t) => t.node === node);
      if (i >= 0) this._targets[i].o = o; else this._targets.push({ node, o });
      observers.add(this);
    }
    disconnect() { this._targets.length = 0; this._rec = []; observers.delete(this); }
    takeRecords() { const r = this._rec; this._rec = []; return r; }
  }
  g.MutationObserver = MutationObserver;
  const within = (node, root) => { for (let n = node; n; n = n.parentNode) if (n === root) return true; return false; };
  const queue = (rec, opt) => {
    for (const ob of observers) {
      for (const { node, o } of ob._targets) {
        if (rec.type === 'childList' ? !o.childList : rec.type === 'attributes' ? !o.attributes : !o.characterData) continue;
        if (!(node === rec.target || (o.subtree && within(rec.target, node)))) continue;
        if (rec.type === 'attributes' && o.attributeFilter && !o.attributeFilter.includes(rec.attributeName)) continue;
        const r = Object.assign({ addedNodes: [], removedNodes: [], previousSibling: null, nextSibling: null, attributeName: null, attributeNamespace: null, oldValue: null }, rec);
        if (rec.type === 'attributes' && !o.attributeOldValue) r.oldValue = null;
        if (rec.type === 'characterData' && !o.characterDataOldValue) r.oldValue = null;
        ob._rec.push(r);
        if (!ob._pending) { ob._pending = true; Promise.resolve().then(() => { ob._pending = false; const list = ob.takeRecords(); if (list.length) { try { ob._cb(list, ob); } catch (e) { g.reportError && g.reportError(e); } } }); }
        break;
      }
    }
  };
  const kids = (n) => Array.from(n.childNodes || []);
  const wrapMethod = (proto, k, make) => {
    const o = own(proto, k); if (!o) return;
    const d = Object.getOwnPropertyDescriptor(o, k); const orig = d.value;
    def(o, k, { writable: true, value: make(orig) });
  };
  const wrapSetter = (proto, k, make) => {
    const o = own(proto, k); if (!o) return;
    const d = Object.getOwnPropertyDescriptor(o, k);
    if (!d.set) return;
    const orig = d.set;
    def(o, k, { get: d.get, set: make(orig, d.get) });
  };
  const Nd = g.Node.prototype;
  wrapMethod(Nd, 'appendChild', (orig) => function (c) {
    if (!observers.size) return orig.call(this, c);
    const old = c && c.parentNode, added = c && c._type === 11 ? kids(c) : [c], prev = this.lastChild;
    const r = orig.call(this, c);
    if (old && c._type !== 11) queue({ type: 'childList', target: old, removedNodes: [c] });
    queue({ type: 'childList', target: this, addedNodes: added, previousSibling: prev });
    return r;
  });
  wrapMethod(Nd, 'insertBefore', (orig) => function (c, ref) {
    if (!observers.size) return orig.call(this, c, ref);
    const old = c && c.parentNode, added = c && c._type === 11 ? kids(c) : [c], prev = ref ? ref.previousSibling : this.lastChild;
    const r = orig.call(this, c, ref);
    if (old && c._type !== 11) queue({ type: 'childList', target: old, removedNodes: [c] });
    queue({ type: 'childList', target: this, addedNodes: added, previousSibling: prev, nextSibling: ref || null });
    return r;
  });
  wrapMethod(Nd, 'removeChild', (orig) => function (c) {
    if (!observers.size) return orig.call(this, c);
    const prev = c.previousSibling, next = c.nextSibling;
    const r = orig.call(this, c);
    queue({ type: 'childList', target: this, removedNodes: [c], previousSibling: prev, nextSibling: next });
    return r;
  });
  // replaceChild calls insertBefore and removeChild, which report themselves
  wrapSetter(Nd, 'textContent', (orig) => function (v) {
    if (!observers.size || this._type === 3 || this._type === 8) return orig.call(this, v);
    const removed = kids(this);
    orig.call(this, v);
    const added = kids(this);
    if (removed.length || added.length) queue({ type: 'childList', target: this, addedNodes: added, removedNodes: removed });
  });
  wrapSetter(El, 'innerHTML', (orig) => function (v) {
    if (!observers.size) return orig.call(this, v);
    const removed = kids(this);
    orig.call(this, v);
    queue({ type: 'childList', target: this, addedNodes: kids(this), removedNodes: removed });
  });
  wrapMethod(El, 'setAttribute', (orig) => function (k, v) {
    if (!observers.size) return orig.call(this, k, v);
    const name = String(k).toLowerCase(), old = this.getAttribute(name);
    const r = orig.call(this, k, v);
    queue({ type: 'attributes', target: this, attributeName: name, oldValue: old });
    return r;
  });
  wrapMethod(El, 'removeAttribute', (orig) => function (k) {
    if (!observers.size) return orig.call(this, k);
    const name = String(k).toLowerCase(), old = this.getAttribute(name);
    const r = orig.call(this, k);
    if (old !== null) queue({ type: 'attributes', target: this, attributeName: name, oldValue: old });
    return r;
  });
  if (g.CharacterData) wrapSetter(g.CharacterData.prototype, 'data', (orig, get) => function (v) {
    if (!observers.size) return orig.call(this, v);
    const old = get.call(this); orig.call(this, v);
    queue({ type: 'characterData', target: this, oldValue: old });
  });

  // ------------------------------------------------------------------ attribute nodes
  class Attr {
    constructor(name, value, owner) { this.name = name; this.value = value === undefined ? '' : String(value); this.ownerElement = owner || null; }
    get nodeName() { return this.name; } get localName() { return this.name; } get nodeValue() { return this.value; } get specified() { return true; }
    get namespaceURI() { return null; } get prefix() { return null; } get nodeType() { return 2; }
  }
  g.Attr = Attr;
  def(g.Document.prototype, 'createAttribute', { writable: true, value(name) { return new Attr(String(name).toLowerCase(), '', null); } });
  def(g.Document.prototype, 'createAttributeNS', { writable: true, value(ns, name) { return new Attr(String(name).toLowerCase(), '', null); } });
  def(El, 'setAttributeNode', { writable: true, value(a) { const old = this.getAttributeNode(a.name); this.setAttribute(a.name, a.value); a.ownerElement = this; return old; } });
  def(El, 'setAttributeNodeNS', { writable: true, value(a) { return this.setAttributeNode(a); } });
  def(El, 'getAttributeNode', { writable: true, value(n) { n = String(n).toLowerCase(); const v = this.getAttribute(n); return v === null ? null : new Attr(n, v, this); } });
  def(El, 'removeAttributeNode', { writable: true, value(a) { this.removeAttribute(a.name); return a; } });
  def(El, 'getAttributeNS', { writable: true, value(ns, n) { return this.getAttribute(n); } });
  def(El, 'setAttributeNS', { writable: true, value(ns, n, v) { this.setAttribute(String(n).replace(/^[^:]*:/, (m) => (ns ? m : '')), v); } });
  def(El, 'removeAttributeNS', { writable: true, value(ns, n) { this.removeAttribute(n); } });
  // attributes can be read by position and by name (`el.attributes.class`, `el.attributes['data-x']`)
  const attrsDesc = Object.getOwnPropertyDescriptor(own(El, 'attributes'), 'attributes');
  def(own(El, 'attributes'), 'attributes', {
    get() {
      const list = attrsDesc.get.call(this);
      const owner = this;
      const nodes = Array.from(list, (a) => new Attr(a.name, a.value, owner));
      const methods = { item: (i) => nodes[i] || null, getNamedItem: (n) => nodes.find((a) => a.name === String(n).toLowerCase()) || null, getNamedItemNS: (ns, n) => nodes.find((a) => a.name === String(n).toLowerCase()) || null, setNamedItem: (a) => owner.setAttributeNode(a), removeNamedItem: (n) => { const a = owner.getAttributeNode(n); if (!a) throw new g.DOMException('the attribute does not exist', 'NotFoundError'); owner.removeAttribute(n); return a; }, [Symbol.iterator]: () => nodes[Symbol.iterator]() };
      return new Proxy(nodes, {
        get: (t, p) => { if (p === 'length') return nodes.length; if (p in methods) return methods[p]; if (typeof p === 'string' && !(p in t)) { const a = methods.getNamedItem(p); if (a) return a; } return t[p]; },
        has: (t, p) => p in t || (typeof p === 'string' && !!methods.getNamedItem(p)),
      });
    },
    configurable: true,
  });

  // ------------------------------------------------------------------ canvas that takes drawing and draws nothing
  class CanvasRenderingContext2D {
    constructor(canvas) {
      hide(this, 'canvas', canvas);
      Object.assign(this, { fillStyle: '#000000', strokeStyle: '#000000', font: '10px sans-serif', textAlign: 'start', textBaseline: 'alphabetic', globalAlpha: 1, globalCompositeOperation: 'source-over', lineWidth: 1, lineCap: 'butt', lineJoin: 'miter', miterLimit: 10, shadowBlur: 0, shadowColor: 'rgba(0, 0, 0, 0)', shadowOffsetX: 0, shadowOffsetY: 0, imageSmoothingEnabled: true, direction: 'ltr', filter: 'none', lineDashOffset: 0 });
      hide(this, '_dash', []);
    }
    measureText(t) {
      const px = parseFloat(/([\d.]+)px/.exec(this.font) ? /([\d.]+)px/.exec(this.font)[1] : '10') || 10; const w = String(t).length * px * 0.5;
      return { width: w, actualBoundingBoxLeft: 0, actualBoundingBoxRight: w, actualBoundingBoxAscent: px * 0.8, actualBoundingBoxDescent: px * 0.2, fontBoundingBoxAscent: px * 0.9, fontBoundingBoxDescent: px * 0.25, emHeightAscent: px * 0.8, emHeightDescent: px * 0.2 };
    }
    getImageData(x, y, w, h) { w = Math.max(1, Math.floor(w)); h = Math.max(1, Math.floor(h)); return { width: w, height: h, data: new Uint8ClampedArray(Math.min(w * h * 4, 64 << 20)), colorSpace: 'srgb' }; }
    createImageData(w, h) { if (w && w.width) { h = w.height; w = w.width; } return this.getImageData(0, 0, w, h); }
    createLinearGradient() { return { addColorStop() {} }; } createRadialGradient() { return { addColorStop() {} }; } createConicGradient() { return { addColorStop() {} }; }
    createPattern() { return {}; }
    getLineDash() { return this._dash.slice(); } setLineDash(d) { this._dash = Array.from(d || []); }
    getTransform() { return new (g.DOMMatrix || Object)(); }
    isPointInPath() { return false; } isPointInStroke() { return false; }
  }
  for (const m of ['save', 'restore', 'beginPath', 'closePath', 'moveTo', 'lineTo', 'bezierCurveTo', 'quadraticCurveTo', 'arc', 'arcTo', 'ellipse', 'rect', 'roundRect', 'fill', 'stroke', 'clip', 'fillRect', 'strokeRect', 'clearRect', 'fillText', 'strokeText', 'drawImage', 'putImageData', 'scale', 'rotate', 'translate', 'transform', 'setTransform', 'resetTransform', 'drawFocusIfNeeded', 'reset']) def(CanvasRenderingContext2D.prototype, m, { writable: true, value() {} });
  g.CanvasRenderingContext2D = CanvasRenderingContext2D;
  const Canvas = g.HTMLCanvasElement && g.HTMLCanvasElement.prototype;
  if (Canvas) {
    def(Canvas, 'getContext', { writable: true, value(type) {
      if (String(type) !== '2d') return null;
      return this._ctx || (hide(this, '_ctx', new CanvasRenderingContext2D(this)), this._ctx);
    } });
    def(Canvas, 'toBlob', { writable: true, value(cb, type) { Promise.resolve().then(() => cb(new g.Blob([], { type: type || 'image/png' }))); } });
    for (const k of ['width', 'height']) if (!Object.getOwnPropertyDescriptor(Canvas, k)) def(Canvas, k, { get() { const v = parseInt(this.getAttribute(k), 10); return Number.isFinite(v) ? v : k === 'width' ? 300 : 150; }, set(v) { this.setAttribute(k, String(Math.trunc(v) || 0)); } });
  }

  // ------------------------------------------------------------------ object URLs
  if (g.URL && typeof g.URL.createObjectURL !== 'function') {
    const blobs = new Map(); let seq = 0;
    g.URL.createObjectURL = (b) => { const u = 'blob:' + g.location.origin + '/' + (++seq).toString(16).padStart(8, '0') + '-0000-4000-8000-000000000000'; blobs.set(u, b); return u; };
    g.URL.revokeObjectURL = (u) => { blobs.delete(String(u)); };
    hide(g, '__blobs', blobs);
  }

  // ------------------------------------------------------------------ small members
  const reflectStr = (proto, name, attr, dflt) => def(proto, name, { get() { const v = this.getAttribute(attr); return v === null ? dflt : v; }, set(v) { if (v === null || v === undefined) this.removeAttribute(attr); else this.setAttribute(attr, String(v)); } });
  const reflectBool = (proto, name, attr) => def(proto, name, { get() { return this.hasAttribute(attr); }, set(v) { if (v) this.setAttribute(attr, ''); else this.removeAttribute(attr); } });
  def(El, 'webkitMatchesSelector', { writable: true, value(s) { return this.matches(s); } });
  def(El, 'msMatchesSelector', { writable: true, value(s) { return this.matches(s); } });
  for (const k of ['Label', 'Hidden', 'Expanded', 'Checked', 'Selected', 'Pressed', 'Current', 'Disabled', 'Live', 'Busy', 'Controls', 'Describedby', 'Haspopup', 'Modal', 'Atomic', 'Relevant', 'Placeholder', 'Description', 'Valuenow']) reflectStr(El, 'aria' + k, 'aria-' + k.toLowerCase(), null);
  reflectStr(El, 'role', 'role', null); reflectStr(El, 'slot', 'slot', '');
  reflectStr(El, 'inputMode', 'inputmode', ''); reflectStr(El, 'enterKeyHint', 'enterkeyhint', ''); reflectStr(El, 'popover', 'popover', null);
  reflectStr(El, 'contentEditable', 'contenteditable', 'inherit');
  def(El, 'isContentEditable', { get() { for (let n = this; n && n.getAttribute; n = n.parentNode) { const v = n.getAttribute('contenteditable'); if (v !== null) return v === '' || v === 'true'; } return false; } });
  reflectBool(El, 'inert', 'inert');
  def(El, 'draggable', { get() { return this.getAttribute('draggable') === 'true'; }, set(v) { this.setAttribute('draggable', v ? 'true' : 'false'); } });
  def(El, 'spellcheck', { get() { return this.getAttribute('spellcheck') !== 'false'; }, set(v) { this.setAttribute('spellcheck', v ? 'true' : 'false'); } });
  def(El, 'part', { get() { const el = this, get = () => (el.getAttribute('part') || '').split(/\s+/).filter(Boolean); return { get length() { return get().length; }, contains: (c) => get().includes(String(c)), add(...c) { const a = get(); for (const x of c) if (!a.includes(String(x))) a.push(String(x)); el.setAttribute('part', a.join(' ')); }, remove(...c) { el.setAttribute('part', get().filter((x) => !c.map(String).includes(x)).join(' ')); }, toString: () => get().join(' '), [Symbol.iterator]: () => get()[Symbol.iterator]() }; } });
  def(El, 'assignedSlot', { get() { return null; } });
  def(El, 'getAnimations', { writable: true, value() { return []; } });
  for (const k of ['setPointerCapture', 'releasePointerCapture', 'showPopover', 'hidePopover']) def(El, k, { writable: true, value() {} });
  def(El, 'hasPointerCapture', { writable: true, value() { return false; } });
  def(El, 'togglePopover', { writable: true, value() { return false; } });
  def(El, 'attachInternals', { writable: true, value() { return { setFormValue() {}, setValidity() {}, checkValidity() { return true; }, reportValidity() { return true; }, form: null, labels: [], states: new Set(), shadowRoot: null }; } });
  const Dlg = g.HTMLDialogElement && g.HTMLDialogElement.prototype;
  if (Dlg) {
    def(Dlg, 'show', { writable: true, value() { this.setAttribute('open', ''); } });
    def(Dlg, 'showModal', { writable: true, value() { this.setAttribute('open', ''); } });
    def(Dlg, 'close', { writable: true, value(rv) { if (rv !== undefined) this.returnValue = String(rv); if (this.hasAttribute('open')) { this.removeAttribute('open'); this.dispatchEvent(new g.Event('close')); } } });
    def(Dlg, 'open', { get() { return this.hasAttribute('open'); }, set(v) { if (v) this.setAttribute('open', ''); else this.removeAttribute('open'); } });
  }
  const installMedia = (Med) => {
    def(Med, 'duration', { get() { return NaN; } });
    def(Med, 'currentTime', { get() { return this._t || 0; }, set(v) { hide(this, '_t', +v || 0); } });
    def(Med, 'buffered', { get() { return new g.TimeRanges(); } });
    def(Med, 'seekable', { get() { return new g.TimeRanges(); } });
    def(Med, 'played', { get() { return new g.TimeRanges(); } });
    def(Med, 'paused', { get() { return true; } });
    def(Med, 'ended', { get() { return false; } });
    def(Med, 'readyState', { get() { return 0; } });
    def(Med, 'networkState', { get() { return 3; } });
    def(Med, 'error', { get() { return null; } });
    def(Med, 'volume', { get() { return this._v === undefined ? 1 : this._v; }, set(v) { hide(this, '_v', +v); } });
    def(Med, 'playbackRate', { get() { return this._pr === undefined ? 1 : this._pr; }, set(v) { hide(this, '_pr', +v); } });
    def(Med, 'canPlayType', { writable: true, value() { return ''; } });
    reflectBool(Med, 'muted', 'muted'); reflectBool(Med, 'loop', 'loop'); reflectBool(Med, 'controls', 'controls'); reflectBool(Med, 'autoplay', 'autoplay');
  };
  { const base = g.HTMLMediaElement && g.HTMLMediaElement.prototype; for (const C of [g.HTMLMediaElement, g.HTMLVideoElement, g.HTMLAudioElement]) { const p = C && C.prototype; if (p && (p === base || !base || !base.isPrototypeOf(p))) installMedia(p); } }
  const Img = g.HTMLImageElement && g.HTMLImageElement.prototype;
  if (Img) def(Img, 'decode', { writable: true, value() { return Promise.resolve(); } });
  const Inp = g.HTMLInputElement && g.HTMLInputElement.prototype;
  if (Inp) {
    def(Inp, 'files', { get() { const a = []; a.item = (i) => a[i] || null; return a; } });
    def(Inp, 'labels', { get() { const out = []; const id = this.id; if (id) out.push(...doc.querySelectorAll('label[for="' + id + '"]')); const l = this.closest('label'); if (l && !out.includes(l)) out.push(l); out.item = (i) => out[i] || null; return out; } });
    def(Inp, 'selectionStart', { get() { return this._ss === undefined ? String(this.value).length : this._ss; }, set(v) { hide(this, '_ss', +v); } });
    def(Inp, 'selectionEnd', { get() { return this._se === undefined ? String(this.value).length : this._se; }, set(v) { hide(this, '_se', +v); } });
    def(Inp, 'setSelectionRange', { writable: true, value(a, b) { hide(this, '_ss', +a); hide(this, '_se', +b); } });
    def(Inp, 'validity', { get() { return { valid: true, valueMissing: false, typeMismatch: false, patternMismatch: false, tooLong: false, tooShort: false, rangeUnderflow: false, rangeOverflow: false, stepMismatch: false, badInput: false, customError: false }; } });
    def(Inp, 'willValidate', { get() { return !this.hasAttribute('disabled'); } });
    def(Inp, 'validationMessage', { get() { return ''; } });
    def(Inp, 'valueAsNumber', { get() { const v = parseFloat(this.value); return Number.isNaN(v) ? NaN : v; }, set(v) { this.value = String(v); } });
    def(Inp, 'valueAsDate', { get() { const t = Date.parse(this.value); return Number.isNaN(t) ? null : new Date(t); }, set(d) { this.value = d ? new Date(d).toISOString().slice(0, 10) : ''; } });
    def(Inp, 'stepUp', { writable: true, value(n) { this.value = String((parseFloat(this.value) || 0) + (n === undefined ? 1 : n) * (parseFloat(this.getAttribute('step')) || 1)); } });
    def(Inp, 'stepDown', { writable: true, value(n) { this.stepUp(-(n === undefined ? 1 : n)); } });
    def(Inp, 'showPicker', { writable: true, value() {} });
  }
  const Frm = g.HTMLFormElement && g.HTMLFormElement.prototype;
  if (Frm) def(Frm, 'requestSubmit', { writable: true, value(submitter) { const e = new g.Event('submit', { bubbles: true, cancelable: true }); e.submitter = submitter || null; if (this.dispatchEvent(e) !== false && !e.defaultPrevented) this.submit(); } });
  def(g.Document.prototype, 'timeline', { get() { return { currentTime: g.performance ? g.performance.now() : 0 }; } });
  def(g.Document.prototype, 'fullscreenElement', { get() { return null; } });
  def(g.Document.prototype, 'exitFullscreen', { writable: true, value() { return Promise.resolve(); } });
  def(g.Document.prototype, 'caretPositionFromPoint', { writable: true, value() { return null; } });
  def(g.Document.prototype, 'startViewTransition', { writable: true, value(cb) { const done = Promise.resolve().then(() => (typeof cb === 'function' ? cb() : undefined)); return { ready: Promise.resolve(), finished: done, updateCallbackDone: done, skipTransition() {} }; } });

  // navigator
  const nav = g.navigator;
  const addNav = (k, v) => { if (!(k in nav)) def(nav, k, { value: v, writable: true }); };
  addNav('storage', { estimate: () => Promise.resolve({ usage: 0, quota: 1 << 30 }), persist: () => Promise.resolve(false), persisted: () => Promise.resolve(false), getDirectory: () => Promise.reject(new g.DOMException('not available', 'NotSupportedError')) });
  addNav('connection', Object.assign(new g.EventTarget(), { effectiveType: '4g', downlink: 10, rtt: 50, saveData: false, type: 'unknown' }));
  const empty = () => { const a = []; a.item = () => null; a.namedItem = () => null; a.refresh = () => {}; return a; };
  addNav('plugins', empty()); addNav('mimeTypes', empty()); addNav('pdfViewerEnabled', false);
  addNav('credentials', { get: () => Promise.resolve(null), store: () => Promise.resolve(), create: () => Promise.resolve(null), preventSilentAccess: () => Promise.resolve() });
  addNav('userAgentData', { brands: [{ brand: 'Iyakki', version: '0' }], mobile: false, platform: 'Linux', getHighEntropyValues: (h) => Promise.resolve({ brands: [{ brand: 'Iyakki', version: '0' }], mobile: false, platform: 'Linux' }), toJSON() { return { brands: this.brands, mobile: false, platform: 'Linux' }; } });
  if (g.ServiceWorkerContainer) addNav('serviceWorker', new g.ServiceWorkerContainer());
  addNav('geolocation', { getCurrentPosition(ok, err) { if (typeof err === 'function') Promise.resolve().then(() => err({ code: 1, message: 'User denied Geolocation', PERMISSION_DENIED: 1 })); }, watchPosition(ok, err) { this.getCurrentPosition(ok, err); return 0; }, clearWatch() {} });

  addNav('locks', { request(name, a, b) { const cb = typeof a === 'function' ? a : b; return Promise.resolve().then(() => cb({ name: String(name), mode: 'exclusive' })); }, query: () => Promise.resolve({ held: [], pending: [] }) });
  g.MediaMetadata = class MediaMetadata { constructor(init) { Object.assign(this, { title: '', artist: '', album: '', artwork: [] }, init || {}); } };
  addNav('mediaSession', { metadata: null, playbackState: 'none', setActionHandler() {}, setPositionState() {}, setCameraActive() {}, setMicrophoneActive() {} });
  if (typeof g.ReportingObserver === 'undefined') g.ReportingObserver = class ReportingObserver { constructor(cb) { this._cb = cb; } observe() {} disconnect() {} takeRecords() { return []; } };
  if (g.CSSStyleSheet && !('cssRules' in g.CSSStyleSheet.prototype)) { def(g.CSSStyleSheet.prototype, 'cssRules', { get() { return this._rules || (hide(this, '_rules', []), this._rules); } }); def(g.CSSStyleSheet.prototype, 'rules', { get() { return this.cssRules; } }); }

  // Cache storage: a store that lives as long as the page does (like indexedDB's first cut), so `caches.open(...).then(c => c.put(...))`
  // and `match` work. Nothing is kept between visits.
  if (typeof g.Response === 'function' && typeof g.Request === 'function') {
    const stores = new Map();
    const keyOf = (r) => { try { return new URL(typeof r === 'string' ? r : r.url, g.location.href).href; } catch (e) { return String(typeof r === 'string' ? r : r.url); } };
    class MemCache {
      constructor() { hide(this, '_m', new Map()); }
      match(req) { const e = this._m.get(keyOf(req)); return Promise.resolve(e ? e.res.clone() : undefined); }
      matchAll(req) { const out = []; if (req === undefined) for (const e of this._m.values()) out.push(e.res.clone()); else { const e = this._m.get(keyOf(req)); if (e) out.push(e.res.clone()); } return Promise.resolve(out); }
      put(req, res) { this._m.set(keyOf(req), { req: typeof req === 'string' ? new g.Request(keyOf(req)) : req, res: res.clone() }); return Promise.resolve(); }
      add(req) { return g.fetch(req).then((r) => { if (!r.ok) throw new TypeError('bad response status'); return this.put(req, r); }); }
      addAll(reqs) { return Promise.all(Array.from(reqs, (r) => this.add(r))).then(() => undefined); }
      delete(req) { return Promise.resolve(this._m.delete(keyOf(req))); }
      keys(req) { return Promise.resolve(req === undefined ? Array.from(this._m.values(), (e) => e.req) : this._m.has(keyOf(req)) ? [this._m.get(keyOf(req)).req] : []); }
    }
    g.Cache = MemCache;
    g.caches = {
      open(name) { name = String(name); if (!stores.has(name)) stores.set(name, new MemCache()); return Promise.resolve(stores.get(name)); },
      has(name) { return Promise.resolve(stores.has(String(name))); },
      delete(name) { return Promise.resolve(stores.delete(String(name))); },
      keys() { return Promise.resolve(Array.from(stores.keys())); },
      match(req) { return (async () => { for (const c of stores.values()) { const r = await c.match(req); if (r) return r; } return undefined; })(); },
    };
  }

  // `Intl.DateTimeFormat()`, `Intl.NumberFormat()` and `Intl.Collator()` may be called without `new` (learn.microsoft.com does)
  if (g.Intl) for (const k of ['DateTimeFormat', 'NumberFormat', 'Collator']) {
    const C = g.Intl[k];
    if (typeof C === 'function') g.Intl[k] = new Proxy(C, { apply: (t, th, args) => new t(...args) });
  }

  // Intl.Locale
  if (g.Intl && g.Intl.Locale) {
    const L = g.Intl.Locale.prototype;
    for (const [k, v] of Object.entries({ maximize() { return this; }, minimize() { return this; }, getTextInfo() { return { direction: 'ltr' }; }, getWeekInfo() { return { firstDay: 1, weekend: [6, 7], minimalDays: 4 }; }, getCalendars() { return ['gregory']; }, getHourCycles() { return ['h23']; }, getNumberingSystems() { return ['latn']; }, getTimeZones() { return []; } })) if (!(k in L)) def(L, k, { writable: true, value: v });
    for (const [k, v] of Object.entries({ textInfo: { direction: 'ltr' }, calendar: 'gregory', numberingSystem: 'latn' })) if (!(k in L)) def(L, k, { get() { return v; } });
    if (!('region' in L)) def(L, 'region', { get() { return String(this.baseName).split('-')[1] || undefined; } });
    if (!('script' in L)) def(L, 'script', { get() { return undefined; } });
  }
})(globalThis);
