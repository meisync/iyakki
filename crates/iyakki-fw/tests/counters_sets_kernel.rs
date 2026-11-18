//! Per-rule counters and address sets against a real kernel, in a private network namespace made with `unshare -rn` (no root
//! needed; the machine's own firewall is never touched). Ignored by default because it needs user namespaces and nf_tables:
//! `cargo test -p iyakki-fw --test counters_sets_kernel -- --ignored --nocapture`. It never needs the `nft` program.
//! Not run on the build machines yet: a person with a Linux kernel has to run it once (the matrix row says so).

#![cfg(target_os = "linux")]

use iyakki_fw::model::{Config, Kind, Proto, Resolved, Rule};
use iyakki_fw::nl::NfSocket;
use iyakki_fw::ruleset::{build, rule_counts, Ctx, Step, SET_MIN};

const INNER: &str = "IYAKKI_COUNTERS_SETS_INNER";

fn inside() -> Result<(), String> {
    let mut c = Config::default();
    c.presets.clear();
    let mut h = Rule::new(Kind::Host, "big.example.com");
    h.proto = Proto::Tcp;
    h.ports = "22".into();
    c.add(h);
    c.add(Rule::new(Kind::Ip, "198.51.100.7"));
    let mut r = Resolved::default();
    r.addrs.insert("big.example.com".into(), (1..=(SET_MIN as u8 + 8)).map(|i| format!("198.51.100.{i}").parse().unwrap()).collect());
    let rs = build(&c, &r, &Ctx::default())?.ruleset;
    assert_eq!(rs.sets.len(), 1, "the host's answers became a set");
    let mut s = NfSocket::open(5).map_err(|e| e.to_string())?;
    s.replace(&rs).map_err(|e| format!("replace with a set: {e}"))?;
    s.replace(&rs).map_err(|e| format!("replace again: {e}"))?;
    let per = s.rule_counters().map_err(|e| e.to_string())?;
    assert_eq!(per.len(), rs.rules.len(), "one entry per rule, in order");
    for (r, c) in rs.rules.iter().zip(&per) {
        assert_eq!(r.steps.contains(&Step::Counter), c.is_some(), "{}", r.note);
        assert!(c.is_none_or(|(p, _)| p == 0), "nothing has arrived yet: {}", r.note);
    }
    assert!(rule_counts(&rs, &per).iter().any(|c| c.note == "rule 1"));
    assert_eq!(s.counters().map_err(|e| e.to_string())?, (0, 0), "the blocked total is the last counted rule");
    s.remove().map_err(|e| e.to_string())?;
    Ok(())
}

#[test]
#[ignore = "needs user namespaces and nf_tables in the host kernel; loads rules into a private namespace"]
fn rules_with_counters_and_a_set_load_into_a_real_kernel_and_the_counters_read_back_in_order() {
    if std::env::var(INNER).is_ok() {
        inside().unwrap();
        return;
    }
    let exe = std::env::current_exe().unwrap();
    let out = match std::process::Command::new("unshare").args(["-rn"]).arg(&exe).args(["--exact", "rules_with_counters_and_a_set_load_into_a_real_kernel_and_the_counters_read_back_in_order", "--ignored", "--nocapture"]).env(INNER, "1").output() {
        Ok(o) => o,
        Err(e) => {
            eprintln!("skipped: cannot run unshare ({e})");
            return;
        }
    };
    let text = format!("{}{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr));
    if !out.status.success() && (text.contains("unshare failed") || text.contains("not available in this kernel") || text.contains("Operation not permitted")) {
        eprintln!("skipped: {text}");
        return;
    }
    assert!(out.status.success(), "{text}");
}
