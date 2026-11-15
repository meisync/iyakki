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
        iyakki_flow::host::Host::tick(&*self.host);
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

    /// The ledger of waiting work of this account (the flow, rollout, git and browser producers report to it).
    pub fn ledger(&self) -> crate::pending::Ledger {
        crate::pending::Ledger::open(&self.svc.dir().join("pending"), &self.user)
    }

    /// Report a started run to the ledger: it is waiting work until the run ends, so the assistant that started it (or the person who
    /// asked it to report back) is told once with how it ended (F-AI-16). One small thread per run waits for the end; a run that never
    /// ends is dropped after the longest wait the settings allow.
    pub fn report_run(&self, run_id: &str, flow_id: &str) {
        let flow_name = self.store.get_draft(flow_id).map_or_else(|| flow_id.to_string(), |f| f.name);
        let guard = crate::pendingplus::WorkGuard::start(&self.ledger(), crate::pending::Source::Flow, run_id, &format!("run of the flow {flow_name}"), "");
        let engine = self.engine.clone();
        let (id, max) = (run_id.to_string(), std::time::Duration::from_secs(self.ledger().settings().max_wait_secs.max(60)));
        let spawned = std::thread::Builder::new().name("flow-ledger".into()).spawn(move || {
            match engine.wait_for(&id, max, |r| r.status.is_terminal()) {
                Some(r) if r.status == iyakki_flow::RunStatus::Succeeded => guard.finish(true, "the run succeeded"),
                Some(r) => guard.finish(false, &format!("the run ended {}{}", r.status.label(), if r.error.is_empty() { String::new() } else { format!(": {}", r.error.chars().take(120).collect::<String>()) })),
                None => guard.finish(false, "the run did not end within the longest wait"),
            }
        });
        let _ = spawned;
    }

    /// A whole-flow trial of the flow's draft (the published version when there is no draft) against the input its newest run recorded:
    /// read-only steps are tried once, steps with effects are refused and listed. Nothing is sent, written or published, and the draft
    /// is not changed (F-FLOW-33).
    pub fn trial_whole(&self, flow: &str) -> Result<iyakki_flow::assist_run::WholeTrial, String> {
        let f = self.store.get_draft(flow).ok_or_else(|| format!("there is no flow `{flow}`"))?;
        let newest = self.engine.summaries().into_iter().filter(|r| r.flow_id == flow).max_by_key(|r| r.created_ms).ok_or("this flow has no run yet: a trial uses the input of a run")?;
        let run = self.engine.get_run(&newest.id).ok_or("the newest run is gone")?;
        let reg = self.engine.registry();
        let t = EngineTrial { engine: &self.engine, reg: &reg };
        let w = t.whole(&f, &run);
        iyakki_flow::host::Host::audit(self.host.as_ref(), "flow.assist.trial", &format!("{flow}: {}", w.summary()));
        Ok(w)
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
        for l in env.notify_new_failures() {
            iyakki_flow::host::Host::audit(env.host.as_ref(), "flow.failure.noted", &l.chars().take(200).collect::<String>());
        }
        let lines = env.assist_tick();
        if !lines.is_empty() {
            iyakki_flow::host::Host::audit(env.host.as_ref(), "flow.assist.pass", &lines.join(" | ").chars().take(300).collect::<String>());
        }
    });
}
