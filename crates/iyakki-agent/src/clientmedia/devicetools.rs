//! The lead assistant's tools for client devices (Settings, Remote access, Connect a client; EXPERIMENTAL): every setting there is also
//! reachable by chat.
//!
//! * `client_devices` (free): the enrolled devices, numbered, with what each may ask for (`audio.out`, `clipboard.client`, `camera.view`,
//!   `client.camera.offer`, `client.microphone.offer`). Names only: no key, no token, no link.
//! * `client_device_permission` (asks every time): switch one permission of one device on or off. "On" means allowed to ask: the clipboard
//!   and the device's camera and microphone still put a card on the machine at every use.
//! * `client_device_role` (asks every time): put one device on a role of the table (viewer, listener, presenter, full): every switch of the role at once.
//! * `client_device_revoke` (asks every time): revoke a device; its open session ends within a second.
//!
//! Administrators only (the same rule as the Settings page). Every change is audited like the Settings page does it.

use std::path::PathBuf;
use std::sync::Arc;

use serde_json::{json, Value};

use crate::agent::{Risk, ToolDef, Tools};
use crate::chat::ToolSpec;

use super::{device_holds, permission_capability, set_permission, DEVICE_PERMISSIONS};

pub use iyakki_shared::clientmedia::DEVICE_PROMPT_RULE as PROMPT_RULE;

pub const LIST_TOOL: &str = "client_devices";
pub const PERMISSION_TOOL: &str = "client_device_permission";
pub const REVOKE_TOOL: &str = "client_device_revoke";
pub const SOURCES_TOOL: &str = "client_sources";
pub const ROLE_TOOL: &str = "client_device_role";
pub const LOOK_TOOL: &str = "client_source_look";
pub const SOUND_TOOL: &str = "client_sound_mode";

pub fn is_device_tool(n: &str) -> bool {
    n == SOUND_TOOL || n == LIST_TOOL || n == ROLE_TOOL || n == PERMISSION_TOOL || n == REVOKE_TOOL || n == SOURCES_TOOL || n == LOOK_TOOL
}

/// One enrolled device as the tools show it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Row {
    pub id: String,
    pub name: String,
    pub by: String,
    pub last_seen: i64,
    /// Capabilities held now (broker names).
    pub holds: Vec<String>,
}

/// "never", "just now", "5 min ago", "3 h ago", "2 days ago".
pub fn ago(now: i64, at: i64) -> String {
    if at <= 0 {
        return "never".into();
    }
    match (now - at).max(0) {
        0..=59 => "just now".into(),
        d @ 60..=3599 => format!("{} min ago", d / 60),
        d @ 3600..=86399 => format!("{} h ago", d / 3600),
        d => format!("{} days ago", d / 86400),
    }
}

/// What the tools need from the machine (a fake in tests).
pub trait Backend: Send + Sync {
    fn is_admin(&self) -> bool;
    fn enabled(&self) -> bool;
    /// Active devices, newest first (the order of the Settings list).
    fn devices(&self) -> Vec<Row>;
    fn set(&self, device_id: &str, permission: &str, on: bool) -> Result<(), String>;
    fn revoke(&self, device_id: &str) -> Result<bool, String>;
    fn audit(&self, action: &str, detail: &str);
    /// The role a device is on (`viewer`, `listener`, `presenter`, `full`, or `custom`).
    fn role_of(&self, _device_id: &str) -> String {
        "custom".into()
    }
    /// Put a device on a role of the table.
    fn set_role(&self, _device_id: &str, _role: &str) -> Result<(), String> {
        Err("roles are not available here".into())
    }
    /// Does each person's client hear only that person's own sounds? (default: one sound per machine)
    fn sound_per_person(&self) -> bool {
        false
    }
    fn set_sound_per_person(&self, _on: bool) -> Result<(), String> {
        Err("the per-person sound is not available here".into())
    }
    /// The cameras and microphones client devices offer to this machine now, with the programs using each (names only).
    fn sources(&self) -> Vec<(String, Vec<String>)> {
        Vec::new()
    }
    /// Take the newest picture (camera) or the last moment of sound (microphone) of the shared source called `name` and keep it as a file
    /// in the person's data folder; the answer names the file. The source's own consent card was answered when it was offered.
    fn look(&self, _name: &str) -> Result<String, String> {
        Err("looking at a client's camera or microphone is not available here".into())
    }
}

pub struct DeviceTools {
    inner: Arc<dyn Tools>,
    backend: Arc<dyn Backend>,
}

impl DeviceTools {
    pub fn new(inner: Arc<dyn Tools>, backend: Arc<dyn Backend>) -> DeviceTools {
        DeviceTools { inner, backend }
    }

    /// A device by its number in the list or by its name (any case).
    fn find(&self, args: &Value) -> Result<Row, String> {
        let list = self.backend.devices();
        let key = args.get("device").map(|v| v.as_str().map(str::to_string).unwrap_or_else(|| v.to_string())).unwrap_or_default();
        let key = key.trim();
        if key.is_empty() {
            return Err("say which device: its number in client_devices or its name".into());
        }
        if let Ok(n) = key.parse::<usize>() {
            return list.get(n.wrapping_sub(1)).cloned().ok_or_else(|| format!("there is no device number {n}; client_devices lists them"));
        }
        let hits: Vec<&Row> = list.iter().filter(|r| r.name.eq_ignore_ascii_case(key)).collect();
        match hits.len() {
            1 => Ok(hits[0].clone()),
            0 => Err(format!("no enrolled device is called {key}; client_devices lists them")),
            _ => Err(format!("more than one device is called {key}; use its number")),
        }
    }

    fn role_arg(args: &Value) -> Result<(&'static str, &'static str), String> {
        let name = args.get("role").and_then(Value::as_str).unwrap_or("").trim();
        super::DEVICE_ROLES.iter().find(|r| r.0.eq_ignore_ascii_case(name)).map(|r| (r.0, r.1)).ok_or_else(|| format!("{name:?} is not a role; the choices are {}", super::DEVICE_ROLES.iter().map(|r| r.0).collect::<Vec<_>>().join(", ")))
    }

    /// `true` for `person`, `false` for `machine`.
    fn sound_mode(args: &Value) -> Result<bool, String> {
        match args.get("mode").and_then(Value::as_str).map(|m| m.trim().to_ascii_lowercase()).as_deref() {
            Some("person") => Ok(true),
            Some("machine") => Ok(false),
            _ => Err("say mode: \"machine\" or \"person\"".into()),
        }
    }

    fn permission_of(args: &Value) -> Result<(&'static str, &'static str, bool), String> {
        let name = args.get("permission").and_then(Value::as_str).unwrap_or("").trim();
        let Some((perm, _, what)) = DEVICE_PERMISSIONS.iter().find(|(n, c, _)| *n == name || *c == name).copied() else {
            return Err(format!("{name:?} is not something a device can be given; the choices are {}", DEVICE_PERMISSIONS.iter().map(|p| p.0).collect::<Vec<_>>().join(", ")));
        };
        let on = args.get("on").and_then(Value::as_bool).ok_or_else(|| "say on: true or on: false".to_string())?;
        Ok((perm, what, on))
    }

    fn list(&self) -> String {
        if !self.backend.is_admin() {
            return "Only an administrator sees the client devices.".into();
        }
        let on = self.backend.enabled();
        let devices = self.backend.devices();
        let mut out = format!("Client connections (experimental): {}.\n", if on { "on" } else { "off" });
        out.push_str(&format!("Sound to a client: {}.\n", if self.backend.sound_per_person() { "each person's own sounds go to that person's client" } else { "one sound for the machine, for the person it belongs to" }));
        if devices.is_empty() {
            out.push_str("No client device is enrolled.");
            return out;
        }
        for (i, d) in devices.iter().enumerate() {
            let allowed: Vec<&str> = DEVICE_PERMISSIONS.iter().filter(|(_, c, _)| d.holds.iter().any(|h| h == c)).map(|p| p.0).collect();
            out.push_str(&format!("{}. {} (enrolled by {}, last seen {}, role {}): may ask for {}\n", i + 1, d.name, d.by, ago(crate::clock::now_unix(), d.last_seen), self.backend.role_of(&d.id), if allowed.is_empty() { "nothing yet".to_string() } else { allowed.join(", ") }));
        }
        out.push_str("On means allowed to ask; the clipboard and the device's camera and microphone still ask on the machine at every use.");
        out
    }
}

impl Tools for DeviceTools {

    fn inner_tools(&self) -> Option<&dyn Tools> {

        Some(&*self.inner)

    }
    fn defs(&self) -> Vec<ToolDef> {
        let mut v = self.inner.defs();
        v.retain(|d| !is_device_tool(&d.spec.name));
        let perms: Vec<&str> = DEVICE_PERMISSIONS.iter().map(|p| p.0).collect();
        v.push(ToolDef { spec: ToolSpec { name: LIST_TOOL.into(), description: "List the client devices (browsers, windows, phones) enrolled to use this desktop, numbered, with what each may ask for. No keys or links are shown. Administrators only.".into(), schema: json!({"type": "object", "properties": {}}) }, risk: Risk::Safe, untrusted: false });
        v.push(ToolDef { spec: ToolSpec { name: SOURCES_TOOL.into(), description: "List the cameras and microphones that client devices share with this machine right now, and which programs use each. Names only.".into(), schema: json!({"type": "object", "properties": {}}) }, risk: Risk::Safe, untrusted: false });
        v.push(ToolDef { spec: ToolSpec { name: LOOK_TOOL.into(), description: "Take the newest picture of a client device's shared camera, or the last second of its shared microphone, and keep it as a file. The person approves a card first. Needs the source to be on offer (client_sources).".into(), schema: json!({"type": "object", "properties": {"source": {"type": "string", "description": "the source's name from client_sources"}}, "required": ["source"]}) }, risk: Risk::Ask, untrusted: false });
        v.push(ToolDef { spec: ToolSpec { name: PERMISSION_TOOL.into(), description: "Switch one permission of one client device on or off. The person approves a card first. On only means the device may ask: the clipboard and the device's camera and microphone still ask on the machine at every use.".into(), schema: json!({"type": "object", "properties": {"device": {"type": "string", "description": "the number from client_devices, or the device's name"}, "permission": {"type": "string", "enum": perms}, "on": {"type": "boolean"}}, "required": ["device", "permission", "on"]}) }, risk: Risk::Ask, untrusted: false });
        let roles: Vec<&str> = super::DEVICE_ROLES.iter().map(|r| r.0).collect();
        v.push(ToolDef { spec: ToolSpec { name: ROLE_TOOL.into(), description: "Put one client device on a role: viewer (asks for nothing), listener (hears this computer's sound), presenter (also shares its clipboard), full (also cameras and its own camera and microphone). Sets every switch of the role at once and takes the others away. The person approves a card first. A role only says what the device may ask for.".into(), schema: json!({"type": "object", "properties": {"device": {"type": "string", "description": "the number from client_devices, or the device's name"}, "role": {"type": "string", "enum": roles}}, "required": ["device", "role"]}) }, risk: Risk::Ask, untrusted: false });
        v.push(ToolDef { spec: ToolSpec { name: SOUND_TOOL.into(), description: "Choose how the machine's sound reaches client devices: 'machine' (one sound for the machine, played to the client of the person it belongs to; the default) or 'person' (each person's own sounds, such as their player, go only to that person's client; the machine's own sounds go to the person it belongs to). The person approves a card first. Administrators only.".into(), schema: json!({"type": "object", "properties": {"mode": {"type": "string", "enum": ["machine", "person"]}}, "required": ["mode"]}) }, risk: Risk::Ask, untrusted: false });
        v.push(ToolDef { spec: ToolSpec { name: REVOKE_TOOL.into(), description: "Revoke a client device: it cannot connect again and its open session ends within a second. The person approves a card first.".into(), schema: json!({"type": "object", "properties": {"device": {"type": "string", "description": "the number from client_devices, or the device's name"}}, "required": ["device"]}) }, risk: Risk::Ask, untrusted: false });
        v
    }

    fn call(&self, name: &str, args: &Value) -> Result<String, String> {
        match name {
            LIST_TOOL => Ok(self.list()),
            SOURCES_TOOL => {
                if !self.backend.is_admin() {
                    return Ok("Only an administrator sees what client devices share.".into());
                }
                let all = self.backend.sources();
                if all.is_empty() {
                    return Ok("No client device shares a camera or microphone with this machine now.".into());
                }
                Ok(all.iter().enumerate().map(|(i, (n, u))| format!("{}. {} ({})", i + 1, n, if u.is_empty() { "nothing uses it; what it sends is dropped".to_string() } else { format!("used by {}", u.join(", ")) })).collect::<Vec<_>>().join("\n"))
            }
            LOOK_TOOL => {
                if !self.backend.is_admin() {
                    return Err("Only an administrator may look at what a client device shares.".into());
                }
                let name = args.get("source").and_then(|v| v.as_str()).unwrap_or("").trim();
                if name.is_empty() {
                    return Err("say which source (the name from client_sources)".into());
                }
                let r = self.backend.look(name)?;
                self.backend.audit("client.source_look", &format!("{name} (chat)"));
                Ok(r)
            }
            PERMISSION_TOOL => {
                if !self.backend.is_admin() {
                    return Err("Only an administrator may change what a client device can ask for.".into());
                }
                let d = self.find(args)?;
                let (perm, what, on) = Self::permission_of(args)?;
                self.backend.set(&d.id, perm, on)?;
                self.backend.audit("settings.client_permission", &format!("{perm} {} {} {} (chat)", if on { "on" } else { "off" }, d.name, d.id));
                Ok(if on { format!("{} may now ask to {what}. It counts from the device's next request; where it says asked every use, a card still comes up on the machine each time.", d.name) } else { format!("{} may no longer {what}. It is refused from its next request.", d.name) })
            }
            ROLE_TOOL => {
                if !self.backend.is_admin() {
                    return Err("Only an administrator may change what a client device can ask for.".into());
                }
                let d = self.find(args)?;
                let (role, what) = Self::role_arg(args)?;
                self.backend.set_role(&d.id, role)?;
                self.backend.audit("settings.client_role", &format!("{role} {} {} (chat)", d.name, d.id));
                Ok(format!("{} is on the role {role}: it {what}. It counts from the device's next request; a card still comes up on the machine at every hard ask.", d.name))
            }
            SOUND_TOOL => {
                if !self.backend.is_admin() {
                    return Err("Only an administrator may change how sound reaches client devices.".into());
                }
                let on = Self::sound_mode(args)?;
                self.backend.set_sound_per_person(on)?;
                self.backend.audit("settings.client_sound", &format!("{} (chat)", if on { "person" } else { "machine" }));
                Ok(if on { "Each person's own sounds now go only to that person's client; the machine's own sounds go to the person it belongs to. It counts from the next sound.".into() } else { "Sound to clients is one sound for the machine again, for the person it belongs to.".into() })
            }
            REVOKE_TOOL => {
                if !self.backend.is_admin() {
                    return Err("Only an administrator may revoke a client device.".into());
                }
                let d = self.find(args)?;
                match self.backend.revoke(&d.id)? {
                    true => {
                        self.backend.audit("settings.client_revoke", &format!("{} {} (chat)", d.name, d.id));
                        Ok(format!("{} is revoked: it cannot connect again and its open session ends within a second.", d.name))
                    }
                    false => Err("That device is revoked already.".into()),
                }
            }
            _ => self.inner.call(name, args),
        }
    }

    fn describe(&self, name: &str, args: &Value) -> String {
        match name {
            LIST_TOOL => "List the client devices".into(),
            SOURCES_TOOL => "List what client devices share".into(),
            LOOK_TOOL => format!("Take a picture or a moment of sound from {}", args.get("source").and_then(|v| v.as_str()).unwrap_or("a client device")),
            ROLE_TOOL => match (self.find(args), Self::role_arg(args)) {
                (Ok(d), Ok((role, _))) => format!("Put {} on the role {role}", d.name),
                _ => "Change the role of a client device".into(),
            },
            PERMISSION_TOOL => match (self.find(args), Self::permission_of(args)) {
                (Ok(d), Ok((perm, _, on))) => format!("Switch {perm} {} for {}", if on { "on" } else { "off" }, d.name),
                _ => "Change what a client device may ask for".into(),
            },
            SOUND_TOOL => match Self::sound_mode(args) {
                Ok(true) => "Send each person's own sounds to that person's client".into(),
                Ok(false) => "Send one sound for the machine to the client".into(),
                Err(_) => "Change how sound reaches client devices".into(),
            },
            REVOKE_TOOL => match self.find(args) {
                Ok(d) => format!("Revoke the client device {}", d.name),
                Err(_) => "Revoke a client device".into(),
            },
            _ => self.inner.describe(name, args),
        }
    }

    fn preview(&self, name: &str, args: &Value) -> Option<String> {
        match name {
            PERMISSION_TOOL => Some(match (self.find(args), Self::permission_of(args)) {
                (Ok(d), Ok((perm, what, on))) => {
                    let now = permission_capability(perm).is_some_and(|c| d.holds.iter().any(|h| h == c));
                    format!("{}: {perm} is {} now, and would be {}.\nIt lets the device {what}. On only means allowed to ask; the clipboard and the device's camera and microphone still ask on the machine at every use.", d.name, if now { "on" } else { "off" }, if on { "on" } else { "off" })
                }
                (Err(e), _) | (_, Err(e)) => format!("Cannot change it: {e}"),
            }),
            ROLE_TOOL => Some(match (self.find(args), Self::role_arg(args)) {
                (Ok(d), Ok((role, what))) => format!("{}: on the role {} now, would be on {role}, which {what}. Every other switch a role does not have is taken away. A role only says what the device may ask for.", d.name, self.backend.role_of(&d.id)),
                (Err(e), _) | (_, Err(e)) => format!("Cannot change it: {e}"),
            }),
            SOUND_TOOL => Some(match Self::sound_mode(args) {
                Ok(true) => format!("Now: {}. Would be: each person's own sounds (their player, their apps) go only to that person's client, so one person never hears another's sound on their device. The machine's own sounds go to the person the machine's sound belongs to.", if self.backend.sound_per_person() { "per person" } else { "one sound for the machine" }),
                Ok(false) => "Would be: one sound for the machine, played to the client of the person it belongs to.".into(),
                Err(e) => format!("Cannot change it: {e}"),
            }),
            REVOKE_TOOL => Some(match self.find(args) {
                Ok(d) => format!("{} (enrolled by {}) will not be able to connect again; its open session ends. To use it again a new one-time link is needed.", d.name, d.by),
                Err(e) => format!("Cannot revoke: {e}"),
            }),
            LOOK_TOOL => Some(format!("Takes the newest picture, or the last second of sound, of {:?} (a camera or microphone a client device shares with this machine) and saves it in your data folder. Nothing is sent anywhere; the client's red band shows while it shares.", args.get("source").and_then(|v| v.as_str()).unwrap_or(""))),
            LIST_TOOL | SOURCES_TOOL => None,
            _ => self.inner.preview(name, args),
        }
    }

    fn refusal(&self, name: &str, args: &Value) -> Option<String> {
        match name {
            PERMISSION_TOOL if !self.backend.is_admin() => Some("Only an administrator may change what a client device can ask for.".into()),
            REVOKE_TOOL if !self.backend.is_admin() => Some("Only an administrator may revoke a client device.".into()),
            PERMISSION_TOOL => self.find(args).err().or_else(|| Self::permission_of(args).err()),
            REVOKE_TOOL => self.find(args).err(),
            LOOK_TOOL if !self.backend.is_admin() => Some("Only an administrator may look at what a client device shares.".into()),
            LOOK_TOOL if args.get("source").and_then(|v| v.as_str()).is_none_or(|n| n.trim().is_empty()) => Some("say which source (the name from client_sources)".into()),
            ROLE_TOOL if !self.backend.is_admin() => Some("Only an administrator may change what a client device can ask for.".into()),
            SOUND_TOOL if !self.backend.is_admin() => Some("Only an administrator may change how sound reaches client devices.".into()),
            SOUND_TOOL => Self::sound_mode(args).err(),
            ROLE_TOOL => self.find(args).err().or_else(|| Self::role_arg(args).err()),
            LIST_TOOL | SOURCES_TOOL => None,
            _ => self.inner.refusal(name, args),
        }
    }

    fn pre_approved(&self, name: &str) -> bool {
        !is_device_tool(name) && self.inner.pre_approved(name)
    }

    fn approved_always(&self, name: &str) -> bool {
        !is_device_tool(name) && self.inner.approved_always(name)
    }

    fn grant_blocker(&self, name: &str, args: &Value) -> Option<String> {
        if name == LOOK_TOOL { Some("a look at somebody's camera or microphone asks every time".into()) } else if name == PERMISSION_TOOL || name == REVOKE_TOOL || name == ROLE_TOOL || name == SOUND_TOOL { Some("what a device may ask for, and whether it is allowed at all, is the person's security choice, so it asks every time".into()) } else { self.inner.grant_blocker(name, args) }
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

/// The real backend: the data folder and the audit log.
pub struct RealBackend {
    pub dir: PathBuf,
    pub user: String,
    pub admin: bool,
    pub audit: Arc<dyn Fn(&str, &str) + Send + Sync>,
}

impl Backend for RealBackend {
    fn is_admin(&self) -> bool {
        self.admin
    }
    fn enabled(&self) -> bool {
        crate::clientlink::enabled(&self.dir)
    }
    fn devices(&self) -> Vec<Row> {
        crate::clientlink::list_devices(&self.dir).into_iter().filter(|d| d.active()).map(|d| Row { holds: device_holds(&self.dir, &d.id), id: d.id, name: d.name, by: d.by, last_seen: d.last_seen }).collect()
    }
    fn set(&self, device_id: &str, permission: &str, on: bool) -> Result<(), String> {
        set_permission(&self.dir, device_id, permission, on, &self.user, crate::clock::now_unix()).map(|_| ())
    }
    fn revoke(&self, device_id: &str) -> Result<bool, String> {
        crate::clientlink::revoke_device(&self.dir, device_id, crate::clock::now_unix())
    }
    fn audit(&self, action: &str, detail: &str) {
        (self.audit)(action, detail)
    }
    fn role_of(&self, device_id: &str) -> String {
        super::role_of(&self.dir, device_id).to_string()
    }
    fn set_role(&self, device_id: &str, role: &str) -> Result<(), String> {
        super::apply_role(&self.dir, device_id, role, &self.user, crate::clock::now_unix())
    }
    fn sound_per_person(&self) -> bool {
        crate::clientlink::sound_per_person(&self.dir)
    }
    fn set_sound_per_person(&self, on: bool) -> Result<(), String> {
        crate::clientlink::set_sound_per_person(&self.dir, on)?;
        super::audio_routes().set_per_person(on);
        Ok(())
    }
    fn sources(&self) -> Vec<(String, Vec<String>)> {
        // one tenant per machine unless the cloud profile says otherwise: every tenant's sources are listed for the administrator of this one
        super::sources().users("").into_iter().map(|(s, u)| (s.name, u)).collect()
    }
    fn look(&self, name: &str) -> Result<String, String> {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        let mut wait = || {
            std::thread::sleep(std::time::Duration::from_millis(50));
            std::time::Instant::now() < deadline
        };
        let (src, got) = super::look_at(&super::sources(), name, "", "Chat", &mut wait)?;
        save_look(&self.dir, &src.name, &got)
    }
}

/// Keep a look as a file (0600) in `<data>/client-looks`; only the newest [`KEEP_LOOKS`] stay.
pub const KEEP_LOOKS: usize = 8;

pub fn save_look(dir: &std::path::Path, name: &str, got: &super::Look) -> Result<String, String> {
    use std::io::Write;
    use std::os::unix::fs::OpenOptionsExt;
    let (ext, bytes) = match got {
        super::Look::Picture(b) => ("jpg", b),
        super::Look::Sound(b) => ("wav", b),
    };
    let slug: String = name.chars().map(|c| if c.is_ascii_alphanumeric() { c.to_ascii_lowercase() } else { '-' }).collect();
    let folder = dir.join("client-looks");
    std::fs::create_dir_all(&folder).map_err(|e| format!("could not make the folder: {e}"))?;
    let mut n = crate::clock::now_unix();
    let file = loop {
        let f = folder.join(format!("{slug}-{n}.{ext}"));
        if !f.exists() {
            break f;
        }
        n += 1;
    };
    let mut h = std::fs::OpenOptions::new().write(true).create_new(true).mode(0o600).open(&file).map_err(|e| format!("could not save it: {e}"))?;
    h.write_all(bytes).map_err(|e| format!("could not save it: {e}"))?;
    let mut all: Vec<_> = std::fs::read_dir(&folder).map_err(|e| e.to_string())?.flatten().map(|e| e.path()).collect();
    all.sort();
    // names end in the time: oldest first within one source; keep the newest few overall by modification time
    all.sort_by_key(|p| std::fs::metadata(p).and_then(|m| m.modified()).ok());
    while all.len() > KEEP_LOOKS {
        let _ = std::fs::remove_file(all.remove(0));
    }
    Ok(format!("Took {} from {name}: {} bytes, saved as {}.", if ext == "jpg" { "its newest picture" } else { "the last moment of its sound" }, bytes.len(), file.display()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    struct Nothing;
    impl Tools for Nothing {
        fn defs(&self) -> Vec<ToolDef> {
            Vec::new()
        }
        fn call(&self, name: &str, _: &Value) -> Result<String, String> {
            Err(format!("inner {name}"))
        }
        fn describe(&self, name: &str, _: &Value) -> String {
            format!("inner {name}")
        }
    }

    struct Fake {
        admin: bool,
        rows: Mutex<Vec<Row>>,
        log: Mutex<Vec<String>>,
    }
    impl Backend for Fake {
        fn is_admin(&self) -> bool {
            self.admin
        }
        fn enabled(&self) -> bool {
            true
        }
        fn devices(&self) -> Vec<Row> {
            self.rows.lock().unwrap().clone()
        }
        fn set(&self, id: &str, perm: &str, on: bool) -> Result<(), String> {
            let cap = permission_capability(perm).ok_or("no")?.to_string();
            let mut rows = self.rows.lock().unwrap();
            let r = rows.iter_mut().find(|r| r.id == id).ok_or("no device")?;
            r.holds.retain(|h| *h != cap);
            if on {
                r.holds.push(cap);
            }
            Ok(())
        }
        fn revoke(&self, id: &str) -> Result<bool, String> {
            let mut rows = self.rows.lock().unwrap();
            let n = rows.len();
            rows.retain(|r| r.id != id);
            Ok(rows.len() != n)
        }
        fn audit(&self, a: &str, d: &str) {
            self.log.lock().unwrap().push(format!("{a} {d}"));
        }
    }

    fn tools(admin: bool) -> (DeviceTools, Arc<Fake>) {
        let rows = vec![
            Row { id: "d2".into(), name: "Phone".into(), by: "root".into(), last_seen: 0, holds: vec![] },
            Row { id: "d1".into(), name: "Maya's laptop".into(), by: "root".into(), last_seen: crate::clock::now_unix() - 300, holds: vec!["speaker.play".into()] },
        ];
        let f = Arc::new(Fake { admin, rows: Mutex::new(rows), log: Mutex::new(Vec::new()) });
        (DeviceTools::new(Arc::new(Nothing), f.clone()), f)
    }

    #[test]
    fn the_list_is_free_numbered_and_shows_names_and_permissions_only() {
        let (t, _) = tools(true);
        let defs = t.defs();
        let risk = |n: &str| defs.iter().find(|d| d.spec.name == n).unwrap().risk;
        assert!(matches!(risk(LIST_TOOL), Risk::Safe) && matches!(risk(PERMISSION_TOOL), Risk::Ask) && matches!(risk(REVOKE_TOOL), Risk::Ask));
        let out = t.call(LIST_TOOL, &json!({})).unwrap();
        assert!(out.contains("1. Phone") && out.contains("nothing yet") && out.contains("2. Maya's laptop") && out.contains("last seen 5 min ago") && out.contains("may ask for audio.out"), "{out}");
        assert!(!out.contains("d1") && !out.contains("speaker.play"), "no ids or broker names: {out}");
        assert!(t.call(LIST_TOOL, &json!({})).is_ok() && tools(false).0.call(LIST_TOOL, &json!({})).unwrap().contains("Only an administrator"));
    }

    #[test]
    fn the_sources_list_is_free_names_only_and_says_who_uses_each() {
        struct WithSources(Fake);
        impl Backend for WithSources {
            fn is_admin(&self) -> bool { self.0.is_admin() }
            fn enabled(&self) -> bool { true }
            fn devices(&self) -> Vec<Row> { vec![] }
            fn set(&self, _: &str, _: &str, _: bool) -> Result<(), String> { Ok(()) }
            fn revoke(&self, _: &str) -> Result<bool, String> { Ok(false) }
            fn audit(&self, _: &str, _: &str) {}
            fn sources(&self) -> Vec<(String, Vec<String>)> { vec![("Laptop camera".into(), vec!["Video call".into()]), ("Laptop microphone".into(), vec![])] }
        }
        let t = DeviceTools::new(Arc::new(Nothing), Arc::new(WithSources(Fake { admin: true, rows: Mutex::new(vec![]), log: Mutex::new(vec![]) })));
        assert!(matches!(t.defs().iter().find(|d| d.spec.name == SOURCES_TOOL).unwrap().risk, Risk::Safe));
        let out = t.call(SOURCES_TOOL, &json!({})).unwrap();
        assert!(out.contains("1. Laptop camera (used by Video call)") && out.contains("2. Laptop microphone (nothing uses it; what it sends is dropped)"), "{out}");
        let t2 = DeviceTools::new(Arc::new(Nothing), Arc::new(WithSources(Fake { admin: false, rows: Mutex::new(vec![]), log: Mutex::new(vec![]) })));
        assert!(t2.call(SOURCES_TOOL, &json!({})).unwrap().contains("Only an administrator"));
    }

    #[test]
    fn a_permission_change_names_the_device_by_number_or_name_asks_and_is_audited() {
        let (t, f) = tools(true);
        let args = json!({"device": "maya's LAPTOP", "permission": "client.camera.offer", "on": true});
        assert_eq!(t.describe(PERMISSION_TOOL, &args), "Switch client.camera.offer on for Maya's laptop");
        let pv = t.preview(PERMISSION_TOOL, &args).unwrap();
        assert!(pv.contains("is off now, and would be on") && pv.contains("still ask on the machine at every use"), "{pv}");
        assert!(t.grant_blocker(PERMISSION_TOOL, &args).is_some(), "never covered by a session grant");
        assert!(!t.approved_always(PERMISSION_TOOL));
        let out = t.call(PERMISSION_TOOL, &args).unwrap();
        assert!(out.contains("Maya's laptop may now ask to share the device's camera"), "{out}");
        assert!(f.rows.lock().unwrap()[1].holds.contains(&"client.camera.offer".to_string()));
        // by number, and off
        t.call(PERMISSION_TOOL, &json!({"device": 2, "permission": "audio.out", "on": false})).unwrap();
        assert!(!f.rows.lock().unwrap()[1].holds.contains(&"speaker.play".to_string()));
        let log = f.log.lock().unwrap().join("\n");
        assert!(log.contains("settings.client_permission client.camera.offer on Maya's laptop d1 (chat)") && log.contains("audio.out off"), "{log}");
    }

    #[test]
    fn bad_names_unknown_devices_and_non_administrators_are_refused_before_anything_is_asked() {
        let (t, f) = tools(true);
        assert!(t.refusal(PERMISSION_TOOL, &json!({"device": "9", "permission": "audio.out", "on": true})).unwrap().contains("no device number 9"));
        assert!(t.refusal(PERMISSION_TOOL, &json!({"device": "Phone", "permission": "files.read", "on": true})).unwrap().contains("not something a device can be given"));
        assert!(t.refusal(PERMISSION_TOOL, &json!({"device": "Phone", "permission": "audio.out"})).unwrap().contains("on: true"));
        assert!(t.refusal(PERMISSION_TOOL, &json!({"device": "Phone", "permission": "audio.out", "on": true})).is_none());
        assert!(t.refusal(REVOKE_TOOL, &json!({"device": ""})).is_some());
        let (u, fu) = tools(false);
        assert!(u.refusal(PERMISSION_TOOL, &json!({"device": "Phone", "permission": "audio.out", "on": true})).unwrap().contains("Only an administrator"));
        assert!(u.call(REVOKE_TOOL, &json!({"device": "Phone"})).is_err());
        assert_eq!(fu.rows.lock().unwrap().len(), 2);
        assert!(f.log.lock().unwrap().is_empty() && fu.log.lock().unwrap().is_empty(), "nothing audited when nothing changed");
        // other tools pass through
        assert_eq!(t.call("other", &json!({})), Err("inner other".into()));
    }

    #[test]
    fn a_role_change_asks_never_rides_a_grant_is_refused_for_bad_names_and_is_audited() {
        struct R(Fake, Mutex<String>);
        impl Backend for R {
            fn is_admin(&self) -> bool { self.0.is_admin() }
            fn enabled(&self) -> bool { true }
            fn devices(&self) -> Vec<Row> { self.0.devices() }
            fn set(&self, _: &str, _: &str, _: bool) -> Result<(), String> { Ok(()) }
            fn revoke(&self, _: &str) -> Result<bool, String> { Ok(false) }
            fn audit(&self, a: &str, d: &str) { self.0.audit(a, d) }
            fn role_of(&self, _: &str) -> String { self.1.lock().unwrap().clone() }
            fn set_role(&self, _: &str, role: &str) -> Result<(), String> { *self.1.lock().unwrap() = role.to_string(); Ok(()) }
        }
        let f = Fake { admin: true, rows: Mutex::new(vec![Row { id: "d1".into(), name: "Phone".into(), by: "root".into(), last_seen: 0, holds: vec![] }]), log: Mutex::new(vec![]) };
        let b = Arc::new(R(f, Mutex::new("viewer".into())));
        let t = DeviceTools::new(Arc::new(Nothing), b.clone());
        assert!(matches!(t.defs().iter().find(|d| d.spec.name == ROLE_TOOL).unwrap().risk, Risk::Ask));
        assert!(t.grant_blocker(ROLE_TOOL, &json!({})).is_some() && !t.approved_always(ROLE_TOOL));
        assert!(t.call(LIST_TOOL, &json!({})).unwrap().contains("role viewer"));
        let args = json!({"device": "Phone", "role": "Listener"});
        assert!(t.preview(ROLE_TOOL, &args).unwrap().contains("on the role viewer now, would be on listener"));
        assert!(t.call(ROLE_TOOL, &args).unwrap().contains("Phone is on the role listener"));
        assert_eq!(b.role_of("d1"), "listener");
        assert!(b.0.log.lock().unwrap()[0].contains("settings.client_role listener Phone d1 (chat)"));
        assert!(t.refusal(ROLE_TOOL, &json!({"device": "Phone", "role": "root"})).unwrap().contains("not a role"));
        assert!(t.refusal(ROLE_TOOL, &json!({"device": "Tablet", "role": "full"})).is_some());
        let (u, _) = tools(false);
        assert!(u.call(ROLE_TOOL, &json!({"device": "Phone", "role": "full"})).is_err());
        assert!(tools(true).0.call(ROLE_TOOL, &json!({"device": "Phone", "role": "full"})).unwrap_err().contains("not available"), "a backend without roles says so");
    }

    #[test]
    fn revoking_a_device_asks_and_is_audited() {
        let (t, f) = tools(true);
        assert_eq!(t.describe(REVOKE_TOOL, &json!({"device": "Phone"})), "Revoke the client device Phone");
        assert!(t.grant_blocker(REVOKE_TOOL, &json!({})).is_some());
        assert!(t.call(REVOKE_TOOL, &json!({"device": 1})).unwrap().contains("Phone is revoked"));
        assert_eq!(f.rows.lock().unwrap().len(), 1);
        assert!(f.log.lock().unwrap()[0].contains("settings.client_revoke Phone d2 (chat)"));
    }

    #[test]
    fn the_real_backend_uses_the_same_files_as_the_settings_page() {
        let d = std::env::temp_dir().join(format!("iyakki-devtools-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        let dev = crate::clientlink::enrol_device(&d, "Maya's laptop", &crate::clientlink::DeviceKey::from_seed([4; 32]).public_hex(), "root", 10).unwrap();
        let lines = Arc::new(Mutex::new(Vec::<String>::new()));
        let l2 = lines.clone();
        let t = DeviceTools::new(Arc::new(Nothing), Arc::new(RealBackend { dir: d.clone(), user: "root".into(), admin: true, audit: Arc::new(move |a, x| l2.lock().unwrap().push(format!("{a} {x}"))) }));
        t.call(PERMISSION_TOOL, &json!({"device": "Maya's laptop", "permission": "camera.view", "on": true})).unwrap();
        assert_eq!(device_holds(&d, &dev.id), vec!["camera.view"]);
        assert!(t.call(LIST_TOOL, &json!({})).unwrap().contains("may ask for camera.view"));
        t.call(ROLE_TOOL, &json!({"device": "1", "role": "presenter"})).unwrap();
        assert_eq!(device_holds(&d, &dev.id).len(), 2, "presenter holds the sound and the clipboard; camera.view from before is taken away");
        assert_eq!(super::super::role_of(&d, &dev.id), "presenter");
        assert!(t.call(LIST_TOOL, &json!({})).unwrap().contains("role presenter"));
        t.call(REVOKE_TOOL, &json!({"device": "1"})).unwrap();
        assert!(!crate::clientlink::get_device(&d, &dev.id).unwrap().active());
        assert!(lines.lock().unwrap().iter().any(|l| l.starts_with("settings.client_revoke")));
        let _ = std::fs::remove_dir_all(d);
    }

    #[test]
    fn a_look_asks_every_time_is_for_administrators_and_names_the_saved_file_without_leaking_it_into_the_audit() {
        struct Looks(Fake, Mutex<Vec<String>>);
        impl Backend for Looks {
            fn is_admin(&self) -> bool { self.0.is_admin() }
            fn enabled(&self) -> bool { true }
            fn devices(&self) -> Vec<Row> { vec![] }
            fn set(&self, _: &str, _: &str, _: bool) -> Result<(), String> { Ok(()) }
            fn revoke(&self, _: &str) -> Result<bool, String> { Ok(false) }
            fn audit(&self, a: &str, d: &str) { self.1.lock().unwrap().push(format!("{a} {d}")); }
            fn look(&self, name: &str) -> Result<String, String> { Ok(format!("looked at {name}")) }
        }
        let mk = |admin| Arc::new(Looks(Fake { admin, rows: Mutex::new(vec![]), log: Mutex::new(vec![]) }, Mutex::new(vec![])));
        let b = mk(true);
        let t = DeviceTools::new(Arc::new(Nothing), b.clone());
        assert!(matches!(t.defs().iter().find(|d| d.spec.name == LOOK_TOOL).unwrap().risk, Risk::Ask));
        assert!(t.grant_blocker(LOOK_TOOL, &json!({})).is_some(), "never rides a standing grant");
        assert!(!t.pre_approved(LOOK_TOOL) && !t.approved_always(LOOK_TOOL));
        assert!(t.refusal(LOOK_TOOL, &json!({})).is_some(), "a name is needed");
        assert_eq!(t.call(LOOK_TOOL, &json!({"source": "Laptop camera"})).unwrap(), "looked at Laptop camera");
        assert_eq!(b.1.lock().unwrap().clone(), vec!["client.source_look Laptop camera (chat)".to_string()]);
        let t2 = DeviceTools::new(Arc::new(Nothing), mk(false));
        assert!(t2.refusal(LOOK_TOOL, &json!({"source": "x"})).unwrap().contains("administrator") && t2.call(LOOK_TOOL, &json!({"source": "x"})).is_err());
    }

    #[test]
    fn a_look_is_saved_private_and_only_the_newest_few_are_kept() {
        let d = std::env::temp_dir().join(format!("iyakki-looks-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        for _ in 0..(KEEP_LOOKS + 3) {
            let r = save_look(&d, "Laptop camera", &crate::clientmedia::Look::Picture(vec![0xFF, 0xD8, 1, 0xFF, 0xD9])).unwrap();
            assert!(r.contains("5 bytes") && r.contains("laptop-camera-"), "{r}");
        }
        let files: Vec<_> = std::fs::read_dir(d.join("client-looks")).unwrap().flatten().collect();
        assert_eq!(files.len(), KEEP_LOOKS);
        use std::os::unix::fs::PermissionsExt;
        assert!(files.iter().all(|f| f.metadata().unwrap().permissions().mode() & 0o077 == 0));
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn the_sound_mode_is_shown_in_the_free_list_and_changed_by_a_tool_that_asks_never_rides_a_grant_and_is_audited() {
        struct Snd(Fake, Mutex<bool>);
        impl Backend for Snd {
            fn is_admin(&self) -> bool { self.0.is_admin() }
            fn enabled(&self) -> bool { true }
            fn devices(&self) -> Vec<Row> { vec![] }
            fn set(&self, _: &str, _: &str, _: bool) -> Result<(), String> { Ok(()) }
            fn revoke(&self, _: &str) -> Result<bool, String> { Ok(false) }
            fn audit(&self, a: &str, d: &str) { self.0.audit(a, d) }
            fn sound_per_person(&self) -> bool { *self.1.lock().unwrap() }
            fn set_sound_per_person(&self, on: bool) -> Result<(), String> { *self.1.lock().unwrap() = on; Ok(()) }
        }
        let mk = |admin: bool| Arc::new(Snd(Fake { admin, rows: Mutex::new(vec![]), log: Mutex::new(vec![]) }, Mutex::new(false)));
        let b = mk(true);
        let t = DeviceTools::new(Arc::new(Nothing), b.clone());
        assert!(matches!(t.defs().iter().find(|d| d.spec.name == SOUND_TOOL).unwrap().risk, Risk::Ask));
        assert!(t.call(LIST_TOOL, &json!({})).unwrap().contains("one sound for the machine"));
        let args = json!({"mode": "person"});
        assert_eq!(t.describe(SOUND_TOOL, &args), "Send each person's own sounds to that person's client");
        assert!(t.preview(SOUND_TOOL, &args).unwrap().contains("never hears another's sound") || t.preview(SOUND_TOOL, &args).unwrap().contains("one person never hears"));
        assert!(t.call(SOUND_TOOL, &args).unwrap().contains("Each person's own sounds"));
        assert!(*b.1.lock().unwrap() && t.call(LIST_TOOL, &json!({})).unwrap().contains("each person's own sounds go to that person's client"));
        assert!(b.0.log.lock().unwrap().iter().any(|l| l == "settings.client_sound person (chat)"));
        assert!(t.call(SOUND_TOOL, &json!({"mode": "machine"})).unwrap().contains("one sound for the machine again") && !*b.1.lock().unwrap());
        assert!(t.call(SOUND_TOOL, &json!({"mode": "loud"})).is_err() && t.refusal(SOUND_TOOL, &json!({})).is_some());
        assert!(t.grant_blocker(SOUND_TOOL, &args).is_some() && !t.pre_approved(SOUND_TOOL) && !t.approved_always(SOUND_TOOL), "it asks every time");
        let denied = DeviceTools::new(Arc::new(Nothing), mk(false));
        assert!(denied.call(SOUND_TOOL, &args).is_err() && denied.refusal(SOUND_TOOL, &args).unwrap().contains("administrator"));
    }
}
