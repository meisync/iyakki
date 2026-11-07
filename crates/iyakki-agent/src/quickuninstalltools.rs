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

pub const PROMPT_RULE: &str = "\nQUICK INSTALL: quick_install_status lists every Quick install item with its state (names only). quick_uninstall removes one item: it ALWAYS asks the person on a card that says what goes, what stays and whether the data is deleted. Leave delete_data false unless the person asked to delete the data too (it cannot be undone). An item that others need (k3s, Lemonade) is refused with the list of what depends on it; with_dependants=true removes those first, with their data kept.";

pub const READ_TOOL: &str = "quick_install_status";
pub const WRITE_TOOL: &str = "quick_uninstall";

pub fn is_quick_tool(n: &str) -> bool {
    n == READ_TOOL || n == WRITE_TOOL
}

/// What the tools need from the machine (a fake in tests).
pub trait Backend: Send + Sync {
    /// `(id, title, state word)` of every item.
    fn rows(&self) -> Vec<(String, String, String)>;
    /// The card lines for removing `item` now, or the plain reason it cannot be removed.
    fn review(&self, item: Item, delete_data: bool, with_dependants: bool) -> Result<Vec<String>, String>;
    /// Do it; the plain result line.
    fn uninstall(&self, item: Item, delete_data: bool, with_dependants: bool) -> Result<String, String>;
}

pub struct QuickUninstallTools {
    inner: Arc<dyn Tools>,
    backend: Arc<dyn Backend>,
}

fn parse(args: &Value) -> Result<(Item, bool, bool), String> {
    let id = args.get("item").and_then(Value::as_str).unwrap_or("");
    let item = Item::parse(id).ok_or_else(|| format!("`{id}` is not a Quick install item; quick_install_status lists the ids"))?;
    let flag = |k: &str| args.get(k).and_then(Value::as_bool).unwrap_or(false);
    Ok((item, flag("delete_data"), flag("with_dependants")))
}

impl QuickUninstallTools {
    pub fn new(inner: Arc<dyn Tools>, backend: Arc<dyn Backend>) -> QuickUninstallTools {
        QuickUninstallTools { inner, backend }
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
        v
    }

    fn call(&self, name: &str, args: &Value) -> Result<String, String> {
        match name {
            READ_TOOL => Ok(self.backend.rows().iter().map(|(id, t, s)| format!("{id}: {t}, {s}")).collect::<Vec<_>>().join("\n")),
            WRITE_TOOL => {
                let (item, del, deps) = parse(args)?;
                self.backend.uninstall(item, del, deps)
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
            READ_TOOL => "Show the Quick install items".into(),
            _ => self.inner.describe(name, args),
        }
    }

    fn preview(&self, name: &str, args: &Value) -> Option<String> {
        match name {
            WRITE_TOOL => self.card(args).ok(),
            READ_TOOL => None,
            _ => self.inner.preview(name, args),
        }
    }

    fn refusal(&self, name: &str, args: &Value) -> Option<String> {
        match name {
            WRITE_TOOL => self.card(args).err(),
            READ_TOOL => None,
            _ => self.inner.refusal(name, args),
        }
    }

    fn pre_approved(&self, name: &str) -> bool {
        !is_quick_tool(name) && self.inner.pre_approved(name)
    }

    fn approved_always(&self, name: &str) -> bool {
        name != WRITE_TOOL && self.inner.approved_always(name)
    }

    fn grant_blocker(&self, name: &str, args: &Value) -> Option<String> {
        if name == WRITE_TOOL { Some("removing a Quick install item asks every time".into()) } else { self.inner.grant_blocker(name, args) }
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
}
