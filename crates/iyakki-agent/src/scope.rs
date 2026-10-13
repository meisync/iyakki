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

pub fn check_read(policy: &Policy, user: &str, p: &Path) -> Result<(), String> {
    if policy.allows_read(user, &p.to_string_lossy()) { Ok(()) } else { Err(format!("permission denied: {}", p.display())) }
}

pub fn check_write(policy: &Policy, user: &str, p: &Path) -> Result<(), String> {
    if policy.allows_write(user, &p.to_string_lossy()) { Ok(()) } else { Err(format!("permission denied (write): {}", p.display())) }
}

#[cfg(test)]
mod tests {
    use super::*;
    use iyakki_sec::policy::Role;

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
}
