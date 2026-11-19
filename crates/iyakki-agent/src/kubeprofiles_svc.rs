//! The vault resolver of a person for kubeconfigs held in a vault (the part of `kubeprofiles` that needs the services).

use std::sync::Arc;

use iyakki_shared::kubeprofiles::Resolver;

/// The resolver for one person: `uvault:` reads their personal vault, `vault:` the system vault.
pub fn resolver_for(svc: &Arc<crate::services::Services>, user: &str) -> Resolver {
    let (svc, user) = (svc.clone(), user.to_string());
    Arc::new(move |r: &str| {
        if let Some(name) = r.strip_prefix("uvault:") {
            svc.user_get(&user, name).map(|z| z.to_string()).map_err(|e| format!("your vault: {e}"))
        } else {
            svc.secrets.resolve(r, None).map(|z| z.to_string()).map_err(|e| format!("the system vault: {e}"))
        }
    })
}

/// A rollout of the cluster explorer is waiting work in the ledger while it goes on (`iyakki_shared::k8sexp::run_action_reported`).
impl iyakki_shared::k8sexp::WorkReport for crate::pending::Ledger {
    fn rollout(&self, key: &str, title: &str) -> Box<dyn FnOnce(bool, &str)> {
        let guard = crate::pendingplus::WorkGuard::start(self, crate::pending::Source::Rollout, key, title, "");
        Box::new(move |ok, words| guard.finish(ok, words))
    }
}
