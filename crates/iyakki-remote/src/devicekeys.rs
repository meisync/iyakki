//! Device sign-in for the web desktop (docs/development/CONNECTOR-APP-PLAN.md, 3a): a device, a script or `iyakki-connect` signs a request
//! with an Ed25519 key that its person enrolled once, with the `IYAKKI-SIG` scheme the vault uses (`iyakki_link::sigscheme`). The server
//! answers with the same opaque session token a password sign-in gives (cookie and CSRF value), so everything after it is the same.
//!
//! The rules, each with a test:
//! * the signature covers the account name, `POST`, `/api/login-key`, the time, the body and a random nonce: it cannot be turned into
//!   another request, and a key verifies only the account it was enrolled for;
//! * the time must be within a minute of the server's clock either way (an old capture is refused);
//! * a nonce is accepted once per account while its time is still inside the window (an exact copy is refused); only a request with
//!   a valid signature is remembered, so a stranger cannot fill the memory;
//! * a revoked key signs nothing: the list is read at every sign-in, and revoking also ends the sessions that key opened;
//! * only `ssh-ed25519` (or 64 hex digits) is enrolled: no RSA, no other type.
//!
//! Every refusal is the same answer (`sign-in refused`), so the route does not tell which account or key exists.

use iyakki_link::sigscheme::{fingerprint, parse_authorization, parse_public};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Mutex;

/// How far the signed time may be from the server's clock, either way.
pub const WINDOW_SECS: u64 = 60;
/// Keys one account can have enrolled.
pub const MAX_PER_USER: usize = 8;
const MAX_NONCES: usize = 20_000;

/// The path the signature covers.
pub const LOGIN_PATH: &str = "/api/login-key";

/// The enrolled keys: `user ed25519-public-hex` per line, in a file of mode 600 (or only in memory, for tests).
pub struct DeviceKeys {
    file: Option<PathBuf>,
    keys: Mutex<Vec<(String, [u8; 32])>>,
}

impl DeviceKeys {
    pub fn in_memory() -> DeviceKeys {
        DeviceKeys { file: None, keys: Mutex::new(Vec::new()) }
    }

    pub fn open(path: PathBuf) -> Result<DeviceKeys, String> {
        let mut keys = Vec::new();
        match std::fs::read_to_string(&path) {
            Ok(t) => {
                for (n, l) in t.lines().enumerate().filter(|(_, l)| !l.trim().is_empty() && !l.starts_with('#')) {
                    let (u, k) = l.split_once(' ').and_then(|(u, k)| parse_public(k).ok().map(|k| (u.to_string(), k))).ok_or_else(|| format!("{}: line {} is not understood", path.display(), n + 1))?;
                    keys.push((u, k));
                }
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(format!("{}: {e}", path.display())),
        }
        Ok(DeviceKeys { file: Some(path), keys: Mutex::new(keys) })
    }

    /// The enrolled public keys of `user`.
    pub fn keys_of(&self, user: &str) -> Vec<[u8; 32]> {
        self.keys.lock().map(|k| k.iter().filter(|(u, _)| u == user).map(|(_, k)| *k).collect()).unwrap_or_default()
    }

    pub fn fingerprints(&self, user: &str) -> Vec<String> {
        self.keys_of(user).iter().map(fingerprint).collect()
    }

    /// Enrol a key (an `ssh-ed25519` line or 64 hex digits) for `user`. Returns its fingerprint. Enrolling it twice changes nothing.
    pub fn enroll(&self, user: &str, key: &str) -> Result<String, String> {
        if user.is_empty() || user.len() > 128 || user.chars().any(|c| c.is_control() || c.is_whitespace()) {
            return Err("that account name cannot have keys".into());
        }
        let k = parse_public(key)?;
        let mut all = self.keys.lock().map_err(|_| "busy".to_string())?;
        if all.iter().any(|(u, e)| u == user && *e == k) {
            return Ok(fingerprint(&k));
        }
        if all.iter().filter(|(u, _)| u == user).count() >= MAX_PER_USER {
            return Err(format!("at most {MAX_PER_USER} keys per account: revoke one first"));
        }
        all.push((user.to_string(), k));
        self.save(&all)?;
        Ok(fingerprint(&k))
    }

    /// Revoke the key of `user` with this fingerprint. True when there was one.
    pub fn revoke(&self, user: &str, fp: &str) -> Result<bool, String> {
        let mut all = self.keys.lock().map_err(|_| "busy".to_string())?;
        let before = all.len();
        all.retain(|(u, k)| !(u == user && fingerprint(k) == fp));
        if all.len() == before {
            return Ok(false);
        }
        self.save(&all)?;
        Ok(true)
    }

    fn save(&self, all: &[(String, [u8; 32])]) -> Result<(), String> {
        use std::io::Write;
        use std::os::unix::fs::OpenOptionsExt;
        let Some(file) = &self.file else { return Ok(()) };
        let mut text = String::from("# IYAKKI device sign-in keys: account and Ed25519 public key (hex)\n");
        for (u, k) in all {
            text.push_str(&format!("{u} {}\n", iyakki_link::sigscheme::hex(k)));
        }
        let tmp = file.with_extension("new");
        let _ = std::fs::remove_file(&tmp);
        let mut f = std::fs::OpenOptions::new().write(true).create_new(true).mode(0o600).open(&tmp).map_err(|e| format!("{}: {e}", tmp.display()))?;
        f.write_all(text.as_bytes()).and_then(|_| f.sync_all()).map_err(|e| e.to_string())?;
        std::fs::rename(&tmp, file).map_err(|e| e.to_string())
    }
}

/// Nonces seen in the last minutes, per account.
#[derive(Default)]
pub struct Replay {
    seen: Mutex<HashMap<(String, String), u64>>,
}

impl Replay {
    pub fn new() -> Replay {
        Replay::default()
    }

    /// Remember `(user, nonce)` until `until`; false when it was already there (a copy) or the memory is full (fail closed).
    fn first_use(&self, user: &str, nonce: &str, now: u64, until: u64) -> bool {
        let Ok(mut m) = self.seen.lock() else { return false };
        m.retain(|_, u| *u > now);
        if m.len() >= MAX_NONCES {
            return false;
        }
        m.insert((user.to_string(), nonce.to_string()), until).is_none()
    }
}

/// The one answer of every refusal.
pub const REFUSED: &str = "sign-in refused";

/// Check a `POST /api/login-key` request: `header` is its `Authorization`, `body` its body (empty), `now` the server's clock in Unix
/// seconds, `keys_of` the enrolled keys of an account. On success the account name and the key that signed.
pub fn verify_sign_in(header: &str, body: &[u8], now: u64, keys_of: &dyn Fn(&str) -> Vec<[u8; 32]>, replay: &Replay) -> Result<(String, [u8; 32]), &'static str> {
    let req = parse_authorization(header).ok_or(REFUSED)?;
    if req.ts.abs_diff(now) > WINDOW_SECS {
        return Err(REFUSED);
    }
    let key = keys_of(&req.id).into_iter().find(|k| req.verify(k, "POST", LOGIN_PATH, body)).ok_or(REFUSED)?;
    // only a request that verified is remembered
    if !replay.first_use(&req.id, &req.nonce, now, req.ts.max(now) + WINDOW_SECS + 1) {
        return Err(REFUSED);
    }
    Ok((req.id, key))
}

#[cfg(test)]
mod tests {
    use super::*;
    use iyakki_link::sigscheme::DeviceKey;

    const NOW: u64 = 1_800_000_000;

    fn setup() -> (DeviceKeys, DeviceKey, Replay) {
        let keys = DeviceKeys::in_memory();
        let k = DeviceKey::from_seed([3; 32]);
        keys.enroll("ann", &k.ssh_public_line("laptop")).unwrap();
        (keys, k, Replay::new())
    }

    fn check(h: &str, keys: &DeviceKeys, replay: &Replay, now: u64) -> Result<String, &'static str> {
        verify_sign_in(h, b"", now, &|u| keys.keys_of(u), replay).map(|(u, _)| u)
    }

    #[test]
    fn a_good_signature_signs_in_the_enrolled_account() {
        let (keys, k, replay) = setup();
        let h = k.sign_request("ann", "POST", LOGIN_PATH, NOW, b"").unwrap();
        assert_eq!(check(&h, &keys, &replay, NOW), Ok("ann".to_string()));
    }

    #[test]
    fn a_key_that_is_not_enrolled_or_enrolled_for_another_account_is_refused() {
        let (keys, _, replay) = setup();
        let other = DeviceKey::from_seed([4; 32]);
        assert_eq!(check(&other.sign_request("ann", "POST", LOGIN_PATH, NOW, b"").unwrap(), &keys, &replay, NOW), Err(REFUSED), "wrong key");
        let (_, k, _) = setup();
        assert_eq!(check(&k.sign_request("bob", "POST", LOGIN_PATH, NOW, b"").unwrap(), &keys, &replay, NOW), Err(REFUSED), "ann's key does not sign in bob");
        assert_eq!(check(&k.sign_request("nobody", "POST", LOGIN_PATH, NOW, b"").unwrap(), &keys, &replay, NOW), Err(REFUSED), "unknown account: the same answer");
    }

    #[test]
    fn the_same_signature_cannot_be_used_twice() {
        let (keys, k, replay) = setup();
        let h = k.sign_request("ann", "POST", LOGIN_PATH, NOW, b"").unwrap();
        assert!(check(&h, &keys, &replay, NOW).is_ok());
        assert_eq!(check(&h, &keys, &replay, NOW), Err(REFUSED), "replayed");
        assert_eq!(check(&h, &keys, &replay, NOW + 30), Err(REFUSED), "replayed later, still inside the window");
        // a different nonce is a different request
        assert!(check(&k.sign_request("ann", "POST", LOGIN_PATH, NOW, b"").unwrap(), &keys, &replay, NOW).is_ok());
    }

    #[test]
    fn an_old_or_future_signature_is_refused() {
        let (keys, k, replay) = setup();
        let old = k.sign_request("ann", "POST", LOGIN_PATH, NOW - WINDOW_SECS - 1, b"").unwrap();
        assert_eq!(check(&old, &keys, &replay, NOW), Err(REFUSED), "expired");
        let future = k.sign_request("ann", "POST", LOGIN_PATH, NOW + WINDOW_SECS + 1, b"").unwrap();
        assert_eq!(check(&future, &keys, &replay, NOW), Err(REFUSED), "from the future");
        let edge = k.sign_request("ann", "POST", LOGIN_PATH, NOW - WINDOW_SECS, b"").unwrap();
        assert!(check(&edge, &keys, &replay, NOW).is_ok(), "the edge of the window is fine");
    }

    #[test]
    fn a_revoked_key_signs_nothing() {
        let (keys, k, replay) = setup();
        let fp = keys.fingerprints("ann").remove(0);
        assert!(check(&k.sign_request("ann", "POST", LOGIN_PATH, NOW, b"").unwrap(), &keys, &replay, NOW).is_ok());
        assert_eq!(keys.revoke("bob", &fp), Ok(false), "only the account's own key can be revoked there");
        assert_eq!(keys.revoke("ann", &fp), Ok(true));
        assert_eq!(check(&k.sign_request("ann", "POST", LOGIN_PATH, NOW, b"").unwrap(), &keys, &replay, NOW), Err(REFUSED));
        assert_eq!(keys.revoke("ann", &fp), Ok(false));
    }

    #[test]
    fn a_signature_is_for_this_route_and_this_body_only() {
        let (keys, k, replay) = setup();
        for (method, path, body) in [("GET", LOGIN_PATH, &b""[..]), ("POST", "/api/other", b""), ("POST", LOGIN_PATH, b"x")] {
            assert_eq!(check(&k.sign_request("ann", method, path, NOW, body).unwrap(), &keys, &replay, NOW), Err(REFUSED), "{method} {path}");
        }
        // a vault request signed by the same key does not sign in
        let vault = k.sign_request("ann", "GET", "/v1/scope/tenant/secrets/x", NOW, b"").unwrap();
        assert_eq!(check(&vault, &keys, &replay, NOW), Err(REFUSED));
    }

    #[test]
    fn a_failed_signature_leaves_no_trace_in_the_replay_memory() {
        let (keys, k, replay) = setup();
        let bad = DeviceKey::from_seed([9; 32]).sign_request_with_nonce("ann", "POST", LOGIN_PATH, NOW, b"", &"ab".repeat(16));
        assert_eq!(check(&bad, &keys, &replay, NOW), Err(REFUSED));
        assert_eq!(replay.seen.lock().unwrap().len(), 0, "a stranger fills nothing");
        // the real owner may use that nonce afterwards
        assert!(check(&k.sign_request_with_nonce("ann", "POST", LOGIN_PATH, NOW, b"", &"ab".repeat(16)), &keys, &replay, NOW).is_ok());
    }

    #[test]
    fn only_ed25519_keys_are_enrolled_and_the_list_is_bounded() {
        let keys = DeviceKeys::in_memory();
        assert!(keys.enroll("ann", "ssh-rsa AAAAB3NzaC1yc2EAAAADAQABAAABAQ x").unwrap_err().contains("RSA"));
        assert!(keys.enroll("ann", "garbage").is_err());
        assert!(keys.enroll("", &DeviceKey::from_seed([1; 32]).public_hex()).is_err());
        assert!(keys.enroll("a b", &DeviceKey::from_seed([1; 32]).public_hex()).is_err());
        for i in 0..MAX_PER_USER as u8 {
            keys.enroll("ann", &DeviceKey::from_seed([i + 10; 32]).public_hex()).unwrap();
        }
        assert!(keys.enroll("ann", &DeviceKey::from_seed([99; 32]).public_hex()).unwrap_err().contains("at most"));
        assert!(keys.enroll("ann", &DeviceKey::from_seed([10; 32]).public_hex()).is_ok(), "enrolling the same key again is not an error");
    }

    #[test]
    fn the_key_file_is_private_and_survives_a_restart() {
        use std::os::unix::fs::PermissionsExt;
        let dir = std::env::temp_dir().join(format!("iyakki-devkeys-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("device-keys");
        let k = DeviceKey::from_seed([5; 32]);
        let a = DeviceKeys::open(path.clone()).unwrap();
        a.enroll("ann", &k.ssh_public_line("x")).unwrap();
        assert_eq!(std::fs::metadata(&path).unwrap().permissions().mode() & 0o777, 0o600);
        let b = DeviceKeys::open(path.clone()).unwrap();
        assert_eq!(b.keys_of("ann"), vec![k.public()]);
        b.revoke("ann", &fingerprint(&k.public())).unwrap();
        assert!(DeviceKeys::open(path).unwrap().keys_of("ann").is_empty());
        let _ = std::fs::remove_dir_all(dir);
    }
}
