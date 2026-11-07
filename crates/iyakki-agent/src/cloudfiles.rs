//! File and clone operations on a tenant's volume, for the cloud profile's routes (docs/development/CLOUD-MULTITENANT-PLAN.md, 3b).
//!
//! This module only does the disk work on a folder it is handed. WHICH folder, and whether the caller may touch it, is decided by the
//! route: the person comes from the signed-in session, the folder from [`crate::cloudstorage::resolve`] (locate + mount + ensure + jail) and
//! the right from [`action`] checked with `cloudaccess::Access::check`. Nothing here builds a path from a request by hand.
//!
//! **Binary files** (`write_stream`, `open_for_download`, `copy_exact`): any bytes, at most [`MAX_BINARY`] on upload (the declared
//! `Content-Length` is checked before a byte is read, and exactly that many bytes must arrive), written in 64 KiB pieces to a private
//! temporary file beside the target and renamed into place with its SHA-256 (checked against the caller's, if given), so memory use does not
//! depend on the file's size and a reader never sees half a file. A download opens the file without following a link, takes its length from
//! the open file and copies exactly that many bytes. The path jail is the same as for text (the route resolves the folder through
//! `cloudstorage::resolve`; nothing here builds a path).
//!
//! Limits (all refusals, never truncation): a file read or written as text is at most [`MAX_FILE`] bytes of UTF-8 text; a listing shows at most
//! [`MAX_ENTRIES`] entries; a clone takes at most [`CLONE_SECS`] seconds. Not done: a per-person size cap (planned), binary files (the
//! routes carry text), a check that a host name does not resolve to a private address (literal addresses and cluster names are refused).

use std::path::Path;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use crate::cloudroles::Action;
use crate::cloudstorage::{plain_name, Scope};

pub const MAX_FILE: usize = 512 * 1024;
/// The largest binary upload (bytes).
pub const MAX_BINARY: u64 = 128 * 1024 * 1024;
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

// ------------------------------------------------------------------------------------------------------------------ move and mkdir

/// The longest name of one path part.
pub const MAX_NAME: usize = 255;

/// The parts of a path given by a person, each a plain name: no empty part, `.` or `..`, no control character, at most [`MAX_NAME`] bytes.
pub fn parts(rel: &str) -> Result<Vec<&str>, String> {
    if rel.starts_with('/') {
        return Err("give a path inside your folder, not an absolute one".into());
    }
    let v: Vec<&str> = rel.split('/').filter(|p| !p.is_empty() && *p != ".").collect();
    for p in &v {
        if *p == ".." {
            return Err("\"..\" is not allowed".into());
        }
        if p.len() > MAX_NAME || p.chars().any(|c| c.is_control()) {
            return Err("a name is at most 255 bytes and has no control characters".into());
        }
    }
    Ok(v)
}

#[cfg(unix)]
mod at {
    use std::ffi::CString;
    use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
    use std::path::Path;

    fn c(s: &str) -> Result<CString, String> {
        CString::new(s).map_err(|_| "a name has a NUL byte".to_string())
    }
    fn errno() -> i32 {
        std::io::Error::last_os_error().raw_os_error().unwrap_or(0)
    }
    fn fail(what: &str) -> String {
        match errno() {
            libc::ENOENT => "no such file".to_string(),
            libc::ELOOP | libc::ENOTDIR => format!("{what}: a link or a file is on the way; paths here are plain folders"),
            libc::EEXIST | libc::ENOTEMPTY => "already exists".to_string(),
            e => format!("{what}: {}", std::io::Error::from_raw_os_error(e)),
        }
    }

    /// `base` opened as a folder (a link is refused), then each part opened with `O_NOFOLLOW`: a link swapped in after the path check is
    /// still never followed. With `make` the missing folders are made (mode 0700).
    pub fn dir(base: &Path, comps: &[&str], make: bool) -> Result<OwnedFd, String> {
        let b = c(&base.to_string_lossy())?;
        let fd = unsafe { libc::open(b.as_ptr(), libc::O_RDONLY | libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC) };
        if fd < 0 {
            return Err(fail("the folder"));
        }
        let mut cur = unsafe { OwnedFd::from_raw_fd(fd) };
        for name in comps {
            let n = c(name)?;
            let flags = libc::O_RDONLY | libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC;
            let mut fd = unsafe { libc::openat(cur.as_raw_fd(), n.as_ptr(), flags) };
            if fd < 0 && make && errno() == libc::ENOENT {
                if unsafe { libc::mkdirat(cur.as_raw_fd(), n.as_ptr(), 0o700) } != 0 && errno() != libc::EEXIST {
                    return Err(fail("make the folder"));
                }
                fd = unsafe { libc::openat(cur.as_raw_fd(), n.as_ptr(), flags) };
            }
            if fd < 0 {
                return Err(fail(name));
            }
            cur = unsafe { OwnedFd::from_raw_fd(fd) };
        }
        Ok(cur)
    }

    /// 0: nothing there, 1: a file (or anything but a folder, a link included), 2: a folder. Never follows a link.
    pub fn kind(d: &OwnedFd, name: &str) -> Result<u8, String> {
        let n = c(name)?;
        let mut st: libc::stat = unsafe { std::mem::zeroed() };
        if unsafe { libc::fstatat(d.as_raw_fd(), n.as_ptr(), &mut st, libc::AT_SYMLINK_NOFOLLOW) } != 0 {
            return if errno() == libc::ENOENT { Ok(0) } else { Err(fail(name)) };
        }
        Ok(if st.st_mode & libc::S_IFMT == libc::S_IFDIR { 2 } else { 1 })
    }

    pub fn mkdir(d: &OwnedFd, name: &str) -> Result<(), String> {
        let n = c(name)?;
        if unsafe { libc::mkdirat(d.as_raw_fd(), n.as_ptr(), 0o700) } != 0 {
            return Err(fail("make the folder"));
        }
        Ok(())
    }

    pub fn rename(from: &OwnedFd, a: &str, to: &OwnedFd, b: &str) -> Result<(), String> {
        let (a, b) = (c(a)?, c(b)?);
        if unsafe { libc::renameat(from.as_raw_fd(), a.as_ptr(), to.as_raw_fd(), b.as_ptr()) } != 0 {
            return Err(fail("move"));
        }
        Ok(())
    }
}

/// Move or rename `from` to `to`, both paths inside `base` (the caller's own folder for the scope; the route checked the rights). Each folder
/// on the way is opened from `base` without following links, so nothing leaves `base` even if a link appears while this runs. The folder
/// of `to` must exist. A file may replace a file only with `overwrite`; a folder is never replaced; a folder cannot move into itself.
/// `base` itself cannot be moved. A rename moves no data, so the size a person uses does not change.
#[cfg(unix)]
pub fn rename(base: &Path, from: &str, to: &str, overwrite: bool) -> Result<(), String> {
    let (f, t) = (parts(from)?, parts(to)?);
    let (Some((fname, fdir)), Some((tname, tdir))) = (f.split_last(), t.split_last()) else {
        return Err("give the path to move and the path to move it to; the folder as a whole stays".into());
    };
    if t.len() >= f.len() && t[..f.len()] == f[..] {
        return if t == f { Ok(()) } else { Err("a folder cannot be moved into itself".into()) };
    }
    let (fd, td) = (at::dir(base, fdir, false)?, at::dir(base, tdir, false).map_err(|e| if e == "no such file" { "no such folder to move it into".to_string() } else { e })?);
    let src = at::kind(&fd, fname)?;
    if src == 0 {
        return Err("no such file".into());
    }
    match at::kind(&td, tname)? {
        0 => {}
        2 => return Err("already exists (a folder is never replaced)".into()),
        _ if src == 2 => return Err("already exists (a folder cannot replace a file)".into()),
        _ if !overwrite => return Err("already exists (send overwrite:true to replace it)".into()),
        _ => {}
    }
    at::rename(&fd, fname, &td, tname)
}

/// Make the folder `rel` inside `base` (missing parents too, mode 0700), without following a link on the way. A name that exists is refused.
#[cfg(unix)]
pub fn mkdir(base: &Path, rel: &str) -> Result<(), String> {
    let p = parts(rel)?;
    let Some((name, dirs)) = p.split_last() else { return Err("give the name of the folder".into()) };
    let d = at::dir(base, dirs, true)?;
    if at::kind(&d, name)? != 0 {
        return Err("already exists".into());
    }
    at::mkdir(&d, name)
}

// ------------------------------------------------------------------------------------------------------------------ binary

static UPLOADS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

fn sha_hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

/// Stream exactly `declared` bytes from `src` into `file` (made if missing, replaced atomically). Refused before reading when `declared`
/// is above `max`; refused, with nothing left behind, when `src` ends early, when a folder or a link is at `file`, or when `want_sha`
/// (hex SHA-256) does not match what arrived. Returns the byte count and the SHA-256.
pub fn write_stream(file: &Path, src: &mut dyn std::io::Read, declared: u64, max: u64, want_sha: Option<&str>) -> Result<(u64, String), String> {
    use sha2::{Digest, Sha256};
    use std::io::Write;
    if declared > max {
        return Err(format!("the file is larger than {} MiB", max / (1024 * 1024)));
    }
    if let Some(w) = want_sha {
        if w.len() != 64 || !w.bytes().all(|c| c.is_ascii_hexdigit()) {
            return Err("the checksum is 64 hex digits (SHA-256)".into());
        }
    }
    match std::fs::symlink_metadata(file) {
        Ok(m) if !m.is_file() => return Err("that is not a regular file".into()),
        _ => {}
    }
    let parent = file.parent().ok_or("no folder to write in")?;
    std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    let tmp = parent.join(format!(".iyakki-up-{}-{}", std::process::id(), UPLOADS.fetch_add(1, std::sync::atomic::Ordering::Relaxed)));
    let r = (|| -> Result<(u64, String), String> {
        use std::os::unix::fs::OpenOptionsExt;
        let mut out = std::fs::OpenOptions::new().write(true).create_new(true).mode(0o600).open(&tmp).map_err(|e| e.to_string())?;
        let (mut h, mut left, mut buf) = (Sha256::new(), declared, vec![0u8; 64 * 1024]);
        while left > 0 {
            let want = buf.len().min(left as usize);
            let n = src.read(&mut buf[..want]).map_err(|e| format!("the upload was interrupted: {e}"))?;
            if n == 0 {
                return Err(format!("the body ended after {} of {declared} bytes", declared - left));
            }
            h.update(&buf[..n]);
            out.write_all(&buf[..n]).map_err(|e| e.to_string())?;
            left -= n as u64;
        }
        out.flush().and_then(|_| out.sync_all()).map_err(|e| e.to_string())?;
        let sum = sha_hex(&h.finalize());
        if want_sha.is_some_and(|w| !w.eq_ignore_ascii_case(&sum)) {
            return Err("the checksum does not match what arrived; nothing was stored".into());
        }
        std::fs::rename(&tmp, file).map_err(|e| e.to_string())?;
        Ok((declared, sum))
    })();
    if r.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    r
}

/// The file opened for a download, and its length: a regular file only, never through a link.
pub fn open_for_download(file: &Path) -> Result<(std::fs::File, u64), String> {
    use std::os::unix::fs::OpenOptionsExt;
    let m = std::fs::symlink_metadata(file).map_err(|_| "no such file".to_string())?;
    if !m.is_file() {
        return Err("that is not a regular file".into());
    }
    let f = std::fs::OpenOptions::new().read(true).custom_flags(libc::O_NOFOLLOW).open(file).map_err(|_| "no such file".to_string())?;
    let m = f.metadata().map_err(|e| e.to_string())?;
    if !m.is_file() {
        return Err("that is not a regular file".into());
    }
    Ok((f, m.len()))
}

/// Copy exactly `len` bytes from `src` to `out` in 64 KiB pieces. A source that ends early is an error (the length was already promised).
pub fn copy_exact(src: &mut dyn std::io::Read, out: &mut dyn std::io::Write, len: u64) -> Result<(), String> {
    let (mut left, mut buf) = (len, vec![0u8; 64 * 1024]);
    while left > 0 {
        let want = buf.len().min(left as usize);
        let n = src.read(&mut buf[..want]).map_err(|e| e.to_string())?;
        if n == 0 {
            return Err("the file became shorter while it was sent".into());
        }
        out.write_all(&buf[..n]).map_err(|e| e.to_string())?;
        left -= n as u64;
    }
    Ok(())
}

/// A file name that is safe in a `Content-Disposition` header: letters, digits, `.`, `_` and `-`; anything else becomes `_`.
pub fn download_name(file: &Path) -> String {
    let n: String = file.file_name().and_then(|n| n.to_str()).unwrap_or("").chars().map(|c| if c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-') { c } else { '_' }).take(100).collect();
    if n.is_empty() || n.starts_with('.') { format!("download{n}") } else { n }
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

    /// A source that produces `n` bytes without ever holding them, and records the largest buffer it was asked to fill.
    struct Lazy {
        left: u64,
        biggest: usize,
    }
    impl std::io::Read for Lazy {
        fn read(&mut self, b: &mut [u8]) -> std::io::Result<usize> {
            let n = b.len().min(self.left as usize);
            self.biggest = self.biggest.max(b.len());
            b[..n].fill(7);
            self.left -= n as u64;
            Ok(n)
        }
    }

    #[test]
    fn binary_files_stream_in_small_pieces_with_a_checksum_and_exact_length() {
        use sha2::{Digest, Sha256};
        let base = dir("bin");
        let bytes: Vec<u8> = (0..200_000u32).map(|i| (i * 31 % 251) as u8).collect();
        let want = sha_hex(&Sha256::digest(&bytes));
        let (n, sum) = write_stream(&base.join("a/b.bin"), &mut &bytes[..], bytes.len() as u64, MAX_BINARY, Some(&want)).unwrap();
        assert_eq!((n, sum.as_str()), (200_000, want.as_str()));
        assert_eq!(std::fs::read(base.join("a/b.bin")).unwrap(), bytes, "bytes with zeros and 0xff survive");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(std::fs::metadata(base.join("a/b.bin")).unwrap().permissions().mode() & 0o777, 0o600);
        }
        // wrong checksum, short body, over the limit: refused and nothing left (the old file stays)
        let before = std::fs::read(base.join("a/b.bin")).unwrap();
        assert!(write_stream(&base.join("a/b.bin"), &mut &b"new"[..], 3, MAX_BINARY, Some(&"0".repeat(64))).unwrap_err().contains("checksum"));
        assert!(write_stream(&base.join("a/b.bin"), &mut &b"new"[..], 10, MAX_BINARY, None).unwrap_err().contains("ended after 3 of 10"));
        assert!(write_stream(&base.join("a/b.bin"), &mut &b"new"[..], 3, MAX_BINARY, Some("xyz")).unwrap_err().contains("64 hex"));
        let mut never = Lazy { left: u64::MAX, biggest: 0 };
        assert!(write_stream(&base.join("big"), &mut never, MAX_BINARY + 1, MAX_BINARY, None).unwrap_err().contains("larger than"));
        assert_eq!(never.left, u64::MAX, "refused before a byte was read");
        assert_eq!(std::fs::read(base.join("a/b.bin")).unwrap(), before);
        assert!(!base.join("big").exists());
        assert_eq!(std::fs::read_dir(base.join("a")).unwrap().count(), 1, "no temporary file is left");
        // a folder is not overwritten, a link is not followed
        assert!(write_stream(&base.join("a"), &mut &b"x"[..], 1, MAX_BINARY, None).is_err());
        #[cfg(unix)]
        {
            let outside = dir("bin-outside");
            std::os::unix::fs::symlink(outside.join("t"), base.join("lnk")).unwrap();
            assert!(write_stream(&base.join("lnk"), &mut &b"x"[..], 1, MAX_BINARY, None).is_err());
            assert!(!outside.join("t").exists());
        }
        // a large body is read in pieces no bigger than 64 KiB, and the size on disk is right
        let mut big = Lazy { left: 40 * 1024 * 1024, biggest: 0 };
        let (n, _) = write_stream(&base.join("huge.bin"), &mut big, 40 * 1024 * 1024, MAX_BINARY, None).unwrap();
        assert_eq!((n, big.biggest), (40 * 1024 * 1024, 64 * 1024));
        assert_eq!(std::fs::metadata(base.join("huge.bin")).unwrap().len(), n);
    }

    #[cfg(unix)]
    #[test]
    fn a_download_opens_a_regular_file_only_and_sends_exactly_its_length() {
        let base = dir("down");
        std::fs::write(base.join("f.bin"), [0u8, 255, 10, 13, 0]).unwrap();
        let (mut f, len) = open_for_download(&base.join("f.bin")).unwrap();
        let mut out = Vec::new();
        copy_exact(&mut f, &mut out, len).unwrap();
        assert_eq!(out, [0u8, 255, 10, 13, 0]);
        // promised more than there is: an error, not a short success
        let (mut f, _) = open_for_download(&base.join("f.bin")).unwrap();
        assert!(copy_exact(&mut f, &mut Vec::new(), 99).unwrap_err().contains("shorter"));
        assert!(open_for_download(&base).unwrap_err().contains("regular"), "a folder");
        assert!(open_for_download(&base.join("gone")).is_err());
        let outside = dir("down-outside");
        std::fs::write(outside.join("secret"), "s").unwrap();
        std::os::unix::fs::symlink(outside.join("secret"), base.join("lnk")).unwrap();
        assert!(open_for_download(&base.join("lnk")).is_err(), "a link is never followed");
        assert_eq!(download_name(&base.join("a b\"c.txt")), "a_b_c.txt");
        assert_eq!(download_name(Path::new("/")), "download");
        assert_eq!(download_name(Path::new(".env")), "download.env");
    }
}
