//! What developer mode asks of the firewall (`DevMode::fw_want`): nothing while it is off, the addresses of paired computers (private
//! ones only) while it is on, the one-time pairing request only while the pairing is open, the opt-in, and nothing surviving "off".

use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::Arc;

use iyakki_sec::policy::Role;

use super::*;
use crate::store::install::tests::{fx, NOW};

fn clocked(tag: &str) -> (DevMode, Arc<AtomicI64>) {
    let f = fx(tag);
    let t = Arc::new(AtomicI64::new(NOW));
    let c = t.clone();
    (DevMode::open(&f.dir).with_clock(Arc::new(move || c.load(Ordering::SeqCst))), t)
}

fn key_file(dm: &DevMode, name: &str, key: &str) {
    let k = DevKey { name: name.into(), key: key.into(), account: "root".into(), added: NOW, added_by: "root".into(), last_used: 0, scopes: scopes::default_scopes() };
    let mut keys = dm.keys();
    keys.push(k);
    dm.write("keys.json", &keys).unwrap();
}

#[test]
fn private_addresses_only_and_only_for_keys_that_still_exist() {
    assert!(is_local_ip("192.168.50.20") && is_local_ip("10.0.0.1") && is_local_ip("172.16.1.1") && is_local_ip("169.254.1.1") && is_local_ip("fe80::1") && is_local_ip("fd12::1"));
    assert!(!is_local_ip("8.8.8.8") && !is_local_ip("172.32.0.1") && !is_local_ip("2001:db8::1") && !is_local_ip("not an address"));
    let (dm, _t) = clocked("dm-fw-addr");
    let adm = Role::Admin.template();
    dm.enable(&adm, "root", 0).unwrap();
    key_file(&dm, "laptop", &"ab".repeat(32));
    dm.note_key_addr(&"ab".repeat(32), "8.8.8.8");
    assert!(dm.fw_want().peers.is_empty(), "a public address is not remembered");
    dm.note_key_addr(&"ab".repeat(32), "192.168.50.20");
    dm.note_key_addr(&"cd".repeat(32), "192.168.50.21");
    assert_eq!(dm.fw_want().peers, vec!["192.168.50.20".to_string()], "the second key is not paired");
    // the address changed (DHCP): the next authenticated connection moves the rule
    dm.note_key_addr(&"ab".repeat(32), "192.168.50.44");
    assert_eq!(dm.fw_want().peers, vec!["192.168.50.44".to_string()]);
    assert!(dm.allow_peer(&adm, "root", "1.2.3.4").is_err());
    assert!(dm.allow_peer(&Role::User.template(), "bob", "192.168.1.2").is_err(), "administrators only");
    dm.allow_peer(&adm, "root", "192.168.50.50").unwrap();
    assert_eq!(dm.fw_want().peers.len(), 2);
}

#[test]
fn the_pairing_request_needs_an_open_pairing_lasts_ten_minutes_at_most_and_is_consumed_by_the_pairing() {
    let (dm, t) = clocked("dm-fw-pair");
    let adm = Role::Admin.template();
    assert!(dm.allow_pairing_through_firewall(&adm, "root").is_err(), "developer mode off");
    dm.enable(&adm, "root", 0).unwrap();
    assert!(dm.allow_pairing_through_firewall(&adm, "root").unwrap_err().contains("pairing"), "no code on the screen");
    dm.start_pairing(&adm, "root").unwrap();
    assert!(dm.allow_pairing_through_firewall(&Role::User.template(), "bob").is_err());
    let until = dm.allow_pairing_through_firewall(&adm, "root").unwrap();
    assert_eq!(until, NOW + FW_PAIR_SECS);
    assert_eq!(dm.fw_want().pair_until, until);
    // the pairing closes (completed, cancelled or out of time): the request is consumed
    dm.cancel_pairing();
    assert_eq!(dm.fw_want().pair_until, 0);
    dm.start_pairing(&adm, "root").unwrap();
    assert_eq!(dm.fw_want().pair_until, 0, "a new pairing needs a new request");
    dm.allow_pairing_through_firewall(&adm, "root").unwrap();
    t.store(NOW + PAIR_SECS + 1, Ordering::SeqCst);
    assert_eq!(dm.fw_want().pair_until, 0, "the code ran out");
}

#[test]
fn switching_off_or_restarting_clears_what_was_asked_and_the_opt_in_is_off_by_default() {
    let (dm, _t) = clocked("dm-fw-off");
    let adm = Role::Admin.template();
    dm.enable(&adm, "root", 0).unwrap();
    assert!(!dm.network_wide());
    key_file(&dm, "laptop", &"ab".repeat(32));
    dm.note_key_addr(&"ab".repeat(32), "192.168.50.20");
    dm.set_network_wide(&adm, "root", true).unwrap();
    dm.allow_peer(&adm, "root", "192.168.50.50").unwrap();
    dm.start_pairing(&adm, "root").unwrap();
    dm.allow_pairing_through_firewall(&adm, "root").unwrap();
    // a restart forgets the pairing request, keeps the rest while developer mode stays on
    dm.fw_start_reset();
    let w = dm.fw_want();
    assert!(w.on && w.network_wide && w.pair_until == 0 && w.peers.len() == 2);
    // off: nothing is asked any more, and the opt-in does not come back on its own
    dm.disable(&adm, "root").unwrap();
    let w = dm.fw_want();
    assert!(!w.on && !w.network_wide && w.pair_until == 0 && w.peers.iter().all(|p| p == "192.168.50.20"), "{w:?}");
    dm.enable(&adm, "root", 0).unwrap();
    assert!(!dm.fw_want().network_wide && dm.fw_want().peers == vec!["192.168.50.20".to_string()]);
}
