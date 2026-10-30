//! Tests for the `.ovpn` sanitiser. All certificate and key text below is FAKE filler (not PEM that decodes to anything);
//! the canary strings stand in for a private key and a static key.

use crate::net::{default_cluster_cidrs, Cidr, Context};
use crate::ovpn::{assemble, parse};
use crate::profile::Body;
use crate::VpnError;

const KEY_CANARY: &str = "FAKE-PRIVATE-KEY-CANARY-0001";
const TLS_CANARY: &str = "FAKE-TLS-CRYPT-CANARY-0002";

fn ctx() -> Context {
    Context { cluster: default_cluster_cidrs(), lan: vec![Cidr::parse("192.168.1.0/24").unwrap()], tunnel: vec![] }
}

fn base() -> String {
    format!(
        "client\ndev tun\nproto udp\nremote vpn.example.com 1194\nresolv-retry infinite\nnobind\npersist-key\npersist-tun\nremote-cert-tls server\ncipher AES-256-GCM\nauth SHA256\nverb 3\nauth-user-pass\n\
<ca>\n-----BEGIN CERTIFICATE-----\nFAKECACERT\n-----END CERTIFICATE-----\n</ca>\n<cert>\n-----BEGIN CERTIFICATE-----\nFAKECLIENTCERT\n-----END CERTIFICATE-----\n</cert>\n\
<key>\n-----BEGIN PRIVATE KEY-----\n{KEY_CANARY}\n-----END PRIVATE KEY-----\n</key>\n<tls-crypt>\n#\n# 2048 bit OpenVPN static key\n#\n{TLS_CANARY}\n</tls-crypt>\n"
    )
}

fn with(extra: &str) -> String {
    format!("{extra}\n{}", base())
}

#[test]
fn a_normal_profile_is_accepted_and_the_secret_blocks_are_cut_out() {
    let imp = parse("Home", base().as_bytes(), &ctx()).unwrap();
    let Body::Openvpn(o) = &imp.body else { panic!() };
    assert_eq!(o.remotes.len(), 1);
    assert_eq!((o.remotes[0].host.as_str(), o.remotes[0].port), ("vpn.example.com", 1194));
    assert!(o.needs_credentials);
    assert!(!o.redirect_gateway);
    assert_eq!(o.secret_blocks, vec!["key", "tls-crypt"]);
    assert!(o.text.contains("<ca>") && o.text.contains("FAKECACERT") && o.text.contains("<cert>"));
    assert!(!o.text.contains(KEY_CANARY) && !o.text.contains(TLS_CANARY));
    assert!(!o.text.contains("<key>") && !o.text.contains("auth-user-pass"));
    assert!(o.text.contains("remote-cert-tls server"));
    assert_eq!(imp.secrets.blocks.len(), 2);
    assert!(imp.warnings.is_empty(), "{:?}", imp.warnings);

    // the stored profile and every printed form are free of the secrets
    let (profile, secrets, _) = imp.into_profile("p2");
    let json = serde_json::to_string(&profile).unwrap();
    for s in [KEY_CANARY, TLS_CANARY] {
        assert!(!json.contains(s) && !format!("{profile:?}").contains(s) && !format!("{secrets:?}").contains(s));
    }
    // at connect time the helper puts them back
    let Body::Openvpn(o) = &profile.body else { panic!() };
    let full = assemble(o, &secrets).unwrap();
    assert!(full.contains(KEY_CANARY) && full.contains(TLS_CANARY) && full.contains("</tls-crypt>"));
    assert!(matches!(assemble(o, &Default::default()), Err(VpnError::Missing(_))));
}

#[test]
fn every_dangerous_directive_is_refused_and_leaves_nothing_behind() {
    let refused = [
        "script-security 1",
        "script-security 2",
        "script-security 3",
        "up /etc/openvpn/update-resolv-conf",
        "down /etc/openvpn/update-resolv-conf",
        "route-up /bin/sh",
        "route-pre-down /bin/sh",
        "ipchange /bin/sh",
        "tls-verify /bin/true",
        "auth-user-pass-verify /bin/true via-env",
        "client-connect /bin/true",
        "client-disconnect /bin/true",
        "learn-address /bin/true",
        "plugin /usr/lib/openvpn/plugin.so",
        "management 127.0.0.1 7505",
        "management-client",
        "management-query-passwords",
        "management-hold",
        "ifconfig-noexec",
        "route-noexec",
        "setenv PATH /tmp",
        "setenv LD_PRELOAD /tmp/x.so",
        "setenv-safe LD_LIBRARY_PATH /tmp",
        "echo hello",
        "daemon",
        "log /etc/passwd",
        "log-append /etc/cron.d/x",
        "writepid /etc/x",
        "status /etc/x",
        "chroot /",
        "cd /",
        "user root",
        "group root",
        "config /etc/shadow",
        "askpass /etc/shadow",
        "auth-user-pass /etc/shadow",
        "ca /etc/shadow",
        "cert /etc/shadow",
        "key /var/lib/iyakki/vault",
        "dh /etc/shadow",
        "tls-auth /etc/shadow 1",
        "tls-crypt /etc/shadow",
        "tls-crypt-v2 /etc/shadow",
        "pkcs12 /etc/shadow",
        "crl-verify /etc/shadow",
        "http-proxy-user-pass /etc/shadow",
        "pull-filter accept route",
        "compress",
        "compress lz4",
        "comp-lzo",
        "ignore-unknown-option up",
        "--up /bin/sh",
        "--script-security 2",
        "engine dynamic",
        "providers /tmp/x.so",
        "dev tap",
        "dev-node /dev/null",
        "cipher none",
        "auth none",
        "data-ciphers AES-256-GCM:none",
        "totally-unknown-directive 1",
        "http-proxy proxy.example.com 8080",
        "tls-version-min 1.2; reboot",
        "remote vpn.example.com 1194 udp extra",
        "verb 3 ; id",
        "verb $(id)",
        "remote `id` 1194",
        "setenv CLIENT_CERT 0 1",
    ];
    for line in refused {
        let r = parse("x", with(line).as_bytes(), &ctx());
        assert!(matches!(r, Err(VpnError::Rejected { .. }) | Err(VpnError::Invalid { .. })), "{line}: {r:?}");
    }
    // a harmless script-security 0 and the safe setenv are fine, and dropped or kept as documented
    let imp = parse("x", with("script-security 0\nsetenv CLIENT_CERT 0").as_bytes(), &ctx()).unwrap();
    let Body::Openvpn(o) = &imp.body else { panic!() };
    assert!(!o.text.contains("script-security") && o.text.contains("setenv CLIENT_CERT 0"));
}

#[test]
fn the_rejection_names_the_directive_and_a_reason_but_never_the_value() {
    let e = parse("x", with("up /secret/path/script.sh").as_bytes(), &ctx()).unwrap_err();
    let msg = e.to_string();
    assert!(msg.contains("\"up\"") && msg.contains("program"));
    assert!(!msg.contains("script.sh"));
    let e = parse("x", with("auth-user-pass /home/someone/pass.txt").as_bytes(), &ctx()).unwrap_err();
    assert!(!e.to_string().contains("pass.txt"));
}

#[test]
fn inline_blocks_follow_the_rules() {
    let ok = base();
    // a block inside a block, an unterminated block, an unknown block, a duplicate, tags that are not alone
    assert!(parse("x", ok.replace("FAKECACERT", "<ca>").as_bytes(), &ctx()).is_err());
    assert!(parse("x", format!("{ok}<tls-auth>\nabc\n").as_bytes(), &ctx()).is_err());
    assert!(parse("x", format!("{ok}<connection>\nremote a 1\n</connection>\n").as_bytes(), &ctx()).is_err());
    assert!(parse("x", format!("{ok}<ca>\nx\n</ca>\n").as_bytes(), &ctx()).is_err());
    assert!(parse("x", format!("{ok}<dh> junk\n").as_bytes(), &ctx()).is_err());
    assert!(parse("x", format!("{ok}<unknown>\nx\n</unknown>\n").as_bytes(), &ctx()).is_err());
    // non-ASCII inside a block
    assert!(parse("x", ok.replace("FAKECACERT", "FAKE\u{e9}CERT").as_bytes(), &ctx()).is_err());
    // an oversized block
    let huge = format!("<dh>\n{}\n</dh>\n", "A".repeat(70 * 1024));
    assert!(matches!(parse("x", format!("{ok}{huge}").as_bytes(), &ctx()), Err(VpnError::TooLarge(_))));
    // a profile needs a CA block and a server
    let no_ca = ok.replace("<ca>\n-----BEGIN CERTIFICATE-----\nFAKECACERT\n-----END CERTIFICATE-----\n</ca>\n", "");
    assert!(matches!(parse("x", no_ca.as_bytes(), &ctx()), Err(VpnError::Missing(_))));
    let no_remote = ok.replace("remote vpn.example.com 1194\n", "");
    assert!(matches!(parse("x", no_remote.as_bytes(), &ctx()), Err(VpnError::Missing(_))));
}

#[test]
fn odd_input_table() {
    let ok = base();
    let big = format!("{ok}{}", "# filler\n".repeat(40_000));
    assert!(matches!(parse("x", big.as_bytes(), &ctx()), Err(VpnError::TooLarge(_))));
    let lines = format!("{ok}{}", "\n".repeat(4500));
    assert!(matches!(parse("x", lines.as_bytes(), &ctx()), Err(VpnError::TooLarge(_))));
    let wide = format!("{ok}# {}\n", "x".repeat(9000));
    assert!(matches!(parse("x", wide.as_bytes(), &ctx()), Err(VpnError::TooLarge(_))));
    assert!(matches!(parse("x", format!("{ok}\0").as_bytes(), &ctx()), Err(VpnError::BadText(_))));
    assert!(matches!(parse("x", &[b'a', 0xc3], &ctx()), Err(VpnError::BadText(_))));
    assert!(matches!(parse("x", format!("{ok}# a\u{1b}b\n").as_bytes(), &ctx()), Err(VpnError::BadText(_))));
    // BOM and CRLF are normalised
    let crlf = format!("\u{feff}{}", ok.replace('\n', "\r\n"));
    let imp = parse("x", crlf.as_bytes(), &ctx()).unwrap();
    let Body::Openvpn(o) = &imp.body else { panic!() };
    assert!(!o.text.contains('\r'));
    // quotes and backslashes
    for bad in ["verb \"3", "verb 3\\", "remote \"vpn.example.com\"x 1194", "cipher AES-\"256\"-GCM", "verb \"\""] {
        assert!(parse("x", with(bad).as_bytes(), &ctx()).is_err(), "{bad}");
    }
    assert!(parse("x", with("remote \"vpn2.example.com\" 443 tcp").as_bytes(), &ctx()).is_ok());
}

#[test]
fn routes_dns_and_redirect_gateway() {
    let imp = parse("x", with("redirect-gateway def1 bypass-dhcp\ndhcp-option DNS 10.8.0.1\ndhcp-option DOMAIN corp.example\nroute 10.8.0.0 255.255.255.0 vpn_gateway\nblock-outside-dns").as_bytes(), &ctx()).unwrap();
    let Body::Openvpn(o) = &imp.body else { panic!() };
    assert!(o.redirect_gateway);
    assert_eq!(o.dns, vec!["10.8.0.1".parse::<std::net::IpAddr>().unwrap()]);
    assert_eq!(o.routes, vec![Cidr::parse("10.8.0.0/24").unwrap()]);
    // DOMAIN ignored, block-outside-dns ignored, and 10.8.0.0/24 is a private range the tunnel's own address does not cover yet
    assert_eq!(imp.warnings.len(), 3);
    assert!(!o.text.contains("redirect-gateway") && !o.text.contains("dhcp-option") && !o.text.contains("route "));

    // a default route as a route line is a full tunnel
    let d = parse("x", with("route 0.0.0.0 0.0.0.0").as_bytes(), &ctx()).unwrap();
    let Body::Openvpn(o) = &d.body else { panic!() };
    assert!(o.redirect_gateway && o.routes.is_empty());

    // the threat-table cases for static routes
    for r in ["route 10.42.0.0 255.255.255.0", "route 10.43.0.0 255.255.0.0", "route 127.0.0.0 255.0.0.0", "route 169.254.169.254", "route 192.168.1.0 255.255.255.0", "route 10.0.0.0 255.0.0.0"] {
        assert!(matches!(parse("x", with(r).as_bytes(), &ctx()), Err(VpnError::Unsafe(_))), "{r}");
    }
    let w = parse("x", with("route 172.20.0.0 255.255.0.0").as_bytes(), &ctx()).unwrap();
    assert_eq!(w.warnings.len(), 1);
    for r in ["route not-an-address", "route 10.0.0.0 255.0.255.0", "route 10.8.0.0 255.255.255.0 a b c d", "route-ipv6 10.0.0.0/8"] {
        assert!(parse("x", with(r).as_bytes(), &ctx()).is_err(), "{r}");
    }
    // DNS: hostnames, loopback, cluster DNS
    for dns in ["dhcp-option DNS dns.example.com", "dhcp-option DNS 127.0.0.1", "dhcp-option DNS 10.43.0.10"] {
        assert!(matches!(parse("x", with(dns).as_bytes(), &ctx()), Err(VpnError::Unsafe(_))), "{dns}");
    }
    // ten servers: only three are kept
    let many: String = (1..=10).map(|i| format!("dhcp-option DNS 9.9.9.{i}\n")).collect();
    let imp = parse("x", with(&many).as_bytes(), &ctx()).unwrap();
    let Body::Openvpn(o) = &imp.body else { panic!() };
    assert_eq!(o.dns.len(), 3);
}

#[test]
fn servers_are_checked_like_wireguard_endpoints() {
    for r in ["remote 127.0.0.1 1194", "remote 10.42.0.5 1194", "remote 169.254.169.254 80", "remote 0.0.0.0 1194"] {
        let c = base().replace("remote vpn.example.com 1194", r);
        assert!(matches!(parse("x", c.as_bytes(), &ctx()), Err(VpnError::Unsafe(_))), "{r}");
    }
    let lan = base().replace("remote vpn.example.com 1194", "remote 192.168.1.9 1194");
    assert_eq!(parse("x", lan.as_bytes(), &ctx()).unwrap().warnings.len(), 1);
    for r in ["remote vpn.example.com 0", "remote vpn.example.com 99999", "remote vpn.example.com 1194 sctp", "remote 1.2.3 1194", "remote bad_host 1194"] {
        assert!(parse("x", base().replace("remote vpn.example.com 1194", r).as_bytes(), &ctx()).is_err(), "{r}");
    }
    // default port
    let d = parse("x", base().replace("remote vpn.example.com 1194", "remote vpn.example.com").as_bytes(), &ctx()).unwrap();
    assert_eq!(d.body_remote_port(), 1194);
}

#[test]
fn weak_settings_warn() {
    let c = base().replace("remote-cert-tls server\n", "tls-version-min 1.0\n");
    let w = parse("x", c.as_bytes(), &ctx()).unwrap().warnings;
    assert!(w.iter().any(|m| m.contains("older than 1.2")));
    assert!(w.iter().any(|m| m.contains("server certificate")));
}

impl crate::profile::Imported {
    fn body_remote_port(&self) -> u16 {
        match &self.body {
            Body::Openvpn(o) => o.remotes[0].port,
            _ => 0,
        }
    }
}

#[test]
fn harmless_directives_are_ignored_with_a_note_and_the_exact_form_matters() {
    for line in [
        "key-method 2", "tls-timeout 2", "disable-occ", "suppress-timestamps", "ifconfig-nowarn", "windows-driver wintun", "route-delay 5", "route-delay 5 30", "route-metric 100", "tun-mtu-extra 32", "mlock", "mtu-disc yes",
        "allow-compression no", "comp-lzo no", "setenv opt block-outside-dns", "ignore-unknown-option block-outside-dns",
    ] {
        let imp = parse("x", with(line).as_bytes(), &ctx()).unwrap_or_else(|e| panic!("{line}: {e}"));
        assert!(imp.warnings.iter().any(|w| w.starts_with("ignored: ")), "{line}: {:?}", imp.warnings);
        let Body::Openvpn(o) = &imp.body else { panic!() };
        let first = line.split(' ').next().unwrap();
        assert!(!o.text.lines().any(|l| l.starts_with(first) && first != "setenv"), "{line} must not reach the config text");
    }
    // other forms of the same names are not the harmless ones
    for line in ["comp-lzo yes", "comp-lzo", "allow-compression yes", "allow-compression asym", "key-method 1", "mtu-disc maybe2", "setenv opt something-else", "ignore-unknown-option up", "route-delay x", "tls-timeout", "mlock now", "windows-driver"] {
        let r = parse("x", with(line).as_bytes(), &ctx());
        let ignored = r.as_ref().map(|i| i.warnings.iter().any(|w| w.starts_with("ignored: "))).unwrap_or(false);
        assert!(!ignored, "{line}");
    }
    assert!(crate::ovpn::IGNORED_DIRECTIVES.len() >= 15);
}
