//! The firewall rules of developer mode, with the fake firewall backend: added when developer mode is on, narrow after pairing,
//! one-time pairing rule consumed, private addresses only, gone when it is off or its time is up, the hint.

use std::net::IpAddr;
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::{Arc, Mutex};

use crate::backend::FakeBackend;
use crate::blocked::Blocked;
use crate::devrules::*;
use crate::engine::{Clock, Engine, Resolver, Saved, Store};
use crate::model::Config;
use crate::service::Handle;

struct Clk(Arc<AtomicI64>);
impl Clock for Clk {
    fn now(&self) -> i64 {
        self.0.load(Ordering::SeqCst)
    }
}
struct NoDns;
impl Resolver for NoDns {
    fn resolve(&self, _: &str) -> Result<Vec<IpAddr>, String> {
        Err("none".into())
    }
}
#[derive(Default, Clone)]
struct Mem(Arc<Mutex<Option<Saved>>>);
impl Store for Mem {
    fn load(&self) -> Result<Option<Saved>, String> {
        Ok(self.0.lock().unwrap().clone())
    }
    fn save(&self, s: &Saved) -> Result<(), String> {
        *self.0.lock().unwrap() = Some(s.clone());
        Ok(())
    }
}

const T0: i64 = 2_000_000;

fn rig() -> (Handle, Arc<AtomicI64>, Mem) {
    let t = Arc::new(AtomicI64::new(T0));
    let mem = Mem::default();
    let mut e = Engine::new(Box::new(FakeBackend::default()), Box::new(Clk(t.clone())), Box::new(NoDns), Box::new(mem.clone()));
    e.boot(false);
    (Handle::Own(Arc::new(Mutex::new(e))), t, mem)
}

fn ip(s: &str) -> IpAddr {
    s.parse().unwrap()
}

fn allows(h: &Handle, src: &str, port: u16) -> bool {
    h.lock().would_allow_tcp(ip(src), port)
}

fn managed(h: &Handle) -> Vec<(String, String, i64)> {
    h.lock().config().rules.iter().filter(|r| is_managed(r)).map(|r| (r.value.clone(), r.comment.clone(), r.expires)).collect()
}

fn want() -> Want {
    Want { dev_on: true, until: 0, port: 7979, ..Default::default() }
}

#[test]
fn nothing_is_added_without_a_paired_address_or_a_pairing_request_and_the_port_stays_closed() {
    let (h, _t, _m) = rig();
    assert_eq!(sync(&h, &want(), "devmode").unwrap(), vec![]);
    assert!(managed(&h).is_empty() && !allows(&h, "192.168.50.20", 7979));
    let hint = hint(&h.lock(), &[], 7979, true).expect("hint");
    assert_eq!(hint, "the firewall blocks the developer port 7979: Allow");
}

#[test]
fn a_paired_computer_gets_a_narrow_rule_valid_while_developer_mode_is_on_and_only_for_its_address() {
    let (h, t, _m) = rig();
    let w = Want { peers: vec![ip("192.168.50.20")], until: T0 + 3600, ..want() };
    let ch = sync(&h, &w, "devmode").unwrap();
    assert_eq!(ch.len(), 1);
    assert_eq!(ch[0].0, "devmode.firewall.allow");
    assert!(allows(&h, "192.168.50.20", 7979) && !allows(&h, "192.168.50.21", 7979) && !allows(&h, "192.168.50.20", 22) && !allows(&h, "8.8.8.8", 7979));
    assert_eq!(managed(&h), vec![("192.168.50.20".into(), REASON_DEV.into(), T0 + 3600)], "{:?}", managed(&h));
    // nothing changes when nothing changed
    assert_eq!(sync(&h, &w, "devmode").unwrap(), vec![]);
    // no hint now: a rule exists
    assert!(hint(&h.lock(), &[], 7979, true).is_none());
    // its time is up (the engine also stops applying an expired rule)
    t.fetch_add(3601, Ordering::SeqCst);
    let ch = sync(&h, &w, "devmode").unwrap();
    assert!(ch.iter().all(|c| c.0 == "devmode.firewall.removed") && managed(&h).is_empty(), "{ch:?}");
}

#[test]
fn switching_developer_mode_off_removes_every_rule_it_made_and_leaves_the_persons_own() {
    let (h, _t, mem) = rig();
    {
        let mut e = h.lock();
        let mut c = e.config().clone();
        let mut mine = crate::model::Rule::new(crate::model::Kind::Ip, "192.168.50.99");
        mine.comment = "my laptop".into();
        c.add(mine);
        e.propose(c, "me").unwrap();
        e.keep("me").unwrap();
    }
    let w = Want { peers: vec![ip("192.168.50.20")], pair_until: T0 + 600, ..want() };
    sync(&h, &w, "devmode").unwrap();
    assert!(managed(&h).len() > 1);
    let off = Want { dev_on: false, ..w };
    let ch = sync(&h, &off, "devmode").unwrap();
    assert!(!ch.is_empty() && ch.iter().all(|c| c.0 == "devmode.firewall.removed"));
    assert!(managed(&h).is_empty());
    assert!(allows(&h, "192.168.50.99", 22) || h.lock().config().rules.iter().any(|r| r.comment == "my laptop"), "the person's rule stays");
    // and the saved copy does not hold them either
    let saved = mem.0.lock().unwrap().clone().unwrap();
    assert!(!saved.config.rules.iter().any(is_managed));
}

#[test]
fn the_one_time_pairing_rule_opens_the_developer_port_to_the_local_networks_only_and_is_consumed_by_the_first_pairing() {
    let (h, _t, _m) = rig();
    let w = Want { pair_until: T0 + PAIR_SECS, ..want() };
    sync(&h, &w, "devmode").unwrap();
    for ok in ["192.168.50.20", "10.1.2.3", "172.16.5.5", "169.254.9.9", "fe80::1", "fd00::5"] {
        assert!(allows(&h, ok, 7979), "{ok}");
    }
    for no in ["8.8.8.8", "172.32.0.1", "100.64.0.1", "2001:db8::1"] {
        assert!(!allows(&h, no, 7979), "{no}");
    }
    assert!(!allows(&h, "192.168.50.20", 22) && !allows(&h, "192.168.50.20", 7980), "only the developer port");
    assert!(managed(&h).iter().all(|m| m.1 == REASON_PAIR && m.2 == T0 + PAIR_SECS));
    // the first pairing: the request is consumed, the computer's own address takes over
    let after = Want { pair_until: 0, peers: vec![ip("192.168.50.20")], ..want() };
    let ch = sync(&h, &after, "devmode").unwrap();
    assert!(ch.iter().any(|c| c.0 == "devmode.firewall.removed") && ch.iter().any(|c| c.0 == "devmode.firewall.allow"));
    assert!(allows(&h, "192.168.50.20", 7979) && !allows(&h, "192.168.50.21", 7979) && !allows(&h, "10.1.2.3", 7979));
    // …and without a pairing it also ends by itself after ten minutes
    let (h2, t2, _m) = rig();
    sync(&h2, &w, "devmode").unwrap();
    t2.fetch_add(PAIR_SECS + 1, Ordering::SeqCst);
    assert!(!allows(&h2, "192.168.50.20", 7979), "expired rules do not let anything in");
    sync(&h2, &w, "devmode").unwrap();
    assert!(managed(&h2).is_empty(), "removed from the list too");
}

#[test]
fn public_addresses_are_never_allowed_for_a_peer() {
    let (h, _t, _m) = rig();
    let w = Want { peers: vec![ip("8.8.8.8"), ip("2001:db8::1"), ip("192.168.1.5"), ip("::ffff:192.168.1.6")], ..want() };
    sync(&h, &w, "devmode").unwrap();
    let m = managed(&h);
    assert_eq!(m.iter().map(|x| x.0.as_str()).collect::<Vec<_>>(), vec!["192.168.1.5", "192.168.1.6"], "{m:?}");
    assert!(!is_local(ip("8.8.8.8")) && is_local(ip("192.168.1.5")) && is_local(ip("fe80::1")));
}

#[test]
fn the_opt_in_allows_the_whole_network_of_the_paired_computer_and_default_is_the_narrow_rule() {
    let (h, _t, _m) = rig();
    let narrow = Want { peers: vec![ip("192.168.50.20")], ..want() };
    sync(&h, &narrow, "devmode").unwrap();
    assert!(!allows(&h, "192.168.50.77", 7979));
    let wide = Want { network_wide: true, ..narrow.clone() };
    sync(&h, &wide, "devmode").unwrap();
    assert!(allows(&h, "192.168.50.77", 7979) && !allows(&h, "192.168.69.77", 7979));
    assert_eq!(managed(&h).len(), 1);
    assert!(managed(&h)[0].1.starts_with(REASON_DEV) && managed(&h)[0].0 == "192.168.50.0/24");
    sync(&h, &narrow, "devmode").unwrap();
    assert!(!allows(&h, "192.168.50.77", 7979) && allows(&h, "192.168.50.20", 7979));
}

#[test]
fn a_restart_starts_from_the_saved_rules_and_the_next_sync_with_a_fresh_want_removes_the_pairing_rule() {
    let (h, _t, mem) = rig();
    let w = Want { pair_until: T0 + PAIR_SECS, peers: vec![ip("192.168.50.20")], ..want() };
    sync(&h, &w, "devmode").unwrap();
    // a new process: the saved settings are loaded, the pairing request is gone with the old process
    let t = Arc::new(AtomicI64::new(T0 + 5));
    let mut e = Engine::new(Box::new(FakeBackend::default()), Box::new(Clk(t)), Box::new(NoDns), Box::new(mem.clone()));
    e.boot(false);
    let h2 = Handle::Own(Arc::new(Mutex::new(e)));
    assert!(managed(&h2).len() > 1, "saved with the settings");
    let fresh = Want { pair_until: 0, peers: vec![ip("192.168.50.20")], ..want() };
    sync(&h2, &fresh, "devmode").unwrap();
    assert_eq!(managed(&h2).len(), 1);
    // and with developer mode off after the restart, nothing is left
    sync(&h2, &Want { dev_on: false, ..fresh }, "devmode").unwrap();
    assert!(managed(&h2).is_empty());
}

#[test]
fn the_hint_names_the_blocked_computer_and_is_silent_when_the_firewall_is_off_or_developer_mode_is_off() {
    let (h, _t, _m) = rig();
    let b = Blocked { at: T0, src: ip("192.168.50.20"), proto: 6, dport: 7979, count: 3 };
    let other = Blocked { at: T0, src: ip("192.168.50.21"), proto: 6, dport: 22, count: 1 };
    assert_eq!(hint(&h.lock(), &[other, b.clone()], 7979, true).unwrap(), "the firewall blocks the developer port 7979 from 192.168.50.20: Allow");
    assert!(hint(&h.lock(), &[b.clone()], 7979, false).is_none(), "developer mode off");
    {
        let mut e = h.lock();
        let mut c: Config = e.config().clone();
        c.enabled = false;
        e.propose(c, "me").unwrap();
    }
    assert!(hint(&h.lock(), &[b], 7979, true).is_none(), "firewall off");
}

#[test]
fn a_change_waiting_for_keep_is_left_alone() {
    let (h, _t, _m) = rig();
    {
        let mut e = h.lock();
        let mut c = e.config().clone();
        c.ping = crate::model::Ping::Off;
        e.propose(c, "me").unwrap();
    }
    let w = Want { peers: vec![ip("192.168.50.20")], ..want() };
    assert_eq!(sync(&h, &w, "devmode").unwrap(), vec![]);
    assert!(managed(&h).is_empty());
}
