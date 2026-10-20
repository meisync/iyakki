//! Catalogs: the signed index of what can be installed, and where it comes from. A catalog is only an index: the
//! sha256 in an entry pins the exact package, and the trust level is always worked out again from the package itself,
//! so a catalog can describe a package but never make it more trusted than its own signature. See docs/STORE.md, 4.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};

use super::manifest::{valid_publisher_id, Meta};
use super::trust::{verify_doc, Cert, Roots};
use crate::pkg::{public_key_hex, unhex, valid_version, KINDS};

pub const MAX_CATALOG_BYTES: usize = 2 << 20;
pub const MAX_ENTRIES: usize = 2000;
/// The address of the official IYAKKI store catalog (on the project's portal). Empty until the owner publishes it: the
/// store then says "coming soon" and adds nothing. This is the only place the address is written.
pub const OFFICIAL_STORE_URL: &str = "";

/// What the entry does instead of installing a package (a CLI entry opens the CLI tile's own installer).
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq, Default)]
#[serde(default)]
pub struct Action {
    /// The tile to open: `cli`.
    pub open: String,
    pub id: String,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq, Default)]
#[serde(default)]
pub struct Entry {
    pub id: String,
    pub kind: String,
    pub name: String,
    pub version: String,
    pub license: String,
    pub size: u64,
    /// sha256 of the whole package file.
    pub sha256: String,
    /// Where the package is: absolute, or relative to the catalog. Empty for built-in items and links.
    pub url: String,
    /// The key that signed the package and its certificate, copied here so the badge can be shown without downloading.
    pub signer: String,
    pub cert: Option<Cert>,
    pub featured: bool,
    /// When it was added (unix seconds), for "New".
    pub added: i64,
    /// A made-up example: shown, never installable.
    pub demo: bool,
    pub action: Option<Action>,
    /// The key of an item compiled into this image (`builtin::package`).
    pub builtin: String,
    #[serde(flatten)]
    pub meta: Meta,
}

impl Entry {
    pub fn title(&self) -> &str {
        if self.meta.title.is_empty() { &self.name } else { &self.meta.title }
    }
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq, Default)]
#[serde(default)]
pub struct Catalog {
    pub v: u32,
    pub id: String,
    pub name: String,
    pub serial: u64,
    pub issued: i64,
    /// Unix seconds after which the catalog may not be used to install (0: never).
    pub expires: i64,
    /// Other places serving the same files (base addresses).
    pub mirrors: Vec<String>,
    pub key: String,
    pub cert: Option<Cert>,
    pub entries: Vec<Entry>,
    pub sig: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CatalogSigner {
    Builtin,
    Official(String),
    Verified(String),
    Trusted(String),
    Unknown(String),
    Unsigned,
}

impl CatalogSigner {
    pub fn label(&self) -> String {
        match self {
            CatalogSigner::Builtin => "built into this image".into(),
            CatalogSigner::Official(id) => format!("signed by the IYAKKI root key {id}"),
            CatalogSigner::Verified(n) => format!("signed by {n} (identity verified by IYAKKI)"),
            CatalogSigner::Trusted(l) => format!("signed by {l} (your trusted key)"),
            CatalogSigner::Unknown(k) => format!("signed by an unknown key {k}…"),
            CatalogSigner::Unsigned => "not signed".into(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CatalogCheck {
    pub signer: CatalogSigner,
    pub warnings: Vec<String>,
    /// Past its `expires`: it can be browsed but nothing can be installed from it.
    pub expired: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CatalogError {
    TooLarge,
    Malformed(String),
    BadSignature,
    Unsigned,
    Rollback { have: u64, got: u64 },
}

impl std::fmt::Display for CatalogError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            CatalogError::TooLarge => write!(f, "the catalog is larger than {} MiB", MAX_CATALOG_BYTES >> 20),
            CatalogError::Malformed(m) => write!(f, "the catalog cannot be read: {m}"),
            CatalogError::BadSignature => write!(f, "the catalog has a bad signature"),
            CatalogError::Unsigned => write!(f, "this catalog is not signed, and catalogs from the network must be"),
            CatalogError::Rollback { have, got } => write!(f, "this catalog (number {got}) is older than the one already seen (number {have})"),
        }
    }
}

fn valid_catalog_id(id: &str) -> bool {
    valid_publisher_id(id)
}

fn url_safe(u: &str) -> bool {
    if u.is_empty() {
        return true;
    }
    if u.len() > 400 || u.chars().any(|c| c.is_control() || c == ' ' || c == '\\') {
        return false;
    }
    if u.starts_with("https://") || u.starts_with("http://") {
        return true;
    }
    // relative: no scheme, no absolute path, no climbing out
    !u.contains(':') && !u.starts_with('/') && !u.split('/').any(|p| p == "..")
}

fn entry_problem(e: &Entry) -> Option<String> {
    if !crate::skills::valid_skill_name(&e.id) {
        return Some(format!("entry id {:?} is not a valid name", e.id));
    }
    if !KINDS.contains(&e.kind.as_str()) {
        return Some(format!("{}: unknown type {:?}", e.id, e.kind));
    }
    if !crate::skills::valid_skill_name(&e.name) || !valid_version(&e.version) {
        return Some(format!("{}: bad name or version", e.id));
    }
    if let Err(m) = e.meta.validate() {
        return Some(format!("{}: {m}", e.id));
    }
    if !url_safe(&e.url) {
        return Some(format!("{}: the package address is not allowed", e.id));
    }
    if !e.url.is_empty() && (unhex::<32>(&e.sha256).is_none() || e.size == 0 || e.size as usize > crate::pkg::max_bytes(&e.kind)) {
        return Some(format!("{}: a package needs a sha256 and a size", e.id));
    }
    if !e.signer.is_empty() && unhex::<32>(&e.signer).is_none() {
        return Some(format!("{}: the signer is not a key", e.id));
    }
    if e.url.is_empty() && e.builtin.is_empty() && e.action.is_none() && !e.demo {
        return Some(format!("{}: it has no package, link or built-in item", e.id));
    }
    None
}

impl Catalog {
    /// Read, check the signature, drop entries that do not validate (and say so). `require_sig`: catalogs from the
    /// network must be signed; a folder or a stick may be unsigned (the administrator put it there).
    pub fn parse(text: &str, roots: &Roots, own_keys: &[(String, String)], now: i64, require_sig: bool) -> Result<(Catalog, CatalogCheck), CatalogError> {
        if text.len() > MAX_CATALOG_BYTES {
            return Err(CatalogError::TooLarge);
        }
        let v: Value = serde_json::from_str(text).map_err(|e| CatalogError::Malformed(e.to_string()))?;
        let mut c: Catalog = serde_json::from_value(v.clone()).map_err(|e| CatalogError::Malformed(e.to_string()))?;
        if c.v != 1 {
            return Err(CatalogError::Malformed("an unknown format version".into()));
        }
        if !valid_catalog_id(&c.id) || c.name.chars().count() > 60 || c.name.chars().any(|ch| ch.is_control()) {
            return Err(CatalogError::Malformed("the catalog id or name is not valid".into()));
        }
        if c.entries.len() > MAX_ENTRIES || c.mirrors.len() > 8 || c.mirrors.iter().any(|m| !(m.starts_with("https://") || m.starts_with("http://")) || m.len() > 300) {
            return Err(CatalogError::Malformed("too many entries or a bad mirror address".into()));
        }
        let mut warnings = Vec::new();
        let signer = if c.sig.is_empty() {
            if require_sig {
                return Err(CatalogError::Unsigned);
            }
            warnings.push("This catalog is not signed: its descriptions and prices cannot be vouched for (each package still carries its own signature).".to_string());
            CatalogSigner::Unsigned
        } else {
            if !verify_doc("catalog", &v, &c.key, &c.sig) {
                return Err(CatalogError::BadSignature);
            }
            let short = c.key.get(..12).unwrap_or("").to_string();
            if let Some(r) = roots.valid_at(&c.key, now) {
                CatalogSigner::Official(r.id.clone())
            } else if let Some((_, label)) = own_keys.iter().find(|(k, _)| *k == c.key) {
                CatalogSigner::Trusted(label.clone())
            } else if let Some(cert) = c.cert.as_ref().filter(|x| x.vouches(roots, &c.key, "catalog", now).is_ok()) {
                CatalogSigner::Verified(cert.publisher.name.clone())
            } else {
                warnings.push("This catalog is signed by a key nobody vouches for.".to_string());
                CatalogSigner::Unknown(short)
            }
        };
        let before = c.entries.len();
        let mut seen = std::collections::HashSet::new();
        let mut dropped = Vec::new();
        c.entries.retain(|e| match entry_problem(e) {
            Some(p) => {
                dropped.push(p);
                false
            }
            None if !seen.insert(e.id.clone()) => {
                dropped.push(format!("{}: listed twice", e.id));
                false
            }
            None => true,
        });
        if c.entries.len() < before {
            warnings.push(format!("{} entries were ignored: {}", before - c.entries.len(), dropped.iter().take(3).cloned().collect::<Vec<_>>().join("; ")));
        }
        let expired = c.expires != 0 && now > c.expires;
        if expired {
            warnings.push(format!("This catalog expired on {}: it can be browsed, but nothing can be installed from it until a newer one is loaded.", super::trust::date(c.expires)));
        }
        Ok((c, CatalogCheck { signer, warnings, expired }))
    }

    /// Sign (or re-sign) with `key`, which becomes the catalog's signer.
    pub fn sign(&mut self, key: &ed25519_dalek::SigningKey) {
        self.key = public_key_hex(key);
        self.sig.clear();
        self.sig = super::trust::sign_doc("catalog", &serde_json::to_value(&*self).unwrap_or_default(), key);
    }

    pub fn to_json(&self) -> String {
        serde_json::to_string_pretty(self).unwrap_or_default()
    }

    pub fn find(&self, id: &str) -> Option<&Entry> {
        self.entries.iter().find(|e| e.id == id)
    }
}

// ------------------------------------------------------------------------------------------------ what was seen

/// The highest serial seen for each catalog id.
#[derive(Serialize, Deserialize, Default, Debug)]
pub struct Seen(std::collections::BTreeMap<String, u64>);

impl Seen {
    pub fn load(dir: &Path) -> Seen {
        std::fs::read(dir.join("catalog-state.json")).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default()
    }

    pub fn note(&mut self, dir: &Path, id: &str, serial: u64) -> Result<(), CatalogError> {
        let have = self.0.get(id).copied().unwrap_or(0);
        if serial < have {
            return Err(CatalogError::Rollback { have, got: serial });
        }
        if serial > have {
            self.0.insert(id.to_string(), serial);
            let _ = std::fs::create_dir_all(dir);
            let _ = std::fs::write(dir.join("catalog-state.json"), serde_json::to_vec(self).unwrap_or_default());
        }
        Ok(())
    }
}

// ------------------------------------------------------------------------------------------------ settings

#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq, Default)]
#[serde(rename_all = "lowercase")]
pub enum SourceKind {
    #[default]
    Url,
    Folder,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq, Default)]
#[serde(default)]
pub struct SourceCfg {
    pub id: String,
    pub name: String,
    pub kind: SourceKind,
    /// The address of `catalog.json`, or a folder.
    pub location: String,
    pub enabled: bool,
    /// The catalog's public key, pinned after the person confirmed its fingerprint (a private store). Empty: not pinned.
    pub pin: String,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq, Default)]
#[serde(default)]
pub struct Config {
    pub sources: Vec<SourceCfg>,
    /// Off by default: a daily look for new versions when the tile is opened.
    pub auto_check: bool,
    pub last_check: i64,
    /// Show the catalog (Discover, Publishers, built-in listings): off by default, the tile shows what is installed.
    pub catalog_enabled: bool,
    /// The source (id) that `store_publish` and `iyakki pkg publish` use when none is named.
    pub publish_target: String,
}

impl Config {
    /// Whether the store shows a catalog: switched on in Settings, or there is a catalog configured.
    pub fn catalog_visible(&self) -> bool {
        self.catalog_enabled || !self.sources.is_empty()
    }

    pub fn load(dir: &Path) -> Config {
        std::fs::read(dir.join("config.json")).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default()
    }

    pub fn save(&self, dir: &Path) -> Result<(), String> {
        std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
        let tmp = dir.join("config.json.tmp");
        std::fs::write(&tmp, serde_json::to_vec_pretty(self).unwrap_or_default()).map_err(|e| e.to_string())?;
        std::fs::rename(&tmp, dir.join("config.json")).map_err(|e| e.to_string())
    }

    pub fn add(&mut self, name: &str, kind: SourceKind, location: &str) -> Result<String, String> {
        let location = location.trim();
        match kind {
            SourceKind::Url if !(location.starts_with("https://") || location.starts_with("http://")) || location.len() > 300 || location.chars().any(|c| c.is_control() || c == ' ') => {
                return Err("a catalog address starts with https:// or http://".into());
            }
            SourceKind::Folder if !location.starts_with('/') || location.contains("..") || location.len() > 300 => return Err("a catalog folder is an absolute path such as /mnt/usb".into()),
            _ => {}
        }
        if self.sources.len() >= 8 {
            return Err("up to 8 catalogs".into());
        }
        if self.sources.iter().any(|s| s.location == location) {
            return Err("that catalog is already there".into());
        }
        let name: String = name.chars().filter(|c| !c.is_control()).take(40).collect();
        let mut id = format!("src{}", self.sources.len() + 1);
        while self.sources.iter().any(|s| s.id == id) {
            id.push('x');
        }
        self.sources.push(SourceCfg { id: id.clone(), name: if name.trim().is_empty() { location.rsplit('/').find(|p| !p.is_empty()).unwrap_or("catalog").to_string() } else { name.trim().to_string() }, kind, location: location.to_string(), enabled: true, pin: String::new() });
        Ok(id)
    }

    /// Add a web catalog whose key the person has confirmed (`pin` is the 64-character public key).
    pub fn add_pinned(&mut self, name: &str, location: &str, pin: &str) -> Result<String, String> {
        if unhex::<32>(pin).is_none() {
            return Err("the pinned key is 64 hexadecimal characters".into());
        }
        let id = self.add(name, SourceKind::Url, location)?;
        if let Some(s) = self.sources.iter_mut().find(|s| s.id == id) {
            s.pin = pin.to_ascii_lowercase();
        }
        Ok(id)
    }
}

// ------------------------------------------------------------------------------------------------ getting bytes

/// A way to fetch an address. The real one uses the system's egress; tests use a table.
pub trait Fetch: Send + Sync {
    fn get(&self, url: &str, max: usize) -> Result<Vec<u8>, String>;
}

pub struct HttpFetch(pub Arc<dyn iyakki_proxy::Upstream>);

impl Fetch for HttpFetch {
    fn get(&self, url: &str, max: usize) -> Result<Vec<u8>, String> {
        let r = self.0.request("GET", url, &[], &[]).map_err(|e| format!("could not reach {}: {e}", host_of(url)))?;
        if r.status != 200 {
            return Err(format!("{} answered {}", host_of(url), r.status));
        }
        if r.body.len() > max {
            return Err(format!("{} sent more than {} KiB", host_of(url), max / 1024));
        }
        Ok(r.body)
    }
}

fn host_of(url: &str) -> &str {
    url.split("://").nth(1).unwrap_or(url).split('/').next().unwrap_or(url)
}

pub fn sha256_hex(b: &[u8]) -> String {
    crate::pkg::hex(&Sha256::digest(b))
}

/// Where a catalog's files come from.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Origin {
    Builtin,
    Folder(PathBuf),
    /// Base addresses (the catalog's own folder, then its mirrors) and whether this came from the cache.
    Remote { bases: Vec<String>, cached_at: Option<i64> },
}

impl Origin {
    /// Fetch the package bytes of `e`, once, and check them against the catalog's pin. Not for built-in items.
    pub fn fetch_package(&self, e: &Entry, fetch: &dyn Fetch) -> Result<Vec<u8>, String> {
        let max = crate::pkg::max_bytes(&e.kind) + 1024;
        let bytes = match self {
            Origin::Builtin => return Err("a built-in item has no file to fetch".into()),
            Origin::Folder(dir) => {
                if !url_safe(&e.url) || e.url.contains("://") {
                    return Err("the package address is not allowed in a folder catalog".into());
                }
                let p = dir.join(&e.url);
                let meta = std::fs::metadata(&p).map_err(|_| format!("{} is not in the folder", e.url))?;
                if !meta.is_file() || meta.len() as usize > max {
                    return Err(format!("{} is not a package file of a usable size", e.url));
                }
                std::fs::read(&p).map_err(|er| er.to_string())?
            }
            Origin::Remote { bases, .. } => {
                let mut last = "no address to try".to_string();
                let mut got = None;
                let urls: Vec<String> = if e.url.starts_with("http://") || e.url.starts_with("https://") { vec![e.url.clone()] } else { bases.iter().map(|b| format!("{}/{}", b.trim_end_matches('/'), e.url)).collect() };
                for u in urls {
                    match fetch.get(&u, max) {
                        Ok(b) => {
                            got = Some(b);
                            break;
                        }
                        Err(er) => last = er,
                    }
                }
                got.ok_or(last)?
            }
        };
        if bytes.len() as u64 != e.size {
            return Err(format!("the package is {} bytes but the catalog says {}", bytes.len(), e.size));
        }
        if sha256_hex(&bytes) != e.sha256 {
            return Err("the package does not match the hash in the catalog (it was changed or damaged)".into());
        }
        Ok(bytes)
    }
}

/// One catalog as loaded, with where it came from.
#[derive(Clone, Debug)]
pub struct Loaded {
    pub source: String,
    pub name: String,
    pub catalog: Catalog,
    pub check: CatalogCheck,
    pub origin: Origin,
}

/// Read `catalog.json` from a folder or a stick.
pub fn read_folder(source: &str, dir: &Path, roots: &Roots, own: &[(String, String)], now: i64) -> Result<Loaded, String> {
    let p = dir.join("catalog.json");
    let meta = std::fs::metadata(&p).map_err(|_| format!("there is no catalog.json in {}", dir.display()))?;
    if meta.len() as usize > MAX_CATALOG_BYTES {
        return Err(CatalogError::TooLarge.to_string());
    }
    let text = std::fs::read_to_string(&p).map_err(|e| e.to_string())?;
    let (catalog, check) = Catalog::parse(&text, roots, own, now, false).map_err(|e| e.to_string())?;
    Ok(Loaded { source: source.into(), name: catalog.name.clone(), catalog, check, origin: Origin::Folder(dir.to_path_buf()) })
}

/// Fetch a catalog over the network, verify it, and cache the verified bytes. Falls back to the mirrors the cached copy
/// lists, then to the cache itself (with its age) when there is no connection.
#[allow(clippy::too_many_arguments)]
pub fn fetch_remote(source: &str, url: &str, fetch: &dyn Fetch, store_dir: &Path, roots: &Roots, own: &[(String, String)], now: i64) -> Result<Loaded, String> {
    fetch_remote_pinned(source, url, "", fetch, store_dir, roots, own, now)
}

/// Like [`fetch_remote`]; with a non-empty `pin` only a catalog signed by exactly that key is accepted (nothing else is cached
/// and its serial is not noted), and the catalog counts as signed by a key the person trusts.
#[allow(clippy::too_many_arguments)]
pub fn fetch_remote_pinned(source: &str, url: &str, pin: &str, fetch: &dyn Fetch, store_dir: &Path, roots: &Roots, own: &[(String, String)], now: i64) -> Result<Loaded, String> {
    let cache = store_dir.join("cache").join(format!("{source}.json"));
    let base_of = |u: &str| u.rsplit_once('/').map_or(u.to_string(), |(b, _)| b.to_string());
    let cached = std::fs::read_to_string(&cache).ok().and_then(|t| Catalog::parse(&t, roots, own, now, true).ok()).filter(|(c, _)| pin.is_empty() || c.key == pin);
    let mut urls = vec![url.to_string()];
    if let Some((c, _)) = &cached {
        urls.extend(c.mirrors.iter().map(|m| format!("{}/catalog.json", m.trim_end_matches('/'))));
    }
    let mut seen = Seen::load(store_dir);
    let mut last = String::new();
    for u in &urls {
        let bytes = match fetch.get(u, MAX_CATALOG_BYTES) {
            Ok(b) => b,
            Err(e) => {
                if last.is_empty() {
                    last = e;
                }
                continue;
            }
        };
        let text = String::from_utf8_lossy(&bytes).into_owned();
        let (catalog, check) = match Catalog::parse(&text, roots, own, now, true) {
            Ok(x) => x,
            Err(e) => {
                if last.is_empty() {
                    last = e.to_string();
                }
                continue;
            }
        };
        if !pin.is_empty() && catalog.key != pin {
            if last.is_empty() {
                last = PIN_MISMATCH.to_string();
            }
            continue;
        }
        if let Err(e) = seen.note(store_dir, &catalog.id, catalog.serial) {
            if last.is_empty() {
                last = e.to_string();
            }
            continue;
        }
        let _ = std::fs::create_dir_all(cache.parent().unwrap_or(store_dir));
        let _ = std::fs::write(&cache, &text);
        let mut bases = vec![base_of(u)];
        bases.extend(catalog.mirrors.iter().cloned());
        return Ok(Loaded { source: source.into(), name: catalog.name.clone(), catalog, check, origin: Origin::Remote { bases, cached_at: None } });
    }
    match cached {
        Some((catalog, mut check)) => {
            let at = std::fs::metadata(&cache).ok().and_then(|m| m.modified().ok()).and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok()).map_or(now, |d| d.as_secs() as i64);
            check.warnings.push(format!("Could not refresh this catalog ({last}); showing the copy from {}.", super::trust::date(at)));
            let mut bases = vec![base_of(url)];
            bases.extend(catalog.mirrors.iter().cloned());
            Ok(Loaded { source: source.into(), name: catalog.name.clone(), catalog, check, origin: Origin::Remote { bases, cached_at: Some(at) } })
        }
        None => Err(last),
    }
}

/// The cached copy of a remote catalog, checked again now (offline use).
pub fn load_cached(source: &str, url: &str, store_dir: &Path, roots: &Roots, own: &[(String, String)], now: i64) -> Option<Loaded> {
    load_cached_pinned(source, url, "", store_dir, roots, own, now)
}

#[allow(clippy::too_many_arguments)]
pub fn load_cached_pinned(source: &str, url: &str, pin: &str, store_dir: &Path, roots: &Roots, own: &[(String, String)], now: i64) -> Option<Loaded> {
    let cache = store_dir.join("cache").join(format!("{source}.json"));
    let text = std::fs::read_to_string(&cache).ok()?;
    let (catalog, mut check) = Catalog::parse(&text, roots, own, now, true).ok()?;
    if !pin.is_empty() && catalog.key != pin {
        return None;
    }
    let at = std::fs::metadata(&cache).ok().and_then(|m| m.modified().ok()).and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok()).map_or(now, |d| d.as_secs() as i64);
    check.warnings.push(format!("Showing the copy of this catalog from {}; use Check for updates when you are online.", super::trust::date(at)));
    let mut bases = vec![url.rsplit_once('/').map_or(url.to_string(), |(b, _)| b.to_string())];
    bases.extend(catalog.mirrors.iter().cloned());
    Some(Loaded { source: source.into(), name: catalog.name.clone(), catalog, check, origin: Origin::Remote { bases, cached_at: Some(at) } })
}

/// Folders on mounted sticks that hold a store catalog (`<mount>/catalog.json` or `<mount>/iyakki-store/catalog.json`).
pub const PIN_MISMATCH: &str = "this catalog is signed by a different key than the one you confirmed when you added it: it is refused (remove it and add it again only if you expected the key to change)";

/// A catalog from a pinned source counts as signed by a key the person trusts, not by an unknown one.
pub fn apply_pin(mut l: Loaded, pin: &str, name: &str) -> Loaded {
    if !pin.is_empty() && l.catalog.key == pin {
        l.check.signer = CatalogSigner::Trusted(format!("the pinned key of {name}"));
        l.check.warnings.retain(|w| !w.contains("nobody vouches"));
    }
    l
}

/// The packages a private store withdrew (`yanked.json`, signed by the pinned catalog key): `(name, version, sha256)`.
pub fn read_yanked(text: &str, pin: &str) -> Result<Vec<(String, String, String)>, String> {
    let v: Value = serde_json::from_str(text).map_err(|e| e.to_string())?;
    let (key, sig) = (v["key"].as_str().unwrap_or(""), v["sig"].as_str().unwrap_or(""));
    if pin.is_empty() || key != pin || !verify_doc("yanked", &v, key, sig) {
        return Err("the list of withdrawn packages is not signed by the pinned key".into());
    }
    Ok(v["packages"].as_array().into_iter().flatten().map(|p| (p["name"].as_str().unwrap_or("").to_string(), p["version"].as_str().unwrap_or("").to_string(), p["sha256"].as_str().unwrap_or("").to_string())).collect())
}

/// Drop entries the store withdrew.
pub fn drop_yanked(l: &mut Loaded, yanked: &[(String, String, String)]) {
    let n = l.catalog.entries.len();
    l.catalog.entries.retain(|e| !yanked.iter().any(|(name, ver, sha)| *name == e.name && *ver == e.version && *sha == e.sha256));
    if l.catalog.entries.len() < n {
        l.check.warnings.push(format!("{} package(s) were withdrawn by the store and are hidden.", n - l.catalog.entries.len()));
    }
}

pub fn discover_sticks(roots_to_scan: &[&Path]) -> Vec<PathBuf> {
    let mut out = Vec::new();
    for base in roots_to_scan {
        let mut stack = vec![(base.to_path_buf(), 0)];
        while let Some((d, depth)) = stack.pop() {
            for cand in [d.clone(), d.join("iyakki-store")] {
                if cand.join("catalog.json").is_file() && !out.contains(&cand) {
                    out.push(cand);
                }
            }
            if depth < 2 {
                if let Ok(rd) = std::fs::read_dir(&d) {
                    for e in rd.flatten().take(64) {
                        if e.file_type().is_ok_and(|t| t.is_dir()) && e.file_name() != "iyakki-store" {
                            stack.push((e.path(), depth + 1));
                        }
                    }
                }
            }
        }
    }
    out.sort();
    out
}

/// The places sticks are mounted on a typical system.
pub fn stick_mounts() -> Vec<PathBuf> {
    ["/mnt", "/media", "/run/media"].iter().map(PathBuf::from).filter(|p| p.is_dir()).collect()
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::pkg::new_signing_key;
    use std::collections::HashMap;
    use std::sync::Mutex;

    pub const NOW: i64 = 1_800_000_000;

    #[derive(Default)]
    pub struct FakeFetch {
        pub files: Mutex<HashMap<String, Vec<u8>>>,
        pub asked: Mutex<Vec<String>>,
    }

    impl FakeFetch {
        pub fn put(&self, url: &str, b: &[u8]) {
            self.files.lock().unwrap().insert(url.to_string(), b.to_vec());
        }
        pub fn drop_all(&self) {
            self.files.lock().unwrap().clear();
        }
    }

    impl Fetch for FakeFetch {
        fn get(&self, url: &str, max: usize) -> Result<Vec<u8>, String> {
            self.asked.lock().unwrap().push(url.to_string());
            let b = self.files.lock().unwrap().get(url).cloned().ok_or_else(|| format!("could not reach {}: offline", host_of(url)))?;
            if b.len() > max {
                return Err("too large".into());
            }
            Ok(b)
        }
    }

    pub fn entry(name: &str, bytes: &[u8], url: &str) -> Entry {
        Entry { id: name.into(), kind: "skill".into(), name: name.into(), version: "1.0.0".into(), license: "MIT".into(), size: bytes.len() as u64, sha256: sha256_hex(bytes), url: url.into(), meta: Meta { title: name.into(), ..Default::default() }, ..Default::default() }
    }

    pub fn catalog(id: &str, serial: u64, entries: Vec<Entry>) -> Catalog {
        Catalog { v: 1, id: id.into(), name: "Test catalog".into(), serial, issued: NOW, expires: 0, entries, ..Default::default() }
    }

    fn tmp(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("iyakki-cat-{}-{}", std::process::id(), tag));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn a_signed_catalog_parses_and_says_who_signed_it() {
        let (root, own, cert_pub, stranger) = (new_signing_key().unwrap(), new_signing_key().unwrap(), new_signing_key().unwrap(), new_signing_key().unwrap());
        let roots = Roots::with("r1", &root);
        let own_keys = vec![(public_key_hex(&own), "IT dept".to_string())];
        let sign_with = |k: &ed25519_dalek::SigningKey, cert: Option<Cert>| {
            let mut c = catalog("main", 1, vec![entry("alpha", b"x", "alpha.iyp")]);
            c.cert = cert;
            c.sign(k);
            c.to_json()
        };
        let parse = |t: &str, req| Catalog::parse(t, &roots, &own_keys, NOW, req);
        assert_eq!(parse(&sign_with(&root, None), true).unwrap().1.signer, CatalogSigner::Official("r1".into()));
        assert_eq!(parse(&sign_with(&own, None), true).unwrap().1.signer, CatalogSigner::Trusted("IT dept".into()));
        let publisher = super::super::manifest::Publisher { id: "acme".into(), name: "Acme".into(), url: String::new() };
        let cert = Cert::issue(&root, publisher, &public_key_hex(&cert_pub), NOW - 5, NOW + 5, &["catalog"]);
        assert_eq!(parse(&sign_with(&cert_pub, Some(cert)), true).unwrap().1.signer, CatalogSigner::Verified("Acme".into()));
        let (_, check) = parse(&sign_with(&stranger, None), true).unwrap();
        assert!(matches!(check.signer, CatalogSigner::Unknown(_)) && check.warnings[0].contains("nobody vouches"));
        // unsigned: refused from the network, allowed from a folder with a warning
        let mut unsigned = catalog("main", 1, vec![]);
        unsigned.sig.clear();
        assert_eq!(parse(&unsigned.to_json(), true).unwrap_err(), CatalogError::Unsigned);
        let (_, check) = parse(&unsigned.to_json(), false).unwrap();
        assert_eq!(check.signer, CatalogSigner::Unsigned);
        assert!(check.warnings[0].contains("not signed"));
        // changed after signing
        let t = sign_with(&root, None).replace("alpha", "bravo");
        assert_eq!(parse(&t, true).unwrap_err(), CatalogError::BadSignature);
        assert!(matches!(parse("{", true), Err(CatalogError::Malformed(_))));
        assert_eq!(parse(&"x".repeat(MAX_CATALOG_BYTES + 1), true).unwrap_err(), CatalogError::TooLarge);
        assert!(CatalogError::BadSignature.to_string().contains("bad signature"));
    }

    #[test]
    fn bad_entries_are_dropped_with_a_note_and_an_expired_catalog_can_only_be_browsed() {
        let root = new_signing_key().unwrap();
        let roots = Roots::with("r", &root);
        let good = entry("good", b"x", "good.iyp");
        let mut climbing = entry("climb", b"x", "../../etc/passwd");
        climbing.id = "climb".into();
        let mut nohash = entry("nohash", b"x", "n.iyp");
        nohash.sha256 = "zz".into();
        let mut twice = good.clone();
        twice.url = "other.iyp".into();
        let mut kind = entry("odd", b"x", "o.iyp");
        kind.kind = "virus".into();
        let mut c = catalog("main", 1, vec![good, climbing, nohash, twice, kind]);
        c.expires = NOW - 100;
        c.sign(&root);
        let (c, check) = Catalog::parse(&c.to_json(), &roots, &[], NOW, true).unwrap();
        assert_eq!(c.entries.len(), 1);
        assert!(check.warnings.iter().any(|w| w.contains("4 entries were ignored")), "{:?}", check.warnings);
        assert!(check.expired && check.warnings.iter().any(|w| w.contains("nothing can be installed")));
        assert!(entry_problem(&c.entries[0]).is_none());
    }

    #[test]
    fn serials_only_go_up() {
        let d = tmp("seen");
        let mut s = Seen::load(&d);
        s.note(&d, "main", 4).unwrap();
        s.note(&d, "main", 4).unwrap();
        assert_eq!(s.note(&d, "main", 3), Err(CatalogError::Rollback { have: 4, got: 3 }));
        assert_eq!(Seen::load(&d).note(&d, "main", 2), Err(CatalogError::Rollback { have: 4, got: 2 }), "remembered across restarts");
        s.note(&d, "other", 1).unwrap();
        let _ = std::fs::remove_dir_all(d);
    }

    #[test]
    fn a_remote_catalog_is_fetched_verified_cached_and_used_offline_with_its_age() {
        let d = tmp("remote");
        let root = new_signing_key().unwrap();
        let roots = Roots::with("r", &root);
        let pkg = b"package bytes";
        let mut c = catalog("main", 5, vec![entry("alpha", pkg, "packages/alpha.iyp")]);
        c.mirrors = vec!["https://mirror.example/store".into()];
        c.sign(&root);
        let f = FakeFetch::default();
        f.put("https://cat.example/store/catalog.json", c.to_json().as_bytes());
        f.put("https://cat.example/store/packages/alpha.iyp", pkg);
        let l = fetch_remote("src1", "https://cat.example/store/catalog.json", &f, &d, &roots, &[], NOW).unwrap();
        assert_eq!(l.catalog.serial, 5);
        assert_eq!(l.check.signer, CatalogSigner::Official("r".into()));
        let e = l.catalog.find("alpha").unwrap().clone();
        assert_eq!(l.origin.fetch_package(&e, &f).unwrap(), pkg);
        // the hash pin catches a swapped package
        f.put("https://cat.example/store/packages/alpha.iyp", b"package bytez");
        assert!(l.origin.fetch_package(&e, &f).unwrap_err().contains("does not match the hash"));
        // the primary is down: the mirror from the cached catalog serves it
        f.drop_all();
        f.put("https://mirror.example/store/catalog.json", c.to_json().as_bytes());
        let l2 = fetch_remote("src1", "https://cat.example/store/catalog.json", &f, &d, &roots, &[], NOW).unwrap();
        assert!(f.asked.lock().unwrap().iter().any(|u| u.starts_with("https://mirror.example")));
        assert!(l2.origin == Origin::Remote { bases: vec!["https://mirror.example/store".into(), "https://mirror.example/store".into()], cached_at: None });
        // everything is down: the cache is shown, with a note
        f.drop_all();
        let l3 = fetch_remote("src1", "https://cat.example/store/catalog.json", &f, &d, &roots, &[], NOW).unwrap();
        assert!(matches!(l3.origin, Origin::Remote { cached_at: Some(_), .. }));
        assert!(l3.check.warnings.iter().any(|w| w.contains("Could not refresh")));
        // an older catalog from a stale mirror is refused, and the cache stands in
        let mut old = catalog("main", 4, vec![]);
        old.sign(&root);
        f.put("https://cat.example/store/catalog.json", old.to_json().as_bytes());
        let l4 = fetch_remote("src1", "https://cat.example/store/catalog.json", &f, &d, &roots, &[], NOW).unwrap();
        assert_eq!(l4.catalog.serial, 5);
        assert!(l4.check.warnings.iter().any(|w| w.contains("older than the one already seen")));
        // no cache and no network
        assert!(fetch_remote("src2", "https://nowhere.example/catalog.json", &f, &d, &roots, &[], NOW).unwrap_err().contains("offline"));
        let _ = std::fs::remove_dir_all(d);
    }

    #[test]
    fn a_folder_or_stick_catalog_works_without_a_network_and_cannot_climb_out() {
        let d = tmp("stick");
        std::fs::create_dir_all(d.join("stick/iyakki-store/packages")).unwrap();
        let dir = d.join("stick/iyakki-store");
        let pkg = b"offline package";
        std::fs::write(dir.join("packages/a.iyp"), pkg).unwrap();
        let c = catalog("usb", 1, vec![entry("usbapp", pkg, "packages/a.iyp")]);
        std::fs::write(dir.join("catalog.json"), c.to_json()).unwrap();
        assert_eq!(discover_sticks(&[&d.join("stick")]), vec![dir.clone()]);
        let l = read_folder("usb", &dir, &Roots::default(), &[], NOW).unwrap();
        assert_eq!(l.check.signer, CatalogSigner::Unsigned);
        let e = l.catalog.find("usbapp").unwrap().clone();
        assert_eq!(l.origin.fetch_package(&e, &FakeFetch::default()).unwrap(), pkg);
        let mut bad = e.clone();
        bad.url = "../../../etc/passwd".into();
        assert!(l.origin.fetch_package(&bad, &FakeFetch::default()).is_err());
        bad.url = "https://evil.example/x.iyp".into();
        assert!(l.origin.fetch_package(&bad, &FakeFetch::default()).is_err(), "a folder catalog only reads its own folder");
        std::fs::write(dir.join("packages/a.iyp"), b"offline packagE").unwrap();
        assert!(l.origin.fetch_package(&e, &FakeFetch::default()).unwrap_err().contains("hash"));
        assert!(read_folder("x", &d.join("empty"), &Roots::default(), &[], NOW).unwrap_err().contains("no catalog.json"));
        let _ = std::fs::remove_dir_all(d);
    }

    #[test]
    fn sources_are_configured_checked_and_kept() {
        let d = tmp("cfg");
        let mut c = Config::load(&d);
        assert!(c.sources.is_empty() && !c.auto_check, "the daily check is off by default");
        assert!(c.add("x", SourceKind::Url, "ftp://x").is_err() && c.add("x", SourceKind::Folder, "relative").is_err() && c.add("x", SourceKind::Folder, "/mnt/../etc").is_err());
        let id = c.add("", SourceKind::Url, "https://cat.example/store/catalog.json").unwrap();
        assert_eq!(c.sources[0].name, "catalog.json");
        assert!(c.add("dup", SourceKind::Url, "https://cat.example/store/catalog.json").is_err());
        c.add("Stick", SourceKind::Folder, "/mnt/usb").unwrap();
        c.save(&d).unwrap();
        assert_eq!(Config::load(&d), c);
        assert_eq!(id, "src1");
        let _ = std::fs::remove_dir_all(d);
    }
}
