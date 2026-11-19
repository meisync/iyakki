//! Hot reload support: notice that a watched file changed (modified time or size), by polling. No operating-system watcher is used, so
//! it behaves the same everywhere.

use std::path::PathBuf;
use std::time::SystemTime;

pub struct Watcher {
    files: Vec<PathBuf>,
    seen: Vec<Option<(SystemTime, u64)>>,
}

impl Watcher {
    pub fn new(files: Vec<PathBuf>) -> Watcher {
        let seen = files.iter().map(|f| stamp(f)).collect();
        Watcher { files, seen }
    }
    /// True once for each change, including a file appearing or disappearing.
    pub fn changed(&mut self) -> bool {
        let now: Vec<_> = self.files.iter().map(|f| stamp(f)).collect();
        let c = now != self.seen;
        self.seen = now;
        c
    }
}

fn stamp(f: &PathBuf) -> Option<(SystemTime, u64)> {
    let m = std::fs::metadata(f).ok()?;
    Some((m.modified().ok()?, m.len()))
}
