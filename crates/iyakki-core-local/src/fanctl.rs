//! The fan service's setting file as a typed handle (F-ARCH-07): Settings -> Power and the chat tools `fan_control_status` and
//! `fan_control_set` read and write `fan.json` and the service's report through [`Fan`], and ask it whether this computer has a fan
//! IYAKKI can run. They never name the drivers crate. The service itself (`fanservice::run`) is started by init and stays in the
//! drivers crate.

use std::path::{Path, PathBuf};

pub use iyakki_drivers::fanservice::Setting;
/// The setting file inside the folder.
pub const SETTING_FILE: &str = iyakki_drivers::fanservice::SETTING_FILE;

/// The fan setting folder (the settings directory the service reads).
#[derive(Clone, Debug)]
pub struct Fan {
    dir: PathBuf,
}

impl Fan {
    pub fn in_dir(dir: &Path) -> Fan {
        Fan { dir: dir.to_path_buf() }
    }
    /// Does this computer have an applesmc fan with a temperature input below the given `/sys` root?
    pub fn supported(sys: &Path) -> bool {
        iyakki_drivers::fanservice::supported(sys)
    }
    /// The saved setting; off when the file is missing.
    pub fn setting(&self) -> Setting {
        iyakki_drivers::fanservice::load(&self.dir)
    }
    /// Save a setting whose curve is valid (atomic write).
    pub fn save(&self, s: &Setting) -> Result<(), String> {
        iyakki_drivers::fanservice::save(&self.dir, s)
    }
    /// What the service last reported, one line.
    pub fn report(&self) -> String {
        iyakki_drivers::fanservice::status(&self.dir)
    }
    /// Turn control on or off the other way; the new state.
    pub fn toggle(&self) -> Result<bool, String> {
        let mut s = self.setting();
        s.enabled = !s.enabled;
        self.save(&s)?;
        Ok(s.enabled)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dir(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("iyakki-fanctl-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        d
    }

    #[test]
    fn nothing_is_on_until_saved_and_a_toggle_flips_and_keeps_the_curve() {
        let d = dir("toggle");
        let f = Fan::in_dir(&d);
        assert!(!f.setting().enabled);
        assert!(f.report().contains("not reported"));
        assert_eq!(f.toggle(), Ok(true));
        assert!(f.setting().enabled);
        let mut s = f.setting();
        s.low = 45;
        f.save(&s).unwrap();
        assert_eq!(f.toggle(), Ok(false));
        assert_eq!(f.setting().low, 45);
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn a_curve_that_is_not_valid_is_refused_and_nothing_is_written() {
        let d = dir("invalid");
        let f = Fan::in_dir(&d);
        let mut s = Setting::default();
        s.low = 100;
        s.high = 50;
        assert!(f.save(&s).unwrap_err().contains("not valid"));
        assert!(!d.exists());
    }

    #[test]
    fn a_computer_without_applesmc_has_no_fan_to_run() {
        let d = dir("nosys");
        std::fs::create_dir_all(&d).unwrap();
        assert!(!Fan::supported(&d));
        let _ = std::fs::remove_dir_all(&d);
    }
}
