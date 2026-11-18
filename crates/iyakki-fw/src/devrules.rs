//! The allow rules developer mode manages by itself, so a firewall that blocks incoming connections does not cut off the
//! developer computer: [`plan`] (pure) turns "what developer mode needs" into the rule list, [`sync`] applies it to an engine
//! without the 60-second confirmation (these rules only ever allow one TCP port from private addresses, so they cannot cut anybody
//! off), [`hint`] says on the Developer mode page when the firewall blocks the developer port and nothing allows it.
//!
//! * `developer mode (switches off with it)`: TCP on the developer port from the address of each paired computer (the address it
//!   last connected from), valid while developer mode is on; optionally from the whole /24 of that address (opt-in).
//! * `developer mode pairing (one time)`: TCP on the developer port from the local networks ([`local_networks`]), for a pairing
//!   that has just been opened; valid 10 minutes at most and gone as soon as a computer has paired.
//!
//! Public addresses are never allowed by any of these rules. The rules are removed when developer mode is off.

use std::net::IpAddr;

use crate::blocked::Blocked;
use crate::engine::{Engine, Status};
use crate::model::{Config, Kind, Proto, Rule};
use crate::ruleset::{in_addr, local_networks, Addr};
use crate::service::Handle;

pub const REASON_DEV: &str = "developer mode (switches off with it)";
pub const REASON_NET: &str = "developer mode (switches off with it): paired computers on this network";
pub const REASON_PAIR: &str = "developer mode pairing (one time)";
/// A rule of developer mode that has no switch-off time lives this long at a time (it is renewed while developer mode is on).
pub const DAY: i64 = 86_400;
/// How long "Allow a computer to pair" lasts at most.
pub const PAIR_SECS: i64 = 600;

/// Is this one of the rules developer mode manages (the Firewall page explains it and offers Remove)?
pub fn is_managed(r: &Rule) -> bool {
    r.comment == REASON_PAIR || r.comment.starts_with(REASON_DEV)
}

/// A private, link-local or unique-local address (what "the local network" means in the firewall).
pub fn is_local(ip: IpAddr) -> bool {
    let ip = ip.to_canonical();
    local_networks(true).iter().any(|a| in_addr(a, ip))
}

/// What developer mode needs from the firewall right now.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Want {
    pub dev_on: bool,
    /// The automatic switch-off (0: none).
    pub until: i64,
    pub port: u16,
    /// Addresses of paired computers (and addresses the person chose to allow).
    pub peers: Vec<IpAddr>,
    /// "Allow a computer to pair" is valid until this time (0: not asked).
    pub pair_until: i64,
    /// Opt-in: the whole /24 of each peer instead of the single address.
    pub network_wide: bool,
}

struct Desired {
    kind: Kind,
    value: String,
    comment: &'static str,
    expires: i64,
}

fn desired(w: &Want, now: i64, ipv6: bool) -> Vec<Desired> {
    let mut v: Vec<Desired> = Vec::new();
    if !w.dev_on || (w.until > 0 && w.until <= now) {
        return v;
    }
    let exp = if w.until > 0 { w.until } else { now + DAY };
    for p in &w.peers {
        let p = p.to_canonical();
        if !is_local(p) {
            continue;
        }
        let d = match (w.network_wide, p) {
            (true, IpAddr::V4(a)) => {
                let o = a.octets();
                Desired { kind: Kind::Cidr, value: format!("{}.{}.{}.0/24", o[0], o[1], o[2]), comment: REASON_NET, expires: exp }
            }
            _ => Desired { kind: Kind::Ip, value: p.to_string(), comment: REASON_DEV, expires: exp },
        };
        if !v.iter().any(|x| x.value == d.value && x.comment == d.comment) {
            v.push(d);
        }
    }
    if w.pair_until > now {
        for n in local_networks(ipv6) {
            if let Addr::Cidr(a, l) = n {
                v.push(Desired { kind: Kind::Cidr, value: format!("{a}/{l}"), comment: REASON_PAIR, expires: w.pair_until });
            }
        }
    }
    v
}

/// The rule list developer mode wants, and what changed (`(audit action, detail)`, no secrets). Rules of the person stay untouched.
pub fn plan(cfg: &Config, w: &Want, now: i64, ipv6: bool) -> (Config, Vec<(String, String)>) {
    let ports = w.port.to_string();
    let mut out = cfg.clone();
    let mut kept: Vec<u32> = Vec::new();
    let mut changes: Vec<(String, String)> = Vec::new();
    for d in desired(w, now, ipv6) {
        let found = out.rules.iter_mut().find(|r| is_managed(r) && r.kind == d.kind && r.value == d.value && r.comment == d.comment && r.ports == ports && r.proto == Proto::Tcp);
        if let Some(r) = found {
            kept.push(r.id);
            r.enabled = true;
            // no switch-off time: the day is renewed when less than half of it is left; with one the rule ends exactly then
            let stale = if d.comment == REASON_PAIR || w.until > 0 { r.expires != d.expires } else { r.expires < now + DAY / 2 };
            if stale {
                r.expires = d.expires;
            }
        } else {
            let mut r = Rule::new(d.kind, &d.value);
            r.proto = Proto::Tcp;
            r.ports = ports.clone();
            r.comment = d.comment.to_string();
            r.expires = d.expires;
            kept.push(out.add(r));
            changes.push(("devmode.firewall.allow".into(), format!("TCP {} from {} ({}), until {}", w.port, d.value, d.comment, d.expires)));
        }
    }
    for r in out.rules.iter().filter(|r| is_managed(r) && !kept.contains(&r.id)) {
        changes.push(("devmode.firewall.removed".into(), format!("TCP {} from {} ({})", r.ports, r.value, r.comment)));
    }
    out.rules.retain(|r| !is_managed(r) || kept.contains(&r.id));
    (out, changes)
}

/// Apply [`plan`] to the engine. Nothing is done while the person has a change waiting for Keep. Returns the changes for the audit log.
pub fn sync(h: &Handle, w: &Want, who: &str) -> Result<Vec<(String, String)>, String> {
    let mut e = h.lock();
    if matches!(e.status(), Status::Pending { .. }) {
        return Ok(vec![]);
    }
    let (new, changes) = plan(e.config(), w, e.now(), e.ctx().ipv6);
    if new == *e.config() {
        return Ok(vec![]);
    }
    e.propose(new, who)?;
    // only allow rules for one port from private addresses (or their removal): nothing here can cut the person off
    let _ = e.keep(who);
    Ok(changes)
}

/// The Developer mode page's hint: the firewall is in force, developer mode is on, and nothing lets the developer port in.
pub fn hint(e: &Engine, blocked: &[Blocked], port: u16, dev_on: bool) -> Option<String> {
    if !dev_on || !e.config().enabled || !e.status().protected() {
        return None;
    }
    if let Some(b) = blocked.iter().rev().find(|b| b.proto == 6 && b.dport == port && !e.would_allow_tcp(b.src, port)) {
        return Some(format!("the firewall blocks the developer port {port} from {}: Allow", b.src));
    }
    let now = e.now();
    if e.config().rules.iter().any(|r| r.enabled && is_managed(r) && (r.expires == 0 || r.expires > now)) {
        return None;
    }
    let samples: [IpAddr; 3] = ["10.0.0.2".parse().unwrap(), "172.16.0.2".parse().unwrap(), "192.168.0.2".parse().unwrap()];
    if samples.iter().all(|s| !e.would_allow_tcp(*s, port)) {
        return Some(format!("the firewall blocks the developer port {port}: Allow"));
    }
    None
}
