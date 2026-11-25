//! The network and sharing pages on a fake [`NetworkSettingsPort`] (lane ST-C): Remote access and its Connect, Machines and SFTP parts, Announce,
//! Fleet, VPN, SSH tunnels, Domains, Firewall, Shared folders, Share internet and the Network half of the hardware page. A change is applied
//! and audited with the person's name, a non-administrator is refused and the refusal is audited, a refusal from the service shows and writes
//! no audit line, and every page draws at six sizes. No shell, no sleeping.

use std::sync::atomic::Ordering;

use super::*;
use super::network_port::{FakeNetworkPort, NetworkSettingsPort};
use crate::apps::tests::cx_with_admin_store;
use iyakki_shared::fleetview::{FleetAct, Status as FleetStatus};

fn app(tab: Tab) -> SettingsApp {
    let cx = cx_with_admin_store("snp-app");
    let mut a = SettingsApp::new(&cx);
    a.go_to(tab);
    a
}

fn say(a: &SettingsApp) -> (String, bool) {
    a.status().cloned().unwrap_or_default()
}

fn denied(p: &FakeNetworkPort) -> bool {
    p.audited().iter().any(|(act, _)| act == "settings.denied")
}

const SIZES: [(u32, u32); 6] = [(640, 400), (800, 600), (1024, 768), (1280, 720), (1920, 1080), (360, 640)];

/// Draw the page with `f` at every size and say that something can be pressed.
fn draws(mut f: impl FnMut(&mut Gfx, R, f32)) {
    for (w, h) in SIZES {
        let mut g = Gfx::new(w, h);
        g.clear(rgb(0, 0, 0));
        let s = (w as f32 / 1280.0).min(h as f32 / 720.0).clamp(0.6, 3.0);
        f(&mut g, R::new(0.0, 72.0, w as f32, h as f32 - 72.0), s);
    }
}

fn open_form(a: &mut SettingsApp) -> (FormKind, Form) {
    match a.modal.take() {
        Some(Modal::Form(k, f)) => (k, f),
        _ => panic!("no form is open"),
    }
}

fn fleet_status() -> FleetStatus {
    FleetStatus { me: "ada".into(), my_fingerprint: "aa:bb".into(), listening: None, accept: Default::default(), fleets: vec![], offers: vec![], requests: vec![], pending: vec![], inbox: vec![], outbox: vec![], tls: false, relay: String::new() }
}

// ---- Remote access ----------------------------------------------------------------------------------------------------------------

#[test]
fn remote_access_is_switched_on_and_off_through_the_port_and_audited() {
    let p = FakeNetworkPort::new("snp-remote", true);
    let mut a = app(Tab::Remote);
    a.handle_remote(&Key::Char('e'), &p);
    assert!(p.web_access_on());
    assert!(p.audited().contains(&("settings.web_access".to_string(), "on (port 8443, HTTPS)".to_string())), "{:?}", p.audited());
    assert!(say(&a).1 && say(&a).0.contains("Serving this desktop"));
    a.handle_remote(&Key::Char('e'), &p);
    assert!(!p.web_access_on());
    assert!(p.audited().contains(&("settings.web_access".to_string(), "off (port 8443, HTTPS)".to_string())));
}

#[test]
fn remote_access_does_not_switch_on_without_an_administrator_who_can_sign_in() {
    let p = FakeNetworkPort::new("snp-remote-nopw", true);
    p.admin_password.store(false, Ordering::SeqCst);
    let mut a = app(Tab::Remote);
    a.handle_remote(&Key::Char('e'), &p);
    assert!(!p.web_access_on());
    assert!(!say(&a).1 && say(&a).0.contains("no administrator account"), "{:?}", say(&a));
    assert!(p.audited().is_empty());
}

#[test]
fn a_refusal_from_the_service_shows_and_nothing_is_audited() {
    let p = FakeNetworkPort::new("snp-remote-refuse", true);
    p.web_refuses.store(true, Ordering::SeqCst);
    let mut a = app(Tab::Remote);
    a.handle_remote(&Key::Char('e'), &p);
    assert!(!say(&a).1 && say(&a).0.contains("listener"), "{:?}", say(&a));
    assert!(p.audited().is_empty());
}

#[test]
fn a_person_who_is_not_an_administrator_cannot_switch_remote_access_and_the_refusal_is_audited() {
    let p = FakeNetworkPort::new("snp-remote-user", false);
    let mut a = app(Tab::Remote);
    a.handle_remote(&Key::Char('e'), &p);
    a.handle_remote(&Key::Char('p'), &p);
    assert!(!p.web_access_on());
    assert!(denied(&p));
    assert!(a.modal.is_none(), "no port form for a non-administrator");
}

#[test]
fn the_port_form_saves_through_the_port_and_a_bad_port_is_refused() {
    let p = FakeNetworkPort::new("snp-remote-port", true);
    let mut a = app(Tab::Remote);
    a.handle_remote(&Key::Char('p'), &p);
    let (kind, mut f) = open_form(&mut a);
    let FormKind::Remote(k) = kind else { panic!("another form") };
    f.set_value(0, "9443");
    let m = a.remote_submit(&p, &k, &mut f).unwrap();
    assert!(m.contains("9443"), "{m}");
    assert_eq!(p.web_access_port(), 9443);
    assert!(p.audited().contains(&("settings.web_access.port".to_string(), "9443".to_string())));
    f.set_value(0, "12");
    assert!(a.remote_submit(&p, &k, &mut f).is_err());
    assert_eq!(p.web_access_port(), 9443);
}

#[test]
fn the_remote_page_draws_from_the_fake_at_six_sizes() {
    let p = FakeNetworkPort::new("snp-remote-draw", true);
    let mut a = app(Tab::Remote);
    draws(|g, r, s| a.render_remote(g, r, s, &p));
    p.web_on.store(true, Ordering::SeqCst);
    draws(|g, r, s| a.render_remote(g, r, s, &p));
}

// ---- Connect (experimental), Machines, SFTP ---------------------------------------------------------------------------------------------

#[test]
fn client_connections_are_switched_in_the_fakes_data_folder_and_audited() {
    let p = FakeNetworkPort::new("snp-connect", true);
    let mut a = app(Tab::Remote);
    a.handle_remote(&Key::Char('k'), &p);
    assert!(iyakki_shared::clientlink::enabled(p.dir()));
    assert!(p.audited().contains(&("settings.client_endpoint".to_string(), "on".to_string())));
    a.handle_remote(&Key::Char('k'), &p);
    assert!(!iyakki_shared::clientlink::enabled(p.dir()));
}

#[test]
fn a_one_time_link_needs_client_connections_on_first() {
    let p = FakeNetworkPort::new("snp-connect-link", true);
    let mut a = app(Tab::Remote);
    a.handle_remote(&Key::Char('l'), &p);
    assert!(!say(&a).1 && say(&a).0.contains("Switch client connections on first"), "{:?}", say(&a));
}

#[test]
fn the_connect_keys_are_for_administrators_only() {
    let p = FakeNetworkPort::new("snp-connect-user", false);
    let mut a = app(Tab::Remote);
    a.handle_remote(&Key::Char('k'), &p);
    assert!(!iyakki_shared::clientlink::enabled(p.dir()));
    assert!(denied(&p));
}

#[test]
fn a_machine_for_flows_is_saved_and_forgotten_in_the_persons_folder_and_audited() {
    let p = FakeNetworkPort::new("snp-machines", true);
    let mut a = app(Tab::Remote);
    a.handle_remote(&Key::Char('M'), &p);
    let (kind, mut f) = open_form(&mut a);
    let FormKind::Machine(k) = kind else { panic!("another form") };
    f.set_value(0, "builder");
    f.set_value(1, "https://builder.example.org:8443");
    f.set_value(2, "builder-token");
    assert!(a.machine_submit(&p, &k, &mut f).unwrap().contains("builder"));
    assert!(p.audited().iter().any(|(act, what)| act == "flow.remote_machine" && what.starts_with("saved builder")));
    a.handle_remote(&Key::Char('X'), &p);
    let (kind, mut f) = open_form(&mut a);
    let FormKind::Machine(k) = kind else { panic!("another form") };
    assert_eq!(f.value(0), "builder", "the only machine is offered");
    assert!(a.machine_submit(&p, &k, &mut f).unwrap().contains("Forgot"));
    f.set_value(0, "builder");
    assert!(a.machine_submit(&p, &k, &mut f).is_err(), "it is gone");
}

#[test]
fn a_machine_without_a_vault_entry_name_is_refused_and_not_audited() {
    let p = FakeNetworkPort::new("snp-machines-bad", true);
    let mut a = app(Tab::Remote);
    a.handle_remote(&Key::Char('M'), &p);
    let (kind, mut f) = open_form(&mut a);
    let FormKind::Machine(k) = kind else { panic!("another form") };
    f.set_value(0, "builder");
    f.set_value(1, "https://builder.example.org");
    f.set_value(2, "not a vault name with spaces and a secret");
    assert!(a.machine_submit(&p, &k, &mut f).is_err());
    assert!(p.audited().is_empty());
}

#[test]
fn the_sftp_card_draws_from_the_fake_server() {
    let p = FakeNetworkPort::new("snp-sftp", true);
    let a = app(Tab::Remote);
    let mut lines = Vec::new();
    a.sftp_lines(&p, &mut lines);
    assert!(lines.iter().any(|l| l.0 == "SFTP file server"));
    assert!(lines.iter().any(|l| l.0 == "SFTP is off."), "the fake's own status line is shown");
}

// ---- Announce ---------------------------------------------------------------------------------------------------------------------------

#[test]
fn announcing_the_device_name_is_switched_through_the_port_and_audited() {
    let p = FakeNetworkPort::new("snp-announce", true);
    let mut a = app(Tab::Network);
    assert!(a.announce_line(&p).contains("OFF"));
    a.toggle_announce(&p);
    assert!(p.announce_on());
    assert!(p.audited().contains(&("settings.announce_name".to_string(), "on".to_string())));
    assert!(a.announce_line(&p).contains("ON"));
    a.toggle_announce(&p);
    assert!(!p.announce_on());
}

#[test]
fn a_person_who_is_not_an_administrator_cannot_switch_announcing() {
    let p = FakeNetworkPort::new("snp-announce-user", false);
    let mut a = app(Tab::Network);
    a.toggle_announce(&p);
    assert!(!p.announce_on());
    assert!(denied(&p));
    assert!(!say(&a).1);
}

// ---- the Network half of the hardware page -----------------------------------------------------------------------------------------------

#[test]
fn the_network_keys_that_change_the_machine_are_for_administrators_only() {
    let p = FakeNetworkPort::new("snp-net-user", false);
    let mut a = app(Tab::Network);
    a.choose_mac_mode(&p);
    a.ask_mac_apply(&p);
    a.scan_wifi(&p);
    assert!(a.modal.is_none());
    assert_eq!(p.audited().iter().filter(|(act, _)| act == "settings.denied").count(), 3);
}

#[test]
fn a_sign_in_page_is_not_opened_when_the_network_asked_for_none_and_one_adapter_cannot_be_switched() {
    let p = FakeNetworkPort::new("snp-net-link", true);
    let mut a = app(Tab::Network);
    a.open_sign_in(&p);
    assert!(!say(&a).1 && say(&a).0.contains("has not asked for a sign-in"), "{:?}", say(&a));
    a.use_next_adapter(&p);
    assert!(!say(&a).1 && say(&a).0.contains("one WiFi adapter"), "{:?}", say(&a));
    a.link_tick(&p);
}

// ---- Fleet ------------------------------------------------------------------------------------------------------------------------------

#[test]
fn a_confirmed_fleet_act_goes_through_the_port_and_its_answer_shows() {
    let p = FakeNetworkPort::new("snp-fleet", true);
    let mut a = app(Tab::Fleet);
    a.fleet_confirm(&p, FleetAct::Tls(true));
    assert_eq!(*p.fleet_acts.lock().unwrap(), vec![FleetAct::Tls(true)]);
    assert!(say(&a).1 && say(&a).0 == "Done.");
    *p.fleet_answer.lock().unwrap() = Err("The other computer refused.".into());
    a.fleet_confirm(&p, FleetAct::Relay(true));
    assert!(!say(&a).1 && say(&a).0.contains("refused"));
}

#[test]
fn a_person_who_is_not_an_administrator_runs_no_fleet_act() {
    let p = FakeNetworkPort::new("snp-fleet-user", false);
    let mut a = app(Tab::Fleet);
    a.fleet_confirm(&p, FleetAct::Tls(true));
    assert!(p.fleet_acts.lock().unwrap().is_empty());
    assert!(denied(&p));
}

#[test]
fn the_fleet_page_asks_before_it_acts_and_says_when_fleets_are_unavailable() {
    let p = FakeNetworkPort::new("snp-fleet-card", true);
    *p.fleet.lock().unwrap() = Ok(fleet_status());
    let mut a = app(Tab::Fleet);
    a.handle_fleet(&Key::Char('T'), &p);
    assert!(matches!(a.modal, Some(Modal::Confirm(ConfirmKind::Fleet(FleetAct::Tls(true)), _))), "a card first");
    assert!(p.fleet_acts.lock().unwrap().is_empty(), "nothing runs before the answer");
    a.modal = None;
    draws(|g, r, s| a.render_fleet(g, r, s, &p));
    *p.fleet.lock().unwrap() = Err("Fleets need the core.".into());
    a.handle_fleet(&Key::Char('T'), &p);
    assert!(!say(&a).1 && say(&a).0.contains("Fleets need the core"));
    draws(|g, r, s| a.render_fleet(g, r, s, &p));
}

// ---- VPN, SSH tunnels, Domains -----------------------------------------------------------------------------------------------------------

#[test]
fn the_vpn_page_draws_and_a_manager_that_cannot_delete_says_so() {
    let p = FakeNetworkPort::new("snp-vpn", true);
    let mut a = app(Tab::Vpn);
    draws(|g, r, s| a.render_vpn(g, r, s, &p));
    a.vpn_delete(&p, "nothing");
    assert!(!say(&a).1, "{:?}", say(&a));
    assert!(p.audited().is_empty());
    a.vpn_tick(&p);
}

#[test]
fn the_ssh_tunnels_page_draws_and_a_manager_that_cannot_delete_says_so() {
    let p = FakeNetworkPort::new("snp-ssh", true);
    let mut a = app(Tab::SshTunnels);
    draws(|g, r, s| a.render_tun(g, r, s, &p));
    a.tun_delete(&p, "nothing");
    assert!(!say(&a).1, "{:?}", say(&a));
    a.handle_tun(&Key::Char('a'), &p);
    a.tun_tick(&p);
}

#[test]
fn the_domains_page_draws_and_a_manager_that_cannot_delete_says_so() {
    let p = FakeNetworkPort::new("snp-dom", true);
    let mut a = app(Tab::Domains);
    draws(|g, r, s| a.render_dom(g, r, s, &p));
    a.dom_delete(&p, "example.org", "/");
    assert!(!say(&a).1, "{:?}", say(&a));
    a.dom_tick(&p);
}

// ---- Firewall and Shared folders ----------------------------------------------------------------------------------------------------------

#[test]
fn the_firewall_page_draws_in_both_views_and_refuses_a_non_administrator() {
    let p = FakeNetworkPort::new("snp-fw", false);
    let mut a = app(Tab::Firewall);
    draws(|g, r, s| a.render_firewall(g, r, s, &p));
    a.fw.advanced = true;
    draws(|g, r, s| a.render_firewall(g, r, s, &p));
    a.handle_firewall(&Key::Char('a'), &p);
    assert!(denied(&p));
    assert!(a.modal.is_none());
    a.fw_import(&p, "not a rule file");
}

#[test]
fn opening_the_firewall_for_shared_folders_is_for_administrators_only() {
    let p = FakeNetworkPort::new("snp-fw-shares", false);
    let mut a = app(Tab::Shares);
    a.fw_open_shares_here(&p);
    assert!(denied(&p));
}

#[test]
fn the_shared_folders_page_draws_from_the_fakes_manager_at_six_sizes() {
    let p = FakeNetworkPort::new("snp-shares", true);
    let mut a = app(Tab::Shares);
    draws(|g, r, s| a.render_shares(g, r, s, &p));
    assert_eq!(SettingsApp::shares_len(&p), 0);
}

#[test]
fn receiving_a_package_is_for_administrators_only_and_nothing_is_started() {
    let p = FakeNetworkPort::new("snp-shares-user", false);
    let mut a = app(Tab::Shares);
    a.handle_shares(&Key::Char('r'), &p);
    assert!(denied(&p));
}

#[test]
fn share_internet_draws_without_a_backend() {
    let p = FakeNetworkPort::new("snp-share", true);
    let mut a = app(Tab::Network);
    a.share_open(&p);
    draws(|g, r, s| a.render_share(g, r, s, &p));
}
