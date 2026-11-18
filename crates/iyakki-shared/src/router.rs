//! Types moved out of `iyakki_agent::router` (F-SPLIT-03, scripted by dist/split/move_types.py); the agent re-exports them.

use serde::Deserialize;
use serde::Serialize;

/// The limits of one run. The values a person sets are held to the `HARD_*` caps; a strategy cannot change them.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Budget {
    /// Tokens (in and out, all models) one run may use.
    pub max_tokens: u64,
    /// Model calls one run may make.
    pub max_calls: u32,
    /// Tries of one step in total (all executors).
    pub max_attempts: u32,
    /// Times the planner is asked about one step.
    pub max_escalations: u32,
}

pub const HARD_TOKENS: u64 = 10_000_000;

pub const HARD_CALLS: u32 = 1000;

pub const HARD_ATTEMPTS: u32 = 10;

pub const HARD_ESCALATIONS: u32 = 5;

impl Default for Budget {
    fn default() -> Self {
        Budget { max_tokens: 200_000, max_calls: 60, max_attempts: 4, max_escalations: 2 }
    }
}

impl Budget {
    pub fn validate(&self) -> Result<(), String> {
        if self.max_tokens == 0 || self.max_calls == 0 || self.max_attempts == 0 {
            return Err("the budget needs at least 1 token, 1 call and 1 attempt".into());
        }
        if self.max_tokens > HARD_TOKENS || self.max_calls > HARD_CALLS || self.max_attempts > HARD_ATTEMPTS || self.max_escalations > HARD_ESCALATIONS {
            return Err(format!("the budget is too large (at most {HARD_TOKENS} tokens, {HARD_CALLS} calls, {HARD_ATTEMPTS} attempts and {HARD_ESCALATIONS} rounds with the planner per step)"));
        }
        Ok(())
    }
}

pub const DEFAULT_STRATEGY: &str = "default";

// ---- moved from iyakki-agent (F-SPLIT-03) ----
use crate::router_registry::ProfileOverride;
use crate::router_registry::Zone;
use std::collections::BTreeMap;

/// The router's settings of one person.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RouterConfig {
    /// Off until the person turns it on.
    #[serde(default)]
    pub enabled: bool,
    /// `default` or the name of a strategy plugin.
    #[serde(default = "default_name")]
    pub strategy: String,
    /// The least private place a model may run in (the privacy zone the person allows).
    #[serde(default = "internet")]
    pub max_zone: Zone,
    #[serde(default)]
    pub budget: Budget,
    /// A model the person fixed as the planner (a strategy is then not asked who plans).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub planner: Option<String>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub overrides: BTreeMap<String, ProfileOverride>,
    /// The most tokens all runs together may use in 24 hours (0: no daily limit). A run starts only with what is left of it, so the router
    /// shares the person's own limit the way a tile app shares its tokens a day (F-ROUTER-14).
    #[serde(default, skip_serializing_if = "is_zero_u64")]
    pub daily_tokens: u64,
    /// A strategy chosen for one place instead of the computer's own: `workspace:NAME` (an IDE workspace) or `fleet:NAME` (F-ROUTER-13).
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub scoped: BTreeMap<String, String>,
    /// Steps that may run at the same time when more than one is ready (1: one at a time, as before; at most [`HARD_PARALLEL`]).
    #[serde(default = "one", skip_serializing_if = "is_one")]
    pub parallel: u32,
}

/// The most steps that run at once, whatever the person sets.
pub const HARD_PARALLEL: u32 = 4;

pub fn one() -> u32 {
    1
}

pub fn is_one(n: &u32) -> bool {
    *n == 1
}

pub fn is_zero_u64(n: &u64) -> bool {
    *n == 0
}

pub fn default_name() -> String {
    DEFAULT_STRATEGY.into()
}

pub fn internet() -> Zone {
    Zone::Internet
}

impl Default for RouterConfig {
    fn default() -> Self {
        RouterConfig { enabled: false, strategy: default_name(), max_zone: Zone::Internet, budget: Budget::default(), planner: None, overrides: BTreeMap::new(), daily_tokens: 0, scoped: BTreeMap::new(), parallel: 1 }
    }
}
