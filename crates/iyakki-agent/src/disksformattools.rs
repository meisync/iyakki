//! The lead assistant's tools for formatting and resizing a partition: `disks_format_plan` shows the dry run (safe: it changes nothing),
//! `disks_format` and `disks_resize` ask every time (a hard ask, no standing allow) and, when approved, only leave a request for the
//! DISKS tile. The tile then shows the same dry run, asks the person to type the drive's or partition's name or label, and counts down
//! five seconds with a Cancel button before the first write. What is typed there never reaches the model, and the model has no argument
//! that could carry it: the confirmation is on the screen only.

use std::path::PathBuf;
use std::sync::Arc;

use serde_json::{json, Value};

use crate::agent::{Risk, ToolDef, Tools};
use crate::chat::ToolSpec;
pub use iyakki_disksx::disksformattools::{request, take_request, to_op, Req};
use iyakki_disksx::disksformattools::fs_arg;
pub use iyakki_shared::disksformattools::{PROMPT_RULE};


pub const PLAN_TOOL: &str = "disks_format_plan";
pub const FORMAT_TOOL: &str = "disks_format";
pub const RESIZE_TOOL: &str = "disks_resize";

pub fn is_format_tool(n: &str) -> bool {
    matches!(n, PLAN_TOOL | FORMAT_TOOL | RESIZE_TOOL)
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

    fn inner_tools(&self) -> Option<&dyn Tools> {

        Some(&*self.inner)

    }
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
            Ok(Req::Resize { part, bytes }) if is_format_tool(name) => format!("{} {part} to {}", if name == PLAN_TOOL { "Preview resizing" } else { "Resize" }, iyakki_core_local::storage::human(bytes)),
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
