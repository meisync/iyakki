//! Settings as a typed key/value store with a schema: get, set, watch, per-key scope, an audit line per change.

use super::context::{CallContext, Risk};
use super::error::{CallResult, IdempotencyKey};
use super::event::{EventBatch, Resume};
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", content = "value", rename_all = "snake_case")]
pub enum SettingValue {
    Bool(bool),
    Int(i64),
    Text(String),
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum SettingKind {
    Bool,
    Int { min: i64, max: i64 },
    Text { max_len: u32 },
    /// One of these texts.
    Choice { options: Vec<String> },
}

impl SettingKind {
    /// Does the value fit this kind?
    pub fn accepts(&self, v: &SettingValue) -> bool {
        match (self, v) {
            (SettingKind::Bool, SettingValue::Bool(_)) => true,
            (SettingKind::Int { min, max }, SettingValue::Int(n)) => n >= min && n <= max,
            (SettingKind::Text { max_len }, SettingValue::Text(t)) => t.chars().count() <= *max_len as usize,
            (SettingKind::Choice { options }, SettingValue::Text(t)) => options.iter().any(|o| o == t),
            _ => false,
        }
    }
}

/// The schema of one key. There is no kind for a secret: secrets are not settings (see the vault).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SettingSpec {
    pub key: String,
    pub kind: SettingKind,
    pub default: SettingValue,
    /// Scope a remote caller needs to read / write this key.
    pub read_scope: String,
    pub write_scope: String,
    /// `Change`, or `AskPerson` for keys a person must confirm; `LocalOnly` for keys a remote caller must never touch.
    pub write_risk: Risk,
    pub label_key: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SettingChange {
    pub key: String,
    pub value: SettingValue,
    /// The event sequence number the change got.
    pub seq: u64,
    /// True when an idempotency key matched an earlier identical change and nothing was written again.
    pub repeated: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SettingEvent {
    pub key: String,
    pub value: SettingValue,
    pub by: String,
}

pub trait Settings: Send + Sync {
    fn schema(&self, ctx: &CallContext) -> CallResult<Vec<SettingSpec>>;
    fn get(&self, ctx: &CallContext, key: &str) -> CallResult<SettingValue>;
    fn set(&self, ctx: &CallContext, key: &str, value: SettingValue, idem: Option<&IdempotencyKey>) -> CallResult<SettingChange>;
    /// Changes after `resume`, optionally for one key (`watch`).
    fn watch(&self, ctx: &CallContext, key: Option<&str>, resume: Resume, limit: usize) -> CallResult<EventBatch<SettingEvent>>;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kinds_accept_only_fitting_values() {
        assert!(SettingKind::Bool.accepts(&SettingValue::Bool(true)));
        assert!(!SettingKind::Bool.accepts(&SettingValue::Int(1)));
        let i = SettingKind::Int { min: 0, max: 10 };
        assert!(i.accepts(&SettingValue::Int(10)) && !i.accepts(&SettingValue::Int(11)));
        let c = SettingKind::Choice { options: vec!["a".into(), "b".into()] };
        assert!(c.accepts(&SettingValue::Text("b".into())) && !c.accepts(&SettingValue::Text("c".into())));
        assert!(!SettingKind::Text { max_len: 2 }.accepts(&SettingValue::Text("abc".into())));
    }
}
