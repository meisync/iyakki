//! The app store. Logic lives here (headless and tested); the STORE tile in `iyakki-shell` only draws it.
//!
//! * [`manifest`]: what an app page says (metadata, pricing, permissions in plain words)
//! * [`trust`]: ed25519 trust levels, roots, publisher certificates, look-alike names
//! * [`revocation`]: the signed revocation list, never rolled back
//! * [`license`]: signed licence tokens for paid apps, checked offline
//! * [`catalog`]: signed catalogs, local and remote sources, caching
//! * [`builtin`]: the catalog compiled into the image
//! * [`install`]: review and install through the existing skill, tool and MCP stores
//! * [`tools`]: the assistant's `store_search`, `store_info`, `store_install`
//! * [`publish`]: `iyakki pkg ...` for people who make packages and catalogs
//!
//! See docs/STORE.md for the trust model, the threat model and the payment design.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use iyakki_sec::policy::Policy;

pub mod author;
pub mod authtools;
pub mod builtin;
pub mod catalog;
pub mod compcli;
pub mod compmanifest;
pub mod compapply;
pub mod compinstalled;
pub mod components;
pub mod drivers;
pub mod install;
pub mod keys;
pub mod installed;
pub mod license;
pub mod lifecycle;
pub mod manifest;
pub mod organize;
pub mod privatemanage;
pub mod privatepub;
pub mod privatestore;
pub mod publishflow;
pub mod ratings;
pub mod publish;
pub mod revocation;
pub mod tile;
pub mod tilehub;
pub mod tilesdk;
pub mod server;
pub mod server_http;
pub mod storecmd;
pub mod tools;
pub mod trust;
pub mod vet;

use catalog::{Config, Entry, Fetch, Loaded, Origin, SourceKind};
use install::{Actor, Review};
use manifest::PriceModel;
use revocation::Revocations;
use trust::{Judge, Level, Roots, Verdict};

use crate::pkg::{self, newer};
use crate::services::Services;
use crate::skills::Scope;

/// What a private store's catalog says about itself, shown before it is trusted.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Probe {
    pub name: String,
    pub id: String,
    pub key: String,
    pub fingerprint: String,
    pub serial: u64,
    pub entries: usize,
    pub url: String,
    /// Who signed it and how far that is trusted, in words (shown before the store is enabled).
    pub trust: String,
}

/// Where the store keeps its files and which root keys and clock it uses.
#[derive(Clone)]
pub struct Store {
    dir: PathBuf,
    roots: Roots,
    clock: Arc<dyn Fn() -> i64 + Send + Sync>,
    /// How long an updated MCP server has to start and answer (tests make it short).
    pub(crate) check_timeout_ms: u64,
}

impl Store {
    /// The store of the system whose data folder is `data_dir` (files go in `<data_dir>/store`).
    pub fn open(data_dir: &Path) -> Store {
        Store { dir: data_dir.join("store"), roots: Roots::pinned(), clock: Arc::new(crate::clock::now_unix), check_timeout_ms: lifecycle::MCP_CHECK_SECS * 1000 }
    }

    pub fn for_services(svc: &Services) -> Store {
        Store::open(svc.dir())
    }

    pub fn with_roots(mut self, roots: Roots) -> Store {
        self.roots = roots;
        self
    }

    pub fn with_check_timeout(mut self, ms: u64) -> Store {
        self.check_timeout_ms = ms;
        self
    }

    pub fn with_clock(mut self, clock: Arc<dyn Fn() -> i64 + Send + Sync>) -> Store {
        self.clock = clock;
        self
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    /// The data folder this store lives in (`dir` is `<data>/store`); driver packs keep their files in `<data>/firmware`.
    pub fn data_dir(&self) -> &Path {
        self.dir.parent().unwrap_or(&self.dir)
    }

    pub fn now(&self) -> i64 {
        (self.clock)()
    }

    pub fn roots(&self) -> &Roots {
        &self.roots
    }

    pub fn revocations(&self) -> Revocations {
        Revocations::load(&Revocations::path(&self.dir))
    }

    /// Take a signed revocation list if it is genuine and not older than the one in force. True when it changed anything.
    pub fn accept_revocations(&self, text: &str) -> Result<bool, String> {
        let mut r = self.revocations();
        let changed = r.accept(text, &self.roots, self.now()).map_err(|e| e.to_string())?;
        if changed {
            r.save(&Revocations::path(&self.dir))?;
        }
        Ok(changed)
    }

    pub fn judge<'a>(&'a self, own: &'a [(String, String)], revs: &'a Revocations) -> Judge<'a> {
        Judge { roots: &self.roots, own_keys: own, revocations: revs, now: self.now() }
    }

    pub fn config(&self) -> Config {
        Config::load(&self.dir)
    }

    // ---------------------------------------------------------------------------------------- settings (administrators)

    fn need_admin(actor: &Actor) -> Result<(), String> {
        if actor.allowed && actor.admin { Ok(()) } else { Err("Only administrators can change the store's settings.".into()) }
    }

    pub fn add_source(&self, actor: &Actor, name: &str, kind: SourceKind, location: &str) -> Result<String, String> {
        Self::need_admin(actor)?;
        let mut c = self.config();
        let id = c.add(name, kind, location)?;
        c.save(&self.dir)?;
        Ok(id)
    }

    /// The address of a private store's `catalog.json` from what the person typed (the base address works too).
    /// A portal link works as it is: an address ending in `.json` is the catalog, any other (`https://portal.example/store/`)
    /// is a page whose catalog is `<address>/catalog.json`.
    pub fn catalog_url(typed: &str) -> String {
        let t = typed.trim().trim_end_matches('/');
        if t.ends_with(".json") { t.to_string() } else { format!("{t}/catalog.json") }
    }

    /// The official store's address when the project has published one (see [`catalog::OFFICIAL_STORE_URL`]).
    pub fn official_store_url() -> Option<&'static str> {
        (!catalog::OFFICIAL_STORE_URL.is_empty()).then_some(catalog::OFFICIAL_STORE_URL)
    }

    /// First step of adding a private store: fetch its catalog and say who signed it, so the person can compare the fingerprint
    /// with the one the store's administrator gave them. Nothing is saved.
    pub fn probe_private(&self, fetch: &dyn Fetch, typed: &str) -> Result<Probe, String> {
        let url = Self::catalog_url(typed);
        if !(url.starts_with("https://") || url.starts_with("http://")) || url.chars().any(|c| c.is_control() || c == ' ') {
            return Err("a store address starts with https:// (the portal link, or the address of its catalog.json)".into());
        }
        let bytes = fetch.get(&url, catalog::MAX_CATALOG_BYTES)?;
        let (c, check) = catalog::Catalog::parse(&String::from_utf8_lossy(&bytes), &self.roots, &[], self.now(), true).map_err(|e| e.to_string())?;
        let mut trust = match &check.signer {
            catalog::CatalogSigner::Official(_) => format!("Official: {}", check.signer.label()),
            catalog::CatalogSigner::Verified(_) => format!("Verified: {}", check.signer.label()),
            _ => format!("Not vouched for by IYAKKI: {}. Add it only if you know who runs it.", check.signer.label()),
        };
        if url.starts_with("http://") {
            trust.push_str(" The address is not encrypted (http).");
        }
        Ok(Probe { fingerprint: server::fingerprint(&c.key), key: c.key.clone(), name: c.name.clone(), id: c.id.clone(), serial: c.serial, entries: c.entries.len(), url, trust })
    }

    /// Second step: add the catalog with its key pinned. `confirmed` is the fingerprint the person compared and accepted; it
    /// must match what the catalog shows now (a store whose key changed between the two steps is refused).
    pub fn add_private_source(&self, actor: &Actor, fetch: &dyn Fetch, name: &str, typed: &str, confirmed: &str) -> Result<String, String> {
        Self::need_admin(actor)?;
        let p = self.probe_private(fetch, typed)?;
        self.pin_probed(actor, name, &p, confirmed)
    }

    /// Add what [`Store::probe_private`] found (no network): `confirmed` must be its fingerprint.
    pub fn pin_probed(&self, actor: &Actor, name: &str, p: &Probe, confirmed: &str) -> Result<String, String> {
        Self::need_admin(actor)?;
        let norm = |s: &str| s.chars().filter(|c| !c.is_whitespace()).collect::<String>().to_ascii_lowercase();
        if norm(confirmed) != norm(&p.fingerprint) {
            return Err("the fingerprint does not match this store's key: nothing was added".into());
        }
        let mut c = self.config();
        let id = c.add_pinned(if name.trim().is_empty() { &p.name } else { name }, &p.url, &p.key)?;
        c.save(&self.dir)?;
        Ok(id)
    }

    /// Where publishing goes by default (a source id; empty clears it).
    pub fn set_publish_target(&self, actor: &Actor, id: &str) -> Result<(), String> {
        Self::need_admin(actor)?;
        let mut c = self.config();
        if !id.is_empty() && !c.sources.iter().any(|s| s.id == id) {
            return Err("There is no such catalog.".into());
        }
        c.publish_target = id.to_string();
        c.save(&self.dir)
    }

    pub fn remove_source(&self, actor: &Actor, id: &str) -> Result<(), String> {
        Self::need_admin(actor)?;
        let mut c = self.config();
        let n = c.sources.len();
        c.sources.retain(|s| s.id != id);
        if c.sources.len() == n {
            return Err("There is no such catalog.".into());
        }
        c.save(&self.dir)?;
        let _ = std::fs::remove_file(self.dir.join("cache").join(format!("{id}.json")));
        Ok(())
    }

    pub fn toggle_source(&self, actor: &Actor, id: &str) -> Result<bool, String> {
        Self::need_admin(actor)?;
        let mut c = self.config();
        let s = c.sources.iter_mut().find(|s| s.id == id).ok_or("There is no such catalog.")?;
        s.enabled = !s.enabled;
        let on = s.enabled;
        c.save(&self.dir)?;
        Ok(on)
    }

    pub fn set_auto_check(&self, actor: &Actor, on: bool) -> Result<(), String> {
        Self::need_admin(actor)?;
        let mut c = self.config();
        c.auto_check = on;
        c.save(&self.dir)
    }

    pub fn set_catalog_enabled(&self, actor: &Actor, on: bool) -> Result<(), String> {
        Self::need_admin(actor)?;
        let mut c = self.config();
        c.catalog_enabled = on;
        c.save(&self.dir)
    }

    /// Whether Discover and Publishers are shown.
    pub fn catalog_visible(&self) -> bool {
        self.config().catalog_visible()
    }

    /// Whether the optional daily look for news is switched on and due. Never true by default.
    pub fn auto_check_due(&self) -> bool {
        let c = self.config();
        c.auto_check && self.now() - c.last_check > 86_400
    }

    pub fn trust_key(&self, svc: &Services, actor: &Actor, key: &str, label: &str) -> Result<(), String> {
        Self::need_admin(actor)?;
        svc.packages.trust_key(key, label).map_err(|e| e.to_string())
    }

    // ---------------------------------------------------------------------------------------- catalogs

    fn own_keys(svc: &Services) -> Vec<(String, String)> {
        svc.packages.trusted_keys()
    }

    /// Everything that can be read without a network: the built-in catalog, cached copies of remote ones, folders and sticks.
    pub fn local_catalogs(&self, svc: &Services) -> Vec<Loaded> {
        let own = Self::own_keys(svc);
        let now = self.now();
        let b = builtin::catalog();
        let mut out = vec![Loaded { source: "builtin".into(), name: b.name.clone(), check: catalog::CatalogCheck { signer: catalog::CatalogSigner::Builtin, warnings: vec![], expired: false }, catalog: b, origin: Origin::Builtin }];
        let cfg = self.config();
        if !cfg.catalog_visible() {
            // the made-up example apps only exist to show a storefront: no storefront, no examples
            out[0].catalog.entries.retain(|e| !e.demo);
        }
        for s in cfg.sources.iter().filter(|s| s.enabled) {
            match s.kind {
                SourceKind::Url => out.extend(catalog::load_cached_pinned(&s.id, &s.location, &s.pin, &self.dir, &self.roots, &own, now).map(|l| self.pinned_view(s, l))),
                SourceKind::Folder => {
                    let dir = PathBuf::from(&s.location);
                    if let Ok(l) = catalog::read_folder(&s.id, &dir, &self.roots, &own, now) {
                        if let Ok(t) = std::fs::read_to_string(dir.join("revocations.json")) {
                            let _ = self.accept_revocations(&t);
                        }
                        out.push(Loaded { name: if s.name.is_empty() { l.name.clone() } else { s.name.clone() }, ..l });
                    }
                }
            }
        }
        out
    }

    /// A cached private catalog with its pin applied and the withdrawn packages hidden.
    fn pinned_view(&self, s: &catalog::SourceCfg, l: Loaded) -> Loaded {
        let mut l = catalog::apply_pin(l, &s.pin, &s.name);
        if !s.pin.is_empty() {
            if let Ok(y) = std::fs::read_to_string(self.dir.join("cache").join(format!("{}.yanked.json", s.id))).map_err(|e| e.to_string()).and_then(|t| catalog::read_yanked(&t, &s.pin)) {
                catalog::drop_yanked(&mut l, &y);
            }
        }
        l
    }

    /// Did the private store this copy came from withdraw exactly this package (name, version and hash) in its signed
    /// `yanked.json`, as last kept on this device? Yanking hides the package from the catalog; this finds the copies that
    /// are already installed, so they can be marked and offered for removal.
    pub fn withdrawn_by_source(&self, source_id: &str, name: &str, version: &str, sha256: &str) -> Option<String> {
        let cfg = self.config();
        let s = cfg.sources.iter().find(|s| s.id == source_id && !s.pin.is_empty())?;
        let text = std::fs::read_to_string(self.dir.join("cache").join(format!("{}.yanked.json", s.id))).ok()?;
        let y = catalog::read_yanked(&text, &s.pin).ok()?;
        y.iter().any(|(n, v, h)| n == name && v == version && (h == sha256 || sha256.is_empty())).then(|| s.name.clone())
    }

    /// After a fetch of a pinned catalog: keep its signed list of withdrawn packages beside the cache.
    fn pinned_refresh(&self, s: &catalog::SourceCfg, l: Loaded, fetch: &dyn Fetch) -> Loaded {
        if s.pin.is_empty() {
            return l;
        }
        if let Origin::Remote { bases, cached_at: None } = &l.origin {
            if let Ok(b) = fetch.get(&format!("{}/yanked.json", bases[0].trim_end_matches('/')), catalog::MAX_CATALOG_BYTES) {
                let text = String::from_utf8_lossy(&b).into_owned();
                if catalog::read_yanked(&text, &s.pin).is_ok() {
                    let _ = std::fs::write(self.dir.join("cache").join(format!("{}.yanked.json", s.id)), text);
                }
            }
        }
        self.pinned_view(s, l)
    }

    /// Look at the network (and the folders) for news: the catalogs, and a revocation list beside each. Slow: call it off
    /// the UI thread. Failures are reported per source and never lose what was cached.
    pub fn refresh(&self, svc: &Services, fetch: &dyn Fetch) -> Refresh {
        let own = Self::own_keys(svc);
        let now = self.now();
        let cfg = self.config();
        let mut out = Refresh::default();
        for s in cfg.sources.iter().filter(|s| s.enabled) {
            match s.kind {
                SourceKind::Url => match catalog::fetch_remote_pinned(&s.id, &s.location, &s.pin, fetch, &self.dir, &self.roots, &own, now) {
                    Ok(l) => {
                        let l = self.pinned_refresh(s, l, fetch);
                        if let Origin::Remote { bases, cached_at: None } = &l.origin {
                            if let Ok(b) = fetch.get(&format!("{}/revocations.json", bases[0].trim_end_matches('/')), revocation::MAX_LIST_BYTES) {
                                match self.accept_revocations(&String::from_utf8_lossy(&b)) {
                                    Ok(true) => out.notes.push("A newer revocation list was loaded.".into()),
                                    Ok(false) => {}
                                    Err(e) => out.notes.push(e),
                                }
                            }
                        }
                        out.loaded.push(l);
                    }
                    Err(e) => out.errors.push((s.name.clone(), e)),
                },
                SourceKind::Folder => match catalog::read_folder(&s.id, Path::new(&s.location), &self.roots, &own, now) {
                    Ok(l) => out.loaded.push(l),
                    Err(e) => out.errors.push((s.name.clone(), e)),
                },
            }
        }
        let mut c = self.config();
        c.last_check = now;
        let _ = c.save(&self.dir);
        out.disabled = self.enforce_revocations(svc);
        out
    }

    // ---------------------------------------------------------------------------------------- the view

    fn entry_manifest(e: &Entry) -> pkg::Manifest {
        pkg::Manifest { kind: e.kind.clone(), name: e.name.clone(), version: e.version.clone(), license: e.license.clone(), sha256: e.sha256.clone(), signer: (!e.signer.is_empty()).then(|| e.signer.clone()), meta: None, cert: e.cert.as_ref().map(|c| c.to_json()) }
    }

    /// The trust level shown in lists, before anything is downloaded (from the signer and certificate the catalog copied
    /// into the entry). The review works it out again from the package itself.
    pub fn provisional(&self, svc: &Services, e: &Entry, source: &str) -> Verdict {
        let (own, revs) = (Self::own_keys(svc), self.revocations());
        self.provisional_with(&own, &revs, e, source)
    }

    fn provisional_with(&self, own: &[(String, String)], revs: &Revocations, e: &Entry, source: &str) -> Verdict {
        if source == "builtin" && !e.demo {
            return Verdict::builtin();
        }
        let mut v = self.judge(own, revs).verdict(&Self::entry_manifest(e));
        if e.demo {
            v = Verdict { level: Level::Community, signer: None, publisher: None, label: "a made-up example".into(), chain: vec![], warnings: vec!["DEMO: not a real product.".into()], refusal: None };
        }
        if v.publisher.is_none() && !e.meta.publisher.name.is_empty() {
            v.publisher = Some(e.meta.publisher.clone());
        }
        v
    }

    fn license_ok(&self, name: &str, signer: &str, user: &str) -> bool {
        let ls = self.licenses();
        let Some(t) = ls.get(name) else { return false };
        let issuers = if signer.is_empty() { vec![] } else { vec![signer.to_string()] };
        let revs = self.revocations();
        t.verify(&license::Check { app: name, issuers: &issuers, roots: &self.roots, device: &ls.device_id(), account: user, now: self.now(), revocations: &revs }).is_ok()
    }

    /// All apps of the given catalogs for this person, one row per app (the newest version; the first catalog wins a tie).
    pub fn items(&self, svc: &Services, actor: &Actor, loaded: &[Loaded]) -> Vec<Item> {
        let mut out: Vec<Item> = Vec::new();
        let mine = Scope::User(actor.user.clone());
        let (own, revs) = (Self::own_keys(svc), self.revocations());
        for l in loaded {
            for e in &l.catalog.entries {
                if let Some(pos) = out.iter().position(|i| i.entry.kind == e.kind && i.entry.name == e.name) {
                    if !newer(&e.version, &out[pos].entry.version) {
                        continue;
                    }
                    out.remove(pos);
                }
                let verdict = self.provisional_with(&own, &revs, e, &l.source);
                let installed = [&mine, &Scope::Shared].into_iter().find_map(|s| self.installed_version(svc, &e.kind, &e.name, s));
                let off = [&mine, &Scope::Shared].into_iter().any(|s| self.is_disabled(&e.kind, &e.name, s));
                let cli_here = e.action.as_ref().filter(|a| a.open == "cli").and_then(|a| svc.clis.is_installed(&a.id)).is_some();
                let state = if e.demo {
                    State::Demo
                } else if verdict.level == Level::Revoked {
                    State::Revoked
                } else if let Some(a) = e.action.as_ref().filter(|_| !cli_here) {
                    State::Link(a.open.clone())
                } else if cli_here {
                    State::Installed
                } else if off {
                    State::Disabled
                } else if let Some((v, _)) = &installed {
                    if v != "preinstalled" && newer(&e.version, v) { State::Update(e.version.clone()) } else { State::Installed }
                } else if e.meta.pricing.model.needs_licence() && !self.license_ok(&e.name, &e.signer, &actor.user) {
                    State::Locked
                } else {
                    State::Available
                };
                out.push(Item { entry: e.clone(), source: l.source.clone(), source_name: l.name.clone(), verdict, state, installed_version: installed.as_ref().map(|i| i.0.clone()), preinstalled: installed.as_ref().is_some_and(|i| i.1), installable: !l.check.expired });
            }
        }
        // installed through the store from a catalog that is no longer listed
        for r in self.records().values().filter(|r| r.scope() == Scope::Shared || r.scope() == mine) {
            if out.iter().any(|i| i.entry.kind == r.kind && i.entry.name == r.name) {
                continue;
            }
            let entry = Entry { id: r.name.clone(), kind: r.kind.clone(), name: r.name.clone(), version: r.version.clone(), meta: manifest::Meta { title: r.title.clone(), publisher: manifest::Publisher { id: r.publisher.clone(), name: r.publisher.clone(), url: String::new() }, ..Default::default() }, ..Default::default() };
            let level = Level::ALL.into_iter().find(|l| l.id() == r.level).unwrap_or(Level::Community);
            let verdict = Verdict { level, signer: r.signer.clone(), publisher: None, label: "as installed".into(), chain: vec![], warnings: vec![], refusal: None };
            out.push(Item { entry, source: r.source.clone(), source_name: "installed".into(), verdict, state: if r.disabled { State::Disabled } else { State::Installed }, installed_version: Some(r.version.clone()), preinstalled: r.preinstalled, installable: false });
        }
        out
    }

    /// `(name, publisher id)` of everything known, for the look-alike check.
    pub fn known_names(items: &[Item]) -> Vec<(String, String)> {
        items.iter().map(|i| (i.entry.name.clone(), i.entry.meta.publisher.id.clone())).collect()
    }

    /// Get the package of an item (this may use the network: call it off the UI thread) and review it.
    pub fn review_item(&self, svc: &Services, actor: &Actor, item: &Item, loaded: &[Loaded], fetch: &dyn Fetch, scope: Scope, known: &[(String, String)]) -> Result<Review, String> {
        let e = &item.entry;
        if e.demo {
            return Err("This is a made-up example. It cannot be installed, and nothing is sold here.".into());
        }
        if let Some(a) = &e.action {
            return Err(format!("Open the {} tile to install this one.", a.open.to_uppercase()));
        }
        let l = loaded.iter().find(|l| l.source == item.source).ok_or("That catalog is not loaded any more.")?;
        let (bytes, builtin) = match &l.origin {
            Origin::Builtin => (builtin::package(&e.builtin).ok_or("That built-in item has no package.")?.as_bytes().to_vec(), true),
            origin => (origin.fetch_package(e, fetch)?, false),
        };
        Ok(self.review(svc, actor, bytes, Some(e), &item.source, builtin, scope, l.check.expired, known))
    }

    /// Review a file the person pointed at (a stick, a download). `bytes` were read once by the caller.
    pub fn review_sideload(&self, svc: &Services, actor: &Actor, bytes: Vec<u8>, scope: Scope, known: &[(String, String)]) -> Review {
        self.review(svc, actor, bytes, None, "file", false, scope, false, known)
    }
}

/// What a look at the network found.
#[derive(Default, Debug)]
pub struct Refresh {
    pub loaded: Vec<Loaded>,
    pub errors: Vec<(String, String)>,
    pub notes: Vec<String>,
    /// Apps turned off because they were withdrawn.
    pub disabled: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum State {
    Available,
    Installed,
    Update(String),
    Disabled,
    /// Paid, and no licence on this device.
    Locked,
    /// A made-up example.
    Demo,
    /// Opens another tile to install.
    Link(String),
    Revoked,
}

impl State {
    pub fn label(&self) -> String {
        match self {
            State::Available => "Available".into(),
            State::Installed => "Installed".into(),
            State::Update(v) => format!("Update {v}"),
            State::Disabled => "Turned off".into(),
            State::Locked => "Locked".into(),
            State::Demo => "DEMO".into(),
            State::Link(t) => format!("Opens {}", t.to_uppercase()),
            State::Revoked => "Revoked".into(),
        }
    }

    pub fn is_installed(&self) -> bool {
        matches!(self, State::Installed | State::Update(_) | State::Disabled)
    }
}

#[derive(Clone, Debug)]
pub struct Item {
    pub entry: Entry,
    pub source: String,
    pub source_name: String,
    pub verdict: Verdict,
    pub state: State,
    pub installed_version: Option<String>,
    pub preinstalled: bool,
    /// False when the catalog has expired.
    pub installable: bool,
}

impl Item {
    pub fn title(&self) -> &str {
        self.entry.title()
    }
}

// ------------------------------------------------------------------------------------------------ finding things

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum PriceFilter {
    #[default]
    All,
    Free,
    Paid,
}

#[derive(Clone, Debug, Default)]
pub struct Filter {
    pub query: String,
    pub price: PriceFilter,
    pub level: Option<Level>,
    pub kind: Option<String>,
    pub category: Option<String>,
    pub publisher: Option<String>,
}

fn score(i: &Item, words: &[String]) -> Option<u32> {
    if words.is_empty() {
        return Some(0);
    }
    let e = &i.entry;
    let (title, name, summary) = (e.title().to_lowercase(), e.name.to_lowercase(), e.meta.summary.to_lowercase());
    let tags = e.meta.tags.iter().chain(&e.meta.categories).map(|t| t.to_lowercase()).collect::<Vec<_>>();
    let publisher = e.meta.publisher.name.to_lowercase();
    let mut total = 0;
    for w in words {
        let s = if title == *w || name == *w { 10 } else if title.contains(w.as_str()) || name.contains(w.as_str()) { 6 } else if tags.iter().any(|t| t.contains(w.as_str())) { 4 } else if summary.contains(w.as_str()) || publisher.contains(w.as_str()) || e.kind == *w { 2 } else { return None };
        total += s;
    }
    Some(total)
}

/// The items that pass the filters, best matches first (then featured, then by title).
pub fn filter<'a>(items: &'a [Item], f: &Filter) -> Vec<&'a Item> {
    let words: Vec<String> = f.query.to_lowercase().split_whitespace().map(str::to_string).collect();
    let mut v: Vec<(u32, &Item)> = items
        .iter()
        .filter(|i| match f.price {
            PriceFilter::All => true,
            PriceFilter::Free => !i.entry.meta.pricing.model.needs_licence() && i.entry.meta.pricing.model != PriceModel::Paid,
            PriceFilter::Paid => i.entry.meta.pricing.model.needs_licence(),
        })
        .filter(|i| f.level.is_none_or(|l| i.verdict.level == l))
        .filter(|i| f.kind.as_ref().is_none_or(|k| i.entry.kind == *k))
        .filter(|i| f.category.as_ref().is_none_or(|c| i.entry.meta.categories.iter().any(|x| x == c)))
        .filter(|i| f.publisher.as_ref().is_none_or(|p| i.entry.meta.publisher.id == *p))
        .filter_map(|i| score(i, &words).map(|s| (s, i)))
        .collect();
    v.sort_by(|a, b| b.0.cmp(&a.0).then(b.1.entry.featured.cmp(&a.1.entry.featured)).then(a.1.title().to_lowercase().cmp(&b.1.title().to_lowercase())));
    v.into_iter().map(|(_, i)| i).collect()
}

pub fn featured(items: &[Item]) -> Vec<&Item> {
    items.iter().filter(|i| i.entry.featured && !i.state.is_installed()).collect()
}

/// Newest first, at most `n`.
pub fn newest(items: &[Item], n: usize) -> Vec<&Item> {
    let mut v: Vec<&Item> = items.iter().filter(|i| i.entry.added > 0 && !i.entry.demo).collect();
    v.sort_by(|a, b| b.entry.added.cmp(&a.entry.added).then(a.title().cmp(b.title())));
    v.truncate(n);
    v
}

pub fn categories(items: &[Item]) -> Vec<(String, usize)> {
    let mut m = std::collections::BTreeMap::<String, usize>::new();
    for i in items {
        for c in &i.entry.meta.categories {
            *m.entry(c.clone()).or_default() += 1;
        }
    }
    let mut v: Vec<(String, usize)> = m.into_iter().collect();
    v.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
    v
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PublisherInfo {
    pub id: String,
    pub name: String,
    pub url: String,
    pub level: Level,
    pub apps: usize,
}

pub fn publishers(items: &[Item]) -> Vec<PublisherInfo> {
    let mut v: Vec<PublisherInfo> = Vec::new();
    for i in items {
        let p = &i.entry.meta.publisher;
        if p.id.is_empty() {
            continue;
        }
        match v.iter_mut().find(|x| x.id == p.id) {
            Some(x) => {
                x.apps += 1;
                x.level = x.level.min(i.verdict.level);
            }
            None => v.push(PublisherInfo { id: p.id.clone(), name: p.name.clone(), url: p.url.clone(), level: i.verdict.level, apps: 1 }),
        }
    }
    v.sort_by(|a, b| a.level.cmp(&b.level).then(a.name.cmp(&b.name)));
    v
}

/// Convenience for callers that have a policy: the actor and the scope a new install goes to by default.
pub fn actor_of(user: &str, p: &Policy) -> Actor {
    Actor::from_policy(user, p)
}

pub mod updatecheck;
pub mod updatenotice;
pub mod sysannounce;

#[cfg(test)]
mod tests;

#[cfg(test)]
mod updatecheck_tests;

#[cfg(test)]
mod tile_tests;

#[cfg(test)]
mod tilesdk_tests;

#[cfg(test)]
mod publish_tests;

#[cfg(test)]
mod server_tests;

#[cfg(test)]
mod components_tests;
#[cfg(test)]
mod compinstalled_tests;
#[cfg(test)]
mod compapply_tests;

#[cfg(test)]
mod private_tests;

#[cfg(test)]
mod lifecycle_tests;
