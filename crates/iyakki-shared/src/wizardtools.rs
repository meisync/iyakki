//! Types moved out of `iyakki_agent::wizardtools` (F-SPLIT-03, scripted by dist/split/move_types.py); the agent re-exports them.



pub const PROMPT_RULE: &str = "\nWIZARDS: after an app is installed or updated it may bring a setup wizard. wizard_pending lists them; wizard_start opens one and shows the first step; ask the person each question in the step, then call wizard_answer with their values (never invent an answer, never ask for a secret in plain chat: tell the person to enter it in the app). wizard_continue runs the checks and data migrations and asks the person first. Say what the wizard reported; do not claim it finished until wizard_continue shows the summary.";

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use iyakki_core_api::wizard::WizardSpec;
use serde_json::Value;

use crate::wizardrun::{Env, Run, State, View};

pub trait Backend: Send + Sync {
    /// `(app, version, spec)` of every installed app wizard.
    fn installed(&self) -> Vec<(String, String, WizardSpec)>;
    /// `(app, wizard id, trigger, title)` waiting after an install or update.
    fn pending(&self) -> Vec<(String, String, String, String)>;
    fn clear_pending(&self, app: &str, wizard: &str);
    fn env(&self) -> Arc<dyn Env>;
    /// The run in progress (one per person).
    fn run(&self) -> Arc<Mutex<Option<(String, Run)>>>;
    /// The line for the audit log.
    fn audit(&self, what: &str, detail: &str);
}

/// A screen that shows the wizard (Settings, System update) works on the SAME run as the chat tools: the one run of the person in
/// [`Backend::run`]. A wizard started in the chat can be answered on the screen and the other way round (a secret is only ever typed on the screen).
pub mod screen {
    use super::*;

    /// What the screen has to show now: the app, the wizard's run view, and what the automatic steps ahead are.
    pub struct Shown {
        pub app: String,
        pub view: View,
        pub upcoming: Vec<String>,
        pub log: Vec<String>,
    }

    /// Open the wizard `wiz` of `app` (or pick up the one that is already running for this app). `from` is the version an update comes from.
    pub fn open(be: &dyn Backend, app: &str, wiz: &str, from: Option<&str>) -> Result<(), String> {
        let slot = be.run();
        let mut g = slot.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Some((a, r)) = g.as_ref().filter(|(_, r)| r.state() == &State::Running) {
            if a == app && r.id() == wiz {
                return Ok(());
            }
            return Err(format!("a wizard of {a} is already running; finish or cancel it first"));
        }
        let (_, ver, spec) = be.installed().into_iter().find(|(x, _, w)| x == app && w.id == wiz).ok_or("that app has no such wizard")?;
        let trigger = spec.trigger.on.clone();
        let installed_from = from.map(str::to_string).or_else(|| (trigger == "on_update").then_some(ver));
        let run = Run::start(spec, &trigger, installed_from.as_deref())?;
        *g = Some((app.to_string(), run));
        // it stays waiting until it is over: a cancelled or failed one can be started again
        be.audit("wizard.start", &format!("{app}/{wiz} on {trigger} (screen)"));
        Ok(())
    }

    /// The step the run is at, or `None` when no wizard is open.
    pub fn shown(be: &dyn Backend) -> Option<Shown> {
        let slot = be.run();
        let g = slot.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        g.as_ref().map(|(a, r)| Shown { app: a.clone(), view: r.view(), upcoming: r.upcoming(), log: r.log().to_vec() })
    }

    /// Hand in the answers of the form that is showing. The errors are one line per field, in the person's words.
    pub fn answer(be: &dyn Backend, values: &BTreeMap<String, Value>) -> Result<(), Vec<(String, String)>> {
        let slot = be.run();
        let mut g = slot.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        let Some((app, run)) = g.as_mut() else { return Err(vec![(String::new(), "no wizard is open".into())]) };
        run.submit(values).map_err(|e| e.into_iter().map(|x| (x.field, x.message)).collect::<Vec<_>>())?;
        be.audit("wizard.answer", &format!("{app}/{} ({} value(s), screen)", run.id(), values.len()));
        Ok(())
    }

    /// Run the automatic steps ahead (checks, migrations). The caller has asked the person first; the real work may take a while, so a
    /// screen calls this from a thread. Returns the state after it.
    pub fn run_steps(be: &dyn Backend) -> Result<State, String> {
        let env = be.env();
        let slot = be.run();
        let mut g = slot.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        let (app, run) = g.as_mut().ok_or("no wizard is open")?;
        run.advance(env.as_ref());
        be.audit("wizard.run", &format!("{app}/{} -> {:?} (screen)", run.id(), run.state()));
        if run.state() == &State::Done {
            be.clear_pending(app, run.id());
        }
        Ok(run.state().clone())
    }

    /// The person read the summary: the wizard is over.
    pub fn finish(be: &dyn Backend) -> Result<(), String> {
        let slot = be.run();
        let mut g = slot.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        let (app, run) = g.as_mut().ok_or("no wizard is open")?;
        run.finish();
        be.audit("wizard.finish", &format!("{app}/{} (screen)", run.id()));
        if run.state() == &State::Done {
            be.clear_pending(app, run.id());
        }
        Ok(())
    }

    pub fn cancel(be: &dyn Backend) {
        let slot = be.run();
        let mut g = slot.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Some((app, run)) = g.as_mut() {
            run.cancel();
            be.audit("wizard.cancel", &format!("{app}/{} (screen)", run.id()));
        }
    }
}
