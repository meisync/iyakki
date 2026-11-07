//! Puts the finished parts of the domain listener together for `iyakki cloud listen` (docs/development/CLOUD-MULTITENANT-PLAN.md, 3c).
//!
//! [`build`] turns the command line (and the environment) into a [`Setup`]: the certificate source, the firewall gate and the
//! [`Options`]. Everything that touches the world (the vault, the firewall, `kubectl`, the environment) comes in through [`Deps`], so
//! the tests use fakes and nothing here starts a process or opens a socket. Every step is behind a flag and the defaults fail closed:
//!
//! * Certificates: `--cert-source file` (default; `NAME.crt` and `NAME.key` in `--cert-dir`) or `--cert-source vault` (the PEM chain
//!   and key are read from the vault into memory on every reload; no key is written to disk; the member's key file is named by
//!   `--vault-key`). A vault that is down means the https hosts are not served, never a fallback to files.
//! * Mutual TLS: `--client-ca HOST=NAME` (repeatable) for an https route; the CA is read from the same source. A name that is not an
//!   https route of the table is an error (it would silently do nothing).
//! * Pods: `--service-dialer port-forward|cluster-dns` with `--namespace`.
//! * Firewall: the real gate is the default. A listener that is not bound to a loopback address and has no working firewall does not
//!   start. `--no-firewall` is accepted only with a loopback `--bind`. `--reach local|anywhere` (default local network only).
//! * Tenants: `--route-tenant HOST=TENANT` (repeatable) and `--tenant-limit N` (default 64 open requests per tenant).

use std::collections::BTreeMap;
use std::net::IpAddr;
use std::sync::Arc;
use std::time::Duration;

use super::gate::{FirewallGate, GateReach};
use super::pod::{ClusterDns, PortForward};
use super::vault::{Reader, SecretReader, VaultCerts};
use super::{CertSource, FileCerts, NoGate, Options, PortGate, ServiceDialer};
use crate::cloudkeys::MemberKeys;
use crate::cloudroute::{RouteTable, Scheme, Target};
use crate::cloudstorage::plain_name;
use crate::cloudvaultd::{Client, Scope};
use crate::cloudvaultlink::{Link, ENV_FINGERPRINT, ENV_TENANT, ENV_URL};

pub const USAGE: &str = "iyakki cloud listen --routes FILE [--bind ADDR] [--redirect-port PORT]\n    \
    [--cert-source file|vault] [--cert-dir DIR]   (file: NAME.crt/NAME.key in DIR; default source is file, DIR defaults to .)\n    \
    [--vault URL] [--vault-fingerprint SHA256] [--tenant NAME] [--vault-user NAME] [--vault-key FILE] [--vault-scope tenant|common|namespace:NAME]\n    \
        (vault source: certificates NAME-chain and NAME-key are read into memory, no key touches the disk; the URL, fingerprint and tenant\n    \
        also come from IYAKKI_VAULT_URL, IYAKKI_VAULT_FINGERPRINT, IYAKKI_TENANT)\n    \
    [--client-ca HOST=NAME]...   (mutual TLS for the https route HOST: only clients with a certificate of the CA NAME are served)\n    \
    [--service-dialer none|port-forward|cluster-dns] [--namespace NS] [--cluster-domain DOMAIN] [--kubeconfig FILE] [--context NAME]\n    \
        (routes whose target is a Kubernetes Service; without a dialer they answer 502)\n    \
    [--reach local|anywhere]   (who the firewall lets reach the ports: the local network (default) or every address)\n    \
    [--no-firewall]            (do not drive the firewall; accepted only when --bind is a loopback address)\n    \
    [--firewall-start]         (start this process's firewall from the saved settings first, as the shell does at boot)\n    \
    [--route-tenant HOST=TENANT]... [--tenant-limit N]   (tenant of each route; every route must have one; N open requests per tenant, default 64)\n  \
  Without a working firewall the listener does NOT open a port to the network: it stops with the reason.";

/// What the outside world is asked for. [`Deps::real`] is the real one.
pub struct Deps {
    pub env: Box<dyn Fn(&str) -> Option<String>>,
    pub read_file: Box<dyn Fn(&str) -> Result<String, String>>,
    /// The secrets of a vault scope as a reader (the real one talks to the vault service).
    pub vault: Box<dyn Fn(Client, Scope) -> Box<dyn SecretReader>>,
    /// The firewall gate; the flag says "start this process's firewall engine first".
    pub gate: Box<dyn Fn(GateReach, bool) -> Result<Arc<dyn PortGate>, String>>,
    pub port_forward: Box<dyn Fn(&str, Option<&str>, Option<&str>) -> Result<Arc<dyn ServiceDialer>, String>>,
}

impl Deps {
    pub fn real() -> Deps {
        Deps {
            env: Box::new(|k| std::env::var(k).ok()),
            read_file: Box::new(|p| std::fs::read_to_string(p).map_err(|e| format!("{p}: {e}"))),
            vault: Box::new(|client, scope| Box::new(Reader { client, scope })),
            gate: Box::new(|reach, start| {
                if start {
                    let cmdline = std::fs::read_to_string("/proc/cmdline").unwrap_or_default();
                    iyakki_fw::service::boot(&cmdline);
                }
                Ok(Arc::new(FirewallGate::real(reach)))
            }),
            port_forward: Box::new(|ns, kc, ctx| Ok(Arc::new(PortForward::new(ns, kc, ctx)?))),
        }
    }
}

pub struct Setup {
    pub source: Arc<dyn CertSource>,
    pub gate: Arc<dyn PortGate>,
    pub options: Options,
    /// Lines for the person: what was chosen, and what is not covered.
    pub notes: Vec<String>,
}

fn opt(a: &[String], name: &str) -> Option<String> {
    a.iter().position(|x| x == name).and_then(|i| a.get(i + 1)).cloned()
}

fn all(a: &[String], name: &str) -> Vec<String> {
    a.iter().enumerate().filter(|(_, x)| *x == name).filter_map(|(i, _)| a.get(i + 1).cloned()).collect()
}

fn pair(flag: &str, v: &str) -> Result<(String, String), String> {
    match v.split_once('=') {
        Some((h, n)) if !h.is_empty() && plain_name(n) => Ok((h.to_ascii_lowercase(), n.to_string())),
        _ => Err(format!("{flag} \"{v}\" is not HOST=NAME")),
    }
}

fn is_loopback(ip: IpAddr) -> bool {
    ip.is_loopback()
}

pub fn build(a: &[String], table: &RouteTable, deps: &Deps) -> Result<Setup, String> {
    let mut o = Options::default();
    let mut notes = Vec::new();
    if let Some(b) = opt(a, "--bind") {
        o.bind = b.parse().map_err(|_| format!("--bind \"{b}\" is not an address"))?;
    }
    if let Some(p) = opt(a, "--redirect-port") {
        o.redirect_port = p.parse().map_err(|_| format!("--redirect-port \"{p}\" is not a port"))?;
    }

    // (a) certificates
    let source: Arc<dyn CertSource> = match opt(a, "--cert-source").as_deref().unwrap_or("file") {
        "file" => Arc::new(FileCerts::new(opt(a, "--cert-dir").unwrap_or_else(|| ".".into()))),
        "vault" => {
            if opt(a, "--cert-dir").is_some() {
                return Err("--cert-dir and --cert-source vault together: certificates come from one place only".into());
            }
            let url = opt(a, "--vault").or_else(|| (deps.env)(ENV_URL));
            let fp = opt(a, "--vault-fingerprint").or_else(|| (deps.env)(ENV_FINGERPRINT));
            let tenant = opt(a, "--tenant").or_else(|| (deps.env)(ENV_TENANT));
            let target = match Link::from_values(url.as_deref(), fp.as_deref(), tenant.as_deref()) {
                Link::Target(t) => t,
                Link::NotConfigured => return Err("--cert-source vault needs --vault URL (or IYAKKI_VAULT_URL)".into()),
                Link::Invalid(e) => return Err(format!("the vault address is not usable: {e}")),
            };
            if target.fingerprint.is_none() {
                notes.push("the vault's certificate is not pinned (no --vault-fingerprint): the connection is not protected against a fake vault".into());
            }
            let user = opt(a, "--vault-user").ok_or("--vault-user NAME is needed with the vault source")?;
            let keyfile = opt(a, "--vault-key").ok_or("--vault-key FILE is needed with the vault source (the member's secret; never written by this command)")?;
            let keys = MemberKeys::from_hex(&(deps.read_file)(&keyfile)?).map_err(|e| format!("{keyfile}: {e}"))?;
            let scope_text = opt(a, "--vault-scope").unwrap_or_else(|| "tenant".into());
            let scope = Scope::parse(&scope_text).ok_or_else(|| format!("--vault-scope \"{scope_text}\" is not tenant, common or namespace:NAME"))?;
            let client = target.client(&user, keys)?;
            notes.push("certificates and keys come from the vault, in memory only".into());
            Arc::new(VaultCerts::new((deps.vault)(client, scope)))
        }
        other => return Err(format!("--cert-source \"{other}\" is not file or vault")),
    };

    // (b) mutual TLS
    for v in all(a, "--client-ca") {
        let (host, name) = pair("--client-ca", &v)?;
        if !table.routes.iter().any(|r| r.scheme == Scheme::Https && r.host.eq_ignore_ascii_case(&host)) {
            return Err(format!("--client-ca {host}: no https route has this host (a client CA on a plain or passthrough route would do nothing)"));
        }
        if o.client_ca.insert(host.clone(), name).is_some() {
            return Err(format!("--client-ca {host} is given twice"));
        }
    }

    // (c) pods
    let has_service = table.routes.iter().any(|r| matches!(r.target, Target::Service { .. }));
    match opt(a, "--service-dialer").as_deref().unwrap_or("none") {
        "none" => {
            if has_service {
                notes.push("some routes point at a Kubernetes Service and no --service-dialer is set: they answer 502".into());
            }
        }
        kind @ ("port-forward" | "cluster-dns") => {
            let ns = opt(a, "--namespace").ok_or("--namespace NS is needed with --service-dialer")?;
            let d: Arc<dyn ServiceDialer> = if kind == "port-forward" {
                (deps.port_forward)(&ns, opt(a, "--kubeconfig").as_deref(), opt(a, "--context").as_deref())?
            } else {
                Arc::new(ClusterDns::new(&ns, &opt(a, "--cluster-domain").unwrap_or_else(|| "svc.cluster.local".into()))?)
            };
            o.dialer = Some(d);
        }
        other => return Err(format!("--service-dialer \"{other}\" is not none, port-forward or cluster-dns")),
    }

    // (e) tenants
    for v in all(a, "--route-tenant") {
        let (host, tenant) = pair("--route-tenant", &v)?;
        if !table.routes.iter().any(|r| r.host.eq_ignore_ascii_case(&host)) {
            return Err(format!("--route-tenant {host}: the routes file has no route with this host"));
        }
        if o.tenants.insert(host.clone(), tenant).is_some() {
            return Err(format!("--route-tenant {host} is given twice"));
        }
    }
    if let Some(n) = opt(a, "--tenant-limit") {
        o.tenant_max_conns = n.parse().ok().filter(|n| *n > 0).ok_or_else(|| format!("--tenant-limit \"{n}\" is not a number above 0"))?;
    }
    if o.tenants.is_empty() && opt(a, "--tenant-limit").is_some() {
        return Err("--tenant-limit has no effect without --route-tenant".into());
    }
    let by: BTreeMap<_, _> = o.tenants.iter().map(|(h, t)| (h.clone(), t.clone())).collect();
    if !by.is_empty() {
        if let Some(r) = table.routes.iter().find(|r| !by.contains_key(&r.host.to_ascii_lowercase())) {
            return Err(format!("route {} has no --route-tenant; with tenants every route needs one", r.host));
        }
    }

    // (d) the firewall gate: fail closed
    let reach = match opt(a, "--reach").as_deref().unwrap_or("local") {
        "local" => GateReach::LocalNetwork,
        "anywhere" => GateReach::Anywhere,
        other => return Err(format!("--reach \"{other}\" is not local or anywhere")),
    };
    let gate: Arc<dyn PortGate> = if a.iter().any(|x| x == "--no-firewall") {
        if !is_loopback(o.bind) {
            return Err(format!("--no-firewall is only accepted with a loopback --bind (it is {}): a listener open to the network needs the firewall", o.bind));
        }
        notes.push("the firewall is not driven (loopback only)".into());
        Arc::new(NoGate)
    } else {
        (deps.gate)(reach, a.iter().any(|x| x == "--firewall-start")).map_err(|e| format!("the firewall is not available, so the listener does not open: {e}"))?
    };
    o.cert_reload = Some(Duration::from_secs(60));
    Ok(Setup { source, gate, options: o, notes })
}
