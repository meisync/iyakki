//! HTML web-bundle tiles in the `tile-host` profile (F-WEBSVC-12, KI-821): a tile written as a small web page (the form new tiles take by
//! default, `docs/development/TILE-TESTING.md`) installed on the service and served from it. A bundle is a flat set of plain-named files
//! (`index.html` and its scripts, style sheets, pictures, data), the service keeps them under `<data>/tilebundles/<id>/` and serves each at
//! `GET /tiles/{id}/b/{file}`, public and static like the canvas page (nothing of anyone's is in it), under a strict content policy: scripts and
//! styles only from the same origin, no inline script (rejected when the bundle is installed, so the developer hears of it instead of a silent
//! block), no `eval`, and a sandbox of its own: a person opens the bundle through the wrapper page (`bundlewrap.rs`), which holds the token and
//! shows the bundle in a frame with no same-origin rights. The page reaches the service only through `window.iyakki` (`postMessage` to the wrapper,
//! which uses the WebSocket bridge of `tilebridge.rs`) and can make only the calls the bundle's manifest lists.
//!
//! This file is the part with no socket in it: the rules for a bundle, the folder it is kept in, and the page's policy.

use super::server::Fail;
use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

/// Files in one bundle, bytes in one file, bytes in all, and bundles on one service.
pub const MAX_FILES: usize = 48;
pub const MAX_FILE_BYTES: usize = 1 << 20;
pub const MAX_BUNDLE_BYTES: usize = 4 << 20;
pub const MAX_BUNDLES: usize = 32;

/// The policy of every file of a bundle: scripts and styles of this service only, no inline code, a sandbox with no same-origin rights (a throw-away
/// origin, even when a file is opened outside the wrapper), no connections (the page reaches the service only through the wrapper), and it may be
/// framed by this service's own wrapper page only.
pub const CSP: &str = "sandbox allow-scripts; default-src 'none'; script-src 'self'; style-src 'self'; img-src 'self' data:; font-src 'self'; connect-src 'none'; base-uri 'none'; form-action 'none'; frame-ancestors 'self'";

/// The script a bundle's page loads (`<script src="/tiles/bridge.js">`) to get `window.iyakki`. The page runs in a sandboxed frame of the wrapper page
/// (`bundlewrap.rs`), with a throw-away origin: it holds no token, cannot sign in and cannot reach the service itself. The script says hello to the
/// wrapper by `postMessage`, gets the list of calls the manifest allows (`ready`), and `iyakki.call(name, args)` (a promise of the value) asks the
/// wrapper to make the call. `iyakki.onready(callback)` runs when the bridge is open. A call outside the manifest is refused by the service, not
/// by this script. Opened outside the wrapper (no parent window) the script does nothing and `call` is refused.
pub const BRIDGE_JS: &str = r##"(function () {
  'use strict';
  var parts = location.pathname.split('/');
  var id = parts.length >= 4 && parts[1] === 'tiles' ? parts[2] : '';
  var embedded = window.parent !== window;
  var ready = false, caps = [], nextId = 1, pending = {}, waiting = [];
  window.addEventListener('message', function (ev) {
    if (ev.source !== window.parent) return;
    var m = ev.data;
    if (!m || m.iyakki !== 1) return;
    if (m.type === 'ready') {
      ready = true; caps = Array.isArray(m.caps) ? m.caps : [];
      var cbs = waiting; waiting = [];
      cbs.forEach(function (cb) { try { cb(caps.slice()); } catch (e) { /* a listener's fault is its own */ } });
    } else if (m.type === 'result') {
      var p = pending[m.id];
      if (p) { delete pending[m.id]; if (m.ok) p.res(m.value); else p.rej(new Error(typeof m.error === 'string' ? m.error : 'the call failed')); }
    }
  });
  function call(name, args) {
    return new Promise(function (res, rej) {
      if (!embedded) { rej(new Error('open this tile through its wrapper page (/tiles/' + id + '/w)')); return; }
      if (!ready) { rej(new Error('the bridge is not open')); return; }
      var i = nextId++;
      pending[i] = { res: res, rej: rej };
      window.parent.postMessage({ iyakki: 1, type: 'call', id: i, call: String(name), args: args === undefined ? {} : args }, '*');
    });
  }
  window.iyakki = Object.freeze({ version: 3, tile: id, call: call, onready: function (cb) { if (typeof cb !== 'function') return; if (ready) cb(caps.slice()); else waiting.push(cb); } });
  if (embedded) window.parent.postMessage({ iyakki: 1, type: 'hello' }, '*');
})();
"##;

/// The media type of a file name, from a short list. Nothing that can carry script by itself (SVG, XML) and nothing unknown is served.
pub fn content_type(name: &str) -> Option<&'static str> {
    let ext = name.rsplit_once('.')?.1.to_ascii_lowercase();
    Some(match ext.as_str() {
        "html" => "text/html; charset=utf-8",
        "css" => "text/css; charset=utf-8",
        "js" | "mjs" => "text/javascript; charset=utf-8",
        "json" => "application/json",
        "txt" => "text/plain; charset=utf-8",
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "woff2" => "font/woff2",
        _ => return None,
    })
}

/// A plain file name: letters, digits, `.`, `-` and `_`, at most 64, not starting with a dot, no path.
pub fn valid_file_name(n: &str) -> bool {
    !n.is_empty() && n.len() <= 64 && !n.starts_with('.') && !n.contains("..") && n.bytes().all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'-' | b'_'))
}

fn valid_id(id: &str) -> bool {
    !id.is_empty() && id.len() <= 40 && id.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-' || b == b'_')
}

/// Does an HTML page carry code of its own that the policy would block (a `<script>` without `src`, an `on...=` handler, a `javascript:` address)?
/// Returns what was found, for the developer.
pub fn inline_code(html: &str) -> Option<&'static str> {
    let low = html.to_ascii_lowercase();
    let mut from = 0;
    while let Some(i) = low[from..].find("<script") {
        let at = from + i;
        let end = low[at..].find('>').map_or(low.len(), |e| at + e);
        if !low[at..end].contains("src=") && !low[at..end].contains("src =") {
            return Some("a <script> without src (inline script is not run)");
        }
        from = end;
    }
    if low.contains("javascript:") {
        return Some("a javascript: address");
    }
    // an event handler attribute inside a tag: ` onclick=`, ` onload =`
    let b = low.as_bytes();
    let mut i = 0;
    while let Some(p) = low[i..].find(" on") {
        let s = i + p + 3;
        let name_end = s + b[s..].iter().take_while(|c| c.is_ascii_lowercase()).count();
        let after = low[name_end..].trim_start();
        if name_end > s && after.starts_with('=') {
            return Some("an on... event attribute (inline handlers are not run)");
        }
        i = s;
    }
    None
}

/// One installed bundle.
#[derive(Clone, Debug)]
pub struct Bundle {
    pub id: String,
    pub title: String,
    /// The calls the page may make through its bridge (`domain.method`, from the same short list as a WASM tile's).
    pub caps: Vec<String>,
    pub files: BTreeMap<String, Arc<Vec<u8>>>,
}

impl Bundle {
    pub fn size(&self) -> usize {
        self.files.values().map(|f| f.len()).sum()
    }
}

/// Check a bundle against the rules; nothing is kept here.
pub fn check(id: &str, title: &str, caps: &[String], files: &[(String, Vec<u8>)]) -> Result<Bundle, Fail> {
    let bad = |m: String| Fail::new(400, "invalid_input", &m);
    if !valid_id(id) {
        return Err(bad("the id is 1 to 40 characters of a-z, 0-9, - and _".into()));
    }
    if title.trim().is_empty() {
        return Err(bad("a bundle needs a title".into()));
    }
    if let Some(c) = caps.iter().find(|c| !super::tiles::install_cap_ok(c) || c.starts_with("pictures.")) {
        return Err(bad(format!("the call {c:?} is not one a tile may ask for")));
    }
    if files.is_empty() || files.len() > MAX_FILES {
        return Err(bad(format!("a bundle has 1 to {MAX_FILES} files")));
    }
    let mut map = BTreeMap::new();
    let mut total = 0usize;
    for (name, bytes) in files {
        if !valid_file_name(name) {
            return Err(bad(format!("{name:?} is not a plain file name (letters, digits, . - _; no folders)")));
        }
        if content_type(name).is_none() {
            return Err(bad(format!("{name:?}: this kind of file is not served (html, css, js, json, txt, png, jpg, gif, webp, woff2)")));
        }
        if bytes.len() > MAX_FILE_BYTES {
            return Err(bad(format!("{name:?} is over {} KiB", MAX_FILE_BYTES / 1024)));
        }
        total += bytes.len();
        if total > MAX_BUNDLE_BYTES {
            return Err(bad(format!("a bundle is at most {} MiB in all", MAX_BUNDLE_BYTES >> 20)));
        }
        if map.insert(name.clone(), Arc::new(bytes.clone())).is_some() {
            return Err(bad(format!("{name:?} is there twice")));
        }
    }
    let Some(index) = map.get("index.html") else { return Err(bad("a bundle has an index.html".into())) };
    let text = String::from_utf8_lossy(index);
    if let Some(what) = inline_code(&text) {
        return Err(bad(format!("index.html has {what}; put the code in a .js file of the bundle")));
    }
    for (name, bytes) in &map {
        if name.ends_with(".html") && name != "index.html" {
            if let Some(what) = inline_code(&String::from_utf8_lossy(bytes)) {
                return Err(bad(format!("{name} has {what}; put the code in a .js file of the bundle")));
            }
        }
    }
    Ok(Bundle { id: id.to_string(), title: title.chars().take(60).collect(), caps: caps.to_vec(), files: map })
}

/// The installed bundles, kept as plain files: `<dir>/<id>/bundle.json` and one file for each of the bundle's files.
#[derive(Default)]
pub struct Bundles {
    list: Mutex<BTreeMap<String, Arc<Bundle>>>,
}

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
}

fn write_private(path: &std::path::Path, bytes: &[u8]) -> Result<(), Fail> {
    use std::io::Write;
    use std::os::unix::fs::OpenOptionsExt;
    let tmp = path.with_extension("tmp");
    let _ = std::fs::remove_file(&tmp);
    let err = |e: std::io::Error| Fail::new(500, "internal", &format!("cannot keep the bundle: {e}"));
    let mut f = std::fs::OpenOptions::new().write(true).create_new(true).mode(0o600).open(&tmp).map_err(err)?;
    f.write_all(bytes).map_err(err)?;
    drop(f);
    std::fs::rename(&tmp, path).map_err(err)
}

pub fn bundles_dir(data_dir: &str) -> std::path::PathBuf {
    std::path::Path::new(data_dir).join("tilebundles")
}

impl Bundles {
    pub fn get(&self, id: &str) -> Option<Arc<Bundle>> {
        lock(&self.list).get(id).cloned()
    }

    pub fn list(&self) -> Vec<Arc<Bundle>> {
        lock(&self.list).values().cloned().collect()
    }

    /// Install (or replace) a bundle and keep it under `dir`. `taken` says an id a tile of the service already has.
    pub fn install(&self, dir: &std::path::Path, taken: &dyn Fn(&str) -> bool, id: &str, title: &str, caps: &[String], files: &[(String, Vec<u8>)]) -> Result<Arc<Bundle>, Fail> {
        let b = check(id, title, caps, files)?;
        if taken(id) {
            return Err(Fail::new(409, "conflict", "a tile with this id already exists"));
        }
        if lock(&self.list).len() >= MAX_BUNDLES && !lock(&self.list).contains_key(id) {
            return Err(Fail::new(409, "conflict", &format!("at most {MAX_BUNDLES} bundles")));
        }
        let folder = dir.join(id);
        let staging = dir.join(format!(".{id}.new"));
        let _ = std::fs::remove_dir_all(&staging);
        std::fs::create_dir_all(&staging).map_err(|e| Fail::new(500, "internal", &format!("cannot keep the bundle: {e}")))?;
        for (name, bytes) in &b.files {
            write_private(&staging.join(name), bytes)?;
        }
        write_private(&staging.join("bundle.json"), json!({"id": b.id, "title": b.title, "caps": b.caps}).to_string().as_bytes())?;
        let _ = std::fs::remove_dir_all(&folder);
        std::fs::rename(&staging, &folder).map_err(|e| Fail::new(500, "internal", &format!("cannot keep the bundle: {e}")))?;
        let b = Arc::new(b);
        lock(&self.list).insert(id.to_string(), b.clone());
        Ok(b)
    }

    pub fn remove(&self, dir: &std::path::Path, id: &str) -> Result<(), Fail> {
        if lock(&self.list).remove(id).is_none() {
            return Err(Fail::new(404, "not_found", "no such bundle"));
        }
        if valid_id(id) {
            let _ = std::fs::remove_dir_all(dir.join(id));
        }
        Ok(())
    }

    /// Read the kept bundles (at start). One that no longer fits the rules is left out and said in the log.
    pub fn load(&self, dir: &std::path::Path, log: &dyn Fn(&str)) -> usize {
        let Ok(rd) = std::fs::read_dir(dir) else { return 0 };
        let mut n = 0;
        for e in rd.flatten() {
            let id = e.file_name().to_string_lossy().into_owned();
            if !valid_id(&id) || !e.path().is_dir() {
                continue;
            }
            let Some(meta) = std::fs::read(e.path().join("bundle.json")).ok().and_then(|b| serde_json::from_slice::<Value>(&b).ok()) else {
                log(&format!("bundle {id}: bundle.json is missing; left out"));
                continue;
            };
            let caps: Vec<String> = meta["caps"].as_array().map(|a| a.iter().filter_map(|c| c.as_str().map(String::from)).collect()).unwrap_or_default();
            let mut files = Vec::new();
            if let Ok(fd) = std::fs::read_dir(e.path()) {
                for f in fd.flatten() {
                    let name = f.file_name().to_string_lossy().into_owned();
                    if name == "bundle.json" || !f.path().is_file() {
                        continue;
                    }
                    if let Ok(bytes) = std::fs::read(f.path()) {
                        files.push((name, bytes));
                    }
                }
            }
            match check(&id, meta["title"].as_str().unwrap_or(""), &caps, &files) {
                Ok(b) => {
                    lock(&self.list).insert(id, Arc::new(b));
                    n += 1;
                }
                Err(e) => log(&format!("bundle {id}: {}; left out", e.message)),
            }
        }
        n
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn index() -> Vec<u8> {
        b"<!doctype html><html><head><link rel=stylesheet href=app.css></head><body><script src=app.js></script></body></html>".to_vec()
    }

    fn files() -> Vec<(String, Vec<u8>)> {
        vec![("index.html".into(), index()), ("app.js".into(), b"window.iyakki.call('settings.get')".to_vec()), ("app.css".into(), b"body{}".to_vec())]
    }

    #[test]
    fn a_plain_bundle_is_accepted_and_every_rule_has_a_refusal() {
        let caps = vec!["settings.get".to_string()];
        assert!(check("notes", "Notes", &caps, &files()).is_ok());
        let msg = |r: Result<Bundle, Fail>| r.unwrap_err().message;
        assert!(msg(check("Bad Id", "x", &[], &files())).contains("id"));
        assert!(msg(check("n", " ", &[], &files())).contains("title"));
        assert!(msg(check("n", "x", &["tools.shell".into()], &files())).contains("tools.shell"));
        assert!(msg(check("n", "x", &[], &[])).contains("files"));
        let mut f = files();
        f.push(("../evil.js".into(), vec![]));
        assert!(msg(check("n", "x", &[], &f)).contains("plain file name"));
        let mut f = files();
        f.push(("pic.svg".into(), b"<svg/>".to_vec()));
        assert!(msg(check("n", "x", &[], &f)).contains("not served"));
        let mut f = files();
        f.push(("app.js".into(), vec![]));
        assert!(msg(check("n", "x", &[], &f)).contains("twice"));
        let mut f = files();
        f.push(("big.json".into(), vec![b' '; MAX_FILE_BYTES + 1]));
        assert!(msg(check("n", "x", &[], &f)).contains("KiB"));
        let f: Vec<_> = files().into_iter().filter(|(n, _)| n != "index.html").collect();
        assert!(msg(check("n", "x", &[], &f)).contains("index.html"));
        let many: Vec<(String, Vec<u8>)> = (0..MAX_FILES + 1).map(|i| (format!("f{i}.txt"), vec![])).collect();
        assert!(msg(check("n", "x", &[], &many)).contains("files"));
    }

    #[test]
    fn inline_code_is_found_and_a_normal_page_is_not_blamed() {
        assert!(inline_code("<script>alert(1)</script>").is_some());
        assert!(inline_code("<SCRIPT >x</SCRIPT>").is_some());
        assert!(inline_code("<a href=\"javascript:void(0)\">x</a>").is_some());
        assert!(inline_code("<button onclick=\"go()\">x</button>").is_some());
        assert!(inline_code("<button onClick = go()>x</button>").is_some());
        assert!(inline_code("<script src=\"app.js\"></script><p>one on one, once on top</p>").is_none());
        assert!(inline_code("<script type=module src=app.js></script>").is_none());
        let mut f = files();
        f[0].1 = b"<html><body><script>x()</script></body></html>".to_vec();
        assert!(check("n", "x", &[], &f).unwrap_err().message.contains("inline"));
    }

    #[test]
    fn the_bridge_script_is_frozen_and_a_manifest_call_must_be_well_formed() {
        assert!(BRIDGE_JS.contains("window.iyakki = Object.freeze("), "the bridge cannot be replaced by the page's own script");
        // a bundle may not ask for the picture table (it has none) and a manifest call must be well formed
        assert!(check("n", "x", &["pictures.load".into()], &files()).is_err());
    }

    #[test]
    fn file_names_and_types_are_a_short_list() {
        for ok in ["index.html", "app.js", "a-b_c.1.css", "logo.png"] {
            assert!(valid_file_name(ok) && content_type(ok).is_some(), "{ok}");
        }
        for bad in ["", ".hidden", "a/b.js", "a\\b.js", "..", "a..b.js", "sp ace.js", &"x".repeat(65)] {
            assert!(!valid_file_name(bad), "{bad}");
        }
        for none in ["x.svg", "x.xml", "x.exe", "x", "x.wasm"] {
            assert!(content_type(none).is_none(), "{none}");
        }
        assert!(CSP.contains("script-src 'self'") && CSP.contains("frame-ancestors 'self'") && !CSP.contains("unsafe-"));
        assert!(CSP.starts_with("sandbox allow-scripts;") && !CSP.contains("allow-same-origin") && CSP.contains("connect-src 'none'"), "a file of a bundle never has the service's origin");
    }

    #[test]
    fn a_bundle_is_kept_read_again_replaced_and_removed() {
        let dir = std::env::temp_dir().join(format!("iyakki-bundles-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let none = |_: &str| false;
        let s = Bundles::default();
        s.install(&dir, &none, "notes", "Notes", &["clipboard.copy".to_string()], &files()).unwrap();
        assert!(s.install(&dir, &|id| id == "counter", "counter", "x", &[], &files()).unwrap_err().message.contains("already"));
        assert_eq!(s.get("notes").unwrap().files.len(), 3);
        // kept privately
        {
            use std::os::unix::fs::PermissionsExt;
            let m = std::fs::metadata(dir.join("notes").join("index.html")).unwrap().permissions().mode() & 0o777;
            assert_eq!(m, 0o600);
        }
        let again = Bundles::default();
        let said = Mutex::new(Vec::new());
        assert_eq!(again.load(&dir, &|l| said.lock().unwrap().push(l.to_string())), 1, "{:?}", said.lock().unwrap());
        assert_eq!(again.get("notes").unwrap().caps, ["clipboard.copy"]);
        // replaced
        let mut f = files();
        f[1].1 = b"// new".to_vec();
        s.install(&dir, &none, "notes", "Notes 2", &[], &f).unwrap();
        assert_eq!(s.get("notes").unwrap().title, "Notes 2");
        assert_eq!(s.list().len(), 1);
        // a damaged kept bundle (the index gone) is left out and said
        std::fs::remove_file(dir.join("notes").join("index.html")).unwrap();
        let third = Bundles::default();
        assert_eq!(third.load(&dir, &|l| said.lock().unwrap().push(l.to_string())), 0);
        assert!(said.lock().unwrap().iter().any(|l| l.contains("notes") && l.contains("index.html")));
        s.remove(&dir, "notes").unwrap();
        assert!(!dir.join("notes").exists());
        assert_eq!(s.remove(&dir, "notes").unwrap_err().status, 404);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_number_of_bundles_is_capped() {
        let dir = std::env::temp_dir().join(format!("iyakki-bundles-cap-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let s = Bundles::default();
        for i in 0..MAX_BUNDLES {
            s.install(&dir, &|_| false, &format!("b{i}"), "B", &[], &files()).unwrap();
        }
        assert_eq!(s.install(&dir, &|_| false, "one-more", "B", &[], &files()).unwrap_err().status, 409);
        // replacing one that exists is still allowed
        assert!(s.install(&dir, &|_| false, "b0", "B again", &[], &files()).is_ok());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
