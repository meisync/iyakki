//! Adding a found model server to Settings -> Models (NET SCAN): the rules, behind their own port.
//!
//! [`ModelsPort`] is what the writing needs of the rest of the system: whether the account may change settings, the audit line, the system vault,
//! the provider list and the chat-model choice, and Lemonade's typed model list. The real port ([`CxModels`]) is the only place that reads the
//! session, the vault and the settings; the rules ([`add_model_server`], [`add_lemonade`]) run over any port, so a fake port tests them
//! without a vault, a settings file or a network.

use std::net::Ipv4Addr;

use iyakki_shared::providers::{valid_provider_id, Kind as ProviderKind, ProviderCfg};
use iyakki_shared::settings::ModelChoice;
use iyakki_tile_scan::is_lemonade;
use iyakki_tile_api::scan::{Kind, Service};

use super::AppCx;

/// What Lemonade's typed list says: the models that chat (downloaded ones only), and how many models it holds in all.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct LemonadeList {
    pub chats: Vec<String>,
    pub total: usize,
}

pub trait ModelsPort {
    /// The account may change settings (administrators).
    fn manage_settings(&self) -> bool;
    /// A line in the audit log for the signed-in person.
    fn audit(&self, action: &str, subject: &str);
    /// Keep a secret in the system vault; the text says what to do when the vault is locked.
    fn vault_set(&self, name: &str, value: &str) -> Result<(), String>;
    /// Add or replace a provider.
    fn upsert_provider(&self, cfg: ProviderCfg) -> Result<(), String>;
    /// Make `provider`/`model` the chat model when none is chosen yet; true when it was made.
    fn adopt_chat_if_none(&self, provider: &str, model: &str) -> Result<bool, String>;
    /// Read Lemonade's typed model list from the provider just added.
    fn refresh_lemonade(&self, id: &str) -> LemonadeList;
}

/// The real port, over the shell's context.
pub struct CxModels<'a>(pub &'a AppCx);

impl ModelsPort for CxModels<'_> {
    fn manage_settings(&self) -> bool {
        self.0.session.policy.manage_settings
    }
    fn audit(&self, action: &str, subject: &str) {
        self.0.audit.record(&self.0.session.user, action, subject);
    }
    fn vault_set(&self, name: &str, value: &str) -> Result<(), String> {
        self.0.vault().system_set(name, value).map_err(|e| match e {
            iyakki_shared::vault::VaultError::Locked => "The system vault is locked. Sign in as an administrator who has access to it.".to_string(),
            other => other.to_string(),
        })
    }
    fn upsert_provider(&self, cfg: ProviderCfg) -> Result<(), String> {
        self.0.settings().lock().unwrap_or_else(std::sync::PoisonError::into_inner).upsert_provider(cfg).map_err(|e| e.to_string())
    }
    fn adopt_chat_if_none(&self, provider: &str, model: &str) -> Result<bool, String> {
        let mut st = self.0.settings().lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        if st.get().chat.is_some() {
            return Ok(false);
        }
        st.set_chat(Some(ModelChoice { provider: provider.to_string(), model: model.to_string() })).map_err(|e| e.to_string())?;
        Ok(true)
    }
    fn refresh_lemonade(&self, id: &str) -> LemonadeList {
        let list = self.0.services.refresh_lemonade(id).unwrap_or_default();
        LemonadeList { chats: list.iter().filter(|m| m.downloaded && m.kind.chats()).map(|m| m.id.clone()).collect(), total: list.len() }
    }
}

/// Add a Lemonade server from the scan: its typed list is read at once so the chat list holds its chat models only, and the
/// model chosen in the form becomes the chat model when none is set and it is one of those.
pub fn add_lemonade(port: &dyn ModelsPort, ip: Ipv4Addr, mut cfg: ProviderCfg, model: &str, key: &str) -> Result<String, String> {
    let id = cfg.id.clone();
    let key_name = format!("{id}-key");
    if !key.is_empty() {
        cfg.key_ref = Some(format!("vault:{key_name}"));
    }
    cfg.validate()?;
    if !key.is_empty() {
        port.vault_set(&key_name, key)?;
        port.audit("vault.system.set", &key_name);
    }
    port.upsert_provider(cfg)?;
    port.audit("settings.provider.save", &format!("{id} (Lemonade) from net scan {ip}"));
    let list = port.refresh_lemonade(&id);
    let pick = if list.chats.iter().any(|m| m == model) { Some(model.to_string()) } else { list.chats.first().cloned() };
    let mut note = String::new();
    if let Some(m) = pick {
        if port.adopt_chat_if_none(&id, &m)? {
            note = format!(" It is now the chat model ({m}).");
        }
    }
    let (chats, others) = (list.chats.len(), list.total - list.chats.len());
    Ok(format!("Added {id} (Lemonade) to Models with {chats} chat model{} and {others} other{} (pictures, speech, embeddings: see Settings → Models).{note}", if chats == 1 { "" } else { "s" }, if others == 1 { "" } else { "s" }))
}

/// Add a found model server to Settings → Models. The API key, if there is one, goes into the system vault and the provider
/// keeps only a reference to it. If no chat model is chosen yet, this one becomes it. Returns what to tell the person.
pub fn add_model_server(port: &dyn ModelsPort, ip: Ipv4Addr, svc: &Service, id: &str, address: &str, model: &str, key: &str) -> Result<String, String> {
    if !port.manage_settings() {
        port.audit("settings.denied", "netscan add model");
        return Err("Only an administrator can add models.".into());
    }
    if !valid_provider_id(id) {
        return Err("Provider name: lowercase letters, digits, - and _; it starts with a letter.".into());
    }
    let kind = if svc.kind == Kind::Ollama { ProviderKind::Ollama } else if is_lemonade(svc) { ProviderKind::Lemonade } else { ProviderKind::Compat };
    let mut cfg = ProviderCfg::new(id, kind);
    cfg.base_url = Some(address.trim().to_string());
    if kind == ProviderKind::Lemonade {
        // Lemonade's list says what each model does: only its chat models go to the chat list (read after it is added)
        return add_lemonade(port, ip, cfg, model, key);
    }
    cfg.models = svc.models.clone();
    cfg.default_model = if model.is_empty() { svc.models.first().cloned() } else { Some(model.to_string()) };
    let key_name = format!("{id}-key");
    if !key.is_empty() {
        cfg.key_ref = Some(format!("vault:{key_name}"));
    }
    cfg.validate()?; // before touching the vault: a rejected form stores nothing
    if !key.is_empty() {
        port.vault_set(&key_name, key)?;
        port.audit("vault.system.set", &key_name);
    }
    let chosen = cfg.default_model.clone();
    port.upsert_provider(cfg)?;
    let mut note = String::new();
    if let Some(m) = &chosen {
        if port.adopt_chat_if_none(id, m)? {
            note = format!(" It is now the chat model ({m}).");
        }
    }
    port.audit("settings.provider.save", &format!("{id} ({}) from net scan {ip}", kind.label()));
    Ok(format!("Added {id} to Models{}.{note}", if key.is_empty() { String::new() } else { " and its key to the vault".to_string() }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    #[derive(Default)]
    struct Fake {
        admin: bool,
        vault: Mutex<Vec<(String, String)>>,
        vault_error: Option<String>,
        providers: Mutex<Vec<ProviderCfg>>,
        chat: Mutex<Option<(String, String)>>,
        audit: Mutex<Vec<String>>,
        lemonade: LemonadeList,
    }

    impl Fake {
        fn admin() -> Fake {
            Fake { admin: true, ..Default::default() }
        }
        fn audit_text(&self) -> String {
            self.audit.lock().unwrap().join("\n")
        }
    }

    impl ModelsPort for Fake {
        fn manage_settings(&self) -> bool {
            self.admin
        }
        fn audit(&self, action: &str, subject: &str) {
            self.audit.lock().unwrap().push(format!("{action} {subject}"));
        }
        fn vault_set(&self, name: &str, value: &str) -> Result<(), String> {
            if let Some(e) = &self.vault_error {
                return Err(e.clone());
            }
            self.vault.lock().unwrap().push((name.into(), value.into()));
            Ok(())
        }
        fn upsert_provider(&self, cfg: ProviderCfg) -> Result<(), String> {
            self.providers.lock().unwrap().push(cfg);
            Ok(())
        }
        fn adopt_chat_if_none(&self, provider: &str, model: &str) -> Result<bool, String> {
            let mut c = self.chat.lock().unwrap();
            if c.is_some() {
                return Ok(false);
            }
            *c = Some((provider.into(), model.into()));
            Ok(true)
        }
        fn refresh_lemonade(&self, _id: &str) -> LemonadeList {
            self.lemonade.clone()
        }
    }

    fn ollama() -> Service {
        let mut s = Service::new(11434, Kind::Ollama, "Ollama 0.5.7");
        s.models = vec!["llama3.2:3b".into(), "qwen3:32b".into()];
        s
    }

    fn ip() -> Ipv4Addr {
        Ipv4Addr::new(192, 168, 1, 50)
    }

    #[test]
    fn an_ordinary_account_is_refused_and_the_refusal_is_audited_and_nothing_is_written() {
        let f = Fake::default();
        let e = add_model_server(&f, ip(), &ollama(), "ollama-1", "http://192.168.1.50:11434", "", "").unwrap_err();
        assert!(e.contains("administrator"), "{e}");
        assert!(f.audit_text().contains("settings.denied netscan add model"));
        assert!(f.providers.lock().unwrap().is_empty() && f.vault.lock().unwrap().is_empty());
    }

    #[test]
    fn a_bad_provider_name_is_refused_before_anything_is_stored() {
        let f = Fake::admin();
        let e = add_model_server(&f, ip(), &ollama(), "Bad Name!", "http://h:1", "", "tok").unwrap_err();
        assert!(e.contains("Provider name"), "{e}");
        assert!(f.providers.lock().unwrap().is_empty() && f.vault.lock().unwrap().is_empty());
    }

    #[test]
    fn an_ollama_is_added_with_its_models_a_key_in_the_vault_only_as_a_reference_and_becomes_the_chat_model() {
        let f = Fake::admin();
        let msg = add_model_server(&f, ip(), &ollama(), "ollama-1", "  http://192.168.1.50:11434 ", "qwen3:32b", "tok-123").unwrap();
        assert!(msg.contains("Added ollama-1") && msg.contains("vault") && msg.contains("(qwen3:32b)"), "{msg}");
        let p = f.providers.lock().unwrap();
        assert_eq!((p[0].kind, p[0].base_url.as_deref(), p[0].default_model.as_deref()), (ProviderKind::Ollama, Some("http://192.168.1.50:11434"), Some("qwen3:32b")));
        assert_eq!(p[0].key_ref.as_deref(), Some("vault:ollama-1-key"), "the provider holds a reference, not the key");
        assert_eq!(f.vault.lock().unwrap().as_slice(), [("ollama-1-key".to_string(), "tok-123".to_string())]);
        assert_eq!(f.chat.lock().unwrap().clone(), Some(("ollama-1".into(), "qwen3:32b".into())));
        let t = f.audit_text();
        assert!(t.contains("vault.system.set ollama-1-key") && t.contains("settings.provider.save ollama-1 (Ollama)"), "{t}");
    }

    #[test]
    fn a_second_server_does_not_take_over_the_chat_model_and_an_empty_model_takes_the_first_listed() {
        let f = Fake::admin();
        add_model_server(&f, ip(), &ollama(), "first", "http://a:1", "", "").unwrap();
        let msg = add_model_server(&f, ip(), &ollama(), "second", "http://b:1", "", "").unwrap();
        assert!(!msg.contains("chat model"), "{msg}");
        assert_eq!(f.chat.lock().unwrap().clone(), Some(("first".into(), "llama3.2:3b".into())));
        assert_eq!(f.providers.lock().unwrap().len(), 2);
        assert!(f.vault.lock().unwrap().is_empty(), "no key, nothing in the vault");
    }

    #[test]
    fn a_locked_vault_stops_the_add_before_the_provider_is_saved() {
        let mut f = Fake::admin();
        f.vault_error = Some("The system vault is locked.".into());
        let e = add_model_server(&f, ip(), &ollama(), "o1", "http://a:1", "", "k").unwrap_err();
        assert!(e.contains("locked"), "{e}");
        assert!(f.providers.lock().unwrap().is_empty());
    }

    #[test]
    fn an_openai_compatible_server_is_a_compat_provider() {
        let f = Fake::admin();
        let mut svc = Service::new(1234, Kind::LlmServer, "LM Studio");
        svc.models = vec!["qwen2-7b".into()];
        add_model_server(&f, ip(), &svc, "lmstudio", "http://192.168.1.50:1234/v1", "", "").unwrap();
        let p = f.providers.lock().unwrap();
        assert_eq!((p[0].kind, p[0].default_model.as_deref()), (ProviderKind::Compat, Some("qwen2-7b")));
        assert!(p[0].key_ref.is_none());
    }

    #[test]
    fn a_lemonade_puts_only_its_chat_models_in_the_chat_list_and_the_chosen_chat_model_wins_when_it_is_one() {
        let mut f = Fake::admin();
        f.lemonade = LemonadeList { chats: vec!["Qwen3-0.6B-GGUF".into(), "Phi-4-Multimodal-GGUF".into()], total: 4 };
        let svc = Service::new(8000, Kind::LlmServer, "Lemonade Server 2026.42.1");
        let msg = add_model_server(&f, ip(), &svc, "lemonade-1", "http://192.168.1.50:8000/api/v1", "Phi-4-Multimodal-GGUF", "").unwrap();
        assert!(msg.contains("(Lemonade)") && msg.contains("2 chat models") && msg.contains("2 others") && msg.contains("(Phi-4-Multimodal-GGUF)"), "{msg}");
        assert_eq!(f.providers.lock().unwrap()[0].kind, ProviderKind::Lemonade);
        assert_eq!(f.chat.lock().unwrap().clone(), Some(("lemonade-1".into(), "Phi-4-Multimodal-GGUF".into())));
        assert!(f.audit_text().contains("settings.provider.save lemonade-1 (Lemonade) from net scan 192.168.1.50"));
    }

    #[test]
    fn a_lemonade_chat_model_that_is_not_in_its_list_falls_back_to_its_first_chat_model_and_an_empty_list_picks_none() {
        let mut f = Fake::admin();
        f.lemonade = LemonadeList { chats: vec!["Qwen3-0.6B-GGUF".into()], total: 3 };
        let svc = Service::new(8000, Kind::LlmServer, "Lemonade Server 1");
        let msg = add_model_server(&f, ip(), &svc, "lem", "http://h:8000/api/v1", "Whisper-Base", "").unwrap();
        assert!(msg.contains("1 chat model ") && msg.contains("2 others"), "{msg}");
        assert_eq!(f.chat.lock().unwrap().clone(), Some(("lem".into(), "Qwen3-0.6B-GGUF".into())));
        let g = Fake::admin();
        let msg = add_model_server(&g, ip(), &svc, "lem", "http://h:8000/api/v1", "", "").unwrap();
        assert!(msg.contains("0 chat models") && g.chat.lock().unwrap().is_none(), "{msg}");
    }

    #[test]
    fn a_lemonade_key_goes_to_the_vault_before_the_provider() {
        let f = Fake::admin();
        let svc = Service::new(8000, Kind::LlmServer, "Lemonade Server 1");
        add_model_server(&f, ip(), &svc, "lem", "http://h:8000/api/v1", "", "secret").unwrap();
        assert_eq!(f.vault.lock().unwrap().len(), 1);
        assert_eq!(f.providers.lock().unwrap()[0].key_ref.as_deref(), Some("vault:lem-key"));
    }
}
