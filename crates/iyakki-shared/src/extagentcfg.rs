//! The external coding agents' settings (Settings -> Agents), moved out of `iyakki_agent::extagent` (F-SPLIT-03). The part that builds the agent
//! objects (`agent`, `all`) stays in the agent as `AgentsCfgExt`.

use serde::{Deserialize, Serialize};
use iyakki_extagentx::*;

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct AgentsCfg {
    pub claude: AgentCfg,
    pub copilot: AgentCfg,
    pub custom: Vec<CustomCfg>,
    /// The agent new chats start with (`None`: a built-in model).
    pub default: Option<String>,
    /// The CLI tile's folder of installed programs: a program installed there is found without a command being set
    /// (filled in by `Services::agents_cfg`, never saved).
    #[serde(skip)]
    pub extra_bin: Option<std::path::PathBuf>,
    /// The signed-in person's home for the vendor programs (`HOME`, `CLAUDE_CONFIG_DIR`), the same one the login uses (filled in by
    /// `Services::agents_cfg`, never saved).
    #[serde(skip)]
    pub env: Vec<(String, String)>,
    /// Agent id -> the account whose token is in the person's vault.
    #[serde(skip)]
    pub vault_logins: std::collections::BTreeMap<String, String>,
    #[serde(skip)]
    pub vault_locked: bool,
}

impl AgentsCfg {
    pub fn validate(&self) -> Result<(), String> {
        let mut seen = std::collections::HashSet::new();
        for c in &self.custom {
            if !valid_custom_id(&c.id) {
                return Err(format!("custom agent `{}`: the id may use letters, digits and dashes", c.id));
            }
            if !seen.insert(c.id.clone()) {
                return Err(format!("duplicate custom agent id `{}`", c.id));
            }
            if c.command.trim().is_empty() {
                return Err(format!("custom agent `{}`: give the command to run", c.id));
            }
            if c.model.contains(' ') || c.custom_models.iter().any(|m| !valid_model_id(m)) || (!c.model.is_empty() && !valid_model_id(&c.model)) {
                return Err(format!("custom agent `{}`: a model id has no spaces or shell characters", c.id));
            }
            if !c.key_env.is_empty() && !valid_env_name(&c.key_env) {
                return Err(format!("custom agent `{}`: the key variable must look like OPENAI_API_KEY", c.id));
            }
        }
        for (n, a) in [("Claude Code", &self.claude), ("GitHub Copilot", &self.copilot)] {
            if (!a.model.is_empty() && !valid_model_id(&a.model)) || a.custom_models.iter().any(|m| !valid_model_id(m)) {
                return Err(format!("{n}: a model id has no spaces or shell characters"));
            }
        }
        for (n, k) in [("Claude Code", &self.claude.key_ref), ("GitHub Copilot", &self.copilot.key_ref)] {
            if !k.is_empty() && !k.starts_with("vault:") {
                return Err(format!("{n}: the API key must come from the vault (vault:NAME)"));
            }
        }
        Ok(())
    }

    /// A program installed from the CLI tile is used when no command is set.
    pub fn with_bin(&self, c: &AgentCfg, name: &str) -> AgentCfg {
        let mut c = c.clone();
        if c.command.trim().is_empty() {
            if let Some(p) = self.extra_bin.as_ref().map(|d| d.join(name)).filter(|p| p.exists()) {
                c.command = p.display().to_string();
            }
        }
        c.env = self.env.clone();
        c.vault_login = self.vault_logins.get(if name == "claude" { CLAUDE_CODE } else { COPILOT }).cloned();
        c.vault_locked = self.vault_locked;
        c
    }

    pub fn custom_with_bin(&self, c: &CustomCfg) -> CustomCfg {
        let mut c = c.clone();
        let words = crate::wsexec::split_command(&c.command);
        if let (Some(first), Some(d)) = (words.first(), self.extra_bin.as_ref()) {
            let p = d.join(first);
            if !first.contains('/') && p.exists() {
                c.command = c.command.trim_start().replacen(first.as_str(), &p.display().to_string(), 1);
            }
        }
        c
    }

    /// Does this agent write PDF files itself (`native_pdf_output`)? Then `pdf_create` is not offered to it.
    pub fn native_pdf_output(&self, id: &str) -> bool {
        match id {
            CLAUDE_CODE => self.claude.native_pdf_output,
            COPILOT => self.copilot.native_pdf_output,
            other => other.strip_prefix("custom:").and_then(|c| self.custom.iter().find(|x| x.id == c)).is_some_and(|c| c.native_pdf_output),
        }
    }

    /// Is `id` an agent these settings can build (Claude Code, Copilot or a configured custom one)?
    pub fn knows(&self, id: &str) -> bool {
        match id {
            CLAUDE_CODE | COPILOT => true,
            other => other.strip_prefix("custom:").is_some_and(|c| self.custom.iter().any(|x| x.id == c)),
        }
    }

    pub fn cfg_of(&self, id: &str) -> Option<(bool, String)> {
        match id {
            CLAUDE_CODE => Some((self.claude.allow_auto, self.claude.key_ref.clone())),
            COPILOT => Some((self.copilot.allow_auto, self.copilot.key_ref.clone())),
            other => other.strip_prefix("custom:").and_then(|c| self.custom.iter().find(|x| x.id == c)).map(|c| (c.allow_auto, c.key_ref.clone())),
        }
    }
}

