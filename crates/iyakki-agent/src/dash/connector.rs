//! The connector seam. A [`DataSource`] is one configured data system (an Elasticsearch cluster, a Prometheus server, a
//! ClickHouse server, a DuckDB database or folder of files). The dashboard, the assistant's tools and the tests only ever
//! talk to this trait, so a new system is one new file plus one line in [`crate::dash::registry`], and the whole thing can
//! sit behind a remote backend later: every type here is `serde` and a [`QueryRequest`] / [`Frame`] pair is the entire wire
//! protocol (see `docs/DASHBOARD.md`).
//!
//! Rules every connector follows: read-only (see [`crate::dash::readonly`]); limits on rows, bytes and time; credentials only
//! as vault references, resolved at the moment of the request and never stored, logged or put in a URL or an argument list;
//! errors that say what to do.

use std::collections::BTreeMap;
use std::fmt;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use iyakki_proxy::Upstream;
use serde::{Deserialize, Serialize};
use serde_json::Value;
pub use zeroize::Zeroizing;

use super::frame::Frame;

/// Hard limits applied by every connector (a request may ask for less, never more).
pub const MAX_ROWS: usize = 10_000;
pub const MAX_SERIES: usize = 200;
pub const MAX_POINTS: usize = 11_000;
pub const MAX_RESPONSE_BYTES: usize = 8 << 20;
pub const DEFAULT_TIMEOUT_S: u64 = 30;
pub const MAX_TIMEOUT_S: u64 = 120;

// ------------------------------------------------------------------------------------------------------ errors

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ErrKind {
    /// The source is not set up right (missing field, bad address, unknown secret).
    Config,
    /// Could not reach it (refused, timed out, DNS, TLS).
    Network,
    /// 401 / 403.
    Auth,
    /// The server rejected the query; the message is the server's own.
    BadQuery,
    /// Refused here because the query would write or reach outside.
    ReadOnly,
    /// A limit (rows, bytes, time) was hit.
    Limit,
    /// The answer could not be understood.
    Parse,
    /// The program a source needs is not installed.
    Missing,
    Cancelled,
    Server,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct DashError {
    pub kind: ErrKind,
    pub message: String,
    /// What to do about it.
    #[serde(default)]
    pub fix: Option<String>,
}

impl DashError {
    pub fn new(kind: ErrKind, message: impl Into<String>) -> DashError {
        DashError { kind, message: message.into(), fix: None }
    }
    pub fn fix(mut self, fix: impl Into<String>) -> DashError {
        self.fix = Some(fix.into());
        self
    }
    pub fn config(message: impl Into<String>) -> DashError {
        DashError::new(ErrKind::Config, message)
    }
    pub fn parse(message: impl Into<String>) -> DashError {
        DashError::new(ErrKind::Parse, message).fix("The server answered in a way this version does not understand. Check that the address points at the right kind of server (and not at a login page or proxy).")
    }
    pub fn read_only(message: impl Into<String>) -> DashError {
        DashError::new(ErrKind::ReadOnly, message).fix("Dashboards only read data. Rewrite the query as a read (SELECT / search / query).")
    }
    pub fn cancelled() -> DashError {
        DashError::new(ErrKind::Cancelled, "cancelled")
    }
}

impl fmt::Display for DashError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self.fix {
            Some(x) => write!(f, "{} {}", self.message.trim_end_matches('.'), format_args!("({x})")),
            None => write!(f, "{}", self.message),
        }
    }
}

impl std::error::Error for DashError {}

// ------------------------------------------------------------------------------------------------------ cancel

/// Set by the screen when the person leaves or changes the query; connectors look at it between steps and the caller drops
/// a late answer. (An HTTP call already in flight ends by its own timeout.)
#[derive(Clone, Debug, Default)]
pub struct Cancel(Arc<AtomicBool>);

impl Cancel {
    pub fn new() -> Cancel {
        Cancel::default()
    }
    pub fn cancel(&self) {
        self.0.store(true, Ordering::SeqCst);
    }
    pub fn is_cancelled(&self) -> bool {
        self.0.load(Ordering::SeqCst)
    }
    pub fn check(&self) -> Result<(), DashError> {
        if self.is_cancelled() { Err(DashError::cancelled()) } else { Ok(()) }
    }
}

// ------------------------------------------------------------------------------------------------------ config

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FieldKind {
    Text,
    Url,
    Path,
    Bool,
    Number,
    Choice(Vec<String>),
    /// A vault reference (`uvault:name`); the value itself is never typed into the form.
    Secret,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Field {
    pub key: String,
    pub label: String,
    pub kind: FieldKind,
    pub required: bool,
    pub help: String,
    pub default: String,
}

impl Field {
    pub fn new(key: &str, label: &str, kind: FieldKind, help: &str) -> Field {
        Field { key: key.into(), label: label.into(), kind, required: false, help: help.into(), default: String::new() }
    }
    pub fn required(mut self) -> Field {
        self.required = true;
        self
    }
    pub fn default(mut self, d: &str) -> Field {
        self.default = d.into();
        self
    }
}

/// How to reach a source. Holds no secret: `secrets` maps a field key to a vault reference.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct SourceConfig {
    pub id: String,
    pub kind: String,
    pub name: String,
    #[serde(default)]
    pub values: BTreeMap<String, String>,
    #[serde(default)]
    pub secrets: BTreeMap<String, String>,
}

impl SourceConfig {
    pub fn value(&self, key: &str) -> &str {
        self.values.get(key).map_or("", |s| s.trim())
    }
    pub fn flag(&self, key: &str) -> bool {
        matches!(self.value(key), "true" | "yes" | "1" | "on")
    }
    pub fn number(&self, key: &str, default: u64) -> u64 {
        self.value(key).parse().unwrap_or(default)
    }
}

/// What a kind of source is: shown by the "add source" form, which is generated from this.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct KindInfo {
    pub kind: String,
    pub label: String,
    pub language: Lang,
    pub about: String,
    pub fields: Vec<Field>,
    /// Default port / address hint.
    pub example: String,
}

/// The query language the editor offers.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Lang {
    PromQl,
    Sql,
    EsDsl,
}

impl Lang {
    pub fn name(self) -> &'static str {
        match self {
            Lang::PromQl => "PromQL",
            Lang::Sql => "SQL",
            Lang::EsDsl => "Elasticsearch query (JSON)",
        }
    }
}

/// Common connection fields (address, authentication, TLS, proxy, timeout).
pub fn net_fields(default_url: &str) -> Vec<Field> {
    vec![
        Field::new("url", "Address", FieldKind::Url, "http:// or https:// with the port").required().default(default_url),
        Field::new("auth", "Sign-in", FieldKind::Choice(vec!["none".into(), "basic".into(), "bearer".into(), "api_key".into()]), "how the server knows you").default("none"),
        Field::new("username", "User name", FieldKind::Text, "for basic sign-in; prefer a read-only user"),
        Field::new("password", "Password", FieldKind::Secret, "vault reference of the password (basic)"),
        Field::new("token", "Token / API key", FieldKind::Secret, "vault reference of the bearer token or API key"),
        Field::new("ca_file", "CA certificate file", FieldKind::Path, "PEM file to trust in addition to the system roots (private CA)"),
        Field::new("client_cert_file", "Client certificate file", FieldKind::Path, "PEM file for mutual TLS"),
        Field::new("client_key", "Client key", FieldKind::Secret, "vault reference of the PEM private key for mutual TLS"),
        Field::new("insecure", "Skip certificate checks", FieldKind::Bool, "NOT safe: anyone on the network could pose as the server").default("false"),
        Field::new("proxy", "Proxy", FieldKind::Url, "http://proxy:3128 (optional)"),
        Field::new("timeout_s", "Timeout (s)", FieldKind::Number, "per request, up to 120").default("30"),
    ]
}

/// Check a config against its kind: required fields present, secrets are vault references (never values), no value hides in
/// a secret field, numbers are numbers. Returns every problem.
pub fn validate(cfg: &SourceConfig, info: &KindInfo) -> Vec<String> {
    let mut bad = vec![];
    if !valid_id(&cfg.id) {
        bad.push("the id may use letters, digits, - and _ (up to 40)".into());
    }
    if cfg.name.trim().is_empty() {
        bad.push("give the source a name".into());
    }
    for f in &info.fields {
        match &f.kind {
            FieldKind::Secret => {
                if cfg.values.contains_key(&f.key) {
                    bad.push(format!("{}: a secret is never stored in the settings; keep it in the vault and give its reference (uvault:name)", f.label));
                }
                if let Some(r) = cfg.secrets.get(&f.key).filter(|r| !r.is_empty()) {
                    if !valid_secret_ref(r) {
                        bad.push(format!("{}: `{}` is not a vault reference; use uvault:name (your vault) or vault:name (the system vault)", f.label, crate::dash::frame::clip(&redact_ref(r), 30)));
                    }
                }
            }
            FieldKind::Number => {
                let v = cfg.value(&f.key);
                if !v.is_empty() && v.parse::<u64>().is_err() {
                    bad.push(format!("{}: `{v}` is not a number", f.label));
                }
            }
            FieldKind::Url => {
                let v = cfg.value(&f.key);
                bad.extend(url_problems(&f.label, v));
            }
            _ => {}
        }
        if f.required && f.kind != FieldKind::Secret && cfg.value(&f.key).is_empty() {
            bad.push(format!("{} is required", f.label));
        }
    }
    for k in cfg.values.keys().chain(cfg.secrets.keys()) {
        if !info.fields.iter().any(|f| &f.key == k) {
            bad.push(format!("unknown setting `{k}`"));
        }
    }
    // a value that looks like a credential never belongs in the settings, whatever the field
    for (k, v) in &cfg.values {
        if info.fields.iter().find(|f| &f.key == k).is_some_and(|f| f.kind != FieldKind::Secret) && crate::credguard::detect(v).is_some() {
            bad.push(format!("{k}: this looks like a password or a token; keep secrets in the vault and pick the entry in the sign-in fields"));
        }
    }
    bad
}

/// Names of query-string keys that carry a secret.
const SECRET_PARAMS: &[&str] = &["api_key", "apikey", "api-key", "token", "access_token", "auth_token", "auth", "key", "password", "passwd", "pwd", "secret", "sig", "signature", "x-api-key", "bearer", "authorization", "private_token"];

/// Problems of an address that would save a secret in plain text: a user name and password in front of the host, or a token in the
/// query string.
pub fn url_problems(label: &str, v: &str) -> Vec<String> {
    let mut bad = vec![];
    if v.is_empty() {
        return bad;
    }
    if !(v.starts_with("http://") || v.starts_with("https://")) {
        bad.push(format!("{label}: must start with http:// or https://"));
    }
    let rest = v.split("://").nth(1).unwrap_or("");
    if rest.split(['/', '?', '#']).next().unwrap_or("").contains('@') {
        bad.push(format!("{label}: do not put a user name or a password in the address; store the secret in the vault (pick an entry or create one in the sign-in fields)"));
    }
    if let Some((_, q)) = v.split_once('?') {
        for pair in q.split(['&', ';']) {
            let k = pair.split('=').next().unwrap_or("").to_ascii_lowercase();
            if SECRET_PARAMS.contains(&k.as_str()) {
                bad.push(format!("{label}: the address carries `{k}=`; a token in the address would be saved in plain text. Remove it and use API key or bearer sign-in with a vault entry"));
            }
        }
    }
    bad
}

/// Take what must not be stored out of an address: the user name and password in front of the host and secret query parameters.
/// Returns the cleaned address and a sentence for each thing taken out.
pub fn strip_url(v: &str) -> (String, Vec<String>) {
    let mut notes = vec![];
    let Some((scheme, rest)) = v.split_once("://") else { return (v.to_string(), notes) };
    let (auth_host, tail) = match rest.find(['/', '?', '#']) {
        Some(i) => (&rest[..i], &rest[i..]),
        None => (rest, ""),
    };
    let host = match auth_host.rsplit_once('@') {
        Some((_, h)) => {
            notes.push("a user name and password in the address were removed".to_string());
            h
        }
        None => auth_host,
    };
    let mut tail = tail.to_string();
    if let Some((path, q)) = tail.clone().split_once('?') {
        let (q, frag) = q.split_once('#').map_or((q, ""), |(a, b)| (a, b));
        let keep: Vec<&str> = q
            .split('&')
            .filter(|p| {
                let k = p.split('=').next().unwrap_or("").to_ascii_lowercase();
                let secret = SECRET_PARAMS.contains(&k.as_str());
                if secret {
                    notes.push(format!("`{k}=` was removed from the address"));
                }
                !secret
            })
            .collect();
        tail = format!("{path}{}{}", if keep.is_empty() { String::new() } else { format!("?{}", keep.join("&")) }, if frag.is_empty() { String::new() } else { format!("#{frag}") });
    }
    (format!("{scheme}://{host}{tail}"), notes)
}

/// A source read from an imported file: plaintext secrets, credentials in addresses and anything that is not a vault reference are
/// taken out, with a sentence for each. The source then needs a vault entry picked before it works.
pub fn sanitize_imported_source(mut cfg: SourceConfig, info: &KindInfo) -> (SourceConfig, Vec<String>) {
    let mut notes = vec![];
    let name = cfg.name.clone();
    for f in &info.fields {
        match f.kind {
            FieldKind::Secret => {
                if cfg.values.remove(&f.key).is_some() {
                    notes.push(format!("{name}: a plain-text {} was removed; pick a vault entry for it", f.label));
                }
                if let Some(r) = cfg.secrets.get(&f.key).cloned() {
                    if !valid_secret_ref(&r) {
                        cfg.secrets.remove(&f.key);
                        notes.push(format!("{name}: the {} was not a vault reference and was removed; pick a vault entry for it", f.label));
                    }
                }
            }
            FieldKind::Url => {
                if let Some(v) = cfg.values.get(&f.key).cloned() {
                    let (clean, n) = strip_url(&v);
                    notes.extend(n.into_iter().map(|x| format!("{name}: {x}")));
                    cfg.values.insert(f.key.clone(), clean);
                }
            }
            _ => {}
        }
    }
    // anything that is not a known setting goes (it could be a secret under an odd name)
    let known: Vec<&str> = info.fields.iter().map(|f| f.key.as_str()).collect();
    cfg.values.retain(|k, _| {
        let ok = known.contains(&k.as_str());
        if !ok {
            notes.push(format!("{name}: the unknown setting `{k}` was removed"));
        }
        ok
    });
    cfg.secrets.retain(|k, _| known.contains(&k.as_str()));
    let creds: Vec<String> = cfg.values.iter().filter(|(_, v)| crate::credguard::detect(v).is_some()).map(|(k, _)| k.clone()).collect();
    for k in creds {
        cfg.values.remove(&k);
        notes.push(format!("{name}: the setting `{k}` looked like a credential and was removed"));
    }
    (cfg, notes)
}

pub fn valid_id(id: &str) -> bool {
    !id.is_empty() && id.len() <= 40 && id.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

/// `uvault:name`, `vault:name` or `ext:provider/path`: references into the vault. Anything else (a plain value, `env:`,
/// `file:`) is refused so a credential cannot live in a settings file.
pub fn valid_secret_ref(r: &str) -> bool {
    match r.split_once(':') {
        Some(("uvault" | "vault", n)) => crate::vault::valid_secret_name(n),
        Some(("ext", n)) => !n.is_empty() && n.len() < 200 && !n.contains(char::is_whitespace),
        _ => false,
    }
}

/// Never echo a rejected reference whole: it may be the secret itself typed in the wrong box.
fn redact_ref(r: &str) -> String {
    if r.contains(':') { r.split(':').next().unwrap_or("").to_string() + ":…" } else { "(hidden)".into() }
}

// ------------------------------------------------------------------------------------------------------ secrets and transport

/// Turns a vault reference into the secret, at the moment of use.
pub trait SecretResolver: Send + Sync {
    fn resolve(&self, reference: &str) -> Result<Zeroizing<String>, String>;
}

/// Makes the HTTP transport for a source (timeout, CA, proxy, mutual TLS, insecure). Tests use a fake.
pub trait Transport: Send + Sync {
    fn upstream(&self, o: &NetOptions) -> Result<Arc<dyn Upstream>, String>;
}

#[derive(Clone, Default)]
pub struct NetOptions {
    pub timeout_s: u64,
    pub ca_file: Option<String>,
    pub insecure: bool,
    pub proxy: Option<String>,
    pub client_cert_file: Option<String>,
    pub client_key: Option<Zeroizing<String>>,
}

impl fmt::Debug for NetOptions {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("NetOptions").field("timeout_s", &self.timeout_s).field("insecure", &self.insecure).field("proxy", &self.proxy.as_ref().map(|_| "set")).field("client_key", &self.client_key.as_ref().map(|_| "set")).finish()
    }
}

/// The real transport (ureq + rustls).
pub struct RealTransport;

impl Transport for RealTransport {
    fn upstream(&self, o: &NetOptions) -> Result<Arc<dyn Upstream>, String> {
        let read = |p: &Option<String>| -> Result<Option<Vec<u8>>, String> { p.as_ref().map(|p| std::fs::read(p).map_err(|e| format!("{p}: {e}"))).transpose() };
        let opts = iyakki_proxy::https::HttpOptions {
            timeout: std::time::Duration::from_secs(o.timeout_s.clamp(1, MAX_TIMEOUT_S)),
            ca_pem: read(&o.ca_file)?,
            insecure: o.insecure,
            proxy: o.proxy.clone(),
            client_cert_pem: read(&o.client_cert_file)?,
            client_key_pem: o.client_key.as_ref().map(|k| k.as_bytes().to_vec()),
        };
        Ok(Arc::new(iyakki_proxy::https::UreqUpstream::with_options(&opts)?))
    }
}

/// What a connector is built from.
#[derive(Clone)]
pub struct Deps {
    pub transport: Arc<dyn Transport>,
    pub secrets: Arc<dyn SecretResolver>,
    pub runner: Arc<dyn crate::wsexec::Runner>,
    /// Files a SQL text may name (DuckDB): the account's readable folders.
    pub path_ok: Arc<dyn Fn(&std::path::Path) -> bool + Send + Sync>,
    pub work_dir: std::path::PathBuf,
}

// ------------------------------------------------------------------------------------------------------ HTTP

/// A small JSON-over-HTTP client for one server: base address, authentication headers, limits, and the mapping of failures to
/// sentences. Credentials sit in `auth` and are only ever sent as headers; `Debug` hides them.
pub struct Http {
    up: Arc<dyn Upstream>,
    base: String,
    auth: Vec<(String, Zeroizing<String>)>,
    /// Every secret value in use (a password, a token, the encoded basic header): removed from anything that comes back.
    secrets: Vec<Zeroizing<String>>,
    what: String,
}

impl fmt::Debug for Http {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Http({} with {} credential header(s))", self.base, self.auth.len())
    }
}

pub struct Reply {
    pub status: u16,
    pub body: Vec<u8>,
}

impl Reply {
    pub fn json(&self) -> Result<Value, DashError> {
        serde_json::from_slice(&self.body).map_err(|_| {
            let head: String = String::from_utf8_lossy(&self.body).chars().take(80).collect();
            DashError::parse(format!("the answer is not JSON (it starts with `{}`)", head.replace('\n', " ")))
        })
    }
    pub fn text(&self) -> String {
        String::from_utf8_lossy(&self.body).into_owned()
    }
}

impl Http {
    /// Build from a config: the address, the sign-in headers (secrets resolved now) and the transport options.
    pub fn from_config(cfg: &SourceConfig, d: &Deps, what: &str) -> Result<Http, DashError> {
        let base = cfg.value("url").trim_end_matches('/').to_string();
        if !(base.starts_with("http://") || base.starts_with("https://")) {
            return Err(DashError::config(format!("{what}: the address `{base}` must start with http:// or https://")).fix("Edit the source and give the full address, for example http://localhost:9090"));
        }
        let secret = |key: &str| -> Result<Option<Zeroizing<String>>, DashError> {
            match cfg.secrets.get(key).filter(|r| !r.is_empty()) {
                None => Ok(None),
                Some(r) => {
                    if !valid_secret_ref(r) {
                        return Err(DashError::config(format!("{what}: the {key} must be a vault reference like uvault:name")));
                    }
                    d.secrets.resolve(r).map(Some).map_err(|e| vault_error(what, key, r, &e))
                }
            }
        };
        let mut auth: Vec<(String, Zeroizing<String>)> = vec![];
        let mut secrets: Vec<Zeroizing<String>> = vec![];
        match cfg.value("auth") {
            "" | "none" => {}
            "basic" => {
                let pw = secret("password")?.ok_or_else(|| DashError::config(format!("{what}: basic sign-in needs the password's vault reference")))?;
                let raw = Zeroizing::new(format!("{}:{}", cfg.value("username"), pw.as_str()));
                let enc = b64(raw.as_bytes());
                secrets.push(pw.clone());
                secrets.push(Zeroizing::new(enc.clone()));
                auth.push(("Authorization".into(), Zeroizing::new(format!("Basic {enc}"))));
            }
            "bearer" => {
                let t = secret("token")?.ok_or_else(|| DashError::config(format!("{what}: bearer sign-in needs the token's vault reference")))?;
                secrets.push(Zeroizing::new(t.trim().to_string()));
                auth.push(("Authorization".into(), Zeroizing::new(format!("Bearer {}", t.trim()))));
            }
            "api_key" => {
                let t = secret("token")?.ok_or_else(|| DashError::config(format!("{what}: API key sign-in needs the key's vault reference")))?;
                secrets.push(Zeroizing::new(t.trim().to_string()));
                auth.push(("Authorization".into(), Zeroizing::new(format!("ApiKey {}", t.trim()))));
            }
            other => return Err(DashError::config(format!("{what}: unknown sign-in `{other}`"))),
        }
        let o = NetOptions {
            timeout_s: cfg.number("timeout_s", DEFAULT_TIMEOUT_S).clamp(1, MAX_TIMEOUT_S),
            ca_file: Some(cfg.value("ca_file")).filter(|s| !s.is_empty()).map(String::from),
            insecure: cfg.flag("insecure"),
            proxy: Some(cfg.value("proxy")).filter(|s| !s.is_empty()).map(String::from),
            client_cert_file: Some(cfg.value("client_cert_file")).filter(|s| !s.is_empty()).map(String::from),
            client_key: secret("client_key")?,
        };
        let up = d.transport.upstream(&o).map_err(|e| DashError::config(format!("{what}: {e}")))?;
        if let Some(k) = &o.client_key {
            secrets.push(k.clone());
        }
        Ok(Http { up, base, auth, secrets, what: what.to_string() })
    }

    pub fn for_test(up: Arc<dyn Upstream>, base: &str) -> Http {
        Http { up, base: base.trim_end_matches('/').into(), auth: vec![], secrets: vec![], what: "test".into() }
    }

    pub fn base(&self) -> &str {
        &self.base
    }

    /// Send a request. `path` starts with `/` and may carry a query string (already encoded). Transport failures and error
    /// statuses become [`DashError`]s; the server's own message is kept for a 400.
    pub fn send(&self, method: &str, path: &str, extra: &[(&str, &str)], body: &[u8], cancel: &Cancel) -> Result<Reply, DashError> {
        cancel.check()?;
        let url = format!("{}{}", self.base, path);
        let mut headers: Vec<(String, String)> = self.auth.iter().map(|(k, v)| (k.clone(), v.to_string())).collect();
        headers.extend(extra.iter().map(|(k, v)| (k.to_string(), v.to_string())));
        headers.push(("Accept".into(), "application/json".into()));
        let r = self.up.request(method, &url, &headers, body).map_err(|e| self.net_error(&self.redact(&e)));
        cancel.check()?;
        let mut r = r?;
        // nothing the server sends back may carry a credential on (a server that echoes headers, an error that quotes them)
        r.body = self.redact_bytes(r.body);
        if r.body.len() >= MAX_RESPONSE_BYTES {
            return Err(DashError::new(ErrKind::Limit, format!("{}: the answer is larger than {} MB", self.what, MAX_RESPONSE_BYTES >> 20)).fix("Ask for fewer rows or a shorter time range, or add a LIMIT."));
        }
        match r.status {
            200..=299 => Ok(Reply { status: r.status, body: r.body }),
            401 | 403 => Err(DashError::new(ErrKind::Auth, format!("{}: the server refused the sign-in (HTTP {})", self.what, r.status)).fix("Check the user name and the vault secret, and that this user may read. A 403 also comes from a proxy or firewall in front of the server.")),
            400 | 422 => Err(DashError::new(ErrKind::BadQuery, format!("{}: {}", self.what, server_message(&r.body)))),
            404 => Err(DashError::new(ErrKind::BadQuery, format!("{}: not found ({})", self.what, server_message(&r.body))).fix("Check the address (a path prefix such as /prometheus may be missing) and the index / table name.")),
            408 | 504 => Err(DashError::new(ErrKind::Network, format!("{}: the server timed out (HTTP {})", self.what, r.status)).fix("Use a shorter time range or a simpler query.")),
            s => Err(DashError::new(ErrKind::Server, format!("{}: the server answered HTTP {s}: {}", self.what, server_message(&r.body)))),
        }
    }

    /// `s` without any secret value in use and without anything that looks like a credential.
    pub fn redact(&self, s: &str) -> String {
        let mut out = s.to_string();
        for sec in &self.secrets {
            if sec.len() >= 3 {
                out = out.replace(sec.as_str(), "[redacted]");
            }
        }
        for (_, h) in &self.auth {
            out = out.replace(h.as_str(), "[redacted]");
        }
        match crate::credguard::detect(&out) {
            Some(f) => f.redacted,
            None => out,
        }
    }

    fn redact_bytes(&self, b: Vec<u8>) -> Vec<u8> {
        if self.secrets.is_empty() && self.auth.is_empty() {
            return b;
        }
        let text = String::from_utf8_lossy(&b);
        let mut out = text.to_string();
        let mut changed = false;
        for sec in self.secrets.iter().map(|s| s.as_str()).chain(self.auth.iter().map(|a| a.1.as_str())) {
            if sec.len() >= 3 && out.contains(sec) {
                out = out.replace(sec, "[redacted]");
                changed = true;
            }
        }
        if changed { out.into_bytes() } else { b }
    }

    fn net_error(&self, e: &str) -> DashError {
        let l = e.to_ascii_lowercase();
        let (msg, fix) = if l.contains("refused") {
            ("connection refused", "Nothing is listening at that address and port. Check the address, that the server is running, and any firewall.")
        } else if l.contains("timed out") || l.contains("timeout") {
            ("the server did not answer in time", "Check the address, or raise the timeout in the source (up to 120 s), or use a shorter time range.")
        } else if l.contains("certificate") || l.contains("tls") || l.contains("ssl") || l.contains("unknownissuer") || l.contains("invalid peer") {
            ("the TLS certificate was not accepted", "Add the server's CA certificate file to the source. Only as a last resort, switch on 'Skip certificate checks' (not safe).")
        } else if l.contains("dns") || l.contains("resolve") || l.contains("lookup") || l.contains("no such host") || l.contains("name or service") {
            ("the host name could not be found", "Check the spelling of the host name, or use its IP address.")
        } else {
            ("the server could not be reached", "Check the address and your network.")
        };
        // the transport's text may name the URL (never a credential: those travel in headers); keep it short
        DashError::new(ErrKind::Network, format!("{}: {msg} ({})", self.what, crate::dash::frame::clip(e.lines().next().unwrap_or(""), 120))).fix(fix)
    }
}

/// "unlock the vault" or "entry X is missing", without ever showing a value.
pub fn vault_error(what: &str, key: &str, reference: &str, e: &str) -> DashError {
    let l = e.to_ascii_lowercase();
    let name = reference.split_once(':').map_or(reference, |x| x.1);
    if l.contains("locked") {
        DashError::config(format!("{what}: your vault is locked, so the {key} cannot be read")).fix("Unlock the vault (sign in again with your password) and test the source again.")
    } else if l.contains("not found") || l.contains("no such") || l.contains("missing") {
        DashError::config(format!("{what}: the vault entry `{name}` is missing")).fix("Pick another entry for this source, or create the entry in SETTINGS > Vault.")
    } else {
        DashError::config(format!("{what}: the vault entry `{name}` could not be read")).fix("Check the vault (SETTINGS > Vault), then test the source again.")
    }
}

/// The message inside a server error body: Elasticsearch `error.reason`, Prometheus `error`, ClickHouse text, or plain text.
pub fn server_message(body: &[u8]) -> String {
    if let Ok(v) = serde_json::from_slice::<Value>(body) {
        if let Some(r) = v.pointer("/error/reason").and_then(Value::as_str) {
            let root = v.pointer("/error/root_cause/0/reason").and_then(Value::as_str).filter(|x| *x != r);
            return match root {
                Some(x) => format!("{r}: {x}"),
                None => r.to_string(),
            };
        }
        if let Some(e) = v.get("error").and_then(Value::as_str) {
            return e.to_string();
        }
        if let Some(e) = v.get("exception").and_then(Value::as_str) {
            return e.trim().to_string();
        }
        if let Some(e) = v.get("message").and_then(Value::as_str) {
            return e.to_string();
        }
    }
    let t = String::from_utf8_lossy(body);
    let t = t.trim();
    if t.is_empty() { "no message".into() } else { crate::dash::frame::clip(&t.lines().take(3).collect::<Vec<_>>().join(" "), 400) }
}

pub fn b64(b: &[u8]) -> String {
    const T: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::new();
    for c in b.chunks(3) {
        let n = (c[0] as u32) << 16 | (*c.get(1).unwrap_or(&0) as u32) << 8 | *c.get(2).unwrap_or(&0) as u32;
        out.push(T[(n >> 18) as usize & 63] as char);
        out.push(T[(n >> 12) as usize & 63] as char);
        out.push(if c.len() > 1 { T[(n >> 6) as usize & 63] as char } else { '=' });
        out.push(if c.len() > 2 { T[n as usize & 63] as char } else { '=' });
    }
    out
}

/// Percent-encode for a query string value.
pub fn enc(s: &str) -> String {
    let mut o = String::new();
    for b in s.bytes() {
        if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.' | b'~') {
            o.push(b as char);
        } else {
            o.push_str(&format!("%{b:02X}"));
        }
    }
    o
}

// ------------------------------------------------------------------------------------------------------ requests

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TimeRange {
    pub from_ms: i64,
    pub to_ms: i64,
}

impl TimeRange {
    pub fn last(ms: i64, now_ms: i64) -> TimeRange {
        TimeRange { from_ms: now_ms - ms, to_ms: now_ms }
    }
    pub fn len_ms(&self) -> i64 {
        (self.to_ms - self.from_ms).max(1)
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct QueryRequest {
    /// The query in the connector's language (after variables were filled in).
    pub text: String,
    /// The index / database / table / file the query is about, when the language needs one apart from the text.
    #[serde(default)]
    pub target: Option<String>,
    pub range: TimeRange,
    /// Resolution for time series (Prometheus `step`); `None` asks for a single value / the table as is.
    #[serde(default)]
    pub step_ms: Option<i64>,
    #[serde(default = "default_limit")]
    pub limit: usize,
    #[serde(default)]
    pub timeout_s: Option<u64>,
}

fn default_limit() -> usize {
    1000
}

impl QueryRequest {
    pub fn new(text: &str, range: TimeRange) -> QueryRequest {
        QueryRequest { text: text.into(), target: None, range, step_ms: None, limit: default_limit(), timeout_s: None }
    }
    pub fn limit(&self) -> usize {
        self.limit.clamp(1, MAX_ROWS)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NodeKind {
    Database,
    Index,
    Table,
    Metric,
    Column,
    Label,
    Field,
    File,
}

/// One entry of the schema browser.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Node {
    pub name: String,
    pub kind: NodeKind,
    /// Type for a column / field, size or count for a table / index, a sample for a label.
    #[serde(default)]
    pub detail: String,
    /// Can be opened for more (a table has columns).
    #[serde(default)]
    pub has_children: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TestInfo {
    pub ok: bool,
    /// "Prometheus 2.53.0", "Elasticsearch 8.14 (green)".
    pub detail: String,
}

pub trait DataSource: Send + Sync {
    fn id(&self) -> &str;
    fn kind(&self) -> &str;
    fn language(&self) -> Lang;
    /// Reach the server and prove the credentials work, without reading data.
    fn test(&self, cancel: &Cancel) -> Result<TestInfo, DashError>;
    /// Browse the schema: `path` empty lists the top level (databases, indices, metrics); `[x]` lists what is inside x.
    fn browse(&self, path: &[String], cancel: &Cancel) -> Result<Vec<Node>, DashError>;
    fn query(&self, req: &QueryRequest, cancel: &Cancel) -> Result<Frame, DashError>;
    /// A query that lists a sensible first look at `target` (a table, an index, a metric), for the "new panel" form.
    fn starter(&self, target: &str) -> String;
}

/// Start a stopwatch for `meta.took_ms`.
pub fn took(t: std::time::Instant) -> u64 {
    t.elapsed().as_millis() as u64
}

#[cfg(test)]
pub mod fake {
    use super::*;
    use std::sync::Mutex;

    /// A scripted upstream: the first rule whose text is contained in `METHOD url` answers.
    #[derive(Default)]
    pub struct FakeUp {
        pub rules: Mutex<Vec<(String, u16, Vec<u8>)>>,
        pub seen: Mutex<Vec<(String, String, Vec<(String, String)>, String)>>,
        pub fail: Mutex<Option<String>>,
    }

    impl FakeUp {
        pub fn new() -> Arc<FakeUp> {
            Arc::new(FakeUp::default())
        }
        pub fn on(self: &Arc<Self>, contains: &str, status: u16, body: &str) -> Arc<Self> {
            self.rules.lock().unwrap().push((contains.into(), status, body.as_bytes().to_vec()));
            self.clone()
        }
        pub fn urls(&self) -> Vec<String> {
            self.seen.lock().unwrap().iter().map(|s| format!("{} {}", s.0, s.1)).collect()
        }
        pub fn bodies(&self) -> Vec<String> {
            self.seen.lock().unwrap().iter().map(|s| s.3.clone()).collect()
        }
    }

    impl Upstream for FakeUp {
        fn request(&self, method: &str, url: &str, headers: &[(String, String)], body: &[u8]) -> Result<iyakki_proxy::UpstreamResponse, String> {
            self.seen.lock().unwrap().push((method.into(), url.into(), headers.to_vec(), String::from_utf8_lossy(body).into_owned()));
            if let Some(e) = self.fail.lock().unwrap().clone() {
                return Err(e);
            }
            let line = format!("{method} {url} {}", String::from_utf8_lossy(body));
            let hit = self.rules.lock().unwrap().iter().find(|r| line.contains(&r.0)).cloned();
            match hit {
                Some((_, status, body)) => Ok(iyakki_proxy::UpstreamResponse { status, body }),
                None => Ok(iyakki_proxy::UpstreamResponse { status: 404, body: format!("no rule for {line}").into_bytes() }),
            }
        }
    }

    pub struct FakeTransport(pub Arc<FakeUp>, pub Mutex<Vec<String>>);
    impl Transport for FakeTransport {
        fn upstream(&self, o: &NetOptions) -> Result<Arc<dyn Upstream>, String> {
            self.1.lock().unwrap().push(format!("{o:?}"));
            Ok(self.0.clone())
        }
    }

    pub struct FakeSecrets(pub Vec<(String, String)>);
    impl SecretResolver for FakeSecrets {
        fn resolve(&self, r: &str) -> Result<Zeroizing<String>, String> {
            self.0.iter().find(|(k, _)| k == r).map(|(_, v)| Zeroizing::new(v.clone())).ok_or_else(|| format!("no secret {r}"))
        }
    }

    pub fn deps(up: &Arc<FakeUp>, secrets: &[(&str, &str)], runner: Arc<dyn crate::wsexec::Runner>) -> Deps {
        Deps {
            transport: Arc::new(FakeTransport(up.clone(), Mutex::new(vec![]))),
            secrets: Arc::new(FakeSecrets(secrets.iter().map(|(a, b)| (a.to_string(), b.to_string())).collect())),
            runner,
            path_ok: Arc::new(|p| p.starts_with("/home/amy")),
            work_dir: std::env::temp_dir(),
        }
    }

    pub fn cfg(kind: &str, url: &str) -> SourceConfig {
        let mut c = SourceConfig { id: "s1".into(), kind: kind.into(), name: "test".into(), ..Default::default() };
        c.values.insert("url".into(), url.into());
        c
    }

    pub fn range() -> TimeRange {
        TimeRange { from_ms: 1_792_065_600_000, to_ms: 1_792_069_200_000 }
    }
}
