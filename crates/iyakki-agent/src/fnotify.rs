//! A kernel watch for the folders apps listen to (`files.changed`, F-SDK-208): inotify on Linux, so a created, changed, moved or removed file is
//! seen when it happens and not at the next once-a-second look. The core's timer thread waits on it ([`DirWatch::wait`]) instead of sleeping,
//! and makes the file source look at once when it says yes; the events themselves still come from the same difference of the person's own
//! file listing (so the file policy applies, names only), the watch only says WHEN to look. Where there is no inotify (another system, a
//! kernel that refuses) [`DirWatch::new`] fails and the once-a-second look stays.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::Duration;

/// The most folders watched at once (the per-user inotify limit is shared with every other program).
pub const MAX_WATCHED: usize = 64;

pub struct DirWatch {
    #[cfg(target_os = "linux")]
    fd: std::os::fd::OwnedFd,
    /// Watch descriptor to folder.
    watched: Mutex<BTreeMap<i32, PathBuf>>,
}

#[cfg(target_os = "linux")]
impl DirWatch {
    pub fn new() -> std::io::Result<DirWatch> {
        use std::os::fd::FromRawFd;
        // SAFETY: inotify_init1 takes only flags and returns a new descriptor or -1.
        let fd = unsafe { libc::inotify_init1(libc::IN_NONBLOCK | libc::IN_CLOEXEC) };
        if fd < 0 {
            return Err(std::io::Error::last_os_error());
        }
        // SAFETY: the descriptor was just made and nothing else owns it.
        Ok(DirWatch { fd: unsafe { std::os::fd::OwnedFd::from_raw_fd(fd) }, watched: Mutex::new(BTreeMap::new()) })
    }

    /// Watch exactly these folders: new ones are added, ones no longer listed are let go. A folder that is not there (yet) is skipped and tried
    /// again at the next call. Returns how many are watched.
    pub fn set_dirs(&self, dirs: &BTreeSet<PathBuf>) -> usize {
        use std::os::fd::AsRawFd;
        use std::os::unix::ffi::OsStrExt;
        let mut w = self.watched.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        let gone: Vec<i32> = w.iter().filter(|(_, p)| !dirs.contains(*p)).map(|(wd, _)| *wd).collect();
        for wd in gone {
            // SAFETY: removes a watch this object added on its own descriptor.
            unsafe { libc::inotify_rm_watch(self.fd.as_raw_fd(), wd) };
            w.remove(&wd);
        }
        for d in dirs.iter().take(MAX_WATCHED) {
            if w.values().any(|p| p == d) {
                continue;
            }
            let Ok(c) = std::ffi::CString::new(d.as_os_str().as_bytes()) else { continue };
            let mask = libc::IN_CREATE | libc::IN_CLOSE_WRITE | libc::IN_DELETE | libc::IN_MOVED_FROM | libc::IN_MOVED_TO | libc::IN_DELETE_SELF | libc::IN_ONLYDIR;
            // SAFETY: a NUL-terminated path and a mask; the descriptor is this object's.
            let wd = unsafe { libc::inotify_add_watch(self.fd.as_raw_fd(), c.as_ptr(), mask) };
            if wd >= 0 {
                w.insert(wd, d.clone());
            }
        }
        w.len()
    }

    /// Wait until something happened in a watched folder, or `max` is over. Returns whether something did; everything waiting is read, so the next
    /// call waits for news.
    pub fn wait(&self, max: Duration) -> bool {
        use std::os::fd::AsRawFd;
        let mut p = libc::pollfd { fd: self.fd.as_raw_fd(), events: libc::POLLIN, revents: 0 };
        // SAFETY: one valid pollfd; the timeout is in milliseconds.
        let n = unsafe { libc::poll(&mut p, 1, max.as_millis().min(i32::MAX as u128) as i32) };
        if n <= 0 {
            return false;
        }
        let mut buf = [0u8; 8192];
        let mut any = false;
        loop {
            // SAFETY: reads into a buffer of the stated length from this object's non-blocking descriptor.
            let r = unsafe { libc::read(self.fd.as_raw_fd(), buf.as_mut_ptr() as *mut libc::c_void, buf.len()) };
            if r <= 0 {
                break;
            }
            any = true;
        }
        any
    }
}

#[cfg(not(target_os = "linux"))]
impl DirWatch {
    pub fn new() -> std::io::Result<DirWatch> {
        Err(std::io::Error::new(std::io::ErrorKind::Unsupported, "a kernel file watch needs Linux"))
    }
    pub fn set_dirs(&self, _dirs: &BTreeSet<PathBuf>) -> usize {
        0
    }
    pub fn wait(&self, max: Duration) -> bool {
        std::thread::sleep(max);
        false
    }
}

/// The folder a person's watch names, inside their home (the same place the file listing reads).
pub fn folder_path(home: &Path, folder: &str) -> PathBuf {
    if folder.is_empty() { home.to_path_buf() } else { home.join(folder) }
}

#[cfg(all(test, target_os = "linux"))]
mod tests {
    use super::*;

    fn dir(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("iyakki-fnotify-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn a_file_made_changed_moved_and_removed_wakes_the_watch_and_a_folder_let_go_does_not() {
        let (a, b) = (dir("a"), dir("b"));
        let w = DirWatch::new().unwrap();
        assert_eq!(w.set_dirs(&[a.clone(), b.clone()].into_iter().collect()), 2);
        assert_eq!(w.set_dirs(&[a.clone(), b.clone()].into_iter().collect()), 2, "the same folders are not watched twice");
        // each kind of change is seen as it happens (the wait ends at the event; only a failure runs into the limit)
        std::fs::write(a.join("one.txt"), "x").unwrap();
        assert!(w.wait(Duration::from_secs(20)), "a file was made");
        std::fs::write(a.join("one.txt"), "changed").unwrap();
        assert!(w.wait(Duration::from_secs(20)), "a file was changed");
        std::fs::rename(a.join("one.txt"), a.join("two.txt")).unwrap();
        assert!(w.wait(Duration::from_secs(20)), "a file was moved");
        std::fs::remove_file(a.join("two.txt")).unwrap();
        assert!(w.wait(Duration::from_secs(20)), "a file was removed");
        // let b go: what happens there is not news any more, and a still is
        assert_eq!(w.set_dirs(&[a.clone()].into_iter().collect()), 1);
        std::fs::write(b.join("quiet.txt"), "x").unwrap();
        std::fs::write(a.join("loud.txt"), "x").unwrap();
        assert!(w.wait(Duration::from_secs(20)));
        // a folder that is not there yet is skipped, then watched once it exists
        let later = a.join("later");
        assert_eq!(w.set_dirs(&[a.clone(), later.clone()].into_iter().collect()), 1);
        std::fs::create_dir_all(&later).unwrap();
        assert_eq!(w.set_dirs(&[a.clone(), later.clone()].into_iter().collect()), 2);
        for d in [a, b] {
            let _ = std::fs::remove_dir_all(d);
        }
    }

    #[test]
    fn a_folder_path_is_inside_the_home_and_the_home_itself_when_the_name_is_empty() {
        assert_eq!(folder_path(Path::new("/h"), ""), PathBuf::from("/h"));
        assert_eq!(folder_path(Path::new("/h"), "Docs/x"), PathBuf::from("/h/Docs/x"));
    }
}
