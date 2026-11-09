//! The lock screen's choices and requests: when the screen locks by itself, and how another part of the system (Settings, the assistant)
//! asks the shell to lock now.
//!
//! Pure data, no drawing, no passwords: the shell (`iyakki-shell`: `shell_lock.rs`) draws the lock and checks the password with the
//! same account store the sign-in screen uses. Everyone keeps their own choice in `lockscreen-<user>.json` next to their other interface
//! choices. Locking does not sign anyone out: the person's apps and runs go on; only the screen and the personal vault are closed
//! until the password is typed.
//!
//! The default is "never lock by itself". IYAKKI is also run unattended (a kiosk, a remote browser session, a build box) where a lock
//! nobody asked for would close the vault under a flow that needs a key; one click in Settings -> Display & sound -> Screen saver turns
//! it on. Locking on request (Super+L, the Power menu, the assistant) and locking when the computer goes to sleep need no setting.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// Minutes of no input before the screen locks itself (0 = never).
pub const AUTO_CHOICES: [u32; 6] = [0, 5, 10, 15, 30, 60];

/// One person's choice.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct LockConfig {
    /// Lock after this many minutes without input (0 = never).
    pub auto_min: u32,
}

impl LockConfig {
    /// The same choice snapped to one that is offered (a hand-edited file cannot make a strange delay).
    pub fn normalized(mut self) -> LockConfig {
        if !AUTO_CHOICES.contains(&self.auto_min) {
            self.auto_min = AUTO_CHOICES.iter().copied().min_by_key(|c| c.abs_diff(self.auto_min)).unwrap_or(0);
        }
        self
    }

    /// Seconds of no input before the lock, `None` = never.
    pub fn after_secs(&self) -> Option<u64> {
        (self.auto_min > 0).then(|| u64::from(self.auto_min) * 60)
    }

    /// The saved choice; a missing or unreadable file means never.
    pub fn load(dir: &Path, user: &str) -> LockConfig {
        std::fs::read_to_string(config_path(dir, user)).ok().and_then(|t| serde_json::from_str::<LockConfig>(&t).ok()).unwrap_or_default().normalized()
    }

    pub fn save(&self, dir: &Path, user: &str) -> Result<(), String> {
        let p = config_path(dir, user);
        let tmp = p.with_extension("tmp");
        let bytes = serde_json::to_vec_pretty(&self.clone().normalized()).map_err(|e| e.to_string())?;
        std::fs::write(&tmp, bytes).map_err(|e| format!("{}: {e}", tmp.display()))?;
        std::fs::rename(&tmp, &p).map_err(|e| format!("{}: {e}", p.display()))
    }

    /// "off" or "after 15 minutes", for the Settings page, the assistant and the audit log.
    pub fn describe(&self) -> String {
        if self.auto_min == 0 { "never by itself".to_string() } else { format!("after {} minutes without input", self.auto_min) }
    }
}

fn who(user: &str) -> String {
    user.chars().filter(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.')).collect()
}

fn config_path(dir: &Path, user: &str) -> PathBuf {
    dir.join(format!("lockscreen-{}.json", who(user)))
}

fn request_path(dir: &Path, user: &str) -> PathBuf {
    dir.join(format!("lockscreen-request-{}", who(user)))
}

/// Ask the shell showing `user` to lock the screen now (Settings -> Lock now, the assistant's `lock_screen`). The shell and its
/// callers share nothing but the data folder.
pub fn request(dir: &Path, user: &str) {
    let _ = std::fs::write(request_path(dir, user), b"lock\n");
}

/// The shell of `user` takes its request; true once per request. Cheap: one `stat` while nothing is asked.
pub fn take_request(dir: &Path, user: &str) -> bool {
    std::fs::remove_file(request_path(dir, user)).is_ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("iyakki-lockcfg-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).expect("temp dir");
        d
    }

    #[test]
    fn the_default_is_never_and_strange_values_snap_to_a_choice() {
        assert_eq!(LockConfig::default().after_secs(), None);
        assert_eq!(LockConfig { auto_min: 15 }.after_secs(), Some(900));
        assert_eq!(LockConfig { auto_min: 14 }.normalized().auto_min, 15);
        assert_eq!(LockConfig { auto_min: 1 }.normalized().auto_min, 0);
        assert!(AUTO_CHOICES.contains(&LockConfig { auto_min: u32::MAX }.normalized().auto_min));
    }

    #[test]
    fn the_choice_is_kept_per_person_and_a_broken_file_means_never() {
        let d = temp("save");
        assert_eq!(LockConfig::load(&d, "amy").auto_min, 0);
        LockConfig { auto_min: 10 }.save(&d, "amy").expect("saved");
        assert_eq!(LockConfig::load(&d, "amy").auto_min, 10);
        assert_eq!(LockConfig::load(&d, "bob").auto_min, 0, "bob has his own (none)");
        std::fs::write(d.join("lockscreen-amy.json"), "{ not json").expect("write");
        assert_eq!(LockConfig::load(&d, "amy").auto_min, 0);
        std::fs::write(d.join("lockscreen-amy.json"), r#"{"auto_min":30,"future":true}"#).expect("write");
        assert_eq!(LockConfig::load(&d, "amy").auto_min, 30, "a field from a later version is ignored");
        LockConfig { auto_min: 5 }.save(&d, "../evil").expect("saved");
        assert!(d.join("lockscreen-..evil.json").is_file(), "a name cannot leave the folder");
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn a_lock_request_reaches_only_the_shell_of_that_person_once() {
        let d = temp("request");
        request(&d, "amy");
        assert!(!take_request(&d, "bob"), "not bob's");
        assert!(take_request(&d, "amy"));
        assert!(!take_request(&d, "amy"), "taken once");
        let _ = std::fs::remove_dir_all(&d);
    }
}
