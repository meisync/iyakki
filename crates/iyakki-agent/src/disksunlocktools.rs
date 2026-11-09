//! The lead assistant's tools for encrypted drives: `disks_status` lists the partitions that are LUKS or BitLocker and whether they are
//! unlocked (safe), `disks_unlock` asks for a password ON THE SCREEN (it always asks, and never gets a standing allow), `disks_lock`
//! closes an unlocked volume (asks).
//!
//! The assistant never sees or passes a password. `disks_unlock` has one argument, the partition; when the person approves, the tool
//! only leaves a request for the shell, which opens the DISKS tile with a masked field. What is typed goes from that field straight
//! to the helper's pipe (`disks_unlock.rs`); no model, log or audit line ever holds it.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use serde_json::{json, Value};

use crate::agent::{Risk, ToolDef, Tools};
use crate::chat::ToolSpec;

pub const PROMPT_RULE: &str = "\nENCRYPTED DRIVES: disks_status lists LUKS and BitLocker partitions and whether each is unlocked. disks_unlock (partition, for example sdb1) asks the person first, then shows a masked password box in the DISKS tile: you never receive, type or ask for the password or recovery key, and must tell the person to type it in that box. A volume opens READ-ONLY; writing is a separate button the person presses. disks_lock closes an unlocked volume (asks first). Formatting and resizing are separate tools (below, administrators only).";

pub const STATUS_TOOL: &str = "disks_status";
pub const UNLOCK_TOOL: &str = "disks_unlock";
pub const LOCK_TOOL: &str = "disks_lock";

pub fn is_disks_tool(n: &str) -> bool {
    n == STATUS_TOOL || n == UNLOCK_TOOL || n == LOCK_TOOL
}

/// One encrypted partition, for the status.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Row {
    pub part: String,
    pub drive: String,
    /// "LUKS2", "BitLocker" ...
    pub kind: String,
    pub unlocked: bool,
    pub open_at: Option<String>,
    pub read_only: bool,
    /// Why it cannot be unlocked now ("damaged header", "the helper is missing"), if so.
    pub blocked: Option<String>,
}

/// What the tools need from the machine (a fake in tests).
pub trait Backend: Send + Sync {
    fn rows(&self) -> Vec<Row>;
    /// Check the partition could be unlocked and leave the request for the shell (the password is typed there).
    fn request_unlock(&self, part: &str) -> Result<String, String>;
    fn lock(&self, part: &str) -> Result<String, String>;
}

pub struct DisksCryptTools {
    inner: Arc<dyn Tools>,
    backend: Arc<dyn Backend>,
}

/// A partition name from a model is data: letters, digits, `-` and `_` only.
fn part_arg(args: &Value) -> Result<String, String> {
    let p = args.get("partition").and_then(Value::as_str).unwrap_or("").trim();
    if p.is_empty() || p.len() > 32 || !p.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_')) {
        return Err("name the partition, for example sdb1 (disks_status lists them)".into());
    }
    Ok(p.to_string())
}

impl DisksCryptTools {
    pub fn new(inner: Arc<dyn Tools>, backend: Arc<dyn Backend>) -> DisksCryptTools {
        DisksCryptTools { inner, backend }
    }

    fn status(&self) -> String {
        let rows = self.backend.rows();
        if rows.is_empty() {
            return "No encrypted (LUKS or BitLocker) partitions found.".into();
        }
        rows.iter()
            .map(|r| {
                let state = if r.unlocked {
                    match &r.open_at {
                        Some(m) => format!("unlocked, open at {m}{}", if r.read_only { " (read-only)" } else { " (writable)" }),
                        None => "unlocked, not open".to_string(),
                    }
                } else {
                    r.blocked.as_ref().map_or("locked".to_string(), |b| format!("locked ({b})"))
                };
                format!("{} on {}: {} {state}", r.part, r.drive, r.kind)
            })
            .collect::<Vec<_>>()
            .join("\n")
    }
}

impl Tools for DisksCryptTools {
    fn defs(&self) -> Vec<ToolDef> {
        let mut v = self.inner.defs();
        v.retain(|d| !is_disks_tool(&d.spec.name));
        let one = json!({"type": "object", "properties": {"partition": {"type": "string", "description": "the partition, for example sdb1"}}, "required": ["partition"]});
        v.push(ToolDef { spec: ToolSpec { name: STATUS_TOOL.into(), description: "List the encrypted (LUKS, BitLocker) partitions and whether each is locked or unlocked.".into(), schema: json!({"type": "object", "properties": {}}) }, risk: Risk::Safe, untrusted: false });
        v.push(ToolDef { spec: ToolSpec { name: UNLOCK_TOOL.into(), description: "Unlock an encrypted partition. The person approves a card, then types the password or recovery key into a masked box in the DISKS tile; you never get it. The drive opens read-only.".into(), schema: one.clone() }, risk: Risk::Ask, untrusted: false });
        v.push(ToolDef { spec: ToolSpec { name: LOCK_TOOL.into(), description: "Lock an unlocked encrypted partition again (it is unmounted first and closed). The person approves a card.".into(), schema: one }, risk: Risk::Ask, untrusted: false });
        v
    }

    fn call(&self, name: &str, args: &Value) -> Result<String, String> {
        match name {
            STATUS_TOOL => Ok(self.status()),
            UNLOCK_TOOL => self.backend.request_unlock(&part_arg(args)?),
            LOCK_TOOL => self.backend.lock(&part_arg(args)?),
            _ => self.inner.call(name, args),
        }
    }

    fn describe(&self, name: &str, args: &Value) -> String {
        match name {
            STATUS_TOOL => "List the encrypted drives".into(),
            UNLOCK_TOOL => format!("Unlock the encrypted drive {} (you type the password on the screen, read-only first)", part_arg(args).unwrap_or_else(|_| "?".into())),
            LOCK_TOOL => format!("Lock the encrypted drive {} again (unmount and close it)", part_arg(args).unwrap_or_else(|_| "?".into())),
            _ => self.inner.describe(name, args),
        }
    }

    fn preview(&self, name: &str, args: &Value) -> Option<String> {
        match name {
            UNLOCK_TOOL => Some(match part_arg(args) {
                Ok(p) => format!("Drive: {p}\nThe DISKS tile will show a password box. The assistant never sees what you type.\nThe drive opens read-only; nothing is written to it."),
                Err(e) => format!("Cannot unlock: {e}"),
            }),
            LOCK_TOOL => Some(match part_arg(args) {
                Ok(p) => format!("Drive: {p}\nIt is unmounted, then closed. Close what uses it first."),
                Err(e) => format!("Cannot lock: {e}"),
            }),
            STATUS_TOOL => None,
            _ => self.inner.preview(name, args),
        }
    }

    fn refusal(&self, name: &str, args: &Value) -> Option<String> {
        match name {
            UNLOCK_TOOL | LOCK_TOOL => part_arg(args).err(),
            STATUS_TOOL => None,
            _ => self.inner.refusal(name, args),
        }
    }

    fn pre_approved(&self, name: &str) -> bool {
        !is_disks_tool(name) && self.inner.pre_approved(name)
    }

    fn approved_always(&self, name: &str) -> bool {
        !matches!(name, UNLOCK_TOOL | LOCK_TOOL) && self.inner.approved_always(name)
    }

    fn grant_blocker(&self, name: &str, args: &Value) -> Option<String> {
        if matches!(name, UNLOCK_TOOL | LOCK_TOOL) { Some("opening and closing an encrypted drive is the person's decision every time, so it asks every time".into()) } else { self.inner.grant_blocker(name, args) }
    }
    fn grant_scope(&self, name: &str, args: &Value) -> String {
        self.inner.grant_scope(name, args)
    }
    fn unresolved_work(&self) -> Vec<String> {
        self.inner.unresolved_work()
    }
    fn begin_step(&self) {
        self.inner.begin_step()
    }
    fn should_end_run(&self) -> Option<String> {
        self.inner.should_end_run()
    }
    fn take_diagnosis(&self) -> Option<(String, String)> {
        self.inner.take_diagnosis()
    }
    fn note_external(&self, name: &str, args: &Value, ok: bool) {
        self.inner.note_external(name, args, ok)
    }
    fn take_notes(&self) -> Vec<String> {
        self.inner.take_notes()
    }
    fn jobs_running(&self) -> Vec<String> {
        self.inner.jobs_running()
    }
}

// ---------------------------------------------------------------------------------------------------------------------
// the request file between the tool and the shell
// ---------------------------------------------------------------------------------------------------------------------

fn who(user: &str) -> String {
    user.chars().filter(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.')).collect()
}

fn request_path(dir: &Path, user: &str) -> PathBuf {
    dir.join(format!("disks-unlock-request-{}", who(user)))
}

/// Ask the shell of `user` to open the unlock box for `part`. The file holds a partition name and nothing else.
pub fn request(dir: &Path, user: &str, part: &str) {
    let _ = std::fs::write(request_path(dir, user), part.as_bytes());
}

/// The shell takes its request (once). Only a plain partition name comes out.
pub fn take_request(dir: &Path, user: &str) -> Option<String> {
    let p = request_path(dir, user);
    let t = std::fs::read_to_string(&p).ok()?;
    let _ = std::fs::remove_file(&p);
    let t = t.trim().to_string();
    (!t.is_empty() && t.len() <= 32 && t.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_'))).then_some(t)
}

/// The real backend: the engine, the helper runner and the data folder.
pub struct RealBackend {
    pub engine: Arc<crate::disks::Disks>,
    pub runner: Arc<dyn crate::disks_helper::CryptRunner>,
    pub who: crate::disks::Actor,
    pub dir: PathBuf,
    pub audit: Arc<dyn Fn(&str, &str) + Send + Sync>,
}

impl Backend for RealBackend {
    fn rows(&self) -> Vec<Row> {
        let snap = self.engine.scan();
        snap.disks
            .iter()
            .filter(|d| crate::disks::may_use(&self.who, d).is_ok())
            .flat_map(|d| {
                d.parts.iter().filter_map(|p| {
                    let c = p.crypt.as_ref()?;
                    let blocked = c.problem.clone().or_else(|| self.runner.available(c.kind).err().map(|_| format!("the {} program is not installed", c.kind.helper())));
                    Some(Row { part: p.name.clone(), drive: d.title(), kind: c.kind.name().into(), unlocked: p.unlocked, open_at: p.mountpoints.first().cloned(), read_only: p.mounted_ro, blocked })
                })
            })
            .collect()
    }

    fn request_unlock(&self, part: &str) -> Result<String, String> {
        let snap = self.engine.scan();
        let kind = self.engine.plan_unlock(&snap, &self.who, part, &*self.runner).inspect_err(|e| (self.audit)("disks.unlock.refused", &format!("part={part} why={e}")))?;
        request(&self.dir, &self.who.user, part);
        (self.audit)("disks.unlock.requested", &format!("part={part} kind={}", kind.name()));
        Ok(format!("The password box for {part} is waiting in the DISKS tile (open DISKS). Tell the person to type the {} there; you never see it. It opens read-only.", kind.secret_word()))
    }

    fn lock(&self, part: &str) -> Result<String, String> {
        let a = self.audit.clone();
        self.engine.lock(&self.who, part, &*self.runner, &move |x, y| a(x, y))
    }
}
