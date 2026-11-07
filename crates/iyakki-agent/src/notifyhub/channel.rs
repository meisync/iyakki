//! Channel configuration: the tenant-config kind `notify.channel` (keyed per team and name) and `notify.routing`. A channel is OFF
//! unless the item is switched on. Secrets are references into the tenant vault (`vault:notify.channel/<name>`), never values.

use super::rules::{Quiet, Rules};
use super::sink::Severity;
use super::smtp::TlsMode;
use crate::tenantconfig::{store::Store, Item};
use serde_json::Value;
use std::collections::BTreeMap;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Webhook,
    Email,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Channel {
    /// `<team>/<name>`, or `/<name>` for a tenant-level channel.
    pub key: String,
    pub team: Option<String>,
    pub kind: Kind,
    pub min_severity: Severity,
    /// Titles and bodies go out only when this is true AND the admin acknowledged that data leaves the tenant.
    pub include_details: bool,
    pub max_per_hour: u32,
    pub link_base: String,
    // webhook
    pub url: String,
    pub signing_secret: String,
    // email
    pub host: String,
    pub port: u16,
    pub tls: TlsMode,
    pub from: String,
    pub username: String,
    pub password: String,
    /// person -> address
    pub addresses: BTreeMap<String, String>,
}

pub fn key(team: Option<&str>, name: &str) -> String {
    format!("{}/{name}", team.unwrap_or(""))
}

fn s<'a>(v: &'a BTreeMap<String, Value>, k: &str) -> &'a str {
    v.get(k).and_then(|x| x.as_str()).unwrap_or("")
}

/// A channel from its item; an item that is switched off or wrong is `Err` (and does nothing).
pub fn from_item(it: &Item) -> Result<Channel, String> {
    if !it.enabled {
        return Err("switched off".into());
    }
    let v = &it.values;
    let kind = match s(v, "type") {
        "webhook" => Kind::Webhook,
        "email" => Kind::Email,
        _ => return Err("type must be webhook or email".into()),
    };
    let mut addresses = BTreeMap::new();
    for e in v.get("addresses").and_then(|x| x.as_array()).into_iter().flatten().filter_map(|x| x.as_str()) {
        let (p, a) = e.split_once('=').ok_or("addresses are person=address")?;
        if !super::smtp::valid_address(a.trim()) {
            return Err("an address is not a plain address".into());
        }
        addresses.insert(p.trim().to_string(), a.trim().to_string());
    }
    let details = v.get("include_details") == Some(&Value::Bool(true)) && v.get("details_ack") == Some(&Value::Bool(true));
    let c = Channel {
        key: key(it.namespace.as_deref(), &it.name),
        team: it.namespace.clone(),
        kind,
        min_severity: Severity::parse(if s(v, "min_severity").is_empty() { "normal" } else { s(v, "min_severity") }).ok_or("unknown severity")?,
        include_details: details,
        max_per_hour: v.get("max_per_hour").and_then(|x| x.as_u64()).unwrap_or(60).clamp(1, 10_000) as u32,
        link_base: s(v, "link_base").trim_end_matches('/').to_string(),
        url: s(v, "url").into(),
        signing_secret: s(v, "signing_secret").into(),
        host: s(v, "host").to_ascii_lowercase(),
        port: v.get("port").and_then(|x| x.as_u64()).unwrap_or(587).min(65535) as u16,
        tls: if s(v, "tls") == "implicit" { TlsMode::Implicit } else { TlsMode::Starttls },
        from: s(v, "from").into(),
        username: s(v, "username").into(),
        password: s(v, "password").into(),
        addresses,
    };
    match kind {
        Kind::Webhook if super::webhook::https_host(&c.url).is_err() || c.signing_secret.is_empty() => Err("a webhook needs an https url and a signing secret".into()),
        Kind::Email if c.host.is_empty() || !super::smtp::valid_address(&c.from) => Err("an email channel needs a host and a plain From address".into()),
        _ => Ok(c),
    }
}

/// Every working channel of the tenant.
pub fn load(dir: &std::path::Path) -> Vec<Channel> {
    let Ok(cfg) = Store::new(dir).load() else { return vec![] };
    cfg.entries().iter().filter(|e| e.item.kind == "notify.channel").filter_map(|e| from_item(&e.item).ok()).collect()
}

fn hhmm(t: &str) -> Option<u32> {
    let (h, m) = t.split_once(':')?;
    let (h, m): (u32, u32) = (h.parse().ok()?, m.parse().ok()?);
    (h < 24 && m < 60).then_some(h * 60 + m)
}

pub fn rules_from_item(it: &Item) -> Rules {
    let v = &it.values;
    let mut r = Rules::default();
    if !it.enabled {
        return r;
    }
    if let (Some(a), Some(b)) = (hhmm(s(v, "quiet_start")), hhmm(s(v, "quiet_end"))) {
        r.quiet = Some(Quiet { start_min: a, end_min: b });
    }
    r.digest_min = hhmm(s(v, "digest_time"));
    r.offset_min = v.get("utc_offset_minutes").and_then(|x| x.as_i64()).unwrap_or(0).clamp(-840, 840) as i32;
    r.escalate_after_secs = v.get("escalate_after_minutes").and_then(|x| x.as_u64()).filter(|m| *m > 0).map(|m| m.min(10_080) * 60);
    r.escalate_min = Severity::parse(s(v, "escalate_min_severity")).unwrap_or(Severity::High);
    r
}

/// The routing of a team: its own item, else the tenant's, else the defaults (nothing held, nothing escalated).
pub fn load_rules(dir: &std::path::Path, team: Option<&str>) -> Rules {
    let Ok(cfg) = Store::new(dir).load() else { return Rules::default() };
    let es = cfg.entries();
    let pick = |ns: Option<&str>| es.iter().find(|e| e.item.kind == "notify.routing" && e.item.namespace.as_deref() == ns && e.item.enabled);
    pick(team).or_else(|| pick(None)).map(|e| rules_from_item(&e.item)).unwrap_or_default()
}
