//! The page side of Web Push (F-INBOX-09): what a person's own browser (a phone, another computer) loads to subscribe to this computer's
//! notifications. [`PushPages::handle`] is a pure request handler a web route can mount; it serves
//!
//! * `GET  <base>/sw.js`        the service worker: on a `push` event it shows the notification (the JSON the sender encrypted);
//! * `GET  <base>/subscribe?t=` the page that registers that worker, calls `PushManager.subscribe` with the person's VAPID public key
//!   and posts the answer back;
//! * `POST <base>/subscribe`    the answer, saved with [`WebPush::subscribe`] for the person the one-time token was made for.
//!
//! The one-time token ([`PushPages::mint`]) is how the person tells the page whose vault to use: a token is bound to one signed-in person,
//! is used once, and lapses after [`TOKEN_LIFE`] seconds. A person who is signed out (vault locked) has no entry in the open-vault
//! registry, so a token for them cannot be used.
//!
//! The PUBLIC HTTPS NAME the pages are reached under is the owner's: it is read from `webpush-public.json` (`{"public_name": "host"}`) and
//! is unset by default. With none, [`PushPages::mint`] and the handler refuse in words and nothing is served (a service worker and the
//! push permission need a secure origin, and the push services need to reach the page's host; there is no default name).

use std::collections::HashMap;
use std::path::Path;
use std::sync::{Arc, Mutex};

use serde_json::{json, Value};

use crate::webpush::OpenVaults;
#[cfg(test)]
use crate::webpush::WebPush;

pub const CONFIG_FILE: &str = "webpush-public.json";
/// Seconds a subscribe token lives.
pub const TOKEN_LIFE: u64 = 600;
/// Most tokens waiting at once.
pub const MAX_TOKENS: usize = 200;
/// The words when the owner has not chosen a public name.
pub const NO_NAME: &str = "Web Push pages are not set up: the owner has not chosen the public HTTPS name this computer is reached under (webpush-public.json). Nothing is served.";

/// The owner's public host name, if set and plain (letters, digits, dots, dashes, an optional port).
pub fn public_name(dir: &Path) -> Option<String> {
    let v: Value = serde_json::from_str(&std::fs::read_to_string(dir.join(CONFIG_FILE)).ok()?).ok()?;
    let name = v["public_name"].as_str()?.trim().to_ascii_lowercase();
    let (host, port) = match name.split_once(':') {
        Some((h, p)) => (h, Some(p)),
        None => (name.as_str(), None),
    };
    let host_ok = host.contains('.') && !host.starts_with(['.', '-']) && host.chars().all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '-');
    let port_ok = port.is_none_or(|p| !p.is_empty() && p.len() <= 5 && p.chars().all(|c| c.is_ascii_digit()));
    (host_ok && port_ok).then_some(name)
}

/// Is this path one of the pages' own (`/push/sw.js`, `/push/subscribe`)? The web route mounts exactly these and nothing else under `/push`.
pub fn owns(path: &str) -> bool {
    matches!(path, "/push/sw.js" | "/push/subscribe")
}

/// Set the public name (an administrator's act: the settings tool `webpush_public_name` is the only caller and asks on a card). An empty
/// text takes it away again (nothing is served). A name that is not a plain host with a dot is refused in words and nothing is written.
pub fn set_public_name(dir: &Path, name: &str) -> Result<String, String> {
    let path = dir.join(CONFIG_FILE);
    let name = name.trim();
    if name.is_empty() {
        let _ = std::fs::remove_file(&path);
        return Ok("The public name is removed: the Web Push pages are not served.".into());
    }
    let old = std::fs::read(&path).ok();
    std::fs::write(&path, json!({ "public_name": name }).to_string()).map_err(|e| format!("could not keep the name: {e}"))?;
    match public_name(dir) {
        Some(n) => Ok(format!("The public name is {n}. The Web Push pages are served under https://{n}/push/ while the web service runs with TLS.")),
        None => {
            match old {
                Some(b) => {
                    let _ = std::fs::write(&path, b);
                }
                None => {
                    let _ = std::fs::remove_file(&path);
                }
            }
            Err("a public name is a plain host name with a dot (letters, digits, dots, dashes, an optional :port), for example notify.example.org".into())
        }
    }
}

/// The service worker. It shows only plain text (a title and a body), never markup, and opens nothing on a click but the origin's root.
pub const SERVICE_WORKER: &str = r#"'use strict';
self.addEventListener('install', () => self.skipWaiting());
self.addEventListener('activate', (e) => e.waitUntil(self.clients.claim()));
self.addEventListener('push', (event) => {
  let d = {};
  try { d = event.data ? event.data.json() : {}; } catch (e) { d = {}; }
  const title = String(d.title || 'IYAKKI').slice(0, 120);
  const body = String(d.body || '').slice(0, 400);
  event.waitUntil(self.registration.showNotification(title, { body: body, tag: 'iyakki' }));
});
self.addEventListener('notificationclick', (event) => {
  event.notification.close();
  event.waitUntil(self.clients.openWindow('/'));
});
"#;

fn page(base: &str, key: &str, token: &str) -> String {
    // the key is base64url and the token is base64url: neither can break out of a JS string; both are checked anyway before they are put here
    let safe = |s: &str| s.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_');
    assert!(safe(key) && safe(token), "page values are plain");
    format!(
        r#"<!doctype html><html lang="en"><head><meta charset="utf-8"><meta name="viewport" content="width=device-width,initial-scale=1"><title>Notifications from IYAKKI</title></head><body>
<h1>Notifications from IYAKKI</h1>
<p id="s">This browser can receive IYAKKI's notifications. Give it a short name and press the button; the browser will ask you to allow notifications.</p>
<p><label>Name <input id="n" value="phone" maxlength="40"></label> <button id="b">Allow notifications</button></p>
<script>
(function () {{
  var s = document.getElementById('s');
  var say = function (m) {{ s.textContent = m; }};
  var b64 = function (t) {{ t = t.replace(/-/g, '+').replace(/_/g, '/'); var r = atob(t + '==='.slice((t.length + 3) % 4)); var o = new Uint8Array(r.length); for (var i = 0; i < r.length; i++) o[i] = r.charCodeAt(i); return o; }};
  var url = function (buf) {{ var a = new Uint8Array(buf), t = ''; for (var i = 0; i < a.length; i++) t += String.fromCharCode(a[i]); return btoa(t).replace(/\+/g, '-').replace(/\//g, '_').replace(/=+$/, ''); }};
  if (!('serviceWorker' in navigator) || !('PushManager' in window)) {{ say('This browser has no push notifications.'); document.getElementById('b').disabled = true; return; }}
  document.getElementById('b').onclick = function () {{
    navigator.serviceWorker.register('{base}/sw.js', {{ scope: '{base}/' }}).then(function () {{ return navigator.serviceWorker.ready; }})
      .then(function (reg) {{ return reg.pushManager.subscribe({{ userVisibleOnly: true, applicationServerKey: b64('{key}') }}); }})
      .then(function (sub) {{
        var j = sub.toJSON();
        return fetch('{base}/subscribe', {{ method: 'POST', headers: {{ 'Content-Type': 'application/json' }}, body: JSON.stringify({{ token: '{token}', label: document.getElementById('n').value, endpoint: j.endpoint, p256dh: j.keys.p256dh, auth: j.keys.auth }}) }});
      }})
      .then(function (r) {{ return r.json(); }})
      .then(function (r) {{ say(r.ok ? 'Done: this browser will get IYAKKI notifications.' : 'Not saved: ' + r.error); }})
      .catch(function (e) {{ say('Not saved: ' + e); }});
  }};
}})();
</script></body></html>"#
    )
}

struct Token {
    person: String,
    expires: u64,
}

/// The pages of one tenant (`dir`), over the open-vault registry.
pub struct PushPages {
    dir: std::path::PathBuf,
    base: String,
    vaults: &'static OpenVaults,
    tokens: Mutex<HashMap<String, Token>>,
    now: fn() -> u64,
}

/// A request the route hands over and the answer it gives back.
pub struct Reply {
    pub status: u16,
    pub content_type: &'static str,
    pub body: Vec<u8>,
}

fn reply(status: u16, ct: &'static str, body: impl Into<Vec<u8>>) -> Reply {
    Reply { status, content_type: ct, body: body.into() }
}

fn json_reply(status: u16, v: Value) -> Reply {
    reply(status, "application/json", v.to_string())
}

impl PushPages {
    /// `base` is the path the route is mounted at (`/push`).
    pub fn new(dir: &Path, base: &str, vaults: &'static OpenVaults) -> PushPages {
        PushPages { dir: dir.to_path_buf(), base: base.trim_end_matches('/').to_string(), vaults, tokens: Mutex::new(HashMap::new()), now: || crate::clock::now_unix().max(0) as u64 }
    }

    /// The pages of this tenant, shared by every caller in the process (a link made by the chat tool is answered by the web route that
    /// mounts these pages), over the global open-vault registry. Mounted at `/push`.
    pub fn shared(dir: &Path) -> Arc<PushPages> {
        static ALL: std::sync::OnceLock<Mutex<HashMap<String, Arc<PushPages>>>> = std::sync::OnceLock::new();
        let mut m = ALL.get_or_init(Default::default).lock().unwrap_or_else(|e| e.into_inner());
        m.entry(dir.display().to_string()).or_insert_with(|| Arc::new(PushPages::new(dir, "/push", OpenVaults::global()))).clone()
    }

    /// The address a person opens in the browser to subscribe: `https://<public name><base>/subscribe?t=<token>`. Needs the owner's public
    /// name and the person's open vault.
    pub fn mint(&self, person: &str) -> Result<String, String> {
        let name = public_name(&self.dir).ok_or(NO_NAME)?;
        if self.vaults.get(&self.dir, person).is_none() {
            return Err("your vault is locked: sign in again, then try again".into());
        }
        let mut raw = [0u8; 24];
        iyakki_webcrypto::random(&mut raw)?;
        let token = iyakki_webcrypto::b64::url(&raw);
        let now = (self.now)();
        let mut t = self.tokens.lock().unwrap_or_else(|e| e.into_inner());
        t.retain(|_, v| v.expires > now);
        if t.len() >= MAX_TOKENS {
            return Err("too many subscribe links are waiting; wait a few minutes".into());
        }
        t.insert(token.clone(), Token { person: person.to_string(), expires: now + TOKEN_LIFE });
        Ok(format!("https://{name}{}/subscribe?t={token}", self.base))
    }

    /// Take a token (once). `None` when unknown, used or lapsed.
    fn take(&self, token: &str) -> Option<String> {
        let now = (self.now)();
        let mut t = self.tokens.lock().unwrap_or_else(|e| e.into_inner());
        t.remove(token).filter(|k| k.expires > now).map(|k| k.person)
    }

    fn peek(&self, token: &str) -> Option<String> {
        let now = (self.now)();
        self.tokens.lock().unwrap_or_else(|e| e.into_inner()).get(token).filter(|k| k.expires > now).map(|k| k.person.clone())
    }

    /// Answer one request. `path` is the full path with its query. Nothing is served without the owner's public name.
    pub fn handle(&self, method: &str, path: &str, body: &[u8]) -> Reply {
        if public_name(&self.dir).is_none() {
            return reply(503, "text/plain; charset=utf-8", NO_NAME);
        }
        let Some(rest) = path.strip_prefix(&self.base) else { return reply(404, "text/plain; charset=utf-8", "not found") };
        let (route, query) = rest.split_once('?').unwrap_or((rest, ""));
        match (method, route) {
            ("GET", "/sw.js") => reply(200, "text/javascript; charset=utf-8", SERVICE_WORKER),
            ("GET", "/subscribe") => {
                let token = query.split('&').find_map(|kv| kv.strip_prefix("t=")).unwrap_or("");
                let ok = !token.is_empty() && token.len() <= 64 && token.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_');
                let Some(person) = ok.then(|| self.peek(token)).flatten() else { return reply(410, "text/plain; charset=utf-8", "this link has expired or was already used; ask for a new one") };
                let Some(w) = self.vaults.get(&self.dir, &person) else { return reply(423, "text/plain; charset=utf-8", "the person is signed out; sign in on the computer and ask for a new link") };
                match w.public_key() {
                    Ok(key) => reply(200, "text/html; charset=utf-8", page(&self.base, &key, token)),
                    Err(e) => reply(500, "text/plain; charset=utf-8", e),
                }
            }
            ("POST", "/subscribe") => {
                let Ok(v) = serde_json::from_slice::<Value>(&body[..body.len().min(8192)]) else { return json_reply(400, json!({"ok": false, "error": "the answer is not readable"})) };
                let s = |k: &str| v[k].as_str().unwrap_or("");
                let Some(person) = self.take(s("token")) else { return json_reply(410, json!({"ok": false, "error": "this link has expired or was already used"})) };
                let Some(w) = self.vaults.get(&self.dir, &person) else { return json_reply(423, json!({"ok": false, "error": "the person is signed out"})) };
                match w.subscribe(s("label"), s("endpoint"), s("p256dh"), s("auth")) {
                    Ok(id) => json_reply(200, json!({"ok": true, "id": id})),
                    Err(e) => json_reply(422, json!({"ok": false, "error": e})),
                }
            }
            _ => reply(404, "text/plain; charset=utf-8", "not found"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gitkeys::MemStore;
    use iyakki_webcrypto::{b64, p256};

    struct NoNet;
    impl crate::notifyhub::transport::Net for NoNet {
        fn post(&self, _: Option<&str>, _: &crate::notifyhub::webhook::HttpReq) -> Result<u16, String> {
            Err("no".into())
        }
        fn send_mail(&self, _: Option<&str>, _: &crate::notifyhub::transport::MailJob) -> Result<(), String> {
            Err("no".into())
        }
    }

    fn rig(tag: &str, named: bool) -> (PushPages, Arc<WebPush>, std::path::PathBuf) {
        let dir = std::env::temp_dir().join(format!("iyakki-pushpages-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        if named {
            std::fs::write(dir.join(CONFIG_FILE), r#"{"public_name": "Notify.Example.org"}"#).unwrap();
        }
        let reg: &'static OpenVaults = Box::leak(Box::new(OpenVaults::default()));
        let w = Arc::new(WebPush::new(Arc::new(MemStore::default()), Arc::new(NoNet), Arc::new(|_, _| {})));
        reg.register(&dir, "ada", w.clone());
        (PushPages::new(&dir, "/push", reg), w, dir)
    }

    fn token_of(link: &str) -> String {
        link.split("t=").nth(1).unwrap().to_string()
    }

    #[test]
    fn with_no_public_name_nothing_is_minted_or_served() {
        let (p, _, dir) = rig("unset", false);
        assert_eq!(public_name(&dir), None);
        assert_eq!(p.mint("ada").unwrap_err(), NO_NAME);
        let r = p.handle("GET", "/push/sw.js", b"");
        assert_eq!(r.status, 503);
        assert!(String::from_utf8_lossy(&r.body).contains("public HTTPS name"));
        assert_eq!(p.handle("POST", "/push/subscribe", b"{}").status, 503);
    }

    #[test]
    fn an_administrator_sets_and_removes_the_public_name_and_a_bad_one_changes_nothing() {
        let d = std::env::temp_dir().join(format!("iyakki-pushpages-setname-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        assert_eq!(public_name(&d), None, "unset by default");
        assert!(set_public_name(&d, "Notify.Example.org").unwrap().contains("notify.example.org"));
        assert_eq!(public_name(&d).as_deref(), Some("notify.example.org"));
        // a bad name is refused in words and the kept one stays
        assert!(set_public_name(&d, "localhost").unwrap_err().contains("plain host name"));
        assert!(set_public_name(&d, "x.example.org/path").is_err());
        assert_eq!(public_name(&d).as_deref(), Some("notify.example.org"));
        assert!(set_public_name(&d, "  ").unwrap().contains("removed"));
        assert_eq!(public_name(&d), None);
        // a bad name with nothing kept leaves no file behind
        assert!(set_public_name(&d, "nodot").is_err());
        assert!(!d.join(CONFIG_FILE).exists());
        assert!(owns("/push/sw.js") && owns("/push/subscribe") && !owns("/push/") && !owns("/push/other") && !owns("/ui/push/sw.js"));
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn the_name_is_the_owners_and_only_a_plain_host_counts() {
        let d = std::env::temp_dir().join(format!("iyakki-pushpages-names-{}", std::process::id()));
        std::fs::create_dir_all(&d).unwrap();
        for (text, want) in [(r#"{"public_name":"Notify.Example.org"}"#, Some("notify.example.org")), (r#"{"public_name":"h.example.org:8443"}"#, Some("h.example.org:8443")), (r#"{"public_name":"localhost"}"#, None), (r#"{"public_name":"a b.example.org"}"#, None), (r#"{"public_name":"x.example.org/path"}"#, None), (r#"{"public_name":"x.example.org:"}"#, None), (r#"{"public_name":""}"#, None), ("nope", None)] {
            std::fs::write(d.join(CONFIG_FILE), text).unwrap();
            assert_eq!(public_name(&d).as_deref(), want, "{text}");
        }
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn the_worker_and_the_page_are_served_and_a_browser_answer_is_saved_once_for_the_person_the_link_was_made_for() {
        let (p, w, dir) = rig("flow", true);
        let link = p.mint("ada").unwrap();
        assert!(link.starts_with("https://notify.example.org/push/subscribe?t="));
        let sw = p.handle("GET", "/push/sw.js", b"");
        assert_eq!((sw.status, sw.content_type), (200, "text/javascript; charset=utf-8"));
        let js = String::from_utf8(sw.body).unwrap();
        assert!(js.contains("'push'") && js.contains("showNotification") && !js.contains("innerHTML"));
        let t = token_of(&link);
        let pg = p.handle("GET", &format!("/push/subscribe?t={t}"), b"");
        assert_eq!(pg.status, 200);
        let html = String::from_utf8(pg.body).unwrap();
        assert!(html.contains("pushManager.subscribe") && html.contains(&w.public_key().unwrap()) && html.contains("/push/sw.js") && html.contains(&t));
        let browser = [3u8; 32];
        let sub = json!({"token": t, "label": "phone", "endpoint": "https://push.example.net/s/77", "p256dh": b64::url(&p256::public_from_private(&browser).unwrap()), "auth": b64::url(&[9u8; 16])});
        let ok = p.handle("POST", "/push/subscribe", sub.to_string().as_bytes());
        assert_eq!(ok.status, 200, "{}", String::from_utf8_lossy(&ok.body));
        assert!(w.status().unwrap().contains("1 browser(s) saved"));
        // the token is used up: the page and a second answer are refused
        assert_eq!(p.handle("GET", &format!("/push/subscribe?t={t}"), b"").status, 410);
        assert_eq!(p.handle("POST", "/push/subscribe", sub.to_string().as_bytes()).status, 410);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_bad_answer_a_signed_out_person_a_lapsed_link_and_a_hostile_token_are_refused() {
        let (mut p, w, dir) = rig("refuse", true);
        // a subscription that is not a real one is refused by the same checks as a pasted one, and the link is spent
        let t = token_of(&p.mint("ada").unwrap());
        let bad = json!({"token": t, "label": "phone", "endpoint": "http://plain.example.net/x", "p256dh": "x", "auth": "y"});
        assert_eq!(p.handle("POST", "/push/subscribe", bad.to_string().as_bytes()).status, 422);
        assert_eq!(w.status().unwrap().contains("No browsers"), true);
        // a person who signed out cannot mint, and a link made before the sign-out is refused with the vault locked
        let t2 = token_of(&p.mint("ada").unwrap());
        p.vaults.forget(&p.dir, "ada");
        assert_eq!(p.mint("ada").unwrap_err(), "your vault is locked: sign in again, then try again");
        assert_eq!(p.handle("GET", &format!("/push/subscribe?t={t2}"), b"").status, 423);
        // hostile tokens never reach the page text
        assert_eq!(p.handle("GET", "/push/subscribe?t=';alert(1);//", b"").status, 410);
        assert_eq!(p.handle("GET", "/push/subscribe", b"").status, 410);
        // a lapsed link
        p.vaults.register(&p.dir, "ada", w.clone());
        let t3 = token_of(&p.mint("ada").unwrap());
        p.now = || u64::MAX / 2;
        assert_eq!(p.handle("GET", &format!("/push/subscribe?t={t3}"), b"").status, 410);
        assert_eq!(p.handle("GET", "/other/x", b"").status, 404);
        assert_eq!(p.handle("DELETE", "/push/subscribe", b"").status, 404);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
