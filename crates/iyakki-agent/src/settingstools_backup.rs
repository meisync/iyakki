//! Chat tools for Settings -> Backup and restore, and for the files CHECK-UP leaves (F-BACKUP-01, F-CHECKUP-02): `backup_status` (free: what
//! a backup would hold, area by area), `backup_inspect` (free: what a backup file holds), `backup_create` (asks), `backup_restore` (asks),
//! `checkup_result_verify` (free), `edit_run_stop` (asks) and `edit_run_input` (asks).
//!
//! Rules the tools keep because a chat is not a safe place for secrets or for reaching anywhere: a **passphrase never goes through the
//! chat**, so `backup_create` makes an unencrypted file only when the call says `unencrypted: true` (the card says so), and an encrypted
//! file is opened in Settings -> Backup where the person types it; files and folders must be inside the person's own home folder (no `..`,
//! no link that leads out); `backup_restore` brings back only the areas named, only a file made for this same account, keeps what was
//! there (renamed) and shows the card with the areas and the file.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use serde_json::{json, Value};

use crate::backup::{self, CreateOptions, Layout, RestoreRequest};
use crate::backup_schedule::Schedule;
use crate::settingstools::{arg_bool, arg_str, no_args, Entry, Env};
pub use iyakki_shared::settingstools_backup::{PROMPT_RULE};

const TAB: &str = "Backup";

pub const TOOLS: [&str; 8] = ["backup_status", "backup_inspect", "backup_create", "backup_restore", "backup_schedule", "checkup_result_verify", "edit_run_stop", "edit_run_input"];


pub(crate) fn layout(e: &Env) -> Layout {
    Layout { data: e.dir.clone(), home: crate::scope::home_root(&e.user, &e.dir), user: e.user.clone() }
}

/// A path the person named, inside their own home folder: relative to it, or absolute under it; no `..`; a link that leaves is refused.
pub(crate) fn inside_home(l: &Layout, given: &str, must_be_dir: bool) -> Result<PathBuf, String> {
    let g = given.trim();
    if g.is_empty() || g.split('/').any(|s| s == "..") {
        return Err("name a path inside your home folder (no `..`)".into());
    }
    let p = if Path::new(g).is_absolute() { PathBuf::from(g) } else { l.home.join(g) };
    let home = std::fs::canonicalize(&l.home).map_err(|_| "your home folder does not exist yet".to_string())?;
    let real = std::fs::canonicalize(&p).map_err(|_| format!("{g} does not exist"))?;
    if !real.starts_with(&home) {
        return Err("that is outside your home folder; backups are written and read there only".into());
    }
    if must_be_dir && !real.is_dir() {
        return Err(format!("{g} is not a folder"));
    }
    if !must_be_dir && !real.is_file() {
        return Err(format!("{g} is not a file"));
    }
    Ok(real)
}

pub(crate) fn areas_arg(a: &Value) -> Result<Option<BTreeSet<String>>, String> {
    let Some(v) = a.get("areas") else { return Ok(None) };
    let known = backup::all_areas();
    let set: BTreeSet<String> = v.as_array().ok_or("`areas` is a list of area ids")?.iter().filter_map(|x| x.as_str().map(String::from)).collect();
    if let Some(bad) = set.iter().find(|s| !known.contains(*s)) {
        return Err(format!("{bad} is not a backup area (backup_status lists them)"));
    }
    Ok(Some(set))
}

pub fn entries(env: &Env) -> Vec<Entry> {
    let mut v = Vec::new();
    {
        let e = env.clone();
        v.push(Entry::read(TAB, "backup_status", "Show what a backup of the person's own data would hold: each area with its id, files and size, and what is left out.", no_args(), move |_| {
            let l = layout(&e);
            let mut out = String::from("A backup would hold (area id: files, size):");
            for a in backup::preview(&l, &backup::all_areas()).map_err(|x| x.to_string())? {
                out.push_str(&format!("\n{}: {} ({} files, {})", a.id, a.label, a.files, backup::human(a.bytes)));
                for s in &a.skipped {
                    out.push_str(&format!("\n  left out: {s}"));
                }
            }
            out.push_str(&format!("\nScheduled backup: {}", Schedule::load(&e.dir, &e.user).describe(&crate::clock::Zone::system_with(None), crate::clock::now_unix())));
            Ok(out)
        }));
    }
    {
        let e = env.clone();
        let schema = json!({"type": "object", "properties": {"file": {"type": "string", "description": "A backup file inside the person's home folder."}}, "required": ["file"]});
        v.push(Entry::read(TAB, "backup_inspect", "Read a backup file (made for this account, not encrypted) and say what it holds. An encrypted file is opened in Settings, Backup.", schema, move |a| {
            let l = layout(&e);
            let f = inside_home(&l, &arg_str(a, "file")?, false)?;
            let c = backup::inspect(&f, None, &l).map_err(|x| x.to_string())?;
            let mut out = format!("Made by {} with {} ({}), {} files, {}.", c.account, c.release, if c.same_account { "this account" } else { "another account: restoring needs the person in Settings" }, c.total_files, backup::human(c.total_bytes));
            for x in &c.areas {
                out.push_str(&format!("\n{}: {} ({} files, {})", x.id, x.label, x.files, backup::human(x.bytes)));
            }
            Ok(out)
        }));
    }
    {
        let e = env.clone();
        let schema = json!({"type": "object", "properties": {"folder": {"type": "string", "description": "An existing folder inside the person's home."}, "areas": {"type": "array", "items": {"type": "string"}, "description": "Area ids from backup_status (all when left out)."}, "unencrypted": {"type": "boolean", "description": "Must be true: a passphrase cannot be given to you, so the file is not encrypted."}}, "required": ["folder", "unencrypted"]});
        v.push(Entry::change(
            TAB,
            "backup_create",
            "Back up the person's own data into ONE unencrypted file in a folder of their home. The person approves a card that names the folder and the areas. For an encrypted file the person uses Settings, Backup.",
            schema,
            |a| format!("Back up your data into {} ({}), NOT encrypted", a["folder"].as_str().unwrap_or("a folder"), a.get("areas").map(|x| x.to_string()).unwrap_or_else(|| "all areas".into())),
            move |a| {
                if !arg_bool(a, "unencrypted")? {
                    return Err("a passphrase cannot go through the chat: the person makes an encrypted backup in Settings, Backup. Say unencrypted: true only if they agreed to a plain file".into());
                }
                let l = layout(&e);
                let dest = inside_home(&l, &arg_str(a, "folder")?, true)?;
                let areas = areas_arg(a)?.unwrap_or_else(backup::all_areas);
                let c = backup::create(&l, &dest, &CreateOptions { areas, passphrase: None, now: crate::clock::now_unix(), release: env!("CARGO_PKG_VERSION").into() }).map_err(|x| x.to_string())?;
                e.audit("backup.create", &format!("{} files, not encrypted", c.files));
                crate::appevent_more::note_backup_made(&e.dir, &e.user, &c.path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default(), true);
                Ok(format!("Wrote {} ({} files, {}). It is not encrypted: anyone who gets the file can read it.", c.path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default(), c.files, backup::human(c.bytes)))
            },
        ));
    }
    {
        let e = env.clone();
        let schema = json!({"type": "object", "properties": {
            "enabled": {"type": "boolean", "description": "true turns the schedule on, false turns it off (the other settings are kept)."},
            "every_days": {"type": "integer", "description": "A backup every this many days, 1 to 31."},
            "hour": {"type": "integer", "description": "From this hour of the day, 0 to 23, local time."},
            "folder": {"type": "string", "description": "An existing folder inside the person's home to write to."},
            "keep": {"type": "integer", "description": "Keep this many files of this account in the folder, 1 to 50."},
            "areas": {"type": "array", "items": {"type": "string"}, "description": "Area ids from backup_status (all when left out)."},
            "passphrase_file": {"type": "string", "description": "A file in the person's home whose first line is the passphrase (8 or more characters); the person makes it, the chat never sees it. Empty: no encryption."},
            "unencrypted": {"type": "boolean", "description": "Say true only if the person agreed that the scheduled files are NOT encrypted."}}});
        v.push(Entry::change(
            TAB,
            "backup_schedule",
            "Set, change or turn off the scheduled backup of the person's own data (it runs while they are signed in). The person approves a card.",
            schema,
            |a| format!("{} the scheduled backup{}{}", if a.get("enabled").and_then(Value::as_bool) == Some(false) { "Turn off" } else { "Set" }, a.get("folder").and_then(Value::as_str).map(|f| format!(" into {f}")).unwrap_or_default(), if a.get("passphrase_file").and_then(Value::as_str).is_some_and(|p| !p.is_empty()) { ", encrypted with the passphrase in the file you named" } else { ", NOT encrypted unless a passphrase file is set" }),
            move |a| {
                let l = layout(&e);
                let mut s = Schedule::load(&e.dir, &e.user);
                let num = |k: &str, lo: u64, hi: u64| -> Result<Option<u64>, String> {
                    match a.get(k) {
                        None => Ok(None),
                        Some(v) => v.as_u64().filter(|n| (lo..=hi).contains(n)).map(Some).ok_or_else(|| format!("`{k}` is a whole number from {lo} to {hi}")),
                    }
                };
                if let Some(n) = num("every_days", 1, 31)? {
                    s.every_days = n as u32;
                }
                if let Some(n) = num("hour", 0, 23)? {
                    s.hour = n as u8;
                }
                if let Some(n) = num("keep", 1, 50)? {
                    s.keep = n as u32;
                }
                if let Some(f) = a.get("folder").and_then(Value::as_str) {
                    s.dest = inside_home(&l, f, true)?.display().to_string();
                }
                if let Some(x) = areas_arg(a)? {
                    s.areas = x.into_iter().collect();
                }
                if let Some(p) = a.get("passphrase_file").and_then(Value::as_str) {
                    s.passphrase_file = if p.trim().is_empty() { String::new() } else { inside_home(&l, p, false)?.display().to_string() };
                }
                if let Some(b) = a.get("enabled").and_then(Value::as_bool) {
                    s.enabled = b;
                }
                if !["enabled", "every_days", "hour", "keep", "folder", "areas", "passphrase_file"].iter().any(|k| a.get(*k).is_some()) {
                    return Err("say what to set: enabled, every_days, hour, folder, keep, areas or passphrase_file".into());
                }
                if s.enabled {
                    if s.passphrase_file.is_empty() && a.get("unencrypted").and_then(Value::as_bool) != Some(true) {
                        return Err("a passphrase cannot go through the chat: name a passphrase_file the person made in their home, or say unencrypted: true only if they agreed to plain files".into());
                    }
                    s.validate()?;
                }
                s.save(&e.dir, &e.user)?;
                e.audit("backup.schedule", &format!("{}, every {} day(s), {}", if s.enabled { "on" } else { "off" }, s.every_days, if s.passphrase_file.is_empty() { "not encrypted" } else { "encrypted" }));
                Ok(s.describe(&crate::clock::Zone::system_with(None), crate::clock::now_unix()))
            },
        ));
    }
    {
        let e = env.clone();
        let schema = json!({"type": "object", "properties": {"file": {"type": "string", "description": "A backup file in the person's home, made for this account, not encrypted."}, "areas": {"type": "array", "items": {"type": "string"}, "description": "The areas to bring back (named one by one)."}}, "required": ["file", "areas"]});
        v.push(Entry::change(
            TAB,
            "backup_restore",
            "Bring back the named areas from a backup file made for this account. What is there now is kept (renamed), never deleted. The person approves a card that names the file and the areas.",
            schema,
            |a| format!("Restore {} from the backup {}; what is there now is kept", a.get("areas").map(|x| x.to_string()).unwrap_or_default(), a["file"].as_str().unwrap_or("file")),
            move |a| {
                let l = layout(&e);
                let f = inside_home(&l, &arg_str(a, "file")?, false)?;
                let areas = areas_arg(a)?.filter(|s| !s.is_empty()).ok_or("name the areas to bring back")?;
                let r = backup::restore(&l, &f, None, &RestoreRequest { areas, allow_other_account: false, now: crate::clock::now_unix() }).map_err(|x| x.to_string())?;
                e.audit("backup.restore", &r.restored.iter().map(|x| x.id.clone()).collect::<Vec<_>>().join(","));
                Ok(format!("Restored {}. The old data was kept as: {}.", r.restored.iter().map(|x| x.label.clone()).collect::<Vec<_>>().join(", "), r.kept.iter().filter_map(|k| k.file_name().map(|n| n.to_string_lossy().into_owned())).collect::<Vec<_>>().join(", ")))
            },
        ));
    }
    {
        let e = env.clone();
        let schema = json!({"type": "object", "properties": {"file": {"type": "string", "description": "A CHECK-UP result file inside the person's home (or `latest` for the one this computer wrote)."}}, "required": ["file"]});
        v.push(Entry::read(TAB, "checkup_result_verify", "Check the signature of a CHECK-UP result file and list the hardware-matrix rows in it.", schema, move |a| {
            let l = layout(&e);
            let w = arg_str(a, "file")?;
            let path = if w.trim() == "latest" { e.dir.join(crate::checkup_result::FILE_NAME) } else { inside_home(&l, &w, false)? };
            let text = std::fs::read_to_string(&path).map_err(|x| format!("{}: {x}", path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default()))?;
            let b = crate::checkup_result::verify(&text)?;
            Ok(format!("The signature is good (it proves the file was not edited, not which computer wrote it).\n{}", crate::checkup_result::lines(&b).join("\n")))
        }));
    }
    {
        let e = env.clone();
        v.push(Entry::change(TAB, "edit_run_stop", "Stop the file the editor is running: a WebAssembly run is interrupted at once, a script run in the container cluster has its job deleted. The person approves a card first.", no_args(), |_| "Stop the run in the editor".into(), move |_| {
            request_run_stop(&e.dir, &e.user);
            e.audit("edit.run.stop.request", "");
            Ok("Asked the editor to stop the run. If nothing was running, nothing happens.".into())
        }));
    }
    {
        let e = env.clone();
        let schema = json!({"type": "object", "properties": {"text": {"type": "string", "description": "What the program reads on its standard input (up to 16 KB of text; a newline ends a line). Empty clears it."}}, "required": ["text"]});
        v.push(Entry::change(TAB, "edit_run_input", "Set the text the editor gives the next run of the open file on its standard input (WebAssembly reads it with iyakki.stdin_read; Python, JavaScript and shell read it as stdin). The person approves a card first.", schema, |a| format!("Give the editor's next run this input: {}", a["text"].as_str().unwrap_or("").chars().take(120).collect::<String>().replace('\n', " ⏎ ")), move |a| {
            let text = a.get("text").and_then(Value::as_str).ok_or("give the text as `text` (empty clears it)")?;
            if text.len() > crate::cluster::MAX_STDIN || text.contains('\0') {
                return Err(format!("the input may be up to {} KB of text", crate::cluster::MAX_STDIN / 1024));
            }
            request_run_input(&e.dir, &e.user, text);
            e.audit("edit.run.input.request", &format!("{} bytes", text.len()));
            Ok(format!("The editor will use {} bytes of input for its next run.", text.len()))
        }));
    }
    v
}

fn input_path(dir: &Path, user: &str) -> PathBuf {
    let who: String = user.chars().filter(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.')).collect();
    dir.join(format!("edit-input-{who}"))
}

/// Ask the editor of `user` to use `text` as the input of its next run (the assistant's `edit_run_input`; an empty text clears it).
pub fn request_run_input(dir: &Path, user: &str, text: &str) {
    let _ = std::fs::write(input_path(dir, user), text.as_bytes());
}

/// The editor of `user` takes the new input, once. Cheap: one `stat` while nothing is asked.
pub fn take_run_input(dir: &Path, user: &str) -> Option<String> {
    let p = input_path(dir, user);
    let t = std::fs::read_to_string(&p).ok()?;
    std::fs::remove_file(&p).ok()?;
    Some(t)
}

fn stop_path(dir: &Path, user: &str) -> PathBuf {
    let who: String = user.chars().filter(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.')).collect();
    dir.join(format!("edit-stop-{who}"))
}

/// Ask the editor of `user` to stop waiting for its run (the assistant's `edit_run_stop`).
pub fn request_run_stop(dir: &Path, user: &str) {
    let _ = std::fs::write(stop_path(dir, user), b"1\n");
}

/// The editor of `user` takes the request. Cheap: one `stat` while nothing is asked.
pub fn take_run_stop(dir: &Path, user: &str) -> bool {
    let p = stop_path(dir, user);
    p.exists() && std::fs::remove_file(&p).is_ok()
}
