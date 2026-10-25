//! `iyakki dev patch build|show|apply|list|remove` (docs/development/REMOTE-PATCH.md): the computer's side of a hot patch. The device
//! decides everything (scope, approval, paths); this only builds and checks the pack locally and sends it in a few signed requests.

use std::path::{Path, PathBuf};

use serde_json::Value;

use super::cli::Args;
use super::cli_adb::{ask_grant, CHUNK};
use super::client::DevClient;
use super::patch;

pub const USAGE: &str = "usage: iyakki dev patch <build DIR -o PACK | show PACK|DIR | apply PACK|DIR [--live|--next-boot] [--shell] | list | remove ID>";

/// Returns `Some(result)` when `cmd` is `patch`.
pub(super) fn run(cmd: &str, args: &Args, connect: &dyn Fn() -> Result<DevClient, String>, out: &mut dyn FnMut(&str)) -> Option<Result<(), String>> {
    if cmd != "patch" {
        return None;
    }
    let arg = |i: usize| args.pos.get(i).map(String::as_str);
    Some(match arg(1) {
        Some("build") => build(args, out),
        Some("show") => arg(2).ok_or_else(|| USAGE.to_string()).and_then(|p| show(Path::new(p), out)),
        Some("apply") => arg(2).ok_or_else(|| USAGE.to_string()).and_then(|p| connect().and_then(|c| apply(&c, Path::new(p), args, out))),
        Some("list") => connect().and_then(|c| list(&c, out)),
        Some("remove") => arg(2).ok_or_else(|| USAGE.to_string()).and_then(|id| connect().and_then(|c| remove(&c, id, args, out))),
        _ => Err(USAGE.into()),
    })
}

fn build(args: &Args, out: &mut dyn FnMut(&str)) -> Result<(), String> {
    let dir = args.pos.get(2).ok_or_else(|| USAGE.to_string())?;
    let to = PathBuf::from(args.get("-o").ok_or("where should the pack go? -o PACK")?);
    let bytes = patch::build(Path::new(dir))?;
    std::fs::write(&to, &bytes).map_err(|e| format!("{}: {e}", to.display()))?;
    let p = patch::decode(&bytes)?;
    out(&format!("Built {} ({}): {} file(s), {} KB, in {}", p.manifest.id, p.manifest.title, p.manifest.ops.len(), bytes.len().div_ceil(1024), to.display()));
    Ok(())
}

fn show(path: &Path, out: &mut dyn FnMut(&str)) -> Result<(), String> {
    let p = patch::load(path)?;
    let m = &p.manifest;
    out(&format!("{}  {}", m.id, m.title));
    if !m.description.is_empty() {
        out(&m.description);
    }
    out(&format!("for root: {}   obsolete from: {}", m.base, m.obsolete_from.as_deref().unwrap_or("never")));
    for o in &m.ops {
        let what = match (o.action, o.kind) {
            (patch::Action::Delete, _) => "delete".to_string(),
            (a, patch::Kind::Symlink) => format!("{a:?} link -> {}", o.link.as_deref().unwrap_or("")).to_lowercase(),
            (a, _) => format!("{a:?} {} bytes", o.size).to_lowercase(),
        };
        let note = match patch::path_reach(&o.path) {
            Ok(patch::Reach::Normal) => "",
            Ok(patch::Reach::Wide) => "  [needs the shell scope and its approval]",
            Ok(patch::Reach::Reboot) => "  [next start only; needs the shell scope and its approval]",
            Err(_) => "  [NEVER allowed]",
        };
        out(&format!("  {:<8} {}{note}", what.split(' ').next().unwrap_or(""), o.path));
    }
    for s in &m.pre {
        out(&format!("  before: {s:?}"));
    }
    for s in &m.post {
        out(&format!("  after:  {s:?}"));
    }
    Ok(())
}

/// A call that may need the approval card: ask for it once and repeat.
fn approved(c: &DevClient, method: &str, path: &str, query: &[(&str, &str)], body: &[u8], why: &str, out: &mut dyn FnMut(&str)) -> Result<(u16, Value), String> {
    let (code, v) = c.call_status(method, path, query, body)?;
    if code == 403 && v["error"].as_str().is_some_and(|e| e.starts_with("not approved")) {
        ask_grant(c, "patch", 10, false, why, 125, out)?;
        return c.call_status(method, path, query, body);
    }
    Ok((code, v))
}

fn fail(code: u16, v: &Value) -> String {
    v["error"].as_str().map(str::to_string).unwrap_or_else(|| format!("the device answered {code}"))
}

fn apply(c: &DevClient, path: &Path, args: &Args, out: &mut dyn FnMut(&str)) -> Result<(), String> {
    let pack = patch::load(path)?;
    let m = &pack.manifest;
    let mode = if args.has("--next-boot") { "next-boot" } else { "live" };
    if args.has("--live") && args.has("--next-boot") {
        return Err("--live and --next-boot exclude each other".into());
    }
    if args.has("--shell") {
        ask_grant(c, "shell", 10, false, &format!("patch {}", m.id), 125, out)?;
    }
    let body = serde_json::to_vec(m).map_err(|e| e.to_string())?;
    let why = format!("patch {}", m.id);
    let (code, plan) = approved(c, "POST", "/v1/dev/patch/plan", &[], &body, &why, out)?;
    if code != 200 {
        return Err(fail(code, &plan));
    }
    for n in plan["notes"].as_array().into_iter().flatten().filter_map(Value::as_str) {
        out(&format!("  note: {n}"));
    }
    out(&format!("Patch {}: {} file(s), {} bytes. Sending...", m.id, m.ops.len(), plan["bytes"]));
    for (i, bytes) in &pack.content {
        let mut offset = 0usize;
        loop {
            let end = (offset + CHUNK).min(bytes.len());
            let (code, v) = c.call_status("POST", "/v1/dev/patch/stage", &[("id", &m.id), ("file", &i.to_string()), ("offset", &offset.to_string())], &bytes[offset..end])?;
            if code != 200 {
                return Err(fail(code, &v));
            }
            offset = end;
            if v["done"].as_bool() == Some(true) || offset >= bytes.len() {
                break;
            }
        }
    }
    let (code, v) = approved(c, "POST", "/v1/dev/patch/apply", &[("id", &m.id), ("mode", mode)], b"", &why, out)?;
    if code != 200 {
        return Err(fail(code, &v));
    }
    out(v["message"].as_str().unwrap_or("done"));
    for w in v["warnings"].as_array().into_iter().flatten().filter_map(Value::as_str) {
        out(&format!("  warning: {w}"));
    }
    if v["needs_reboot"] == true {
        out("Restart the device for the rest to take effect (iyakki dev reboot).");
    }
    Ok(())
}

fn list(c: &DevClient, out: &mut dyn FnMut(&str)) -> Result<(), String> {
    let (code, v) = c.call_status("GET", "/v1/dev/patch/list", &[], b"")?;
    if code != 200 {
        return Err(fail(code, &v));
    }
    let items = v["patches"].as_array().cloned().unwrap_or_default();
    if items.is_empty() {
        out("No patches.");
    }
    for i in items {
        out(&format!("{:<24} {:<11} {:>3} file(s)  by {:<12} {}{}", i["id"].as_str().unwrap_or(""), i["status"].as_str().unwrap_or(""), i["files"], i["by"].as_str().unwrap_or(""), i["title"].as_str().unwrap_or(""), i["reason"].as_str().filter(|r| !r.is_empty()).map(|r| format!("  ({r})")).unwrap_or_default()));
    }
    if let Some(d) = v["guard"]["disabled"].as_str() {
        out(&format!("GUARD: patches are switched off at start. {}", d.trim()));
    }
    if let Some(s) = v["skipped_at_boot"].as_str().filter(|s| !s.trim().is_empty()) {
        out(&format!("Skipped at the last start:\n{}", s.trim_end()));
    }
    Ok(())
}

fn remove(c: &DevClient, id: &str, _args: &Args, out: &mut dyn FnMut(&str)) -> Result<(), String> {
    let (code, v) = approved(c, "POST", "/v1/dev/patch/remove", &[("id", id)], b"", &format!("remove patch {id}"), out)?;
    if code != 200 {
        return Err(fail(code, &v));
    }
    out(v["message"].as_str().unwrap_or("removed"));
    Ok(())
}
