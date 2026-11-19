//! Types moved out of `iyakki_agent::router_registry` (F-SPLIT-03, scripted by dist/split/move_types.py); the agent re-exports them.

use serde::Deserialize;
use serde::Serialize;

/// What a model is used for in a router run.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    /// Plans and answers an executor that is blocked; never runs the steps.
    Planner,
    /// Runs the small steps and fixes them.
    Executor,
    Both,
}

impl Role {
    pub fn can_plan(self) -> bool {
        !matches!(self, Role::Executor)
    }
    pub fn can_execute(self) -> bool {
        !matches!(self, Role::Planner)
    }
    pub fn label(self) -> &'static str {
        match self {
            Role::Planner => "planner",
            Role::Executor => "executor",
            Role::Both => "both",
        }
    }
    pub fn parse(s: &str) -> Option<Role> {
        match s.trim().to_ascii_lowercase().as_str() {
            "planner" => Some(Role::Planner),
            "executor" => Some(Role::Executor),
            "both" => Some(Role::Both),
            _ => None,
        }
    }
}

/// Rough price class. Ordered: the default strategy prefers the lowest class that can do the step.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Cost {
    Free,
    Low,
    Medium,
    High,
}

impl Cost {
    pub fn label(self) -> &'static str {
        match self {
            Cost::Free => "free",
            Cost::Low => "low",
            Cost::Medium => "medium",
            Cost::High => "high",
        }
    }
    pub fn parse(s: &str) -> Option<Cost> {
        match s.trim().to_ascii_lowercase().as_str() {
            "free" => Some(Cost::Free),
            "low" => Some(Cost::Low),
            "medium" => Some(Cost::Medium),
            "high" => Some(Cost::High),
            _ => None,
        }
    }
}

/// Where a model runs: the privacy zone. Ordered from the most private.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Zone {
    /// This computer.
    Computer,
    /// The local network.
    Network,
    /// The internet.
    Internet,
}

impl Zone {
    pub fn label(self) -> &'static str {
        match self {
            Zone::Computer => "this computer",
            Zone::Network => "the local network",
            Zone::Internet => "the internet",
        }
    }
    pub fn parse(s: &str) -> Option<Zone> {
        match s.trim().to_ascii_lowercase().as_str() {
            "computer" => Some(Zone::Computer),
            "network" => Some(Zone::Network),
            "internet" => Some(Zone::Internet),
            _ => None,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Caps {
    pub tools: bool,
    pub vision: bool,
    pub audio: bool,
    /// Context size in tokens (a guess unless overridden).
    pub context: u32,
}

/// One model the router may use. Carries no key and no address, so it can go to a strategy as it is.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Profile {
    /// `provider/model`: the name used everywhere in the router.
    pub id: String,
    pub provider: String,
    pub model: String,
    pub role: Role,
    pub caps: Caps,
    pub cost: Cost,
    pub zone: Zone,
    pub local: bool,
    pub enabled: bool,
}

// ---- moved from iyakki-agent (F-SPLIT-03) ----

/// What the person corrected for one model (`router_set`). Every field is optional: a missing one keeps the hint.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProfileOverride {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub role: Option<Role>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cost: Option<Cost>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tools: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub vision: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub audio: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub enabled: Option<bool>,
    /// Where the model runs, when the hint is wrong (a server on the local network that is really on the internet, or the other way round).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub zone: Option<Zone>,
}

impl ProfileOverride {
    pub fn is_empty(&self) -> bool {
        *self == ProfileOverride::default()
    }
}
