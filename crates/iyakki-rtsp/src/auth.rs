//! HTTP-style sign-in (RFC 7617 Basic, RFC 2617 / 7616 Digest with MD5 or SHA-256), used by RTSP and the HTTP Motion-JPEG client.
//! Digest is preferred when a camera offers both; Basic sends the password readable by anyone on the network path, which the caller may
//! want to say.

use crate::hash::{base64_encode, md5_hex, sha256_hex};

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Challenge {
    Basic { realm: String },
    /// `sha256`: the camera asked for `algorithm=SHA-256` (RFC 7616) instead of MD5.
    Digest { realm: String, nonce: String, opaque: Option<String>, qop_auth: bool, sha256: bool },
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
    let mut digest: Option<Challenge> = None;
    let mut unsupported = None;
    for h in headers {
        let h = h.trim();
        let (scheme, rest) = h.split_once(' ').unwrap_or((h, ""));
        match scheme.to_ascii_lowercase().as_str() {
            "basic" => basic = Some(Challenge::Basic { realm: params(rest).into_iter().find(|p| p.0 == "realm").map(|p| p.1).unwrap_or_default() }),
            "digest" => {
                let p = params(rest);
                let get = |k: &str| p.iter().find(|x| x.0 == k).map(|x| x.1.clone());
                let sha256 = match get("algorithm").map(|a| a.to_ascii_uppercase()).as_deref() {
                    None | Some("MD5") => false,
                    Some("SHA-256") => true,
                    Some(a) => {
                        unsupported = Some(format!("the camera wants {a} digest sign-in, which is not supported yet (only MD5 and SHA-256)"));
                        continue;
                    }
                };
                let Some(nonce) = get("nonce") else { continue };
                let qop_auth = get("qop").is_some_and(|q| q.split(',').any(|x| x.trim().eq_ignore_ascii_case("auth")));
                // a camera that offers both is answered with the stronger one
                if sha256 || !matches!(digest, Some(Challenge::Digest { sha256: true, .. })) {
                    digest = Some(Challenge::Digest { realm: get("realm").unwrap_or_default(), nonce, opaque: get("opaque"), qop_auth, sha256 });
                }
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
    digest_response_with(false, user, realm, password, method, uri, nonce, qop)
}

/// The Digest `response` value with MD5 (`sha256` false) or SHA-256 (RFC 7616 section 3.4.1).
#[allow(clippy::too_many_arguments)]
pub fn digest_response_with(sha256: bool, user: &str, realm: &str, password: &str, method: &str, uri: &str, nonce: &str, qop: Option<(&str, &str)>) -> String {
    let h = |s: String| if sha256 { sha256_hex(s.as_bytes()) } else { md5_hex(s.as_bytes()) };
    let ha1 = h(format!("{user}:{realm}:{password}"));
    let ha2 = h(format!("{method}:{uri}"));
    match qop {
        Some((nc_cnonce, qop_name)) => {
            // nc_cnonce is "nc:cnonce"
            h(format!("{ha1}:{nonce}:{nc_cnonce}:{qop_name}:{ha2}"))
        }
        None => h(format!("{ha1}:{nonce}:{ha2}")),
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
            Challenge::Digest { realm, nonce, opaque, qop_auth, sha256 } => {
                self.nc += 1;
                let nc = format!("{:08x}", self.nc);
                let cnonce = md5_hex(format!("{}:{}", self.cnonce_seed, self.nc).as_bytes())[..16].to_string();
                let resp = if *qop_auth {
                    digest_response_with(*sha256, &self.user, realm, &self.password, method, uri, nonce, Some((&format!("{nc}:{cnonce}"), "auth")))
                } else {
                    digest_response_with(*sha256, &self.user, realm, &self.password, method, uri, nonce, None)
                };
                let mut h = format!("Digest username=\"{}\", realm=\"{}\", nonce=\"{}\", uri=\"{}\", response=\"{}\"", q(&self.user), q(realm), q(nonce), q(uri), resp);
                if *sha256 {
                    h.push_str(", algorithm=SHA-256");
                }
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
        assert_eq!(c, Challenge::Digest { realm: "IPCAM".into(), nonce: "abc,def".into(), opaque: Some("xyz".into()), qop_auth: true, sha256: false });
        assert_eq!(choose(&["Basic realm=\"cam, one\""]).unwrap(), Challenge::Basic { realm: "cam, one".into() });
        assert!(choose(&["Digest realm=\"x\", nonce=\"n\", algorithm=SHA-512-256"]).unwrap_err().contains("SHA-512-256"));
        assert!(matches!(choose(&["Digest realm=\"x\", nonce=\"n\", algorithm=SHA-256"]).unwrap(), Challenge::Digest { sha256: true, .. }));
        let both = choose(&["Digest realm=\"x\", nonce=\"m\"", "Digest realm=\"x\", nonce=\"s\", algorithm=SHA-256", "Digest realm=\"x\", nonce=\"m2\", algorithm=MD5"]).unwrap();
        assert!(matches!(both, Challenge::Digest { ref nonce, sha256: true, .. } if nonce == "s"), "the stronger one wins wherever it stands: {both:?}");
        assert!(choose(&["Negotiate"]).is_err());
        assert!(choose(&[]).is_err());
        assert!(choose(&["Digest realm=\"x\""]).is_err(), "a digest challenge without a nonce is useless");
    }

    #[test]
    fn the_rfc_7616_sha_256_example_and_the_hash_itself_match_the_documents() {
        assert_eq!(sha256_hex(b"abc"), "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad");
        assert_eq!(sha256_hex(b""), "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855");
        assert_eq!(sha256_hex(&[b'a'; 1000]), "41edece42d63e8d9bf515a9ba6932e1c20cbc9f5a5d134645adb5db1b9737ea3");
        // RFC 7616 section 3.9.1
        let r = digest_response_with(true, "Mufasa", "http-auth@example.org", "Circle of Life", "GET", "/dir/index.html", "7ypf/xlj9XXwfDPEoM4URrv/xwf94BcCAzFZH4GiTo0v", Some(("00000001:f2/wE4q74E6zIJEtWaHKaf5wv/H5QzzpXusqGemxURZJ", "auth")));
        assert_eq!(r, "753927fa0e85d155564e2e272a28d1802ca10daf4496794697cf8db5856cb6c1");
        let mut a = Auth::new(Challenge::Digest { realm: "r".into(), nonce: "n".into(), opaque: None, qop_auth: true, sha256: true }, "u", "p").with_seed(3);
        let h = a.header("GET", "/x");
        assert!(h.contains("algorithm=SHA-256") && h.contains("response=\""), "{h}");
    }

    #[test]
    fn headers_carry_the_response_and_count_up_and_basic_is_plain_base64() {
        let mut a = Auth::new(Challenge::Digest { realm: "r".into(), nonce: "n".into(), opaque: None, qop_auth: true, sha256: false }, "u\"x", "p").with_seed(7);
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
