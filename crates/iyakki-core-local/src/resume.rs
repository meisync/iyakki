//! Put the person back at the desktop after the shell process died (F-SHELL-65, owner decision 2026-11-16: allowed on a developer and
//! test machine). A small ticket in the data folder says who was signed in and when the shell last said so. It is:
//!
//! * short-lived: valid [`MAX_AGE`] seconds after the shell last refreshed it (the shell refreshes it every few seconds while somebody
//!   is signed in), so a machine left alone is not left signed in;
//! * per boot: it carries the kernel's boot id and is worth nothing after a restart of the machine;
//! * one person only, removed at every sign-out, and never holding a password or a key. A person resumed this way has NOT typed the
//!   password, so the vault stays locked until they do (the caller signs in with an empty password);
//! * written only when the caller says the machine may (developer mode on and the Settings switch on): the caller decides, this module
//!   only keeps the file (mode 600).

use std::path::{Path, PathBuf};

/// Seconds a ticket stays good after the shell last refreshed it.
pub const MAX_AGE: u64 = 90;
const FILE: &str = "resume-session.json";
/// The most apps a ticket holds (the one held aside and the one on the screen, with room for panes).
pub const MAX_APPS: usize = 8;
const SWITCH: &str = "resume-after-crash.enabled";

/// The kernel's id of this boot (empty when it cannot be read).
pub fn boot_id() -> String {
    std::fs::read_to_string(Path::new("/proc").join("sys/kernel/random/boot_id")).map(|s| s.trim().to_string()).unwrap_or_default()
}

/// An app id as the shell names it: short, lower-case letters, digits, `-` and `_`.
fn app_id_ok(s: &str) -> bool {
    !s.is_empty() && s.len() <= 24 && s.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-' || c == '_')
}

pub struct Resume {
    dir: PathBuf,
    boot: String,
}

impl Resume {
    pub fn new(dir: &Path) -> Resume {
        Resume { dir: dir.to_path_buf(), boot: boot_id() }
    }

    /// A test names the boot.
    pub fn with_boot(dir: &Path, boot: &str) -> Resume {
        Resume { dir: dir.to_path_buf(), boot: boot.to_string() }
    }

    fn file(&self) -> PathBuf {
        self.dir.join(FILE)
    }

    /// The Settings switch (a file in the data folder; off when absent).
    pub fn switch_on(&self) -> bool {
        self.dir.join(SWITCH).exists()
    }

    pub fn set_switch(&self, on: bool) -> Result<(), String> {
        let p = self.dir.join(SWITCH);
        if on {
            std::fs::write(&p, b"on\n").map_err(|e| e.to_string())
        } else {
            let _ = std::fs::remove_file(&p);
            self.forget();
            Ok(())
        }
    }

    /// Say who is signed in now (called again every few seconds). Nothing when the boot id is unknown.
    pub fn remember(&self, user: &str, now: u64) -> Result<(), String> {
        if self.boot.is_empty() || user.is_empty() || user.len() > 64 || user.chars().any(char::is_control) {
            return Err("no ticket for this person".into());
        }
        // the open apps stay in the ticket while the same person refreshes it
        let apps = self.apps_of(user, now);
        let v = serde_json::json!({"user": user, "boot": self.boot, "seen": now, "apps": apps});
        let tmp = self.dir.join(format!("{FILE}.tmp"));
        std::fs::write(&tmp, v.to_string()).map_err(|e| e.to_string())?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o600));
        }
        std::fs::rename(&tmp, self.file()).map_err(|e| e.to_string())
    }

    fn apps_of(&self, user: &str, now: u64) -> Vec<String> {
        let Some(v) = std::fs::read_to_string(self.file()).ok().and_then(|t| serde_json::from_str::<serde_json::Value>(&t).ok()) else { return vec![] };
        if v["user"].as_str() != Some(user) || v["boot"].as_str() != Some(self.boot.as_str()) || v["seen"].as_u64().is_none_or(|s| now < s || now - s > MAX_AGE) {
            return vec![];
        }
        v["apps"].as_array().map(|a| a.iter().filter_map(|x| x.as_str()).filter(|x| app_id_ok(x)).take(MAX_APPS).map(String::from).collect()).unwrap_or_default()
    }

    /// Say which apps are open (their ids only), for the person the ticket names. Refused when the ticket is not fresh or names someone else.
    pub fn remember_apps(&self, user: &str, apps: &[String], now: u64) -> Result<(), String> {
        let apps: Vec<&String> = apps.iter().filter(|a| app_id_ok(a)).take(MAX_APPS).collect();
        if self.who(now).as_deref() != Some(user) {
            return Err("there is no fresh ticket for this person".into());
        }
        let v = serde_json::json!({"user": user, "boot": self.boot, "seen": now, "apps": apps});
        let tmp = self.dir.join(format!("{FILE}.tmp"));
        std::fs::write(&tmp, v.to_string()).map_err(|e| e.to_string())?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o600));
        }
        std::fs::rename(&tmp, self.file()).map_err(|e| e.to_string())
    }

    /// The app ids to open again, when the ticket is fresh (otherwise none).
    pub fn apps(&self, now: u64) -> Vec<String> {
        match self.who(now) {
            Some(u) => self.apps_of(&u, now),
            None => vec![],
        }
    }

    /// The person to put back, if a ticket of this boot is fresh. The ticket stays until the person signs out or it goes stale.
    pub fn who(&self, now: u64) -> Option<String> {
        let v: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(self.file()).ok()?).ok()?;
        let (user, boot, seen) = (v["user"].as_str()?, v["boot"].as_str()?, v["seen"].as_u64()?);
        (!self.boot.is_empty() && boot == self.boot && now >= seen && now - seen <= MAX_AGE && !user.is_empty()).then(|| user.to_string())
    }

    pub fn forget(&self) {
        let _ = std::fs::remove_file(self.file());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp(n: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("iyakki-resume-{n}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn a_fresh_ticket_of_this_boot_names_the_person_and_goes_stale_and_is_forgotten() {
        let d = tmp("a");
        let r = Resume::with_boot(&d, "boot-1");
        assert_eq!(r.who(1000), None);
        r.remember("alice", 1000).unwrap();
        assert_eq!(r.who(1000).as_deref(), Some("alice"));
        assert_eq!(r.who(1000 + MAX_AGE).as_deref(), Some("alice"));
        assert_eq!(r.who(1000 + MAX_AGE + 1), None, "stale");
        r.remember("alice", 2000).unwrap();
        assert_eq!(r.who(2010).as_deref(), Some("alice"), "refreshing keeps it good");
        r.forget();
        assert_eq!(r.who(2010), None, "a sign-out removes it");
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn the_open_apps_ride_in_the_ticket_for_the_same_person_only_and_go_with_it() {
        let d = tmp("apps");
        let r = Resume::with_boot(&d, "boot-1");
        assert!(r.remember_apps("alice", &["chat".into()], 5).is_err(), "no ticket yet");
        r.remember("alice", 1000).unwrap();
        r.remember_apps("alice", &["ide".into(), "chat".into(), "Bad Id!".into(), "x".repeat(40)], 1001).unwrap();
        assert_eq!(r.apps(1002), vec!["ide".to_string(), "chat".to_string()], "only plain ids, in order");
        r.remember("alice", 1010).unwrap();
        assert_eq!(r.apps(1011), vec!["ide".to_string(), "chat".to_string()], "a refresh keeps them");
        assert!(r.remember_apps("bob", &["chat".into()], 1012).is_err());
        assert_eq!(Resume::with_boot(&d, "boot-2").apps(1012), Vec::<String>::new(), "another boot");
        assert_eq!(r.apps(1010 + MAX_AGE + 1), Vec::<String>::new(), "stale");
        r.remember("bob", 1020).unwrap();
        assert!(r.apps(1021).is_empty(), "another person starts with none");
        r.forget();
        assert!(r.apps(1021).is_empty());
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn a_ticket_is_worth_nothing_after_another_boot_or_with_a_clock_gone_back_or_a_bad_name() {
        let d = tmp("b");
        Resume::with_boot(&d, "boot-1").remember("alice", 1000).unwrap();
        assert_eq!(Resume::with_boot(&d, "boot-2").who(1001), None);
        assert_eq!(Resume::with_boot(&d, "boot-1").who(999), None);
        assert!(Resume::with_boot(&d, "boot-1").remember("", 1).is_err());
        assert!(Resume::with_boot(&d, "boot-1").remember("a\nb", 1).is_err());
        assert!(Resume::with_boot(&d, "").remember("alice", 1).is_err(), "an unknown boot id writes nothing");
        std::fs::write(d.join(FILE), "not json").unwrap();
        assert_eq!(Resume::with_boot(&d, "boot-1").who(1), None);
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn the_switch_is_off_until_set_and_turning_it_off_removes_the_ticket_and_the_file_is_private() {
        let d = tmp("c");
        let r = Resume::with_boot(&d, "b");
        assert!(!r.switch_on());
        r.set_switch(true).unwrap();
        assert!(r.switch_on());
        r.remember("bob", 5).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(std::fs::metadata(d.join(FILE)).unwrap().permissions().mode() & 0o777, 0o600);
        }
        r.set_switch(false).unwrap();
        assert!(!r.switch_on() && r.who(6).is_none());
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn the_real_boot_id_is_read_where_there_is_one() {
        let id = boot_id();
        if cfg!(target_os = "linux") {
            assert!(!id.is_empty());
        }
    }
}
