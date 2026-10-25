//! A person's publisher signing keys, for the IDE's Publish flow. The private half is a 0600 file in the person's own folder of the
//! data directory (`publisher-keys/<user>/<label>.key`), never in a workspace and so never in git; the assistant is never given it,
//! only a signature made with it. The public half and its fingerprint are shown, and "Copy key for the store admin" gives the line
//! the store's `trusted-keys.txt` takes.

use std::path::PathBuf;

pub use ed25519_dalek::SigningKey;

use crate::pkg;
use crate::services::Services;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct KeyInfo {
    pub label: String,
    pub public: String,
    /// Groups of four hex digits, for people to compare.
    pub fingerprint: String,
    pub default: bool,
}

impl KeyInfo {
    /// The line an administrator adds to the store's `trusted-keys.txt`.
    pub fn admin_line(&self) -> String {
        format!("{} {}", self.public, self.label)
    }
}

fn dir(svc: &Services, user: &str) -> Result<PathBuf, String> {
    if !iyakki_sec::policy::valid_username(user) {
        return Err("no such account".into());
    }
    Ok(svc.dir().join("publisher-keys").join(user))
}

pub fn valid_label(l: &str) -> bool {
    crate::skills::valid_skill_name(l)
}

fn info(label: &str, k: &SigningKey, default: bool) -> KeyInfo {
    let public = pkg::public_key_hex(k);
    KeyInfo { label: label.into(), fingerprint: super::server::fingerprint(&public), public, default }
}

fn read_key(p: &std::path::Path) -> Option<SigningKey> {
    pkg::signing_key_from_hex(std::fs::read_to_string(p).ok()?.trim()).ok()
}

fn default_label(d: &std::path::Path) -> Option<String> {
    std::fs::read_to_string(d.join("default")).ok().map(|t| t.trim().to_string()).filter(|t| !t.is_empty())
}

/// `iyakki pkg keygen`, for a person: a new key under `label`. The first key becomes the default.
pub fn create(svc: &Services, user: &str, label: &str) -> Result<KeyInfo, String> {
    if !valid_label(label) {
        return Err("a key name is lowercase letters, digits and dashes (for example my-key)".into());
    }
    let d = dir(svc, user)?;
    let file = d.join(format!("{label}.key"));
    if file.exists() {
        return Err(format!("you already have a key called {label}"));
    }
    {
        use std::os::unix::fs::DirBuilderExt;
        std::fs::DirBuilder::new().recursive(true).mode(0o700).create(&d).map_err(|e| crate::scope::explain_io(&e, &d, "make the folder for your keys in"))?;
    }
    let k = pkg::new_signing_key().map_err(|e| e.to_string())?;
    {
        use std::io::Write;
        use std::os::unix::fs::OpenOptionsExt;
        let mut f = std::fs::OpenOptions::new().write(true).create_new(true).mode(0o600).open(&file).map_err(|e| crate::scope::explain_io(&e, &file, "write your signing key"))?;
        f.write_all(pkg::signing_key_hex(&k).as_bytes()).map_err(|e| crate::scope::explain_io(&e, &file, "write your signing key"))?;
    }
    let first = default_label(&d).is_none();
    if first {
        std::fs::write(d.join("default"), label).map_err(|e| crate::scope::explain_io(&e, &d.join("default"), "write"))?;
    }
    Ok(info(label, &k, first))
}

pub fn list(svc: &Services, user: &str) -> Vec<KeyInfo> {
    let Ok(d) = dir(svc, user) else { return vec![] };
    let def = default_label(&d);
    let mut out: Vec<KeyInfo> = std::fs::read_dir(&d)
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|e| {
            let n = e.file_name().to_string_lossy().to_string();
            let label = n.strip_suffix(".key")?.to_string();
            let k = read_key(&e.path())?;
            Some(info(&label, &k, def.as_deref() == Some(label.as_str())))
        })
        .collect();
    out.sort_by(|a, b| b.default.cmp(&a.default).then(a.label.cmp(&b.label)));
    out
}

pub fn set_default(svc: &Services, user: &str, label: &str) -> Result<(), String> {
    let d = dir(svc, user)?;
    if !d.join(format!("{label}.key")).is_file() {
        return Err(format!("you have no key called {label}"));
    }
    std::fs::write(d.join("default"), label).map_err(|e| crate::scope::explain_io(&e, &d.join("default"), "write"))
}

/// The key to sign with (named, or the default) and its fingerprint.
pub fn load(svc: &Services, user: &str, label: Option<&str>) -> Option<(SigningKey, String)> {
    let d = dir(svc, user).ok()?;
    let label = label.map(str::to_string).or_else(|| default_label(&d)).or_else(|| list(svc, user).first().map(|k| k.label.clone()))?;
    let k = read_key(&d.join(format!("{label}.key")))?;
    let fp = super::server::fingerprint(&pkg::public_key_hex(&k));
    Some((k, fp))
}

pub fn load_default(svc: &Services, user: &str) -> Option<(SigningKey, String)> {
    load(svc, user, None)
}

/// The key to sign with; when the person has none, IYAKKI makes one (`my-publisher-key`, the default) and trusts its public half on
/// this device for the person's own packages, so a tile app made here installs from their own store as "Trusted" with nothing to do.
/// The private half is a 0600 file in the person's own folder of the data directory (never the workspace, never given to an agent).
/// Trusting a key only vouches for packages that key signs, which only this person can sign; it opens no other store or catalog.
/// Returns the key, its fingerprint and whether it was just made.
pub fn ensure_default(svc: &Services, user: &str) -> Result<(SigningKey, String, bool), String> {
    if let Some((k, fp)) = load_default(svc, user) {
        return Ok((k, fp, false));
    }
    let info = create(svc, user, "my-publisher-key")?;
    let (k, fp) = load(svc, user, Some(&info.label)).ok_or("the new publisher key could not be read back")?;
    // best effort: without it the package is listed as Community instead of Trusted
    let _ = svc.packages.trust_key(&info.public, &format!("{user}'s own publisher key (made by IYAKKI)"));
    Ok((k, fp, true))
}
