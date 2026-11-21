//! The lead assistant's tools for the encrypted home folder (F-STORE-05, `iyakki-enchome`).
//!
//! `encrypted_home_status` and `encrypted_home_plan_move` are free (they read and change nothing). Everything else asks the person first and
//! never rides on a session grant: `_create`, `_unlock`, `_new_recovery_key`, `_set_admin_slot` and `_move_files` only leave a one-word
//! request for the shell, which opens the on-screen form (masked fields). `_lock` closes the folder directly after the person approved.
//!
//! The password and the recovery key NEVER pass through the model: no tool has an argument for them, no result or audit line holds them,
//! and the recovery key is shown once on the person's screen, not in the chat.

use std::path::PathBuf;
use std::sync::Arc;

use serde_json::{json, Value};

use crate::agent::{Risk, ToolDef, Tools};
use crate::chat::ToolSpec;
use iyakki_enchome::form::{self, Request};
use iyakki_enchome::{migrate, EncHome, State};
pub use iyakki_shared::enchometools::{PROMPT_RULE};


pub const STATUS: &str = "encrypted_home_status";
pub const CREATE: &str = "encrypted_home_create";
pub const LOCK: &str = "encrypted_home_lock";
pub const UNLOCK: &str = "encrypted_home_unlock";
pub const RECOVERY: &str = "encrypted_home_new_recovery_key";
pub const ADMIN_SLOT: &str = "encrypted_home_set_admin_slot";
pub const PLAN: &str = "encrypted_home_plan_move";
pub const MOVE: &str = "encrypted_home_move_files";
pub const EXPORT: &str = "encrypted_home_export_luks2";

const ALL: [&str; 9] = [STATUS, CREATE, LOCK, UNLOCK, RECOVERY, ADMIN_SLOT, PLAN, MOVE, EXPORT];

pub fn is_enchome_tool(n: &str) -> bool {
    ALL.contains(&n)
}

fn asks(n: &str) -> bool {
    is_enchome_tool(n) && n != STATUS && n != PLAN
}

/// What the tools need from the machine (a fake in tests).
pub trait Backend: Send + Sync {
    /// A guest has no password to encrypt with.
    fn can_encrypt(&self) -> bool;
    fn status(&self) -> String;
    /// The read-only plan for moving the existing files.
    fn plan(&self) -> Result<String, String>;
    fn lock(&self) -> Result<String, String>;
    /// Leave the request for the shell; `Err` when it makes no sense now.
    fn request(&self, r: Request) -> Result<(), String>;
    fn audit(&self, action: &str, detail: &str);
}

pub struct EncHomeTools {
    inner: Arc<dyn Tools>,
    backend: Arc<dyn Backend>,
}

impl EncHomeTools {
    pub fn new(inner: Arc<dyn Tools>, backend: Arc<dyn Backend>) -> EncHomeTools {
        EncHomeTools { inner, backend }
    }

    fn enable(args: &Value) -> Result<bool, String> {
        args.get("enable").and_then(Value::as_bool).ok_or_else(|| "say enable: true (allow an administrator slot) or false (remove it)".to_string())
    }

    fn form_request(&self, r: Request, what: &str, then: &str) -> Result<String, String> {
        if !self.backend.can_encrypt() {
            return Err("This account has no password (a guest), so there is nothing to encrypt with.".into());
        }
        self.backend.request(r)?;
        self.backend.audit(&format!("enchome.{what}.requested"), "form opened on the person's screen");
        Ok(format!("The form is waiting on the person's screen (Settings, Encrypted home). {then} Tell the person to type their password there; you never see it."))
    }
}

fn def(name: &str, desc: &str, risk: Risk, schema: Value) -> ToolDef {
    ToolDef { spec: ToolSpec { name: name.into(), description: desc.into(), schema }, risk, untrusted: false }
}

impl Tools for EncHomeTools {

    fn inner_tools(&self) -> Option<&dyn Tools> {

        Some(&*self.inner)

    }
    fn defs(&self) -> Vec<ToolDef> {
        let mut v = self.inner.defs();
        v.retain(|d| !is_enchome_tool(&d.spec.name));
        let none = json!({"type": "object", "properties": {}});
        v.push(def(STATUS, "Show whether the person's private folder is encrypted, open or locked, what can open it, and whether the recovery key was confirmed.", Risk::Safe, none.clone()));
        v.push(def(CREATE, "Make an encrypted private home folder. The person approves a card, then types their sign-in password into a masked form and sees a recovery key once, which they must type back. You never get the password or the key. Refused when the home already holds files (use encrypted_home_move_files).", Risk::Ask, none.clone()));
        v.push(def(LOCK, "Lock the encrypted home now (it is unmounted and closed). The person approves a card. Apps using it must be closed first.", Risk::Ask, none.clone()));
        v.push(def(UNLOCK, "Open the encrypted home. The person approves a card, then types the password (or the recovery key) into a masked form. You never get it.", Risk::Ask, none.clone()));
        v.push(def(RECOVERY, "Make a new recovery key (the old one stops working). The person approves a card, types their password and sees the key once on the screen.", Risk::Ask, none.clone()));
        v.push(def(ADMIN_SLOT, "Allow (enable true) or remove (enable false) an optional administrator slot. With it an administrator can open the home and reset the password without data loss; without it only the person's password and recovery key can. The person approves a card and types the passwords into a form.", Risk::Ask, json!({"type": "object", "properties": {"enable": {"type": "boolean"}}, "required": ["enable"]})));
        v.push(def(PLAN, "Show what moving the existing home folder into an encrypted one would copy (files, size, free space needed). Changes nothing.", Risk::Safe, none.clone()));
        v.push(def(EXPORT, "Export the LOCKED encrypted home as a standard LUKS2 file (opens with `cryptsetup luksOpen` and the same password on any Linux computer), written to the data folder, never over an existing file. The person approves a card, then types their password into a masked form. You never get the password, the volume key or the file's contents. Refused while the home is open (lock it first).", Risk::Ask, none.clone()));
        v.push(def(MOVE, "Move the existing files into a new encrypted home: copy, check every file with SHA-256, then switch; the old folder is kept until the person agrees to remove it. Opt-in: only when the person asks. The person approves a card, then types their password into a form that shows the plan first.", Risk::Ask, none));
        v
    }

    fn call(&self, name: &str, args: &Value) -> Result<String, String> {
        match name {
            STATUS => Ok(self.backend.status()),
            PLAN => {
                if !self.backend.can_encrypt() {
                    return Err("This account has no password (a guest), so there is nothing to encrypt with.".into());
                }
                self.backend.plan()
            }
            CREATE => self.form_request(Request::Create, "create", "After the person types their sign-in password, a recovery key appears once; they must write it down and type it back before it counts as saved."),
            UNLOCK => self.form_request(Request::Unlock, "unlock", "The person can use the password, or the recovery key if the password no longer works."),
            RECOVERY => self.form_request(Request::ChangeRecovery, "recovery", "The old recovery key stops working as soon as the new one is made."),
            EXPORT => self.form_request(Request::Export, "export", "The file is written once the person has typed the password; it is the home's encrypted data in the standard LUKS2 format."),
            MOVE => self.form_request(Request::Migrate, "move", "The form shows what will be copied first; nothing is touched until the person presses Move."),
            ADMIN_SLOT => {
                let on = Self::enable(args)?;
                self.form_request(if on { Request::AdminSlotOn } else { Request::AdminSlotOff }, "adminslot", if on { "The person also types the administrator's password; whoever knows it can read this home." } else { "Administrators can no longer open the home." })
            }
            LOCK => {
                if !self.backend.can_encrypt() {
                    return Err("This account has no password (a guest), so there is no encrypted home.".into());
                }
                let r = self.backend.lock();
                self.backend.audit("enchome.lock", if r.is_ok() { "closed" } else { "refused" });
                r
            }
            _ => self.inner.call(name, args),
        }
    }

    fn describe(&self, name: &str, args: &Value) -> String {
        match name {
            STATUS => "Show the state of the encrypted home".into(),
            PLAN => "Show what moving the existing files would copy".into(),
            CREATE => "Make an encrypted private home folder (you type your password on the screen)".into(),
            LOCK => "Lock the encrypted home (unmount and close it)".into(),
            UNLOCK => "Open the encrypted home (you type your password or recovery key on the screen)".into(),
            RECOVERY => "Make a new recovery key for the encrypted home (the old one stops working)".into(),
            EXPORT => "Export my locked encrypted home as a standard LUKS2 file (you type your password on the screen)".into(),
            MOVE => "Move my existing files into an encrypted home (copy, check every file, switch; the old folder is kept)".into(),
            ADMIN_SLOT => match Self::enable(args) {
                Ok(true) => "Allow an administrator slot: an administrator password could then open my encrypted home".into(),
                Ok(false) => "Remove the administrator slot from my encrypted home".into(),
                Err(_) => "Change the administrator slot of my encrypted home".into(),
            },
            _ => self.inner.describe(name, args),
        }
    }

    fn preview(&self, name: &str, args: &Value) -> Option<String> {
        match name {
            CREATE => Some("Settings, Encrypted home opens with a masked password box. A recovery key is then shown once; you type it back to confirm.\nAn encrypted folder protects your files if the computer or drive is lost or stolen while signed out. It does not protect a signed-in, unlocked computer. System files stay readable.\nThe assistant never sees the password or the key.".into()),
            EXPORT => Some("A masked password box opens in Settings, Encrypted home. Your whole encrypted home is then copied, as it is (still encrypted), into one standard LUKS2 file in the data folder. The recovery key and the administrator slot are not part of it. The folder must be locked first.".into()),
            UNLOCK => Some("A masked box opens in Settings, Encrypted home. The assistant never sees what you type.".into()),
            RECOVERY => Some("The old recovery key stops working. A new one is shown once on your screen; you type it back to confirm.".into()),
            LOCK => Some("Your private folder is unmounted and closed. Close the apps and files that use it first.".into()),
            MOVE => Some(format!("{}\nNothing is changed until you press Move in the form.", self.backend.plan().unwrap_or_else(|e| format!("Cannot move now: {e}")))),
            ADMIN_SLOT => Some(match Self::enable(args) {
                Ok(true) => "An administrator slot lets someone who knows its password open and READ your home, and reset your sign-in password without losing it. You choose the password in the form. You can remove it at any time.".into(),
                Ok(false) => "Only your password and your recovery key will open the home afterwards.".into(),
                Err(e) => format!("Cannot change it: {e}"),
            }),
            STATUS | PLAN => None,
            _ => self.inner.preview(name, args),
        }
    }

    fn refusal(&self, name: &str, args: &Value) -> Option<String> {
        match name {
            ADMIN_SLOT => Self::enable(args).err(),
            n if is_enchome_tool(n) => None,
            _ => self.inner.refusal(name, args),
        }
    }

    fn pre_approved(&self, name: &str) -> bool {
        !is_enchome_tool(name) && self.inner.pre_approved(name)
    }

    fn approved_always(&self, name: &str) -> bool {
        !asks(name) && self.inner.approved_always(name)
    }

    fn grant_blocker(&self, name: &str, args: &Value) -> Option<String> {
        if asks(name) { Some("changing how the person's private files are protected is their decision every time, so it asks every time".into()) } else { self.inner.grant_blocker(name, args) }
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

pub use iyakki_enchome::describe_status;

/// The real backend.
pub struct RealBackend {
    pub dir: PathBuf,
    pub user: String,
    pub guest: bool,
    pub audit: Arc<dyn Fn(&str, &str) + Send + Sync>,
}

impl Backend for RealBackend {
    fn can_encrypt(&self) -> bool {
        !self.guest
    }
    fn status(&self) -> String {
        let env = EncHome::for_status(&self.dir);
        describe_status(&env.status(&self.user), env.is_open(&self.user)) + " What is NOT protected: a signed-in, unlocked computer; the boot partition; system files, logs and service data (readable until whole-drive encryption exists)."
    }
    fn plan(&self) -> Result<String, String> {
        let env = EncHome::for_status(&self.dir);
        migrate::plan(&env, &self.user).map(|p| p.describe()).map_err(|e| e.to_string())
    }
    fn lock(&self) -> Result<String, String> {
        let env = EncHome::for_status(&self.dir);
        env.lock(&self.user).map(|_| "The encrypted home is locked.".to_string()).map_err(|e| e.to_string())
    }
    fn request(&self, r: Request) -> Result<(), String> {
        let env = EncHome::for_status(&self.dir);
        let st = env.status(&self.user).state;
        match (r, st) {
            (Request::Create | Request::Migrate, State::Locked | State::Open | State::Damaged) => return Err("This account already has an encrypted home.".into()),
            (Request::Unlock | Request::ChangeRecovery | Request::AdminSlotOn | Request::AdminSlotOff | Request::Export, State::NotSetUp) => return Err("There is no encrypted home yet; encrypted_home_create makes one.".into()),
            (Request::Export, State::Open) => return Err("The encrypted home is open; lock it first (encrypted_home_lock), then export.".into()),
            (Request::Unlock, State::Open) => return Err("The encrypted home is already open.".into()),
            _ => {}
        }
        form::leave_request(&self.dir, &self.user, r);
        Ok(())
    }
    fn audit(&self, action: &str, detail: &str) {
        (self.audit)(action, detail)
    }
}
