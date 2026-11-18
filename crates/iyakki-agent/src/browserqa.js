/* IYAKKI browser QA script: runs inside the page (injected by browserqa.rs). No network, no eval of page strings; every
   operation gets its arguments as JSON text and answers JSON text. Selectors are CSS. Kept to plain functions so that
   the in-house engine and WebKit run it the same way. */
(function (g) {
  'use strict';
  if (g.__iyakkiQA && g.__iyakkiQA.v === 1) return;
  var QA = { v: 1 };
  var LOG = [];
  var LOG_READ = 0;
  var MAX_LOG = 400;

  function fmt(a) {
    if (typeof a === 'string') return a;
    try {
      if (a && a.stack && a.message !== undefined) return String(a.message) + (a.stack ? ' ' + String(a.stack).split('\n')[0] : '');
      if (typeof a === 'object' && a !== null) return JSON.stringify(a);
    } catch (e) {}
    return String(a);
  }
  function push(level, text) {
    if (LOG.length >= MAX_LOG) return;
    LOG.push({ level: level, text: String(text).slice(0, 600), at: Date.now() });
  }
  try {
    var c = g.console || {};
    ['log', 'info', 'warn', 'error', 'debug'].forEach(function (lv) {
      var orig = c[lv];
      c[lv] = function () {
        try { push(lv, Array.prototype.map.call(arguments, fmt).join(' ')); } catch (e) {}
        if (typeof orig === 'function') return orig.apply(c, arguments);
      };
    });
    g.console = c;
  } catch (e) {}
  try {
    g.addEventListener('error', function (ev) {
      var t = ev && ev.target;
      if (t && t !== g && t.tagName) {
        push('resource', 'failed to load <' + String(t.tagName).toLowerCase() + '> ' + (t.src || t.href || ''));
      } else {
        push('exception', (ev && ev.message) || 'script error');
      }
    }, true);
    g.addEventListener('unhandledrejection', function (ev) { push('unhandledrejection', fmt(ev && ev.reason)); });
  } catch (e) {}

  /* ---- finding things */
  function root(a) {
    var d = g.document;
    var path = (a && a.frame) || [];
    for (var i = 0; i < path.length; i++) {
      var f = d.querySelector(path[i]);
      if (!f) throw new Error('the frame ' + path[i] + ' is not on the page');
      var inner = null;
      try { inner = f.contentDocument || (f.contentWindow && f.contentWindow.document); } catch (e) { inner = null; }
      if (!inner) throw new Error('the frame ' + path[i] + ' is from another site or is not readable');
      d = inner;
    }
    return d;
  }
  function all(a) {
    var d = root(a);
    var list;
    try { list = d.querySelectorAll(a.selector); } catch (e) { throw new Error('"' + a.selector + '" is not a valid selector'); }
    return Array.prototype.slice.call(list);
  }
  function pick(a, required) {
    var list = all(a);
    var i = typeof a.index === 'number' ? a.index : 0;
    if (i < 0) i = list.length + i;
    var el = list[i] || null;
    if (!el && required) throw new Error('no element matches "' + a.selector + '"' + (list.length ? ' at position ' + a.index + ' (there are ' + list.length + ')' : ''));
    return { el: el, count: list.length };
  }
  function style(el) {
    try { return (el.ownerDocument.defaultView || g).getComputedStyle(el); } catch (e) { return null; }
  }
  function hasLayout(el) {
    try {
      var de = el.ownerDocument.documentElement;
      var r = de && de.getBoundingClientRect();
      return !!(r && (r.width > 0 || r.height > 0));
    } catch (e) { return false; }
  }
  function visible(el) {
    if (!el || el.hidden) return false;
    var s = style(el);
    if (s) {
      if (s.display === 'none' || s.visibility === 'hidden' || s.visibility === 'collapse') return false;
      if (s.opacity === '0') return false;
    }
    for (var p = el.parentElement; p; p = p.parentElement) {
      if (p.hidden) return false;
      var ps = style(p);
      if (ps && ps.display === 'none') return false;
    }
    // with real layout (a browser), an element that takes no room is not visible; an engine without layout cannot tell
    if (hasLayout(el)) {
      try {
        var r = el.getBoundingClientRect();
        if (r && r.width === 0 && r.height === 0 && el.tagName !== 'INPUT') return false;
      } catch (e) {}
    }
    return true;
  }
  function enabled(el) {
    if (el.disabled) return false;
    for (var p = el.parentElement; p; p = p.parentElement) {
      if (p.tagName === 'FIELDSET' && p.disabled) return false;
    }
    return el.getAttribute && el.getAttribute('aria-disabled') !== 'true';
  }
  function textOf(el) {
    var t = el.innerText;
    if (t === undefined || t === null || t === '') t = el.textContent;
    return String(t || '').replace(/[ \t\r\f\v]+/g, ' ').replace(/\n\s*\n+/g, '\n').trim();
  }
  var SECRETISH = /pass|pwd|token|secret|otp|cvv|cvc|card|csrf|xsrf|auth|session|apikey|api-key|api_key|private|ssn/i;
  function isSecretField(el) {
    if (!el || !el.tagName) return false;
    var t = (el.getAttribute('type') || '').toLowerCase();
    if (t === 'password') return true;
    var tag = el.tagName;
    if (tag === 'INPUT' || tag === 'TEXTAREA') {
      var ac = (el.getAttribute('autocomplete') || '').toLowerCase();
      if (/password|one-time-code|cc-/.test(ac)) return true;
      if (t === 'hidden' && SECRETISH.test((el.getAttribute('name') || '') + ' ' + (el.id || ''))) return true;
      if (SECRETISH.test((el.getAttribute('name') || '') + ' ' + (el.id || ''))) return t !== 'checkbox' && t !== 'radio' && t !== 'submit' && t !== 'button';
    }
    return false;
  }
  function attrsOf(el) {
    var o = {};
    var secret = isSecretField(el);
    for (var i = 0; el.attributes && i < el.attributes.length; i++) {
      var at = el.attributes[i];
      var n = at.name;
      o[n] = ((secret && n === 'value') || (/^(data-)?(csrf|xsrf|token|auth|secret)/i.test(n))) ? '[redacted]' : String(at.value).slice(0, 500);
    }
    return o;
  }
  function describe(el) {
    var v;
    if (isSecretField(el)) v = '[redacted]';
    else if (el.tagName === 'SELECT') {
      var so = el.options && el.options[el.selectedIndex];
      v = so ? (so.value !== undefined ? so.value : so.text) : '';
    } else v = el.value;
    return {
      tag: String(el.tagName || '').toLowerCase(),
      id: el.id || '',
      text: textOf(el).slice(0, 2000),
      value: v === undefined ? null : String(v),
      checked: el.checked === undefined ? null : !!el.checked,
      visible: visible(el),
      enabled: enabled(el),
      attrs: attrsOf(el)
    };
  }
  function scrub(root_) {
    var clone = root_.cloneNode(true);
    var list = clone.querySelectorAll ? clone.querySelectorAll('input,textarea,meta,script') : [];
    for (var i = 0; i < list.length; i++) {
      var el = list[i];
      if (el.tagName === 'META') {
        if (SECRETISH.test((el.getAttribute('name') || '') + ' ' + (el.getAttribute('property') || ''))) el.setAttribute('content', '[redacted]');
        continue;
      }
      if (el.tagName === 'SCRIPT') continue;
      if (isSecretField(el)) {
        el.setAttribute('value', '[redacted]');
        if (el.tagName === 'TEXTAREA') el.textContent = '[redacted]';
      }
    }
    var withAttrs = clone.querySelectorAll ? clone.querySelectorAll('[data-token],[data-csrf],[data-auth],[data-secret],[data-api-key]') : [];
    for (var j = 0; j < withAttrs.length; j++) {
      ['data-token', 'data-csrf', 'data-auth', 'data-secret', 'data-api-key'].forEach(function (n) { if (withAttrs[j].hasAttribute(n)) withAttrs[j].setAttribute(n, '[redacted]'); });
    }
    return clone;
  }
  function fire(el, type, init) {
    var ev;
    try {
      if (type.indexOf('mouse') === 0 || type === 'click' || type === 'dblclick' || type === 'contextmenu') ev = new MouseEvent(type, Object.assign({ bubbles: true, cancelable: true, view: g }, init || {}));
      else if (type === 'pointerover' || type === 'pointerenter' || type === 'pointermove' || type === 'pointerdown' || type === 'pointerup') ev = (typeof PointerEvent === 'function') ? new PointerEvent(type, Object.assign({ bubbles: true, cancelable: true }, init || {})) : new MouseEvent(type.replace('pointer', 'mouse'), Object.assign({ bubbles: true, cancelable: true }, init || {}));
      else ev = new Event(type, Object.assign({ bubbles: true, cancelable: true }, init || {}));
    } catch (e) {
      ev = new Event(type, { bubbles: true, cancelable: true });
    }
    el.dispatchEvent(ev);
  }
  function scrollTo_(el) {
    try { el.scrollIntoView({ block: 'center', inline: 'nearest' }); } catch (e) { try { el.scrollIntoView(); } catch (e2) {} }
  }
  function setValue(el, v) {
    var proto = el.tagName === 'TEXTAREA' ? g.HTMLTextAreaElement : el.tagName === 'SELECT' ? g.HTMLSelectElement : g.HTMLInputElement;
    var desc = proto && proto.prototype && Object.getOwnPropertyDescriptor(proto.prototype, 'value');
    if (desc && desc.set) desc.set.call(el, v); else el.value = v;
  }

  /* ---- operations */
  var OPS = {};
  OPS.hook = function () { return { ok: true, installed: true }; };
  OPS.probe = function () {
    return { ok: true, url: String(g.location.href), title: String(g.document.title || ''), ready: g.document.readyState, width: g.innerWidth, height: g.innerHeight, scroll_h: Math.max(g.document.documentElement ? g.document.documentElement.scrollHeight : 0, g.document.body ? g.document.body.scrollHeight : 0), dpr: g.devicePixelRatio || 1, scroll_y: g.pageYOffset || 0 };
  };
  OPS.console = function (a) {
    var out = LOG.slice(LOG_READ);
    if (!a || a.mark !== false) LOG_READ = LOG.length;
    return { ok: true, messages: out, total: LOG.length };
  };
  OPS.nav = function () {
    var r = { ok: true, status: null, type: null };
    try {
      var e = g.performance && g.performance.getEntriesByType && g.performance.getEntriesByType('navigation');
      if (e && e[0]) { r.status = e[0].responseStatus === undefined ? null : e[0].responseStatus; r.type = e[0].type; }
    } catch (x) {}
    return r;
  };
  OPS.info = function (a) {
    var p = pick(a, false);
    var o = { ok: true, count: p.count, exists: !!p.el };
    if (p.el) { var d = describe(p.el); for (var k in d) o[k] = d[k]; }
    return o;
  };
  OPS.page_text = function (a) {
    var d = root(a);
    var b = d.body || d.documentElement;
    return { ok: true, text: textOf(b), url: String(g.location.href), title: String(g.document.title || '') };
  };
  OPS.dom = function (a) {
    var d = root(a);
    var mode = a.mode || 'page';
    var cap = a.max || 200000;
    var o = { ok: true, url: String(g.location.href), title: String(g.document.title || '') };
    function cut(s) { o.bytes = s.length; if (s.length > cap) { o.truncated = true; return s.slice(0, cap); } o.truncated = false; return s; }
    if (mode === 'page') {
      o.html = cut(scrub(d.documentElement).outerHTML);
    } else if (mode === 'outer_html' || mode === 'text' || mode === 'attributes') {
      var p = pick(a, true);
      o.matches = p.count;
      if (mode === 'outer_html') o.outer_html = cut(scrub(p.el).outerHTML);
      else if (mode === 'text') o.text = cut(textOf(p.el));
      else o.attributes = attrsOf(p.el);
    }
    if (a.links) {
      var ls = d.querySelectorAll('a[href]');
      o.links = [];
      for (var i = 0; i < ls.length && i < 500; i++) o.links.push({ href: ls[i].href || ls[i].getAttribute('href'), text: textOf(ls[i]).slice(0, 120) });
    }
    if (a.failures) {
      var bad = [];
      var imgs = d.querySelectorAll('img');
      for (var j = 0; j < imgs.length && j < 500; j++) { if (imgs[j].complete === true && imgs[j].naturalWidth === 0 && imgs[j].getAttribute('src')) bad.push({ tag: 'img', url: imgs[j].src || imgs[j].getAttribute('src') }); }
      o.failed_loads = bad;
    }
    return o;
  };
  OPS.rect = function (a) {
    var p = pick(a, true);
    scrollTo_(p.el);
    var r = p.el.getBoundingClientRect();
    return { ok: true, x: r.left, y: r.top, w: r.width, h: r.height, cx: Math.round(r.left + r.width / 2), cy: Math.round(r.top + r.height / 2), vw: g.innerWidth, vh: g.innerHeight, dpr: g.devicePixelRatio || 1 };
  };
  OPS.click = function (a) {
    var p = pick(a, true);
    var el = p.el;
    if (!visible(el)) throw new Error('"' + a.selector + '" is not visible, so it cannot be clicked');
    if (!enabled(el)) throw new Error('"' + a.selector + '" is disabled, so it cannot be clicked');
    scrollTo_(el);
    try { el.focus(); } catch (e) {}
    var r = el.getBoundingClientRect();
    var init = { clientX: Math.round(r.left + r.width / 2), clientY: Math.round(r.top + r.height / 2), button: 0, detail: 1 };
    fire(el, 'pointerdown', init); fire(el, 'mousedown', init); fire(el, 'pointerup', init); fire(el, 'mouseup', init);
    // a click on a submit button submits its form; an engine whose click() does not do that is helped (once)
    var form = el.form || (el.closest && el.closest('form'));
    var type_ = String(el.getAttribute('type') || '').toLowerCase();
    var submits = !!form && ((el.tagName === 'BUTTON' && (type_ === '' || type_ === 'submit')) || (el.tagName === 'INPUT' && (type_ === 'submit' || type_ === 'image')));
    var submitted = false;
    var mark = function () { submitted = true; };
    if (submits) form.addEventListener('submit', mark, true);
    if (typeof el.click === 'function') el.click(); else fire(el, 'click', init);
    if (submits) {
      form.removeEventListener('submit', mark, true);
      if (!submitted) OPS.submit({ selector: null, frame: a.frame, form: form });
    }
    if (a.double) { fire(el, 'dblclick', init); }
    return { ok: true, tag: String(el.tagName).toLowerCase(), text: textOf(el).slice(0, 80) };
  };
  OPS.hover = function (a) {
    var p = pick(a, true);
    scrollTo_(p.el);
    var r = p.el.getBoundingClientRect();
    var init = { clientX: Math.round(r.left + r.width / 2), clientY: Math.round(r.top + r.height / 2) };
    ['pointerover', 'pointerenter', 'mouseover', 'mouseenter', 'pointermove', 'mousemove'].forEach(function (t) { fire(p.el, t, init); });
    return { ok: true };
  };
  OPS.focus = function (a) {
    var p = pick(a, true);
    scrollTo_(p.el);
    try { p.el.focus(); } catch (e) {}
    return { ok: true };
  };
  OPS.type = function (a) {
    var p = pick(a, true);
    var el = p.el;
    if (!enabled(el)) throw new Error('"' + a.selector + '" is disabled');
    scrollTo_(el);
    try { el.focus(); } catch (e) {}
    var text = String(a.text === undefined ? '' : a.text);
    if (el.isContentEditable || el.getAttribute('contenteditable') === 'true') {
      el.textContent = a.clear === false ? (el.textContent + text) : text;
      fire(el, 'input');
    } else if ('value' in el) {
      var t = (el.getAttribute('type') || '').toLowerCase();
      if (t === 'file') throw new Error('a file input cannot be typed into');
      setValue(el, a.clear === false ? String(el.value || '') + text : text);
      fire(el, 'input');
      fire(el, 'change');
    } else throw new Error('"' + a.selector + '" is not something text can be typed into');
    return { ok: true, length: text.length, secret: isSecretField(el) };
  };
  OPS.fill_one = function (a) {
    var p = pick(a, true);
    var el = p.el;
    if (!enabled(el)) throw new Error('"' + a.selector + '" is disabled');
    var tag = el.tagName;
    var t = (el.getAttribute('type') || '').toLowerCase();
    var v = a.value;
    scrollTo_(el);
    if (tag === 'INPUT' && t === 'file') throw new Error('FILE');
    if (tag === 'INPUT' && (t === 'checkbox' || t === 'radio')) {
      var want = (typeof v === 'boolean') ? v : /^(true|yes|on|1|checked)$/i.test(String(v));
      if (t === 'radio' && !a.valueIsBool) { want = true; }
      if (!!el.checked !== want) { if (typeof el.click === 'function') el.click(); }
      if (!!el.checked !== want) { el.checked = want; fire(el, 'input'); fire(el, 'change'); }
      return { ok: true, kind: t, checked: !!el.checked };
    }
    if (tag === 'SELECT') return sel(el, v, 'auto');
    try { el.focus(); } catch (e) {}
    if (el.isContentEditable) { el.textContent = String(v); fire(el, 'input'); return { ok: true, kind: 'editable' }; }
    if (!('value' in el)) throw new Error('"' + a.selector + '" is not a form field');
    setValue(el, String(v));
    fire(el, 'input'); fire(el, 'change');
    return { ok: true, kind: tag === 'TEXTAREA' ? 'textarea' : (t || 'text'), secret: isSecretField(el) };
  };
  function sel(el, option, by) {
    var opts = Array.prototype.slice.call(el.options || []);
    var want = String(option);
    var found = -1;
    function find(fn) { for (var i = 0; i < opts.length; i++) if (fn(opts[i], i)) return i; return -1; }
    if (by === 'index' || (by === 'auto' && /^\d+$/.test(want) && find(function (o) { return o.value === want || o.text.trim() === want; }) < 0)) found = parseInt(want, 10);
    if (found < 0 && (by === 'value' || by === 'auto')) found = find(function (o) { return o.value === want; });
    if (found < 0 && (by === 'label' || by === 'auto')) found = find(function (o) { return o.text.trim() === want; });
    if (found < 0 && by === 'auto') found = find(function (o) { return o.text.trim().toLowerCase() === want.toLowerCase(); });
    if (found < 0 || found >= opts.length) throw new Error('no option "' + want + '"; the options are: ' + opts.slice(0, 12).map(function (o) { return '"' + o.text.trim() + '"'; }).join(', '));
    if (opts[found].disabled) throw new Error('the option "' + want + '" is disabled');
    el.selectedIndex = found;
    opts[found].selected = true;
    fire(el, 'input'); fire(el, 'change');
    return { ok: true, kind: 'select', selected: opts[found].text.trim(), value: opts[found].value };
  }
  OPS.select = function (a) {
    var p = pick(a, true);
    if (p.el.tagName !== 'SELECT') throw new Error('"' + a.selector + '" is not a drop-down list');
    scrollTo_(p.el);
    return sel(p.el, a.option, a.by || 'auto');
  };
  OPS.submit = function (a) {
    var el;
    if (a.form) el = a.form; else if (a.selector) el = pick(a, true).el; else el = root(a).querySelector('form');
    if (!el) throw new Error('there is no form on the page');
    var form = el.tagName === 'FORM' ? el : (el.form || (el.closest && el.closest('form')));
    if (!form) throw new Error('"' + (a.selector || 'form') + '" is not in a form');
    if (typeof form.requestSubmit === 'function') form.requestSubmit();
    else {
      var ev = new Event('submit', { bubbles: true, cancelable: true });
      if (form.dispatchEvent(ev) && typeof form.submit === 'function') form.submit();
    }
    return { ok: true };
  };
  OPS.key = function (a) {
    if (a.selector) { var p = pick(a, true); try { p.el.focus(); } catch (e) {} }
    return { ok: true };
  };
  OPS.keyjs = function (a) {
    var el = g.document.activeElement || g.document.body;
    var o = { key: a.key, bubbles: true, cancelable: true };
    fire(el, 'keydown', o); fire(el, 'keypress', o); fire(el, 'keyup', o);
    if (a.key === 'Enter' && el && el.form && el.tagName === 'INPUT') { OPS.submit({ selector: null, frame: a.frame, form: el.form }); }
    return { ok: true };
  };
  OPS.scroll = function (a) {
    var to = a.to || 'selector';
    if (to === 'selector') { var p = pick(a, true); scrollTo_(p.el); }
    else if (to === 'top') g.scrollTo(0, 0);
    else if (to === 'bottom') g.scrollTo(0, 1e9);
    else { var d = (typeof a.pixels === 'number' ? a.pixels : g.innerHeight) * (to === 'up' ? -1 : 1); g.scrollBy ? g.scrollBy(0, d) : g.scrollTo(0, (g.pageYOffset || 0) + d); }
    return { ok: true, scroll_y: g.pageYOffset || 0 };
  };
  OPS.scrollpos = function (a) { g.scrollTo(0, a.y || 0); return { ok: true, scroll_y: g.pageYOffset || 0 }; };
  OPS.extract = function (a) {
    var out = {};
    var f = a.fields || {};
    var d = root(a);
    Object.keys(f).forEach(function (name) {
      var spec = f[name];
      if (typeof spec === 'string') spec = { selector: spec };
      var list;
      try { list = Array.prototype.slice.call(d.querySelectorAll(spec.selector)); } catch (e) { throw new Error('"' + spec.selector + '" is not a valid selector'); }
      function val(el) {
        if (spec.attr) { var v = el.getAttribute(spec.attr); if (isSecretField(el) && spec.attr === 'value') return '[redacted]'; return v; }
        if (spec.html) return el.innerHTML;
        if (spec.value) return isSecretField(el) ? '[redacted]' : el.value;
        return textOf(el);
      }
      out[name] = spec.all ? list.map(val) : (list.length ? val(list[0]) : null);
    });
    var res = { ok: true, data: out };
    if (a.rows && a.rows.selector) {
      var rows = [];
      var rl = Array.prototype.slice.call(d.querySelectorAll(a.rows.selector));
      var cols = a.rows.columns || {};
      for (var i = 0; i < rl.length && i < 1000; i++) {
        var row = {};
        var any = false;
        Object.keys(cols).forEach(function (cn) {
          var c = rl[i].querySelector(typeof cols[cn] === 'string' ? cols[cn] : cols[cn].selector);
          if (c) any = true;
          row[cn] = c ? (cols[cn].attr ? c.getAttribute(cols[cn].attr) : textOf(c)) : null;
        });
        if (any) rows.push(row);
      }
      res.rows = rows;
    }
    return res;
  };
  OPS.js = function (a) {
    var fn = new Function('args', 'return (' + a.expression + ');');
    var v = fn(a.args);
    return { ok: true, value: v === undefined ? null : JSON.parse(JSON.stringify(v)) };
  };
  OPS.storage = function (a) {
    var k = a.kind || 'cookie';
    if (k === 'cookie') {
      var c = encodeURIComponent(a.name) + '=' + encodeURIComponent(a.value) + '; path=' + (a.path || '/');
      g.document.cookie = c;
      return { ok: true, kind: 'cookie' };
    }
    var s = k === 'session_storage' ? g.sessionStorage : g.localStorage;
    s.setItem(a.name, a.value);
    return { ok: true, kind: k };
  };
  OPS.frame_check = function (a) { root(a); return { ok: true }; };
  OPS.links = function (a) {
    var d = root(a);
    var ls = d.querySelectorAll('a[href]');
    var out = [];
    for (var i = 0; i < ls.length && i < 1000; i++) out.push({ href: ls[i].href || ls[i].getAttribute('href'), text: textOf(ls[i]).slice(0, 120) });
    return { ok: true, links: out };
  };

  QA.run = function (name, argsJson) {
    try {
      var a = argsJson ? JSON.parse(argsJson) : {};
      var fn = OPS[name];
      if (!fn) return JSON.stringify({ ok: false, error: 'unknown operation ' + name });
      return JSON.stringify(fn(a));
    } catch (e) {
      return JSON.stringify({ ok: false, error: String((e && e.message) || e) });
    }
  };
  g.__iyakkiQA = QA;
})(typeof window !== 'undefined' ? window : globalThis);
