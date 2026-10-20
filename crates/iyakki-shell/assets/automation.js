/* IYAKKI browser automation bundle, version 1.
 *
 * Evaluated inside the page by the shell's web engine (docs/BROWSER-AGENT.md). It touches no network, evaluates no string the page
 * provides (only CSS selectors the caller built from its own ids), and exposes one entry point:
 *
 *     window.__iyakkiAuto.run(op, argsJson)  ->  JSON text
 *
 * Every answer is a JSON object `{ok: true, ...}` or `{ok: false, error, ...}`. Operations:
 *   snapshot  accessibility-style snapshot with stable ids (e1, e2, ...)     describe  facts about one element (for consent checks)
 *   click type press scroll select hover                                       find      elements by role / name / label / text
 *   read_form  every field of every form with label, type, required, validation message
 *   probe      one cheap check for wait(): text present? element present? network quiet?
 *   fill_secret_prepare / secret_check  focus a secret field without ever carrying the secret (the shell types it with real input)
 *
 * The bundle works in an engine with layout (WebKit) and in the layout-less test engine (iyakki-js): without layout every element
 * counts as inside the viewport and only attributes and inline styles decide visibility.
 */
(function () {
  'use strict';
  var VERSION = 1;
  var g = typeof globalThis !== 'undefined' ? globalThis : window;
  if (g.__iyakkiAuto && g.__iyakkiAuto.v === VERSION) return;

  var MAX_TEXT = 12000;       // characters of snapshot text
  var MAX_NAME = 80;
  var MAX_VALUE = 120;
  var MAX_ELEMENTS = 400;
  var MAX_DEPTH = 60;

  var st = g.__iyakki = g.__iyakki || {};
  st.seq = st.seq || 0;
  st.byId = st.byId || new Map();      // id -> element
  st.ids = st.ids || new WeakMap();    // element -> id
  st.prev = st.prev || null;           // previous snapshot signature: id -> "role|name|value|state"
  st.snaps = st.snaps || 0;
  st.inflight = 0;
  st.lastMutation = Date.now();

  var doc = g.document;

  // -------------------------------------------------------------------------------------------- small helpers
  function lc(s) { return String(s == null ? '' : s).toLowerCase(); }
  function norm(s) { return String(s == null ? '' : s).replace(/\s+/g, ' ').trim(); }
  function cut(s, n) { s = String(s == null ? '' : s); return s.length > n ? s.slice(0, n - 1) + '…' : s; }
  function attr(el, k) { try { var v = el.getAttribute(k); return v == null ? null : v; } catch (e) { return null; } }
  function tag(el) { return lc(el.localName || el.tagName); }
  function isEl(n) { return n && n.nodeType === 1; }
  function out(o) { return JSON.stringify(o); }
  function fail(error, extra) { var o = { ok: false, error: error }; if (extra) for (var k in extra) o[k] = extra[k]; return o; }
  function hostOf(href) { try { return new URL(href, g.location.href).host; } catch (e) { return ''; } }
  function now() { return Date.now(); }

  var HAS_LAYOUT = (function () {
    try {
      var r = doc.documentElement.getBoundingClientRect();
      var b = doc.body ? doc.body.getBoundingClientRect() : r;
      return (r && r.width > 0 && r.height > 0) || (b && b.width > 0 && b.height > 0);
    } catch (e) { return false; }
  })();

  // network-quiet heuristic: count requests that are still open, remember when the page last changed
  try {
    if (typeof g.fetch === 'function' && !g.fetch.__iyakki) {
      var of = g.fetch;
      var wrapped = function () {
        st.inflight++; st.lastMutation = now();
        var p;
        try { p = of.apply(this, arguments); } catch (e) { st.inflight--; throw e; }
        var done = function () { st.inflight = Math.max(0, st.inflight - 1); st.lastMutation = now(); };
        if (p && typeof p.then === 'function') p.then(done, done); else done();
        return p;
      };
      wrapped.__iyakki = true;
      g.fetch = wrapped;
    }
    if (g.XMLHttpRequest && g.XMLHttpRequest.prototype && !g.XMLHttpRequest.prototype.__iyakkiSend) {
      var P = g.XMLHttpRequest.prototype, os = P.send;
      if (typeof os === 'function') {
        P.send = function () {
          var x = this;
          st.inflight++; st.lastMutation = now();
          var fin = function () { if (x.__iyakkiOpen) { x.__iyakkiOpen = false; st.inflight = Math.max(0, st.inflight - 1); st.lastMutation = now(); } };
          x.__iyakkiOpen = true;
          try { x.addEventListener('loadend', fin); } catch (e) { fin(); }
          try { return os.apply(this, arguments); } catch (e) { fin(); throw e; }
        };
        P.__iyakkiSend = true;
      }
    }
    if (typeof g.MutationObserver === 'function' && doc.documentElement) {
      new g.MutationObserver(function () { st.lastMutation = now(); }).observe(doc.documentElement, { childList: true, subtree: true, characterData: true, attributes: true });
    }
  } catch (e) { /* the heuristics are best effort */ }

  // -------------------------------------------------------------------------------------------- ids
  function idOf(el) {
    var id = st.ids.get(el);
    if (!id) { id = 'e' + (++st.seq); st.ids.set(el, id); }
    st.byId.set(id, el);
    return id;
  }
  // attached to the page, also inside a shadow root (whose own root is a fragment in the layout-less test engine)
  function alive(el) {
    if (el.isConnected !== false) return true;
    var r = null;
    try { r = el.getRootNode && el.getRootNode(); } catch (e) {}
    return !!(r && r.nodeType === 11 && r.host && alive(r.host));
  }
  function byId(id) {
    id = String(id || '');
    var el = st.byId.get(id);
    if (!el) return null;
    if (!alive(el)) { st.byId.delete(id); return null; }
    return el;
  }
  function pruneIds() {
    st.byId.forEach(function (el, id) { if (!alive(el)) st.byId.delete(id); });
  }

  // -------------------------------------------------------------------------------------------- roles, names, state
  var INLINE = { a: 1, abbr: 1, b: 1, bdi: 1, bdo: 1, cite: 1, code: 1, data: 1, dfn: 1, em: 1, i: 1, kbd: 1, mark: 1, q: 1, s: 1, samp: 1, small: 1, span: 1, strong: 1, sub: 1, sup: 1, time: 1, u: 1, 'var': 1, wbr: 1, font: 1, big: 1, tt: 1, del: 1, ins: 1, label: 1, img: 1 };
  var SKIP = { script: 1, style: 1, noscript: 1, template: 1, head: 1, meta: 1, link: 1, title: 1, svg: 1, canvas: 1, 'iyakki-hl': 1, datalist: 1, option: 1, optgroup: 1 };
  var TEXTINPUT = { text: 1, email: 1, search: 1, url: 1, tel: 1, password: 1, number: 1, '': 1, date: 1, 'datetime-local': 1, month: 1, week: 1, time: 1 };

  function inputType(el) { return lc(attr(el, 'type') || el.type || 'text'); }

  function isEditable(el) {
    var ce = attr(el, 'contenteditable');
    if (ce !== null && lc(ce) !== 'false') return true;
    try { if (el.isContentEditable === true) return true; } catch (e) {}
    return false;
  }

  function roleOf(el) {
    var t = tag(el);
    var r = lc(attr(el, 'role')).split(/\s+/)[0];
    if (r) {
      if (r === 'searchbox') return 'textbox';
      if (r === 'switch') return 'checkbox';
      if (r === 'link' || r === 'button' || r === 'textbox' || r === 'checkbox' || r === 'radio' || r === 'option' || r === 'heading' || r === 'img' || r === 'combobox' || r === 'listbox' || r === 'menuitem' || r === 'tab' || r === 'slider' || r === 'menuitemcheckbox' || r === 'menuitemradio' || r === 'treeitem' || r === 'spinbutton') return r === 'img' ? 'image' : r === 'menuitemcheckbox' ? 'checkbox' : r === 'menuitemradio' ? 'radio' : r;
    }
    if (t === 'a') return attr(el, 'href') !== null ? 'link' : (attr(el, 'onclick') !== null || attr(el, 'tabindex') !== null ? 'button' : null);
    if (t === 'button' || t === 'summary') return 'button';
    if (t === 'select') return 'select';
    if (t === 'textarea') return 'textbox';
    if (t === 'option') return 'option';
    if (/^h[1-6]$/.test(t)) return 'heading';
    if (t === 'img') return 'image';
    if (t === 'input') {
      var ty = inputType(el);
      if (ty === 'hidden') return null;
      if (ty === 'checkbox') return 'checkbox';
      if (ty === 'radio') return 'radio';
      if (ty === 'button' || ty === 'submit' || ty === 'reset' || ty === 'image') return 'button';
      if (ty === 'range') return 'slider';
      if (ty === 'file') return 'file';
      if (ty === 'color') return 'button';
      return 'textbox';
    }
    if (isEditable(el)) return 'textbox';
    if (attr(el, 'onclick') !== null) return 'button';
    return null;
  }

  function textOf(el, max) {
    var s = '';
    try { s = el.innerText != null && el.innerText !== '' ? el.innerText : el.textContent; } catch (e) { s = el.textContent; }
    return cut(norm(s), max || MAX_NAME);
  }

  var labelCache = null;
  function labelsFor(el) {
    var out_ = [];
    var id = el.id || attr(el, 'id');
    var root = el.getRootNode ? el.getRootNode() : doc;
    var scope = root && root.querySelectorAll ? root : doc;
    if (id) {
      var ls = scope.querySelectorAll('label');
      for (var i = 0; i < ls.length; i++) if (attr(ls[i], 'for') === id) out_.push(ls[i]);
    }
    var wrap = el.closest ? el.closest('label') : null;
    if (wrap && out_.indexOf(wrap) < 0) out_.push(wrap);
    return out_;
  }

  function labelledBy(el) {
    var ids = attr(el, 'aria-labelledby');
    if (!ids) return '';
    var root = el.getRootNode ? el.getRootNode() : doc;
    var parts = [];
    ids.split(/\s+/).forEach(function (i) {
      if (!i) return;
      var t = null;
      try { t = (root && root.getElementById ? root.getElementById(i) : null) || doc.getElementById(i); } catch (e) {}
      if (t) parts.push(textOf(t, MAX_NAME));
    });
    return norm(parts.join(' '));
  }

  function nameOf(el, role) {
    var n = norm(attr(el, 'aria-label'));
    if (n) return cut(n, MAX_NAME);
    n = labelledBy(el);
    if (n) return cut(n, MAX_NAME);
    var t = tag(el);
    if (t === 'input' || t === 'textarea' || t === 'select' || isEditable(el)) {
      var ls = labelsFor(el);
      for (var i = 0; i < ls.length; i++) {
        // the label text without the field's own text (a wrapping label)
        var lt = norm(ls[i].textContent);
        if (lt) return cut(lt, MAX_NAME);
      }
      var ty = t === 'input' ? inputType(el) : '';
      if (ty === 'button' || ty === 'submit' || ty === 'reset') return cut(norm(attr(el, 'value')) || (ty === 'submit' ? 'Submit' : ty === 'reset' ? 'Reset' : ''), MAX_NAME);
      if (ty === 'image') return cut(norm(attr(el, 'alt')) || 'Submit', MAX_NAME);
      n = norm(attr(el, 'placeholder')) || norm(attr(el, 'aria-placeholder')) || norm(attr(el, 'data-placeholder')) || norm(attr(el, 'title')) || norm(attr(el, 'name'));
      return cut(n, MAX_NAME);
    }
    if (t === 'img') return cut(norm(attr(el, 'alt')) || norm(attr(el, 'title')), MAX_NAME);
    n = textOf(el, MAX_NAME);
    if (n) return n;
    var im = el.querySelector ? el.querySelector('img[alt]') : null;
    if (im) return cut(norm(attr(im, 'alt')), MAX_NAME);
    return cut(norm(attr(el, 'title')) || norm(attr(el, 'value')) || norm(attr(el, 'name')), MAX_NAME);
  }

  function valueOf(el, role) {
    var t = tag(el);
    try {
      if (t === 'input') {
        var ty = inputType(el);
        if (ty === 'password') return el.value ? '(hidden, ' + String(el.value).length + ' chars)' : '';
        if (ty === 'checkbox' || ty === 'radio' || ty === 'button' || ty === 'submit' || ty === 'reset' || ty === 'image' || ty === 'file') return '';
        return cut(String(el.value == null ? '' : el.value), MAX_VALUE);
      }
      if (t === 'textarea') return cut(String(el.value != null && el.value !== '' ? el.value : (el.value === '' ? '' : el.textContent)), MAX_VALUE);
      if (t === 'select') {
        var opts = el.options || el.querySelectorAll('option');
        for (var i = 0; i < opts.length; i++) if (opts[i].selected === true || (attr(opts[i], 'selected') !== null && !el.__iyakkiTouched)) return cut(norm(opts[i].textContent), MAX_VALUE);
        if (typeof el.selectedIndex === 'number' && el.selectedIndex >= 0 && opts[el.selectedIndex]) return cut(norm(opts[el.selectedIndex].textContent), MAX_VALUE);
        return opts.length ? cut(norm(opts[0].textContent), MAX_VALUE) : '';
      }
      if (isEditable(el)) return cut(norm(el.textContent), MAX_VALUE);
      if (role === 'combobox' || role === 'listbox') {
        var v = attr(el, 'aria-valuetext');
        if (v) return cut(norm(v), MAX_VALUE);
        if (role === 'combobox' && el.value) return cut(String(el.value), MAX_VALUE);
        return cut(textOf(el, MAX_VALUE), MAX_VALUE);
      }
      if (role === 'slider') return String(attr(el, 'aria-valuenow') || el.value || '');
    } catch (e) {}
    return '';
  }

  function styleOf(el) {
    try { return g.getComputedStyle ? g.getComputedStyle(el) : null; } catch (e) { return null; }
  }

  function rectOf(el) {
    try {
      var r = el.getBoundingClientRect();
      return r ? { x: r.left, y: r.top, w: r.width, h: r.height } : { x: 0, y: 0, w: 0, h: 0 };
    } catch (e) { return { x: 0, y: 0, w: 0, h: 0 }; }
  }

  function isHidden(el) {
    // attributes first: they work in every engine
    if (attr(el, 'hidden') !== null) return true;
    if (tag(el) === 'input' && inputType(el) === 'hidden') return true;
    if (attr(el, 'inert') !== null) return true;
    var s = styleOf(el);
    if (s) {
      var d = '', v = '', o = '';
      try { d = s.display; v = s.visibility; o = s.opacity; } catch (e) {}
      if (d === 'none' || v === 'hidden' || v === 'collapse') return true;
      if (HAS_LAYOUT && o !== '' && parseFloat(o) === 0) return true;
    }
    if (HAS_LAYOUT) {
      var r = rectOf(el);
      if (r.w <= 0 && r.h <= 0) return true;
    }
    return false;
  }

  // an element is shown when it and every ancestor (through shadow hosts and frames) is not hidden
  function isShown(el) {
    var n = el, guard = 0;
    while (n && guard++ < 80) {
      if (isEl(n) && isHidden(n)) return false;
      var det = isEl(n) && n.parentNode && isEl(n.parentNode) && tag(n.parentNode) === 'details' && attr(n.parentNode, 'open') === null && tag(n) !== 'summary';
      if (det) return false;
      var p = n.parentNode;
      if (!p) break;
      if (p.nodeType === 11 && p.host) p = p.host;
      n = p;
    }
    return true;
  }

  function inViewport(box) {
    if (!HAS_LAYOUT) return true;
    var vw = g.innerWidth || 1280, vh = g.innerHeight || 720;
    return box.x + box.w > 0 && box.y + box.h > 0 && box.x < vw && box.y < vh;
  }

  function isDisabled(el) {
    if (attr(el, 'disabled') !== null || lc(attr(el, 'aria-disabled')) === 'true') return true;
    var fs = el.closest ? el.closest('fieldset[disabled]') : null;
    return !!fs;
  }

  function stateOf(el, role) {
    var t = tag(el), s = [];
    var box = rectOf(el);
    var shown = isShown(el);
    if (!shown) s.push('hidden');
    else if (!inViewport(box)) s.push('offscreen');
    if (isDisabled(el)) s.push('disabled');
    var active = null;
    try { active = doc.activeElement; } catch (e) {}
    if (active === el) s.push('focused');
    var checked = null;
    if (t === 'input' && (inputType(el) === 'checkbox' || inputType(el) === 'radio')) checked = el.checked === true || (attr(el, 'checked') !== null && el.__iyakkiTouched !== true);
    else if (attr(el, 'aria-checked') !== null) checked = lc(attr(el, 'aria-checked')) === 'true';
    if (checked !== null) s.push(checked ? 'checked' : 'unchecked');
    var ex = attr(el, 'aria-expanded');
    if (ex !== null) s.push(lc(ex) === 'true' ? 'expanded' : 'collapsed');
    else if (t === 'details') s.push(attr(el, 'open') !== null ? 'expanded' : 'collapsed');
    if (attr(el, 'aria-selected') !== null && lc(attr(el, 'aria-selected')) === 'true') s.push('selected');
    if (attr(el, 'aria-pressed') === 'true') s.push('pressed');
    if (attr(el, 'required') !== null || lc(attr(el, 'aria-required')) === 'true') s.push('required');
    if (attr(el, 'readonly') !== null || lc(attr(el, 'aria-readonly')) === 'true') s.push('readonly');
    if (lc(attr(el, 'aria-invalid')) === 'true') s.push('invalid');
    if (t === 'input') { var ty = inputType(el); if (ty && ty !== 'text' && role === 'textbox') s.push('type=' + ty); }
    if (attr(el, 'aria-haspopup') && attr(el, 'aria-haspopup') !== 'false') s.push('popup');
    return s;
  }

  // -------------------------------------------------------------------------------------------- the walk
  function interactiveRole(el) {
    var r = roleOf(el);
    if (!r || r === 'heading' || r === 'image') return null;
    if (r === 'option' || r === 'tab' || r === 'menuitem' || r === 'treeitem') return r;
    return r;
  }

  function chatLike(el) {
    var role = lc(attr(el, 'role'));
    if (role === 'log' || role === 'feed') return true;
    var live = lc(attr(el, 'aria-live'));
    if (live === 'polite' || live === 'assertive') return true;
    var sig = lc((attr(el, 'id') || '') + ' ' + (attr(el, 'class') || '') + ' ' + (attr(el, 'data-testid') || ''));
    if (/(^|[^a-z])(chat|conversation|messages?|thread|transcript|history)([^a-z]|$)/.test(sig) && el.children && el.children.length >= 2) return true;
    var s = styleOf(el);
    if (s && HAS_LAYOUT) {
      var oy = s.overflowY;
      if ((oy === 'auto' || oy === 'scroll') && el.scrollHeight > el.clientHeight + 20 && el.children && el.children.length >= 3) return true;
    }
    return false;
  }

  function Walk(limit) {
    this.items = [];
    this.run = [];
    this.runCtx = null;
    this.limit = limit;
    this.chat = [];          // stack of chat container records
    this.frames = [];
    this.heading = 0;
    this.offset = { x: 0, y: 0 };
    this.count = 0;
    this.truncated = false;
    this.notes = [];
  }

  Walk.prototype.flush = function () {
    if (!this.run.length) return;
    var text = norm(this.run.join(' '));
    this.run = [];
    if (!text) return;
    var it = { kind: this.heading ? 'heading' : 'text', text: cut(text, 400), level: this.heading || 0 };
    if (this.chat.length) it.chat = this.chat[this.chat.length - 1].n;
    this.items.push(it);
  };

  Walk.prototype.element = function (el, depth) {
    if (depth > MAX_DEPTH || this.count > this.limit * 3) return;
    var t = tag(el);
    if (SKIP[t]) return;
    if (attr(el, 'data-iyakki') !== null) return;
    if (isHidden(el)) { this.flush(); return; }
    if (t === 'details' && attr(el, 'open') === null) {
      // only the summary of a closed disclosure is visible
      var kids0 = el.children || [];
      for (var d = 0; d < kids0.length; d++) if (tag(kids0[d]) === 'summary') this.element(kids0[d], depth + 1);
      return;
    }
    var role = interactiveRole(el);
    var isHeading = /^h[1-6]$/.test(t) || lc(attr(el, 'role')) === 'heading';
    if (role) {
      this.flush();
      this.item(el, role);
      // a combobox / listbox / menu exposes its options as separate items
      if (role === 'listbox' || role === 'combobox' || role === 'menuitem' || role === 'tab') {
        // descend only for listbox-like containers that contain options
        if (role === 'listbox') {
          var opts = el.querySelectorAll ? el.querySelectorAll('[role=option],[role=menuitem],[role=treeitem],option') : [];
          for (var i = 0; i < opts.length && i < 60; i++) if (isShown(opts[i])) this.item(opts[i], roleOf(opts[i]) || 'option');
        }
      }
      return;
    }
    var block = !INLINE[t];
    var chatRec = null;
    if (block && chatLike(el)) {
      chatRec = { n: (this.chatCount = (this.chatCount || 0) + 1) };
      this.flush();
      this.chat.push(chatRec);
      this.items.push({ kind: 'region', text: 'conversation / scrolling log' + (attr(el, 'aria-label') ? ' "' + cut(norm(attr(el, 'aria-label')), 40) + '"' : ''), chat: chatRec.n, start: true });
    }
    if (isHeading) { this.flush(); this.heading = /^h([1-6])$/.test(t) ? parseInt(t.charAt(1), 10) : (parseInt(attr(el, 'aria-level'), 10) || 2); }
    else if (block) this.flush();

    var isLabel = t === 'label' && (attr(el, 'for') !== null || (el.querySelector && el.querySelector('input,select,textarea,button,[contenteditable]')));
    if (isLabel) this.noText = (this.noText || 0) + 1;
    if (t === 'img') {
      var alt = norm(attr(el, 'alt'));
      if (alt) { this.flush(); this.items.push({ kind: 'image', text: cut(alt, MAX_NAME) }); }
    } else if (t === 'iframe' || t === 'frame') {
      this.flush();
      this.frame(el, depth);
    } else {
      var sr = null;
      try { sr = el.shadowRoot; } catch (e) {}
      if (sr) {
        this.children(sr, depth);
        // slotted light DOM is walked too
      }
      this.children(el, depth);
    }

    if (isLabel) this.noText--;
    if (isHeading) { this.flush(); this.heading = 0; }
    else if (block) this.flush();
    if (chatRec) { this.flush(); this.chat.pop(); this.items.push({ kind: 'region', text: 'end of conversation', chat: chatRec.n, end: true }); }
  };

  Walk.prototype.children = function (parent, depth) {
    var kids = parent.childNodes || [];
    for (var i = 0; i < kids.length; i++) {
      var n = kids[i];
      if (n.nodeType === 3) { if (this.noText) continue; var v = n.nodeValue != null ? n.nodeValue : n.textContent; if (v && /\S/.test(v)) this.run.push(v); }
      else if (n.nodeType === 1) this.element(n, depth + 1);
      if (this.count > this.limit * 3) { this.truncated = true; break; }
    }
  };

  Walk.prototype.frame = function (el, depth) {
    var d = null, src = attr(el, 'src') || '';
    try { d = el.contentDocument; } catch (e) { d = null; }
    if (d && d.body) {
      var r = rectOf(el);
      var saved = this.offset;
      this.offset = { x: saved.x + r.x, y: saved.y + r.y };
      this.items.push({ kind: 'region', text: 'frame' + (attr(el, 'title') ? ' "' + cut(norm(attr(el, 'title')), 40) + '"' : ''), start: true });
      this.element(d.body, depth + 1);
      this.flush();
      this.items.push({ kind: 'region', text: 'end of frame', end: true });
      this.offset = saved;
    } else {
      var h = hostOf(src);
      this.items.push({ kind: 'note', text: 'frame from ' + (h || 'another origin') + ' (its content cannot be read)' });
    }
  };

  Walk.prototype.item = function (el, role) {
    this.count++;
    if (this.count > this.limit) { this.truncated = true; return; }
    var id = idOf(el);
    var box = rectOf(el);
    var it = { kind: 'el', id: id, role: role, name: nameOf(el, role), value: valueOf(el, role), state: stateOf(el, role), el: el };
    if (role === 'link') { var h = hostOf(attr(el, 'href')); if (h) it.host = h; }
    if (role === 'heading') it.level = parseInt((tag(el).match(/^h([1-6])$/) || [0, attr(el, 'aria-level') || 2])[1], 10);
    it.box = { x: Math.round(box.x + this.offset.x), y: Math.round(box.y + this.offset.y), w: Math.round(box.w), h: Math.round(box.h) };
    if (this.chat.length) it.chat = this.chat[this.chat.length - 1].n;
    this.items.push(it);
  };

  function fmtState(s) { return s.length ? ' [' + s.join(', ') + ']' : ''; }

  function fmtItem(it) {
    if (it.kind === 'text') return 'text "' + it.text + '"';
    if (it.kind === 'heading') return 'heading(' + it.level + ') "' + it.text + '"';
    if (it.kind === 'image') return 'image "' + it.text + '"';
    if (it.kind === 'region') return it.start ? '-- ' + it.text + ' --' : '-- ' + it.text + ' --';
    if (it.kind === 'note') return '(' + it.text + ')';
    var s = it.id + ' ' + it.role + ' "' + it.name + '"';
    if (it.level) s = it.id + ' heading(' + it.level + ') "' + it.name + '"';
    if (it.value) s += ' value="' + it.value + '"';
    if (it.host) s += ' -> ' + it.host;
    s += fmtState(it.state);
    if (it.box && (it.box.w || it.box.h)) s += ' @' + it.box.x + ',' + it.box.y + ' ' + it.box.w + 'x' + it.box.h;
    return s;
  }

  function sig(it) { return it.role + '|' + it.name + '|' + it.value + '|' + it.state.filter(function (x) { return x !== 'offscreen'; }).join(','); }

  // -------------------------------------------------------------------------------------------- snapshot
  function snapshot(a) {
    a = a || {};
    var max = Math.max(1000, Math.min(a.max || MAX_TEXT, 40000));
    pruneIds();
    var w = new Walk(MAX_ELEMENTS);
    var root = doc.body || doc.documentElement;
    if (root) w.element(root, 0);
    w.flush();
    // drop empty headings / duplicates produced by nested walks
    // a conversation region with nothing in it says nothing
    var items = w.items.filter(function (it, ix, arr) { return !(it.kind === 'region' && ((it.start && arr[ix + 1] && arr[ix + 1].kind === 'region' && arr[ix + 1].end && arr[ix + 1].chat === it.chat) || (it.end && arr[ix - 1] && arr[ix - 1].kind === 'region' && arr[ix - 1].start && arr[ix - 1].chat === it.chat))); });
    var els = items.filter(function (i) { return i.kind === 'el'; });

    // diff against the previous snapshot
    var cur = {}, added = [], changed = [], gone = [];
    els.forEach(function (i) { cur[i.id] = sig(i); });
    var diff = '';
    if (st.prev) {
      els.forEach(function (i) {
        if (!(i.id in st.prev)) added.push(i);
        else if (st.prev[i.id] !== cur[i.id]) changed.push(i);
      });
      for (var k in st.prev) if (!(k in cur)) gone.push(k);
      var parts = [];
      var label = function (i) { return i.id + ' ' + i.role + ' "' + cut(i.name, 30) + '"'; };
      if (added.length) parts.push(added.length + ' new (' + added.slice(0, 6).map(label).join('; ') + (added.length > 6 ? '; ...' : '') + ')');
      if (changed.length) parts.push(changed.length + ' changed (' + changed.slice(0, 6).map(label).join('; ') + (changed.length > 6 ? '; ...' : '') + ')');
      if (gone.length) parts.push(gone.length + ' gone (' + gone.slice(0, 8).join(', ') + (gone.length > 8 ? ', ...' : '') + ')');
      diff = parts.length ? 'Changes since the last snapshot: ' + parts.join('; ') + '.' : 'No changes since the last snapshot.';
    } else diff = 'First snapshot of this page.';
    if (st.prevUrl && st.prevUrl !== g.location.href) diff = 'The address changed since the last snapshot. ' + diff;
    st.prev = cur; st.prevUrl = g.location.href; st.snaps++;

    var lines = items.map(fmtItem);
    var head = 'Page "' + cut(norm(doc.title), 120) + '" ' + g.location.href;
    var scrollInfo = '';
    try {
      var de = doc.documentElement;
      if (HAS_LAYOUT) scrollInfo = ' | viewport ' + (g.innerWidth || 0) + 'x' + (g.innerHeight || 0) + ', scrolled ' + Math.round(g.scrollY || 0) + ' of ' + Math.max(0, Math.round((de.scrollHeight || 0) - (g.innerHeight || 0)));
    } catch (e) {}
    head += scrollInfo;

    // fit the budget: chat containers lose their oldest messages first, then plain text is cut from the end; interactive elements stay
    var target = max - 150;
    var total = function () { var n = head.length + diff.length + 4; for (var i = 0; i < lines.length; i++) if (lines[i] !== null) n += lines[i].length + 1; return n; };
    var omitted = 0;
    var notes = [];
    if (total() > target) {
      var byChat = {};
      items.forEach(function (it, idx) { if (it.chat && (it.kind === 'text' || it.kind === 'heading')) (byChat[it.chat] = byChat[it.chat] || []).push(idx); });
      var keys = Object.keys(byChat), progress = true, cutChat = false;
      while (total() > target && progress) {
        progress = false;
        for (var c = 0; c < keys.length && total() > target; c++) {
          var arr = byChat[keys[c]];
          if (arr.length > 3) { var ix = arr.shift(); lines[ix] = cutChat ? null : '(... earlier messages left out to fit; scroll up inside the conversation to read them ...)'; if (!cutChat) { cutChat = true; } progress = true; omitted++; }
        }
        // later drops in a run leave no marker of their own: the first marker stands for the run
        if (progress) cutChat = true;
      }
      // chat heads dropped leave null holes; the first hole of each run keeps the marker
      var marked = false;
      for (var m0 = 0; m0 < lines.length; m0++) { if (lines[m0] === null) continue; if (lines[m0].indexOf('(... earlier messages') === 0) { if (marked) lines[m0] = null; marked = true; } }
      // still too long: drop plain text lines from the end, never interactive elements
      var from = lines.length - 1;
      while (total() > target && from >= 0) {
        var itx = items[from];
        if (lines[from] !== null && itx && (itx.kind === 'text' || itx.kind === 'heading' || itx.kind === 'image' || itx.kind === 'note') && !itx.chat) { lines[from] = null; omitted++; notes.push(from); }
        from--;
      }
      // and, as a last resort, the text of conversations from the start
      for (var q = 0; q < lines.length && total() > target; q++) { if (lines[q] !== null && items[q] && items[q].chat && (items[q].kind === 'text')) { lines[q] = null; omitted++; } }
      if (notes.length) { var first = Math.min.apply(null, notes); lines[first] = '(... ' + notes.length + ' more text lines left out to fit; scroll or use find / read_form for the rest ...)'; }
      lines = lines.filter(function (l) { return l !== null; });
    }
    if (w.truncated) lines.push('(the page has more than ' + MAX_ELEMENTS + ' interactive elements; only the first ones are listed)');
    var text = head + '\n' + diff + '\n' + lines.join('\n');
    return {
      ok: true, url: g.location.href, title: cut(norm(doc.title), 200), text: text, elements: els.length, truncated: omitted > 0 || w.truncated,
      layout: HAS_LAYOUT, snap: st.snaps, added: added.length, changed: changed.length, gone: gone.length,
      els: els.map(function (i) { return { id: i.id, role: i.role, name: i.name, value: i.value, state: i.state, host: i.host || '', box: i.box }; }),
      viewport: { w: g.innerWidth || 0, h: g.innerHeight || 0, dpr: g.devicePixelRatio || 1 },
    };
  }

  // -------------------------------------------------------------------------------------------- describe, find
  var SECRET_NAME = /(^|[^a-z])(pass(word|wd|code)?|pwd|otp|totp|one[-_ ]?time|2fa|mfa|verification[-_ ]?code|security[-_ ]?code|captcha|recaptcha|pin|cvv|cvc|card[-_ ]?number|cc[-_ ]?(number|num|csc|exp)|iban|ssn)([^a-z]|$)/i;
  var PAY_WORDS = /(pay\b|pay now|place (your )?order|buy now|confirm (purchase|order|payment)|complete (purchase|order)|checkout|purchase|subscribe now|donate)/i;
  var DESTRUCT_WORDS = /(delete (my )?(account|profile|data)|close (my )?account|deactivate|remove|transfer|send money|withdraw|revoke|terminate|cancel (my )?(subscription|membership|plan)|erase|destroy|unlink)/i;

  function describeEl(el) {
    var role = roleOf(el) || tag(el);
    var t = tag(el);
    var ty = t === 'input' ? inputType(el) : '';
    var name = nameOf(el, role);
    var form = el.closest ? el.closest('form') : null;
    var ac = lc(attr(el, 'autocomplete'));
    var sigText = lc([attr(el, 'name'), attr(el, 'id'), attr(el, 'autocomplete'), attr(el, 'aria-label'), attr(el, 'placeholder'), attr(el, 'data-testid'), name].join(' '));
    var secret = '';
    if (ty === 'password') secret = 'password';
    else if (ac.indexOf('one-time-code') >= 0 || /(^|[^a-z])(otp|totp|2fa|mfa|one[-_ ]?time|verification[-_ ]?code)([^a-z]|$)/.test(sigText)) secret = 'otp';
    else if (/captcha/.test(sigText) || (el.closest && el.closest('[class*=captcha],[id*=captcha],.g-recaptcha,.h-captcha,[data-sitekey]'))) secret = 'captcha';
    else if (ac.indexOf('cc-') === 0 || /(card[-_ ]?number|cardnumber|cvv|cvc|security[-_ ]?code|cc[-_ ]?(number|csc|exp))/.test(sigText)) secret = 'card';
    var submits = false;
    if (t === 'button') submits = (attr(el, 'type') === null || lc(attr(el, 'type')) === 'submit') && !!form;
    else if (t === 'input') submits = (ty === 'submit' || ty === 'image') && !!form;
    var formHasSecret = false, formHasCard = false;
    if (form) {
      try {
        formHasSecret = !!form.querySelector('input[type=password]');
        formHasCard = !!form.querySelector('[autocomplete^=cc-],input[name*=card i],input[name*=cvv i],input[name*=cvc i]');
      } catch (e) {}
    }
    var label = role + ' ' + name;
    var pay = (role === 'button' || role === 'link') && PAY_WORDS.test(name);
    var destruct = (role === 'button' || role === 'link') && DESTRUCT_WORDS.test(name);
    var textOwn = norm(el.textContent);
    return {
      id: idOf(el), role: role, name: name, tag: t, type: ty, secret: secret, submits: submits || (role === 'button' && attr(el, 'type') === 'submit'),
      editable: t === 'textarea' || isEditable(el) || (t === 'input' && !!TEXTINPUT[ty]), file: ty === 'file',
      pay: pay || (formHasCard && submits), destructive: destruct, href_host: role === 'link' ? hostOf(attr(el, 'href')) : '',
      in_form: !!form, form_action_host: form ? hostOf(attr(form, 'action') || g.location.href) : '', disabled: isDisabled(el),
      shown: isShown(el), text: cut(textOwn, 160), native_select: t === 'select', checkable: ty === 'checkbox' || ty === 'radio' || attr(el, 'aria-checked') !== null,
      form_has_secret: formHasSecret,
    };
  }

  function describe(a) {
    var el = byId(a.id);
    if (!el) return fail('There is no element ' + a.id + ' on the page now. Take a new snapshot and use an id from it.', { stale: true });
    return { ok: true, el: describeEl(el), url: g.location.href, title: norm(doc.title) };
  }

  function allElements() {
    var out_ = [];
    var w = new Walk(MAX_ELEMENTS * 2);
    var root = doc.body || doc.documentElement;
    if (root) w.element(root, 0);
    w.flush();
    w.items.forEach(function (i) { if (i.kind === 'el') out_.push(i); });
    return { items: out_, walk: w };
  }

  function find(a) {
    var role = lc(a.role), name = lc(a.name), label = lc(a.label), text = lc(a.text), exact = !!a.exact;
    var walked = allElements().walk.items;
    var all = walked.filter(function (i) { return i.kind === 'el' || (i.kind === 'heading' && (role === 'heading' || (!role && name))); }).map(function (i) { return i.kind === 'el' ? i : { kind: 'el', id: '', role: 'heading', name: i.text, value: '', state: [] }; });
    var res = [];
    var match = function (hay, needle) { hay = lc(hay); return exact ? hay === needle : hay.indexOf(needle) >= 0; };
    all.forEach(function (i) {
      if (i.state.indexOf('hidden') >= 0) return;
      if (role && i.role !== role && !(role === 'select' && i.role === 'combobox') && !(role === 'combobox' && i.role === 'select')) return;
      if (name && !match(i.name, name)) return;
      if (label && !match(i.name, label)) return;
      if (text && !match(i.name + ' ' + i.value, text)) return;
      res.push({ id: i.id, role: i.role, name: i.name, value: i.value, state: i.state });
    });
    return { ok: true, matches: res.slice(0, 20), count: res.length };
  }

  // -------------------------------------------------------------------------------------------- events and actions
  function fire(el, type, init, Ctor) {
    var ev;
    init = init || {};
    init.bubbles = init.bubbles !== false;
    init.cancelable = init.cancelable !== false;
    try {
      var C = Ctor || g.Event;
      ev = new C(type, init);
    } catch (e) {
      ev = new g.Event(type, { bubbles: init.bubbles, cancelable: init.cancelable });
    }
    // engines with thin event classes drop some fields: put them back before listeners run
    for (var k in init) { try { if (ev[k] === undefined) ev[k] = init[k]; } catch (e) {} }
    el.dispatchEvent(ev);
    return ev;
  }

  function centre(el) {
    var r = rectOf(el);
    return { x: Math.round(r.x + r.w / 2), y: Math.round(r.y + r.h / 2) };
  }

  function highlight(el) {
    try {
      if (!HAS_LAYOUT || !doc.body) return;
      var r = rectOf(el);
      var h = doc.createElement('div');
      h.setAttribute('data-iyakki', 'highlight');
      h.setAttribute('style', 'position:fixed;z-index:2147483647;pointer-events:none;border:3px solid #ff7a00;border-radius:4px;box-shadow:0 0 0 2px rgba(255,255,255,.7);left:' + Math.round(r.x - 3) + 'px;top:' + Math.round(r.y - 3) + 'px;width:' + Math.round(r.w) + 'px;height:' + Math.round(r.h) + 'px;transition:opacity .4s');
      doc.body.appendChild(h);
      setTimeout(function () { try { h.style.opacity = '0'; } catch (e) {} }, 500);
      setTimeout(function () { try { h.remove(); } catch (e) {} }, 1000);
    } catch (e) {}
  }

  function target(a, needInteractive) {
    var el = byId(a.id);
    if (!el) return { err: fail('There is no element ' + a.id + ' on the page now. Take a new snapshot and use an id from it.', { stale: true }) };
    if (!isShown(el)) return { err: fail('Element ' + a.id + ' is not visible.', { hidden: true }) };
    if (needInteractive && isDisabled(el)) return { err: fail('Element ' + a.id + ' is disabled.', { disabled: true }) };
    return { el: el };
  }

  function scrollTo(el) {
    try { if (typeof el.scrollIntoView === 'function') el.scrollIntoView({ block: 'center', inline: 'nearest' }); } catch (e) { try { el.scrollIntoView(); } catch (e2) {} }
  }

  function covered(el) {
    if (!HAS_LAYOUT || typeof doc.elementFromPoint !== 'function') return null;
    var c = centre(el);
    if (c.x < 0 || c.y < 0 || c.x > (g.innerWidth || 1e9) || c.y > (g.innerHeight || 1e9)) return null;
    var hit = null;
    try { hit = doc.elementFromPoint(c.x, c.y); } catch (e) { return null; }
    if (!hit || hit === el || el.contains(hit) || (hit.contains && hit.contains(el))) return null;
    // a label or wrapper of the control is fine
    var lab = hit.closest ? hit.closest('label') : null;
    if (lab && (lab.contains(el) || (lab.getAttribute('for') && lab.getAttribute('for') === el.id))) return null;
    // shadow DOM hosts report themselves
    var root = el.getRootNode ? el.getRootNode() : null;
    if (root && root.host && (root.host === hit || root.host.contains(hit))) return null;
    return hit;
  }

  function click(a) {
    var t = target(a, true);
    if (t.err) return t.err;
    var el = t.el;
    if (tag(el) === 'input' && inputType(el) === 'file') return fail('File inputs are not supported: ask the person to choose the file themselves.', { file: true });
    scrollTo(el);
    var cv = covered(el);
    if (cv) return fail('Element ' + a.id + ' is covered by another element (' + (tag(cv) + ' "' + cut(textOf(cv, 40), 40) + '"') + '), for example a banner or dialog. Deal with that first.', { covered: true, by: cut(textOf(cv, 60), 60) });
    highlight(el);
    var c = centre(el);
    var m = { clientX: c.x, clientY: c.y, button: 0, buttons: 1, view: g };
    var P = g.PointerEvent || g.MouseEvent, M = g.MouseEvent;
    fire(el, 'pointerover', m, P); fire(el, 'mouseover', m, M); fire(el, 'pointermove', m, P); fire(el, 'mousemove', m, M);
    fire(el, 'pointerdown', m, P); fire(el, 'mousedown', m, M);
    try { if (typeof el.focus === 'function') el.focus(); } catch (e) {}
    m.buttons = 0;
    fire(el, 'pointerup', m, P); fire(el, 'mouseup', m, M);
    var url0 = g.location.href;
    if (typeof el.click === 'function') el.click(); else fire(el, 'click', m, M);
    el.__iyakkiTouched = true;
    return { ok: true, id: a.id, clicked: nameOf(el, roleOf(el)), url_changed: g.location.href !== url0 };
  }

  var nativeValue = (function () {
    var d = {};
    try {
      if (g.HTMLInputElement) d.input = Object.getOwnPropertyDescriptor(g.HTMLInputElement.prototype, 'value');
      if (g.HTMLTextAreaElement) d.textarea = Object.getOwnPropertyDescriptor(g.HTMLTextAreaElement.prototype, 'value');
    } catch (e) {}
    return d;
  })();

  function setNative(el, v) {
    var d = nativeValue[tag(el) === 'textarea' ? 'textarea' : 'input'];
    if (d && d.set) d.set.call(el, v); else el.value = v;
  }

  function inputEvents(el, data, kind) {
    var I = g.InputEvent || g.Event;
    fire(el, 'input', { data: data, inputType: kind || 'insertText', composed: true }, I);
    fire(el, 'change', {});
  }

  function type(a) {
    var t = target(a, true);
    if (t.err) return t.err;
    var el = t.el, tg = tag(el);
    var text = String(a.text == null ? '' : a.text);
    var d = describeEl(el);
    if (d.file) return fail('File inputs are not supported: ask the person to choose the file themselves.', { file: true });
    if (d.secret && !a.allowSecret) return fail('This is a ' + d.secret + ' field. The assistant never types into it: use browser_login for sign-in, or ask the person to fill it in and press Continue.', { secret: d.secret });
    if (!d.editable) return fail('Element ' + a.id + ' (' + d.role + ') is not a text field.', { notEditable: true });
    scrollTo(el);
    highlight(el);
    try { if (typeof el.focus === 'function') el.focus(); } catch (e) {}
    var mode = 'value';
    if (tg === 'input' || tg === 'textarea') {
      var cur = String(el.value == null ? '' : el.value);
      var ty = tg === 'input' ? inputType(el) : 'text';
      var next = a.clear ? text : cur + text;
      if (ty === 'number' && next !== '' && isNaN(Number(next))) return fail('That is not a number: ' + cut(text, 40), { value: true });
      var ml = parseInt(attr(el, 'maxlength'), 10);
      if (ml > 0 && next.length > ml) next = next.slice(0, ml);
      if (a.clear && cur) { setNative(el, ''); inputEvents(el, null, 'deleteContent'); }
      try { fire(el, 'keydown', { key: text.charAt(0) || 'a' }, g.KeyboardEvent); } catch (e) {}
      setNative(el, next);
      el.__iyakkiTouched = true;
      inputEvents(el, text, 'insertText');
      try { fire(el, 'keyup', { key: text.charAt(text.length - 1) || 'a' }, g.KeyboardEvent); } catch (e) {}
    } else {
      mode = 'contenteditable';
      var ok = false;
      try {
        if (a.clear && doc.execCommand) { doc.execCommand('selectAll', false, null); doc.execCommand('delete', false, null); }
        if (doc.execCommand) {
          if (!a.clear) { var sel = g.getSelection && g.getSelection(); if (sel && sel.selectAllChildren) { sel.selectAllChildren(el); sel.collapseToEnd && sel.collapseToEnd(); } }
          ok = doc.execCommand('insertText', false, text) === true;
        }
      } catch (e) { ok = false; }
      if (!ok || norm(el.textContent).indexOf(norm(text).slice(0, 20)) < 0) {
        mode = 'contenteditable-dom';
        fire(el, 'beforeinput', { data: text, inputType: 'insertText' }, g.InputEvent || g.Event);
        el.textContent = a.clear ? text : String(el.textContent || '') + text;
        fire(el, 'input', { data: text, inputType: 'insertText' }, g.InputEvent || g.Event);
      }
    }
    var res = { ok: true, id: a.id, mode: mode, value: valueOf(el, 'textbox'), submitted: false };
    if (a.submit) {
      var pr = pressOn(el, 'Enter');
      res.submitted = !!pr.submitted;
      res.url_changed = false;
    }
    return res;
  }

  var KEYS = {
    Enter: { code: 'Enter', keyCode: 13 }, Escape: { code: 'Escape', keyCode: 27 }, Tab: { code: 'Tab', keyCode: 9 }, Backspace: { code: 'Backspace', keyCode: 8 },
    Delete: { code: 'Delete', keyCode: 46 }, ArrowUp: { code: 'ArrowUp', keyCode: 38 }, ArrowDown: { code: 'ArrowDown', keyCode: 40 }, ArrowLeft: { code: 'ArrowLeft', keyCode: 37 },
    ArrowRight: { code: 'ArrowRight', keyCode: 39 }, Home: { code: 'Home', keyCode: 36 }, End: { code: 'End', keyCode: 35 }, PageUp: { code: 'PageUp', keyCode: 33 },
    PageDown: { code: 'PageDown', keyCode: 34 }, ' ': { code: 'Space', keyCode: 32 }, Space: { code: 'Space', keyCode: 32, key: ' ' },
  };

  function normKey(k) {
    k = String(k || '');
    var m = { enter: 'Enter', return: 'Enter', esc: 'Escape', escape: 'Escape', tab: 'Tab', backspace: 'Backspace', delete: 'Delete', up: 'ArrowUp', down: 'ArrowDown', left: 'ArrowLeft', right: 'ArrowRight', arrowup: 'ArrowUp', arrowdown: 'ArrowDown', arrowleft: 'ArrowLeft', arrowright: 'ArrowRight', home: 'Home', end: 'End', pageup: 'PageUp', pagedown: 'PageDown', space: ' ', ' ': ' ' };
    return m[lc(k)] || (k.length === 1 ? k : null);
  }

  function pressOn(el, key) {
    var k = KEYS[key] || { code: key.length === 1 ? 'Key' + key.toUpperCase() : key, keyCode: key.length === 1 ? key.toUpperCase().charCodeAt(0) : 0 };
    var kk = k.key || key;
    var init = { key: kk, code: k.code, keyCode: k.keyCode, which: k.keyCode };
    var KB = g.KeyboardEvent;
    var down = fire(el, 'keydown', init, KB);
    var res = { ok: true, key: key, submitted: false };
    var tg = tag(el);
    if (!down.defaultPrevented) {
      if (key === 'Enter') {
        fire(el, 'keypress', init, KB);
        var form = el.closest ? el.closest('form') : null;
        if (tg === 'textarea' || isEditable(el)) { /* a new line, not a submit */ }
        else if (tg === 'button' || tag(el) === 'a' || attr(el, 'role') === 'button') { if (typeof el.click === 'function') el.click(); }
        else if (form && tg === 'input') {
          if (typeof form.requestSubmit === 'function') { try { form.requestSubmit(); res.submitted = true; } catch (e) { var sb = form.querySelector('[type=submit]'); if (sb && sb.click) { sb.click(); res.submitted = true; } } }
          else { var ev = fire(form, 'submit', {}); res.submitted = true; if (!ev.defaultPrevented && typeof form.submit === 'function') form.submit(); }
        }
      } else if (key === ' ' && (tg === 'button' || attr(el, 'role') === 'button') && typeof el.click === 'function') el.click();
      else if (key === 'Tab') {
        var f = [].slice.call(doc.querySelectorAll('a[href],button,input:not([type=hidden]),select,textarea,[tabindex]')).filter(function (x) { return isShown(x) && !isDisabled(x) && attr(x, 'tabindex') !== '-1'; });
        var i = f.indexOf(el), nxt = f[(i + 1) % Math.max(f.length, 1)];
        if (nxt && typeof nxt.focus === 'function') nxt.focus();
      } else if (key === 'Escape' || key.indexOf('Arrow') === 0 || key === 'Home' || key === 'End') { /* events only */ }
      else if (key.length === 1 && (tg === 'input' || tg === 'textarea')) {
        setNative(el, String(el.value || '') + key); inputEvents(el, key, 'insertText');
      } else if (key === 'Backspace' && (tg === 'input' || tg === 'textarea')) {
        setNative(el, String(el.value || '').slice(0, -1)); inputEvents(el, null, 'deleteContentBackward');
      }
    }
    fire(el, 'keyup', init, KB);
    return res;
  }

  function press(a) {
    var key = normKey(a.key);
    if (!key) return fail('Unknown key "' + cut(a.key, 20) + '". Use Enter, Escape, Tab, Backspace, Delete, ArrowUp/Down/Left/Right, Home, End, PageUp, PageDown, Space or a single character.');
    var el = a.id ? byId(a.id) : (doc.activeElement && doc.activeElement !== doc.body ? doc.activeElement : doc.body);
    if (!el) return fail('There is no element ' + a.id + ' on the page now. Take a new snapshot and use an id from it.', { stale: true });
    if (a.id && !isShown(el)) return fail('Element ' + a.id + ' is not visible.', { hidden: true });
    var r = pressOn(el, key);
    r.target = el === doc.body ? 'page' : (idOf(el));
    return r;
  }

  function scroll(a) {
    var dir = lc(a.dir || a.direction || 'down'), pages = a.pages || 1;
    var el = a.id ? byId(a.id) : null;
    if (a.id && !el) return fail('There is no element ' + a.id + ' on the page now. Take a new snapshot and use an id from it.', { stale: true });
    var before, after, max;
    var amount = function (h) { return Math.round(h * 0.85 * pages); };
    if (el) {
      var scrollable = el;
      // a plain element is scrolled into view; a scrolling container is scrolled by itself
      var s = styleOf(el);
      var isBox = s && (s.overflowY === 'auto' || s.overflowY === 'scroll') && el.scrollHeight > el.clientHeight;
      if (!isBox && dir === 'into' ) { scrollTo(el); return { ok: true, scrolled: 'into view' }; }
      if (isBox) {
        before = el.scrollTop;
        if (dir === 'up') el.scrollTop = Math.max(0, before - amount(el.clientHeight));
        else if (dir === 'top') el.scrollTop = 0;
        else if (dir === 'bottom') el.scrollTop = el.scrollHeight;
        else el.scrollTop = before + amount(el.clientHeight);
        after = el.scrollTop; max = el.scrollHeight - el.clientHeight;
        return { ok: true, scrolled: 'container', from: Math.round(before), to: Math.round(after), max: Math.round(max) };
      }
      scrollTo(el);
      return { ok: true, scrolled: 'into view' };
    }
    var de = doc.documentElement, h = g.innerHeight || 720;
    before = g.scrollY || 0;
    max = Math.max(0, (de.scrollHeight || 0) - h);
    var y = before;
    if (dir === 'up') y = Math.max(0, before - amount(h));
    else if (dir === 'top') y = 0;
    else if (dir === 'bottom') y = max;
    else y = before + amount(h);
    try { g.scrollTo(0, y); } catch (e) {}
    after = g.scrollY || 0;
    return { ok: true, scrolled: 'page', from: Math.round(before), to: Math.round(after), max: Math.round(max), at_bottom: HAS_LAYOUT ? after >= max - 2 : true };
  }

  function optionsOf(el) {
    return [].slice.call(el.options || el.querySelectorAll('option'));
  }

  function select(a) {
    var t = target(a, true);
    if (t.err) return t.err;
    var el = t.el, want = String(a.option == null ? '' : a.option), w = lc(norm(want));
    if (tag(el) === 'select') {
      scrollTo(el); highlight(el);
      var opts = optionsOf(el), hit = -1;
      for (var i = 0; i < opts.length && hit < 0; i++) if (lc(norm(opts[i].textContent)) === w || lc(attr(opts[i], 'value')) === w) hit = i;
      for (var j = 0; j < opts.length && hit < 0; j++) if (lc(norm(opts[j].textContent)).indexOf(w) >= 0) hit = j;
      if (hit < 0) return fail('No option "' + cut(want, 40) + '". Options: ' + opts.slice(0, 30).map(function (o) { return '"' + cut(norm(o.textContent), 30) + '"'; }).join(', '), { options: opts.slice(0, 50).map(function (o) { return norm(o.textContent); }) });
      if (attr(opts[hit], 'disabled') !== null) return fail('That option is disabled.');
      try { el.focus && el.focus(); } catch (e) {}
      for (var k = 0; k < opts.length; k++) { if (k === hit) opts[k].setAttribute('selected', ''); else opts[k].removeAttribute('selected'); try { opts[k].selected = k === hit; } catch (e) {} }
      try { el.selectedIndex = hit; } catch (e) {}
      try { var v = attr(opts[hit], 'value'); el.value = v == null ? norm(opts[hit].textContent) : v; } catch (e) {}
      el.__iyakkiTouched = true;
      fire(el, 'input', {}, g.Event); fire(el, 'change', {});
      return { ok: true, id: a.id, selected: norm(opts[hit].textContent), value: attr(opts[hit], 'value') };
    }
    // custom combobox / listbox: open it, then click the option with that text
    scrollTo(el); highlight(el);
    var list = null;
    var findOpts = function () {
      var ctl = attr(el, 'aria-controls') || attr(el, 'aria-owns');
      var scope = doc;
      if (ctl) { var c = doc.getElementById(ctl.split(/\s+/)[0]); if (c) scope = c; }
      var found = [].slice.call(scope.querySelectorAll('[role=option],[role=menuitem],[role=treeitem],li[role],option')).filter(isShown);
      if (!found.length && scope === doc) found = [].slice.call(doc.querySelectorAll('[role=listbox] *')).filter(function (x) { return roleOf(x) === 'option' && isShown(x); });
      return found;
    };
    list = findOpts();
    var opened = false;
    if (!list.length) {
      var r = click({ id: a.id });
      if (!r.ok) return r;
      opened = true;
      list = findOpts();
    }
    var pick = null;
    for (var m = 0; m < list.length && !pick; m++) if (lc(norm(list[m].textContent)) === w) pick = list[m];
    for (var n = 0; n < list.length && !pick; n++) if (lc(norm(list[n].textContent)).indexOf(w) >= 0) pick = list[n];
    if (!pick) return fail(list.length ? 'No option "' + cut(want, 40) + '" in the list. Options: ' + list.slice(0, 20).map(function (o) { return '"' + cut(norm(o.textContent), 30) + '"'; }).join(', ') : 'The list of options is not visible yet' + (opened ? ' (the control was opened; take a snapshot and click the option, or try again in a moment)' : '') + '.', { opened: opened, options: list.slice(0, 30).map(function (o) { return norm(o.textContent); }), retry: !list.length });
    var cr = click({ id: idOf(pick) });
    if (!cr.ok) return cr;
    return { ok: true, id: a.id, selected: norm(pick.textContent), custom: true };
  }

  function hover(a) {
    var t = target(a, false);
    if (t.err) return t.err;
    var el = t.el;
    scrollTo(el); highlight(el);
    var c = centre(el), m = { clientX: c.x, clientY: c.y, view: g };
    var P = g.PointerEvent || g.MouseEvent, M = g.MouseEvent;
    fire(el, 'pointerover', m, P); fire(el, 'pointerenter', Object.assign({ bubbles: false }, m), P);
    fire(el, 'mouseover', m, M); fire(el, 'mouseenter', Object.assign({ bubbles: false }, m), M); fire(el, 'mousemove', m, M);
    return { ok: true, id: a.id };
  }

  // -------------------------------------------------------------------------------------------- forms
  function validationOf(el) {
    var msg = '';
    try { msg = el.validationMessage || ''; } catch (e) {}
    if (!msg) {
      var ids = [attr(el, 'aria-errormessage'), attr(el, 'aria-describedby')].filter(Boolean).join(' ').split(/\s+/);
      for (var i = 0; i < ids.length && !msg; i++) {
        if (!ids[i]) continue;
        var n = doc.getElementById(ids[i]);
        if (n && isShown(n) && norm(n.textContent) && (lc(attr(el, 'aria-invalid')) === 'true' || /error|invalid|required|must|please/i.test(norm(n.textContent) + ' ' + (attr(n, 'class') || '') + ' ' + (attr(n, 'role') || '')))) msg = norm(n.textContent);
      }
    }
    if (!msg) {
      var p = el.parentElement, sib = null;
      for (var up = 0; up < 2 && p && !msg; up++, p = p.parentElement) {
        var cand = p.querySelectorAll ? p.querySelectorAll('[class*=error],[class*=invalid],[role=alert],.help-block,[class*=feedback]') : [];
        for (var c = 0; c < cand.length; c++) if (cand[c] !== el && isShown(cand[c]) && norm(cand[c].textContent) && !cand[c].contains(el)) { msg = norm(cand[c].textContent); break; }
      }
    }
    return cut(norm(msg), 160);
  }

  function readForm() {
    var all = allElements();
    var forms = [], loose = [], byForm = new Map();
    var fieldOf = function (it) {
      var el = it.el, tg = tag(el), r = it.role;
      var f = { id: it.id, label: it.name, role: r, type: tg === 'input' ? inputType(el) : (tg === 'select' ? 'select' : (tg === 'textarea' ? 'textarea' : (r === 'combobox' ? 'combobox' : r))) };
      f.required = attr(el, 'required') !== null || lc(attr(el, 'aria-required')) === 'true';
      f.disabled = isDisabled(el);
      f.visible = isShown(el);
      var secret = describeEl(el).secret;
      if (secret) f.secret = secret;
      f.value = it.value;
      if (r === 'checkbox' || r === 'radio') f.checked = it.state.indexOf('checked') >= 0;
      if (r === 'radio' && attr(el, 'name')) f.group = attr(el, 'name');
      if (tg === 'select') f.options = optionsOf(el).slice(0, 40).map(function (o) { return norm(o.textContent); });
      var ph = attr(el, 'placeholder'); if (ph && ph !== it.name) f.placeholder = cut(ph, 40);
      ['min', 'max', 'pattern', 'maxlength', 'minlength', 'step'].forEach(function (k) { var v = attr(el, k); if (v !== null) f[k] = v; });
      if (attr(el, 'list')) f.autocomplete_list = true;
      if (lc(attr(el, 'aria-autocomplete')) === 'list' || lc(attr(el, 'role')) === 'combobox') f.autocomplete = true;
      var m = validationOf(el);
      if (m) f.message = m;
      if (lc(attr(el, 'aria-invalid')) === 'true') f.invalid = true;
      if (tg === 'input' && inputType(el) === 'file') f.unsupported = 'file input';
      return f;
    };
    all.items.forEach(function (it) {
      var el = it.el;
      var isField = ['textbox', 'checkbox', 'radio', 'select', 'combobox', 'slider', 'file', 'listbox', 'spinbutton'].indexOf(it.role) >= 0 || (it.role === 'button' && false);
      var isSubmit = it.role === 'button' && (tag(el) === 'button' || tag(el) === 'input') && (attr(el, 'type') === null || attr(el, 'type') === 'submit') && el.closest && el.closest('form');
      if (!isField && !isSubmit) return;
      var form = el.closest ? el.closest('form') : null;
      var bucket;
      if (form) {
        bucket = byForm.get(form);
        if (!bucket) { bucket = { form: attr(form, 'id') || attr(form, 'name') || ('form' + (forms.length + 1)), action_host: hostOf(attr(form, 'action') || g.location.href), method: lc(attr(form, 'method') || 'get'), fields: [], submit: [] }; byForm.set(form, bucket); forms.push(bucket); }
      } else bucket = { fields: loose, submit: [] };
      if (isField) bucket.fields.push(fieldOf(it));
      else bucket.submit.push({ id: it.id, label: it.name });
    });
    var text = [];
    forms.forEach(function (f) {
      text.push('form ' + f.form + (f.action_host ? ' -> ' + f.action_host : '') + ' (' + f.method + ')');
      f.fields.forEach(function (x) { text.push('  ' + fmtField(x)); });
      f.submit.forEach(function (s) { text.push('  submit: ' + s.id + ' "' + s.label + '"'); });
    });
    if (loose.length) { text.push('fields outside a form'); loose.forEach(function (x) { text.push('  ' + fmtField(x)); }); }
    if (!text.length) text.push('No form fields are visible on this page.');
    var invalid = 0;
    forms.concat([{ fields: loose }]).forEach(function (f) { f.fields.forEach(function (x) { if (x.message || x.invalid) invalid++; }); });
    return { ok: true, forms: forms, loose: loose, text: text.join('\n'), invalid: invalid };
  }

  function fmtField(x) {
    var s = x.id + ' ' + x.type + ' "' + x.label + '"';
    if (x.required) s += ' *required';
    if (x.value) s += ' value="' + x.value + '"';
    if (x.checked !== undefined) s += x.checked ? ' [checked]' : ' [unchecked]';
    if (x.group) s += ' group=' + x.group;
    if (x.options) s += ' options=[' + x.options.slice(0, 12).map(function (o) { return '"' + cut(o, 24) + '"'; }).join(', ') + (x.options.length > 12 ? ', ...' : '') + ']';
    if (x.min !== undefined || x.max !== undefined) s += ' range=' + (x.min || '') + '..' + (x.max || '');
    if (x.pattern) s += ' pattern=' + x.pattern;
    if (x.autocomplete || x.autocomplete_list) s += ' [suggestions appear as you type]';
    if (x.secret) s += ' [' + x.secret + ' field: the person fills it in]';
    if (x.unsupported) s += ' [' + x.unsupported + ': not supported]';
    if (x.disabled) s += ' [disabled]';
    if (!x.visible) s += ' [hidden]';
    if (x.message) s += ' !! ' + x.message;
    else if (x.invalid) s += ' !! invalid';
    return s;
  }

  // -------------------------------------------------------------------------------------------- wait, secrets, state
  function probe(a) {
    var bodyText = '';
    try { bodyText = norm(doc.body ? (doc.body.innerText || doc.body.textContent) : ''); } catch (e) {}
    var r = { ok: true, ready: doc.readyState, inflight: st.inflight, quiet_ms: now() - st.lastMutation, url: g.location.href, title: cut(norm(doc.title), 120) };
    if (a && a.text) r.text_found = lc(bodyText).indexOf(lc(a.text)) >= 0;
    if (a && a.gone_text) r.gone = lc(bodyText).indexOf(lc(a.gone_text)) < 0;
    if (a && (a.role || a.name)) {
      var f = find({ role: a.role, name: a.name, label: a.label, exact: a.exact });
      r.element_count = f.count;
      r.element_id = f.matches.length ? f.matches[0].id : '';
    }
    return r;
  }

  function secretPrepare(a) {
    var el = byId(a.id);
    if (!el) return fail('There is no element ' + a.id + ' on the page now.', { stale: true });
    if (!isShown(el)) return fail('Element ' + a.id + ' is not visible.');
    scrollTo(el); highlight(el);
    try { el.focus && el.focus(); } catch (e) {}
    if (tag(el) === 'input' || tag(el) === 'textarea') {
      if (a.clear !== false && el.value) { setNative(el, ''); inputEvents(el, null, 'deleteContent'); }
      try { el.select && el.select(); } catch (e) {}
    }
    return { ok: true, id: a.id, focused: doc.activeElement === el };
  }

  // focus an element (no selecting, no clearing) so that real key events from the shell reach it
  function focusOp(a) {
    var t = target(a, false);
    if (t.err) return t.err;
    scrollTo(t.el);
    try { if (typeof t.el.focus === 'function') t.el.focus(); } catch (e) {}
    return { ok: true, id: a.id, focused: doc.activeElement === t.el };
  }

  // after the shell typed the secret with real input: only the LENGTH is reported, never the value
  function secretCheck(a) {
    var el = byId(a.id);
    if (!el) return fail('stale', { stale: true });
    var len = String(el.value == null ? '' : el.value).length;
    return { ok: true, id: a.id, length: len, matches: len === a.length };
  }

  // fallback when typed input did not arrive: set the value natively (the secret travels in the script text for this one call)
  function secretSet(a) {
    var el = byId(a.id);
    if (!el) return fail('stale', { stale: true });
    setNative(el, String(a.value == null ? '' : a.value));
    inputEvents(el, null, 'insertText');
    return { ok: true, id: a.id, length: String(el.value).length };
  }

  function nav(a) {
    var op = a.op;
    if (op === 'back') g.history.back();
    else if (op === 'forward') g.history.forward();
    else if (op === 'reload') g.location.reload();
    else if (op === 'goto') g.location.assign(String(a.url));
    else return fail('unknown navigation ' + op);
    return { ok: true };
  }

  function chatTail(a) {
    // the newest text blocks of chat-like containers, for "what did it answer?"
    var w = allElements().walk;
    var n = a && a.n || 3, res = [];
    var groups = {};
    w.items.forEach(function (i) { if (i.chat && (i.kind === 'text' || i.kind === 'heading')) (groups[i.chat] = groups[i.chat] || []).push(i.text); });
    Object.keys(groups).forEach(function (k) { res.push(groups[k].slice(-n)); });
    return { ok: true, containers: res };
  }

  var OPS = { snapshot: snapshot, describe: describe, find: find, click: click, type: type, press: press, scroll: scroll, select: select, hover: hover, read_form: readForm, probe: probe, fill_secret_prepare: secretPrepare, focus: focusOp, secret_check: secretCheck, secret_set: secretSet, nav: nav, chat_tail: chatTail };

  g.__iyakkiAuto = {
    v: VERSION,
    layout: HAS_LAYOUT,
    run: function (op, argsJson) {
      var a = {};
      try { a = argsJson ? JSON.parse(argsJson) : {}; } catch (e) { return out(fail('bad arguments')); }
      var f = OPS[op];
      if (!f) return out(fail('unknown operation ' + op));
      try { return out(f(a)); } catch (e) { return out(fail('script error in ' + op + ': ' + cut(String(e && e.message || e), 200))); }
    },
  };
})();
