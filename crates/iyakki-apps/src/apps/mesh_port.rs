//! What the MESH tile asks of the rest of the system (the app standard, docs/development/APP-DEVELOPMENT-PROCEDURE.md section 12).
//!
//! The tile (`mesh.rs`) names no `Services`, no session, no vault and no audit log: it asks a [`MeshPort`] for the saved Consul address, the
//! account's host policy, a Consul client (the real one carries the vault token and the core's quick upstream), the token's storage and the
//! audit line. The real port ([`CxPort`]) is the only place that reads the session and calls the core.


use std::sync::Arc;

use iyakki_shared::consul::{self, Consul};
use iyakki_tile_api::mesh as t;

use super::AppCx;

pub trait MeshPort {
    /// The saved Consul address (empty until an administrator sets it).
    fn load_address(&self) -> String;
    fn save_address(&self, address: &str) -> Result<(), String>;
    /// The account may change the connection (administrators).
    fn can_configure(&self) -> bool;
    /// The account's host policy lets it reach `host`.
    fn allows_host(&self, host: &str) -> bool;
    /// A client for `address`, with the token stored under `token_name` when there is one.
    fn consul(&self, address: &str, token_name: &str) -> Result<Consul, String>;
    /// Keep the Consul token in the system vault.
    fn store_token(&self, token_name: &str, token: &str) -> Result<(), String>;
    /// A token is stored under `token_name`.
    fn has_token(&self, token_name: &str) -> bool;
    /// A line in the audit log for the signed-in person.
    fn audit(&self, action: &str, subject: &str);
}

fn config_path(cx: &AppCx) -> std::path::PathBuf {
    cx.data_dir().join("mesh.json")
}

pub fn load_address(cx: &AppCx) -> String {
    std::fs::read_to_string(config_path(cx)).ok().and_then(|t| serde_json::from_str::<serde_json::Value>(&t).ok()).and_then(|v| v["address"].as_str().map(str::to_string)).unwrap_or_default()
}

pub fn save_address(cx: &AppCx, address: &str) -> Result<(), String> {
    let p = config_path(cx);
    let tmp = p.with_extension("tmp");
    std::fs::write(&tmp, serde_json::json!({ "address": address }).to_string()).and_then(|_| std::fs::rename(&tmp, &p)).map_err(|e| e.to_string())
}

/// The real port, over the shell's context.
pub struct CxPort<'a>(pub &'a AppCx);

impl MeshPort for CxPort<'_> {
    fn load_address(&self) -> String {
        load_address(self.0)
    }
    fn save_address(&self, address: &str) -> Result<(), String> {
        save_address(self.0, address)
    }
    fn can_configure(&self) -> bool {
        self.0.session.policy.manage_settings
    }
    fn allows_host(&self, host: &str) -> bool {
        self.0.session.policy.allows_host(host)
    }
    fn consul(&self, address: &str, token_name: &str) -> Result<Consul, String> {
        let token = self.0.secrets().resolve(&format!("vault:{token_name}"), None).ok().map(|t| t.to_string());
        Consul::new(address, token, self.0.quick_upstream())
    }
    fn store_token(&self, token_name: &str, token: &str) -> Result<(), String> {
        self.0.vault().system_set(token_name, token).map_err(|e| e.to_string())
    }
    fn has_token(&self, token_name: &str) -> bool {
        self.0.secrets_api().has_ref(&format!("vault:{token_name}"))
    }
    fn audit(&self, action: &str, subject: &str) {
        self.0.audit.record(&self.0.session.user, action, subject);
    }
}

// ------------------------------------------------------------------------------------------------ the tile crate's port

fn health_to_tile(h: consul::Health) -> t::Health {
    match h {
        consul::Health::Passing => t::Health::Passing,
        consul::Health::Warning => t::Health::Warning,
        consul::Health::Critical => t::Health::Critical,
    }
}

/// The real Consul client as the tile crate's client: the same calls, the answers turned into the tile's plain types.
pub struct ShellClient(pub Arc<Consul>);

impl t::MeshClient for ShellClient {
    fn cluster(&self) -> Result<t::ClusterInfo, String> {
        self.0.cluster().map(|c| t::ClusterInfo { leader: c.leader, peers: c.peers, version: c.version, datacenter: c.datacenter })
    }
    fn services(&self) -> Result<Vec<t::ServiceInfo>, String> {
        self.0.services().map(|v| v.into_iter().map(|s| t::ServiceInfo { name: s.name, tags: s.tags }).collect())
    }
    fn instances(&self, service: &str) -> Result<Vec<t::Instance>, String> {
        self.0.instances(service).map(|v| {
            v.into_iter()
                .map(|i| t::Instance { node: i.node, address: i.address, port: i.port, status: health_to_tile(i.status), checks: i.checks.into_iter().map(|c| t::Check { name: c.name, status: health_to_tile(c.status), output: c.output }).collect() })
                .collect()
        })
    }
    fn nodes(&self) -> Result<Vec<t::NodeInfo>, String> {
        self.0.nodes().map(|v| v.into_iter().map(|n| t::NodeInfo { name: n.name, address: n.address }).collect())
    }
    fn kv_list(&self, prefix: &str) -> Result<Vec<String>, String> {
        self.0.kv_list(prefix)
    }
    fn kv_get(&self, key: &str) -> Result<Option<String>, String> {
        self.0.kv_get(key)
    }
    fn kv_put(&self, key: &str, value: &str) -> Result<(), String> {
        self.0.kv_put(key, value)
    }
    fn kv_delete(&self, key: &str) -> Result<(), String> {
        self.0.kv_delete(key)
    }
}

/// The shell's port as the tile crate's port.
pub struct Bridge<'a>(pub &'a dyn MeshPort);

impl t::MeshPort for Bridge<'_> {
    fn load_address(&self) -> String {
        self.0.load_address()
    }
    fn save_address(&self, address: &str) -> Result<(), String> {
        self.0.save_address(address)
    }
    fn can_configure(&self) -> bool {
        self.0.can_configure()
    }
    fn allows_host(&self, host: &str) -> bool {
        self.0.allows_host(host)
    }
    fn client(&self, address: &str, token_name: &str) -> Result<Arc<dyn t::MeshClient>, String> {
        self.0.consul(address, token_name).map(|c| Arc::new(ShellClient(Arc::new(c))) as Arc<dyn t::MeshClient>)
    }
    fn valid_address(&self, address: &str) -> Result<String, String> {
        consul::valid_address(address)
    }
    fn host_of(&self, address: &str) -> String {
        consul::host_of(address)
    }
    fn store_token(&self, token_name: &str, token: &str) -> Result<(), String> {
        self.0.store_token(token_name, token)
    }
    fn has_token(&self, token_name: &str) -> bool {
        self.0.has_token(token_name)
    }
    fn audit(&self, action: &str, subject: &str) {
        self.0.audit(action, subject)
    }
}

#[cfg(test)]
mod mirror_tests {
    use super::*;

    #[test]
    fn health_has_the_same_labels_and_order_in_both_crates() {
        for (a, b) in [(consul::Health::Passing, t::Health::Passing), (consul::Health::Warning, t::Health::Warning), (consul::Health::Critical, t::Health::Critical)] {
            assert_eq!(health_to_tile(a), b);
            assert_eq!(a.label(), b.label());
        }
        assert!(health_to_tile(consul::Health::Passing) < health_to_tile(consul::Health::Warning) && health_to_tile(consul::Health::Warning) < health_to_tile(consul::Health::Critical));
    }
}
