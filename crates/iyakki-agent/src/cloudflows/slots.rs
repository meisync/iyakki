//! The tenant's run cap across processes (F-CLOUD-71). Every process that runs flows for a tenant (the replicas of the tenant pod share the
//! tenant data folder) writes how many runs it has going to a small file of its own in `flows-slots/`, again every few seconds; a process
//! asked to admit a run adds up the fresh files of the OTHER processes to its own count. A file that has not been written for
//! [`STALE_SECS`] belongs to a process that is gone and counts for nothing (and is removed after a longer while), so a crashed process
//! cannot hold the cap down for ever. When the folder cannot be written or read the cap falls back to this process's own count.

use std::path::{Path, PathBuf};

/// A file older than this is a process that stopped.
pub const STALE_SECS: u64 = 20;
/// Files older than this are deleted.
const REMOVE_SECS: u64 = 600;
const PREFIX: &str = "slot-";

pub struct SlotBoard {
    dir: PathBuf,
    id: String,
}

fn name_ok(n: &str) -> bool {
    n.len() <= 80 && n.starts_with(PREFIX) && n[PREFIX.len()..].chars().all(|c| c.is_ascii_alphanumeric() || c == '-')
}

impl SlotBoard {
    /// A board in `dir`; `id` tells this process from the others (a random or process-unique text).
    pub fn new(dir: &Path, id: &str) -> SlotBoard {
        let id: String = id.chars().filter(|c| c.is_ascii_alphanumeric() || *c == '-').take(60).collect();
        SlotBoard { dir: dir.into(), id: if id.is_empty() { "x".into() } else { id } }
    }

    /// An id that differs between processes and between starts of one process.
    pub fn unique_id() -> String {
        let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_nanos());
        format!("{}-{nanos:x}", std::process::id())
    }

    fn file(&self) -> PathBuf {
        self.dir.join(format!("{PREFIX}{}", self.id))
    }

    /// Say how many runs this process has going now. Returns false when it could not be written.
    pub fn publish(&self, count: usize, now: u64) -> bool {
        use std::io::Write;
        if std::fs::create_dir_all(&self.dir).is_err() {
            return false;
        }
        let tmp = self.dir.join(format!(".tmp-{}", self.id));
        let ok = std::fs::File::create(&tmp).and_then(|mut f| f.write_all(format!("{count} {now}\n").as_bytes())).is_ok();
        ok && std::fs::rename(&tmp, self.file()).is_ok()
    }

    /// Runs the other processes have going, counting the fresh files only.
    pub fn others(&self, now: u64) -> usize {
        let Ok(rd) = std::fs::read_dir(&self.dir) else { return 0 };
        let mut total = 0usize;
        for e in rd.flatten() {
            let name = e.file_name().to_string_lossy().into_owned();
            if !name_ok(&name) || name == format!("{PREFIX}{}", self.id) {
                continue;
            }
            // a file of the wrong size is not one of ours
            if e.metadata().map_or(true, |m| !m.is_file() || m.len() > 64) {
                continue;
            }
            let Ok(text) = std::fs::read_to_string(e.path()) else { continue };
            let mut it = text.split_whitespace();
            let (Some(Ok(count)), Some(Ok(at))) = (it.next().map(str::parse::<usize>), it.next().map(str::parse::<u64>)) else { continue };
            if count > 100_000 {
                continue;
            }
            if now.saturating_sub(at) > REMOVE_SECS {
                let _ = std::fs::remove_file(e.path());
            } else if now.saturating_sub(at) <= STALE_SECS {
                total = total.saturating_add(count);
            }
        }
        total
    }

    /// This process is stopping: its file goes.
    pub fn leave(&self) {
        let _ = std::fs::remove_file(self.file());
    }
}

/// May one more run start? `mine` runs of this process and `started` this pass count with the runs the other processes have going.
pub fn within_cap(cap: usize, mine: usize, started: usize, others: usize) -> bool {
    mine.saturating_add(started).saturating_add(others) < cap
}

impl Drop for SlotBoard {
    fn drop(&mut self) {
        self.leave();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dir(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("iyakki-slots-{}-{tag}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        d
    }

    #[test]
    fn a_process_counts_the_fresh_runs_of_the_others_and_never_its_own_file() {
        let d = dir("sum");
        let (a, b, c) = (SlotBoard::new(&d, "a"), SlotBoard::new(&d, "b"), SlotBoard::new(&d, "c"));
        assert_eq!(a.others(1000), 0, "no folder yet");
        assert!(a.publish(3, 1000) && b.publish(2, 1000) && c.publish(4, 1000));
        assert_eq!(a.others(1005), 6);
        assert_eq!(b.others(1005), 7);
        // b has more going later
        assert!(b.publish(5, 1010));
        assert_eq!(a.others(1012), 9);
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn a_process_that_stopped_writing_stops_counting_and_its_file_is_cleaned_up_later() {
        let d = dir("stale");
        let (a, b) = (SlotBoard::new(&d, "a"), SlotBoard::new(&d, "b"));
        b.publish(7, 1000);
        assert_eq!(a.others(1000 + STALE_SECS), 7, "still fresh at the limit");
        assert_eq!(a.others(1000 + STALE_SECS + 1), 0, "a crashed process cannot hold the cap down");
        assert!(d.join("slot-b").exists());
        assert_eq!(a.others(1000 + REMOVE_SECS + 1), 0);
        assert!(!d.join("slot-b").exists(), "removed after a longer while");
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn the_tenant_cap_holds_across_two_processes() {
        let d = dir("cap");
        let (a, b) = (SlotBoard::new(&d, "a"), SlotBoard::new(&d, "b"));
        a.publish(2, 100);
        b.publish(1, 100);
        // a tenant cap of 3: neither process may start another while together they have 3 going
        assert!(!within_cap(3, 2, 0, a.others(101)));
        assert!(!within_cap(3, 1, 0, b.others(101)));
        // one run ends in a: both may start one
        a.publish(1, 105);
        assert!(within_cap(3, 1, 0, b.others(106)) && within_cap(3, 1, 0, a.others(106)));
        // the runs started in this pass count too, and a crashed process stops counting
        assert!(!within_cap(3, 1, 1, b.others(106)));
        assert!(within_cap(3, 1, 1, b.others(105 + STALE_SECS + 1)) == (1 + 1 < 3));
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn leaving_removes_the_file_and_files_that_are_not_ours_are_ignored() {
        let d = dir("leave");
        let (a, b) = (SlotBoard::new(&d, "a"), SlotBoard::new(&d, "b"));
        b.publish(2, 500);
        assert_eq!(a.others(500), 2);
        b.leave();
        assert_eq!(a.others(500), 0);
        std::fs::write(d.join("slot-junk"), "not numbers\n").unwrap();
        std::fs::write(d.join("slot-big"), "9 500\n".repeat(100)).unwrap();
        std::fs::write(d.join("other"), "9 500\n").unwrap();
        std::fs::write(d.join("slot-neg"), "-3 500\n").unwrap();
        std::fs::write(d.join("slot-huge"), "99999999999999 500\n").unwrap();
        std::fs::create_dir_all(d.join("slot-dir")).unwrap();
        assert_eq!(a.others(500), 0, "damaged, oversized, foreign and directory entries count for nothing");
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn an_unwritable_folder_is_reported_and_ids_are_made_safe() {
        let d = dir("nowrite");
        std::fs::create_dir_all(&d).unwrap();
        std::fs::write(d.join("file"), "x").unwrap();
        let b = SlotBoard::new(&d.join("file"), "a");
        assert!(!b.publish(1, 1), "a folder that is a file");
        assert_eq!(b.others(1), 0);
        let weird = SlotBoard::new(&d, "../../etc/x y");
        assert!(weird.publish(1, 1));
        assert!(d.join("slot-etcxy").exists(), "only letters, digits and dashes survive");
        assert!(SlotBoard::unique_id().contains('-'));
        let _ = std::fs::remove_dir_all(&d);
    }
}
