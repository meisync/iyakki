use std::os::unix::fs::PermissionsExt;
use std::sync::Arc;

use serde_json::json;

use super::*;
use crate::agent::Tools;
use crate::settingstools_tests::{assert_policy, rig, tools};

fn world(tag: &str, scan: &str) -> (WifiCtl, std::path::PathBuf) {
    let d = std::env::temp_dir().join(format!("iyakki-wifitools-{}-{tag}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    let net = d.join("sys/class/net/wlan0");
    std::fs::create_dir_all(net.join("device")).unwrap();
    std::fs::create_dir_all(net.join("wireless")).unwrap();
    std::fs::write(net.join("type"), "1\n").unwrap();
    std::fs::create_dir_all(d.join("proc")).unwrap();
    let bin = d.join("bin");
    std::fs::create_dir_all(&bin).unwrap();
    let rec = d.join("rec.log");
    let script = |name: &str, body: &str| {
        let p = bin.join(name);
        std::fs::write(&p, format!("#!/bin/sh\necho \"{name} $*\" >> {}\n{body}\n", rec.display())).unwrap();
        std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o755)).unwrap();
    };
    script("ip", "exit 0");
    script("iw", &format!("printf '{scan}'"));
    script("wpa_cli", "exit 1");
    script("wpa_supplicant", "exit 0");
    script("udhcpc", "exit 0");
    let roots = iyakki_core_local::devctl::Roots { sys: d.join("sys"), proc_: d.join("proc") };
    let mut mac = MacEnv::real();
    mac.roots = roots.clone();
    mac.host_managed = true;
    mac.known_wifi = d.join("wifi.json");
    mac.secret_path = d.join("mac-secret");
    let c = WifiCtl { wifi: Arc::new(Wifi { run_dir: d.join("run"), path: Some(bin) }), known: Arc::new(KnownStore::new(&d.join("wifi.json"))), roots, mac };
    (c, d)
}

const SCAN: &str = "BSS aa:bb:cc:00:00:01(on wlan0)\\n\\tfreq: 2437.0\\n\\tcapability: ESS Privacy (0x0411)\\n\\tsignal: -50.00 dBm\\n\\tSSID: Seen\\n\\tRSN:\\t * Version: 1\\n\\t\\t * Authentication suites: PSK\\nBSS aa:bb:cc:00:00:02(on wlan0)\\n\\tfreq: 2412.0\\n\\tcapability: ESS (0x0401)\\n\\tsignal: -70.00 dBm\\n\\tSSID: Cafe\\nBSS aa:bb:cc:00:00:03(on wlan0)\\n\\tfreq: 2437.0\\n\\tcapability: ESS Privacy (0x0411)\\n\\tsignal: -50.00 dBm\\n\\tSSID: Campus\\n\\tRSN:\\t * Version: 1\\n\\t\\t * Authentication suites: IEEE 802.1X\\nBSS aa:bb:cc:00:00:04(on wlan0)\\n\\tfreq: 2412.0\\n\\tcapability: ESS Privacy (0x0411)\\n\\tsignal: -70.00 dBm\\n\\tSSID: OldAP\\n";

fn reader() -> SecretReader {
    Arc::new(|e: &str| match e {
        "wifi-pass" => Ok("password1".to_string()),
        "wep-key" => Ok("abcde".to_string()),
        "campus-pw" => Ok("hunter22".to_string()),
        _ => Err(format!("there is no usable vault entry \"{e}\" (the vault may be locked)")),
    })
}

#[test]
fn wifi_tools_ask_every_time_and_join_with_secrets_only_from_the_vault() {
    let r = rig("wifi", true);
    let (c, d) = world("join", SCAN);
    let t = tools(&r, entries(&r.env, c.clone(), reader()));
    assert_policy(&t, &["wifi_status", "wifi_scan", "hotspot_status"], &["wifi_join", "wifi_forget", "hotspot_set"]);
    let scan = t.call("wifi_scan", &json!({})).unwrap();
    assert!(scan.contains("Seen (WPA2") && scan.contains("Cafe (open") && scan.contains("Campus (enterprise") && scan.contains("OldAP (WEP"), "{scan}");
    // an open network needs nothing; a protected one needs a vault entry and never takes the secret as text
    assert!(t.call("wifi_join", &json!({"ssid": "Cafe"})).unwrap().contains("Joining Cafe"));
    assert!(t.call("wifi_join", &json!({"ssid": "Seen"})).unwrap_err().contains("vault_request"));
    assert!(t.call("wifi_join", &json!({"ssid": "Seen", "secret_entry": "nope"})).unwrap_err().contains("vault"));
    assert!(t.call("wifi_join", &json!({"ssid": "Seen", "secret_entry": "wifi-pass"})).unwrap().contains("Joining Seen"));
    let known = c.known.list();
    assert_eq!(known.iter().map(|k| k.ssid.as_str()).collect::<Vec<_>>(), ["Seen", "Cafe"]);
    assert!(known[0].psk.is_some() && known[0].passphrase.is_none());
    // WEP only after the warning is accepted
    let e = t.call("wifi_join", &json!({"ssid": "OldAP", "secret_entry": "wep-key"})).unwrap_err();
    assert!(e.contains("broken") && e.contains("accept_wep_risk"), "{e}");
    assert!(t.call("wifi_join", &json!({"ssid": "OldAP", "secret_entry": "wep-key", "accept_wep_risk": true})).is_ok());
    // enterprise: the certificate file is required, then it joins and is written for the supplicant
    assert!(t.call("wifi_join", &json!({"ssid": "Campus", "secret_entry": "campus-pw", "identity": "bob"})).unwrap_err().contains("certificate"));
    assert!(t.call("wifi_join", &json!({"ssid": "Campus", "secret_entry": "campus-pw", "identity": "bob", "ca_cert": "/etc/ca.pem"})).is_ok());
    let conf = std::fs::read_to_string(d.join("run/wpa_supplicant-wlan0.conf")).unwrap();
    assert!(conf.contains("eap=PEAP") && conf.contains("wep_key0=\"abcde\"") && !conf.contains("hunter22") && !conf.contains("password1"));
    // a saved network is joined again by name alone; the secret never reaches the audit lines
    assert!(t.call("wifi_join", &json!({"ssid": "Seen"})).is_ok());
    let audits = r.audits.lock().unwrap().join("\n");
    assert!(audits.contains("settings.wifi.join Seen") && !audits.contains("password1") && !audits.contains("hunter22") && !audits.contains("abcde"));
    let st = t.call("wifi_status", &json!({})).unwrap();
    assert!(st.contains("wlan0") && st.contains("Campus (enterprise)") && st.contains("OldAP (WEP)") && st.contains("Seen (WPA/WPA2)"), "{st}");
    assert!(t.call("wifi_forget", &json!({"ssid": "Cafe"})).unwrap().contains("Forgot"));
    assert!(t.call("wifi_forget", &json!({"ssid": "Cafe"})).is_err());
    assert!(t.call("wifi_join", &json!({"ssid": "Nowhere"})).unwrap_err().contains("not in range"));
    // an ordinary account has none of these tools
    let user = tools(&rig("wifi-user", false), entries(&r.env, c, reader()));
    assert!(user.call("wifi_status", &json!({})).is_err());
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn the_hotspot_country_security_and_band_are_changed_checked_and_never_show_the_password() {
    let r = rig("hotspot", true);
    let (c, d) = world("hs", SCAN);
    let t = tools(&r, entries(&r.env, c, reader()));
    assert!(t.call("hotspot_set", &json!({})).unwrap_err().contains("at least one"));
    assert!(t.call("hotspot_set", &json!({"country": "de"})).is_err());
    assert!(t.call("hotspot_set", &json!({"security": "wep"})).is_err());
    let out = t.call("hotspot_set", &json!({"country": "DE", "security": "transition", "band": "5"})).unwrap();
    assert!(out.contains("Country: DE") && out.contains("WPA2 and WPA3 together") && out.contains("Band: 5") && out.contains("next time sharing starts"), "{out}");
    let (_, pass) = crate::sharing::real::hotspot_credentials(&r.dir).unwrap();
    assert!(!out.contains(&pass) && !t.call("hotspot_status", &json!({})).unwrap().contains(&pass));
    assert!(t.call("hotspot_set", &json!({"country": ""})).unwrap().contains("none chosen"));
    assert!(r.audits.lock().unwrap().iter().any(|l| l.starts_with("settings.share.hotspot")));
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn the_wifi_adapter_is_chosen_by_chat_with_a_card_and_only_among_adapters_that_exist() {
    let r = rig("wifiadapter", true);
    let (c, d) = world("adapter", SCAN);
    // a second adapter next to the first
    let net = d.join("sys/class/net/wlan1");
    std::fs::create_dir_all(net.join("device")).unwrap();
    std::fs::create_dir_all(net.join("wireless")).unwrap();
    std::fs::write(net.join("type"), "1\n").unwrap();
    let t = tools(&r, entries(&r.env, c.clone(), reader()));
    assert_policy(&t, &["wifi_status"], &["wifi_adapter_set"]);
    let st = t.call("wifi_status", &json!({})).unwrap();
    assert!(st.contains("WiFi adapter: wlan0") && st.contains("wlan0, wlan1"), "{st}");
    assert!(t.call("wifi_adapter_set", &json!({"adapter": "wlan9"})).is_err());
    assert!(t.call("wifi_adapter_set", &json!({"adapter": "../x"})).is_err());
    assert_eq!(c.known.chosen_adapter(), None, "a refusal writes nothing");
    t.call("wifi_adapter_set", &json!({"adapter": "wlan1"})).unwrap();
    assert_eq!(c.iface().as_deref(), Some("wlan1"));
    assert!(t.call("wifi_status", &json!({})).unwrap().contains("WiFi adapter: wlan1"));
    assert!(r.audits.lock().unwrap().iter().any(|a| a == "settings.wifi.adapter wlan1"));
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn internet_sharing_is_started_and_stopped_by_chat_with_the_same_checks_as_the_page_and_asks_every_time() {
    use crate::sharing::control::{Backend, MemBackend};
    use crate::sharing::{IfaceInfo, LinkKind};
    let r = rig("share", true);
    let (c, d) = world("sh", SCAN);
    let t = tools(&r, entries(&r.env, c, reader()));
    assert_policy(&t, &["internet_share_status"], &["internet_share_set"]);
    // an ordinary account has neither
    let user = tools(&rig("share-user", false), entries(&r.env, world("sh2", SCAN).0, reader()));
    assert!(user.call("internet_share_status", &json!({})).is_err() && user.call("internet_share_set", &json!({"action": "stop"})).is_err());
    // the pure part, against a made-up machine
    let mut up = IfaceInfo::new("eth0", LinkKind::Ethernet);
    up.default_route = true;
    up.cidrs = vec![iyakki_hotspot::subnet::Cidr::parse("192.168.1.5/24").unwrap()];
    let b = MemBackend::new(vec![up, IfaceInfo::new("enx0", LinkKind::Usb)], false);
    let said = share_status_text(&b);
    assert!(said.contains("Sharing is off") && said.contains("internet can come from: Ethernet eth0") && said.contains("Bluetooth tethering (phones connect to this computer) (cannot be used:"), "{said}");
    // phones over Bluetooth: refused with the reason while it is not possible, no request written
    let e = share_apply(&b, &json!({"action": "start", "through": "bluetooth"}), 0).unwrap_err();
    assert!(e.contains("not built yet"), "{e}");
    assert_eq!(b.request().id, 0);
    b.set_nap_why(None);
    assert!(share_status_text(&b).contains("share through: Bluetooth - phones connect to this computer"));
    let ok = share_apply(&b, &json!({"action": "start", "through": "bluetooth", "hours": 4, "isolate": false}), 0).unwrap();
    assert!(ok.contains("did not answer yet"), "{ok}");
    let req = b.request();
    assert!(req.on && req.nap && !req.wifi && req.idle_hours == 4 && !req.isolate && req.id == 1, "{req:?}");
    // a mistake is explained before anything is written
    assert!(share_apply(&b, &json!({"action": "start", "hours": 99}), 0).unwrap_err().contains("1 to 72"));
    assert!(share_apply(&b, &json!({"action": "start", "through": "nonexistent0"}), 0).unwrap_err().contains("nonexistent0"));
    assert!(share_apply(&b, &json!({"action": "pause"}), 0).is_err());
    assert_eq!(b.request().id, 1);
    // a cable by name, then stop
    assert!(share_apply(&b, &json!({"action": "start", "through": "enx0"}), 0).is_ok());
    assert!(share_apply(&b, &json!({"action": "stop"}), 0).is_ok());
    assert!(!b.request().on);
    let _ = std::fs::remove_dir_all(&d);
}
