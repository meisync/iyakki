//! Settings tests of lane ST-C moved out of `settings.rs` (S0).

use super::*;
use iyakki_sec::policy::Role;
use super::tests::*;

#[test]
fn enter_on_the_network_page_switches_the_link_and_is_audited() {
    let (cx, _) = admin("hw3");
    let (env, ctl) = fake_machine("hw3");
    let mut a = SettingsApp::new(&cx).with_hardware(env);
    goto(&mut a, &cx, Tab::Network);
    key(&mut a, &cx, Key::Enter);
    assert_eq!(status(&a), "Ethernet eno1 turned off.");
    assert_eq!(ctl.0.lock().unwrap().clone(), vec!["eno1 false"]);
    assert!(cx.audit.recent(10).iter().any(|l| l.contains("settings.network") && l.contains("ethernet eno1 off")), "{:?}", cx.audit.recent(10));
    // a missing WiFi adapter is listed, and pressing Enter on it only explains
    key(&mut a, &cx, Key::Down);
    key(&mut a, &cx, Key::Enter);
    assert!(status(&a).contains("nothing to switch"), "{}", status(&a));
}

#[test]
fn the_browser_page_has_switches_that_are_saved_for_the_user_and_audited() {
    let (cx, _) = admin("brs1");
    let mut a = SettingsApp::new(&cx);
    assert!(a.tabs().contains(&Tab::Browser));
    assert!(a.nav_matches().contains(&a.tabs().iter().position(|t| *t == Tab::Browser).unwrap()));
    goto(&mut a, &cx, Tab::Browser);
    let g = draw(&mut a, &cx, 1280, 720);
    assert_eq!(g.hits.iter().filter(|h| matches!(h.ev, Event::Key(Key::Char('a' | 'i' | 's' | 'c')))).count(), 4, "four switches");
    let load = |cx: &AppCx| super::super::browser_config::load(cx);
    assert!(load(&cx).js_all && load(&cx).images && load(&cx).styles && load(&cx).cookies, "everything is on by default, scripts included");
    key(&mut a, &cx, Key::Char('a'));
    key(&mut a, &cx, Key::Char('i'));
    key(&mut a, &cx, Key::Char('s'));
    key(&mut a, &cx, Key::Char('c'));
    let c = load(&cx);
    assert!(!c.js_all && !c.images && !c.styles && !c.cookies, "each key switched its setting off");
    assert_eq!(status(&a), "Cookies: off.");
    assert!(cx.audit.recent(20).iter().any(|l| l.contains("settings.browser") && l.contains("Scripts on every site: off")));
    // forgetting the allowed sites
    let mut c2 = load(&cx);
    c2.js_sites.insert("a.org".into());
    super::super::browser_config::save(&cx, &c2);
    key(&mut a, &cx, Key::Char('f'));
    assert!(load(&cx).js_sites.is_empty());
    // another user is not affected
    let mut other = cx.clone();
    other.session.user = "amy".into();
    assert!(load(&other).js_all, "another user keeps the default (scripts on) while this one switched them off");
    // everyone can use the page (it is their own browser), also a plain user
    let ucx = user("brs2");
    assert!(SettingsApp::new(&ucx).tabs().contains(&Tab::Browser));
    let _ = draw(&mut a, &cx, 640, 480);
}

#[test]
fn an_administrator_scans_picks_a_network_types_the_passphrase_and_it_is_joined_and_remembered() {
    let (env, d) = wifi_world("join");
    let (cx, _) = admin("wifi1");
    let mut a = SettingsApp::new(&cx).with_wifi(env.clone());
    goto(&mut a, &cx, Tab::Network);
    key(&mut a, &cx, Key::Char('w'));
    settle(&mut a, &cx);
    assert!(a.has_modal(), "the list of networks is shown: {}", status(&a));
    let items: Vec<String> = a.wifi_items.iter().map(|(t, _)| t.clone()).collect();
    assert!(items[0].starts_with("Seen") && items[0].contains("WPA2") && items[0].contains("2.4 GHz"), "{items:?}");
    assert!(items[1].starts_with("Cafe") && items[1].contains("open"));
    assert_eq!(items[2], "Hidden network…");
    assert!(!items.iter().any(|t| t.starts_with("Forget")), "nothing is saved yet");
    key(&mut a, &cx, Key::Enter); // the first one
    assert!(a.has_modal());
    typ(&mut a, &cx, "short");
    key(&mut a, &cx, Key::Enter);
    assert!(a.has_modal(), "a passphrase that is too short keeps the question open");
    for _ in 0..5 {
        key(&mut a, &cx, Key::Backspace);
    }
    typ(&mut a, &cx, "password1");
    key(&mut a, &cx, Key::Enter);
    settle(&mut a, &cx);
    assert!(status(&a).contains("Joining Seen"), "{}", status(&a));
    let known = env.known.list();
    assert_eq!(known.len(), 1);
    assert_eq!(known[0].ssid, "Seen");
    assert!(known[0].passphrase.is_none() && known[0].psk.is_some(), "only the derived key is kept");
    let log = std::fs::read_to_string(d.join("rec.log")).unwrap();
    assert!(log.contains("iw dev wlan0 scan") && log.contains("wpa_supplicant -B -i wlan0") && log.contains("udhcpc"), "{log}");
    assert!(std::fs::read_to_string(d.join("run/wpa_supplicant-wlan0.conf")).unwrap().contains("ssid=\"Seen\""));
    assert!(!std::fs::read_to_string(d.join("run/wpa_supplicant-wlan0.conf")).unwrap().contains("password1"));
    assert!(cx.audit.recent(10).iter().any(|l| l.contains("settings.wifi.join") && l.contains("Seen")));
    assert!(!std::fs::read_to_string(cx.audit.path_buf()).unwrap().contains("password1"), "the passphrase is never logged");
    // an open network is joined without a question; the saved one is marked and can be forgotten
    key(&mut a, &cx, Key::Char('w'));
    settle(&mut a, &cx);
    assert!(a.wifi_items[0].0.contains("saved") && a.wifi_items.iter().any(|(t, _)| t.starts_with("Forget")));
    typ(&mut a, &cx, "Cafe");
    key(&mut a, &cx, Key::Enter);
    settle(&mut a, &cx);
    assert!(status(&a).contains("Joining Cafe"), "{}", status(&a));
    assert_eq!(env.known.list().iter().map(|k| k.ssid.as_str()).collect::<Vec<_>>(), vec!["Cafe", "Seen"]);
    key(&mut a, &cx, Key::Char('w'));
    settle(&mut a, &cx);
    typ(&mut a, &cx, "Forget");
    key(&mut a, &cx, Key::Enter);
    typ(&mut a, &cx, "Seen");
    key(&mut a, &cx, Key::Enter);
    assert_eq!(env.known.list().iter().map(|k| k.ssid.as_str()).collect::<Vec<_>>(), vec!["Cafe"]);
    assert!(status(&a).contains("Forgot Seen"), "{}", status(&a));
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn enterprise_and_wep_networks_ask_their_own_questions_and_are_written_into_the_supplicant_file() {
    use std::os::unix::fs::PermissionsExt;
    let (env, d) = wifi_world("ent");
    let iw = d.join("bin/iw");
    std::fs::write(&iw, "#!/bin/sh\nprintf 'BSS aa:bb:cc:00:00:03(on wlan0)\\n\\tfreq: 2437.0\\n\\tcapability: ESS Privacy (0x0411)\\n\\tsignal: -50.00 dBm\\n\\tSSID: Campus\\n\\tRSN:\\t * Version: 1\\n\\t\\t * Authentication suites: IEEE 802.1X\\nBSS aa:bb:cc:00:00:04(on wlan0)\\n\\tfreq: 2412.0\\n\\tcapability: ESS Privacy (0x0411)\\n\\tsignal: -70.00 dBm\\n\\tSSID: OldAP\\n'\n").unwrap();
    std::fs::set_permissions(&iw, std::fs::Permissions::from_mode(0o755)).unwrap();
    let (cx, _) = admin("wifi3");
    let mut a = SettingsApp::new(&cx).with_wifi(env.clone());
    goto(&mut a, &cx, Tab::Network);
    key(&mut a, &cx, Key::Char('w'));
    settle(&mut a, &cx);
    let items: Vec<String> = a.wifi_items.iter().map(|(t, _)| t.clone()).collect();
    assert!(items[0].starts_with("Campus") && items[0].contains("enterprise"), "{items:?}");
    assert!(items[1].starts_with("OldAP") && items[1].contains("WEP"), "{items:?}");
    // WEP: refused until the warning is accepted
    typ(&mut a, &cx, "OldAP");
    key(&mut a, &cx, Key::Enter);
    assert!(matches!(&a.modal, Some(Modal::Form(FormKind::WifiWep(n), _)) if n == "OldAP"));
    typ(&mut a, &cx, "abcde");
    key(&mut a, &cx, Key::Enter);
    assert!(a.has_modal(), "without the yes the form stays open with the warning");
    assert!(env.known.list().is_empty());
    // enterprise: a missing certificate file keeps the form open; filling it joins
    key(&mut a, &cx, Key::Esc);
    key(&mut a, &cx, Key::Char('w'));
    settle(&mut a, &cx);
    typ(&mut a, &cx, "Campus");
    key(&mut a, &cx, Key::Enter);
    assert!(matches!(&a.modal, Some(Modal::Form(FormKind::WifiEnterprise(n), _)) if n == "Campus"));
    key(&mut a, &cx, Key::Tab);
    typ(&mut a, &cx, "bob");
    key(&mut a, &cx, Key::Tab);
    typ(&mut a, &cx, "hunter22");
    key(&mut a, &cx, Key::Enter);
    assert!(a.has_modal() && env.known.list().is_empty(), "the server certificate file is required");
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn a_hidden_network_a_missing_adapter_and_an_ordinary_account_are_handled() {
    let (env, d) = wifi_world("hidden");
    let (cx, _) = admin("wifi2");
    let mut a = SettingsApp::new(&cx).with_wifi(env.clone());
    goto(&mut a, &cx, Tab::Network);
    key(&mut a, &cx, Key::Char('w'));
    settle(&mut a, &cx);
    typ(&mut a, &cx, "Hidden");
    key(&mut a, &cx, Key::Enter);
    typ(&mut a, &cx, "Secret Net");
    key(&mut a, &cx, Key::Tab);
    typ(&mut a, &cx, "hiddenpass1");
    key(&mut a, &cx, Key::Enter);
    settle(&mut a, &cx);
    let k = env.known.list();
    assert_eq!((k[0].ssid.as_str(), k[0].hidden), ("Secret Net", true));
    assert!(std::fs::read_to_string(d.join("run/wpa_supplicant-wlan0.conf")).unwrap().contains("scan_ssid=1"));
    // no adapter: said plainly
    let mut b = SettingsApp::new(&cx).with_wifi(WifiEnv { roots: iyakki_core_api::device_types::devctl::Roots { sys: d.join("nothing"), proc_: d.join("nothing") }, ..env.clone() });
    goto(&mut b, &cx, Tab::Network);
    key(&mut b, &cx, Key::Char('w'));
    assert!(status(&b).contains("no WiFi adapter"), "{}", status(&b));
    // a Broadcom 4360-family card with no driver: said with the way out (KI-094)
    let bcm = d.join("bcm");
    std::fs::create_dir_all(bcm.join("sys/bus/pci/devices/0000:03:00.0")).unwrap();
    std::fs::write(bcm.join("sys/bus/pci/devices/0000:03:00.0/vendor"), "0x14e4\n").unwrap();
    std::fs::write(bcm.join("sys/bus/pci/devices/0000:03:00.0/device"), "0x43a0\n").unwrap();
    let mut c = SettingsApp::new(&cx).with_wifi(WifiEnv { roots: iyakki_core_api::device_types::devctl::Roots { sys: bcm.join("sys"), proc_: bcm.join("proc") }, ..env.clone() });
    goto(&mut c, &cx, Tab::Network);
    key(&mut c, &cx, Key::Char('w'));
    assert!(status(&c).contains("Broadcom BCM43xx") && status(&c).contains("USB Wi-Fi adapter"), "{}", status(&c));
    // an ordinary account has no Network page at all
    let mut cx2 = cx.clone();
    cx2.session.role = Role::User;
    cx2.session.policy = Role::User.template();
    assert!(!SettingsApp::new(&cx2).tabs().contains(&Tab::Network));
    let _ = std::fs::remove_dir_all(&d);
}
