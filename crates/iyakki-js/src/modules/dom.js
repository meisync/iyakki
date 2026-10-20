(function (g) {
  'use strict';
  // The calls to the host go as JSON text. Building and reading that text is a large part of what a call costs, so the
  // hot calls below write their few arguments by hand and do not parse answers that are nothing or one number.
  const J = (r) => (r === 'null' ? null : JSON.parse(r));
  const H = (op, a) => J(__host('dom', op, a === undefined ? '{}' : JSON.stringify(a)));
  const q = JSON.stringify;
  // the same for the many calls whose only argument is a node
  const H1 = (op, n) => J(__host('dom', op, '{"n":' + n + '}'));
  // Bumped whenever the shape of the tree changes (insert, remove, innerHTML, textContent). What was asked about the
  // shape (parent, children, document parts) is remembered together with this number and is good until it moves on.
  let ver = 0;
  const newCache = () => ({ tn: undefined, pv: -1, p: null, kv: -1, k: null, ekv: -1, ek: null, am: undefined });
  const cc = (w) => w._c || (Object.defineProperty(w, '_c', { value: newCache(), writable: true, enumerable: false, configurable: true }), w._c);
  const wrappers = new Map();
  const hide = (o, k, v) => Object.defineProperty(o, k, { value: v, writable: true, enumerable: false, configurable: true });
  const list = (a) => { Object.defineProperty(a, 'item', { value: (i) => a[i] || null, enumerable: false }); return a; };

  // ------------------------------------------------------------------ events
  class Event {
    constructor(type, init) {
      init = init || {};
      this.type = String(type); this.bubbles = !!init.bubbles; this.cancelable = !!init.cancelable; this.composed = !!init.composed;
      this.defaultPrevented = false; this.target = null; this.currentTarget = null; this.eventPhase = 0;
      this.timeStamp = typeof performance !== 'undefined' ? performance.now() : 0; this.isTrusted = false;
      hide(this, '_stop', false); hide(this, '_stopNow', false);
    }
    preventDefault() { if (this.cancelable) this.defaultPrevented = true; }
    stopPropagation() { this._stop = true; }
    stopImmediatePropagation() { this._stop = true; this._stopNow = true; }
    initEvent(type, bubbles, cancelable) { this.type = String(type); this.bubbles = !!bubbles; this.cancelable = !!cancelable; this.defaultPrevented = false; }
    initCustomEvent(type, bubbles, cancelable, detail) { this.initEvent(type, bubbles, cancelable); this.detail = detail === undefined ? null : detail; }
    initUIEvent(type, bubbles, cancelable, view, detail) { this.initEvent(type, bubbles, cancelable); this.view = view || null; this.detail = detail || 0; }
    initMouseEvent(type, bubbles, cancelable, view, detail, sx, sy, cx, cy, ctrl, alt, shift, meta, button) { this.initUIEvent(type, bubbles, cancelable, view, detail); this.clientX = cx || 0; this.clientY = cy || 0; this.ctrlKey = !!ctrl; this.altKey = !!alt; this.shiftKey = !!shift; this.metaKey = !!meta; this.button = button || 0; }
    initKeyboardEvent(type, bubbles, cancelable, view, key) { this.initUIEvent(type, bubbles, cancelable, view, 0); this.key = key || ''; }
    composedPath() { return this._path || []; }
    get srcElement() { return this.target; }
    get returnValue() { return !this.defaultPrevented; }
  }
  Event.NONE = 0; Event.CAPTURING_PHASE = 1; Event.AT_TARGET = 2; Event.BUBBLING_PHASE = 3;
  class CustomEvent extends Event { constructor(t, i) { super(t, i); this.detail = i && i.detail !== undefined ? i.detail : null; } }
  class UIEvent extends Event { constructor(t, i) { super(t, i); this.detail = (i && i.detail) || 0; this.view = (i && i.view) || null; } }
  class MouseEvent extends UIEvent { constructor(t, i) { super(t, i); i = i || {}; this.clientX = i.clientX || 0; this.clientY = i.clientY || 0; this.button = i.button || 0; this.ctrlKey = !!i.ctrlKey; this.shiftKey = !!i.shiftKey; this.altKey = !!i.altKey; this.metaKey = !!i.metaKey; } }
  class KeyboardEvent extends UIEvent { constructor(t, i) { super(t, i); i = i || {}; this.key = i.key || ''; this.code = i.code || ''; this.ctrlKey = !!i.ctrlKey; this.shiftKey = !!i.shiftKey; this.altKey = !!i.altKey; this.metaKey = !!i.metaKey; } }
  class FocusEvent extends UIEvent {}
  class InputEvent extends UIEvent { constructor(t, i) { super(t, i); this.data = (i && i.data) || null; } }
  class ErrorEvent extends Event { constructor(t, i) { super(t, i); i = i || {}; this.message = i.message || ''; this.error = i.error || null; } }
  class MessageEvent extends Event { constructor(t, i) { super(t, i); this.data = i && i.data !== undefined ? i.data : null; } }
  class PopStateEvent extends Event { constructor(t, i) { super(t, i); this.state = i && i.state !== undefined ? i.state : null; } }

  class EventTarget {
    constructor() {}
    addEventListener(type, fn, opts) {
      if (this === undefined || this === null) return g.addEventListener(type, fn, opts);
      if (!fn) return;
      const cap = opts === true || !!(opts && opts.capture); const once = !!(opts && opts.once);
      const map = this._l || (hide(this, '_l', {}), this._l);
      const l = (map[type] = map[type] || []);
      if (!l.some((x) => x.fn === fn && x.cap === cap)) l.push({ fn, cap, once });
    }
    removeEventListener(type, fn, opts) {
      if (this === undefined || this === null) return g.removeEventListener(type, fn, opts);
      const cap = opts === true || !!(opts && opts.capture);
      if (!this._l) return;
      this._l[type] = (this._l[type] || []).filter((x) => !(x.fn === fn && x.cap === cap));
    }
    _parentTarget() { return null; }
    _invoke(ev, phase) {
      ev.currentTarget = this; ev.eventPhase = phase;
      const l = ((this._l && this._l[ev.type]) || []).slice();
      for (const x of l) {
        if (phase === 1 && !x.cap) continue;
        if (phase === 3 && x.cap) continue;
        if (x.once) this.removeEventListener(ev.type, x.fn, x.cap);
        try { typeof x.fn === 'function' ? x.fn.call(this, ev) : x.fn.handleEvent(ev); } catch (e) { report(e); }
        if (ev._stopNow) return;
      }
      if (phase !== 1) {
        const h = this['on' + ev.type];
        if (typeof h === 'function') { try { h.call(this, ev); } catch (e) { report(e); } }
      }
    }
    dispatchEvent(ev) {
      if (this === undefined || this === null) return g.dispatchEvent(ev);
      ev.target = this;
      const path = [];
      for (let t = this; t; t = t._parentTarget()) { path.push(t); if (path.length > 500) break; }
      ev._path = path.slice();
      for (let i = path.length - 1; i > 0 && !ev._stop; i--) path[i]._invoke(ev, 1);
      if (!ev._stop) path[0]._invoke(ev, 2);
      if (ev.bubbles) for (let i = 1; i < path.length && !ev._stop; i++) path[i]._invoke(ev, 3);
      ev.eventPhase = 0; ev.currentTarget = null;
      return !ev.defaultPrevented;
    }
  }
  class Window extends EventTarget {}
  const report = (e) => { try { __host('console', 'log', JSON.stringify({ level: 'error', text: 'Uncaught ' + String(e) + (e && e.stack ? '\n' + e.stack : '') })); } catch (_) {} };

  // ------------------------------------------------------------------ nodes
  const kebab = (s) => s.replace(/[A-Z]/g, (c) => '-' + c.toLowerCase());
  const camel = (s) => s.replace(/-([a-z])/g, (_, c) => c.toUpperCase());
  const VOID = new Set(['br', 'hr', 'img', 'input', 'meta', 'link', 'area', 'base', 'col', 'embed', 'source', 'track', 'wbr', 'param']);

  class Node extends EventTarget {
    constructor(id) {
      super();
      if (id === undefined) {
        // `new MyElement()` from a class registered with customElements.define: the element being upgraded, or a new one
        let el = g.__upgrading; g.__upgrading = null;
        if (!el) { const tag = ceTagFor(new.target); if (!tag) throw new TypeError('Illegal constructor'); el = wrap(+__host('dom', 'create', '{"tag":' + q(tag) + '}'), tag); }
        Object.setPrototypeOf(el, new.target.prototype);
        return el;
      }
      hide(this, '_id', id);
      hide(this, '_c', newCache());
    }
    get nodeType() { return this._type; }
    get nodeName() { return this._name; }
    get nodeValue() { return this._type === 3 ? this.data : null; }
    set nodeValue(v) {}
    get ownerDocument() { return this === doc ? null : doc; }
    get parentNode() { const c = cc(this); if (c.pv !== ver) { c.p = H1('parent', this._id); c.pv = ver; } return c.p === null ? null : wrap(c.p[0], c.p[1]); }
    get parentElement() { const p = this.parentNode; return p && p !== doc ? p : null; }
    _k() { const c = cc(this); if (c.kv !== ver) { c.k = H1('kids', this._id); c.kv = ver; } return c.k; }
    get childNodes() { return list(wrapPairs(this._k())); }
    get firstChild() { const k = this._k(); return k.length ? wrap(k[0], k[1]) : null; }
    get lastChild() { const k = this._k(); return k.length ? wrap(k[k.length - 2], k[k.length - 1]) : null; }
    _sib(d) { const r = JSON.parse(__host('dom', 'sib', '{"n":' + this._id + ',"d":' + d + '}')); return r === null ? null : wrap(r[0], r[1]); }
    get nextSibling() { return this._sib(1); }
    get previousSibling() { return this._sib(-1); }
    get isConnected() { return H1('connected', this._id); }
    get textContent() { return H('text_get', { n: this._id }); }
    set textContent(v) { __host('dom', 'text_set', '{"n":' + this._id + ',"text":' + q(v === null ? '' : String(v)) + '}'); ver++; }
    hasChildNodes() { return this._k().length > 0; }
    contains(o) { return o instanceof Node ? JSON.parse(__host('dom', 'contains', '{"n":' + this._id + ',"o":' + o._id + '}')) : false; }
    _insert(child, before) {
      if (child && child._type === 11) { for (const k of child.childNodes) this._insert(k, before); return child; }
      if (!(child instanceof Node)) throw new TypeError('the argument is not a Node');
      const r = J(__host('dom', 'insert', '{"p":' + this._id + ',"c":' + child._id + ',"r":' + (before ? before._id : 'null') + '}'));
      ver++;
      afterInsert(child, r);
      return child;
    }
    appendChild(c) { return this._insert(c, null); }
    insertBefore(c, ref) { return this._insert(c, ref || null); }
    removeChild(c) { if (c.parentNode !== this) throw new Error('NotFoundError: the node is not a child of this node'); __host('dom', 'remove', '{"c":' + c._id + '}'); ver++; return c; }
    replaceChild(n, old) { this.insertBefore(n, old); this.removeChild(old); return old; }
    cloneNode(deep) { const r = H('clone', { n: this._id, deep: !!deep }); return wrap(r[0], r[1]); }
    normalize() {}
    getRootNode() { let n = this; while (n.parentNode) n = n.parentNode; return n; }
    compareDocumentPosition() { return 0; }
    isEqualNode(o) { return this === o; }
    isSameNode(o) { return this === o; }
    _parentTarget() { return this.parentNode; }
  }
  Node.ELEMENT_NODE = 1; Node.TEXT_NODE = 3; Node.COMMENT_NODE = 8; Node.DOCUMENT_NODE = 9; Node.DOCUMENT_FRAGMENT_NODE = 11;

  class CharacterData extends Node {
    get data() { return H('text_get', { n: this._id }); }
    set data(v) { __host('dom', 'text_set', '{"n":' + this._id + ',"text":' + q(String(v)) + '}'); }
    get length() { return this.data.length; }
    get nodeValue() { return this.data; }
    set nodeValue(v) { this.data = v; }
    substringData(o, n) { return this.data.substr(o, n); }
    appendData(t) { this.data = this.data + t; }
    insertData(o, t) { const d = this.data; this.data = d.slice(0, o) + t + d.slice(o); }
    deleteData(o, n) { const d = this.data; this.data = d.slice(0, o) + d.slice(o + n); }
    replaceData(o, n, t) { const d = this.data; this.data = d.slice(0, o) + t + d.slice(o + n); }
  }
  class Text extends CharacterData {
    get _type() { return 3; }
    get _name() { return '#text'; }
    get data() { return H('text_get', { n: this._id }); }
    set data(v) { __host('dom', 'text_set', '{"n":' + this._id + ',"text":' + q(String(v)) + '}'); }
    get length() { return this.data.length; }
    get wholeText() { return this.data; }
    get nodeValue() { return this.data; }
    set nodeValue(v) { this.data = v; }
    splitText() { return null; }
  }
  class Comment extends CharacterData { get _type() { return 8; } get _name() { return '#comment'; } }

  const REFLECT = ['href', 'src', 'alt', 'title', 'type', 'name', 'placeholder', 'rel', 'target', 'action', 'method', 'lang', 'dir', 'role', 'for', 'content', 'charset', 'media', 'srcset', 'sizes', 'accept', 'autocomplete', 'download', 'integrity', 'crossorigin', 'loading', 'id'];
  const BOOLS = ['disabled', 'checked', 'selected', 'hidden', 'readonly', 'required', 'multiple', 'autofocus', 'async', 'defer', 'open', 'controls', 'loop', 'muted', 'autoplay', 'novalidate'];
  const NUMS = ['width', 'height', 'tabindex', 'colspan', 'rowspan', 'maxlength', 'size', 'min', 'max', 'step'];

  class Element extends Node {
    get _type() { return 1; }
    get _name() { return this.tagName; }
    _tag() { const c = cc(this); if (c.tn === undefined) c.tn = H1('info', this._id).name; return c.tn; }
    get tagName() { return this._tag().toUpperCase(); }
    get localName() { return this._tag(); }
    get nodeName() { return this.tagName; }
    _attrs() { const c = cc(this); return c.am || (c.am = new Map(H1('attrs', this._id))); }
    getAttribute(k) { const v = this._attrs().get(String(k).toLowerCase()); return v === undefined ? null : v; }
    setAttribute(k, v) {
      k = String(k); v = String(v);
      const m = this._attrs(); const lk = k.toLowerCase();
      const old = m.has(lk) ? m.get(lk) : null;
      __host('dom', 'setattr', '{"n":' + this._id + ',"k":' + q(k) + ',"v":' + q(v) + '}');
      if (m.has(lk) || m.size < 128) m.set(lk, v);
      if (ce.size && this._ceUp) { const C = ce.get(this.localName); if (C && C.ctor.observedAttributes && C.ctor.observedAttributes.includes(k) && typeof this.attributeChangedCallback === 'function') { try { this.attributeChangedCallback(k, old, v); } catch (e) { report(e); } } }
    }
    removeAttribute(k) { k = String(k); __host('dom', 'rmattr', '{"n":' + this._id + ',"k":' + q(k) + '}'); const m = cc(this).am; if (m) m.delete(k.toLowerCase()); }
    hasAttribute(k) { return this._attrs().has(String(k).toLowerCase()); }
    toggleAttribute(k, force) { const has = this.hasAttribute(k); const want = force === undefined ? !has : !!force; if (want && !has) this.setAttribute(k, ''); if (!want && has) this.removeAttribute(k); return want; }
    getAttributeNames() { return [...this._attrs().keys()]; }
    hasAttributes() { return this._attrs().size > 0; }
    get attributes() {
      const self = this;
      const a = list([...this._attrs()].map(([name, value]) => ({ name, value, nodeName: name, nodeValue: value, localName: name, namespaceURI: null, prefix: null, specified: true, ownerElement: self })));
      const get = (n) => a.find((x) => x.name === String(n).toLowerCase()) || null;
      // the NamedNodeMap methods
      for (const [k, v] of Object.entries({
        getNamedItem: get, getNamedItemNS: (ns, n) => get(n),
        setNamedItem(at) { const old = get(at.name); self.setAttribute(at.name, at.value); return old; },
        removeNamedItem(n) { const old = get(n); if (!old) throw new DOMException('the attribute does not exist', 'NotFoundError'); self.removeAttribute(n); return old; },
      })) Object.defineProperty(a, k, { value: v, enumerable: false });
      return a;
    }
    get id() { return this.getAttribute('id') || ''; }
    set id(v) { this.setAttribute('id', v); }
    get className() { return this.getAttribute('class') || ''; }
    set className(v) { this.setAttribute('class', v); }
    set classList(v) { this.className = String(v); }
    get classList() {
      const el = this; const get = () => (el.getAttribute('class') || '').split(/\s+/).filter(Boolean); const set = (a) => el.setAttribute('class', a.join(' '));
      return { get length() { return get().length; }, item: (i) => get()[i] || null, contains: (c) => get().includes(String(c)), add(...c) { const a = get(); for (const x of c) if (!a.includes(String(x))) a.push(String(x)); set(a); }, remove(...c) { set(get().filter((x) => !c.map(String).includes(x))); }, toggle(c, f) { c = String(c); const has = get().includes(c); const want = f === undefined ? !has : !!f; if (want && !has) this.add(c); if (!want && has) this.remove(c); return want; }, replace(a, b) { const l = get(); const i = l.indexOf(String(a)); if (i < 0) return false; l[i] = String(b); set(l); return true; }, toString: () => el.className, forEach: (f) => get().forEach(f), [Symbol.iterator]: () => get()[Symbol.iterator](), get value() { return el.className; } };
    }
    get dataset() {
      const el = this;
      return new Proxy({}, {
        get: (t, p) => { if (typeof p !== 'string') return undefined; const v = el.getAttribute('data-' + kebab(p)); return v === null ? undefined : v; },
        set: (t, p, v) => { el.setAttribute('data-' + kebab(String(p)), String(v)); return true; },
        deleteProperty: (t, p) => { el.removeAttribute('data-' + kebab(String(p))); return true; },
        has: (t, p) => typeof p === 'string' && el.hasAttribute('data-' + kebab(p)),
        ownKeys: () => el.getAttributeNames().filter((n) => n.startsWith('data-')).map((n) => camel(n.slice(5))),
        getOwnPropertyDescriptor: (t, p) => { const v = typeof p === 'string' ? el.getAttribute('data-' + kebab(p)) : null; return v === null ? undefined : { value: v, enumerable: true, configurable: true, writable: true }; },
      });
    }
    set style(v) { this.style.cssText = String(v); }
    get style() {
      const el = this;
      const read = () => { const m = new Map(); for (const part of (el.getAttribute('style') || '').split(';')) { const i = part.indexOf(':'); if (i > 0) m.set(part.slice(0, i).trim().toLowerCase(), part.slice(i + 1).trim()); } return m; };
      const write = (m) => { const s = [...m].map(([k, v]) => k + ': ' + v).join('; '); if (s) el.setAttribute('style', s); else el.removeAttribute('style'); };
      const api = { getPropertyValue: (k) => read().get(String(k).toLowerCase()) || '', setProperty(k, v) { const m = read(); if (v === '' || v == null) m.delete(String(k).toLowerCase()); else m.set(String(k).toLowerCase(), String(v)); write(m); }, removeProperty(k) { const m = read(); const old = m.get(String(k).toLowerCase()) || ''; m.delete(String(k).toLowerCase()); write(m); return old; }, get cssText() { return el.getAttribute('style') || ''; }, set cssText(v) { if (String(v).trim()) el.setAttribute('style', String(v)); else el.removeAttribute('style'); }, get length() { return read().size; }, item: (i) => [...read().keys()][i] || '' };
      return new Proxy(api, {
        get: (t, p) => { if (p in t) return t[p]; if (typeof p !== 'string') return undefined; return read().get(kebab(p)) || ''; },
        set: (t, p, v) => { if (p === 'cssText') { t.cssText = v; return true; } if (typeof p !== 'string') return false; t.setProperty(kebab(p), v); return true; },
      });
    }
    _ek() { const c = cc(this); if (c.ekv !== ver) { c.ek = H1('elkids', this._id); c.ekv = ver; } return c.ek; }
    get children() { return list(wrapPairs(this._ek())); }
    get childElementCount() { return this._ek().length / 2; }
    get firstElementChild() { const k = this._ek(); return k.length ? wrap(k[0], k[1]) : null; }
    get lastElementChild() { const k = this._ek(); return k.length ? wrap(k[k.length - 2], k[k.length - 1]) : null; }
    get nextElementSibling() { const r = H('sibling', { n: this._id, d: 1 }); return r === null ? null : wrap(r[0], r[1]); }
    get previousElementSibling() { const r = H('sibling', { n: this._id, d: -1 }); return r === null ? null : wrap(r[0], r[1]); }
    get innerHTML() { return H('html_get', { n: this._id, outer: false }); }
    set innerHTML(v) { if (ce.size && this.isConnected) ceDisconnect(this); H('html_set', { n: this._id, html: String(v) }); ver++; ceUpgradeIn(this); }
    get outerHTML() { return H('html_get', { n: this._id, outer: true }); }
    get innerText() { return this.textContent; }
    set innerText(v) { this.textContent = v; }
    querySelector(s) { const r = H('query', { n: this._id, sel: String(s), all: false }); return r.length ? wrap(r[0], r[1]) : null; }
    querySelectorAll(s) { return list(wrapPairs(H('query', { n: this._id, sel: String(s), all: true }))); }
    getElementsByTagName(t) { return list(wrapPairs(H('query', { n: this._id, sel: String(t) || '*', all: true }))); }
    getElementsByClassName(c) { return list(wrapPairs(H('query', { n: this._id, sel: '.' + String(c).trim().split(/\s+/).join('.'), all: true }))); }
    matches(s) { return H('matches', { n: this._id, sel: String(s) }); }
    closest(s) { const r = H('closest', { n: this._id, sel: String(s) }); return r === null ? null : wrap(r[0], r[1]); }
    append(...n) { for (const x of n) this.appendChild(typeof x === 'string' ? doc.createTextNode(x) : x); }
    prepend(...n) { const first = this.firstChild; for (const x of n) this.insertBefore(typeof x === 'string' ? doc.createTextNode(x) : x, first); }
    before(...n) { const p = this.parentNode; if (p) for (const x of n) p.insertBefore(typeof x === 'string' ? doc.createTextNode(x) : x, this); }
    after(...n) { const p = this.parentNode; if (!p) return; const next = this.nextSibling; for (const x of n) p.insertBefore(typeof x === 'string' ? doc.createTextNode(x) : x, next); }
    remove() { const p = this.parentNode; if (p) p.removeChild(this); }
    replaceWith(...n) { this.before(...n); this.remove(); }
    replaceChildren(...n) { this.textContent = ''; this.append(...n); }
    insertAdjacentHTML(pos, html) {
      const holder = doc.createElement('div'); holder.innerHTML = html; const kids = holder.childNodes;
      const p = this.parentNode; pos = String(pos).toLowerCase();
      if (pos === 'beforeend') for (const k of kids) this.appendChild(k);
      else if (pos === 'afterbegin') { const f = this.firstChild; for (const k of kids) this.insertBefore(k, f); }
      else if (pos === 'beforebegin' && p) for (const k of kids) p.insertBefore(k, this);
      else if (pos === 'afterend' && p) { const n = this.nextSibling; for (const k of kids) p.insertBefore(k, n); }
    }
    insertAdjacentElement(pos, el) { const h = doc.createElement('div'); h.appendChild(el); const html = h.innerHTML; this.insertAdjacentHTML(pos, html); return el; }
    insertAdjacentText(pos, t) { this.insertAdjacentHTML(pos, String(t).replace(/&/g, '&amp;').replace(/</g, '&lt;')); }
    focus() { doc._active = this; this.dispatchEvent(new FocusEvent('focus')); }
    blur() { this.dispatchEvent(new FocusEvent('blur')); }
    click() { this.dispatchEvent(new MouseEvent('click', { bubbles: true, cancelable: true })); }
    scrollIntoView() {} scrollTo() {} scroll() {} select() {} showModal() {} close() {} requestFullscreen() { return Promise.resolve(); }
    getBoundingClientRect() { return { x: 0, y: 0, width: 0, height: 0, top: 0, left: 0, right: 0, bottom: 0, toJSON() { return this; } }; }
    getClientRects() { return list([]); }
    get offsetWidth() { return 0; } get offsetHeight() { return 0; } get offsetTop() { return 0; } get offsetLeft() { return 0; }
    get clientWidth() { return 0; } get clientHeight() { return 0; } get scrollWidth() { return 0; } get scrollHeight() { return 0; }
    get scrollTop() { return 0; } set scrollTop(v) {} get scrollLeft() { return 0; } set scrollLeft(v) {}
    get offsetParent() { return null; }
    get isContentEditable() { return false; }
    animate() { return { finished: Promise.resolve(), cancel() {}, play() {}, pause() {}, addEventListener() {} }; }
    attachShadow(init) {
      if (this._shadow) throw new DOMException('a shadow root is already attached', 'NotSupportedError');
      const id = H('create', { tag: '#fragment' });
      const root = new classes.ShadowRoot(id);
      wrappers.set(id, root);
      hide(root, 'host', this);
      hide(root, 'mode', init && init.mode === 'closed' ? 'closed' : 'open');
      hide(this, '_shadow', root);
      this.setAttribute('data-iyakki-shadow', String(id));
      return root;
    }
    get shadowRoot() { return this._shadow && this._shadow.mode === 'open' ? this._shadow : null; }
    get value() { const v = this.getAttribute('value'); return v === null ? (this.localName === 'textarea' ? this.textContent : '') : v; }
    set value(v) { this.setAttribute('value', v); }
    get form() { return this.closest('form'); }
    get options() { return this.querySelectorAll('option'); }
    get selectedIndex() { return this.options.findIndex((o) => o.hasAttribute('selected')); }
    set selectedIndex(i) { this.options.forEach((o, j) => { if (j === i) o.setAttribute('selected', ''); else o.removeAttribute('selected'); }); }
    get text() { return this.textContent; }
    set text(v) { this.textContent = v; }
    get currentSrc() { return this.getAttribute('src') || ''; }
    get complete() { return true; }
    get naturalWidth() { return 0; } get naturalHeight() { return 0; }
    get sheet() { return null; }
    get rows() { return this.querySelectorAll('tr'); }
    getContext() { return null; }
    toDataURL() { return 'data:,'; }
    play() { return Promise.resolve(); } pause() {} load() {}
    submit() {} reset() {} checkValidity() { return true; } reportValidity() { return true; } setCustomValidity() {}
    get elements() { return this.querySelectorAll('input,select,textarea,button'); }
  }
  for (const k of REFLECT) if (k !== 'id') Object.defineProperty(Element.prototype, k === 'for' ? 'htmlFor' : k, { get() { return this.getAttribute(k) || ''; }, set(v) { this.setAttribute(k, v); }, configurable: true });
  for (const k of BOOLS) Object.defineProperty(Element.prototype, k === 'readonly' ? 'readOnly' : k, { get() { return this.hasAttribute(k); }, set(v) { if (this._id === undefined) { Object.defineProperty(this, k === 'readonly' ? 'readOnly' : k, { value: v, writable: true, configurable: true, enumerable: true }); return; } v ? this.setAttribute(k, '') : this.removeAttribute(k); }, configurable: true });
  for (const k of NUMS) Object.defineProperty(Element.prototype, k === 'colspan' ? 'colSpan' : k === 'rowspan' ? 'rowSpan' : k === 'tabindex' ? 'tabIndex' : k === 'maxlength' ? 'maxLength' : k, { get() { const v = parseInt(this.getAttribute(k), 10); return Number.isNaN(v) ? 0 : v; }, set(v) { this.setAttribute(k, String(v)); }, configurable: true });
  Object.defineProperty(Element.prototype, 'href', { get() { const h = this.getAttribute('href'); if (h === null) return ''; try { return new URL(h, g.location.href).href; } catch (e) { return h; } }, set(v) { this.setAttribute('href', v); }, configurable: true });
  // the URL parts of links (<a>, <area>): `a.pathname`, `a.search`, ...
  for (const part of ['protocol', 'host', 'hostname', 'port', 'pathname', 'search', 'hash', 'origin', 'username', 'password']) {
    Object.defineProperty(Element.prototype, part, {
      get() {
        if (this.localName !== 'a' && this.localName !== 'area') return undefined;
        const h = this.getAttribute('href');
        if (h === null) return '';
        try { return new URL(h, g.location.href)[part]; } catch (e) { return ''; }
      },
      set(v) {
        // only links have URL parts; on any other object (a ShadowRoot polyfill sets `this.host`) it is a plain property
        if (this.localName !== 'a' && this.localName !== 'area') { Object.defineProperty(this, part, { value: v, writable: true, configurable: true, enumerable: true }); return; }
        try { const u = new URL(this.getAttribute('href') || '', g.location.href); u[part] = v; this.setAttribute('href', u.href); } catch (e) {}
      },
      configurable: true,
    });
  }
  Object.defineProperty(Element.prototype, 'src', { get() { const h = this.getAttribute('src'); if (h === null) return ''; try { return new URL(h, g.location.href).href; } catch (e) { return h; } }, set(v) { this.setAttribute('src', v); if (this.localName === 'script') afterInsert(this); }, configurable: true });
  Object.defineProperty(Element.prototype, 'checked', { get() { return this.hasAttribute('checked'); }, set(v) { v ? this.setAttribute('checked', '') : this.removeAttribute('checked'); }, configurable: true });

  class HTMLElement extends Element {}
  // custom elements: classes registered with customElements.define are run on the elements that carry their tag
  const ce = new Map();
  const ceByCtor = new Map();
  const ceTagFor = (C) => ceByCtor.get(C) || null;
  // the custom elements in a subtree (the root first): the host lists elements with a dash in their name, which is cheap
  const ceUnder = (root) => { const r = H1('dashed', root._id); const out = []; for (let i = 0; i < r.length; i += 2) if (ce.has(r[i + 1])) out.push(wrap(r[i], r[i + 1])); if (ce.has(root.localName)) out.unshift(root); return out; };
  const upgrade = (el, name) => {
    const d = ce.get(name);
    if (!d || el._ceDone) return;
    hide(el, '_ceDone', true);
    g.__upgrading = el;
    try { new d.ctor(); } catch (e) { report(e); g.__upgrading = null; return; }
    if (el.isConnected) ceConnect(el);
  };
  const ceConnect = (el) => { if (el._ceUp && !el._ceConnected) { hide(el, '_ceConnected', true); try { if (typeof el.connectedCallback === 'function') el.connectedCallback(); } catch (e) { report(e); } } };
  const ceUpgradeIn = (root) => {
    if (!ce.size || !(root instanceof Element)) return;
    const list = ceUnder(root);
    for (const el of list) { upgrade(el, el.localName); hide(el, '_ceUp', true); if (el.isConnected) ceConnect(el); }
  };
  const ceDisconnect = (root) => {
    if (!ce.size || !(root instanceof Element)) return;
    const list = ceUnder(root);
    for (const el of list) if (el._ceConnected) { hide(el, '_ceConnected', false); try { if (typeof el.disconnectedCallback === 'function') el.disconnectedCallback(); } catch (e) { report(e); } }
  };
  Object.defineProperty(Element.prototype, 'content', { get() {
    if (this.localName !== 'template') return undefined;
    if (!this._content) { const f = doc.createDocumentFragment(); for (const k of [...this.childNodes]) f.appendChild(k); hide(this, '_content', f); }
    return this._content;
  }, configurable: true });
  Object.defineProperty(Element.prototype, 'adoptedStyleSheets', { get() { return this._adopted || (hide(this, '_adopted', []), this._adopted); }, set(v) { hide(this, '_adopted', Array.from(v)); }, configurable: true });
  const classes = { CharacterData, HTMLElement, Element, Node, Text, Comment, EventTarget, Event, CustomEvent, UIEvent, MouseEvent, KeyboardEvent, FocusEvent, InputEvent, ErrorEvent, MessageEvent, PopStateEvent };
  for (const n of ['Anchor', 'Image', 'Input', 'Script', 'Style', 'Link', 'Form', 'Div', 'Span', 'Button', 'Select', 'TextArea', 'Canvas', 'Body', 'Head', 'Html', 'Table', 'Paragraph', 'Heading', 'IFrame', 'Template', 'Media', 'Video', 'Audio', 'UList', 'OList', 'LI', 'Label', 'Option']) classes['HTML' + n + 'Element'] = class extends HTMLElement {};
  class DocumentFragment extends Element { get _type() { return 11; } get _name() { return '#document-fragment'; } }
  classes.DocumentFragment = DocumentFragment;

  const tagClass = Object.create(null);
  const wrap = (id, tag) => {
    if (id === null || id === undefined) return null;
    let w = wrappers.get(id);
    if (w) return w;
    if (id === 0) return doc;
    let name = tag;
    if (name === undefined) { const info = H1('info', id); name = info.t === 3 ? '' : info.name; }
    w = name === '' ? new Text(id) : name === '#fragment' ? new DocumentFragment(id) : new (tagClass[name] || HTMLElement)(id);
    if (name !== '') w._c.tn = name;
    wrappers.set(id, w);
    return w;
  };
  // [id, tag, id, tag, ...] as the host sends lists of nodes
  const wrapPairs = (r) => { const out = new Array(r.length >> 1); for (let i = 0; i < r.length; i += 2) out[i >> 1] = wrap(r[i], r[i + 1]); return out; };

  // scripts added by other scripts run when they are attached
  const run = (code, name) => { try { __eval(String(code), String(name)); } catch (e) { report(e); } };
  const afterInsert = (child, found) => {
    if (!(child instanceof Element)) return;
    // `found`: what the host said about an insert (null: the node is not in the page; else the scripts that came with it)
    if (found === undefined) { if (!child.isConnected) return; found = H('scripts', { n: child._id }); }
    else if (found === null) return;
    ceUpgradeIn(child);
    const scripts = found.map((id) => wrap(id, 'script'));
    for (const s of scripts) {
      if (!H('claim_script', { n: s._id })) continue;
      const type = (s.getAttribute('type') || '').toLowerCase();
      if (type && !/^(text|application)\/(x-)?(java|ecma)script$/.test(type)) continue;
      const src = s.getAttribute('src');
      if (src) {
        try { const r = JSON.parse(__host('fetch', 'get', JSON.stringify({ url: src, method: 'GET' }))); if (r.status < 400) run(r.body, src); } catch (e) { report(e); s.dispatchEvent(new Event('error')); continue; }
        s.dispatchEvent(new Event('load'));
      } else run(s.textContent, 'inline');
    }
  };

  // ------------------------------------------------------------------ the document
  let docV = -1; let docC = null;
  class Document extends Node {
    constructor() { super(0); hide(this, '_ready', 'loading'); hide(this, '_active', null); }
    get _type() { return 9; }
    get _name() { return '#document'; }
    get ownerDocument() { return null; }
    _doc() { if (docV !== ver) { docC = H('doc'); docV = ver; } return docC; }
    get documentElement() { const d = this._doc(); return d.html === null ? null : wrap(d.html, 'html'); }
    get head() { const d = this._doc(); return d.head === null ? null : wrap(d.head, 'head'); }
    get body() { const d = this._doc(); return d.body === null ? null : wrap(d.body, 'body'); }
    set body(b) { const old = this.body; if (old) old.parentNode.replaceChild(b, old); }
    get title() { const t = this.querySelector('title'); return t ? t.textContent.trim().replace(/\s+/g, ' ') : ''; }
    set title(v) { let t = this.querySelector('title'); if (!t) { t = this.createElement('title'); const h = this.head; if (h) h.appendChild(t); } t.textContent = String(v); }
    createElement(tag) { tag = String(tag); const el = wrap(+__host('dom', 'create', '{"tag":' + q(tag) + '}'), tag.toLowerCase()); el._c.am = new Map(); const n = el.localName; if (ce.size && ce.has(n)) { upgrade(el, n); hide(el, '_ceUp', true); } return el; }
    createElementNS(ns, tag) { return this.createElement(tag); }
    createTextNode(t) { return wrap(+__host('dom', 'text', '{"text":' + q(String(t)) + '}'), ''); }
    createComment(t) { const c = wrap(H('text', { text: '' }), ''); return c; }
    createDocumentFragment() { return wrap(H('create', { tag: '#fragment' }), '#fragment'); }
    createEvent(t) {
      const k = String(t).toLowerCase().replace(/s$/, '');
      const C = { event: Event, htmlevent: Event, customevent: CustomEvent, uievent: UIEvent, mouseevent: MouseEvent, keyboardevent: KeyboardEvent, focusevent: FocusEvent, messageevent: MessageEvent }[k];
      if (!C) throw new DOMException('The provided event type (' + t + ') is invalid.', 'NotSupportedError');
      const e = new C(''); e.type = ''; return e;
    }
    createRange() { return new classes.Range(); }
    createTreeWalker(root, what, filter) { return new classes.TreeWalker(root, what, filter); }
    createNodeIterator(root, what, filter) { return new classes.NodeIterator(root, what, filter); }
    get implementation() { return classes.__impl || (classes.__impl = new classes.DOMImplementation()); }
    importNode(n, deep) { return n.cloneNode(deep); }
    adoptNode(n) { return n; }
    getElementById(id) { const r = H('by_id', { id: String(id) }); return r === null ? null : wrap(r[0], r[1]); }
    querySelector(s) { const r = H('query', { n: 0, sel: String(s), all: false }); return r.length ? wrap(r[0], r[1]) : null; }
    querySelectorAll(s) { return list(wrapPairs(H('query', { n: 0, sel: String(s), all: true }))); }
    getElementsByTagName(t) { return list(wrapPairs(H('query', { n: 0, sel: String(t) || '*', all: true }))); }
    getElementsByClassName(c) { return list(wrapPairs(H('query', { n: 0, sel: '.' + String(c).trim().split(/\s+/).join('.'), all: true }))); }
    getElementsByName(n) { return list(wrapPairs(H('query', { n: 0, sel: '[name="' + String(n).replace(/"/g, '') + '"]', all: true }))); }
    get forms() { return this.querySelectorAll('form'); }
    get images() { return this.querySelectorAll('img'); }
    get links() { return this.querySelectorAll('a[href],area[href]'); }
    get scripts() { return this.querySelectorAll('script'); }
    get styleSheets() { return list([]); }
    get readyState() { return this._ready; }
    get URL() { return g.location.href; }
    get documentURI() { return g.location.href; }
    get baseURI() { return g.location.href; }
    get referrer() { return ''; }
    get domain() { return g.location.hostname; }
    get characterSet() { return 'UTF-8'; }
    get charset() { return 'UTF-8'; }
    get contentType() { return 'text/html'; }
    get compatMode() { return 'CSS1Compat'; }
    get visibilityState() { return 'visible'; }
    get hidden() { return false; }
    get defaultView() { return g; }
    get location() { return g.location; }
    get activeElement() { return this._active || this.body; }
    get currentScript() { return this._cur || null; }
    get fonts() { return { ready: Promise.resolve(), load: () => Promise.resolve([]), check: () => true, addEventListener() {} }; }
    get cookie() { return typeof __cookies === 'object' ? __cookies.get() : ''; }
    set cookie(v) { if (typeof __cookies === 'object') __cookies.set(v); }
    hasFocus() { return true; }
    elementFromPoint() { return null; }
    getSelection() { return { rangeCount: 0, removeAllRanges() {}, addRange() {}, toString: () => '' }; }
    execCommand() { return false; }
    write() { console.warn('document.write is not supported'); }
    writeln() { this.write(); }
    open() { return this; }
    close() {}
    _parentTarget() { return g; }
    get nodeName() { return '#document'; }
  }
  const doc = new Document();
  Object.defineProperty(Document.prototype, 'adoptedStyleSheets', { get() { return this._adopted || (hide(this, '_adopted', []), this._adopted); }, set(v) { hide(this, '_adopted', Array.from(v)); }, configurable: true });
  wrappers.set(0, doc);

  // the window is an event target: its prototype chain is Window.prototype, EventTarget.prototype, Object.prototype
  Object.setPrototypeOf(g, Window.prototype);
  hide(g, '_l', {});
  // the document's chain ends at the window
  Document.prototype._parentTarget = function () { return g; };

  class MutationObserver { constructor(cb) { this._cb = cb; } observe() {} disconnect() {} takeRecords() { return []; } }
  class IntersectionObserver { constructor(cb) { this._cb = cb; } observe(t) { Promise.resolve().then(() => this._cb([{ target: t, isIntersecting: true, intersectionRatio: 1, boundingClientRect: t.getBoundingClientRect() }], this)); } unobserve() {} disconnect() {} takeRecords() { return []; } }
  class ResizeObserver { constructor(cb) { this._cb = cb; } observe() {} unobserve() {} disconnect() {} }
  class DOMParser { parseFromString(html) { const d = doc.createElement('div'); d.innerHTML = html; const body = d; return { body, documentElement: d, head: d, querySelector: (s) => d.querySelector(s), querySelectorAll: (s) => d.querySelectorAll(s), getElementById: (i) => d.querySelector('#' + i), title: '' }; } }

  g.document = doc;

  // ------------------------------------------------------------------ interfaces pages test for or subclass
  // Most of these are only here so that `typeof X`, `instanceof X` and `class extends X` work; the few with behaviour are small.
  class DOMException extends Error {
    constructor(message, name) { super(message === undefined ? '' : String(message)); this.name = name === undefined ? 'Error' : String(name); this.code = ({ IndexSizeError: 1, HierarchyRequestError: 3, WrongDocumentError: 4, InvalidCharacterError: 5, NotFoundError: 8, NotSupportedError: 9, InvalidStateError: 11, SyntaxError: 12, InvalidAccessError: 15, SecurityError: 18, NetworkError: 19, AbortError: 20, TimeoutError: 23, DataCloneError: 25 })[this.name] || 0; }
  }
  classes.DOMException = DOMException;
  classes.Attr = class Attr { constructor(name, value) { this.name = name; this.localName = name; this.value = value; this.nodeName = name; this.nodeValue = value; this.specified = true; this.ownerElement = null; } };
  classes.NodeList = class NodeList {}; classes.HTMLCollection = class HTMLCollection {}; classes.NamedNodeMap = class NamedNodeMap {}; classes.DOMTokenList = class DOMTokenList {};
  classes.DOMStringList = class DOMStringList {}; classes.DOMStringMap = class DOMStringMap {};
  classes.DocumentType = class DocumentType extends Node { get _type() { return 10; } get _name() { return 'html'; } };
  classes.ShadowRoot = class ShadowRoot extends DocumentFragment { get mode() { return 'open'; } };
  classes.HTMLSlotElement = class HTMLSlotElement extends HTMLElement { assignedNodes() { return []; } assignedElements() { return []; } };
  classes.HTMLUnknownElement = class HTMLUnknownElement extends HTMLElement {};
  classes.SVGElement = class SVGElement extends Element {};
  classes.DOMImplementation = class DOMImplementation { hasFeature() { return true; } createHTMLDocument(t) { const d = Object.create(g.document); const mk = (x) => g.document.createElement(x); const html = mk('html'), head = mk('head'), body = mk('body'); html.appendChild(head); html.appendChild(body); Object.defineProperties(d, { documentElement: { value: html }, head: { value: head }, body: { value: body } }); return d; } createDocument() { return this.createHTMLDocument(); } };
  classes.CustomElementRegistry = class CustomElementRegistry {
    constructor() { this._w = new Map(); }
    define(name, ctor) {
      name = String(name);
      if (!/^[a-z][a-z0-9._]*-[a-z0-9._-]*$/.test(name)) throw new DOMException("'" + name + "' is not a valid custom element name", 'SyntaxError');
      if (ce.has(name) || ceByCtor.has(ctor)) throw new DOMException("the name '" + name + "' has already been used", 'NotSupportedError');
      ce.set(name, { ctor }); ceByCtor.set(ctor, name);
      const w = this._w.get(name); if (w) { this._w.delete(name); w(ctor); }
      // only the elements with this name are looked for (pages define thousands of elements one after the other)
      const top = doc.documentElement;
      if (top && top instanceof Element) {
        const found = [...top.querySelectorAll(name)];
        if (top.localName === name) found.unshift(top);
        for (const el of found) { upgrade(el, el.localName); hide(el, '_ceUp', true); if (el.isConnected) ceConnect(el); }
      }
    }
    get(name) { const d = ce.get(String(name)); return d ? d.ctor : undefined; }
    whenDefined(name) { const d = ce.get(String(name)); if (d) return Promise.resolve(d.ctor); return new Promise((res) => { this._w.set(String(name), res); }); }
    upgrade(root) { ceUpgradeIn(root); }
    getName(ctor) { return ceByCtor.get(ctor) || null; }
  };
  classes.Range = class Range { constructor() { this.collapsed = true; this.startOffset = 0; this.endOffset = 0; this.startContainer = null; this.endContainer = null; this.commonAncestorContainer = null; } setStart() {} setEnd() {} setStartBefore() {} setEndAfter() {} selectNode() {} selectNodeContents() {} collapse() {} cloneRange() { return new classes.Range(); } detach() {} deleteContents() {} toString() { return ''; } getBoundingClientRect() { return new classes.DOMRect(); } getClientRects() { return []; } createContextualFragment(h) { const t = g.document.createElement('template'); t.innerHTML = h; return t.content || g.document.createDocumentFragment(); } };
  classes.Selection = class Selection { constructor() { this.rangeCount = 0; this.isCollapsed = true; this.anchorNode = null; this.focusNode = null; this.type = 'None'; } getRangeAt() { return new classes.Range(); } removeAllRanges() {} addRange() {} collapse() {} toString() { return ''; } };
  classes.NodeFilter = { FILTER_ACCEPT: 1, FILTER_REJECT: 2, FILTER_SKIP: 3, SHOW_ALL: 0xFFFFFFFF, SHOW_ELEMENT: 1, SHOW_ATTRIBUTE: 2, SHOW_TEXT: 4, SHOW_COMMENT: 128, SHOW_DOCUMENT: 256 };
  const walk = (self, n) => {
    const bit = 1 << (n.nodeType - 1);
    if (!(self.whatToShow & bit)) return 3;
    if (!self.filter) return 1;
    const r = typeof self.filter === 'function' ? self.filter(n) : self.filter.acceptNode(n);
    return r;
  };
  const after = (n, root) => { // the next node in document order
    let x = n.firstChild;
    if (x) return x;
    while (x === null || x === undefined) { if (!n || n === root) return null; x = n.nextSibling; if (!x) n = n.parentNode; }
    return x;
  };
  const before = (n, root) => { // the previous node in document order
    if (n === root) return null;
    let p = n.previousSibling;
    if (!p) return n.parentNode;
    while (p.lastChild) p = p.lastChild;
    return p;
  };
  classes.TreeWalker = class TreeWalker {
    constructor(root, what, filter) { this.root = root; this.currentNode = root; this.whatToShow = what === undefined ? 0xFFFFFFFF : what >>> 0; this.filter = filter || null; }
    nextNode() { let n = this.currentNode; for (let i = 0; i < 100000; i++) { n = after(n, this.root); if (!n) return null; if (walk(this, n) === 1) { this.currentNode = n; return n; } } return null; }
    previousNode() { let n = this.currentNode; for (let i = 0; i < 100000; i++) { n = before(n, this.root); if (!n) return null; if (walk(this, n) === 1) { this.currentNode = n; return n; } } return null; }
    parentNode() { let n = this.currentNode; while (n && n !== this.root) { n = n.parentNode; if (n && walk(this, n) === 1) { this.currentNode = n; return n; } } return null; }
    firstChild() { for (let c = this.currentNode.firstChild; c; c = c.nextSibling) if (walk(this, c) === 1) { this.currentNode = c; return c; } return null; }
    lastChild() { for (let c = this.currentNode.lastChild; c; c = c.previousSibling) if (walk(this, c) === 1) { this.currentNode = c; return c; } return null; }
    nextSibling() { for (let c = this.currentNode.nextSibling; c; c = c.nextSibling) if (walk(this, c) === 1) { this.currentNode = c; return c; } return null; }
    previousSibling() { for (let c = this.currentNode.previousSibling; c; c = c.previousSibling) if (walk(this, c) === 1) { this.currentNode = c; return c; } return null; }
  };
  classes.NodeIterator = class NodeIterator {
    constructor(root, what, filter) { this.root = root; this.referenceNode = root; this.pointerBeforeReferenceNode = true; this.whatToShow = what === undefined ? 0xFFFFFFFF : what >>> 0; this.filter = filter || null; this._first = true; }
    nextNode() { let n = this.referenceNode; if (this._first) { this._first = false; if (walk(this, n) === 1) return n; } for (let i = 0; i < 100000; i++) { n = after(n, this.root); if (!n) return null; if (walk(this, n) === 1) { this.referenceNode = n; return n; } } return null; }
    previousNode() { let n = this.referenceNode; for (let i = 0; i < 100000; i++) { n = before(n, this.root); if (!n) return null; if (walk(this, n) === 1) { this.referenceNode = n; return n; } } return null; }
    detach() {}
  };
  classes.MediaQueryList = class MediaQueryList extends EventTarget { constructor(q) { super(); this.media = String(q); this.matches = false; this.onchange = null; } addListener() {} removeListener() {} };
  classes.DOMRect = class DOMRect { constructor(x, y, w, h) { this.x = x || 0; this.y = y || 0; this.width = w || 0; this.height = h || 0; this.top = this.y; this.left = this.x; this.right = this.x + this.width; this.bottom = this.y + this.height; } toJSON() { return Object.assign({}, this); } static fromRect(r) { r = r || {}; return new classes.DOMRect(r.x, r.y, r.width, r.height); } };
  classes.DOMRectReadOnly = classes.DOMRect;
  classes.DOMPoint = class DOMPoint { constructor(x, y, z, w) { this.x = x || 0; this.y = y || 0; this.z = z || 0; this.w = w === undefined ? 1 : w; } };
  classes.DOMMatrix = class DOMMatrix { constructor() { this.a = 1; this.b = 0; this.c = 0; this.d = 1; this.e = 0; this.f = 0; this.is2D = true; this.isIdentity = true; } translate() { return this; } scale() { return this; } rotate() { return this; } multiply() { return this; } inverse() { return this; } };
  classes.Storage = class Storage {};
  classes.Location = class Location {}; classes.History = class History {}; classes.Screen = class Screen {}; classes.Window = Window; classes.Performance = class Performance {};
  classes.HTMLDocument = Document;
  classes.CSSStyleSheet = class CSSStyleSheet { constructor() { this.cssRules = []; this.rules = this.cssRules; this.disabled = false; } insertRule() { return 0; } deleteRule() {} replaceSync() {} replace() { return Promise.resolve(this); } };
  classes.StyleSheet = classes.CSSStyleSheet; classes.CSSStyleDeclaration = class CSSStyleDeclaration {}; classes.CSSRule = class CSSRule {};
  classes.Blob = class Blob { constructor(parts, o) { this._p = (parts || []).map((x) => (x instanceof Blob ? x._p.join('') : String(x))).join(''); this.size = this._p.length; this.type = (o && o.type) || ''; } text() { return Promise.resolve(this._p); } arrayBuffer() { return Promise.resolve(typeof TextEncoder === 'function' ? new TextEncoder().encode(this._p).buffer : new ArrayBuffer(0)); } slice(a, b, t) { return new Blob([this._p.slice(a, b)], { type: t || this.type }); } };
  classes.File = class File extends classes.Blob { constructor(parts, name, o) { super(parts, o); this.name = String(name); this.lastModified = (o && o.lastModified) || 0; } };
  classes.FileReader = class FileReader extends EventTarget { constructor() { super(); this.readyState = 0; this.result = null; this.error = null; this.onload = null; this.onerror = null; this.onloadend = null; } readAsText(b) { Promise.resolve().then(() => { this.result = b._p; this.readyState = 2; for (const t of ['load', 'loadend']) { const e = new Event(t); this.dispatchEvent(e); } }); } readAsDataURL(b) { this.readAsText(b); } readAsArrayBuffer(b) { this.readAsText(b); } abort() {} };
  classes.FormData = class FormData { constructor() { this._e = []; } append(k, v) { this._e.push([String(k), v]); } set(k, v) { this.delete(k); this.append(k, v); } get(k) { const e = this._e.find((x) => x[0] === k); return e ? e[1] : null; } getAll(k) { return this._e.filter((x) => x[0] === k).map((x) => x[1]); } has(k) { return this._e.some((x) => x[0] === k); } delete(k) { this._e = this._e.filter((x) => x[0] !== k); } entries() { return this._e[Symbol.iterator](); } keys() { return this._e.map((x) => x[0])[Symbol.iterator](); } values() { return this._e.map((x) => x[1])[Symbol.iterator](); } forEach(f) { this._e.forEach(([k, v]) => f(v, k, this)); } [Symbol.iterator]() { return this.entries(); } };
  class MessagePort extends EventTarget { constructor() { super(); this.onmessage = null; this._o = null; } postMessage(d) { const o = this._o; if (o) Promise.resolve().then(() => { const e = new MessageEvent('message', { data: d }); o.dispatchEvent(e); }); } start() {} close() {} }
  classes.MessagePort = MessagePort;
  classes.MessageChannel = class MessageChannel { constructor() { this.port1 = new MessagePort(); this.port2 = new MessagePort(); this.port1._o = this.port2; this.port2._o = this.port1; } };
  const bcast = new Map();
  classes.BroadcastChannel = class BroadcastChannel extends EventTarget {
    constructor(n) { super(); this.name = String(n); this.onmessage = null; this.onmessageerror = null; hide(this, '_closed', false); if (!bcast.has(this.name)) bcast.set(this.name, new Set()); bcast.get(this.name).add(this); }
    postMessage(d) {
      if (this._closed) throw new DOMException('the channel is closed', 'InvalidStateError');
      const copy = d === undefined ? undefined : JSON.parse(JSON.stringify(d));
      for (const c of Array.from(bcast.get(this.name))) if (c !== this) Promise.resolve().then(() => { if (!c._closed) c.dispatchEvent(new MessageEvent('message', { data: copy })); });
    }
    close() { this._closed = true; const s = bcast.get(this.name); if (s) s.delete(this); }
  };
  const offline = (name) => class extends EventTarget { constructor(url) { super(); this.url = String(url); this.readyState = 3; this.onerror = null; this.onopen = null; this.onmessage = null; this.onclose = null; Promise.resolve().then(() => { const e = new Event('error'); this.dispatchEvent(e); }); } send() { throw new DOMException('the connection is not open', 'InvalidStateError'); } close() {} };
  classes.WebSocket = offline('WebSocket'); classes.EventSource = offline('EventSource');
  classes.Worker = class Worker extends EventTarget { constructor() { super(); throw new DOMException('Workers are not available here', 'NotSupportedError'); } };
  classes.SharedWorker = classes.Worker; classes.ServiceWorker = class ServiceWorker {}; classes.Notification = class Notification { constructor() {} static get permission() { return 'denied'; } static requestPermission() { return Promise.resolve('denied'); } };
  classes.ImageData = class ImageData { constructor(w, h) { this.width = w; this.height = h; this.data = new Uint8ClampedArray(w * h * 4); } };
  classes.ImageBitmap = class ImageBitmap {}; classes.Path2D = class Path2D {}; classes.OffscreenCanvas = class OffscreenCanvas { constructor(w, h) { this.width = w; this.height = h; } getContext() { return null; } };
  classes.CanvasRenderingContext2D = class CanvasRenderingContext2D {};
  classes.MediaSource = class MediaSource extends EventTarget { static isTypeSupported() { return false; } };
  classes.AudioContext = class AudioContext { constructor() { throw new DOMException('audio is not available here', 'NotSupportedError'); } };
  classes.Audio = class Audio extends HTMLElement { play() { return Promise.reject(new DOMException('playback is not available', 'NotSupportedError')); } pause() {} canPlayType() { return ''; } load() {} };
  classes.ReadableStream = class ReadableStream {}; classes.WritableStream = class WritableStream {}; classes.TransformStream = class TransformStream {};
  classes.CDATASection = class CDATASection extends CharacterData { get _type() { return 4; } get _name() { return '#cdata-section'; } };
  classes.ProcessingInstruction = class ProcessingInstruction extends CharacterData { get _type() { return 7; } get _name() { return 'xml'; } };
  for (const n of ['Anchor', 'Area', 'Base', 'BR', 'Data', 'DataList', 'Details', 'Dialog', 'Embed', 'FieldSet', 'Font', 'HR', 'Mod', 'Map', 'Meta', 'Meter', 'Object', 'OptGroup', 'Output', 'Param', 'Picture', 'Pre', 'Progress', 'Quote', 'Source', 'TableCell', 'TableCol', 'TableRow', 'TableSection', 'Time', 'Title', 'Track', 'Legend', 'Menu', 'Slot']) if (!classes['HTML' + n + 'Element']) classes['HTML' + n + 'Element'] = class extends HTMLElement {};
  classes.XMLSerializer = class XMLSerializer { serializeToString(n) { return n && n.outerHTML !== undefined ? n.outerHTML : String(n && n.textContent || ''); } };
  classes.DOMParser = classes.DOMParser || class DOMParser { parseFromString(str, type) { const d = classes.DOMImplementation.prototype.createHTMLDocument.call({}); const body = d.body; body.innerHTML = String(str); return d; } };
  if (typeof g.Intl === 'undefined') {
    const num = (n, o) => { o = o || {}; const f = Number(n); if (!Number.isFinite(f)) return String(f); const d = o.maximumFractionDigits !== undefined ? o.maximumFractionDigits : (o.style === 'percent' ? 0 : 3); const v = o.style === 'percent' ? f * 100 : f; let t = Math.abs(v).toFixed(Math.max(o.minimumFractionDigits || 0, d)); if (t.indexOf('.') >= 0 && !(o.minimumFractionDigits > 0)) t = t.replace(/\.?0+$/, ''); const [i, fr] = t.split('.'); const grouped = o.useGrouping === false ? i : i.replace(/\B(?=(\d{3})+(?!\d))/g, ','); return (v < 0 ? '-' : '') + (o.style === 'currency' ? '$' : '') + grouped + (fr ? '.' + fr : '') + (o.style === 'percent' ? '%' : ''); };
    class NumberFormat { constructor(l, o) { this._o = o || {}; } format(n) { return num(n, this._o); } formatToParts(n) { return [{ type: 'literal', value: num(n, this._o) }]; } resolvedOptions() { return Object.assign({ locale: 'en-US', numberingSystem: 'latn' }, this._o); } static supportedLocalesOf() { return ['en-US']; } }
    class DateTimeFormat { constructor(l, o) { this._o = o || {}; } format(d) { const x = d === undefined ? new Date() : new Date(d); return (x.getUTCMonth() + 1) + '/' + x.getUTCDate() + '/' + x.getUTCFullYear(); } formatToParts(d) { return [{ type: 'literal', value: this.format(d) }]; } resolvedOptions() { return { locale: 'en-US', calendar: 'gregory', numberingSystem: 'latn', timeZone: 'UTC' }; } static supportedLocalesOf() { return ['en-US']; } }
    class Collator { constructor(l, o) { this._o = o || {}; } compare(a, b) { a = String(a); b = String(b); if (this._o.sensitivity === 'base' || this._o.sensitivity === 'accent') { a = a.toLowerCase(); b = b.toLowerCase(); } return a < b ? -1 : a > b ? 1 : 0; } resolvedOptions() { return { locale: 'en-US' }; } }
    class PluralRules { select(n) { return n === 1 ? 'one' : 'other'; } resolvedOptions() { return { locale: 'en-US' }; } }
    class RelativeTimeFormat { format(v, u) { const n = Math.abs(v); const unit = String(u).replace(/s$/, ''); return v < 0 ? n + ' ' + unit + (n === 1 ? '' : 's') + ' ago' : 'in ' + n + ' ' + unit + (n === 1 ? '' : 's'); } }
    class ListFormat { constructor(l, o) { this._o = o || {}; } format(l) { const a = Array.from(l); const w = this._o.type === 'disjunction' ? 'or' : 'and'; return a.length < 3 ? a.join(' ' + w + ' ') : a.slice(0, -1).join(', ') + ', ' + w + ' ' + a[a.length - 1]; } }
    class Locale { constructor(t) { this.baseName = String(t); this.language = String(t).split('-')[0]; } toString() { return this.baseName; } }
    class Segmenter { segment(s) { return Array.from(String(s)).map((c, i) => ({ segment: c, index: i, input: s })); } }
    class DisplayNames { of(c) { return String(c); } }
    g.Intl = { NumberFormat, DateTimeFormat, Collator, PluralRules, RelativeTimeFormat, ListFormat, Locale, Segmenter, DisplayNames, getCanonicalLocales: (l) => [].concat(l || []), supportedValuesOf: () => [] };
  }
  g.createImageBitmap = () => Promise.reject(new DOMException('not available', 'NotSupportedError'));
  g.indexedDB = { open() { const r = new EventTarget(); r.onerror = null; r.error = new DOMException('storage is not available', 'InvalidStateError'); Promise.resolve().then(() => { const e = new Event('error'); r.dispatchEvent(e); if (typeof r.onerror === 'function') r.onerror(e); }); return r; }, deleteDatabase() { return this.open(); } };
  classes.IDBKeyRange = class IDBKeyRange {};
  g.speechSynthesis = { speaking: false, pending: false, paused: false, getVoices() { return []; }, speak() {}, cancel() {}, pause() {}, resume() {}, addEventListener() {}, removeEventListener() {} };

  // ------------------------------------------------------------------ element classes by tag, SVG, more events
  const hc = (tag, name) => { const C = classes['HTML' + name + 'Element']; if (C) tagClass[tag] = C; };
  for (const [t, n] of Object.entries({ a: 'Anchor', area: 'Area', audio: 'Audio', base: 'Base', body: 'Body', br: 'BR', button: 'Button', canvas: 'Canvas', data: 'Data', datalist: 'DataList', details: 'Details', dialog: 'Dialog', div: 'Div', dl: 'DList', embed: 'Embed', fieldset: 'FieldSet', form: 'Form', h1: 'Heading', h2: 'Heading', h3: 'Heading', h4: 'Heading', h5: 'Heading', h6: 'Heading', head: 'Head', hr: 'HR', html: 'Html', iframe: 'IFrame', img: 'Image', input: 'Input', label: 'Label', legend: 'Legend', li: 'LI', link: 'Link', map: 'Map', meta: 'Meta', meter: 'Meter', object: 'Object', ol: 'OList', optgroup: 'OptGroup', option: 'Option', output: 'Output', p: 'Paragraph', picture: 'Picture', pre: 'Pre', progress: 'Progress', q: 'Quote', blockquote: 'Quote', script: 'Script', select: 'Select', slot: 'Slot', source: 'Source', span: 'Span', style: 'Style', table: 'Table', td: 'TableCell', th: 'TableCell', col: 'TableCol', colgroup: 'TableCol', tr: 'TableRow', tbody: 'TableSection', thead: 'TableSection', tfoot: 'TableSection', template: 'Template', textarea: 'TextArea', time: 'Time', title: 'Title', track: 'Track', ul: 'UList', video: 'Video' })) hc(t, n);
  classes.HTMLDListElement = classes.HTMLDListElement || class HTMLDListElement extends HTMLElement {};
  tagClass.dl = classes.HTMLDListElement;
  classes.SVGGraphicsElement = class SVGGraphicsElement extends classes.SVGElement { getBBox() { return new classes.DOMRect(); } getCTM() { return new classes.DOMMatrix(); } getScreenCTM() { return new classes.DOMMatrix(); } };
  classes.SVGGeometryElement = class SVGGeometryElement extends classes.SVGGraphicsElement { getTotalLength() { return 0; } getPointAtLength() { return new classes.DOMPoint(); } isPointInFill() { return false; } };
  classes.SVGSVGElement = class SVGSVGElement extends classes.SVGGraphicsElement { createSVGMatrix() { return new classes.DOMMatrix(); } createSVGPoint() { return new classes.DOMPoint(); } createSVGNumber() { return { value: 0 }; } getElementById(i) { return this.querySelector('#' + i); } suspendRedraw() { return 0; } unsuspendRedraw() {} };
  classes.SVGGElement = class SVGGElement extends classes.SVGGraphicsElement {};
  classes.SVGPathElement = class SVGPathElement extends classes.SVGGeometryElement {};
  classes.SVGUseElement = class SVGUseElement extends classes.SVGGraphicsElement {};
  classes.SVGAnimatedLength = class SVGAnimatedLength {};
  for (const t of ['circle', 'ellipse', 'line', 'polygon', 'polyline', 'rect']) { const n = 'SVG' + t[0].toUpperCase() + t.slice(1) + 'Element'; classes[n] = class extends classes.SVGGeometryElement {}; Object.defineProperty(classes[n], 'name', { value: n }); tagClass[t] = classes[n]; }
  for (const t of ['text', 'tspan', 'image', 'foreignObject', 'switch', 'symbol', 'marker', 'pattern', 'mask', 'clipPath']) { const n = 'SVG' + t[0].toUpperCase() + t.slice(1) + 'Element'; classes[n] = class extends classes.SVGGraphicsElement {}; Object.defineProperty(classes[n], 'name', { value: n }); tagClass[t] = classes[n]; }
  for (const t of ['defs', 'linearGradient', 'radialGradient', 'stop', 'filter', 'animate', 'style', 'title', 'desc', 'metadata']) { if (t === 'style' || t === 'title') continue; const n = 'SVG' + t[0].toUpperCase() + t.slice(1) + 'Element'; classes[n] = class extends classes.SVGElement {}; Object.defineProperty(classes[n], 'name', { value: n }); tagClass[t] = classes[n]; }
  tagClass.svg = classes.SVGSVGElement; tagClass.g = classes.SVGGElement; tagClass.path = classes.SVGPathElement; tagClass.use = classes.SVGUseElement;
  const mouse = (name, B) => { classes[name] = class extends B {}; Object.defineProperty(classes[name], 'name', { value: name }); };
  mouse('PointerEvent', MouseEvent); mouse('WheelEvent', MouseEvent); mouse('DragEvent', MouseEvent); mouse('TouchEvent', UIEvent); mouse('CompositionEvent', UIEvent);
  for (const n of ['AnimationEvent', 'ClipboardEvent', 'CloseEvent', 'FormDataEvent', 'HashChangeEvent', 'MediaQueryListEvent', 'PageTransitionEvent', 'ProgressEvent', 'PromiseRejectionEvent', 'StorageEvent', 'TransitionEvent']) { classes[n] = class extends Event { constructor(t, i) { super(t, i); Object.assign(this, i || {}); } }; Object.defineProperty(classes[n], 'name', { value: n }); }
  classes.WheelEvent.prototype.deltaY = 0;
  classes.MutationRecord = class MutationRecord {}; classes.IntersectionObserverEntry = class IntersectionObserverEntry {}; classes.ResizeObserverEntry = class ResizeObserverEntry {}; classes.StaticRange = class StaticRange {};
  classes.Touch = class Touch {}; classes.DataTransfer = class DataTransfer { constructor() { this.files = []; this.items = []; this.types = []; this.dropEffect = 'none'; this.effectAllowed = 'all'; } setData() {} getData() { return ''; } clearData() {} setDragImage() {} };
  classes.FileList = class FileList {}; classes.StyleSheetList = class StyleSheetList {}; classes.MediaList = class MediaList {}; classes.XMLDocument = class XMLDocument extends Document {};
  classes.HTMLFormControlsCollection = class HTMLFormControlsCollection {}; classes.HTMLOptionsCollection = class HTMLOptionsCollection {};
  classes.TimeRanges = class TimeRanges { get length() { return 0; } start() { return 0; } end() { return 0; } };
  classes.TextTrack = class TextTrack extends EventTarget {}; classes.VideoPlaybackQuality = class VideoPlaybackQuality {}; classes.SourceBuffer = class SourceBuffer extends EventTarget {}; classes.MediaStream = class MediaStream extends EventTarget {};
  classes.TextMetrics = class TextMetrics {}; classes.CanvasGradient = class CanvasGradient {}; classes.CanvasPattern = class CanvasPattern {}; classes.WebGLRenderingContext = class WebGLRenderingContext {};
  classes.RTCPeerConnection = class RTCPeerConnection extends EventTarget { constructor() { super(); throw new DOMException('WebRTC is not available here', 'NotSupportedError'); } };
  classes.Gamepad = class Gamepad {}; classes.GeolocationPosition = class GeolocationPosition {}; classes.ElementInternals = class ElementInternals {}; classes.FontFace = class FontFace { constructor(f, s) { this.family = f; this.status = 'unloaded'; this.loaded = Promise.resolve(this); } load() { return Promise.resolve(this); } };
  classes.FontFaceSet = class FontFaceSet extends EventTarget {};
  classes.IDBFactory = class IDBFactory {}; classes.IDBDatabase = class IDBDatabase extends EventTarget {}; classes.IDBObjectStore = class IDBObjectStore {}; classes.IDBTransaction = class IDBTransaction extends EventTarget {}; classes.IDBRequest = class IDBRequest extends EventTarget {}; classes.IDBCursor = class IDBCursor {};
  classes.Cache = class Cache { match() { return Promise.resolve(undefined); } put() { return Promise.resolve(); } add() { return Promise.resolve(); } delete() { return Promise.resolve(false); } keys() { return Promise.resolve([]); } };
  classes.CacheStorage = class CacheStorage { match() { return Promise.resolve(undefined); } has() { return Promise.resolve(false); } open() { return Promise.resolve(new classes.Cache()); } delete() { return Promise.resolve(false); } keys() { return Promise.resolve([]); } };
  g.caches = new classes.CacheStorage();
  classes.ServiceWorkerContainer = class ServiceWorkerContainer extends EventTarget { register() { return Promise.reject(new DOMException('service workers are not available here', 'SecurityError')); } getRegistration() { return Promise.resolve(undefined); } getRegistrations() { return Promise.resolve([]); } get ready() { return new Promise(() => {}); } get controller() { return null; } };
  classes.ServiceWorkerRegistration = class ServiceWorkerRegistration extends EventTarget {};
  classes.Permissions = class Permissions { query() { return Promise.resolve({ state: 'denied', onchange: null, addEventListener() {} }); } }; classes.PermissionStatus = class PermissionStatus extends EventTarget {};
  classes.VisualViewport = class VisualViewport extends EventTarget {}; { const vv = new classes.VisualViewport(); Object.assign(vv, { width: 1280, height: 720, scale: 1, offsetLeft: 0, offsetTop: 0, pageLeft: 0, pageTop: 0 }); g.visualViewport = vv; }
  g.getSelection = () => new classes.Selection();
  g.scheduler = { postTask(fn) { return new Promise((res, rej) => { setTimeout(() => { try { res(fn()); } catch (e) { rej(e); } }, 0); }); }, yield() { return Promise.resolve(); } };
  g.trustedTypes = { createPolicy(name, rules) { return Object.assign({ name }, { createHTML: (s) => (rules && rules.createHTML ? rules.createHTML(s) : s), createScript: (s) => (rules && rules.createScript ? rules.createScript(s) : s), createScriptURL: (s) => (rules && rules.createScriptURL ? rules.createScriptURL(s) : s) }); }, isHTML: () => false, isScript: () => false, isScriptURL: () => false, emptyHTML: '', emptyScript: '', getAttributeType: () => null, defaultPolicy: null };
  classes.Navigator = class Navigator {};
  Object.setPrototypeOf(g.navigator, classes.Navigator.prototype);
  classes.BarProp = class BarProp {}; classes.Atomics = typeof Atomics === 'undefined' ? {} : Atomics;
  if (typeof g.Atomics === 'undefined') g.Atomics = { load: (a, i) => a[i], store: (a, i, v) => { a[i] = v; return v; }, add: (a, i, v) => { const o = a[i]; a[i] += v; return o; }, wait() { return 'timed-out'; }, notify() { return 0; } };
  delete classes.Atomics;
  classes.XMLHttpRequestEventTarget = class XMLHttpRequestEventTarget extends EventTarget {}; classes.XMLHttpRequestUpload = class XMLHttpRequestUpload extends classes.XMLHttpRequestEventTarget {};
  classes.XPathResult = { ANY_TYPE: 0, NUMBER_TYPE: 1, STRING_TYPE: 2, BOOLEAN_TYPE: 3, UNORDERED_NODE_ITERATOR_TYPE: 4, ORDERED_NODE_ITERATOR_TYPE: 5, FIRST_ORDERED_NODE_TYPE: 9 };
  classes.XPathEvaluator = class XPathEvaluator { evaluate() { return { singleNodeValue: null, snapshotLength: 0, iterateNext() { return null; }, snapshotItem() { return null; } }; } };
  classes.XSLTProcessor = class XSLTProcessor {};
  classes.PerformanceResourceTiming = class PerformanceResourceTiming {}; classes.PerformanceObserverEntryList = class PerformanceObserverEntryList { getEntries() { return []; } getEntriesByType() { return []; } getEntriesByName() { return []; } };
  classes.BaseAudioContext = class BaseAudioContext extends EventTarget {};
  classes.Animation = class Animation extends EventTarget { constructor() { super(); this.finished = Promise.resolve(this); this.ready = Promise.resolve(this); this.playState = 'finished'; } play() {} pause() {} cancel() {} finish() {} reverse() {} };
  // web streams: enough for pages that build or pipe a stream of chunks
  class ReadableStreamDefaultReader {
    constructor(stream) { this._s = stream; this.closed = new Promise((res, rej) => { stream._closedP = { res, rej }; }); stream._locked = true; }
    read() { return this._s._read(); }
    releaseLock() { this._s._locked = false; }
    cancel(r) { return this._s.cancel(r); }
  }
  class ReadableStream {
    constructor(src, strategy) {
      src = src || {};
      this._q = []; this._waiters = []; this._done = false; this._err = null; this._locked = false; this._src = src; this._pulling = false;
      const self = this;
      this._ctl = {
        enqueue(c) { if (self._waiters.length) self._waiters.shift().res({ value: c, done: false }); else self._q.push(c); },
        close() { self._done = true; while (self._waiters.length) self._waiters.shift().res({ value: undefined, done: true }); if (self._closedP) self._closedP.res(); },
        error(e) { self._err = e || new Error('stream error'); while (self._waiters.length) self._waiters.shift().rej(self._err); if (self._closedP) self._closedP.rej(self._err); },
        get desiredSize() { return 1 - self._q.length; },
      };
      try { const r = src.start && src.start(this._ctl); if (r && typeof r.then === 'function') r.catch((e) => this._ctl.error(e)); } catch (e) { this._ctl.error(e); }
    }
    get locked() { return this._locked; }
    _read() {
      if (this._err) return Promise.reject(this._err);
      if (this._q.length) return Promise.resolve({ value: this._q.shift(), done: false });
      if (this._done) return Promise.resolve({ value: undefined, done: true });
      if (this._src.pull && !this._pulling) { this._pulling = true; Promise.resolve().then(() => this._src.pull(this._ctl)).then(() => { this._pulling = false; }, (e) => { this._pulling = false; this._ctl.error(e); }); }
      return new Promise((res, rej) => { this._waiters.push({ res, rej }); });
    }
    getReader() { if (this._locked) throw new TypeError('the stream is locked'); return new ReadableStreamDefaultReader(this); }
    cancel(r) { this._done = true; this._q.length = 0; try { if (this._src.cancel) this._src.cancel(r); } catch (e) {} while (this._waiters.length) this._waiters.shift().res({ value: undefined, done: true }); return Promise.resolve(); }
    tee() { const a = []; const r = this.getReader(); const mk = () => new ReadableStream({ pull: async (c) => { const x = await r.read(); if (x.done) c.close(); else { for (const o of a) if (o !== c) o.enqueue(x.value); c.enqueue(x.value); } } }); const s1 = mk(); const s2 = mk(); a.push(s1._ctl, s2._ctl); return [s1, s2]; }
    pipeThrough(t) { this.pipeTo(t.writable).catch(() => {}); return t.readable; }
    async pipeTo(w) { const wr = w.getWriter(); const r = this.getReader(); for (;;) { const x = await r.read(); if (x.done) break; await wr.write(x.value); } await wr.close(); }
    [Symbol.asyncIterator]() { const r = this.getReader(); return { next: () => r.read(), return: () => { r.releaseLock(); return Promise.resolve({ done: true }); }, [Symbol.asyncIterator]() { return this; } }; }
    static from(it) { const i = it[Symbol.asyncIterator] ? it[Symbol.asyncIterator]() : it[Symbol.iterator](); return new ReadableStream({ async pull(c) { const x = await i.next(); if (x.done) c.close(); else c.enqueue(x.value); } }); }
  }
  class WritableStreamDefaultWriter {
    constructor(s) { this._s = s; this.closed = Promise.resolve(); this.ready = Promise.resolve(); this.desiredSize = 1; }
    write(c) { try { const r = this._s._sink.write ? this._s._sink.write(c, this._s._ctl) : undefined; return Promise.resolve(r); } catch (e) { return Promise.reject(e); } }
    close() { try { const r = this._s._sink.close ? this._s._sink.close() : undefined; return Promise.resolve(r); } catch (e) { return Promise.reject(e); } }
    abort(r) { return Promise.resolve(this._s._sink.abort ? this._s._sink.abort(r) : undefined); }
    releaseLock() {}
  }
  class WritableStream { constructor(sink) { this._sink = sink || {}; this._ctl = { error() {} }; if (this._sink.start) this._sink.start(this._ctl); this.locked = false; } getWriter() { return new WritableStreamDefaultWriter(this); } close() { return this.getWriter().close(); } abort(r) { return this.getWriter().abort(r); } }
  class TransformStream {
    constructor(tr) {
      tr = tr || {};
      let rc;
      this.readable = new ReadableStream({ start(c) { rc = c; } });
      const ctl = { enqueue: (c) => rc.enqueue(c), error: (e) => rc.error(e), terminate: () => rc.close(), desiredSize: 1 };
      if (tr.start) tr.start(ctl);
      this.writable = new WritableStream({ write: (c) => (tr.transform ? tr.transform(c, ctl) : ctl.enqueue(c)), close: () => { const r = tr.flush ? tr.flush(ctl) : undefined; return Promise.resolve(r).then(() => rc.close()); } });
    }
  }
  classes.ReadableStream = ReadableStream; classes.ReadableStreamDefaultReader = ReadableStreamDefaultReader; classes.WritableStream = WritableStream; classes.WritableStreamDefaultWriter = WritableStreamDefaultWriter; classes.TransformStream = TransformStream;
  classes.CountQueuingStrategy = class CountQueuingStrategy { constructor(o) { this.highWaterMark = o && o.highWaterMark; } size() { return 1; } };
  classes.ByteLengthQueuingStrategy = class ByteLengthQueuingStrategy { constructor(o) { this.highWaterMark = o && o.highWaterMark; } size(c) { return c.byteLength; } };
  if (typeof TextEncoder === 'function') {
    classes.TextEncoderStream = class TextEncoderStream extends TransformStream { constructor() { const e = new TextEncoder(); super({ transform(c, ctl) { ctl.enqueue(e.encode(c)); } }); this.encoding = 'utf-8'; } };
    classes.TextDecoderStream = class TextDecoderStream extends TransformStream { constructor(l, o) { const d = new TextDecoder(l, o); super({ transform(c, ctl) { const t = d.decode(c, { stream: true }); if (t) ctl.enqueue(t); } }); this.encoding = 'utf-8'; } };
  }
  classes.CompressionStream = class CompressionStream extends TransformStream { constructor() { super({}); } }; classes.DecompressionStream = class DecompressionStream extends TransformStream { constructor() { super({}); } };
  // crypto.subtle: digests only
  const rotl = (x, n) => (x << n) | (x >>> (32 - n));
  const sha1 = (bytes) => { const h = [0x67452301, 0xEFCDAB89, 0x98BADCFE, 0x10325476, 0xC3D2E1F0]; const l = bytes.length; const pad = new Uint8Array(((l + 9 + 63) >> 6) << 6); pad.set(bytes); pad[l] = 0x80; const dv = new DataView(pad.buffer); dv.setUint32(pad.length - 4, l * 8 >>> 0); dv.setUint32(pad.length - 8, Math.floor(l / 0x20000000)); const w = new Array(80); for (let o = 0; o < pad.length; o += 64) { for (let i = 0; i < 16; i++) w[i] = dv.getUint32(o + i * 4); for (let i = 16; i < 80; i++) w[i] = rotl(w[i - 3] ^ w[i - 8] ^ w[i - 14] ^ w[i - 16], 1); let [a, b, c, d, e] = h; for (let i = 0; i < 80; i++) { const [f, k] = i < 20 ? [(b & c) | (~b & d), 0x5A827999] : i < 40 ? [b ^ c ^ d, 0x6ED9EBA1] : i < 60 ? [(b & c) | (b & d) | (c & d), 0x8F1BBCDC] : [b ^ c ^ d, 0xCA62C1D6]; const t = (rotl(a, 5) + f + e + k + w[i]) | 0; e = d; d = c; c = rotl(b, 30); b = a; a = t; } h[0] = (h[0] + a) | 0; h[1] = (h[1] + b) | 0; h[2] = (h[2] + c) | 0; h[3] = (h[3] + d) | 0; h[4] = (h[4] + e) | 0; } const out = new Uint8Array(20); const od = new DataView(out.buffer); h.forEach((v, i) => od.setUint32(i * 4, v)); return out; };
  const K256 = [0x428a2f98, 0x71374491, 0xb5c0fbcf, 0xe9b5dba5, 0x3956c25b, 0x59f111f1, 0x923f82a4, 0xab1c5ed5, 0xd807aa98, 0x12835b01, 0x243185be, 0x550c7dc3, 0x72be5d74, 0x80deb1fe, 0x9bdc06a7, 0xc19bf174, 0xe49b69c1, 0xefbe4786, 0x0fc19dc6, 0x240ca1cc, 0x2de92c6f, 0x4a7484aa, 0x5cb0a9dc, 0x76f988da, 0x983e5152, 0xa831c66d, 0xb00327c8, 0xbf597fc7, 0xc6e00bf3, 0xd5a79147, 0x06ca6351, 0x14292967, 0x27b70a85, 0x2e1b2138, 0x4d2c6dfc, 0x53380d13, 0x650a7354, 0x766a0abb, 0x81c2c92e, 0x92722c85, 0xa2bfe8a1, 0xa81a664b, 0xc24b8b70, 0xc76c51a3, 0xd192e819, 0xd6990624, 0xf40e3585, 0x106aa070, 0x19a4c116, 0x1e376c08, 0x2748774c, 0x34b0bcb5, 0x391c0cb3, 0x4ed8aa4a, 0x5b9cca4f, 0x682e6ff3, 0x748f82ee, 0x78a5636f, 0x84c87814, 0x8cc70208, 0x90befffa, 0xa4506ceb, 0xbef9a3f7, 0xc67178f2];
  const rotr = (x, n) => (x >>> n) | (x << (32 - n));
  const sha256 = (bytes) => { const h = [0x6a09e667, 0xbb67ae85, 0x3c6ef372, 0xa54ff53a, 0x510e527f, 0x9b05688c, 0x1f83d9ab, 0x5be0cd19]; const l = bytes.length; const pad = new Uint8Array(((l + 9 + 63) >> 6) << 6); pad.set(bytes); pad[l] = 0x80; const dv = new DataView(pad.buffer); dv.setUint32(pad.length - 4, l * 8 >>> 0); dv.setUint32(pad.length - 8, Math.floor(l / 0x20000000)); const w = new Array(64); for (let o = 0; o < pad.length; o += 64) { for (let i = 0; i < 16; i++) w[i] = dv.getUint32(o + i * 4); for (let i = 16; i < 64; i++) { const s0 = rotr(w[i - 15], 7) ^ rotr(w[i - 15], 18) ^ (w[i - 15] >>> 3); const s1 = rotr(w[i - 2], 17) ^ rotr(w[i - 2], 19) ^ (w[i - 2] >>> 10); w[i] = (w[i - 16] + s0 + w[i - 7] + s1) | 0; } let [a, b, c, d, e, f, gg, hh] = h; for (let i = 0; i < 64; i++) { const S1 = rotr(e, 6) ^ rotr(e, 11) ^ rotr(e, 25); const ch = (e & f) ^ (~e & gg); const t1 = (hh + S1 + ch + K256[i] + w[i]) | 0; const S0 = rotr(a, 2) ^ rotr(a, 13) ^ rotr(a, 22); const mj = (a & b) ^ (a & c) ^ (b & c); const t2 = (S0 + mj) | 0; hh = gg; gg = f; f = e; e = (d + t1) | 0; d = c; c = b; b = a; a = (t1 + t2) | 0; } h[0] = (h[0] + a) | 0; h[1] = (h[1] + b) | 0; h[2] = (h[2] + c) | 0; h[3] = (h[3] + d) | 0; h[4] = (h[4] + e) | 0; h[5] = (h[5] + f) | 0; h[6] = (h[6] + gg) | 0; h[7] = (h[7] + hh) | 0; } const out = new Uint8Array(32); const od = new DataView(out.buffer); h.forEach((v, i) => od.setUint32(i * 4, v)); return out; };
  classes.SubtleCrypto = class SubtleCrypto {
    digest(alg, data) { const n = String(alg && alg.name || alg).toUpperCase(); const b = data instanceof ArrayBuffer ? new Uint8Array(data) : new Uint8Array(data.buffer, data.byteOffset, data.byteLength); try { if (n === 'SHA-256') return Promise.resolve(sha256(b).buffer); if (n === 'SHA-1') return Promise.resolve(sha1(b).buffer); } catch (e) { return Promise.reject(e); } return Promise.reject(new DOMException('the digest ' + n + ' is not available', 'NotSupportedError')); }
    importKey() { return Promise.reject(new DOMException('not available', 'NotSupportedError')); } generateKey() { return Promise.reject(new DOMException('not available', 'NotSupportedError')); } sign() { return Promise.reject(new DOMException('not available', 'NotSupportedError')); } encrypt() { return Promise.reject(new DOMException('not available', 'NotSupportedError')); } decrypt() { return Promise.reject(new DOMException('not available', 'NotSupportedError')); }
  };
  classes.Crypto = class Crypto {}; classes.CryptoKey = class CryptoKey {};
  { const c = g.crypto; Object.setPrototypeOf(c, classes.Crypto.prototype); c.subtle = new classes.SubtleCrypto(); }
  for (const k of Object.keys(classes)) g[k] = classes[k];
  g.customElements = new classes.CustomElementRegistry();
  g.Document = Document; g.MutationObserver = MutationObserver; g.IntersectionObserver = IntersectionObserver; g.ResizeObserver = ResizeObserver; g.DOMParser = DOMParser;
  g.Audio = function Audio(src) { const a = doc.createElement('audio'); if (src !== undefined) a.setAttribute('src', String(src)); return a; };
  g.Audio.prototype = (classes.HTMLAudioElement || HTMLElement).prototype;
  g.Image = function (w, h) { const i = doc.createElement('img'); if (w) i.width = w; if (h) i.height = h; return i; };
  g.Option = function (text, value) { const o = doc.createElement('option'); o.textContent = text || ''; if (value !== undefined) o.value = value; return o; };

  g.__dom = {
    setCurrent(id) { hide(doc, '_cur', id === null ? null : wrap(id)); },
    fireLoad() {
      doc._ready = 'interactive'; doc.dispatchEvent(new Event('readystatechange'));
      doc.dispatchEvent(new Event('DOMContentLoaded', { bubbles: true }));
      doc._ready = 'complete'; doc.dispatchEvent(new Event('readystatechange'));
      g.dispatchEvent(new Event('load'));
    },
    fire(target, type) { (target || g).dispatchEvent(new Event(type, { bubbles: true, cancelable: true })); },
    // an element by the id of its node in the page tree (a host that keeps the page alive acts on controls this way)
    byNode(id) { return wrap(id, H1('info', id).name); },
    click(id) { this.byNode(id).click(); },
    // set what the person typed, then fire the events a browser fires
    input(id, value) { const e = this.byNode(id); e.value = String(value); e.dispatchEvent(new Event('input', { bubbles: true })); e.dispatchEvent(new Event('change', { bubbles: true })); },
    check(id, on) { const e = this.byNode(id); if (!!e.checked !== !!on) e.click(); },
  };
})(globalThis);
