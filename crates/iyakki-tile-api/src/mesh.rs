//! The plain data and the port of the MESH tile (D-S1-E4-01): what a Consul cluster answers, as plain mirrors of the types in
//! `iyakki_shared::consul`, and what the tile asks of the rest of the system. The shell converts between the two and hands the tile a
//! client that carries the vault token and the core's upstream; the tile never sees a token, an address policy or an HTTP request.

use std::sync::Arc;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ServiceInfo {
    pub name: String,
    pub tags: Vec<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Health {
    Passing,
    Warning,
    Critical,
}

impl Health {
    pub fn label(self) -> &'static str {
        match self {
            Health::Passing => "passing",
            Health::Warning => "warning",
            Health::Critical => "critical",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Check {
    pub name: String,
    pub status: Health,
    pub output: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Instance {
    pub node: String,
    pub address: String,
    pub port: u16,
    pub status: Health,
    pub checks: Vec<Check>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NodeInfo {
    pub name: String,
    pub address: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ClusterInfo {
    pub leader: String,
    pub peers: Vec<String>,
    pub version: String,
    pub datacenter: String,
}

/// A client for one Consul cluster (the shell's real one speaks HTTP through the core's upstream; a fake answers in tests). It is used from
/// threads of the tile's own, so it is `Send + Sync`.
pub trait MeshClient: Send + Sync {
    fn cluster(&self) -> Result<ClusterInfo, String>;
    fn services(&self) -> Result<Vec<ServiceInfo>, String>;
    fn instances(&self, service: &str) -> Result<Vec<Instance>, String>;
    fn nodes(&self) -> Result<Vec<NodeInfo>, String>;
    fn kv_list(&self, prefix: &str) -> Result<Vec<String>, String>;
    fn kv_get(&self, key: &str) -> Result<Option<String>, String>;
    fn kv_put(&self, key: &str, value: &str) -> Result<(), String>;
    fn kv_delete(&self, key: &str) -> Result<(), String>;
}

/// What the MESH tile asks of the rest of the system.
pub trait MeshPort {
    /// The saved Consul address (empty until an administrator sets it).
    fn load_address(&self) -> String;
    fn save_address(&self, address: &str) -> Result<(), String>;
    /// The account may change the connection (administrators).
    fn can_configure(&self) -> bool;
    /// The account's host policy lets it reach `host`.
    fn allows_host(&self, host: &str) -> bool;
    /// A client for `address`, with the token stored under `token_name` when there is one.
    fn client(&self, address: &str, token_name: &str) -> Result<Arc<dyn MeshClient>, String>;
    /// `http://host:8500` or `https://host`: the address cleaned up, or why it is not one.
    fn valid_address(&self, address: &str) -> Result<String, String>;
    /// The host part of an address (what the policy and the audit line name).
    fn host_of(&self, address: &str) -> String;
    /// Keep the Consul token in the system vault.
    fn store_token(&self, token_name: &str, token: &str) -> Result<(), String>;
    /// A token is stored under `token_name`.
    fn has_token(&self, token_name: &str) -> bool;
    /// A line in the audit log for the signed-in person.
    fn audit(&self, action: &str, subject: &str);
}
