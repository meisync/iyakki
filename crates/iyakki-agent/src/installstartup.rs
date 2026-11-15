//! Quick install on the start-up page: one row ("install") that says what the installer jobs are doing, so a person who watches the
//! start-up list sees an install that is running, one that failed (with the reason) and nothing when no install was started.
//! Pure: [`state_of`] maps the installer jobs to a registry state; [`report`] pushes it.

use crate::jobs::{Job, State};
use crate::quickinstall::item_of;
use crate::startup::{Registry, StartupState};

/// The registry id of the row.
pub const ID: &str = "install";

fn name_of(j: &Job, fallback: &str) -> String {
    item_of(j).map(|i| i.title().to_string()).unwrap_or_else(|| fallback.to_string())
}

/// The state of the row for the installer jobs of a person (the newest finished job decides when none runs).
pub fn state_of(jobs: &[Job]) -> StartupState {
    let running: Vec<&Job> = jobs.iter().filter(|j| j.running()).collect();
    if !running.is_empty() {
        let mut names: Vec<String> = running.iter().map(|j| name_of(j, "an installer")).collect();
        names.dedup();
        let what = names.join(", ");
        let step = match running.iter().find_map(|j| j.progress) {
            Some(p) => format!("installing {what} ({}%)", p.max(0.0).min(100.0) as u32),
            None => format!("installing {what}"),
        };
        return StartupState::Starting { step };
    }
    let mut done: Vec<&Job> = jobs.iter().filter(|j| !j.dismissed && j.state.finished()).collect();
    done.sort_by_key(|j| std::cmp::Reverse(j.started_ms));
    match done.first() {
        None => StartupState::Disabled,
        Some(j) => match j.state {
            // a stop by the person is not a failure
            State::Succeeded | State::Stopped => StartupState::Ready,
            State::Failed(124) => StartupState::Failed(format!("{} timed out", name_of(j, "an install"))),
            State::Failed(code) => StartupState::Failed(format!("{} did not install (exit {code})", name_of(j, "an item"))),
            _ => StartupState::Failed(format!("{} was lost (the machine restarted during the install)", name_of(j, "an install"))),
        },
    }
}

/// Push the row. Returns true when something changed.
pub fn report(reg: &Registry, jobs: &[Job]) -> bool {
    reg.report(ID, state_of(jobs))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn job(cmd: &str, state: State, started: u64) -> Job {
        let ended_ms = if state == State::Running { 0 } else { started + 1 };
        Job { id: format!("j{started}"), label: String::new(), command: cmd.into(), cwd: String::new(), owner: "u".into(), workspace: crate::quickinstall::WORKSPACE.into(), pid: 0, state, started_ms: started, ended_ms, progress: None, last_line: String::new(), last_active_ms: 0, dismissed: false }
    }

    #[test]
    fn no_install_leaves_the_row_off_and_a_running_one_says_what_is_installing() {
        assert_eq!(state_of(&[]), StartupState::Disabled);
        let mut j = job("iyakki quick-install k3s", State::Running, 5);
        j.progress = Some(42.0);
        match state_of(&[j]) {
            StartupState::Starting { step } => assert!(step.contains("42%") && step.starts_with("installing"), "{step}"),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn the_newest_finished_job_decides_failed_or_ready_and_a_timeout_is_said() {
        let old_fail = job("iyakki quick-install k3s", State::Failed(1), 1);
        let new_ok = job("iyakki quick-install k3s", State::Succeeded, 9);
        assert_eq!(state_of(&[old_fail.clone(), new_ok]), StartupState::Ready);
        match state_of(&[job("iyakki quick-install k3s", State::Failed(124), 3), old_fail]) {
            StartupState::Failed(r) => assert!(r.contains("timed out"), "{r}"),
            other => panic!("{other:?}"),
        }
        assert!(matches!(state_of(&[job("iyakki quick-install k3s", State::Lost, 3)]), StartupState::Failed(_)));
        assert_eq!(state_of(&[job("iyakki quick-install k3s", State::Stopped, 3)]), StartupState::Ready);
    }

    #[test]
    fn the_report_reaches_the_registry_once_and_a_failure_is_queued_once() {
        let reg = Registry::new(|| 0);
        let failed = [job("iyakki quick-install k3s", State::Failed(2), 3)];
        assert!(report(&reg, &failed));
        assert!(!report(&reg, &failed));
        assert_eq!(reg.take_failures().len(), 1);
        assert!(matches!(reg.state(ID), Some(StartupState::Failed(_))));
    }
}
