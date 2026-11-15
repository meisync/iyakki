//! The lead assistant's tools for the log of the last session (F-SYS-05): `last_session_status` reads (no card) whether the last
//! session ended cleanly or unexpectedly, when it started, how long it ran and the last kernel line that was kept; `last_session_log_copy`
//! asks every time and copies the end of the kept kernel log into a new private file in the person's own home folder (the same copy
//! the Settings -> Last session log button makes). The log text itself is never returned to the model: it holds device names and
//! addresses, and the person can open the copied file themselves.
//!
//! * Only an administrator copies (the same people who manage Settings); the status is free for everyone.
//! * Nothing is overwritten: a second copy gets its own name.

use std::path::PathBuf;
use std::sync::Arc;

use serde_json::{json, Value};

use crate::agent::{Risk, ToolDef, Tools};
use crate::chat::ToolSpec;

pub const PROMPT_RULE: &str = "\nLAST SESSION: last_session_status tells whether the last session ended cleanly or unexpectedly (a crash or a power cut), when it started, how long it ran and its last kernel line. last_session_log_copy asks the person, then copies the end of the kept kernel log into a new private file in their home folder; the log text is never shown to you, so tell the person where the file is and let them open it or send it themselves.";

pub const STATUS_TOOL: &str = "last_session_status";
pub const COPY_TOOL: &str = "last_session_log_copy";

pub fn is_session_log_tool(n: &str) -> bool {
    n == STATUS_TOOL || n == COPY_TOOL
}

/// What the status tool says, as plain values (the real backend reads the kept files; tests give them).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Note {
    /// The session that ended unexpectedly: when it started (text), seconds it ran, its last kernel line, whether the person cleared the note.
    pub unclean: Option<(String, u64, String, bool)>,
    /// How the newest finished session ended in the history (`clean`, `clean-with-errors`, `unclean`), if there is one.
    pub last_how: Option<String>,
    /// The kept log a copy would take (a bare file name), if any.
    pub log: Option<String>,
}

/// What the tools need from the machine (a fake in tests).
pub trait Backend: Send + Sync {
    fn note(&self) -> Note;
    /// May this person copy the log (administrators)?
    fn may_copy(&self) -> bool;
    /// Copy the kept log `name` into the person's home folder; the new path.
    fn copy(&self, name: &str) -> Result<PathBuf, String>;
}

pub struct SessionLogTools {
    inner: Arc<dyn Tools>,
    backend: Arc<dyn Backend>,
}

impl SessionLogTools {
    pub fn new(inner: Arc<dyn Tools>, backend: Arc<dyn Backend>) -> SessionLogTools {
        SessionLogTools { inner, backend }
    }

    fn status(&self) -> String {
        let n = self.backend.note();
        let mut out = vec![];
        match &n.unclean {
            Some((started, secs, last, seen)) => {
                out.push(format!("The session that started {started} ended unexpectedly (a crash, a freeze or a power cut); it ran for about {secs} s."));
                out.push(if last.is_empty() { "No kernel line was kept at its end.".to_string() } else { format!("Last kernel line kept: {last}") });
                if *seen {
                    out.push("The person already marked it as seen.".into());
                }
            }
            None => out.push(match n.last_how.as_deref() {
                Some("unclean") => "The last finished session did not end cleanly, and the note about it was cleared.".to_string(),
                Some(_) => "The last session ended cleanly.".to_string(),
                None => "No session history is kept yet.".to_string(),
            }),
        }
        out.push(match &n.log {
            Some(_) => "A kernel log of it is kept; last_session_log_copy copies it into the person's home folder after a yes.".to_string(),
            None => "No kernel log is kept for it.".to_string(),
        });
        out.join("\n")
    }

    fn card(&self) -> Result<String, String> {
        if !self.backend.may_copy() {
            return Err("only an administrator can copy the session log".into());
        }
        if self.backend.note().log.is_none() {
            return Err("there is no kept kernel log to copy".into());
        }
        Ok("Copy the end of the kept kernel log of the last session into a new private file in your home folder. The log can name devices and network addresses of this computer. Nothing is sent anywhere.".into())
    }
}

impl Tools for SessionLogTools {

    fn inner_tools(&self) -> Option<&dyn Tools> {

        Some(&*self.inner)

    }
    fn defs(&self) -> Vec<ToolDef> {
        let mut v = self.inner.defs();
        v.retain(|d| !is_session_log_tool(&d.spec.name));
        v.push(ToolDef { spec: ToolSpec { name: STATUS_TOOL.into(), description: "Say whether the last session of this computer ended cleanly or unexpectedly (a crash or a power cut), when it started, how long it ran and its last kept kernel line. Reads kept files; changes nothing.".into(), schema: json!({"type": "object", "properties": {}}) }, risk: Risk::Safe, untrusted: false });
        v.push(ToolDef { spec: ToolSpec { name: COPY_TOOL.into(), description: "Copy the end of the kept kernel log of the last session into a new private file in the person's home folder, so they can open or send it. The log text is not returned. The person approves a card first.".into(), schema: json!({"type": "object", "properties": {}}) }, risk: Risk::Ask, untrusted: false });
        v
    }

    fn call(&self, name: &str, args: &Value) -> Result<String, String> {
        match name {
            STATUS_TOOL => Ok(self.status()),
            COPY_TOOL => {
                self.card()?;
                let log = self.backend.note().log.ok_or("there is no kept kernel log to copy")?;
                let p = self.backend.copy(&log)?;
                Ok(format!("Copied to {} (private to you). The log text was not read out; open the file or send it yourself.", p.display()))
            }
            _ => self.inner.call(name, args),
        }
    }

    fn describe(&self, name: &str, args: &Value) -> String {
        match name {
            STATUS_TOOL => "Show how the last session ended".into(),
            COPY_TOOL => "Copy the last session's kernel log to your home folder".into(),
            _ => self.inner.describe(name, args),
        }
    }

    fn preview(&self, name: &str, args: &Value) -> Option<String> {
        match name {
            COPY_TOOL => self.card().ok(),
            STATUS_TOOL => None,
            _ => self.inner.preview(name, args),
        }
    }

    fn refusal(&self, name: &str, args: &Value) -> Option<String> {
        match name {
            COPY_TOOL => self.card().err(),
            STATUS_TOOL => None,
            _ => self.inner.refusal(name, args),
        }
    }

    fn pre_approved(&self, name: &str) -> bool {
        !is_session_log_tool(name) && self.inner.pre_approved(name)
    }

    fn approved_always(&self, name: &str) -> bool {
        name != COPY_TOOL && self.inner.approved_always(name)
    }

    fn grant_blocker(&self, name: &str, args: &Value) -> Option<String> {
        if name == COPY_TOOL { Some("a kernel log can name devices and addresses of this computer, so copying it asks every time".into()) } else { self.inner.grant_blocker(name, args) }
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

/// The real backend: the kept log folder on the data volume and the person's home folder.
pub struct RealBackend {
    pub svc: Arc<crate::services::Services>,
    pub policy: iyakki_sec::policy::Policy,
    pub user: String,
    pub audit: Arc<dyn Fn(&str, &str) + Send + Sync>,
}

impl RealBackend {
    fn dir(&self) -> PathBuf {
        self.svc.dir().join(iyakki_storage::crashlog::LOG_SUBDIR)
    }
}

impl Backend for RealBackend {
    fn note(&self) -> Note {
        use iyakki_storage::crashlog as cl;
        let dir = self.dir();
        let u = cl::read_unclean(&dir);
        let log = u.as_ref().map(|u| u.file.clone()).filter(|f| !f.is_empty() && dir.join(f).is_file()).or_else(|| cl::latest_crash(&dir));
        Note { unclean: u.map(|u| (cl::wall(u.started), u.uptime_s, u.last_line, u.seen)).filter(|(_, _, _, seen)| !*seen), last_how: cl::last_finished(&dir).map(|f| f.how), log }
    }
    fn may_copy(&self) -> bool {
        self.policy.manage_settings
    }
    fn copy(&self, name: &str) -> Result<PathBuf, String> {
        let home = crate::scope::home_root_for_writes(&self.user, self.svc.dir());
        let p = iyakki_storage::crashlog::copy_to_folder(&self.dir(), name, &home, crate::clock::now_unix())?;
        (self.audit)("system.unclean-shutdown.copy", &format!("the kept kernel log {name} was copied to a file in the home folder (assistant)"));
        Ok(p)
    }
}
