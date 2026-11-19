//! The model router's door to the core (F-ROUTER-06; the owner's core rule: nothing that decides which model sees a person's work is
//! reached around the core). The router itself (plans, strategies, model calls) lives in `iyakki-agent`; what it must ask the core is small:
//!
//! * `router.use` (a hard ask, a card every time) before a run starts: a run sends the person's goal and what the steps produce to the
//!   models the person allowed, and spends tokens;
//! * `router.strategy.set` (a hard ask) before the strategy that decides which model takes which step is changed or a strategy script is added;
//! * one access-log line for every decision of a run (`record`): ids and numbers only, never message content.
//!
//! The device gives a run `iyakki_core_local::LocalRouter` (the broker has already decided; the line goes to the guard), a test gives
//! `iyakki_core_fake::FakeRouter`. A machine whose `Os` does not say otherwise has no router and refuses ([`NoRouter`]).

pub trait Router: Send + Sync {
    /// Permission to start a run; `label` says what (the number of steps, the strategy), never the goal's words.
    fn begin_run(&self, label: &str) -> Result<(), String>;
    /// Permission to listen through the microphone for one step (`microphone.listen`, a card every time; F-ROUTER-11). `label` says how long,
    /// never what is wanted. The default refuses.
    fn allow_listen(&self, _label: &str) -> Result<(), String> {
        Err("nobody can be asked on this machine, so a step cannot listen".into())
    }
    /// Permission for ONE tool call of a step that runs unattended (F-ROUTER-40): a card on the screen, the step waits for the answer. `label`
    /// names the tool and what it would do, never the goal. The default refuses: a machine with nobody to ask gives no permission.
    fn ask_step_tool(&self, _label: &str) -> Result<(), String> {
        Err("nobody can be asked on this machine, so a step cannot use a tool that asks".into())
    }
    /// Permission to choose a strategy or add a strategy script.
    fn allow_strategy(&self, name: &str) -> Result<(), String>;
    /// One decision of a run for the access log (`what` like `router.assign`, `detail` ids and numbers).
    fn record(&self, what: &str, detail: &str);
}

/// A machine without a router: every request is refused and nothing is written.
pub struct NoRouter;

impl Router for NoRouter {
    fn begin_run(&self, _: &str) -> Result<(), String> {
        Err("this machine has no model router".into())
    }
    fn allow_strategy(&self, _: &str) -> Result<(), String> {
        Err("this machine has no model router".into())
    }
    fn record(&self, _: &str, _: &str) {}
}
