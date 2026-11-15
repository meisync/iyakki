//! The named agent profile of FLOWS assist (F-FLOW-34): which models the fixing agent may use, in which order, with which
//! time limit each, and whether the next one is tried when one fails. Plain data and logic; the host does the asking.

use serde::{Deserialize, Serialize};

use crate::host::{HealProposal, HealRequest, Host};
use crate::Cancel;

/// The longest any one model gets, whatever the profile says.
pub const MAX_TIMEOUT_SECS: u64 = 1800;
/// The shortest limit worth setting.
pub const MIN_TIMEOUT_SECS: u64 = 10;
/// A profile lists at most this many models.
pub const MAX_MODELS: usize = 6;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct ProfileModel {
    pub name: String,
    /// Runs on this computer or this network (no cost, nothing leaves).
    pub local: bool,
    pub timeout_secs: u64,
}

impl Default for ProfileModel {
    fn default() -> ProfileModel {
        ProfileModel { name: String::new(), local: false, timeout_secs: 120 }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct AgentProfile {
    pub name: String,
    pub models: Vec<ProfileModel>,
    /// Ask the local models before the others.
    pub local_first: bool,
    /// When a model fails or runs out of time, ask the next one.
    pub fallback: bool,
}

impl Default for AgentProfile {
    fn default() -> AgentProfile {
        AgentProfile { name: "default".into(), models: vec![], local_first: true, fallback: true }
    }
}

impl AgentProfile {
    /// The models in the order they are asked: local first when set (the list order is kept inside each group); with
    /// no fallback only the first one counts. An empty list means one try with the assistant's own model.
    pub fn order(&self) -> Vec<ProfileModel> {
        let mut v: Vec<ProfileModel> = if self.local_first {
            self.models.iter().filter(|m| m.local).chain(self.models.iter().filter(|m| !m.local)).cloned().collect()
        } else {
            self.models.clone()
        };
        if v.is_empty() {
            v.push(ProfileModel { name: String::new(), local: false, timeout_secs: 120 });
        }
        if !self.fallback {
            v.truncate(1);
        }
        v
    }

    /// Clean a profile the person or the assistant wrote: bounded names and limits, no duplicates.
    pub fn cleaned(mut self) -> Result<AgentProfile, String> {
        self.name = self.name.trim().chars().filter(|c| !c.is_control()).take(40).collect();
        if self.name.is_empty() {
            self.name = "default".into();
        }
        if self.models.len() > MAX_MODELS {
            return Err(format!("a profile lists at most {MAX_MODELS} models"));
        }
        let mut seen: Vec<String> = vec![];
        for m in &mut self.models {
            m.name = m.name.trim().chars().filter(|c| !c.is_control()).take(80).collect();
            if m.name.is_empty() {
                return Err("every model needs a name".into());
            }
            if seen.contains(&m.name) {
                return Err(format!("{} is listed twice", m.name));
            }
            seen.push(m.name.clone());
            m.timeout_secs = m.timeout_secs.clamp(MIN_TIMEOUT_SECS, MAX_TIMEOUT_SECS);
        }
        Ok(self)
    }

    /// One line for the screen and the audit.
    pub fn summary(&self) -> String {
        if self.models.is_empty() {
            return format!("{}: the assistant's own model", self.name);
        }
        let list = self.models.iter().map(|m| format!("{}{} {}s", m.name, if m.local { " (local)" } else { "" }, m.timeout_secs)).collect::<Vec<_>>().join(", ");
        format!("{}: {list}; {}{}", self.name, if self.local_first { "local first" } else { "in list order" }, if self.fallback { ", next model on failure" } else { ", no fallback" })
    }
}

/// Ask for a patch along the profile: each model in turn, the next one only when the profile allows and the last failed.
/// The answer carries the cost of every try. On failure the error names each model and why.
pub fn propose_with_profile(host: &dyn Host, profile: &AgentProfile, req: &HealRequest, cancel: &Cancel) -> Result<HealProposal, String> {
    // the cost of a failed try is not reported by the host, so only the answer's own cost counts
    let spent = 0u64;
    let mut why: Vec<String> = vec![];
    for m in profile.order() {
        if cancel.is_cancelled() {
            why.push("cancelled".into());
            break;
        }
        let left = req.max_cents.saturating_sub(spent);
        if spent > 0 && left == 0 {
            why.push("the cost limit of this fix is used up".into());
            break;
        }
        let mut r = req.clone();
        r.model = m.name.clone();
        r.timeout_secs = m.timeout_secs;
        r.max_cents = left;
        match host.propose_patch(&r, cancel) {
            Ok(mut p) => {
                p.cents += spent;
                return Ok(p);
            }
            Err(e) => why.push(format!("{}: {e}", if m.name.is_empty() { "assistant" } else { m.name.as_str() })),
        }
    }
    Err(why.join("; "))
}
