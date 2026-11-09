//! HTTP-style sign-in (RFC 7617 Basic, RFC 2617 / 7616 Digest with MD5), used by RTSP and the HTTP Motion-JPEG client.
//! Digest is preferred when a camera offers both; Basic sends the password readable by anyone on the network path, which the caller may
//! want to say.

use crate::hash::{base64_encode, md5_hex};

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Challenge {
    Basic { realm: String },
    Digest { realm: String, nonce: String, opaque: Option<String>, qop_auth: bool },
}

/// The key=value list after the scheme name; values may be quoted and may contain commas.
pub(crate) fn params(s: &str) -> Vec<(String, String)> {
    let mut out = Vec::new();
    let b: Vec<char> = s.chars().collect();
    let mut i = 0;
    while i < b.len() && out.len() < 20 {
        while i < b.len() && (b[i] == ',' || b[i].is_whitespace()) {
            i += 1;
        }
        let ks = i;
        while i < b.len() && b[i] != '=' && b[i] != ',' {
            i += 1;
        }
        let key: String = b[ks..i].iter().collect::<String>().trim().to_ascii_lowercase();
        if i >= b.len() || b[i] != '=' {
            continue;
        }
        i += 1;
        let mut val = String::new();
        if i < b.len() && b[i] == '"' {
            i += 1;
            while i < b.len() && b[i] != '"' {
                if b[i] == '\\' && i + 1 < b.len() {
                    i += 1;
                }
                val.push(b[i]);
                i += 1;
            }
            i += 1;
        } else {
            while i < b.len() && b[i] != ',' {
                val.push(b[i]);
                i += 1;
            }
        }
        if !key.is_empty() {
            out.push((key, val.trim().to_string()));
        }
    }
    out
}

/// The best challenge among the `WWW-Authenticate` header values of a 401. `Err` for a scheme or algorithm that is not supported.
pub fn choose(headers: &[&str]) -> Result<Challenge, String> {
    let mut basic = None;
    let mut digest = None;
    let mut unsupported = None;
    for h in headers {
        let h = h.trim();
        let (scheme, rest) = h.split_once(' ').unwrap_or((h, ""));
        match scheme.to_ascii_lowercase().as_str() {
            "basic" => basic = Some(Challenge::Basic { realm: params(rest).into_iter().find(|p| p.0 == "realm").map(|p| p.1).unwrap_or_default() }),
            "digest" => {
                let p = params(rest);
                let get = |k: &str| p.iter().find(|x| x.0 == k).map(|x| x.1.clone());
                match get("algorithm").map(|a| a.to_ascii_uppercase()).as_deref() {
                    None | Some("MD5") => {}
                    Some(a) => {
                        unsupported = Some(format!("the camera wants {a} digest sign-in, which is not supported yet (only MD5)"));
                        continue;
                    }
                }
                let Some(nonce) = get("nonce") else { continue };
                let qop_auth = get("qop").is_some_and(|q| q.split(',').any(|x| x.trim().eq_ignore_ascii_case("auth")));
                digest = Some(Challenge::Digest { realm: get("realm").unwrap_or_default(), nonce, opaque: get("opaque"), qop_auth });
            }
            _ => {}
        }
    }
    digest.or(basic).ok_or_else(|| unsupported.unwrap_or_else(|| "the camera asked for a sign-in method that is not supported".into()))
}

pub struct Auth {
    pub challenge: Challenge,
    user: String,
    password: String,
    nc: u32,
    cnonce_seed: u64,
}

fn q(s: &str) -> String {
    s.replace('\\', "\\\\").replace('"', "\\\"")
}

/// The Digest `response` value (RFC 2617 section 3.2.2): pure, for tests with known vectors.
#[allow(clippy::too_many_arguments)]
pub fn digest_response(user: &str, realm: &str, password: &str, method: &str, uri: &str, nonce: &str, qop: Option<(&str, &str)>) -> String {
    let ha1 = md5_hex(format!("{user}:{realm}:{password}").as_bytes());
    let ha2 = md5_hex(format!("{method}:{uri}").as_bytes());
    match qop {
        Some((nc_cnonce, qop_name)) => {
            // nc_cnonce is "nc:cnonce"
            md5_hex(format!("{ha1}:{nonce}:{nc_cnonce}:{qop_name}:{ha2}").as_bytes())
        }
        None => md5_hex(format!("{ha1}:{nonce}:{ha2}").as_bytes()),
    }
}

impl Auth {
    pub fn new(challenge: Challenge, user: &str, password: &str) -> Auth {
        let seed = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_nanos() as u64).unwrap_or(1);
        Auth { challenge, user: user.to_string(), password: password.to_string(), nc: 0, cnonce_seed: seed }
    }

    /// Fix the client nonce (tests).
    pub fn with_seed(mut self, seed: u64) -> Auth {
        self.cnonce_seed = seed;
        self
    }

    pub fn is_basic(&self) -> bool {
        matches!(self.challenge, Challenge::Basic { .. })
    }

    /// The `Authorization` header value for one request.
    pub fn header(&mut self, method: &str, uri: &str) -> String {
        match &self.challenge {
            Challenge::Basic { .. } => format!("Basic {}", base64_encode(format!("{}:{}", self.user, self.password).as_bytes())),
            Challenge::Digest { realm, nonce, opaque, qop_auth } => {
                self.nc += 1;
                let nc = format!("{:08x}", self.nc);
                let cnonce = md5_hex(format!("{}:{}", self.cnonce_seed, self.nc).as_bytes())[..16].to_string();
                let resp = if *qop_auth {
                    digest_response(&self.user, realm, &self.password, method, uri, nonce, Some((&format!("{nc}:{cnonce}"), "auth")))
                } else {
                    digest_response(&self.user, realm, &self.password, method, uri, nonce, None)
                };
                let mut h = format!("Digest username=\"{}\", realm=\"{}\", nonce=\"{}\", uri=\"{}\", response=\"{}\"", q(&self.user), q(realm), q(nonce), q(uri), resp);
                if *qop_auth {
                    h.push_str(&format!(", qop=auth, nc={nc}, cnonce=\"{cnonce}\""));
                }
                if let Some(o) = opaque {
                    h.push_str(&format!(", opaque=\"{}\"", q(o)));
                }
                h
            }
        }
    }
}

impl std::fmt::Debug for Auth {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Auth({})", if self.is_basic() { "basic" } else { "digest" })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_rfc_2617_digest_example_gives_the_documented_response() {
        let r = digest_response("Mufasa", "testrealm@host.com", "Circle Of Life", "GET", "/dir/index.html", "dcd98b7102dd2f0e8b11d0f600bfb0c093", Some(("00000001:0a4f113b", "auth")));
        assert_eq!(r, "6629fae49393a05397450978507c4ef1");
        // RFC 2069 style (no qop)
        assert_eq!(digest_response("Mufasa", "testrealm@host.com", "Circle Of Life", "GET", "/dir/index.html", "dcd98b7102dd2f0e8b11d0f600bfb0c093", None), "670fd8c2df070c60b045671b8b24ff02");
    }

    #[test]
    fn challenges_are_read_with_quoted_commas_and_digest_wins_over_basic() {
        let c = choose(&["Basic realm=\"cam, one\"", "Digest realm=\"IPCAM\", nonce=\"abc,def\", qop=\"auth,auth-int\", opaque=\"xyz\""]).unwrap();
        assert_eq!(c, Challenge::Digest { realm: "IPCAM".into(), nonce: "abc,def".into(), opaque: Some("xyz".into()), qop_auth: true });
        assert_eq!(choose(&["Basic realm=\"cam, one\""]).unwrap(), Challenge::Basic { realm: "cam, one".into() });
        assert!(choose(&["Digest realm=\"x\", nonce=\"n\", algorithm=SHA-256"]).unwrap_err().contains("SHA-256"));
        assert!(choose(&["Negotiate"]).is_err());
        assert!(choose(&[]).is_err());
        assert!(choose(&["Digest realm=\"x\""]).is_err(), "a digest challenge without a nonce is useless");
    }

    #[test]
    fn headers_carry_the_response_and_count_up_and_basic_is_plain_base64() {
        let mut a = Auth::new(Challenge::Digest { realm: "r".into(), nonce: "n".into(), opaque: None, qop_auth: true }, "u\"x", "p").with_seed(7);
        let h1 = a.header("DESCRIBE", "rtsp://h/x");
        let h2 = a.header("SETUP", "rtsp://h/x/t1");
        assert!(h1.contains("nc=00000001") && h2.contains("nc=00000002"));
        assert!(h1.contains("username=\"u\\\"x\"") && h1.contains("qop=auth"));
        assert!(!h1.contains(" p\"") && !h1.contains("password"));
        let mut b = Auth::new(Challenge::Basic { realm: "r".into() }, "Aladdin", "open sesame");
        assert_eq!(b.header("GET", "/"), "Basic QWxhZGRpbjpvcGVuIHNlc2FtZQ==");
        assert!(format!("{b:?}").find("sesame").is_none());
    }

    #[test]
    fn hostile_parameter_lists_do_not_panic() {
        for s in ["Digest ====", "Digest ,,,,", "Digest realm=\"unterminated", "Digest a=\"\\", "Digest =x,=y", "Basic"] {
            let _ = choose(&[s]);
        }
        let long = format!("Digest {}", "a=b,".repeat(10_000));
        let _ = choose(&[&long]);
    }
}
