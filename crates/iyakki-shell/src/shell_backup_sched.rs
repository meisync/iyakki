//! The scheduled backup in the shell (F-BACKUP-01): while a person is signed in the shell looks, a few times a minute, at their
//! `backup-schedule-<user>.json` (`iyakki_shared::backup_schedule`) and, when a run is due, makes the backup on a worker thread. A run never
//! starts while the screen is locked (the person is not there to see it), never two at once, and a failure waits for the next slot instead of
//! retrying every minute. The result is a note (toast), an audit line and the schedule file's `last_result`.

use super::*;
use iyakki_shared::backup::{Created, Layout, Progress};
use iyakki_shared::backup_schedule::Schedule;
use std::sync::mpsc::{channel, Receiver};

/// Beats between two looks at the schedule file (the heartbeat that calls this is a few times a second).
const LOOK_EVERY: u64 = 20;

#[derive(Default)]
pub(super) struct BackupSched {
    /// A run in progress: the result channel and the user it is for.
    running: Option<(Receiver<(Schedule, Result<Created, String>)>, String)>,
    looked: u64,
    /// Tests: the layout to back up (the real folders otherwise) and the clock (the real one otherwise).
    #[cfg(test)]
    pub(super) layout: Option<Layout>,
    #[cfg(test)]
    pub(super) now: Option<i64>,
}

impl Shell {
    fn bksched_now(&self) -> i64 {
        #[cfg(test)]
        if let Some(n) = self.bksched.now {
            return n;
        }
        iyakki_clock::now_unix()
    }

    /// Called from the heartbeat: take a finished run's result, or start a due one.
    pub(super) fn backup_schedule_poll(&mut self) {
        let Some(user) = self.session.as_ref().map(|s| s.user.clone()) else { return };
        let dir = self.env.data_dir().to_path_buf();
        if let Some((rx, who)) = &self.bksched.running {
            let Ok((done, res)) = rx.try_recv() else { return };
            let who = who.clone();
            self.bksched.running = None;
            // keep what the person changed meanwhile; only the result of the run is added
            let mut s = Schedule::load(&dir, &who);
            (s.last_run, s.last_result) = (done.last_run, done.last_result.clone());
            let _ = s.save(&dir, &who);
            self.env.audit.record(&who, "backup.scheduled", &done.last_result);
            {
                use iyakki_core_api::appevents::{payload, Val};
                // the apps that listen hear that a backup ended, not what it holds or where it went
                let _ = self.core.app_events().emit("backup.finished", Some(&who), payload(&[("name", Val::Text("scheduled backup".into())), ("outcome", Val::Text(if res.is_ok() { "finished" } else { "failed" }.into()))]));
            }
            if who == user {
                self.say_info(if res.is_ok() { "Scheduled backup done".to_string() } else { format!("Scheduled backup: {}", done.last_result) });
            }
            return;
        }
        if self.tick.saturating_sub(self.bksched.looked) < LOOK_EVERY || self.is_locked() {
            return;
        }
        self.bksched.looked = self.tick;
        let zone = iyakki_clock::Zone::system_with(None);
        let now = self.bksched_now();
        let mut sched = Schedule::load(&dir, &user);
        if !sched.due(&zone, now) {
            return;
        }
        #[cfg(test)]
        let layout = self.bksched.layout.clone();
        #[cfg(not(test))]
        let layout: Option<Layout> = None;
        let layout = layout.unwrap_or_else(|| Layout { home: iyakki_apps::agent().scope_home_root(&user, &dir), data: dir.clone(), user: user.clone() });
        let (tx, rx) = channel();
        self.bksched.running = Some((rx, user));
        std::thread::spawn(move || {
            let res = iyakki_shared::backup_schedule::run(&layout, &mut sched, now, &Progress::default());
            let _ = tx.send((sched, res));
        });
    }

    #[cfg(test)]
    pub(crate) fn backup_schedule_running_for_test(&self) -> bool {
        self.bksched.running.is_some()
    }
}
