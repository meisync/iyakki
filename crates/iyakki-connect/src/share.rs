//! `--share DIR`: one folder of this computer that the page may exchange files with. Read-only unless `--write`.
//!
//! The jail is not a path check followed by a path open (that can be raced: swap a folder for a link after the check). Every access walks
//! the path component by component from a descriptor of the shared folder with `openat(O_NOFOLLOW)`, so a link anywhere in the path is
//! refused at the moment it would be followed, and nothing is ever opened by a name that was only checked earlier. Stricter than "links
//! that leave the folder": no link is followed at all, inside or out. A name is plain: no `..`, no `.`, no empty part, no leading `/`, no
//! backslash, no control character or NUL, at most 255 bytes a part. Files are regular files only (no device, pipe or folder is read).
//!
//! Writing makes a private temporary file (`.iyakki-part-*`, mode 600) in the target folder and moves it into place: a reader never sees
//! half a file. Without `replace` an existing name is `Exists` (the page asks the person first; the answer defaults to no) and is never
//! overwritten, also not by a racing writer (the move is a hard link, which refuses an existing name).

use std::ffi::CString;
use std::io::{Read, Write};
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd, RawFd};
use std::path::{Path, PathBuf};

/// The biggest file moved either way (the tenant's own upload limit).
pub const MAX_FILE: u64 = 128 * 1024 * 1024;
pub const MAX_ENTRIES: usize = 1000;
const PART: &str = ".iyakki-part-";

#[derive(Debug, PartialEq, Eq)]
pub enum ShareError {
    /// The path or the thing it names is not allowed (the sentence is safe to show).
    Refused(String),
    NotFound,
    /// The target exists and `replace` was not given.
    Exists,
    TooLarge,
    /// The share is read-only.
    ReadOnly,
    Io(String),
}

impl ShareError {
    pub fn text(&self) -> String {
        match self {
            ShareError::Refused(m) | ShareError::Io(m) => m.clone(),
            ShareError::NotFound => "no such file or folder in the shared folder".into(),
            ShareError::Exists => "that name already exists".into(),
            ShareError::TooLarge => format!("larger than the {} MiB limit", MAX_FILE >> 20),
            ShareError::ReadOnly => "the shared folder is read-only (start the program with --write to allow saving into it)".into(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Entry {
    pub name: String,
    /// `file`, `dir` or `link` (a link is shown but never opened).
    pub kind: &'static str,
    pub size: u64,
}

/// Split a path the page gave into plain parts. The empty path is the shared folder itself. Used for the tenant's side too.
pub fn split_rel(rel: &str) -> Result<Vec<&str>, ShareError> {
    let bad = |why: &str| Err(ShareError::Refused(format!("that path is not allowed ({why})")));
    if rel.is_empty() {
        return Ok(vec![]);
    }
    if rel.len() > 1024 {
        return bad("too long");
    }
    if rel.starts_with('/') {
        return bad("it must be inside the shared folder, not absolute");
    }
    if rel.chars().any(|c| c.is_control() || c == '\\') {
        return bad("control or backslash character");
    }
    let parts: Vec<&str> = rel.split('/').collect();
    for p in &parts {
        match *p {
            "" => return bad("empty part"),
            "." | ".." => return bad("'.' and '..' are not allowed"),
            p if p.len() > 255 => return bad("a name is too long"),
            p if p.starts_with(PART) => return bad("reserved name"),
            _ => {}
        }
    }
    Ok(parts)
}

fn cstr(s: &str) -> Result<CString, ShareError> {
    CString::new(s).map_err(|_| ShareError::Refused("that path is not allowed (NUL)".into()))
}

fn errno() -> i32 {
    std::io::Error::last_os_error().raw_os_error().unwrap_or(0)
}

fn io_err(op: &str) -> ShareError {
    let e = std::io::Error::last_os_error();
    match e.raw_os_error() {
        Some(libc::ENOENT) => ShareError::NotFound,
        Some(libc::ELOOP) | Some(libc::ENOTDIR) => ShareError::Refused("that path goes through a link or a file where a folder is needed: not allowed".into()),
        Some(libc::EEXIST) => ShareError::Exists,
        Some(libc::EACCES) | Some(libc::EPERM) => ShareError::Refused("the operating system does not allow that".into()),
        _ => ShareError::Io(format!("{op}: {e}")),
    }
}

fn owned(fd: RawFd, op: &str) -> Result<OwnedFd, ShareError> {
    if fd < 0 {
        return Err(io_err(op));
    }
    // SAFETY: `fd` is a freshly opened descriptor that nothing else owns.
    Ok(unsafe { OwnedFd::from_raw_fd(fd) })
}

pub struct Share {
    root: PathBuf,
    write: bool,
}

impl Share {
    /// `dir` must be an existing folder; it is made canonical here once (the one place a link may be followed: the person's own choice).
    pub fn new(dir: &Path, write: bool) -> Result<Share, String> {
        let root = std::fs::canonicalize(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
        if !std::fs::metadata(&root).map_err(|e| e.to_string())?.is_dir() {
            return Err(format!("{} is not a folder", dir.display()));
        }
        if root == Path::new("/") {
            return Err("sharing the whole file system is not allowed: choose a folder".into());
        }
        Ok(Share { root, write })
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn writable(&self) -> bool {
        self.write
    }

    fn root_fd(&self) -> Result<OwnedFd, ShareError> {
        let c = cstr(&self.root.to_string_lossy())?;
        // SAFETY: `c` is a valid NUL-terminated path.
        owned(unsafe { libc::open(c.as_ptr(), libc::O_RDONLY | libc::O_DIRECTORY | libc::O_CLOEXEC) }, "open the shared folder")
    }

    /// A descriptor of the folder `comps` names, reached without following any link.
    fn dir_at(&self, comps: &[&str]) -> Result<OwnedFd, ShareError> {
        let mut cur = self.root_fd()?;
        for c in comps {
            let name = cstr(c)?;
            // SAFETY: `cur` is an open directory descriptor and `name` a valid NUL-terminated string.
            let fd = unsafe { libc::openat(cur.as_raw_fd(), name.as_ptr(), libc::O_RDONLY | libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC) };
            cur = owned(fd, "open a folder")?;
        }
        Ok(cur)
    }

    pub fn list(&self, rel: &str) -> Result<Vec<Entry>, ShareError> {
        let comps = split_rel(rel)?;
        let dir = self.dir_at(&comps)?;
        // SAFETY: fdopendir takes over a duplicate, so `dir` stays ours.
        let dup = unsafe { libc::dup(dir.as_raw_fd()) };
        if dup < 0 {
            return Err(io_err("list"));
        }
        let d = unsafe { libc::fdopendir(dup) };
        if d.is_null() {
            unsafe { libc::close(dup) };
            return Err(io_err("list"));
        }
        let mut out = Vec::new();
        let mut too_many = false;
        loop {
            // SAFETY: `d` is a valid open directory stream until closedir below.
            let e = unsafe { libc::readdir(d) };
            if e.is_null() {
                break;
            }
            let name = unsafe { std::ffi::CStr::from_ptr((*e).d_name.as_ptr()) }.to_string_lossy().into_owned();
            if name == "." || name == ".." || name.starts_with(PART) {
                continue;
            }
            let c = cstr(&name)?;
            let mut st: libc::stat = unsafe { std::mem::zeroed() };
            if unsafe { libc::fstatat(dir.as_raw_fd(), c.as_ptr(), &mut st, libc::AT_SYMLINK_NOFOLLOW) } != 0 {
                continue;
            }
            let (kind, size) = match st.st_mode & libc::S_IFMT {
                libc::S_IFDIR => ("dir", 0),
                libc::S_IFREG => ("file", st.st_size as u64),
                libc::S_IFLNK => ("link", 0),
                _ => continue,
            };
            if name.chars().any(|c| c.is_control()) {
                continue;
            }
            out.push(Entry { name, kind, size });
            if out.len() > MAX_ENTRIES {
                too_many = true;
                break;
            }
        }
        unsafe { libc::closedir(d) };
        if too_many {
            return Err(ShareError::Refused(format!("more than {MAX_ENTRIES} entries: open a sub-folder")));
        }
        out.sort_by(|a, b| a.name.cmp(&b.name));
        Ok(out)
    }

    /// The bytes of one regular file.
    pub fn read(&self, rel: &str) -> Result<Vec<u8>, ShareError> {
        let comps = split_rel(rel)?;
        let (name, parent) = comps.split_last().ok_or_else(|| ShareError::Refused("name a file".into()))?;
        let dir = self.dir_at(parent)?;
        let c = cstr(name)?;
        // O_NONBLOCK so that a named pipe cannot hang the program; the type is checked on the open descriptor.
        let fd = unsafe { libc::openat(dir.as_raw_fd(), c.as_ptr(), libc::O_RDONLY | libc::O_NOFOLLOW | libc::O_CLOEXEC | libc::O_NONBLOCK) };
        let fd = owned(fd, "open the file")?;
        let mut st: libc::stat = unsafe { std::mem::zeroed() };
        if unsafe { libc::fstat(fd.as_raw_fd(), &mut st) } != 0 {
            return Err(io_err("stat"));
        }
        if st.st_mode & libc::S_IFMT != libc::S_IFREG {
            return Err(ShareError::Refused("that is not a regular file".into()));
        }
        if st.st_size as u64 > MAX_FILE {
            return Err(ShareError::TooLarge);
        }
        let mut f = std::fs::File::from(fd);
        let mut buf = Vec::with_capacity(st.st_size as usize);
        (&mut f).take(MAX_FILE + 1).read_to_end(&mut buf).map_err(|e| ShareError::Io(e.to_string()))?;
        if buf.len() as u64 > MAX_FILE {
            return Err(ShareError::TooLarge);
        }
        Ok(buf)
    }

    /// Does this name exist (a file, a folder or a link)? Never follows a link.
    pub fn exists(&self, rel: &str) -> Result<bool, ShareError> {
        let comps = split_rel(rel)?;
        let (name, parent) = comps.split_last().ok_or_else(|| ShareError::Refused("name a file".into()))?;
        let dir = match self.dir_at(parent) {
            Ok(d) => d,
            Err(ShareError::NotFound) => return Ok(false),
            Err(e) => return Err(e),
        };
        let c = cstr(name)?;
        let mut st: libc::stat = unsafe { std::mem::zeroed() };
        Ok(unsafe { libc::fstatat(dir.as_raw_fd(), c.as_ptr(), &mut st, libc::AT_SYMLINK_NOFOLLOW) } == 0)
    }

    /// Save `bytes` as `rel` (the folder must exist). Refuses when read-only, and an existing name unless `replace`.
    pub fn write(&self, rel: &str, bytes: &[u8], replace: bool) -> Result<(), ShareError> {
        if !self.write {
            return Err(ShareError::ReadOnly);
        }
        if bytes.len() as u64 > MAX_FILE {
            return Err(ShareError::TooLarge);
        }
        let comps = split_rel(rel)?;
        let (name, parent) = comps.split_last().ok_or_else(|| ShareError::Refused("name a file".into()))?;
        let dir = self.dir_at(parent)?;
        let target = cstr(name)?;
        let mut st: libc::stat = unsafe { std::mem::zeroed() };
        if unsafe { libc::fstatat(dir.as_raw_fd(), target.as_ptr(), &mut st, libc::AT_SYMLINK_NOFOLLOW) } == 0 {
            if st.st_mode & libc::S_IFMT != libc::S_IFREG {
                return Err(ShareError::Refused("that name is a folder or a link: it is never replaced".into()));
            }
            if !replace {
                return Err(ShareError::Exists);
            }
        }
        let mut rnd = [0u8; 8];
        iyakki_sec::random_fill(&mut rnd).map_err(|_| ShareError::Io("no random numbers".into()))?;
        let tmp_name = format!("{PART}{}", iyakki_link::sigscheme::hex(&rnd));
        let tmp = cstr(&tmp_name)?;
        let fd = unsafe { libc::openat(dir.as_raw_fd(), tmp.as_ptr(), libc::O_WRONLY | libc::O_CREAT | libc::O_EXCL | libc::O_NOFOLLOW | libc::O_CLOEXEC, 0o600 as libc::c_uint) };
        let mut f = std::fs::File::from(owned(fd, "create the file")?);
        let cleanup = |dir: &OwnedFd| {
            unsafe { libc::unlinkat(dir.as_raw_fd(), tmp.as_ptr(), 0) };
        };
        if let Err(e) = f.write_all(bytes).and_then(|_| f.sync_all()) {
            cleanup(&dir);
            return Err(ShareError::Io(e.to_string()));
        }
        drop(f);
        let r = if replace {
            // SAFETY: both names are relative to the same open directory; rename replaces a regular file, and never follows a link.
            unsafe { libc::renameat(dir.as_raw_fd(), tmp.as_ptr(), dir.as_raw_fd(), target.as_ptr()) }
        } else {
            // a hard link refuses an existing name, also when another writer made it a moment ago
            let r = unsafe { libc::linkat(dir.as_raw_fd(), tmp.as_ptr(), dir.as_raw_fd(), target.as_ptr(), 0) };
            let e = errno();
            cleanup(&dir);
            if r != 0 {
                return Err(if e == libc::EEXIST { ShareError::Exists } else { io_err("save") });
            }
            return Ok(());
        };
        if r != 0 {
            let e = io_err("save");
            cleanup(&dir);
            return Err(e);
        }
        Ok(())
    }
}
