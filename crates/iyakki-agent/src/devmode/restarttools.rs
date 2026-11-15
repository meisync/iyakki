//! The lead assistant's tools for the standing restart grant (Settings, Developer mode; every setting is also reachable by chat).
//!
//! * `restart_grant_status` (free): which paired computers may restart or switch off this device without asking, and until when.
//! * `restart_grant` (asks every time, administrators only): tick or untick the box for one paired computer. The grant lasts exactly as
//!   long as the developer connection and covers restart and switch-off only (see [`super::restartgrant`]).

use std::sync::Arc;

use iyakki_sec::policy::Policy;
use serde_json::{json, Value};

use super::DevMode;
use crate::agent::{Risk, ToolDef, Tools};
use crate::chat::ToolSpec;

pub const PROMPT_RULE: &str = "\nRESTART GRANT: restart_grant_status shows which paired computers may restart or switch off this device without asking (developer mode). restart_grant ticks or unticks that for one paired computer; it asks the person first and only an administrator may. The grant lasts exactly as long as the developer connection, covers restart and switch-off only (never commands, root or patching), is audited at every use, and a second restart waits until the device reported a successful start.";

pub const STATUS_TOOL: &str = "restart_grant_status";
pub const SET_TOOL: &str = "restart_grant";

fn is_ours(n: &str) -> bool {
    n == STATUS_TOOL || n == SET_TOOL
}

pub struct RestartTools {
    inner: Arc<dyn Tools>,
    dm: DevMode,
    policy: Policy,
    user: String,
    audit: Arc<dyn Fn(&str, &str) + Send + Sync>,
}

impl RestartTools {
    pub fn new(inner: Arc<dyn Tools>, dm: DevMode, policy: Policy, user: &str, audit: Arc<dyn Fn(&str, &str) + Send + Sync>) -> RestartTools {
        RestartTools { inner, dm, policy, user: user.into(), audit }
    }

    fn args(args: &Value) -> Result<(String, bool), String> {
        let c = args.get("computer").and_then(Value::as_str).unwrap_or("").trim().to_string();
        if c.is_empty() {
            return Err("say which paired computer (its name from restart_grant_status)".into());
        }
        let on = args.get("on").and_then(Value::as_bool).ok_or_else(|| "say on: true or on: false".to_string())?;
        Ok((c, on))
    }

    fn status(&self) -> String {
        if !self.policy.manage_settings {
            return "Only an administrator sees the developer-mode restart grants.".into();
        }
        if !self.dm.is_on() {
            return "Developer mode is off, so no computer may restart this device without asking.".into();
        }
        let mut out = String::new();
        let grants = self.dm.restart_grant_list();
        let st = self.dm.state();
        for k in self.dm.keys() {
            let g = grants.iter().find(|g| g.computer == k.name);
            out.push_str(&format!(
                "{}: {}{}\n",
                k.name,
                match g {
                    Some(g) => format!("may restart or switch off without asking, until {}", if g.until > 0 { super::stamp(g.until) } else { "developer mode is switched off".into() }),
                    None if k.has(super::scopes::Scope::Power) => "asks each time".to_string(),
                    None => "no power scope".to_string(),
                },
                if g.is_some_and(|g| g.awaiting_start) { " (waiting for the device to report a successful start before the next use)" } else { "" }
            ));
        }
        if out.is_empty() {
            return "No computer is paired.".into();
        }
        let _ = st;
        out
    }
}

impl Tools for RestartTools {

    fn inner_tools(&self) -> Option<&dyn Tools> {

        Some(&*self.inner)

    }
    fn defs(&self) -> Vec<ToolDef> {
        let mut v = self.inner.defs();
        v.retain(|d| !is_ours(&d.spec.name));
        v.push(ToolDef { spec: ToolSpec { name: STATUS_TOOL.into(), description: "Show which paired computers may restart or switch off this device without asking (developer mode), and until when. Administrators only.".into(), schema: json!({"type": "object", "properties": {}}) }, risk: Risk::Safe, untrusted: false });
        v.push(ToolDef { spec: ToolSpec { name: SET_TOOL.into(), description: "Allow or stop allowing one paired computer to restart or switch off this device without asking. It lasts exactly as long as the developer connection; restart and switch-off only. The person approves a card first.".into(), schema: json!({"type": "object", "properties": {"computer": {"type": "string", "description": "the paired computer's name"}, "on": {"type": "boolean"}}, "required": ["computer", "on"]}) }, risk: Risk::Ask, untrusted: false });
        v
    }

    fn call(&self, name: &str, args: &Value) -> Result<String, String> {
        match name {
            STATUS_TOOL => Ok(self.status()),
            SET_TOOL => {
                let (c, on) = Self::args(args)?;
                if on {
                    let v = self.dm.restart_grant_on(&self.policy, &self.user, &c)?;
                    (self.audit)("devmode.restart.grant_on", &format!("{c} (chat)"));
                    Ok(format!("{c} may restart or switch off this device without asking until {}. Commands, root and patching still need their own approval.", if v.until > 0 { super::stamp(v.until) } else { "developer mode is switched off".into() }))
                } else {
                    self.dm.restart_grant_off(&self.policy, &self.user, &c)?;
                    (self.audit)("devmode.restart.grant_off", &format!("{c} (chat)"));
                    Ok(format!("{c} must ask for approval again."))
                }
            }
            _ => self.inner.call(name, args),
        }
    }

    fn describe(&self, name: &str, args: &Value) -> String {
        match (name, Self::args(args)) {
            (STATUS_TOOL, _) => "Show the developer-mode restart grants".into(),
            (SET_TOOL, Ok((c, true))) => format!("Let {c} restart or switch off this device without asking"),
            (SET_TOOL, Ok((c, false))) => format!("Make {c} ask before restarting this device"),
            (SET_TOOL, Err(_)) => "Change a developer-mode restart grant".into(),
            _ => self.inner.describe(name, args),
        }
    }

    fn preview(&self, name: &str, args: &Value) -> Option<String> {
        match name {
            SET_TOOL => Some(match Self::args(args) {
                Ok((c, true)) => format!("{c} will be able to restart or switch off this device without a card, also while the sign-in screen shows, until the developer connection ends ({}). It cannot run commands, become root or patch; every use is written to the audit log; after a restart it waits for the device to report a successful start.", { let u = self.dm.state().until; if u > 0 { super::stamp(u) } else { "when developer mode is switched off".into() } }),
                Ok((c, false)) => format!("{c} will have to be approved at the device's screen for each restart again."),
                Err(e) => format!("Cannot change it: {e}"),
            }),
            STATUS_TOOL => None,
            _ => self.inner.preview(name, args),
        }
    }

    fn refusal(&self, name: &str, args: &Value) -> Option<String> {
        match name {
            SET_TOOL if !self.policy.manage_settings => Some("Only an administrator may change a restart grant.".into()),
            SET_TOOL => Self::args(args).err().or_else(|| if !self.dm.is_on() && args.get("on").and_then(Value::as_bool) == Some(true) { Some("Developer mode is off: switch it on first.".into()) } else { None }),
            STATUS_TOOL => None,
            _ => self.inner.refusal(name, args),
        }
    }

    fn pre_approved(&self, name: &str) -> bool {
        !is_ours(name) && self.inner.pre_approved(name)
    }

    fn approved_always(&self, name: &str) -> bool {
        !is_ours(name) && self.inner.approved_always(name)
    }

    fn grant_blocker(&self, name: &str, args: &Value) -> Option<String> {
        if name == SET_TOOL { Some("who may restart the device without asking is the person's security choice, so it asks every time".into()) } else { self.inner.grant_blocker(name, args) }
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

#[cfg(test)]
mod tests {
    use super::*;
    use iyakki_sec::policy::Role;
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

    fn setup(tag: &str, role: Role) -> (RestartTools, DevMode, std::path::PathBuf, Arc<Mutex<Vec<String>>>) {
        let d = std::env::temp_dir().join(format!("iyakki-rgtools-{}-{tag}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(d.join("devmode")).unwrap();
        let dm = DevMode::open(&d);
        dm.enable(&Role::Admin.template(), "root", 0).unwrap();
        let k = super::super::DevKey { name: "laptop".into(), key: format!("{:0>64}", 6), account: "root".into(), added: 1, added_by: "root".into(), last_used: 0, scopes: vec!["install".into(), "power".into()] };
        std::fs::write(d.join("devmode/keys.json"), serde_json::to_string(&vec![k]).unwrap()).unwrap();
        let lines = Arc::new(Mutex::new(Vec::new()));
        let l2 = lines.clone();
        let t = RestartTools::new(Arc::new(Nothing), dm.clone(), role.template(), "root", Arc::new(move |a, x| l2.lock().unwrap().push(format!("{a} {x}"))));
        (t, dm, d, lines)
    }

    #[test]
    fn status_is_free_the_change_asks_every_time_and_is_audited() {
        let (t, dm, d, lines) = setup("a", Role::Admin);
        let defs = t.defs();
        assert!(defs.iter().find(|x| x.spec.name == STATUS_TOOL).is_some_and(|x| matches!(x.risk, Risk::Safe)));
        assert!(defs.iter().find(|x| x.spec.name == SET_TOOL).is_some_and(|x| matches!(x.risk, Risk::Ask)));
        assert!(t.grant_blocker(SET_TOOL, &json!({})).is_some(), "never covered by a session grant");
        assert!(t.call(STATUS_TOOL, &json!({})).unwrap().contains("laptop: asks each time"));
        assert!(t.preview(SET_TOOL, &json!({"computer": "laptop", "on": true})).unwrap().contains("cannot run commands"));
        t.call(SET_TOOL, &json!({"computer": "laptop", "on": true})).unwrap();
        assert!(dm.restart_grant_active("laptop"));
        assert!(t.call(STATUS_TOOL, &json!({})).unwrap().contains("without asking, until developer mode is switched off"));
        t.call(SET_TOOL, &json!({"computer": "laptop", "on": false})).unwrap();
        assert!(!dm.restart_grant_active("laptop"));
        let l = lines.lock().unwrap().clone();
        assert!(l.iter().any(|x| x.starts_with("devmode.restart.grant_on laptop")) && l.iter().any(|x| x.starts_with("devmode.restart.grant_off")));
        assert!(dm.activity(20).iter().any(|a| a.action == "devmode.restart.grant_on"));
        let _ = std::fs::remove_dir_all(d);
    }

    #[test]
    fn non_administrators_unknown_computers_and_bad_input_are_refused_before_anything_is_asked() {
        let (t, dm, d, _) = setup("b", Role::User);
        assert!(t.refusal(SET_TOOL, &json!({"computer": "laptop", "on": true})).unwrap().contains("administrator"));
        assert!(t.call(SET_TOOL, &json!({"computer": "laptop", "on": true})).is_err());
        assert!(!dm.restart_grant_active("laptop"));
        assert!(t.call(STATUS_TOOL, &json!({})).unwrap().contains("administrator"));
        let (t, _, d2, _) = setup("c", Role::Admin);
        assert!(t.refusal(SET_TOOL, &json!({"on": true})).is_some() && t.refusal(SET_TOOL, &json!({"computer": "x"})).is_some());
        assert!(t.call(SET_TOOL, &json!({"computer": "nobody", "on": true})).is_err());
        assert!(t.call("other", &json!({})).unwrap_err().contains("inner"));
        let _ = std::fs::remove_dir_all(d);
        let _ = std::fs::remove_dir_all(d2);
    }
}
