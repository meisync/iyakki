//! `iyakki dev diag` (scope `debug`): ONE plain-text bundle for a person or an assistant to read when something is wrong: version, uptime,
//! memory, disks, network summary, the dev cluster's node and pod status (names and status only, through the read-only `kube` plan),
//! the audit tail, recent errors from the logs, the shell's frame lines, the sound card state and the app on the screen.
//!
//! Nothing secret goes in: no kernel command line, no environment, no file contents, no settings values, no vault, no screen text
//! beyond the app name; every line passes the credential filter ([`super::debug::scrub`]); the whole bundle is capped at [`MAX_BUNDLE`].

use std::path::Path;

use super::debug::{self, Paths};
use super::kube::{self, Kubectl};
use super::state::Snapshot;

pub const MAX_BUNDLE: usize = 96 * 1024;
const ERR_WORDS: [&str; 7] = ["error", "fail", "panic", "warn", "denied", "timeout", "oops"];

/// What the bundle is made of; each part is optional where the device may not have it.
pub struct Input<'a> {
    pub paths: &'a Paths,
    pub hostname: &'a str,
    pub data_dir: &'a Path,
    pub firewall_file: &'a Path,
    pub audit_tail: &'a dyn Fn(usize) -> Vec<String>,
    pub kube: Option<&'a dyn Kubectl>,
    /// The screen right now, if one answered.
    pub screen: Option<&'a Snapshot>,
    /// The administrator-only parts (kernel log, audit) are included.
    pub admin: bool,
}

fn gib(b: u64) -> String {
    crate::sysinfo::human_bytes(b)
}

fn section(out: &mut String, title: &str) {
    out.push_str(&format!("\n== {title} ==\n"));
}

fn lines(out: &mut String, it: impl IntoIterator<Item = String>) {
    for l in it {
        out.push_str("  ");
        out.push_str(&debug::scrub(l.trim_end()));
        out.push('\n');
    }
}

fn is_error_line(l: &str) -> bool {
    let low = l.to_lowercase();
    ERR_WORDS.iter().any(|w| low.contains(w))
}

fn tail<T: Clone>(v: &[T], n: usize) -> Vec<T> {
    v[v.len().saturating_sub(n)..].to_vec()
}

/// The text of the bundle.
pub fn bundle(i: &Input) -> String {
    let mut o = String::new();
    let v = debug::info(i.paths, i.hostname);
    let s = |p: &serde_json::Value| p.as_str().unwrap_or("").to_string();
    o.push_str(&format!("IYAKKI diagnostic bundle for {}\n", i.hostname));
    section(&mut o, "version and machine");
    lines(
        &mut o,
        [
            format!("image {} ({}) built {} from {}", s(&v["image"]["version"]), s(&v["image"]["variant"]), s(&v["image"]["built"]), s(&v["image"]["source"])),
            format!("device {} {} {}", s(&v["device"]["vendor"]), s(&v["device"]["product"]), s(&v["device"]["version"])),
            format!("kernel {} (tainted {}: {})", s(&v["kernel"]["release"]), v["kernel"]["tainted"], v["kernel"]["taint"].as_array().map(|a| a.iter().filter_map(|x| x.as_str()).collect::<Vec<_>>().join(", ")).unwrap_or_default()),
            format!("uptime {} s, load {}, cpus {}", v["uptime_secs"], v["load"].as_array().map(|a| a.iter().map(|x| format!("{:.2}", x.as_f64().unwrap_or(0.0))).collect::<Vec<_>>().join(" ")).unwrap_or_default(), v["cpus"]),
            format!("firmware: uefi {}, missing files: {}", v["firmware"]["uefi"], v["firmware"]["missing_files"].as_array().map(|a| a.iter().filter_map(|x| x.as_str()).collect::<Vec<_>>().join(", ")).unwrap_or_default()),
        ],
    );
    section(&mut o, "memory");
    let m = &v["memory"];
    let n = |k: &str| m[k].as_u64().unwrap_or(0);
    lines(&mut o, [format!("total {} available {} swap {} free of {}", gib(n("total")), gib(n("available")), gib(n("swap_free")), gib(n("swap_total")))]);
    section(&mut o, "disks and mounts");
    lines(&mut o, v["disks"].as_array().into_iter().flatten().map(|d| format!("{} {} {}{}", s(&d["name"]), gib(d["bytes"].as_u64().unwrap_or(0)), s(&d["model"]), if d["removable"] == true { " (removable)" } else { "" })));
    lines(&mut o, v["mounts"].as_array().into_iter().flatten().map(|d| format!("{} on {} ({}{}) {} free of {}", s(&d["dev"]), s(&d["mount"]), s(&d["fs"]), if d["ro"] == true { ", read-only" } else { "" }, gib(d["free"].as_u64().unwrap_or(0)), gib(d["total"].as_u64().unwrap_or(0)))));
    section(&mut o, "network");
    lines(&mut o, v["network"].as_array().into_iter().flatten().map(|d| format!("{} {}", s(&d["iface"]), s(&d["address"]))));
    lines(&mut o, [format!("device name (mDNS): {}", crate::devname::read_status(i.data_dir).line())]);
    section(&mut o, "sound");
    lines(&mut o, v["sound"].as_array().into_iter().flatten().map(|c| s(c)));
    if v["sound"].as_array().is_none_or(|a| a.is_empty()) {
        lines(&mut o, ["the kernel sees no sound card".to_string()]);
    }
    section(&mut o, "services");
    let st = debug::status(i.paths, i.data_dir, i.firewall_file);
    lines(&mut o, st["services"].as_array().into_iter().flatten().map(|r| format!("{:<26} {:<8} {}", s(&r["name"]), s(&r["state"]), s(&r["detail"]))));
    section(&mut o, "browser data");
    lines(&mut o, [crate::profilestore::summary(i.data_dir)]);
    section(&mut o, "startup");
    lines(&mut o, debug::startup_rows().iter().map(|r| format!("{:<20} {:<9} {}", s(&r["name"]), s(&r["state"]), s(&r["detail"])).trim_end().to_string()));
    section(&mut o, "dev cluster");
    match i.kube {
        None => lines(&mut o, ["(not shown: this computer does not hold the kube scope, or there is no cluster here)".to_string()]),
        Some(k) => {
            for args in [vec!["get", "nodes"], vec!["get", "pods", "-A"]] {
                let a: Vec<String> = args.iter().map(|x| x.to_string()).collect();
                match kube::plan(&a).and_then(|p| kube::run(k, &p)) {
                    Ok(r) => {
                        let text = r["text"].as_str().unwrap_or("").to_string();
                        // names and status only: the first columns of each row
                        lines(&mut o, text.lines().take(120).map(|l| l.split_whitespace().take(if args[1] == "pods" { 4 } else { 3 }).collect::<Vec<_>>().join("  ")));
                    }
                    Err(e) => lines(&mut o, [format!("kubectl {}: {e}", args.join(" "))]),
                }
            }
        }
    }
    section(&mut o, "screen");
    match i.screen {
        Some(sn) => lines(&mut o, [format!("open app: {}{}", sn.app, if sn.page.is_empty() { String::new() } else { format!(" / {}", sn.page) }), format!("focused: {}", if sn.focused.is_empty() { "(nothing)" } else { &sn.focused })]),
        None => lines(&mut o, ["(no screen answered)".to_string()]),
    }
    if i.admin {
        let shell = debug::kmsg(i.paths, 0, "[iyakki ", debug::MAX_RECORDS);
        section(&mut o, "shell frame log (newest 40)");
        lines(&mut o, tail(&shell, 40).iter().map(|k| format!("[{:>9.3}] {}", k.us as f64 / 1e6, k.msg)));
        section(&mut o, "recent errors (kernel log, shell log, boot log; newest 30)");
        let mut errs: Vec<String> = debug::kmsg(i.paths, 0, "", debug::MAX_RECORDS).iter().filter(|k| k.level <= 4 || is_error_line(&k.msg)).map(|k| format!("[{:>9.3}] {}", k.us as f64 / 1e6, k.msg)).collect();
        errs.extend(std::fs::read_to_string(&i.paths.boot_log).unwrap_or_default().lines().filter(|l| is_error_line(l)).map(|l| format!("boot: {l}")));
        lines(&mut o, tail(&errs, 30));
        section(&mut o, "audit log (newest 25)");
        lines(&mut o, (i.audit_tail)(25));
    } else {
        section(&mut o, "logs");
        lines(&mut o, ["the kernel, shell and audit logs are for administrator accounts".to_string()]);
    }
    if o.len() > MAX_BUNDLE {
        let mut cut = MAX_BUNDLE;
        while !o.is_char_boundary(cut) {
            cut -= 1;
        }
        o.truncate(cut);
        o.push_str("\n[cut]\n");
    }
    o
}
