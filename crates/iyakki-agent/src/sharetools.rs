//! The lead assistant's tools for Settings -> Shared folders (`sharedfolder.rs`): `share_list` reads (no card); `share_create`,
//! `share_start`, `share_stop` and `share_delete` each ask on a card, every time (hard ask: a share makes files reachable from the
//! network).
//!
//! * The folder rule is the manager's own ([`crate::sharedfolder::Scope`]): an administrator any folder of the disks, everyone else only
//!   folders inside their own home. The card is made from the manager's own check, so a folder the manager would refuse never reaches
//!   a card.
//! * The card says what becomes reachable, by whom (this machine only, a link with a secret, a password, anyone on the network) and
//!   the address.
//! * Credentials never pass through the chat: a password share names a vault entry (`password_entry`) that the person stored with
//!   `vault_request`; the password is read from the vault when the share is made, never shown. The secret part of a link is not
//!   printed either: the result says where to copy it (Settings -> Shared folders).

use std::sync::Arc;

use serde_json::{json, Value};

use crate::agent::{Risk, ToolDef, Tools};
use crate::chat::ToolSpec;
use crate::sharedfolder::{Access, Draft, Mode, Scope, ShareCfg, ShareInfo, Shares, EXPIRIES};

pub const PROMPT_RULE: &str = "\nSHARED FOLDERS: share_list shows the person's shared folders. share_create, share_start, share_stop and share_delete each ask the person on a card that says what becomes reachable and by whom. An administrator may share any folder of the disks; everyone else only folders inside their own home. For a password share first call vault_request for the password, then pass its entry name as password_entry; never ask for or type a password or a link secret in the chat, and never repeat a link's secret part (it is in Settings -> Shared folders). Prefer access `link` or `local`; `open` makes the files visible to everyone on the network.";

pub const WRITE_TOOLS: [&str; 4] = ["share_create", "share_start", "share_stop", "share_delete"];
pub const READ_TOOL: &str = "share_list";

pub fn is_share_tool(n: &str) -> bool {
    n == READ_TOOL || WRITE_TOOLS.contains(&n)
}

/// What the tools need from the share manager and the vault (the real one is [`Shares`]; tests wrap it so nothing listens).
pub trait Api: Send + Sync {
    fn scope(&self) -> Scope;
    fn list(&self) -> Vec<ShareInfo>;
    fn check(&self, d: &Draft) -> Result<ShareCfg, String>;
    fn create(&self, d: &Draft) -> Result<String, String>;
    fn start(&self, id: &str) -> Result<(), String>;
    fn stop(&self, id: &str);
    fn delete(&self, id: &str) -> Result<(), String>;
    /// The password stored under this vault entry of the person (never shown to the assistant).
    fn vault_password(&self, entry: &str) -> Result<String, String>;
}

/// The real manager and the person's vault.
pub struct RealApi {
    pub shares: Arc<Shares>,
    pub svc: Arc<crate::services::Services>,
    pub user: String,
}

impl Api for RealApi {
    fn scope(&self) -> Scope {
        self.shares.scope()
    }
    fn list(&self) -> Vec<ShareInfo> {
        self.shares.tick();
        self.shares.list()
    }
    fn check(&self, d: &Draft) -> Result<ShareCfg, String> {
        self.shares.check(d)
    }
    fn create(&self, d: &Draft) -> Result<String, String> {
        self.shares.create(d)
    }
    fn start(&self, id: &str) -> Result<(), String> {
        self.shares.start(id)
    }
    fn stop(&self, id: &str) {
        self.shares.stop(id)
    }
    fn delete(&self, id: &str) -> Result<(), String> {
        self.shares.delete(id)
    }
    fn vault_password(&self, entry: &str) -> Result<String, String> {
        self.svc.user_get(&self.user, entry).map(|z| z.to_string()).map_err(|_| format!("there is no usable vault entry \"{entry}\" (the vault may be locked). Ask the person to store the password with vault_request, then pass its name as password_entry."))
    }
}

pub struct ShareTools {
    inner: Arc<dyn Tools>,
    api: Arc<dyn Api>,
}

fn text(args: &Value, k: &str) -> String {
    args.get(k).and_then(Value::as_str).unwrap_or("").trim().to_string()
}

/// The secret part of a link is never printed.
fn masked(url: &str, token: &str) -> String {
    if token.is_empty() { url.to_string() } else { url.replace(token, "…") }
}

/// Who can open a share with this access, in the words of the card.
fn reach(c: &ShareCfg) -> String {
    match c.access {
        Access::Local => "ONLY THIS COMPUTER (127.0.0.1) can open it.".into(),
        Access::Token => "Anyone who has the secret link can open it, from any computer that can reach this one. The link is a secret: send it only to people you trust. You can change it later to cut off the old one.".into(),
        Access::Password => "Anyone who knows the password can open it, from any computer that can reach this one. The password travels unencrypted on a plain network (http).".into(),
        Access::Open => "EVERYONE ON THIS NETWORK can see and open these files, no secret needed.".into(),
    }
}

fn what(c: &ShareCfg) -> String {
    match c.mode {
        Mode::Website => "as a website (index.html is shown; no list, no uploads)".into(),
        Mode::Files => if c.upload { format!("as a list of files; visitors can download and UPLOAD{} (largest {} MB)", if c.overwrite { ", and replace files" } else if c.mkdir { ", and make folders" } else { "" }, c.max_upload_mb) } else { "as a list of files, read only (downloads)".into() },
    }
}

fn expiry(c: &ShareCfg) -> String {
    EXPIRIES.iter().find(|(_, s)| *s == c.expiry_secs).map_or("until stopped".into(), |(l, s)| if *s == 0 { "until stopped".into() } else { format!("stops by itself after {l}") })
}

impl ShareTools {
    pub fn new(inner: Arc<dyn Tools>, api: Arc<dyn Api>) -> ShareTools {
        ShareTools { inner, api }
    }

    fn find(&self, args: &Value) -> Result<ShareInfo, String> {
        let w = text(args, "share");
        if w.is_empty() {
            return Err("name the share (its id or its name; share_list shows them)".into());
        }
        self.api.list().into_iter().find(|i| i.cfg.id == w || i.cfg.name.eq_ignore_ascii_case(&w)).ok_or_else(|| "there is no such share (share_list shows them)".into())
    }

    /// The draft of a `share_create` call, with the password read from the vault only when the call really runs.
    fn draft(&self, args: &Value, with_password: bool) -> Result<Draft, String> {
        for (k, v) in args.as_object().into_iter().flatten() {
            if let Some(s) = v.as_str() {
                if k != "folder" && (s.len() > 300 || s.contains('\n') || s.contains("-----BEGIN")) {
                    return Err("a value is not a plain short setting. Never send secrets through a tool.".into());
                }
            }
        }
        if args.get("password").is_some() {
            return Err("never send a password through a tool: store it with vault_request and pass the entry name as password_entry".into());
        }
        let access = match text(args, "access").as_str() {
            "" | "link" | "token" => Access::Token,
            "password" => Access::Password,
            "open" => Access::Open,
            "local" => Access::Local,
            _ => return Err("access is `link` (secret link), `password`, `open` (anyone on the network) or `local` (this computer only)".into()),
        };
        let mode = match text(args, "mode").as_str() {
            "" | "files" => Mode::Files,
            "website" => Mode::Website,
            _ => return Err("mode is `files` or `website`".into()),
        };
        let expiry_secs = match text(args, "expires").as_str() {
            "" => 86400,
            w => EXPIRIES.iter().find(|(l, _)| l.replace(' ', "") == w.replace(' ', "")).map(|(_, s)| *s).ok_or("expires is `1 hour`, `1 day`, `7 days` or `never`")?,
        };
        let entry = text(args, "password_entry");
        if access == Access::Password && entry.is_empty() {
            return Err("a password share needs password_entry: the NAME of a vault entry. Call vault_request first so the person types the password into their vault.".into());
        }
        let password = if access == Access::Password && with_password { self.api.vault_password(&entry)? } else if access == Access::Password { "placeholder-checked-later".into() } else { String::new() };
        let upload = args.get("upload").and_then(Value::as_bool).unwrap_or(false);
        Ok(Draft {
            name: text(args, "name"),
            folder: text(args, "folder"),
            mode,
            access,
            upload: upload && mode == Mode::Files,
            max_upload_mb: 25,
            bind: "all".into(),
            password,
            expiry_secs,
            confirm_open: access == Access::Open,
            ..Default::default()
        })
    }

    fn address_lines(&self, info: &ShareInfo) -> String {
        let c = &info.cfg;
        let urls: Vec<String> = info.urls.iter().map(|u| masked(u, &c.token)).collect();
        let mut s = urls.join(", ");
        if c.access == Access::Token {
            s.push_str(" (the part shown as … is the secret: copy the link from Settings -> Shared folders)");
        }
        s
    }

    fn card(&self, name: &str, args: &Value) -> Result<String, String> {
        match name {
            "share_create" => {
                let d = self.draft(args, false)?;
                let c = self.api.check(&d)?;
                let who = match self.api.scope() {
                    Scope::Admin => "as an administrator you may share any folder of the disks".to_string(),
                    Scope::Home(h) => format!("you may share folders inside your own home ({})", h.display()),
                };
                Ok(format!(
                    "Make the shared folder “{}” (it is NOT started yet; starting is a second question).\n• Folder: {} ({who})\n• Shown {}\n• {}\n• {}\n• Port {}.{}",
                    c.name,
                    c.folder,
                    what(&c),
                    reach(&c),
                    expiry(&c),
                    c.port,
                    if c.access == Access::Password { format!("\n• The password is read from your vault entry {} (the assistant never sees it).", text(args, "password_entry")) } else { String::new() }
                ))
            }
            "share_start" => {
                let i = self.find(args)?;
                if i.running {
                    return Err(format!("“{}” is already running", i.cfg.name));
                }
                Ok(format!("Start the shared folder “{}”: {}\n• Shown {}\n• {}\n• {}\n• Address: {}", i.cfg.name, i.cfg.folder, what(&i.cfg), reach(&i.cfg), expiry(&i.cfg), self.address_lines(&i)))
            }
            "share_stop" => {
                let i = self.find(args)?;
                Ok(format!("Stop the shared folder “{}”: nobody can open it until it is started again. The share is kept.", i.cfg.name))
            }
            "share_delete" => {
                let i = self.find(args)?;
                Ok(format!("DELETE the shared folder “{}” ({}): it stops now and is removed from the list{}. Your files are not touched. Its link or password stops working for good.", i.cfg.name, i.cfg.folder, if i.running { "" } else { " (it was not running)" }))
            }
            _ => Err("not a share tool".into()),
        }
    }

    fn one_line(&self, i: &ShareInfo) -> String {
        let c = &i.cfg;
        format!(
            "{} · {} · {} · {} · {} · {}{}{}",
            c.id,
            c.name,
            c.folder,
            what(&c.clone()),
            c.access.label(),
            if i.running { "RUNNING" } else { "stopped" },
            if i.running { format!(" at {}", self.address_lines(i)) } else { String::new() },
            i.error.as_ref().map(|e| format!(" · {e}")).unwrap_or_default()
        )
    }
}

impl Tools for ShareTools {
    fn defs(&self) -> Vec<ToolDef> {
        let mut v = self.inner.defs();
        v.retain(|d| !is_share_tool(&d.spec.name));
        let share = json!({"type": "object", "properties": {"share": {"type": "string", "description": "The share's id or name (share_list shows them)."}}, "required": ["share"]});
        let ask = |name: &str, desc: &str, schema: Value| ToolDef { spec: ToolSpec { name: name.into(), description: desc.into(), schema }, risk: Risk::Ask, untrusted: false };
        v.push(ToolDef { spec: ToolSpec { name: READ_TOOL.into(), description: "List the person's shared folders: name, folder, how it is shown, who can open it, whether it runs and its address (a link's secret part is left out).".into(), schema: json!({"type": "object", "properties": {}}) }, risk: Risk::Safe, untrusted: false });
        v.push(ask(
            "share_create",
            "Make a shared folder (it does not start). The person approves a card that says what becomes reachable and by whom. An administrator may share any folder of the disks, everyone else only folders in their own home. For access `password` call vault_request first and pass the entry name as password_entry; never send a password.",
            json!({"type": "object", "properties": {
                "name": {"type": "string", "description": "Short name (letters, digits, space, - _ .; at most 40)."},
                "folder": {"type": "string", "description": "Full path of the folder."},
                "mode": {"type": "string", "enum": ["files", "website"], "description": "files: a list to browse (default); website: serves index.html."},
                "access": {"type": "string", "enum": ["link", "password", "open", "local"], "description": "link: secret address (default); password: one shared password; open: anyone on the network; local: this computer only."},
                "password_entry": {"type": "string", "description": "access password only: the NAME of the person's vault entry that holds the password."},
                "upload": {"type": "boolean", "description": "Let visitors upload (files mode only; default false)."},
                "expires": {"type": "string", "enum": ["1 hour", "1 day", "7 days", "never"], "description": "How long a start lasts (default 1 day)."}
            }, "required": ["name", "folder"]}),
        ));
        v.push(ask("share_start", "Start a shared folder so it can be opened. The person approves a card with who can open it and the address.", share.clone()));
        v.push(ask("share_stop", "Stop a shared folder (it is kept).", share.clone()));
        v.push(ask("share_delete", "Delete a shared folder (stops it; the files are not touched).", share));
        v
    }

    fn call(&self, name: &str, args: &Value) -> Result<String, String> {
        match name {
            READ_TOOL => {
                let rows = self.api.list();
                if rows.is_empty() {
                    return Ok("No shared folders yet.".into());
                }
                Ok(rows.iter().map(|i| self.one_line(i)).collect::<Vec<_>>().join("\n"))
            }
            "share_create" => {
                self.card(name, args)?;
                let d = self.draft(args, true)?;
                let id = self.api.create(&d)?;
                Ok(format!("Made the shared folder “{}” (id {id}). It is not started: share_start asks the person.", d.name))
            }
            "share_start" => {
                let i = self.find(args)?;
                self.api.start(&i.cfg.id)?;
                let now = self.api.list().into_iter().find(|x| x.cfg.id == i.cfg.id).unwrap_or(i);
                Ok(format!("Started “{}”. {}\nAddress: {}", now.cfg.name, reach(&now.cfg), self.address_lines(&now)))
            }
            "share_stop" => {
                let i = self.find(args)?;
                self.api.stop(&i.cfg.id);
                Ok(format!("Stopped “{}”; it is kept.", i.cfg.name))
            }
            "share_delete" => {
                let i = self.find(args)?;
                self.api.delete(&i.cfg.id)?;
                Ok(format!("Deleted the shared folder “{}”. The files were not touched.", i.cfg.name))
            }
            _ => self.inner.call(name, args),
        }
    }

    fn describe(&self, name: &str, args: &Value) -> String {
        if WRITE_TOOLS.contains(&name) {
            return match self.card(name, args) {
                Ok(c) => c.lines().next().unwrap_or("").to_string(),
                Err(e) => format!("{name} (invalid: {e})"),
            };
        }
        if name == READ_TOOL { "List the shared folders".into() } else { self.inner.describe(name, args) }
    }

    fn preview(&self, name: &str, args: &Value) -> Option<String> {
        if WRITE_TOOLS.contains(&name) {
            return self.card(name, args).ok();
        }
        if name == READ_TOOL { None } else { self.inner.preview(name, args) }
    }

    fn refusal(&self, name: &str, args: &Value) -> Option<String> {
        if WRITE_TOOLS.contains(&name) {
            return self.card(name, args).err();
        }
        if name == READ_TOOL { None } else { self.inner.refusal(name, args) }
    }

    fn pre_approved(&self, name: &str) -> bool {
        !is_share_tool(name) && self.inner.pre_approved(name)
    }

    fn approved_always(&self, name: &str) -> bool {
        !WRITE_TOOLS.contains(&name) && self.inner.approved_always(name)
    }

    fn grant_blocker(&self, name: &str, args: &Value) -> Option<String> {
        if WRITE_TOOLS.contains(&name) { Some("sharing folders asks every time".into()) } else { self.inner.grant_blocker(name, args) }
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
