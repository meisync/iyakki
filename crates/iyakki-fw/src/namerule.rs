//! The firewall rule of the stable device name: incoming UDP 5353 (mDNS) from the local networks, so other computers can ask the
//! device for `iyakki-<id>.local`. It exists only while "Announce this device on the local network" is on, is listed in the
//! firewall with the reason [`REASON_MDNS`], and can be removed like any rule (the watcher then switches the announcing off:
//! the person's removal is respected, it is not made again). Public addresses are never allowed by it.

use crate::engine::{Engine, Status};
use crate::model::{Config, Kind, Proto, Rule};
use crate::ruleset::{local_networks, Addr};
use crate::service::Handle;

pub const REASON_MDNS: &str = "device name (mDNS)";
const PORTS: &str = "5353";

pub fn is_mdns(r: &Rule) -> bool {
    r.comment == REASON_MDNS
}

/// The rule list with the mDNS rules as wanted (`on`) and what changed, as `(audit action, detail)`. Rules of the person stay untouched.
pub fn plan(cfg: &Config, on: bool) -> (Config, Vec<(String, String)>) {
    let mut out = cfg.clone();
    let mut changes = Vec::new();
    let wanted: Vec<String> = if on { local_networks(false).into_iter().filter_map(|a| if let Addr::Cidr(ip, l) = a { Some(format!("{ip}/{l}")) } else { None }).collect() } else { vec![] };
    let mut kept: Vec<u32> = Vec::new();
    for v in &wanted {
        if let Some(r) = out.rules.iter_mut().find(|r| is_mdns(r) && r.value == *v && r.proto == Proto::Udp && r.ports == PORTS) {
            r.enabled = true;
            kept.push(r.id);
        } else {
            let mut r = Rule::new(Kind::Cidr, v);
            r.proto = Proto::Udp;
            r.ports = PORTS.into();
            r.comment = REASON_MDNS.into();
            kept.push(out.add(r));
            changes.push(("devname.firewall.allow".into(), format!("UDP {PORTS} from {v} ({REASON_MDNS})")));
        }
    }
    for r in out.rules.iter().filter(|r| is_mdns(r) && !kept.contains(&r.id)) {
        changes.push(("devname.firewall.removed".into(), format!("UDP {} from {} ({REASON_MDNS})", r.ports, r.value)));
    }
    out.rules.retain(|r| !is_mdns(r) || kept.contains(&r.id));
    (out, changes)
}

/// Is the rule (any part of it) in the list now?
pub fn present(e: &Engine) -> bool {
    e.config().rules.iter().any(|r| is_mdns(r) && r.enabled)
}

/// Apply [`plan`] to the engine without the 60-second question (one UDP port, private networks only: it cannot cut anybody off).
/// Nothing is done while the person has a change waiting for Keep.
pub fn sync(h: &Handle, on: bool, who: &str) -> Result<Vec<(String, String)>, String> {
    let mut e = h.lock();
    if matches!(e.status(), Status::Pending { .. }) {
        return Ok(vec![]);
    }
    let (new, changes) = plan(e.config(), on);
    if new == *e.config() {
        return Ok(vec![]);
    }
    e.propose(new, who)?;
    let _ = e.keep(who);
    Ok(changes)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_rule_exists_only_while_on_allows_private_networks_for_udp_5353_and_leaves_other_rules_alone() {
        let mut cfg = Config::default();
        let mut mine = Rule::new(Kind::Ip, "192.168.1.5");
        mine.comment = "mine".into();
        cfg.add(mine.clone());
        let (on, ch) = plan(&cfg, true);
        let rules: Vec<&Rule> = on.rules.iter().filter(|r| is_mdns(r)).collect();
        assert_eq!(rules.len(), 4, "10/8, 172.16/12, 192.168/16, 169.254/16");
        assert!(rules.iter().all(|r| r.proto == Proto::Udp && r.ports == "5353" && r.kind == Kind::Cidr && r.expires == 0));
        assert!(!rules.iter().any(|r| r.value == "0.0.0.0/0" || r.value.contains("::")), "never a public address");
        assert_eq!(ch.len(), 4);
        assert!(on.rules.iter().any(|r| r.comment == "mine"));
        // steady: nothing changes
        assert!(plan(&on, true).1.is_empty() && plan(&on, true).0 == on);
        // off: gone, and the person's rule stays
        let (off, ch) = plan(&on, false);
        assert!(!off.rules.iter().any(is_mdns) && off.rules.iter().any(|r| r.comment == "mine"));
        assert!(ch.len() == 4 && ch.iter().all(|c| c.0 == "devname.firewall.removed"));
    }
}
