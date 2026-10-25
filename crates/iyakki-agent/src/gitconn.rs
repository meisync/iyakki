//! Git connections: "Connect Git or Gitea". A connection says which git host a person uses (GitHub, GitLab, a Gitea or Forgejo
//! server, any other), how they sign in (a personal access token kept in the vault, or an SSH key of the key service), which
//! account that is, and the name and email for commits. The list is a plain JSON file next to the key table (`connections.json`,
//! no secret in it); a token lives in the vault under `git.conn.<id>.token`, or, for the Gitea that runs on the person's cluster,
//! stays in the vault entry the platform services already made (`svc.<instance>.admin-token`): it is never copied.
//!
//! The same code serves Settings → Git and the AI IDE (`docs/GIT-AND-SSH.md`, "Connect Git or Gitea"): testing a connection with
//! the host's own API (`GET /user`), listing and creating repositories, and the git side of a workspace (`origin`, the first
//! commit). Rules that hold everywhere: a token is sent in a header from a private temp file (never in a command line, a URL, a log, an
//! error or an audit line; [`scrub`] runs on every error text); a certificate that is not trusted is shown with its fingerprint and
//! trusted only on request, for that connection only (verification is never switched off for a request); the audit gets host and
//! repository names only.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde::{Deserialize, Serialize};
use zeroize::Zeroizing;

use crate::gitkeys::{AuditFn, SecretStore};
use crate::wsexec::Runner;

// ------------------------------------------------------------------ kinds

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Kind {
    GitHub,
    GitLab,
    /// Gitea and Forgejo (the same API).
    #[default]
    Gitea,
    /// Any git over HTTPS or SSH: no account API is known.
    Other,
}

impl Kind {
    pub const ALL: [Kind; 4] = [Kind::GitHub, Kind::GitLab, Kind::Gitea, Kind::Other];
    pub fn label(self) -> &'static str {
        match self {
            Kind::GitHub => "GitHub",
            Kind::GitLab => "GitLab",
            Kind::Gitea => "Gitea / Forgejo",
            Kind::Other => "Other git host",
        }
    }
    pub fn default_url(self) -> &'static str {
        match self {
            Kind::GitHub => "https://github.com",
            Kind::GitLab => "https://gitlab.com",
            _ => "",
        }
    }
    /// Has an account API that [`test_connection`], listing and creating repositories can use.
    pub fn has_api(self) -> bool {
        self != Kind::Other
    }
    /// The word for what a token needs, shown with "cannot write".
    pub fn write_scope(self) -> &'static str {
        match self {
            Kind::GitHub => "the repo scope",
            Kind::GitLab => "the api scope",
            _ => "write:repository",
        }
    }
    /// Where the person adds a token.
    pub fn token_url(self, base: &str) -> String {
        match self {
            Kind::GitHub => format!("{base}/settings/tokens"),
            Kind::GitLab => format!("{base}/-/user_settings/personal_access_tokens"),
            _ => format!("{base}/user/settings/applications"),
        }
    }
    /// Where the person adds an SSH key.
    pub fn key_url(self, base: &str) -> String {
        match self {
            Kind::GitHub => format!("{base}/settings/keys"),
            Kind::GitLab => format!("{base}/-/user_settings/ssh_keys"),
            _ => format!("{base}/user/settings/keys"),
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Method {
    /// HTTPS with the token.
    #[default]
    Token,
    /// SSH with a key of the key service (a token may still be kept for the host's API).
    Ssh,
}

/// Where the data goes: shown beside the connection.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Placement {
    Cluster,
    Lan,
    Internet,
}

impl Placement {
    pub fn label(self) -> &'static str {
        match self {
            Placement::Cluster => "on your cluster",
            Placement::Lan => "on your local network",
            Placement::Internet => "on the internet",
        }
    }
}

/// `https://git.example.org:3000/x` -> `git.example.org`.
pub fn host_of(url: &str) -> String {
    let rest = url.split("://").nth(1).unwrap_or(url);
    let auth = rest.split(['/', '?', '#']).next().unwrap_or("");
    let auth = auth.rsplit('@').next().unwrap_or(auth);
    if let Some(r) = auth.strip_prefix('[') {
        return r.split(']').next().unwrap_or("").to_ascii_lowercase();
    }
    auth.split(':').next().unwrap_or("").to_ascii_lowercase()
}

/// The `host[:port]` of an address, lower case (the key of the host table is the host alone).
pub fn authority_of(url: &str) -> String {
    let rest = url.split("://").nth(1).unwrap_or(url);
    let auth = rest.split(['/', '?', '#']).next().unwrap_or("");
    auth.rsplit('@').next().unwrap_or(auth).to_ascii_lowercase()
}

pub fn placement_of(url: &str, cluster: bool) -> Placement {
    if cluster {
        return Placement::Cluster;
    }
    let h = host_of(url);
    if h == "localhost" || !h.contains('.') && !h.contains(':') || h.ends_with(".local") || h.ends_with(".lan") || h.ends_with(".internal") || h.ends_with(".home.arpa") {
        return Placement::Lan;
    }
    if let Ok(ip) = h.parse::<std::net::IpAddr>() {
        let private = match ip {
            std::net::IpAddr::V4(v) => v.is_private() || v.is_loopback() || v.is_link_local(),
            std::net::IpAddr::V6(v) => v.is_loopback() || (v.segments()[0] & 0xfe00) == 0xfc00 || (v.segments()[0] & 0xffc0) == 0xfe80,
        };
        if private {
            return Placement::Lan;
        }
    }
    Placement::Internet
}

// ------------------------------------------------------------------ the record

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct LastTest {
    pub at: i64,
    pub ok: bool,
    pub text: String,
}

/// One connection. Nothing secret: `token_vault` is a vault entry name.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Connection {
    pub id: String,
    pub name: String,
    pub kind: Kind,
    /// The host's web address without a path (`https://github.com`, `https://git.example.org:3000`).
    pub url: String,
    /// The account (login), known after a test.
    pub account: String,
    pub method: Method,
    /// The SSH key id of the key service (method `ssh`).
    pub key: String,
    /// The vault entry holding the token (empty: none).
    pub token_vault: String,
    /// The platform-services instance when this is the Gitea of the person's cluster.
    pub cluster: String,
    pub default: bool,
    pub identity_name: String,
    pub identity_email: String,
    /// SHA256 fingerprint of the certificate the person chose to trust for this host (empty: the system's trust).
    pub pinned: String,
    pub last_test: Option<LastTest>,
}

impl Connection {
    pub fn host(&self) -> String {
        host_of(&self.url)
    }
    pub fn placement(&self) -> Placement {
        placement_of(&self.url, !self.cluster.is_empty())
    }
    pub fn auth_label(&self) -> String {
        match self.method {
            Method::Token => "personal access token".into(),
            Method::Ssh if self.key.is_empty() => "SSH key".into(),
            Method::Ssh => format!("SSH key {}", self.key),
        }
    }
    pub fn has_token(&self) -> bool {
        !self.token_vault.is_empty()
    }
    /// `https://host/owner/repo.git` and the SSH form for a repository of this host, from `owner/repo`.
    pub fn clone_urls(&self, full: &str) -> (String, String) {
        let base = self.url.trim_end_matches('/');
        let h = host_of(&self.url);
        (format!("{base}/{full}.git"), format!("git@{h}:{full}.git"))
    }
    /// The remote address a workspace should get for `full`, by the way this connection signs in.
    pub fn remote_for(&self, full: &str) -> String {
        let (https, ssh) = self.clone_urls(full);
        if self.method == Method::Ssh { ssh } else { https }
    }
}

/// A repository name from a workspace name: lower case, letters, digits, `-`, `_`, `.`.
pub fn repo_name(ws: &str) -> String {
    crate::platform::connect::repo_name(ws)
}

pub fn valid_name(n: &str) -> bool {
    let n = n.trim();
    !n.is_empty() && n.chars().count() <= 40 && !n.chars().any(|c| c.is_control())
}

/// An address for a host: http or https, a host name, no credentials, no path to speak of.
pub fn clean_url(u: &str) -> Result<String, String> {
    let t = u.trim().trim_end_matches('/');
    if !(t.starts_with("https://") || t.starts_with("http://")) {
        return Err("The address must start with https:// (or http:// for a server on your own network).".into());
    }
    let auth = t.split("://").nth(1).unwrap_or("").split(['/', '?', '#']).next().unwrap_or("");
    if auth.contains('@') {
        return Err("The address must not hold a user name or password: the sign-in is the next step.".into());
    }
    let h = host_of(t);
    if h.is_empty() || !h.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_' | ':')) {
        return Err(format!("`{h}` is not a host name."));
    }
    if t.chars().any(|c| c.is_control() || c.is_whitespace() || matches!(c, '\'' | '"' | '`' | '\\')) {
        return Err("The address has characters that cannot be in a web address.".into());
    }
    Ok(t.to_string())
}

/// Removes a token (and the usual shapes of other secrets) from a text that may be shown or logged.
pub fn scrub(text: &str, token: &str) -> String {
    let mut t = text.to_string();
    if token.len() >= 4 {
        t = t.replace(token, "***");
    }
    crate::bugs::redact(&t)
}

// ------------------------------------------------------------------ the store

#[derive(Clone)]
pub struct Conns {
    dir: PathBuf,
    store: Arc<dyn SecretStore>,
    audit: AuditFn,
}

pub fn vault_name(id: &str) -> String {
    format!("git.conn.{id}.token")
}

fn slug(name: &str) -> String {
    let mut s: String = name.trim().to_ascii_lowercase().chars().map(|c| if c.is_ascii_alphanumeric() { c } else { '-' }).collect();
    while s.contains("--") {
        s = s.replace("--", "-");
    }
    let s = s.trim_matches('-');
    if s.is_empty() { "git".into() } else { s.chars().take(24).collect() }
}

impl Conns {
    pub fn new(dir: &Path, store: Arc<dyn SecretStore>, audit: AuditFn) -> Conns {
        Conns { dir: dir.into(), store, audit }
    }

    fn path(&self) -> PathBuf {
        self.dir.join("connections.json")
    }

    pub fn certs_dir(&self) -> PathBuf {
        self.dir.join("certs")
    }
    pub fn cert_path(&self, id: &str) -> PathBuf {
        self.certs_dir().join(format!("{id}.pem"))
    }
    /// The certificate file of a connection that trusts one (to hand to curl and git), if it is there.
    pub fn pinned_cert(&self, c: &Connection) -> Option<PathBuf> {
        let p = self.cert_path(&c.id);
        (!c.pinned.is_empty() && p.is_file()).then_some(p)
    }

    pub fn audit(&self, action: &str, detail: &str) {
        (self.audit)(&format!("git.{action}"), detail);
    }

    pub fn list(&self) -> Vec<Connection> {
        std::fs::read(self.path()).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default()
    }

    pub fn get(&self, id: &str) -> Option<Connection> {
        self.list().into_iter().find(|c| c.id == id)
    }

    pub fn default_connection(&self) -> Option<Connection> {
        let all = self.list();
        all.iter().find(|c| c.default).cloned().or_else(|| if all.len() == 1 { all.first().cloned() } else { None })
    }

    /// The connection that serves a web or clone host (for the host's credentials when the table has none).
    pub fn for_host(&self, host: &str) -> Option<Connection> {
        let h = host.to_ascii_lowercase();
        let hb = h.split(':').next().unwrap_or("").to_string();
        let all = self.list();
        let hit = |want_token: bool| all.iter().find(|c| (!want_token || c.has_token()) && (authority_of(&c.url) == h || c.host() == hb)).cloned();
        hit(true).or_else(|| hit(false))
    }

    fn save(&self, all: &[Connection]) -> Result<(), String> {
        std::fs::create_dir_all(&self.dir).map_err(|e| e.to_string())?;
        let tmp = self.path().with_extension("tmp");
        std::fs::write(&tmp, serde_json::to_vec_pretty(all).map_err(|e| e.to_string())?).map_err(|e| e.to_string())?;
        std::fs::rename(&tmp, self.path()).map_err(|e| e.to_string())
    }

    /// The token of a connection, from the vault.
    pub fn token(&self, c: &Connection) -> Result<Zeroizing<String>, String> {
        if c.token_vault.is_empty() {
            return Err(format!("{} has no token: add one with Edit.", c.name));
        }
        self.store.get(&c.token_vault)
    }

    pub fn token_present(&self, c: &Connection) -> bool {
        !c.token_vault.is_empty() && self.store.names().map(|n| n.contains(&c.token_vault)).unwrap_or(false)
    }

    /// Adds a connection. `token` goes to the vault (`git.conn.<id>.token`) unless the connection already names a vault entry
    /// (the cluster's Gitea). The first connection becomes the default.
    pub fn add(&self, mut c: Connection, token: Option<&str>) -> Result<Connection, String> {
        if !valid_name(&c.name) {
            return Err("Give the connection a name (up to 40 characters).".into());
        }
        c.url = clean_url(&c.url)?;
        let mut all = self.list();
        if all.iter().any(|x| x.name.eq_ignore_ascii_case(c.name.trim())) {
            return Err(format!("There is already a connection called {}.", c.name.trim()));
        }
        c.name = c.name.trim().to_string();
        let base = slug(&c.name);
        let mut id = base.clone();
        let mut n = 2;
        while all.iter().any(|x| x.id == id) {
            id = format!("{base}-{n}");
            n += 1;
        }
        c.id = id;
        match token.map(str::trim).filter(|t| !t.is_empty()) {
            Some(t) => {
                if t.chars().any(|ch| ch.is_control() || ch.is_whitespace()) {
                    return Err("The token has spaces or control characters: paste it again.".into());
                }
                let name = vault_name(&c.id);
                self.store.set(&name, t)?;
                c.token_vault = name;
            }
            None if c.token_vault.is_empty() && c.method == Method::Token => return Err("Paste the token for this connection.".into()),
            None => {}
        }
        if c.method == Method::Ssh && c.key.is_empty() {
            return Err("Choose or make an SSH key for this connection.".into());
        }
        if all.is_empty() {
            c.default = true;
        }
        if c.default {
            all.iter_mut().for_each(|x| x.default = false);
        }
        all.push(c.clone());
        self.save(&all)?;
        self.audit("connect.add", &format!("{} {}", c.name, c.host()));
        Ok(c)
    }

    /// Changes name, address, kind, method, key, identity. A new token replaces the stored one only when given.
    pub fn update(&self, c: &Connection, new_token: Option<&str>) -> Result<(), String> {
        if !valid_name(&c.name) {
            return Err("Give the connection a name (up to 40 characters).".into());
        }
        let mut all = self.list();
        let Some(slot) = all.iter().position(|x| x.id == c.id) else { return Err("That connection is gone.".into()) };
        if all.iter().any(|x| x.id != c.id && x.name.eq_ignore_ascii_case(c.name.trim())) {
            return Err(format!("There is already a connection called {}.", c.name.trim()));
        }
        let mut n = c.clone();
        n.name = n.name.trim().to_string();
        n.url = clean_url(&n.url)?;
        if all[slot].url != n.url {
            // another server: the old sign-in, test and trusted certificate do not carry over
            n.account.clear();
            n.last_test = None;
            n.pinned.clear();
            let _ = std::fs::remove_file(self.cert_path(&n.id));
        }
        if let Some(t) = new_token.map(str::trim).filter(|t| !t.is_empty()) {
            if t.chars().any(|ch| ch.is_control() || ch.is_whitespace()) {
                return Err("The token has spaces or control characters: paste it again.".into());
            }
            if n.token_vault.is_empty() || !n.token_vault.starts_with("git.conn.") {
                n.token_vault = vault_name(&n.id);
            }
            self.store.set(&n.token_vault, t)?;
        }
        if n.method == Method::Token && n.token_vault.is_empty() {
            return Err("A connection that signs in with a token needs one: paste it.".into());
        }
        if n.method == Method::Ssh && n.key.is_empty() {
            return Err("Choose or make an SSH key for this connection.".into());
        }
        n.default = all[slot].default;
        all[slot] = n.clone();
        self.save(&all)?;
        self.audit("connect.edit", &format!("{} {}", n.name, n.host()));
        Ok(())
    }

    pub fn remove(&self, id: &str) -> Result<(), String> {
        let mut all = self.list();
        let Some(i) = all.iter().position(|x| x.id == id) else { return Err("That connection is gone.".into()) };
        let c = all.remove(i);
        // only our own entry goes: the cluster's token belongs to the platform services
        if c.token_vault.starts_with("git.conn.") {
            let _ = self.store.delete(&c.token_vault);
        }
        let _ = std::fs::remove_file(self.cert_path(&c.id));
        if c.default {
            if let Some(f) = all.first_mut() {
                f.default = true;
            }
        }
        self.save(&all)?;
        self.audit("connect.remove", &format!("{} {}", c.name, c.host()));
        Ok(())
    }

    pub fn set_default(&self, id: &str) -> Result<(), String> {
        let mut all = self.list();
        if !all.iter().any(|x| x.id == id) {
            return Err("That connection is gone.".into());
        }
        for x in all.iter_mut() {
            x.default = x.id == id;
        }
        self.save(&all)?;
        let c = all.iter().find(|x| x.id == id).cloned().unwrap_or_default();
        self.audit("connect.default", &format!("{} {}", c.name, c.host()));
        Ok(())
    }

    pub fn set_identity(&self, id: &str, name: &str, email: &str) -> Result<(), String> {
        let (name, email) = (name.trim(), email.trim());
        if name.chars().any(|c| c.is_control() || c == '<' || c == '>') || name.len() > 100 {
            return Err("The name for commits must not have < > or control characters.".into());
        }
        if !email.is_empty() && (!email.contains('@') || email.chars().any(|c| c.is_control() || c.is_whitespace() || c == '<' || c == '>') || email.len() > 120) {
            return Err("That is not an email address.".into());
        }
        let mut all = self.list();
        let Some(c) = all.iter_mut().find(|x| x.id == id) else { return Err("That connection is gone.".into()) };
        c.identity_name = name.into();
        c.identity_email = email.into();
        self.save(&all)
    }

    /// Writes the result of a test into the record (what the list shows) and the audit.
    pub fn record_test(&self, id: &str, r: &TestReport) -> Result<(), String> {
        let mut all = self.list();
        let Some(c) = all.iter_mut().find(|x| x.id == id) else { return Err("That connection is gone.".into()) };
        if !r.account.is_empty() {
            c.account = r.account.clone();
        }
        c.last_test = Some(LastTest { at: crate::clock::now_unix(), ok: r.ok, text: r.summary() });
        let (name, host) = (c.name.clone(), c.host());
        self.save(&all)?;
        self.audit("connect.test", &format!("{name} {host} {}", if r.ok { "ok" } else { "failed" }));
        Ok(())
    }

    /// Trust the certificate shown for this connection (after the person compared the fingerprint).
    pub fn trust_cert(&self, id: &str, cert: &CertInfo) -> Result<(), String> {
        let mut all = self.list();
        let Some(c) = all.iter_mut().find(|x| x.id == id) else { return Err("That connection is gone.".into()) };
        std::fs::create_dir_all(self.certs_dir()).map_err(|e| e.to_string())?;
        std::fs::write(self.cert_path(id), &cert.pem).map_err(|e| e.to_string())?;
        c.pinned = cert.fingerprint.clone();
        let (name, host) = (c.name.clone(), c.host());
        self.save(&all)?;
        self.audit("connect.trust", &format!("{name} {host} {}", cert.fingerprint));
        Ok(())
    }

    /// A repository was made on the host (name only; never a token).
    pub fn audit_repo(&self, c: &Connection, full: &str) {
        self.audit("repo.create", &format!("{} {full}", c.host()));
    }
}

// ------------------------------------------------------------------ clusters

/// A Gitea the person installed through the platform services.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ClusterGitea {
    pub instance: String,
    pub title: String,
    pub url: String,
    pub token_vault: String,
    /// Why it cannot be used from here yet (empty: it can).
    pub problem: String,
}

pub fn cluster_gitea(dir: &Path, user: &str) -> Vec<ClusterGitea> {
    crate::platform::installed::load(dir, user)
        .into_iter()
        .filter(|i| i.kind == "git" && i.web_url().is_some())
        .map(|i| {
            let url = i.web_url().unwrap_or_default().trim_end_matches('/').to_string();
            let problem = if i.token_vault().is_none() {
                "IYAKKI has no token for it".to_string()
            } else if i.options.get("exposure") == "ClusterIP" {
                "only reachable inside the cluster (expose it with NodePort or Ingress)".to_string()
            } else {
                String::new()
            };
            ClusterGitea { instance: i.id.clone(), title: i.title.clone(), url, token_vault: i.token_vault().unwrap_or_default().to_string(), problem }
        })
        .collect()
}

// ------------------------------------------------------------------ http

/// How the token is sent.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Style {
    /// `Authorization: token <t>` (Gitea, Forgejo).
    Token,
    /// `Authorization: Bearer <t>` (GitHub, GitLab).
    Bearer,
}

#[derive(Clone, Debug)]
pub struct Req {
    pub method: String,
    pub url: String,
    pub token: Option<String>,
    pub style: Style,
    pub body: Option<String>,
    /// A certificate file the person chose to trust for this host.
    pub cert: Option<PathBuf>,
    /// The `Accept` header (none: `application/json`). Browsing repositories asks for diffs and raw files this way.
    pub accept: Option<String>,
    /// The most bytes of answer to keep (0: 512 KB).
    pub max_bytes: usize,
    /// Seconds before curl gives up (0: 20).
    pub timeout_s: u64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Resp {
    pub status: u16,
    /// Header names in lower case.
    pub headers: Vec<(String, String)>,
    pub body: String,
}

impl Resp {
    pub fn header(&self, n: &str) -> Option<&str> {
        self.headers.iter().find(|(k, _)| k == n).map(|(_, v)| v.as_str())
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum NetErr {
    /// No answer: the host is down, the name is wrong, a firewall.
    Unreachable(String),
    /// The server answered but its certificate is not trusted.
    Tls(String),
    Other(String),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CertInfo {
    /// `SHA256:` and the hex of the certificate, in pairs.
    pub fingerprint: String,
    pub pem: String,
}

pub trait GitHttp: Send + Sync {
    fn send(&self, r: &Req) -> Result<Resp, NetErr>;
    /// The certificate a server presents (for the person to compare), without trusting it.
    fn peer_cert(&self, url: &str) -> Result<CertInfo, String>;
}

/// What [`CurlGit`] says when the answer did not fit the byte limit of the request (the status line at its end was cut off).
pub const TOO_BIG: &str = "the answer is larger than the limit";

/// curl through the [`Runner`], with the token in a private temp config file.
pub struct CurlGit {
    pub run: Arc<dyn Runner>,
    pub tmp: PathBuf,
}

fn q(s: &str) -> String {
    format!("\"{}\"", s.replace('\\', "\\\\").replace('"', "\\\"").replace('\n', "\\n").replace('\r', "\\r"))
}

pub fn parse_curl(text: &str) -> Option<Resp> {
    // headers (possibly several blocks: a proxy's CONNECT, a 100), a blank line, the body, then "\n<code>"
    let (rest, code) = text.rsplit_once('\n')?;
    let status: u16 = code.trim().parse().ok()?;
    let mut rest = rest;
    loop {
        let (head, body) = rest.split_once("\r\n\r\n").or_else(|| rest.split_once("\n\n"))?;
        if body.starts_with("HTTP/") && (head.contains(" 100 ") || head.contains("200 Connection established")) {
            rest = body;
            continue;
        }
        let headers = head.lines().skip(1).filter_map(|l| l.split_once(':')).map(|(k, v)| (k.trim().to_ascii_lowercase(), v.trim().to_string())).collect();
        return Some(Resp { status, headers, body: body.to_string() });
    }
}

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

fn first_pem(text: &str) -> Option<String> {
    let s = text.find("-----BEGIN CERTIFICATE-----")?;
    let e = text[s..].find("-----END CERTIFICATE-----")? + s + "-----END CERTIFICATE-----".len();
    Some(format!("{}\n", text[s..e].replace("\r", "").trim()))
}

pub fn cert_info(pem: &str) -> Result<CertInfo, String> {
    use sha2::{Digest, Sha256};
    let body: String = pem.lines().filter(|l| !l.starts_with("-----")).collect::<Vec<_>>().join("");
    let der = crate::sshkey::unb64(body.trim()).ok_or("the certificate does not decode")?;
    let h = Sha256::digest(&der);
    let hex: Vec<String> = h.iter().map(|b| format!("{b:02X}")).collect();
    Ok(CertInfo { fingerprint: format!("SHA256:{}", hex.join(":")), pem: pem.to_string() })
}

/// A scripted [`GitHttp`] for tests: the first rule whose `"METHOD url-part"` is contained in the request answers.
#[derive(Default)]
pub struct FakeGitHttp {
    rules: Mutex<Vec<(String, Result<Resp, NetErr>)>>,
    pub calls: Mutex<Vec<Req>>,
    pub cert: Mutex<Option<Result<CertInfo, String>>>,
}

impl FakeGitHttp {
    pub fn answer(&self, rule: &str, status: u16, headers: &[(&str, &str)], body: &str) -> &Self {
        self.rules.lock().unwrap_or_else(|e| e.into_inner()).push((rule.into(), Ok(Resp { status, headers: headers.iter().map(|(k, v)| (k.to_ascii_lowercase(), v.to_string())).collect(), body: body.into() })));
        self
    }
    pub fn fail(&self, rule: &str, e: NetErr) -> &Self {
        self.rules.lock().unwrap_or_else(|x| x.into_inner()).push((rule.into(), Err(e)));
        self
    }
    pub fn lines(&self) -> Vec<String> {
        self.calls.lock().unwrap_or_else(|e| e.into_inner()).iter().map(|c| format!("{} {}", c.method, c.url)).collect()
    }
    pub fn requests(&self) -> Vec<Req> {
        self.calls.lock().unwrap_or_else(|e| e.into_inner()).clone()
    }
}

impl GitHttp for FakeGitHttp {
    fn send(&self, r: &Req) -> Result<Resp, NetErr> {
        self.calls.lock().unwrap_or_else(|e| e.into_inner()).push(r.clone());
        let line = format!("{} {}", r.method, r.url);
        let hit = self.rules.lock().unwrap_or_else(|e| e.into_inner()).iter().find(|(k, _)| line.contains(k.as_str())).map(|(_, v)| v.clone());
        hit.unwrap_or_else(|| Err(NetErr::Unreachable("could not connect".into())))
    }
    fn peer_cert(&self, _url: &str) -> Result<CertInfo, String> {
        self.cert.lock().unwrap_or_else(|e| e.into_inner()).clone().unwrap_or_else(|| Err("no certificate".into()))
    }
}

// ------------------------------------------------------------------ the host's API

pub fn style_of(kind: Kind) -> Style {
    if kind == Kind::Gitea { Style::Token } else { Style::Bearer }
}

/// `https://api.github.com` for github.com, `<base>/api/v3` for a GitHub Enterprise address, `<base>/api/v4` for GitLab,
/// `<base>/api/v1` for Gitea and Forgejo.
pub fn api_base(kind: Kind, base: &str) -> String {
    let base = base.trim_end_matches('/');
    match kind {
        Kind::GitHub if host_of(base) == "github.com" => "https://api.github.com".into(),
        Kind::GitHub => format!("{base}/api/v3"),
        Kind::GitLab => format!("{base}/api/v4"),
        _ => format!("{base}/api/v1"),
    }
}

fn req(c: &Connection, method: &str, path: &str, token: &str, body: Option<String>, cert: Option<&PathBuf>) -> Req {
    Req { method: method.into(), url: format!("{}{path}", api_base(c.kind, &c.url)), token: Some(token.to_string()), style: style_of(c.kind), body, cert: cert.cloned(), accept: None, max_bytes: 0, timeout_s: 0 }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tls {
    /// http: nothing is encrypted.
    Plain,
    /// https, certificate trusted by the system.
    Verified,
    /// https, certificate the person chose to trust (fingerprint pinned).
    Pinned,
    /// https, certificate not trusted: the person may compare the fingerprint and trust it.
    Untrusted,
    Unknown,
}

#[derive(Clone, Debug)]
pub struct TestReport {
    pub ok: bool,
    pub account: String,
    pub name: String,
    pub email: String,
    pub can_read: Option<bool>,
    pub can_write: Option<bool>,
    /// What could not be checked, and why.
    pub notes: Vec<String>,
    /// The plain-words problem when `ok` is false.
    pub error: Option<String>,
    pub tls: Tls,
    pub cert: Option<CertInfo>,
    pub placement: Placement,
}

impl TestReport {
    /// An empty report for a connection (nothing asked yet).
    pub fn blank(c: &Connection) -> TestReport {
        TestReport { ok: false, account: String::new(), name: String::new(), email: String::new(), can_read: None, can_write: None, notes: vec![], error: None, tls: if c.url.starts_with("http://") { Tls::Plain } else { Tls::Unknown }, cert: None, placement: c.placement() }
    }
    /// One line for the list ("Signed in as octo; can read and write repositories").
    pub fn summary(&self) -> String {
        if let Some(e) = &self.error {
            return e.clone();
        }
        let mut s = if self.account.is_empty() { "Reached the host".to_string() } else { format!("Signed in as {}", self.account) };
        match (self.can_read, self.can_write) {
            (Some(true), Some(true)) => s.push_str("; can read and write repositories"),
            (Some(true), None) => s.push_str("; can read repositories (write access could not be checked)"),
            (Some(true), Some(false)) => s.push_str("; can read but not write repositories"),
            _ => {}
        }
        s
    }
}

fn jstr(v: &serde_json::Value, k: &str) -> String {
    v.get(k).and_then(|x| x.as_str()).unwrap_or("").to_string()
}

/// A plain-words problem for a status code from the host.
pub fn status_text(status: u16, kind: Kind) -> String {
    match status {
        401 => "The token was refused (401). Check that it is complete, not expired and for this server.".into(),
        403 => format!("The server refused the request (403): the token may lack a permission ({}).", kind.write_scope()),
        404 => "The server does not have that address (404). Check the web address of the host.".into(),
        429 => "The server asked to slow down (429). Try again in a minute.".into(),
        500..=599 => format!("The server had a problem (HTTP {status}). Try again later."),
        s => format!("The server answered HTTP {s}, which was not expected."),
    }
}

fn net_error(e: &NetErr, report: &mut TestReport, conns_token: &str) {
    match e {
        NetErr::Unreachable(w) => {
            report.error = Some(format!("The server did not answer ({}).", scrub(w, conns_token)));
        }
        NetErr::Tls(w) => {
            report.tls = Tls::Untrusted;
            report.error = Some(format!("The server's certificate is not trusted ({}). If this is your own server you can look at its fingerprint and trust it.", scrub(w, conns_token)));
        }
        NetErr::Other(w) => report.error = Some(scrub(w, conns_token)),
    }
}

/// Asks the host who the token belongs to and what it may do. `egress` says whether this account may reach the host at all.
pub fn test_connection(http: &dyn GitHttp, c: &Connection, token: &str, cert: Option<&PathBuf>) -> TestReport {
    let mut r = TestReport::blank(c);
    if !c.kind.has_api() {
        r.ok = true;
        r.notes.push("This kind of host has no account API to ask. The first fetch or push shows whether the sign-in works.".into());
        return r;
    }
    let who = http.send(&req(c, "GET", "/user", token, None, cert));
    let who = match who {
        Ok(w) => w,
        Err(e) => {
            net_error(&e, &mut r, token);
            return r;
        }
    };
    if c.url.starts_with("https://") {
        r.tls = if cert.is_some() { Tls::Pinned } else { Tls::Verified };
    }
    if who.status != 200 {
        r.error = Some(if who.status == 403 && c.kind == Kind::Gitea { "The token cannot read the account (403): give it the read:user scope.".into() } else { status_text(who.status, c.kind) });
        return r;
    }
    let j: serde_json::Value = serde_json::from_str(&who.body).unwrap_or_default();
    r.account = match c.kind {
        Kind::GitLab => jstr(&j, "username"),
        _ => jstr(&j, "login"),
    };
    if r.account.is_empty() {
        r.error = Some("The server answered, but not like a ".to_string() + c.kind.label() + " server: check the address.");
        return r;
    }
    r.name = jstr(&j, "name");
    r.email = jstr(&j, "email");
    if r.email.is_empty() {
        r.email = jstr(&j, "public_email");
    }
    r.ok = true;
    match c.kind {
        Kind::GitHub => match who.header("x-oauth-scopes") {
            Some(s) => {
                let scopes: Vec<&str> = s.split(',').map(str::trim).filter(|x| !x.is_empty()).collect();
                let write = scopes.iter().any(|x| *x == "repo" || *x == "public_repo");
                r.can_read = Some(write || scopes.iter().any(|x| x.starts_with("read:") || *x == "repo"));
                r.can_write = Some(write);
                if !write {
                    r.ok = false;
                    r.error = Some("The token cannot write repositories (needs the repo scope).".into());
                }
            }
            None => {
                r.notes.push("This is a fine-grained token: GitHub does not list its permissions, so writing could not be checked. Creating or pushing shows it.".into());
                r.can_read = Some(true);
            }
        },
        Kind::GitLab => {
            let s = http.send(&req(c, "GET", "/personal_access_tokens/self", token, None, cert));
            match s {
                Ok(s) if s.status == 200 => {
                    let j: serde_json::Value = serde_json::from_str(&s.body).unwrap_or_default();
                    let scopes: Vec<String> = j.get("scopes").and_then(|x| x.as_array()).map(|a| a.iter().filter_map(|v| v.as_str().map(String::from)).collect()).unwrap_or_default();
                    let write = scopes.iter().any(|x| x == "api" || x == "write_repository");
                    r.can_read = Some(write || scopes.iter().any(|x| x == "read_api" || x == "read_repository"));
                    r.can_write = Some(write);
                    if !write {
                        r.ok = false;
                        r.error = Some("The token cannot write repositories (needs the api scope).".into());
                    }
                }
                _ => {
                    r.notes.push("GitLab did not list this token's scopes, so writing could not be checked.".into());
                    r.can_read = Some(true);
                }
            }
        }
        _ => {
            // Gitea and Forgejo do not say what a token may do: list repositories for reading, and say that writing is unknown
            match http.send(&req(c, "GET", "/user/repos?limit=1", token, None, cert)) {
                Ok(s) if s.status == 200 => r.can_read = Some(true),
                Ok(s) if s.status == 403 => {
                    r.can_read = Some(false);
                    r.ok = false;
                    r.error = Some("The token cannot read repositories (403): give it read:repository.".into());
                }
                _ => r.notes.push("Reading repositories could not be checked.".into()),
            }
            if r.ok {
                r.notes.push("Gitea does not say what a token may write: creating a repository or pushing shows it.".into());
            }
        }
    }
    r
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RepoInfo {
    pub full_name: String,
    pub https_url: String,
    pub ssh_url: String,
    pub web_url: String,
    pub private: bool,
}

fn repo_of(v: &serde_json::Value, kind: Kind) -> Option<RepoInfo> {
    let (full, https, ssh, web) = match kind {
        Kind::GitLab => (jstr(v, "path_with_namespace"), jstr(v, "http_url_to_repo"), jstr(v, "ssh_url_to_repo"), jstr(v, "web_url")),
        _ => (jstr(v, "full_name"), jstr(v, "clone_url"), jstr(v, "ssh_url"), jstr(v, "html_url")),
    };
    if full.is_empty() || https.contains('@') {
        return None;
    }
    let private = v.get(if kind == Kind::GitLab { "visibility" } else { "private" }).map(|x| x.as_bool().unwrap_or(x.as_str() != Some("public"))).unwrap_or(true);
    Some(RepoInfo { full_name: full, https_url: https, ssh_url: ssh, web_url: web, private })
}

/// The first page of the repositories the account can see, most recently used first where the host can sort.
pub fn list_repos(http: &dyn GitHttp, c: &Connection, token: &str, cert: Option<&PathBuf>) -> Result<Vec<RepoInfo>, String> {
    let path = match c.kind {
        Kind::GitHub => "/user/repos?per_page=50&sort=updated",
        Kind::GitLab => "/projects?membership=true&simple=true&per_page=50&order_by=last_activity_at",
        Kind::Gitea => "/user/repos?limit=50",
        Kind::Other => return Err("This kind of host has no list of repositories: type the address of the repository.".into()),
    };
    let r = http.send(&req(c, "GET", path, token, None, cert)).map_err(|e| net_message(&e, token))?;
    if r.status != 200 {
        return Err(status_text(r.status, c.kind));
    }
    let v: serde_json::Value = serde_json::from_str(&r.body).map_err(|_| "The host's list of repositories could not be read.".to_string())?;
    Ok(v.as_array().map(|a| a.iter().filter_map(|x| repo_of(x, c.kind)).collect()).unwrap_or_default())
}

pub fn net_message(e: &NetErr, token: &str) -> String {
    let mut r = TestReport { ok: false, account: String::new(), name: String::new(), email: String::new(), can_read: None, can_write: None, notes: vec![], error: None, tls: Tls::Unknown, cert: None, placement: Placement::Internet };
    net_error(e, &mut r, token);
    r.error.unwrap_or_default()
}

/// The request that creates a repository: (path under the API base, JSON body).
pub fn create_body(kind: Kind, name: &str, private: bool, description: &str) -> (String, String) {
    match kind {
        Kind::GitLab => ("/projects".into(), serde_json::json!({"name": name, "path": name, "visibility": if private { "private" } else { "public" }, "description": description, "initialize_with_readme": false}).to_string()),
        Kind::GitHub => ("/user/repos".into(), serde_json::json!({"name": name, "private": private, "description": description, "auto_init": false}).to_string()),
        _ => ("/user/repos".into(), serde_json::json!({"name": name, "private": private, "description": description, "auto_init": false}).to_string()),
    }
}

pub fn valid_repo_name(n: &str) -> bool {
    !n.is_empty() && n.len() <= 100 && n.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.')) && n != "." && n != ".." && !n.ends_with(".git") && !n.starts_with('.')
}

/// Makes the repository on the host. Does not touch any workspace. Audit: host and repository name.
pub fn create_repo(conns: &Conns, http: &dyn GitHttp, c: &Connection, name: &str, private: bool, description: &str) -> Result<RepoInfo, String> {
    if !valid_repo_name(name) {
        return Err(format!("`{name}` is not a repository name (letters, digits, - _ . only)."));
    }
    if !c.kind.has_api() {
        return Err("This kind of host has no API to make a repository: make it on the site, then connect to it.".into());
    }
    let token = conns.token(c).map_err(|_| "Making a repository needs a token for this host: add one with Edit.".to_string())?;
    let cert = conns.pinned_cert(c);
    let (path, body) = create_body(c.kind, name, private, description);
    let r = http.send(&req(c, "POST", &path, &token, Some(body), cert.as_ref())).map_err(|e| net_message(&e, &token))?;
    match r.status {
        200 | 201 => {
            let v: serde_json::Value = serde_json::from_str(&r.body).unwrap_or_default();
            let info = repo_of(&v, c.kind).ok_or("The host made the repository but did not describe it.")?;
            conns.audit_repo(c, &info.full_name);
            Ok(info)
        }
        401 => Err(status_text(401, c.kind)),
        403 => Err(format!("The token cannot write repositories (needs {}).", c.kind.write_scope())),
        409 | 422 | 400 if scrub(&r.body, &token).to_ascii_lowercase().contains("exist") || r.status == 409 => Err(format!("A repository called {name} already exists on {}: choose another name, or connect to it.", c.host())),
        s => Err(format!("{} {}", status_text(s, c.kind), first_line(&scrub(&r.body, &token)))),
    }
}

fn first_line(s: &str) -> String {
    s.lines().find(|l| !l.trim().is_empty()).unwrap_or("").trim().chars().take(160).collect()
}

// ------------------------------------------------------------------ the workspace's git

/// What the git side of a workspace looks like.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct WsGit {
    pub is_repo: bool,
    pub branch: String,
    pub origin: Option<String>,
    pub commits: usize,
    /// Number of changed or untracked files (what a first commit would take).
    pub changed: usize,
}

fn git(run: &dyn Runner, root: &Path, args: &[&str]) -> crate::wsexec::Out {
    let mut argv: Vec<String> = vec!["git".into(), "-C".into(), root.display().to_string()];
    argv.extend(args.iter().map(|s| s.to_string()));
    let env = vec![("GIT_TERMINAL_PROMPT".to_string(), "0".to_string()), ("LC_ALL".to_string(), "C".to_string())];
    run.run(root, &argv, &env, Duration::from_secs(60), 256 * 1024, None)
}

pub fn ws_git(run: &dyn Runner, root: &Path) -> WsGit {
    let top = git(run, root, &["rev-parse", "--show-toplevel"]);
    let canon = std::fs::canonicalize(root).unwrap_or_else(|_| root.to_path_buf());
    let same = top.ok() && std::fs::canonicalize(top.stdout.trim()).map(|t| t == canon).unwrap_or(top.stdout.trim() == root.display().to_string());
    if !same {
        return WsGit::default();
    }
    let br = git(run, root, &["symbolic-ref", "--short", "HEAD"]);
    let ou = git(run, root, &["remote", "get-url", "origin"]);
    let n = git(run, root, &["rev-list", "--count", "HEAD"]);
    let st = git(run, root, &["status", "--porcelain"]);
    WsGit {
        is_repo: true,
        branch: br.stdout.trim().to_string(),
        origin: Some(ou.stdout.trim().to_string()).filter(|u| ou.ok() && !u.is_empty()),
        commits: n.stdout.trim().parse().unwrap_or(0),
        changed: st.stdout.lines().filter(|l| !l.trim().is_empty()).count(),
    }
}

pub fn git_init(run: &dyn Runner, root: &Path) -> Result<(), String> {
    let o = git(run, root, &["init", "-q"]);
    if !o.ok() {
        return Err(format!("git init failed: {}", first_line(&crate::bugs::redact(&o.combined()))));
    }
    // a branch called main whatever the person's git default is
    let _ = git(run, root, &["symbolic-ref", "HEAD", "refs/heads/main"]);
    Ok(())
}

/// `git remote add origin <url>` (or set-url with `replace`). The address must hold no credential.
pub fn set_origin(run: &dyn Runner, root: &Path, url: &str, replace: bool) -> Result<(), String> {
    let parsed = crate::gitsite::parse(url, crate::gitsite::ParseOptions::default())?;
    let have = git(run, root, &["remote", "get-url", "origin"]);
    let o = if have.ok() && !have.stdout.trim().is_empty() {
        if have.stdout.trim() == parsed.url {
            return Ok(());
        }
        if !replace {
            return Err(format!("This project already has an origin ({}). Disconnect it first, or choose to replace it.", crate::gitsite::redact_url(have.stdout.trim())));
        }
        git(run, root, &["remote", "set-url", "origin", &parsed.url])
    } else {
        git(run, root, &["remote", "add", "origin", &parsed.url])
    };
    if !o.ok() {
        return Err(format!("git could not set origin: {}", first_line(&crate::bugs::redact(&o.combined()))));
    }
    Ok(())
}

pub fn remove_origin(run: &dyn Runner, root: &Path) -> Result<(), String> {
    let o = git(run, root, &["remote", "remove", "origin"]);
    if !o.ok() {
        return Err(format!("git could not remove origin: {}", first_line(&crate::bugs::redact(&o.combined()))));
    }
    Ok(())
}

/// Sets the name and email for commits in this repository only, where the person has not set them.
pub fn apply_identity(run: &dyn Runner, root: &Path, c: &Connection) {
    for (k, v) in [("user.name", &c.identity_name), ("user.email", &c.identity_email)] {
        if v.is_empty() {
            continue;
        }
        let have = git(run, root, &["config", "--local", "--get", k]);
        if !have.ok() || have.stdout.trim().is_empty() {
            let _ = git(run, root, &["config", "--local", k, v]);
        }
    }
}

/// The first commit of a project that has none: everything git does not ignore. The caller shows `ws_git(..).changed` first.
pub fn first_commit(run: &dyn Runner, root: &Path, c: &Connection) -> Result<(), String> {
    apply_identity(run, root, c);
    let a = git(run, root, &["add", "-A"]);
    if !a.ok() {
        return Err(format!("git add failed: {}", first_line(&crate::bugs::redact(&a.combined()))));
    }
    let o = git(run, root, &["commit", "-q", "-m", "First commit from IYAKKI"]);
    if !o.ok() {
        let t = crate::bugs::redact(&o.combined());
        return Err(if t.contains("tell me who you are") || t.contains("Author identity unknown") { "git does not know who you are: fill in the name and email of the connection (Settings, Git, Edit).".to_string() } else { format!("git commit failed: {}", first_line(&t)) });
    }
    Ok(())
}

#[cfg(test)]
#[path = "gitconn_tests.rs"]
mod tests;
