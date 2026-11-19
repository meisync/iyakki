//! The webhook channel: one signed https POST per notification. The signature is the one the flows' webhook gate verifies
//! (`flows::webhook::sign`: HMAC-SHA256 over `<timestamp>.<body>`), sent with the timestamp so a receiver can refuse a replay. https only,
//! no credentials in the address, a size cap, a timeout, and the response body is never read into the queue or the log.

use serde_json::Value;

pub const TS_HEADER: &str = "X-Iyakki-Timestamp";
pub const SIG_HEADER: &str = "X-Iyakki-Signature";
pub const MAX_PAYLOAD: usize = 16 * 1024;
pub const TIMEOUT_SECS: u64 = 10;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HttpReq {
    pub url: String,
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
    pub timeout_secs: u64,
}

/// The host of an https address, or why it is not acceptable (not https, a user name or password in it, no host).
pub fn https_host(url: &str) -> Result<String, String> {
    let rest = url.strip_prefix("https://").ok_or("a webhook address must start with https://")?;
    let authority = rest.split(['/', '?', '#']).next().unwrap_or("");
    if authority.is_empty() || authority.contains('@') {
        return Err("a webhook address must have a host and no user name or password".into());
    }
    if url.chars().any(|c| c.is_control() || c == ' ') {
        return Err("a webhook address has a space or control character".into());
    }
    let host = authority.rsplit_once(':').filter(|(_, p)| p.chars().all(|c| c.is_ascii_digit())).map_or(authority, |(h, _)| h);
    if host.is_empty() {
        return Err("a webhook address must have a host".into());
    }
    Ok(host.to_ascii_lowercase())
}

pub fn build(url: &str, secret: &[u8], ts: i64, payload: &Value) -> Result<HttpReq, String> {
    https_host(url)?;
    let body = serde_json::to_vec(payload).map_err(|e| e.to_string())?;
    if body.len() > MAX_PAYLOAD {
        return Err("the payload is too large".into());
    }
    let sig = crate::flows::webhook::sign(secret, ts, &body);
    Ok(HttpReq {
        url: url.into(),
        headers: vec![("Content-Type".into(), "application/json".into()), (TS_HEADER.into(), ts.to_string()), (SIG_HEADER.into(), sig)],
        body,
        timeout_secs: TIMEOUT_SECS,
    })
}

/// Success is a 2xx answer; anything else is retried (a 4xx other than 408/429 is not worth retrying, but the cap of tries bounds it).
pub fn status_ok(s: u16) -> bool {
    (200..300).contains(&s)
}
