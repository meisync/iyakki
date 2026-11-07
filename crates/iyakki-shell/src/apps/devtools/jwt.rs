//! JWT decoder. The signature is NOT checked unless a secret is typed and HS256 is chosen.

use super::b64;
use super::hashes::{hex, hmac_sha256};
use super::json::{self, Json};
use super::uuid_time::{iso_utc_s, now_ms};

fn part(p: &str, what: &str) -> Result<Json, String> {
    let raw = b64::decode(p).map_err(|e| format!("The {what} is not Base64URL: {e}"))?;
    let text = String::from_utf8(raw).map_err(|_| format!("The {what} is not text"))?;
    json::parse(&text).map_err(|e| format!("The {what} is not JSON: {e}"))
}

/// One of exp / iat / nbf as a readable line.
fn claim(payload: &Json, name: &str, now_s: i64) -> Option<String> {
    let Json::Obj(m) = payload else { return None };
    let Json::Num(n) = &m.iter().find(|(k, _)| k == name)?.1 else { return None };
    let t = n.parse::<f64>().ok()? as i64;
    let state = match name {
        "exp" if t <= now_s => "  EXPIRED",
        "exp" => "  still valid",
        "nbf" if t > now_s => "  NOT VALID YET",
        "nbf" => "  already valid",
        _ => "",
    };
    Some(format!("  {name}  {t}  =  {}{state}", iso_utc_s(t)))
}

/// Decode `token` (a leading "Bearer " is fine). With `secret` non-empty the HS256 signature is checked too.
pub fn decode(token: &str, secret: &str, now_s: i64) -> Result<String, String> {
    let t = token.trim();
    let t = t.strip_prefix("Bearer ").or_else(|| t.strip_prefix("bearer ")).unwrap_or(t).trim();
    let parts: Vec<&str> = t.split('.').collect();
    if parts.len() != 3 {
        return Err(format!("A JWT has three parts separated by dots; this has {}.", parts.len()));
    }
    let (h, p) = (part(parts[0], "header")?, part(parts[1], "payload")?);
    let mut out = String::from("HEADER\n");
    out.push_str(&json::to_string(&h, Some(2)));
    out.push_str("\n\nPAYLOAD\n");
    out.push_str(&json::to_string(&p, Some(2)));
    let dates: Vec<String> = ["exp", "iat", "nbf"].iter().filter_map(|n| claim(&p, n, now_s)).collect();
    if !dates.is_empty() {
        out.push_str(&format!("\n\nTIMES (UTC, now is {})\n{}", iso_utc_s(now_s), dates.join("\n")));
    }
    out.push_str("\n\nSIGNATURE\n");
    let alg = match &h {
        Json::Obj(m) => m.iter().find(|(k, _)| k == "alg").and_then(|(_, v)| if let Json::Str(s) = v { Some(s.clone()) } else { None }),
        _ => None,
    };
    if secret.is_empty() {
        out.push_str("NOT VERIFIED. This page only decodes the token; anyone can write these claims. Type a secret and press Verify HS256 to check an HS256 token.");
    } else if alg.as_deref() != Some("HS256") {
        out.push_str(&format!("NOT VERIFIED. Only HS256 can be checked here; this token says alg = {}.", alg.unwrap_or_else(|| "(none)".into())));
    } else {
        let want = hmac_sha256(secret.as_bytes(), format!("{}.{}", parts[0], parts[1]).as_bytes());
        let got = b64::decode(parts[2]).unwrap_or_default();
        // compare without stopping at the first difference
        let same = got.len() == want.len() && got.iter().zip(want.iter()).fold(0u8, |a, (x, y)| a | (x ^ y)) == 0;
        out.push_str(if same { "VALID: the HS256 signature matches this secret." } else { "INVALID: the signature does not match this secret." });
        if !same {
            out.push_str(&format!("\nExpected {}", hex(&want)));
        }
    }
    Ok(out)
}

pub fn decode_now(token: &str, secret: &str) -> Result<String, String> {
    decode(token, secret, now_ms() / 1000)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The jwt.io sample (HS256, secret "your-256-bit-secret").
    const SAMPLE: &str = "eyJhbGciOiJIUzI1NiIsInR5cCI6IkpXVCJ9.eyJzdWIiOiIxMjM0NTY3ODkwIiwibmFtZSI6IkpvaG4gRG9lIiwiaWF0IjoxNTE2MjM5MDIyfQ.SflKxwRJSMeKKF2QT4fwpMeJf36POk6yJV_adQssw5c";

    #[test]
    fn decodes_the_sample_and_says_it_is_not_verified() {
        let t = decode(SAMPLE, "", 1_700_000_000).unwrap();
        assert!(t.contains("\"alg\": \"HS256\"") && t.contains("\"name\": \"John Doe\""), "{t}");
        assert!(t.contains("iat  1516239022  =  2018-01-18T01:30:22Z"), "{t}");
        assert!(t.contains("NOT VERIFIED"), "{t}");
    }

    #[test]
    fn hs256_verifies_with_the_right_secret_only() {
        assert!(decode(SAMPLE, "your-256-bit-secret", 0).unwrap().contains("VALID: the HS256"));
        assert!(decode(SAMPLE, "wrong", 0).unwrap().contains("INVALID"));
    }

    #[test]
    fn expiry_is_called_out_and_bad_tokens_are_refused() {
        let p = b64::encode(br#"{"exp":1000,"nbf":5000}"#, true, false);
        let h = b64::encode(br#"{"alg":"none"}"#, true, false);
        let tok = format!("Bearer {h}.{p}.");
        let t = decode(&tok, "x", 2000).unwrap();
        assert!(t.contains("EXPIRED") && t.contains("NOT VALID YET") && t.contains("Only HS256"), "{t}");
        assert!(decode("a.b", "", 0).unwrap_err().contains("three parts"));
        assert!(decode("!!.e30.x", "", 0).unwrap_err().contains("header"));
    }
}
