//! More chat tools for Settings -> Backup and restore (F-BACKUP-04, F-BACKUP-05, F-BACKUP-06), every one asking first:
//! `backup_share_create` (a backup written to an SMB or NFS share the machine mounts for the length of the backup),
//! `backup_system_create` and `backup_system_restore` (the machine's own settings and keys, administrators only, always encrypted),
//! `backup_incremental_create` (a link that holds only what changed since the chain before it) and `backup_chain_restore`.
//!
//! The same rules as the first backup tools: a passphrase and a share password never go through the chat; both are read from a file the
//! person made inside their own home (`passphrase_file`: first line; `credentials_file`: user on the first line, password on the second).
//! The card never shows a password. The share is let go again whatever happens (`iyakki_shared::backup_share::with_share`).

use std::path::PathBuf;
use std::sync::Arc;

use serde_json::{json, Value};

use crate::backup::{self, AdminProof, CreateOptions, Progress, RestoreRequest};
use crate::backup_schedule::read_passphrase;
use crate::settingstools::{arg_str, Entry, Env};
use crate::settingstools_backup::{areas_arg, inside_home, layout};
use iyakki_shared::backup_share::{self, Mounter, ShareSpec};

const TAB: &str = "Backup";

pub const TOOLS: [&str; 5] = ["backup_share_create", "backup_system_create", "backup_system_restore", "backup_incremental_create", "backup_chain_restore"];

type SharedMounter = Arc<dyn Mounter + Send + Sync>;

/// The passphrase from a file in the person's home, or none when the call says plainly that a plain file is wanted.
fn passphrase(e: &Env, a: &Value) -> Result<Option<String>, String> {
    match a.get("passphrase_file").and_then(Value::as_str).filter(|p| !p.trim().is_empty()) {
        Some(f) => {
            let p = inside_home(&layout(e), f, false)?;
            read_passphrase(&p.display().to_string()).map(Some)
        }
        None if a.get("unencrypted").and_then(Value::as_bool) == Some(true) => Ok(None),
        None => Err("a passphrase cannot go through the chat: name a passphrase_file the person made in their home, or say unencrypted: true only if they agreed to a plain file".into()),
    }
}

fn chain_arg(e: &Env, a: &Value) -> Result<Vec<PathBuf>, String> {
    let list = a.get("chain").and_then(Value::as_array).filter(|l| !l.is_empty() && l.len() <= 200).ok_or("`chain` is the list of backup files, the full one first, then each link in order (at most 200)")?;
    list.iter().map(|v| inside_home(&layout(e), v.as_str().ok_or("`chain` holds file names")?, false)).collect()
}

fn names(r: &backup::RestoreReport) -> String {
    format!("Restored {}. The old data was kept as: {}.", r.restored.iter().map(|x| x.label.clone()).collect::<Vec<_>>().join(", "), r.kept.iter().filter_map(|k| k.file_name().map(|n| n.to_string_lossy().into_owned())).collect::<Vec<_>>().join(", "))
}

fn file_name(p: &std::path::Path) -> String {
    p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default()
}

pub fn entries(env: &Env) -> Vec<Entry> {
    entries_with(env, Arc::new(crate::backupshare::RealMounter))
}

pub fn entries_with(env: &Env, mounter: SharedMounter) -> Vec<Entry> {
    let mut v = Vec::new();
    {
        let e = env.clone();
        let schema = json!({"type": "object", "properties": {
            "address": {"type": "string", "description": "smb://host/share[/folder] or nfs://host/export."},
            "credentials_file": {"type": "string", "description": "SMB only: a file in the person's home with the user name on the first line and the password on the second. The chat never sees it."},
            "areas": {"type": "array", "items": {"type": "string"}, "description": "Area ids from backup_status (all when left out)."},
            "passphrase_file": {"type": "string", "description": "A file in the person's home whose first line is the passphrase (8 or more characters)."},
            "unencrypted": {"type": "boolean", "description": "Say true only if the person agreed that the file is NOT encrypted."}}, "required": ["address"]});
        v.push(Entry::change(
            TAB,
            "backup_share_create",
            "Back up the person's own data into ONE file on a network share (SMB or NFS): the machine mounts it only for the backup (no program may run from it) and lets it go again. The person approves a card that names the share.",
            schema,
            |a| format!("Back up your data to the network share {} ({})", a["address"].as_str().unwrap_or("?"), if a.get("passphrase_file").and_then(Value::as_str).is_some_and(|p| !p.is_empty()) { "encrypted" } else { "NOT encrypted" }),
            move |a| {
                let spec = ShareSpec::parse(arg_str(a, "address")?.trim())?;
                let pass = passphrase(&e, a)?;
                let creds = match a.get("credentials_file").and_then(Value::as_str).filter(|s| !s.trim().is_empty()) {
                    Some(f) => {
                        let p = inside_home(&layout(&e), f, false)?;
                        let text = std::fs::read_to_string(&p).map_err(|x| format!("the credentials file cannot be read: {x}"))?;
                        Some(backup_share::parse_credentials(&text)?)
                    }
                    None => None,
                };
                let areas = areas_arg(a)?.unwrap_or_else(backup::all_areas);
                let work = e.dir.join("backup-share-work");
                {
                    use std::os::unix::fs::DirBuilderExt;
                    std::fs::DirBuilder::new().recursive(true).mode(0o700).create(&work).map_err(|x| format!("no private folder for the mount: {x}"))?;
                }
                let o = CreateOptions { areas, passphrase: pass.clone(), now: crate::clock::now_unix(), release: env!("CARGO_PKG_VERSION").into() };
                let r = backup_share::create_on_share(&layout(&e), &o, &spec, creds.as_ref().map(|(u, p)| (u.as_str(), p.as_str())), mounter.as_ref(), &work);
                let (name, bytes) = match r {
                    Ok(x) => x,
                    Err(x) => {
                        e.audit("backup.share.failed", &spec.source());
                        return Err(x);
                    }
                };
                e.audit("backup.share", &format!("{}, {}", spec.source(), if pass.is_some() { "encrypted" } else { "not encrypted" }));
                Ok(format!("Wrote {name} ({}) to {}. The share was let go again.{}", backup::human(bytes), spec.source(), if pass.is_some() { "" } else { " It is not encrypted: anyone who can read the share can read the file." }))
            },
        ));
    }
    {
        let e = env.clone();
        let schema = json!({"type": "object", "properties": {
            "folder": {"type": "string", "description": "An existing folder inside the administrator's home."},
            "passphrase_file": {"type": "string", "description": "A file in the home whose first line is the passphrase (8 or more characters). Required: this backup holds keys and tokens."}}, "required": ["folder", "passphrase_file"]});
        v.push(
            Entry::change(TAB, "backup_system_create", "Back up the machine's own settings, connections, Git keys and VPN profiles into ONE encrypted file (administrators only). The person approves a card.", schema, |a| format!("Back up the machine's own settings and keys into {}, encrypted with the passphrase in the file you named", a["folder"].as_str().unwrap_or("a folder")), move |a| {
                let proof = AdminProof::of(e.admin).ok_or("only an administrator may back up the machine's own data")?;
                let l = layout(&e);
                let dest = inside_home(&l, &arg_str(a, "folder")?, true)?;
                let pass = passphrase(&e, a)?.ok_or("the machine's own data is never written without a passphrase")?;
                let o = CreateOptions { areas: Default::default(), passphrase: Some(pass), now: crate::clock::now_unix(), release: env!("CARGO_PKG_VERSION").into() };
                let c = backup::create_system(&l, &dest, &o, &Progress::default(), proof).map_err(|x| x.to_string())?;
                e.audit("backup.system.create", &format!("{} files", c.files));
                Ok(format!("Wrote {} ({} files, {}), encrypted.", file_name(&c.path), c.files, backup::human(c.bytes)))
            })
            .admin(),
        );
    }
    {
        let e = env.clone();
        let schema = json!({"type": "object", "properties": {
            "file": {"type": "string", "description": "A system backup file in the administrator's home."},
            "passphrase_file": {"type": "string", "description": "A file in the home whose first line is the passphrase."}}, "required": ["file", "passphrase_file"]});
        v.push(
            Entry::change(TAB, "backup_system_restore", "Bring back the machine's own settings and keys from a system backup (administrators only). What is there now is kept (renamed). The person approves a card.", schema, |a| format!("Replace the machine's own settings and keys from the backup {}; what is there now is kept", a["file"].as_str().unwrap_or("file")), move |a| {
                let proof = AdminProof::of(e.admin).ok_or("only an administrator may restore the machine's own data")?;
                let l = layout(&e);
                let f = inside_home(&l, &arg_str(a, "file")?, false)?;
                let pass = passphrase(&e, a)?.ok_or("name the passphrase_file")?;
                let r = backup::restore_system(&l, &f, Some(&pass), crate::clock::now_unix(), false, proof).map_err(|x| x.to_string())?;
                e.audit("backup.system.restore", &r.restored.iter().map(|x| x.id.clone()).collect::<Vec<_>>().join(","));
                Ok(names(&r))
            })
            .admin(),
        );
    }
    {
        let e = env.clone();
        let schema = json!({"type": "object", "properties": {
            "folder": {"type": "string", "description": "An existing folder inside the person's home to write the new link to."},
            "chain": {"type": "array", "items": {"type": "string"}, "description": "The backups so far, the full one first, then each incremental in order."},
            "areas": {"type": "array", "items": {"type": "string"}, "description": "Area ids (all when left out)."},
            "passphrase_file": {"type": "string", "description": "The passphrase file the chain was made with."},
            "unencrypted": {"type": "boolean", "description": "Say true only for a chain of plain files."}}, "required": ["folder", "chain"]});
        v.push(Entry::change(TAB, "backup_incremental_create", "Add one link to a chain of backups: a small file with only what is new or changed since the chain, and a list of what is gone. The person approves a card.", schema, |a| format!("Add an incremental backup to the chain of {} file(s) in {}", a["chain"].as_array().map_or(0, Vec::len), a["folder"].as_str().unwrap_or("a folder")), move |a| {
            let l = layout(&e);
            let dest = inside_home(&l, &arg_str(a, "folder")?, true)?;
            let chain = chain_arg(&e, a)?;
            let pass = passphrase(&e, a)?;
            let areas = areas_arg(a)?.unwrap_or_else(backup::all_areas);
            let base = backup::chain_state(&l, &chain, pass.as_deref()).map_err(|x| x.to_string())?;
            let o = CreateOptions { areas, passphrase: pass, now: crate::clock::now_unix(), release: env!("CARGO_PKG_VERSION").into() };
            let c = backup::create_incremental(&l, &dest, &o, &Progress::default(), &base).map_err(|x| x.to_string())?;
            e.audit("backup.incremental", &format!("link {}, {} files", base.links + 1, c.files));
            Ok(format!("Wrote {} ({} changed files, {}). Keep it after the {} file(s) before it, in that order.", file_name(&c.path), c.files, backup::human(c.bytes), base.links))
        }));
    }
    {
        let e = env.clone();
        let schema = json!({"type": "object", "properties": {
            "chain": {"type": "array", "items": {"type": "string"}, "description": "The full backup first, then each incremental in order."},
            "areas": {"type": "array", "items": {"type": "string"}, "description": "The areas to bring back (named one by one)."},
            "passphrase_file": {"type": "string", "description": "The passphrase file the chain was made with."},
            "unencrypted": {"type": "boolean", "description": "Say true only for a chain of plain files."}}, "required": ["chain", "areas"]});
        v.push(Entry::change(TAB, "backup_chain_restore", "Bring back the named areas from a chain of backups (the full one and its links, in order). What is there now is kept (renamed). The person approves a card.", schema, |a| format!("Restore {} from a chain of {} backup file(s); what is there now is kept", a.get("areas").map(|x| x.to_string()).unwrap_or_default(), a["chain"].as_array().map_or(0, Vec::len)), move |a| {
            let l = layout(&e);
            let chain = chain_arg(&e, a)?;
            let pass = passphrase(&e, a)?;
            let areas = areas_arg(a)?.filter(|s| !s.is_empty()).ok_or("name the areas to bring back")?;
            let scratch = e.dir.join("backup-chain-scratch");
            std::fs::create_dir_all(&scratch).map_err(|x| x.to_string())?;
            let r = backup::restore_chain(&l, &chain, pass.as_deref(), &scratch, &RestoreRequest { areas, allow_other_account: false, now: crate::clock::now_unix() });
            let _ = std::fs::remove_dir(&scratch);
            let r = r.map_err(|x| x.to_string())?;
            e.audit("backup.chain.restore", &r.restored.iter().map(|x| x.id.clone()).collect::<Vec<_>>().join(","));
            Ok(names(&r))
        }));
    }
    v
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::settingstools_tests::{assert_policy, rig, tools};
    use iyakki_shared::agent::Tools;

    #[test]
    fn share_system_and_chain_backups_keep_secrets_out_of_the_chat_and_ask_every_time() {
        let r = rig("backup-more", true);
        let fm = Arc::new(backup_share::FakeMounter::default());
        let t = tools(&r, entries_with(&r.env, fm.clone()));
        assert_policy(&t, &[], &TOOLS);
        let home = crate::scope::home_root(&r.env.user, &r.env.dir);
        // the rig's folder is named, not unique: a backup file left by an earlier run would meet the "nothing is overwritten" rule
        let _ = std::fs::remove_dir_all(home.join("b"));
        std::fs::create_dir_all(home.join("b")).unwrap();
        std::fs::create_dir_all(home.join("Documents")).unwrap();
        std::fs::write(home.join("Documents/a.txt"), "one").unwrap();
        std::fs::write(home.join("pass.txt"), "correct horse battery\n").unwrap();
        std::fs::write(home.join("smb.txt"), "ada\nsecret-pw\n").unwrap();
        // the share: a bad address and a missing passphrase are refused before anything is mounted
        assert!(t.call("backup_share_create", &json!({"address": "http://x/y", "unencrypted": true})).is_err());
        assert!(t.call("backup_share_create", &json!({"address": "nfs://h/export"})).unwrap_err().contains("passphrase"));
        assert!(fm.calls.lock().unwrap().is_empty());
        assert!(!t.describe("backup_share_create", &json!({"address": "smb://h/s", "credentials_file": "smb.txt", "passphrase_file": "pass.txt"})).contains("secret-pw"));
        let said = t.call("backup_share_create", &json!({"address": "smb://h/s/bk", "credentials_file": "smb.txt", "passphrase_file": "pass.txt"})).unwrap();
        assert!(said.contains("let go again") && !said.contains("secret-pw"), "{said}");
        let calls = fm.calls.lock().unwrap().clone();
        assert!(calls.len() == 2 && calls[0].starts_with("mount ") && calls[1].starts_with("umount "), "{calls:?}");
        assert!(!calls[0].contains("secret-pw"));
        // the machine's own data: always encrypted
        assert!(t.call("backup_system_create", &json!({"folder": "b"})).is_err());
        let said = t.call("backup_system_create", &json!({"folder": "b", "passphrase_file": "pass.txt"})).unwrap();
        assert!(said.contains("encrypted"), "{said}");
        // a chain: a full file made with the library, then a link through the tool, then a restore through the tool
        let l = layout(&r.env);
        let full = backup::create(&l, &home.join("b"), &CreateOptions { areas: backup::all_areas(), passphrase: Some("correct horse battery".into()), now: 1_700_000_000, release: "t".into() }).unwrap();
        std::fs::write(home.join("Documents/b.txt"), "two").unwrap();
        let full_name = format!("b/{}", file_name(&full.path));
        let said = t.call("backup_incremental_create", &json!({"folder": "b", "chain": [full_name], "passphrase_file": "pass.txt"})).unwrap();
        assert!(said.contains("changed files"), "{said}");
        assert!(t.call("backup_incremental_create", &json!({"folder": "b", "chain": []})).is_err());
        assert!(t.call("backup_chain_restore", &json!({"chain": ["b/none"], "areas": ["documents"]})).is_err());
        let audits = r.audits.lock().unwrap().join("\n");
        assert!(audits.contains("backup.share") && audits.contains("backup.system.create") && audits.contains("backup.incremental"), "{audits}");
    }

    #[test]
    fn the_machine_own_data_tools_are_not_offered_to_people_who_are_not_administrators() {
        let r = rig("backup-more-na", false);
        let t = tools(&r, entries_with(&r.env, Arc::new(backup_share::FakeMounter::default())));
        let names: Vec<String> = t.defs().iter().map(|d| d.spec.name.to_string()).collect();
        assert!(!names.iter().any(|n| n.starts_with("backup_system_")), "{names:?}");
        assert!(names.contains(&"backup_share_create".to_string()));
    }
}
