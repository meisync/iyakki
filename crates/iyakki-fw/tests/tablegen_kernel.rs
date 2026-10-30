//! The general table writer against a real kernel, in a private network namespace made with `unshare -rn` (no root needed; the
//! machine's own firewall is never touched). Ignored by default because it needs user namespaces and nf_tables in the kernel:
//! `cargo test -p iyakki-fw --test tablegen_kernel -- --ignored --nocapture`.
//! The test re-runs itself inside the namespace. It passes with a note when `unshare` is missing or the kernel says nf_tables
//! (or the nat modules) are not there; it never needs the `nft` program.

#![cfg(target_os = "linux")]

use iyakki_fw::nl::tablegen::*;
use iyakki_fw::nl::NfSocket;

const INNER: &str = "IYAKKI_TABLEGEN_INNER";

fn guard() -> TableSpec {
    let tun = IfName::Exact("iyvpn0".into());
    TableSpec {
        name: "iyakki_vpn".into(),
        sets: vec![SetSpec { name: "vpn_dest4".into(), v4: true, timeout: true, members: vec![] }, SetSpec { name: "vpn_dest6".into(), v4: false, timeout: true, members: vec![] }],
        chains: vec![ChainSpec {
            name: "output".into(),
            kind: ChainKind::Filter,
            priority: -10,
            policy_accept: true,
            rules: vec![
                vec![Expr::OifIs(IfName::Exact("lo".into())), Expr::Accept],
                vec![Expr::L4(17), Expr::Sport(68), Expr::Dport(67), Expr::Accept],
                vec![Expr::Daddr("fe80::".parse().unwrap(), 10), Expr::L4(58), Expr::Accept],
                vec![Expr::OifIs(IfName::Prefix("cni".into())), Expr::Accept],
                vec![Expr::OifNot(tun.clone()), Expr::Daddr("203.0.113.0".parse().unwrap(), 24), Expr::Drop],
                vec![Expr::OifNot(tun.clone()), Expr::DaddrSet("vpn_dest4".into()), Expr::Drop],
                vec![Expr::OifNot(tun), Expr::DaddrSet("vpn_dest6".into()), Expr::Drop],
            ],
        }],
    }
}

fn dns() -> TableSpec {
    TableSpec {
        name: "iyakki_vpn_dns".into(),
        sets: vec![],
        chains: vec![ChainSpec {
            name: "output".into(),
            kind: ChainKind::Nat,
            priority: -100,
            policy_accept: true,
            rules: vec![vec![Expr::MarkIs(0x4956_0001), Expr::Return], vec![Expr::L4(17), Expr::Dport(53), Expr::RedirectPort(15353)], vec![Expr::L4(6), Expr::Dport(53), Expr::RedirectPort(15353)]],
        }],
    }
}

fn inside() -> Result<(), String> {
    for m in ["nf_tables", "nft_chain_nat", "nft_redir"] {
        let _ = std::process::Command::new("modprobe").arg(m).output();
    }
    let mut s = NfSocket::open(5).map_err(|e| e.to_string())?;
    s.apply_table(&guard()).map_err(|e| format!("guard: {e}"))?;
    assert!(s.table_exists("iyakki_vpn").map_err(|e| e.to_string())?);
    s.apply_table(&guard()).map_err(|e| format!("guard again (replace): {e}"))?;
    s.set_elem("iyakki_vpn", "vpn_dest4", "198.51.100.7".parse().unwrap(), Some(60), true).map_err(|e| format!("add element: {e}"))?;
    s.set_elem("iyakki_vpn", "vpn_dest4", "198.51.100.7".parse().unwrap(), Some(60), true).map_err(|e| format!("add element twice: {e}"))?;
    s.set_elem("iyakki_vpn", "vpn_dest6", "2001:db8::7".parse().unwrap(), Some(60), true).map_err(|e| format!("add v6 element: {e}"))?;
    s.set_elem("iyakki_vpn", "vpn_dest4", "198.51.100.7".parse().unwrap(), None, false).map_err(|e| format!("delete element: {e}"))?;
    assert!(s.set_elem("iyakki_vpn", "vpn_dest4", "198.51.100.7".parse().unwrap(), None, false).is_err(), "deleting an absent element is an error the caller may ignore");
    match s.apply_table(&dns()) {
        Ok(()) => assert!(s.table_exists("iyakki_vpn_dns").map_err(|e| e.to_string())?),
        Err(e) => eprintln!("the nat table was refused (module missing?): {e}"),
    }
    if std::env::var("IYAKKI_TABLEGEN_KEEP").is_ok() {
        return Ok(()); // leave the tables for `nft list ruleset` in a shell of the same namespace
    }
    s.remove_table("iyakki_vpn").map_err(|e| e.to_string())?;
    s.remove_table("iyakki_vpn_dns").map_err(|e| e.to_string())?;
    assert!(!s.table_exists("iyakki_vpn").map_err(|e| e.to_string())?);
    assert!(!s.table_exists("iyakki_vpn_dns").map_err(|e| e.to_string())?);
    s.remove_table("iyakki_vpn").map_err(|e| format!("removing an absent table must not fail: {e}"))?;
    Ok(())
}

#[test]
#[ignore = "needs user namespaces and nf_tables in the host kernel; loads rules into a private namespace"]
fn the_guard_and_the_dns_redirect_load_into_a_real_kernel_and_elements_come_and_go() {
    if std::env::var(INNER).is_ok() {
        inside().unwrap();
        return;
    }
    let exe = std::env::current_exe().unwrap();
    let out = match std::process::Command::new("unshare").args(["-rn"]).arg(&exe).args(["--exact", "the_guard_and_the_dns_redirect_load_into_a_real_kernel_and_elements_come_and_go", "--ignored", "--nocapture"]).env(INNER, "1").output() {
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
