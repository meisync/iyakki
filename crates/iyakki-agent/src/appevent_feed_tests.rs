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
    assert!(update_phases(&d).is_empty());
    std::fs::create_dir_all(crate::sysupdate::staged_root(&d).join("v2")).unwrap();
    assert_eq!(update_phases(&d), BTreeMap::from([("system".to_string(), "downloaded".to_string())]));
    let _ = std::fs::remove_dir_all(&d);
}
