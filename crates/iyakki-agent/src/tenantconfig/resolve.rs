//! Precedence, as a pure function: operator policy > tenant > team > person. A lower layer can only narrow what a higher one
//! allows, and a layer that says "off" switches the item off whatever the others say (deny always wins).

use super::schema::{Field, KindSpec, Merge, Ty};
use super::vault::{SecretState, SecretStore};
use super::Item;
use serde_json::Value;
use std::collections::BTreeMap;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum LayerKind {
    Operator,
    Tenant,
    Team,
    Person,
}

impl LayerKind {
    pub fn name(self) -> &'static str {
        match self {
            LayerKind::Operator => "operator",
            LayerKind::Tenant => "tenant",
            LayerKind::Team => "team",
            LayerKind::Person => "person",
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub struct Layer<'a> {
    pub kind: LayerKind,
    pub item: &'a Item,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Effective {
    pub exists: bool,
    pub enabled: bool,
    /// The highest layer that switched it off, if any.
    pub off_by: Option<LayerKind>,
    pub values: BTreeMap<String, Value>,
}

/// `hi` covers `lo`: `*`, the same text, or `*.example.com` for a name under example.com (not example.com itself).
pub fn covers(hi: &str, lo: &str) -> bool {
    hi == "*"
        || hi == lo
        || hi.strip_prefix('*').is_some_and(|suffix| suffix.starts_with('.') && lo.len() > suffix.len() && lo.ends_with(suffix))
        || hi.strip_suffix('*').is_some_and(|prefix| !prefix.is_empty() && lo.starts_with(prefix))
}

fn strs(v: &Value) -> Vec<String> {
    v.as_array().map(|a| a.iter().filter_map(|x| x.as_str().map(String::from)).collect()).unwrap_or_default()
}

fn merge_value(fd: &Field, hi: &Value, lo: &Value) -> Value {
    match fd.merge {
        Merge::First => hi.clone(),
        Merge::And => Value::Bool(hi.as_bool().unwrap_or(false) && lo.as_bool().unwrap_or(false)),
        Merge::Or => Value::Bool(hi.as_bool().unwrap_or(false) || lo.as_bool().unwrap_or(false)),
        Merge::Min => match (hi.as_i64(), lo.as_i64()) {
            (Some(a), Some(b)) => Value::from(a.min(b)),
            (Some(a), None) => Value::from(a),
            _ => lo.clone(),
        },
        Merge::MaxOrdered => {
            let Ty::Enum(opts) = fd.ty else { return hi.clone() };
            let idx = |v: &Value| v.as_str().and_then(|s| opts.iter().position(|o| *o == s));
            match (idx(hi), idx(lo)) {
                (Some(a), Some(b)) => Value::from(opts[a.max(b)]),
                _ => hi.clone(),
            }
        }
        Merge::Intersect => {
            let (h, l) = (strs(hi), strs(lo));
            Value::from(l.into_iter().filter(|x| h.contains(x)).collect::<Vec<_>>())
        }
        Merge::IntersectCover => {
            let h = strs(hi);
            Value::from(strs(lo).into_iter().filter(|x| h.iter().any(|y| covers(y, x))).collect::<Vec<_>>())
        }
        Merge::IntersectPrefix => {
            let h = strs(hi);
            Value::from(strs(lo).into_iter().filter(|x| h.iter().any(|y| x.starts_with(y.as_str()))).collect::<Vec<_>>())
        }
        Merge::Union => {
            let mut h = strs(hi);
            for x in strs(lo) {
                if !h.contains(&x) {
                    h.push(x);
                }
            }
            Value::from(h)
        }
    }
}

/// Combine the layers of one item. No layer: it does not exist (and is off). The person layer counts only where the kind allows it.
pub fn resolve(spec: &KindSpec, layers: &[Layer]) -> Effective {
    let mut ls: Vec<&Layer> = layers.iter().filter(|l| l.kind != LayerKind::Person || spec.person_override).collect();
    ls.sort_by_key(|l| (l.kind, l.item.namespace.is_some()));
    if ls.is_empty() {
        return Effective { exists: false, enabled: false, off_by: None, values: BTreeMap::new() };
    }
    let fields = spec.all_fields();
    let off_by = ls.iter().find(|l| !l.item.enabled).map(|l| l.kind);
    let mut values: BTreeMap<String, Value> = BTreeMap::new();
    for l in &ls {
        for (k, lo) in &l.item.values {
            let Some(fd) = fields.iter().find(|x| x.name == k) else { continue };
            let merged = match values.get(k) {
                Some(hi) => merge_value(fd, hi, lo),
                None => lo.clone(),
            };
            values.insert(k.clone(), merged);
        }
    }
    for fd in &fields {
        if !values.contains_key(fd.name) {
            if let Some(d) = fd.default.and_then(|d| serde_json::from_str::<Value>(d).ok()) {
                values.insert(fd.name.into(), d);
            }
        }
    }
    Effective { exists: true, enabled: off_by.is_none(), off_by, values }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum State {
    Ready,
    Disabled,
    SecretMissing,
    VaultLocked,
    Invalid,
}

impl State {
    pub fn name(self) -> &'static str {
        match self {
            State::Ready => "ready",
            State::Disabled => "disabled",
            State::SecretMissing => "secret_missing",
            State::VaultLocked => "vault_locked",
            State::Invalid => "invalid",
        }
    }
}

/// Config plus vault status. Disabled wins over everything (an item that is off asks nothing of the vault).
pub fn state_of(spec: &KindSpec, eff: &Effective, vault: &dyn SecretStore) -> (State, Vec<String>) {
    if !eff.exists || !eff.enabled {
        let why = match eff.off_by {
            Some(l) => format!("switched off by the {} level", l.name()),
            None => "not configured".into(),
        };
        return (State::Disabled, vec![why]);
    }
    let fields = spec.all_fields();
    let mut reasons = Vec::new();
    let mut state = State::Ready;
    for fd in &fields {
        let Ty::SecretRef(_) = fd.ty else { continue };
        let needed = fd.required || fd.required_if.is_some_and(|(o, want)| eff.values.get(o).and_then(|v| v.as_str()) == Some(want));
        match eff.values.get(fd.name).and_then(|v| v.as_str()).and_then(super::schema::SecretRef::parse) {
            None if needed => {
                state = State::Invalid;
                reasons.push(format!("{}: no vault reference", fd.name));
            }
            None => {}
            Some(r) => match vault.state(&r) {
                SecretState::Set => {}
                SecretState::Locked => {
                    if state != State::Invalid {
                        state = State::VaultLocked;
                    }
                    reasons.push(format!("{}: the tenant vault is locked", fd.name));
                }
                SecretState::Missing => {
                    if state == State::Ready {
                        state = State::SecretMissing;
                    }
                    reasons.push(format!("{}: secret not set", fd.name));
                }
            },
        }
    }
    (state, reasons)
}
