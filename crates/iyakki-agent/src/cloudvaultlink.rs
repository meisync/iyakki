//! How a tenant's pod finds its vault service (docs/development/CLOUD-MULTITENANT-PLAN.md, 3a): the environment the launch writes into the
//! pod (`cloudtenant.rs`) is `IYAKKI_VAULT_URL` (https, a host name and an optional port), optionally `IYAKKI_VAULT_FINGERPRINT` (the
//! certificate's SHA-256 to pin) and `IYAKKI_TENANT` (the name the vault knows the tenant by).
//!
//! * [`Link`]: not configured, configured badly, or a [`VaultTarget`]; [`Link::from_env`] reads the three variables.
//! * [`status`]: one word for the health detail of the cloud profile, `not_configured`, `invalid_address`, `unreachable` or `ok`. It is
//!   found with a short TCP connect and an unsigned request (the service answers 401 to it, which is enough to say a vault answers); it is
//!   cached for 15 seconds, so a probe never runs for every health check. The address, the fingerprint and the error text are never part of
//!   the answer: a health check is public, and what it says is the word only.
//! * A vault's failure never makes the pod "not ready": the tenant's desktop works without it, only the shared secrets do not.

use crate::cloudkeys::MemberKeys;
use crate::cloudstorage::plain_name;
use crate::cloudvaultd::{Client, Principal};
use std::net::{TcpStream, ToSocketAddrs};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

pub const ENV_URL: &str = "IYAKKI_VAULT_URL";
pub const ENV_FINGERPRINT: &str = "IYAKKI_VAULT_FINGERPRINT";
pub const ENV_TENANT: &str = "IYAKKI_TENANT";

const CACHE_FOR: Duration = Duration::from_secs(15);

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct VaultTarget {
    pub host: String,
    pub port: u16,
    pub fingerprint: Option<String>,
    pub tenant: Option<String>,
}

impl VaultTarget {
    /// `https://host[:port]` (the port is 443 when missing).
    pub fn parse(url: &str, fingerprint: Option<&str>, tenant: Option<&str>) -> Result<VaultTarget, String> {
        crate::cloudvault::check_url(url)?;
        let rest = url.strip_prefix("https://").unwrap_or(url);
        let (host, port) = match rest.rsplit_once(':') {
            Some((h, p)) if !p.is_empty() && p.bytes().all(|c| c.is_ascii_digit()) => (h, p.parse::<u16>().map_err(|_| "the vault port is not valid".to_string())?),
            _ => (rest, 443),
        };
        if tenant.is_some_and(|t| !plain_name(t)) {
            return Err("the tenant name is not a plain name".into());
        }
        Ok(VaultTarget { host: host.into(), port, fingerprint: fingerprint.filter(|f| !f.is_empty()).map(String::from), tenant: tenant.map(String::from) })
    }

    /// The member's side for `user` with `keys`; needs the tenant name.
    pub fn client(&self, user: &str, keys: MemberKeys) -> Result<Client, String> {
        let tenant = self.tenant.clone().ok_or("the tenant's name for the vault is not known (IYAKKI_TENANT)")?;
        if !plain_name(user) {
            return Err("not a plain user name".into());
        }
        Ok(Client { host: self.host.clone(), port: self.port, fingerprint: self.fingerprint.clone(), who: Principal::Member { tenant, user: user.into() }, keys })
    }

    /// `ok` when something that speaks TLS and HTTP answers (and the pinned certificate, if one is named, is the one it shows).
    pub fn probe(&self) -> &'static str {
        let reach = (self.host.as_str(), self.port).to_socket_addrs().ok().and_then(|mut a| a.next()).and_then(|a| TcpStream::connect_timeout(&a, Duration::from_secs(2)).ok());
        if reach.is_none() {
            return "unreachable";
        }
        drop(reach);
        match crate::devmode::client::https(&self.host, self.port, self.fingerprint.as_deref(), "GET", "/v1/ping", &[], b"", Duration::from_secs(3)) {
            Ok(r) if r.status < 500 => "ok",
            _ => "unreachable",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Link {
    NotConfigured,
    /// Configured, but the address (or the tenant name) is not usable. The text is for the log, not for an answer.
    Invalid(String),
    Target(VaultTarget),
}

impl Link {
    pub fn from_values(url: Option<&str>, fingerprint: Option<&str>, tenant: Option<&str>) -> Link {
        match url.map(str::trim).filter(|u| !u.is_empty()) {
            None => Link::NotConfigured,
            Some(u) => match VaultTarget::parse(u, fingerprint, tenant.filter(|t| !t.is_empty())) {
                Ok(t) => Link::Target(t),
                Err(e) => Link::Invalid(e),
            },
        }
    }

    pub fn from_env() -> Link {
        let get = |k: &str| std::env::var(k).ok();
        Link::from_values(get(ENV_URL).as_deref(), get(ENV_FINGERPRINT).as_deref(), get(ENV_TENANT).as_deref())
    }

    pub fn configured(&self) -> bool {
        *self != Link::NotConfigured
    }
}

type Cache = Mutex<Option<(Link, Instant, &'static str)>>;

fn cache() -> &'static Cache {
    static C: OnceLock<Cache> = OnceLock::new();
    C.get_or_init(|| Mutex::new(None))
}

/// The one word for the health detail of `link` (cached for 15 seconds per distinct link).
pub fn status(link: &Link) -> &'static str {
    match link {
        Link::NotConfigured => "not_configured",
        Link::Invalid(e) => {
            log::warn!("vault link: {e}");
            "invalid_address"
        }
        Link::Target(t) => {
            {
                let c = cache().lock().unwrap_or_else(|e| e.into_inner());
                if let Some((l, at, word)) = c.as_ref() {
                    if l == link && at.elapsed() < CACHE_FOR {
                        return word;
                    }
                }
            }
            let word = t.probe();
            *cache().lock().unwrap_or_else(|e| e.into_inner()) = Some((link.clone(), Instant::now(), word));
            word
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_environment_values_become_a_link_or_a_reason_not_to() {
        assert_eq!(Link::from_values(None, None, None), Link::NotConfigured);
        assert_eq!(Link::from_values(Some("  "), None, None), Link::NotConfigured);
        let l = Link::from_values(Some("https://vault.iyakki-vault.svc:8445"), Some("ab:cd"), Some("acme"));
        assert_eq!(l, Link::Target(VaultTarget { host: "vault.iyakki-vault.svc".into(), port: 8445, fingerprint: Some("ab:cd".into()), tenant: Some("acme".into()) }));
        match Link::from_values(Some("https://vault.example.org"), Some(""), None) {
            Link::Target(t) => assert_eq!((t.port, t.fingerprint, t.tenant), (443, None, None)),
            x => panic!("{x:?}"),
        }
        for bad in ["http://plain", "vault:8445", "https://x y", "https://h:0", "https://h:99999", "https://"] {
            assert!(matches!(Link::from_values(Some(bad), None, None), Link::Invalid(_)), "{bad}");
        }
        assert!(matches!(Link::from_values(Some("https://h"), None, Some("../x")), Link::Invalid(_)));
        assert!(Link::NotConfigured.configured() == false && Link::Invalid("x".into()).configured());
        // a client needs the tenant's name
        let t = VaultTarget::parse("https://h:1", None, None).unwrap();
        assert!(t.client("bob", MemberKeys::from_seed([1; 32])).is_err());
    }

    #[test]
    fn the_status_is_one_word_and_names_nothing() {
        assert_eq!(status(&Link::NotConfigured), "not_configured");
        assert_eq!(status(&Link::Invalid("https://secret-host:1 is not an address".into())), "invalid_address");
        // nothing listens on a closed loopback port
        let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = l.local_addr().unwrap().port();
        drop(l);
        let dead = Link::from_values(Some(&format!("https://127.0.0.1:{port}")), None, None);
        assert_eq!(status(&dead), "unreachable");
        // a real vault service on loopback answers (401 to the unsigned probe is an answer)
        let d = crate::testdir::unique("vaultlink", "ok");
        let run = crate::cloudvaultd::start("127.0.0.1:0".parse().unwrap(), &d, None).unwrap();
        let url = format!("https://127.0.0.1:{}", run.local_addr().port());
        assert_eq!(status(&Link::from_values(Some(&url), Some(&run.fingerprint), None)), "ok");
        // the wrong pinned certificate is not "ok"
        assert_eq!(status(&Link::from_values(Some(&url), Some("00:11"), None)), "unreachable");
        run.stop();
    }
}
