//! The shell's side of power and updates: the saved power profile at start, sleep after idle (never while something runs), the safe
//! fall-back on a T2 Mac (the power saver profile until the next input), and the daily update check.
//!
//! The rules are `iyakki_shared::power::idle_action` and `iyakki_shared::store_updatecheck::due`; this file only feeds them: the idle
//! seconds from the screen saver's clock, the reasons the machine is busy, and the person's choices from the settings file. The
//! update check only looks and keeps the answer (`update-check.json`); it never downloads or installs anything, and it runs only while
//! an administrator is signed in (they are the ones who can act on it).

use crate::stale_stats::mark_stale;
use super::*;
use iyakki_shared::power::{Idle, PowerEnv};
use iyakki_shared::power::{self, Profile};
use std::sync::atomic::{AtomicBool, Ordering as AtOrd};

pub(super) struct PowerIdle {
    pub(super) env: PowerEnv,
    /// What the machine can do, read again every minute (the clock of the saver, in ms).
    caps: Option<(u64, power::Caps)>,
    /// The saved profile is set once after the shell starts.
    applied: bool,
    /// The lid at the last look (`None`: no lid switch, or not looked at yet).
    lid: Option<power::Lid>,
    /// The idle rule put the power saver profile on: the profile to bring back with the first input.
    fallback: Option<Profile>,
    /// An update check is running in its thread.
    checking: Arc<AtomicBool>,
}

impl Default for PowerIdle {
    fn default() -> PowerIdle {
        PowerIdle { env: if cfg!(test) { PowerEnv::at("/nonexistent-iyakki-sys") } else { PowerEnv::real() }, caps: None, lid: None, applied: false, fallback: None, checking: Arc::new(AtomicBool::new(false)) }
    }
}

impl Shell {
    fn power_settings(&self) -> power::PowerSettings {
        self.env.settings().lock().map(|s| s.get().power.clone()).unwrap_or_default()
    }

    /// What the machine can do, read again at most every minute.
    pub(super) fn power_caps(&mut self, now_ms: u64) -> power::Caps {
        let stale = self.saver.pw.caps.as_ref().is_none_or(|(at, _)| now_ms.saturating_sub(*at) > 60_000);
        if stale {
            self.saver.pw.caps = Some((now_ms, iyakki_apps::agent().host_probe().power_caps(&self.saver.pw.env)));
        }
        self.saver.pw.caps.as_ref().map(|c| c.1.clone()).unwrap_or_default()
    }

    /// Do what a lid closing or a power button press means (`power::lid_step`, `power::button_press`).
    fn power_do(&mut self, what: power::Do, why: &str) -> bool {
        let user = self.session.as_ref().map(|s| s.user.clone()).unwrap_or_default();
        match what {
            power::Do::Lock if self.session.is_some() => {
                self.lock_now(&user, why);
                false
            }
            power::Do::Sleep if self.env.host.can_sleep() => {
                self.sleep_now(&user, why);
                true
            }
            // asked to sleep but the host cannot: the screen locks at least
            power::Do::Sleep if self.session.is_some() => {
                self.lock_now(&user, why);
                false
            }
            _ => false,
        }
    }

    /// The power button: what the person chose (`Settings -> Power`). True when the press was handled here; false means "open the card".
    pub(super) fn power_button_pressed(&mut self, now_ms: u64) -> bool {
        let settings = self.power_settings();
        let caps = self.power_caps(now_ms);
        let busy = self.power_busy();
        match power::button_press(&settings, &caps, &busy) {
            power::Do::Card => false,
            // the kernel could sleep but this host cannot: the card, as the page says
            power::Do::Sleep if !self.env.host.can_sleep() => false,
            what => {
                self.power_do(what, "power button");
                true
            }
        }
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
            if self.env.jobs().running_count(&s.user) > 0 {
                v.push("a job".into());
            }
        }
        if iyakki_apps::agent().devmode().update_interrupted(self.env.data_dir()) {
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
        // the lid: only the moment it closes counts
        let lid_now = iyakki_apps::agent().host_probe().power_lid_state(&self.saver.pw.env.proc);
        let lid_before = std::mem::replace(&mut self.saver.pw.lid, lid_now);
        if settings.lid != power::LidAction::Nothing && lid_before.is_some() && lid_now != lid_before {
            let caps = self.power_caps(now_ms);
            let busy = self.power_busy();
            let docked = iyakki_apps::agent().host_probe().power_external_display(&self.saver.pw.env.root);
            let what = power::lid_step(lid_before, lid_now, &settings, &caps, &busy, docked);
            if what != power::Do::Nothing {
                let slept = self.power_do(what, "lid closed");
                if slept {
                    return true;
                }
            }
        }
        if !self.saver.pw.applied {
            self.saver.pw.applied = true;
            match iyakki_apps::agent().host_probe().power_apply_saved(&self.saver.pw.env, &settings) {
                Ok(m) => log::info!("power: {m}"),
                Err(e) => log::debug!("power: {e}"),
            }
        }
        // the first input brings the chosen profile back
        if idle_secs < 5 {
            if let Some(p) = self.saver.pw.fallback.take() {
                let _ = iyakki_apps::agent().host_probe().power_set_profile(&self.saver.pw.env, p);
                log::info!("power: back to {}", p.label());
            }
            return false;
        }
        if settings.sleep_after_min == 0 {
            return false;
        }
        let caps = self.power_caps(now_ms);
        let busy = self.power_busy();
        match power::idle_action(&settings, &caps, idle_secs, &busy) {
            Idle::Sleep if self.env.host.can_sleep() => {
                let user = self.session.as_ref().map(|s| s.user.clone()).unwrap_or_default();
                self.sleep_now(&user, &format!("idle for {} minutes", idle_secs / 60));
                true
            }
            Idle::SaverProfile if self.saver.pw.fallback.is_none() => {
                if iyakki_apps::agent().host_probe().power_set_profile(&self.saver.pw.env, Profile::PowerSaver).is_ok() {
                    self.saver.pw.fallback = Some(settings.profile);
                    self.env.audit.record("system", "power.idle", "power saver profile while idle");
                    log::info!("power: idle on a machine where sleep is unreliable: power saver profile");
                }
                false
            }
            _ => false,
        }
    }

    /// Internet sharing ended without being asked to (policy, cable, uplink, idle): one note in the inbox of the person who started it
    /// and of a signed-in administrator (`sharing::notice`).
    pub(super) fn sharing_notice_tick(&mut self) {
        let Some(sess) = self.session.as_ref() else { return };
        let (user, admin) = (sess.user.clone(), sess.policy.manage_settings);
        let svc = self.env.services.clone();
        if svc.sharing_notice_tick(&user, admin) {
            self.refresh_badge();
            mark_stale!(self);
        }
    }

    /// Messages copied from the rooms the person saved into their INBOX (F-MSG-07): once a minute, for accounts whose copy is switched on.
    /// The round runs on its own thread; this only asks whether it is time.
    pub(super) fn mirror_tick(&mut self) {
        let Some(sess) = self.session.as_ref() else { return };
        let (audit, user) = (self.env.audit.clone(), sess.user.clone());
        let log: iyakki_shared::gitkeys::AuditFn = std::sync::Arc::new(move |a, d| audit.record(&user, a, d));
        if self.env.services.msg_mirror_tick(&sess.user, log) {
            self.refresh_badge();
        }
    }

    /// The daily update check (a few times a minute this only reads the cached time). Only while an administrator is signed in.
    pub(super) fn update_check_tick(&mut self) {
        let Some(sess) = self.session.as_ref().filter(|s| s.policy.manage_settings) else { return };
        if self.saver.pw.checking.load(AtOrd::SeqCst) {
            return;
        }
        let _svc = self.env.services.clone();
        let settings = self.env.settings().lock().map(|s| s.get().update_check.clone()).unwrap_or_default();
        if !self.env.storeapi().update_check_due(&settings) {
            return;
        }
        let (policy, user) = (sess.policy.clone(), sess.user.clone());
        let flag = self.saver.pw.checking.clone();
        flag.store(true, AtOrd::SeqCst);
        let api = self.env.storeapi().clone();
        std::thread::spawn(move || {
            let rel = iyakki_shared::relinfo::ReleaseInfo::parse(&std::fs::read_to_string(iyakki_shared::relinfo::RELEASE_FILE).unwrap_or_default());
            let running = if rel.version.is_empty() { iyakki_shared::relinfo::VERSION.to_string() } else { rel.version };
            let rep = api.update_check_run(&policy, &user, &settings, &running, &rel.variant);
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
