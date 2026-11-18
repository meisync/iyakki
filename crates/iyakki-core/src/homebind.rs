//! `/home` on the data volume. The root filesystem is the initramfs in RAM, so everything the apps write under
//! `/home/<name>` (Pictures, Videos, Stories, Workspaces) and the logins of tools in `~/.claude` would be lost at power-off.
//! When the data volume is a disk, `<data>/home` is bind-mounted over `/home` right after the volume is mounted, before
//! anything writes a user file. `/root` is not touched (it holds the image's SSH keys).

use std::io;
use std::path::Path;

pub const HOME: &str = "/home";

/// True when `mounts_text` (the /proc/mounts format) lists a mount at exactly `target`.
pub fn is_mounted(mounts_text: &str, target: &str) -> bool {
    mounts_text.lines().any(|l| l.split_whitespace().nth(1).map(|m| m.replace("\\040", " ")).as_deref() == Some(target))
}

/// Copy what is under `src` into `dst` without overwriting anything that already exists there. Returns how many files
/// were copied and the names of conflicts (left where they are).
pub fn merge_dir(src: &Path, dst: &Path) -> io::Result<(usize, Vec<String>)> {
    use std::os::unix::fs::{MetadataExt, PermissionsExt};
    let (mut copied, mut skipped) = (0, Vec::new());
    std::fs::create_dir_all(dst)?;
    for e in std::fs::read_dir(src)? {
        let e = e?;
        let (from, to) = (e.path(), dst.join(e.file_name()));
        let md = std::fs::symlink_metadata(&from)?;
        let ft = md.file_type();
        if ft.is_dir() {
            let fresh = !to.exists();
            let (n, s) = merge_dir(&from, &to)?;
            copied += n;
            skipped.extend(s);
            if fresh {
                let _ = std::fs::set_permissions(&to, std::fs::Permissions::from_mode(md.mode() & 0o7777));
                let _ = std::os::unix::fs::lchown(&to, Some(md.uid()), Some(md.gid()));
            }
        } else if to.symlink_metadata().is_ok() {
            skipped.push(from.display().to_string());
        } else if ft.is_symlink() {
            std::os::unix::fs::symlink(std::fs::read_link(&from)?, &to)?;
            copied += 1;
        } else if ft.is_file() {
            std::fs::copy(&from, &to)?;
            let _ = std::os::unix::fs::lchown(&to, Some(md.uid()), Some(md.gid()));
            copied += 1;
        }
    }
    Ok((copied, skipped))
}

/// Everything decided here is injected (no real mounts in tests): the mount table text, whether the data directory is on a
/// disk, whether this is PID 1, and the bind-mount call `mount(source, target)`.
pub fn bind_home(data: &Path, home: &Path, mounts_text: &str, on_disk: bool, pid1: bool, mount: &dyn Fn(&Path, &Path) -> Result<(), String>) -> Vec<String> {
    if !pid1 {
        return vec!["home: left alone (this is not the system's first process)".into()];
    }
    if !on_disk {
        return vec![format!("home: no data volume, {} stays in RAM and is lost at power-off", home.display())];
    }
    if is_mounted(mounts_text, &home.display().to_string()) {
        return vec![format!("home: {} is already a mount point", home.display())];
    }
    let target = data.join("home");
    if let Err(e) = std::fs::create_dir_all(&target) {
        return vec![format!("home: could not make {}: {e}; {} stays in RAM", target.display(), home.display())];
    }
    let mut log = Vec::new();
    // user folders made earlier in this boot, before the data volume appeared, are merged into the data volume
    if home.is_dir() {
        match merge_dir(home, &target) {
            Ok((0, s)) if s.is_empty() => {}
            Ok((n, s)) => log.push(format!("home: moved {n} file(s) made earlier in this boot into {}{}", target.display(), if s.is_empty() { String::new() } else { format!(" ({} name(s) already there, kept as they were: {})", s.len(), s.join(", ")) })),
            Err(e) => log.push(format!("home: could not merge the earlier files into {}: {e}", target.display())),
        }
    }
    match mount(&target, home) {
        Ok(()) => log.push(format!("home: {} is now on the data volume ({})", home.display(), target.display())),
        Err(e) => log.push(format!("home: could not bind {} onto {}: {e}; it stays in RAM", target.display(), home.display())),
    }
    log
}

/// The real thing, for boot: the data directory's location and the mount table are read from the system.
#[cfg(target_os = "linux")]
pub fn bind_home_real(data: &Path, mount: &dyn Fn(&Path, &Path) -> Result<(), String>) -> Vec<String> {
    let on_disk = matches!(iyakki_storage::data_location(&iyakki_storage::Roots::real(), data), iyakki_storage::DataLocation::Disk(_));
    let mounts = iyakki_storage::read_mounts().unwrap_or_default();
    bind_home(data, Path::new(HOME), &mounts, on_disk, std::process::id() == 1, mount)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;

    fn tmp(n: &str) -> std::path::PathBuf {
        let d = std::env::temp_dir().join(format!("iyakki-homebind-{n}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn mount_table_lookup() {
        let t = "rootfs / rootfs rw 0 0\n/dev/sda2 /var/lib/iyakki ext4 rw 0 0\n/dev/sda2 /home ext4 rw 0 0\n";
        assert!(is_mounted(t, "/home") && is_mounted(t, "/var/lib/iyakki"));
        assert!(!is_mounted(t, "/root") && !is_mounted("rootfs / rootfs rw 0 0\n", "/home"));
    }

    #[test]
    fn nothing_is_mounted_without_a_disk_volume_outside_pid_1_or_when_already_mounted() {
        let (d, h) = (tmp("noop"), tmp("noop-home"));
        let called = RefCell::new(0);
        let m = |_: &Path, _: &Path| {
            *called.borrow_mut() += 1;
            Ok(())
        };
        assert!(bind_home(&d, &h, "", false, true, &m)[0].contains("stays in RAM"));
        assert!(bind_home(&d, &h, "", true, false, &m)[0].contains("not the system's first process"));
        let mounted = format!("/dev/sda2 {} ext4 rw 0 0\n", h.display());
        assert!(bind_home(&d, &h, &mounted, true, true, &m)[0].contains("already a mount point"));
        assert_eq!(*called.borrow(), 0);
        assert!(!d.join("home").exists(), "no data folder is made when nothing is bound");
        let _ = std::fs::remove_dir_all(&d);
        let _ = std::fs::remove_dir_all(&h);
    }

    #[test]
    fn binds_the_data_home_and_merges_earlier_files_without_overwriting() {
        let (d, h) = (tmp("merge"), tmp("merge-home"));
        std::fs::create_dir_all(d.join("home/amy/Pictures")).unwrap();
        std::fs::write(d.join("home/amy/Pictures/old.jpg"), "persisted").unwrap();
        std::fs::write(d.join("home/amy/note.txt"), "on the volume").unwrap();
        std::fs::create_dir_all(h.join("amy/Pictures")).unwrap();
        std::fs::create_dir_all(h.join("bob/Videos")).unwrap();
        std::fs::write(h.join("amy/Pictures/new.jpg"), "made earlier").unwrap();
        std::fs::write(h.join("amy/note.txt"), "RAM copy").unwrap();
        std::fs::write(h.join("bob/Videos/a.mp4"), "v").unwrap();
        std::os::unix::fs::symlink("Videos/a.mp4", h.join("bob/link")).unwrap();
        let seen = RefCell::new(None);
        let m = |s: &Path, t: &Path| {
            *seen.borrow_mut() = Some((s.to_path_buf(), t.to_path_buf()));
            Ok(())
        };
        let log = bind_home(&d, &h, "rootfs / rootfs rw 0 0\n", true, true, &m);
        assert_eq!(seen.borrow().clone().unwrap(), (d.join("home"), h.clone()));
        assert!(log.iter().any(|l| l.contains("moved 3 file(s)") && l.contains("kept as they were")), "{log:?}");
        assert!(log.last().unwrap().contains(&format!("home: {} is now on the data volume ({})", h.display(), d.join("home").display())));
        assert_eq!(std::fs::read_to_string(d.join("home/amy/note.txt")).unwrap(), "on the volume", "never overwritten");
        assert_eq!(std::fs::read_to_string(d.join("home/amy/Pictures/old.jpg")).unwrap(), "persisted");
        assert_eq!(std::fs::read_to_string(d.join("home/amy/Pictures/new.jpg")).unwrap(), "made earlier");
        assert_eq!(std::fs::read_to_string(d.join("home/bob/Videos/a.mp4")).unwrap(), "v");
        assert_eq!(std::fs::read_link(d.join("home/bob/link")).unwrap(), Path::new("Videos/a.mp4"));
        let _ = std::fs::remove_dir_all(&d);
        let _ = std::fs::remove_dir_all(&h);
    }

    #[test]
    fn a_failed_mount_is_logged_and_loses_nothing() {
        let (d, h) = (tmp("fail"), tmp("fail-home"));
        std::fs::write(h.join("f"), "x").unwrap();
        let log = bind_home(&d, &h, "", true, true, &|_, _| Err("EPERM".into()));
        assert!(log.last().unwrap().contains("could not bind") && log.last().unwrap().contains("EPERM"));
        assert!(h.join("f").exists() && d.join("home/f").exists());
        let _ = std::fs::remove_dir_all(&d);
        let _ = std::fs::remove_dir_all(&h);
    }
}
