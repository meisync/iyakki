//! The service's configuration: one small JSON file (`/etc/iyakki/service.json` by default, or `--config FILE`, or
//! `IYAKKI_SERVICE_CONFIG`). Every field has a safe default, unknown fields are an error (a typo must not silently open
//! something), and the checks that keep the service on loopback unless the administrator opts in live here.

use super::Profile;
use serde::{Deserialize, Serialize};
use std::net::{IpAddr, SocketAddr};
use std::path::{Path, PathBuf};

pub const DEFAULT_PORT: u16 = 8444;
pub const DEFAULT_PATH: &str = "/etc/iyakki/service.json";

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Limits {
    /// Requests a signed-in person may make in a minute.
    pub requests_per_minute: u32,
    /// Sign-in tries per client address in a minute.
    pub logins_per_minute: u32,
    /// Chat runs one person may have going at once.
    pub concurrent_runs: u32,
    /// Largest request body.
    pub max_body_bytes: usize,
    /// Open connections.
    pub max_connections: usize,
    /// How long a tool call waits for an approval before it is refused.
    pub approval_timeout_secs: u64,
    /// Longest a signed-in token lives, however busy it is.
    pub token_lifetime_secs: u64,
    /// A token unused for this long ends.
    pub token_idle_secs: u64,
}

impl Default for Limits {
    fn default() -> Limits {
        Limits { requests_per_minute: 60, logins_per_minute: 10, concurrent_runs: 4, max_body_bytes: 1 << 20, max_connections: 128, approval_timeout_secs: 300, token_lifetime_secs: 24 * 3600, token_idle_secs: 8 * 3600 }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct TlsFiles {
    /// PEM certificate chain (the administrator's own). Both paths or neither.
    pub cert: String,
    pub key: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct ServiceConfig {
    /// `agent-api` or `cloud` (see [`Profile`]).
    pub profile: String,
    /// `ip:port`. Loopback unless `lan` is true.
    pub listen: String,
    /// The explicit opt-in to listen on a non-loopback address. Needs `allow_from`.
    pub lan: bool,
    /// Who the firewall lets in when `lan` is on: CIDR blocks such as `192.168.1.0/24`. Never empty with `lan`.
    pub allow_from: Vec<String>,
    /// The administrator's own certificate; empty means a self-signed one is made under `<data_dir>/service/tls`.
    pub tls: TlsFiles,
    /// Where the (hashed) sign-in tokens are kept so they survive a restart; empty keeps them in memory only.
    pub token_store: String,
    /// The machine's data directory (accounts, runs, settings).
    pub data_dir: String,
    /// Browser origins allowed to call the API (exact match, `https://app.example`). Empty: none (CORS locked).
    pub cors_origins: Vec<String>,
    /// High-risk tools an administrator enabled by name (each still asks for approval on every call). Default none.
    pub enable_tools: Vec<String>,
    pub limits: Limits,
}

impl Default for ServiceConfig {
    fn default() -> ServiceConfig {
        ServiceConfig {
            profile: "agent-api".into(),
            listen: format!("127.0.0.1:{DEFAULT_PORT}"),
            lan: false,
            allow_from: vec![],
            tls: TlsFiles::default(),
            token_store: String::new(),
            data_dir: "/var/lib/iyakki".into(),
            cors_origins: vec![],
            enable_tools: vec![],
            limits: Limits::default(),
        }
    }
}

/// A checked configuration.
#[derive(Clone, Debug)]
pub struct Checked {
    pub cfg: ServiceConfig,
    pub profile: Profile,
    pub addr: SocketAddr,
}

impl ServiceConfig {
    pub fn parse(text: &str) -> Result<ServiceConfig, String> {
        serde_json::from_str(text).map_err(|e| format!("service config: {e}"))
    }

    pub fn load(path: &Path) -> Result<ServiceConfig, String> {
        let t = std::fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
        ServiceConfig::parse(&t)
    }

    /// `--profile`, `IYAKKI_PROFILE` and `IYAKKI_LISTEN` beat the file; the file beats the defaults.
    pub fn with_overrides(mut self, profile: Option<&str>, listen: Option<&str>) -> ServiceConfig {
        if let Some(p) = profile.filter(|p| !p.is_empty()) {
            self.profile = p.into();
        }
        if let Some(l) = listen.filter(|l| !l.is_empty()) {
            self.listen = l.into();
        }
        self
    }

    pub fn tls_dir(&self) -> PathBuf {
        Path::new(&self.data_dir).join("service").join("tls")
    }

    /// Refuse what is unsafe or unknown: an unknown profile, a listen address that is not `ip:port`, a LAN address without the
    /// opt-in (or without an `allow_from`), half a certificate, a high-risk tool name that does not exist in the profile's
    /// reach, limits of zero.
    pub fn check(&self) -> Result<Checked, String> {
        let profile = Profile::parse(&self.profile)?;
        let addr: SocketAddr = self.listen.parse().map_err(|_| format!("listen: \"{}\" is not ip:port (a host name is not accepted: say which address)", self.listen))?;
        if addr.port() == 0 && !addr.ip().is_loopback() {
            return Err("listen: port 0 is only for tests on loopback".into());
        }
        if !is_loopback(addr.ip()) {
            if !self.lan {
                return Err(format!("listen: {} is not a loopback address; the service listens on loopback only unless \"lan\": true is set (and \"allow_from\" names who may connect)", self.listen));
            }
            if self.allow_from.is_empty() {
                return Err("lan is on but \"allow_from\" is empty: name the networks allowed to connect (for example \"192.168.1.0/24\")".into());
            }
        }
        for c in &self.allow_from {
            parse_cidr(c).map_err(|e| format!("allow_from \"{c}\": {e}"))?;
            if matches!(c.as_str(), "0.0.0.0/0" | "::/0") {
                return Err("allow_from: the whole internet is not accepted; name a network".into());
            }
        }
        if self.tls.cert.is_empty() != self.tls.key.is_empty() {
            return Err("tls: give both cert and key, or neither".into());
        }
        for o in &self.cors_origins {
            if o == "*" || !(o.starts_with("https://") || o.starts_with("http://localhost") || o.starts_with("http://127.0.0.1")) || o.ends_with('/') {
                return Err(format!("cors_origins: \"{o}\" is not an exact origin (https://host[:port], no wildcard, no trailing slash)"));
            }
        }
        let l = &self.limits;
        if l.requests_per_minute == 0 || l.logins_per_minute == 0 || l.concurrent_runs == 0 || l.max_body_bytes == 0 || l.max_connections == 0 || l.approval_timeout_secs == 0 || l.token_lifetime_secs == 0 || l.token_idle_secs == 0 {
            return Err("limits: every limit must be above zero".into());
        }
        if l.max_body_bytes > 8 << 20 {
            return Err("limits: max_body_bytes above 8 MiB is not accepted".into());
        }
        for t in &self.enable_tools {
            if !profile.high_risk_enableable().contains(&t.as_str()) {
                return Err(format!("enable_tools: \"{t}\" is not a tool this profile can offer (see docs/development/MINIMAL-WEBSERVICE.md, section 9)"));
            }
        }
        Ok(Checked { cfg: self.clone(), profile, addr })
    }
}

pub fn is_loopback(ip: IpAddr) -> bool {
    ip.is_loopback()
}

/// `a.b.c.d/n` or an IPv6 block; returns (address, prefix length).
pub fn parse_cidr(s: &str) -> Result<(IpAddr, u8), String> {
    let (a, n) = s.split_once('/').ok_or("not a network block (address/length)")?;
    let ip: IpAddr = a.parse().map_err(|_| "bad address".to_string())?;
    let n: u8 = n.parse().map_err(|_| "bad length".to_string())?;
    let max = if ip.is_ipv4() { 32 } else { 128 };
    if n > max {
        return Err("length too long".into());
    }
    Ok((ip, n))
}
