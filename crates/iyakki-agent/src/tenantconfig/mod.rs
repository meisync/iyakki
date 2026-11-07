//! Tenant-level configuration of a cloud tenant (plan: `docs/development/TENANT-CONFIG-PLAN.md`).
//!
//! A tenant's configuration is two things together:
//! 1. a declarative CONFIG document of non-secret settings, typed by a registry of item kinds ([`schema`]), kept in two files in
//!    `<data_dir>/cloud/config/`: `tenant-config.json` (shipped by the operator, e.g. a mounted ConfigMap, never written by the
//!    service) and `tenant-config.local.json` (edited through the API, versioned, last ten kept), and
//! 2. the tenant's common VAULT ([`vault`]), which holds every secret. A config field that needs a secret holds only a reference
//!    (`vault:agent.claude/token`), resolved at the enforcement point ([`enforce`]) by "use a secret for a named action".
//!
//! Effective state of an item = config + vault status: `ready`, `secret_missing`, `vault_locked`, `disabled` or `invalid`.
//! Precedence ([`resolve`]): operator policy (items the file marks `managed`) > tenant > team/namespace > person; a lower layer can
//! only narrow, and a layer that says "off" wins. Nothing here starts a process; the web handlers are in `webservice/tenantcfg.rs`.

pub mod api;
pub mod approvals;
pub mod catalog;
pub mod fairshare;
pub mod enforce;
pub mod resolve;
pub mod runtime;
pub mod discovery;
pub mod realtest;
pub mod safety;
pub mod schema;
pub mod store;
pub mod vault;

#[cfg(test)]
mod tests;

use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;

pub const SCHEMA_VERSION: u32 = 1;
/// How many versions of the API-edited document are kept.
pub const MAX_VERSIONS: usize = 10;
pub const MAX_ITEMS: usize = 500;
pub const DEFAULT_NAME: &str = "default";

fn default_name() -> String {
    DEFAULT_NAME.into()
}

/// One configured thing. Its key is (`kind`, `namespace`, `name`): `namespace` is the team (`None` = the tenant level), `name` is
/// `default` for a single-value kind, otherwise what the collection is keyed by (a cluster's environment, a remote's name).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Item {
    pub kind: String,
    #[serde(default = "default_name")]
    pub name: String,
    #[serde(default)]
    pub namespace: Option<String>,
    /// Off unless switched on.
    #[serde(default)]
    pub enabled: bool,
    /// Only meaningful in the operator's file: the API may not change or delete this item.
    #[serde(default)]
    pub managed: bool,
    /// Non-secret settings; a secret field holds `vault:<ns>/<name>`.
    #[serde(default)]
    pub values: BTreeMap<String, Value>,
}

pub type Key = (String, Option<String>, String);

impl Item {
    pub fn key(&self) -> Key {
        (self.kind.clone(), self.namespace.clone(), self.name.clone())
    }
    /// What a hash of "this item's state" covers: switch and settings (references, never secrets, are all it can hold).
    pub fn hash(&self) -> String {
        let v = serde_json::json!({"enabled": self.enabled, "values": self.values});
        short_hash(v.to_string().as_bytes())
    }
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Doc {
    #[serde(default)]
    pub schema: u32,
    #[serde(default)]
    pub items: Vec<Item>,
}

pub fn short_hash(b: &[u8]) -> String {
    crate::ocipush::sha256_hex(b).chars().take(12).collect()
}

/// A time source and nothing else, so tests do not need a clock.
pub fn now_secs() -> u64 {
    iyakki_schema::now_secs()
}
