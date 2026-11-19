use super::*;
use crate::validate::check_comment;

fn rule(id: u32, verdict: Verdict, dest: &str, proto: Proto, ports: &str) -> OutRule {
    OutRule { id, enabled: true, verdict, dest: dest.into(), proto, ports: ports.into(), iface: String::new(), note: String::new() }
}

fn full() -> OutConfig {
    OutConfig {
        enabled: true,
        block_by_default: true,
        rules: vec![
            rule(1, Verdict::Allow, "", Proto::Both, "53"),
            rule(2, Verdict::Allow, "", Proto::Tcp, "80,443,8000-8100"),
            rule(3, Verdict::Block, "203.0.113.0/24", Proto::Both, ""),
            OutRule { iface: "wlan0".into(), note: "work".into(), ..rule(4, Verdict::Allow, "198.51.100.10-198.51.100.20", Proto::Udp, "123") },
            rule(5, Verdict::Allow, "2001:db8::1", Proto::Tcp, "22"),
            OutRule { enabled: false, ..rule(6, Verdict::Block, "192.0.2.1", Proto::Both, "") },
        ],
        forwards: vec![ForwardRule { id: 1, enabled: true, from_iface: "usb0".into(), to_iface: "wlan0".into(), dest: String::new(), proto: Proto::Both, ports: String::new(), note: "share".into() }, ForwardRule { id: 2, enabled: true, from_iface: "eth1".into(), to_iface: "eth0".into(), dest: "10.1.0.0/16".into(), proto: Proto::Tcp, ports: "443".into(), note: String::new() }],
        port_forwards: vec![PortForward { id: 1, enabled: true, iface: "eth0".into(), proto: Proto::Tcp, port: 8080, to: "192.168.1.5".into(), to_port: 80, note: "web".into() }, PortForward { id: 2, enabled: true, iface: "eth0".into(), proto: Proto::Udp, port: 5353, to: "fd00::5".into(), to_port: 53, note: String::new() }],
    }
}

/// Runs the script through `nft -c -f` (check only, nothing is applied) inside a private user and network namespace
/// (`unshare -rn`), so this computer's own network is never opened. `Ok(None)` when the tools are not there.
fn check_with_nft(script: &str) -> Result<Option<()>, String> {
    use std::io::Write;
    use std::process::{Command, Stdio};
    let have = |p: &str| Command::new(p).arg("--version").stdout(Stdio::null()).stderr(Stdio::null()).status().is_ok();
    if !(have("nft") && Command::new("unshare").args(["-rn", "true"]).status().is_ok_and(|s| s.success())) {
        return Ok(None);
    }
    let mut child = Command::new("unshare").args(["-rn", "nft", "-c", "-f", "-"]).stdin(Stdio::piped()).stdout(Stdio::null()).stderr(Stdio::piped()).spawn().map_err(|e| e.to_string())?;
    child.stdin.take().ok_or("no stdin")?.write_all(script.as_bytes()).map_err(|e| e.to_string())?;
    let out = child.wait_with_output().map_err(|e| e.to_string())?;
    if out.status.success() {
        Ok(Some(()))
    } else {
        Err(String::from_utf8_lossy(&out.stderr).trim().to_string())
    }
}

#[test]
fn off_gives_nothing_and_default_allow_needs_no_rules() {
    assert_eq!(generate(&OutConfig::default()).unwrap(), "");
    let t = generate(&OutConfig { enabled: true, ..Default::default() }).unwrap();
    assert!(t.contains("hook output priority filter; policy accept;") && !t.contains("chain forward") && !t.contains("nat"), "{t}");
}

#[test]
fn the_script_has_the_never_cut_rules_the_persons_rules_in_order_and_the_forwarding_and_nat_chains() {
    let t = generate(&full()).unwrap();
    for want in [
        "policy drop;",
        "ct state established,related accept",
        "oif \"lo\" accept",
        "oifname \"cni*\" accept",
        "meta nfproto ipv4 meta l4proto udp th dport 67 accept",
        "meta l4proto { tcp, udp } th dport { 53 } counter accept  # outgoing rule 1",
        "meta l4proto tcp th dport { 80, 443, 8000-8100 } counter accept  # outgoing rule 2",
        "ip daddr 203.0.113.0/24 counter reject  # outgoing rule 3",
        "oifname \"wlan0\" ip daddr 198.51.100.10-198.51.100.20 meta l4proto udp th dport { 123 } counter accept  # outgoing rule 4: work",
        "ip6 daddr 2001:db8::1 meta l4proto tcp th dport { 22 } counter accept",
        "chain forward {",
        "iifname \"usb0\" oifname \"wlan0\" counter accept  # forwarding rule 1: share",
        "iifname \"eth1\" oifname \"eth0\" ip daddr 10.1.0.0/16 meta l4proto tcp th dport { 443 } counter accept",
        "ct status dnat counter accept  # port forward 1",
        "table ip iyakki_fw_nat {",
        "iifname \"eth0\" tcp dport 8080 counter dnat to 192.168.1.5:80",
        "table ip6 iyakki_fw_nat {",
        "iifname \"eth0\" udp dport 5353 counter dnat to [fd00::5]:53",
    ] {
        assert!(t.contains(want), "{want}\n{t}");
    }
    assert!(!t.contains("outgoing rule 6"), "a disabled rule is left out");
    let (a, b) = (t.find("outgoing rule 2").unwrap(), t.find("outgoing rule 3").unwrap());
    assert!(a < b, "rules keep the order of the list");
}

#[test]
fn what_would_cut_the_machine_off_or_open_too_much_is_refused_with_a_reason() {
    let mut c = full();
    c.rules.retain(|r| r.verdict == Verdict::Block);
    assert!(check(&c).unwrap_err().contains("at least one allow rule"));
    c.block_by_default = false;
    assert!(check(&c).is_ok(), "block rules alone are fine while the default is to let out");
    let bad = |f: &dyn Fn(&mut OutConfig), word: &str| {
        let mut c = full();
        f(&mut c);
        let e = check(&c).unwrap_err();
        assert!(e.to_lowercase().contains(word), "{e}");
        assert!(generate(&c).is_err());
    };
    bad(&|c| c.rules[0].dest = "not an address".into(), "not an ip");
    bad(&|c| c.rules[0].ports = "70000".into(), "outside");
    bad(&|c| c.rules[0].iface = "eth0; drop".into(), "interface");
    bad(&|c| c.rules[0].note = "a\nb".into(), "line breaks");
    bad(&|c| c.forwards[0].to_iface = "usb0".into(), "same interface");
    bad(&|c| c.forwards[0].from_iface = String::new(), "interface");
    bad(&|c| c.port_forwards[0].to = "8.8.8.8".into(), "local network");
    bad(&|c| c.port_forwards[0].proto = Proto::Both, "tcp or udp");
    bad(&|c| c.port_forwards[0].to_port = 0, "1 to 65535");
    bad(&|c| c.rules = (0..201).map(|i| rule(i, Verdict::Allow, "", Proto::Tcp, "1")).collect(), "at most");
}

#[test]
fn nothing_typed_can_become_rule_text() {
    let mut c = full();
    c.rules[3].note = "x\" accept; flush ruleset; #".into();
    let t = generate(&c).unwrap();
    assert!(!t.contains("flush") || t.contains("x accept; flush ruleset; #"), "{t}");
    assert!(!t.contains('"') || t.matches('"').count() % 2 == 0);
    assert!(!t.lines().any(|l| l.trim_start().starts_with("flush")));
    assert!(check_comment("ok").is_ok());
}

/// Real nft (check only, inside a private user and network namespace): the generated scripts parse and are accepted. Skipped with a
/// message when `nft` or user namespaces are not there.
#[test]
fn nft_accepts_every_generated_script_in_check_only_mode() {
    let mut scripts = vec![generate(&full()).unwrap(), generate(&OutConfig { enabled: true, ..Default::default() }).unwrap()];
    let mut v6 = full();
    v6.port_forwards.retain(|p| p.to.contains(':'));
    scripts.push(generate(&v6).unwrap());
    let mut only_fw = full();
    only_fw.port_forwards.clear();
    only_fw.rules.truncate(2);
    scripts.push(generate(&only_fw).unwrap());
    let mut ran = 0;
    for s in &scripts {
        match check_with_nft(s) {
            Ok(Some(())) => ran += 1,
            Ok(None) => {
                eprintln!("skipped: nft or user namespaces are not available here");
                return;
            }
            Err(e) => panic!("nft refused:\n{e}\n--- script ---\n{s}"),
        }
    }
    assert_eq!(ran, scripts.len());
    // the check really checks: a script with a typo is refused
    assert!(check_with_nft("table inet x { chain y { type filter hook output priority filter; policy accept; meta l4proto tcp th dport banana accept } }\n").is_err());
}

#[test]
fn the_typed_settings_become_table_specs_with_ranges_reject_counter_dnat_and_invalid() {
    use crate::nl::tablegen::{encode_apply, Expr, Hook};
    assert!(to_table_specs(&OutConfig::default()).unwrap().is_empty());
    let specs = to_table_specs(&full()).unwrap();
    assert_eq!(specs.len(), 2);
    let out = &specs[0].chains[0];
    assert!(!out.policy_accept);
    assert!(out.rules.contains(&vec![Expr::CtState(crate::nl::tablegen::CT_INVALID), Expr::Drop]));
    assert_eq!(out.rules.iter().filter(|r| r.contains(&Expr::L4(6)) && r.contains(&Expr::DportRange(8000, 8100))).count(), 1);
    assert!(out.rules.iter().any(|r| r.contains(&Expr::Dport(443)) && r.last() == Some(&Expr::Accept) && r.contains(&Expr::Counter)));
    assert!(out.rules.iter().any(|r| r.contains(&Expr::Daddr("203.0.113.0".parse().unwrap(), 24)) && r.last() == Some(&Expr::Reject)));
    assert!(out.rules.iter().any(|r| matches!(r.first(), Some(Expr::OifIs(_))) && r.iter().any(|e| matches!(e, Expr::DaddrRange(..)))));
    assert_eq!(out.rules.iter().filter(|r| r.contains(&Expr::Dport(53)) && r.contains(&Expr::Counter)).count(), 2);
    assert!(!out.rules.iter().any(|r| r.contains(&Expr::Daddr("192.0.2.1".parse().unwrap(), 32))));
    assert_eq!(specs[0].chains[1].hook, Hook::Forward);
    assert!(specs[0].chains[1].rules.iter().any(|r| r.contains(&Expr::CtStatusDnat)));
    let nat = &specs[1].chains[0];
    assert_eq!(nat.hook, Hook::Prerouting);
    assert!(nat.rules.iter().any(|r| matches!(r.last(), Some(Expr::Dnat(a, 80)) if a.to_string() == "192.168.1.5")));
    for t in &specs {
        let (b, acks) = encode_apply(t, 1).unwrap();
        assert!(!b.is_empty() && acks > 3);
    }
    let bad = OutConfig { enabled: true, block_by_default: true, ..Default::default() };
    assert!(to_table_specs(&bad).is_err());
}
