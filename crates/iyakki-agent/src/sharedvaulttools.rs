//! The lead assistant's tools for the shared vault's provider (docs/development/SHARED-VAULT-CRYPTO-OPTIONS.md): `shared_vault_provider`
//! reads the setting (no secrets; it still asks, as every tool with "vault" in its name does, `agent::hard_ask`); `shared_vault_provider_set` changes it and asks the person every time, in Auto mode too
//! (`agent::hard_ask`). The assistant never sends or sees a token: `token_secret` is the NAME of a vault secret, and text that looks like
//! a token or a key is refused without being echoed. Switching the provider does not move secrets from one system to the other.

use std::path::PathBuf;
use std::sync::Arc;

use serde_json::{json, Value};

use crate::agent::{Risk, ToolDef, Tools};
use crate::chat::ToolSpec;
use crate::sharedvault::{ExternalRef, Provider, Setting, LABEL};

pub const PROMPT_RULE: &str = "\nSHARED VAULT: shared_vault_provider shows which cryptography keeps the cloud tenant's shared vault (built-in, which is not externally reviewed, or the person's own external vault). To change it call shared_vault_provider_set; it asks the person every time. Give the NAME of the vault secret that holds the external access token, never the token. Switching does not move existing secrets.";

pub const READ_TOOL: &str = "shared_vault_provider";
pub const WRITE_TOOL: &str = "shared_vault_provider_set";

pub fn is_shared_vault_tool(n: &str) -> bool {
    n == READ_TOOL || n == WRITE_TOOL
}

pub struct SharedVaultTools {
    inner: Arc<dyn Tools>,
    data_dir: PathBuf,
    actor: String,
}

impl SharedVaultTools {
    pub fn new(inner: Arc<dyn Tools>, data_dir: PathBuf, actor: &str) -> SharedVaultTools {
        SharedVaultTools { inner, data_dir, actor: actor.to_string() }
    }

    fn text(args: &Value, k: &str) -> Option<String> {
        args.get(k).and_then(Value::as_str).map(|s| s.trim().to_string()).filter(|s| !s.is_empty())
    }

    /// The setting a call asks for. Anything that looks like a token or key text is refused without being echoed.
    fn wanted(&self, args: &Value) -> Result<Setting, String> {
        for k in ["address", "mount", "path", "namespace", "token_secret", "provider"] {
            if let Some(t) = Self::text(args, k) {
                if t.len() > 300 || t.contains(['\n', ' ']) || t.contains("-----BEGIN") || t.starts_with("hvs.") || (t.starts_with("s.") && k == "token_secret") {
                    return Err("a value looks like a token or is not a plain setting. Never send a token: give the NAME of the vault secret that holds it.".into());
                }
            }
        }
        let s = match Self::text(args, "provider").as_deref() {
            Some("builtin") => Setting { v: crate::sharedvault::FILE_VERSION, provider: Provider::Builtin, external: None },
            Some("external") => {
                let get = |k: &str| Self::text(args, k).ok_or_else(|| format!("external needs {k}"));
                Setting {
                    v: crate::sharedvault::FILE_VERSION,
                    provider: Provider::External,
                    external: Some(ExternalRef { address: get("address")?, mount: Self::text(args, "mount").unwrap_or_else(|| "secret".into()), path: get("path")?, namespace: Self::text(args, "namespace"), token_secret: get("token_secret")? }),
                }
            }
            _ => return Err("provider is builtin or external".into()),
        };
        s.check()?;
        Ok(s)
    }

    fn card(&self, args: &Value) -> Result<String, String> {
        let s = self.wanted(args)?;
        Ok(match (&s.external, s.provider) {
            (Some(e), _) => format!("Keep the shared vault's secrets in the person's own external vault ({LABEL} here).\n• Address: {}\n• Mount and path: {}/{}\n• Access token: the vault secret named {} (the assistant never sees it)\n• Existing secrets are not moved. If that system cannot be reached the shared vault stays closed; it never falls back to the built-in store.", e.address, e.mount, e.path, e.token_secret),
            _ => format!("Use the built-in shared vault ({LABEL}). Secrets kept in an external system are not moved."),
        })
    }
}

impl Tools for SharedVaultTools {
    fn defs(&self) -> Vec<ToolDef> {
        let mut v = self.inner.defs();
        v.retain(|d| !is_shared_vault_tool(&d.spec.name));
        v.push(ToolDef { spec: ToolSpec { name: READ_TOOL.into(), description: "Show which cryptography keeps the shared vault (built-in or external) and its review status. No secrets.".into(), schema: json!({"type": "object", "properties": {}}) }, risk: Risk::Safe, untrusted: false });
        let schema = json!({"type": "object", "properties": {
            "provider": {"type": "string", "enum": ["builtin", "external"], "description": "builtin (default) or external (the person's own HashiCorp-style KV v2 vault)."},
            "address": {"type": "string", "description": "external: https://host[:port]."},
            "mount": {"type": "string", "description": "external: the KV v2 mount (secret)."},
            "path": {"type": "string", "description": "external: where under the mount to keep IYAKKI's secrets, for example iyakki/acme."},
            "namespace": {"type": "string", "description": "external: the provider's namespace, if any."},
            "token_secret": {"type": "string", "description": "external: the NAME of the vault secret that holds the access token. Never the token."}
        }, "required": ["provider"]});
        v.push(ToolDef { spec: ToolSpec { name: WRITE_TOOL.into(), description: "Choose which cryptography keeps the shared vault. The person approves a card every time. Secrets are not moved.".into(), schema }, risk: Risk::Ask, untrusted: false });
        v
    }

    fn call(&self, name: &str, args: &Value) -> Result<String, String> {
        match name {
            READ_TOOL => Setting::load(&self.data_dir).map(|s| s.summary()),
            WRITE_TOOL => {
                let s = self.wanted(args)?;
                s.save(&self.data_dir, &self.actor)?;
                Ok(format!("Saved. {}", s.summary()))
            }
            _ => self.inner.call(name, args),
        }
    }

    fn describe(&self, name: &str, args: &Value) -> String {
        match name {
            READ_TOOL => "Show the shared vault's provider".into(),
            WRITE_TOOL => match self.card(args) {
                Ok(c) => c.lines().next().unwrap_or("").to_string(),
                Err(e) => format!("{name} (invalid: {e})"),
            },
            _ => self.inner.describe(name, args),
        }
    }

    fn preview(&self, name: &str, args: &Value) -> Option<String> {
        match name {
            WRITE_TOOL => self.card(args).ok(),
            READ_TOOL => None,
            _ => self.inner.preview(name, args),
        }
    }

    fn refusal(&self, name: &str, args: &Value) -> Option<String> {
        match name {
            WRITE_TOOL => self.card(args).err(),
            READ_TOOL => None,
            _ => self.inner.refusal(name, args),
        }
    }

    fn pre_approved(&self, name: &str) -> bool {
        !is_shared_vault_tool(name) && self.inner.pre_approved(name)
    }

    fn approved_always(&self, name: &str) -> bool {
        name != WRITE_TOOL && self.inner.approved_always(name)
    }

    fn grant_blocker(&self, name: &str, args: &Value) -> Option<String> {
        if name == WRITE_TOOL { Some("changing the shared vault's provider asks every time".into()) } else { self.inner.grant_blocker(name, args) }
    }
    fn grant_scope(&self, name: &str, args: &Value) -> String {
        self.inner.grant_scope(name, args)
    }

    fn unresolved_work(&self) -> Vec<String> {
        self.inner.unresolved_work()
    }

    fn begin_step(&self) {
        self.inner.begin_step()
    }

    fn should_end_run(&self) -> Option<String> {
        self.inner.should_end_run()
    }

    fn take_diagnosis(&self) -> Option<(String, String)> {
        self.inner.take_diagnosis()
    }

    fn note_external(&self, name: &str, args: &Value, ok: bool) {
        self.inner.note_external(name, args, ok)
    }
}
