//! The licence a seller hands over, as it travels (F-APPSTORE-13): the marker in a link, `extract_token` (a token from pasted text, a file,
//! a link or an `iyakki-licence:` line) and the navigation hook `token_in_address` the browser tile uses. Nothing here verifies a token:
//! `Token::verify` (signature, issuer, device, account, expiry, revocation) is the only trust, whatever way the token arrived.

use crate::store_license::MAX_TOKEN_BYTES;

/// The marker in a link, an `iyakki-licence:` line or a QR code.
pub const MARK: &str = "iyakki-licence";

/// Where the browser tile leaves a licence that arrived on the seller's return page (app data of the person, namespace and key): the
/// STORE tile takes it from there and adds it when the person presses the key it names.
pub const PENDING_APP: &str = "store";
pub const PENDING_KEY: &str = "pending-licence";

/// The navigation hook (F-APPSTORE-13): the licence token carried by an address the browser tile has just reached, or `None` when the
/// address carries none. Only `https` pages and the `iyakki-licence:` scheme count, and only the fragment or the query is read (never the
/// path); the token is not verified here: the STORE tile's add path does that offline (`Token::verify`), so a page can offer a token but
/// can never make one valid.
pub fn token_in_address(url: &str) -> Option<String> {
    let u = url.trim();
    if u.len() > MAX_TOKEN_BYTES * 2 + 2048 {
        return None;
    }
    let carries = if u.starts_with(&format!("{MARK}:")) {
        true
    } else if u.starts_with("https://") {
        let rest = u.split_once('#').map(|x| x.1).into_iter().chain(u.split_once('?').map(|x| x.1.split('#').next().unwrap_or(""))).collect::<Vec<_>>();
        rest.iter().any(|r| r.split('&').any(|kv| kv.starts_with(&format!("{MARK}="))))
    } else {
        false
    };
    if carries { extract_token(u).ok() } else { None }
}

fn unb64(s: &str) -> Option<Vec<u8>> {
    let mut out = Vec::new();
    let (mut acc, mut bits) = (0u32, 0u32);
    for c in s.trim().trim_end_matches('=').chars() {
        let v = match c {
            'A'..='Z' => c as u32 - 'A' as u32,
            'a'..='z' => c as u32 - 'a' as u32 + 26,
            '0'..='9' => c as u32 - '0' as u32 + 52,
            '+' | '-' => 62,
            '/' | '_' => 63,
            _ => return None,
        };
        acc = (acc << 6) | v;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((acc >> bits) as u8);
            acc &= (1 << bits) - 1;
        }
        if out.len() > MAX_TOKEN_BYTES {
            return None;
        }
    }
    Some(out)
}

/// The token JSON from what the person pasted, opened from a file or scanned. Does not verify anything.
pub fn extract_token(input: &str) -> Result<String, String> {
    let t = input.trim();
    if t.len() > MAX_TOKEN_BYTES * 2 {
        return Err("that is too long to be a licence".into());
    }
    if t.starts_with('{') {
        return Ok(t.to_string());
    }
    let payload = if let Some(p) = t.strip_prefix(&format!("{MARK}:")) {
        Some(p)
    } else {
        // a link: the token rides in the fragment or the query, never in the path
        let (_, after) = t.split_once('#').or_else(|| t.split_once('?')).unwrap_or(("", ""));
        after.split('&').find_map(|kv| kv.strip_prefix(&format!("{MARK}=")))
    };
    let p = payload.ok_or("that is not a licence: paste the token, or the link or code the seller gave you")?;
    let bytes = unb64(p).ok_or("the licence text is damaged")?;
    let s = String::from_utf8(bytes).map_err(|_| "the licence text is damaged".to_string())?;
    if !s.trim_start().starts_with('{') {
        return Err("the licence text is damaged".into());
    }
    Ok(s)
}

