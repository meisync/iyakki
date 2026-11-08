//! The private store server: the logic of `iyakki store-server`. It serves a signed catalog and immutable packages and
//! accepts signed packages from publishers who hold a token. Request handling is a pure function of a parsed request
//! (`StoreServer::handle`), so it is tested without a socket; `server_http.rs` puts it on a listener.
//!
//! Layout of the data folder: `catalog.json` (re-signed after every change), `yanked.json` (signed), `state.json`,
//! `packages/<sha256>.iyp` (never rewritten), `publishers.json` (token hashes only), `trusted-keys.txt` and `roots.txt`
//! (optional), `audit.log`. The catalog key is read from a file (a Kubernetes Secret) and never written anywhere.
//! See docs/STORE.md, section 13.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use ed25519_dalek::SigningKey;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use super::catalog::{sha256_hex, Catalog, Entry};
use super::manifest::{Meta, Publisher as MetaPublisher};
use super::revocation::Revocations;
use super::trust::{Judge, Level, Roots};
use crate::pkg::{self, newer, public_key_hex, signing_key_from_hex};

#[path = "server_components.rs"]
mod components;

pub use components::{parse_range, Body, Slice, StreamResp, BUF as BLOB_BUF};

pub const DEFAULT_KINDS: [&str; 8] = ["skill", "tool", "mcp", "agent", "template", "theme", "extension", "tile"];

#[derive(Clone)]
pub struct Config {
    pub data: PathBuf,
    pub id: String,
    pub name: String,
    /// sha256 (hex) of the bootstrap administrator token.
    pub admin_token_hash: Option<String>,
    pub read_only: bool,
    pub max_body: usize,
    pub writes_per_min: usize,
    pub reads_per_min: usize,
    pub fails_per_min: usize,
    /// Accept packages signed by a key nobody vouches for (they are listed as Community).
    pub allow_community: bool,
    pub kinds: Vec<String>,
    /// System components (docs/development/COMPONENT-UPDATES-PLAN.md): the names taken, the largest blob of each (the default for the rest),
    /// and the room kept free on the data folder's disk beyond an upload.
    pub component_names: Vec<String>,
    pub component_caps: HashMap<String, u64>,
    pub max_component: u64,
    pub disk_margin: u64,
}

impl Config {
    pub fn new(data: &Path) -> Config {
        Config { data: data.to_path_buf(), id: "private-store".into(), name: "Private store".into(), admin_token_hash: None, read_only: false, max_body: 4 << 20, writes_per_min: 20, reads_per_min: 600, fails_per_min: 10, allow_community: true, kinds: DEFAULT_KINDS.iter().map(|s| s.to_string()).collect(), component_names: super::compmanifest::COMPONENTS.iter().map(|s| s.to_string()).collect(), component_caps: HashMap::new(), max_component: 1 << 30, disk_margin: 256 << 20 }
    }
}

#[derive(Clone, Debug)]
pub struct Req {
    pub method: String,
    pub path: String,
    pub query: HashMap<String, String>,
    /// Lower-case names.
    pub headers: HashMap<String, String>,
    pub body: Vec<u8>,
}

#[derive(Clone, Debug)]
pub struct Resp {
    pub status: u16,
    pub content_type: &'static str,
    pub body: Vec<u8>,
    pub headers: Vec<(&'static str, String)>,
}

impl Resp {
    fn new(status: u16, content_type: &'static str, body: Vec<u8>) -> Resp {
        Resp { status, content_type, body, headers: vec![] }
    }

    fn json(status: u16, v: Value) -> Resp {
        Resp::new(status, "application/json", v.to_string().into_bytes())
    }

    fn err(status: u16, msg: &str) -> Resp {
        Resp::json(status, json!({"ok": false, "error": msg}))
    }

    pub fn text(&self) -> String {
        String::from_utf8_lossy(&self.body).into_owned()
    }
}

#[derive(Serialize, Deserialize, Clone, Debug)]
struct Publisher {
    id: String,
    name: String,
    token_hash: String,
    created: i64,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
struct Stored {
    entry: Entry,
    /// The publisher (token owner) that uploaded it.
    owner: String,
    level: String,
    yanked: Option<String>,
}

#[derive(Serialize, Deserialize, Default, Debug)]
struct State {
    serial: u64,
    packages: Vec<Stored>,
}

enum Who {
    Admin,
    Publisher(Publisher),
}

pub struct StoreServer {
    cfg: Config,
    key: SigningKey,
    /// Serialises everything that writes.
    write: Mutex<()>,
    rate: Mutex<HashMap<String, Vec<i64>>>,
    clock: Arc<dyn Fn() -> i64 + Send + Sync>,
    /// Free bytes of the disk holding a folder (a fake in tests).
    free_space: Arc<dyn Fn(&Path) -> Option<u64> + Send + Sync>,
    /// Blobs being uploaded right now (one upload per blob at a time).
    uploading: Mutex<std::collections::HashSet<String>>,
}

/// A fingerprint a person can compare: the SHA-256 of the public key, 32 hex characters in groups of four.
pub fn fingerprint(key_hex: &str) -> String {
    let bytes = pkg::unhex::<32>(key_hex).unwrap_or([0; 32]);
    let h = sha256_hex(&bytes);
    h[..32].as_bytes().chunks(4).map(|c| std::str::from_utf8(c).unwrap_or("")).collect::<Vec<_>>().join(" ")
}

pub fn hash_token(token: &str) -> String {
    sha256_hex(token.trim().as_bytes())
}

fn ct_eq(a: &str, b: &str) -> bool {
    a.len() == b.len() && a.bytes().zip(b.bytes()).fold(0u8, |d, (x, y)| d | (x ^ y)) == 0
}

/// The catalog key made from a seed (the Secret of a deployed store holds a random seed, not a key file).
pub fn key_from_seed(seed: &str) -> Result<String, String> {
    let s = seed.trim();
    if s.len() < 24 {
        return Err("the catalog seed must be at least 24 characters".into());
    }
    Ok(sha256_hex(format!("iyakki-store-catalog-key\n{s}").as_bytes()))
}

/// A new random token (`iyk_` and 64 hex characters).
pub fn new_token() -> Result<String, String> {
    use std::io::Read;
    let mut b = [0u8; 32];
    std::fs::File::open("/dev/urandom").and_then(|mut f| f.read_exact(&mut b)).map_err(|e| e.to_string())?;
    Ok(format!("iyk_{}", pkg::hex(&b)))
}

fn clean(s: &str) -> String {
    s.chars().map(|c| if c.is_control() { ' ' } else { c }).take(200).collect()
}

fn esc(s: &str) -> String {
    s.chars().map(|c| match c { '&' => "&amp;".into(), '<' => "&lt;".into(), '>' => "&gt;".into(), '"' => "&quot;".into(), '\'' => "&#39;".into(), c => c.to_string() }).collect()
}

fn atomic_write(p: &Path, bytes: &[u8]) -> Result<(), String> {
    let tmp = p.with_extension("tmp");
    std::fs::write(&tmp, bytes).and_then(|_| std::fs::rename(&tmp, p)).map_err(|e| format!("{}: {e}", p.display()))
}

fn is_package_file(f: &str) -> bool {
    f.len() == 68 && f.ends_with(".iyp") && f[..64].bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

impl StoreServer {
    /// Open (and create) the data folder. `key_text` is the catalog key as `iyakki pkg keygen` writes it.
    pub fn open(cfg: Config, key_text: &str) -> Result<StoreServer, String> {
        let key = signing_key_from_hex(key_text).map_err(|e| format!("the catalog key: {e}"))?;
        std::fs::create_dir_all(cfg.data.join("packages")).map_err(|e| format!("{}: {e}", cfg.data.display()))?;
        let s = StoreServer { cfg, key, write: Mutex::new(()), rate: Mutex::new(HashMap::new()), clock: Arc::new(crate::clock::now_unix), free_space: Arc::new(crate::sharedsite::statvfs_free), uploading: Mutex::new(Default::default()) };
        let _g = s.write.lock().unwrap_or_else(|e| e.into_inner());
        let st = s.state();
        s.regenerate(&st, false)?;
        drop(_g);
        Ok(s)
    }

    pub fn with_clock(mut self, c: Arc<dyn Fn() -> i64 + Send + Sync>) -> StoreServer {
        self.clock = c;
        self
    }

    pub fn with_free_space(mut self, f: Arc<dyn Fn(&Path) -> Option<u64> + Send + Sync>) -> StoreServer {
        self.free_space = f;
        self
    }

    pub fn catalog_key(&self) -> String {
        public_key_hex(&self.key)
    }

    pub fn config(&self) -> &Config {
        &self.cfg
    }

    fn now(&self) -> i64 {
        (self.clock)()
    }

    fn path(&self, f: &str) -> PathBuf {
        self.cfg.data.join(f)
    }

    fn state(&self) -> State {
        std::fs::read(self.path("state.json")).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default()
    }

    fn publishers(&self) -> Vec<Publisher> {
        std::fs::read(self.path("publishers.json")).ok().and_then(|b| serde_json::from_slice::<Value>(&b).ok()).and_then(|v| serde_json::from_value(v["publishers"].clone()).ok()).unwrap_or_default()
    }

    fn save_publishers(&self, p: &[Publisher]) -> Result<(), String> {
        atomic_write(&self.path("publishers.json"), json!({"publishers": p}).to_string().as_bytes())
    }

    fn roots(&self) -> Roots {
        let mut r = Roots::pinned();
        if let Ok(t) = std::fs::read_to_string(self.path("roots.txt")) {
            for x in Roots::parse(&t).list() {
                r.push(x.clone());
            }
        }
        r
    }

    fn own_keys(&self) -> Vec<(String, String)> {
        std::fs::read_to_string(self.path("trusted-keys.txt")).map(|t| t.lines().filter_map(|l| l.trim().split_once(char::is_whitespace).map(|(k, n)| (k.to_ascii_lowercase(), n.trim().to_string())).or_else(|| (l.trim().len() == 64).then(|| (l.trim().to_ascii_lowercase(), "publisher".to_string())))).collect()).unwrap_or_default()
    }

    /// The lines of the audit log (never a token or a secret), oldest first.
    pub fn audit_lines(&self) -> Vec<String> {
        std::fs::read_to_string(self.path("audit.log")).map(|t| t.lines().map(str::to_string).collect()).unwrap_or_default()
    }

    pub fn audit(&self, peer: &str, who: &str, action: &str, detail: &str, result: &str) {
        use std::io::Write;
        let line = format!("{} {} {} {} {} {}\n", self.now(), clean(peer).replace(' ', "_"), clean(who).replace(' ', "_"), action, clean(result).replace(' ', "_"), clean(detail));
        if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(self.path("audit.log")) {
            let _ = f.write_all(line.as_bytes());
        }
    }

    // ---------------------------------------------------------------------------------------- rate limits

    fn allow(&self, bucket: &str, limit: usize) -> bool {
        let now = self.now();
        let mut m = self.rate.lock().unwrap_or_else(|e| e.into_inner());
        if m.len() > 4096 {
            m.retain(|_, v| v.last().is_some_and(|t| now - t < 60));
        }
        let v = m.entry(bucket.to_string()).or_default();
        v.retain(|t| now - t < 60);
        if v.len() >= limit {
            return false;
        }
        v.push(now);
        true
    }

    fn auth(&self, r: &Req) -> Option<Who> {
        let tok = r.headers.get("authorization")?.strip_prefix("Bearer ").or_else(|| r.headers.get("authorization")?.strip_prefix("bearer "))?.trim();
        if tok.is_empty() || tok.len() > 200 {
            return None;
        }
        let h = hash_token(tok);
        if self.cfg.admin_token_hash.as_deref().is_some_and(|a| ct_eq(a, &h)) {
            return Some(Who::Admin);
        }
        self.publishers().into_iter().find(|p| ct_eq(&p.token_hash, &h)).map(Who::Publisher)
    }

    // ---------------------------------------------------------------------------------------- the catalog

    fn build_catalog(&self, st: &State) -> Catalog {
        let mut best: HashMap<&str, &Stored> = HashMap::new();
        for p in st.packages.iter().filter(|p| p.yanked.is_none()) {
            let e = best.entry(p.entry.name.as_str()).or_insert(p);
            if newer(&p.entry.version, &e.entry.version) {
                *e = p;
            }
        }
        let mut entries: Vec<Entry> = best.values().map(|s| s.entry.clone()).collect();
        entries.sort_by(|a, b| a.id.cmp(&b.id));
        let mut c = Catalog { v: 1, id: self.cfg.id.clone(), name: self.cfg.name.clone(), serial: st.serial, issued: self.now(), expires: 0, entries, ..Default::default() };
        c.sign(&self.key);
        c
    }

    fn yanked_doc(&self, st: &State) -> String {
        let pk: Vec<Value> = st.packages.iter().filter_map(|p| p.yanked.as_ref().map(|r| json!({"name": p.entry.name, "version": p.entry.version, "sha256": p.entry.sha256, "reason": r}))).collect();
        let mut doc = json!({"v": 1, "id": self.cfg.id, "serial": st.serial, "issued": self.now(), "packages": pk, "key": public_key_hex(&self.key)});
        let sig = super::trust::sign_doc("yanked", &doc, &self.key);
        doc["sig"] = Value::String(sig);
        doc.to_string()
    }

    /// Write the catalog and the yanked list for `st`. With `bump` the serial goes up first (every change).
    fn regenerate(&self, st: &State, bump: bool) -> Result<u64, String> {
        let mut st2 = State { serial: st.serial, packages: st.packages.clone() };
        if bump || st2.serial == 0 {
            st2.serial += 1;
        }
        let cat = self.build_catalog(&st2);
        atomic_write(&self.path("state.json"), &serde_json::to_vec(&st2).unwrap_or_default())?;
        atomic_write(&self.path("catalog.json"), cat.to_json().as_bytes())?;
        atomic_write(&self.path("yanked.json"), self.yanked_doc(&st2).as_bytes())?;
        Ok(st2.serial)
    }

    // ---------------------------------------------------------------------------------------- requests

    pub fn handle(&self, r: &Req, peer: &str) -> Resp {
        let mut resp = self.route(r, peer);
        resp.headers.push(("X-Content-Type-Options", "nosniff".into()));
        resp.headers.push(("Content-Security-Policy", "default-src 'none'; style-src 'unsafe-inline'".into()));
        resp.headers.push(("Cache-Control", if r.path.starts_with("/packages/") { "public, max-age=31536000, immutable".into() } else { "no-cache".into() }));
        resp
    }

    fn route(&self, r: &Req, peer: &str) -> Resp {
        let write = matches!(r.method.as_str(), "POST" | "DELETE");
        if write {
            if !self.allow(&format!("w:{peer}"), self.cfg.writes_per_min) {
                self.audit(peer, "-", "rate-limit", &r.path, "429");
                return Resp::err(429, "too many changes from this address; wait a minute");
            }
        } else if !self.allow(&format!("r:{peer}"), self.cfg.reads_per_min) {
            return Resp::err(429, "too many requests; wait a minute");
        }
        let path = r.path.as_str();
        match (r.method.as_str(), path) {
            ("GET", "/healthz") => Resp::new(200, "text/plain", b"ok\n".to_vec()),
            ("GET", "/catalog.json") => self.file("catalog.json", "application/json"),
            ("GET", "/yanked.json") => self.file("yanked.json", "application/json"),
            ("GET", "/") => self.home(),
            ("GET", super::sysannounce::PATH) => self.system_announcement(),
            ("GET", "/v1/info") => {
                let st = self.state();
                Resp::json(200, json!({"id": self.cfg.id, "name": self.cfg.name, "key": self.catalog_key(), "fingerprint": fingerprint(&self.catalog_key()), "serial": st.serial, "read_only": self.cfg.read_only, "packages": st.packages.len()}))
            }
            ("GET", p) if p.starts_with("/packages/") => {
                let f = &p["/packages/".len()..];
                if !is_package_file(f) {
                    return Resp::err(404, "no such package");
                }
                match std::fs::read(self.path("packages").join(f)) {
                    Ok(b) => Resp::new(200, "application/octet-stream", b),
                    Err(_) => Resp::err(404, "no such package"),
                }
            }
            ("GET", super::compmanifest::PATH) => self.components_doc(),
            ("POST", "/v1/components") => self.component_publish(r, peer),
            ("POST", "/v1/components/retention") => self.component_retention(r, peer),
            ("DELETE", p) if p.starts_with("/v1/components/") => self.component_yank(r, peer),
            // a small blob through `handle` (tests, tools); the socket code streams the body instead (`blob_put`, `blob_get`)
            ("PUT", p) if p.starts_with("/v1/blobs/") => self.blob_put(r, r.body.len() as u64, &mut &r.body[..], peer),
            ("POST", "/v1/publish") => self.publish(r, peer),
            ("POST", "/v1/publishers") => self.add_publisher(r, peer),
            ("GET", "/v1/publishers") => match self.auth(r) {
                Some(Who::Admin) => Resp::json(200, json!({"publishers": self.publishers().iter().map(|p| json!({"id": p.id, "name": p.name, "created": p.created})).collect::<Vec<_>>()})),
                _ => self.unauthorised(peer, &r.path),
            },
            ("DELETE", p) if p.starts_with("/v1/packages/") => self.yank(r, peer),
            (_, "/healthz" | "/catalog.json" | "/yanked.json" | "/" | "/v1/info" | super::sysannounce::PATH | super::compmanifest::PATH | "/v1/publish" | "/v1/publishers" | "/v1/components") => Resp::err(405, "method not allowed"),
            (_, p) if p.starts_with("/v1/blobs/") || p.starts_with("/v1/components/") => Resp::err(405, "method not allowed"),
            (_, p) if p.starts_with("/packages/") || p.starts_with("/v1/packages/") => Resp::err(405, "method not allowed"),
            _ => Resp::err(404, "not found"),
        }
    }

    /// `GET /v1/system.json`: the content of `system.json` in the data folder, signed with the catalog key when asked. No file: 404
    /// (nothing is announced). The catalog and everything else are untouched.
    fn system_announcement(&self) -> Resp {
        let raw = match std::fs::read(self.path("system.json")) {
            Ok(b) => b,
            Err(_) => return Resp::err(404, "nothing announced"),
        };
        match super::sysannounce::sign(&raw, &self.cfg.id, self.now(), &self.key) {
            Ok(doc) => Resp::new(200, "application/json", doc.into_bytes()),
            Err(e) => Resp::err(500, &e),
        }
    }

    fn file(&self, name: &str, ct: &'static str) -> Resp {
        match std::fs::read(self.path(name)) {
            Ok(b) => Resp::new(200, ct, b),
            Err(_) => Resp::err(500, "the catalog is not available"),
        }
    }

    fn unauthorised(&self, peer: &str, path: &str) -> Resp {
        self.audit(peer, "-", "auth", path, "401");
        if !self.allow(&format!("f:{peer}"), self.cfg.fails_per_min) {
            return Resp::err(429, "too many failed sign-ins; wait a minute");
        }
        let mut r = Resp::err(401, "a valid bearer token is needed");
        r.headers.push(("WWW-Authenticate", "Bearer".into()));
        r
    }

    fn locked_out(&self, peer: &str) -> bool {
        let now = self.now();
        let m = self.rate.lock().unwrap_or_else(|e| e.into_inner());
        m.get(&format!("f:{peer}")).is_some_and(|v| v.iter().filter(|t| now - **t < 60).count() >= self.cfg.fails_per_min)
    }

    fn home(&self) -> Resp {
        let st = self.state();
        let cat = self.build_catalog(&st);
        let mut h = format!("<!doctype html><html lang=\"en\"><head><meta charset=\"utf-8\"><meta name=\"viewport\" content=\"width=device-width,initial-scale=1\"><title>{}</title><style>body{{font:16px system-ui,sans-serif;max-width:46rem;margin:2rem auto;padding:0 1rem;color:#1b1b1b}}table{{border-collapse:collapse;width:100%}}td,th{{border-bottom:1px solid #ddd;padding:.4rem;text-align:left;vertical-align:top}}.b{{border:1px solid;border-radius:.6rem;padding:0 .5rem;font-size:.8rem}}.Official{{color:#006b7a}}.Verified{{color:#0a6b2a}}.Trusted{{color:#1a4fb4}}.Community{{color:#8a5a00}}code{{background:#f0f0f0;padding:0 .2rem}}</style></head><body><h1>{}</h1><p>A private IYAKKI app store. Add it in STORE, Settings, Catalogs with the address of this page plus <code>/catalog.json</code>. Catalog key fingerprint: <code>{}</code></p>", esc(&self.cfg.name), esc(&self.cfg.name), esc(&fingerprint(&self.catalog_key())));
        if cat.entries.is_empty() {
            h.push_str("<p>Nothing has been published yet.</p>");
        } else {
            h.push_str("<table><tr><th>Name</th><th>Type</th><th>Version</th><th>Publisher</th><th>Trust</th></tr>");
            for e in &cat.entries {
                let level = st.packages.iter().find(|p| p.entry.id == e.id && p.entry.version == e.version).map_or("Community", |p| p.level.as_str());
                h.push_str(&format!("<tr><td><b>{}</b><br>{}</td><td>{}</td><td>{}</td><td>{}</td><td><span class=\"b {}\">{}</span></td></tr>", esc(e.title()), esc(&e.meta.summary), esc(&e.kind), esc(&e.version), esc(&e.meta.publisher.name), esc(level), esc(level)));
            }
            h.push_str("</table>");
        }
        h.push_str("</body></html>");
        Resp::new(200, "text/html; charset=utf-8", h.into_bytes())
    }

    fn add_publisher(&self, r: &Req, peer: &str) -> Resp {
        if self.locked_out(peer) {
            return Resp::err(429, "too many failed sign-ins; wait a minute");
        }
        if !matches!(self.auth(r), Some(Who::Admin)) {
            return self.unauthorised(peer, &r.path);
        }
        if self.cfg.read_only {
            return Resp::err(403, "this store is read-only");
        }
        let v: Value = serde_json::from_slice(&r.body).unwrap_or(Value::Null);
        let (id, name) = (v["id"].as_str().unwrap_or(""), v["name"].as_str().unwrap_or(""));
        match self.create_publisher(id, if name.is_empty() { id } else { name }) {
            Ok(tok) => {
                self.audit(peer, "admin", "publisher-add", id, "201");
                Resp::json(201, json!({"ok": true, "id": id, "token": tok, "note": "shown once; only its hash is kept"}))
            }
            Err(e) => Resp::err(400, &e),
        }
    }

    /// Add a publisher and return the new token (shown once). Also behind `iyakki store-server publisher-add`.
    pub fn create_publisher(&self, id: &str, name: &str) -> Result<String, String> {
        let _g = self.write.lock().unwrap_or_else(|e| e.into_inner());
        if !super::manifest::valid_publisher_id(id) || id == "admin" {
            return Err("the publisher id: 2 to 32 lowercase letters, digits and dashes".into());
        }
        let mut ps = self.publishers();
        if ps.iter().any(|p| p.id == id) {
            return Err("that publisher already exists".into());
        }
        let name: String = clean(name).chars().take(60).collect();
        let tok = new_token()?;
        ps.push(Publisher { id: id.into(), name, token_hash: hash_token(&tok), created: self.now() });
        self.save_publishers(&ps)?;
        Ok(tok)
    }

    fn publish(&self, r: &Req, peer: &str) -> Resp {
        if self.locked_out(peer) {
            return Resp::err(429, "too many failed sign-ins; wait a minute");
        }
        let Some(who) = self.auth(r) else { return self.unauthorised(peer, &r.path) };
        let (owner, owner_name) = match &who {
            Who::Admin => ("admin".to_string(), "Store administrator".to_string()),
            Who::Publisher(p) => (p.id.clone(), p.name.clone()),
        };
        if self.cfg.read_only {
            self.audit(peer, &owner, "publish", "-", "403-read-only");
            return Resp::err(403, "this store is read-only");
        }
        if r.body.len() > self.cfg.max_body {
            self.audit(peer, &owner, "publish", "-", "413");
            return Resp::err(413, &format!("the package is larger than {} KiB", self.cfg.max_body / 1024));
        }
        let bytes = match package_bytes(r) {
            Ok(b) => b,
            Err(e) => return Resp::err(400, &e),
        };
        let _g = self.write.lock().unwrap_or_else(|e| e.into_inner());
        match self.accept(&bytes, &owner, &owner_name) {
            Ok((name, version, sha, level, serial)) => {
                self.audit(peer, &owner, "publish", &format!("{name} {version} {sha}"), "200");
                Resp::json(200, json!({"ok": true, "name": name, "version": version, "sha256": sha, "trust": level, "serial": serial}))
            }
            Err((code, msg)) => {
                self.audit(peer, &owner, "publish", &msg, &code.to_string());
                Resp::err(code, &msg)
            }
        }
    }

    /// Verify a package and add it. Called with the write lock held.
    fn accept(&self, bytes: &[u8], owner: &str, owner_name: &str) -> Result<(String, String, String, String, u64), (u16, String)> {
        let text = std::str::from_utf8(bytes).map_err(|_| (400, "not a package (not text)".to_string()))?;
        let p = pkg::parse(text).map_err(|e| (422, format!("not accepted: {e}")))?;
        let m = &p.manifest;
        let signer = m.signer.clone().ok_or((422, "not accepted: the package is not signed (iyakki pkg sign)".to_string()))?;
        if !self.cfg.kinds.iter().any(|k| *k == m.kind) {
            return Err((422, format!("this store does not take {} packages", m.kind)));
        }
        if bytes.len() > pkg::max_bytes(&m.kind) {
            return Err((413, format!("{} packages are limited to {} KiB", m.kind, pkg::max_bytes(&m.kind) / 1024)));
        }
        let mut st = self.state();
        let others: Vec<&Stored> = st.packages.iter().filter(|s| s.entry.name == m.name).collect();
        if let Some(o) = others.iter().find(|s| s.owner != owner) {
            let _ = o;
            return Err((403, format!("the name {} belongs to another publisher", m.name)));
        }
        if let Some(top) = others.iter().map(|s| s.entry.version.as_str()).reduce(|a, b| if newer(b, a) { b } else { a }) {
            if !newer(&m.version, top) {
                return Err((409, format!("version {} is not newer than {top}: versions only go up", m.version)));
            }
        }
        let known: Vec<(String, String)> = st.packages.iter().map(|s| (s.entry.name.clone(), s.owner.clone())).collect();
        let like = super::trust::look_alikes(&m.name, owner, &known);
        if !like.is_empty() {
            return Err((409, format!("the name {} is too close to {}", m.name, like.join(", "))));
        }
        let mut meta = match m.meta.as_deref().map(Meta::parse) {
            Some(Ok(x)) => x,
            Some(Err(e)) => return Err((422, e)),
            None => Meta::default(),
        };
        if !meta.publisher.id.is_empty() && meta.publisher.id != owner {
            return Err((403, format!("the store information names the publisher {:?} but your token is for {owner:?}", meta.publisher.id)));
        }
        if meta.publisher.id.is_empty() {
            meta.publisher = MetaPublisher { id: owner.into(), name: owner_name.into(), url: String::new() };
        }
        let report = super::vet::vet_package(text, &[]);
        if let Some(pr) = report.problems.iter().find(|x| !x.contains("licence")) {
            return Err((422, format!("not accepted: {pr}")));
        }
        if m.license.is_empty() {
            return Err((422, "not accepted: licence is required".into()));
        }
        let revs = Revocations::default();
        let (roots, own) = (self.roots(), self.own_keys());
        let v = Judge { roots: &roots, own_keys: &own, revocations: &revs, now: self.now() }.verdict(m);
        if let Some(why) = &v.refusal {
            return Err((422, format!("not accepted: {why}")));
        }
        if v.level == Level::Community && !self.cfg.allow_community {
            return Err((422, "this store only takes packages signed by a key the administrator trusts (trusted-keys.txt)".into()));
        }
        if !matches!(v.level, Level::Community | Level::Trusted | Level::Verified | Level::Official) {
            return Err((422, format!("not accepted: trust level {}", v.level.label())));
        }
        let sha = sha256_hex(bytes);
        let file = format!("{sha}.iyp");
        let dest = self.path("packages").join(&file);
        if !dest.exists() {
            atomic_write(&dest, bytes).map_err(|e| (500, e))?;
        }
        let entry = Entry {
            id: m.name.clone(),
            kind: m.kind.clone(),
            name: m.name.clone(),
            version: m.version.clone(),
            license: m.license.clone(),
            size: bytes.len() as u64,
            sha256: sha.clone(),
            url: format!("packages/{file}"),
            signer,
            cert: m.cert.as_deref().and_then(|c| super::trust::Cert::parse(c).ok()),
            added: self.now(),
            meta,
            ..Default::default()
        };
        st.packages.push(Stored { entry, owner: owner.into(), level: v.level.label().into(), yanked: None });
        let serial = self.regenerate(&st, true).map_err(|e| (500, e))?;
        Ok((m.name.clone(), m.version.clone(), sha, v.level.label().to_string(), serial))
    }

    /// `DELETE /v1/packages/<name>[/<version>]?reason=...` (administrator): the package is taken out of the catalog and listed in
    /// the signed `yanked.json`; the files stay (hashes are immutable) but no catalog points to them.
    fn yank(&self, r: &Req, peer: &str) -> Resp {
        if self.locked_out(peer) {
            return Resp::err(429, "too many failed sign-ins; wait a minute");
        }
        if !matches!(self.auth(r), Some(Who::Admin)) {
            return self.unauthorised(peer, &r.path);
        }
        if self.cfg.read_only {
            return Resp::err(403, "this store is read-only");
        }
        let rest = &r.path["/v1/packages/".len()..];
        let (name, version) = match rest.split_once('/') {
            Some((n, v)) => (n, Some(v)),
            None => (rest, None),
        };
        if !crate::skills::valid_skill_name(name) || version.is_some_and(|v| !pkg::valid_version(v)) {
            return Resp::err(400, "give a package name and optionally a version");
        }
        let reason: String = clean(r.query.get("reason").map_or("withdrawn by the administrator", String::as_str)).chars().take(120).collect();
        let _g = self.write.lock().unwrap_or_else(|e| e.into_inner());
        let mut st = self.state();
        let mut n = 0;
        for s in st.packages.iter_mut().filter(|s| s.entry.name == name && version.is_none_or(|v| v == s.entry.version) && s.yanked.is_none()) {
            s.yanked = Some(reason.clone());
            n += 1;
        }
        if n == 0 {
            return Resp::err(404, "no such published package");
        }
        match self.regenerate(&st, true) {
            Ok(serial) => {
                self.audit(peer, "admin", "yank", &format!("{name} {} {reason}", version.unwrap_or("all")), "200");
                Resp::json(200, json!({"ok": true, "yanked": n, "serial": serial}))
            }
            Err(e) => Resp::err(500, &e),
        }
    }
}

/// The package text of a publish request: the raw body, or the file part of a `multipart/form-data` body.
fn package_bytes(r: &Req) -> Result<Vec<u8>, String> {
    let ct = r.headers.get("content-type").map(String::as_str).unwrap_or("");
    if !ct.to_ascii_lowercase().starts_with("multipart/form-data") {
        return Ok(r.body.clone());
    }
    let boundary = ct.split(';').filter_map(|p| p.trim().strip_prefix("boundary=")).next().ok_or("the multipart body has no boundary")?.trim_matches('"');
    let delim = format!("--{boundary}");
    let body = &r.body;
    let find = |from: usize, pat: &[u8]| body[from.min(body.len())..].windows(pat.len()).position(|w| w == pat).map(|i| i + from);
    let mut at = find(0, delim.as_bytes()).ok_or("the multipart body is malformed")?;
    while let Some(next) = find(at + delim.len(), delim.as_bytes()) {
        let part = &body[at + delim.len()..next];
        if let Some(h_end) = part.windows(4).position(|w| w == b"\r\n\r\n") {
            let head = String::from_utf8_lossy(&part[..h_end]).to_ascii_lowercase();
            if head.contains("filename=") || head.contains("name=\"package\"") {
                let data = &part[h_end + 4..];
                return Ok(data.strip_suffix(b"\r\n").unwrap_or(data).to_vec());
            }
        }
        at = next;
    }
    Err("the multipart body has no file part".into())
}
