//! The Jobs strip and the job log view (see `docs/JOBS.md`): the drawing and the keys live in `iyakki-tile-runlog` (they do not depend on the
//! agent or the shared crate). This file is the shell's side of it: the port that reads the person's jobs from the core ([`JobsCx`]) and the
//! old path of the types, so the chat and the RUNS tile keep naming `jobs_ui::JobsUi`. The same drawing serves the chat, the AI IDE's chat pane
//! and the Jobs list in RUNS.

use iyakki_shared::jobs::{self, Job, State};
use iyakki_tile_api::runs as api;

pub use iyakki_tile_runlog::jobs::*;

use super::AppCx;

pub fn to_state(s: &State) -> api::JobState {
    match s {
        State::Running => api::JobState::Running,
        State::Succeeded => api::JobState::Succeeded,
        State::Failed(c) => api::JobState::Failed(*c),
        State::Stopped => api::JobState::Stopped,
        State::Lost => api::JobState::Lost,
    }
}

pub fn to_job(j: &Job) -> api::Job {
    api::Job { id: j.id.clone(), label: j.label.clone(), command: j.command.clone(), state: to_state(&j.state), started_ms: j.started_ms, ended_ms: j.ended_ms, progress: j.progress, last_line: j.last_line.clone(), last_active_ms: j.last_active_ms }
}

/// The real port: the signed-in person's jobs, read through the core's job handle (the engine never shows another person's jobs).
pub struct JobsCx<'a>(pub &'a AppCx);

impl api::JobsPort for JobsCx<'_> {
    fn visible(&self, ws: Option<&str>) -> Vec<api::Job> {
        let user = &self.0.session.user;
        let list = match ws {
            Some(w) => self.0.jobs().visible(user, w),
            None => self.0.jobs().visible_all(user),
        };
        list.iter().map(to_job).collect()
    }
    fn take_finished(&self, ws: &str) -> Vec<String> {
        self.0.jobs().take_finished(&self.0.session.user, ws, jobs::Channel::Ui).iter().map(jobs::finish_line).collect()
    }
    fn log_len(&self, id: &str) -> u64 {
        self.0.jobs().log_len(&self.0.session.user, id)
    }
    fn log_text(&self, id: &str, max: usize) -> Result<String, String> {
        self.0.jobs().log_text(&self.0.session.user, id, max)
    }
    fn stop_job(&self, id: &str) -> Box<dyn FnOnce() + Send> {
        let (jobs_api, user, id) = (self.0.jobs().clone(), self.0.session.user.clone(), id.to_string());
        Box::new(move || {
            let _ = jobs_api.stop(&user, &id, jobs::STOP_GRACE);
        })
    }
    fn dismiss(&self, id: &str) -> Result<(), String> {
        self.0.jobs().dismiss(&self.0.session.user, id)
    }
    fn audit(&self, action: &str, subject: &str) {
        self.0.audit.record(&self.0.session.user, action, subject);
    }
}
