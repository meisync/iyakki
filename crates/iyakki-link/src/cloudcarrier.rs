//! The carrier of the sealed browser data between this computer and a cloud tenant (F-VAULT-11): the tenant's own file service holds ONE
//! file for the person, the sealed profile exactly as it is on disk. The tenant never gets the key (it stays in the person's own vault), so
//! what it stores is unreadable to it. Same trust as the other connectors: HTTPS to the pinned certificate, the tenant's own account, one
//! fresh connection per request, the bearer token zeroizing and never in an error. The service is the cloud profile's streamed file route
//! (`PUT /api/v1/files/upload`, `GET /api/v1/files/download`, scope `personal`), whose rights, folder jail and size limit are its own.

use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::time::Duration;
use zeroize::Zeroizing;

use crate::cloudclient::{error_text, http, http_max, tls_open, Target};

const IO_TIMEOUT: Duration = Duration::from_secs(60);
/// The name of the one file, in the person's own area on the tenant.
pub const REMOTE_NAME: &str = "iyakki-browser-data.sealed";
/// The most a carried profile may be (the service's own limit is larger).
pub const MAX_CARRIED: usize = 64 * 1024 * 1024;

pub struct TenantFiles {
    target: Target,
    token: Zeroizing<String>,
}

fn pct(s: &str) -> String {
    s.bytes().map(|b| if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.' | b'~') { (b as char).to_string() } else { format!("%{b:02X}") }).collect()
}

fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

impl TenantFiles {
    pub fn sign_in(t: &Target, user: &str, password: &str) -> Result<TenantFiles, String> {
        let (mut tls, _) = tls_open(&t.addr, Some(t.pin), IO_TIMEOUT).map_err(|e| e.text())?;
        let body = json!({"name": user.trim(), "password": password}).to_string();
        let (st, _, b) = http(&mut tls, &t.addr, "POST", "/api/v1/login", &[("Content-Type", "application/json")], body.as_bytes()).map_err(|e| e.text())?;
        match st {
            200 => {
                let token = serde_json::from_slice::<Value>(&b).ok().and_then(|v| v["token"].as_str().map(str::to_string)).filter(|t| !t.is_empty()).ok_or("the tenant gave no session")?;
                Ok(TenantFiles { target: t.clone(), token: Zeroizing::new(token) })
            }
            s => Err(format!("the tenant refused the sign-in ({s}): {}", error_text(&b, s))),
        }
    }

    fn path(op: &str) -> String {
        format!("/api/v1/files/{op}?scope=personal&path={}", pct(REMOTE_NAME))
    }

    /// Put the sealed file on the tenant (replacing the one that was there). The hash is checked by the service against what arrived.
    pub fn put(&self, bytes: &[u8]) -> Result<String, String> {
        if bytes.is_empty() || bytes.len() > MAX_CARRIED {
            return Err(format!("the sealed browser data must be between 1 byte and {} MB to be carried", MAX_CARRIED >> 20));
        }
        let sum = hex(&Sha256::digest(bytes));
        let (mut tls, _) = tls_open(&self.target.addr, Some(self.target.pin), IO_TIMEOUT).map_err(|e| e.text())?;
        let auth = format!("Bearer {}", self.token.as_str());
        let path = Self::path("upload");
        let (st, _, b) = http(&mut tls, &self.target.addr, "PUT", &path, &[("Authorization", &auth), ("Content-Type", "application/octet-stream"), ("X-Content-SHA256", &sum)], bytes).map_err(|e| e.text())?;
        match st {
            200 => Ok(sum),
            401 => Err("the tenant signed you out: sign in again".into()),
            s => Err(format!("the tenant answered {s}: {}", error_text(&b, s))),
        }
    }

    /// The sealed file the tenant holds; `Ok(None)` when it holds none.
    pub fn get(&self) -> Result<Option<Vec<u8>>, String> {
        let (mut tls, _) = tls_open(&self.target.addr, Some(self.target.pin), IO_TIMEOUT).map_err(|e| e.text())?;
        let auth = format!("Bearer {}", self.token.as_str());
        let path = Self::path("download");
        let (st, headers, b) = http_max(&mut tls, &self.target.addr, "GET", &path, &[("Authorization", &auth)], &[], MAX_CARRIED).map_err(|e| e.text())?;
        match st {
            200 => {
                let want = headers.iter().find(|(k, _)| k == "content-length").and_then(|(_, v)| v.parse::<usize>().ok()).unwrap_or(0);
                if want > MAX_CARRIED || b.len() != want {
                    return Err("the tenant's copy did not arrive whole; nothing was changed".into());
                }
                Ok(Some(b))
            }
            404 => Ok(None),
            401 => Err("the tenant signed you out: sign in again".into()),
            s => Err(format!("the tenant answered {s}: {}", error_text(&b, s))),
        }
    }

    pub fn sign_out(&self) {
        if let Ok((mut tls, _)) = tls_open(&self.target.addr, Some(self.target.pin), IO_TIMEOUT) {
            let auth = format!("Bearer {}", self.token.as_str());
            let _ = http(&mut tls, &self.target.addr, "POST", "/api/v1/logout", &[("Authorization", &auth), ("Content-Type", "application/json")], b"{}");
        }
    }
}
