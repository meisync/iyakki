//! The real implementation of the core contract's first domains (settings, vault, flows, consent, streams) over the services that
//! exist today: the settings store, the personal vault and the flows engine. Same traits, same guards, same audit lines and event
//! rules as the in-memory fakes of `iyakki-core-fake`; one conformance suite (`conformance_tests.rs`) runs against both.
//!
//! Nothing here opens a port or a socket. A transport (`devmode::core` today) calls these adapters with a [`CallContext`] it built
//! from the paired computer; a tile calls them with a local one. See docs/development/CORE-CONTRACT.md.
//!
//! The vault rule holds here too: no function in this module returns a secret value; `vault.use_secret` hands the secret to a
//! registered action and returns only the words the action gave back (with the secret scrubbed out if it ever appeared).

use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock};

use iyakki_core_api::contract::*;

use crate::services::Services;

pub mod consent;
pub mod flows;
pub mod settings;
pub mod streams;
pub mod vault;

#[cfg(test)]
mod conformance_tests;

pub use consent::CoreConsents;
pub use flows::CoreFlows;
pub use settings::CoreSettings;
pub use streams::CoreStreams;
pub use vault::CoreVault;

/// Milliseconds since the epoch (event times; the sequence number, not the clock, orders events).
pub(crate) fn now_ms() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis() as u64).unwrap_or(0)
}

const AUDIT_KEEP: usize = 2000;

/// The audit lines of one adapter: kept in memory (bounded) and handed to an optional sink (the system audit log) as they are made.
/// A line names who, over what, which call and the outcome; never a value, a password or a secret.
#[derive(Default)]
pub struct Audit {
    lines: Mutex<Vec<String>>,
    sink: Mutex<Option<Arc<dyn Fn(&str) + Send + Sync>>>,
}

impl Audit {
    pub fn set_sink(&self, f: Arc<dyn Fn(&str) + Send + Sync>) {
        *self.sink.lock().unwrap_or_else(|e| e.into_inner()) = Some(f);
    }
    pub fn line(&self, ctx: &CallContext, call: &str, risk: Risk, outcome: &str) {
        self.raw(ctx.audit_line(call, risk, outcome));
    }
    pub(crate) fn raw(&self, line: String) {
        if let Some(s) = self.sink.lock().unwrap_or_else(|e| e.into_inner()).clone() {
            s(&line);
        }
        let mut g = self.lines.lock().unwrap_or_else(|e| e.into_inner());
        g.push(line);
        if g.len() > AUDIT_KEEP {
            let drop_n = g.len() - AUDIT_KEEP;
            g.drain(..drop_n);
        }
    }
    pub fn all(&self) -> Vec<String> {
        self.lines.lock().unwrap_or_else(|e| e.into_inner()).clone()
    }
}

/// Run the guard and write the refusal to the audit log when it says no.
pub(crate) fn guarded(audit: &Audit, ctx: &CallContext, call: &str, scope: &str, risk: Risk) -> CallResult<()> {
    let r = guard(ctx, scope, risk);
    if let Err(e) = &r {
        audit.line(ctx, call, risk, &format!("refused: {}", e.code.as_str()));
    }
    r
}

/// A local call needs a signed-in person; a remote one is checked by its scopes in [`guard`].
pub(crate) fn who(ctx: &CallContext) -> String {
    if ctx.is_local() { format!("user:{}", ctx.identity.user) } else { format!("remote:{}", ctx.identity.user) }
}

/// Everything the first domains need, built on the services of the device. One per `Services`, shared by every transport so the
/// idempotency keys, event logs and stream handles live as long as the process.
pub struct Core {
    pub svc: Arc<Services>,
    pub settings: CoreSettings,
    pub vault: CoreVault,
    pub consents: CoreConsents,
    pub streams: Arc<CoreStreams>,
    flows: Mutex<HashMap<String, Arc<CoreFlows>>>,
}

impl Core {
    pub fn new(svc: Arc<Services>) -> Core {
        Core { settings: CoreSettings::new(svc.settings.clone()), vault: CoreVault::new(svc.clone()), consents: CoreConsents::new(), streams: Arc::new(CoreStreams::new()), flows: Mutex::new(HashMap::new()), svc }
    }

    /// The flows of one account, over the engine that account already has while signed in (`FlowEnv::open`). An account that may not
    /// use FLOWS gets `denied`.
    pub fn flows_for(&self, account: &str, policy: &iyakki_sec::policy::Policy) -> CallResult<Arc<CoreFlows>> {
        let env = crate::flows::FlowEnv::open(&self.svc, account, policy).map_err(|e| CallError::new(ErrorCode::Denied, "flows.not_allowed", &e))?;
        Ok(self.flows_for_env(env))
    }

    /// The same, for an environment the caller already holds (a tile has its own).
    pub fn flows_for_env(&self, env: Arc<crate::flows::FlowEnv>) -> Arc<CoreFlows> {
        let key = format!("{}|{}", env.dir.display(), env.user);
        let mut g = self.flows.lock().unwrap_or_else(|e| e.into_inner());
        g.entry(key).or_insert_with(|| Arc::new(CoreFlows::new(env, self.streams.clone()))).clone()
    }

    /// Send every adapter's audit lines to the system audit log too: `sink(line)`.
    pub fn audit_to(&self, sink: Arc<dyn Fn(&str) + Send + Sync>) {
        self.settings.audit_handle().set_sink(sink.clone());
        self.vault.audit_handle().set_sink(sink.clone());
        self.streams.audit_handle().set_sink(sink);
    }
}

/// The process-wide `Core` of a device (keyed by its data folder), made on first use.
pub fn core_for(svc: &Arc<Services>) -> Arc<Core> {
    static HUB: OnceLock<Mutex<HashMap<String, Arc<Core>>>> = OnceLock::new();
    let key = svc.dir().display().to_string();
    let mut g = HUB.get_or_init(|| Mutex::new(HashMap::new())).lock().unwrap_or_else(|e| e.into_inner());
    g.entry(key).or_insert_with(|| Arc::new(Core::new(svc.clone()))).clone()
}
