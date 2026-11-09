//! The lead assistant's tools for formatting and resizing a partition: `disks_format_plan` shows the dry run (safe: it changes nothing),
//! `disks_format` and `disks_resize` ask every time (a hard ask, no standing allow) and, when approved, only leave a request for the
//! DISKS tile. The tile then shows the same dry run, asks the person to type the drive's or partition's name or label, and counts down
//! five seconds with a Cancel button before the first write. What is typed there never reaches the model, and the model has no argument
//! that could carry it: the confirmation is on the screen only.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use serde_json::{json, Value};

use crate::agent::{Risk, ToolDef, Tools};
use crate::chat::ToolSpec;
use crate::disks_part::{FormatFs, PartOp};

pub const PROMPT_RULE: &str = "\nFORMAT AND RESIZE: disks_format_plan (partition, and either filesystem [ext4, fat32, exfat, ntfs] with an optional label, or new_size) shows exactly what would happen and changes nothing. disks_format and disks_resize ask the person first (every time), then the DISKS tile shows the plan again and the person types the drive's or partition's name or label and waits out a 5 second countdown: you cannot type, confirm or skip that, so tell the person to do it on the screen. Always call disks_format_plan first and explain what will be erased. exFAT, NTFS and resizing need programs the person installs from CLI tools; if the plan says one is missing, say so. The system drive and anything mounted or unlocked are refused.";

pub const PLAN_TOOL: &str = "disks_format_plan";
pub const FORMAT_TOOL: &str = "disks_format";
pub const RESIZE_TOOL: &str = "disks_resize";

pub fn is_format_tool(n: &str) -> bool {
    matches!(n, PLAN_TOOL | FORMAT_TOOL | RESIZE_TOOL)
}

/// What the assistant asks for (data from a model: checked before it is used).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Req {
    Format { part: String, fs: FormatFs, label: String },
    Resize { part: String, bytes: u64 },
}

impl Req {
    pub fn part(&self) -> &str {
        match self {
            Req::Format { part, .. } | Req::Resize { part, .. } => part,
        }
    }
}

fn fs_arg(s: &str) -> Result<FormatFs, String> {
    match s.trim().to_lowercase().as_str() {
        "ext4" => Ok(FormatFs::Ext4),
        "fat32" | "fat" | "vfat" => Ok(FormatFs::Fat32),
        "exfat" => Ok(FormatFs::Exfat),
        "ntfs" => Ok(FormatFs::Ntfs),
        other => Err(format!("\"{other}\" is not a file system IYAKKI can make: ext4, fat32, exfat or ntfs")),
    }
}

fn part_arg(args: &Value) -> Result<String, String> {
    let p = args.get("partition").and_then(Value::as_str).unwrap_or("").trim();
    if p.is_empty() || p.len() > 32 || !p.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_')) {
        return Err("name the partition, for example sdc1".into());
    }
    Ok(p.to_string())
}

fn label_arg(args: &Value) -> Result<String, String> {
    let l = args.get("label").and_then(Value::as_str).unwrap_or("").trim();
    if l.chars().count() > 32 || l.chars().any(|c| c.is_control()) {
        return Err("the label is at most 32 characters, without control characters".into());
    }
    Ok(l.to_string())
}

fn format_req(args: &Value) -> Result<Req, String> {
    let fs = fs_arg(args.get("filesystem").and_then(Value::as_str).unwrap_or(""))?;
    Ok(Req::Format { part: part_arg(args)?, fs, label: label_arg(args)? })
}

fn resize_req(args: &Value) -> Result<Req, String> {
    let s = args.get("new_size").and_then(Value::as_str).unwrap_or("");
    Ok(Req::Resize { part: part_arg(args)?, bytes: crate::disks_fmt::parse_size(s)? })
}

/// For `disks_format_plan`: a file system means a format, a new size a resize.
fn plan_req(args: &Value) -> Result<Req, String> {
    let has = |k: &str| args.get(k).and_then(Value::as_str).is_some_and(|s| !s.trim().is_empty());
    match (has("filesystem"), has("new_size")) {
        (true, false) => format_req(args),
        (false, true) => resize_req(args),
        _ => Err("give either filesystem (to format) or new_size (to resize), not both".into()),
    }
}

/// What the tools need from the machine (a fake in tests).
pub trait Backend: Send + Sync {
    /// The dry run in words; changes nothing. `Err` is a refusal in plain words.
    fn plan(&self, req: &Req) -> Result<String, String>;
    /// Check the request would be accepted and leave it for the shell. The person confirms there.
    fn request(&self, req: &Req) -> Result<String, String>;
}

pub struct DisksFormatTools {
    inner: Arc<dyn Tools>,
    backend: Arc<dyn Backend>,
}

impl DisksFormatTools {
    pub fn new(inner: Arc<dyn Tools>, backend: Arc<dyn Backend>) -> DisksFormatTools {
        DisksFormatTools { inner, backend }
    }

    fn req_for(name: &str, args: &Value) -> Result<Req, String> {
        match name {
            PLAN_TOOL => plan_req(args),
            FORMAT_TOOL => format_req(args),
            _ => resize_req(args),
        }
    }
}

impl Tools for DisksFormatTools {
    fn defs(&self) -> Vec<ToolDef> {
        let mut v = self.inner.defs();
        v.retain(|d| !is_format_tool(&d.spec.name));
        let fmt = json!({"type": "object", "properties": {"partition": {"type": "string", "description": "the partition, for example sdc1"}, "filesystem": {"type": "string", "enum": ["ext4", "fat32", "exfat", "ntfs"]}, "label": {"type": "string", "description": "the new name of the volume (optional)"}}, "required": ["partition", "filesystem"]});
        let rsz = json!({"type": "object", "properties": {"partition": {"type": "string", "description": "the partition, for example sdc1"}, "new_size": {"type": "string", "description": "the new size of the partition, for example 8 GB"}}, "required": ["partition", "new_size"]});
        let plan = json!({"type": "object", "properties": {"partition": {"type": "string"}, "filesystem": {"type": "string", "enum": ["ext4", "fat32", "exfat", "ntfs"], "description": "to preview a format"}, "label": {"type": "string"}, "new_size": {"type": "string", "description": "to preview a resize, for example 8 GB"}}, "required": ["partition"]});
        v.push(ToolDef { spec: ToolSpec { name: PLAN_TOOL.into(), description: "Show exactly what formatting a partition (filesystem) or resizing it (new_size) would do, and what would be erased. Changes nothing. Call this first.".into(), schema: plan }, risk: Risk::Safe, untrusted: false });
        v.push(ToolDef { spec: ToolSpec { name: FORMAT_TOOL.into(), description: "Format a partition. ERASES everything on it. The person approves a card, then confirms by typing the name on the DISKS tile and waiting a 5 second countdown; you cannot confirm for them.".into(), schema: fmt }, risk: Risk::Ask, untrusted: false });
        v.push(ToolDef { spec: ToolSpec { name: RESIZE_TOOL.into(), description: "Grow or shrink a partition and its file system. The person approves a card, then confirms by typing the name on the DISKS tile and waiting a 5 second countdown.".into(), schema: rsz }, risk: Risk::Ask, untrusted: false });
        v
    }

    fn call(&self, name: &str, args: &Value) -> Result<String, String> {
        match name {
            PLAN_TOOL => self.backend.plan(&plan_req(args)?),
            FORMAT_TOOL | RESIZE_TOOL => self.backend.request(&Self::req_for(name, args)?),
            _ => self.inner.call(name, args),
        }
    }

    fn describe(&self, name: &str, args: &Value) -> String {
        match Self::req_for(name, args) {
            Ok(Req::Format { part, fs, .. }) if is_format_tool(name) => format!("{} {part} as {} (erases everything on it)", if name == PLAN_TOOL { "Preview formatting" } else { "Format" }, fs.name()),
            Ok(Req::Resize { part, bytes }) if is_format_tool(name) => format!("{} {part} to {}", if name == PLAN_TOOL { "Preview resizing" } else { "Resize" }, iyakki_storage::human(bytes)),
            _ if is_format_tool(name) => "Format or resize a partition".into(),
            _ => self.inner.describe(name, args),
        }
    }

    /// The card is the dry run: the person reads what will be erased before approving, and again on the tile before typing.
    fn preview(&self, name: &str, args: &Value) -> Option<String> {
        match name {
            FORMAT_TOOL | RESIZE_TOOL => Some(match Self::req_for(name, args).and_then(|r| self.backend.plan(&r)) {
                Ok(t) => format!("{t}\nAfter you approve, the DISKS tile shows this again; you type the name there and wait 5 seconds. The assistant cannot do that for you."),
                Err(e) => format!("Cannot be done: {e}"),
            }),
            PLAN_TOOL => None,
            _ => self.inner.preview(name, args),
        }
    }

    fn refusal(&self, name: &str, args: &Value) -> Option<String> {
        match name {
            PLAN_TOOL | FORMAT_TOOL | RESIZE_TOOL => Self::req_for(name, args).err(),
            _ => self.inner.refusal(name, args),
        }
    }

    fn pre_approved(&self, name: &str) -> bool {
        !is_format_tool(name) && self.inner.pre_approved(name)
    }

    fn approved_always(&self, name: &str) -> bool {
        !matches!(name, FORMAT_TOOL | RESIZE_TOOL) && self.inner.approved_always(name)
    }

    fn grant_blocker(&self, name: &str, args: &Value) -> Option<String> {
        if matches!(name, FORMAT_TOOL | RESIZE_TOOL) { Some("erasing or resizing a partition is the person's decision every time, so it asks every time".into()) } else { self.inner.grant_blocker(name, args) }
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
    dir.join(format!("disks-format-request-{}", who(user)))
}

fn fs_word(f: FormatFs) -> &'static str {
    match f {
        FormatFs::Ext4 => "ext4",
        FormatFs::Fat32 => "fat32",
        FormatFs::Exfat => "exfat",
        FormatFs::Ntfs => "ntfs",
    }
}

/// Ask the shell of `user` to show the review for `req`. One line of plain text, no confirmation in it.
pub fn request(dir: &Path, user: &str, req: &Req) {
    let line = match req {
        Req::Format { part, fs, label } => format!("format\t{part}\t{}\t{label}", fs_word(*fs)),
        Req::Resize { part, bytes } => format!("resize\t{part}\t{bytes}"),
    };
    let _ = std::fs::write(request_path(dir, user), line.as_bytes());
}

/// The shell takes its request (once). Anything that does not parse is dropped.
pub fn take_request(dir: &Path, user: &str) -> Option<Req> {
    let p = request_path(dir, user);
    let t = std::fs::read_to_string(&p).ok()?;
    let _ = std::fs::remove_file(&p);
    let mut it = t.trim_end_matches('\n').split('\t');
    let kind = it.next()?;
    let part = it.next()?.to_string();
    if part.is_empty() || part.len() > 32 || !part.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_')) {
        return None;
    }
    match kind {
        "format" => {
            let fs = fs_arg(it.next()?).ok()?;
            let label = it.next().unwrap_or("").to_string();
            (label.chars().count() <= 32 && !label.chars().any(|c| c.is_control())).then_some(Req::Format { part, fs, label })
        }
        "resize" => Some(Req::Resize { part, bytes: it.next()?.parse().ok().filter(|b| *b >= 1 << 20)? }),
        _ => None,
    }
}

/// Turn a request into the engine's change: the drive and partition number the name belongs to.
pub fn to_op(snap: &crate::disks::Snapshot, req: &Req) -> Result<(String, PartOp), String> {
    let (disk, p) = snap.part(req.part()).map(|(d, p)| (d.name.clone(), p.number)).ok_or_else(|| format!("{}: there is no such partition (the drive may have been unplugged).", req.part()))?;
    if p == 0 {
        return Err(format!("{} is a file system straight on the drive without a partition table; make a partition table in the DISKS tile first.", req.part()));
    }
    Ok((disk, match req {
        Req::Format { fs, label, .. } => PartOp::Format { number: p, fs: *fs, label: label.clone() },
        Req::Resize { bytes, .. } => PartOp::Resize { number: p, size_bytes: *bytes },
    }))
}

/// The real backend: the engine (with the person-installed programs set) and the data folder.
pub struct RealBackend {
    pub engine: Arc<crate::disks::Disks>,
    pub who: crate::disks::Actor,
    pub dir: PathBuf,
    pub audit: Arc<dyn Fn(&str, &str) + Send + Sync>,
}

impl Backend for RealBackend {
    fn plan(&self, req: &Req) -> Result<String, String> {
        let snap = self.engine.scan();
        let (disk, op) = to_op(&snap, req)?;
        self.engine.plan_partition(&self.who, &disk, &op).map(|p| p.render())
    }

    fn request(&self, req: &Req) -> Result<String, String> {
        let text = self.plan(req).inspect_err(|e| (self.audit)("disks.format.refused", &format!("part={} why={e}", req.part())))?;
        request(&self.dir, &self.who.user, req);
        (self.audit)("disks.format.requested", &format!("part={} op={}", req.part(), if matches!(req, Req::Format { .. }) { "format" } else { "resize" }));
        Ok(format!("{text}\nThe review is waiting in the DISKS tile (open DISKS). Tell the person to read it, type the name or label shown there, and wait for the 5 second countdown; they can cancel. Nothing has been changed yet, and you cannot confirm it for them."))
    }
}
