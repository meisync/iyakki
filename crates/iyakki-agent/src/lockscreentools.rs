//! The lead assistant's tools for the lock screen: `lock_screen` locks now and asks nothing (locking only protects the person: apps and
//! runs go on, the personal vault closes until the password is typed), `lock_screen_status` reads the automatic lock, and
//! `lock_screen_set_auto` changes after how many idle minutes the screen locks itself and always asks (it decides how long the computer
//! stays open to anyone at the keyboard).
//!
//! The assistant can lock but never unlock: only the password, typed at the lock screen, opens it again.

use std::path::PathBuf;
use std::sync::Arc;

use serde_json::{json, Value};

use crate::agent::{Risk, ToolDef, Tools};
use crate::chat::ToolSpec;
use crate::lockscreen::{self, LockConfig, AUTO_CHOICES};

pub const PROMPT_RULE: &str = "\nLOCK SCREEN: lock_screen locks the person's screen at once (it asks nothing; their apps and runs keep going, their personal vault closes until they type their password at the lock screen). You cannot unlock it and must never ask for the password. lock_screen_status shows after how many idle minutes the screen locks by itself. lock_screen_set_auto changes that (0, 5, 10, 15, 30 or 60 minutes; 0 = never) and asks the person first. Guests have no password, so for them the screen cannot be locked.";

pub const LOCK_TOOL: &str = "lock_screen";
pub const STATUS_TOOL: &str = "lock_screen_status";
pub const SET_TOOL: &str = "lock_screen_set_auto";

pub fn is_lock_tool(n: &str) -> bool {
    n == LOCK_TOOL || n == STATUS_TOOL || n == SET_TOOL
}

/// What the tools need from the machine (a fake in tests).
pub trait Backend: Send + Sync {
    /// Whether this person has a password to lock with (a guest has none).
    fn can_lock(&self) -> bool;
    fn config(&self) -> LockConfig;
    fn save(&self, c: &LockConfig) -> Result<(), String>;
    /// Ask the shell showing this person to lock now (it does so within a second).
    fn request_lock(&self);
    /// One audit line.
    fn audit(&self, action: &str, detail: &str);
}

pub struct LockTools {
    inner: Arc<dyn Tools>,
    backend: Arc<dyn Backend>,
}

impl LockTools {
    pub fn new(inner: Arc<dyn Tools>, backend: Arc<dyn Backend>) -> LockTools {
        LockTools { inner, backend }
    }

    fn minutes(args: &Value) -> Result<u32, String> {
        let v = args.get("minutes").and_then(Value::as_u64).ok_or_else(|| "say how many minutes: 0, 5, 10, 15, 30 or 60".to_string())?;
        u32::try_from(v).ok().filter(|m| AUTO_CHOICES.contains(m)).ok_or_else(|| format!("{v} is not a choice; use 0 (never), 5, 10, 15, 30 or 60"))
    }

    fn status(&self) -> String {
        let c = self.backend.config();
        let lock = if self.backend.can_lock() { "The screen can be locked (Super+L, the Power menu, or lock_screen)." } else { "This account has no password (a guest), so the screen cannot be locked." };
        format!("Automatic lock: {}. {lock} The screen always locks when the computer goes to sleep.", c.describe())
    }
}

impl Tools for LockTools {
    fn defs(&self) -> Vec<ToolDef> {
        let mut v = self.inner.defs();
        v.retain(|d| !is_lock_tool(&d.spec.name));
        v.push(ToolDef { spec: ToolSpec { name: LOCK_TOOL.into(), description: "Lock the person's screen now. Their apps and runs keep going; the personal vault closes and the lock screen asks for their password. You cannot unlock it.".into(), schema: json!({"type": "object", "properties": {}}) }, risk: Risk::Safe, untrusted: false });
        v.push(ToolDef { spec: ToolSpec { name: STATUS_TOOL.into(), description: "Show after how many idle minutes the screen locks by itself (never by default).".into(), schema: json!({"type": "object", "properties": {}}) }, risk: Risk::Safe, untrusted: false });
        v.push(ToolDef { spec: ToolSpec { name: SET_TOOL.into(), description: "Change after how many idle minutes the screen locks by itself: 0 (never), 5, 10, 15, 30 or 60. The person approves a card first. Media playing and a waiting approval hold the lock off.".into(), schema: json!({"type": "object", "properties": {"minutes": {"type": "integer", "enum": AUTO_CHOICES}}, "required": ["minutes"]}) }, risk: Risk::Ask, untrusted: false });
        v
    }

    fn call(&self, name: &str, args: &Value) -> Result<String, String> {
        match name {
            LOCK_TOOL => {
                if !self.backend.can_lock() {
                    return Err("This account has no password (a guest), so the screen cannot be locked. Signing out is on the Power menu.".into());
                }
                self.backend.request_lock();
                Ok("The screen is locking now. Apps and runs keep going; the person's password opens it again.".into())
            }
            STATUS_TOOL => Ok(self.status()),
            SET_TOOL => {
                if !self.backend.can_lock() {
                    return Err("This account has no password (a guest), so there is nothing to lock with.".into());
                }
                let c = LockConfig { auto_min: Self::minutes(args)? };
                self.backend.save(&c)?;
                self.backend.audit("settings.lockscreen", &format!("automatic lock: {}", c.describe()));
                Ok(format!("Automatic lock is now: {}.", c.describe()))
            }
            _ => self.inner.call(name, args),
        }
    }

    fn describe(&self, name: &str, args: &Value) -> String {
        match name {
            LOCK_TOOL => "Lock the screen".into(),
            STATUS_TOOL => "Show the automatic lock".into(),
            SET_TOOL => match Self::minutes(args) {
                Ok(0) => "Turn the automatic screen lock off".into(),
                Ok(m) => format!("Lock the screen by itself after {m} minutes without input"),
                Err(_) => "Change the automatic screen lock".into(),
            },
            _ => self.inner.describe(name, args),
        }
    }

    fn preview(&self, name: &str, args: &Value) -> Option<String> {
        match name {
            SET_TOOL => Some(match Self::minutes(args) {
                Ok(m) => format!("Now: {}.\nAfter: {}.\nLocking keeps apps and runs going and closes the personal vault until the password is typed.", self.backend.config().describe(), LockConfig { auto_min: m }.describe()),
                Err(e) => format!("Cannot change it: {e}"),
            }),
            LOCK_TOOL | STATUS_TOOL => None,
            _ => self.inner.preview(name, args),
        }
    }

    fn refusal(&self, name: &str, args: &Value) -> Option<String> {
        match name {
            SET_TOOL => Self::minutes(args).err(),
            LOCK_TOOL | STATUS_TOOL => None,
            _ => self.inner.refusal(name, args),
        }
    }

    fn pre_approved(&self, name: &str) -> bool {
        !is_lock_tool(name) && self.inner.pre_approved(name)
    }

    fn approved_always(&self, name: &str) -> bool {
        name != SET_TOOL && self.inner.approved_always(name)
    }

    fn grant_blocker(&self, name: &str, args: &Value) -> Option<String> {
        if name == SET_TOOL { Some("how long the computer stays open to anyone at the keyboard is the person's security choice, so it asks every time".into()) } else { self.inner.grant_blocker(name, args) }
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
    /// A guest has no password.
    pub guest: bool,
    pub audit: Arc<dyn Fn(&str, &str) + Send + Sync>,
}

impl Backend for RealBackend {
    fn can_lock(&self) -> bool {
        !self.guest
    }
    fn config(&self) -> LockConfig {
        LockConfig::load(&self.dir, &self.user)
    }
    fn save(&self, c: &LockConfig) -> Result<(), String> {
        c.save(&self.dir, &self.user)
    }
    fn request_lock(&self) {
        lockscreen::request(&self.dir, &self.user);
    }
    fn audit(&self, action: &str, detail: &str) {
        (self.audit)(action, detail)
    }
}
