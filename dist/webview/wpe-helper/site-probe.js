// Site probe (docs/WEBVIEW-PROTOCOL.md, KeySystem and PageMeta). Injected by helper.c into the top frame at document start.
// Tells the shell two things about a page, through the "iyakkiSite" message handler:
//   "keysys <name>"                        the page asked for a media key system (requestMediaKeySystemAccess), once per name: the shell says
//                                          plainly that this computer has no decryption module when the name needs one (Widevine, PlayReady,
//                                          FairPlay). The call itself is passed on unchanged: the engine answers as it always did.
//   "meta <manifest url>\x1f<manifest text>\x1f<icon url>"   once the document is ready: where the page's web app manifest and its icon
//                                          are, and the manifest's text when it could be read from the page's own origin (at most 100000 bytes).
//   "access <json>"                        the headings, links, buttons, form fields, pictures and landmarks that are on screen now, with their
//                                          names and boxes in CSS pixels (at most 160, names cut at 120 characters), so the shell can give them to
//                                          the screen reader (F-A11Y-06). Sent when the document is ready and again, at most twice a second, after a
//                                          scroll, a resize or a change of the page. Password fields send no text.
// Nothing here changes what the page sees or does.
(function () {
  try {
    if (window.__iyakkiSiteProbe || window !== window.top) return;
    window.__iyakkiSiteProbe = 1;
    var post = function (m) { try { window.webkit.messageHandlers.iyakkiSite.postMessage(m); } catch (e) {} };
    var seen = {};
    var n = navigator;
    if (n && typeof n.requestMediaKeySystemAccess === 'function') {
      var orig = n.requestMediaKeySystemAccess;
      n.requestMediaKeySystemAccess = function (name) {
        try {
          var k = String(name).slice(0, 100);
          if (!seen[k] && Object.keys(seen).length < 8) { seen[k] = 1; post('keysys ' + k); }
        } catch (e) {}
        return orig.apply(this, arguments);
      };
    }
    var abs = function (href) { try { return new URL(href, document.baseURI).href; } catch (e) { return ''; } };
    var meta = function () {
      var m = document.querySelector('link[rel~="manifest"]');
      var i = document.querySelector('link[rel~="icon"], link[rel="apple-touch-icon"]');
      var murl = m ? abs(m.getAttribute('href')) : '';
      var iurl = i ? abs(i.getAttribute('href')) : (location.protocol === 'https:' ? location.origin + '/favicon.ico' : '');
      var send = function (text) { post('meta ' + murl + '\x1f' + (text || '') + '\x1f' + iurl); };
      if (!murl || new URL(murl).origin !== location.origin) { send(''); return; }
      try {
        fetch(murl, { credentials: 'same-origin' }).then(function (r) { return r.ok ? r.text() : ''; }).then(function (t) { send(t.length <= 100000 ? t : ''); }, function () { send(''); });
      } catch (e) { send(''); }
    };
    if (document.readyState === 'loading') document.addEventListener('DOMContentLoaded', meta, { once: true }); else meta();
    var SEL = 'h1,h2,h3,h4,h5,h6,[role=heading],img[alt],[role=img],main,nav,aside,header,footer,form,[role=main],[role=navigation],[role=banner],[role=contentinfo],[role=complementary],[role=search],a[href],button,[role=button],[role=link],input,select,textarea';
    var nameOf = function (el) {
      var t = el.getAttribute('aria-label');
      if (!t && el.getAttribute('aria-labelledby')) {
        t = el.getAttribute('aria-labelledby').split(/\s+/).slice(0, 4).map(function (id) { var e = document.getElementById(id); return e ? e.textContent : ''; }).join(' ');
      }
      if (!t && el.tagName === 'IMG') t = el.getAttribute('alt');
      if (!t && el.labels && el.labels.length) t = el.labels[0].textContent;
      if (!t) t = el.getAttribute('title') || el.getAttribute('placeholder') || '';
      if (!t && el.tagName !== 'INPUT' && el.tagName !== 'SELECT' && el.tagName !== 'TEXTAREA') t = el.textContent || '';
      if (!t && el.tagName === 'INPUT' && /^(submit|button|reset)$/.test(el.type)) t = el.value || '';
      return String(t).replace(/\s+/g, ' ').trim().slice(0, 120);
    };
    var roleOf = function (el) {
      var r = el.getAttribute('role'), tag = el.tagName.toLowerCase();
      if (r) return /^(heading|img|main|navigation|banner|contentinfo|complementary|search|button|link)$/.test(r) ? r : '';
      if (/^h[1-6]$/.test(tag)) return 'heading';
      if (tag === 'img') return 'img';
      if (tag === 'a') return 'link';
      if (tag === 'button') return 'button';
      if (tag === 'main' || tag === 'nav' || tag === 'aside' || tag === 'header' || tag === 'footer' || tag === 'form') return { main: 'main', nav: 'navigation', aside: 'complementary', header: 'banner', footer: 'contentinfo', form: 'form' }[tag];
      if (tag === 'input') return /^(submit|button|reset)$/.test(el.type) ? 'button' : (el.type === 'password' ? 'password' : (el.type === 'checkbox' || el.type === 'radio' ? 'check' : (el.type === 'hidden' ? '' : 'field')));
      if (tag === 'select' || tag === 'textarea') return 'field';
      return '';
    };
    var sendAccess = function () {
      try {
        var out = [], els = document.querySelectorAll(SEL), vw = window.innerWidth, vh = window.innerHeight;
        for (var i = 0; i < els.length && out.length < 160; i++) {
          var el = els[i], role = roleOf(el);
          if (!role) continue;
          var b = el.getBoundingClientRect();
          if (b.width < 4 || b.height < 4 || b.bottom < 0 || b.top > vh || b.right < 0 || b.left > vw) continue;
          var o = { r: role, n: role === 'password' ? '' : nameOf(el), x: Math.round(b.left), y: Math.round(b.top), w: Math.round(b.width), h: Math.round(b.height) };
          if (role === 'heading') o.l = el.getAttribute('aria-level') ? +el.getAttribute('aria-level') : (/^H([1-6])$/.test(el.tagName) ? +RegExp.$1 : 2);
          if (role === 'password') o.n = nameOf(el);
          out.push(o);
        }
        var s = JSON.stringify(out);
        if (s.length < 60000 && s !== window.__iyakkiLastAccess) { window.__iyakkiLastAccess = s; post('access ' + s); }
      } catch (e) {}
    };
    var again = function () { if (!window.__iyakkiAccessTimer) window.__iyakkiAccessTimer = setTimeout(function () { window.__iyakkiAccessTimer = 0; sendAccess(); }, 500); };
    var startAccess = function () {
      sendAccess();
      window.addEventListener('scroll', again, { passive: true, capture: true });
      window.addEventListener('resize', again, { passive: true });
      try { new MutationObserver(again).observe(document.documentElement, { childList: true, subtree: true, attributes: true, attributeFilter: ['class', 'style', 'hidden', 'aria-hidden'] }); } catch (e) {}
    };
    if (document.readyState === 'loading') document.addEventListener('DOMContentLoaded', startAccess, { once: true }); else startAccess();
  } catch (e) {}
})();
