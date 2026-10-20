//! File-scope enforcement shared by the CLI and the agent's file tools: a path is resolved to its
//! real location (symlinks followed) *before* it is checked against the user's read/write scopes,
//! so a link inside a user's folder can never lead out of it.

use std::path::{Path, PathBuf};

use iyakki_sec::policy::Policy;

/// Resolve `input` (absolute, or relative to `cwd`) to a canonical path. A path that does not exist
/// yet is resolved through its parent directory, which must exist.
pub fn resolve(cwd: &Path, input: &str) -> Result<PathBuf, String> {
    let joined = if input.starts_with('/') { input.to_string() } else { format!("{}/{}", cwd.display(), input) };
    let norm = iyakki_vfs::normalize(&joined).map_err(|_| format!("invalid path: {input}"))?;
    match std::fs::canonicalize(&norm) {
        Ok(r) => Ok(r),
        Err(_) => {
            let parent = Path::new(&norm).parent().unwrap_or(Path::new("/"));
            let name = Path::new(&norm).file_name().ok_or("invalid path")?;
            Ok(std::fs::canonicalize(parent).map_err(|_| format!("no such directory: {}", parent.display()))?.join(name))
        }
    }
}

/// Like [`resolve`], but for a file about to be written: the folders above it need not exist yet. The nearest folder that
/// does exist is resolved (symlinks followed), so the result can be checked against the write scope before anything is made.
/// A file in the middle of the path is an error that names it.
pub fn resolve_for_write(cwd: &Path, input: &str) -> Result<PathBuf, String> {
    let joined = if input.starts_with('/') { input.to_string() } else { format!("{}/{}", cwd.display(), input) };
    let norm = iyakki_vfs::normalize(&joined).map_err(|_| format!("invalid path: {input}"))?;
    let mut tail: Vec<std::ffi::OsString> = Vec::new();
    let mut cur = PathBuf::from(&norm);
    loop {
        match std::fs::canonicalize(&cur) {
            Ok(real) => {
                if !tail.is_empty() && !real.is_dir() {
                    return Err(format!("cannot write {input}: {}", blocking_file_text(&real)));
                }
                return Ok(tail.iter().rev().fold(real, |p, c| p.join(c)));
            }
            Err(_) => {
                let Some(name) = cur.file_name().map(|n| n.to_os_string()) else { return Err(format!("invalid path: {input}")) };
                tail.push(name);
                if !cur.pop() {
                    return Err(format!("invalid path: {input}"));
                }
            }
        }
    }
}

/// A name with no extension that is usually a folder (what a model means by `deploy/…`, not a file).
pub fn looks_like_folder_name(p: &Path) -> bool {
    let n = p.file_name().and_then(|n| n.to_str()).unwrap_or("");
    !n.is_empty() && !n.contains('.') && !n.starts_with('.') && !matches!(n, "Dockerfile" | "Makefile" | "Containerfile" | "LICENSE" | "Procfile" | "README")
}

/// "<f> is a file, not a folder" plus what to do; an empty file (a leftover of a mistaken write) is said to be safe to delete.
fn blocking_file_text(f: &Path) -> String {
    let empty = std::fs::metadata(f).is_ok_and(|m| m.is_file() && m.len() == 0);
    if empty {
        format!("{0} is a file, not a folder, so nothing can be put inside it. It is EMPTY (probably made by mistake), so deleting it is safe: ask the person to delete {0} (or delete it with a command if you may), then retry. Or choose a different folder name.", f.display())
    } else {
        format!("{0} is a file, not a folder, so nothing can be put inside it. It has content, so do not delete it: {SUGGEST_OTHER_NAME}", f.display())
    }
}

const SUGGEST_OTHER_NAME: &str = "Next: choose a different file or folder name, or call list_dir on the parent folder to see what is there.";

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

/// The folders a person's file pickers start in and stay inside: their home folder, the folders of their workspaces and any
/// narrow folder their account is allowed to read (a scope such as `/shared`). A scope that is the whole filesystem, or a
/// folder that holds the home folder (`/home`), is too broad to count: an administrator's `/` does not make `/etc` part of
/// the home folder. Every root is a real path (symlinks followed), and so is every path checked against them.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FileRoots {
    /// Where pickers start.
    pub home: PathBuf,
    pub roots: Vec<PathBuf>,
    /// Host folders granted to the account (window mode), real paths; they are also in `roots`.
    pub mounts: Vec<crate::hostfiles::HostMount>,
}

/// The home folder of `user`: `/home/<user>` when it exists; otherwise, in window mode, the folder `$HOME` names (not `/`
/// or `/root`); otherwise a folder of the account's own under the data folder (made when missing).
pub fn home_root(user: &str, data_dir: &Path) -> PathBuf {
    home_root_with(user, data_dir, std::env::var("HOME").ok().as_deref())
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

impl FileRoots {
    pub fn new(policy: &Policy, user: &str, data_dir: &Path) -> FileRoots {
        Self::with_home(policy, user, data_dir, home_root(user, data_dir))
    }

    pub fn with_home(policy: &Policy, user: &str, data_dir: &Path, home: PathBuf) -> FileRoots {
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

    /// The granted host folder that holds this real path.
    pub fn mount_of(&self, real: &Path) -> Option<&crate::hostfiles::HostMount> {
        self.mounts.iter().filter(|m| real.starts_with(&m.path)).max_by_key(|m| m.path.len())
    }

    /// May this real path be read? Inside the roots, and the account's policy allows it or a granted host folder holds it.
    pub fn read_ok(&self, policy: &Policy, user: &str, real: &Path) -> bool {
        self.contains(real) && (self.mount_of(real).is_some() || policy.allows_read(user, &real.to_string_lossy()))
    }

    /// May this real path be written? In a granted host folder only when its writing is on; elsewhere as the policy says.
    pub fn write_ok(&self, policy: &Policy, user: &str, real: &Path) -> bool {
        match self.mount_of(real) {
            Some(m) => m.write,
            None => policy.allows_write(user, &real.to_string_lossy()),
        }
    }

    /// Is this real path inside one of the roots?
    pub fn contains(&self, real: &Path) -> bool {
        self.roots.iter().any(|r| real.starts_with(r))
    }

    /// The root that holds `real`, if any.
    pub fn root_of(&self, real: &Path) -> Option<&PathBuf> {
        self.roots.iter().filter(|r| real.starts_with(r)).max_by_key(|r| r.components().count())
    }

    /// `~` and `~/x` mean the home folder.
    pub fn expand(&self, input: &str) -> String {
        let t = input.trim();
        if let Some(rest) = t.strip_prefix("host:") {
            let (name, tail) = rest.split_once('/').unwrap_or((rest, ""));
            return match self.mounts.iter().find(|m| m.name == name) {
                Some(m) => Path::new(&m.path).join(tail).to_string_lossy().into_owned(),
                // an unknown name stays as typed and is refused as outside the roots, with the name in the message
                None => format!("/host-folder-not-shared/{name}"),
            };
        }
        if t == "~" {
            self.home.to_string_lossy().into_owned()
        } else if let Some(rest) = t.strip_prefix("~/") {
            self.home.join(rest).to_string_lossy().into_owned()
        } else {
            t.to_string()
        }
    }

    /// Resolve `input` (`~`, relative to `cwd`, `..` and links all handled) to a real path inside the roots.
    pub fn resolve(&self, cwd: &Path, input: &str) -> Result<PathBuf, String> {
        let real = resolve(cwd, &self.expand(input)).map_err(|e| if input.trim().starts_with("host:") { format!("no host folder with that name is shared ({input}); add it under Host files first") } else { e })?;
        if self.contains(&real) { Ok(real) } else { Err(format!("{} is outside your home folder ({}).", real.display(), self.home.display())) }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use iyakki_sec::policy::Role;

    #[test]
    fn explains_errors_and_creates_parents() {
        let d = std::fs::canonicalize(std::env::temp_dir()).unwrap().join(format!("iyakki-scope2-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
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
        let r = FileRoots::with_home(&admin, "amy", &data, home.clone());
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
        let r = FileRoots::with_home(&user, "amy", &data, home.clone());
        assert!(r.contains(&shared) && r.contains(&home) && !r.contains(&base.join("home/bob")), "{:?}", r.roots);
        // a workspace elsewhere is a root
        let ws = base.join("outside");
        let mut reg = crate::workspace::Registry::load(&data, "amy");
        reg.add("Out", &ws).unwrap();
        assert!(FileRoots::with_home(&admin, "amy", &data, home.clone()).contains(&ws.join("secret.csv")));
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
}
