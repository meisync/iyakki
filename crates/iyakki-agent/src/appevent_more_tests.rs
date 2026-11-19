//! The further event sources (F-SDK-204 to 207, 211) with made-up looks: no machine, no clock, no sleeping.

use super::*;

fn show(v: &[(&'static str, Payload)]) -> Vec<String> {
    v.iter().map(|(n, p)| format!("{n} {}", p.iter().map(|(k, v)| format!("{k}={}", match v { Val::Text(t) => t.clone(), Val::Int(n) => n.to_string(), Val::Bool(b) => b.to_string() })).collect::<Vec<_>>().join(" ")).trim().to_string()).collect()
}

fn base() -> MoreFacts {
    MoreFacts { locale: "en_US".into(), resolution: "1920x1080".into(), ..Default::default() }
}

#[test]
fn the_first_look_only_remembers_and_an_unchanged_look_says_nothing() {
    let mut w = MoreWatch::default();
    assert!(w.observe(base()).is_empty());
    assert!(w.observe(base()).is_empty());
}

#[test]
fn metered_captive_and_back_online_are_told_once_each() {
    let mut w = MoreWatch::default();
    w.observe(base());
    let mut f = base();
    f.metered = true;
    assert_eq!(show(&w.observe(f.clone())), ["connectivity.changed state=metered"]);
    assert!(w.observe(f.clone()).is_empty());
    f.captive = true;
    assert_eq!(show(&w.observe(f.clone())), ["connectivity.changed state=captive_portal"]);
    assert_eq!(show(&w.observe(base())), ["connectivity.changed state=online"]);
}

#[test]
fn wake_shutdown_and_quiet_hours() {
    let mut w = MoreWatch::default();
    w.observe(MoreFacts { quiet: Some(false), ..base() });
    let f = MoreFacts { suspends: 1, quiet: Some(true), shutdown_in: Some(60), ..base() };
    assert_eq!(show(&w.observe(f.clone())), ["power.shutdown_pending in_seconds=60", "power.sleep phase=wake", "quiet_hours.changed state=on"]);
    assert!(w.observe(MoreFacts { shutdown_in: Some(30), ..f.clone() }).is_empty(), "a pending shutdown is told once");
    w.observe(MoreFacts { shutdown_in: None, ..f.clone() });
    assert_eq!(show(&w.observe(MoreFacts { shutdown_in: Some(10), ..f })), ["power.shutdown_pending in_seconds=10"]);
    // a machine with no quiet-hours setting says nothing about it
    let mut w = MoreWatch::default();
    w.observe(base());
    assert!(w.observe(MoreFacts { quiet: Some(true), ..base() }).is_empty());
}

#[test]
fn paired_computers_come_go_and_report_their_battery() {
    let mut w = MoreWatch::default();
    w.observe(base());
    let mut f = base();
    f.peers.insert("desk".into(), PeerFact { online: true, battery: Some(80) });
    assert_eq!(show(&w.observe(f.clone())), ["peers.changed change=paired device=desk"]);
    f.peers.insert("desk".into(), PeerFact { online: false, battery: Some(79) });
    assert_eq!(show(&w.observe(f.clone())), ["peers.changed change=offline device=desk", "peers.battery device=desk percent=79"]);
    assert_eq!(show(&w.observe(base())), ["peers.changed change=forgotten device=desk"]);
}

#[test]
fn low_space_is_told_when_it_falls_and_again_only_after_recovering() {
    let mut w = MoreWatch::default();
    let mut f = base();
    f.free_percent.insert("/data".into(), 40);
    w.observe(f.clone());
    f.free_percent.insert("/data".into(), 4);
    assert_eq!(show(&w.observe(f.clone())), ["storage.volume change=low_space volume=/data"]);
    f.free_percent.insert("/data".into(), 3);
    assert!(w.observe(f.clone()).is_empty());
    f.free_percent.insert("/data".into(), 50);
    w.observe(f.clone());
    f.free_percent.insert("/data".into(), 2);
    assert_eq!(show(&w.observe(f)), ["storage.volume change=low_space volume=/data"]);
}

#[test]
fn disks_bluetooth_and_printers() {
    let mut w = MoreWatch::default();
    let mut f = base();
    f.disk_ok.insert("nvme0".into(), true);
    w.observe(f.clone());
    f.disk_ok.insert("nvme0".into(), false);
    f.bluetooth.insert("hci0:11".into());
    f.printers.insert(("printer".into(), "lp0".into()));
    assert_eq!(show(&w.observe(f.clone())), ["disk.health disk=nvme0 state=warning", "bluetooth.device change=connected device=hci0:11", "printer.changed change=added device=lp0 kind=printer"]);
    assert_eq!(show(&w.observe(base())), ["bluetooth.device change=disconnected device=hci0:11", "printer.changed change=removed device=lp0 kind=printer"]);
}

#[test]
fn language_resolution_updates_model_host_and_the_wake_phrase() {
    let mut w = MoreWatch::default();
    let mut f = base();
    f.model = ("ready".into(), "small".into());
    w.observe(f.clone());
    f.locale = "ta_IN".into();
    f.resolution = "3840x2160".into();
    f.updates.insert("kernel".into(), "available".into());
    f.model = ("ready".into(), "large".into());
    f.wake_heard = 1;
    assert_eq!(show(&w.observe(f.clone())), ["locale.changed locale=ta_IN", "display.changed what=resolution", "updates.changed component=kernel phase=available", "models.host model=large state=changed", "voice.wake_heard"]);
    f.updates.insert("kernel".into(), "downloaded".into());
    f.model = ("failed".into(), "large".into());
    assert_eq!(show(&w.observe(f)), ["updates.changed component=kernel phase=downloaded", "models.host model=large state=failed"]);
}

#[test]
fn a_certificate_is_told_at_each_threshold_once() {
    let mut w = MoreWatch::default();
    let mut f = base();
    f.certs.insert("tls".into(), 90);
    assert!(w.observe(f.clone()).is_empty());
    f.certs.insert("tls".into(), 29);
    assert_eq!(show(&w.observe(f.clone())), ["cert.expiring days=29 what=tls"]);
    f.certs.insert("tls".into(), 20);
    assert!(w.observe(f.clone()).is_empty());
    f.certs.insert("tls".into(), 6);
    assert_eq!(show(&w.observe(f.clone())), ["cert.expiring days=6 what=tls"]);
    f.certs.insert("tls".into(), -2);
    assert_eq!(show(&w.observe(f.clone())), ["cert.expiring days=0 what=tls"]);
    // renewed, then close again: told again
    f.certs.insert("tls".into(), 365);
    w.observe(f.clone());
    f.certs.insert("tls".into(), 25);
    assert_eq!(show(&w.observe(f)), ["cert.expiring days=25 what=tls"]);
}

#[test]
fn approvals_security_counts_and_reminders_per_person() {
    let mut w = MorePersonWatch::default();
    assert!(w.observe("pat", MorePersonFacts::default()).is_empty());
    let f = MorePersonFacts { approvals: 2, failed_signins: 3, reminders_due: ["Call".to_string()].into(), ..Default::default() };
    assert_eq!(show(&w.observe("pat", f.clone())), ["approvals.pending count=2", "security.alert count=3 kind=failed_signin", "reminder.due title=Call"]);
    assert!(w.observe("pat", f.clone()).is_empty(), "a reminder that stays due is told once");
    assert!(w.observe("sam", f).is_empty(), "another person's first look only remembers");
}

#[test]
fn the_plain_file_readers() {
    let d = std::fs::canonicalize(std::env::temp_dir()).unwrap().join(format!("iyakki-more-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    for p in ["class/nvme/nvme0", "class/bluetooth/hci0:11", "class/bluetooth/hci0", "class/usbmisc/lp0", "class/usbmisc/hiddev0", "class/drm/card0-HDMI-A-1", "power/suspend_stats", "block/sda/device"] {
        std::fs::create_dir_all(d.join(p)).unwrap();
    }
    std::fs::write(d.join("class/nvme/nvme0/state"), "dead\n").unwrap();
    std::fs::write(d.join("block/sda/device/state"), "running\n").unwrap();
    std::fs::write(d.join("class/drm/card0-HDMI-A-1/status"), "connected\n").unwrap();
    std::fs::write(d.join("class/drm/card0-HDMI-A-1/modes"), "2560x1440\n1920x1080\n").unwrap();
    std::fs::write(d.join("power/suspend_stats/success"), "7\n").unwrap();
    assert_eq!(disk_states(&d), BTreeMap::from([("nvme0".to_string(), false), ("sda".to_string(), true)]));
    assert_eq!(bluetooth_links(&d), BTreeSet::from(["hci0:11".to_string()]));
    assert_eq!(printers_and_scanners(&d), BTreeSet::from([("printer".to_string(), "lp0".to_string())]));
    assert_eq!(screen_resolution(&d), "2560x1440");
    assert_eq!(suspend_count(&d), 7);
    assert_eq!(suspend_count(&d.join("none")), 0);
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn shutdown_and_certificate_arithmetic() {
    assert_eq!(shutdown_in("USEC=1000000060000000\nMODE=poweroff\n", 1_000_000_000), Some(60));
    assert_eq!(shutdown_in("USEC=5000000\n", 1_000_000_000), Some(0));
    assert_eq!(shutdown_in("MODE=x\n", 0), None);
    assert_eq!(days_left(86_400 * 10 + 5, 0), 10);
    assert_eq!(days_left(0, 86_400), -1);
}

#[test]
fn the_free_percent_of_a_real_mount_is_a_percentage() {
    let t = std::fs::canonicalize(std::env::temp_dir()).unwrap();
    let text = format!("/dev/fake {} ext4 rw 0 0\n", t.display());
    for (_, p) in free_percents(&text) {
        assert!(p <= 100);
    }
    assert!(free_percents("tmpfs /tmp tmpfs rw 0 0\n").is_empty());
}

#[test]
fn a_calendar_entry_is_due_from_its_reminder_until_it_ends_and_the_machine_language_has_one_source() {
    use iyakki_shared::calendar::{Occurrence, Repeat};
    let occ = |title: &str, start: i64, end: i64, remind: Option<u32>| Occurrence { id: "e".into(), title: title.into(), start, end, all_day: false, repeat: Repeat::None, notes: String::new(), by: "pat".into(), remind };
    let list = [occ("Dentist", 10_000, 12_000, Some(30)), occ("Call", 20_000, 20_000, None), occ("Done", 1_000, 2_000, None)];
    assert!(reminders_due_now(&list, 8_000).is_empty());
    assert_eq!(reminders_due_now(&list, 8_200), ["Dentist".to_string()].into());
    assert!(reminders_due_now(&list, 12_000).is_empty(), "over once it ends");
    assert_eq!(reminders_due_now(&list, 20_100), ["Call".to_string()].into(), "no length: five minutes");
    assert!(reminders_due_now(&list, 20_300).is_empty());
    let d = std::fs::canonicalize(std::env::temp_dir()).unwrap().join(format!("iyakki-locale-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    assert_eq!(machine_locale(&d, "", "en_US.UTF-8"), "en_US.UTF-8");
    assert_eq!(machine_locale(&d, "de_DE.UTF-8", "en_US.UTF-8"), "de_DE.UTF-8");
    std::fs::write(d.join("locale"), "ta_IN.UTF-8\n").unwrap();
    assert_eq!(machine_locale(&d, "de_DE.UTF-8", "en_US.UTF-8"), "ta_IN.UTF-8", "the owner's file wins");
    assert_eq!(machine_locale(&d.join("none"), "", ""), "");
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn a_backup_made_by_hand_is_counted_and_told_once_by_its_file_name() {
    let d = std::fs::canonicalize(std::env::temp_dir()).unwrap().join(format!("iyakki-bkmade-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    assert_eq!(backups_made(&d, "pat"), (0, (String::new(), false)));
    let look = |n: u64, last: (String, bool)| MorePersonFacts { backups: n, backup_last: last, ..Default::default() };
    let mut w = MorePersonWatch::default();
    let (n, last) = backups_made(&d, "pat");
    assert!(w.observe("pat", look(n, last)).is_empty());
    note_backup_made(&d, "pat", "backup-1.iyb", true);
    let (n, last) = backups_made(&d, "pat");
    assert_eq!((n, last.0.as_str()), (1, "backup-1.iyb"));
    assert_eq!(show(&w.observe("pat", look(n, last.clone()))), ["backup.finished name=backup-1.iyb outcome=finished"]);
    assert!(w.observe("pat", look(n, last)).is_empty());
    note_backup_made(&d, "pat", "backup", false);
    let (n, last) = backups_made(&d, "pat");
    assert_eq!(show(&w.observe("pat", look(n, last))), ["backup.finished name=backup outcome=failed"]);
    assert_eq!(backups_made(&d, "sam").0, 0, "another person's count is apart");
    note_backup_made(&d, "../x", "f", true);
    assert!(std::fs::read_dir(&d).unwrap().flatten().all(|e| e.file_name().to_string_lossy().starts_with("backups-made-pat")), "a bad name writes nothing");
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn a_connected_bluetooth_device_is_told_by_its_paired_name_and_never_by_its_address() {
    let store = iyakki_devctl::bthid::Store::parse(r#"{"devices":[{"address":"AA:BB:CC:00:11:22","addr_type":0,"name":"Desk Keys","kind":"keyboard"},{"address":"AA:BB:CC:00:11:33","addr_type":0,"name":" ","kind":"other"}]}"#);
    let got = bluetooth_names(&["AA:BB:CC:00:11:22".to_string(), "AA:BB:CC:00:11:33".to_string(), "11:22:33:44:55:66".to_string()], &store);
    assert_eq!(got, ["Desk Keys".to_string(), "device".to_string()].into());
    assert!(bluetooth_names(&[], &store).is_empty());
    assert!(bluetooth_connected(std::path::Path::new("/nonexistent"), std::path::Path::new("/nonexistent")).is_empty(), "no kernel links, nothing to ask");
}
