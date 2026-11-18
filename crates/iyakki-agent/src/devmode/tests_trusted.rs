//! The trusted dev computer switch and "patches without asking": off by default, a hard ask on the device, covers only the approval
//! cards, timed or kept, audited, shown in the status text and the bar, ended with the pairing, and the lost restart grant explained.

use std::sync::atomic::Ordering;

use iyakki_sec::policy::Role;

use super::grants::ShellStatus;
use super::scopes::Scope;
use super::tests_adb::{call, key, rig, Rig};
use super::trusted::{self, TrustFor, TrustLevel};
use super::*;

const NOW: i64 = 1_800_000_000;

fn put_keys(r: &Rig) {
    let v = vec![
        key("laptop", "root", &[Scope::Install, Scope::Shell, Scope::Update, Scope::Power]),
        key("other", "root", &[Scope::Install, Scope::Shell, Scope::Update, Scope::Power]),
        key("plain", "root", &[Scope::Install]),
        key("rooty", "root", &[Scope::Install, Scope::Shell, Scope::Update, Scope::Power]),
    ];
    std::fs::write(r.dir.join("devmode/keys.json"), serde_json::to_string(&v).unwrap()).unwrap();
}

fn acts(r: &Rig) -> Vec<String> {
    r.dm.activity(200).into_iter().map(|a| format!("{} {} {}", a.action, a.ok, a.detail)).collect()
}

fn k(r: &Rig, n: &str) -> DevKey {
    r.dm.keys().into_iter().find(|k| k.name == n).unwrap()
}

#[test]
fn schema_is_valid_and_the_switch_is_off_by_default_so_every_approval_asks() {
    trusted::TRUSTED_SPEC.check().unwrap();
    let r = rig("tr-off");
    put_keys(&r);
    let laptop = k(&r, "laptop");
    assert!(r.dm.trusted_list().is_empty() && r.dm.trusted_banner().is_none());
    for kind in ["shell", "patch", "power"] {
        assert_eq!(r.dm.grant_request(&laptop, kind, false, 5, "").unwrap(), ShellStatus::Pending, "{kind} asks");
    }
    let adm = Role::Admin.template();
    assert_eq!(call(&r, &laptop, &adm, "POST", "/v1/dev/patch/plan", &[], b"x").unwrap().status, 403);
    assert_eq!(call(&r, &laptop, &adm, "POST", "/v1/dev/power/reboot", &[], b"").unwrap().status, 403);
}

#[test]
fn switching_on_is_for_administrators_with_developer_mode_on_and_a_paired_computer() {
    let r = rig("tr-who");
    put_keys(&r);
    let (adm, user) = (Role::Admin.template(), Role::User.template());
    assert!(r.dm.trusted_on(&user, "bob", "laptop", TrustLevel::Everything, TrustFor::Kept).is_err(), "a hard ask on the device: only an administrator");
    assert!(r.dm.trusted_on(&adm, "root", "nobody", TrustLevel::Everything, TrustFor::Kept).is_err());
    assert!(r.dm.trusted_on(&adm, "root", "plain", TrustLevel::Everything, TrustFor::Kept).unwrap_err().contains("scope"));
    assert!(r.dm.trusted_on(&adm, "root", "laptop", TrustLevel::Everything, TrustFor::Hours(0)).is_err());
    assert!(r.dm.trusted_on(&adm, "root", "laptop", TrustLevel::Everything, TrustFor::Hours(99)).is_err());
    assert!(r.dm.trusted_list().is_empty());
    r.dm.disable(&adm, "root").unwrap();
    assert!(r.dm.trusted_on(&adm, "root", "laptop", TrustLevel::Everything, TrustFor::Kept).unwrap_err().contains("off"));
    assert!(r.dm.trusted_off(&user, "bob", "laptop").is_err());
}

#[test]
fn with_the_switch_on_every_approval_kind_is_answered_yes_and_a_request_from_another_computer_still_asks() {
    let r = rig("tr-all");
    put_keys(&r);
    let adm = Role::Admin.template();
    r.dm.trusted_on(&adm, "root", "laptop", TrustLevel::Everything, TrustFor::Kept).unwrap();
    let laptop = k(&r, "laptop");
    for kind in ["shell", "patch", "power"] {
        assert!(matches!(r.dm.grant_request(&laptop, kind, false, 5, "").unwrap(), ShellStatus::Granted(g) if g.kind == kind && !g.as_root && g.by.contains("trusted")), "{kind} answered yes");
    }
    assert!(r.dm.shell_pending().is_empty(), "no card was raised");
    // through the same route the client uses (grant/ask) and the routes that need the approval
    assert_eq!(call(&r, &laptop, &adm, "POST", "/v1/dev/grant/ask", &[], br#"{"kind":"patch"}"#).unwrap().status, 200);
    assert_eq!(call(&r, &laptop, &adm, "POST", "/v1/dev/patch/plan", &[], b"x").unwrap().status, 400, "past the approval gate (bad body)");
    assert_eq!(call(&r, &laptop, &adm, "POST", "/v1/dev/power/reboot", &[], b"").unwrap().status, 200);
    // another computer, with the same scopes, still asks
    let other = k(&r, "other");
    assert_eq!(r.dm.grant_request(&other, "shell", false, 5, "").unwrap(), ShellStatus::Pending);
    assert_eq!(r.dm.grant_request(&other, "patch", false, 5, "").unwrap(), ShellStatus::Pending);
    assert_eq!(call(&r, &other, &adm, "POST", "/v1/dev/patch/plan", &[], b"x").unwrap().status, 403);
    assert_eq!(call(&r, &other, &adm, "POST", "/v1/dev/power/reboot", &[], b"").unwrap().status, 403);
    // the scope still decides: a computer without the shell scope gets no shell approval from the switch
    r.dm.set_scope(&adm, "root", "laptop", Scope::Shell, false).ok();
    let a = acts(&r);
    assert!(a.iter().any(|l| l.contains("dev.trusted.auto") && l.contains("run commands")) && a.iter().any(|l| l.contains("dev.trusted.auto") && l.contains("patch")) && a.iter().any(|l| l.contains("dev.trusted.auto") && l.contains("restart or switch off")), "{a:?}");
    assert!(a.iter().all(|l| !l.contains("x ") || !l.contains("dev.trusted.auto")), "names only, never arguments");
}

#[test]
fn a_request_to_run_as_root_still_gets_its_card() {
    let r = rig("tr-root");
    put_keys(&r);
    r.dm.trusted_on(&Role::Admin.template(), "root", "rooty", TrustLevel::Everything, TrustFor::Kept).unwrap();
    let rooty = k(&r, "rooty");
    assert_eq!(r.dm.grant_request(&rooty, "shell", true, 5, "").unwrap(), ShellStatus::Pending);
}

#[test]
fn patches_level_covers_the_patch_approval_only() {
    let r = rig("tr-patches");
    put_keys(&r);
    let adm = Role::Admin.template();
    r.dm.trusted_on(&adm, "root", "laptop", TrustLevel::Patches, TrustFor::Kept).unwrap();
    let laptop = k(&r, "laptop");
    assert!(matches!(r.dm.grant_request(&laptop, "patch", false, 5, "").unwrap(), ShellStatus::Granted(_)));
    assert_eq!(r.dm.grant_request(&laptop, "shell", false, 5, "").unwrap(), ShellStatus::Pending);
    assert_eq!(r.dm.grant_request(&laptop, "power", false, 5, "").unwrap(), ShellStatus::Pending);
    assert_eq!(call(&r, &laptop, &adm, "POST", "/v1/dev/power/reboot", &[], b"").unwrap().status, 403);
    assert!(r.dm.trusted_banner().unwrap().starts_with("PATCHES WITHOUT ASKING"));
    assert!(r.dm.status_text().contains("patches without asking"));
}

#[test]
fn the_banner_and_status_text_name_the_computer_and_the_bar_sign_follows() {
    let r = rig("tr-banner");
    put_keys(&r);
    let adm = Role::Admin.template();
    assert!(!r.dm.status_text().contains("TRUSTED"));
    r.dm.trusted_on(&adm, "root", "laptop", TrustLevel::Everything, TrustFor::Hours(4)).unwrap();
    assert_eq!(r.dm.trusted_banner().unwrap(), "TRUSTED COMPUTER laptop");
    assert_eq!(r.dm.shell_banner().unwrap(), "TRUSTED COMPUTER laptop");
    assert!(r.dm.status_text().contains("DEV · TRUSTED COMPUTER: laptop"), "{}", r.dm.status_text());
    r.dm.trusted_off(&adm, "root", "laptop").unwrap();
    assert!(r.dm.shell_banner().is_none() && !r.dm.status_text().contains("TRUSTED"));
    assert!(acts(&r).iter().any(|l| l.contains("devmode.trusted.off")));
}

#[test]
fn a_timed_switch_expires_and_says_so_and_a_kept_one_survives_the_timer_and_a_reload() {
    let r = rig("tr-time");
    put_keys(&r);
    let adm = Role::Admin.template();
    r.dm.enable(&adm, "root", 24).unwrap();
    r.dm.trusted_on(&adm, "root", "laptop", TrustLevel::Everything, TrustFor::Hours(4)).unwrap();
    assert_eq!(r.dm.trusted_list()[0].until, NOW + 4 * 3600);
    assert!(r.dm.state().until > 0, "a timed switch leaves the automatic switch-off alone");
    r.clock.store(NOW + 4 * 3600 + 1, Ordering::SeqCst);
    assert!(r.dm.trusted_banner().is_none() && !r.dm.trusted_active("laptop"));
    assert!(acts(&r).iter().any(|l| l.contains("devmode.trusted.expired")));
    assert_eq!(r.dm.grant_request(&k(&r, "laptop"), "shell", false, 5, "").unwrap(), ShellStatus::Pending);
    // kept: the connection stays, the timer cannot end it, and a fresh DevMode on the same folder (a restart) still has it
    r.clock.store(NOW, Ordering::SeqCst);
    r.dm.trusted_on(&adm, "root", "laptop", TrustLevel::Everything, TrustFor::Kept).unwrap();
    assert_eq!(r.dm.state().until, 0);
    r.clock.store(NOW + 100 * 3600, Ordering::SeqCst);
    let again = DevMode::open(&r.dir).with_clock(std::sync::Arc::new(|| NOW + 100 * 3600));
    assert!(again.is_on() && again.trusted_active("laptop"));
    // ending it by hand puts the default switch-off time back
    again.trusted_off(&adm, "root", "laptop").unwrap();
    assert!(again.state().until > 0 && !again.trusted_active("laptop"));
}

#[test]
fn removing_the_computer_or_pairing_it_again_or_switching_developer_mode_off_ends_it() {
    let r = rig("tr-end");
    put_keys(&r);
    let adm = Role::Admin.template();
    r.dm.trusted_on(&adm, "root", "laptop", TrustLevel::Everything, TrustFor::Kept).unwrap();
    r.dm.remove_pairing(&adm, "root", "laptop").unwrap();
    assert!(!r.dm.trusted_active("laptop") && r.dm.trusted_load().grants.is_empty());
    assert!(acts(&r).iter().any(|l| l.contains("devmode.trusted.off") && l.contains("pairing")));
    // a new computer under the same name does not inherit it
    put_keys(&r);
    assert!(!r.dm.trusted_active("laptop"));
    // developer mode off by hand forgets it and the log says why
    r.dm.trusted_on(&adm, "root", "other", TrustLevel::Everything, TrustFor::Kept).unwrap();
    r.dm.disable(&adm, "root").unwrap();
    assert!(r.dm.trusted_load().grants.is_empty());
    assert!(acts(&r).iter().any(|l| l.contains("devmode.grants.ended") && l.contains("1 trusted")));
    r.dm.enable(&adm, "root", 0).unwrap();
    assert!(!r.dm.trusted_active("other"));
}

#[test]
fn the_standing_restart_grant_answers_the_power_ask_at_once_and_a_lost_grant_is_explained_in_the_log() {
    let r = rig("tr-restart");
    put_keys(&r);
    let adm = Role::Admin.template();
    let laptop = k(&r, "laptop");
    // the client asks first (the narrow `power` name); without the grant it waits for a card
    let ask = |r: &Rig| call(r, &laptop, &adm, "POST", "/v1/dev/grant/ask", &[], br#"{"kind":"power"}"#).unwrap();
    assert!(String::from_utf8_lossy(&ask(&r).body).contains("pending"));
    r.dm.restart_grant_on(&adm, "root", "laptop").unwrap();
    let a = ask(&r);
    assert!(String::from_utf8_lossy(&a.body).contains("granted"), "no card needed");
    assert_eq!(call(&r, &laptop, &adm, "POST", "/v1/dev/power/reboot", &[], b"").unwrap().status, 200);
    // the usual loss: the connection's own timer switches developer mode off and the grant goes with it; the log says so
    r.dm.enable(&adm, "root", 4).unwrap();
    r.dm.restart_started();
    r.clock.store(NOW + 4 * 3600 + 1, Ordering::SeqCst);
    assert!(!r.dm.is_on() && !r.dm.restart_grant_active("laptop"));
    assert!(acts(&r).iter().any(|l| l.contains("devmode.grants.ended") && l.contains("restart grant")));
}

#[test]
fn pairing_again_under_the_same_name_without_power_says_the_restart_grant_no_longer_applies() {
    let r = rig("tr-repair");
    put_keys(&r);
    let adm = Role::Admin.template();
    r.dm.restart_grant_on(&adm, "root", "laptop").unwrap();
    assert!(r.dm.restart_load_has("laptop"));
    let p = r.dm.start_pairing(&adm, "root").unwrap();
    let fp = "ab".repeat(32);
    let key_hex = format!("{:0>64}", 77);
    let pk = auth::pair_key(&p.code, &fp).unwrap();
    let proof = auth::pair_proof(&pk, &fp, &key_hex, "laptop");
    r.dm.complete_pairing(&fp, "laptop", &key_hex, &proof).unwrap();
    assert!(!r.dm.restart_grant_active("laptop"));
    assert!(acts(&r).iter().any(|l| l.contains("devmode.restart.grant_ended") && l.contains("paired again")));
}
