//! Who owns what the camera writes. IYAKKI runs as root, so a photo or video it saves in `/home/<name>/Pictures` would be
//! root's; the person's account must own the folders and files made for them. The owner is the account's entry in
//! `/etc/passwd` (any uid but 0), else whoever owns the home folder (when that is not root either). Nothing changes when
//! no such owner is found, or when IYAKKI is not root.

use std::io;
use std::path::{Path, PathBuf};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Owner {
    pub uid: u32,
    pub gid: u32,
}

/// The `user:x:uid:gid:...` line of a passwd file, when the uid is not root's.
pub fn from_passwd(text: &str, user: &str) -> Option<Owner> {
    text.lines().filter(|l| !l.starts_with('#')).find_map(|l| {
        let f: Vec<&str> = l.split(':').collect();
        if f.len() >= 4 && f[0] == user {
            let (uid, gid) = (f[2].parse().ok()?, f[3].parse().ok()?);
            (uid != 0).then_some(Owner { uid, gid })
        } else {
            None
        }
    })
}

/// The account's owner ids: its passwd entry, else the home folder's owner when that is not root.
pub fn owner_of(home: &Path, user: &str, passwd: &Path) -> Option<Owner> {
    if let Ok(t) = std::fs::read_to_string(passwd) {
        if let Some(o) = from_passwd(&t, user) {
            return Some(o);
        }
    }
    use std::os::unix::fs::MetadataExt;
    let m = std::fs::metadata(home).ok()?;
    (m.uid() != 0).then_some(Owner { uid: m.uid(), gid: m.gid() })
}

pub type Chown<'a> = &'a dyn Fn(&Path, Owner) -> io::Result<()>;

/// Give `path` and every folder between it and `home` (not `home` itself) to the account, using `chown`.
pub fn give_with(path: &Path, home: &Path, passwd: &Path, chown: Chown) {
    let Some(user) = home.file_name().map(|n| n.to_string_lossy().into_owned()) else { return };
    let Some(owner) = owner_of(home, &user, passwd) else { return };
    let mut chain: Vec<PathBuf> = path.ancestors().take_while(|p| *p != home && p.starts_with(home)).map(Path::to_path_buf).collect();
    chain.reverse();
    for p in chain {
        if p.exists() {
            if let Err(e) = chown(&p, owner) {
                log::warn!("could not give {} to {}: {e}", p.display(), user);
            }
        }
    }
}

/// The real thing: `chown` through libc, only when running as root.
pub fn real_chown(p: &Path, o: Owner) -> io::Result<()> {
    use std::os::unix::ffi::OsStrExt;
    use std::os::unix::fs::MetadataExt;
    if std::fs::symlink_metadata(p)?.uid() == o.uid {
        return Ok(());
    }
    let c = std::ffi::CString::new(p.as_os_str().as_bytes()).map_err(|_| io::Error::from(io::ErrorKind::InvalidInput))?;
    // SAFETY: a valid NUL-terminated path; chown has no other preconditions.
    if unsafe { libc::chown(c.as_ptr(), o.uid, o.gid) } == 0 {
        Ok(())
    } else {
        Err(io::Error::last_os_error())
    }
}

/// Give a path (and the folders above it up to the home) to the account whose home is `home`.
pub fn give(path: &Path, home: &Path) {
    // SAFETY: geteuid has no preconditions.
    if unsafe { libc::geteuid() } != 0 {
        return;
    }
    give_with(path, home, Path::new("/etc/passwd"), &real_chown);
}

/// The account home of something saved in its Pictures or Videos folder: the parent of that folder.
pub fn home_of_saved(path: &Path) -> Option<PathBuf> {
    path.ancestors().find(|a| a.file_name().is_some_and(|n| n == "Pictures" || n == "Videos")).and_then(|a| a.parent()).map(Path::to_path_buf)
}

/// Give a file or folder the camera saved (and the Pictures/Videos folder it is in) to the person whose home it is under.
pub fn give_saved(path: &Path) {
    if let Some(home) = home_of_saved(path) {
        give(path, &home);
    }
}

/// The home of the account a folder like `/home/admin/Videos` lives in (the folder's parent).
pub fn home_of(dir: &Path) -> Option<&Path> {
    dir.parent().filter(|p| p.file_name().is_some())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;

    #[test]
    fn the_owner_is_the_passwd_entry_or_the_home_folders_owner_never_root() {
        let t = "root:x:0:0:root:/root:/bin/sh\n# c\nadmin:x:1000:1001:Admin:/home/admin:/bin/sh\nbob:x:0:0::/home/bob:/bin/sh\n";
        assert_eq!(from_passwd(t, "admin"), Some(Owner { uid: 1000, gid: 1001 }));
        assert_eq!(from_passwd(t, "bob"), None, "uid 0 is not an owner to give files to");
        assert_eq!(from_passwd(t, "nobody-here"), None);
        assert_eq!(from_passwd("admin:x:abc:1::", "admin"), None);
        let d = std::env::temp_dir().join(format!("iyakki-own-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(d.join("home/amy")).unwrap();
        std::fs::write(d.join("passwd"), t).unwrap();
        // no passwd entry: the home folder's owner (this test's own user, unless that is root)
        use std::os::unix::fs::MetadataExt;
        let me = std::fs::metadata(d.join("home/amy")).unwrap();
        let want = (me.uid() != 0).then_some(Owner { uid: me.uid(), gid: me.gid() });
        assert_eq!(owner_of(&d.join("home/amy"), "amy", &d.join("passwd")), want);
        std::fs::create_dir_all(d.join("home/admin")).unwrap();
        assert_eq!(owner_of(&d.join("home/admin"), "admin", &d.join("passwd")), Some(Owner { uid: 1000, gid: 1001 }), "the passwd entry wins");
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn made_files_and_their_new_folders_are_given_to_the_account_but_not_the_home_or_anything_else() {
        let d = std::env::temp_dir().join(format!("iyakki-own2-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        let home = d.join("home/admin");
        std::fs::create_dir_all(home.join("Videos")).unwrap();
        std::fs::write(home.join("Videos/video-1.avi"), b"x").unwrap();
        std::fs::write(d.join("passwd"), "admin:x:1000:1001::/home/admin:/bin/sh\n").unwrap();
        let calls: RefCell<Vec<(PathBuf, Owner)>> = RefCell::new(Vec::new());
        let fake = |p: &Path, o: Owner| {
            calls.borrow_mut().push((p.to_path_buf(), o));
            Ok(())
        };
        give_with(&home.join("Videos/video-1.avi"), &home, &d.join("passwd"), &fake);
        let got: Vec<PathBuf> = calls.borrow().iter().map(|c| c.0.clone()).collect();
        assert_eq!(got, vec![home.join("Videos"), home.join("Videos/video-1.avi")], "the folder, then the file; not the home itself");
        assert!(calls.borrow().iter().all(|c| c.1 == Owner { uid: 1000, gid: 1001 }));
        // a path outside the home is left alone; an unknown account (no passwd entry, root-owned home) too
        calls.borrow_mut().clear();
        give_with(&d.join("elsewhere/x"), &home, &d.join("passwd"), &fake);
        give_with(&home.join("Videos"), &home, &d.join("nonexistent-passwd-file"), &|p, o| {
            assert!(o.uid != 0, "{p:?}");
            Ok(())
        });
        assert!(calls.borrow().is_empty());
        assert_eq!(home_of(&home.join("Videos")), Some(home.as_path()));
        assert_eq!(home_of_saved(&home.join("Videos/video-1.avi")), Some(home.clone()));
        assert_eq!(home_of_saved(&home.join("Pictures")), Some(home.clone()));
        assert_eq!(home_of_saved(&home.join("Documents/x")), None);
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn the_stamp_uses_the_clocks_zone_not_utc() {
        // 2026-10-18 19:37:33 UTC
        let unix = 1_792_352_253;
        assert_eq!(stamp_at(Some("UTC".into()), unix), "20261018-193733");
        let la = stamp_at(Some("America/Los_Angeles".into()), unix);
        assert_eq!(la, "20261018-123733", "the same instant in the chosen zone");
        let ist = stamp_at(Some("Asia/Kolkata".into()), unix);
        assert_eq!(ist, "20261019-010733");
        set_zone(Some("Asia/Kolkata".into()));
        assert_ne!(stamp_at(None, unix), "", "no zone chosen falls back to the machine's own");
        set_zone(None);
    }

    use super::super::{set_zone, stamp_at};
}
