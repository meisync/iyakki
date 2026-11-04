//! The firewall rule of "Serve this desktop to browsers": incoming TCP on the web port from the local networks, so a browser on the
//! same network can reach the desktop when the firewall blocks incoming connections. It exists only while the switch is on, is listed
//! in the firewall with the reason [`REASON_WEB`], follows a port change, and can be removed like any rule (the watcher then
//! switches the service off: the person's removal is respected, it is not made again). Public addresses are never allowed by it.

use crate::engine::{Engine, Status};
use crate::model::{Config, Kind, Proto, Rule};
use crate::ruleset::{local_networks, Addr};
use crate::service::Handle;

pub const REASON_WEB: &str = "remote access web (switches off with it)";

pub fn is_web(r: &Rule) -> bool {
    r.comment == REASON_WEB
}

/// The rule list with the web rules as wanted (`port`: the listening port, `None`: off) and what changed, as `(audit action, detail)`.
/// Rules of the person stay untouched.
pub fn plan(cfg: &Config, port: Option<u16>, ipv6: bool) -> (Config, Vec<(String, String)>) {
    let mut out = cfg.clone();
    let mut changes = Vec::new();
    let ports = port.map(|p| p.to_string()).unwrap_or_default();
    let wanted: Vec<String> = if port.is_some() { local_networks(ipv6).into_iter().filter_map(|a| if let Addr::Cidr(ip, l) = a { Some(format!("{ip}/{l}")) } else { None }).collect() } else { vec![] };
    let mut kept: Vec<u32> = Vec::new();
    for v in &wanted {
        if let Some(r) = out.rules.iter_mut().find(|r| is_web(r) && r.value == *v && r.proto == Proto::Tcp && r.ports == ports) {
            r.enabled = true;
            kept.push(r.id);
        } else {
            let mut r = Rule::new(Kind::Cidr, v);
            r.proto = Proto::Tcp;
            r.ports = ports.clone();
            r.comment = REASON_WEB.into();
            kept.push(out.add(r));
            changes.push(("webaccess.firewall.allow".into(), format!("TCP {ports} from {v} ({REASON_WEB})")));
        }
    }
    for r in out.rules.iter().filter(|r| is_web(r) && !kept.contains(&r.id)) {
        changes.push(("webaccess.firewall.removed".into(), format!("TCP {} from {} ({REASON_WEB})", r.ports, r.value)));
    }
    out.rules.retain(|r| !is_web(r) || kept.contains(&r.id));
    (out, changes)
}

/// Is the rule (any part of it) in the list now?
pub fn present(e: &Engine) -> bool {
    e.config().rules.iter().any(|r| is_web(r) && r.enabled)
}

/// Apply [`plan`] to the engine without the 60-second question (one TCP port, private networks only: it cannot cut anybody off).
/// Nothing is done while the person has a change waiting for Keep.
pub fn sync(h: &Handle, port: Option<u16>, who: &str) -> Result<Vec<(String, String)>, String> {
    let mut e = h.lock();
    if matches!(e.status(), Status::Pending { .. }) {
        return Ok(vec![]);
    }
    let (new, changes) = plan(e.config(), port, e.ctx().ipv6);
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
    fn the_rule_exists_only_while_on_follows_the_port_and_leaves_other_rules_alone() {
        let mut cfg = Config::default();
        let mut mine = Rule::new(Kind::Ip, "192.168.1.5");
        mine.comment = "mine".into();
        cfg.add(mine);
        let (on, ch) = plan(&cfg, Some(8443), false);
        let rules: Vec<&Rule> = on.rules.iter().filter(|r| is_web(r)).collect();
        assert_eq!(rules.len(), 4, "10/8, 172.16/12, 192.168/16, 169.254/16");
        assert!(rules.iter().all(|r| r.proto == Proto::Tcp && r.ports == "8443" && r.kind == Kind::Cidr && r.expires == 0));
        assert!(!rules.iter().any(|r| r.value == "0.0.0.0/0" || r.value.contains("::")), "never a public address");
        assert_eq!(ch.len(), 4);
        // steady: nothing changes
        assert!(plan(&on, Some(8443), false).1.is_empty() && plan(&on, Some(8443), false).0 == on);
        // a new port: the old rules go, new ones come
        let (moved, ch) = plan(&on, Some(9443), false);
        assert!(moved.rules.iter().filter(|r| is_web(r)).all(|r| r.ports == "9443") && ch.iter().filter(|c| c.0.ends_with("removed")).count() == 4 && ch.iter().filter(|c| c.0.ends_with("allow")).count() == 4);
        // off: gone, and the person's rule stays
        let (off, ch) = plan(&on, None, false);
        assert!(!off.rules.iter().any(is_web) && off.rules.iter().any(|r| r.comment == "mine"));
        assert!(ch.len() == 4 && ch.iter().all(|c| c.0 == "webaccess.firewall.removed"));
    }
}
