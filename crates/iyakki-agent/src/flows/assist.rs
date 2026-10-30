//! FLOWS assist, the agent's side: the account's suggestion store and the periodic pass that lets the configured agent look at
//! failed runs and the versions it published. The decisions are in `iyakki_flow::assist` (pure) and `iyakki_flow::assist_run`; this
//! only wires them to the account's store, engine and host (the model, the audit log and the bell are the host's). See
//! `docs/FLOWS.md` ("Assist: copilot and autopilot") and `docs/development/FLOW-AUTOPILOT-PLAN.md`.

use std::sync::{Arc, Weak};
use std::time::Duration;

use iyakki_flow::assist_run::{Assistant, EngineTrial};
use iyakki_flow::assist_store::{AssistStore, Suggestion};

use super::FlowEnv;

/// How often the unattended pass looks (seconds). Flows with assist off cost one small file read each.
const TICK_SECS: u64 = 60;

impl FlowEnv {
    /// The suggestions and counters of this account's flows.
    pub fn assist_store(&self) -> AssistStore {
        AssistStore::new(&self.dir, Arc::new(super::now_ms))
    }

    /// Run `f` with an [`Assistant`] on this account's store, engine and host.
    pub fn with_assistant<R>(&self, f: impl FnOnce(&Assistant) -> R) -> R {
        let book = self.assist_store();
        let reg = self.engine.registry();
        let trial = EngineTrial { engine: &self.engine, reg: &reg };
        let a = Assistant { store: &self.store, book: &book, host: self.host.as_ref(), reg: &reg, trial: &trial };
        f(&a)
    }

    /// One pass over the account's flows: failed runs get a suggestion (or, in autopilot within its limits, a published fix) and the
    /// versions autopilot published are watched. Returns a line per thing that happened.
    pub fn assist_tick(&self) -> Vec<String> {
        let runs = self.engine.summaries();
        let engine = self.engine.clone();
        self.with_assistant(|a| a.tick(&runs, &move |id| engine.get_run(id)))
    }

    /// A version was published by the person (the screen or the assistant, after they approved): tell assist, which marks a loaded
    /// suggestion approved and retires the ones that are out of date.
    pub fn assist_after_publish(&self, flow: &str, version: u32) {
        let who = self.who();
        self.with_assistant(|a| a.after_publish(flow, version, &who));
    }

    /// The suggestions of a flow still waiting for the person (newest first).
    pub fn open_suggestions(&self, flow: &str) -> Vec<Suggestion> {
        self.assist_store().open_list(flow)
    }
}

/// The unattended pass for the life of the account's environment (it ends when the environment is dropped at sign-out).
pub fn spawn_ticker(env: &Arc<FlowEnv>) {
    let weak: Weak<FlowEnv> = Arc::downgrade(env);
    let _ = std::thread::Builder::new().name("flow-assist".into()).spawn(move || loop {
        std::thread::sleep(Duration::from_secs(TICK_SECS));
        let Some(env) = weak.upgrade() else { return };
        if !env.engine.ticking() {
            return;
        }
        let lines = env.assist_tick();
        if !lines.is_empty() {
            iyakki_flow::host::Host::audit(env.host.as_ref(), "flow.assist.pass", &lines.join(" | ").chars().take(300).collect::<String>());
        }
    });
}
