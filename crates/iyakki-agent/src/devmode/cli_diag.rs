//! The diagnosis commands of `iyakki dev`: `kube`, `state`, `inbox`, `diag`, and the scope list of `whoami`. Each is one signed request;
//! the device decides what is allowed (scope, verbs, secrets), so nothing here can be talked past.

use serde_json::json;

use super::cli::Args;
use super::client::DevClient;
use super::scopes::Scope;

pub const USAGE: &str = "  kube get|describe|logs|top|events|api-resources|version|auth can-i ...   look at the dev cluster, read-only (scope kube; Secrets blocked)
  state [--json]                      what the screen shows: app, page, focused field, texts and the buttons (scope debug; secret fields hidden)
  input click \"LABEL\"                 press the button called LABEL (scope input; never over a secret field)
  inbox [--bodies] [--json]           the inbox: counts by kind and source, titles (bodies need the scope files:system)
  diag [-o FILE]                      one bundle: version, uptime, memory, disks, network, cluster, audit tail, recent errors, sound (scope debug)";

/// `args` raw (`kube` and what follows, without `--device X` / `--home X`, which belong to the CLI itself).
pub fn kube_args(raw: &[String]) -> Vec<String> {
    let at = raw.iter().position(|x| x == "kube").map_or(0, |i| i + 1);
    let mut v = Vec::new();
    let mut it = raw[at..].iter();
    while let Some(x) = it.next() {
        if x == "--device" || x == "--home" {
            it.next();
        } else if x.starts_with("--device=") || x.starts_with("--home=") {
        } else {
            v.push(x.clone());
        }
    }
    v
}

pub(super) fn run(cmd: &str, raw: &[String], args: &Args, connect: &dyn Fn() -> Result<DevClient, String>, out: &mut dyn FnMut(&str)) -> Option<Result<(), String>> {
    Some(match cmd {
        "kube" => kube(&kube_args(raw), connect, out),
        "state" => connect().and_then(|c| state(&c, args, out)),
        "inbox" => connect().and_then(|c| inbox(&c, args, out)),
        "diag" => connect().and_then(|c| diag(&c, args, out)),
        _ => return None,
    })
}

fn kube(a: &[String], connect: &dyn Fn() -> Result<DevClient, String>, out: &mut dyn FnMut(&str)) -> Result<(), String> {
    if a.is_empty() || a[0] == "help" || a[0] == "--help" {
        out(&super::kube::usage());
        return Ok(());
    }
    // the same checks run here first: a refusal does not need the network
    super::kube::plan(a)?;
    let v = connect()?.call("POST", "/v1/dev/kube", &[], json!({ "args": a }).to_string().as_bytes())?;
    out(v["text"].as_str().unwrap_or("").trim_end_matches('\n'));
    if v["truncated"] == true {
        out("(the answer was cut)");
    }
    match v["code"].as_i64() {
        Some(0) | None => Ok(()),
        Some(c) => Err(format!("kubectl exited with {c}")),
    }
}

fn state(c: &DevClient, args: &Args, out: &mut dyn FnMut(&str)) -> Result<(), String> {
    let v = c.call("GET", "/v1/dev/state", &[], b"")?;
    if args.has("--json") {
        let mut v2 = v.clone();
        if let Some(o) = v2.as_object_mut() {
            o.remove("text");
        }
        out(&serde_json::to_string_pretty(&v2).unwrap_or_default());
    } else {
        out(v["text"].as_str().unwrap_or("").trim_end_matches('\n'));
    }
    Ok(())
}

fn inbox(c: &DevClient, args: &Args, out: &mut dyn FnMut(&str)) -> Result<(), String> {
    let bodies = args.has("--bodies");
    let v = c.call("GET", "/v1/dev/inbox", &[("bodies", if bodies { "1" } else { "0" })], b"")?;
    if args.has("--json") {
        out(&serde_json::to_string_pretty(&v).unwrap_or_default());
        return Ok(());
    }
    out(&format!("{} notifications, {} unread", v["total"], v["unread"]));
    let join = |k: &str, f: &str| v[k].as_array().into_iter().flatten().map(|x| format!("{} {}", x[f].as_str().unwrap_or(""), x["count"])).collect::<Vec<_>>().join(", ");
    out(&format!("by kind: {}", join("by_kind", "kind")));
    out(&format!("by source: {}", join("by_source", "source")));
    for n in v["items"].as_array().into_iter().flatten() {
        out(&format!("{} {} {:<8} {:<16} {}", super::stamp(n["time"].as_i64().unwrap_or(0)), if n["read"] == true { " " } else { "*" }, n["kind"].as_str().unwrap_or(""), n["source"].as_str().unwrap_or(""), n["title"].as_str().unwrap_or("")));
        if bodies {
            out(&format!("      {}", n["body"].as_str().unwrap_or("")));
        }
    }
    if v["total"].as_u64().unwrap_or(0) > v["shown"].as_u64().unwrap_or(0) {
        out(&format!("(the newest {} are shown)", v["shown"]));
    }
    Ok(())
}

fn diag(c: &DevClient, args: &Args, out: &mut dyn FnMut(&str)) -> Result<(), String> {
    let v = c.call("GET", "/v1/dev/diag", &[], b"")?;
    let text = v["text"].as_str().unwrap_or("");
    match args.get("-o") {
        Some(f) => {
            std::fs::write(f, text).map_err(|e| format!("{f}: {e}"))?;
            out(&format!("Saved {f} ({} KB).", text.len().div_ceil(1024)));
        }
        None => out(text.trim_end_matches('\n')),
    }
    Ok(())
}

/// The lines `whoami` prints for the scopes: every scope, held or not, with a one-line explanation.
pub fn scope_lines(held: &[String]) -> Vec<String> {
    let mut v = vec!["Scopes (an administrator grants more on the device: Settings, Developer mode):".to_string()];
    for s in Scope::ALL {
        let on = held.iter().any(|h| h == s.id()) || (s == Scope::Power && held.iter().any(|h| h == "shell"));
        v.push(format!("  {} {:<15} {}", if on { "[x]" } else { "[ ]" }, s.id(), s.describe()));
    }
    v
}
