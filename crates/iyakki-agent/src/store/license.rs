//! Licence tokens for paid apps. IYAKKI never handles money: a payment provider (or the publisher) hands back a signed
//! token, and this module checks it offline. A token names the app, the device (and optionally the account), an
//! expiry and a serial the revocation list can withdraw. See docs/STORE.md, section 8.

use std::path::{Path, PathBuf};

use ed25519_dalek::SigningKey;
use serde::{Deserialize, Serialize};

use super::revocation::Revocations;
use super::trust::{sign_doc, verify_doc, Roots};
use crate::pkg::public_key_hex;

pub const MAX_TOKEN_BYTES: usize = 8 * 1024;

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq, Default)]
#[serde(default)]
pub struct Token {
    pub v: u32,
    /// The package name.
    pub app: String,
    pub publisher: String,
    /// The device id this token works on (`LicenseStore::device_id`).
    pub device: String,
    /// The account it is for, or empty for anyone on the device.
    pub account: String,
    pub serial: String,
    pub plan: String,
    pub issued: i64,
    /// Unix seconds; 0 never expires.
    pub expires: i64,
    pub issuer: String,
    pub sig: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LicenseError {
    Malformed,
    UnknownIssuer,
    BadSignature,
    WrongApp,
    WrongDevice,
    WrongAccount,
    Expired(i64),
    Revoked(String),
}

impl std::fmt::Display for LicenseError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            LicenseError::Malformed => write!(f, "that is not a licence token"),
            LicenseError::UnknownIssuer => write!(f, "the licence was not issued by this app's publisher or by IYAKKI"),
            LicenseError::BadSignature => write!(f, "the licence has a bad signature"),
            LicenseError::WrongApp => write!(f, "the licence is for a different app"),
            LicenseError::WrongDevice => write!(f, "the licence is for a different device"),
            LicenseError::WrongAccount => write!(f, "the licence is for a different account"),
            LicenseError::Expired(t) => write!(f, "the licence expired on {}", super::trust::date(*t)),
            LicenseError::Revoked(why) => write!(f, "the licence was withdrawn ({why})"),
        }
    }
}

/// What a token is checked against.
pub struct Check<'a> {
    pub app: &'a str,
    /// The keys that may issue a licence for this app: the package's signer, a publisher key, a root.
    pub issuers: &'a [String],
    pub roots: &'a Roots,
    pub device: &'a str,
    pub account: &'a str,
    pub now: i64,
    pub revocations: &'a Revocations,
}

impl Token {
    pub fn parse(text: &str) -> Result<Token, LicenseError> {
        if text.len() > MAX_TOKEN_BYTES {
            return Err(LicenseError::Malformed);
        }
        let t: Token = serde_json::from_str(text.trim()).map_err(|_| LicenseError::Malformed)?;
        if t.v != 1 || t.app.is_empty() || t.serial.is_empty() || t.device.is_empty() {
            return Err(LicenseError::Malformed);
        }
        Ok(t)
    }

    pub fn to_json(&self) -> String {
        serde_json::to_string_pretty(self).unwrap_or_default()
    }

    pub fn issue(key: &SigningKey, app: &str, publisher: &str, device: &str, account: &str, serial: &str, plan: &str, issued: i64, expires: i64) -> Token {
        let mut t = Token { v: 1, app: app.into(), publisher: publisher.into(), device: device.into(), account: account.into(), serial: serial.into(), plan: plan.into(), issued, expires, issuer: public_key_hex(key), sig: String::new() };
        t.sig = sign_doc("license", &serde_json::to_value(&t).unwrap_or_default(), key);
        t
    }

    /// Everything, in the order a person would want to hear about it.
    pub fn verify(&self, c: &Check) -> Result<(), LicenseError> {
        if !(c.issuers.contains(&self.issuer) || c.roots.valid_at(&self.issuer, c.now).is_some()) {
            return Err(LicenseError::UnknownIssuer);
        }
        if !verify_doc("license", &serde_json::to_value(self).unwrap_or_default(), &self.issuer, &self.sig) {
            return Err(LicenseError::BadSignature);
        }
        if self.app != c.app {
            return Err(LicenseError::WrongApp);
        }
        if self.device != c.device {
            return Err(LicenseError::WrongDevice);
        }
        if !self.account.is_empty() && self.account != c.account {
            return Err(LicenseError::WrongAccount);
        }
        if self.expires != 0 && c.now > self.expires {
            return Err(LicenseError::Expired(self.expires));
        }
        if let Some(why) = c.revocations.license_revoked(&self.serial) {
            return Err(LicenseError::Revoked(why));
        }
        Ok(())
    }

    pub fn describe(&self) -> String {
        let until = if self.expires == 0 { "no expiry".to_string() } else { format!("until {}", super::trust::date(self.expires)) };
        format!("{} licence {} ({until})", if self.plan.is_empty() { "standard" } else { &self.plan }, self.serial)
    }
}

/// Licences kept on this device: `store/licenses/<app>.json` (owner-only), and the device id they are bound to.
pub struct LicenseStore {
    dir: PathBuf,
}

impl LicenseStore {
    pub fn new(store_dir: &Path) -> LicenseStore {
        LicenseStore { dir: store_dir.to_path_buf() }
    }

    /// A random id made once and kept in the store folder. (Not the MAC address or the machine id: nothing about the
    /// hardware leaves the device, the id only has to be the same next time.)
    pub fn device_id(&self) -> String {
        let p = self.dir.join("device-id");
        if let Ok(t) = std::fs::read_to_string(&p) {
            let t = t.trim().to_string();
            if t.len() == 32 && t.chars().all(|c| c.is_ascii_hexdigit()) {
                return t;
            }
        }
        use std::io::{Read, Write};
        use std::os::unix::fs::OpenOptionsExt;
        let mut b = [0u8; 16];
        let id = match std::fs::File::open("/dev/urandom").and_then(|mut f| f.read_exact(&mut b)) {
            Ok(()) => crate::pkg::hex(&b),
            Err(_) => crate::pkg::hex(&sha2_of(format!("{:?}{}", std::time::SystemTime::now(), std::process::id()).as_bytes())[..16]),
        };
        let _ = std::fs::create_dir_all(&self.dir);
        let _ = std::fs::OpenOptions::new().write(true).create(true).truncate(true).mode(0o600).open(&p).and_then(|mut f| f.write_all(id.as_bytes()));
        id
    }

    fn file(&self, app: &str) -> PathBuf {
        self.dir.join("licenses").join(format!("{app}.json"))
    }

    /// Keep a token that verified. The caller checked it with [`Token::verify`].
    pub fn save(&self, t: &Token) -> Result<(), String> {
        use std::io::Write;
        use std::os::unix::fs::OpenOptionsExt;
        if !crate::skills::valid_skill_name(&t.app) {
            return Err("the licence names an app that cannot exist".into());
        }
        let d = self.dir.join("licenses");
        std::fs::create_dir_all(&d).map_err(|e| e.to_string())?;
        let (tmp, p) = (d.join(format!("{}.tmp", t.app)), self.file(&t.app));
        std::fs::OpenOptions::new().write(true).create(true).truncate(true).mode(0o600).open(&tmp).and_then(|mut f| f.write_all(t.to_json().as_bytes())).map_err(|e| e.to_string())?;
        std::fs::rename(&tmp, p).map_err(|e| e.to_string())
    }

    pub fn get(&self, app: &str) -> Option<Token> {
        if !crate::skills::valid_skill_name(app) {
            return None;
        }
        Token::parse(&std::fs::read_to_string(self.file(app)).ok()?).ok()
    }

    pub fn remove(&self, app: &str) {
        if crate::skills::valid_skill_name(app) {
            let _ = std::fs::remove_file(self.file(app));
        }
    }

    pub fn list(&self) -> Vec<Token> {
        let mut v: Vec<Token> = std::fs::read_dir(self.dir.join("licenses")).into_iter().flatten().flatten().filter_map(|e| Token::parse(&std::fs::read_to_string(e.path()).ok()?).ok()).collect();
        v.sort_by(|a, b| a.app.cmp(&b.app));
        v
    }
}

fn sha2_of(b: &[u8]) -> Vec<u8> {
    use sha2::{Digest, Sha256};
    Sha256::digest(b).to_vec()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pkg::new_signing_key;

    const NOW: i64 = 1_800_000_000;

    struct Fx {
        publisher: SigningKey,
        roots: Roots,
        revs: Revocations,
        issuers: Vec<String>,
    }

    fn fx() -> Fx {
        let (publisher, root) = (new_signing_key().unwrap(), new_signing_key().unwrap());
        Fx { issuers: vec![public_key_hex(&publisher)], roots: Roots::with("r", &root), revs: Revocations::default(), publisher }
    }

    fn check<'a>(f: &'a Fx, device: &'a str, account: &'a str, now: i64) -> Check<'a> {
        Check { app: "pro-editor", issuers: &f.issuers, roots: &f.roots, device, account, now, revocations: &f.revs }
    }

    fn token(f: &Fx, device: &str, account: &str, expires: i64) -> Token {
        Token::issue(&f.publisher, "pro-editor", "acme", device, account, "L-1", "standard", NOW - 10, expires)
    }

    #[test]
    fn a_valid_token_passes_and_every_way_of_failing_is_named() {
        let f = fx();
        let t = token(&f, "dev1", "", NOW + 1000);
        assert_eq!(t.verify(&check(&f, "dev1", "alice", NOW)), Ok(()));
        assert_eq!(t.verify(&check(&f, "dev2", "alice", NOW)), Err(LicenseError::WrongDevice));
        assert_eq!(t.verify(&check(&f, "dev1", "alice", NOW + 2000)), Err(LicenseError::Expired(NOW + 1000)));
        let mine = token(&f, "dev1", "alice", 0);
        assert_eq!(mine.verify(&check(&f, "dev1", "alice", NOW + 99_999_999)), Ok(()), "0 never expires");
        assert_eq!(mine.verify(&check(&f, "dev1", "bob", NOW)), Err(LicenseError::WrongAccount));
        let mut c = check(&f, "dev1", "alice", NOW);
        c.app = "other-app";
        assert_eq!(t.verify(&c), Err(LicenseError::WrongApp));
        // changed after signing
        let mut forged = t.clone();
        forged.expires = 0;
        assert_eq!(forged.verify(&check(&f, "dev1", "alice", NOW)), Err(LicenseError::BadSignature));
        // signed by a stranger
        let stranger = Token::issue(&new_signing_key().unwrap(), "pro-editor", "acme", "dev1", "", "L-2", "", NOW, 0);
        assert_eq!(stranger.verify(&check(&f, "dev1", "alice", NOW)), Err(LicenseError::UnknownIssuer));
        for e in [LicenseError::WrongDevice, LicenseError::Expired(NOW), LicenseError::Revoked("refund".into()), LicenseError::BadSignature] {
            assert!(!e.to_string().is_empty());
        }
    }

    #[test]
    fn a_root_may_issue_for_any_app_and_a_revoked_serial_stops_working() {
        let mut f = fx();
        let root = new_signing_key().unwrap();
        f.roots = Roots::with("r", &root);
        let t = Token::issue(&root, "pro-editor", "acme", "dev1", "", "L-9", "", NOW, 0);
        assert_eq!(t.verify(&check(&f, "dev1", "a", NOW)), Ok(()));
        let list = f.revs.sign_for_tests(&root, 1, NOW, vec![], vec![], vec![]);
        // the helper cannot add licences; sign one by hand
        let _ = list;
        let signed = super::super::revocation::sign_list(&root, 2, NOW, NOW + 100, vec![], vec![], vec![], vec!["L-9".into()]);
        f.revs.accept(&signed, &f.roots, NOW).unwrap();
        assert_eq!(t.verify(&check(&f, "dev1", "a", NOW)), Err(LicenseError::Revoked("revoked".into())));
    }

    #[test]
    fn tokens_parse_strictly_and_are_kept_per_app_with_a_stable_device_id() {
        let f = fx();
        let d = std::env::temp_dir().join(format!("iyakki-lic-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        let s = LicenseStore::new(&d);
        let id = s.device_id();
        assert_eq!(id.len(), 32);
        assert_eq!(s.device_id(), id, "the same next time");
        let t = token(&f, &id, "", 0);
        assert_eq!(Token::parse(&t.to_json()).unwrap(), t);
        assert_eq!(Token::parse("{}"), Err(LicenseError::Malformed));
        assert_eq!(Token::parse("nonsense"), Err(LicenseError::Malformed));
        assert_eq!(Token::parse(&"x".repeat(MAX_TOKEN_BYTES + 1)), Err(LicenseError::Malformed));
        assert!(s.get("pro-editor").is_none());
        s.save(&t).unwrap();
        assert_eq!(s.get("pro-editor"), Some(t.clone()));
        assert_eq!(s.list().len(), 1);
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(std::fs::metadata(d.join("licenses/pro-editor.json")).unwrap().permissions().mode() & 0o777, 0o600);
        assert!(s.get("../x").is_none());
        s.remove("pro-editor");
        assert!(s.get("pro-editor").is_none());
        assert!(t.describe().contains("L-1") && t.describe().contains("no expiry"));
        let _ = std::fs::remove_dir_all(d);
    }
}
