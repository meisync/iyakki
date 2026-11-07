//! The machine's accounts and audit log as the web service's traits, so the desktop's browser server can answer `/api/v1/...`
//! (`iyakki_agent::webservice::embed`). The same adapters as `iyakki serve --profile` uses; they only talk to the account store and
//! the audit log, never to a kernel interface.

use iyakki_agent::webservice::embed::Embedded;
use iyakki_core_api::{ApiError, ApiResult, AuditSink, Authenticator, Principal};
use iyakki_shell::accounts::{AccountError, AuditLog, UserStore};
use iyakki_shell::shell::ShellEnv;
use std::sync::{Arc, Mutex};

pub struct ShellAuth(pub Arc<Mutex<UserStore>>);

fn principal(name: &str, role: iyakki_sec::policy::Role, policy: iyakki_sec::policy::Policy) -> Principal {
    Principal { user: name.to_string(), role: role.name().to_string(), admin: role == iyakki_sec::policy::Role::Admin, policy }
}

impl Authenticator for ShellAuth {
    fn login(&self, name: &str, password: &str) -> ApiResult<Principal> {
        let mut s = self.0.lock().unwrap_or_else(|e| e.into_inner());
        match s.authenticate(name, password) {
            Ok(sess) => Ok(principal(&sess.user, sess.role, sess.policy)),
            Err(AccountError::Throttled(n)) => Err(ApiError::Busy(format!("too many attempts; try again in {n} s"))),
            Err(_) => Err(ApiError::Denied("invalid username or password".into())),
        }
    }

    fn lookup(&self, user: &str) -> Option<Principal> {
        let mut s = self.0.lock().unwrap_or_else(|e| e.into_inner());
        s.refresh();
        s.get(user).map(|u| principal(&u.name, u.role, u.policy()))
    }
}

pub struct ShellAudit(pub Arc<AuditLog>);

impl AuditSink for ShellAudit {
    fn record(&self, user: &str, event: &str, detail: &str) {
        self.0.record(user, event, detail);
    }
}

/// The API for this machine's shell environment (none when it cannot be set up; the page then gets 404 for `/api/v1/`).
pub fn embedded(env: &ShellEnv) -> Option<Embedded> {
    let deps = iyakki_agent::webservice::real::assemble(Arc::new(ShellAuth(env.store.clone())), Arc::new(ShellAudit(env.audit.clone())), env.services.clone(), env.host.clone());
    match Embedded::new(deps) {
        Ok(e) => Some(e),
        Err(e) => {
            log::warn!("web: the API for the page at /ui/ is not available: {e}");
            None
        }
    }
}
