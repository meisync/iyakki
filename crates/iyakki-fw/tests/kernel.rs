//! Real nf_tables, in a private network namespace made with `unshare -rn` (no root needed, the machine's own firewall and
//! network are never touched). Ignored by default; run with `cargo test -p iyakki-fw --test kernel -- --ignored`.
//! Skipped (passes with a note) when `unshare`, `nft` or `python3` are missing or the kernel has no nf_tables.

use std::process::Command;

fn have(tool: &str) -> bool {
    Command::new("sh").args(["-c", &format!("command -v {tool} >/dev/null")]).status().is_ok_and(|s| s.success())
}

fn demo() -> std::path::PathBuf {
    let exe = std::env::current_exe().unwrap();
    // the profile folder (target/debug): older cargo puts the test program in deps/, newer in build/PKG/HASH/out/
    exe.ancestors().find(|d| d.file_name().is_some_and(|n| n == "debug" || n == "release")).expect("the profile folder above the test program").join("examples/fw_demo")
}

fn userns_works() -> bool {
    Command::new("unshare").args(["-rn", "true"]).status().is_ok_and(|s| s.success())
}

fn run_in_ns(script: &str) -> Option<(bool, String)> {
    if !(have("unshare") && have("nft") && have("python3") && have("ip") && have("nsenter") && userns_works()) {
        eprintln!("skipped: needs unshare, nft, python3, ip, nsenter and user namespaces");
        return None;
    }
    let out = Command::new("unshare").args(["-rn", "sh", "-c", script]).output().unwrap();
    Some((out.status.success(), format!("{}{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr))))
}

#[test]
#[ignore]
fn the_ruleset_loads_into_nf_tables_is_listed_by_nft_and_can_be_removed() {
    let d = demo();
    let script = format!("{d} apply && nft list table inet iyakki_fw && {d} remove && ! nft list table inet iyakki_fw 2>/dev/null && {d} apply && {d} apply && nft list tables", d = d.display());
    let Some((ok, out)) = run_in_ns(&script) else { return };
    if !ok && (out.contains("not available in this kernel") || out.contains("error 22")) {
        eprintln!("skipped: {out}");
        return;
    }
    assert!(ok, "{out}");
    let without_counters = {
        let mut t = String::new();
        let mut rest = out.as_str();
        while let Some(i) = rest.find(" counter packets ") {
            t.push_str(&rest[..i]);
            let after = &rest[i + " counter packets ".len()..];
            // skip "<n> bytes <n>"
            let words: Vec<&str> = after.splitn(4, ' ').collect();
            let used = if words.len() >= 4 && words[1] == "bytes" { words[0].len() + 1 + words[1].len() + 1 + words[2].len() } else { 0 };
            rest = &after[used..];
        }
        t.push_str(rest);
        t
    };
    for want in ["policy drop", "ct state established,related accept", "iifname \"lo\" accept", "ip saddr 203.0.113.0/24 tcp dport 8000-8100 accept", "ip saddr 198.51.100.10-198.51.100.20 accept", "ip6 saddr 2001:db8:1::/48 accept", "limit rate 10/second burst 5 packets log group 217", "counter packets 0 bytes 0"] {
        // nft 1.1 prints `counter packets 0 bytes 0` inside a rule that carries a counter (older nft omits it for an unused one): match with and without it
        assert!(out.contains(want) || without_counters.contains(want), "missing {want:?} in:\n{out}");
    }
    assert!(out.contains("table inet iyakki_fw"), "applying twice replaces the table, it does not stack");
}

#[test]
#[ignore]
fn a_second_namespace_gets_in_only_where_the_rules_allow_and_blocked_attempts_are_counted_and_logged() {
    let script = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/behaviour.sh");
    let log = std::env::temp_dir().join(format!("iyakki-fw-watch-{}", std::process::id()));
    let cmd = format!("timeout 60 bash {} {} {}", script.display(), demo().display(), log.display());
    let Some((ok, out)) = run_in_ns(&cmd) else { return };
    assert!(ok, "{out}");
    let _ = std::fs::remove_file(&log);
    assert!(out.contains("9000 blocked TimeoutError"), "a port nobody allowed is silently dropped:\n{out}");
    assert!(!out.contains("8000 blocked TimeoutError"), "an allowed range is not dropped:\n{out}");
    assert!(out.contains("blocked 203.0.113.9 tcp port 9000"), "the blocked attempt is in the log:\n{out}");
    assert!(!out.contains("(0, 0)"), "the blocked packets were counted:\n{out}");
}
