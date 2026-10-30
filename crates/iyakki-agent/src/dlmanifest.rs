//! The downloads manifest: the version and checksum data of the items IYAKKI downloads at the person's click (docs/development/DOWNLOADS-MANIFEST.md).
//!
//! * `legal/downloads-manifest.json` is built into the program as the offline fallback and as the **allow-list**: it names every item, its
//!   licence, its flags and the hosts it may be fetched from.
//! * A newer manifest (fetched later, signed with the owner's key; the public key is built in) may only change the `version`, `url` and
//!   `checksum` of items the bundled one already has. It can never add an item, change a licence or a flag, add a host, or make an item
//!   of another kind. The signature is a detached ed25519 signature over the exact bytes of the file (no canonical form to get wrong).
//! * An older or equal-but-different manifest is refused (rollback protection); the last good one is kept in one file, replaced atomically.
//!
//! This module does no network access. Nothing here ever holds a private key: `sign` reads it from a path given at run time.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use ed25519_dalek::{Signature, Signer, VerifyingKey};
use serde::{Deserialize, Serialize};

use crate::pkg::{hex, signing_key_from_hex, unhex};

/// The manifest bundled in the program (the ISO's offline copy and the allow-list).
pub const BUNDLED: &str = include_str!("../../../legal/downloads-manifest.json");
/// The owner's public key (64 hex characters), or `unset` until the owner has made a key (`iyakki dlmanifest keygen`).
pub const PUBKEY_FILE: &str = include_str!("../../../legal/downloads-manifest.pub");
/// The largest manifest read from anywhere.
pub const MAX_BYTES: usize = 1 << 20;
const SCHEMA: u32 = 1;
/// Items whose lookup already goes through the manifest (the others still use their constants in the code; see the migration backlog).
pub const WIRED: &[&str] = &["cli:kubectl", "cli:helm"];
/// Items that are proprietary vendor software: the flag is part of the bundled data and a fetched manifest cannot change it.
pub const PROPRIETARY: &[&str] = &["cli:claude-code", "cli:github-copilot", "mac-firmware", "mac-camera"];

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Manifest {
    pub schema: u32,
    /// Grows with every published manifest; a manifest below the one held is refused.
    pub sequence: u64,
    /// `YYYY-MM-DD`.
    pub issued: String,
    pub items: Vec<Item>,
}

#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    /// The version floats (newest when downloaded); the vendor's checksum file is verified at every download.
    #[serde(rename = "float+sum")]
    FloatSum,
    /// An exact version, no checksum from the vendor (a checksum of our own may be added, with its date and source).
    #[serde(rename = "none-vendor")]
    NoneVendor,
    /// Nothing to pin in IYAKKI's data (per-file hashes in the code, the system's package manager...).
    #[serde(rename = "no-pin")]
    NoPin,
}

#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum SumFormat {
    /// A file holding only the hash (or `hash  name`).
    Single,
    /// A file of `hash  name` lines.
    Lines,
    /// A JSON manifest of the vendor with the checksum per platform.
    Manifest,
    /// An MD5 file (the only digest some vendors publish).
    Md5,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct FileSum {
    pub file: String,
    pub sha256: String,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
#[serde(tag = "type", rename_all = "kebab-case")]
pub enum Checksum {
    /// The vendor's own checksum file, fetched from `url` at every download (preferred).
    VendorFile { url: String, format: SumFormat },
    /// Our own SHA-256 per file, only where the vendor publishes none; `date` and `source` say when and where it was taken.
    Sha256 { files: Vec<FileSum>, date: String, source: String },
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Item {
    pub id: String,
    /// Other `legal/downloads.txt` rows this item stands for (the Quick install row of the same program).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub covers: Vec<String>,
    pub licence: String,
    pub proprietary: bool,
    /// Always true: nothing in this list is ever part of an image or an export.
    pub downloaded_not_shipped: bool,
    pub kind: Kind,
    /// Empty = floats (follow the vendor) for a `float+sum` item.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub version: String,
    /// The download address template (`{v}`, `{arch}`... as in the code); empty = not set by the manifest.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub url: String,
    /// The hosts (and their sub-domains) the url and the checksum file may name. Fixed by the bundled copy.
    #[serde(default)]
    pub hosts: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub checksum: Option<Checksum>,
}

fn is_date(s: &str) -> bool {
    let b = s.as_bytes();
    b.len() == 10 && b[4] == b'-' && b[7] == b'-' && b.iter().enumerate().all(|(i, c)| i == 4 || i == 7 || c.is_ascii_digit())
}

fn plain(s: &str, max: usize) -> bool {
    !s.is_empty() && s.len() <= max && s.chars().all(|c| !c.is_control())
}

fn id_ok(s: &str) -> bool {
    !s.is_empty() && s.len() <= 48 && s.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || matches!(c, ':' | '_' | '.' | '-'))
}

fn version_ok(s: &str) -> bool {
    s.len() <= 40 && s.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '+' | '-' | '~'))
}

fn host_name_ok(s: &str) -> bool {
    !s.is_empty() && s.len() <= 64 && s.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || matches!(c, '.' | '-')) && !s.starts_with('.') && !s.ends_with('.')
}

fn sha_ok(s: &str) -> bool {
    s.len() == 64 && s.chars().all(|c| matches!(c, '0'..='9' | 'a'..='f'))
}

/// `https://host/...` on one of `hosts` (the host or a sub-domain of it). No user info, no ports, nothing but printable ASCII.
pub fn url_on_hosts(url: &str, hosts: &[String]) -> Result<(), String> {
    if url.len() > 400 || !url.is_ascii() || url.chars().any(|c| c.is_control() || c == ' ' || c == '\\' || c == '@') {
        return Err(format!("`{}` is not a plain address", url.chars().take(60).collect::<String>()));
    }
    let rest = url.strip_prefix("https://").ok_or_else(|| format!("`{url}` is not an https address"))?;
    let host = rest.split(['/', '?', '#']).next().unwrap_or("");
    // the one host placeholder the code knows: it becomes nodejs.org or the builder for musl systems (clis.rs `template_for`)
    if host == "{node_dist}" && hosts.iter().any(|h| h == "nodejs.org") {
        return Ok(());
    }
    if host.contains([':', '{', '}']) || !host_name_ok(host) {
        return Err(format!("`{url}`: the host part is not a plain host name"));
    }
    if hosts.iter().any(|h| host == h || host.ends_with(&format!(".{h}"))) {
        Ok(())
    } else {
        Err(format!("`{url}`: {host} is not one of this item's hosts"))
    }
}

/// The rules every manifest must meet by itself (the bundled one and a fetched one).
pub fn validate(m: &Manifest) -> Result<(), String> {
    if m.schema != SCHEMA {
        return Err(format!("schema {} is not understood (this program reads {SCHEMA})", m.schema));
    }
    if m.sequence == 0 || !is_date(&m.issued) {
        return Err("the sequence number (from 1) and the issue date (YYYY-MM-DD) are required".into());
    }
    if m.items.is_empty() || m.items.len() > 200 {
        return Err("a manifest holds 1 to 200 items".into());
    }
    let mut ids = BTreeSet::new();
    for it in &m.items {
        let at = format!("item `{}`", it.id);
        if !id_ok(&it.id) {
            return Err(format!("{at}: the id is not plain"));
        }
        for c in std::iter::once(&it.id).chain(it.covers.iter()) {
            if !id_ok(c) || !ids.insert(c.clone()) {
                return Err(format!("{at}: `{c}` is not a plain id or appears twice"));
            }
        }
        if !plain(&it.licence, 120) {
            return Err(format!("{at}: the licence is missing or not plain text"));
        }
        if !it.downloaded_not_shipped {
            return Err(format!("{at}: downloaded_not_shipped must be true (nothing in the list may be shipped)"));
        }
        if it.hosts.len() > 12 || it.hosts.iter().any(|h| !host_name_ok(h)) {
            return Err(format!("{at}: the host list is not plain"));
        }
        if !version_ok(&it.version) {
            return Err(format!("{at}: the version `{}` has characters a version does not", it.version.chars().take(40).collect::<String>()));
        }
        if !it.url.is_empty() {
            url_on_hosts(&it.url, &it.hosts).map_err(|e| format!("{at}: {e}"))?;
        }
        match (&it.kind, &it.checksum) {
            (Kind::FloatSum, Some(Checksum::VendorFile { .. })) | (Kind::NoneVendor, None) | (Kind::NoneVendor, Some(Checksum::Sha256 { .. })) | (Kind::NoPin, None) => {}
            (k, c) => return Err(format!("{at}: kind {k:?} does not go with checksum {}", if c.is_some() { "of that type" } else { "missing" })),
        }
        match &it.checksum {
            Some(Checksum::VendorFile { url, .. }) => url_on_hosts(url, &it.hosts).map_err(|e| format!("{at}: checksum file {e}"))?,
            Some(Checksum::Sha256 { files, date, source }) => {
                if files.is_empty() || files.len() > 64 || !is_date(date) || !plain(source, 200) {
                    return Err(format!("{at}: a sha256 checksum needs 1 to 64 files, the date it was taken and its source"));
                }
                if it.version.is_empty() {
                    return Err(format!("{at}: a sha256 checksum belongs to one exact version"));
                }
                if files.iter().any(|f| !sha_ok(&f.sha256) || !plain(&f.file, 120) || f.file.contains('/')) {
                    return Err(format!("{at}: a file name or sha256 value is not plain (64 lower-case hex characters)"));
                }
            }
            None => {}
        }
    }
    Ok(())
}

/// The rules for a manifest that is not the bundled one: against the bundled copy it may change `version`, `url` and `checksum` of
/// existing items, and nothing else.
pub fn validate_against(bundled: &Manifest, m: &Manifest) -> Result<(), String> {
    validate(m)?;
    for it in &m.items {
        let at = format!("item `{}`", it.id);
        let Some(b) = bundled.items.iter().find(|b| b.id == it.id) else {
            return Err(format!("{at} is not in this program's list: a manifest cannot add an item"));
        };
        if it.covers != b.covers || it.licence != b.licence || it.proprietary != b.proprietary || it.downloaded_not_shipped != b.downloaded_not_shipped || it.kind != b.kind {
            return Err(format!("{at}: id, licence, flags and kind are fixed by this program and cannot be changed"));
        }
        if it.hosts != b.hosts {
            return Err(format!("{at}: the host list is fixed by this program"));
        }
        if b.url.is_empty() != it.url.is_empty() {
            return Err(format!("{at}: a manifest cannot add or remove a download address"));
        }
        match (&b.checksum, &it.checksum) {
            (Some(Checksum::VendorFile { .. }), Some(Checksum::VendorFile { .. })) | (None, None) | (None, Some(Checksum::Sha256 { .. })) | (Some(Checksum::Sha256 { .. }), Some(Checksum::Sha256 { .. })) => {}
            _ => return Err(format!("{at}: the checksum may change its address or value but not its kind (a vendor file is never replaced by our own value)")),
        }
    }
    Ok(())
}

/// The bundled manifest, parsed and validated (it is part of the program: a failure here is a build mistake and a test fails).
pub fn bundled() -> Manifest {
    parse(BUNDLED.as_bytes()).and_then(|m| validate(&m).map(|_| m)).expect("legal/downloads-manifest.json is valid (cargo test -p iyakki-agent dlmanifest)")
}

pub fn parse(bytes: &[u8]) -> Result<Manifest, String> {
    if bytes.len() > MAX_BYTES {
        return Err("the manifest is too large".into());
    }
    serde_json::from_slice(bytes).map_err(|e| format!("the manifest is malformed: {e}"))
}

/// The key built into the program, if the owner has made one.
#[cfg(not(test))]
pub fn builtin_key() -> Option<VerifyingKey> {
    key_in(PUBKEY_FILE)
}

/// In tests the built-in key is a throwaway one made from a fixed seed in the test file (never a real key).
#[cfg(test)]
pub fn builtin_key() -> Option<VerifyingKey> {
    Some(tests::test_signing_key().verifying_key())
}

/// The key named in the text of `legal/downloads-manifest.pub` (`None` while it says `unset`).
pub fn key_in(text: &str) -> Option<VerifyingKey> {
    key_from_hex(text.lines().find(|l| !l.trim().is_empty() && !l.starts_with('#'))?.trim())
}

pub fn key_from_hex(s: &str) -> Option<VerifyingKey> {
    unhex::<32>(s.trim()).and_then(|b| VerifyingKey::from_bytes(&b).ok())
}

/// `sig_hex` (128 hex characters) must be the ed25519 signature of exactly `bytes` by `key`.
pub fn verify_signature(key: &VerifyingKey, bytes: &[u8], sig_hex: &str) -> Result<(), String> {
    let sig = unhex::<64>(sig_hex.trim()).map(|b| Signature::from_bytes(&b)).ok_or("the signature is not 128 hexadecimal characters")?;
    key.verify_strict(bytes, &sig).map_err(|_| "the signature does not match this manifest and key".to_string())
}

/// The signature of `bytes` for a manifest file (the signer reads the key from a path given at run time; see `cli_main`).
pub fn sign_bytes(key: &ed25519_dalek::SigningKey, bytes: &[u8]) -> String {
    hex(&key.sign(bytes).to_bytes())
}

/// What a fetched manifest did.
#[derive(Debug, PartialEq, Eq)]
pub enum Applied {
    /// Accepted and saved; the ids of the items whose version, address or checksum differ from before.
    Updated { sequence: u64, changed: Vec<String> },
    /// The same manifest as the one held.
    Unchanged,
}

/// The bundled manifest with the mutable parts of `fetched` laid over it.
pub fn merge(bundled: &Manifest, fetched: &Manifest) -> Manifest {
    let mut out = bundled.clone();
    out.sequence = fetched.sequence;
    out.issued = fetched.issued.clone();
    for it in &mut out.items {
        if let Some(f) = fetched.items.iter().find(|f| f.id == it.id) {
            it.version = f.version.clone();
            it.url = f.url.clone();
            it.checksum = f.checksum.clone();
        }
    }
    out
}

/// The ids whose version, address or checksum differ.
pub fn changed(a: &Manifest, b: &Manifest) -> Vec<String> {
    b.items.iter().filter(|n| a.items.iter().find(|o| o.id == n.id).is_none_or(|o| o.version != n.version || o.url != n.url || o.checksum != n.checksum)).map(|n| n.id.clone()).collect()
}

/// Keeps the last good fetched manifest in one file under `dir` (`<data>/cli/dlmanifest` for the CLI tile).
/// File format: the first line is the signature in hex, the rest is the manifest exactly as signed.
pub struct Store {
    dir: PathBuf,
    key: Option<VerifyingKey>,
    bundled: Manifest,
}

impl Store {
    pub fn new(dir: &Path) -> Store {
        Store { dir: dir.into(), key: builtin_key(), bundled: bundled() }
    }

    /// For tests: another key and another bundled copy.
    pub fn with(dir: &Path, key: Option<VerifyingKey>, bundled: Manifest) -> Store {
        Store { dir: dir.into(), key, bundled }
    }

    fn file(&self) -> PathBuf {
        self.dir.join("last-good.dlm")
    }

    /// The saved manifest, checked again as if it had just arrived; anything wrong with it and it is ignored (the bundled copy is used).
    fn saved(&self) -> Option<(Manifest, Vec<u8>)> {
        let raw = std::fs::read(self.file()).ok()?;
        let nl = raw.iter().position(|&b| b == b'\n')?;
        let (sig, body) = (std::str::from_utf8(&raw[..nl]).ok()?, &raw[nl + 1..]);
        verify_signature(self.key.as_ref()?, body, sig).ok()?;
        let m = parse(body).ok()?;
        validate_against(&self.bundled, &m).ok()?;
        Some((m, body.to_vec()))
    }

    /// What the program uses now: the bundled copy with the last good manifest over it, if that is newer.
    pub fn active(&self) -> Manifest {
        match self.saved() {
            Some((m, _)) if m.sequence > self.bundled.sequence => merge(&self.bundled, &m),
            _ => self.bundled.clone(),
        }
    }

    /// Check a fetched manifest and its signature; when it is good and newer, keep it as the last good one.
    pub fn apply(&self, bytes: &[u8], sig_hex: &str) -> Result<Applied, String> {
        let key = self.key.as_ref().ok_or("this program has no manifest key built in, so no fetched manifest is accepted")?;
        if bytes.len() > MAX_BYTES {
            return Err("the manifest is too large".into());
        }
        verify_signature(key, bytes, sig_hex)?;
        let m = parse(bytes)?;
        validate_against(&self.bundled, &m)?;
        let now = self.active();
        let held = self.saved();
        if held.as_ref().is_some_and(|(h, hb)| h.sequence == m.sequence && hb == bytes) || (held.is_none() && m.sequence == self.bundled.sequence && merge(&self.bundled, &m) == self.bundled) {
            return Ok(Applied::Unchanged);
        }
        // the floor is the highest sequence this program has seen: the bundled copy or the saved one
        let floor = self.bundled.sequence.max(held.as_ref().map_or(0, |(h, _)| h.sequence));
        if m.sequence < floor {
            return Err(format!("sequence {} is older than the {floor} already held: refused (rollback)", m.sequence));
        }
        if m.sequence == floor {
            return Err(format!("sequence {} is already used by a different manifest: refused", m.sequence));
        }
        if m.issued < now.issued {
            return Err(format!("issue date {} is earlier than {} already held: refused (rollback)", m.issued, now.issued));
        }
        let next = merge(&self.bundled, &m);
        let diff = changed(&now, &next);
        self.save(bytes, sig_hex.trim())?;
        Ok(Applied::Updated { sequence: m.sequence, changed: diff })
    }

    fn save(&self, body: &[u8], sig: &str) -> Result<(), String> {
        use std::io::Write;
        std::fs::create_dir_all(&self.dir).map_err(|e| e.to_string())?;
        let tmp = self.dir.join(format!("last-good.dlm.tmp{}", std::process::id()));
        let r = (|| -> std::io::Result<()> {
            let mut f = std::fs::File::create(&tmp)?;
            f.write_all(sig.as_bytes())?;
            f.write_all(b"\n")?;
            f.write_all(body)?;
            f.sync_all()?;
            std::fs::rename(&tmp, self.file())?;
            if let Ok(d) = std::fs::File::open(&self.dir) {
                let _ = d.sync_all();
            }
            Ok(())
        })();
        if r.is_err() {
            let _ = std::fs::remove_file(&tmp);
        }
        r.map_err(|e| format!("could not save the manifest: {e}"))
    }
}

/// The manifest's version for `id`, for a lookup that goes through the manifest: only for [`WIRED`] items, only when a fetched manifest
/// named one (the bundled copy leaves floating items empty), and only a plain version string. `None` = use the constants in the code.
pub fn version_override(dir: &Path, id: &str) -> Option<String> {
    version_override_with(&Store::new(dir), id)
}

pub fn version_override_with(s: &Store, id: &str) -> Option<String> {
    let key = format!("cli:{id}");
    if !WIRED.contains(&key.as_str()) {
        return None;
    }
    let it = s.active().items.into_iter().find(|i| i.id == key)?;
    (!it.version.is_empty() && version_ok(&it.version)).then_some(it.version)
}

// ---- the bundled data, made from the code (for `iyakki dlmanifest gen` and the drift test)

/// One `legal/downloads.txt` row: (id, licence, pin kind, pin, date).
pub fn download_rows(text: &str) -> Vec<(String, String, String, String, String)> {
    text.lines()
        .map(str::trim)
        .filter(|l| !l.is_empty() && !l.starts_with('#'))
        .filter_map(|l| {
            let f: Vec<&str> = l.split('|').map(str::trim).collect();
            (f.len() == 7).then(|| (f[0].to_string(), f[1].to_string(), f[3].to_string(), f[4].to_string(), f[6].to_string()))
        })
        .collect()
}

/// The pin kinds that `licence-check` lists as REVIEW.
pub fn is_review(kind: &str) -> bool {
    matches!(kind, "checksum" | "none-vendor" | "floating")
}

/// The Quick install rows that are the same program as a CLI-tile tool.
fn quick_alias(cli_id: &str) -> Option<&'static str> {
    ["nodejs", "python", "k3s", "duckdb", "claude-code", "github-copilot", "ollama", "ffmpeg"].into_iter().find(|a| *a == cli_id)
}

/// The items of the CLI tile and the other REVIEW rows, made from the constants in the code and the rows of `legal/downloads.txt`.
/// `sequence` and `issued` are kept from the manifest being regenerated.
pub fn from_catalog(rows: &[(String, String, String, String, String)], sequence: u64, issued: &str) -> Manifest {
    use crate::clis::{Checksum as C, Source, CATALOG};
    let row = |id: &str| rows.iter().find(|r| r.0 == id);
    let mut items = Vec::new();
    for d in CATALOG {
        let id = format!("cli:{}", d.id);
        let Some(r) = row(&id).filter(|r| is_review(&r.2)) else { continue };
        let url = match d.source {
            Source::File(u) | Source::TarGz(u) | Source::Zip(u) | Source::TarZst(u) | Source::TarXz(u) => u.to_string(),
            _ => String::new(),
        };
        // a fixed list with nothing in it (an address and its SHA-256 the person supplies) is no checksum row
        let checksum = d.checksum.filter(|c| !matches!(c, C::Fixed(t) if t.is_empty())).map(|c| match c {
            C::Single(u) => Checksum::VendorFile { url: u.into(), format: SumFormat::Single },
            C::Lines(u) => Checksum::VendorFile { url: u.into(), format: SumFormat::Lines },
            C::Manifest(u) => Checksum::VendorFile { url: u.into(), format: SumFormat::Manifest },
            C::Md5(u) => Checksum::VendorFile { url: u.into(), format: SumFormat::Md5 },
            C::Fixed(t) => Checksum::Sha256 { files: t.iter().map(|(f, s)| FileSum { file: (*f).into(), sha256: (*s).into() }).collect(), date: r.4.clone(), source: "the digest field of the vendor's release assets".into() },
        });
        let kind = if checksum.is_some() && d.pin.is_empty() { Kind::FloatSum } else if d.pin.is_empty() { Kind::NoPin } else { Kind::NoneVendor };
        items.push(Item {
            covers: quick_alias(d.id).map(|a| vec![a.to_string()]).unwrap_or_default(),
            licence: r.1.clone(),
            proprietary: PROPRIETARY.contains(&id.as_str()),
            downloaded_not_shipped: true,
            kind,
            version: d.pin.to_string(),
            url,
            hosts: d.hosts.iter().map(|h| h.to_string()).collect(),
            checksum,
            id,
        });
    }
    // the REVIEW rows that are not command-line tools
    for id in ["mac-firmware", "mac-camera", "linux-windows"] {
        if let Some(r) = row(id).filter(|r| is_review(&r.2)) {
            items.push(Item { id: id.into(), covers: vec![], licence: r.1.clone(), proprietary: PROPRIETARY.contains(&id), downloaded_not_shipped: true, kind: Kind::NoPin, version: String::new(), url: String::new(), hosts: vec![], checksum: None });
        }
    }
    if let Some(r) = row("linux-apps").filter(|r| is_review(&r.2)) {
        items.push(Item {
            id: "linux-apps".into(),
            covers: vec![],
            licence: r.1.clone(),
            proprietary: false,
            downloaded_not_shipped: true,
            kind: Kind::FloatSum,
            version: String::new(),
            url: String::new(),
            hosts: ["ubuntu.com", "docker.io", "docker.com"].iter().map(|h| h.to_string()).collect(),
            checksum: Some(Checksum::VendorFile { url: "https://cdimage.ubuntu.com/ubuntu-base/releases/{v}/release/SHA256SUMS".into(), format: SumFormat::Lines }),
        });
    }
    Manifest { schema: SCHEMA, sequence, issued: issued.into(), items }
}

pub fn to_json(m: &Manifest) -> String {
    let mut s = serde_json::to_string_pretty(m).unwrap_or_default();
    s.push('\n');
    s
}

// ---- `iyakki dlmanifest ...`

const USAGE: &str = "usage:
  iyakki dlmanifest keygen FILE                   make a signing key at FILE (refuses to overwrite; keep it private and backed up, outside the repository); prints the public key
  iyakki dlmanifest sign MANIFEST --key FILE [--out SIG]
                                                  check MANIFEST against the rules, then write its signature (default MANIFEST.sig)
  iyakki dlmanifest verify MANIFEST [--sig FILE] [--pubkey HEX|FILE]
                                                  check the signature (default key: the one built in), the rules and the sequence against the bundled copy
  iyakki dlmanifest check                         check the bundled manifest and the built-in key
  iyakki dlmanifest pin MANIFEST ID --version V --source TEXT --date YYYY-MM-DD (--file LOCAL [--as NAME] | --sha256 HEX --as NAME | --pypi-json FILE)
                                                  record our own SHA-256 of a download in MANIFEST (a none-vendor item; the old values of another version are dropped).
                                                  --file hashes a file you fetched over a trusted link; --pypi-json takes every file of one PyPI release (curl https://pypi.org/pypi/NAME/VERSION/json);
                                                  repeat the command for each dependency of a Python tool. Sequence and signature are not touched: raise `sequence` and sign after
  iyakki dlmanifest gen [--root DIR]              print the bundled manifest made from the code and DIR/legal/downloads.txt (DIR defaults to .)";

fn flag(a: &[String], name: &str) -> Option<String> {
    a.iter().position(|x| x == name).and_then(|i| a.get(i + 1)).cloned()
}

fn key_arg(v: &str) -> Result<VerifyingKey, String> {
    let text = if v.len() == 64 { v.to_string() } else { std::fs::read_to_string(v).map_err(|e| format!("cannot read {v}: {e}"))? };
    key_from_hex(&text).ok_or_else(|| "the public key is not 64 hexadecimal characters".to_string())
}

fn run(a: &[String]) -> Result<String, String> {
    let sub = a.first().map(String::as_str).unwrap_or("");
    let b = bundled();
    match sub {
        "keygen" => {
            use std::io::Write;
            use std::os::unix::fs::OpenOptionsExt;
            let path = a.get(1).ok_or(USAGE)?;
            let k = crate::pkg::new_signing_key().map_err(|e| e.to_string())?;
            let mut f = std::fs::OpenOptions::new().write(true).create_new(true).mode(0o600).open(path).map_err(|e| format!("cannot create {path}: {e}"))?;
            f.write_all(format!("{}\n", crate::pkg::signing_key_hex(&k)).as_bytes()).map_err(|e| e.to_string())?;
            Ok(format!("private key written to {path} (mode 600): keep it out of the repository and back it up\npublic key (put it in legal/downloads-manifest.pub): {}", crate::pkg::public_key_hex(&k)))
        }
        "sign" => {
            let (path, key) = (a.get(1).ok_or(USAGE)?, flag(a, "--key").ok_or("--key FILE is required")?);
            let bytes = std::fs::read(path).map_err(|e| format!("cannot read {path}: {e}"))?;
            let m = parse(&bytes)?;
            validate_against(&b, &m)?;
            let k = signing_key_from_hex(&std::fs::read_to_string(&key).map_err(|e| format!("cannot read the key file: {e}"))?).map_err(|e| e.to_string())?;
            let out = flag(a, "--out").unwrap_or_else(|| format!("{path}.sig"));
            std::fs::write(&out, format!("{}\n", sign_bytes(&k, &bytes))).map_err(|e| e.to_string())?;
            Ok(format!("signed {path} (sequence {}, {} items); signature in {out}; public key {}", m.sequence, m.items.len(), crate::pkg::public_key_hex(&k)))
        }
        "verify" => {
            let path = a.get(1).ok_or(USAGE)?;
            let bytes = std::fs::read(path).map_err(|e| format!("cannot read {path}: {e}"))?;
            let sig = std::fs::read_to_string(flag(a, "--sig").unwrap_or_else(|| format!("{path}.sig"))).map_err(|e| format!("cannot read the signature: {e}"))?;
            let key = match flag(a, "--pubkey") {
                Some(v) => key_arg(&v)?,
                None => builtin_key().ok_or("no public key is built in yet (legal/downloads-manifest.pub says `unset`); pass --pubkey")?,
            };
            verify_signature(&key, &bytes, &sig)?;
            let m = parse(&bytes)?;
            validate_against(&b, &m)?;
            if m.sequence < b.sequence {
                return Err(format!("sequence {} is older than the bundled {}", m.sequence, b.sequence));
            }
            Ok(format!("OK: signature good, rules met, sequence {} (bundled {}), {} items change against the bundled copy", m.sequence, b.sequence, changed(&b, &merge(&b, &m)).len()))
        }
        "pin" => {
            let (path, id) = (a.get(1).ok_or(USAGE)?, a.get(2).ok_or(USAGE)?);
            let (version, source, date) = (flag(a, "--version").ok_or("--version is required")?, flag(a, "--source").ok_or("--source is required")?, flag(a, "--date").ok_or("--date is required")?);
            let mut m = parse(&std::fs::read(path).map_err(|e| format!("cannot read {path}: {e}"))?)?;
            let mut add: Vec<FileSum> = Vec::new();
            if let Some(f) = flag(a, "--file") {
                use sha2::Digest;
                let mut h = sha2::Sha256::new();
                std::io::copy(&mut std::fs::File::open(&f).map_err(|e| format!("cannot read {f}: {e}"))?, &mut h).map_err(|e| e.to_string())?;
                let name = flag(a, "--as").or_else(|| Path::new(&f).file_name().map(|n| n.to_string_lossy().into_owned())).ok_or("no file name")?;
                add.push(FileSum { file: name, sha256: hex(&h.finalize()) });
            } else if let Some(h) = flag(a, "--sha256") {
                add.push(FileSum { file: flag(a, "--as").ok_or("--as NAME is required with --sha256")?, sha256: h.to_ascii_lowercase() });
            } else if let Some(f) = flag(a, "--pypi-json") {
                let j: serde_json::Value = serde_json::from_slice(&std::fs::read(&f).map_err(|e| format!("cannot read {f}: {e}"))?).map_err(|e| format!("{f} is not JSON: {e}"))?;
                for u in j["urls"].as_array().ok_or("no `urls` list in the PyPI JSON")? {
                    if let (Some(n), Some(h)) = (u["filename"].as_str(), u["digests"]["sha256"].as_str()) {
                        add.push(FileSum { file: n.into(), sha256: h.into() });
                    }
                }
            } else {
                return Err("one of --file, --sha256 or --pypi-json is required".into());
            }
            let key = id.strip_prefix("cli:").map(|_| id.clone()).unwrap_or_else(|| format!("cli:{id}"));
            let it = m.items.iter_mut().find(|i| i.id == key).ok_or_else(|| format!("{key} is not in {path}"))?;
            if it.kind != Kind::NoneVendor {
                return Err(format!("{key} is `{:?}`: our own SHA-256 belongs to a none-vendor item with one exact version (see docs/development/DOWNLOADS-MANIFEST.md section 10 for ffmpeg)", it.kind));
            }
            if it.version != version {
                return Err(format!("{key} is pinned to version `{}` in the manifest, not `{version}`; change the pin in the code and the bundled row first", it.version));
            }
            let mut files = match it.checksum.take() {
                Some(Checksum::Sha256 { files, .. }) => files,
                _ => Vec::new(),
            };
            for f in add {
                files.retain(|x| x.file != f.file);
                files.push(f);
            }
            it.checksum = Some(Checksum::Sha256 { files, date, source });
            validate_against(&b, &m)?;
            std::fs::write(path, to_json(&m)).map_err(|e| e.to_string())?;
            Ok(format!("{key}: SHA-256 values recorded in {path}; now raise `sequence`, set `issued`, then run `./run.sh dlmanifest sign {path} --key <keyfile>` and `verify`"))
        }
        "check" => Ok(format!("bundled manifest OK: sequence {}, {} items; public key {}", b.sequence, b.items.len(), if builtin_key().is_some() { "built in" } else { "unset (no fetched manifest is accepted)" })),
        "gen" => {
            let root = flag(a, "--root").unwrap_or_else(|| ".".into());
            let rows = download_rows(&std::fs::read_to_string(Path::new(&root).join("legal/downloads.txt")).map_err(|e| format!("cannot read legal/downloads.txt under {root}: {e}"))?);
            Ok(to_json(&from_catalog(&rows, b.sequence, &b.issued)).trim_end().to_string())
        }
        _ => Err(USAGE.to_string()),
    }
}

/// Entry point of `iyakki dlmanifest`; returns the exit code.
pub fn cli_main(a: &[String]) -> i32 {
    match run(a) {
        Ok(t) => {
            println!("{t}");
            0
        }
        Err(e) => {
            eprintln!("{e}");
            1
        }
    }
}

#[path = "dlmanifest_fetch.rs"]
pub mod fetch;

#[path = "dlmanifest_pins.rs"]
pub mod pins;

#[cfg(test)]
#[path = "dlmanifest_tests.rs"]
mod tests;
