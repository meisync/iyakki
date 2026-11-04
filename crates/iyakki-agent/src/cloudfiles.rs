//! File and clone operations on a tenant's volume, for the cloud profile's routes (docs/development/CLOUD-MULTITENANT-PLAN.md, 3b).
//!
//! This module only does the disk work on a folder it is handed. WHICH folder, and whether the caller may touch it, is decided by the
//! route: the person comes from the signed-in session, the folder from [`crate::cloudstorage::resolve`] (locate + mount + ensure + jail) and
//! the right from [`action`] checked with `cloudaccess::Access::check`. Nothing here builds a path from a request by hand.
//!
//! Limits (all refusals, never truncation): a file read or written is at most [`MAX_FILE`] bytes of UTF-8 text; a listing shows at most
//! [`MAX_ENTRIES`] entries; a clone takes at most [`CLONE_SECS`] seconds. Not done: a per-person size cap (planned), binary files (the
//! routes carry text), a check that a host name does not resolve to a private address (literal addresses and cluster names are refused).

use std::path::Path;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use crate::cloudroles::Action;
use crate::cloudstorage::{plain_name, Scope};

pub const MAX_FILE: usize = 512 * 1024;
pub const MAX_ENTRIES: usize = 1000;
pub const CLONE_SECS: u64 = 300;

/// The scope a request names: `person` (default when a namespace is given), `shared`, or `personal` (default without a namespace).
pub fn parse_scope(scope: Option<&str>, namespace: Option<&str>) -> Result<Scope, String> {
    let ns = || namespace.filter(|n| plain_name(n)).map(str::to_string).ok_or_else(|| "give the namespace (a plain name)".to_string());
    match (scope, namespace) {
        (None, None) | (Some("personal"), None) => Ok(Scope::Personal),
        (Some("personal"), Some(_)) => Err("the personal scope has no namespace".into()),
        (None, Some(_)) | (Some("person"), _) => Ok(Scope::Person { namespace: ns()? }),
        (Some("shared"), _) => Ok(Scope::Shared { namespace: ns()? }),
        (Some(other), _) => Err(format!("the scope is person, shared or personal, not \"{other}\"")),
    }
}

/// The right a scope needs. Namespace folders follow the namespace's roles (viewers read, editors write); a person's own folder outside
/// every namespace needs only a place in the tenant.
pub fn action(scope: &Scope, write: bool) -> Action {
    match scope {
        Scope::Person { namespace } | Scope::Shared { namespace } => {
            if write {
                Action::WriteNamespace(namespace.clone())
            } else {
                Action::ReadNamespace(namespace.clone())
            }
        }
        Scope::Personal => Action::EnterTenant,
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Entry {
    pub name: String,
    /// `file`, `dir` or `link` (a link is shown, never followed).
    pub kind: &'static str,
    pub size: u64,
}

pub fn list(dir: &Path) -> Result<Vec<Entry>, String> {
    let m = std::fs::symlink_metadata(dir).map_err(|_| "no such folder".to_string())?;
    if !m.is_dir() {
        return Err("that is a file, not a folder".into());
    }
    let mut v = Vec::new();
    for e in std::fs::read_dir(dir).map_err(|e| e.to_string())?.flatten() {
        let Ok(m) = std::fs::symlink_metadata(e.path()) else { continue };
        let kind = if m.file_type().is_symlink() { "link" } else if m.is_dir() { "dir" } else { "file" };
        v.push(Entry { name: e.file_name().to_string_lossy().into_owned(), kind, size: if kind == "file" { m.len() } else { 0 } });
        if v.len() > MAX_ENTRIES {
            return Err(format!("more than {MAX_ENTRIES} entries; ask for a sub-folder"));
        }
    }
    v.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(v)
}

pub fn read_text(file: &Path) -> Result<String, String> {
    let m = std::fs::symlink_metadata(file).map_err(|_| "no such file".to_string())?;
    if !m.is_file() {
        return Err("that is not a regular file".into());
    }
    if m.len() as usize > MAX_FILE {
        return Err(format!("the file is larger than {} KiB", MAX_FILE / 1024));
    }
    String::from_utf8(std::fs::read(file).map_err(|e| e.to_string())?).map_err(|_| "the file is not UTF-8 text".to_string())
}

/// Writes `content` to `file` through a temporary file in the same folder and a rename, so a reader never sees half a file. Missing
/// parent folders are made. A folder or a link at `file` is refused.
pub fn write_text(file: &Path, content: &str) -> Result<(), String> {
    if content.len() > MAX_FILE {
        return Err(format!("the content is larger than {} KiB", MAX_FILE / 1024));
    }
    match std::fs::symlink_metadata(file) {
        Ok(m) if !m.is_file() => return Err("that is not a regular file".into()),
        _ => {}
    }
    let parent = file.parent().ok_or("no folder to write in")?;
    std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    let tmp = parent.join(format!(".iyakki-tmp-{}", std::process::id()));
    std::fs::write(&tmp, content).map_err(|e| e.to_string())?;
    std::fs::rename(&tmp, file).map_err(|e| {
        let _ = std::fs::remove_file(&tmp);
        e.to_string()
    })
}

/// Removes a file, or a folder with `recursive`. Links are removed, never followed. `base` (the person's own folder) is never removed.
pub fn delete(base: &Path, path: &Path, recursive: bool) -> Result<(), String> {
    if path == base {
        return Err("a folder of yours as a whole cannot be deleted; delete what is in it".into());
    }
    let m = std::fs::symlink_metadata(path).map_err(|_| "no such file".to_string())?;
    if m.is_dir() {
        if recursive {
            std::fs::remove_dir_all(path)
        } else {
            std::fs::remove_dir(path)
        }
    } else {
        std::fs::remove_file(path)
    }
    .map_err(|e| format!("{e} (a folder needs recursive:true unless it is empty)"))
}

// ------------------------------------------------------------------------------------------------------------------ git

/// Only https, no credentials in the address, no host that is an address of this machine or this cluster.
pub fn check_clone_url(url: &str) -> Result<(), String> {
    if url.len() > 500 || url.chars().any(|c| c.is_control() || c.is_whitespace()) {
        return Err("that is not an address".into());
    }
    let rest = url.strip_prefix("https://").ok_or("only https:// addresses can be cloned here")?;
    let authority = rest.split(['/', '?', '#']).next().unwrap_or("");
    if authority.contains('@') {
        return Err("leave the user name and password out of the address".into());
    }
    let host = authority.rsplit_once(':').filter(|(_, p)| p.chars().all(|c| c.is_ascii_digit())).map(|(h, _)| h).unwrap_or(authority).trim_matches(['[', ']']).to_ascii_lowercase();
    if host.is_empty() || !host.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '.' | ':')) {
        return Err("that host name is not valid".into());
    }
    let internal = host == "localhost" || host.ends_with(".localhost") || host.ends_with(".local") || host.ends_with(".internal") || host.ends_with(".svc") || host.ends_with(".cluster.local") || !host.contains('.') && !host.contains(':');
    let literal = host.parse::<std::net::IpAddr>().is_ok() || host.chars().all(|c| c.is_ascii_digit() || c == '.');
    if internal || literal {
        return Err("addresses of this machine, this cluster and bare IP addresses are not cloned from here".into());
    }
    Ok(())
}

/// The folder name a clone gets when the request names none: the address's last part without `.git`.
pub fn default_folder(url: &str) -> Option<String> {
    let last = url.trim_end_matches('/').rsplit('/').next()?;
    let n = last.strip_suffix(".git").unwrap_or(last);
    plain_name(n).then(|| n.to_string())
}

/// `git clone` of `url` into `dest` (which must not exist), with the protocols in `allow` (`https` in the routes), a short-lived minimal
/// environment, no hooks, no credentials helper, and symlinks written as plain files. On failure the half clone is removed.
pub fn git_clone(url: &str, dest: &Path, allow: &str, branch: Option<&str>, depth: u32, timeout: Duration) -> Result<(), String> {
    if dest.exists() {
        return Err("a folder of that name exists already".into());
    }
    if let Some(b) = branch {
        if b.is_empty() || b.starts_with('-') || !b.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.' | '/')) {
            return Err("that is not a branch name".into());
        }
    }
    let mut cmd = Command::new("git");
    cmd.env_clear()
        .env("PATH", "/usr/local/bin:/usr/bin:/bin")
        .env("HOME", "/nonexistent")
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_ALLOW_PROTOCOL", allow)
        .env("LC_ALL", "C")
        .args(["-c", "core.symlinks=false", "-c", "core.hooksPath=/dev/null", "-c", "credential.helper=", "clone", "--no-tags", "--quiet", "--no-recurse-submodules"]);
    if depth > 0 {
        cmd.args(["--depth", &depth.min(100_000).to_string()]);
    }
    if let Some(b) = branch {
        cmd.args(["--branch", b]);
    }
    cmd.arg("--").arg(url).arg(dest).stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::piped());
    let mut child = cmd.spawn().map_err(|e| format!("git cannot be started: {e}"))?;
    let end = Instant::now() + timeout;
    let status = loop {
        match child.try_wait() {
            Ok(Some(s)) => break s,
            Ok(None) if Instant::now() < end => std::thread::sleep(Duration::from_millis(50)),
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                let _ = std::fs::remove_dir_all(dest);
                return Err("the clone took too long and was stopped".into());
            }
        }
    };
    if !status.success() {
        let mut err = String::new();
        if let Some(mut e) = child.stderr.take() {
            use std::io::Read;
            let _ = e.read_to_string(&mut err);
        }
        let _ = std::fs::remove_dir_all(dest);
        let tail: String = err.lines().rev().find(|l| !l.trim().is_empty()).unwrap_or("git failed").chars().take(200).collect();
        return Err(format!("git clone failed: {tail}"));
    }
    Ok(())
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Checkout {
    pub folder: String,
    pub branch: String,
}

/// The checkouts directly inside `base` (a folder with a `.git`), with the branch `HEAD` names.
pub fn list_clones(base: &Path) -> Vec<Checkout> {
    let mut v: Vec<Checkout> = std::fs::read_dir(base)
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|e| {
            let m = std::fs::symlink_metadata(e.path()).ok()?;
            if !m.is_dir() {
                return None;
            }
            let head = std::fs::read_to_string(e.path().join(".git").join("HEAD")).ok()?;
            let branch = head.trim().strip_prefix("ref: refs/heads/").unwrap_or("(detached)").to_string();
            Some(Checkout { folder: e.file_name().to_string_lossy().into_owned(), branch })
        })
        .collect();
    v.sort_by(|a, b| a.folder.cmp(&b.folder));
    v
}


#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn dir(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("iyakki-cloudfiles-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn scopes_parse_and_need_the_right_rights() {
        assert_eq!(parse_scope(None, None).unwrap(), Scope::Personal);
        assert_eq!(parse_scope(None, Some("web")).unwrap(), Scope::Person { namespace: "web".into() });
        assert_eq!(parse_scope(Some("shared"), Some("web")).unwrap(), Scope::Shared { namespace: "web".into() });
        for (s, n) in [(Some("shared"), None), (Some("personal"), Some("web")), (Some("root"), None), (None, Some("../x")), (Some("person"), None)] {
            assert!(parse_scope(s, n).is_err(), "{s:?} {n:?}");
        }
        assert_eq!(action(&Scope::Shared { namespace: "web".into() }, true), Action::WriteNamespace("web".into()));
        assert_eq!(action(&Scope::Person { namespace: "web".into() }, false), Action::ReadNamespace("web".into()));
        assert_eq!(action(&Scope::Personal, true), Action::EnterTenant);
    }

    #[test]
    fn write_read_list_and_delete_work_and_keep_to_their_limits() {
        let base = dir("rw");
        write_text(&base.join("a/b/c.txt"), "hello").unwrap();
        assert_eq!(read_text(&base.join("a/b/c.txt")).unwrap(), "hello");
        write_text(&base.join("a/b/c.txt"), "again").unwrap();
        assert_eq!(read_text(&base.join("a/b/c.txt")).unwrap(), "again");
        let l = list(&base.join("a/b")).unwrap();
        assert_eq!(l, vec![Entry { name: "c.txt".into(), kind: "file", size: 5 }], "no temporary file is left");
        assert!(list(&base.join("a/b/c.txt")).is_err() && read_text(&base.join("a")).is_err());
        assert!(write_text(&base.join("a"), "x").is_err(), "a folder is not overwritten");
        assert!(write_text(&base.join("big"), &"x".repeat(MAX_FILE + 1)).is_err());
        std::fs::write(base.join("bin"), [0xff, 0xfe]).unwrap();
        assert!(read_text(&base.join("bin")).unwrap_err().contains("UTF-8"));
        assert!(delete(&base, &base.join("a"), false).is_err(), "a folder with things in it needs recursive");
        delete(&base, &base.join("a"), true).unwrap();
        assert!(delete(&base, &base, true).unwrap_err().contains("as a whole"));
        assert!(delete(&base, &base.join("gone"), false).is_err());
    }

    #[cfg(unix)]
    #[test]
    fn a_link_is_listed_and_removed_but_never_followed() {
        let base = dir("link");
        let outside = dir("link-outside");
        std::fs::write(outside.join("secret"), "s").unwrap();
        std::os::unix::fs::symlink(&outside, base.join("out")).unwrap();
        assert_eq!(list(&base).unwrap()[0].kind, "link");
        assert!(read_text(&base.join("out")).is_err());
        assert!(write_text(&base.join("out"), "x").is_err(), "writing onto a link is refused");
        delete(&base, &base.join("out"), true).unwrap();
        assert!(outside.join("secret").exists(), "the target survived");
    }

    #[test]
    fn clone_addresses_are_https_public_and_without_credentials() {
        assert!(check_clone_url("https://example.org/a/b.git").is_ok());
        assert!(check_clone_url("https://example.org:8443/a/b").is_ok());
        for bad in [
            "http://example.org/a", "ssh://git@example.org/a", "git@example.org:a/b.git", "file:///etc", "/etc/passwd", "--upload-pack=x", "https://user:pw@example.org/a",
            "https://localhost/a", "https://127.0.0.1/a", "https://[::1]/a", "https://169.254.169.254/latest", "https://10.0.0.1/a", "https://db/a", "https://x.svc/a", "https://x.cluster.local/a",
            "https://exa mple.org/a", "https://example.org/a\nb", "",
        ] {
            assert!(check_clone_url(bad).is_err(), "{bad:?}");
        }
        assert_eq!(default_folder("https://example.org/a/b.git").as_deref(), Some("b"));
        assert_eq!(default_folder("https://example.org/a/b/").as_deref(), Some("b"));
        assert_eq!(default_folder("https://example.org/a/.."), None);
    }

    fn have_git() -> bool {
        Command::new("git").arg("--version").stdout(Stdio::null()).stderr(Stdio::null()).status().map(|s| s.success()).unwrap_or(false)
    }

    #[cfg(unix)]
    #[test]
    fn a_clone_lands_in_the_folder_with_links_written_as_plain_files_and_failures_leave_nothing() {
        if !have_git() {
            eprintln!("git is not installed: skipped");
            return;
        }
        let src = dir("git-src");
        let g = |args: &[&str], at: &Path| {
            let ok = Command::new("git").current_dir(at).env_clear().env("PATH", "/usr/bin:/bin").env("GIT_CONFIG_GLOBAL", "/dev/null").env("GIT_AUTHOR_NAME", "t").env("GIT_AUTHOR_EMAIL", "t@example.org").env("GIT_COMMITTER_NAME", "t").env("GIT_COMMITTER_EMAIL", "t@example.org").args(args).status().unwrap().success();
            assert!(ok, "{args:?}");
        };
        g(&["init", "-q", "-b", "main"], &src);
        std::fs::write(src.join("f.txt"), "x").unwrap();
        std::os::unix::fs::symlink("/", src.join("rootlink")).unwrap();
        g(&["add", "."], &src);
        g(&["commit", "-q", "-m", "one"], &src);
        let base = dir("git-dest");
        let dest = base.join("repo");
        // the routes allow https only: a local path is refused by git itself
        assert!(git_clone(&src.to_string_lossy(), &dest, "https", None, 0, Duration::from_secs(60)).is_err());
        assert!(!dest.exists(), "a failed clone leaves nothing");
        git_clone(&src.to_string_lossy(), &dest, "file", None, 0, Duration::from_secs(60)).unwrap();
        assert_eq!(read_text(&dest.join("f.txt")).unwrap(), "x");
        let m = std::fs::symlink_metadata(dest.join("rootlink")).unwrap();
        assert!(m.is_file(), "core.symlinks=false: the link became a plain file");
        assert!(crate::cloudstorage::links_leaving(&base).is_none());
        assert_eq!(list_clones(&base), vec![Checkout { folder: "repo".into(), branch: "main".into() }]);
        assert!(git_clone(&src.to_string_lossy(), &dest, "file", None, 0, Duration::from_secs(60)).unwrap_err().contains("exists"));
        assert!(git_clone("x", &base.join("b"), "file", Some("--evil"), 0, Duration::from_secs(5)).unwrap_err().contains("branch"));
    }
}
