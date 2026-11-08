//! The lead assistant's tools for removing Quick install items: `quick_install_status` reads (names and states only, no card) and
//! `quick_uninstall` asks on a card every time (hard ask, `agent::hard_ask`: it is never run by Auto and never remembered).
//!
//! * The card says what goes, what stays, and whether the data is deleted (and how much is freed, and that it cannot be undone).
//! * `delete_data` is false unless the call says true; a removal that other installed items depend on is refused with their names
//!   unless `with_dependants` is true.
//! * Nothing secret is read: the tools learn item ids, titles and state words.

use std::sync::Arc;

use serde_json::{json, Value};

use crate::agent::{Risk, ToolDef, Tools};
use crate::chat::ToolSpec;
use crate::quickinstall::Item;

pub const PROMPT_RULE: &str = "\nQUICK INSTALL: quick_install_status lists every Quick install item with its state (names only). quick_uninstall removes one item: it ALWAYS asks the person on a card that says what goes, what stays and whether the data is deleted. Leave delete_data false unless the person asked to delete the data too (it cannot be undone). An item that others need (k3s, Lemonade) is refused with the list of what depends on it; with_dependants=true removes those first, with their data kept. A state like \"installed earlier; the last attempt failed: <reason>\" means the newest install or update failed (the reason starts with its cause: network problem, timed out, disk full, licence needed, dependency failed, cluster not ready, verify failed): tell the person the reason and offer a retry from Store -> Setup. hardware_firmware_install prepares the restricted hardware firmware this computer's kernel log asks for: its card shows the plan and the vendors' licence lines, and the person's approval of that card is their acceptance of those licences; never say a licence is accepted, and never try to accept one yourself.";

pub const READ_TOOL: &str = "quick_install_status";
pub const WRITE_TOOL: &str = "quick_uninstall";
/// Installs the restricted hardware firmware this computer's kernel log asks for. The card shows the plan and the vendors' licence lines;
/// approving the card is the person's acceptance (the assistant never accepts a licence, and a changed list needs a new card).
pub const FIRMWARE_TOOL: &str = "hardware_firmware_install";

pub fn is_quick_tool(n: &str) -> bool {
    n == READ_TOOL || n == WRITE_TOOL || n == FIRMWARE_TOOL
}

/// What the tools need from the machine (a fake in tests).
pub trait Backend: Send + Sync {
    /// `(id, title, state word)` of every item.
    fn rows(&self) -> Vec<(String, String, String)>;
    /// The card lines for removing `item` now, or the plain reason it cannot be removed.
    fn review(&self, item: Item, delete_data: bool, with_dependants: bool) -> Result<Vec<String>, String>;
    /// Do it; the plain result line.
    fn uninstall(&self, item: Item, delete_data: bool, with_dependants: bool) -> Result<String, String>;
    /// The card for the hardware firmware this computer needs now: its lines (the plan with the vendors' licence lines) and the id of
    /// exactly that list; the plain reason when there is nothing to install.
    fn firmware_card(&self, _include_nvidia: bool) -> Result<(Vec<String>, String), String> {
        Err("installing hardware firmware is not available here".into())
    }
    /// After the person approved the card with list id `list`: record their acceptance of exactly that list and start the job. Refuses
    /// when the list is not the one the card showed.
    fn firmware_install(&self, _include_nvidia: bool, _list: &str) -> Result<String, String> {
        Err("installing hardware firmware is not available here".into())
    }
}

pub struct QuickUninstallTools {
    inner: Arc<dyn Tools>,
    backend: Arc<dyn Backend>,
    /// The id of the firmware list the last card showed (the approval counts for that list only).
    shown_list: std::sync::Mutex<String>,
}

fn parse(args: &Value) -> Result<(Item, bool, bool), String> {
    let id = args.get("item").and_then(Value::as_str).unwrap_or("");
    let item = Item::parse(id).ok_or_else(|| format!("`{id}` is not a Quick install item; quick_install_status lists the ids"))?;
    let flag = |k: &str| args.get(k).and_then(Value::as_bool).unwrap_or(false);
    Ok((item, flag("delete_data"), flag("with_dependants")))
}

impl QuickUninstallTools {
    pub fn new(inner: Arc<dyn Tools>, backend: Arc<dyn Backend>) -> QuickUninstallTools {
        QuickUninstallTools { inner, backend, shown_list: std::sync::Mutex::new(String::new()) }
    }

    fn firmware_card(&self, args: &Value) -> Result<String, String> {
        let nv = args.get("include_nvidia").and_then(Value::as_bool).unwrap_or(false);
        let (lines, list) = self.backend.firmware_card(nv)?;
        *self.shown_list.lock().unwrap_or_else(|e| e.into_inner()) = list;
        Ok(lines.join("\n"))
    }

    fn card(&self, args: &Value) -> Result<String, String> {
        let (item, del, deps) = parse(args)?;
        let lines = self.backend.review(item, del, deps)?;
        Ok(lines.join("\n"))
    }
}

impl Tools for QuickUninstallTools {
    fn defs(&self) -> Vec<ToolDef> {
        let mut v = self.inner.defs();
        v.retain(|d| !is_quick_tool(&d.spec.name));
        v.push(ToolDef { spec: ToolSpec { name: READ_TOOL.into(), description: "List the Quick install items with their id, title and state (installed, not installed, installed earlier, not IYAKKI's). Names and states only.".into(), schema: json!({"type": "object", "properties": {}}) }, risk: Risk::Safe, untrusted: false });
        v.push(ToolDef {
            spec: ToolSpec {
                name: WRITE_TOOL.into(),
                description: "Remove a Quick install item (any of them). The person approves a card every time that says what goes, what stays and whether the data is deleted. The data (volumes, databases, models, vault entries, DASH connections) is kept unless delete_data is true.".into(),
                schema: json!({"type": "object", "properties": {
                    "item": {"type": "string", "description": "The item id from quick_install_status, for example k3s, elasticsearch, lemonade, seaweedfs."},
                    "delete_data": {"type": "boolean", "description": "true also deletes the data and cannot be undone; default false keeps it."},
                    "with_dependants": {"type": "boolean", "description": "true removes the installed items that need this one first (their data is kept); default false refuses and lists them."}
                }, "required": ["item"]}),
            },
            risk: Risk::Ask,
            untrusted: false,
        });
        v.push(ToolDef {
            spec: ToolSpec {
                name: FIRMWARE_TOOL.into(),
                description: "Install the restricted hardware firmware this computer's kernel log says is missing (Wi-Fi, Bluetooth, network, and other devices), from the pinned linux-firmware release, every file checked against its SHA-256. The person approves a card that lists the files and the vendors' licences: approving it is THEIR acceptance of those licences (you cannot accept for them, and there is no argument for it). Nothing is downloaded before they approve. The NVIDIA firmware is a separate licence group, only with include_nvidia=true.".into(),
                schema: json!({"type": "object", "properties": {
                    "include_nvidia": {"type": "boolean", "description": "true also lists the NVIDIA firmware group (its own licence); default false."}
                }}),
            },
            risk: Risk::Ask,
            untrusted: false,
        });
        v
    }

    fn call(&self, name: &str, args: &Value) -> Result<String, String> {
        match name {
            READ_TOOL => Ok(self.backend.rows().iter().map(|(id, t, s)| format!("{id}: {t}, {s}")).collect::<Vec<_>>().join("\n")),
            WRITE_TOOL => {
                let (item, del, deps) = parse(args)?;
                self.backend.uninstall(item, del, deps)
            }
            FIRMWARE_TOOL => {
                let list = self.shown_list.lock().unwrap_or_else(|e| e.into_inner()).clone();
                if list.is_empty() {
                    return Err("no card was shown for this list of firmware, so no licence was accepted; ask again".into());
                }
                self.backend.firmware_install(args.get("include_nvidia").and_then(Value::as_bool).unwrap_or(false), &list)
            }
            _ => self.inner.call(name, args),
        }
    }

    fn describe(&self, name: &str, args: &Value) -> String {
        match name {
            WRITE_TOOL => match self.card(args) {
                Ok(c) => c.lines().next().unwrap_or("").to_string(),
                Err(e) => format!("{name} (invalid: {e})"),
            },
            FIRMWARE_TOOL => match self.firmware_card(args) {
                Ok(c) => c.lines().next().unwrap_or("").to_string(),
                Err(e) => format!("{name} (nothing to do: {e})"),
            },
            READ_TOOL => "Show the Quick install items".into(),
            _ => self.inner.describe(name, args),
        }
    }

    fn preview(&self, name: &str, args: &Value) -> Option<String> {
        match name {
            WRITE_TOOL => self.card(args).ok(),
            FIRMWARE_TOOL => self.firmware_card(args).ok(),
            READ_TOOL => None,
            _ => self.inner.preview(name, args),
        }
    }

    fn refusal(&self, name: &str, args: &Value) -> Option<String> {
        match name {
            WRITE_TOOL => self.card(args).err(),
            FIRMWARE_TOOL => self.firmware_card(args).err(),
            READ_TOOL => None,
            _ => self.inner.refusal(name, args),
        }
    }

    fn pre_approved(&self, name: &str) -> bool {
        !is_quick_tool(name) && self.inner.pre_approved(name)
    }

    fn approved_always(&self, name: &str) -> bool {
        name != WRITE_TOOL && name != FIRMWARE_TOOL && self.inner.approved_always(name)
    }

    fn grant_blocker(&self, name: &str, args: &Value) -> Option<String> {
        if name == WRITE_TOOL {
            Some("removing a Quick install item asks every time".into())
        } else if name == FIRMWARE_TOOL {
            Some("installing hardware firmware asks every time: each approval is the person's acceptance of that list's licences".into())
        } else {
            self.inner.grant_blocker(name, args)
        }
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
}

/// The real backend: this machine's Quick install.
pub struct RealBackend {
    pub svc: Arc<crate::services::Services>,
    pub user: String,
}

impl Backend for RealBackend {
    fn rows(&self) -> Vec<(String, String, String)> {
        crate::quickinstall::uninstall::status_rows(&self.svc).into_iter().map(|(a, b, c)| (a.to_string(), b.to_string(), c)).collect()
    }

    fn review(&self, item: Item, delete_data: bool, with_dependants: bool) -> Result<Vec<String>, String> {
        use crate::quickinstall::uninstall as u;
        if let Some(why) = u::not_ours(&crate::quickinstall::presence(&self.svc, item)) {
            return Err(format!("{}: {why}", item.title()));
        }
        if !u::removable(&self.svc, item) {
            return Err(format!("{} is not installed, so there is nothing to remove.", item.title()));
        }
        let svc = self.svc.clone();
        let installed = move |i: Item| u::is_installed(&svc, i);
        let (deps, warn) = u::dependency_lines(item, &installed);
        if !deps.is_empty() && !with_dependants {
            return Err(u::refusal_text(item, &deps));
        }
        let ctx = u::real_ctx(&self.svc, &self.user, item);
        let p = u::plan(item, &ctx);
        if p.data_requires && !delete_data {
            return Err(format!("{} is only its own data: ask again with delete_data=true if the person wants it deleted.", item.title()));
        }
        let mut lines = Vec::new();
        for d in &deps {
            lines.push(format!("First removes {} (its data is kept).", d.title()));
        }
        lines.extend(p.review(delete_data).lines(delete_data));
        lines.extend(warn);
        Ok(lines)
    }

    fn uninstall(&self, item: Item, delete_data: bool, with_dependants: bool) -> Result<String, String> {
        crate::quickinstall::uninstall::uninstall_real(&self.svc, &self.user, item, crate::quickinstall::uninstall::Opts { delete_data, with_dependants }, None)
    }

    fn firmware_card(&self, include_nvidia: bool) -> Result<(Vec<String>, String), String> {
        let (sel, plan_lines) = self.firmware_selection(include_nvidia)?;
        let m = crate::fwextra::manifest();
        let list = sel.digest(m);
        let mut lines = vec![format!("Install hardware firmware: {} file(s), {} KB with the licence texts, from linux-firmware {}, downloaded from git.kernel.org on approval and checked against pinned SHA-256 sums.", sel.unique().len(), sel.bytes(m) >> 10, crate::fwextra::TAG)];
        lines.push("Approving this card is YOUR acceptance of the vendors' licences for exactly this list:".into());
        lines.extend(plan_lines);
        lines.push(format!("List id {}. If the list changes, you are asked again. A restart may be needed for a driver to pick the files up; the job says so.", &list[..16]));
        Ok((lines, list))
    }

    fn firmware_install(&self, include_nvidia: bool, list: &str) -> Result<String, String> {
        let (sel, _) = self.firmware_selection(include_nvidia)?;
        if sel.digest(crate::fwextra::manifest()) != list {
            return Err("the list of firmware changed since the card was shown, so nothing was accepted or downloaded; ask again to see the new list".into());
        }
        let l = crate::quickinstall::Launch::this_program(self.svc.dir())?;
        crate::fwextra::accept(self.svc.dir(), &sel, &format!("chat:{}", self.user), crate::clock::now_unix())?;
        let opts = crate::quickinstall::HwFirmwareOpts { from: None, with: if include_nvidia { vec!["nvidia".into()] } else { Vec::new() }, link: None };
        let j = crate::quickinstall::start_hardware_firmware(&self.svc.jobs, &self.user, &opts, &l)?;
        Ok(format!("Started: {}. The licences of the approved list were accepted by {}; progress is in Store -> Setup and beside the clock. If it fails, the card there shows why (network problem, timed out, disk full, verify failed) and Retry.", j.label, self.user))
    }
}

impl RealBackend {
    /// The files this computer's kernel log asks for (and the separate NVIDIA group when asked), with the plan lines.
    fn firmware_selection(&self, include_nvidia: bool) -> Result<(crate::fwextra::Selection, Vec<String>), String> {
        let plan = crate::fwextra_cli::plan_here(self.svc.dir());
        let sel = plan.select(&if include_nvidia { vec!["nvidia".to_string()] } else { Vec::new() });
        if sel.files.is_empty() {
            return Err("No missing firmware file was found in the kernel log: nothing to install.".into());
        }
        Ok((sel, plan.lines()))
    }
}
