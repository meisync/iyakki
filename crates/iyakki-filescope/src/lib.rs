//! Which folders a person's file pickers and tools stay inside, and the checks against them (split out of iyakki-agent for F-SPLIT-03).
//! Pure: the agent builds a [`FileRoots`] from the real machine (`iyakki_agent::scope::file_roots`), the UI and the tests read it.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// A folder of the host the person granted (window mode).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct HostMount {
    /// Used as `host:<name>`.
    pub name: String,
    /// The real path of the folder.
    pub path: String,
    /// Writing is allowed (read-only by default).
    #[serde(default)]
    pub write: bool,
}

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
    pub mounts: Vec<HostMount>,
}

impl FileRoots {
    /// The granted host folder that holds this real path.
    pub fn mount_of(&self, real: &Path) -> Option<&HostMount> {
        self.mounts.iter().filter(|m| real.starts_with(&m.path)).max_by_key(|m| m.path.len())
    }

    /// May this real path be read? Inside the roots, and the account's policy allows it or a granted host folder holds it.
    pub fn read_ok(&self, policy: &iyakki_sec::policy::Policy, user: &str, real: &Path) -> bool {
        self.contains(real) && (self.mount_of(real).is_some() || policy.allows_read(user, &real.to_string_lossy()))
    }

    /// May this real path be written? In a granted host folder only when its writing is on; elsewhere as the policy says.
    pub fn write_ok(&self, policy: &iyakki_sec::policy::Policy, user: &str, real: &Path) -> bool {
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

    fn roots() -> FileRoots {
        FileRoots {
            home: PathBuf::from("/home/amy"),
            roots: vec![PathBuf::from("/home/amy"), PathBuf::from("/srv/share")],
            mounts: vec![HostMount { name: "docs".into(), path: "/srv/share".into(), write: false }],
        }
    }

    #[test]
    fn the_home_marks_and_host_names_expand_and_unknown_hosts_stay_outside() {
        let r = roots();
        assert_eq!(r.expand("~"), "/home/amy");
        assert_eq!(r.expand("~/Notes"), "/home/amy/Notes");
        assert_eq!(r.expand("host:docs/a.txt"), "/srv/share/a.txt");
        assert!(r.expand("host:nope/a.txt").starts_with("/host-folder-not-shared/"));
        assert!(r.resolve(Path::new("/"), "host:nope/a").is_err());
    }

    #[test]
    fn a_path_is_inside_when_a_root_holds_it_and_a_read_only_mount_refuses_writes() {
        let r = roots();
        assert!(r.contains(Path::new("/home/amy/x")) && r.contains(Path::new("/srv/share/y")));
        assert!(!r.contains(Path::new("/etc/passwd")));
        assert_eq!(r.root_of(Path::new("/srv/share/y")), Some(&PathBuf::from("/srv/share")));
        let policy = iyakki_sec::policy::Policy::resolve(iyakki_sec::policy::Role::Admin, &Default::default());
        assert!(!r.write_ok(&policy, "amy", Path::new("/srv/share/y")), "a granted host folder is read-only until its writing is on");
        assert!(r.read_ok(&policy, "amy", Path::new("/srv/share/y")));
    }

    #[test]
    fn resolve_goes_through_the_parent_for_a_file_that_does_not_exist_yet() {
        let d = std::env::temp_dir().join(format!("iyakki-filescope-{}", std::process::id()));
        std::fs::create_dir_all(&d).unwrap();
        let real = std::fs::canonicalize(&d).unwrap();
        assert_eq!(resolve(&real, "new.txt").unwrap(), real.join("new.txt"));
        assert!(resolve(&real, "no/such/new.txt").is_err());
        let _ = std::fs::remove_dir_all(&d);
    }
}
