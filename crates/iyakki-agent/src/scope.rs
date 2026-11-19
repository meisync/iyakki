//! File-scope enforcement shared by the CLI and the agent's file tools: a path is resolved to its
//! real location (symlinks followed) *before* it is checked against the user's read/write scopes,
//! so a link inside a user's folder can never lead out of it.

pub use iyakki_shared::scope::{SUGGEST_OTHER_NAME, blocking_file_text, resolve_for_write};
use std::path::{Path, PathBuf};

use iyakki_sec::policy::Policy;

pub use iyakki_filescope::{resolve, FileRoots, HostMount};


/// A name with no extension that is usually a folder (what a model means by `deploy/…`, not a file).
pub fn looks_like_folder_name(p: &Path) -> bool {
    let n = p.file_name().and_then(|n| n.to_str()).unwrap_or("");
    !n.is_empty() && !n.contains('.') && !n.starts_with('.') && !matches!(n, "Dockerfile" | "Makefile" | "Containerfile" | "LICENSE" | "Procfile" | "README")
}



/// Make the folders above `path` (inside whatever scope the caller already checked).
pub fn ensure_parent(path: &Path) -> Result<(), String> {
    match path.parent() {
        Some(p) if !p.as_os_str().is_empty() => std::fs::create_dir_all(p).map_err(|e| explain_io(&e, p, "create folder")),
        _ => Ok(()),
    }
}

/// Plain, actionable text for a failed file operation: which part of the path is the problem and what to do next, instead of
/// the bare "Not a directory (os error 20)". `action` is a verb phrase such as "write", "read" or "list".
pub fn explain_io(e: &std::io::Error, path: &Path, action: &str) -> String {
    let shown = path.display();
    // the boot drive is gone or failing: say that, not a bare "Input/output error" (iyakki_storage::loss)
    if let Some(t) = iyakki_storage::loss::explain_for_path(path, e.raw_os_error()) {
        return format!("cannot {action} {shown}: {t}");
    }
    // the first existing part of the path that is a file although something is supposed to be inside it
    let file_in_middle = || path.ancestors().skip(1).find(|a| a.is_file()).map(|a| a.display().to_string());
    // the first part that does not exist
    let missing = || {
        let mut have = PathBuf::new();
        for c in path.components() {
            let next = have.join(c);
            if !next.exists() {
                return next.display().to_string();
            }
            have = next;
        }
        shown.to_string()
    };
    match e.raw_os_error() {
        Some(20) => match file_in_middle() {
            Some(f) => format!("cannot {action} {shown}: {}", blocking_file_text(Path::new(&f))),
            None => format!("cannot {action} {shown}: a part of the path is a file, not a folder. {SUGGEST_OTHER_NAME}"),
        },
        Some(2) => {
            let m = missing();
            let parent = path.parent().map_or(String::new(), |p| p.display().to_string());
            if action == "write" || action == "create folder" {
                format!("cannot {action} {shown}: {m} does not exist (writing creates missing folders inside your folder, so this one may be outside it or a dangling link). Next: call list_dir on {parent} and pick an existing place.")
            } else {
                format!("cannot {action} {shown}: {m} does not exist. Next: call list_dir on {parent} to see the real names, then try again.")
            }
        }
        Some(13) | Some(1) => format!("cannot {action} {shown}: the operating system denies access. Next: use another file in a folder you own, or tell the person what permission is missing."),
        Some(21) => format!("cannot {action} {shown}: it is a folder, not a file. Next: name a file inside it (for example {shown}/name.txt), or call list_dir to see what it holds."),
        Some(28) => format!("cannot {action} {shown}: the disk is full. Next: tell the person; free space must be made before anything can be written."),
        Some(30) => format!("cannot {action} {shown}: the disk is read-only. Next: write somewhere else, or tell the person."),
        _ => format!("cannot {action} {shown}: {e}"),
    }
}

pub fn check_read(policy: &Policy, user: &str, p: &Path) -> Result<(), String> {
    if policy.allows_read(user, &p.to_string_lossy()) { Ok(()) } else { Err(format!("permission denied: {}", p.display())) }
}

pub fn check_write(policy: &Policy, user: &str, p: &Path) -> Result<(), String> {
    if policy.allows_write(user, &p.to_string_lossy()) { Ok(()) } else { Err(format!("permission denied (write): {}", p.display())) }
}

// ------------------------------------------------------------------ the folders a person's file pickers and table tools stay in


/// The home folder of `user`: `/home/<user>` when it exists; otherwise, in window mode, the folder `$HOME` names (not `/`
/// or `/root`); otherwise a folder of the account's own under the data folder (made when missing).
pub fn home_root(user: &str, data_dir: &Path) -> PathBuf {
    home_root_with(user, data_dir, std::env::var("HOME").ok().as_deref())
}

/// [`home_root`] for code that CREATES folders in the home (the Flows folder): the unit tests of this crate, and any run with
/// `IYAKKI_NO_HOST_HOME=1` (`./run.sh test` sets it), never fall back to the real `$HOME` and use the account's own folder under the
/// data folder. Flow tests once created folders in the real `~/Flows` through the `$HOME` fallback.
pub fn home_root_for_writes(user: &str, data_dir: &Path) -> PathBuf {
    if cfg!(test) || std::env::var_os("IYAKKI_NO_HOST_HOME").is_some_and(|v| v == "1") {
        return home_root_with(user, data_dir, None);
    }
    home_root(user, data_dir)
}

/// The `HOME` a program started for `user` must get, or `None` when this is not the IYAKKI system (a desktop window keeps the
/// real `$HOME`) or the folder is not there. Used by the IDE's terminal, test and coverage runs (F-STORE-09).
pub fn system_home(user: &str) -> Option<PathBuf> {
    system_home_with(user, Path::new("/etc/iyakki").is_dir(), &user_home(user))
}

/// [`system_home`] with the system switch and the folder given (tests).
pub fn system_home_with(user: &str, on_system: bool, home: &Path) -> Option<PathBuf> {
    (on_system && iyakki_sec::policy::valid_username(user) && home.is_dir()).then(|| home.to_path_buf())
}

/// The person's home folder for programs IYAKKI starts for them (the terminal, tool runs, jobs): `/home/<user>`, made when
/// missing, so `HOME` is on the data volume (see `homebind` in iyakki-core) and tools keep their logins there. Only on the
/// IYAKKI system (the image has `/etc/iyakki`); elsewhere nothing is made and the folder is simply returned.
pub fn user_home(user: &str) -> PathBuf {
    let on_system = Path::new("/etc/iyakki").is_dir();
    ensure_home(Path::new("/home"), user, on_system, Path::new("/etc/passwd"))
}

/// [`user_home`] with the parent folder, the system switch and the passwd file given (tests). The folder is private to its
/// account (mode 0700) and owned by the account's passwd entry when that is not root's.
pub fn ensure_home(parent: &Path, user: &str, make: bool, passwd: &Path) -> PathBuf {
    let home = parent.join(user);
    if make && iyakki_sec::policy::valid_username(user) && home.symlink_metadata().is_ok() {
        // an existing home made before the 0700 rule (KI-095) is tightened the first time anything asks for it (KI-260: unless that would
        // lock its account out)
        let _ = tighten_home_for(&home, user, &std::fs::read_to_string(passwd).unwrap_or_default());
    }
    if make && iyakki_sec::policy::valid_username(user) && !home.exists() {
        use std::os::unix::fs::PermissionsExt;
        if std::fs::create_dir_all(&home).is_ok() {
            let _ = std::fs::set_permissions(&home, std::fs::Permissions::from_mode(0o700));
            let owner = std::fs::read_to_string(passwd).ok().and_then(|t| {
                t.lines().filter(|l| !l.starts_with('#')).find_map(|l| {
                    let f: Vec<&str> = l.split(':').collect();
                    (f.len() >= 4 && f[0] == user).then(|| (f[2].parse::<u32>().ok(), f[3].parse::<u32>().ok())).and_then(|(u, g)| u.filter(|u| *u != 0).map(|u| (u, g)))
                })
            });
            if let Some((u, g)) = owner {
                let _ = std::os::unix::fs::chown(&home, Some(u), g);
            }
        }
    }
    home
}

/// Make one home folder private (0700), idempotently. The folder is opened with `O_NOFOLLOW | O_DIRECTORY` and changed through
/// the open descriptor (`fchmod`), so a link is never followed and nothing outside the folder itself is touched. Only the
/// folder's own mode is changed: with 0700 nobody else can traverse it, so the (possibly 0644) files inside are not reachable
/// either, and no walk below the folder is needed. Returns `Ok(true)` when the mode was changed.
pub fn tighten_home(home: &Path) -> std::io::Result<bool> {
    use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
    let dir = std::fs::OpenOptions::new().read(true).custom_flags(libc::O_NOFOLLOW | libc::O_DIRECTORY | libc::O_CLOEXEC).open(home)?;
    let mode = dir.metadata()?.permissions().mode() & 0o7777;
    if mode == 0o700 {
        return Ok(false);
    }
    dir.set_permissions(std::fs::Permissions::from_mode(0o700))?;
    Ok(true)
}

/// What to do with an existing home folder at the repair (KI-095, KI-260).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HomePlan {
    /// Make it 0700.
    Tighten,
    /// Hand it to this user and group, then make it 0700 (a root-owned home of an account that has a system user).
    ChownThenTighten(u32, u32),
    /// Leave it as it is: it is owned by root, the account is not root and no system user is known for it, so 0700 would shut the
    /// account's own commands out of it (a dev shell runs as `nobody` then).
    LeaveRootOwned,
}

/// The user and group of `user` in the text of a passwd file, when it is not root's.
pub fn passwd_ids(passwd: &str, user: &str) -> Option<(u32, u32)> {
    passwd.lines().filter(|l| !l.starts_with('#')).find_map(|l| {
        let f: Vec<&str> = l.split(':').collect();
        let (u, g) = (f.get(2)?.parse::<u32>().ok()?, f.get(3)?.parse::<u32>().ok()?);
        (f.len() >= 4 && f[0] == user && u != 0).then_some((u, g))
    })
}

/// The decision, as a pure function of the folder's owner (`owner_uid`), the account name and the passwd text.
pub fn home_plan(account: &str, owner_uid: u32, passwd: &str) -> HomePlan {
    if owner_uid != 0 || account == "root" {
        return HomePlan::Tighten;
    }
    match passwd_ids(passwd, account) {
        Some((u, g)) => HomePlan::ChownThenTighten(u, g),
        None => HomePlan::LeaveRootOwned,
    }
}

/// [`tighten_home`] for the account `user`, following [`home_plan`]. Returns `Ok(true)` when the folder was changed.
pub fn tighten_home_for(home: &Path, user: &str, passwd: &str) -> std::io::Result<bool> {
    use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
    let dir = std::fs::OpenOptions::new().read(true).custom_flags(libc::O_NOFOLLOW | libc::O_DIRECTORY | libc::O_CLOEXEC).open(home)?;
    match home_plan(user, dir.metadata()?.uid(), passwd) {
        HomePlan::LeaveRootOwned => Ok(false),
        HomePlan::Tighten => tighten_home(home),
        HomePlan::ChownThenTighten(u, g) => {
            std::os::unix::fs::fchown(&dir, Some(u), Some(g))?;
            tighten_home(home).map(|_| true)
        }
    }
}

/// The repair at every start (KI-095): every `/home/<name>` that is a real folder on the same file system as `parent` (a link,
/// a file, or another mount such as an encrypted home, which is private by itself, is left alone) is made 0700. Returns one
/// audit line per folder that was changed or could not be changed; nothing when all is already private.
pub fn tighten_homes(parent: &Path) -> Vec<String> {
    tighten_homes_with(parent, &std::fs::read_to_string("/etc/passwd").unwrap_or_default())
}

/// [`tighten_homes`] with the passwd text given (tests).
pub fn tighten_homes_with(parent: &Path, passwd: &str) -> Vec<String> {
    use std::os::unix::fs::MetadataExt;
    let mut out = Vec::new();
    let (Ok(pm), Ok(rd)) = (std::fs::symlink_metadata(parent), std::fs::read_dir(parent)) else { return out };
    for e in rd.flatten() {
        let p = e.path();
        let Ok(md) = std::fs::symlink_metadata(&p) else { continue };
        if !md.file_type().is_dir() || md.dev() != pm.dev() {
            continue;
        }
        let name = e.file_name().to_string_lossy().into_owned();
        match tighten_home_for(&p, &name, passwd) {
            Ok(true) => out.push(format!("home: {} was {:04o}, now 0700 (private to the account)", p.display(), md.mode() & 0o7777)),
            Ok(false) => {}
            Err(er) => out.push(format!("home: could not make {} private: {er}", p.display())),
        }
    }
    out
}

pub fn home_root_with(user: &str, data_dir: &Path, home_env: Option<&str>) -> PathBuf {
    let real = |p: &Path| std::fs::canonicalize(p).ok().filter(|r| r.is_dir());
    if iyakki_sec::policy::valid_username(user) {
        if let Some(h) = real(&PathBuf::from(format!("/home/{user}"))) {
            return h;
        }
    }
    if let Some(h) = home_env.filter(|h| !h.is_empty() && *h != "/" && *h != "/root").and_then(|h| real(Path::new(h))) {
        return h;
    }
    let own = data_dir.join("home").join(if iyakki_sec::policy::valid_username(user) { user } else { "invalid" });
    let _ = std::fs::create_dir_all(&own);
    real(&own).unwrap_or(own)
}

/// The folder the browser saves downloads in when the person's home cannot hold them: `<data>/downloads/<user>` (the user name cut
/// to the characters the browser keeps in file names).
pub fn downloads_fallback(user: &str, data_dir: &Path) -> PathBuf {
    let who: String = user.chars().filter(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.')).collect();
    data_dir.join("downloads").join(who)
}

pub fn file_roots(policy: &Policy, user: &str, data_dir: &Path) -> FileRoots {
    file_roots_with_home(policy, user, data_dir, home_root(user, data_dir))
}

pub fn file_roots_with_home(policy: &Policy, user: &str, data_dir: &Path, home: PathBuf) -> FileRoots {
    let mut roots = vec![home.clone()];
    let mut add = |p: PathBuf| {
        if let Ok(r) = std::fs::canonicalize(&p) {
            if r.is_dir() && !roots.contains(&r) {
                roots.push(r);
            }
        }
    };
    for w in crate::workspace::Registry::load(data_dir, user).items {
        add(PathBuf::from(&w.root));
    }
    // the person's own Downloads folder under the data folder (where the browser saves when `/home/<user>/Downloads` cannot be
    // made), so EDIT, DATA and MEDIA can open what the browser saved there (F-BROX-01); the account's read rule still applies
    add(downloads_fallback(user, data_dir));
    for sc in policy.read_scopes(user) {
        let p = PathBuf::from(&sc);
        let broad = p.parent().is_none() || home.starts_with(std::fs::canonicalize(&p).unwrap_or(p.clone()));
        if !broad {
            add(p);
        }
    }
    // host folders the person granted (window mode), checked again right now: a folder that went away is not here
    let mounts = crate::hostfiles::active_mounts(data_dir, user);
    for m in &mounts {
        let p = PathBuf::from(&m.path);
        if !roots.contains(&p) {
            roots.push(p);
        }
    }
    FileRoots { home, roots, mounts }
}

#[cfg(test)]
mod tests {
    use super::*;
    use iyakki_sec::policy::Role;

    #[test]
    fn explains_errors_and_creates_parents() {
        let d = crate::testdir::unique("iyakki-scope2", "main");
        std::fs::write(d.join("deploy"), "").unwrap();
        std::fs::write(d.join("full"), "data").unwrap();
        let e = resolve_for_write(&d, "deploy/registry.yaml").unwrap_err();
        assert!(e.contains(&format!("{}/deploy is a file, not a folder", d.display())) && e.contains("EMPTY") && e.contains("safe"), "{e}");
        let e = resolve_for_write(&d, "full/a/b.txt").unwrap_err();
        assert!(e.contains("has content") && e.contains("do not delete"), "{e}");
        let ok = resolve_for_write(&d, "a/b/c.txt").unwrap();
        assert_eq!(ok, d.join("a/b/c.txt"));
        ensure_parent(&ok).unwrap();
        assert!(d.join("a/b").is_dir());
        let enotdir = std::io::Error::from_raw_os_error(20);
        assert!(explain_io(&enotdir, &d.join("deploy/x"), "list").contains("deploy is a file, not a folder"));
        assert!(explain_io(&std::io::Error::from_raw_os_error(2), &d.join("zz/x"), "read").contains("zz does not exist"));
        assert!(explain_io(&std::io::Error::from_raw_os_error(13), &d, "write").contains("denies access"));
        assert!(explain_io(&std::io::Error::from_raw_os_error(21), &d, "write").contains("is a folder"));
        assert!(explain_io(&std::io::Error::from_raw_os_error(28), &d, "write").contains("disk is full"));
        // while the boot drive is lost the wording changes (and only then)
        let lost = iyakki_storage::loss::Snapshot { state: iyakki_storage::loss::State::Lost, ..Default::default() };
        iyakki_storage::loss::set_thread_override(Some(lost));
        let eio = std::io::Error::from_raw_os_error(5);
        assert!(explain_io(&eio, Path::new("/home/admin/Pictures/a.jpg"), "write").contains("drive is not available"));
        assert!(explain_io(&std::io::Error::from_raw_os_error(2), Path::new("/var/lib/iyakki/x"), "read").contains("drive is not available"));
        // a path outside /home and /var/lib/iyakki (the test folder itself may be below /home, as target/tmp is)
        assert!(!explain_io(&std::io::Error::from_raw_os_error(2), Path::new("/srv/elsewhere/zz/x"), "read").contains("drive is not available"), "a path elsewhere keeps its own wording");
        iyakki_storage::loss::set_thread_override(None);
        assert!(!explain_io(&eio, Path::new("/home/admin/Pictures/a.jpg"), "write").contains("drive is not available"));
        assert!(looks_like_folder_name(Path::new("/x/deploy")) && !looks_like_folder_name(Path::new("/x/a.txt")) && !looks_like_folder_name(Path::new("/x/Dockerfile")));
    }

    #[test]
    fn resolves_relative_nonexistent_and_rejects_escapes() {
        let d = std::env::temp_dir().join(format!("iyakki-scope-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(d.join("home")).unwrap();
        std::fs::write(d.join("home/a.txt"), "x").unwrap();
        let real = std::fs::canonicalize(&d).unwrap();
        assert_eq!(resolve(&d.join("home"), "a.txt").unwrap(), real.join("home/a.txt"));
        assert_eq!(resolve(&d, "home/new.txt").unwrap(), real.join("home/new.txt"), "not-yet-existing file resolves via its parent");
        assert!(resolve(&d, "no/such/dir/f").is_err());
        assert!(resolve(&d, "/../etc").is_err(), "cannot climb above the root");
        std::os::unix::fs::symlink("/etc", d.join("home/out")).unwrap();
        let p = resolve(&d.join("home"), "out/hostname").unwrap();
        let mut pol = Role::User.template();
        pol.fs_read = vec![real.join("home").display().to_string()];
        assert!(check_read(&pol, "u", &p).is_err(), "a symlink out of the scope is caught after resolution");
        assert!(check_read(&pol, "u", &resolve(&d.join("home"), "a.txt").unwrap()).is_ok());
        pol.fs_write = vec![];
        assert!(check_write(&pol, "u", &real.join("home/a.txt")).unwrap_err().contains("write"));
    }

    #[test]
    fn roots_confine_to_home_workspaces_and_narrow_scopes() {
        let base = std::fs::canonicalize(std::env::temp_dir()).unwrap().join(format!("iyakki-roots-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        let (home, shared, data) = (base.join("home/amy"), base.join("shared"), base.join("data"));
        for d in [&home, &shared, &data, &base.join("outside")] {
            std::fs::create_dir_all(d).unwrap();
        }
        std::fs::write(home.join("a.csv"), "x").unwrap();
        std::fs::write(base.join("outside/secret.csv"), "x").unwrap();
        std::os::unix::fs::symlink(base.join("outside"), home.join("link")).unwrap();
        std::os::unix::fs::symlink("/etc", home.join("etc")).unwrap();
        let mut admin = Role::Admin.template();
        admin.fs_read = vec!["/".into()];
        let r = file_roots_with_home(&admin, "amy", &data, home.clone());
        assert_eq!(r.roots, vec![home.clone()], "an administrator's / is not a root");
        assert_eq!(r.resolve(&home, "a.csv").unwrap(), home.join("a.csv"));
        assert_eq!(r.resolve(&home, "~/a.csv").unwrap(), r.resolve(&home, "a.csv").unwrap(), "~ is the home folder");
        for bad in ["../../outside/secret.csv", "/etc/passwd", "link/secret.csv", "etc/passwd", "/", "~/../../outside"] {
            let e = r.resolve(&home, bad).unwrap_err();
            assert!(e.contains("outside your home folder"), "{bad}: {e}");
        }
        // a narrow scope counts, the folder above home does not
        let mut user = Role::User.template();
        user.fs_read = vec![home.display().to_string(), shared.display().to_string(), base.join("home").display().to_string()];
        let r = file_roots_with_home(&user, "amy", &data, home.clone());
        assert!(r.contains(&shared) && r.contains(&home) && !r.contains(&base.join("home/bob")), "{:?}", r.roots);
        // a workspace elsewhere is a root
        let ws = base.join("outside");
        let mut reg = crate::workspace::Registry::load(&data, "amy");
        reg.add("Out", &ws).unwrap();
        assert!(file_roots_with_home(&admin, "amy", &data, home.clone()).contains(&ws.join("secret.csv")));
        // the home folder: /home/<user>, else $HOME in window mode, else the data folder
        assert_eq!(home_root_with("nosuchuser9", &data, Some(home.to_str().unwrap())), home);
        let own = home_root_with("nosuchuser9", &data, Some("/"));
        assert_eq!(own, std::fs::canonicalize(data.join("home/nosuchuser9")).unwrap());
        assert_eq!(home_root_with("nosuchuser9", &data, Some("/root")), own, "/root is not a person's home");
    }
}

#[cfg(test)]
mod home_tests {
    use super::*;

    #[test]
    fn a_missing_home_is_made_private_and_only_on_the_system() {
        let d = std::env::temp_dir().join(format!("iyakki-userhome-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        let pw = d.join("passwd");
        std::fs::write(&pw, "root:x:0:0::/root:/bin/sh\n").unwrap();
        assert_eq!(ensure_home(&d, "amy", false, &pw), d.join("amy"));
        assert!(!d.join("amy").exists(), "nothing is made off the IYAKKI system");
        assert_eq!(ensure_home(&d, "amy", true, &pw), d.join("amy"));
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(std::fs::metadata(d.join("amy")).unwrap().permissions().mode() & 0o777, 0o700);
        ensure_home(&d, "../evil", true, &pw);
        assert!(!d.join("../evil").exists() && !d.join("..").join("evil").exists());
        std::fs::create_dir_all(d.join("bob/keep")).unwrap();
        ensure_home(&d, "bob", true, &pw);
        assert!(d.join("bob/keep").exists(), "an existing home is left as it is");
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn the_system_home_is_given_only_on_the_system_for_a_valid_name_and_an_existing_folder() {
        let d = std::env::temp_dir().join(format!("iyakki-syshome-{}", std::process::id()));
        std::fs::create_dir_all(d.join("amy")).unwrap();
        assert_eq!(system_home_with("amy", true, &d.join("amy")), Some(d.join("amy")));
        assert_eq!(system_home_with("amy", false, &d.join("amy")), None, "a desktop window keeps its own HOME");
        assert_eq!(system_home_with("amy", true, &d.join("nobody")), None, "no folder, no HOME override");
        assert_eq!(system_home_with("../evil", true, &d.join("amy")), None);
        let _ = std::fs::remove_dir_all(&d);
    }

    fn mode(p: &Path) -> u32 {
        use std::os::unix::fs::PermissionsExt;
        std::fs::symlink_metadata(p).unwrap().permissions().mode() & 0o7777
    }

    #[test]
    fn existing_homes_are_tightened_without_following_links_or_leaving_the_folder() {
        use std::os::unix::fs::PermissionsExt;
        let d = std::fs::canonicalize(std::env::temp_dir()).unwrap().join(format!("iyakki-tighten-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        let (h, outside) = (d.join("home"), d.join("outside"));
        for p in [h.join("admin/Downloads"), h.join("scratch"), outside.clone()] {
            std::fs::create_dir_all(&p).unwrap();
            std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        std::fs::set_permissions(h.join("admin"), std::fs::Permissions::from_mode(0o755)).unwrap();
        std::fs::write(h.join("admin/diag.txt"), "x").unwrap();
        std::fs::set_permissions(h.join("admin/diag.txt"), std::fs::Permissions::from_mode(0o644)).unwrap();
        std::fs::write(outside.join("f"), "x").unwrap();
        std::os::unix::fs::symlink(&outside, h.join("link")).unwrap();
        std::fs::write(h.join("plain"), "x").unwrap();
        let plain = mode(&h.join("plain"));
        let log = tighten_homes(&h);
        assert_eq!(log.len(), 2, "{log:?}");
        assert!(log.iter().all(|l| l.contains("now 0700")));
        assert_eq!((mode(&h.join("admin")), mode(&h.join("scratch"))), (0o700, 0o700));
        assert_eq!(mode(&h.join("admin/diag.txt")), 0o644, "files inside are shielded by the folder, not rewritten");
        assert_eq!(mode(&outside), 0o755, "a link is not followed");
        assert_eq!(mode(&h.join("plain")), plain, "a plain file next to the homes is untouched");
        assert!(tighten_home(&h.join("link")).is_err(), "a link is refused");
        assert!(tighten_homes(&h).is_empty(), "a second run changes nothing");
        // asking for an existing loose home tightens it too, and a new home is 0700
        std::fs::set_permissions(h.join("scratch"), std::fs::Permissions::from_mode(0o755)).unwrap();
        let pw = d.join("passwd");
        std::fs::write(&pw, "root:x:0:0::/root:/bin/sh\n").unwrap();
        ensure_home(&h, "scratch", true, &pw);
        assert_eq!(mode(&h.join("scratch")), 0o700);
        ensure_home(&h, "fresh", true, &pw);
        assert_eq!(mode(&h.join("fresh")), 0o700);
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn a_root_owned_home_is_handed_to_its_system_user_or_left_alone_but_never_locked_against_its_account() {
        use super::{home_plan, passwd_ids, HomePlan};
        let passwd = "root:x:0:0:root:/root:/bin/sh\nalice:x:1001:1002::/home/alice:/bin/sh\nbob:x:0:0::/home/bob:/bin/sh\n";
        assert_eq!(passwd_ids(passwd, "alice"), Some((1001, 1002)));
        assert_eq!((passwd_ids(passwd, "root"), passwd_ids(passwd, "bob"), passwd_ids(passwd, "admin")), (None, None, None));
        // owned by a real user, or root's own: private as before
        assert_eq!(home_plan("alice", 1001, passwd), HomePlan::Tighten);
        assert_eq!(home_plan("root", 0, passwd), HomePlan::Tighten);
        // root-owned, the account has a system user: given to it, then private
        assert_eq!(home_plan("alice", 0, passwd), HomePlan::ChownThenTighten(1001, 1002));
        // root-owned, no system user (an IYAKKI account): left as it is, so the account's commands can still enter it
        assert_eq!(home_plan("admin", 0, passwd), HomePlan::LeaveRootOwned);
        assert_eq!(home_plan("bob", 0, passwd), HomePlan::LeaveRootOwned);
    }

    #[test]
    fn homes_owned_by_a_real_user_are_tightened_with_the_passwd_text_given() {
        use std::os::unix::fs::PermissionsExt;
        let d = std::env::temp_dir().join(format!("iyakki-homeplan-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(d.join("amy")).unwrap();
        std::fs::set_permissions(d.join("amy"), std::fs::Permissions::from_mode(0o755)).unwrap();
        // this test process owns the folder (not root, so the plan is "tighten"; a root run would leave it by the rule)
        if super::home_plan("amy", std::os::unix::fs::MetadataExt::uid(&std::fs::metadata(d.join("amy")).unwrap()), "") == super::HomePlan::Tighten {
            let lines = super::tighten_homes_with(&d, "");
            assert_eq!(lines.len(), 1, "{lines:?}");
            assert_eq!(std::fs::metadata(d.join("amy")).unwrap().permissions().mode() & 0o777, 0o700);
        }
        let _ = std::fs::remove_dir_all(&d);
    }
}
