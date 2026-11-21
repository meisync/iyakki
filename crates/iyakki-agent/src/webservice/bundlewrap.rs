//! The page a person opens for a web-bundle tile, and the frame the bundle runs in (F-WEBSVC-12, KI-821). A bundle's files share the service's
//! origin, so a bundle page that held the person's token itself could use it for anything the person may do. Here the token never reaches a
//! bundle: `GET /tiles/{id}/w` is a WRAPPER page of the service. It holds the sign-in form, the token (in memory only) and the WebSocket bridge,
//! and shows the bundle's `index.html` in an `<iframe sandbox="allow-scripts">` (no `allow-same-origin`: the frame gets a throw-away origin of
//! its own, so it cannot read the wrapper, the service's pages, cookies or storage, and its own fetches to the service are cross-origin
//! and refused). The bundle's `window.iyakki` (`/tiles/bridge.js`, loaded inside the frame) talks to the wrapper only by `postMessage`; the wrapper
//! makes each call over the bridge, which allows only what the bundle's manifest lists, and answers the frame with the value. Only messages from
//! this frame's own window count, and nothing the wrapper sends carries the token.
//!
//! The files of a bundle are also served with `Content-Security-Policy: sandbox allow-scripts`, so a bundle page opened directly (not through the
//! wrapper) gets the same throw-away origin; there it finds no parent and says to open the tile through the wrapper.

/// Path of the wrapper's script.
pub const WRAPPER_JS_PATH: &str = "/tiles/wrapper.js";

/// The wrapper's policy: its own scripts and styles only, connections to this origin, a frame of this origin, framed by nobody.
pub const CSP: &str = "default-src 'none'; script-src 'self'; style-src 'self'; connect-src 'self'; frame-src 'self'; base-uri 'none'; form-action 'none'; frame-ancestors 'none'";

/// The wrapper page. The tile's id is read by the script from the address; nothing is written into the page.
pub const PAGE_HTML: &str = r##"<!doctype html>
<html lang="en">
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<meta name="referrer" content="no-referrer">
<title>IYAKKI tile</title>
<link rel="stylesheet" href="/tiles/app.css">
</head>
<body>
<main>
<form id="signin" autocomplete="off">
<h1 id="title">Tile</h1>
<label>Account <input id="name" name="name" autocomplete="username" required></label>
<label>Password <input id="password" name="password" type="password" autocomplete="current-password" required></label>
<button type="submit">Open the tile</button>
<p id="note" role="status"></p>
</form>
<div id="stage"></div>
</main>
<script src="/tiles/wrapper.js"></script>
</body>
</html>
"##;

/// The wrapper's script: plain ES5 style, no dependency, no `eval`, no storage. The token lives in this closure only.
pub const WRAPPER_JS: &str = r##"(function () {
  'use strict';
  var parts = location.pathname.split('/');
  var id = parts.length >= 4 && parts[1] === 'tiles' ? parts[2] : '';
  var token = null, socket = null, ready = false, caps = [], frame = null, framed = false;
  var nextId = 1, pending = {};
  var note = document.getElementById('note');
  document.getElementById('title').textContent = 'Tile ' + id;

  function toFrame(message) {
    // the frame has an origin of its own that no address can name, so the only safe recipient is its window itself
    if (frame && frame.contentWindow) frame.contentWindow.postMessage(message, '*');
  }
  function failAll(message) {
    var ids = Object.keys(pending);
    for (var i = 0; i < ids.length; i++) { var f = pending[ids[i]]; delete pending[ids[i]]; toFrame({ iyakki: 1, type: 'result', id: f, ok: false, error: message }); }
  }
  function announce() { if (ready && framed) toFrame({ iyakki: 1, type: 'ready', caps: caps.slice() }); }

  function open() {
    var scheme = location.protocol === 'https:' ? 'wss://' : 'ws://';
    socket = new WebSocket(scheme + location.host + '/tiles/' + encodeURIComponent(id) + '/bridge');
    socket.onopen = function () { socket.send(JSON.stringify({ type: 'auth', token: token })); };
    socket.onmessage = function (ev) {
      var m;
      try { m = JSON.parse(ev.data); } catch (e) { return; }
      if (m.type === 'ready') { ready = true; caps = m.caps || []; announce(); }
      else if (m.type === 'result') {
        var f = pending[m.id];
        if (f !== undefined) { delete pending[m.id]; toFrame({ iyakki: 1, type: 'result', id: f, ok: !!m.ok, value: m.value, error: m.error }); }
      } else if (m.type === 'error') { failAll(m.message || 'the bridge closed'); }
    };
    socket.onclose = function () { ready = false; failAll('the bridge is closed'); };
  }

  // what the frame may ask: say it is there (hello) and make a call; anything else is ignored
  window.addEventListener('message', function (ev) {
    if (!frame || ev.source !== frame.contentWindow) return;
    var m = ev.data;
    if (!m || m.iyakki !== 1 || typeof m.type !== 'string') return;
    if (m.type === 'hello') { framed = true; announce(); return; }
    if (m.type === 'call') {
      var fid = m.id;
      if (typeof fid !== 'number' || typeof m.call !== 'string') return;
      if (!ready) { toFrame({ iyakki: 1, type: 'result', id: fid, ok: false, error: 'the bridge is not open' }); return; }
      var mine = nextId++;
      pending[mine] = fid;
      socket.send(JSON.stringify({ type: 'call', id: mine, call: m.call, args: m.args === undefined ? {} : m.args }));
    }
  });

  document.getElementById('signin').addEventListener('submit', function (ev) {
    ev.preventDefault();
    var pw = document.getElementById('password');
    fetch('/api/v1/login', { method: 'POST', headers: { 'Content-Type': 'application/json' }, body: JSON.stringify({ name: document.getElementById('name').value, password: pw.value }), credentials: 'omit', referrerPolicy: 'no-referrer' })
      .then(function (r) { return r.json().then(function (j) { if (!r.ok) throw new Error((j && j.error && j.error.message) || ('the service answered ' + r.status)); return j; }); })
      .then(function (j) {
        token = j.token; pw.value = ''; note.textContent = '';
        document.getElementById('signin').hidden = true;
        frame = document.createElement('iframe');
        frame.setAttribute('sandbox', 'allow-scripts');
        frame.setAttribute('title', 'Tile ' + id);
        frame.setAttribute('referrerpolicy', 'no-referrer');
        frame.src = '/tiles/' + encodeURIComponent(id) + '/b/index.html';
        document.getElementById('stage').appendChild(frame);
        open();
      })
      .catch(function (e) { pw.value = ''; note.textContent = String(e.message || e); });
  });
})();
"##;

/// Which static file a route id serves: (media type, body).
pub fn kind_of(route_id: &str) -> Option<(&'static str, &'static str)> {
    match route_id {
        "tile_wrapper" => Some(("text/html; charset=utf-8", PAGE_HTML)),
        "tile_wrapper_js" => Some(("text/javascript; charset=utf-8", WRAPPER_JS)),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_wrapper_page_has_no_inline_code_loads_only_this_origin_and_is_framed_by_nobody() {
        assert!(PAGE_HTML.contains("<script src=\"/tiles/wrapper.js\"></script>") && PAGE_HTML.contains("href=\"/tiles/app.css\""));
        assert_eq!(PAGE_HTML.matches("<script").count(), 1);
        assert!(!PAGE_HTML.contains("onclick") && !PAGE_HTML.contains("javascript:") && !PAGE_HTML.contains("<style") && !PAGE_HTML.contains("http"));
        for need in ["script-src 'self'", "frame-src 'self'", "frame-ancestors 'none'", "default-src 'none'", "connect-src 'self'"] {
            assert!(CSP.contains(need), "{need}");
        }
        assert!(!CSP.contains("unsafe-"));
        assert!(crate::webservice::tilepage::APP_CSS.contains("iframe"), "the frame is styled by the style sheet, not inline");
    }

    #[test]
    fn the_frame_is_sandboxed_without_same_origin_and_the_token_never_goes_to_it() {
        for bad in ["eval(", "new Function", "localStorage", "sessionStorage", "document.cookie", "indexedDB", "document.write", "innerHTML", "XMLHttpRequest", "http://", "https://", "importScripts", "window.open", "allow-same-origin", "allow-top-navigation", "allow-popups", "allow-forms"] {
            assert!(!WRAPPER_JS.contains(bad), "the script must not contain {bad}");
        }
        assert!(WRAPPER_JS.contains("setAttribute('sandbox', 'allow-scripts')"));
        assert!(WRAPPER_JS.contains("credentials: 'omit'") && WRAPPER_JS.contains("pw.value = ''") && WRAPPER_JS.contains("encodeURIComponent(id)"));
        // every message to the frame is built from a fixed set of fields; the token is not among them
        for line in WRAPPER_JS.lines().filter(|l| l.contains("toFrame(")) {
            assert!(!line.contains("token"), "{line}");
        }
        assert!(WRAPPER_JS.contains("ev.source !== frame.contentWindow"), "only the frame's own window is heard");
        assert!(WRAPPER_JS.contains("frame.contentWindow.postMessage(message, '*')"), "an opaque origin can only be addressed as *, so the window is the recipient");
        assert_eq!(WRAPPER_JS.matches("postMessage(").count(), 1, "one place sends");
    }

    #[test]
    fn the_frames_script_asks_the_wrapper_and_does_not_hold_a_token_or_reach_the_service() {
        let js = crate::webservice::webbundle::BRIDGE_JS;
        for bad in ["eval(", "new Function", "localStorage", "sessionStorage", "document.cookie", "indexedDB", "document.write", "innerHTML", "XMLHttpRequest", "fetch(", "WebSocket", "/api/v1/login", "token", "http://", "https://", "importScripts", "window.open"] {
            assert!(!js.contains(bad), "the frame's script must not contain {bad}");
        }
        assert!(js.contains("window.iyakki = Object.freeze(") && js.contains("window.parent.postMessage(") && js.contains("ev.source !== window.parent"));
        assert!(js.contains("window.parent !== window") && js.contains("open this tile through its wrapper page"), "opened outside the wrapper it says so and does nothing");
    }

    #[test]
    fn the_route_ids_map_to_the_two_files() {
        assert_eq!(kind_of("tile_wrapper").unwrap().0, "text/html; charset=utf-8");
        assert_eq!(kind_of("tile_wrapper_js").unwrap().0, "text/javascript; charset=utf-8");
        assert!(kind_of("tile_page").is_none());
    }
}
