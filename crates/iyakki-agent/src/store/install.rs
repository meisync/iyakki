//! Reviewing and installing: the part of the store that touches the system. Every package goes through [`Store::review`]
//! (hash, signature, trust level, revocation, version rules, permissions, licence) before anything is written, and
//! [`Store::install`] installs the very bytes that were reviewed, after checking again. Nothing here talks to the
//! network: the caller brings the bytes.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use super::catalog::{sha256_hex, Entry};
use super::license::{Check as LicenseCheck, LicenseStore, Token};
use super::manifest::{canonical_set, Meta, Permission};
use super::trust::{look_alikes, Level, Verdict};
use super::Store;
use crate::pkg::{self, newer, PkgError};
use crate::services::Services;
use crate::skills::Scope;
use crate::wtool;

/// Who is asking, and what their role allows.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Actor {
    pub user: String,
    /// The account may use the STORE tile at all.
    pub allowed: bool,
    /// Install for themselves (policy `skills`).
    pub can_own: bool,
    /// Install for everyone (policy `manage_skills`).
    pub can_shared: bool,
    /// Administrators: servers, unsigned packages, catalogs and keys (policy `manage_settings`).
    pub admin: bool,
}

impl Actor {
    pub fn from_policy(user: &str, p: &iyakki_sec::policy::Policy) -> Actor {
        Actor { user: user.into(), allowed: p.apps.contains(&iyakki_sec::policy::App::Store), can_own: p.skills, can_shared: p.manage_skills, admin: p.manage_settings }
    }

    /// The system itself (turning off a revoked app).
    pub fn system() -> Actor {
        Actor { user: "system".into(), allowed: true, can_own: true, can_shared: true, admin: true }
    }

    pub fn default_scope(&self) -> Scope {
        if self.can_own { Scope::User(self.user.clone()) } else { Scope::Shared }
    }
}

pub fn scope_key(s: &Scope) -> String {
    match s {
        Scope::Shared => "shared".into(),
        Scope::User(u) => format!("user/{u}"),
    }
}

pub fn scope_from_key(k: &str) -> Scope {
    k.strip_prefix("user/").map_or(Scope::Shared, |u| Scope::User(u.into()))
}

/// One thing the store checked, with its result in words.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Step {
    pub label: String,
    pub ok: bool,
    pub note: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Perm {
    pub canonical: String,
    pub words: String,
    pub sensitive: bool,
}

/// What the person is shown before anything is installed.
#[derive(Clone, Debug)]
pub struct Review {
    pub kind: String,
    pub name: String,
    pub version: String,
    pub title: String,
    pub meta: Meta,
    pub verdict: Verdict,
    pub steps: Vec<Step>,
    pub permissions: Vec<Perm>,
    pub warnings: Vec<String>,
    pub scope: Scope,
    /// Why it cannot be installed, if it cannot.
    pub refusal: Option<String>,
    /// A paid app without a valid licence.
    pub locked: bool,
    pub installed: Option<String>,
    /// A short look at what is inside (the text of a skill, the command of a server).
    pub preview: Vec<String>,
    pub sha256: String,
    pub size: usize,
    input: Input,
}

#[derive(Clone, Debug)]
struct Input {
    bytes: Vec<u8>,
    entry: Option<Entry>,
    source: String,
    builtin: bool,
    catalog_expired: bool,
    known: Vec<(String, String)>,
    /// Going back to the previous version on purpose (the version rules for going up do not apply; the store records it).
    rollback: bool,
}

impl Review {
    pub fn can_install(&self) -> bool {
        self.refusal.is_none()
    }

    /// Whether the person must accept something beyond reading the permissions.
    pub fn needs_warning_ack(&self) -> bool {
        !self.warnings.is_empty() || self.verdict.level.needs_warning_ack()
    }

    pub fn is_sensitive(&self) -> bool {
        self.permissions.iter().any(|p| p.sensitive)
    }

    /// The whole review as text: the approval prompt of the assistant, `iyakki pkg verify`, and tests.
    pub fn summary(&self) -> String {
        let mut s = format!("{} {} {}{}\n", self.kind, self.title_or_name(), self.version, match &self.installed {
            Some(v) if newer(&self.version, v) => format!(" (update from {v})"),
            Some(v) => format!(" (installed: {v})"),
            None => " (new)".into(),
        });
        s.push_str(&format!("{}: {}\n", self.verdict.level.label(), self.verdict.label));
        if let Some(p) = &self.verdict.publisher {
            s.push_str(&format!("publisher: {}\n", p.name));
        }
        for st in &self.steps {
            s.push_str(&format!("{} {}{}\n", if st.ok { "ok " } else { "NO " }, st.label, if st.note.is_empty() { String::new() } else { format!(": {}", st.note) }));
        }
        if self.permissions.is_empty() {
            s.push_str("permissions: none\n");
        } else {
            s.push_str(&format!("permissions: {}\n", self.permissions.iter().map(|p| p.words.clone()).collect::<Vec<_>>().join("; ")));
        }
        for w in &self.warnings {
            s.push_str(&format!("warning: {w}\n"));
        }
        if let Some(r) = &self.refusal {
            s.push_str(&format!("cannot be installed: {r}\n"));
        }
        s
    }

    fn title_or_name(&self) -> &str {
        if self.title.is_empty() { &self.name } else { &self.title }
    }
}

/// What the person agreed to.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub struct Confirm {
    pub permissions: bool,
    pub warnings: bool,
}

impl Confirm {
    pub fn all() -> Confirm {
        Confirm { permissions: true, warnings: true }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum InstallError {
    /// The review said no.
    Refused(String),
    /// The person has not yet agreed to the permissions or the warnings.
    NeedsConfirmation(String),
    /// What was reviewed is not what is there now.
    Changed(String),
    Failed(String),
}

impl std::fmt::Display for InstallError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            InstallError::Refused(m) | InstallError::NeedsConfirmation(m) | InstallError::Changed(m) | InstallError::Failed(m) => write!(f, "{m}"),
        }
    }
}

/// An installed app, as the store remembers it.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct Record {
    pub kind: String,
    pub name: String,
    pub title: String,
    pub version: String,
    pub sha256: String,
    pub signer: Option<String>,
    pub cert: Option<String>,
    pub level: String,
    pub publisher: String,
    pub scope: String,
    pub source: String,
    pub installed_at: i64,
    pub disabled: bool,
    /// Came with the system image (seeded or built in).
    pub preinstalled: bool,
}

impl Record {
    pub fn key(&self) -> String {
        format!("{}/{}/{}", self.kind, self.scope, self.name)
    }

    pub fn scope(&self) -> Scope {
        scope_from_key(&self.scope)
    }
}

pub fn record_key(kind: &str, scope: &Scope, name: &str) -> String {
    format!("{kind}/{}/{name}", scope_key(scope))
}

/// Verified installed content of a kind that other tiles may use (dashboard templates, themes, agent presets).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Content {
    pub kind: String,
    pub name: String,
    pub version: String,
    pub scope: Scope,
    pub payload: String,
}

fn io(e: std::io::Error) -> String {
    e.to_string()
}

pub(super) fn write_private(path: &std::path::Path, bytes: &[u8]) -> Result<(), String> {
    use std::io::Write;
    use std::os::unix::fs::OpenOptionsExt;
    if let Some(d) = path.parent() {
        std::fs::create_dir_all(d).map_err(io)?;
    }
    let tmp = path.with_extension("tmp");
    std::fs::OpenOptions::new().write(true).create(true).truncate(true).mode(0o600).open(&tmp).and_then(|mut f| f.write_all(bytes)).map_err(io)?;
    std::fs::rename(&tmp, path).map_err(io)
}

#[derive(Deserialize)]
struct McpPayload {
    name: String,
    command: String,
    #[serde(default)]
    args: Vec<String>,
}

fn mcp_payload(payload: &str) -> Result<McpPayload, String> {
    let p: McpPayload = serde_json::from_str(payload).map_err(|e| format!("the server definition is not readable: {e}"))?;
    if !crate::mcp::valid_server_name(&p.name) {
        return Err("the server name: letters, digits, - and _".into());
    }
    if !(p.command == "{self}" || (p.command.starts_with('/') && !p.command.contains("..") && !p.command.chars().any(|c| c.is_whitespace() || c.is_control()))) {
        return Err("the server command must be an absolute path to a program".into());
    }
    if p.args.len() > 32 || p.args.iter().any(|a| a.len() > 300 || a.chars().any(|c| c.is_control())) {
        return Err("too many or too long arguments".into());
    }
    Ok(p)
}

impl Store {
    pub fn records(&self) -> std::collections::BTreeMap<String, Record> {
        std::fs::read(self.dir().join("installed.json")).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default()
    }

    pub(super) fn save_records(&self, r: &std::collections::BTreeMap<String, Record>) -> Result<(), String> {
        write_private(&self.dir().join("installed.json"), &serde_json::to_vec_pretty(r).unwrap_or_default())
    }

    pub(super) fn highwater(&self) -> std::collections::BTreeMap<String, String> {
        std::fs::read(self.dir().join("highwater.json")).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default()
    }

    fn raise_highwater(&self, kind: &str, name: &str, version: &str) {
        let mut h = self.highwater();
        let k = format!("{kind}/{name}");
        if h.get(&k).is_none_or(|old| newer(version, old)) {
            h.insert(k, version.to_string());
            let _ = write_private(&self.dir().join("highwater.json"), &serde_json::to_vec_pretty(&h).unwrap_or_default());
        }
    }

    pub fn licenses(&self) -> LicenseStore {
        LicenseStore::new(self.dir())
    }

    pub(super) fn pkg_copy(&self, sha: &str) -> PathBuf {
        self.dir().join("pkgs").join(format!("{sha}.iyp"))
    }

    /// The version of this app that is installed in `scope` and by what means, if any: the store's own record, or
    /// something the system put there itself (a seeded skill, the sample server).
    pub fn installed_version(&self, svc: &Services, kind: &str, name: &str, scope: &Scope) -> Option<(String, bool)> {
        if let Some(r) = self.records().get(&record_key(kind, scope, name)) {
            return (!r.disabled).then(|| (r.version.clone(), r.preinstalled));
        }
        let present = match kind {
            "skill" => svc.skills.get(scope, name).is_some(),
            "tool" => svc.packages.get_tool(scope, name).is_some(),
            "mcp" => *scope == Scope::Shared && svc.mcp.list().iter().any(|s| s.name == name),
            _ => false,
        };
        present.then(|| ("preinstalled".to_string(), true))
    }

    /// Every record in the store, plus those the system installed itself that the store knows about from `known`.
    pub fn is_disabled(&self, kind: &str, name: &str, scope: &Scope) -> bool {
        self.records().get(&record_key(kind, scope, name)).is_some_and(|r| r.disabled)
    }

    // ---------------------------------------------------------------------------------------- review

    /// Check everything about `bytes` and say what installing them would do. Never writes anything.
    #[allow(clippy::too_many_arguments)]
    pub fn review(&self, svc: &Services, actor: &Actor, bytes: Vec<u8>, entry: Option<&Entry>, source: &str, builtin: bool, scope: Scope, catalog_expired: bool, known: &[(String, String)]) -> Review {
        let input = Input { bytes, entry: entry.cloned(), source: source.into(), builtin, catalog_expired, known: known.to_vec(), rollback: false };
        self.review_input(svc, actor, input, scope)
    }

    /// Review the kept copy of the previous version for an explicit "go back". Same checks as any package (hash, signature,
    /// trust, revocation, publisher), but the version may be lower than the installed one.
    pub fn review_rollback(&self, svc: &Services, actor: &Actor, bytes: Vec<u8>, source: &str, scope: Scope) -> Review {
        let input = Input { bytes, entry: None, source: source.into(), builtin: source == "builtin", catalog_expired: false, known: vec![], rollback: true };
        self.review_input(svc, actor, input, scope)
    }

    fn review_input(&self, svc: &Services, actor: &Actor, input: Input, scope: Scope) -> Review {
        let mut r = Review {
            kind: input.entry.as_ref().map(|e| e.kind.clone()).unwrap_or_default(),
            name: input.entry.as_ref().map(|e| e.name.clone()).unwrap_or_default(),
            version: input.entry.as_ref().map(|e| e.version.clone()).unwrap_or_default(),
            title: input.entry.as_ref().map(|e| e.title().to_string()).unwrap_or_default(),
            meta: input.entry.as_ref().map(|e| e.meta.clone()).unwrap_or_default(),
            verdict: Verdict { level: Level::Unsigned, signer: None, publisher: None, label: "unknown".into(), chain: vec![], warnings: vec![], refusal: None },
            steps: vec![],
            permissions: vec![],
            warnings: vec![],
            scope: scope.clone(),
            refusal: None,
            locked: false,
            installed: None,
            preview: vec![],
            sha256: sha256_hex(&input.bytes),
            size: input.bytes.len(),
            input: input.clone(),
        };
        let step = |r: &mut Review, label: &str, ok: bool, note: &str| r.steps.push(Step { label: label.into(), ok, note: note.into() });
        let refuse = |r: &mut Review, why: String| {
            if r.refusal.is_none() {
                r.refusal = Some(why);
            }
        };
        if !actor.allowed {
            refuse(&mut r, "Your account may not use the STORE.".into());
        }
        let Ok(text) = String::from_utf8(input.bytes.clone()) else {
            step(&mut r, "Package readable", false, "not text");
            refuse(&mut r, "This is not an IYAKKI package.".into());
            return r;
        };
        // the catalog's pin (the fetch already compared it; this is the last look at the same bytes)
        if let Some(e) = &input.entry {
            if !e.sha256.is_empty() && e.sha256 != r.sha256 {
                step(&mut r, "Matches the catalog", false, "different bytes than the catalog lists");
                refuse(&mut r, "The package is not the one the catalog listed.".into());
                return r;
            }
            if !e.sha256.is_empty() {
                step(&mut r, "Matches the catalog", true, "same hash as listed");
            }
        }
        let package = match pkg::parse(&text) {
            Ok(p) => p,
            Err(e) => {
                let (label, why) = match &e {
                    PkgError::BadHash => ("Package intact", "The package was changed after it was made (its hash does not match). Do not install it.".to_string()),
                    PkgError::BadSignature => ("Signature valid", "The signature does not match this package. It was altered or signed wrongly. Do not install it.".to_string()),
                    other => ("Package readable", format!("This is not a usable package: {other}")),
                };
                step(&mut r, label, false, "");
                refuse(&mut r, why);
                return r;
            }
        };
        let m = &package.manifest;
        r.kind = m.kind.clone();
        r.name = m.name.clone();
        r.version = m.version.clone();
        step(&mut r, "Package intact", true, "the hash matches");
        // metadata
        match m.meta.as_deref().map(Meta::parse) {
            Some(Ok(meta)) => r.meta = meta,
            Some(Err(e)) => {
                step(&mut r, "Store information valid", false, &e);
                refuse(&mut r, format!("The package's store information is not valid: {e}"));
            }
            None => {}
        }
        if let Some(e) = &input.entry {
            if (e.kind != m.kind || e.name != m.name || e.version != m.version) && !input.builtin {
                step(&mut r, "Matches the catalog entry", false, "name, type or version differ");
                refuse(&mut r, "The package is not what the catalog entry describes.".into());
            }
            if !e.signer.is_empty() && m.signer.as_deref() != Some(e.signer.as_str()) {
                step(&mut r, "Same signer as listed", false, "");
                refuse(&mut r, "The package is signed by a different key than the catalog says.".into());
            }
        }
        if r.title.is_empty() || m.meta.is_some() {
            r.title = if r.meta.title.is_empty() { m.name.clone() } else { r.meta.title.clone() };
        }
        // trust
        let own = svc.packages.trusted_keys();
        let revs = self.revocations();
        let verdict = if input.builtin && m.signer.is_none() { Verdict::builtin() } else { self.judge(&own, &revs).verdict(m) };
        // developer mode (`devmode`): whatever signed it, a package from a paired developer computer is labelled Developer (local);
        // a revocation still wins
        let developer = input.source.starts_with(crate::devmode::SOURCE_PREFIX);
        let verdict = if developer && verdict.level != Level::Revoked { Verdict::developer(&input.source, m.signer.as_deref()) } else { verdict };
        step(&mut r, if m.signer.is_some() { "Signature valid" } else { "Signature" }, m.signer.is_some() || input.builtin, &match (&m.signer, input.builtin) {
            (Some(_), _) => verdict.label.clone(),
            (None, true) => "built into this image".to_string(),
            (None, false) => "not signed".to_string(),
        });
        step(&mut r, "Chain of trust", !matches!(verdict.level, Level::Community | Level::Unsigned | Level::Developer | Level::Revoked), &verdict.chain.join(" "));
        step(&mut r, "Not revoked", verdict.level != Level::Revoked, &match &verdict.refusal { Some(m) => m.clone(), None => match self.revocations().freshness(self.now()) {
            super::revocation::Freshness::Missing => "no revocation list on this device yet".into(),
            super::revocation::Freshness::Fresh => "checked against the revocation list".into(),
            super::revocation::Freshness::Old(d) => format!("the revocation list is {d} days old"),
        } });
        r.warnings.extend(verdict.warnings.iter().cloned());
        if let Some(why) = &verdict.refusal {
            refuse(&mut r, why.clone());
        }
        r.verdict = verdict;
        // permissions come from the payload, not from what the page claims
        let mut actual: Vec<String> = vec![];
        match m.kind.as_str() {
            "tool" => match wtool::parse_payload(&package.payload) {
                Ok(t) => {
                    actual = t.caps.iter().map(|c| Permission::Cap(c.clone()).canonical()).collect();
                    r.preview.push(t.manifest.description.clone());
                    let b = &t.manifest.budget;
                    r.preview.push(format!("It runs in the sandbox with at most {} instructions, {} MiB and {} s per call; every call asks you first.", b.fuel, b.memory_mb, b.seconds));
                    if t.manifest.name != m.name || t.manifest.version != m.version {
                        refuse(&mut r, "The package header and the tool inside it disagree.".into());
                    }
                }
                Err(e) => refuse(&mut r, format!("The tool inside is not valid: {e}")),
            },
            "mcp" => match mcp_payload(&package.payload) {
                Ok(p) => {
                    actual = vec![Permission::Run(p.command.clone()).canonical()];
                    r.preview.push(format!("It starts the program {} {} as a tool server and offers its tools to the assistant.", p.command, p.args.join(" ")));
                    if p.name != m.name {
                        refuse(&mut r, "The package name and the server inside it differ.".into());
                    }
                    if !actor.admin {
                        refuse(&mut r, "Only administrators can add a tool server.".into());
                    }
                }
                Err(e) => refuse(&mut r, e),
            },
            "skill" => match crate::skills::parse(&package.payload, scope.clone()) {
                Ok(sk) => {
                    r.preview.push(sk.description.clone());
                    r.preview.extend(sk.body.lines().take(12).map(str::to_string));
                    if sk.name != m.name {
                        refuse(&mut r, "The package name and the skill inside it differ.".into());
                    }
                }
                Err(e) => refuse(&mut r, format!("The skill inside is not valid: {e}")),
            },
            "extension" if r.meta.ext_kind == "driver" && developer => refuse(&mut r, "Driver packs and kernel modules cannot be installed from a developer computer. Install them from the STORE's Drivers page, where the driver-pack rules apply.".into()),
            "extension" if r.meta.ext_kind == "driver" => match super::drivers::Drivers::for_data(self.data_dir()).review_bits(&package.payload, r.verdict.level.id(), actor.admin, scope == Scope::Shared) {
                Ok(b) => {
                    actual = b.permissions;
                    r.preview.extend(b.preview);
                    r.warnings.extend(b.warnings);
                }
                Err(e) => refuse(&mut r, e),
            },
            "tile" => match super::tile::Bundle::from_payload(&package.payload) {
                Ok(b) => {
                    actual = b.manifest.caps.permissions();
                    let mf = &b.manifest;
                    r.preview.push(format!("A custom app: “{}” gets its own tile in your launcher (group “{}”), opens full screen on its own private page and runs in the same sandbox as the browser.", mf.name, mf.group));
                    r.preview.push(if actual.is_empty() { "It asks for nothing: no saved data, no network, no files.".to_string() } else { format!("It asks for: {}.", actual.join(", ")) });
                    r.preview.push(format!("{} files, {} KB. It can never use your vault, other apps or the system, only the bridge listed above.", b.files.len(), b.total().div_ceil(1024)));
                    if mf.id != m.name || mf.version != m.version {
                        refuse(&mut r, "The package header and the app inside it disagree.".into());
                    }
                    let (problems, warnings) = super::tile::lint(&b);
                    if let Some(p) = problems.first() {
                        refuse(&mut r, format!("The app fails its own checks: {p}"));
                    }
                    r.warnings.extend(warnings.into_iter().take(3));
                }
                Err(e) => refuse(&mut r, format!("The app inside is not valid: {e}")),
            },
            "app" => refuse(&mut r, "Apps with their own screen are not supported by this version of IYAKKI.".into()),
            "cli" => refuse(&mut r, "Command-line tools are installed from the CLI tile, which checks its own downloads.".into()),
            _ => {
                actual = r.meta.permissions.clone();
                r.preview.push(format!("A {} for other tiles to use; it runs nothing by itself.", m.kind));
            }
        }
        let declared = canonical_set(&r.meta.permissions);
        let actual_set = canonical_set(&actual);
        if matches!(m.kind.as_str(), "tool" | "mcp" | "skill" | "tile") && declared != actual_set {
            r.warnings.push(format!("The page lists {} permission(s), but the package itself asks for {}. The list below is what the package really asks for.", declared.len(), actual_set.len()));
        }
        r.permissions = actual_set
            .iter()
            .filter_map(|p| Permission::parse(p).ok())
            .map(|p| Perm { canonical: p.canonical(), words: if p == Permission::Run("{self}".into()) { "run IYAKKI itself as a tool server".into() } else { p.words(&actor.user) }, sensitive: p.sensitive() })
            .collect();
        // names that look like other apps
        let la = look_alikes(&m.name, &r.verdict.publisher.as_ref().map_or_else(|| r.meta.publisher.id.clone(), |p| p.id.clone()), &input.known);
        if !la.is_empty() {
            r.warnings.push(format!("The name looks like {}. Check that this is the app you mean.", la.join(", ")));
        }
        // who may install where
        match &scope {
            Scope::Shared if !actor.can_shared => refuse(&mut r, "Only operators and administrators can install for everyone.".into()),
            Scope::User(u) if !actor.can_own || *u != actor.user => refuse(&mut r, "Your role does not allow installing apps for yourself.".into()),
            _ => {}
        }
        if m.kind == "mcp" && scope != Scope::Shared {
            refuse(&mut r, "Tool servers are installed for everyone, by an administrator.".into());
        }
        if r.verdict.level == Level::Unsigned && !actor.admin {
            refuse(&mut r, "Only administrators can install packages that are not signed.".into());
        }
        // versions
        let installed = self.records().get(&record_key(&m.kind, &scope, &m.name)).map(|x| x.version.clone());
        r.installed = installed.clone();
        let hw = self.highwater().get(&format!("{}/{}", m.kind, m.name)).cloned();
        let mut version_note = "newer than anything installed before".to_string();
        let mut version_ok = true;
        if let Some(h) = hw.as_ref().filter(|_| !input.rollback) {
            if newer(h, &m.version) {
                version_ok = false;
                version_note = format!("version {} is older than {h}, which was installed before; going back is refused", m.version);
                refuse(&mut r, format!("This is an older version ({}) than one installed before ({h}). Going back to older versions is refused.", m.version));
            }
        }
        if let Some(i) = installed.as_ref().filter(|_| input.rollback) {
            version_note = format!("going back from {i} to {} on purpose", m.version);
        }
        if let Some(i) = installed.as_ref().filter(|_| !input.rollback) {
            if !newer(&m.version, i) && version_ok {
                version_note = format!("version {i} is already installed");
                if m.version == *i {
                    r.warnings.push(format!("Version {i} is already installed."));
                } else {
                    version_ok = false;
                    refuse(&mut r, format!("Version {} is older than the installed {i}.", m.version));
                }
            }
            // the same publisher must keep the name (dependency confusion)
            if let Some(old) = self.records().get(&record_key(&m.kind, &scope, &m.name)) {
                let same_signer = old.signer == m.signer;
                let same_pub = !old.publisher.is_empty() && old.publisher == r.verdict.publisher.as_ref().map_or(String::new(), |p| p.id.clone());
                if !(same_signer || same_pub || r.verdict.level == Level::Official) {
                    version_ok = false;
                    version_note = "published by someone other than the publisher of the installed version".into();
                    refuse(&mut r, "The installed copy is from a different publisher. Remove it first if you really want this one.".into());
                }
            }
        }
        step(&mut r, "Version rule", version_ok, &version_note);
        if !r.meta.min_version.is_empty() && newer(&r.meta.min_version, env!("CARGO_PKG_VERSION")) {
            let why = format!("It needs IYAKKI {} or newer; this is {}.", r.meta.min_version, env!("CARGO_PKG_VERSION"));
            refuse(&mut r, why);
        }
        if input.catalog_expired {
            refuse(&mut r, "The catalog it came from has expired. Load a newer catalog first.".into());
        }
        // licence
        if r.meta.pricing.model.needs_licence() {
            let ls = self.licenses();
            let issuers: Vec<String> = m.signer.iter().cloned().collect();
            let revs = self.revocations();
            let check = LicenseCheck { app: &m.name, issuers: &issuers, roots: &self.roots, device: &ls.device_id(), account: &actor.user, now: self.now(), revocations: &revs };
            match ls.get(&m.name).map(|t| t.verify(&check).map(|()| t)) {
                Some(Ok(t)) => step(&mut r, "Licence", true, &t.describe()),
                Some(Err(e)) => {
                    r.locked = true;
                    step(&mut r, "Licence", false, &e.to_string());
                    refuse(&mut r, format!("Locked until licensed: {e}."));
                }
                None => {
                    r.locked = true;
                    step(&mut r, "Licence", false, "this app is not free and no licence is on this device");
                    refuse(&mut r, "Locked until licensed: this app is not free. Buying is not available yet; add a licence if you have one.".into());
                }
            }
        }
        r
    }

    // ---------------------------------------------------------------------------------------- install

    /// Install what was reviewed. The review is made again from the same bytes first; if the answer differs the
    /// install stops (a revocation list may have arrived while the person was reading).
    pub fn install(&self, svc: &Services, actor: &Actor, review: &Review, confirm: Confirm) -> Result<Record, InstallError> {
        if let Some(why) = &review.refusal {
            return Err(InstallError::Refused(why.clone()));
        }
        if !review.permissions.is_empty() && !confirm.permissions {
            return Err(InstallError::NeedsConfirmation("Confirm the permissions first.".into()));
        }
        if review.needs_warning_ack() && !confirm.warnings {
            return Err(InstallError::NeedsConfirmation("Read the warnings and confirm that you want to go on.".into()));
        }
        let again = self.review_input(svc, actor, review.input.clone(), review.scope.clone());
        if let Some(why) = &again.refusal {
            return Err(InstallError::Changed(format!("Something changed while you were looking: {why}")));
        }
        if again.verdict.level != review.verdict.level || again.permissions != review.permissions || again.sha256 != review.sha256 {
            return Err(InstallError::Changed("The package or its trust level changed while you were looking. Open it again.".into()));
        }
        let bytes = &review.input.bytes;
        let text = String::from_utf8_lossy(bytes).into_owned();
        let package = pkg::parse(&text).map_err(|e| InstallError::Failed(e.to_string()))?;
        let (can_shared, can_own) = (actor.can_shared, actor.can_own);
        let scope = review.scope.clone();
        match package.manifest.kind.as_str() {
            "skill" => {
                let r = svc.packages.review(&text, &scope).map_err(|e| InstallError::Failed(e.to_string()))?;
                svc.packages.install_with(&r, true, can_shared, can_own, review.input.rollback).map_err(|e| InstallError::Failed(e.to_string()))?;
            }
            "tool" => {
                let r = svc.packages.review_tool(&text, &scope).map_err(|e| InstallError::Failed(e.to_string()))?;
                svc.packages.install_tool_with(&r, true, can_shared, can_own, review.input.rollback).map_err(|e| InstallError::Failed(e.to_string()))?;
            }
            "mcp" => {
                let p = mcp_payload(&package.payload).map_err(InstallError::Failed)?;
                let command = if p.command == "{self}" { std::env::current_exe().map_or_else(|_| "/usr/bin/iyakki".into(), |e| e.display().to_string()) } else { p.command };
                let ours = self.records().contains_key(&record_key("mcp", &scope, &p.name));
                if ours || svc.mcp.list().iter().any(|s| s.name == p.name && s.sample) {
                    svc.mcp.replace(crate::mcp::StoredServer { name: p.name, command, args: p.args, sample: false }).map_err(InstallError::Failed)?;
                } else {
                    svc.mcp.add(crate::mcp::StoredServer { name: p.name, command, args: p.args, sample: false }).map_err(InstallError::Failed)?;
                }
            }
            "extension" if review.meta.ext_kind == "driver" => {
                let level = review.verdict.level.id();
                let license = package.manifest.license.clone();
                super::drivers::Drivers::for_data(self.data_dir()).install(&package.manifest.name, &package.manifest.version, &license, &review.input.source, level, &package.payload).map_err(InstallError::Failed)?;
                write_private(&self.content_file(&scope, &package.manifest.kind, &package.manifest.name), bytes).map_err(InstallError::Failed)?;
            }
            "agent" | "template" | "theme" | "extension" | "tile" => {
                write_private(&self.content_file(&scope, &package.manifest.kind, &package.manifest.name), bytes).map_err(InstallError::Failed)?;
            }
            other => return Err(InstallError::Refused(format!("{other} packages cannot be installed by this version."))),
        }
        let m = &package.manifest;
        write_private(&self.pkg_copy(&m.sha256), bytes).map_err(InstallError::Failed)?;
        let rec = Record {
            kind: m.kind.clone(),
            name: m.name.clone(),
            title: review.title.clone(),
            version: m.version.clone(),
            sha256: m.sha256.clone(),
            signer: m.signer.clone(),
            cert: m.cert.clone(),
            level: review.verdict.level.id().into(),
            publisher: review.verdict.publisher.as_ref().map_or_else(|| review.meta.publisher.id.clone(), |p| p.id.clone()),
            scope: scope_key(&scope),
            source: review.input.source.clone(),
            installed_at: self.now(),
            disabled: false,
            preinstalled: false,
        };
        let mut all = self.records();
        let old = all.insert(rec.key(), rec.clone());
        self.save_records(&all).map_err(InstallError::Failed)?;
        self.raise_highwater(&m.kind, &m.name, &m.version);
        // the replaced version is kept for one generation (rollback); anything older goes
        self.after_install(&old, &rec, review.input.rollback);
        Ok(rec)
    }

    pub(super) fn content_file(&self, scope: &Scope, kind: &str, name: &str) -> PathBuf {
        let base = match scope {
            Scope::Shared => self.dir().join("content/shared"),
            Scope::User(u) => self.dir().join("content/user").join(u),
        };
        base.join(kind).join(format!("{name}.iyp"))
    }

    // ---------------------------------------------------------------------------------------- remove, disable, enable

    pub(super) fn uninstall_from_backend(&self, svc: &Services, actor: &Actor, kind: &str, name: &str, scope: &Scope) -> Result<(), String> {
        let (cs, co) = (actor.can_shared, actor.can_own);
        match kind {
            "skill" => match svc.packages.remove(scope, name, cs, co) {
                Ok(()) => Ok(()),
                Err(PkgError::Skill(crate::skills::SkillError::NotFound)) => Ok(()),
                Err(e) => Err(e.to_string()),
            },
            "tool" => match svc.packages.remove_tool(scope, name, cs, co) {
                Ok(()) => Ok(()),
                Err(PkgError::Skill(crate::skills::SkillError::NotFound)) => Ok(()),
                Err(e) => Err(e.to_string()),
            },
            "mcp" => {
                if !actor.admin {
                    return Err("Only administrators can remove a tool server.".into());
                }
                let _ = svc.mcp.remove(name);
                Ok(())
            }
            _ => {
                if kind == "extension" && super::drivers::Drivers::for_data(self.data_dir()).installed().iter().any(|r| r.id == name) {
                    if !actor.admin {
                        return Err("Only administrators can remove a driver pack.".into());
                    }
                    super::drivers::Drivers::for_data(self.data_dir()).remove(name)?;
                }
                match scope {
                    Scope::Shared if !cs => return Err("Only operators and administrators can remove what is installed for everyone.".into()),
                    Scope::User(_) if !co => return Err("Your role does not allow this.".into()),
                    _ => {}
                }
                let _ = std::fs::remove_file(self.content_file(scope, kind, name));
                Ok(())
            }
        }
    }

    /// Take an app away. A preinstalled one can be installed again from the store.
    pub fn remove(&self, svc: &Services, actor: &Actor, kind: &str, name: &str, scope: &Scope) -> Result<(), String> {
        if !actor.allowed {
            return Err("Your account may not use the STORE.".into());
        }
        if matches!(scope, Scope::User(u) if *u != actor.user) {
            return Err("That app belongs to someone else.".into());
        }
        self.uninstall_from_backend(svc, actor, kind, name, scope)?;
        let mut all = self.records();
        if let Some(r) = all.remove(&record_key(kind, scope, name)) {
            self.save_records(&all)?;
            // its pin, history and kept copies (this version's and the previous one's) go with it
            self.after_remove(&r);
        }
        Ok(())
    }

    /// Turn an app off but keep it (and its verified copy) so it can be turned on again.
    pub fn disable(&self, svc: &Services, actor: &Actor, kind: &str, name: &str, scope: &Scope) -> Result<(), String> {
        let mut all = self.records();
        let key = record_key(kind, scope, name);
        let Some(rec) = all.get_mut(&key) else { return Err("Only apps installed through the store can be turned off; remove it instead.".into()) };
        if rec.disabled {
            return Ok(());
        }
        if matches!(scope, Scope::User(u) if *u != actor.user) && actor.user != "system" {
            return Err("That app belongs to someone else.".into());
        }
        self.uninstall_from_backend(svc, actor, kind, name, scope)?;
        rec.disabled = true;
        self.save_records(&all)
    }

    /// Turn an app on again from the verified copy the store kept. It is checked again as if it were new.
    pub fn enable(&self, svc: &Services, actor: &Actor, kind: &str, name: &str, scope: &Scope) -> Result<Record, InstallError> {
        let mut all = self.records();
        let key = record_key(kind, scope, name);
        let rec = all.get(&key).cloned().ok_or_else(|| InstallError::Refused("That app is not installed.".into()))?;
        if !rec.disabled {
            return Ok(rec);
        }
        let bytes = std::fs::read(self.pkg_copy(&rec.sha256)).map_err(|_| InstallError::Failed("The kept copy of this app is missing. Install it again.".into()))?;
        if !pkg::parse(&String::from_utf8_lossy(&bytes)).is_ok_and(|p| p.manifest.sha256 == rec.sha256) {
            return Err(InstallError::Failed("The kept copy no longer verifies. Install it again.".into()));
        }
        // re-review without the "already installed" rule: drop the record while installing
        all.remove(&key);
        self.save_records(&all).map_err(InstallError::Failed)?;
        let builtin = rec.source == "builtin";
        let r = self.review(svc, actor, bytes, None, &rec.source, builtin, scope.clone(), false, &[]);
        let result = if r.refusal.is_some() { Err(InstallError::Refused(r.refusal.clone().unwrap_or_default())) } else { self.install(svc, actor, &r, Confirm::all()) };
        match result {
            Ok(new) => Ok(Record { preinstalled: rec.preinstalled, ..new }).inspect(|n| {
                let mut a = self.records();
                a.insert(n.key(), n.clone());
                let _ = self.save_records(&a);
            }),
            Err(e) => {
                let mut a = self.records();
                a.insert(key, rec);
                let _ = self.save_records(&a);
                Err(e)
            }
        }
    }

    /// Check every installed app against the revocation list and turn off those that were withdrawn. Returns what was
    /// turned off, in words. Cheap enough to run whenever the tile opens and after each list update.
    pub fn enforce_revocations(&self, svc: &Services) -> Vec<String> {
        let own = svc.packages.trusted_keys();
        let revs = self.revocations();
        let judge = self.judge(&own, &revs);
        let mut off = Vec::new();
        for rec in self.records().values().filter(|r| !r.disabled) {
            let m = pkg::Manifest { kind: rec.kind.clone(), name: rec.name.clone(), version: rec.version.clone(), license: String::new(), sha256: rec.sha256.clone(), signer: rec.signer.clone(), meta: None, cert: rec.cert.clone() };
            let v = if rec.source == "builtin" && rec.signer.is_none() { Verdict::builtin() } else { judge.verdict(&m) };
            if v.level == Level::Revoked && self.disable(svc, &Actor::system(), &rec.kind, &rec.name, &rec.scope()).is_ok() {
                off.push(format!("{} was turned off: {}", rec.title, v.refusal.unwrap_or_default()));
            }
        }
        off
    }

    /// Installed content of `kind` for `user` (their own first, then the shared ones), each file verified again.
    pub fn content(&self, kind: &str, user: &str) -> Vec<Content> {
        let mut out = Vec::new();
        let revs = self.revocations();
        for scope in [Scope::User(user.to_string()), Scope::Shared] {
            if !iyakki_sec::policy::valid_username(user) {
                break;
            }
            let dir = self.content_file(&scope, kind, "x");
            let Some(dir) = dir.parent() else { continue };
            for e in std::fs::read_dir(dir).into_iter().flatten().flatten() {
                let Ok(text) = std::fs::read_to_string(e.path()) else { continue };
                let Ok(p) = pkg::parse(&text) else { continue };
                let m = &p.manifest;
                if m.kind != kind || e.path().file_stem().and_then(|s| s.to_str()) != Some(m.name.as_str()) || self.is_disabled(kind, &m.name, &scope) {
                    continue;
                }
                if revs.package_revoked(&m.kind, &m.name, &m.version, &m.sha256).is_some() || m.signer.as_ref().is_some_and(|k| revs.key_revoked(k).is_some()) {
                    continue;
                }
                if out.iter().any(|c: &Content| c.name == m.name) {
                    continue;
                }
                out.push(Content { kind: kind.into(), name: m.name.clone(), version: m.version.clone(), scope: scope.clone(), payload: p.payload });
            }
        }
        out
    }

    // ---------------------------------------------------------------------------------------- licences

    /// Check a licence token (pasted or read from a file) for the app it names and keep it if it is good.
    pub fn add_license(&self, actor: &Actor, text: &str, signer_of: &dyn Fn(&str) -> Vec<String>) -> Result<Token, String> {
        if !actor.allowed {
            return Err("Your account may not use the STORE.".into());
        }
        let t = Token::parse(text).map_err(|e| e.to_string())?;
        let ls = self.licenses();
        let revs = self.revocations();
        let issuers = signer_of(&t.app);
        let check = LicenseCheck { app: &t.app, issuers: &issuers, roots: &self.roots, device: &ls.device_id(), account: &actor.user, now: self.now(), revocations: &revs };
        t.verify(&check).map_err(|e| e.to_string())?;
        ls.save(&t)?;
        Ok(t)
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::super::catalog::tests::entry;
    use super::super::manifest::{PriceModel, Pricing, Publisher};
    use super::super::trust::{Cert, Roots};
    use super::*;
    use crate::pkg::{new_signing_key, public_key_hex};
    use ed25519_dalek::SigningKey;
    use std::sync::Arc;

    pub const NOW: i64 = 1_800_000_000;

    pub struct Offline;
    impl iyakki_proxy::Upstream for Offline {
        fn request(&self, _m: &str, _u: &str, _h: &[(String, String)], _b: &[u8]) -> Result<iyakki_proxy::UpstreamResponse, String> {
            Err("offline".into())
        }
    }

    pub struct Fx {
        pub svc: Arc<Services>,
        pub store: Store,
        pub root: SigningKey,
        pub dir: PathBuf,
    }

    pub fn fx(tag: &str) -> Fx {
        let dir = std::env::temp_dir().join(format!("iyakki-store-{}-{}", std::process::id(), tag));
        let _ = std::fs::remove_dir_all(&dir);
        let svc = Services::open(&dir, Arc::new(Offline), Arc::new(Offline)).unwrap();
        let root = new_signing_key().unwrap();
        let store = Store::open(&dir).with_roots(Roots::with("r1", &root)).with_clock(Arc::new(|| NOW));
        Fx { svc, store, root, dir }
    }

    pub fn admin() -> Actor {
        Actor { user: "root".into(), allowed: true, can_own: true, can_shared: true, admin: true }
    }

    pub fn user(name: &str) -> Actor {
        Actor { user: name.into(), allowed: true, can_own: true, can_shared: false, admin: false }
    }

    pub const SKILL_MD: &str = "---\nname: meeting-notes\ndescription: Turn raw meeting notes into decisions\ntriggers: meeting, minutes\n---\nList decisions first, then action items.\n";

    pub fn meta(publisher: &str, perms: &[&str]) -> Meta {
        Meta { title: "Meeting notes".into(), summary: "Notes to decisions".into(), publisher: Publisher { id: publisher.into(), name: publisher.to_uppercase(), url: String::new() }, permissions: perms.iter().map(|s| s.to_string()).collect(), ..Default::default() }
    }

    pub fn skill_pkg(version: &str, key: Option<&SigningKey>, cert: Option<&Cert>, m: &Meta) -> Vec<u8> {
        let sk = crate::skills::parse(SKILL_MD, Scope::Shared).unwrap();
        pkg::build_generic("skill", "meeting-notes", version, "MIT", &crate::skills::render(&sk), Some(&m.to_json()), cert.map(|c| c.to_json()).as_deref(), key).unwrap().into_bytes()
    }

    pub fn me() -> Scope {
        Scope::User("alice".into())
    }

    fn review(f: &Fx, a: &Actor, bytes: Vec<u8>, scope: Scope) -> Review {
        f.store.review(&f.svc, a, bytes, None, "test", false, scope, false, &[])
    }

    fn acme_cert(f: &Fx, key: &SigningKey, nb: i64, na: i64) -> Cert {
        Cert::issue(&f.root, Publisher { id: "acme".into(), name: "Acme".into(), url: String::new() }, &public_key_hex(key), nb, na, &["skill", "tool"])
    }

    #[test]
    fn an_official_package_installs_after_confirmation_through_the_skills_store() {
        let f = fx("official");
        let a = user("alice");
        let bytes = skill_pkg("1.0.0", Some(&f.root), None, &meta("iyakki", &[]));
        let r = review(&f, &a, bytes, me());
        assert_eq!(r.verdict.level, Level::Official, "{}", r.summary());
        assert!(r.refusal.is_none() && r.permissions.is_empty() && !r.needs_warning_ack(), "{}", r.summary());
        assert!(r.steps.iter().any(|s| s.label == "Package intact" && s.ok) && r.steps.iter().any(|s| s.label == "Version rule" && s.ok));
        assert!(r.summary().contains("Official") && r.preview.iter().any(|l| l.contains("List decisions")));
        let rec = f.store.install(&f.svc, &a, &r, Confirm::default()).unwrap();
        assert_eq!((rec.level.as_str(), rec.scope.as_str()), ("official", "user/alice"));
        assert!(f.svc.skills.get(&me(), "meeting-notes").is_some());
        assert_eq!(f.store.installed_version(&f.svc, "skill", "meeting-notes", &me()), Some(("1.0.0".to_string(), false)));
        assert!(f.svc.packages.list(&me()).iter().any(|(n, _)| n == "meeting-notes"), "the SKILLS tile's own record is there too");
        // remove: gone from both, and the high-water mark stays
        f.store.remove(&f.svc, &a, "skill", "meeting-notes", &me()).unwrap();
        assert!(f.svc.skills.get(&me(), "meeting-notes").is_none() && f.store.records().is_empty());
        let older = review(&f, &a, skill_pkg("0.9.0", Some(&f.root), None, &meta("iyakki", &[])), me());
        assert!(older.refusal.as_ref().unwrap().contains("older version"), "{}", older.summary());
        assert!(review(&f, &a, skill_pkg("1.0.0", Some(&f.root), None, &meta("iyakki", &[])), me()).refusal.is_none(), "the same version again is fine");
        let _ = std::fs::remove_dir_all(&f.dir);
    }

    #[test]
    fn a_verified_publisher_shows_its_chain_and_an_expired_certificate_falls_back_to_community_with_a_warning() {
        let f = fx("verified");
        let a = user("alice");
        let publisher = new_signing_key().unwrap();
        let good = acme_cert(&f, &publisher, NOW - 100, NOW + 100);
        let r = review(&f, &a, skill_pkg("1.0.0", Some(&publisher), Some(&good), &meta("acme", &[])), me());
        assert_eq!(r.verdict.level, Level::Verified, "{}", r.summary());
        assert!(r.verdict.chain.iter().any(|c| c.contains("certified by the IYAKKI root")) && r.summary().contains("publisher: Acme"));
        assert!(!r.needs_warning_ack());
        let expired = acme_cert(&f, &publisher, NOW - 200, NOW - 1);
        let r = review(&f, &a, skill_pkg("1.0.0", Some(&publisher), Some(&expired), &meta("acme", &[])), me());
        assert_eq!(r.verdict.level, Level::Community);
        assert!(r.warnings.iter().any(|w| w.contains("expired on")) && r.needs_warning_ack());
        assert_eq!(f.store.install(&f.svc, &a, &r, Confirm::default()).unwrap_err(), InstallError::NeedsConfirmation("Read the warnings and confirm that you want to go on.".into()));
        assert!(f.svc.skills.get(&me(), "meeting-notes").is_none(), "nothing was written without the confirmation");
        f.store.install(&f.svc, &a, &r, Confirm { permissions: false, warnings: true }).unwrap();
        assert!(f.svc.skills.get(&me(), "meeting-notes").is_some());
        let _ = std::fs::remove_dir_all(&f.dir);
    }

    #[test]
    fn community_trusted_and_unsigned_paths_and_who_may_take_them() {
        let f = fx("levels");
        let (stranger, ours) = (new_signing_key().unwrap(), new_signing_key().unwrap());
        let (a, root) = (user("alice"), admin());
        let m = meta("someone", &[]);
        let r = review(&f, &a, skill_pkg("1.0.0", Some(&stranger), None, &m), me());
        assert_eq!(r.verdict.level, Level::Community);
        assert!(r.refusal.is_none() && r.warnings.iter().any(|w| w.contains("does not know")));
        // the administrator's own key is Trusted and needs no warning
        f.svc.packages.trust_key(&public_key_hex(&ours), "Our IT").unwrap();
        let r = review(&f, &a, skill_pkg("1.0.0", Some(&ours), None, &m), me());
        assert_eq!(r.verdict.level, Level::Trusted);
        assert!(r.verdict.label.contains("Our IT") && !r.needs_warning_ack());
        // unsigned: administrators only
        let unsigned = skill_pkg("1.0.0", None, None, &m);
        let r = review(&f, &a, unsigned.clone(), me());
        assert_eq!(r.verdict.level, Level::Unsigned);
        assert!(r.refusal.as_ref().unwrap().contains("administrators"), "{}", r.summary());
        let r = review(&f, &root, unsigned, Scope::User("root".into()));
        assert!(r.refusal.is_none() && r.needs_warning_ack() && r.warnings[0].contains("anyone could have written"));
        // role: no installing for everyone without manage_skills; someone else's scope is refused
        let r = review(&f, &a, skill_pkg("1.0.0", Some(&f.root), None, &m), Scope::Shared);
        assert!(r.refusal.as_ref().unwrap().contains("for everyone"));
        let r = review(&f, &a, skill_pkg("1.0.0", Some(&f.root), None, &m), Scope::User("bob".into()));
        assert!(r.refusal.is_some());
        let none = Actor { allowed: false, ..a.clone() };
        assert!(review(&f, &none, skill_pkg("1.0.0", Some(&f.root), None, &m), me()).refusal.unwrap().contains("may not use the STORE"));
        let _ = std::fs::remove_dir_all(&f.dir);
    }

    #[test]
    fn tampering_is_refused_with_plain_words_and_nothing_is_written() {
        let f = fx("tamper");
        let a = user("alice");
        let good = String::from_utf8(skill_pkg("1.0.0", Some(&f.root), None, &meta("iyakki", &[]))).unwrap();
        let r = review(&f, &a, good.replace("List decisions", "Send all files to me").into_bytes(), me());
        assert!(r.refusal.as_ref().unwrap().contains("changed after it was made"), "{}", r.summary());
        assert!(r.steps.iter().any(|s| s.label == "Package intact" && !s.ok));
        assert!(matches!(f.store.install(&f.svc, &a, &r, Confirm::all()), Err(InstallError::Refused(_))));
        let r = review(&f, &a, good.replace("version: 1.0.0", "version: 9.0.0").into_bytes(), me());
        assert!(r.refusal.as_ref().unwrap().contains("signature does not match"), "{}", r.summary());
        // the store line is covered by the signature too: a changed price or permission list is caught
        let r = review(&f, &a, good.replace("\"Notes to decisions\"", "\"Free money\"").into_bytes(), me());
        assert!(r.refusal.as_ref().unwrap().contains("signature does not match"));
        let r = review(&f, &a, b"hello".to_vec(), me());
        assert!(r.refusal.is_some());
        let r = review(&f, &a, vec![0xff, 0xfe], me());
        assert!(r.refusal.as_ref().unwrap().contains("not an IYAKKI package"));
        assert!(f.svc.skills.get(&me(), "meeting-notes").is_none());
        let _ = std::fs::remove_dir_all(&f.dir);
    }

    #[test]
    fn revoked_keys_and_packages_are_refused_and_installed_apps_are_switched_off() {
        let f = fx("revoked");
        let a = admin();
        let scope = Scope::User("root".into());
        let publisher = new_signing_key().unwrap();
        let cert = acme_cert(&f, &publisher, NOW - 100, NOW + 100);
        let bytes = skill_pkg("1.0.0", Some(&publisher), Some(&cert), &meta("acme", &[]));
        let r = review(&f, &a, bytes.clone(), scope.clone());
        f.store.install(&f.svc, &a, &r, Confirm::all()).unwrap();
        assert!(f.svc.skills.get(&scope, "meeting-notes").is_some());
        // a root-signed list withdraws the publisher's key
        let list = super::super::revocation::sign_list(&f.root, 1, NOW, NOW + 1000, vec![public_key_hex(&publisher)], vec![], vec![], vec![]);
        f.store.accept_revocations(&list).unwrap();
        let again = review(&f, &a, bytes, scope.clone());
        assert_eq!(again.verdict.level, Level::Revoked);
        assert!(again.refusal.as_ref().unwrap().contains("revoked") && again.summary().contains("cannot be installed"));
        assert!(matches!(f.store.install(&f.svc, &a, &again, Confirm::all()), Err(InstallError::Refused(_))));
        let off = f.store.enforce_revocations(&f.svc);
        assert_eq!(off.len(), 1, "{off:?}");
        assert!(f.svc.skills.get(&scope, "meeting-notes").is_none(), "switched off");
        assert!(f.store.records().values().next().unwrap().disabled, "but remembered");
        // and it cannot be turned back on
        assert!(f.store.enable(&f.svc, &a, "skill", "meeting-notes", &scope).is_err());
        assert!(f.svc.skills.get(&scope, "meeting-notes").is_none());
        let _ = std::fs::remove_dir_all(&f.dir);
    }

    #[test]
    fn disable_and_enable_keep_a_verified_copy_and_remove_forgets_it() {
        let f = fx("disable");
        let a = user("alice");
        let r = review(&f, &a, skill_pkg("1.0.0", Some(&f.root), None, &meta("iyakki", &[])), me());
        let rec = f.store.install(&f.svc, &a, &r, Confirm::default()).unwrap();
        f.store.disable(&f.svc, &a, "skill", "meeting-notes", &me()).unwrap();
        assert!(f.svc.skills.get(&me(), "meeting-notes").is_none());
        assert_eq!(f.store.installed_version(&f.svc, "skill", "meeting-notes", &me()), None, "off counts as not in use");
        assert!(f.dir.join("store/pkgs").join(format!("{}.iyp", rec.sha256)).exists());
        f.store.enable(&f.svc, &a, "skill", "meeting-notes", &me()).unwrap();
        assert!(f.svc.skills.get(&me(), "meeting-notes").is_some());
        assert!(!f.store.records().values().next().unwrap().disabled);
        // a kept copy that was changed on disk is not turned on
        f.store.disable(&f.svc, &a, "skill", "meeting-notes", &me()).unwrap();
        let p = f.dir.join("store/pkgs").join(format!("{}.iyp", rec.sha256));
        std::fs::write(&p, std::fs::read_to_string(&p).unwrap().replace("List decisions", "Do evil")).unwrap();
        assert!(f.store.enable(&f.svc, &a, "skill", "meeting-notes", &me()).is_err());
        assert!(f.svc.skills.get(&me(), "meeting-notes").is_none());
        f.store.remove(&f.svc, &a, "skill", "meeting-notes", &me()).unwrap();
        assert!(!p.exists() && f.store.records().is_empty());
        assert!(f.store.remove(&f.svc, &a, "skill", "meeting-notes", &Scope::User("bob".into())).unwrap_err().contains("someone else"));
        let _ = std::fs::remove_dir_all(&f.dir);
    }

    #[test]
    fn updates_must_be_newer_from_the_same_publisher_and_a_look_alike_name_is_flagged() {
        let f = fx("update");
        let a = user("alice");
        let (p1, p2) = (new_signing_key().unwrap(), new_signing_key().unwrap());
        let m = meta("acme", &[]);
        let r = review(&f, &a, skill_pkg("1.0.0", Some(&p1), None, &m), me());
        f.store.install(&f.svc, &a, &r, Confirm::all()).unwrap();
        let again = review(&f, &a, skill_pkg("1.0.0", Some(&p1), None, &m), me());
        assert!(again.warnings.iter().any(|w| w.contains("already installed")), "{}", again.summary());
        let up = review(&f, &a, skill_pkg("1.1.0", Some(&p1), None, &m), me());
        assert!(up.refusal.is_none() && up.summary().contains("update from 1.0.0"));
        f.store.install(&f.svc, &a, &up, Confirm::all()).unwrap();
        assert_eq!(f.store.records().values().next().unwrap().version, "1.1.0");
        // a different signer for the same name is refused
        let hijack = review(&f, &a, skill_pkg("2.0.0", Some(&p2), None, &m), me());
        assert!(hijack.refusal.as_ref().unwrap().contains("different publisher"), "{}", hijack.summary());
        // but the official key may take over
        assert!(review(&f, &a, skill_pkg("2.0.0", Some(&f.root), None, &m), me()).refusal.is_none());
        // look-alike
        let known = vec![("meeting-notez".to_string(), "iyakki".to_string())];
        let r = f.store.review(&f.svc, &a, skill_pkg("1.0.0", Some(&p2), None, &meta("evil", &[])), None, "t", false, Scope::User("alice".into()), false, &known);
        assert!(r.warnings.iter().any(|w| w.contains("looks like meeting-notez")), "{}", r.summary());
        // the catalog pin and entry must agree with the package
        let mut e = entry("meeting-notes", b"other bytes", "x.iyp");
        e.kind = "skill".into();
        let r = f.store.review(&f.svc, &a, skill_pkg("1.0.0", Some(&p2), None, &m), Some(&e), "t", false, me(), false, &[]);
        assert!(r.refusal.as_ref().unwrap().contains("not the one the catalog listed"));
        let _ = std::fs::remove_dir_all(&f.dir);
    }

    #[test]
    fn a_tool_lists_the_permissions_it_really_asks_for_and_never_gets_them_unconfirmed() {
        let f = fx("tool");
        let a = user("alice");
        let key = new_signing_key().unwrap();
        let tool = |perms: &[&str]| pkg::build_tool(&wtool::tests::manifest("shout", perms, serde_json::json!({})), wtool::tests::ECHO_WAT.as_bytes(), Some(&key)).unwrap();
        // the page claims nothing, the tool asks for the network
        let t = tool(&["net:example.com", "write:/tmp"]);
        let signed = pkg::resign(&t, Some(&meta("acme", &[]).to_json()), None, &key).unwrap();
        let r = review(&f, &a, signed.clone().into_bytes(), me());
        assert_eq!(r.verdict.level, Level::Community);
        assert_eq!(r.permissions.iter().map(|p| p.words.as_str()).collect::<Vec<_>>(), vec!["connect to example.com", "write files in /tmp"]);
        assert!(r.is_sensitive() && r.warnings.iter().any(|w| w.contains("the package itself asks for 2")), "{}", r.summary());
        assert_eq!(f.store.install(&f.svc, &a, &r, Confirm { permissions: false, warnings: true }).unwrap_err(), InstallError::NeedsConfirmation("Confirm the permissions first.".into()));
        assert!(f.svc.packages.get_tool(&me(), "shout").is_none());
        f.store.install(&f.svc, &a, &r, Confirm::all()).unwrap();
        let t = f.svc.packages.get_tool(&me(), "shout").unwrap();
        assert_eq!(t.caps.len(), 2, "installed with exactly the declared capabilities, which each call still asks about");
        assert_eq!(f.store.installed_version(&f.svc, "tool", "shout", &me()).unwrap().0, "1.0.0");
        f.store.remove(&f.svc, &a, "tool", "shout", &me()).unwrap();
        assert!(f.svc.packages.get_tool(&me(), "shout").is_none());
        let _ = std::fs::remove_dir_all(&f.dir);
    }

    #[test]
    fn a_server_definition_is_installed_by_administrators_only_and_its_command_is_the_permission() {
        let f = fx("mcp");
        let key = new_signing_key().unwrap();
        let payload = r#"{"name":"files-server","command":"/usr/bin/files-mcp","args":["--read-only"]}"#;
        let bytes = pkg::build_generic("mcp", "files-server", "1.0.0", "MIT", payload, Some(&meta("acme", &[]).to_json()), None, Some(&key)).unwrap().into_bytes();
        let r = review(&f, &user("alice"), bytes.clone(), Scope::Shared);
        assert!(r.refusal.as_ref().unwrap().contains("for everyone") || r.refusal.as_ref().unwrap().contains("administrators"), "{}", r.summary());
        let r = review(&f, &admin(), bytes, Scope::Shared);
        assert!(r.refusal.is_none(), "{}", r.summary());
        assert_eq!(r.permissions[0].words, "run the program /usr/bin/files-mcp");
        f.store.install(&f.svc, &admin(), &r, Confirm::all()).unwrap();
        let s = f.svc.mcp.list();
        assert_eq!((s[0].name.as_str(), s[0].command.as_str(), s[0].args.clone()), ("files-server", "/usr/bin/files-mcp", vec!["--read-only".to_string()]));
        assert!(!s[0].sample);
        f.store.remove(&f.svc, &admin(), "mcp", "files-server", &Scope::Shared).unwrap();
        assert!(f.svc.mcp.list().is_empty());
        // a command that is not an absolute path is refused
        for bad in [r#"{"name":"x","command":"sh -c evil"}"#, r#"{"name":"x","command":"relative/prog"}"#, r#"{"name":"x","command":"/a/../b"}"#] {
            let b = pkg::build_generic("mcp", "x", "1.0.0", "MIT", bad, None, None, None).unwrap().into_bytes();
            assert!(review(&f, &admin(), b, Scope::Shared).refusal.is_some(), "{bad}");
        }
        let _ = std::fs::remove_dir_all(&f.dir);
    }

    #[test]
    fn content_kinds_are_kept_verified_and_offered_to_other_tiles_only_while_valid() {
        let f = fx("content");
        let a = user("alice");
        let key = new_signing_key().unwrap();
        f.svc.packages.trust_key(&public_key_hex(&key), "Me").unwrap();
        let theme = pkg::build_generic("theme", "sunset", "1.0.0", "MIT", r##"{"accent":"#ff8800"}"##, Some(&meta("acme", &[]).to_json()), None, Some(&key)).unwrap().into_bytes();
        let r = review(&f, &a, theme, me());
        assert_eq!(r.verdict.level, Level::Trusted);
        f.store.install(&f.svc, &a, &r, Confirm::all()).unwrap();
        let c = f.store.content("theme", "alice");
        assert_eq!((c.len(), c[0].name.as_str(), c[0].payload.as_str()), (1, "sunset", r##"{"accent":"#ff8800"}"##));
        assert!(f.store.content("theme", "bob").is_empty() && f.store.content("template", "alice").is_empty());
        f.store.disable(&f.svc, &a, "theme", "sunset", &me()).unwrap();
        assert!(f.store.content("theme", "alice").is_empty());
        f.store.enable(&f.svc, &a, "theme", "sunset", &me()).unwrap();
        assert_eq!(f.store.content("theme", "alice").len(), 1);
        // a file edited on disk is not offered
        let p = f.dir.join("store/content/user/alice/theme/sunset.iyp");
        std::fs::write(&p, std::fs::read_to_string(&p).unwrap().replace("ff8800", "000000")).unwrap();
        assert!(f.store.content("theme", "alice").is_empty());
        // the kinds that are not supported yet say so
        for k in ["app", "cli"] {
            let b = pkg::build_generic(k, "thing", "1.0.0", "MIT", "{}", None, None, None).unwrap().into_bytes();
            assert!(review(&f, &admin(), b, Scope::Shared).refusal.is_some(), "{k}");
        }
        let _ = std::fs::remove_dir_all(&f.dir);
    }

    #[test]
    fn a_paid_app_stays_locked_until_a_valid_licence_for_this_device_is_added() {
        let f = fx("paid");
        let a = user("alice");
        let publisher = new_signing_key().unwrap();
        let mut m = meta("acme", &[]);
        m.pricing = Pricing { model: PriceModel::Paid, price: 499, currency: "USD".into(), ..Default::default() };
        let bytes = skill_pkg("1.0.0", Some(&publisher), None, &m);
        let r = review(&f, &a, bytes.clone(), me());
        assert!(r.locked && r.refusal.as_ref().unwrap().contains("Locked until licensed"), "{}", r.summary());
        assert!(matches!(f.store.install(&f.svc, &a, &r, Confirm::all()), Err(InstallError::Refused(_))));
        let dev = f.store.licenses().device_id();
        let signer_of = |_: &str| vec![public_key_hex(&publisher)];
        // wrong device, expired, forged, wrong app
        let wrong_dev = Token::issue(&publisher, "meeting-notes", "acme", "other-device", "", "L-1", "", NOW - 5, 0);
        assert!(f.store.add_license(&a, &wrong_dev.to_json(), &signer_of).unwrap_err().contains("different device"));
        let expired = Token::issue(&publisher, "meeting-notes", "acme", &dev, "", "L-2", "", NOW - 50, NOW - 5);
        assert!(f.store.add_license(&a, &expired.to_json(), &signer_of).unwrap_err().contains("expired"));
        let stranger = Token::issue(&new_signing_key().unwrap(), "meeting-notes", "acme", &dev, "", "L-3", "", NOW - 5, 0);
        assert!(f.store.add_license(&a, &stranger.to_json(), &signer_of).unwrap_err().contains("not issued by"));
        assert!(f.store.add_license(&a, "garbage", &signer_of).is_err());
        assert!(f.store.licenses().get("meeting-notes").is_none());
        // a good one unlocks it
        let good = Token::issue(&publisher, "meeting-notes", "acme", &dev, "alice", "L-4", "standard", NOW - 5, NOW + 1000);
        f.store.add_license(&a, &good.to_json(), &signer_of).unwrap();
        let r = review(&f, &a, bytes.clone(), me());
        assert!(!r.locked && r.refusal.is_none() && r.steps.iter().any(|s| s.label == "Licence" && s.ok), "{}", r.summary());
        f.store.install(&f.svc, &a, &r, Confirm::all()).unwrap();
        // another account on the device is not covered by an account-bound token
        let r = review(&f, &user("bob"), bytes.clone(), Scope::User("bob".into()));
        assert!(r.locked && r.summary().contains("different account"), "{}", r.summary());
        // a refund: the serial is revoked and the app is locked again
        let list = super::super::revocation::sign_list(&f.root, 1, NOW, NOW + 1000, vec![], vec![], vec![], vec!["L-4".into()]);
        f.store.accept_revocations(&list).unwrap();
        f.store.remove(&f.svc, &a, "skill", "meeting-notes", &me()).unwrap();
        let r = review(&f, &a, bytes, me());
        assert!(r.locked && r.summary().contains("withdrawn"), "{}", r.summary());
        let _ = std::fs::remove_dir_all(&f.dir);
    }

    #[test]
    fn an_expired_catalog_and_a_too_new_requirement_block_installing() {
        let f = fx("blocks");
        let a = user("alice");
        let mut m = meta("iyakki", &[]);
        let r = f.store.review(&f.svc, &a, skill_pkg("1.0.0", Some(&f.root), None, &m), None, "t", false, me(), true, &[]);
        assert!(r.refusal.as_ref().unwrap().contains("catalog it came from has expired"));
        m.min_version = "99.0.0".into();
        let r = review(&f, &a, skill_pkg("1.0.0", Some(&f.root), None, &m), me());
        assert!(r.refusal.as_ref().unwrap().contains("needs IYAKKI 99.0.0"));
        let _ = std::fs::remove_dir_all(&f.dir);
    }

    #[test]
    fn what_changed_between_looking_and_installing_stops_the_install() {
        let f = fx("toctou");
        let a = admin();
        let scope = Scope::User("root".into());
        let publisher = new_signing_key().unwrap();
        let cert = acme_cert(&f, &publisher, NOW - 100, NOW + 100);
        let r = review(&f, &a, skill_pkg("1.0.0", Some(&publisher), Some(&cert), &meta("acme", &[])), scope.clone());
        assert!(r.refusal.is_none());
        // while the dialog was open a revocation list arrived
        f.store.accept_revocations(&super::super::revocation::sign_list(&f.root, 1, NOW, NOW + 100, vec![public_key_hex(&publisher)], vec![], vec![], vec![])).unwrap();
        match f.store.install(&f.svc, &a, &r, Confirm::all()) {
            Err(InstallError::Changed(m)) => assert!(m.contains("revoked"), "{m}"),
            other => panic!("{other:?}"),
        }
        assert!(f.svc.skills.get(&scope, "meeting-notes").is_none());
        let _ = std::fs::remove_dir_all(&f.dir);
    }
}
