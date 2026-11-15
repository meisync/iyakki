//! Connecting a client (docs/development/CLIENT-SPLIT-DESIGN.md section 5): the experimental switch, the one-time link and the list of
//! enrolled client devices. EXPERIMENTAL and off by default.
//!
//! Files, all under `<data>/client/`, mode 0600, written atomically:
//! * `enabled` exists: the session endpoint answers. Absent: every route of it is a plain 404.
//! * `tokens.json`: one-time tokens as SHA-256 hashes (the token itself is shown once and never stored), with their expiry. A redeemed token
//!   stays as a used mark until it would have expired, so a second try says `used` and not `unknown`.
//! * `devices.json`: the enrolled devices: id, name, Ed25519 public key, who made the link, created, last seen, revoked.
//!
//! The challenge a device signs is the `IYAKKI-SIG` scheme of the vault (`iyakki_link::sigscheme`), over the method `GET`, the path
//! `/client/ws` and the challenge text of the connection, so a signature cannot be reused on another connection or for another route.
//! The shell's Settings and the session endpoint (`iyakki-remote`) share this module, so a revoke made in Settings is seen by an open session.

use std::path::{Path, PathBuf};
use std::sync::Mutex;

use iyakki_core_api::client::WS_PATH;
use iyakki_link::sigscheme::{hex, parse_authorization, parse_public};
/// The device key type, for the programs that make or test client keys without a second dependency.
pub use iyakki_link::sigscheme::DeviceKey;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

/// A link lives this long unless asked for less.
pub const TOKEN_TTL_SECS: i64 = 600;
/// The longest a link can live.
pub const TOKEN_MAX_TTL_SECS: i64 = 900;
/// Links that are made and not yet redeemed or expired.
pub const MAX_OPEN_TOKENS: usize = 8;
/// Devices that are enrolled and not revoked.
pub const MAX_DEVICES: usize = 16;
/// Revoked devices remembered in the list (the oldest are dropped past this).
const MAX_REVOKED_KEPT: usize = 32;
/// How far the signed time may be from the instance's clock.
pub const SIG_WINDOW_SECS: u64 = 60;
/// `last_seen` is written at most this often per device.
const TOUCH_EVERY_SECS: i64 = 30;

static LOCK: Mutex<()> = Mutex::new(());

fn lock() -> std::sync::MutexGuard<'static, ()> {
    LOCK.lock().unwrap_or_else(|e| e.into_inner())
}

fn folder(data: &Path) -> PathBuf {
    data.join("client")
}

/// Is the experimental session endpoint switched on?
pub fn enabled(data: &Path) -> bool {
    folder(data).join("enabled").is_file()
}

pub fn set_enabled(data: &Path, on: bool) -> Result<(), String> {
    let _g = lock();
    let marker = folder(data).join("enabled");
    if on {
        write_private(&marker, b"1\n")
    } else {
        match std::fs::remove_file(&marker) {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(e.to_string()),
        }
    }
}

fn write_private(path: &Path, bytes: &[u8]) -> Result<(), String> {
    use std::io::Write;
    use std::os::unix::fs::OpenOptionsExt;
    if let Some(d) = path.parent() {
        std::fs::create_dir_all(d).map_err(|e| format!("{}: {e}", d.display()))?;
    }
    let tmp = path.with_extension("new");
    let _ = std::fs::remove_file(&tmp);
    let mut f = std::fs::OpenOptions::new().write(true).create_new(true).mode(0o600).open(&tmp).map_err(|e| format!("{}: {e}", tmp.display()))?;
    f.write_all(bytes).and_then(|_| f.sync_all()).map_err(|e| e.to_string())?;
    std::fs::rename(&tmp, path).map_err(|e| e.to_string())
}

fn random_hex(n: usize) -> Result<String, String> {
    let mut b = vec![0u8; n];
    iyakki_sec::random_fill(&mut b).map_err(|e| format!("no random numbers: {e:?}"))?;
    Ok(hex(&b))
}

fn hash(token: &str) -> String {
    hex(&Sha256::digest(token.as_bytes()))
}

// ------------------------------------------------------------------------------------------------------------------ tokens

#[derive(Clone, Debug, Serialize, Deserialize)]
struct TokenRow {
    hash: String,
    expires: i64,
    by: String,
    #[serde(default)]
    used: bool,
}

fn read_tokens(data: &Path) -> Vec<TokenRow> {
    std::fs::read(folder(data).join("tokens.json")).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default()
}

fn write_tokens(data: &Path, rows: &[TokenRow]) -> Result<(), String> {
    write_private(&folder(data).join("tokens.json"), serde_json::to_string(rows).map_err(|e| e.to_string())?.as_bytes())
}

/// Why a token was not accepted. The words are for the person; the codes of the wire are in `iyakki_core_api::client::ErrorCode`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TokenError {
    Unknown,
    Expired,
    Used,
}

/// Make a one-time token for `by` (the signed-in person who made the link). Returns the token, to be shown once.
pub fn create_token(data: &Path, by: &str, now: i64, ttl: i64) -> Result<String, String> {
    let _g = lock();
    let mut rows = read_tokens(data);
    rows.retain(|r| r.expires > now);
    if rows.iter().filter(|r| !r.used).count() >= MAX_OPEN_TOKENS {
        return Err(format!("{MAX_OPEN_TOKENS} links are open already: wait for one to expire"));
    }
    let token = random_hex(32)?;
    rows.push(TokenRow { hash: hash(&token), expires: now + ttl.clamp(30, TOKEN_MAX_TTL_SECS), by: by.chars().filter(|c| !c.is_control()).take(80).collect(), used: false });
    write_tokens(data, &rows)?;
    Ok(token)
}

/// Spend a token. On success, the person who made it. A token works once: the mark is written before this returns.
pub fn redeem_token(data: &Path, token: &str, now: i64) -> Result<String, TokenError> {
    let _g = lock();
    let mut rows = read_tokens(data);
    let h = hash(token);
    let Some(i) = rows.iter().position(|r| same(&r.hash, &h)) else { return Err(TokenError::Unknown) };
    if rows[i].used {
        return Err(TokenError::Used);
    }
    if rows[i].expires <= now {
        return Err(TokenError::Expired);
    }
    rows[i].used = true;
    let by = rows[i].by.clone();
    rows.retain(|r| r.expires > now || r.hash == h);
    // not written down means not spent: fail closed
    write_tokens(data, &rows).map_err(|_| TokenError::Unknown)?;
    Ok(by)
}

fn same(a: &str, b: &str) -> bool {
    a.len() == b.len() && a.bytes().zip(b.bytes()).fold(0u8, |d, (x, y)| d | (x ^ y)) == 0
}

/// How many links are open (made, not redeemed, not expired).
pub fn open_tokens(data: &Path, now: i64) -> usize {
    read_tokens(data).iter().filter(|r| !r.used && r.expires > now).count()
}

/// The link to show: the address of the instance, `/connect`, and the token in the fragment (never sent to a server, so never in a log).
pub fn link_url(base: &str, token: &str) -> String {
    format!("{}/connect#{token}", base.trim_end_matches('/'))
}

/// The link with the instance's certificate fingerprint in it (`/connect?pin=HEX#token`), for a machine that has no public certificate: the
/// window program (`iyakki-connect window`) then pins that certificate at once and asks nothing. `pin` is the SHA-256 as `AB:CD:..` or hex;
/// anything else is left out (the link then works without it). The fingerprint is not secret.
pub fn link_url_pinned(base: &str, token: &str, pin: Option<&str>) -> String {
    let hex: String = pin.map(|p| p.chars().filter(|c| *c != ':' && !c.is_whitespace()).collect::<String>().to_ascii_lowercase()).unwrap_or_default();
    if hex.len() == 64 && hex.bytes().all(|c| c.is_ascii_hexdigit()) {
        format!("{}/connect?pin={hex}#{token}", base.trim_end_matches('/'))
    } else {
        link_url(base, token)
    }
}

// ------------------------------------------------------------------------------------------------------------------ devices

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Device {
    pub id: String,
    pub name: String,
    /// Ed25519 public key, 64 hex digits.
    pub public: String,
    /// The person who made the link.
    pub by: String,
    pub created: i64,
    pub last_seen: i64,
    /// When it was revoked (none: active).
    #[serde(default)]
    pub revoked: Option<i64>,
}

impl Device {
    pub fn active(&self) -> bool {
        self.revoked.is_none()
    }

    pub fn public_key(&self) -> Option<[u8; 32]> {
        parse_public(&self.public).ok()
    }
}

fn read_devices(data: &Path) -> Vec<Device> {
    std::fs::read(folder(data).join("devices.json")).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default()
}

fn write_devices(data: &Path, rows: &[Device]) -> Result<(), String> {
    write_private(&folder(data).join("devices.json"), serde_json::to_string(rows).map_err(|e| e.to_string())?.as_bytes())
}

/// A device name a person can read in a list: 1 to 60 characters, no control characters.
pub fn clean_name(name: &str) -> Result<String, String> {
    let n = name.trim();
    if n.is_empty() || n.chars().count() > 60 || n.chars().any(|c| c.is_control()) {
        return Err("a device name is 1 to 60 characters, without control characters".into());
    }
    Ok(n.to_string())
}

/// Enrol a device: its public key (64 hex digits or an `ssh-ed25519` line) and name. The caller has already spent a token.
pub fn enrol_device(data: &Path, name: &str, public: &str, by: &str, now: i64) -> Result<Device, String> {
    let name = clean_name(name)?;
    let key = parse_public(public)?;
    let _g = lock();
    let mut rows = read_devices(data);
    if rows.iter().any(|d| d.active() && d.public_key() == Some(key)) {
        return Err("that device key is enrolled already".into());
    }
    if rows.iter().filter(|d| d.active()).count() >= MAX_DEVICES {
        return Err(format!("at most {MAX_DEVICES} devices: revoke one first"));
    }
    let dev = Device { id: format!("d{}", random_hex(10)?), name, public: hex(&key), by: by.chars().filter(|c| !c.is_control()).take(80).collect(), created: now, last_seen: 0, revoked: None };
    rows.push(dev.clone());
    let revoked = rows.iter().filter(|d| !d.active()).count();
    if revoked > MAX_REVOKED_KEPT {
        let mut drop = revoked - MAX_REVOKED_KEPT;
        rows.retain(|d| {
            if !d.active() && drop > 0 {
                drop -= 1;
                false
            } else {
                true
            }
        });
    }
    write_devices(data, &rows)?;
    Ok(dev)
}

/// Every device, newest first (revoked ones too, marked).
pub fn list_devices(data: &Path) -> Vec<Device> {
    let mut v = read_devices(data);
    v.reverse();
    v
}

pub fn get_device(data: &Path, id: &str) -> Option<Device> {
    read_devices(data).into_iter().find(|d| d.id == id)
}

/// Revoke a device. True when it was active. Its open sessions close within a second (the endpoint reads this file).
pub fn revoke_device(data: &Path, id: &str, now: i64) -> Result<bool, String> {
    let _g = lock();
    let mut rows = read_devices(data);
    let Some(d) = rows.iter_mut().find(|d| d.id == id && d.active()) else { return Ok(false) };
    d.revoked = Some(now);
    write_devices(data, &rows)?;
    Ok(true)
}

/// Note that a device was seen (at most once in half a minute, so a busy session does not rewrite the file).
pub fn touch_device(data: &Path, id: &str, now: i64) {
    let _g = lock();
    let mut rows = read_devices(data);
    if let Some(d) = rows.iter_mut().find(|d| d.id == id) {
        if now - d.last_seen >= TOUCH_EVERY_SECS {
            d.last_seen = now;
            let _ = write_devices(data, &rows);
        }
    }
}

// ------------------------------------------------------------------------------------------------------------------ challenge

/// A fresh challenge for one connection: 32 random bytes as hex.
pub fn new_challenge() -> Result<String, String> {
    random_hex(32)
}

/// Why a signed answer was not accepted.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AuthError {
    /// Not an `IYAKKI-SIG` header, or it names another device.
    Malformed,
    /// Not signed by the device's key over this challenge, or the time is outside the window.
    BadSignature,
}

/// Check a device's answer to `challenge`: `authorization` must name `device`, carry a time within the window and be that device's
/// signature over (`GET`, [`WS_PATH`], the challenge).
pub fn verify_answer(device: &Device, authorization: &str, challenge: &str, now: u64) -> Result<(), AuthError> {
    let req = parse_authorization(authorization).ok_or(AuthError::Malformed)?;
    if req.id != device.id {
        return Err(AuthError::Malformed);
    }
    if req.ts.abs_diff(now) > SIG_WINDOW_SECS {
        return Err(AuthError::BadSignature);
    }
    let key = device.public_key().ok_or(AuthError::BadSignature)?;
    if req.verify(&key, "GET", WS_PATH, challenge.as_bytes()) {
        Ok(())
    } else {
        Err(AuthError::BadSignature)
    }
}

/// The client side of the same step (the engine, `iyakki-connect`, tests): sign `challenge` as `device_id`.
pub fn sign_answer(key: &DeviceKey, device_id: &str, challenge: &str, now: u64) -> Result<String, String> {
    key.sign_request(device_id, "GET", WS_PATH, now, challenge.as_bytes())
}

#[cfg(test)]
mod tests;
