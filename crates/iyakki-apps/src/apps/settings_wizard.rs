//! Settings, System update: the setup steps an app brought, answered on a screen (F-CORE-24; docs/development/BUNDLES-WIZARDS-EXTENSIONS-DESIGN.md).
//!
//! `w` on the page opens the first waiting wizard (or picks up the one that is already running, for example one started in the chat: the
//! screen and the assistant work on the same run). Each question step is a form (a secret is typed here, masked, never in the chat); a
//! step that runs checks or moves data first shows what it will do and waits for `y`; the summary waits for `y`; a failure says why and the
//! setup stays waiting, so `w` tries again. Esc on a form cancels the run. All the rules are the engine's (`iyakki_agent::wizardrun`):
//! this file turns its views into forms and its answers back into values.

use super::*;
use super::system_port::UpdatesPort;
use iyakki_shared::wizardrun::{State, View};
use iyakki_shared::wizardtools::{screen, Backend};
use serde_json::Value;
use std::collections::BTreeMap;


impl SettingsApp {
    /// What the wizards run on: this machine's store and checks, or (in a test) a fake handed to the panel.
    fn wiz_backend(&self, p: &dyn UpdatesPort) -> Arc<dyn Backend> {
        self.upd.wiz_be.clone().unwrap_or_else(|| p.wizard_backend())
    }

    /// Is there anything for `w` to open: a setup waiting, or one that is open (started in the chat, say)?
    pub(super) fn wiz_available(&self, p: &dyn UpdatesPort) -> bool {
        !self.upd_wizard_lines(p).is_empty() || screen::shown(&*self.wiz_backend(p)).is_some_and(|s| !matches!(s.view, View::Finished(_)))
    }

    /// Key `w`: the first waiting wizard, or the one already open.
    pub(super) fn wiz_start(&mut self, p: &dyn UpdatesPort) {
        let be = self.wiz_backend(p);
        let running = screen::shown(&*be).is_some_and(|s| !matches!(s.view, View::Finished(_)));
        if !running {
            // the waiting list (the file of the person; a fake backend answers it in a test)
            let first = if self.upd.wiz_be.is_some() {
                be.pending().into_iter().next().map(|(app, wizard, _, _)| (app, wizard, None))
            } else {
                p.store_api().pending_wizards(&p.user_name()).into_iter().next().map(|w| (w.pending.app, w.pending.wizard, w.pending.from_version))
            };
            let Some((app, wizard, from)) = first else {
                self.ok("No setup is waiting.");
                return;
            };
            if let Err(e) = screen::open(&*be, &app, &wizard, from.as_deref()) {
                self.fail(format!("The setup of {app} cannot start: {e}"));
                return;
            }
        }
        self.modal = self.wiz_modal(p);
    }

    /// The modal for the step the run is at; `None` (with a message on the page) when the wizard is over.
    pub(super) fn wiz_modal(&mut self, p: &dyn UpdatesPort) -> Option<Modal> {
        let be = self.wiz_backend(p);
        let shown = screen::shown(&*be)?;
        let app = shown.app.clone();
        match shown.view {
            View::Form { title, fields, .. } => {
                let mut f = Form::new(&format!("Setup of {app}{}", if title.is_empty() { String::new() } else { format!(": {title}") }));
                for fl in &fields {
                    let label = format!("{}{}", fl.label, if fl.required { "" } else { " (optional)" });
                    let now = fl.value.as_ref().and_then(|v| v.as_str().map(str::to_string)).unwrap_or_default();
                    f = match fl.kind.as_str() {
                        "secret" => f.secret(&label, &fl.help),
                        "choice" => {
                            let labels: Vec<&str> = fl.choices.iter().map(|(_, l)| l.as_str()).collect();
                            let at = fl.choices.iter().position(|(v, _)| *v == now).unwrap_or(0);
                            f.choice(&label, &labels, at)
                        }
                        "toggle" => f.choice(&label, &["off", "on"], usize::from(fl.value.as_ref().and_then(Value::as_bool).unwrap_or(false))),
                        _ => f.text(&label, &now, &fl.help),
                    };
                }
                Some(Modal::Form(FormKind::BundleWizard, f))
            }
            View::Working { .. } => {
                let steps = if shown.upcoming.is_empty() { "an automatic step".to_string() } else { shown.upcoming.iter().map(|l| format!("- {l}")).collect::<Vec<_>>().join("\n") };
                Some(Modal::Confirm(ConfirmKind::WizardRun, format!("The setup of {app} will now run:\n{steps}\nPress y to run it. Esc leaves it waiting; press w to come back.")))
            }
            View::Summary { title, text } => Some(Modal::Confirm(ConfirmKind::WizardFinish, format!("{app}: {title}\n{text}\nPress y when you have read it."))),
            View::Finished(State::Done) => {
                let more = if self.upd.wiz_be.is_some() { be.pending().len() } else { p.store_api().pending_wizards(&p.user_name()).len() };
                self.ok(format!("The setup of {app} is finished.{}", if more > 0 { format!(" {more} more waiting: press w.") } else { String::new() }));
                None
            }
            View::Finished(State::Failed(e)) => {
                self.fail(format!("The setup of {app} stopped: {e}. It stays waiting; fix that and press w to try again."));
                None
            }
            View::Finished(State::Cancelled) => {
                self.ok(format!("The setup of {app} was cancelled; it stays waiting (press w to start it again)."));
                None
            }
            View::Finished(State::Running) => None,
        }
    }

    /// The form of a question step was submitted: hand the values to the engine. A refused field reopens the form with the reasons.
    pub(super) fn wiz_submit(&mut self, p: &dyn UpdatesPort, form: &mut Form) -> Option<Modal> {
        let be = self.wiz_backend(p);
        let shown = screen::shown(&*be)?;
        let View::Form { fields, .. } = &shown.view else { return None };
        let mut values: BTreeMap<String, Value> = BTreeMap::new();
        for (i, fl) in fields.iter().enumerate() {
            let v = match fl.kind.as_str() {
                "secret" => Value::String(form.raw(i)),
                "choice" => Value::String(fl.choices.get(form.choice_idx(i)).map(|(v, _)| v.clone()).unwrap_or_default()),
                "toggle" => Value::Bool(form.choice_idx(i) == 1),
                _ => Value::String(form.value(i)),
            };
            values.insert(fl.id.clone(), v);
        }
        match screen::answer(&*be, &values) {
            Ok(()) => self.wiz_modal(p),
            Err(errs) => {
                let words: Vec<String> = errs
                    .iter()
                    .map(|(f, m)| match fields.iter().find(|x| &x.id == f) {
                        Some(x) => format!("{}: {m}", x.label),
                        None => m.clone(),
                    })
                    .collect();
                form.error = Some(words.join("; "));
                Some(Modal::Form(FormKind::BundleWizard, form.clone()))
            }
        }
    }

    /// Esc on a question: the run is cancelled (nothing it would have run was run).
    pub(super) fn wiz_cancel(&mut self, p: &dyn UpdatesPort) {
        screen::cancel(&*self.wiz_backend(p));
        self.ok("The setup was cancelled. Nothing it would have checked or moved was run; it stays waiting.");
    }

    /// `y` on the "will now run" card (steps on a worker thread, so the page stays alive) or on the summary.
    pub(super) fn wiz_confirm(&mut self, p: &dyn UpdatesPort, kind: ConfirmKind) {
        match kind {
            ConfirmKind::WizardRun => {
                if self.job.is_some() {
                    self.fail("Another operation is still running. Press w again when it has finished.");
                    return;
                }
                let be = self.wiz_backend(p);
                self.start("Running the setup steps", move || JobOut::Wizard(screen::run_steps(&*be).map(|_| ())));
            }
            ConfirmKind::WizardFinish => {
                if let Err(e) = screen::finish(&*self.wiz_backend(p)) {
                    self.fail(e);
                    return;
                }
                self.modal = self.wiz_modal(p);
            }
            _ => {}
        }
    }

    /// The worker thread came back: show what is next (a question, the summary, or how it ended).
    pub(super) fn wiz_job_done(&mut self, p: &dyn UpdatesPort, r: Result<(), String>) {
        if let Err(e) = r {
            self.fail(e);
            return;
        }
        self.modal = self.wiz_modal(p);
    }
}
