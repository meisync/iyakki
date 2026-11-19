//! The kept kernel logs of earlier sessions (the "last session that ended unexpectedly" and the boots before it), as the core serves them
//! (F-ARCH-06, the owner's core rule: the app layer asks the core, the core reads the folder). The data type and the pure helpers are those of
//! `iyakki-storage`; reading the folder, marking the note seen and copying a log into a person's folder go through [`CrashLog`]: the device has
//! `iyakki_core_local::LocalCrashLog`, a test has `iyakki_core_fake::FakeCrashLog`. Reads need `system.read`; marking and copying need `system.crashlog.write`.

use std::path::{Path, PathBuf};

pub use iyakki_storage::crashlog::{wall, Unclean, LOG_SUBDIR};

pub trait CrashLog: Send + Sync {
    /// The note of the last session that ended unexpectedly, if there is one.
    fn read_unclean(&self, dir: &Path) -> Option<Unclean>;
    /// The kept log of the last crash, by file name.
    fn latest_crash(&self, dir: &Path) -> Option<String>;
    /// The log of this boot, by file name.
    fn current_file(&self, dir: &Path) -> Option<String>;
    /// Is there a kept file of this name?
    fn has_file(&self, dir: &Path, name: &str) -> bool;
    /// The last `n` lines of a kept file.
    fn last_lines(&self, dir: &Path, name: &str, n: usize) -> Vec<String>;
    /// The person has seen the note.
    fn mark_seen(&self, dir: &Path);
    /// Copy a kept log into a new private file in `dest_dir`; returns its path.
    fn copy_to_folder(&self, dir: &Path, name: &str, dest_dir: &Path, now_unix: i64) -> Result<PathBuf, String>;
}
