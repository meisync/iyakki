//! A tile's own page in the browser and the `window.iyakki` bridge (F-WEBSVC-12, KI-821): `GET /tiles/{id}/app` is a small static page with a
//! canvas, `GET /tiles/app.js` is the script it loads. Both are PUBLIC and carry no data of anyone: the page signs in itself with the account's
//! name and password (`POST /api/v1/login`), keeps the token in memory only (never in storage or a cookie), and drives the tile over the same
//! route a program uses (`POST /api/v1/tiles/{id}/events`), drawing the returned display list on the canvas. Nothing here runs a tile's code in
//! the browser: the tile runs in the service's sandbox and only its display list arrives.
//!
//! The page is served with a strict policy: scripts only from this origin (`script-src 'self'`, no inline script, no `eval`), connections
//! only to this origin, images only from `data:`, no framing (`frame-ancestors 'none'`), no referrer. `window.iyakki` is the bridge a page script
//! may use: `iyakki.tile` (the tile's id), `iyakki.send(events)` (a promise of the frame) and `iyakki.onframe(callback)`; the bridge can do nothing
//! the signed-in account could not do over the API with the same token, and it has no other power (no files, no tools, no clipboard here).
//! A WebSocket for the bridge is not needed for this: the events route is a request and an answer, and the page asks again when it has more.

pub const APP_JS_PATH: &str = "/tiles/app.js";

/// The policy of both files.
pub const CSP: &str = "default-src 'none'; script-src 'self'; style-src 'self'; connect-src 'self'; img-src data:; base-uri 'none'; form-action 'none'; frame-ancestors 'none'";

/// The page. The tile's id is read by the script from the address; nothing is written into the page.
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
<canvas id="tile" width="640" height="400" tabindex="0" hidden aria-label="The tile"></canvas>
</main>
<script src="/tiles/app.js"></script>
</body>
</html>
"##;

pub const APP_CSS: &str = "body{margin:0;font:16px system-ui,sans-serif;background:#16181f;color:#e6e8ef}main{max-width:900px;margin:0 auto;padding:16px}form{display:grid;gap:10px;max-width:360px}input{width:100%;padding:8px;box-sizing:border-box}button{padding:10px;font-size:16px}iframe{width:100%;height:80vh;border:0;background:#fff}canvas{max-width:100%;background:#181a22;outline-offset:2px}#note{min-height:1.2em;color:#ff9c8a}";

/// The script. Plain ES5 style, no dependency, no `eval`, no inline handlers, no storage.
pub const APP_JS: &str = r##"(function () {
  'use strict';
  var parts = location.pathname.split('/');
  var id = parts.length >= 3 ? parts[parts.length - 2] : '';
  var token = null; // in memory only: a reload signs in again
  var frameCb = [];
  var canvas = document.getElementById('tile');
  var ctx = canvas.getContext('2d');
  var note = document.getElementById('note');
  var busy = false, queue = [], lastHits = [];
  document.getElementById('title').textContent = 'Tile ' + id;

  function api(method, path, body) {
    var h = { 'Content-Type': 'application/json' };
    if (token) h.Authorization = 'Bearer ' + token;
    return fetch(path, { method: method, headers: h, body: body === undefined ? undefined : JSON.stringify(body), credentials: 'omit', referrerPolicy: 'no-referrer' })
      .then(function (r) { return r.json().then(function (j) { if (!r.ok) throw new Error((j && j.error && j.error.message) || ('the service answered ' + r.status)); return j; }); });
  }

  function colour(c) { return c; }

  function draw(frame) {
    canvas.width = frame.width; canvas.height = frame.height;
    ctx.clearRect(0, 0, canvas.width, canvas.height);
    var clips = 0; lastHits = [];
    (frame.ops || []).forEach(function (o) {
      switch (o.op) {
        case 'rect': ctx.fillStyle = colour(o.color); ctx.fillRect(o.x, o.y, o.w, o.h); break;
        case 'round_rect': ctx.fillStyle = colour(o.color); ctx.beginPath(); ctx.roundRect(o.x, o.y, o.w, o.h, o.radius); ctx.fill(); break;
        case 'outline': ctx.strokeStyle = colour(o.color); ctx.lineWidth = o.thickness; ctx.strokeRect(o.x + o.thickness / 2, o.y + o.thickness / 2, o.w - o.thickness, o.h - o.thickness); break;
        case 'line': ctx.strokeStyle = colour(o.color); ctx.lineWidth = o.thickness; ctx.beginPath(); ctx.moveTo(o.x1, o.y1); ctx.lineTo(o.x2, o.y2); ctx.stroke(); break;
        case 'text': ctx.fillStyle = colour(o.color); ctx.font = o.size + 'px sans-serif'; ctx.textBaseline = 'top'; ctx.fillText(o.text, o.x, o.y); break;
        case 'push_clip': ctx.save(); ctx.beginPath(); ctx.rect(o.x, o.y, o.w, o.h); ctx.clip(); clips++; break;
        case 'pop_clip': if (clips > 0) { ctx.restore(); clips--; } break;
        case 'hit': lastHits.push(o); break;
        default: break; // an operation this page does not know draws nothing
      }
    });
    while (clips-- > 0) ctx.restore();
    frameCb.forEach(function (cb) { try { cb(frame); } catch (e) { /* a listener's fault is its own */ } });
  }

  function flush() {
    if (busy || !token) return Promise.resolve();
    var events = queue.splice(0, 128);
    busy = true;
    return api('POST', '/api/v1/tiles/' + encodeURIComponent(id) + '/events', { width: canvas.width, height: canvas.height, scale_pct: 100, dark: true, events: events })
      .then(function (r) { busy = false; if (r.redraw !== false && r.frame) draw(r.frame); if (queue.length) return flush(); })
      .catch(function (e) { busy = false; note.textContent = String(e.message || e); });
  }

  function send(events) { Array.prototype.push.apply(queue, events); return flush(); }

  // the bridge a page script may use
  window.iyakki = Object.freeze({ version: 1, tile: id, send: send, onframe: function (cb) { if (typeof cb === 'function') frameCb.push(cb); } });

  function pos(ev) {
    var r = canvas.getBoundingClientRect();
    return { x: Math.round((ev.clientX - r.left) * canvas.width / r.width), y: Math.round((ev.clientY - r.top) * canvas.height / r.height) };
  }
  canvas.addEventListener('pointerdown', function (ev) { var p = pos(ev); send([{ type: 'pointer_down', x: p.x, y: p.y, button: ev.button }]); });
  canvas.addEventListener('pointerup', function (ev) { var p = pos(ev); send([{ type: 'pointer_up', x: p.x, y: p.y, button: ev.button }]); });
  canvas.addEventListener('keydown', function (ev) {
    var named = { Backspace: 8, Enter: 13, Escape: 27, ArrowLeft: 37, ArrowUp: 38, ArrowRight: 39, ArrowDown: 40, Tab: 9, Delete: 46 };
    if (named[ev.key] !== undefined) { ev.preventDefault(); send([{ type: 'key', code: named[ev.key], mods: 0, down: true }]); }
    else if (ev.key.length === 1 && !ev.ctrlKey && !ev.metaKey) { ev.preventDefault(); send([{ type: 'text', text: ev.key }]); }
  });
  setInterval(function () { if (token && !busy) send([{ type: 'tick', dt_ms: 100 }]); }, 100);

  document.getElementById('signin').addEventListener('submit', function (ev) {
    ev.preventDefault();
    var pw = document.getElementById('password');
    api('POST', '/api/v1/login', { name: document.getElementById('name').value, password: pw.value })
      .then(function (r) {
        token = r.token; pw.value = ''; note.textContent = '';
        document.getElementById('signin').hidden = true; canvas.hidden = false; canvas.focus();
        return send([{ type: 'resize', w: canvas.width, h: canvas.height, scale_pct: 100 }]);
      })
      .catch(function (e) { pw.value = ''; note.textContent = String(e.message || e); });
  });
})();
"##;

/// The tile id of `/tiles/{id}/app` is checked by the route matcher (plain characters only); this says whether `path` is the script, the
/// style sheet or a tile page (for the server's one dispatch).
pub fn kind_of(route_id: &str) -> Option<(&'static str, &'static str)> {
    match route_id {
        "tile_page" => Some(("text/html; charset=utf-8", PAGE_HTML)),
        "tile_page_js" => Some(("text/javascript; charset=utf-8", APP_JS)),
        "tile_page_css" => Some(("text/css; charset=utf-8", APP_CSS)),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_page_loads_only_scripts_and_styles_of_its_own_origin_and_has_no_inline_code() {
        assert!(PAGE_HTML.contains("<script src=\"/tiles/app.js\"></script>") && PAGE_HTML.contains("href=\"/tiles/app.css\""));
        assert!(!PAGE_HTML.contains("onclick") && !PAGE_HTML.contains("onload") && !PAGE_HTML.contains("javascript:") && !PAGE_HTML.contains("<style"));
        assert_eq!(PAGE_HTML.matches("<script").count(), 1);
        assert!(!PAGE_HTML.contains("http://") && !PAGE_HTML.contains("https://"), "nothing is loaded from another place");
        for need in ["script-src 'self'", "connect-src 'self'", "frame-ancestors 'none'", "default-src 'none'", "form-action 'none'"] {
            assert!(CSP.contains(need), "{need}");
        }
        assert!(!CSP.contains("unsafe-inline") && !CSP.contains("unsafe-eval"));
    }

    #[test]
    fn the_script_keeps_the_token_in_memory_never_evals_and_talks_only_to_this_origin() {
        for bad in ["eval(", "new Function", "localStorage", "sessionStorage", "document.cookie", "indexedDB", "document.write", "innerHTML", "XMLHttpRequest", "WebSocket", "http://", "https://", "importScripts", "postMessage"] {
            assert!(!APP_JS.contains(bad), "the script must not contain {bad}");
        }
        assert!(APP_JS.contains("credentials: 'omit'") && APP_JS.contains("referrerPolicy: 'no-referrer'"));
        assert!(APP_JS.contains("'/api/v1/login'") && APP_JS.contains("'/api/v1/tiles/'"));
        assert!(APP_JS.contains("window.iyakki = Object.freeze("), "the bridge cannot be replaced by the tile's own script");
        assert!(APP_JS.contains("pw.value = ''"), "the password field is emptied after it is used");
        assert!(APP_JS.contains("splice(0, 128)"), "at most 128 events go in one request, as the service takes");
    }

    #[test]
    fn the_route_ids_map_to_the_three_files() {
        assert_eq!(kind_of("tile_page").unwrap().0, "text/html; charset=utf-8");
        assert_eq!(kind_of("tile_page_js").unwrap().0, "text/javascript; charset=utf-8");
        assert_eq!(kind_of("tile_page_css").unwrap().0, "text/css; charset=utf-8");
        assert!(kind_of("tile_list").is_none());
    }
}
