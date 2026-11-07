//! The real connection caller: one authenticated GET of the model-list address, through the shared HTTP client of the egress proxy
//! (`iyakki_proxy::https::UreqUpstream`: no redirects, a short timeout, any status returned). The tenant's `egress.policy` narrows it:
//! a host the policy does not allow is refused before any connection. Only the status (and, for discovery, model names parsed from the
//! answer) leave this function; the key is used for the request only.

use super::catalog::{ConnectionTester, TestRequest, TestResult};
use iyakki_proxy::Upstream;
use std::path::PathBuf;
use std::time::Duration;

pub struct RealTester {
    pub dir: PathBuf,
}

fn headers(auth: &str, key: &str) -> Vec<(String, String)> {
    let mut h = vec![("Accept".to_string(), "application/json".to_string())];
    if key.is_empty() {
        return h;
    }
    match auth {
        "x-api-key" => {
            h.push(("x-api-key".into(), key.into()));
            h.push(("anthropic-version".into(), "2023-06-01".into()));
        }
        "x-goog-api-key" => h.push(("x-goog-api-key".into(), key.into())),
        "api-key" => h.push(("api-key".into(), key.into())),
        _ => h.push(("Authorization".into(), format!("Bearer {key}"))),
    }
    h
}

impl RealTester {
    fn get(&self, req: &TestRequest) -> Result<iyakki_proxy::UpstreamResponse, String> {
        let host = iyakki_sec::url_host(req.url).ok_or("not an address")?;
        if super::enforce::egress_allows(&self.dir, None, &host) == Some(false) {
            return Err("the tenant's egress policy does not allow this host".into());
        }
        let up = iyakki_proxy::https::UreqUpstream::with_timeout(Duration::from_secs(8));
        up.request("GET", req.url, &headers(req.auth, req.key), b"")
    }
}

impl ConnectionTester for RealTester {
    fn test(&self, req: &TestRequest) -> Result<TestResult, String> {
        match self.get(req) {
            Ok(r) => Ok(TestResult { ok: (200..300).contains(&r.status), status: r.status }),
            Err(m) if m.contains("egress policy") => Err(m),
            Err(_) => Ok(TestResult { ok: false, status: 0 }),
        }
    }
    fn list_models(&self, req: &TestRequest, style: &str) -> Result<Vec<String>, String> {
        let r = self.get(req)?;
        if !(200..300).contains(&r.status) {
            return Err(format!("the host answered {}", r.status));
        }
        if r.body.len() > 1 << 20 {
            return Err("the model list is too large".into());
        }
        super::discovery::parse_models(style, &String::from_utf8_lossy(&r.body))
    }
}
