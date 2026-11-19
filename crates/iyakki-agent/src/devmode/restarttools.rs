//! The lead assistant's tools for the paired computers of developer mode: `devmode_pairings` (free) lists them with their age and last
//! use and what each holds, `devmode_pairing_remove` (asks every time, administrators only) removes one and says what went (see
//! [`super::pairings`]). And the tools for the standing restart grant (Settings, Developer mode; every setting is also reachable by chat).
//!
//! * `restart_grant_status` (free): which paired computers may restart or switch off this device without asking, and until when.
//! * `restart_grant` (asks every time, administrators only; `persist: true` keeps the grant and the developer connection until switched off by hand): tick or untick the box for one paired computer. The grant lasts exactly as
//!   long as the developer connection and covers restart and switch-off only (see [`super::restartgrant`]).

use std::sync::Arc;

use iyakki_sec::policy::Policy;
use serde_json::{json, Value};

use super::DevMode;
use crate::agent::{Risk, ToolDef, Tools};
use crate::chat::ToolSpec;

pub use iyakki_shared::devmode::restarttools::{PAIRING_RULE, PROMPT_RULE};

pub const PAIRINGS_TOOL: &str = "devmode_pairings";
pub const REMOVE_TOOL: &str = "devmode_pairing_remove";


pub const STATUS_TOOL: &str = "restart_grant_status";
pub const SET_TOOL: &str = "restart_grant";

fn is_ours(n: &str) -> bool {
    n == STATUS_TOOL || n == SET_TOOL || n == PAIRINGS_TOOL || n == REMOVE_TOOL || super::trustedtools::is_ours(n)
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

    fn persist(args: &Value) -> bool {
        args.get("persist").and_then(Value::as_bool).unwrap_or(false)
    }

    fn args(args: &Value) -> Result<(String, bool), String> {
        let c = args.get("computer").and_then(Value::as_str).unwrap_or("").trim().to_string();
        if c.is_empty() {
            return Err("say which paired computer (its name from restart_grant_status)".into());
        }
        let on = args.get("on").and_then(Value::as_bool).ok_or_else(|| "say on: true or on: false".to_string())?;
        if Self::persist(args) && !on {
            return Err("persist only goes with on: true (to end a kept grant say on: false without persist)".into());
        }
        Ok((c, on))
    }

    fn computer(args: &Value) -> Result<String, String> {
        let c = args.get("computer").and_then(Value::as_str).unwrap_or("").trim().to_string();
        if c.is_empty() { Err("say which paired computer (its name from devmode_pairings)".into()) } else { Ok(c) }
    }

    fn pairings_text(&self) -> String {
        if !self.policy.manage_settings {
            return "Only an administrator sees the paired computers of developer mode.".into();
        }
        let now = self.dm.now();
        let list = self.dm.pairings();
        if list.is_empty() {
            return "No computer is paired.".into();
        }
        let ends = if self.dm.is_on() { { let u = self.dm.state().until; if u > 0 { format!("developer mode switches off {}", super::stamp(u)) } else { "developer mode stays on until switched off".to_string() } } } else { "developer mode is off".to_string() };
        list.iter()
            .map(|p| {
                let windows = p.sessions.iter().map(|s| format!("{} (open {}; ends at the latest when {ends})", s.what, super::cli_devices::ago(now, s.started).replace(" ago", ""))).collect::<Vec<_>>();
                format!(
                    "{} (acts as {}): paired {}, last used {}; scopes: {}{}{}{}",
                    p.name,
                    p.account,
                    super::cli_devices::ago(now, p.added),
                    super::cli_devices::ago(now, p.last_used),
                    if p.scopes.is_empty() { "none".to_string() } else { p.scopes.join(", ") },
                    if p.api_scopes.is_empty() && p.core_scopes.is_empty() { String::new() } else { format!("; also {}", p.api_scopes.iter().chain(&p.core_scopes).cloned().collect::<Vec<_>>().join(", ")) },
                    p.shell_auto_until.map(|u| format!("; shell approved without asking until {}", super::stamp(u))).unwrap_or_default(),
                    if windows.is_empty() { "; no session open".to_string() } else { format!("; open: {}", windows.join(", ")) }
                )
            })
            .collect::<Vec<_>>()
            .join("\n")
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
                    Some(g) if g.persist => "Restarts allowed until you switch it off (developer mode stays on until you switch it off)".to_string(),
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
        v.push(ToolDef { spec: ToolSpec { name: SET_TOOL.into(), description: "Allow or stop allowing one paired computer to restart or switch off this device without asking. It lasts exactly as long as the developer connection unless persist is true (kept until the person switches it off; developer mode then stays on); restart and switch-off only. The person approves a card first.".into(), schema: json!({"type": "object", "properties": {"computer": {"type": "string", "description": "the paired computer's name"}, "on": {"type": "boolean"}, "persist": {"type": "boolean", "description": "with on: true, keep the grant until the person switches it off by hand: it is not ended by developer mode's automatic switch-off, and developer mode then stays on until switched off (the card says so). Default false."}}, "required": ["computer", "on"]}) }, risk: Risk::Ask, untrusted: false });
        v.push(ToolDef { spec: ToolSpec { name: PAIRINGS_TOOL.into(), description: "List the computers paired with this device (developer mode): name, account, when paired, when last used, the scopes it holds, grants and the shell windows it has open. Administrators only.".into(), schema: json!({"type": "object", "properties": {}}) }, risk: Risk::Safe, untrusted: false });
        v.push(ToolDef { spec: ToolSpec { name: REMOVE_TOOL.into(), description: "Remove one paired computer (its pairing is not needed any more). Its scopes and grants are taken away, its open shell windows end at once and it must pair again with a new code to come back. The person approves a card first.".into(), schema: json!({"type": "object", "properties": {"computer": {"type": "string", "description": "the paired computer's name from devmode_pairings"}}, "required": ["computer"]}) }, risk: Risk::Ask, untrusted: false });
        v.extend(super::trustedtools::defs());
        v
    }

    fn call(&self, name: &str, args: &Value) -> Result<String, String> {
        match name {
            STATUS_TOOL => Ok(self.status()),
            PAIRINGS_TOOL => Ok(self.pairings_text()),
            REMOVE_TOOL => {
                let c = Self::computer(args)?;
                let said = self.dm.remove_pairing(&self.policy, &self.user, &c)?;
                (self.audit)("devmode.pairing.remove", &format!("{c} (chat)"));
                Ok(said)
            }
            SET_TOOL => {
                let (c, on) = Self::args(args)?;
                if on && Self::persist(args) {
                    self.dm.restart_grant_persist(&self.policy, &self.user, &c)?;
                    (self.audit)("devmode.restart.grant_persist", &format!("{c} (chat)"));
                    Ok(format!("Restarts allowed until you switch it off: {c} may restart or switch off this device without asking, and developer mode stays on until you switch it off. Commands, root and patching still need their own approval."))
                } else if on {
                    let v = self.dm.restart_grant_on(&self.policy, &self.user, &c)?;
                    (self.audit)("devmode.restart.grant_on", &format!("{c} (chat)"));
                    Ok(format!("{c} may restart or switch off this device without asking until {}. Commands, root and patching still need their own approval.", if v.until > 0 { super::stamp(v.until) } else { "developer mode is switched off".into() }))
                } else {
                    self.dm.restart_grant_off(&self.policy, &self.user, &c)?;
                    (self.audit)("devmode.restart.grant_off", &format!("{c} (chat)"));
                    Ok(format!("{c} must ask for approval again."))
                }
            }
            n if super::trustedtools::is_ours(n) => super::trustedtools::call(&self.dm, &self.policy, &self.user, &*self.audit, n, args),
            _ => self.inner.call(name, args),
        }
    }

    fn describe(&self, name: &str, args: &Value) -> String {
        match (name, Self::args(args)) {
            (STATUS_TOOL, _) => "Show the developer-mode restart grants".into(),
            (PAIRINGS_TOOL, _) => "List the paired computers of developer mode".into(),
            (REMOVE_TOOL, _) => format!("Remove the paired computer {}", Self::computer(args).unwrap_or_else(|_| "(unnamed)".into())),
            (SET_TOOL, Ok((c, true))) if Self::persist(args) => format!("Let {c} restart or switch off this device without asking until I switch it off"),
            (SET_TOOL, Ok((c, true))) => format!("Let {c} restart or switch off this device without asking"),
            (SET_TOOL, Ok((c, false))) => format!("Make {c} ask before restarting this device"),
            (SET_TOOL, Err(_)) => "Change a developer-mode restart grant".into(),
            (n, _) if super::trustedtools::is_ours(n) => super::trustedtools::describe(n, args),
            _ => self.inner.describe(name, args),
        }
    }

    fn preview(&self, name: &str, args: &Value) -> Option<String> {
        match name {
            SET_TOOL => Some(match Self::args(args) {
                Ok((c, true)) if Self::persist(args) => format!("{c} will be able to restart or switch off this device without a card, also while the sign-in screen shows, until YOU switch it off: developer mode's automatic switch-off no longer ends it and developer mode will stay on until you switch it off by hand (untick, disconnect or remove the computer, or switch developer mode off). It cannot run commands, become root or patch; every use is written to the activity log."),
                Ok((c, true)) => format!("{c} will be able to restart or switch off this device without a card, also while the sign-in screen shows, until the developer connection ends ({}). It cannot run commands, become root or patch; every use is written to the audit log; after a restart it waits for the device to report a successful start.", { let u = self.dm.state().until; if u > 0 { super::stamp(u) } else { "when developer mode is switched off".into() } }),
                Ok((c, false)) => format!("{c} will have to be approved at the device's screen for each restart again."),
                Err(e) => format!("Cannot change it: {e}"),
            }),
            REMOVE_TOOL => Some(match Self::computer(args) {
                Ok(c) => self.dm.paired_computer(&c).map(|p| p.removal_text()).unwrap_or_else(|| format!("No computer called {c} is paired.")),
                Err(e) => format!("Cannot remove it: {e}"),
            }),
            n if super::trustedtools::is_ours(n) => Some(super::trustedtools::preview(&self.dm, n, args)),
            STATUS_TOOL | PAIRINGS_TOOL => None,
            _ => self.inner.preview(name, args),
        }
    }

    fn refusal(&self, name: &str, args: &Value) -> Option<String> {
        match name {
            SET_TOOL if !self.policy.manage_settings => Some("Only an administrator may change a restart grant.".into()),
            SET_TOOL => Self::args(args).err().or_else(|| if !self.dm.is_on() && args.get("on").and_then(Value::as_bool) == Some(true) { Some("Developer mode is off: switch it on first.".into()) } else { None }),
            REMOVE_TOOL if !self.policy.manage_settings => Some("Only an administrator may remove a paired computer.".into()),
            REMOVE_TOOL => Self::computer(args).err().or_else(|| if self.dm.paired_computer(&Self::computer(args).unwrap_or_default()).is_none() { Some("No such paired computer: see devmode_pairings.".into()) } else { None }),
            n if super::trustedtools::is_ours(n) => super::trustedtools::refusal(&self.dm, &self.policy, n, args),
            STATUS_TOOL | PAIRINGS_TOOL => None,
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
        if name == REMOVE_TOOL { Some("removing a paired computer ends its access and open windows, so it asks every time".into()) } else if matches!(name, super::trustedtools::TRUSTED_SET | super::trustedtools::PATCH_SET) { Some(super::trustedtools::BLOCKER.into()) } else if name == SET_TOOL { Some("who may restart the device without asking is the person's security choice, so it asks every time".into()) } else { self.inner.grant_blocker(name, args) }
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

    #[test]
    fn the_pairings_are_listed_free_and_one_can_be_removed_after_a_question_that_says_what_goes() {
        let (t, dm, d, lines) = setup("p", Role::Admin);
        let defs = t.defs();
        assert!(defs.iter().find(|x| x.spec.name == PAIRINGS_TOOL).is_some_and(|x| matches!(x.risk, Risk::Safe)));
        assert!(defs.iter().find(|x| x.spec.name == REMOVE_TOOL).is_some_and(|x| matches!(x.risk, Risk::Ask)));
        assert!(t.grant_blocker(REMOVE_TOOL, &json!({})).is_some(), "asks every time");
        let list = t.call(PAIRINGS_TOOL, &json!({})).unwrap();
        assert!(list.contains("laptop (acts as root): paired") && list.contains("scopes: install, power") && list.contains("no session open"), "{list}");
        let q = t.preview(REMOVE_TOOL, &json!({"computer": "laptop"})).unwrap();
        assert!(q.contains("takes away") && q.contains("pair again"), "{q}");
        assert!(t.refusal(REMOVE_TOOL, &json!({"computer": "ghost"})).is_some() && t.refusal(REMOVE_TOOL, &json!({})).is_some());
        assert!(t.refusal(REMOVE_TOOL, &json!({"computer": "laptop"})).is_none());
        let said = t.call(REMOVE_TOOL, &json!({"computer": "laptop"})).unwrap();
        assert!(said.starts_with("Removed laptop"), "{said}");
        assert!(dm.keys().is_empty() && t.call(PAIRINGS_TOOL, &json!({})).unwrap().contains("No computer is paired"));
        assert!(lines.lock().unwrap().iter().any(|l| l.starts_with("devmode.pairing.remove laptop")));
        let _ = std::fs::remove_dir_all(d);
        let (t, dm, d, _) = setup("q", Role::User);
        assert!(t.refusal(REMOVE_TOOL, &json!({"computer": "laptop"})).unwrap().contains("administrator"));
        assert!(t.call(REMOVE_TOOL, &json!({"computer": "laptop"})).is_err() && !dm.keys().is_empty());
        assert!(t.call(PAIRINGS_TOOL, &json!({})).unwrap().contains("administrator"));
        let _ = std::fs::remove_dir_all(d);
    }

    #[test]
    fn persist_is_a_hard_ask_says_what_it_keeps_and_ends_by_hand() {
        let (t, dm, d, lines) = setup("persist", Role::Admin);
        let defs = t.defs();
        let set = defs.iter().find(|x| x.spec.name == SET_TOOL).unwrap();
        assert!(set.spec.schema["properties"]["persist"].is_object() && matches!(set.risk, Risk::Ask));
        assert!(t.grant_blocker(SET_TOOL, &json!({"computer": "laptop", "on": true, "persist": true})).is_some(), "never covered by a session grant");
        let a = json!({"computer": "laptop", "on": true, "persist": true});
        let q = t.preview(SET_TOOL, &a).unwrap();
        assert!(q.contains("stay on until you switch it off") && q.contains("cannot run commands"), "{q}");
        assert!(t.describe(SET_TOOL, &a).contains("until I switch it off"));
        assert!(t.refusal(SET_TOOL, &json!({"computer": "laptop", "on": false, "persist": true})).is_some());
        t.call(SET_TOOL, &a).unwrap();
        assert!(dm.restart_grant_list()[0].persist && dm.state().until == 0);
        assert!(t.call(STATUS_TOOL, &json!({})).unwrap().contains("Restarts allowed until you switch it off"));
        assert!(lines.lock().unwrap().iter().any(|l| l.starts_with("devmode.restart.grant_persist laptop")));
        t.call(SET_TOOL, &json!({"computer": "laptop", "on": false})).unwrap();
        assert!(!dm.restart_grant_active("laptop") && dm.state().until > 0);
        let (t, dm, d2, _) = setup("persist-user", Role::User);
        assert!(t.refusal(SET_TOOL, &json!({"computer": "laptop", "on": true, "persist": true})).unwrap().contains("administrator"));
        assert!(t.call(SET_TOOL, &json!({"computer": "laptop", "on": true, "persist": true})).is_err() && !dm.restart_grant_active("laptop"));
        let _ = std::fs::remove_dir_all(d);
        let _ = std::fs::remove_dir_all(d2);
    }

    #[test]
    fn trusted_tools_are_listed_free_or_hard_ask_and_say_what_they_cover() {
        use super::super::trustedtools::*;
        let (t, dm, d, lines) = setup("trusted", Role::Admin);
        let defs = t.defs();
        for n in [TRUSTED_STATUS, PATCH_STATUS] {
            assert!(defs.iter().find(|x| x.spec.name == n).is_some_and(|x| matches!(x.risk, Risk::Safe)), "{n} is free");
        }
        for n in [TRUSTED_SET, PATCH_SET] {
            assert!(defs.iter().find(|x| x.spec.name == n).is_some_and(|x| matches!(x.risk, Risk::Ask)), "{n} asks");
            assert!(t.grant_blocker(n, &json!({})).is_some(), "{n}: never covered by a session grant");
            assert!(crate::agent::hard_ask(n, &json!({"computer": "laptop", "on": true})).is_some(), "{n}: Auto mode asks too");
            assert!(crate::agent::hard_ask(n, &json!({"computer": "laptop", "on": false})).is_none(), "switching off stays easy");
        }
        let q = t.preview(TRUSTED_SET, &json!({"computer": "laptop", "on": true})).unwrap();
        assert!(q.contains("DEV AND TEST") && q.contains("never covers passwords") && q.contains("until YOU switch it off"), "{q}");
        assert!(t.refusal(TRUSTED_SET, &json!({"computer": "laptop", "on": true, "hours": 5})).is_some());
        assert!(t.call(TRUSTED_STATUS, &json!({})).unwrap().contains("laptop: asks each time"));
        t.call(TRUSTED_SET, &json!({"computer": "laptop", "on": true, "hours": 4})).unwrap();
        assert!(dm.trusted_active("laptop") && t.call(TRUSTED_STATUS, &json!({})).unwrap().contains("every approval answered yes"));
        t.call(PATCH_SET, &json!({"computer": "laptop", "on": true})).unwrap();
        assert!(t.call(PATCH_STATUS, &json!({})).unwrap().contains("patches without asking"));
        t.call(PATCH_SET, &json!({"computer": "laptop", "on": false})).unwrap();
        assert!(!dm.trusted_active("laptop"));
        assert!(lines.lock().unwrap().iter().any(|l| l.starts_with("devmode.trusted.on laptop")) && dm.activity(20).iter().any(|a| a.action == "devmode.trusted.on"));
        let (t, dm, d2, _) = setup("trusted-user", Role::User);
        assert!(t.refusal(TRUSTED_SET, &json!({"computer": "laptop", "on": true})).unwrap().contains("administrator"));
        assert!(t.call(TRUSTED_SET, &json!({"computer": "laptop", "on": true})).is_err() && !dm.trusted_active("laptop"));
        let _ = std::fs::remove_dir_all(d);
        let _ = std::fs::remove_dir_all(d2);
    }
}
