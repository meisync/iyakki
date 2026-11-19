// Site probe (docs/WEBVIEW-PROTOCOL.md, KeySystem and PageMeta). Injected by helper.c into the top frame at document start.
// Tells the shell two things about a page, through the "iyakkiSite" message handler:
//   "keysys <name>"                        the page asked for a media key system (requestMediaKeySystemAccess), once per name: the shell says
//                                          plainly that this computer has no decryption module when the name needs one (Widevine, PlayReady,
//                                          FairPlay). The call itself is passed on unchanged: the engine answers as it always did.
//   "meta <manifest url>\x1f<manifest text>\x1f<icon url>"   once the document is ready: where the page's web app manifest and its icon
//                                          are, and the manifest's text when it could be read from the page's own origin (at most 100000 bytes).
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
  } catch (e) {}
})();
