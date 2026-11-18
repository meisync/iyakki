use super::*;
use crate::agent::Tools;
use crate::settingstools_tests::{assert_policy, rig, tools};

fn fake_sys(dir: &Path) -> PathBuf {
    let sys = dir.join("fake-sys");
    let smc = sys.join("class/hwmon/hwmon2");
    std::fs::create_dir_all(&smc).unwrap();
    std::fs::write(smc.join("name"), "applesmc\n").unwrap();
    for (k, v) in [("fan1_min", "1200"), ("fan1_max", "6200"), ("fan1_manual", "0"), ("fan1_output", "1200"), ("fan1_input", "1210"), ("temp1_label", "CPU"), ("temp1_input", "50000")] {
        std::fs::write(smc.join(k), v).unwrap();
    }
    sys
}

#[test]
fn fan_control_is_read_freely_changed_with_a_card_and_a_bad_curve_is_refused_before_asking() {
    let r = rig("fanchat", true);
    let sys = fake_sys(&r.dir);
    let t = tools(&r, entries(&r.env, sys.clone()));
    assert_policy(&t, &["fan_control_status", "vpn_helper_status"], &["fan_control_set", "vpn_helper_set"]);
    let first = t.call("fan_control_status", &json!({})).unwrap();
    assert!(first.contains("off") && first.contains("has fans IYAKKI can control"), "{first}");
    assert!(t.refusal("fan_control_set", &json!({})).is_some(), "nothing to change");
    assert!(t.refusal("fan_control_set", &json!({"low": 90, "mark": 70})).is_some());
    assert!(t.refusal("fan_control_set", &json!({"max": 200})).is_some());
    assert!(t.refusal("fan_control_set", &json!({"enabled": "yes"})).is_some());
    assert!(!r.dir.join(fanservice::SETTING_FILE).exists(), "a refusal writes nothing");
    assert!(t.preview("fan_control_set", &json!({"enabled": true, "low": 55})).unwrap().contains("After: on"));
    t.call("fan_control_set", &json!({"enabled": true, "low": 55})).unwrap();
    let s = fanservice::load(&r.dir);
    assert!(s.enabled && s.low == 55 && s.max == 86, "untouched fields stay: {s:?}");
    assert!(r.audits.lock().unwrap().iter().any(|a| a == "settings.fan on; curve: slowest up to 55 C, rising to the fastest at 86 C (mark 66 C), lowering only 2 C below the last rise" || a.starts_with("settings.fan")));
    // the tool never writes a fan file itself: only the service does
    assert_eq!(std::fs::read_to_string(sys.join("class/hwmon/hwmon2/fan1_manual")).unwrap(), "0");
    // a computer without an applesmc says so
    let none = tools(&r, entries(&r.env, r.dir.join("no-such-sys")));
    assert!(none.call("fan_control_status", &json!({})).unwrap().contains("no applesmc fan"));
    let _ = std::fs::remove_dir_all(&r.dir);
}

#[test]
fn the_vpn_helper_switch_writes_the_setting_the_core_reads_and_only_administrators_see_it() {
    let r = rig("vpnhelper", true);
    let t = tools(&r, entries(&r.env, r.dir.join("none")));
    assert!(t.call("vpn_helper_status", &json!({})).unwrap().contains("is off"));
    assert!(t.call("vpn_helper_set", &json!({})).is_err());
    t.call("vpn_helper_set", &json!({"enabled": true})).unwrap();
    assert!(crate::vpn_helper::helper_enabled(&vpn_dir(&r.env)));
    assert!(t.call("vpn_helper_status", &json!({})).unwrap().contains("is on"));
    t.call("vpn_helper_set", &json!({"enabled": false})).unwrap();
    assert!(!crate::vpn_helper::helper_enabled(&vpn_dir(&r.env)));
    assert!(r.audits.lock().unwrap().iter().any(|a| a == "settings.vpn-helper on"));
    let p = rig("vpnhelper2", false);
    let t2 = tools(&p, entries(&p.env, p.dir.join("none")));
    let names: Vec<String> = t2.defs().iter().map(|d| d.spec.name.clone()).collect();
    assert_eq!(names.iter().filter(|n| n.starts_with("fan_") || n.starts_with("vpn_helper")).count(), 2, "{names:?}");
    assert!(names.iter().all(|n| !n.ends_with("_set")));
    let _ = std::fs::remove_dir_all(&r.dir);
    let _ = std::fs::remove_dir_all(&p.dir);
}

#[test]
fn the_outgoing_plan_checks_and_shows_rules_but_applies_nothing() {
    let r = rig("outplan", true);
    let t = tools(&r, entries(&r.env, r.dir.join("none")));
    assert_policy(&t, &["firewall_outgoing_plan"], &[]);
    let ok = t.call("firewall_outgoing_plan", &json!({"config": {"block_by_default": true, "rules": [
        {"id": 1, "enabled": true, "verdict": "allow", "dest": "", "proto": "both", "ports": "53", "iface": "", "note": ""},
        {"id": 2, "enabled": true, "verdict": "allow", "dest": "", "proto": "tcp", "ports": "443", "iface": "", "note": "web"}]}})).unwrap();
    assert!(ok.contains("NOTHING WAS APPLIED") && ok.contains("policy drop") && ok.contains("th dport { 443 }"), "{ok}");
    let e = t.call("firewall_outgoing_plan", &json!({"config": {"block_by_default": true, "rules": []}})).unwrap_err();
    assert!(e.contains("at least one allow rule"), "{e}");
    assert!(t.call("firewall_outgoing_plan", &json!({})).is_err());
    assert!(t.call("firewall_outgoing_plan", &json!({"config": {"rules": "x"}})).is_err());
    let p = rig("outplan2", false);
    let t2 = tools(&p, entries(&p.env, p.dir.join("none")));
    assert!(!t2.defs().iter().any(|d| d.spec.name == "firewall_outgoing_plan"));
    let _ = std::fs::remove_dir_all(&r.dir);
    let _ = std::fs::remove_dir_all(&p.dir);
}
