//! What the NET SCAN tile asks of the rest of the system (the app standard, docs/development/APP-DEVELOPMENT-PROCEDURE.md section 12).
//!
//! The tile (`scan.rs`) names no `Services`, no session and no audit log: it asks a [`ScanPort`] for the machine's subnets and the scanner
//! (the core's permission-checked `NetScan`), the account's host policy, the new-device watch, the audit line and, for administrators, adding a
//! found model server to Settings -> Models. The real port ([`CxPort`]) is the only place that reads the session, the vault and the settings.

use std::net::Ipv4Addr;
use std::sync::atomic::AtomicBool;
use std::sync::mpsc::Sender;
use std::sync::Arc;

use iyakki_core_api::netscan::{net, Event as ScanEvent, Kind, Options, Service};
use iyakki_shared::providers::{valid_provider_id, Kind as ProviderKind, ProviderCfg};
use iyakki_shared::settings::ModelChoice;

use super::scan::is_lemonade;
use super::AppCx;

/// The services found on a host (the tile names the services through this one place).
pub(super) fn host_services(h: &iyakki_core_api::netscan::HostInfo) -> &[Service] {
    &h.services
}

pub trait ScanPort {
    /// The subnets this machine is on.
    fn subnets(&self) -> Vec<net::Subnet>;
    /// Start a scan of ; refused (nothing is touched) when the permission is not given.
    fn start_scan(&self, targets: Vec<Ipv4Addr>, opts: Options, cancel: Arc<AtomicBool>, tx: Sender<ScanEvent>) -> Result<(), String>;
    /// The account's host policy lets it touch .
    fn allows_host(&self, ip: &str) -> bool;
    /// The account may change settings (administrators).
    fn admin(&self) -> bool;
    /// The new-device watch.
    fn devwatch(&self) -> Arc<dyn iyakki_agent_api::DevWatchApi>;
    /// A line in the audit log for the signed-in person.
    fn audit(&self, action: &str, subject: &str);
    /// Add a found model server to Settings -> Models (administrators); what to tell the person.
    fn add_model_server(&self, ip: Ipv4Addr, svc: &Service, id: &str, address: &str, model: &str, key: &str) -> Result<String, String>;
}

/// The real port, over the shell's context.
pub struct CxPort<'a>(pub &'a AppCx);

impl ScanPort for CxPort<'_> {
    fn subnets(&self) -> Vec<net::Subnet> {
        self.0.os.netscan().subnets()
    }
    fn start_scan(&self, targets: Vec<Ipv4Addr>, opts: Options, cancel: Arc<AtomicBool>, tx: Sender<ScanEvent>) -> Result<(), String> {
        self.0.os.netscan().start(targets, opts, cancel, tx)
    }
    fn allows_host(&self, ip: &str) -> bool {
        self.0.session.policy.allows_host(ip)
    }
    fn admin(&self) -> bool {
        self.0.session.policy.manage_settings
    }
    fn devwatch(&self) -> Arc<dyn iyakki_agent_api::DevWatchApi> {
        self.0.devwatch()
    }
    fn audit(&self, action: &str, subject: &str) {
        self.0.audit.record(&self.0.session.user, action, subject);
    }
    fn add_model_server(&self, ip: Ipv4Addr, svc: &Service, id: &str, address: &str, model: &str, key: &str) -> Result<String, String> {
        add_model_server(self.0, ip, svc, id, address, model, key)
    }
}

/// Add a Lemonade server from the scan: its typed list is read at once so the chat list holds its chat models only, and the
/// model chosen in the form becomes the chat model when none is set and it is one of those.
fn add_lemonade(cx: &AppCx, ip: Ipv4Addr, mut cfg: ProviderCfg, model: &str, key: &str) -> Result<String, String> {
    let id = cfg.id.clone();
    let key_name = format!("{id}-key");
    if !key.is_empty() {
        cfg.key_ref = Some(format!("vault:{key_name}"));
    }
    cfg.validate()?;
    if !key.is_empty() {
        cx.vault().system_set(&key_name, key).map_err(|e| e.to_string())?;
        cx.audit.record(&cx.session.user, "vault.system.set", &key_name);
    }
    cx.settings().lock().unwrap_or_else(std::sync::PoisonError::into_inner).upsert_provider(cfg).map_err(|e| e.to_string())?;
    cx.audit.record(&cx.session.user, "settings.provider.save", &format!("{id} (Lemonade) from net scan {ip}"));
    let list = cx.services.refresh_lemonade(&id).unwrap_or_default();
    let chats: Vec<String> = list.iter().filter(|m| m.downloaded && m.kind.chats()).map(|m| m.id.clone()).collect();
    let mut note = String::new();
    let mut st = cx.settings().lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    let pick = if chats.iter().any(|m| m == model) { Some(model.to_string()) } else { chats.first().cloned() };
    if let (None, Some(m)) = (st.get().chat.clone(), pick) {
        st.set_chat(Some(ModelChoice { provider: id.clone(), model: m.clone() })).map_err(|e| e.to_string())?;
        note = format!(" It is now the chat model ({m}).");
    }
    let others = list.len() - chats.len();
    Ok(format!("Added {id} (Lemonade) to Models with {} chat model{} and {others} other{} (pictures, speech, embeddings: see Settings → Models).{note}", chats.len(), if chats.len() == 1 { "" } else { "s" }, if others == 1 { "" } else { "s" }))
}

/// Add a found model server to Settings → Models. The API key, if there is one, goes into the system vault and the provider
/// keeps only a reference to it. If no chat model is chosen yet, this one becomes it. Returns what to tell the person.
pub(super) fn add_model_server(cx: &AppCx, ip: Ipv4Addr, svc: &Service, id: &str, address: &str, model: &str, key: &str) -> Result<String, String> {
    if !cx.session.policy.manage_settings {
        cx.audit.record(&cx.session.user, "settings.denied", "netscan add model");
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
        return add_lemonade(cx, ip, cfg, model, key);
    }
    cfg.models = svc.models.clone();
    cfg.default_model = if model.is_empty() { svc.models.first().cloned() } else { Some(model.to_string()) };
    let key_name = format!("{id}-key");
    if !key.is_empty() {
        cfg.key_ref = Some(format!("vault:{key_name}"));
    }
    cfg.validate()?; // before touching the vault: a rejected form stores nothing
    if !key.is_empty() {
        cx.vault().system_set(&key_name, key).map_err(|e| match e {
            iyakki_shared::vault::VaultError::Locked => "The system vault is locked. Sign in as an administrator who has access to it.".to_string(),
            other => other.to_string(),
        })?;
        cx.audit.record(&cx.session.user, "vault.system.set", &key_name);
    }
    let chosen = cfg.default_model.clone();
    let mut settings = cx.settings().lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    settings.upsert_provider(cfg).map_err(|e| e.to_string())?;
    let mut note = String::new();
    if settings.get().chat.is_none() {
        if let Some(m) = &chosen {
            settings.set_chat(Some(ModelChoice { provider: id.to_string(), model: m.clone() })).map_err(|e| e.to_string())?;
            note = format!(" It is now the chat model ({m}).");
        }
    }
    drop(settings);
    cx.audit.record(&cx.session.user, "settings.provider.save", &format!("{id} ({}) from net scan {ip}", kind.label()));
    Ok(format!("Added {id} to Models{}.{note}", if key.is_empty() { String::new() } else { " and its key to the vault".to_string() }))
}

