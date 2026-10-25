//! System settings: model providers, which model chat and embeddings use, RAG tuning, external RAG
//! backends, and external secret providers. Stored as JSON (atomic writes, mode 0600). **Contains no
//! secrets** — only secret *references*, which `validate` enforces (a pasted raw key is rejected).

use std::collections::{BTreeMap, HashSet};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::providers::{valid_model_name, valid_provider_id, ProviderCfg};
use crate::rag_ext::ExternalRag;
use crate::vault::ProviderConfig as SecretProviderCfg;
use crate::voice::VoiceSettings;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModelChoice {
    pub provider: String,
    pub model: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct RagSettings {
    pub chunk_max: usize,
    pub overlap: usize,
    pub top_k: usize,
    /// Query external backends in addition to the local knowledge base.
    pub use_external: bool,
}

impl Default for RagSettings {
    fn default() -> Self {
        Self { chunk_max: 700, overlap: 100, top_k: 5, use_external: true }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExtEntry {
    pub enabled: bool,
    #[serde(flatten)]
    pub backend: ExternalRag,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct VaultSettings {
    /// Re-lock the system vault when the last admin signs out (default keeps it open so chat keeps working).
    pub lock_on_admin_logout: bool,
    /// Optional key file that can unlock the system vault at boot (unattended nodes). Trades
    /// security for availability: anyone who can read this file can open the vault.
    pub keyfile: Option<String>,
}

/// The clock in the top bar and the machine's time (Settings -> Time).
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct TimeSettings {
    /// A zone name such as "Europe/Paris"; empty = what the machine says (`TZ`, `/etc/localtime`, else UTC).
    pub zone: String,
    /// 12-hour clock with AM/PM (the default is 24-hour).
    pub hour12: bool,
    /// Show seconds in the top bar.
    pub seconds: bool,
    /// Do not set the clock from the network.
    pub no_ntp: bool,
    /// A time server other than the built-in ones (a host name).
    pub ntp_server: String,
    /// Detect the zone from the public IP address (opt-in: asks an internet service, see `tzdetect.rs`) and keep it up to date.
    /// Choosing a zone by hand switches it off.
    pub zone_auto: bool,
    /// When the zone was last looked up (seconds since the epoch; 0 = never).
    pub zone_checked_at: i64,
    /// Which service answered the last lookup, and what it said ("ipapi.co: Asia/Kolkata"), or why it failed.
    pub zone_note: String,
}

/// The sign-in screen's saver choices (an administrator changes them; they start as `SaverConfig::signin_default`).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SigninSaver(pub crate::screensaver::SaverConfig);

impl Default for SigninSaver {
    fn default() -> SigninSaver {
        SigninSaver(crate::screensaver::SaverConfig::signin_default())
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    pub time: TimeSettings,
    pub providers: Vec<ProviderCfg>,
    pub chat: Option<ModelChoice>,
    /// `None` = built-in hashed embeddings (no model needed).
    pub embedding: Option<ModelChoice>,
    pub rag: RagSettings,
    pub external_rag: Vec<ExtEntry>,
    pub secret_providers: BTreeMap<String, SecretProviderCfg>,
    pub vault: VaultSettings,
    pub voice: VoiceSettings,
    /// Settings -> Sound: the output IYAKKI plays on (automatic or chosen), its volume and mute.
    pub sound: crate::audioout::SoundSettings,
    /// Settings -> System sounds: cues for signing in and out, notifications and finished jobs (old files load with the defaults).
    pub sounds: crate::syssounds::SystemSounds,
    /// Settings -> Keyboard and function keys: Fn-key mode of Apple keyboards, key repeat.
    pub keyboard: crate::keyboard::KeyboardSettings,
    /// Settings -> Battery (laptops): percentage in the top bar, low-battery warnings, shutdown at the critical level.
    pub battery: crate::battery::BatterySettings,
    /// Settings -> Screen saver: what the sign-in screen does when idle (each person's own choices are in their own file, `screensaver`).
    pub screensaver_signin: SigninSaver,
    /// Settings -> Cloud mode: hide what needs local hardware (Automatic follows the machine; see `cloudmode`).
    pub cloud: crate::cloudmode::Setting,
    /// What a million tokens cost, in US cents. Used only to enforce agents' cost budgets (0 = free / untracked).
    pub token_price_cents_per_mtok: u32,
    /// How many days the log of what the assistant's tools did is kept (0 = the default, 3).
    pub tool_log_days: u32,
    /// The largest media file a download may fetch, in MB (0 = the default, 2048).
    pub media_download_max_mb: u32,
    /// Seconds of pure thinking (no answer, no tool call) before a model call is abandoned and retried with shorter thinking (0 = the default, 60).
    pub think_limit_secs: u32,
    /// Seconds with no token of any kind before a model call is abandoned (0 = the default, 120).
    pub silence_limit_secs: u32,
    /// Turn the workspace history off (it is on by default: Autopilot steps and summaries are kept in the workspace's own knowledge base).
    pub no_workspace_history: bool,
    /// External coding agents (Claude Code, GitHub Copilot, custom commands): Settings → Agents.
    pub agents: crate::extagent::AgentsCfg,
    /// Settings → Cloud mode → "Show all AI tools": show the AI tools the machine's hardware profile would hide (docs/AI-HARDWARE.md).
    pub show_all_ai: bool,
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum SettingsError {
    #[error("{0}")]
    Invalid(String),
    #[error("storage error: {0}")]
    Storage(String),
}

fn bad<T>(s: impl Into<String>) -> Result<T, SettingsError> {
    Err(SettingsError::Invalid(s.into()))
}

impl Settings {
    pub fn provider(&self, id: &str) -> Option<&ProviderCfg> {
        self.providers.iter().find(|p| p.id == id)
    }

    pub fn validate(&self) -> Result<(), SettingsError> {
        if !self.time.zone.is_empty() && crate::clock::Zone::named(&self.time.zone).is_none() {
            return bad(format!("unknown time zone `{}`", self.time.zone));
        }
        if !self.time.ntp_server.is_empty() && !crate::ntp::valid_server(&self.time.ntp_server) {
            return bad("the time server must be a host name or address");
        }
        let mut seen = HashSet::new();
        for p in &self.providers {
            p.validate().map_err(|e| SettingsError::Invalid(format!("provider `{}`: {e}", p.id)))?;
            if !seen.insert(p.id.clone()) {
                return bad(format!("duplicate provider id `{}`", p.id));
            }
        }
        for (what, choice) in [("chat", &self.chat), ("embedding", &self.embedding)] {
            if let Some(c) = choice {
                if let Some(a) = crate::extagent::agent_of(&c.provider) {
                    if what == "chat" && self.agents.agent(a).is_some() {
                        continue;
                    }
                }
                let p = self.provider(&c.provider).ok_or_else(|| SettingsError::Invalid(format!("{what}: unknown provider `{}`", c.provider)))?;
                if !p.enabled {
                    return bad(format!("{what}: provider `{}` is disabled", c.provider));
                }
                if !valid_model_name(&c.model) {
                    return bad(format!("{what}: invalid model name"));
                }
            }
        }
        let r = &self.rag;
        if !(100..=4000).contains(&r.chunk_max) || r.overlap >= r.chunk_max / 2 || !(1..=50).contains(&r.top_k) {
            return bad("rag: chunk size must be 100–4000, overlap under half of it, top-k 1–50");
        }
        let mut ext_ids = HashSet::new();
        for e in &self.external_rag {
            e.backend.validate().map_err(|m| SettingsError::Invalid(format!("external RAG `{}`: {m}", e.backend.id())))?;
            if !ext_ids.insert(e.backend.id().to_string()) {
                return bad(format!("duplicate external RAG id `{}`", e.backend.id()));
            }
        }
        for (alias, cfg) in &self.secret_providers {
            if !valid_provider_id(alias) {
                return bad(format!("secret provider alias `{alias}` is invalid"));
            }
            let (addr, token_ref) = match cfg {
                SecretProviderCfg::Hashicorp { addr, token_ref, .. } | SecretProviderCfg::Onepassword { addr, token_ref } => (addr, token_ref),
            };
            iyakki_sec::url_host(addr).ok_or_else(|| SettingsError::Invalid(format!("secret provider `{alias}`: bad address")))?;
            if token_ref.starts_with("ext:") || !["vault:", "env:", "file:"].iter().any(|p| token_ref.starts_with(p)) {
                return bad(format!("secret provider `{alias}`: the access token reference must be vault:, env: or file: (not another provider)"));
            }
        }
        if self.token_price_cents_per_mtok > 100_000 {
            return bad("token price must be at most 100000 cents ($1000) per million tokens");
        }
        if self.tool_log_days > crate::toollog::MAX_DAYS {
            return bad(format!("the tool log can be kept for at most {} days", crate::toollog::MAX_DAYS));
        }
        self.agents.validate().map_err(SettingsError::Invalid)?;
        self.voice.validate().map_err(SettingsError::Invalid)?;
        self.sound.validate().map_err(|e| SettingsError::Invalid(format!("sound: {e}")))?;
        self.sounds.validate().map_err(|e| SettingsError::Invalid(format!("system sounds: {e}")))?;
        self.keyboard.validate().map_err(|e| SettingsError::Invalid(format!("keyboard: {e}")))?;
        if let Some(k) = &self.vault.keyfile {
            if !k.starts_with('/') {
                return bad("vault key file needs an absolute path");
            }
        }
        Ok(())
    }
}

pub struct SettingsStore {
    path: PathBuf,
    current: Settings,
}

impl SettingsStore {
    pub fn open(path: &Path) -> Result<SettingsStore, SettingsError> {
        let current = match std::fs::read_to_string(path) {
            Ok(t) => {
                let s: Settings = serde_json::from_str(&t).map_err(|e| SettingsError::Storage(format!("corrupt settings file: {e}")))?;
                s.validate()?;
                s
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Settings::default(),
            Err(e) => return Err(SettingsError::Storage(e.to_string())),
        };
        Ok(SettingsStore { path: path.into(), current })
    }

    pub fn get(&self) -> &Settings {
        &self.current
    }

    fn save(&self) -> Result<(), SettingsError> {
        use std::io::Write;
        use std::os::unix::fs::OpenOptionsExt;
        let io = |e: std::io::Error| SettingsError::Storage(e.to_string());
        if let Some(d) = self.path.parent() {
            std::fs::create_dir_all(d).map_err(io)?;
        }
        let tmp = self.path.with_extension("tmp");
        let json = serde_json::to_vec_pretty(&self.current).map_err(|e| SettingsError::Storage(e.to_string()))?;
        let mut f = std::fs::OpenOptions::new().write(true).create(true).truncate(true).mode(0o600).open(&tmp).map_err(io)?;
        f.write_all(&json).and_then(|_| f.sync_all()).map_err(io)?;
        std::fs::rename(&tmp, &self.path).map_err(io)
    }

    /// Apply a change atomically: it takes effect only if the result validates and saves.
    pub fn update(&mut self, f: impl FnOnce(&mut Settings)) -> Result<(), SettingsError> {
        let mut next = self.current.clone();
        f(&mut next);
        next.validate()?;
        let prev = std::mem::replace(&mut self.current, next);
        if let Err(e) = self.save() {
            self.current = prev;
            return Err(e);
        }
        Ok(())
    }

    pub fn upsert_provider(&mut self, p: ProviderCfg) -> Result<(), SettingsError> {
        self.update(|s| match s.providers.iter_mut().find(|x| x.id == p.id) {
            Some(slot) => *slot = p,
            None => s.providers.push(p),
        })
    }

    /// Removing a provider also clears chat/embedding choices that pointed at it.
    pub fn remove_provider(&mut self, id: &str) -> Result<(), SettingsError> {
        if self.current.provider(id).is_none() {
            return bad(format!("no provider `{id}`"));
        }
        self.update(|s| {
            s.providers.retain(|p| p.id != id);
            if s.chat.as_ref().is_some_and(|c| c.provider == id) {
                s.chat = None;
            }
            if s.embedding.as_ref().is_some_and(|c| c.provider == id) {
                s.embedding = None;
            }
        })
    }

    pub fn set_chat(&mut self, c: Option<ModelChoice>) -> Result<(), SettingsError> {
        self.update(|s| s.chat = c)
    }

    pub fn set_embedding(&mut self, c: Option<ModelChoice>) -> Result<(), SettingsError> {
        self.update(|s| s.embedding = c)
    }

    pub fn upsert_external(&mut self, e: ExtEntry) -> Result<(), SettingsError> {
        self.update(|s| match s.external_rag.iter_mut().find(|x| x.backend.id() == e.backend.id()) {
            Some(slot) => *slot = e,
            None => s.external_rag.push(e),
        })
    }

    pub fn remove_external(&mut self, id: &str) -> Result<(), SettingsError> {
        self.update(|s| s.external_rag.retain(|e| e.backend.id() != id))
    }

    pub fn upsert_secret_provider(&mut self, alias: &str, cfg: SecretProviderCfg) -> Result<(), SettingsError> {
        self.update(|s| {
            s.secret_providers.insert(alias.into(), cfg);
        })
    }

    pub fn remove_secret_provider(&mut self, alias: &str) -> Result<(), SettingsError> {
        self.update(|s| {
            s.secret_providers.remove(alias);
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::providers::Kind;

    fn tmp(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("iyakki-settings-{}-{}", std::process::id(), tag));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d.join("settings.json")
    }

    fn ollama(id: &str) -> ProviderCfg {
        ProviderCfg::new(id, Kind::Ollama)
    }

    fn openai() -> ProviderCfg {
        let mut p = ProviderCfg::new("openai", Kind::OpenAi);
        p.key_ref = Some("vault:openai-key".into());
        p
    }

    #[test]
    fn defaults_are_valid_and_empty_file_is_fine() {
        assert!(Settings::default().validate().is_ok());
        let s = SettingsStore::open(&tmp("empty")).unwrap();
        assert!(s.get().providers.is_empty() && s.get().chat.is_none());
    }

    #[test]
    fn providers_models_and_persistence() {
        let p = tmp("persist");
        let mut s = SettingsStore::open(&p).unwrap();
        s.upsert_provider(ollama("local")).unwrap();
        s.upsert_provider(openai()).unwrap();
        s.set_chat(Some(ModelChoice { provider: "local".into(), model: "qwen3:32b".into() })).unwrap();
        s.set_embedding(Some(ModelChoice { provider: "openai".into(), model: "text-embedding-3-small".into() })).unwrap();
        // survives a restart, and the file is private and secret-free
        let again = SettingsStore::open(&p).unwrap();
        assert_eq!(again.get(), s.get());
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(std::fs::metadata(&p).unwrap().permissions().mode() & 0o777, 0o600);
        let text = std::fs::read_to_string(&p).unwrap();
        assert!(text.contains("vault:openai-key"), "only the reference is stored");
        // replacing a provider keeps one entry
        let mut o2 = ollama("local");
        o2.base_url = Some("http://10.0.0.5:11434".into());
        s.upsert_provider(o2).unwrap();
        assert_eq!(s.get().providers.len(), 2);
    }

    #[test]
    fn invalid_changes_are_rejected_and_leave_state_untouched() {
        let mut s = SettingsStore::open(&tmp("invalid")).unwrap();
        s.upsert_provider(ollama("local")).unwrap();
        let before = s.get().clone();
        // unknown provider, bad model, disabled provider
        assert!(s.set_chat(Some(ModelChoice { provider: "ghost".into(), model: "m".into() })).is_err());
        assert!(s.set_chat(Some(ModelChoice { provider: "local".into(), model: "bad model?".into() })).is_err());
        let mut off = ollama("off");
        off.enabled = false;
        s.upsert_provider(off).unwrap();
        assert!(s.set_chat(Some(ModelChoice { provider: "off".into(), model: "m".into() })).is_err());
        // a raw API key pasted where a reference belongs
        let mut leaky = ProviderCfg::new("leaky", Kind::OpenAi);
        leaky.key_ref = Some("sk-live-abcdef".into());
        assert!(s.upsert_provider(leaky).unwrap_err().to_string().contains("must start with"));
        // missing key reference for a provider that needs one
        assert!(s.upsert_provider(ProviderCfg::new("nokey", Kind::Anthropic)).is_err());
        // RAG bounds
        assert!(s.update(|x| x.rag.chunk_max = 10).is_err());
        assert!(s.update(|x| x.rag.overlap = 400).is_err());
        assert!(s.update(|x| x.rag.top_k = 0).is_err());
        assert_eq!(s.get().providers.len(), 2);
        assert_eq!(s.get().chat, before.chat);
    }

    #[test]
    fn removing_a_provider_clears_dependent_choices() {
        let mut s = SettingsStore::open(&tmp("remove")).unwrap();
        s.upsert_provider(ollama("local")).unwrap();
        s.set_chat(Some(ModelChoice { provider: "local".into(), model: "m".into() })).unwrap();
        s.remove_provider("local").unwrap();
        assert!(s.get().providers.is_empty() && s.get().chat.is_none());
        assert!(s.remove_provider("local").is_err());
    }

    #[test]
    fn external_rag_and_secret_providers() {
        let mut s = SettingsStore::open(&tmp("ext")).unwrap();
        s.upsert_external(ExtEntry { enabled: true, backend: ExternalRag::Qdrant { id: "q".into(), url: "http://q:6333".into(), collection: "docs".into(), key_ref: None } }).unwrap();
        s.upsert_external(ExtEntry { enabled: false, backend: ExternalRag::Qdrant { id: "q".into(), url: "http://q:6333".into(), collection: "docs2".into(), key_ref: None } }).unwrap();
        assert_eq!(s.get().external_rag.len(), 1, "same id replaces");
        assert!(!s.get().external_rag[0].enabled);
        assert!(s.upsert_external(ExtEntry { enabled: true, backend: ExternalRag::Http { id: "bad id".into(), url: "http://x".into(), key_ref: None } }).is_err());
        s.remove_external("q").unwrap();
        assert!(s.get().external_rag.is_empty());
        let hv = SecretProviderCfg::Hashicorp { addr: "https://vault:8200".into(), mount: "secret".into(), namespace: None, token_ref: "env:VAULT_TOKEN".into() };
        s.upsert_secret_provider("corp", hv).unwrap();
        // a provider's own token may not come from another external provider
        let chained = SecretProviderCfg::Onepassword { addr: "https://op".into(), token_ref: "ext:corp/x/y".into() };
        assert!(s.upsert_secret_provider("op", chained).is_err());
        let raw = SecretProviderCfg::Onepassword { addr: "https://op".into(), token_ref: "eyJraw-token".into() };
        assert!(s.upsert_secret_provider("op", raw).is_err(), "raw tokens are refused");
        assert!(s.update(|x| x.vault.keyfile = Some("relative/key".into())).is_err());
        s.remove_secret_provider("corp").unwrap();
    }

    #[test]
    fn voice_settings_validate_and_persist() {
        use crate::voice::SpeechEngine;
        let p = tmp("voice");
        let mut s = SettingsStore::open(&p).unwrap();
        assert!(s.get().voice.stt.is_none() && s.get().voice.record_cmd.starts_with("arecord"));
        s.update(|x| {
            x.voice.stt = Some(SpeechEngine::Http { url: "http://127.0.0.1:8080/v1/audio/transcriptions".into(), model: "tiny.en".into(), key_ref: None, voice: None, language: Some("en".into()) });
            x.voice.tts = Some(SpeechEngine::Command { command: "piper -m voice.onnx -f {out}".into() });
            x.voice.speak_replies = true;
        })
        .unwrap();
        assert_eq!(SettingsStore::open(&p).unwrap().get().voice, s.get().voice);
        // invalid engines and a raw key are refused, leaving the saved settings alone
        assert!(s.update(|x| x.voice.stt = Some(SpeechEngine::Http { url: "ftp://x".into(), model: "m".into(), key_ref: None, voice: None, language: None })).is_err());
        assert!(s.update(|x| x.voice.tts = Some(SpeechEngine::Http { url: "http://x".into(), model: "m".into(), key_ref: Some("sk-raw".into()), voice: None, language: None })).is_err());
        assert!(s.update(|x| x.voice.record_cmd = "".into()).is_err());
        assert!(s.get().voice.stt.is_some());
    }

    #[test]
    fn corrupt_or_invalid_files_fail_loudly() {
        let p = tmp("corrupt");
        std::fs::write(&p, "not json").unwrap();
        assert!(matches!(SettingsStore::open(&p), Err(SettingsError::Storage(_))));
        std::fs::write(&p, r#"{"chat":{"provider":"ghost","model":"m"}}"#).unwrap();
        assert!(matches!(SettingsStore::open(&p), Err(SettingsError::Invalid(_))), "a hand-edited file that breaks the rules is not silently accepted");
        // unknown future fields don't break loading
        std::fs::write(&p, r#"{"providers":[],"some_future_field":1}"#).unwrap();
        assert!(SettingsStore::open(&p).is_ok());
    }
}
