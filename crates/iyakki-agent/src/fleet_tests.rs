//! The pure part of the fleet (F-FLEET-01, F-FLEET-02): records, signatures, key rotation, codes and heartbeat figures.

use crate::cloudkeys::MemberKeys;
use crate::fleet::*;

fn keys(n: u8) -> MemberKeys {
    MemberKeys::from_seed([n; 32])
}

fn three() -> (Record, MemberKeys, MemberKeys, MemberKeys) {
    let (a, b, c) = (keys(1), keys(2), keys(3));
    let (r, k) = Record::create(&a, "alpha", "lab").unwrap();
    let r = r.with_member(&a, &k, Member::of(&b, "bravo", false)).unwrap();
    let r = r.with_member(&a, &k, Member::of(&c, "charlie", false)).unwrap();
    (r, a, b, c)
}

#[test]
fn a_new_fleet_is_signed_by_its_creator_and_every_member_opens_the_same_group_key() {
    let (r, a, b, c) = three();
    r.verify_self().unwrap();
    assert_eq!((r.version, r.generation, r.members.len()), (3, 0, 3));
    let seal = r.open_key(&a).unwrap().seal(&r.fleet, "heartbeat", 0, b"x").unwrap();
    for m in [&b, &c] {
        assert_eq!(r.open_key(m).unwrap().open(&r.fleet, "heartbeat", 0, &seal).unwrap(), b"x");
    }
    assert!(r.open_key(&keys(9)).is_err(), "a stranger has no wrap");
}

#[test]
fn a_record_that_was_changed_or_signed_by_a_plain_member_or_a_stranger_is_refused() {
    let (r, a, b, _c) = three();
    let mut t = r.clone();
    t.name = "other".into();
    assert!(t.verify_self().unwrap_err().contains("signature"));
    let mut t = r.clone();
    t.members[1].admin = true; // tampering with who is an administrator
    assert!(t.verify_self().is_err());
    // a plain member cannot make a version
    assert!(r.with_member(&b, &r.open_key(&b).unwrap(), Member::of(&keys(7), "x", false)).unwrap_err().contains("administrator"));
    assert!(r.without_member(&b, &a.public().sign).is_err());
    // a record signed by somebody who is not in the one we hold is not accepted
    let (o, k) = Record::create(&keys(8), "mallory", "lab").unwrap();
    let forged = Record { fleet: r.fleet.clone(), version: 9, ..o.clone() };
    assert!(forged.accept_over(&r).is_err());
    let _ = k;
}

#[test]
fn an_older_or_equal_version_and_another_fleet_are_not_accepted_and_a_newer_one_from_an_administrator_is() {
    let (r, a, _b, _c) = three();
    let k = r.open_key(&a).unwrap();
    let next = r.with_member(&a, &k, Member::of(&keys(4), "delta", false)).unwrap();
    next.accept_over(&r).unwrap();
    assert!(r.accept_over(&next).unwrap_err().contains("not newer"));
    assert!(next.accept_over(&next).is_err());
    let (other, _) = Record::create(&a, "alpha", "other").unwrap();
    assert!(other.accept_over(&r).unwrap_err().contains("another fleet"));
}

#[test]
fn removing_a_member_rotates_the_group_key_so_the_old_key_and_the_removed_member_open_nothing() {
    let (r, a, b, c) = three();
    let old_key = r.open_key(&b).unwrap();
    let (r2, new_key) = r.without_member(&a, &b.public().sign).unwrap();
    r2.verify_self().unwrap();
    r2.accept_over(&r).unwrap();
    assert_eq!((r2.version, r2.generation, r2.members.len()), (4, 1, 2));
    assert!(r2.open_key(&b).is_err(), "the removed member has no wrap in the new record");
    let sealed_new = new_key.seal(&r2.fleet, "assign", 1, b"later work").unwrap();
    assert!(old_key.open(&r2.fleet, "assign", 1, &sealed_new).is_err(), "the old key opens nothing of the new generation");
    assert_eq!(r2.open_key(&c).unwrap().open(&r2.fleet, "assign", 1, &sealed_new).unwrap(), b"later work");
    // the removed member's own copy of the old record still verifies, but a version signed by it is not accepted by a member that holds the new one
    let from_b = r.with_member(&a, &r.open_key(&a).unwrap(), Member::of(&keys(5), "e", false)).unwrap();
    assert!(from_b.accept_over(&r2).is_err());
}

#[test]
fn the_last_administrator_cannot_be_removed_or_demoted_and_admin_changes_are_signed_versions() {
    let (r, a, b, _c) = three();
    assert!(r.without_member(&a, &a.public().sign).unwrap_err().contains("last administrator"));
    assert!(r.with_admin(&a, &a.public().sign, false).unwrap_err().contains("at least one"));
    let r2 = r.with_admin(&a, &b.public().sign, true).unwrap();
    r2.accept_over(&r).unwrap();
    assert_eq!(r2.admins().len(), 2);
    // now b can sign a removal and a may be removed
    let (r3, _) = r2.without_member(&b, &a.public().sign).unwrap();
    r3.accept_over(&r2).unwrap();
    assert!(r.with_admin(&a, &b.public().sign, false).is_err(), "nothing changes");
}

#[test]
fn members_are_found_by_id_prefix_fingerprint_or_a_unique_name() {
    let (r, a, b, _c) = three();
    assert_eq!(r.find("bravo").unwrap().id, b.public().sign);
    assert_eq!(r.find(&fingerprint(&a.public().sign)).unwrap().name, "alpha");
    assert_eq!(r.find(&b.public().sign[..8]).unwrap().name, "bravo");
    assert!(r.find("zed").is_err() && r.find("").is_err() && r.find("ab").is_err());
}

#[test]
fn join_codes_are_nine_digits_compare_by_hash_and_ignore_the_dashes() {
    let c = new_code().unwrap();
    assert_eq!(c.len(), 9);
    assert!(c.chars().all(|x| x.is_ascii_digit()));
    assert_eq!(normalise_code(&show_code(&c)), c);
    assert!(same_hash(&code_hash("f", &c), &code_hash("f", &show_code(&c))));
    assert!(!same_hash(&code_hash("f", &c), &code_hash("g", &c)), "bound to the fleet");
    assert!(!same_hash(&code_hash("f", "123456789"), &code_hash("f", "123456780")));
    assert_ne!(new_code().unwrap(), new_code().unwrap());
}

#[test]
fn a_heartbeat_keeps_only_figures_and_plain_model_names() {
    let h = Heartbeat { disk_free_mb: 5, disk_total_mb: 10, cpu_load_pct: 99999, cpus: 4, mem_used_pct: 400, mem_total_mb: 8000, tasks_running: 2, models: vec!["openai/gpt-x".into(), "/home/person/secret model".into(), "gpt-x".into(), "".into()] }.sanitised();
    assert_eq!((h.cpu_load_pct, h.mem_used_pct), (1000, 100));
    assert_eq!(h.models, vec!["gpt-x".to_string(), "secretmodel".to_string()], "the folder part is cut off and odd characters dropped, duplicates removed");
    assert!(h.line().contains("2 task(s)"));
    let json = serde_json::to_string(&h).unwrap();
    assert!(!json.contains("/home"));
}

#[test]
fn the_figures_come_through_the_core_api() {
    use iyakki_core_fake::{Canned, FakeOs};
    use iyakki_core_api::{LoadAvg, MemInfo};
    let mut c = Canned::default();
    c.meminfo = Some(MemInfo { total_kb: 8 * 1024 * 1024, free_kb: 1, available_kb: 2 * 1024 * 1024, swap_total_kb: 0, swap_free_kb: 0 });
    c.loadavg = Some(LoadAvg { one: 2.0, five: 0.0, fifteen: 0.0 });
    c.cpu_count = Some(4);
    c.space = Some((50 * 1024 * 1024 * 1024, 100 * 1024 * 1024 * 1024));
    let os: std::sync::Arc<dyn iyakki_core_api::Os> = std::sync::Arc::new(FakeOs::with(c));
    let src = OsSource { os, data_dir: std::path::PathBuf::from("/x"), tasks: std::sync::Arc::new(|| 3), models: std::sync::Arc::new(|| vec!["p/m1".into()]) };
    let h = src.read();
    assert_eq!((h.disk_free_mb, h.disk_total_mb, h.cpus, h.cpu_load_pct, h.mem_used_pct, h.mem_total_mb, h.tasks_running), (51200, 102400, 4, 50, 75, 8192, 3));
    assert_eq!(h.models, vec!["m1".to_string()]);
}

#[test]
fn the_figures_of_the_real_machine_through_the_local_core_are_plausible() {
    let os: std::sync::Arc<dyn iyakki_core_api::Os> = std::sync::Arc::new(iyakki_core_local::LocalOs::system());
    let src = OsSource { os, data_dir: std::env::temp_dir(), tasks: std::sync::Arc::new(|| 0), models: std::sync::Arc::new(Vec::new) };
    let h = src.read();
    assert!(h.cpus >= 1 && h.mem_total_mb > 0 && h.disk_total_mb > 0 && h.mem_used_pct <= 100, "{h:?}");
}
