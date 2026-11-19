use super::*;

fn facts() -> ExtraFacts {
    ExtraFacts { zone: "Europe/Paris".into(), ..Default::default() }
}

fn names(v: &[Out]) -> Vec<String> {
    v.iter().map(|(n, _, p)| format!("{n}:{}", p.values().filter_map(Val::text).collect::<Vec<_>>().join("/"))).collect()
}

#[test]
fn a_usb_device_plugged_and_taken_out_is_told_by_name_and_the_first_look_only_remembers() {
    let mut w = ExtraWatch::default();
    let mut f = facts();
    f.usb.insert("Keyboard (046d:c31c)".into());
    assert!(w.observe(f.clone()).is_empty());
    f.usb.insert("Disk (0781:5581)".into());
    assert_eq!(names(&w.observe(f.clone())), ["usb.device:added/Disk (0781:5581)"]);
    f.usb.remove("Keyboard (046d:c31c)");
    assert_eq!(names(&w.observe(f.clone())), ["usb.device:removed/Keyboard (046d:c31c)"]);
    assert!(w.observe(f).is_empty());
}

#[test]
fn the_screen_shown_to_a_client_and_the_clipboard_used_are_signs_and_carry_no_content() {
    let mut w = ExtraWatch::default();
    let mut f = facts();
    w.observe(f.clone());
    f.screen_clients = 1;
    assert_eq!(names(&w.observe(f.clone())), ["sharing.changed:screen/started"]);
    f.screen_clients = 2;
    assert!(w.observe(f.clone()).is_empty(), "a second client is not a new start");
    f.clipboard_uses = 3;
    assert_eq!(names(&w.observe(f.clone())), ["sharing.changed:clipboard/used"]);
    f.screen_clients = 0;
    assert_eq!(names(&w.observe(f)), ["sharing.changed:screen/stopped"]);
    // the counters the client session moves
    let (s0, c0) = sharing_now();
    screen_client_opened();
    clipboard_used();
    let (s1, c1) = sharing_now();
    assert!(s1 >= 1 && c1 > c0 && s1 > s0.saturating_sub(1));
    screen_client_closed();
}

#[test]
fn the_zone_the_network_chose_is_told_only_when_the_person_let_it_and_it_changed() {
    let mut w = ExtraWatch::default();
    let mut f = facts();
    w.observe(f.clone());
    f.zone = "Asia/Tokyo".into();
    assert!(w.observe(f.clone()).is_empty(), "a zone the person chose by hand is clock.changed, not a place from the network");
    f.zone_auto = true;
    f.zone = "Europe/Berlin".into();
    assert_eq!(names(&w.observe(f.clone())), ["location.changed:Europe/Berlin"]);
    assert!(w.observe(f.clone()).is_empty());
    f.zone_auto = false;
    f.zone = "Europe/Rome".into();
    assert!(w.observe(f).is_empty());
}

#[test]
fn an_apps_storage_is_told_near_full_full_and_ok_again_once_each_for_that_person_only() {
    let mut w = ExtraWatch::default();
    let key = ("pat".to_string(), "notes".to_string());
    let mut f = facts();
    f.quotas.insert(key.clone(), (10, 100));
    assert!(w.observe(f.clone()).is_empty(), "plenty of room: nothing to say");
    let mut states = Vec::new();
    for used in [85, 91, 95, 100, 120, 85, 79, 60] {
        f.quotas.insert(key.clone(), (used, 100));
        for (n, who, p) in w.observe(f.clone()) {
            assert_eq!((n, who.as_deref()), ("storage.quota", Some("pat")));
            states.push((used, p["state"].text().unwrap().to_string()));
        }
    }
    assert_eq!(states, [(91, "near_full".to_string()), (100, "full".into()), (85, "near_full".into()), (79, "ok".into())], "{states:?}");
    assert_eq!(quota_state(5, 0, "ok"), ("ok", 0), "no allowance means no warning");
}

#[test]
fn the_real_readers_leave_hubs_interfaces_and_serial_numbers_out_and_count_only_the_apps_own_files() {
    let d = std::env::temp_dir().join(format!("iyakki-extra-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    let dev = |name: &str, class: &str, product: &str| {
        let p = d.join("bus/usb/devices").join(name);
        std::fs::create_dir_all(&p).unwrap();
        std::fs::write(p.join("bDeviceClass"), format!("{class}\n")).unwrap();
        std::fs::write(p.join("idVendor"), "0781\n").unwrap();
        std::fs::write(p.join("idProduct"), "5581\n").unwrap();
        std::fs::write(p.join("product"), format!("{product}\n")).unwrap();
        std::fs::write(p.join("serial"), "SECRET-SERIAL\n").unwrap();
    };
    dev("1-2", "00", "Cruzer");
    dev("1-3", "09", "A Hub");
    dev("1-2:1.0", "08", "interface");
    dev("usb1", "09", "root");
    let u = usb_devices(&d);
    assert_eq!(u.into_iter().collect::<Vec<_>>(), ["Cruzer (0781:5581)"]);
    let files = d.join("files");
    std::fs::create_dir_all(&files).unwrap();
    std::fs::write(files.join("notes.json"), vec![0u8; 100]).unwrap();
    std::fs::write(files.join("notes.sdk2.json"), vec![0u8; 50]).unwrap();
    std::fs::write(files.join("notesy.json"), vec![0u8; 999]).unwrap();
    std::fs::write(files.join("other.json"), vec![0u8; 7]).unwrap();
    assert_eq!(app_bytes(&files, "notes"), 150);
    let _ = std::fs::remove_dir_all(&d);
}
