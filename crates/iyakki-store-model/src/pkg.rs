//! Types moved out of `iyakki_agent::pkg` (F-SPLIT-03, scripted by dist/split/move_types.py); the agent re-exports them.

use ed25519_dalek::{SigningKey, Signature, Verifier, VerifyingKey};
use sha2::{Digest, Sha256};
use serde::Deserialize;
use serde::Serialize;
use crate::skills::Skill;
use crate::skills::SkillError;

pub const MAX_PACKAGE_BYTES: usize = 64 * 1024;

pub const MAX_EXTENSION_PACKAGE_BYTES: usize = 12 * 1024 * 1024; // room for a ~7.5 MB kernel module in base64 (the Mac mini Wi-Fi driver pack)

#[derive(Debug, thiserror::Error, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum PkgError {
    #[error("{0}")]
    Invalid(String),
    #[error("the package was changed after it was made (hash does not match)")]
    BadHash,
    #[error("the signature does not match this package")]
    BadSignature,
    #[error("review the package first: it is not signed by a key you trust")]
    NotApproved,
    #[error("version {new} is not newer than the installed {old}")]
    NotNewer { old: String, new: String },
    #[error("{0}")]
    Skill(#[from] SkillError),
    #[error("storage error: {0}")]
    Storage(String),
}

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum Trust {
    /// Signed by a key listed in `trusted-keys.txt`, with the label given there.
    Trusted(String),
    /// Correctly signed, but by a key nobody has trusted yet.
    UnknownSigner(String),
    Unsigned,
}

impl Trust {
    pub fn label(&self) -> String {
        match self {
            Trust::Trusted(who) => format!("signed by {who} (trusted key)"),
            Trust::UnknownSigner(k) => format!("signed by an unknown key {}…", &k[..12.min(k.len())]),
            Trust::Unsigned => "not signed".into(),
        }
    }
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct Installed {
    pub version: String,
    pub sha256: String,
    pub signer: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Manifest {
    pub kind: String,
    pub name: String,
    pub version: String,
    pub license: String,
    pub sha256: String,
    pub signer: Option<String>,
    /// Store metadata: one line of compact JSON (see `store::manifest::Meta`), covered by the signature.
    pub meta: Option<String>,
    /// A publisher certificate: one line of compact JSON (see `store::trust::Cert`), covered by the signature.
    pub cert: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Package {
    pub manifest: Manifest,
    pub payload: String,
}

pub fn version_key(v: &str) -> Vec<u64> {
    v.split('.').map(|p| p.parse().unwrap_or(0)).collect()
}

pub fn newer(new: &str, old: &str) -> bool {
    let (mut a, mut b) = (version_key(new), version_key(old));
    let n = a.len().max(b.len());
    a.resize(n, 0);
    b.resize(n, 0);
    a > b
}

/// What the person is asked to approve.
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct Review {
    pub package: Package,
    pub trust: Trust,
    pub skill: Skill,
    /// The version installed in this scope now, if any.
    pub installed: Option<String>,
}

impl Review {
    pub fn summary(&self) -> String {
        let m = &self.package.manifest;
        let change = match &self.installed {
            Some(v) if newer(&m.version, v) => format!("update {v} → {}", m.version),
            Some(v) => format!("already installed ({v})"),
            None => "new".to_string(),
        };
        let triggers = if self.skill.triggers.is_empty() { String::new() } else { format!("\nused when you mention: {}", self.skill.triggers.join(", ")) };
        format!(
            "skill {} {} ({}) — {}\n{}\nlicense {} · {}{triggers}\n— instructions —\n{}",
            m.name,
            m.version,
            change,
            self.trust.label(),
            self.skill.description,
            m.license,
            m.sha256.get(..12).unwrap_or(""),
            self.skill.body
        )
    }
}

/// The largest WASM module of a tool package (the agent's `wtool::MAX_MODULE_BYTES` is this).
pub const WASM_MODULE_MAX: usize = 400 * 1024;

/// Tool packages carry a module (in hexadecimal), so they may be larger.
pub const MAX_TOOL_PACKAGE_BYTES: usize = 2 * WASM_MODULE_MAX + 16 * 1024;

/// Driver packs carry firmware blobs (in base64), so `extension` packages may be larger than the other kinds.
/// A tile app is a whole web bundle (see `store::tile`).
pub const MAX_TILE_PACKAGE_BYTES: usize = 3 * 1024 * 1024;

pub const MAGIC: &str = "IYAKKI-PKG 1";

pub fn bad<T>(m: impl Into<String>) -> Result<T, PkgError> {
    Err(PkgError::Invalid(m.into()))
}

/// The package kinds the header may name. `skill` and `tool` have their own installers here; the rest are handled by
/// the store (`crate::store`).
pub const KINDS: [&str; 10] = ["skill", "tool", "mcp", "cli", "agent", "template", "theme", "extension", "app", "tile"];

/// The longest `store:` or `cert:` header line.
pub const MAX_HEADER_JSON: usize = 40 * 1024;

pub fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

pub fn unhex<const N: usize>(s: &str) -> Option<[u8; N]> {
    if s.len() != N * 2 || !s.is_ascii() {
        return None;
    }
    let mut out = [0u8; N];
    for (i, o) in out.iter_mut().enumerate() {
        *o = u8::from_str_radix(&s[i * 2..i * 2 + 2], 16).ok()?;
    }
    Some(out)
}

pub fn hash(payload: &str) -> String {
    hex(&Sha256::digest(payload.as_bytes()))
}

/// The size limit of a package of this kind.
pub fn max_bytes(kind: &str) -> usize {
    match kind {
        "tool" => MAX_TOOL_PACKAGE_BYTES,
        "extension" => MAX_EXTENSION_PACKAGE_BYTES,
        "tile" => MAX_TILE_PACKAGE_BYTES,
        _ => MAX_PACKAGE_BYTES,
    }
}

pub fn valid_version(v: &str) -> bool {
    !v.is_empty() && v.len() <= 24 && v.split('.').count() <= 4 && v.split('.').all(|p| !p.is_empty() && p.len() <= 6 && p.bytes().all(|b| b.is_ascii_digit()))
}

pub fn header_text(m: &Manifest) -> String {
    let mut h = format!("{MAGIC}\ntype: {}\nname: {}\nversion: {}\nlicense: {}\nsha256: {}\n", m.kind, m.name, m.version, m.license, m.sha256);
    if let Some(s) = &m.signer {
        h.push_str(&format!("signer: {s}\n"));
    }
    if let Some(j) = &m.meta {
        h.push_str(&format!("store: {j}\n"));
    }
    if let Some(j) = &m.cert {
        h.push_str(&format!("cert: {j}\n"));
    }
    h
}

pub fn public_key_hex(k: &SigningKey) -> String {
    hex(k.verifying_key().as_bytes())
}

/// Read a package and check its hash and signature. The result is not trusted yet; see [`Packages::review`].
pub fn parse(text: &str) -> Result<Package, PkgError> {
    if text.len() > MAX_EXTENSION_PACKAGE_BYTES {
        return bad(format!("packages are limited to {} KiB", MAX_EXTENSION_PACKAGE_BYTES / 1024));
    }
    let text = text.trim_start_matches('\u{feff}').replace("\r\n", "\n");
    let (head, payload) = text.split_once("\n---\n").ok_or_else(|| PkgError::Invalid("not an IYAKKI package (no `---` after the header)".into()))?;
    let mut lines = head.lines();
    if lines.next() != Some(MAGIC) {
        return bad("not an IYAKKI package, or a newer format than this system understands");
    }
    let (mut kind, mut name, mut version, mut license, mut sha, mut signer, mut sig) = (String::new(), String::new(), String::new(), String::new(), String::new(), None, None);
    let (mut meta, mut cert): (Option<String>, Option<String>) = (None, None);
    let mut signed_len = MAGIC.len() + 1;
    for line in lines {
        let Some((k, v)) = line.split_once(": ") else { return bad(format!("unreadable header line {line:?}")) };
        match k {
            "type" => kind = v.into(),
            "name" => name = v.into(),
            "version" => version = v.into(),
            "license" => license = v.into(),
            "sha256" => sha = v.into(),
            "signer" => signer = Some(v.to_string()),
            "signature" => sig = Some(v.to_string()),
            "store" | "cert" if v.len() > MAX_HEADER_JSON => return bad(format!("the {k} line is too long")),
            "store" => meta = Some(v.to_string()),
            "cert" => cert = Some(v.to_string()),
            _ => return bad(format!("unknown header field {k:?}")),
        }
        if k != "signature" {
            signed_len += line.len() + 1;
        }
    }
    if !KINDS.contains(&kind.as_str()) {
        return bad(format!("package type {kind:?} is not known to this system"));
    }
    if text.len() > max_bytes(&kind) {
        return bad(format!("{kind} packages are limited to {} KiB", max_bytes(&kind) / 1024));
    }
    if !crate::skills::valid_skill_name(&name) {
        return bad("name: lowercase letters, digits and `-`");
    }
    if !valid_version(&version) {
        return bad("version: digits separated by dots, like 1.2.0");
    }
    if license.is_empty() {
        return bad("license is required");
    }
    if hash(payload) != sha {
        return Err(PkgError::BadHash);
    }
    match (&signer, &sig) {
        (Some(s), Some(g)) => {
            let key = unhex::<32>(s).and_then(|b| VerifyingKey::from_bytes(&b).ok()).ok_or(PkgError::BadSignature)?;
            let sig = unhex::<64>(g).map(|b| Signature::from_bytes(&b)).ok_or(PkgError::BadSignature)?;
            key.verify(text[..signed_len].as_bytes(), &sig).map_err(|_| PkgError::BadSignature)?;
        }
        (None, None) => {}
        _ => return Err(PkgError::BadSignature),
    }
    Ok(Package { manifest: Manifest { kind, name, version, license, sha256: sha, signer, meta, cert }, payload: payload.to_string() })
}

/// A fresh signing key for tests (the agent's `pkg::new_signing_key` reads the operating system's random source).
#[cfg(any(test, feature = "testing"))]
pub fn new_signing_key() -> Result<SigningKey, PkgError> {
    use std::io::Read;
    let mut seed = [0u8; 32];
    std::fs::File::open("/dev/urandom").and_then(|mut f| f.read_exact(&mut seed)).map_err(|e| PkgError::Storage(e.to_string()))?;
    Ok(SigningKey::from_bytes(&seed))
}
