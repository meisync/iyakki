//! Scheduled backups (F-BACKUP-01): "back up every N days at this hour into this folder, keep the newest few". The data is a small file
//! (`backup-schedule-<user>.json`, mode 0600); the shell looks at it a few times a minute while the person is signed in and runs
//! [`run`] on a worker thread when [`Schedule::due`] says so. The backup itself is the one of `backup` (the same places, the same rules, a file
//! under a `.part` name that is renamed when complete, nothing over an existing file).
//!
//! An unattended run cannot ask for a passphrase, and one must never travel through a chat. So an encrypted schedule reads the passphrase from a
//! file the person made (`passphrase_file`, an absolute path, 8 or more characters on its first line); with no file the backups are NOT
//! encrypted, and the status, the page and the chat card say so. The passphrase is read for the run and dropped; it is never written anywhere here.
//! A run only starts while the person is signed in (their vault and files are theirs to read); a computer nobody signs in to makes no scheduled backup.

use std::path::{Path, PathBuf};
use std::sync::atomic::Ordering;

use serde::{Deserialize, Serialize};

use crate::backup::{self as bk, Created, Layout, Progress};
use iyakki_clock::Zone;

/// Longest gap between two scheduled backups, in days.
pub const MAX_EVERY_DAYS: u32 = 31;
/// Most scheduled backups kept in the folder (older ones of this account are removed after a good run).
pub const MAX_KEEP: u32 = 50;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Schedule {
    pub enabled: bool,
    /// A backup every this many days (1 to 31).
    pub every_days: u32,
    /// From this hour of the day (0 to 23, the computer's local time); a computer that was off then runs it when the person next signs in.
    pub hour: u8,
    /// The folder the files go to: absolute, and it must exist (a stick, another disk, a mounted share).
    pub dest: String,
    /// Keep this many of the scheduled files of this account in the folder (1 to 50); older ones are removed after a good run.
    pub keep: u32,
    /// The things to include (ids of `backup::AREAS`); empty: all.
    pub areas: Vec<String>,
    /// A file holding the passphrase on its first line; empty: the files are not encrypted.
    pub passphrase_file: String,
    /// When the last run was tried (seconds since the epoch; 0: never) and how it ended, in words.
    pub last_run: i64,
    pub last_result: String,
}

impl Default for Schedule {
    fn default() -> Schedule {
        Schedule { enabled: false, every_days: 1, hour: 3, dest: String::new(), keep: 7, areas: Vec::new(), passphrase_file: String::new(), last_run: 0, last_result: String::new() }
    }
}

fn who(user: &str) -> String {
    user.chars().filter(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.')).collect()
}

fn path(dir: &Path, user: &str) -> PathBuf {
    dir.join(format!("backup-schedule-{}.json", who(user)))
}

fn absolute_plain(p: &str) -> bool {
    let q = Path::new(p);
    q.is_absolute() && !p.contains('\0') && !q.components().any(|c| matches!(c, std::path::Component::ParentDir))
}

impl Schedule {
    pub fn load(dir: &Path, user: &str) -> Schedule {
        std::fs::read_to_string(path(dir, user)).ok().and_then(|t| serde_json::from_str::<Schedule>(&t).ok()).unwrap_or_default().repaired()
    }

    pub fn save(&self, dir: &Path, user: &str) -> Result<(), String> {
        let p = path(dir, user);
        let tmp = p.with_extension("tmp");
        let text = serde_json::to_string_pretty(self).map_err(|e| e.to_string())?;
        std::fs::write(&tmp, text).map_err(|e| format!("could not save the schedule: {e}"))?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o600));
        }
        std::fs::rename(&tmp, &p).map_err(|e| format!("could not save the schedule: {e}"))
    }

    /// Numbers pulled back inside their limits (a hand-edited file never breaks the schedule).
    fn repaired(mut self) -> Schedule {
        self.every_days = self.every_days.clamp(1, MAX_EVERY_DAYS);
        self.hour = self.hour.min(23);
        self.keep = self.keep.clamp(1, MAX_KEEP);
        self.areas.retain(|a| bk::AREAS.iter().any(|(id, _)| id == a));
        self
    }

    /// Whether the settings can run: a folder (absolute), a passphrase file that is absolute when named, and only known things to include.
    pub fn validate(&self) -> Result<(), String> {
        if !(1..=MAX_EVERY_DAYS).contains(&self.every_days) {
            return Err(format!("every_days is 1 to {MAX_EVERY_DAYS}"));
        }
        if self.hour > 23 {
            return Err("hour is 0 to 23".into());
        }
        if !(1..=MAX_KEEP).contains(&self.keep) {
            return Err(format!("keep is 1 to {MAX_KEEP}"));
        }
        if !absolute_plain(&self.dest) {
            return Err("Choose the folder to write to (an absolute path, without ..)".into());
        }
        if !self.passphrase_file.is_empty() && !absolute_plain(&self.passphrase_file) {
            return Err("The passphrase file is an absolute path, without ..".into());
        }
        if let Some(bad) = self.areas.iter().find(|a| !bk::AREAS.iter().any(|(id, _)| id == a)) {
            return Err(format!("\"{bad}\" is not something a backup holds"));
        }
        Ok(())
    }

    /// The first moment a run is wanted: `hour` o'clock, `every_days` days after the day of the last run (never run: today's hour).
    pub fn next_slot(&self, zone: &Zone, now: i64) -> i64 {
        let from = if self.last_run > 0 { self.last_run } else { now - self.every_days as i64 * 86_400 };
        let l = zone.local(from);
        let day = zone.local_to_unix(l.year, l.month, l.day, 12, 0, 0) + self.every_days as i64 * 86_400;
        let t = zone.local(day);
        zone.local_to_unix(t.year, t.month, t.day, self.hour as u32, 0, 0)
    }

    /// Is a run wanted now?
    pub fn due(&self, zone: &Zone, now: i64) -> bool {
        self.enabled && self.validate().is_ok() && now >= self.next_slot(zone, now)
    }

    /// One line for the status of the page and the chat.
    pub fn describe(&self, zone: &Zone, now: i64) -> String {
        if !self.enabled {
            return "No scheduled backup.".into();
        }
        let next = iyakki_clock::format(zone, self.next_slot(zone, now), "%d %b %H:%M");
        format!(
            "Every {} day(s) from {:02}:00 into {}, keeping the newest {}; {}. Next: {next}. Last: {}.",
            self.every_days,
            self.hour,
            self.dest,
            self.keep,
            if self.passphrase_file.is_empty() { "NOT encrypted (no passphrase file)" } else { "encrypted with the passphrase in the file you named" },
            if self.last_run == 0 { "never".to_string() } else { format!("{} - {}", iyakki_clock::format(zone, self.last_run, "%d %b %H:%M"), self.last_result) }
        )
    }
}

/// The passphrase of the file: its first line, 8 or more characters. The text is returned for this run only.
fn read_passphrase(file: &str) -> Result<String, String> {
    use std::io::Read;
    let mut buf = String::new();
    std::fs::File::open(file).and_then(|f| f.take(1024).read_to_string(&mut buf)).map_err(|e| format!("The passphrase file cannot be read: {e}"))?;
    let line = buf.lines().next().unwrap_or("").trim_end_matches('\r').to_string();
    if line.chars().count() < bk::MIN_PASSPHRASE {
        return Err(format!("The first line of the passphrase file needs at least {} characters", bk::MIN_PASSPHRASE));
    }
    Ok(line)
}

/// Remove the oldest scheduled files of this account in `dest` beyond the newest `keep`. Only files named like the ones a backup makes
/// (`iyakki-backup-<account>-<stamp>.iybak`) are touched. Returns how many went.
pub fn prune(dest: &Path, user: &str, keep: u32) -> usize {
    let prefix = format!("iyakki-backup-{}-", who(user));
    let suffix = format!(".{}", bk::EXTENSION);
    let Ok(rd) = std::fs::read_dir(dest) else { return 0 };
    let mut names: Vec<String> = rd.flatten().filter(|e| e.file_type().is_ok_and(|t| t.is_file())).filter_map(|e| e.file_name().into_string().ok()).filter(|n| n.starts_with(&prefix) && n.ends_with(&suffix)).collect();
    names.sort(); // the stamp sorts oldest first
    let extra = names.len().saturating_sub(keep as usize);
    names.iter().take(extra).filter(|n| std::fs::remove_file(dest.join(n)).is_ok()).count()
}

/// Make the scheduled backup now: validate, read the passphrase, write the file, then prune. On a result (good or not) the schedule's
/// `last_run` and `last_result` are set so the next run waits for its slot; the caller saves it. A stop (`Progress::cancel`) leaves no file.
pub fn run(l: &Layout, s: &mut Schedule, now: i64, pr: &Progress) -> Result<Created, String> {
    s.last_run = now;
    let out = (|| {
        s.validate()?;
        if !Path::new(&s.dest).is_dir() {
            return Err(format!("The folder {} is not there (is the stick plugged in?)", s.dest));
        }
        let passphrase = if s.passphrase_file.is_empty() { None } else { Some(read_passphrase(&s.passphrase_file)?) };
        let areas = if s.areas.is_empty() { bk::all_areas() } else { s.areas.iter().cloned().collect() };
        let opts = bk::CreateOptions { areas, passphrase, now, release: env!("CARGO_PKG_VERSION").to_string() };
        let c = bk::create_with_progress(l, Path::new(&s.dest), &opts, pr).map_err(|e| e.to_string())?;
        let gone = prune(Path::new(&s.dest), &l.user, s.keep);
        Ok((c, gone))
    })();
    match out {
        Ok((c, gone)) => {
            s.last_result = format!("done, {} files, {}{}{}", c.files, bk::human(c.bytes), if c.encrypted { ", encrypted" } else { ", not encrypted" }, if gone > 0 { format!("; removed {gone} older file(s)") } else { String::new() });
            Ok(c)
        }
        Err(e) => {
            s.last_result = format!("failed: {e}");
            Err(e)
        }
    }
}

/// Whether the run was stopped by the person (so the page can say "stopped" and not "failed").
pub fn was_stopped(pr: &Progress) -> bool {
    pr.cancel.load(Ordering::Relaxed)
}

#[cfg(test)]
#[path = "backup_schedule_tests.rs"]
mod tests;
