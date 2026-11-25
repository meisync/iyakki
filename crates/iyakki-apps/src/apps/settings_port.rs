//! What the Settings pages ask of the rest of the system (the app standard, docs/development/APP-DEVELOPMENT-PROCEDURE.md section 10).
//!
//! Settings is one tile with many pages. Instead of one trait with hundreds of methods, the port is a handful of SMALL traits, one per kind
//! of thing a page group needs, and a page asks only for the traits it uses (`&dyn SettingsDataPort`, or a group trait that names several):
//!
//! - [`PagePort`]: who is looking (administrator or not), the audit line, the data folder, the clock. Every page needs these.
//! - [`SettingsDataPort`]: read and change the machine's settings file, with the refusal or "busy" answer told apart.
//!
//! Later groups add their own trait here beside these (see docs/development/SETTINGS-EXTRACTION-PLAN.md): a page of that group names the
//! trait, never `AppCx`. The real implementation is `impl ... for AppCx` below, the one place that reads the session, the audit log and the
//! settings store (so the pages' old tests, which hand them an `AppCx`, still run through it). [`FakeSettingsPort`] is the fake: in-memory
//! settings, an audit list, an administrator switch and a manual clock.

use std::path::PathBuf;

use iyakki_shared::settings::Settings;

use super::AppCx;

/// Why a change to the settings did not happen.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum SettingsWriteError {
    /// Somebody else holds the settings right now.
    Busy,
    /// The settings refused the change (a rule, or a file that cannot be written); the words are for the person.
    Refused(String),
}

impl SettingsWriteError {
    /// The sentence a page shows.
    pub(crate) fn message(&self) -> String {
        match self {
            SettingsWriteError::Busy => "The settings are busy; try again.".to_string(),
            SettingsWriteError::Refused(m) => m.clone(),
        }
    }
}

/// What every page needs to know and may do.
pub(crate) trait PagePort {
    /// The person manages the system (administrators): may change the machine's settings.
    fn is_admin(&self) -> bool;
    /// A line in the audit log for the signed-in person.
    fn audit(&self, action: &str, subject: &str);
    /// The data folder of this install (the page groups after Radio and Battery use it).
    #[allow(dead_code)]
    fn data_dir(&self) -> PathBuf;
    /// The clock, seconds since 1970.
    fn now_secs(&self) -> u64;
}

/// The machine's settings file.
pub(crate) trait SettingsDataPort: PagePort {
    /// A copy of the settings now (the defaults when they cannot be read).
    fn settings_get(&self) -> Settings;
    /// Change the settings with `f`; the change is checked and saved like any other.
    fn settings_update(&self, f: &mut dyn FnMut(&mut Settings)) -> Result<(), SettingsWriteError>;
}

impl PagePort for AppCx {
    fn is_admin(&self) -> bool {
        self.session.policy.manage_settings
    }
    fn audit(&self, action: &str, subject: &str) {
        self.audit.record(&self.session.user, action, subject);
    }
    fn data_dir(&self) -> PathBuf {
        AppCx::data_dir(self).to_path_buf()
    }
    fn now_secs(&self) -> u64 {
        iyakki_clock::now_unix().max(0) as u64
    }
}

impl SettingsDataPort for AppCx {
    fn settings_get(&self) -> Settings {
        self.settings().lock().map(|s| s.get().clone()).unwrap_or_default()
    }
    fn settings_update(&self, f: &mut dyn FnMut(&mut Settings)) -> Result<(), SettingsWriteError> {
        match self.settings().lock().map(|mut s| s.update(|s| f(s))) {
            Ok(Ok(())) => Ok(()),
            Ok(Err(e)) => Err(SettingsWriteError::Refused(e.to_string())),
            Err(_) => Err(SettingsWriteError::Busy),
        }
    }
}

/// The fake port: administrator or not, in-memory settings (a change is checked with `validate`, like the real store), an audit list and a
/// manual clock. Nothing is read from the disk and nothing sleeps.
#[cfg(test)]
pub(crate) struct FakeSettingsPort {
    pub admin: std::sync::atomic::AtomicBool,
    pub settings: std::sync::Mutex<Settings>,
    pub audit_lines: std::sync::Mutex<Vec<(String, String)>>,
    pub clock: iyakki_testkit::ManualClock,
    #[allow(dead_code)]
    pub dir: PathBuf,
    /// Holds the settings busy (every change answers `Busy`) while set.
    pub busy: std::sync::atomic::AtomicBool,
}

#[cfg(test)]
impl FakeSettingsPort {
    pub(crate) fn new(admin: bool) -> FakeSettingsPort {
        FakeSettingsPort {
            admin: admin.into(),
            settings: std::sync::Mutex::new(Settings::default()),
            audit_lines: Default::default(),
            clock: iyakki_testkit::ManualClock::new(1_000_000),
            dir: PathBuf::from("fake-data"),
            busy: false.into(),
        }
    }
    pub(crate) fn audited(&self) -> Vec<(String, String)> {
        self.audit_lines.lock().unwrap().clone()
    }
}

#[cfg(test)]
impl PagePort for FakeSettingsPort {
    fn is_admin(&self) -> bool {
        self.admin.load(std::sync::atomic::Ordering::SeqCst)
    }
    fn audit(&self, action: &str, subject: &str) {
        self.audit_lines.lock().unwrap().push((action.to_string(), subject.to_string()));
    }
    fn data_dir(&self) -> PathBuf {
        self.dir.clone()
    }
    fn now_secs(&self) -> u64 {
        self.clock.now_secs()
    }
}

#[cfg(test)]
impl SettingsDataPort for FakeSettingsPort {
    fn settings_get(&self) -> Settings {
        self.settings.lock().unwrap().clone()
    }
    fn settings_update(&self, f: &mut dyn FnMut(&mut Settings)) -> Result<(), SettingsWriteError> {
        if self.busy.load(std::sync::atomic::Ordering::SeqCst) {
            return Err(SettingsWriteError::Busy);
        }
        let mut cur = self.settings.lock().unwrap();
        let mut next = cur.clone();
        f(&mut next);
        next.validate().map_err(|e| SettingsWriteError::Refused(e.to_string()))?;
        *cur = next;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_fake_keeps_settings_in_memory_audits_and_tells_busy_from_refused() {
        let p = FakeSettingsPort::new(true);
        assert!(p.is_admin());
        p.settings_update(&mut |s| s.radio.retries = Some(4)).unwrap();
        assert_eq!(p.settings_get().radio.retries(), 4);
        p.audit("settings.radio", "retries=4");
        assert_eq!(p.audited(), vec![("settings.radio".to_string(), "retries=4".to_string())]);
        p.busy.store(true, std::sync::atomic::Ordering::SeqCst);
        assert_eq!(p.settings_update(&mut |s| s.radio.retries = Some(1)), Err(SettingsWriteError::Busy));
        assert_eq!(p.settings_get().radio.retries(), 4, "a busy answer changes nothing");
        assert!(SettingsWriteError::Busy.message().contains("busy"));
    }

    #[test]
    fn the_fake_clock_moves_only_when_the_test_moves_it() {
        let p = FakeSettingsPort::new(false);
        assert!(!p.is_admin());
        let t0 = p.now_secs();
        assert_eq!(p.now_secs(), t0);
        p.clock.advance(std::time::Duration::from_secs(90));
        assert_eq!(p.now_secs(), t0 + 90);
    }
}
