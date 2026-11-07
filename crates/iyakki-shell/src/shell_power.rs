//! The shell's side of power and updates: the saved power profile at start, sleep after idle (never while something runs), the safe
//! fall-back on a T2 Mac (the power saver profile until the next input), and the daily update check.
//!
//! The rules are `iyakki_agent::power::idle_action` and `iyakki_agent::store::updatecheck::due`; this file only feeds them: the idle
//! seconds from the screen saver's clock, the reasons the machine is busy, and the person's choices from the settings file. The
//! update check only looks and keeps the answer (`update-check.json`); it never downloads or installs anything, and it runs only while
//! an administrator is signed in (they are the ones who can act on it).

use super::*;
use iyakki_agent::power::{self, Idle, PowerEnv, Profile};
use iyakki_agent::store::updatecheck as uc;
use std::sync::atomic::{AtomicBool, Ordering as AtOrd};

pub(super) struct PowerIdle {
    pub(super) env: PowerEnv,
    /// What the machine can do, read again every minute (the clock of the saver, in ms).
    caps: Option<(u64, power::Caps)>,
    /// The saved profile is set once after the shell starts.
    applied: bool,
    /// The idle rule put the power saver profile on: the profile to bring back with the first input.
    fallback: Option<Profile>,
    /// An update check is running in its thread.
    checking: Arc<AtomicBool>,
}

impl Default for PowerIdle {
    fn default() -> PowerIdle {
        PowerIdle { env: if cfg!(test) { PowerEnv::at("/nonexistent-iyakki-sys") } else { PowerEnv::real() }, caps: None, applied: false, fallback: None, checking: Arc::new(AtomicBool::new(false)) }
    }
}

impl Shell {
    fn power_settings(&self) -> power::PowerSettings {
        self.env.services.settings.lock().map(|s| s.get().power.clone()).unwrap_or_default()
    }

    /// Why the computer must not sleep right now (empty when nothing runs).
    pub(super) fn power_busy(&self) -> Vec<String> {
        let mut v = Vec::new();
        if let Some(why) = self.saver_keep_on() {
            v.push(why.to_string());
        }
        if matches!(self.installs.badge(), crate::installpop::Badge::Running { .. }) {
            v.push("an install".into());
        }
        if let Some(s) = &self.session {
            if self.env.services.jobs.running_count(&s.user) > 0 {
                v.push("a job".into());
            }
        }
        if iyakki_agent::devmode::update::interrupted(self.env.services.dir()).is_some() {
            v.push("a system update".into());
        }
        v
    }

    /// Every heartbeat from the screen saver's step, with the seconds without input. Returns true when the computer slept (the caller
    /// then counts the wake-up as input, so it does not fall asleep again at once).
    pub(super) fn power_idle(&mut self, now_ms: u64, idle_secs: u64) -> bool {
        if self.tick % 15 != 5 {
            return false;
        }
        let settings = self.power_settings();
        if !self.saver.pw.applied {
            self.saver.pw.applied = true;
            match power::apply_saved(&self.saver.pw.env, &settings) {
                Ok(m) => log::info!("power: {m}"),
                Err(e) => log::debug!("power: {e}"),
            }
        }
        // the first input brings the chosen profile back
        if idle_secs < 5 {
            if let Some(p) = self.saver.pw.fallback.take() {
                let _ = power::set_profile(&self.saver.pw.env, p);
                log::info!("power: back to {}", p.label());
            }
            return false;
        }
        if settings.sleep_after_min == 0 {
            return false;
        }
        let stale = self.saver.pw.caps.as_ref().is_none_or(|(at, _)| now_ms.saturating_sub(*at) > 60_000);
        if stale {
            self.saver.pw.caps = Some((now_ms, power::caps(&self.saver.pw.env)));
        }
        let caps = self.saver.pw.caps.as_ref().map(|c| c.1.clone()).unwrap_or_default();
        let busy = self.power_busy();
        match power::idle_action(&settings, &caps, idle_secs, &busy) {
            Idle::Sleep if self.env.host.can_sleep() => {
                let user = self.session.as_ref().map(|s| s.user.clone()).unwrap_or_default();
                self.sleep_now(&user, &format!("idle for {} minutes", idle_secs / 60));
                true
            }
            Idle::SaverProfile if self.saver.pw.fallback.is_none() => {
                if power::set_profile(&self.saver.pw.env, Profile::PowerSaver).is_ok() {
                    self.saver.pw.fallback = Some(settings.profile);
                    self.env.audit.record("system", "power.idle", "power saver profile while idle");
                    log::info!("power: idle on a machine where sleep is unreliable: power saver profile");
                }
                false
            }
            _ => false,
        }
    }

    /// The daily update check (a few times a minute this only reads the cached time). Only while an administrator is signed in.
    pub(super) fn update_check_tick(&mut self) {
        use iyakki_agent::store::Store;
        let Some(sess) = self.session.as_ref().filter(|s| s.policy.manage_settings) else { return };
        if self.saver.pw.checking.load(AtOrd::SeqCst) {
            return;
        }
        let svc = self.env.services.clone();
        let settings = svc.settings.lock().map(|s| s.get().update_check.clone()).unwrap_or_default();
        let store = Store::for_services(&svc);
        let last = uc::Report::load(svc.dir()).map_or(0, |r| r.checked_at);
        if uc::due(&settings, last, store.now(), iyakki_agent::meteredlink::metered_now(), &store.licenses().device_id()) != uc::Due::Now {
            return;
        }
        let (policy, user) = (sess.policy.clone(), sess.user.clone());
        let flag = self.saver.pw.checking.clone();
        flag.store(true, AtOrd::SeqCst);
        std::thread::spawn(move || {
            let rel = iyakki_agent::relinfo::ReleaseInfo::parse(&std::fs::read_to_string(iyakki_agent::relinfo::RELEASE_FILE).unwrap_or_default());
            let running = if rel.version.is_empty() { iyakki_agent::relinfo::VERSION.to_string() } else { rel.version };
            let fetch = iyakki_agent::store::catalog::HttpFetch(svc.quick_upstream());
            let rep = uc::run_and_keep(&store, &svc, &policy, &user, &fetch, &settings, &running, &rel.variant);
            log::info!("update check: {} update(s), {} problem(s)", rep.count(), rep.errors.len());
            flag.store(false, AtOrd::SeqCst);
        });
    }

    /// Look somewhere else than /sys (tests).
    #[cfg(test)]
    pub(crate) fn power_env_for_test(&mut self, env: PowerEnv) {
        self.saver.pw.env = env;
        self.saver.pw.caps = None;
    }
}
