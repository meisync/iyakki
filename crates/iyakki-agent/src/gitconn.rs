//! The agent's part of `iyakki_shared::gitconn` (the module moved down for F-SPLIT-03): the Gitea instances of the person's cluster and,
//! in tests, curl as the transport.

#[cfg(test)]
use std::path::PathBuf;
#[cfg(test)]
use std::sync::Arc;
#[cfg(test)]
use std::time::Duration;

pub use iyakki_shared::gitconn::*;
#[cfg(test)]
use crate::wsexec::Runner;
use std::path::Path;

// ------------------------------------------------------------------ clusters


pub fn cluster_gitea(dir: &Path, user: &str) -> Vec<ClusterGitea> {
    crate::platform::installed::load(dir, user)
        .into_iter()
        .filter(|i| i.kind == "git" && i.web_url().is_some())
        .map(|i| {
            let url = i.web_url().unwrap_or_default().trim_end_matches('/').to_string();
            let problem = if i.token_vault().is_none() {
                "IYAKKI has no token for it".to_string()
            } else if i.options.get("exposure") == "ClusterIP" && crate::platform::gitea::Gitea::loopback_of(&i.options).is_none() {
                "only reachable inside the cluster (expose it with NodePort or Ingress)".to_string()
            } else {
                String::new()
            };
            ClusterGitea { instance: i.id.clone(), title: i.title.clone(), url, token_vault: i.token_vault().unwrap_or_default().to_string(), problem, namespace: i.namespace.clone(), found: false }
        })
        .collect()
}


#[cfg(test)]
pub struct CurlGit {
    pub run: Arc<dyn Runner>,
    pub tmp: PathBuf,
}

#[cfg(test)]
fn q(s: &str) -> String {
    format!("\"{}\"", s.replace('\\', "\\\\").replace('"', "\\\"").replace('\n', "\\n").replace('\r', "\\r"))
}

#[cfg(test)]
impl GitHttp for CurlGit {
    fn send(&self, r: &Req) -> Result<Resp, NetErr> {
        if !self.run.has("curl") {
            return Err(NetErr::Other("curl is not installed on this system".into()));
        }
        if !(r.url.starts_with("http://") || r.url.starts_with("https://")) {
            return Err(NetErr::Other("only http and https addresses are used".into()));
        }
        let mut cfg = format!("silent\nshow-error\ninclude\nmax-time = {}\nurl = {}\nrequest = {}\nwrite-out = \"\\n%{{http_code}}\"\n", if r.timeout_s == 0 { 20 } else { r.timeout_s }, q(&r.url), q(&r.method));
        if let Some(t) = &r.token {
            let w = if r.style == Style::Token { "token" } else { "Bearer" };
            cfg.push_str(&format!("header = {}\n", q(&format!("Authorization: {w} {t}"))));
        }
        let accept = r.accept.as_deref().filter(|a| a.chars().all(|c| c.is_ascii_graphic() || c == ' ')).unwrap_or("application/json");
        cfg.push_str(&format!("header = {}\n", q(&format!("Accept: {accept}"))));
        if let Some(c) = &r.cert {
            cfg.push_str(&format!("cacert = {}\n", q(&c.display().to_string())));
        }
        if let Some(b) = &r.body {
            cfg.push_str(&format!("header = \"Content-Type: application/json\"\ndata = {}\n", q(b)));
        }
        let f = crate::platform::secrets::SecretFile::new(&self.tmp, &cfg).map_err(NetErr::Other)?;
        let o = self.run.run(&std::env::temp_dir(), &["curl".into(), "-K".into(), f.path.display().to_string()], &[], Duration::from_secs(30), if r.max_bytes == 0 { 512 * 1024 } else { r.max_bytes }, None);
        drop(f);
        let token = r.token.clone().unwrap_or_default();
        if o.missing {
            return Err(NetErr::Other("curl is not installed on this system".into()));
        }
        if let Some(resp) = parse_curl(&o.stdout).filter(|x| x.status != 0) {
            return Ok(resp);
        }
        if o.truncated {
            return Err(NetErr::Other(TOO_BIG.into()));
        }
        let why = scrub(o.stderr.lines().find(|l| !l.trim().is_empty()).unwrap_or("no answer").trim(), &token);
        match o.code {
            Some(60) | Some(51) | Some(58) | Some(59) | Some(77) | Some(83) => Err(NetErr::Tls(why)),
            Some(35) if why.to_ascii_lowercase().contains("cert") => Err(NetErr::Tls(why)),
            _ => Err(NetErr::Unreachable(why)),
        }
    }

    fn peer_cert(&self, url: &str) -> Result<CertInfo, String> {
        if !self.run.has("curl") {
            return Err("curl is not installed on this system".into());
        }
        if !url.starts_with("https://") {
            return Err("only https servers have a certificate".into());
        }
        // `-k` only to read what the server presents: nothing is sent to it but the handshake and a HEAD request
        let argv: Vec<String> = ["curl", "-k", "-s", "-S", "--max-time", "15", "--certinfo", "-I", "-o", "/dev/null", "-w", "%{certs}", url].iter().map(|s| s.to_string()).collect();
        let o = self.run.run(&std::env::temp_dir(), &argv, &[], Duration::from_secs(20), 256 * 1024, None);
        let pem = first_pem(&o.stdout).ok_or_else(|| "the server did not show a certificate".to_string())?;
        cert_info(&pem)
    }
}

#[cfg(test)]
fn first_pem(text: &str) -> Option<String> {
    let s = text.find("-----BEGIN CERTIFICATE-----")?;
    let e = text[s..].find("-----END CERTIFICATE-----")? + s + "-----END CERTIFICATE-----".len();
    Some(format!("{}\n", text[s..e].replace("\r", "").trim()))
}

#[cfg(test)]
#[path = "gitconn_tests.rs"]
mod tests;
