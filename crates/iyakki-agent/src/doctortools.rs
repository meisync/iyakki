//! The lead assistant's tool for the DOCTOR report (F-DOCTOR-03, chat reach for F-DOCTOR-02): `doctor_report` reads what the DOCTOR tile
//! shows (the same findings, including the running system's own state: dev cluster, vault, VPN) and changes nothing. Free to call (a read
//! of the machine's own state, nothing leaves it); the detail lines are for administrators, like the tile. Fixing a finding is the
//! person's click in the DOCTOR tile or another tool that asks.

use std::sync::Arc;

use serde_json::{json, Value};

use crate::agent::{Risk, ToolDef, Tools};
use crate::chat::ToolSpec;
pub use iyakki_shared::doctortools::{PROMPT_RULE};

pub const READ_TOOL: &str = "doctor_report";


pub trait Backend: Send + Sync {
    /// May this person see the report at all (the DOCTOR tile's rule)?
    fn may_read(&self) -> bool;
    /// Is the person an administrator (the detail lines)?
    fn admin(&self) -> bool;
    /// The report text and whether anything needs attention.
    fn report(&self, detail: bool) -> (String, i32);
}

pub struct DoctorTools {
    inner: Arc<dyn Tools>,
    backend: Arc<dyn Backend>,
}

impl DoctorTools {
    pub fn new(inner: Arc<dyn Tools>, backend: Arc<dyn Backend>) -> DoctorTools {
        DoctorTools { inner, backend }
    }

    fn read(&self, args: &Value) -> Result<String, String> {
        if !self.backend.may_read() {
            return Err("this account may not open DOCTOR".into());
        }
        let detail = args["detail"].as_bool().unwrap_or(false);
        if detail && !self.backend.admin() {
            return Err("the detail lines are for administrator accounts; ask without detail".into());
        }
        let (text, code) = self.backend.report(detail);
        Ok(format!("{}{}", if code == 0 { "Nothing needs attention.\n" } else { "Something needs attention.\n" }, text))
    }
}

impl Tools for DoctorTools {

    fn inner_tools(&self) -> Option<&dyn Tools> {

        Some(&*self.inner)

    }
    fn defs(&self) -> Vec<ToolDef> {
        let mut v = self.inner.defs();
        v.retain(|d| d.spec.name != READ_TOOL);
        v.push(ToolDef { spec: ToolSpec { name: READ_TOOL.into(), description: "Show the DOCTOR report of this computer: findings in plain sentences (memory, disk, clock, network, sound, camera, firewall, dev cluster, vault, VPN, failed services). Reads only. detail=true adds the technical detail (administrators).".into(), schema: json!({"type": "object", "properties": {"detail": {"type": "boolean"}}}) }, risk: Risk::Safe, untrusted: false });
        v
    }

    fn call(&self, name: &str, args: &Value) -> Result<String, String> {
        if name == READ_TOOL { self.read(args) } else { self.inner.call(name, args) }
    }

    fn describe(&self, name: &str, args: &Value) -> String {
        if name == READ_TOOL { "Show the DOCTOR report".into() } else { self.inner.describe(name, args) }
    }

    fn preview(&self, name: &str, args: &Value) -> Option<String> {
        if name == READ_TOOL { None } else { self.inner.preview(name, args) }
    }

    fn refusal(&self, name: &str, args: &Value) -> Option<String> {
        if name == READ_TOOL { self.read_refusal(args) } else { self.inner.refusal(name, args) }
    }

    fn pre_approved(&self, name: &str) -> bool {
        name != READ_TOOL && self.inner.pre_approved(name)
    }

    fn approved_always(&self, name: &str) -> bool {
        name != READ_TOOL && self.inner.approved_always(name)
    }

    fn grant_blocker(&self, name: &str, args: &Value) -> Option<String> {
        self.inner.grant_blocker(name, args)
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

impl DoctorTools {
    fn read_refusal(&self, args: &Value) -> Option<String> {
        if !self.backend.may_read() {
            return Some("this account may not open DOCTOR".into());
        }
        (args["detail"].as_bool() == Some(true) && !self.backend.admin()).then(|| "the detail lines are for administrator accounts; ask without detail".to_string())
    }
}

/// The real backend: the running system's own state through the services.
pub struct RealBackend {
    pub svc: Arc<crate::services::Services>,
    pub policy: iyakki_sec::policy::Policy,
    pub user: String,
    pub audit_recent: crate::doctor_gather::AuditRecent,
}

impl Backend for RealBackend {
    fn may_read(&self) -> bool {
        self.policy.allows_app(iyakki_sec::policy::App::Doctor)
    }
    fn admin(&self) -> bool {
        self.policy.manage_settings
    }
    fn report(&self, detail: bool) -> (String, i32) {
        let slow: crate::doctor_gather::SlowProbe = Arc::new(crate::doctor_gather::slow_probe);
        crate::doctor_gather::live_report(&self.svc, &self.user, detail, self.audit_recent.clone(), &slow)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    struct Fake {
        admin: bool,
        may: bool,
        asked: Mutex<Vec<bool>>,
        code: i32,
    }
    impl Backend for Fake {
        fn may_read(&self) -> bool {
            self.may
        }
        fn admin(&self) -> bool {
            self.admin
        }
        fn report(&self, detail: bool) -> (String, i32) {
            self.asked.lock().unwrap().push(detail);
            ("The vault is locked.\n".into(), self.code)
        }
    }

    struct Empty;
    impl Tools for Empty {
        fn defs(&self) -> Vec<ToolDef> {
            vec![]
        }
        fn call(&self, name: &str, _: &Value) -> Result<String, String> {
            Err(format!("inner got {name}"))
        }
        fn describe(&self, name: &str, _: &Value) -> String {
            name.into()
        }
    }

    fn rig(admin: bool, may: bool, code: i32) -> (DoctorTools, Arc<Fake>) {
        let f = Arc::new(Fake { admin, may, asked: Mutex::new(vec![]), code });
        (DoctorTools::new(Arc::new(Empty), f.clone()), f)
    }

    #[test]
    fn the_tool_is_free_reads_and_says_whether_anything_needs_attention() {
        let (t, f) = rig(false, true, 1);
        let d = t.defs();
        assert_eq!(d.len(), 1);
        assert!(matches!(d[0].risk, Risk::Safe));
        assert!(t.preview(READ_TOOL, &json!({})).is_none() && t.refusal(READ_TOOL, &json!({})).is_none());
        let out = t.call(READ_TOOL, &json!({})).unwrap();
        assert!(out.starts_with("Something needs attention.") && out.contains("The vault is locked."), "{out}");
        assert_eq!(*f.asked.lock().unwrap(), vec![false]);
        let (t, _) = rig(false, true, 0);
        assert!(t.call(READ_TOOL, &json!({})).unwrap().starts_with("Nothing needs attention."));
    }

    #[test]
    fn detail_is_for_administrators_and_an_account_without_the_tile_gets_nothing() {
        let (t, f) = rig(false, true, 0);
        assert!(t.call(READ_TOOL, &json!({"detail": true})).unwrap_err().contains("administrator"));
        assert!(t.refusal(READ_TOOL, &json!({"detail": true})).is_some());
        assert!(f.asked.lock().unwrap().is_empty(), "nothing was read for a refused ask");
        let (t, f) = rig(true, true, 0);
        t.call(READ_TOOL, &json!({"detail": true})).unwrap();
        assert_eq!(*f.asked.lock().unwrap(), vec![true]);
        let (t, f) = rig(true, false, 0);
        assert!(t.call(READ_TOOL, &json!({})).is_err() && t.refusal(READ_TOOL, &json!({})).is_some());
        assert!(f.asked.lock().unwrap().is_empty());
    }

    #[test]
    fn other_tools_pass_to_the_inner_set_and_the_name_is_never_pre_approved() {
        let (t, _) = rig(true, true, 0);
        assert_eq!(t.call("other", &json!({})).unwrap_err(), "inner got other");
        assert!(!t.pre_approved(READ_TOOL) && !t.approved_always(READ_TOOL));
    }
}
