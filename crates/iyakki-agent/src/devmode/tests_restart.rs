//! The standing restart grant: lasts as long as the developer connection, restart and switch-off only, audited, and used again only
//! after the device reported a successful start.

use std::sync::atomic::Ordering;

use iyakki_sec::policy::Role;

use super::restartgrant::{self, FILE};
use super::scopes::Scope;
use super::tests_adb::{call, key, rig};
use super::*;

const NOW: i64 = 1_800_000_000;

fn put_keys(r: &super::tests_adb::Rig) {
    let v = vec![key("laptop", "root", &[Scope::Install, Scope::Power]), key("plain", "root", &[Scope::Install]), key("shelly", "root", &[Scope::Install, Scope::Shell])];
    std::fs::write(r.dir.join("devmode/keys.json"), serde_json::to_string(&v).unwrap()).unwrap();
}

fn acts(r: &super::tests_adb::Rig) -> Vec<String> {
    r.dm.activity(100).into_iter().map(|a| format!("{} {} {}", a.action, a.ok, a.detail)).collect()
}

#[test]
fn schema_spec_is_valid_and_the_file_carries_a_schema_number_and_keeps_unknown_fields() {
    restartgrant::RESTART_SPEC.check().unwrap();
    let r = rig("rg-schema");
    put_keys(&r);
    r.dm.restart_grant_on(&Role::Admin.template(), "root", "laptop").unwrap();
    let p = r.dir.join("devmode").join(FILE);
    let mut v: serde_json::Value = serde_json::from_slice(&std::fs::read(&p).unwrap()).unwrap();
    assert_eq!(v["schema"], 1);
    v["from_the_future"] = serde_json::json!({"a": 1});
    v["grants"][0]["new_field"] = serde_json::json!(true);
    std::fs::write(&p, serde_json::to_vec(&v).unwrap()).unwrap();
    r.dm.restart_grant_on(&Role::Admin.template(), "root", "laptop").unwrap();
    let w: serde_json::Value = serde_json::from_slice(&std::fs::read(&p).unwrap()).unwrap();
    assert_eq!((w["from_the_future"]["a"].clone(), w["grants"][0]["new_field"].clone()), (serde_json::json!(1), serde_json::json!(true)), "kept");
}

#[test]
fn only_an_administrator_ticks_it_for_a_paired_computer_that_holds_power() {
    let r = rig("rg-who");
    put_keys(&r);
    let adm = Role::Admin.template();
    assert!(r.dm.restart_grant_on(&Role::User.template(), "bob", "laptop").is_err());
    assert!(r.dm.restart_grant_on(&adm, "root", "plain").unwrap_err().contains("power"));
    assert!(r.dm.restart_grant_on(&adm, "root", "nobody").is_err());
    assert!(r.dm.restart_grant_list().is_empty());
    r.dm.restart_grant_on(&adm, "root", "shelly").unwrap();
    assert!(r.dm.restart_grant_active("shelly"), "shell implies power");
    assert!(r.dm.restart_grant_off(&Role::User.template(), "bob", "shelly").is_err());
    r.dm.restart_grant_off(&adm, "root", "shelly").unwrap();
    assert!(!r.dm.restart_grant_active("shelly"));
}

#[test]
fn it_lasts_exactly_as_long_as_the_developer_connection_and_ends_with_it() {
    let r = rig("rg-time");
    put_keys(&r);
    let adm = Role::Admin.template();
    // the developer connection switches off after 2 hours
    r.dm.enable(&adm, "root", 2).unwrap();
    let v = r.dm.restart_grant_on(&adm, "root", "laptop").unwrap();
    assert_eq!(v.until, NOW + 7200, "the grant's end is the connection's end");
    // extending the connection extends the grant; there is no time of its own
    r.dm.extend(&adm, "root", 4).unwrap();
    r.clock.store(NOW + 7201, Ordering::SeqCst);
    assert!(r.dm.restart_grant_active("laptop"));
    assert_eq!(r.dm.restart_grant_list()[0].until, NOW + 6 * 3600);
    // time is up: the connection and the grant are gone, and stay gone when developer mode is switched on again
    r.clock.store(NOW + 6 * 3600, Ordering::SeqCst);
    assert!(!r.dm.restart_grant_active("laptop"));
    assert!(r.dm.restart_grant_use("laptop", "restart").is_err());
    r.dm.enable(&adm, "root", 0).unwrap();
    assert!(!r.dm.restart_grant_active("laptop"), "a new connection needs the box ticked again");
    // switching off by hand, and revoking the computer, end it too
    r.dm.restart_grant_on(&adm, "root", "laptop").unwrap();
    r.dm.disable(&adm, "root").unwrap();
    r.dm.enable(&adm, "root", 0).unwrap();
    assert!(!r.dm.restart_grant_active("laptop"));
    r.dm.restart_grant_on(&adm, "root", "laptop").unwrap();
    r.dm.revoke(&adm, "root", "laptop").unwrap();
    assert!(r.dm.restart_grant_list().is_empty());
    assert!(r.dm.restart_grant_on(&adm, "root", "laptop").is_err(), "no longer paired");
    assert!(r.dm.restart_grant_on(&adm, "root", "shelly").is_ok());
    // losing the power scope ends it as well
    r.dm.set_scope(&adm, "root", "shelly", Scope::Shell, false).unwrap();
    assert!(!r.dm.restart_grant_active("shelly"));
    // off: refused to tick
    r.dm.disable(&adm, "root").unwrap();
    assert!(r.dm.restart_grant_on(&adm, "root", "shelly").is_err());
}

#[test]
fn a_second_restart_waits_for_a_reported_successful_start_and_every_use_is_audited() {
    let r = rig("rg-start");
    put_keys(&r);
    let adm = Role::Admin.template();
    r.dm.restart_grant_on(&adm, "root", "laptop").unwrap();
    assert_eq!(r.dm.restart_grant_use("laptop", "restart").unwrap(), "root");
    let e = r.dm.restart_grant_use("laptop", "restart").unwrap_err();
    assert!(e.contains("successful start"), "{e}");
    assert!(r.dm.restart_grant_list()[0].awaiting_start);
    // the state is on disk: a new DevMode (the next boot) still waits until the start is reported
    assert!(DevMode::open(&r.dir).with_clock(std::sync::Arc::new(|| NOW)).restart_grant_use("laptop", "restart").is_err());
    r.dm.restart_started();
    assert!(!r.dm.restart_grant_list()[0].awaiting_start);
    r.dm.restart_grant_use("laptop", "switch off").unwrap();
    let a = acts(&r);
    assert_eq!(a.iter().filter(|l| l.starts_with("devmode.restart.use true")).count(), 2, "{a:?}");
    assert!(a.iter().any(|l| l.starts_with("devmode.restart.refused false")) && a.iter().any(|l| l.starts_with("devmode.restart.grant_on true")) && a.iter().any(|l| l.starts_with("devmode.restart.started true")), "{a:?}");
    // a computer without a grant is refused and logged
    assert!(r.dm.restart_grant_use("plain", "restart").is_err());
}

#[test]
fn the_power_route_uses_the_grant_without_a_card_only_for_restart_and_switch_off_and_is_audited() {
    let r = rig("rg-route");
    put_keys(&r);
    let adm = Role::Admin.template();
    let laptop = r.dm.keys().into_iter().find(|k| k.name == "laptop").unwrap();
    // no card, no grant: refused
    assert_eq!(call(&r, &laptop, &adm, "POST", "/v1/dev/power/reboot", &[], b"").unwrap().status, 403);
    r.dm.restart_grant_on(&adm, "root", "laptop").unwrap();
    // nobody is signed in and no card is open: the grant is enough
    assert!(r.dm.shell_pending().is_empty());
    assert_eq!(call(&r, &laptop, &adm, "POST", "/v1/dev/power/reboot", &[], b"").unwrap().status, 200);
    assert_eq!(call(&r, &laptop, &adm, "POST", "/v1/dev/power/poweroff", &[], b"").unwrap().status, 403, "waits for the reported start");
    r.dm.restart_started();
    assert_eq!(call(&r, &laptop, &adm, "POST", "/v1/dev/power/poweroff", &[], b"").unwrap().status, 200);
    // an account that is not an administrator is refused even with the grant
    r.dm.restart_started();
    assert_eq!(call(&r, &laptop, &Role::User.template(), "POST", "/v1/dev/power/reboot", &[], b"").unwrap().status, 403);
    // the grant opens nothing else: no shell grant appears, no command, no root, no patch
    assert!(r.dm.shell_grant("laptop").is_none());
    assert_eq!(r.dm.grant_request(&laptop, "shell", false, 5, "").unwrap(), super::grants::ShellStatus::Pending);
    assert_eq!(r.dm.grant_request(&laptop, "patch", false, 5, "").unwrap(), super::grants::ShellStatus::Pending);
    assert!(acts(&r).iter().any(|l| l.contains("devmode.restart.use") || l.contains("dev.power.reboot")));
}
