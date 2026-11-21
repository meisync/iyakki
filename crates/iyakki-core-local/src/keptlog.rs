//! The kept kernel log and the boot-drive loss state as typed local calls (F-ARCH-07): the agent's chat tools (`sessionlogtools`),
//! the graphics fall-back (`webgraphics`), the file-error wording (`scope`) and the service wiring (`services`, `corehandle`) read the
//! kept sessions and ask whether the drive is healthy through this module and never name the storage crate.

use std::path::{Path, PathBuf};

pub use iyakki_storage::crashlog::{Finished, Session, Unclean, EARLIEST_SANE, LOG_SUBDIR};

/// The local time text of a unix second, as the kept log shows it.
pub fn wall(unix: i64) -> String {
    iyakki_storage::crashlog::wall(unix)
}

/// The running session (None before the log started).
pub fn current_session() -> Option<Session> {
    iyakki_storage::crashlog::current_session()
}

/// The kept `log` folder on the data volume.
#[derive(Clone, Debug)]
pub struct KeptLog {
    dir: PathBuf,
}

impl KeptLog {
    /// The log folder inside the settings folder `data_dir`.
    pub fn under(data_dir: &Path) -> KeptLog {
        KeptLog { dir: data_dir.join(LOG_SUBDIR) }
    }
    /// The log folder itself.
    pub fn at(dir: &Path) -> KeptLog {
        KeptLog { dir: dir.to_path_buf() }
    }
    pub fn dir(&self) -> &Path {
        &self.dir
    }
    /// The pending "ended unexpectedly" note, if any.
    pub fn unclean(&self) -> Option<Unclean> {
        iyakki_storage::crashlog::read_unclean(&self.dir)
    }
    /// The newest crash file name, if any.
    pub fn latest_crash(&self) -> Option<String> {
        iyakki_storage::crashlog::latest_crash(&self.dir)
    }
    /// The newest finished session (the one before the running session), if any.
    pub fn last_finished(&self) -> Option<Finished> {
        iyakki_storage::crashlog::last_finished(&self.dir)
    }
    /// The finished session before this boot: the newest one that is not the running session.
    pub fn previous_session(&self, running: Option<&Session>) -> Option<Finished> {
        self.last_finished().filter(|f| running.is_none_or(|c| c.boot_id != f.boot_id))
    }
    /// Copy a kept log file (by name) to a new file in `dest_dir`.
    pub fn copy_to(&self, name: &str, dest_dir: &Path, now_unix: i64) -> Result<PathBuf, String> {
        iyakki_storage::crashlog::copy_to_folder(&self.dir, name, dest_dir, now_unix)
    }
}

/// Is the boot drive healthy (nothing lost, nothing failing)?
pub fn drive_healthy() -> bool {
    iyakki_storage::loss::snapshot().state.healthy()
}

/// The plain sentence for a failed file operation when the drive is the reason; None otherwise.
pub fn drive_reason(path: &Path, errno: Option<i32>) -> Option<String> {
    iyakki_storage::loss::explain_for_path(path, errno)
}

/// A service that keeps files open on the boot drive: it is stopped when the drive is lost and started after it is back.
pub fn register_drive_component(name: &str, running: impl Fn() -> bool + Send + Sync + 'static, stop: impl Fn() + Send + Sync + 'static, start: impl Fn() + Send + Sync + 'static) {
    iyakki_storage::loss::register_component(name, running, stop, start);
}

/// While this value lives, the drive reads as lost on THIS thread (tests only; nothing else does).
pub struct PretendLost;

impl PretendLost {
    pub fn new() -> PretendLost {
        let lost = iyakki_storage::loss::Snapshot { state: iyakki_storage::loss::State::Lost, ..Default::default() };
        iyakki_storage::loss::set_thread_override(Some(lost));
        PretendLost
    }
}

impl Default for PretendLost {
    fn default() -> Self {
        Self::new()
    }
}

impl Drop for PretendLost {
    fn drop(&mut self) {
        iyakki_storage::loss::set_thread_override(None);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dir(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("iyakki-keptlog-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn an_empty_folder_has_no_history_no_note_and_no_crash_file() {
        let d = dir("empty");
        let k = KeptLog::under(&d);
        assert_eq!(k.dir(), d.join("log"));
        assert!(k.unclean().is_none() && k.latest_crash().is_none() && k.last_finished().is_none());
        assert!(k.previous_session(None).is_none());
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn the_previous_session_is_the_newest_finished_one_unless_it_is_the_running_one() {
        let d = dir("history");
        let k = KeptLog::under(&d);
        std::fs::create_dir_all(k.dir()).unwrap();
        std::fs::write(k.dir().join("sessions.tsv"), "boot-a\t1800000000\tclean\t100\nboot-b\t1800001000\tunclean\t90\n").unwrap();
        let last = k.last_finished().unwrap();
        assert_eq!((last.boot_id.as_str(), last.unclean()), ("boot-b", true));
        let running = Session { boot_id: "boot-b".into(), started: 1, cmdline: String::new(), clean: false };
        assert!(k.previous_session(Some(&running)).is_none());
        let other = Session { boot_id: "boot-c".into(), started: 2, cmdline: String::new(), clean: false };
        assert_eq!(k.previous_session(Some(&other)).map(|f| f.boot_id), Some("boot-b".to_string()));
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn a_lost_drive_changes_the_wording_and_the_health_only_while_pretended_and_only_here() {
        assert!(drive_healthy());
        assert!(drive_reason(Path::new("/var/lib/iyakki/x"), Some(2)).is_none());
        {
            let _lost = PretendLost::new();
            assert!(!drive_healthy());
            assert!(drive_reason(Path::new("/var/lib/iyakki/x"), Some(2)).unwrap().contains("drive is not available"));
            assert!(drive_reason(Path::new("/srv/elsewhere/x"), Some(2)).is_none());
        }
        assert!(drive_healthy());
    }

    #[test]
    fn copying_a_log_that_is_not_there_is_refused_in_words() {
        let d = dir("copy");
        let k = KeptLog::under(&d);
        assert!(k.copy_to("nothing.log", &d, 1_800_000_000).is_err());
        let _ = std::fs::remove_dir_all(&d);
    }
}
