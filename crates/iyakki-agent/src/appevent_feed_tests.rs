use super::*;

fn scratch(name: &str) -> std::path::PathBuf {
    let d = std::fs::canonicalize(std::env::temp_dir()).unwrap().join(format!("iyakki-feed-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

#[test]
fn the_audit_log_is_counted_for_failed_sign_ins_and_pairings() {
    let log = [
        r#"{"ts":1,"user":"-","event":"login.failed","detail":"web x","prev":"","hash":""}"#,
        r#"{"ts":2,"user":"-","event":"login.failed","detail":"web y","prev":"","hash":""}"#,
        r#"{"ts":3,"user":"ann","event":"devicekey.enroll","detail":"fp","prev":"","hash":""}"#,
        "not json",
    ]
    .join("\n");
    assert_eq!(audit_totals(&log), AuditTotals { failed_signins: 2, new_paired_devices: 1 });
    assert_eq!(audit_totals(""), AuditTotals::default());
}

#[test]
fn a_certificate_on_the_disk_gives_its_days_left_and_a_damaged_one_gives_nothing() {
    let d = scratch("cert");
    std::fs::create_dir_all(d.join("tls")).unwrap();
    let mut p = rcgen::CertificateParams::new(vec!["a.example".to_string()]).unwrap();
    p.not_before = rcgen::date_time_ymd(2020, 1, 1);
    p.not_after = rcgen::date_time_ymd(2030, 1, 11);
    let key = rcgen::KeyPair::generate().unwrap();
    let cert = p.self_signed(&key).unwrap();
    std::fs::write(d.join("tls/cert.pem"), cert.pem()).unwrap();
    std::fs::write(d.join("tls/broken.crt"), "-----BEGIN CERTIFICATE-----\n!!\n-----END CERTIFICATE-----\n").unwrap();
    let ten_days_before = first_cert_not_after(&cert.pem()).unwrap() - 10 * 86_400;
    assert_eq!(cert_days(&d, ten_days_before), BTreeMap::from([("tls/cert.pem".to_string(), 10)]));
    assert_eq!(cert_days(&d.join("none"), 0), BTreeMap::new());
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn a_waiting_bundle_is_a_downloaded_update() {
    let d = scratch("upd");
    assert!(update_phases(&d, &[]).is_empty());
    std::fs::create_dir_all(crate::sysupdate::staged_root(&d).join("v2")).unwrap();
    assert_eq!(update_phases(&d, &[]), BTreeMap::from([("system".to_string(), "downloaded".to_string())]));
    // an installed item with a newer store entry is `available`, beside the system's own phase
    let both = update_phases(&d, &["skill:notes".to_string()]);
    assert_eq!(both.get("skill:notes").map(String::as_str), Some("available"));
    assert_eq!(both.get("system").map(String::as_str), Some("downloaded"));
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn the_model_host_is_ready_with_its_model_failed_without_one_and_silent_with_no_host() {
    let none = |_: &str| None;
    assert_eq!(model_host(&[], &none), (String::new(), String::new()));
    let ids = vec!["mac-studio".to_string(), "mac-mini".to_string()];
    let only_mini = |id: &str| (id == "mac-mini").then(|| "llama".to_string());
    assert_eq!(model_host(&ids, &only_mini), ("ready".to_string(), "llama".to_string()));
    assert_eq!(model_host(&ids, &none), ("failed".to_string(), String::new()));
    let empty = |_: &str| Some(String::new());
    assert_eq!(model_host(&ids, &empty), ("failed".to_string(), String::new()), "an empty model name is no model");
}

#[test]
fn paired_computers_are_the_other_members_and_online_means_a_fresh_heartbeat() {
    let rows = vec![("mine".to_string(), "this computer".to_string(), true, None), ("desk".to_string(), "up".to_string(), false, Some(40)), ("old".to_string(), "lost".to_string(), false, None), ("odd".to_string(), "refused".to_string(), false, None), ("new".to_string(), "unknown".to_string(), false, None)];
    let p = peers_from_members(&rows);
    assert_eq!(p.keys().cloned().collect::<Vec<_>>(), ["desk", "new", "old"]);
    assert!(p["desk"].online && !p["old"].online && !p["new"].online);
    assert_eq!(p["desk"].battery, Some(40));
    assert!(p["old"].battery.is_none());
    assert!(paired_computers(&scratch("nofleet")).is_empty(), "a machine with no fleet is not given one by looking");
}

#[test]
fn quiet_hours_are_read_from_the_outside_message_policy_of_each_person() {
    use iyakki_shared::flows_channel_policy::{Policy, PolicyBook};
    let d = scratch("quiet");
    assert_eq!(quiet_now(&d, 0), None);
    let ann = d.join("flows/ann");
    std::fs::create_dir_all(&ann).unwrap();
    PolicyBook::new(&ann).set_policy(Policy { quiet: Some((22 * 60, 7 * 60)), ..Policy::default() }).unwrap();
    assert_eq!(quiet_now(&d, 23 * 3600), Some(true));
    assert_eq!(quiet_now(&d, 12 * 3600), Some(false));
    let _ = std::fs::remove_dir_all(&d);
}
