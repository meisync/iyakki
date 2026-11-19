use super::*;

fn peap() -> Enterprise {
    Enterprise {
        method: EapMethod::Peap,
        identity: "bob".into(),
        anonymous_identity: Some("anon".into()),
        password: Some("p\"w".into()),
        inner: None,
        ca_cert: "/etc/ssl/certs/campus-ca.pem".into(),
        server_name_match: Some("radius.example.edu".into()),
        client_cert: None,
        private_key: None,
        private_key_password: None,
    }
}

#[test]
fn a_peap_network_is_written_with_hex_names_a_checked_server_and_protected_management_frames() {
    let b = enterprise_block("Campus", true, 3, &peap()).unwrap();
    assert_eq!(
        b,
        "network={\n    ssid=\"Campus\"\n    priority=3\n    scan_ssid=1\n    key_mgmt=WPA-EAP\n    eap=PEAP\n    identity=626f62\n    anonymous_identity=616e6f6e\n    password=702277\n    phase2=\"auth=MSCHAPV2\"\n    ca_cert=\"/etc/ssl/certs/campus-ca.pem\"\n    domain_suffix_match=\"radius.example.edu\"\n    ieee80211w=1\n}\n"
    );
    // a password with a quote or a closing brace cannot end the block: it is hexadecimal
    let mut e = peap();
    e.password = Some("}\nnetwork={".replace('\n', " "));
    let b = enterprise_block("Campus", false, 1, &e).unwrap();
    assert_eq!(b.matches("network={").count(), 1);
    assert!(!b.contains("scan_ssid"));
    let mut e = peap();
    e.method = EapMethod::Ttls;
    e.inner = Some("PAP".into());
    assert!(enterprise_block("Campus", false, 1, &e).unwrap().contains("eap=TTLS") && enterprise_block("Campus", false, 1, &e).unwrap().contains("auth=PAP"));
}

#[test]
fn a_certificate_network_needs_its_files_and_no_password() {
    let e = Enterprise { method: EapMethod::Tls, identity: "bob".into(), anonymous_identity: None, password: None, inner: None, ca_cert: "/c/ca.pem".into(), server_name_match: None, client_cert: Some("/c/me.pem".into()), private_key: Some("/c/me.key".into()), private_key_password: Some("k".into()) };
    let b = enterprise_block("Lab", false, 1, &e).unwrap();
    assert!(b.contains("eap=TLS") && b.contains("client_cert=\"/c/me.pem\"") && b.contains("private_key=\"/c/me.key\"") && b.contains("private_key_passwd=6b") && !b.contains("phase2") && !b.contains("password=6"));
    let mut bad = e.clone();
    bad.private_key = None;
    assert!(bad.check().unwrap_err().contains("private key"));
    let mut bad = e.clone();
    bad.password = Some("x".into());
    assert!(bad.check().unwrap_err().contains("no password"));
}

#[test]
fn bad_enterprise_entries_are_refused_in_plain_words() {
    let no_ca = |f: &dyn Fn(&mut Enterprise)| {
        let mut e = peap();
        f(&mut e);
        e.check().unwrap_err()
    };
    assert!(no_ca(&|e| e.ca_cert = String::new()).contains("pose as the network"));
    assert!(no_ca(&|e| e.ca_cert = "relative.pem".into()).contains("certificate file"));
    assert!(no_ca(&|e| e.ca_cert = "/a/../etc/passwd".into()).contains("certificate file"));
    assert!(no_ca(&|e| e.ca_cert = "/a\"b".into()).contains("certificate file"));
    assert!(no_ca(&|e| e.identity = String::new()).contains("user name"));
    assert!(no_ca(&|e| e.identity = "a\nb".into()).contains("user name"));
    assert!(no_ca(&|e| e.password = None).contains("password"));
    assert!(no_ca(&|e| e.password = Some("x".repeat(257))).contains("256"));
    assert!(no_ca(&|e| e.inner = Some("CHAP".into())).contains("MSCHAPV2"));
    assert!(no_ca(&|e| e.server_name_match = Some("a b;c".into())).contains("host name"));
    assert!(enterprise_block("bad\"name", false, 1, &peap()).is_err());
    assert!(enterprise_block("", false, 1, &peap()).is_err());
}

#[test]
fn a_wep_key_is_five_or_thirteen_characters_or_ten_or_twenty_six_hex_digits() {
    for ok in ["abcde", "abcdefghijklm", "0123456789", "0123456789abcdef0123456789", "ABCDEF1234"] {
        assert!(check_wep_key(ok).is_ok(), "{ok}");
    }
    for bad in ["", "abcd", "abcdef", "abc\"e", "abc\\e", "0123456789abcdef", "ééééé"] {
        assert!(check_wep_key(bad).is_err(), "{bad}");
    }
    assert_eq!(wep_block("Old", false, 2, "abcde").unwrap(), "network={\n    ssid=\"Old\"\n    priority=2\n    key_mgmt=NONE\n    wep_key0=\"abcde\"\n    wep_tx_keyidx=0\n}\n");
    assert!(wep_block("Old", true, 2, "ABCDEF1234").unwrap().contains("scan_ssid=1\n    key_mgmt=NONE\n    wep_key0=abcdef1234\n"));
    assert!(wep_block("Old", false, 1, "no").is_err() && wep_block("", false, 1, "abcde").is_err());
    assert!(wep_warning().contains("can be broken"));
}

#[test]
fn the_probe_request_and_answer_are_plain_http() {
    let r = String::from_utf8(probe_request("connectivity.example.net", "/generate_204").unwrap()).unwrap();
    assert!(r.starts_with("GET /generate_204 HTTP/1.1\r\nHost: connectivity.example.net\r\n") && r.ends_with("\r\n\r\n") && r.contains("Connection: close"));
    assert!(probe_request("bad host", "/x").is_err() && probe_request("h.example", "x").is_err() && probe_request("h.example", "/a b").is_err());
    let (s, loc, body) = parse_http(b"HTTP/1.1 302 Found\r\nLocation: http://login.hotel.example/start?x=1\r\nContent-Length: 0\r\n\r\n").unwrap();
    assert_eq!((s, loc.as_deref(), body.as_str()), (302, Some("http://login.hotel.example/start?x=1"), ""));
    assert_eq!(parse_http(b"HTTP/1.0 204 No Content\r\n\r\n").unwrap().0, 204);
    assert!(parse_http(b"HTTP/1.1 204").is_err() && parse_http(b"SSH-2.0\r\n\r\n").is_err() && parse_http(b"HTTP/1.1 abc\r\n\r\n").is_err());
}

#[test]
fn the_answers_of_a_free_network_and_of_a_sign_in_network_are_told_apart() {
    assert_eq!(classify_probe(Probe::NoContent, 204, None, ""), PortalState::Open);
    assert_eq!(classify_probe(Probe::SuccessPage, 200, None, "Success\n"), PortalState::Open);
    assert_eq!(classify_probe(Probe::SuccessPage, 200, None, "<HTML><BODY>Success</BODY></HTML>"), PortalState::Open);
    // a redirect to the sign-in page
    assert_eq!(classify_probe(Probe::NoContent, 302, Some("http://login.hotel.example/x"), ""), PortalState::SignIn(Some("http://login.hotel.example/x".into())));
    // a page in the place of the empty answer
    assert_eq!(classify_probe(Probe::NoContent, 200, None, "<html>Welcome</html>"), PortalState::SignIn(None));
    assert_eq!(classify_probe(Probe::SuccessPage, 200, None, "<html>Please sign in</html>"), PortalState::SignIn(None));
    assert_eq!(classify_probe(Probe::NoContent, 511, None, ""), PortalState::SignIn(None));
    // a redirect to something that is not a web address is not handed to the browser
    for bad in ["javascript:alert(1)", "file:///etc/passwd", "http://", "http://user@evil.example/", "http://a b/", "ftp://x/"] {
        assert_eq!(classify_probe(Probe::NoContent, 302, Some(bad), ""), PortalState::SignIn(None), "{bad}");
    }
    assert!(matches!(classify_probe(Probe::NoContent, 404, None, ""), PortalState::Unknown(_)));
    assert!(matches!(classify_probe(Probe::NoContent, 500, None, ""), PortalState::Unknown(_)));
    assert!(portal_sentence(&PortalState::SignIn(Some("http://l.example/".into()))).contains("http://l.example/"));
    assert!(portal_sentence(&PortalState::Open).contains("no sign-in"));
    assert!(portal_sentence(&PortalState::Unknown("x".into())).contains("Could not tell"));
    assert!(portal_url(&format!("http://h.example/{}", "a".repeat(2000))).is_none());
}

const LINK: &str = "Connected to 9C:3D:CF:11:22:33 (on wlan0)\n\tSSID: Home Net\n\tfreq: 5180.0\n\tRX: 1234 bytes (10 packets)\n\tTX: 567 bytes (5 packets)\n\tsignal: -58 dBm\n\tsignal avg: -60 dBm\n\trx bitrate: 433.3 MBit/s\n\ttx bitrate: 390.0 MBit/s VHT-MCS 8\n";

#[test]
fn the_joined_network_is_read_from_iw_link() {
    let l = parse_link(LINK).unwrap();
    assert_eq!((l.ssid.as_str(), l.bssid.as_str(), l.freq_mhz, l.signal_dbm, l.tx_mbit), ("Home Net", "9c:3d:cf:11:22:33", 5180, -58.0, Some(390.0)));
    assert_eq!(parse_link("Not connected.\n"), None);
    assert_eq!(parse_link(""), None);
    // an older iw prints an integer frequency and no bitrate
    let old = "Connected to aa:bb:cc:dd:ee:ff (on wlan0)\n\tSSID: x\n\tfreq: 2437\n\tsignal: -71 dBm\n";
    let l = parse_link(old).unwrap();
    assert_eq!((l.freq_mhz, l.tx_mbit), (2437, None));
    assert_eq!(parse_link("Connected to aa:bb:cc:dd:ee:ff (on wlan0)\n\tSSID: x\n"), None, "no signal line");
}

#[test]
fn the_tracker_smooths_the_readings_and_says_better_or_worse() {
    let mut t = SignalTracker::new();
    assert_eq!(t.sentence(), "Signal: not connected.");
    assert_eq!((t.smoothed(), t.bars(), t.trend()), (None, None, Trend::Steady));
    for d in [-50.0, -50.0, -50.0, -50.0] {
        t.push(d);
    }
    assert_eq!((t.smoothed(), t.bars(), t.trend()), (Some(-50.0), Some(4), Trend::Steady));
    for d in [-62.0, -66.0, -70.0, -74.0] {
        t.push(d);
    }
    assert_eq!(t.trend(), Trend::Falling);
    let s = t.sentence();
    assert!(s.contains("getting weaker") && s.contains("bars"), "{s}");
    for d in [-45.0; 8] {
        t.push(d);
    }
    assert_eq!((t.trend(), t.bars()), (Trend::Steady, Some(4)), "only the last eight count");
    for d in [-80.0, -80.0, -55.0, -50.0, -45.0, -42.0] {
        t.push(d);
    }
    assert_eq!(t.trend(), Trend::Rising);
    // junk is ignored and extremes are held
    t.push(f32::NAN);
    t.push(-500.0);
    assert!(t.smoothed().unwrap() >= -120.0);
    let mut weak = SignalTracker::new();
    weak.push(-80.0);
    assert!(weak.sentence().contains("weak: move closer"));
    weak.push(-95.0);
    weak.push(-95.0);
    assert!(weak.sentence().contains("very weak"));
    assert_eq!((bars_from_dbm(-50.0), bars_from_dbm(-60.0), bars_from_dbm(-70.0), bars_from_dbm(-80.0), bars_from_dbm(-90.0)), (4, 3, 2, 1, 0));
}

const IW_DEV: &str = "phy#1\n\tInterface wlan1\n\t\tifindex 4\n\t\twdev 0x100000001\n\t\taddr AA:BB:CC:00:00:02\n\t\ttype managed\n\t\ttxpower 20.00 dBm\nphy#0\n\tInterface wlan0\n\t\tifindex 3\n\t\taddr 11:22:33:44:55:66\n\t\tssid Home Net\n\t\ttype managed\n\tInterface iyakap0\n\t\tifindex 5\n\t\taddr 11:22:33:44:55:67\n\t\ttype AP\n";

#[test]
fn iw_dev_lists_the_adapters_with_their_mode_and_network() {
    let a = parse_iw_dev(IW_DEV);
    assert_eq!(a.len(), 3);
    assert_eq!((a[0].name.as_str(), a[0].phy.as_str(), a[0].address.as_str(), a[0].mode.as_str(), a[0].ssid.clone()), ("wlan1", "phy1", "aa:bb:cc:00:00:02", "managed", None));
    assert_eq!((a[1].name.as_str(), a[1].ssid.as_deref()), ("wlan0", Some("Home Net")));
    assert_eq!((a[2].name.as_str(), a[2].mode.as_str(), a[2].phy.as_str()), ("iyakap0", "AP", "phy0"));
    assert!(parse_iw_dev("").is_empty());
}

#[test]
fn with_two_adapters_a_remembered_one_then_the_joined_one_decides_else_the_person_is_asked() {
    let all = parse_iw_dev(IW_DEV);
    // the access point interface is never chosen; two clients, one joined
    assert_eq!(choose_adapter(&all, None), AdapterChoice::Use("wlan0".into()));
    assert_eq!(choose_adapter(&all, Some("wlan1")), AdapterChoice::Use("wlan1".into()));
    assert_eq!(choose_adapter(&all, Some("gone0")), AdapterChoice::Use("wlan0".into()));
    assert_eq!(choose_adapter(&all, Some("iyakap0")), AdapterChoice::Use("wlan0".into()), "the hotspot's interface cannot be remembered as the client");
    // neither joined: ask
    let mut two = all.clone();
    two[1].ssid = None;
    assert_eq!(choose_adapter(&two, None), AdapterChoice::Ask(vec!["wlan1".into(), "wlan0".into()]));
    // both joined: ask
    let mut both = all.clone();
    both[0].ssid = Some("Other".into());
    assert!(matches!(choose_adapter(&both, None), AdapterChoice::Ask(_)));
    // one client only
    assert_eq!(choose_adapter(&all[1..], None), AdapterChoice::Use("wlan0".into()));
    assert!(matches!(choose_adapter(&all[2..], None), AdapterChoice::None(_)));
    assert!(matches!(choose_adapter(&[], None), AdapterChoice::None(_)));
}

#[test]
fn enterprise_and_wep_entries_are_kept_in_the_store_and_written_into_the_supplicant_file() {
    use crate::wifi::{wpa_conf_for, Known, KnownStore, Security};
    let d = std::env::temp_dir().join(format!("iyakki-wifiext-store-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    let store = KnownStore::new(&d.join("wifi.json"));
    store.add(Known::new("Home", "homepass1", Security::Wpa2, false, 1).unwrap()).unwrap();
    store.add(Known::new_wep("OldAP", "abcde", true, 2).unwrap()).unwrap();
    store.add(Known::new_enterprise("Campus", false, peap(), 3).unwrap()).unwrap();
    let all = store.list();
    assert_eq!(all.iter().map(|k| k.ssid.as_str()).collect::<Vec<_>>(), ["Campus", "OldAP", "Home"]);
    assert_eq!(all[0].enterprise.as_ref().unwrap().identity, "bob");
    let conf = wpa_conf_for(&all);
    assert!(conf.contains("key_mgmt=WPA-EAP") && conf.contains("eap=PEAP"));
    assert!(conf.contains("wep_key0=\"abcde\"") && conf.contains("scan_ssid=1"));
    assert!(conf.contains("ssid=\"Campus\"\n    priority=3") && conf.contains("ssid=\"Home\"\n    priority=1"));
    // the password is written in hexadecimal only
    assert!(!conf.contains("p\"w"));
    // an old file without the new fields still reads
    std::fs::write(d.join("old.json"), r#"[{"ssid":"X","psk":null,"passphrase":null,"hidden":false,"added":1}]"#).unwrap();
    assert_eq!(KnownStore::new(&d.join("old.json")).list().len(), 1);
    // a bad entry is refused at the constructor, in plain words
    let mut bad = peap();
    bad.ca_cert = String::new();
    assert!(Known::new_enterprise("Campus", false, bad, 1).unwrap_err().contains("certificate"));
    assert!(Known::new_wep("OldAP", "abc", false, 1).is_err());
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn the_probe_and_signal_timers_follow_the_join_and_slow_down_once_the_network_is_open() {
    let mut t = LinkTimers::default();
    assert!(!t.probe_due(100) && !t.signal_due(100));
    t.joined(100);
    assert!(!t.probe_due(101) && t.signal_due(101));
    assert!(t.probe_due(103));
    t.probe_done(103, &PortalState::SignIn(None));
    assert!(!t.probe_due(110) && t.probe_due(113));
    t.probe_done(113, &PortalState::Open);
    assert!(!t.probe_due(200) && t.probe_due(233));
    t.signal_done(113);
    assert!(!t.signal_due(117) && t.signal_due(118));
    t.joined(500);
    assert!(!t.probe_due(501) && t.probe_due(503));
    t.left();
    assert!(!t.probe_due(9999) && !t.signal_due(9999));
}

#[test]
fn the_probe_reads_a_real_answer_over_a_socket_and_gives_up_on_a_dead_address() {
    use std::io::{Read, Write};
    let serve = |answer: &'static str| {
        let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = l.local_addr().unwrap().to_string();
        std::thread::spawn(move || {
            if let Ok((mut s, _)) = l.accept() {
                let mut b = [0u8; 512];
                let _ = s.read(&mut b);
                let _ = s.write_all(answer.as_bytes());
            }
        });
        addr
    };
    let t = std::time::Duration::from_secs(3);
    let open = serve("HTTP/1.1 204 No Content\r\n\r\n");
    assert_eq!(probe_once(&open, "check.example.net", "/generate_204", Probe::NoContent, t), PortalState::Open);
    let portal = serve("HTTP/1.1 302 Found\r\nLocation: http://login.example/start\r\n\r\n");
    assert_eq!(probe_once(&portal, "check.example.net", "/generate_204", Probe::NoContent, t), PortalState::SignIn(Some("http://login.example/start".into())));
    let junk = serve("not http at all");
    assert!(matches!(probe_once(&junk, "check.example.net", "/generate_204", Probe::NoContent, t), PortalState::Unknown(_)));
    let dead = std::net::TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().to_string();
    assert!(matches!(probe_once(&dead, "check.example.net", "/x", Probe::NoContent, t), PortalState::Unknown(_)));
    assert!(matches!(probe_once("127.0.0.1:9", "bad host", "/x", Probe::NoContent, t), PortalState::Unknown(_)));
}
