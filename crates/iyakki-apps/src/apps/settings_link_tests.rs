//! Settings -> Network: the sign-in page button and the choice between two WiFi adapters.
use super::*;
use crate::apps::tests::{cx_with_admin_store, temp_dir};

fn two_adapters(tag: &str) -> (WifiEnv, std::path::PathBuf) {
    let d = std::env::temp_dir().join(format!("iyakki-ui-link-{}-{tag}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    for n in ["wlan0", "wlan1"] {
        let net = d.join("sys/class/net").join(n);
        std::fs::create_dir_all(net.join("device")).unwrap();
        std::fs::create_dir_all(net.join("wireless")).unwrap();
        std::fs::write(net.join("type"), "1\n").unwrap();
    }
    std::fs::create_dir_all(d.join("proc")).unwrap();
    let mut env = WifiEnv::real();
    env.known = Arc::new(iyakki_core_api::device_types::devctl::wifi::KnownStore::new(&d.join("wifi.json")));
    env.roots = iyakki_core_api::device_types::devctl::Roots { sys: d.join("sys"), proc_: d.join("proc") };
    (env, d)
}

fn status(a: &SettingsApp) -> String {
    a.status().map(|s| s.0.clone()).unwrap_or_default()
}

#[test]
fn with_two_adapters_the_v_key_switches_and_the_choice_is_remembered_and_read_back() {
    let cx = cx_with_admin_store("link-adapters");
    let (env, d) = two_adapters("two");
    let mut a = SettingsApp::new(&cx).with_wifi(env.clone());
    a.go_to(Tab::Network);
    assert_eq!(a.wifi.adapters(), ["wlan0", "wlan1"]);
    assert_eq!(a.wifi.iface().as_deref(), Some("wlan0"), "the first until the person chooses");
    a.handle(&Event::Key(Key::Char('v')), &cx);
    assert_eq!(a.wifi.iface().as_deref(), Some("wlan1"), "{}", status(&a));
    assert!(status(&a).contains("wlan1"));
    assert_eq!(env.known.chosen_adapter().as_deref(), Some("wlan1"), "kept beside the saved networks");
    // another page (and the chat tools, which read the same file) see the choice
    let mut b = SettingsApp::new(&cx).with_wifi(env.clone());
    assert_eq!(b.wifi.iface().as_deref(), Some("wlan1"));
    // a choice that is no longer there falls back to the first adapter
    std::fs::remove_dir_all(d.join("sys/class/net/wlan1")).unwrap();
    assert_eq!(b.wifi.iface().as_deref(), Some("wlan0"));
    // with one adapter there is nothing to switch
    b.go_to(Tab::Network);
    b.handle(&Event::Key(Key::Char('v')), &cx);
    assert!(status(&b).contains("nothing to switch"), "{}", status(&b));
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn the_o_key_opens_only_a_sign_in_address_the_check_found() {
    let cx = cx_with_admin_store("link-signin");
    let mut a = SettingsApp::new(&cx);
    a.go_to(Tab::Network);
    a.handle(&Event::Key(Key::Char('o')), &cx);
    assert!(status(&a).contains("not asked for a sign-in"), "{}", status(&a));
    assert!(a.take_request().is_none(), "nothing opens without a found sign-in page");
    // a found page: it goes to the Browser tile
    a.link = iyakki_shared::wifilink::LinkWatch::with_portal_for_test("http://portal.example/login");
    a.handle(&Event::Key(Key::Char('o')), &cx);
    assert_eq!(a.take_request(), Some(("browser".to_string(), "http://portal.example/login".to_string())));
    // a scheme that is not http(s) is never opened
    a.link = iyakki_shared::wifilink::LinkWatch::with_portal_for_test("javascript:alert(1)");
    a.handle(&Event::Key(Key::Char('o')), &cx);
    assert!(a.take_request().is_none());
    let _ = temp_dir("unused-link");
}
