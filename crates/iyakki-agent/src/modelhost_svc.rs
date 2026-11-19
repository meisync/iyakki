//! The parts of `modelhost` that need the services and the cluster handle (the rules and the network code are `iyakki_shared::modelhost`).

use iyakki_shared::modelhost::*;
use iyakki_shared::providers::{Kind, ProviderCfg};
use iyakki_shared::wsexec::Out;

/// Register the host in Models as `mac-<nickname>` (an OpenAI-compatible provider) with its key in the system vault, and keep it in
/// the list of hosts. An existing provider of that name that is not a host of this list is never overwritten. `report` is the
/// result of a successful test (the models and the default one).
pub fn add_to_models(svc: &crate::services::Services, n: &NewHost, report: &TestReport) -> Result<Host, String> {
    let target = n.target()?;
    let id = provider_id(n.preset, &n.nickname)?;
    let hosts = Hosts::new(svc.dir());
    let existing = hosts.get(&id);
    {
        let st = svc.settings.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        if st.get().provider(&id).is_some() && existing.is_none() {
            return Err(format!("A provider named {id} already exists in Models and is not a model host; pick another nickname."));
        }
    }
    let mut cfg = ProviderCfg::new(&id, Kind::Compat);
    cfg.base_url = Some(target.base_url());
    cfg.models = report.models.clone();
    cfg.default_model = n.model.clone().or_else(|| report.default_model.clone());
    if !n.key.is_empty() {
        cfg.key_ref = Some(format!("vault:{id}-key"));
    } else if let Some(old) = svc.settings.lock().unwrap_or_else(std::sync::PoisonError::into_inner).get().provider(&id) {
        cfg.key_ref = old.key_ref.clone(); // saving again without typing the key keeps the stored one
    }
    cfg.validate()?;
    if !n.key.is_empty() {
        svc.system_set(&format!("{id}-key"), &n.key).map_err(|e| match e {
            crate::vault::VaultError::Locked => "The system vault is locked. Sign in as an administrator who has access to it.".to_string(),
            other => other.to_string(),
        })?;
    }
    svc.settings.lock().unwrap_or_else(std::sync::PoisonError::into_inner).upsert_provider(cfg).map_err(|e| e.to_string())?;
    let host = Host { id: id.clone(), nickname: n.nickname.trim().to_string(), preset: n.preset, address: target.host.clone(), port: target.port, in_cluster: existing.as_ref().is_some_and(|h| h.in_cluster), public_ok: n.confirm_public() };
    hosts.upsert(host.clone())?;
    Ok(host)
}

/// Take the host out of Models and the list (the vault key stays until the person deletes it there).
pub fn remove_from_models(svc: &crate::services::Services, id: &str) -> Result<(), String> {
    let hosts = Hosts::new(svc.dir());
    if hosts.get(id).is_none() {
        return Err(format!("{id} is not a model host."));
    }
    let _ = svc.settings.lock().unwrap_or_else(std::sync::PoisonError::into_inner).remove_provider(id);
    hosts.remove(id)
}

impl Kube for crate::k3sdev::K3sDev {
    fn kubectl(&self, args: &[&str]) -> Out {
        crate::k3sdev::K3sDev::kubectl(self, args)
    }
}
