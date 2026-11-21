//! What the MESH tile asks of the rest of the system (the app standard, docs/development/APP-DEVELOPMENT-PROCEDURE.md section 12).
//!
//! The tile (`mesh.rs`) names no `Services`, no session, no vault and no audit log: it asks a [`MeshPort`] for the saved Consul address, the
//! account's host policy, a Consul client (the real one carries the vault token and the core's quick upstream), the token's storage and the
//! audit line. The real port ([`CxPort`]) is the only place that reads the session and calls the core.


use iyakki_shared::consul::Consul;

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
