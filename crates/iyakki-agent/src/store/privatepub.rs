//! A private store that was deployed from this device but never added to its STORE, and the one obvious path from there to a publish.
//!
//! Deploying (the Platform flow) records the instance in the installed-services list with its URLs and keeps the admin token in the
//! vault, but it does not add the store as a catalog source: that needs the person to look at the catalog key's fingerprint
//! ([`Store::probe_private`], [`Store::pin_probed`]). This module finds such a store ([`offer`]), words what is wrong when it does not
//! answer ([`unreachable_text`]), compares addresses the way a person would ([`same_location`]: `localhost`, `127.0.0.1` and the
//! device's own address are one place, a trailing `/` or `/catalog.json` changes nothing) and adds it with the fingerprint pinned
//! and chosen as the publishing target ([`add_and_target`]). Nothing here trusts a key silently: the caller shows
//! [`Probe::fingerprint`] and waits for approval, and [`add_and_target`] refuses when the key it reads now differs from the one shown.
//! See docs/STORE.md, "Publishing from the IDE".

use super::catalog::{Fetch, SourceCfg};
use super::install::Actor;
use super::{privatestore, Probe, Store};
use crate::platform::installed;
use crate::services::Services;

/// Hosts that mean "this machine".
fn is_loopback(h: &str) -> bool {
    matches!(h, "localhost" | "127.0.0.1" | "::1" | "0.0.0.0") || h.starts_with("127.")
}

/// The address with scheme and host lower-cased, `/catalog.json`, `/v1/publish` and trailing slashes gone, and any address of this
/// machine (loopback or one of `own`) written as `this-device`. Two addresses of one store compare equal.
pub fn normalise_with(url: &str, own: &[String]) -> String {
    let u = url.trim();
    let (scheme, rest) = u.split_once("://").unwrap_or(("http", u));
    let rest = rest.trim_end_matches('/');
    let rest = rest.strip_suffix("/v1/publish").unwrap_or(rest);
    let rest = rest.strip_suffix("/catalog.json").unwrap_or(rest).trim_end_matches('/');
    let (host_port, path) = rest.split_once('/').map_or((rest, ""), |(h, p)| (h, p));
    let (host, port) = match host_port.strip_prefix('[') {
        Some(v6) => v6.split_once(']').map_or((v6, ""), |(h, r)| (h, r.trim_start_matches(':'))),
        None => host_port.rsplit_once(':').map_or((host_port, ""), |(h, p)| (h, p)),
    };
    let host = host.to_ascii_lowercase();
    let host = if is_loopback(&host) || own.iter().any(|o| *o == host) { "this-device".to_string() } else { host };
    let port = match (scheme.to_ascii_lowercase().as_str(), port) {
        ("http", "80") | ("https", "443") => "",
        (_, p) => p,
    };
    format!("{}://{host}{}{}{}", scheme.to_ascii_lowercase(), if port.is_empty() { String::new() } else { format!(":{port}") }, if path.is_empty() { "" } else { "/" }, path)
}

/// [`normalise_with`] for this machine's own addresses.
pub fn normalise(url: &str) -> String {
    normalise_with(url, &crate::devcluster::local_ips())
}

/// The same store address? (See [`normalise_with`].)
pub fn same_location_with(a: &str, b: &str, own: &[String]) -> bool {
    normalise_with(a, own) == normalise_with(b, own)
}

pub fn same_location(a: &str, b: &str) -> bool {
    let own = crate::devcluster::local_ips();
    same_location_with(a, b, &own)
}

/// A private store the installed-services list knows.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Deployed {
    /// The instance id (`private-store-iyakki-private-store`).
    pub instance: String,
    pub title: String,
    pub namespace: String,
    /// The address of its `catalog.json` as recorded at deploy.
    pub catalog_url: String,
    /// The catalog source already added for it (id, name).
    pub added: Option<(String, String)>,
}

/// Every private store recorded on this device that has a catalog address (running or not), with the source added for it, if any.
pub fn deployed_all(svc: &Services, user: &str, store: &Store) -> Vec<Deployed> {
    let cfg = store.config();
    let own = crate::devcluster::local_ips();
    installed::load(svc.dir(), user)
        .into_iter()
        .filter(|i| i.service == privatestore::ID)
        .filter_map(|i| {
            let catalog_url = i.urls.iter().find(|u| u.kind == "catalog")?.url.clone();
            let added = cfg.sources.iter().find(|s| same_location_with(&s.location, &catalog_url, &own)).map(|s| (s.id.clone(), s.name.clone()));
            Some(Deployed { instance: i.id.clone(), title: i.title.clone(), namespace: i.namespace.clone(), catalog_url, added })
        })
        .collect()
}

/// What the Publish form and the assistant need to offer "add it and publish": the first deployed store that is not yet a source,
/// with what its catalog says now (the key fingerprint) or why that could not be read.
#[derive(Clone, Debug)]
pub struct Offer {
    pub dep: Deployed,
    pub probe: Result<Probe, String>,
}

pub fn offer(svc: &Services, user: &str, store: &Store, fetch: &dyn Fetch) -> Option<Offer> {
    let dep = deployed_all(svc, user, store).into_iter().find(|d| d.added.is_none())?;
    let probe = store.probe_private(fetch, &dep.catalog_url).map_err(|e| unreachable_text(&dep, &e));
    Some(Offer { dep, probe })
}

/// The first words of a fingerprint, for a list row.
pub fn short_fingerprint(fp: &str) -> String {
    fp.split_whitespace().take(2).collect::<Vec<_>>().join(" ")
}

/// The Store choice's row for an unadded private store.
pub fn option_label(o: &Offer) -> String {
    match &o.probe {
        Ok(p) => format!("Private store on this device (not added yet): {} \u{b7} key {}", p.name, short_fingerprint(&p.fingerprint)),
        Err(_) => format!("Private store on this device (not added yet): {} \u{b7} not answering yet", if o.dep.title.is_empty() { &o.dep.instance } else { &o.dep.title }),
    }
}

/// What a failure to read the deployed store's catalog means, and the one next step.
pub fn unreachable_text(d: &Deployed, err: &str) -> String {
    words(d.catalog_url.trim_end_matches("/catalog.json"), Some((&d.instance, &d.namespace)), err)
}

/// [`unreachable_text`] for any store address: when it is the address of a private store deployed from this device the pod and
/// namespace are named, otherwise the store's own administrator is the one to ask.
pub fn unreachable_for(svc: &Services, user: &str, location: &str, err: &str) -> String {
    let own = crate::devcluster::local_ips();
    let inst = installed::load(svc.dir(), user).into_iter().find(|i| i.service == privatestore::ID && i.urls.iter().any(|u| u.kind == "catalog" && same_location_with(&u.url, location, &own)));
    let at = location.trim_end_matches("/catalog.json");
    match &inst {
        Some(i) => words(at, Some((&i.id, &i.namespace)), err),
        None => words(at, None, err),
    }
}

fn words(at: &str, inst: Option<(&str, &str)>, err: &str) -> String {
    let e = err.to_ascii_lowercase();
    let (what, step) = match inst {
        Some((id, ns)) => (format!("The private store {id} is deployed"), format!("open the K8S tile and check the pod private-store in namespace {ns}")),
        None => ("The store".to_string(), "ask its administrator whether it is running".to_string()),
    };
    if e.contains("answered") && !e.contains("could not reach") {
        // it answered, but not with a catalog (a 404, a 503 while the pod starts)
        format!("{what}, but {at} is not serving its catalog yet ({err}). It may still be starting: wait a minute, or {step}, then try again. Nothing was changed.")
    } else if e.contains("could not reach") || e.contains("refused") || e.contains("timed out") || e.contains("timeout") || e.contains("connect") {
        format!("{what}, but nothing answers at {at}: the pod is not ready or the cluster is stopped. Wait a minute if you just deployed it, or {step}, then try again. Nothing was changed.")
    } else {
        format!("{what}, but its catalog at {at} cannot be used ({err}). {}, then try again; if it keeps failing use STORE, Settings, Private store, Manage (Repair).", step[..1].to_uppercase() + &step[1..])
    }
}

/// Adding changes the STORE settings: administrators only, said as what to do.
pub fn need_admin_text() -> String {
    "Adding the store changes the STORE settings, which only an administrator can do. Ask an administrator to add it (STORE, Settings, Private store, Add this store), or sign in as one.".into()
}

fn norm_fp(s: &str) -> String {
    s.chars().filter(|c| !c.is_whitespace()).collect::<String>().to_ascii_lowercase()
}

/// Add the store whose key the person looked at (`shown`) with that key pinned, and make it the publishing target. The catalog is read
/// again first: a key that differs from the one shown is refused (nothing is added). Reuses [`Store::pin_probed`], the code of the
/// Settings "Add this store" step.
pub fn add_and_target(store: &Store, actor: &Actor, fetch: &dyn Fetch, shown: &Probe, name: &str) -> Result<SourceCfg, String> {
    if !(actor.allowed && actor.admin) {
        return Err(need_admin_text());
    }
    let fresh = store.probe_private(fetch, &shown.url).map_err(|e| format!("The store could not be read again before adding it: {e}. Nothing was added."))?;
    if norm_fp(&fresh.fingerprint) != norm_fp(&shown.fingerprint) {
        return Err(format!("The store's key is now {}, not the {} you were shown: nothing was added. Look at the new fingerprint and start again.", fresh.fingerprint, shown.fingerprint));
    }
    let id = store.pin_probed(actor, name, &fresh, &shown.fingerprint)?;
    store.set_publish_target(actor, &id)?;
    store.config().sources.into_iter().find(|s| s.id == id).ok_or_else(|| "The store was added but could not be found again: open STORE, Settings and check the catalogs.".to_string())
}

/// The assistant's version: `fingerprint` is what the model passed back from the tool's text, and the approval card showed it.
/// It must equal the key the store shows now.
pub fn add_for_assistant(svc: &Services, user: &str, store: &Store, actor: &Actor, fetch: &dyn Fetch, fingerprint: &str) -> Result<SourceCfg, String> {
    let o = offer(svc, user, store, fetch).ok_or("There is no deployed private store waiting to be added on this device.")?;
    let p = o.probe?;
    if norm_fp(fingerprint) != norm_fp(&p.fingerprint) {
        return Err(format!("{fingerprint:?} is not the key of the private store ({}): nothing was added. Read the fingerprint from this tool's own text and pass it back unchanged.", p.fingerprint));
    }
    add_and_target(store, actor, fetch, &p, &p.name)
}
