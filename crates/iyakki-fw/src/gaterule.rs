//! The firewall rules of the domain listener (`iyakki-agent`, `cloudlisten.rs`): one allow rule per listening TCP port that has a
//! route, made when the port gains its first route and removed when it loses the last one. They are listed with the reason
//! [`REASON_GATE`] and can be removed like any rule (the listener's next change makes them again; the person's own rules are never touched).
//!
//! By default a port is open to **the local network** only; [`Reach::Anywhere`] (public sites) opens it to every address, which the caller
//! must ask for. Like `namerule`, [`plan`] is pure and [`render`] shows what the kernel would get without applying anything.

use std::collections::BTreeSet;

use crate::engine::{Engine, Status};
use crate::model::{Config, Kind, Proto, Resolved, Rule};
use crate::ruleset::{build, local_networks, Addr, Ctx};
use crate::service::Handle;

pub const REASON_GATE: &str = "domain listener (closes with its last route)";

/// Who may reach the opened ports.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Reach {
    /// Private and link-local networks only.
    LocalNetwork,
    /// Every address. Only for a listener that serves public routes.
    Anywhere,
}

pub fn is_gate(r: &Rule) -> bool {
    r.comment == REASON_GATE
}

fn sources(reach: Reach, ipv6: bool) -> Vec<String> {
    match reach {
        Reach::Anywhere => {
            let mut v = vec!["0.0.0.0/0".to_string()];
            if ipv6 {
                v.push("::/0".into());
            }
            v
        }
        Reach::LocalNetwork => local_networks(ipv6).into_iter().filter_map(|a| if let Addr::Cidr(ip, l) = a { Some(format!("{ip}/{l}")) } else { None }).collect(),
    }
}

/// The rule list with the gate rules as wanted for `ports` (TCP), and what changed as `(audit action, detail)`. Ports of 0 are ignored.
/// Rules of the person stay untouched.
pub fn plan(cfg: &Config, ports: &BTreeSet<u16>, reach: Reach, ipv6: bool) -> (Config, Vec<(String, String)>) {
    plan_until(cfg, ports, reach, ipv6, 0)
}

/// [`plan`] with the rules ending at `expires` (Unix seconds; 0: they stay until the listener closes the port). The firewall engine
/// stops enforcing a rule whose time has passed.
pub fn plan_until(cfg: &Config, ports: &BTreeSet<u16>, reach: Reach, ipv6: bool, expires: i64) -> (Config, Vec<(String, String)>) {
    let mut out = cfg.clone();
    let mut changes = Vec::new();
    let mut kept: Vec<u32> = Vec::new();
    for p in ports.iter().filter(|p| **p != 0) {
        let ps = p.to_string();
        for v in sources(reach, ipv6) {
            if let Some(r) = out.rules.iter_mut().find(|r| is_gate(r) && r.value == v && r.proto == Proto::Tcp && r.ports == ps) {
                r.enabled = true;
                if r.expires != expires {
                    r.expires = expires;
                    changes.push(("cloudlisten.firewall.expiry".into(), format!("TCP {ps} from {v}: {}", if expires == 0 { "no end time".to_string() } else { format!("until {expires}") })));
                }
                kept.push(r.id);
            } else {
                let mut r = Rule::new(Kind::Cidr, &v);
                r.proto = Proto::Tcp;
                r.ports = ps.clone();
                r.comment = REASON_GATE.into();
                r.expires = expires;
                kept.push(out.add(r));
                changes.push(("cloudlisten.firewall.allow".into(), format!("TCP {ps} from {v} ({REASON_GATE})")));
            }
        }
    }
    for r in out.rules.iter().filter(|r| is_gate(r) && !kept.contains(&r.id)) {
        changes.push(("cloudlisten.firewall.removed".into(), format!("TCP {} from {} ({REASON_GATE})", r.ports, r.value)));
    }
    out.rules.retain(|r| !is_gate(r) || kept.contains(&r.id));
    (out, changes)
}

/// Dry run: the whole ruleset in nft-like words that `base` plus the gate rules would give. Nothing is applied. `Err` when the rules would
/// be refused (so the caller knows before it asks the kernel).
pub fn render(base: &Config, ports: &BTreeSet<u16>, reach: Reach, ctx: &Ctx) -> Result<String, String> {
    let (cfg, _) = plan(base, ports, reach, ctx.ipv6);
    for r in &cfg.rules {
        crate::validate::check_rule(r).map_err(|e| format!("rule {} ({}): {e}", r.id, r.value))?;
    }
    build(&cfg, &Resolved::default(), ctx).map(|b| b.ruleset.to_text())
}

/// Apply [`plan`] to the engine without the 60-second question: it only opens TCP ports that a route the person configured needs, and
/// it can never cut anybody off. Nothing is done while the person has a change waiting for Keep (`Err` says so, the caller can retry).
pub fn sync(h: &Handle, ports: &BTreeSet<u16>, reach: Reach, who: &str) -> Result<Vec<(String, String)>, String> {
    sync_until(h, ports, reach, who, 0)
}

/// [`sync`] with the opening ending at `expires` (Unix seconds; 0: no end time).
pub fn sync_until(h: &Handle, ports: &BTreeSet<u16>, reach: Reach, who: &str, expires: i64) -> Result<Vec<(String, String)>, String> {
    let mut e = h.lock();
    if matches!(e.status(), Status::Pending { .. }) {
        return Err("a firewall change is waiting to be kept or undone; try again after that".into());
    }
    let (new, changes) = plan_until(e.config(), ports, reach, e.ctx().ipv6, expires);
    if new == *e.config() {
        // nothing to change, but opening ports on a firewall that does not hold is still refused
        if !ports.is_empty() {
            enforcing(&e)?;
        }
        return Ok(vec![]);
    }
    e.propose(new, who)?;
    // with the firewall off nothing waits for Keep; the check below then says why the port is not gated
    if matches!(e.status(), Status::Pending { .. }) {
        e.keep(who)?;
    }
    if !ports.is_empty() {
        enforcing(&e)?;
    }
    Ok(changes)
}

/// Is the firewall really holding (rules in the kernel)? The domain listener opens a port to the network only when it is: with the
/// firewall off, not protecting or unavailable, an "allow rule" would mean nothing and the port would be open to everybody (fail closed).
pub fn enforcing(e: &Engine) -> Result<(), String> {
    match e.status() {
        Status::On => Ok(()),
        Status::Pending { .. } => Err("a firewall change is waiting to be kept or undone".into()),
        Status::Off | Status::OffThisBoot => Err("the firewall is off, so the port cannot be gated".into()),
        Status::NotProtected(why) => Err(format!("the firewall rules are not in force ({why})")),
        Status::Unavailable(why) => Err(format!("the firewall is not available ({why})")),
    }
}

/// Is any gate rule in the list now?
pub fn present(e: &Engine) -> bool {
    e.config().rules.iter().any(|r| is_gate(r) && r.enabled)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ruleset::{eval, Packet};
    use std::net::IpAddr;

    fn ports(p: &[u16]) -> BTreeSet<u16> {
        p.iter().copied().collect()
    }
    fn open(text_rules: &Config, ip: &str, port: u16) -> bool {
        let b = build(text_rules, &Resolved::default(), &Ctx::default()).unwrap();
        eval(&b.ruleset, &Packet::new_tcp("eth0", ip.parse::<IpAddr>().unwrap(), port))
    }

    #[test]
    fn a_port_is_open_to_the_local_network_only_while_it_has_a_route() {
        let mut cfg = Config::default();
        let mut mine = Rule::new(Kind::Ip, "192.168.1.5");
        mine.comment = "mine".into();
        cfg.add(mine);
        assert!(!open(&cfg, "192.168.1.9", 9443));
        let (on, ch) = plan(&cfg, &ports(&[9443]), Reach::LocalNetwork, true);
        assert!(open(&on, "192.168.1.9", 9443) && open(&on, "10.1.2.3", 9443));
        assert!(!open(&on, "203.0.113.7", 9443), "a public address is refused");
        assert!(!open(&on, "192.168.1.9", 9444), "another port stays closed");
        assert!(ch.iter().all(|c| c.0 == "cloudlisten.firewall.allow") && ch.len() == 6);
        assert!(on.rules.iter().any(|r| r.comment == "mine"));
        // steady
        let (again, ch) = plan(&on, &ports(&[9443]), Reach::LocalNetwork, true);
        assert!(ch.is_empty() && again == on);
        // the last route goes: the rules go, the person's stay
        let (off, ch) = plan(&on, &ports(&[]), Reach::LocalNetwork, true);
        assert!(!off.rules.iter().any(is_gate) && off.rules.iter().any(|r| r.comment == "mine"));
        assert!(!open(&off, "192.168.1.9", 9443));
        assert!(ch.len() == 6 && ch.iter().all(|c| c.0 == "cloudlisten.firewall.removed"));
    }

    #[test]
    fn an_opening_can_end_at_a_time_and_the_end_time_is_updated_in_place() {
        let (on, _) = plan_until(&Config::default(), &ports(&[8080]), Reach::LocalNetwork, true, 1_900_000_000);
        assert!(on.rules.iter().filter(|r| is_gate(r)).all(|r| r.expires == 1_900_000_000));
        let (same, ch) = plan_until(&on, &ports(&[8080]), Reach::LocalNetwork, true, 1_900_000_000);
        assert!(ch.is_empty() && same == on);
        let (longer, ch) = plan_until(&on, &ports(&[8080]), Reach::LocalNetwork, true, 0);
        assert!(longer.rules.iter().filter(|r| is_gate(r)).all(|r| r.expires == 0) && ch.iter().all(|c| c.0 == "cloudlisten.firewall.expiry"), "{ch:?}");
        assert_eq!(plan(&Config::default(), &ports(&[8080]), Reach::LocalNetwork, true).0.rules.iter().map(|r| r.expires).max(), Some(0), "plan keeps its old meaning");
    }

    #[test]
    fn anywhere_opens_every_address_but_only_for_the_listed_tcp_ports_and_port_zero_is_ignored() {
        let (on, _) = plan(&Config::default(), &ports(&[0, 8080]), Reach::Anywhere, true);
        assert!(open(&on, "203.0.113.7", 8080) && open(&on, "2001:db8::1", 8080));
        assert!(!open(&on, "203.0.113.7", 8081) && !open(&on, "203.0.113.7", 0));
        assert!(on.rules.iter().filter(|r| is_gate(r)).all(|r| r.proto == Proto::Tcp && r.ports == "8080"));
        let (v4, _) = plan(&Config::default(), &ports(&[8080]), Reach::Anywhere, false);
        assert_eq!(v4.rules.len(), 1);
        // narrowing the reach replaces the rules
        let (narrow, ch) = plan(&on, &ports(&[8080]), Reach::LocalNetwork, true);
        assert!(!open(&narrow, "203.0.113.7", 8080) && open(&narrow, "10.0.0.1", 8080));
        assert!(ch.iter().any(|c| c.0 == "cloudlisten.firewall.removed"));
    }

    #[test]
    fn the_dry_run_shows_the_rules_and_changes_nothing() {
        let base = Config::default();
        let text = render(&base, &ports(&[9443]), Reach::LocalNetwork, &Ctx::default()).unwrap();
        assert!(text.contains("th dport 9443") && text.contains("domain listener"), "{text}");
        assert!(text.contains("policy drop"), "{text}");
        assert!(!render(&base, &ports(&[]), Reach::LocalNetwork, &Ctx::default()).unwrap().contains("domain listener"));
        assert!(base.rules.is_empty(), "the caller's settings are not changed");
    }

    #[test]
    fn a_rule_the_firewall_would_refuse_is_reported_by_the_dry_run() {
        let mut base = Config::default();
        let mut bad = Rule::new(Kind::Ip, "not-an-address");
        bad.comment = "bad".into();
        base.add(bad);
        assert!(render(&base, &ports(&[9443]), Reach::LocalNetwork, &Ctx::default()).is_err());
    }

    mod engine_tests {
        use super::*;
        use crate::backend::{BackendError, FakeBackend};
        use crate::engine::{Clock, Resolver, Saved, Store};
        use std::sync::{Arc, Mutex};

        struct Clk;
        impl Clock for Clk {
            fn now(&self) -> i64 {
                2_000_000
            }
        }
        struct NoDns;
        impl Resolver for NoDns {
            fn resolve(&self, _: &str) -> Result<Vec<IpAddr>, String> {
                Err("none".into())
            }
        }
        struct Mem;
        impl Store for Mem {
            fn load(&self) -> Result<Option<Saved>, String> {
                Ok(None)
            }
            fn save(&self, _: &Saved) -> Result<(), String> {
                Ok(())
            }
        }

        fn rig(boot_off: bool) -> (Handle, FakeBackend) {
            let be = FakeBackend::default();
            let mut e = Engine::new(Box::new(be.clone()), Box::new(Clk), Box::new(NoDns), Box::new(Mem));
            e.boot(boot_off);
            (Handle::Own(Arc::new(Mutex::new(e))), be)
        }

        #[test]
        fn opening_a_port_on_a_working_firewall_succeeds_and_is_idempotent() {
            let (h, _) = rig(false);
            assert!(sync(&h, &ports(&[9443]), Reach::LocalNetwork, "t").is_ok());
            assert!(sync(&h, &ports(&[9443]), Reach::LocalNetwork, "t").is_ok(), "steady state");
            assert!(present(&h.lock()));
            assert!(sync(&h, &ports(&[]), Reach::LocalNetwork, "t").is_ok());
        }

        #[test]
        fn a_firewall_that_is_off_or_cannot_apply_refuses_to_open_a_port() {
            let (h, _) = rig(false);
            {
                // the administrator switched the firewall off
                let mut e = h.lock();
                let mut off = e.config().clone();
                off.enabled = false;
                e.propose(off, "t").unwrap();
                let _ = e.keep("t");
            }
            let e = sync(&h, &ports(&[9443]), Reach::LocalNetwork, "t").unwrap_err();
            assert!(e.contains("off") || e.contains("not in force") || e.contains("not available"), "{e}");
            let (h, be) = rig(false);
            be.0.lock().unwrap().fail_replace = Some(BackendError::Failed("kernel said no".into()));
            assert!(sync(&h, &ports(&[9443]), Reach::LocalNetwork, "t").is_err());
            assert!(!present(&h.lock()), "nothing half-applied");
        }
    }
}
