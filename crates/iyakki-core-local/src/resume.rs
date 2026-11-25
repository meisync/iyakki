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
/// The marker "somebody is signed in" (F-LIFE-16): no name, no app, nothing to restore; it only lets the next shell say that the screen
/// ended unexpectedly. Written for everybody (also when no ticket may be written), removed at every sign-out.
const OPEN: &str = "session-open.json";

/// The kernel's id of this boot (empty when it cannot be read).
pub fn boot_id() -> String {
    std::fs::read_to_string(Path::new("/proc").join("sys/kernel/random/boot_id")).map(|s| s.trim().to_string()).unwrap_or_default()
}

/// An app id as the shell names it: short, lower-case letters, digits, `-` and `_`.
fn app_id_ok(s: &str) -> bool {
    !s.is_empty() && s.len() <= 24 && s.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-' || c == '_')
}

/// The thing an app holds (a file path): short plain text.
fn item_ok(s: &str) -> bool {
    !s.is_empty() && s.len() <= MAX_ITEM && !s.chars().any(char::is_control)
}

/// The longest item, in bytes.
pub const MAX_ITEM: usize = 512;

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
        let items = self.items_of(user, now);
        let v = serde_json::json!({"user": user, "boot": self.boot, "seen": now, "apps": apps, "items": items});
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
        let items = self.items_of(user, now);
        let v = serde_json::json!({"user": user, "boot": self.boot, "seen": now, "apps": apps, "items": items});
        let tmp = self.dir.join(format!("{FILE}.tmp"));
        std::fs::write(&tmp, v.to_string()).map_err(|e| e.to_string())?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o600));
        }
        std::fs::rename(&tmp, self.file()).map_err(|e| e.to_string())
    }

    fn items_of(&self, user: &str, now: u64) -> serde_json::Map<String, serde_json::Value> {
        let Some(v) = std::fs::read_to_string(self.file()).ok().and_then(|t| serde_json::from_str::<serde_json::Value>(&t).ok()) else { return Default::default() };
        if v["user"].as_str() != Some(user) || v["boot"].as_str() != Some(self.boot.as_str()) || v["seen"].as_u64().is_none_or(|s| now < s || now - s > MAX_AGE) {
            return Default::default();
        }
        v["items"].as_object().map(|o| o.iter().filter(|(k, x)| app_id_ok(k) && x.as_str().is_some_and(item_ok)).take(MAX_APPS).map(|(k, x)| (k.clone(), x.clone())).collect()).unwrap_or_default()
    }

    /// Say what each open app holds (an app id and the thing it shows, such as the path of the file an editor has open: text only, the app's
    /// own words for it, never a password or the text being typed). Refused when the ticket is not fresh or names someone else. Items of apps
    /// that are not in the ticket's list are dropped.
    pub fn remember_items(&self, user: &str, items: &[(String, String)], now: u64) -> Result<(), String> {
        if self.who(now).as_deref() != Some(user) {
            return Err("there is no fresh ticket for this person".into());
        }
        let apps = self.apps_of(user, now);
        let kept: serde_json::Map<String, serde_json::Value> = items.iter().filter(|(a, i)| app_id_ok(a) && item_ok(i) && apps.contains(a)).take(MAX_APPS).map(|(a, i)| (a.clone(), serde_json::Value::String(i.clone()))).collect();
        let v = serde_json::json!({"user": user, "boot": self.boot, "seen": now, "apps": apps, "items": kept});
        let tmp = self.dir.join(format!("{FILE}.tmp"));
        std::fs::write(&tmp, v.to_string()).map_err(|e| e.to_string())?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o600));
        }
        std::fs::rename(&tmp, self.file()).map_err(|e| e.to_string())
    }

    /// The (app id, item) pairs of the ticket to put back, when it is fresh.
    pub fn items(&self, now: u64) -> Vec<(String, String)> {
        match self.who(now) {
            Some(u) => self.items_of(&u, now).into_iter().filter_map(|(k, v)| v.as_str().map(|s| (k, s.to_string()))).collect(),
            None => vec![],
        }
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
        let _ = std::fs::remove_file(self.dir.join(OPEN));
    }

    /// Say that somebody is signed in (called again every few seconds with the ticket; no name, nothing to restore).
    pub fn note_open(&self, now: u64) -> Result<(), String> {
        if self.boot.is_empty() {
            return Err("the boot id is unknown".into());
        }
        let v = serde_json::json!({"boot": self.boot, "seen": now});
        let (p, tmp) = (self.dir.join(OPEN), self.dir.join(format!("{OPEN}.tmp")));
        let res = (|| -> std::io::Result<()> {
            use std::io::Write;
            let mut f = std::fs::File::create(&tmp)?;
            f.write_all(v.to_string().as_bytes())?;
            f.sync_all()?;
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                let _ = std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o600));
            }
            std::fs::rename(&tmp, &p)
        })();
        if res.is_err() {
            let _ = std::fs::remove_file(&tmp);
        }
        res.map_err(|e| e.to_string())
    }

    /// Did the shell end while somebody was signed in, without a sign-out, in this boot? Takes the marker: true once, then false.
    /// (A marker of an earlier boot is removed and says nothing: the machine was restarted, not the screen.)
    pub fn take_unexpected_end(&self) -> bool {
        let p = self.dir.join(OPEN);
        let v = std::fs::read_to_string(&p).ok().and_then(|t| serde_json::from_str::<serde_json::Value>(&t).ok());
        let _ = std::fs::remove_file(&p);
        v.and_then(|v| v["boot"].as_str().map(|b| !self.boot.is_empty() && b == self.boot)).unwrap_or(false)
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
    fn what_an_app_holds_rides_in_the_ticket_as_plain_text_for_listed_apps_and_goes_with_it() {
        let d = tmp("items");
        let r = Resume::with_boot(&d, "boot-1");
        r.remember("alice", 1000).unwrap();
        r.remember_apps("alice", &["edit".into(), "chat".into()], 1001).unwrap();
        let items = vec![("edit".to_string(), "/home/alice/notes.txt".to_string()), ("files".to_string(), "/x".to_string()), ("chat".to_string(), "a\nb".to_string()), ("edit2".to_string(), "x".repeat(MAX_ITEM + 1))];
        r.remember_items("alice", &items, 1002).unwrap();
        assert_eq!(r.items(1003), vec![("edit".to_string(), "/home/alice/notes.txt".to_string())], "only listed apps, only plain text");
        r.remember("alice", 1010).unwrap();
        assert_eq!(r.items(1011).len(), 1, "a refresh keeps them");
        r.remember_apps("alice", &["chat".into()], 1012).unwrap();
        assert!(r.items(1013).len() <= 1);
        assert!(r.remember_items("bob", &items, 1013).is_err());
        assert!(Resume::with_boot(&d, "boot-2").items(1013).is_empty(), "another boot");
        assert!(r.items(1010 + MAX_AGE + 5).is_empty(), "stale");
        r.forget();
        assert!(r.items(1013).is_empty());
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

    #[test]
    fn a_marker_of_a_signed_in_person_is_taken_once_after_the_shell_ended_and_a_sign_out_removes_it() {
        let d = tmp("open-marker");
        let r = Resume::with_boot(&d, "boot-1");
        assert!(!r.take_unexpected_end(), "nobody was signed in");
        r.note_open(100).unwrap();
        let text = std::fs::read_to_string(d.join("session-open.json")).unwrap();
        assert!(!text.contains("alice") && text.contains("boot-1"), "no name in it: {text}");
        assert!(r.take_unexpected_end(), "the shell ended without a sign-out");
        assert!(!r.take_unexpected_end(), "taken once");
        r.note_open(200).unwrap();
        r.forget();
        assert!(!r.take_unexpected_end(), "a sign-out is not a crash");
        assert!(!d.join("session-open.json.tmp").exists());
    }

    #[test]
    fn a_marker_of_an_earlier_boot_says_nothing_and_is_removed() {
        let d = tmp("open-marker-boot");
        Resume::with_boot(&d, "boot-1").note_open(100).unwrap();
        let later = Resume::with_boot(&d, "boot-2");
        assert!(!later.take_unexpected_end(), "the machine was restarted, not the screen");
        assert!(!d.join("session-open.json").exists());
        assert!(Resume::with_boot(&d, "").note_open(1).is_err(), "an unknown boot id writes nothing");
    }
}
