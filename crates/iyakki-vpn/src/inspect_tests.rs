//! Tests for `iyakki vpn inspect`. Every secret below is a FAKE canary string. The central test plants canaries in every place a
//! value can sit and checks that no canary, and no other value of the input, appears in the output.

use crate::inspect::*;

const C_KEY: &str = "CANARYKEYMATERIAL0123456789ABCDEF";
const C_TLS: &str = "canarytlscryptstatickeyfeedface0001";
const C_HOST: &str = "canary-server-7731.example.net";
const C_USER: &str = "canaryuser-pass-file-4471";
const C_PATH: &str = "/home/canaryperson/secret-ca-9921.pem";
const C_CIPHER: &str = "CANARYCIPHER-5521";
const C_ROUTE: &str = "203.0.113.77";
const C_COMMENT: &str = "canary comment 8812";
const C_NAME: &str = "canarydirectivename3344";
const C_HEX: &str = "6acef03f62675b4b1bbd03e53b187727";

fn render(input: &str) -> String {
    inspect(input.as_bytes()).render()
}

fn ovpn_file() -> String {
    format!(
        "# {C_COMMENT}\nclient\ndev tun\nproto udp\nremote {C_HOST} 1194\nremote-cert-tls server\ncipher AES-256-GCM\nauth SHA256\ntls-version-min 1.2\nverb 3\nauth-user-pass\nroute {C_ROUTE} 255.255.255.255\nresolv-retry infinite\nnobind\n\
<ca>\n-----BEGIN CERTIFICATE-----\nFAKECA\n-----END CERTIFICATE-----\n</ca>\n<cert>\n-----BEGIN CERTIFICATE-----\nFAKECERT\n-----END CERTIFICATE-----\n</cert>\n\
<key>\n-----BEGIN PRIVATE KEY-----\n{C_KEY}\n-----END PRIVATE KEY-----\n</key>\n<tls-crypt>\n#\n# 2048 bit OpenVPN static key\n#\n{C_TLS}\n</tls-crypt>\n"
    )
}

#[test]
fn a_good_openvpn_file_is_described_in_plain_words() {
    let r = inspect(ovpn_file().as_bytes());
    assert_eq!(r.kind, Detected::OpenVpn);
    assert!(r.accepted, "{:?}", r.problem);
    assert!(matches!(r.credentials, Need::Yes(_)));
    assert!(matches!(r.passphrase, Need::No(_)));
    let t = r.render();
    assert!(t.contains("1. Kind: OpenVPN profile (.ovpn)"));
    assert!(t.contains("Needs a user name and password: YES"));
    assert!(t.contains("Needs a private-key passphrase: NO"));
    assert!(t.contains("4. Would IYAKKI accept this profile as it is: YES"));
    for name in ["client", "dev", "remote", "cipher", "auth-user-pass", "route", "nobind"] {
        assert!(t.lines().any(|l| l.trim_start().starts_with(name) && l.contains("ACCEPTED")), "{name}\n{t}");
    }
    assert!(t.contains("cipher: AES-256-GCM") && t.contains("auth: SHA256") && t.contains("tls-version-min: 1.2"));
    assert!(t.contains("ca") && t.lines().any(|l| l.contains("key") && l.contains("secret: would be moved to the vault")));
    assert!(t.lines().any(|l| l.trim_start().starts_with("tls-crypt") && l.contains(" 1 ")));
    assert!(t.contains("FULL:") && t.contains("HYBRID:") && t.contains("LAN, the cluster and developer mode keep working"));
    assert!(t.contains("Servers in the file: 1"));
    assert_eq!(r.blocks.iter().map(|b| (b.name.as_str(), b.count)).collect::<Vec<_>>(), [("ca", 1), ("cert", 1), ("key", 1), ("tls-crypt", 1)]);
}

#[test]
fn nothing_from_the_input_reaches_the_output() {
    let canaries = [C_KEY, C_TLS, C_HOST, C_USER, C_PATH, C_CIPHER, C_ROUTE, C_COMMENT, C_NAME, C_HEX, "canaryperson", "FAKECA", "FAKECERT"];
    let hostile = format!(
        "# {C_COMMENT}\nclient\nremote {C_HOST} 1194\nremote-cert-tls server\ncipher {C_CIPHER}\ndata-ciphers AES-256-GCM:{C_CIPHER}\nauth {C_CIPHER}\ncompress {C_CIPHER}\ntls-version-min {C_CIPHER}\n\
auth-user-pass {C_USER}\nca {C_PATH}\nkey {C_PATH}\nup {C_PATH}\nplugin {C_PATH}\nscript-security 2\nsetenv {C_NAME} {C_KEY}\nroute {C_ROUTE} {C_KEY}\ndhcp-option DNS {C_HOST}\nverb {C_KEY}\n\
{C_NAME} {C_KEY}\n{C_HEX}\n{C_KEY}\n--{C_NAME} 1\nmanagement {C_HOST} {C_KEY}\n\
<ca>\n{C_KEY}\n</ca>\n<key>\n-----BEGIN ENCRYPTED PRIVATE KEY-----\n{C_KEY}\n</key>\n<{C_NAME}>\n{C_TLS}\n</{C_NAME}>\n<tls-crypt>\n{C_TLS}\n"
    );
    let wg = format!(
        "# {C_COMMENT}\n[Interface]\nPrivateKey = {C_KEY}\nAddress = {C_ROUTE}/24\nDNS = {C_HOST}\nPostUp = {C_PATH} {C_KEY}\n{C_NAME} = {C_KEY}\n{C_KEY}=\n{C_HEX}\n[Peer]\nPublicKey = {C_TLS}\nPresharedKey = {C_KEY}\nEndpoint = {C_HOST}:51820\nAllowedIPs = {C_ROUTE}/32, 10.42.0.0/16\n\
[{C_NAME}]\nTable = {C_PATH}\n"
    );
    for (label, input) in [("hostile ovpn", hostile), ("hostile wireguard", wg), ("good ovpn", ovpn_file())] {
        let out = render(&input);
        for c in canaries {
            assert!(!out.contains(c), "{label}: the output contains {c:?}:\n{out}");
        }
        // not even a fragment of the long ones
        for c in [C_KEY, C_TLS, C_HOST, C_HEX] {
            assert!(!out.contains(&c[..10]), "{label}: a fragment of {c}\n{out}");
        }
        assert!(!out.to_lowercase().contains("canary"), "{label}\n{out}");
    }
}

#[test]
fn unknown_directive_names_are_never_printed() {
    let out = render(&format!("client\nremote a.example.com 1194\n{C_NAME} 1\n{C_HEX}\n<ca>\nx\n</ca>\n"));
    assert!(out.contains("(unknown directive)") && out.contains("REFUSED") && !out.contains(C_NAME) && !out.contains(C_HEX));
}

#[test]
fn a_provider_style_file_lists_every_refusal_with_a_reason_and_what_to_do() {
    let f = "client\ndev tun\nremote vpn.example.com 1194\nup /etc/openvpn/update-resolv-conf\ndown /etc/openvpn/update-resolv-conf\nscript-security 2\nauth-user-pass /etc/openvpn/pass.txt\nca ca.crt\ncompress lz4\nkey-method 2\ndisable-occ\nsetenv opt block-outside-dns\ncomp-lzo no\ncipher none\nresolv-retry infinite\n<ca>\nx\n</ca>\n";
    let r = inspect(f.as_bytes());
    assert!(!r.accepted);
    let t = r.render();
    if std::env::var("IYAKKI_PRINT_REPORT").is_ok() {
        println!("{t}");
    }
    let row = |n: &str| t.lines().find(|l| l.trim_start().starts_with(n)).unwrap_or_else(|| panic!("{n}\n{t}")).to_string();
    assert!(row("up ").contains("REFUSED") && row("up ").contains("runs a program as root") && row("up ").contains("What you can do: remove this line"));
    assert!(row("down").contains("REFUSED"));
    assert!(row("script-security").contains("REFUSED"));
    assert!(row("auth-user-pass").contains("REFUSED") && row("auth-user-pass").contains("asks for the user name and password"));
    assert!(row("ca ").contains("REFUSED") && row("ca ").contains("inside a <ca>"));
    assert!(row("compress").contains("VORACLE") && row("compress").contains("provider"));
    assert!(row("key-method").contains("IGNORED") && row("key-method").contains("harmless"));
    assert!(row("disable-occ").contains("IGNORED"));
    assert!(row("setenv").contains("IGNORED"));
    assert!(row("comp-lzo").contains("IGNORED"));
    assert!(row("cipher").contains("REFUSED") && row("cipher").contains("encryption"));
    assert!(row("resolv-retry").contains("ACCEPTED"));
    assert!(t.contains("4. Would IYAKKI accept this profile as it is: NO"));
    assert!(t.contains("First problem: line 4: \"up\" is not accepted"));
    assert!(t.contains("setting(s) are REFUSED"));
    // the same directive used twice is one row with its count, and the worse verdict wins
    let two = inspect(b"client\nremote vpn.example.com 1194\nverb 3\nverb banana\n<ca>\nx\n</ca>\n");
    let v = two.rows.iter().find(|r| r.name == "verb").unwrap();
    assert_eq!(v.count, 2);
    assert!(matches!(v.verdict, Verdict::Refused { .. }));
}

#[test]
fn passphrase_and_credentials_are_detected_without_printing_anything() {
    let enc = format!("client\nremote a.example.com 1194\n<ca>\nx\n</ca>\n<key>\n-----BEGIN ENCRYPTED PRIVATE KEY-----\n{C_KEY}\n-----END ENCRYPTED PRIVATE KEY-----\n</key>\n");
    let r = inspect(enc.as_bytes());
    assert!(matches!(r.passphrase, Need::Yes(_)) && matches!(r.credentials, Need::No(_)));
    assert!(!r.render().contains(C_KEY));
    let old = "client\nremote a.example.com 1194\n<ca>\nx\n</ca>\n<key>\n-----BEGIN RSA PRIVATE KEY-----\nProc-Type: 4,ENCRYPTED\nDEK-Info: AES-128-CBC,00\n</key>\n";
    assert!(matches!(inspect(old.as_bytes()).passphrase, Need::Yes(_)));
    assert!(matches!(inspect(b"client\nremote a.example.com 1194\naskpass /x\n<ca>\nx\n</ca>\n").passphrase, Need::Yes(_)));
    assert!(matches!(inspect(b"client\nremote a.example.com 1194\n<ca>\nx\n</ca>\n<pkcs12>\nAAAA\n</pkcs12>\n").passphrase, Need::Unknown(_)));
    let plain = inspect(b"client\nremote a.example.com 1194\nauth-user-pass\n<ca>\nx\n</ca>\n<key>\n-----BEGIN PRIVATE KEY-----\nAAAA\n-----END PRIVATE KEY-----\n</key>\n");
    assert!(matches!(plain.passphrase, Need::No(_)) && matches!(plain.credentials, Need::Yes(_)));
}

#[test]
fn encryption_and_compression_are_shown_by_known_name_only() {
    let r = inspect(format!("client\nremote a.example.com 1194\ncipher {C_CIPHER}\ndata-ciphers AES-256-GCM:CHACHA20-POLY1305\nauth SHA512\ncompress lz4\ncomp-lzo no\ntls-cipher TLS-ECDHE-RSA-WITH-AES-256-GCM-SHA384\n<ca>\nx\n</ca>\n").as_bytes());
    let get = |k: &str| r.crypto.iter().filter(|(n, _)| n == k).map(|(_, v)| v.clone()).collect::<Vec<_>>();
    assert_eq!(get("data-ciphers"), ["AES-256-GCM CHACHA20-POLY1305"]);
    assert_eq!(get("auth"), ["SHA512"]);
    assert_eq!(get("compress"), ["lz4"]);
    assert_eq!(get("comp-lzo"), ["no"]);
    assert_eq!(get("cipher"), ["a value that is not a known algorithm name (not shown)"]);
    assert!(get("tls-cipher")[0].starts_with("TLS-ECDHE-RSA"));
    assert!(!r.render().contains(C_CIPHER));
}

fn wg_file() -> String {
    format!(
        "[Interface]\nPrivateKey = {k1}\nAddress = 10.8.0.2/24\nDNS = 10.8.0.1\n\n[Peer]\nPublicKey = {k2}\nEndpoint = vpn.example.com:51820\nAllowedIPs = 0.0.0.0/0\nPersistentKeepalive = 25\n",
        k1 = crate::b64::encode(&[0x11; 32]),
        k2 = crate::b64::encode(&[0x22; 32])
    )
}

#[test]
fn a_wireguard_file_is_described_and_a_script_line_is_refused() {
    let r = inspect(wg_file().as_bytes());
    assert_eq!(r.kind, Detected::WireGuard);
    assert!(r.accepted, "{:?}", r.problem);
    assert!(matches!(r.credentials, Need::No(_)) && matches!(r.passphrase, Need::No(_)));
    let t = r.render();
    assert!(t.contains("WireGuard profile (.conf)"));
    for k in ["[Interface] PrivateKey", "[Interface] Address", "[Interface] DNS", "[Peer] PublicKey", "[Peer] Endpoint", "[Peer] AllowedIPs", "[Peer] PersistentKeepalive"] {
        assert!(t.lines().any(|l| l.contains(k) && l.contains("ACCEPTED")), "{k}\n{t}");
    }
    assert!(t.contains("FULL:   all traffic goes through the VPN") && t.contains("Any filter works"));
    assert!(t.contains("Peers in the file: 1") && t.contains("Servers in the file: 1"));
    assert!(t.contains("compression: none"));
    // the keys, in any form, are not in the output
    for k in [crate::b64::encode(&[0x11; 32]), crate::b64::encode(&[0x22; 32])] {
        assert!(!t.contains(&k) && !t.contains(&k[..12]));
    }
    let bad = wg_file().replace("DNS = 10.8.0.1", "DNS = 10.8.0.1\nPostUp = reboot\nTable = 100\nFwMark = 5\nSaveConfig = true");
    let r = inspect(bad.as_bytes());
    assert!(!r.accepted);
    let t = r.render();
    for k in ["PostUp", "Table", "FwMark", "SaveConfig"] {
        assert!(t.lines().any(|l| l.contains(k) && l.contains("REFUSED")), "{k}\n{t}");
    }
    assert!(t.contains("run a command as root"));
    // a split-tunnel file says so
    let split = wg_file().replace("0.0.0.0/0", "10.8.0.0/24");
    let t = inspect(split.as_bytes()).render();
    assert!(t.contains("only the networks in the file's AllowedIPs") && t.contains("A filter only works for destinations inside"));
}

#[test]
fn files_that_are_not_profiles_get_a_fixed_answer() {
    for (input, what) in [(&b""[..], "empty"), (&b"hello world\n"[..], "text"), (&[0xff, 0xfe, 0x00, 0x01][..], "binary"), (&b"\x1b[31m client\nremote x 1\n"[..], "escape")] {
        let r = inspect(input);
        assert!(!r.accepted, "{what}");
        let t = r.render();
        assert!(t.contains("4. Would IYAKKI accept this profile as it is: NO"), "{what}");
        assert!(t.contains("First problem:"), "{what}");
        assert!(!t.contains("hello world"), "{what}");
    }
    assert!(render("hello world").contains("does not look like an OpenVPN"));
    let big = vec![b'a'; 300 * 1024];
    let r = inspect(&big);
    assert!(r.problem.unwrap().contains("larger than"));
    // a lone pasted key
    let out = render(&format!("{C_KEY}\n"));
    assert!(!out.contains(C_KEY));
    // a huge number of distinct unknown directives stays one row
    let many: String = (0..3000).map(|i| format!("junk{i} 1\n")).collect::<String>() + "client\nremote a.example.com 1194\n<ca>\nx\n</ca>\n";
    let r = inspect(many.as_bytes());
    assert_eq!(r.rows.iter().filter(|x| x.name == "(unknown directive)").count(), 1);
    assert_eq!(r.rows.iter().find(|x| x.name == "(unknown directive)").unwrap().count, 3000);
}

#[test]
fn the_accepted_list_matches_what_the_importer_accepts() {
    // every name in the list is understood by the parser: alone or with one argument it never says "not on the list"
    for n in crate::ovpn::ACCEPTED_DIRECTIVES {
        let understood = ["", " 1", " tun", " server", " 0"].iter().any(|a| {
            let f = format!("remote vpn.example.com 1194\n<ca>\nx\n</ca>\n{n}{a}\n");
            !matches!(crate::ovpn::parse("x", f.as_bytes(), &crate::net::Context::default()), Err(crate::VpnError::Rejected { reason, .. }) if reason.contains("not on the list"))
        });
        assert!(understood, "{n}");
        assert!(crate::ovpn::known_directive(n));
    }
    assert!(!crate::ovpn::known_directive("definitely-not-a-directive"));
    assert!(crate::ovpn::known_directive("management-hold") && crate::ovpn::known_directive("up") && crate::ovpn::known_directive("key-method"));
}

#[test]
fn the_command_reads_a_file_and_exits_with_the_answer() {
    let good = ovpn_file();
    let read_good = |_: &str| Ok(good.clone().into_bytes());
    let (code, out, err) = run(&["inspect".into(), "/some/where/profile.ovpn".into()], &read_good);
    assert_eq!((code, err.as_str()), (0, ""));
    assert!(out.starts_with("IYAKKI VPN profile check\n") && !out.contains("/some/where") && !out.contains("profile.ovpn"));
    let bad = |_: &str| Ok(b"client\nremote a.example.com 1194\nup /bin/sh\n<ca>\nx\n</ca>\n".to_vec());
    assert_eq!(run(&["inspect".into(), "f".into()], &bad).0, 1);
    let missing = |p: &str| Err(format!("not found {}", &p[..0]));
    let (code, out, err) = run(&["inspect".into(), "/home/canaryperson/secret.ovpn".into()], &missing);
    assert_eq!((code, out.as_str()), (2, ""));
    assert!(err.starts_with("cannot read the file") && !err.contains("canaryperson"));
    for args in [vec![], vec!["inspect".to_string()], vec!["inspect".into(), "a".into(), "b".into()], vec!["bogus".into(), "f".into()]] {
        let (code, _, err) = run(&args, &read_good);
        assert_eq!(code, 2);
        assert!(err.starts_with("usage: iyakki vpn inspect FILE"));
    }
}

#[test]
fn the_real_command_reads_a_temporary_file_and_never_a_huge_one_whole() {
    let dir = std::env::temp_dir().join(format!("iyakki-vpn-inspect-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let f = dir.join("p.ovpn");
    std::fs::write(&f, ovpn_file()).unwrap();
    assert_eq!(cli_main(&["inspect".into(), f.to_string_lossy().into_owned()]), 0);
    let big = dir.join("big.ovpn");
    std::fs::write(&big, vec![b'#'; 2 * 1024 * 1024]).unwrap();
    assert_eq!(cli_main(&["inspect".into(), big.to_string_lossy().into_owned()]), 1);
    assert_eq!(cli_main(&["inspect".into(), dir.join("nope").to_string_lossy().into_owned()]), 2);
    assert_eq!(cli_main(&["inspect".into(), dir.to_string_lossy().into_owned()]), 2, "a directory is not a profile");
    std::fs::remove_dir_all(&dir).unwrap();
}
