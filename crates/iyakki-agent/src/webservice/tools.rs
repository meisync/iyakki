//! The tool filter of a service run. The model is offered only the tools of the profile (by name, an allow-list), a call to any
//! other name is refused, and approvals stay per call: no earlier desktop grant, "always allow" or session grant counts here.

use super::{tool_hardware, Profile};
use crate::agent::{ToolDef, Tools};
use serde_json::Value;
use std::collections::BTreeSet;
use std::sync::Arc;

/// The names a run may use: the profile's list plus what the administrator enabled, minus every high-risk tool the administrator
/// did not enable by name, minus (cloud profile) anything that needs local hardware; narrowed to `requested` when given.
pub fn allowed_set(profile: Profile, enabled: &[String], requested: &[String]) -> Result<BTreeSet<String>, String> {
    let mut base: BTreeSet<String> = profile.tools().iter().map(|s| s.to_string()).collect();
    base.extend(enabled.iter().cloned());
    base.retain(|t| (crate::runs::approval_risk(t) != "high" || enabled.contains(t)) && !(profile.is_cloud() && tool_hardware(t).is_some()));
    if requested.is_empty() {
        return Ok(base);
    }
    let mut out = BTreeSet::new();
    for r in requested {
        if !base.contains(r) {
            return Err(format!("the tool \"{}\" is not available in this profile", r.chars().take(40).collect::<String>()));
        }
        out.insert(r.clone());
    }
    Ok(out)
}

pub struct ProfileTools {
    inner: Arc<dyn Tools>,
    allowed: BTreeSet<String>,
}

impl ProfileTools {
    pub fn new(inner: Arc<dyn Tools>, allowed: BTreeSet<String>) -> ProfileTools {
        ProfileTools { inner, allowed }
    }
}

impl Tools for ProfileTools {
    fn defs(&self) -> Vec<ToolDef> {
        self.inner.defs().into_iter().filter(|d| self.allowed.contains(&d.spec.name)).collect()
    }
    fn call(&self, name: &str, args: &Value) -> Result<String, String> {
        if !self.allowed.contains(name) {
            return Err("that tool is not available in this service profile".into());
        }
        self.inner.call(name, args)
    }
    fn describe(&self, name: &str, args: &Value) -> String {
        self.inner.describe(name, args)
    }
    fn preview(&self, name: &str, args: &Value) -> Option<String> {
        self.inner.preview(name, args)
    }
    fn refusal(&self, name: &str, args: &Value) -> Option<String> {
        if !self.allowed.contains(name) {
            return Some("that tool is not available in this service profile".into());
        }
        self.inner.refusal(name, args)
    }
    /// Grants the person gave at the desktop do not apply over the API.
    fn pre_approved(&self, _name: &str) -> bool {
        false
    }
    fn approved_always(&self, _name: &str) -> bool {
        false
    }
    fn grant_blocker(&self, _name: &str, _args: &Value) -> Option<String> {
        Some("approvals over the API are answered one call at a time".into())
    }
}
