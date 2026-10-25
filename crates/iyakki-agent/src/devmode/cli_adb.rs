//! The `adb`-like commands of `iyakki dev` beyond install/run/logs/screenshot: `info`, `ps`, `dmesg`, `logs --system`, `status`,
//! `files`, `input`, and the screenshot options. Each is a few signed requests to the device ([`super::api`]) and some formatting;
//! no rule lives here (the device decides), so a computer cannot talk its way past a scope.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use serde_json::Value;

use super::cli::Args;
use super::client::DevClient;
use super::files::{MAX_FILE, sha256_file};

/// A chunk of a transfer: below the listener's 12 MiB body limit, big enough for a LAN.
pub const CHUNK: usize = 4 << 20;
/// Most frames `screenshot --record` takes.
pub const MAX_FRAMES: u64 = 120;
/// Most files one recursive push or pull moves.
pub const MAX_TREE: usize = 5000;

fn s<'a>(v: &'a Value, k: &str) -> &'a str {
    v[k].as_str().unwrap_or("")
}

fn gib(b: u64) -> String {
    crate::sysinfo::human_bytes(b)
}

/// Returns `Some(result)` when `cmd` is one of this module's commands.
pub(super) fn run(cmd: &str, args: &Args, connect: &dyn Fn() -> Result<DevClient, String>, out: &mut dyn FnMut(&str)) -> Option<Result<(), String>> {
    let arg = |i: usize| args.pos.get(i).map(String::as_str);
    Some(match cmd {
        "info" => connect().and_then(|c| info(&c, out)),
        "ps" => connect().and_then(|c| ps(&c, out)),
        "dmesg" => connect().and_then(|c| dmesg(&c, args, out)),
        "logs" if args.has("--system") => connect().and_then(|c| syslog(&c, args, out)),
        "status" => connect().and_then(|c| status(&c, out)),
        "files" | "file" => connect().and_then(|c| files(&c, args, out)),
        "input" => connect().and_then(|c| input(&c, args, out)),
        "update-pack" => update_pack(args, out),
        "update" => connect().and_then(|c| update(&c, args, out)),
        "shell" => connect().and_then(|c| shell_cmd(&c, args, out)),
        "forward" => connect().and_then(|c| forward_cmd(&c, args, out)),
        "reboot" | "poweroff" => connect().and_then(|c| power(&c, cmd, args, out)),
        "config" => connect().and_then(|c| config(&c, args, out)),
        "app" => connect().and_then(|c| app(&c, args, out)),
        "screenshot" if args.has("--record") || args.get("--display").is_some() => connect().and_then(|c| screenshots(&c, args, out)),
        _ => {
            let _ = arg;
            return None;
        }
    })
}

fn info(c: &DevClient, out: &mut dyn FnMut(&str)) -> Result<(), String> {
    let v = c.call("GET", "/v1/dev/info", &[], b"")?;
    let (d, i, k, m) = (&v["device"], &v["image"], &v["kernel"], &v["memory"]);
    out(&format!("{}", s(&v, "hostname")));
    out(&format!("  device     {} {} {}", s(d, "vendor"), s(d, "product"), s(d, "version")).trim_end().to_string());
    if !s(d, "board").is_empty() || !s(d, "bios").is_empty() {
        out(&format!("  board/bios {} / {}", s(d, "board"), s(d, "bios")));
    }
    out(&format!("  image      IYAKKI {} · variant {} · built {} · {}", s(i, "version"), if s(i, "variant").is_empty() { "unknown" } else { s(i, "variant") }, s(i, "built"), if i["from_image"].as_bool() == Some(true) { "from an image" } else { "not running from an image" }));
    out(&format!("  kernel     {} · {} modules loaded", s(k, "release"), k["modules"]));
    let taint: Vec<&str> = k["taint"].as_array().map(|a| a.iter().filter_map(|x| x.as_str()).collect()).unwrap_or_default();
    out(&format!("  taint      {}", if taint.is_empty() { "none".to_string() } else { format!("{} ({})", k["tainted"], taint.join(", ")) }));
    out(&format!("  uptime     {} · load {:.2} {:.2} {:.2} · {} cpu", crate::sysinfo::human_duration(v["uptime_secs"].as_u64().unwrap_or(0)), v["load"][0].as_f64().unwrap_or(0.0), v["load"][1].as_f64().unwrap_or(0.0), v["load"][2].as_f64().unwrap_or(0.0), v["cpus"]));
    out(&format!("  memory     {} total, {} available · swap {} ({} free)", gib(m["total"].as_u64().unwrap_or(0)), gib(m["available"].as_u64().unwrap_or(0)), gib(m["swap_total"].as_u64().unwrap_or(0)), gib(m["swap_free"].as_u64().unwrap_or(0))));
    out(&format!("  firmware   {} · {}", if v["firmware"]["uefi"].as_bool() == Some(true) { "UEFI" } else { "BIOS" }, match v["firmware"]["missing_files"].as_array() {
        Some(a) if !a.is_empty() => format!("{} firmware file(s) the kernel could not load: {}", a.len(), a.iter().filter_map(|x| x.as_str()).collect::<Vec<_>>().join(", ")),
        _ => "no missing firmware files in the kernel log".to_string(),
    }));
    for g in v["gpu"].as_array().into_iter().flatten() {
        out(&format!("  gpu        {} driver {} ({}:{}) outputs: {}", s(g, "card"), if s(g, "driver").is_empty() { "none" } else { s(g, "driver") }, s(g, "vendor"), s(g, "device"), g["connected"].as_array().map(|a| a.iter().filter_map(|x| x.as_str()).collect::<Vec<_>>().join(" ")).filter(|x| !x.is_empty()).unwrap_or_else(|| "none connected".into())));
    }
    let snd: Vec<&str> = v["sound"].as_array().map(|a| a.iter().filter_map(|x| x.as_str()).collect()).unwrap_or_default();
    out(&format!("  sound      {}", if snd.is_empty() { "no sound card".to_string() } else { snd.join(" | ") }));
    for d in v["disks"].as_array().into_iter().flatten() {
        out(&format!("  disk       {:<10} {:>9} {}{}", s(d, "name"), gib(d["bytes"].as_u64().unwrap_or(0)), s(d, "model"), if d["removable"].as_bool() == Some(true) { " (removable)" } else { "" }));
    }
    for m in v["mounts"].as_array().into_iter().flatten() {
        out(&format!("  mount      {:<22} {:<5} {} of {} free{}", s(m, "mount"), s(m, "fs"), gib(m["free"].as_u64().unwrap_or(0)), gib(m["total"].as_u64().unwrap_or(0)), if m["ro"].as_bool() == Some(true) { " (read-only)" } else { "" }));
    }
    for n in v["network"].as_array().into_iter().flatten() {
        out(&format!("  network    {:<10} {}", s(n, "iface"), s(n, "address")));
    }
    Ok(())
}

fn ps(c: &DevClient, out: &mut dyn FnMut(&str)) -> Result<(), String> {
    let v = c.call("GET", "/v1/dev/ps", &[], b"")?;
    out(&format!("{:>6} {:>6} {:<10} {:<2} {:>9} {:>8}  {}", "PID", "PPID", "USER", "S", "RSS", "CPU s", "COMMAND"));
    for p in v["processes"].as_array().into_iter().flatten() {
        let cmd = if s(p, "cmd").is_empty() { format!("[{}]", s(p, "name")) } else { s(p, "cmd").to_string() };
        out(&format!("{:>6} {:>6} {:<10} {:<2} {:>9} {:>8}  {}", p["pid"], p["ppid"], s(p, "user"), s(p, "state"), gib(p["rss"].as_u64().unwrap_or(0)), p["cpu_secs"], cmd));
    }
    if v["all"].as_bool() != Some(true) {
        out("(only this account's processes: the full list is for administrator accounts)");
    }
    Ok(())
}

fn dmesg(c: &DevClient, args: &Args, out: &mut dyn FnMut(&str)) -> Result<(), String> {
    let grep = args.get("--grep").unwrap_or("").to_string();
    let max = args.get("-n").unwrap_or("500").to_string();
    let mut since = 0u64;
    loop {
        let v = c.call("GET", "/v1/dev/dmesg", &[("since", &since.to_string()), ("grep", &grep), ("max", &max)], b"")?;
        for r in v["records"].as_array().into_iter().flatten() {
            out(&format!("[{:>10.6}] {}", r["us"].as_f64().unwrap_or(0.0) / 1e6, s(r, "msg")));
        }
        since = v["next"].as_u64().unwrap_or(since);
        if !args.has("--follow") && !args.has("-f") {
            return Ok(());
        }
        std::thread::sleep(Duration::from_secs(1));
    }
}

fn syslog(c: &DevClient, args: &Args, out: &mut dyn FnMut(&str)) -> Result<(), String> {
    let n = args.get("-n").unwrap_or("200").to_string();
    for src in ["shell", "boot", "audit"] {
        if let Some(only) = args.get("--source") {
            if only != src {
                continue;
            }
        }
        let v = c.call("GET", "/v1/dev/syslog", &[("src", src), ("n", &n)], b"")?;
        out(&format!("--- {src} ({} lines)", v["lines"]));
        if !s(&v, "text").is_empty() {
            out(s(&v, "text"));
        }
    }
    Ok(())
}

fn status(c: &DevClient, out: &mut dyn FnMut(&str)) -> Result<(), String> {
    let v = c.call("GET", "/v1/dev/status", &[], b"")?;
    out(&format!("{:<26} {:<10} {}", "SERVICE", "STATE", "DETAIL"));
    for r in v["services"].as_array().into_iter().flatten() {
        out(&format!("{:<26} {:<10} {}", s(r, "name"), s(r, "state"), s(r, "detail")));
    }
    Ok(())
}

// -------------------------------------------------------------------------------------------- files

fn files(c: &DevClient, args: &Args, out: &mut dyn FnMut(&str)) -> Result<(), String> {
    let sub = args.pos.get(1).map(String::as_str).ok_or("usage: iyakki dev files ls|push|pull|rm|mkdir|stat PATH")?;
    let a2 = args.pos.get(2).map(String::as_str);
    match sub {
        "ls" => {
            let v = c.call("GET", "/v1/dev/files/ls", &[("path", a2.unwrap_or("~"))], b"")?;
            out(&format!("{}", s(&v, "path")));
            for e in v["entries"].as_array().into_iter().flatten() {
                out(&format!("  {:<5} {:>10} {:o} {}{}", s(e, "kind"), if s(e, "kind") == "file" { gib(e["size"].as_u64().unwrap_or(0)) } else { String::new() }, e["mode"].as_u64().unwrap_or(0), s(e, "name"), if s(e, "kind") == "dir" { "/" } else { "" }));
            }
            if v["truncated"].as_bool() == Some(true) {
                out("  (more entries than are listed)");
            }
            Ok(())
        }
        "stat" => {
            let v = c.call("GET", "/v1/dev/files/stat", &[("path", a2.ok_or("usage: iyakki dev files stat PATH")?), ("hash", "1")], b"")?;
            out(&serde_json::to_string_pretty(&v).unwrap_or_default());
            Ok(())
        }
        "mkdir" => {
            let v = c.call("POST", "/v1/dev/files/mkdir", &[("path", a2.ok_or("usage: iyakki dev files mkdir PATH")?)], b"")?;
            out(&format!("Made {}", s(&v, "path")));
            Ok(())
        }
        "rm" => {
            let v = c.call("POST", "/v1/dev/files/rm", &[("path", a2.ok_or("usage: iyakki dev files rm PATH [-r]")?), ("recursive", if args.has("-r") { "1" } else { "0" })], b"")?;
            out(&format!("Removed {}", s(&v, "removed")));
            Ok(())
        }
        "push" => {
            let (local, remote) = (a2.ok_or("usage: iyakki dev files push LOCAL REMOTE [--force]")?, args.pos.get(3).map(String::as_str).ok_or("usage: iyakki dev files push LOCAL REMOTE [--force]")?);
            let lp = Path::new(local);
            if lp.is_dir() {
                let mut n = 0usize;
                push_tree(c, lp, remote, args.has("--force"), &mut n, out)?;
                out(&format!("Pushed {n} file(s) to {remote}."));
            } else {
                push_file(c, lp, &remote_name(c, lp, remote), args.has("--force"), out)?;
            }
            Ok(())
        }
        "pull" => {
            let (remote, local) = (a2.ok_or("usage: iyakki dev files pull REMOTE [LOCAL]")?, args.pos.get(3).map(String::as_str));
            let st = c.call("GET", "/v1/dev/files/stat", &[("path", remote)], b"")?;
            if s(&st, "kind") == "dir" {
                let base = PathBuf::from(local.unwrap_or_else(|| Path::new(remote).file_name().and_then(|n| n.to_str()).unwrap_or("pulled")));
                let mut n = 0usize;
                pull_tree(c, remote, &base, &mut n, out)?;
                out(&format!("Pulled {n} file(s) into {}.", base.display()));
            } else {
                let name = Path::new(remote).file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| "file".into());
                let dest = match local {
                    Some(l) if Path::new(l).is_dir() => Path::new(l).join(&name),
                    Some(l) => PathBuf::from(l),
                    None => PathBuf::from(&name),
                };
                pull_file(c, remote, &dest, out)?;
            }
            Ok(())
        }
        other => Err(format!("unknown files command {other:?}: ls, stat, push, pull, rm, mkdir")),
    }
}

fn remote_name(c: &DevClient, local: &Path, remote: &str) -> String {
    let name = local.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    if remote.ends_with('/') {
        return format!("{remote}{name}");
    }
    match c.call("GET", "/v1/dev/files/stat", &[("path", remote)], b"") {
        Ok(v) if s(&v, "kind") == "dir" => format!("{}/{name}", remote.trim_end_matches('/')),
        _ => remote.to_string(),
    }
}

/// Push one file in chunks; an interrupted push resumes from what the device kept; the device checks the SHA-256 at the end.
pub fn push_file(c: &DevClient, local: &Path, remote: &str, force: bool, out: &mut dyn FnMut(&str)) -> Result<(), String> {
    use std::io::{Read, Seek, SeekFrom};
    let total = std::fs::metadata(local).map_err(|e| format!("{}: {e}", local.display()))?.len();
    if total > MAX_FILE {
        return Err(format!("{} is {} bytes: a push carries at most {} MiB", local.display(), total, MAX_FILE >> 20));
    }
    let sha = sha256_file(local)?;
    let mode = std::fs::metadata(local).ok().map(|m| std::os::unix::fs::PermissionsExt::mode(&m.permissions()) & 0o777).unwrap_or(0o644);
    let mut f = std::fs::File::open(local).map_err(|e| format!("{}: {e}", local.display()))?;
    let st = c.call("GET", "/v1/dev/files/stat", &[("path", remote)], b"")?;
    let mut offset = st["part_size"].as_u64().filter(|p| *p > 0 && *p < total).unwrap_or(0);
    if offset > 0 {
        out(&format!("Resuming {remote} at {offset} of {total} bytes."));
    }
    let mut restarted = false;
    let mut last_pct = 101u64;
    loop {
        f.seek(SeekFrom::Start(offset)).map_err(|e| e.to_string())?;
        let mut buf = vec![0u8; CHUNK.min((total - offset) as usize)];
        f.read_exact(&mut buf).map_err(|e| e.to_string())?;
        let q = [("path", remote), ("offset", &offset.to_string()), ("total", &total.to_string()), ("sha256", &sha), ("overwrite", if force { "1" } else { "0" }), ("mode", &mode.to_string())];
        let (code, v) = c.call_status("POST", "/v1/dev/files/put", &q, &buf)?;
        if code == 200 {
            offset = v["have"].as_u64().unwrap_or(offset + buf.len() as u64);
            let pct = if total == 0 { 100 } else { offset * 100 / total };
            if pct / 10 != last_pct / 10 {
                out(&format!("  {remote}: {pct}%"));
                last_pct = pct;
            }
            if v["done"].as_bool() == Some(true) {
                out(&format!("Pushed {} ({total} bytes, sha256 {}…)", s(&v, "path"), &sha[..12]));
                return Ok(());
            }
        } else if code == 409 && v["have"].is_u64() && !restarted && offset > 0 {
            // the device's partial file is not the one we thought: start again from zero, once
            restarted = true;
            offset = 0;
        } else if code == 422 && !restarted && s(&v, "error").contains("checksum") {
            restarted = true;
            offset = 0;
            out("The checksum did not match: sending it again from the start.");
        } else {
            return Err(v["error"].as_str().map(str::to_string).unwrap_or_else(|| format!("the device answered {code}")));
        }
    }
}

fn push_tree(c: &DevClient, dir: &Path, remote: &str, force: bool, n: &mut usize, out: &mut dyn FnMut(&str)) -> Result<(), String> {
    c.call("POST", "/v1/dev/files/mkdir", &[("path", remote)], b"")?;
    let mut entries: Vec<_> = std::fs::read_dir(dir).map_err(|e| format!("{}: {e}", dir.display()))?.flatten().collect();
    entries.sort_by_key(|e| e.file_name());
    for e in entries {
        let p = e.path();
        let name = e.file_name().to_string_lossy().into_owned();
        let r = format!("{}/{name}", remote.trim_end_matches('/'));
        let ft = e.file_type().map_err(|e| e.to_string())?;
        if ft.is_symlink() {
            out(&format!("  skipped link {}", p.display()));
        } else if ft.is_dir() {
            push_tree(c, &p, &r, force, n, out)?;
        } else if ft.is_file() {
            *n += 1;
            if *n > MAX_TREE {
                return Err(format!("more than {MAX_TREE} files: push a smaller folder"));
            }
            push_file(c, &p, &r, force, out)?;
        }
    }
    Ok(())
}

/// Pull one file in chunks into `dest` (a `.iyakki-part` file beside it is kept and resumed), checked against the device's SHA-256.
pub fn pull_file(c: &DevClient, remote: &str, dest: &Path, out: &mut dyn FnMut(&str)) -> Result<(), String> {
    use std::io::Write;
    let st = c.call("GET", "/v1/dev/files/stat", &[("path", remote), ("hash", "1")], b"")?;
    if s(&st, "kind") != "file" {
        return Err(format!("{remote} is not a file on the device"));
    }
    let (total, sha) = (st["size"].as_u64().unwrap_or(0), s(&st, "sha256").to_string());
    let mut part = dest.as_os_str().to_owned();
    part.push(".iyakki-part");
    let part = PathBuf::from(part);
    let mut have = std::fs::metadata(&part).map(|m| m.len()).unwrap_or(0).min(total);
    if have > 0 {
        out(&format!("Resuming {} at {have} of {total} bytes.", dest.display()));
    }
    let mut f = std::fs::OpenOptions::new().create(true).write(true).truncate(have == 0).open(&part).map_err(|e| format!("{}: {e}", part.display()))?;
    std::io::Seek::seek(&mut f, std::io::SeekFrom::Start(have)).map_err(|e| e.to_string())?;
    while have < total {
        let chunk = c.call_bytes("GET", "/v1/dev/files/get", &[("path", remote), ("offset", &have.to_string()), ("len", &CHUNK.to_string())], b"")?;
        if chunk.is_empty() {
            return Err("the device sent nothing: the file changed while it was being pulled".into());
        }
        f.write_all(&chunk).map_err(|e| e.to_string())?;
        have += chunk.len() as u64;
    }
    drop(f);
    if sha256_file(&part)? != sha {
        let _ = std::fs::remove_file(&part);
        return Err("the checksum does not match what the device holds (the file changed during the pull): try again".into());
    }
    std::fs::rename(&part, dest).map_err(|e| format!("{}: {e}", dest.display()))?;
    out(&format!("Pulled {} ({total} bytes, sha256 {}…)", dest.display(), &sha[..12.min(sha.len())]));
    Ok(())
}

fn pull_tree(c: &DevClient, remote: &str, base: &Path, n: &mut usize, out: &mut dyn FnMut(&str)) -> Result<(), String> {
    std::fs::create_dir_all(base).map_err(|e| format!("{}: {e}", base.display()))?;
    let v = c.call("GET", "/v1/dev/files/ls", &[("path", remote)], b"")?;
    for e in v["entries"].as_array().into_iter().flatten() {
        let name = s(e, "name");
        // a name from the device is a name, never a path
        if name.is_empty() || name.contains('/') || name == ".." || name == "." {
            continue;
        }
        let r = format!("{}/{name}", remote.trim_end_matches('/'));
        match s(e, "kind") {
            "dir" => pull_tree(c, &r, &base.join(name), n, out)?,
            "file" => {
                *n += 1;
                if *n > MAX_TREE {
                    return Err(format!("more than {MAX_TREE} files: pull a smaller folder"));
                }
                pull_file(c, &r, &base.join(name), out)?;
            }
            _ => {}
        }
    }
    Ok(())
}

// -------------------------------------------------------------------------------------------- input

fn input(c: &DevClient, args: &Args, out: &mut dyn FnMut(&str)) -> Result<(), String> {
    let p = |i: usize| args.pos.get(i).map(String::as_str);
    let n = |i: usize, what: &str| -> Result<i64, String> { p(i).ok_or_else(|| format!("{what} is missing"))?.parse::<i64>().map_err(|_| format!("{what} must be a whole number")) };
    let body = match p(1).ok_or("usage: iyakki dev input tap X Y | key NAME | text 'abc' | swipe X1 Y1 X2 Y2 [MS]")? {
        "tap" => serde_json::json!({ "kind": "tap", "x": n(2, "X")?, "y": n(3, "Y")? }),
        "swipe" => serde_json::json!({ "kind": "swipe", "x1": n(2, "X1")?, "y1": n(3, "Y1")?, "x2": n(4, "X2")?, "y2": n(5, "Y2")?, "ms": p(6).and_then(|m| m.parse::<u64>().ok()).unwrap_or(300) }),
        "key" => serde_json::json!({ "kind": "key", "name": p(2).ok_or("which key? (Enter, Esc, Tab, Up, Ctrl+c, F5, a)")? }),
        "text" => serde_json::json!({ "kind": "text", "text": args.pos[2..].join(" ") }),
        other => return Err(format!("unknown input {other:?}: tap, swipe, key, text")),
    };
    c.call("POST", "/v1/dev/input", &[], body.to_string().as_bytes())?;
    out("Sent to the shell on the device's screen.");
    Ok(())
}

// -------------------------------------------------------------------------------------------- screenshots

fn screenshots(c: &DevClient, args: &Args, out: &mut dyn FnMut(&str)) -> Result<(), String> {
    let display = args.get("--display").unwrap_or("0");
    if display.parse::<u32>().is_err() {
        return Err("--display takes a number (0 is the first screen)".into());
    }
    let secs: u64 = match args.get("--record") {
        Some(r) => r.parse().map_err(|_| "--record takes a number of seconds".to_string())?,
        None => 0,
    };
    let fps: u64 = args.get("--fps").and_then(|f| f.parse().ok()).unwrap_or(2).clamp(1, 5);
    if secs == 0 {
        let png = c.call_bytes("GET", "/v1/dev/screenshot", &[("display", display)], b"")?;
        let file = args.get("-o").map(PathBuf::from).unwrap_or_else(|| PathBuf::from(format!("iyakki-screen-{}.png", crate::clock::now_unix())));
        std::fs::write(&file, &png).map_err(|e| format!("{}: {e}", file.display()))?;
        out(&format!("Saved {} ({} KB).", file.display(), png.len().div_ceil(1024)));
        return Ok(());
    }
    let frames = (secs * fps).clamp(1, MAX_FRAMES);
    let dir = PathBuf::from(args.get("-o").unwrap_or("iyakki-record"));
    std::fs::create_dir_all(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    let started = std::time::Instant::now();
    for i in 0..frames {
        let png = c.call_bytes("GET", "/v1/dev/screenshot", &[("display", display)], b"")?;
        std::fs::write(dir.join(format!("frame-{i:04}.png")), &png).map_err(|e| e.to_string())?;
        let due = Duration::from_millis(1000 / fps * (i + 1));
        if let Some(wait) = due.checked_sub(started.elapsed()) {
            std::thread::sleep(wait);
        }
    }
    out(&format!("Saved {frames} pictures in {} (frame-0000.png ...; at most {MAX_FRAMES} frames).", dir.display()));
    Ok(())
}

// -------------------------------------------------------------------------------------------- config

fn read_secret(args: &Args) -> Result<String, String> {
    if let Some(f) = args.get("--key-file") {
        return std::fs::read_to_string(f).map(|t| t.trim_end_matches(['\n', '\r']).to_string()).map_err(|e| format!("{f}: {e}"));
    }
    if let Some(v) = args.get("--key-env") {
        return std::env::var(v).map_err(|_| format!("the environment variable {v} is not set"));
    }
    if args.has("--key-stdin") {
        let mut t = String::new();
        std::io::Read::read_to_string(&mut std::io::stdin(), &mut t).map_err(|e| e.to_string())?;
        return Ok(t.trim_end_matches(['\n', '\r']).to_string());
    }
    Err("give the key with --key-file FILE, --key-env VARIABLE or --key-stdin (never on the command line: it would stay in your shell history)".into())
}

fn coerce(ty: &str, v: &str) -> Value {
    if ty.starts_with("bool") {
        match v {
            "true" | "on" | "yes" | "1" => return Value::Bool(true),
            "false" | "off" | "no" | "0" => return Value::Bool(false),
            _ => {}
        }
    }
    if ty.starts_with("int") {
        if let Ok(n) = v.parse::<i64>() {
            return serde_json::json!(n);
        }
    }
    Value::String(v.to_string())
}

fn config(c: &DevClient, args: &Args, out: &mut dyn FnMut(&str)) -> Result<(), String> {
    let p = |i: usize| args.pos.get(i).map(String::as_str);
    match p(1).ok_or("usage: iyakki dev config list|get|set|export|import|provider|firewall")? {
        "list" => {
            let v = c.call("GET", "/v1/dev/config/list", &[], b"")?;
            out(&format!("{:<28} {:<24} {}", "KEY", "VALUE", "TYPE / MEANING"));
            for r in v["settings"].as_array().into_iter().flatten() {
                let val = match &r["value"] {
                    Value::String(s) if s.is_empty() => "\"\"".to_string(),
                    Value::String(s) => s.clone(),
                    Value::Null => "-".into(),
                    o => o.to_string(),
                };
                out(&format!("{:<28} {:<24} {} — {}", s(r, "key"), val, s(r, "type"), s(r, "desc")));
            }
            out("Providers:");
            for r in v["providers"].as_array().into_iter().flatten() {
                out(&format!("  {:<18} {:<10} {} {}{}", s(r, "id"), s(r, "kind"), r["base_url"].as_str().unwrap_or(""), if r["has_key"] == true { "[key stored] " } else { "" }, if r["enabled"] == false { "(disabled)" } else { "" }));
            }
            Ok(())
        }
        "get" => {
            let v = c.call("GET", "/v1/dev/config/get", &[("key", p(2).ok_or("usage: iyakki dev config get KEY")?)], b"")?;
            out(&format!("{} = {}", s(&v, "key"), match &v["value"] { Value::String(t) => t.clone(), o => o.to_string() }));
            Ok(())
        }
        "set" => {
            let (key, val) = (p(2).ok_or("usage: iyakki dev config set KEY VALUE")?, args.pos.get(3..).map(|v| v.join(" ")).unwrap_or_default());
            let list = c.call("GET", "/v1/dev/config/list", &[], b"")?;
            let ty = list["settings"].as_array().into_iter().flatten().find(|r| s(r, "key") == key).map(|r| s(r, "type").to_string()).unwrap_or_default();
            let v = c.call("POST", "/v1/dev/config/set", &[], serde_json::json!({ "key": key, "value": coerce(&ty, &val) }).to_string().as_bytes())?;
            out(s(&v, "message"));
            Ok(())
        }
        "export" => {
            let v = c.call("GET", "/v1/dev/config/export", &[], b"")?;
            let text = serde_json::to_string_pretty(&v).unwrap_or_default();
            match p(2) {
                Some(f) => {
                    std::fs::write(f, &text).map_err(|e| format!("{f}: {e}"))?;
                    out(&format!("Saved the profile to {f} (no secrets in it)."));
                }
                None => out(&text),
            }
            Ok(())
        }
        "import" => {
            let f = p(2).ok_or("usage: iyakki dev config import FILE.json")?;
            let body = std::fs::read(f).map_err(|e| format!("{f}: {e}"))?;
            let v = c.call("POST", "/v1/dev/config/import", &[], &body)?;
            out(&format!("Applied {} setting(s); providers added: {}", v["applied"], v["providers_added"]));
            for f in v["failed"].as_array().into_iter().flatten() {
                out(&format!("  not applied: {}", f.as_str().unwrap_or("")));
            }
            Ok(())
        }
        "provider" => {
            let action = p(2).ok_or("usage: iyakki dev config provider add|key|remove ID ...")?;
            let id = p(3).ok_or("which provider id?")?;
            let mut body = serde_json::json!({ "action": action, "id": id, "kind": args.get("--kind").unwrap_or(""), "url": args.get("--url").unwrap_or("") });
            if matches!(action, "add" | "key") && (args.get("--key-file").is_some() || args.get("--key-env").is_some() || args.has("--key-stdin") || action == "key") {
                body["key"] = Value::String(read_secret(args)?);
            }
            let v = c.call("POST", "/v1/dev/config/provider", &[], body.to_string().as_bytes())?;
            out(&format!("Done: {}", v));
            Ok(())
        }
        "firewall" => firewall(c, args, out),
        other => Err(format!("unknown config command {other:?}")),
    }
}

fn firewall(c: &DevClient, args: &Args, out: &mut dyn FnMut(&str)) -> Result<(), String> {
    let p = |i: usize| args.pos.get(i).map(String::as_str);
    let call = |action: &str, a: Value| c.call("POST", "/v1/dev/config/call", &[], serde_json::json!({ "action": action, "args": a }).to_string().as_bytes());
    match p(2).ok_or("usage: iyakki dev config firewall list|allow|remove|keep|revert")? {
        "list" => {
            let v = call("firewall.list", Value::Null)?;
            out(&format!("Firewall {} · {}", if v["enabled"] == true { "on" } else { "off" }, s(&v, "status")));
            for r in v["rules"].as_array().into_iter().flatten() {
                out(&format!("  #{:<3} {:<22} {:<5} ports {:<12} {}", r["id"], s(r, "value"), s(r, "proto"), if s(r, "ports").is_empty() { "all" } else { s(r, "ports") }, s(r, "comment")));
            }
            Ok(())
        }
        "keep" | "revert" => {
            let v = call(&format!("firewall.{}", p(2).unwrap_or("keep")), Value::Null)?;
            out(s(&v, "message"));
            Ok(())
        }
        a @ ("allow" | "remove") => {
            let body = if a == "allow" {
                serde_json::json!({ "value": p(3).ok_or("who may connect (address, network or host name)?")?, "ports": args.get("--ports").unwrap_or(""), "proto": args.get("--proto").unwrap_or("both"), "comment": args.get("--comment").unwrap_or(""), "expires_minutes": args.get("--minutes").and_then(|m| m.parse::<i64>().ok()).unwrap_or(0) })
            } else {
                serde_json::json!({ "id": p(3).and_then(|i| i.parse::<u64>().ok()).ok_or("which rule number?")? })
            };
            let v = call(&format!("firewall.{a}"), body)?;
            out(s(&v, "message"));
            // from a remote session a rule can cut this very computer off: confirm that the device still answers, then keep.
            // If it does not, the device reverts by itself after 60 seconds.
            let mut alive = false;
            for _ in 0..3 {
                if c.whoami().is_ok() {
                    alive = true;
                    break;
                }
                std::thread::sleep(Duration::from_secs(2));
            }
            if alive && !args.has("--no-keep") {
                out(s(&call("firewall.keep", Value::Null)?, "message"));
            } else if !alive {
                out("The device no longer answers this computer: the firewall will revert by itself within 60 seconds.");
            } else {
                out("Not kept yet (--no-keep): run `iyakki dev config firewall keep` within 60 seconds.");
            }
            Ok(())
        }
        other => Err(format!("unknown firewall command {other:?}")),
    }
}

// -------------------------------------------------------------------------------------------- apps

fn app(c: &DevClient, args: &Args, out: &mut dyn FnMut(&str)) -> Result<(), String> {
    let p = |i: usize| args.pos.get(i).map(String::as_str);
    let sub = p(1).ok_or("usage: iyakki dev app info|permissions|config|data|clear-data|reset|logs|console|grant|revoke APP")?;
    let name = p(2).ok_or("which app?")?.rsplit(':').next().unwrap_or("").to_string();
    match sub {
        "info" => {
            let v = c.call("GET", "/v1/dev/app/info", &[("app", &name)], b"")?;
            out(&serde_json::to_string_pretty(&v).unwrap_or_default());
            Ok(())
        }
        "permissions" => {
            let v = c.call("GET", "/v1/dev/app/permissions", &[("app", &name)], b"")?;
            out(&format!("{} declares: {}", name, v["declared"].as_array().map(|a| a.iter().filter_map(|x| x.as_str()).collect::<Vec<_>>().join(", ")).filter(|x| !x.is_empty()).unwrap_or_else(|| "nothing".into())));
            out(s(&v, "note"));
            Ok(())
        }
        "grant" | "revoke" => {
            let e = c.call("POST", &format!("/v1/dev/app/{sub}"), &[("app", &name)], b"").unwrap_err();
            Err(e)
        }
        "clear-data" => {
            let v = c.call("POST", "/v1/dev/app/clear", &[("app", &name)], b"")?;
            out(&format!("Cleared {} saved key(s) of {name}.", v["cleared_keys"]));
            Ok(())
        }
        "reset" => {
            let v = c.call("POST", "/v1/dev/app/reset", &[("app", &name)], b"")?;
            out(&format!("Reset {name}: {} saved key(s) and its developer log cleared.", v["cleared_keys"]));
            Ok(())
        }
        "logs" | "console" => {
            let mut a2: Vec<String> = vec!["logs".into(), name.clone()];
            if sub == "console" || args.has("--follow") || args.has("-f") {
                a2.push("--follow".into());
            }
            let parsed = Args::parse(&a2)?;
            // the same reader as `iyakki dev logs`
            let mut since = 0u64;
            loop {
                let v = c.logs(&name, since)?;
                let t = v["text"].as_str().unwrap_or("");
                if !t.is_empty() {
                    out(t.trim_end_matches('\n'));
                }
                let next = v["next"].as_u64().unwrap_or(since);
                let more = next > since && v["more"].as_bool() == Some(true);
                since = next;
                if more {
                    continue;
                }
                if !parsed.has("--follow") {
                    return Ok(());
                }
                std::thread::sleep(Duration::from_secs(1));
            }
        }
        "data" | "config" => {
            let (op, key, value) = if sub == "data" { ("get", p(3), None) } else { (p(3).unwrap_or("get"), p(4), args.get("--file").map(str::to_string).or_else(|| args.pos.get(5..).map(|v| v.join(" ")).filter(|v| !v.is_empty()).map(|v| format!("=lit:{v}")))) };
            match op {
                "get" => {
                    let q: Vec<(&str, &str)> = match key {
                        Some(k) => vec![("app", name.as_str()), ("key", k)],
                        None => vec![("app", name.as_str())],
                    };
                    let v = c.call("GET", "/v1/dev/app/data", &q, b"")?;
                    if key.is_some() {
                        out(s(&v, "value"));
                    } else {
                        out(&format!("{name}: {} bytes saved", v["bytes"]));
                        for k in v["keys"].as_array().into_iter().flatten() {
                            out(&format!("  {:<32} {} bytes", s(k, "key"), k["bytes"]));
                        }
                    }
                    Ok(())
                }
                "set" => {
                    let key = key.ok_or("usage: iyakki dev app config APP set KEY VALUE | --file FILE")?;
                    let val = match value {
                        Some(v) if v.starts_with("=lit:") => v[5..].to_string(),
                        Some(f) => std::fs::read_to_string(&f).map_err(|e| format!("{f}: {e}"))?,
                        None => return Err("give the value after the key, or --file FILE".into()),
                    };
                    let v = c.call("POST", "/v1/dev/app/data/set", &[("app", &name), ("key", key)], val.as_bytes())?;
                    out(&format!("Saved {key} ({} bytes) for {name}. An open copy of the app keeps its own data until it is opened again.", v["bytes"]));
                    Ok(())
                }
                "rm" => {
                    c.call("POST", "/v1/dev/app/data/rm", &[("app", &name), ("key", key.ok_or("which key?")?)], b"")?;
                    out("Removed.");
                    Ok(())
                }
                other => Err(format!("unknown app config command {other:?}: get, set, rm")),
            }
        }
        other => Err(format!("unknown app command {other:?}")),
    }
}

// -------------------------------------------------------------------------------------------- approval, shell, forward, power

use std::sync::atomic::{AtomicBool, Ordering};

/// Set by Ctrl+C while `iyakki dev shell` is running.
pub static INTERRUPT: AtomicBool = AtomicBool::new(false);

extern "C" fn on_sigint(_: libc::c_int) {
    INTERRUPT.store(true, Ordering::SeqCst);
}

/// Ask for the approval on the device's screen and wait for it (polling once a second, up to `wait_secs`).
pub fn ask_grant(c: &DevClient, kind: &str, minutes: u32, root: bool, why: &str, wait_secs: u64, out: &mut dyn FnMut(&str)) -> Result<Value, String> {
    let body = serde_json::json!({ "kind": kind, "minutes": minutes, "as_root": root, "why": why }).to_string();
    let started = std::time::Instant::now();
    let mut told = false;
    loop {
        let v = c.call("POST", "/v1/dev/grant/ask", &[], body.as_bytes())?;
        match s(&v, "state") {
            "granted" => return Ok(v),
            "denied" => return Err("the administrator refused on the device".into()),
            _ => {
                if !told {
                    out(&format!("Waiting for the administrator to approve {} on the device's screen (Settings, Developer mode)...", match kind {
                        "power" => "restart or switch-off",
                        "patch" => "patching the system",
                        _ => "shell access",
                    }));
                    told = true;
                }
                if started.elapsed().as_secs() >= wait_secs {
                    return Err("nobody approved it in time (the request is cancelled after two minutes)".into());
                }
                std::thread::sleep(Duration::from_secs(1));
            }
        }
    }
}

/// Open a shell session on the device (after the approval).
pub fn shell_open(c: &DevClient, minutes: u32, root: bool, why: &str, wait_secs: u64, out: &mut dyn FnMut(&str)) -> Result<(u64, String), String> {
    let g = ask_grant(c, "shell", minutes, root, why, wait_secs, out)?;
    let v = c.call("POST", "/v1/dev/shell/open", &[], b"")?;
    out(&format!("Shell access granted{} until {}.", if g["as_root"] == true { " AS ROOT" } else { "" }, super::stamp(g["expires"].as_i64().unwrap_or(0))));
    Ok((v["sid"].as_u64().unwrap_or(0), s(&v, "cwd").to_string()))
}

/// Run one command line and stream its output; returns the exit status. `interrupted` is checked while it runs (Ctrl+C sends SIGINT,
/// a second one kills it).
pub fn shell_exec(c: &DevClient, sid: u64, line: &str, stdin: Option<&[u8]>, interrupted: &AtomicBool, out: &mut dyn FnMut(&str)) -> Result<i32, String> {
    let sid_s = sid.to_string();
    let v = c.call("POST", "/v1/dev/shell/exec", &[("sid", &sid_s)], line.as_bytes())?;
    let id = v["id"].as_u64().unwrap_or(0).to_string();
    if let Some(data) = stdin {
        for chunk in data.chunks(super::shellexec::MAX_STDIN) {
            c.call("POST", "/v1/dev/shell/stdin", &[("sid", &sid_s), ("id", &id)], chunk)?;
        }
        c.call("POST", "/v1/dev/shell/stdin", &[("sid", &sid_s), ("id", &id), ("eof", "1")], b"")?;
    }
    let (mut since, mut part, mut hits) = (0u64, String::new(), 0);
    loop {
        if interrupted.swap(false, Ordering::SeqCst) {
            hits += 1;
            let _ = c.call("POST", "/v1/dev/shell/signal", &[("sid", &sid_s), ("id", &id), ("sig", if hits > 1 { "kill" } else { "int" })], b"");
        }
        let r = c.call("GET", "/v1/dev/shell/read", &[("sid", &sid_s), ("id", &id), ("since", &since.to_string()), ("wait", "1000")], b"")?;
        part.push_str(s(&r, "text"));
        while let Some(i) = part.find('\n') {
            out(&part[..i]);
            part.drain(..=i);
        }
        since = r["next"].as_u64().unwrap_or(since);
        if r["lost"].as_u64().unwrap_or(0) > 0 {
            out(&format!("[{} bytes of output were dropped: too fast to keep]", r["lost"]));
        }
        if r["done"].as_bool() == Some(true) {
            if !part.is_empty() {
                out(&part);
            }
            return Ok(r["code"].as_i64().unwrap_or(-1) as i32);
        }
    }
}

fn shell_cmd(c: &DevClient, args: &Args, out: &mut dyn FnMut(&str)) -> Result<(), String> {
    let minutes: u32 = args.get("--minutes").and_then(|m| m.parse().ok()).unwrap_or(10);
    let (sid, mut cwd) = shell_open(c, minutes, args.has("--root"), args.get("--reason").unwrap_or(""), 125, out)?;
    // SAFETY: installs a handler that only stores to an atomic; the previous handler is not needed.
    unsafe {
        libc::signal(libc::SIGINT, on_sigint as extern "C" fn(libc::c_int) as usize);
    }
    let result = if !args.after.is_empty() {
        let line = args.after.iter().map(|a| shell_quote(a)).collect::<Vec<_>>().join(" ");
        shell_exec(c, sid, &line, None, &INTERRUPT, out).and_then(|code| if code == 0 { Ok(()) } else { Err(format!("the command ended with status {code}")) })
    } else {
        let stdin = std::io::stdin();
        let mut line = String::new();
        loop {
            eprint!("{}@{} {}$ ", c.device.account, c.device.name, cwd);
            let _ = std::io::Write::flush(&mut std::io::stderr());
            line.clear();
            INTERRUPT.store(false, Ordering::SeqCst);
            match stdin.read_line(&mut line) {
                Ok(0) | Err(_) => break Ok(()),
                Ok(_) => {}
            }
            let l = line.trim_end_matches(['\n', '\r']);
            if l.trim() == "exit" || l.trim() == "logout" {
                break Ok(());
            }
            if l.trim().is_empty() {
                continue;
            }
            match shell_exec(c, sid, l, None, &INTERRUPT, out) {
                Ok(code) if code != 0 => out(&format!("[exit status {code}]")),
                Ok(_) => {}
                Err(e) => break Err(e),
            }
            if let Ok(v) = c.call("GET", "/v1/dev/shell/read", &[("sid", &sid.to_string()), ("id", "0"), ("since", "0"), ("wait", "0")], b"") {
                if !s(&v, "cwd").is_empty() {
                    cwd = s(&v, "cwd").to_string();
                }
            }
        }
    };
    let _ = c.call("POST", "/v1/dev/shell/close", &[("sid", &sid.to_string())], b"");
    result
}

/// Quote one word for `sh`.
pub fn shell_quote(a: &str) -> String {
    if !a.is_empty() && a.chars().all(|c| c.is_ascii_alphanumeric() || "-_./=:,+@%".contains(c)) {
        a.to_string()
    } else {
        format!("'{}'", a.replace('\'', "'\\''"))
    }
}

fn power(c: &DevClient, cmd: &str, args: &Args, out: &mut dyn FnMut(&str)) -> Result<(), String> {
    let who = c.whoami()?;
    let has_shell = who["scopes"].as_array().is_some_and(|a| a.iter().any(|x| x == "shell"));
    ask_grant(c, if has_shell { "shell" } else { "power" }, 2, false, args.get("--reason").unwrap_or(""), 125, out)?;
    let v = c.call("POST", &format!("/v1/dev/power/{cmd}"), &[], b"")?;
    out(s(&v, "message"));
    Ok(())
}

/// Forward `127.0.0.1:local` on this computer to `remote` on the device's loopback until `stop` is set. `ready` is called with the
/// address that is listening (local 0 picks a free port).
pub fn forward(c: &DevClient, local: u16, remote: u16, stop: &Arc<AtomicBool>, ready: &dyn Fn(std::net::SocketAddr), out: &mut dyn FnMut(&str)) -> Result<(), String> {
    use std::io::{Read, Write};
    use std::net::TcpListener;
    let fid = c.call("POST", "/v1/dev/forward/start", &[("port", &remote.to_string())], b"")?["fid"].as_u64().unwrap_or(0);
    let listener = TcpListener::bind(("127.0.0.1", local)).map_err(|e| format!("cannot listen on 127.0.0.1:{local}: {e}"))?;
    listener.set_nonblocking(true).map_err(|e| e.to_string())?;
    let addr = listener.local_addr().map_err(|e| e.to_string())?;
    ready(addr);
    out(&format!("Forwarding {addr} to port {remote} of the device (only this computer can use it; Ctrl+C stops)."));
    let fid_s = fid.to_string();
    let result = loop {
        if stop.load(Ordering::SeqCst) || INTERRUPT.swap(false, Ordering::SeqCst) {
            break Ok(());
        }
        match listener.accept() {
            Ok((sock, _)) => {
                let cid = match c.call("POST", "/v1/dev/forward/connect", &[("fid", &fid_s)], b"") {
                    Ok(v) => v["cid"].as_u64().unwrap_or(0),
                    Err(e) => {
                        out(&format!("connection refused: {e}"));
                        continue;
                    }
                };
                let _ = sock.set_nonblocking(false);
                let done = Arc::new(AtomicBool::new(false));
                // computer -> device
                let (c1, mut rd, d1, f1, s1) = (c.clone(), sock.try_clone().map_err(|e| e.to_string())?, done.clone(), fid_s.clone(), stop.clone());
                std::thread::spawn(move || {
                    let _ = rd.set_read_timeout(Some(Duration::from_millis(200)));
                    let mut buf = vec![0u8; 64 * 1024];
                    while !d1.load(Ordering::SeqCst) && !s1.load(Ordering::SeqCst) {
                        match rd.read(&mut buf) {
                            Ok(0) => break,
                            Ok(n) => {
                                if c1.call("POST", "/v1/dev/forward/send", &[("fid", &f1), ("cid", &cid.to_string())], &buf[..n]).is_err() {
                                    break;
                                }
                            }
                            Err(e) if matches!(e.kind(), std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut) => {}
                            Err(_) => break,
                        }
                    }
                    let _ = c1.call("POST", "/v1/dev/forward/close", &[("fid", &f1), ("cid", &cid.to_string())], b"");
                    d1.store(true, Ordering::SeqCst);
                });
                // device -> computer
                let (c2, mut wr, d2, f2, s2) = (c.clone(), sock, done, fid_s.clone(), stop.clone());
                std::thread::spawn(move || {
                    while !d2.load(Ordering::SeqCst) && !s2.load(Ordering::SeqCst) {
                        match c2.request("GET", "/v1/dev/forward/recv", &[("fid".into(), f2.clone()), ("cid".into(), cid.to_string()), ("wait".into(), "1500".into())], b"") {
                            Ok(r) if r.status == 200 => {
                                if !r.body.is_empty() && wr.write_all(&r.body).is_err() {
                                    break;
                                }
                            }
                            _ => break,
                        }
                    }
                    let _ = wr.shutdown(std::net::Shutdown::Both);
                    d2.store(true, Ordering::SeqCst);
                });
            }
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => std::thread::sleep(Duration::from_millis(30)),
            Err(e) => break Err(e.to_string()),
        }
    };
    let _ = c.call("POST", "/v1/dev/forward/stop", &[("fid", &fid_s)], b"");
    result
}

fn forward_cmd(c: &DevClient, args: &Args, out: &mut dyn FnMut(&str)) -> Result<(), String> {
    let spec = args.pos.get(1).ok_or("usage: iyakki dev forward LOCAL:REMOTE (for example 8080:13305)")?;
    let (l, r) = spec.split_once(':').ok_or("write it as LOCAL:REMOTE, for example 8080:13305")?;
    let (local, remote): (u16, u16) = (l.parse().map_err(|_| "LOCAL is a port number")?, r.parse().map_err(|_| "REMOTE is a port number")?);
    // SAFETY: installs a handler that only stores to an atomic.
    unsafe {
        libc::signal(libc::SIGINT, on_sigint as extern "C" fn(libc::c_int) as usize);
    }
    let stop = Arc::new(AtomicBool::new(false));
    forward(c, local, remote, &stop, &|_| {}, out)
}

// -------------------------------------------------------------------------------------------- system update

fn update_pack(args: &Args, out: &mut dyn FnMut(&str)) -> Result<(), String> {
    let img = args.pos.get(1).ok_or("usage: iyakki dev update-pack IMAGE.img -o DIR [--version V] [--variant NAME]")?;
    let dir = PathBuf::from(args.get("-o").ok_or("where should the payload go? -o DIR")?);
    pack(Path::new(img), &dir, args.get("--version"), args.get("--variant"), out)
}

/// Extract the boot files of an install image into `dir` and write the manifest. The image is only read.
pub fn pack(img: &Path, dir: &Path, version: Option<&str>, variant: Option<&str>, out: &mut dyn FnMut(&str)) -> Result<(), String> {
    if dir.exists() && std::fs::read_dir(dir).map(|mut d| d.next().is_some()).unwrap_or(false) {
        return Err(format!("{} is not empty: pick a new folder for the payload", dir.display()));
    }
    std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    let files = iyakki_usb::bootpack::extract(img, dir)?;
    let list: Vec<(String, u64, String)> = files.iter().map(|f| (f.path.clone(), f.size, f.sha256.clone())).collect();
    let name = img.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    let m = match super::update::build_manifest(dir, &list, version, variant, &name) {
        Ok(m) => m,
        Err(e) => {
            let _ = std::fs::remove_dir_all(dir);
            return Err(e);
        }
    };
    super::update::write_manifest(dir, &m)?;
    out(&format!("Payload for IYAKKI {} ({} image{}): {} files, {} MB, in {}", m.version, m.variant, if m.secure_boot { ", Secure Boot signed" } else { "" }, m.files.len(), m.files.iter().map(|f| f.size).sum::<u64>() >> 20, dir.display()));
    Ok(())
}

/// Send one payload file in chunks to the device's staging folder, resuming what it kept.
fn stage_file(c: &DevClient, local: &Path, rel: &str, size: u64, out: &mut dyn FnMut(&str)) -> Result<(), String> {
    use std::io::{Read, Seek, SeekFrom};
    let mut f = std::fs::File::open(local).map_err(|e| format!("{}: {e}", local.display()))?;
    let st = c.call("GET", "/v1/dev/update/staged", &[("path", rel)], b"")?;
    if st["size"].as_u64() == Some(size) && st["exists"] == true {
        out(&format!("  {rel}: already on the device"));
        return Ok(());
    }
    let mut offset = st["part_size"].as_u64().filter(|p| *p > 0 && *p < size).unwrap_or(0);
    if offset > 0 {
        out(&format!("  {rel}: resuming at {} MB", offset >> 20));
    }
    let mut last = 101u64;
    loop {
        f.seek(SeekFrom::Start(offset)).map_err(|e| e.to_string())?;
        let mut buf = vec![0u8; CHUNK.min((size - offset) as usize).max(if size == 0 { 0 } else { 1 })];
        f.read_exact(&mut buf).map_err(|e| e.to_string())?;
        let (code, v) = c.call_status("POST", "/v1/dev/update/stage", &[("path", rel), ("offset", &offset.to_string())], &buf)?;
        if code != 200 {
            if code == 409 && v["have"].is_u64() && offset > 0 {
                offset = v["have"].as_u64().unwrap_or(0);
                continue;
            }
            return Err(v["error"].as_str().map(str::to_string).unwrap_or_else(|| format!("the device answered {code}")));
        }
        offset = v["have"].as_u64().unwrap_or(offset + buf.len() as u64);
        let pct = if size == 0 { 100 } else { offset * 100 / size };
        if pct / 10 != last / 10 && size > (4 << 20) {
            out(&format!("  {rel}: {pct}%"));
            last = pct;
        }
        if v["done"].as_bool() == Some(true) {
            return Ok(());
        }
    }
}

fn update(c: &DevClient, args: &Args, out: &mut dyn FnMut(&str)) -> Result<(), String> {
    if args.has("--status") {
        let v = c.call("GET", "/v1/dev/update/status", &[], b"")?;
        out(&format!("Previous version kept for the boot menu: {}", v["previous_version"].as_str().unwrap_or("none")));
        match v["message"].as_str() {
            Some(m) => out(m),
            None => out("No unfinished update."),
        }
        return Ok(());
    }
    let src = PathBuf::from(args.pos.get(1).ok_or("usage: iyakki dev update PAYLOAD_DIR|IMAGE.img [--force] [--reboot] | --status")?);
    let mut temp = None;
    let dir = if src.is_file() {
        let t = std::env::temp_dir().join(format!("iyakki-update-{}-{}", std::process::id(), crate::clock::now_unix()));
        out("Reading the boot partition of the image...");
        pack(&src, &t, args.get("--version"), args.get("--variant"), out)?;
        temp = Some(t.clone());
        t
    } else {
        src
    };
    let result = (|| {
        let m = super::update::read_manifest(&dir)?;
        super::update::verify_payload(&dir, &m)?;
        let force = if args.has("--force") { "1" } else { "0" };
        let manifest_bytes = std::fs::read(dir.join(super::update::MANIFEST)).map_err(|e| e.to_string())?;
        let p = c.call("POST", "/v1/dev/update/plan", &[("force", force)], &manifest_bytes)?;
        out(&format!("Update plan: IYAKKI {} -> {}; {} file(s) to send, {} already the same.", s(&p, "from"), s(&p, "to"), p["needs"].as_array().map_or(0, |a| a.len()), p["same"]));
        for n in p["notes"].as_array().into_iter().flatten().filter_map(|x| x.as_str()) {
            out(&format!("  note: {n}"));
        }
        for n in p["needs"].as_array().into_iter().flatten().filter_map(|x| x.as_str()) {
            let f = m.files.iter().find(|f| f.path == n).ok_or("the device asked for a file that is not in the payload")?;
            stage_file(c, &dir.join(n), n, f.size, out)?;
        }
        out("Everything is on the device. Applying: the previous version is kept first, then the boot files are replaced...");
        let v = c.call("POST", "/v1/dev/update/apply", &[("force", force), ("reboot", if args.has("--reboot") { "1" } else { "0" })], b"")?;
        out(s(&v, "message"));
        if args.has("--reboot") {
            out("The device is restarting.");
        } else {
            out("Restart the device to use it (the boot menu keeps \"IYAKKI (previous version)\").");
        }
        Ok(())
    })();
    if let Some(t) = temp {
        let _ = std::fs::remove_dir_all(t);
    }
    result
}
