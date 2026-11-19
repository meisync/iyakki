//! Reading the approvals file of the store (moved from the agent; the writing `FileBook` stays there).

use std::collections::BTreeMap;
use std::path::Path;

use iyakki_core_api::appcaps::Approval;
use iyakki_schema::Value;

/// Read the approvals of a file without changing anything: both layouts, any schema. A missing or unreadable file is "nothing
/// approved" (the safe answer for a lookup); nothing is ever written from here.
pub fn read_map(path: &Path) -> BTreeMap<String, Approval> {
    let Some(v) = std::fs::read(path).ok().and_then(|b| serde_json::from_slice::<Value>(&b).ok()) else { return BTreeMap::new() };
    map_of(&v).unwrap_or_default()
}

pub fn map_of(v: &Value) -> Option<BTreeMap<String, Approval>> {
    let inner = match v.get("approvals") {
        Some(a) if a.is_object() && v.get(iyakki_schema::KEY).is_some() => a.clone(),
        _ => v.clone(),
    };
    serde_json::from_value(inner).ok()
}

