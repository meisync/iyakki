//! The scope-confined file layer of the SFTP server. A person sees what their account's file rules give them and nothing else.
//!
//! The virtual tree: `/` is the person's home folder; every other folder their account may read (a workspace, a scope such as
//! `/shared`, a granted host folder) shows as `/+name`. Anything else does not exist. A path from the client is
//! 1. split on `/`, with `.` and empty parts dropped and `..` popping a part (never above `/`, as in a chroot), then
//! 2. walked one part at a time from a real, canonical base: after each part the path is canonicalised (links followed) and must still
//!    be allowed by the account's rules (`read_ok`); a link that leads out of the allowed folders makes the part "not found".
//!
//! So `..`, absolute paths, doubled slashes and symlinks cannot reach outside. Writing (create, write, mkdir, remove, rename,
//! setstat) needs `write_ok` as well, on every real path involved. A file is opened with `O_NOFOLLOW` and checked to be a regular
//! file after opening. A time-of-check/time-of-use race against another program that swaps a folder for a link while a request runs is
//! not closed (the same limit every path-based server has); links inside the person's own folders are the person's own affair.
//!
//! The rules are closures (`FileRoots` in the real server, plain prefixes in the tests), so this module is testable without accounts.

use std::fs::{File, OpenOptions};
use std::io::ErrorKind;
use std::os::unix::fs::{FileExt, MetadataExt, OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use crate::sftp_proto::*;

/// The most entries one folder listing keeps.
pub const MAX_ENTRIES: usize = 50_000;

#[derive(Debug, PartialEq, Eq)]
pub enum FsErr {
    NotFound,
    Denied,
    Exists,
    Unsupported(&'static str),
    Failure(String),
}

impl FsErr {
    pub fn code(&self) -> u32 {
        match self {
            FsErr::NotFound => FX_NO_SUCH_FILE,
            FsErr::Denied => FX_PERMISSION_DENIED,
            FsErr::Unsupported(_) => FX_OP_UNSUPPORTED,
            FsErr::Exists | FsErr::Failure(_) => FX_FAILURE,
        }
    }
    pub fn text(&self) -> String {
        match self {
            FsErr::NotFound => "No such file".into(),
            FsErr::Denied => "Permission denied".into(),
            FsErr::Exists => "File exists".into(),
            FsErr::Unsupported(s) => format!("Unsupported: {s}"),
            FsErr::Failure(s) => s.clone(),
        }
    }
}

fn io_err(e: std::io::Error) -> FsErr {
    match e.kind() {
        ErrorKind::NotFound => FsErr::NotFound,
        ErrorKind::PermissionDenied => FsErr::Denied,
        ErrorKind::AlreadyExists => FsErr::Exists,
        _ => match e.raw_os_error() {
            Some(20) => FsErr::Failure("Not a directory".into()),
            Some(21) => FsErr::Failure("Is a directory".into()),
            Some(39) => FsErr::Failure("Directory not empty".into()),
            Some(28) => FsErr::Failure("No space left on device".into()),
            Some(30) => FsErr::Failure("Read-only file system".into()),
            Some(40) => FsErr::Denied,
            _ => FsErr::Failure("Operation failed".into()),
        },
    }
}

pub type Rule = Arc<dyn Fn(&Path) -> bool + Send + Sync>;

pub struct Jail {
    /// The real, canonical home folder (the virtual `/`).
    home: PathBuf,
    /// Other readable folders, shown as `/+name`.
    extra: Vec<(String, PathBuf)>,
    read_ok: Rule,
    write_ok: Rule,
}

/// A path found inside the tree.
#[derive(Debug)]
pub struct Found {
    pub real: PathBuf,
    /// True for `/` and `/+name`: these are never removed, renamed or changed.
    pub is_root: bool,
    /// The normalised virtual path (`/a/b`).
    pub virt: String,
}

fn parts(v: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for p in v.split('/') {
        match p {
            "" | "." => {}
            ".." => {
                out.pop();
            }
            n => out.push(n.to_string()),
        }
    }
    out
}

fn join_virt(p: &[String]) -> String {
    if p.is_empty() { "/".into() } else { format!("/{}", p.join("/")) }
}

/// A short, safe name for a folder shown as `/+name`: letters, digits, `-`, `_`, `.`; the rest becomes `_`.
fn extra_name(p: &Path) -> String {
    let n: String = p.file_name().map(|s| s.to_string_lossy().into_owned()).unwrap_or_else(|| "root".into()).chars().map(|c| if c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.') { c } else { '_' }).collect();
    let n = n.trim_start_matches('.');
    if n.is_empty() { "folder".into() } else { n.chars().take(40).collect() }
}

impl Jail {
    /// `home` and the folders in `others` must be real paths. Names for `others` are made from their last part and made unique.
    pub fn new(home: PathBuf, others: Vec<PathBuf>, read_ok: Rule, write_ok: Rule) -> Jail {
        let mut extra: Vec<(String, PathBuf)> = Vec::new();
        for p in others {
            if p == home || p.starts_with(&home) || extra.iter().any(|(_, q)| *q == p) {
                continue;
            }
            let base = extra_name(&p);
            let mut name = format!("+{base}");
            let mut i = 2;
            while extra.iter().any(|(n, _)| *n == name) {
                name = format!("+{base}-{i}");
                i += 1;
            }
            extra.push((name, p));
        }
        Jail { home, extra, read_ok, write_ok }
    }

    pub fn home(&self) -> &Path {
        &self.home
    }

    fn base_of<'a>(&'a self, comps: &'a [String]) -> (&'a Path, &'a [String], bool) {
        if let Some(first) = comps.first() {
            if let Some((_, p)) = self.extra.iter().find(|(n, _)| n == first) {
                return (p, &comps[1..], comps.len() == 1);
            }
        }
        (&self.home, comps, comps.is_empty())
    }

    /// Walk `rel` below `base`, one part at a time. `last_may_be_missing` lets the final part not exist yet (create); `follow_last`
    /// is false for lstat, remove and rename, which act on a link itself.
    fn walk(&self, base: &Path, rel: &[String], last_may_be_missing: bool, follow_last: bool) -> Result<PathBuf, FsErr> {
        let mut cur = base.to_path_buf();
        for (i, c) in rel.iter().enumerate() {
            let last = i + 1 == rel.len();
            let next = cur.join(c);
            if last && !follow_last {
                // the final part is used as it is; its folder is already checked. A link is never followed here.
                if std::fs::symlink_metadata(&next).is_err() && !last_may_be_missing {
                    return Err(FsErr::NotFound);
                }
                return Ok(next);
            }
            match std::fs::canonicalize(&next) {
                Ok(p) => {
                    if !(self.read_ok)(&p) {
                        // a link that leads out looks like a missing file, so nothing is learned about what is out there
                        return Err(FsErr::NotFound);
                    }
                    cur = p;
                }
                Err(e) if e.kind() == ErrorKind::NotFound && last && last_may_be_missing => {
                    // a dangling link must not be created through: O_NOFOLLOW on open makes that fail
                    cur = next;
                }
                Err(e) => return Err(io_err(e)),
            }
        }
        Ok(cur)
    }

    fn find_with(&self, v: &str, missing_ok: bool, follow: bool) -> Result<Found, FsErr> {
        if v.contains('\0') {
            return Err(FsErr::NotFound);
        }
        let comps = parts(v);
        let (base, rel, is_root) = self.base_of(&comps);
        let real = self.walk(base, rel, missing_ok, follow)?;
        let probe = if follow || rel.is_empty() { real.as_path() } else { real.parent().unwrap_or(&real) };
        if !(self.read_ok)(probe) {
            return Err(FsErr::NotFound);
        }
        Ok(Found { real, is_root, virt: join_virt(&comps) })
    }

    /// An existing path (links followed).
    pub fn find(&self, v: &str) -> Result<Found, FsErr> {
        self.find_with(v, false, true)
    }
    /// An existing path whose last part is not followed if it is a link.
    pub fn find_link(&self, v: &str) -> Result<Found, FsErr> {
        self.find_with(v, false, false)
    }
    /// A path that may not exist yet (its folder must).
    pub fn find_new(&self, v: &str) -> Result<Found, FsErr> {
        self.find_with(v, true, true)
    }

    fn may_write(&self, f: &Found) -> Result<(), FsErr> {
        if f.is_root || !(self.write_ok)(&f.real) {
            return Err(FsErr::Denied);
        }
        Ok(())
    }

    /// The normalised virtual path; it need not exist (the last part may be missing) but its folder must be inside the tree.
    pub fn realpath(&self, v: &str) -> Result<String, FsErr> {
        let comps = parts(v);
        if comps.is_empty() {
            return Ok("/".into());
        }
        // the folder must exist and be allowed; the last part is only a name
        match self.find_with(v, true, true) {
            Ok(f) => Ok(f.virt),
            Err(FsErr::NotFound) if comps.len() > 1 => Err(FsErr::NotFound),
            Err(e) => Err(e),
        }
    }

    // ------------------------------------------------------------------ reading

    pub fn attrs_of(m: &std::fs::Metadata) -> Attrs {
        Attrs { size: Some(m.len()), uid_gid: Some((m.uid(), m.gid())), perms: Some(m.mode()), times: Some((clamp32(m.atime()), clamp32(m.mtime()))) }
    }

    pub fn stat(&self, v: &str, follow: bool) -> Result<Attrs, FsErr> {
        let f = if follow { self.find(v)? } else { self.find_link(v)? };
        if !follow && !f.is_root && std::fs::symlink_metadata(&f.real).is_ok_and(|m| m.file_type().is_symlink()) && !std::fs::canonicalize(&f.real).is_ok_and(|t| (self.read_ok)(&t)) {
            // a link that leads out is not shown, as in a listing
            return Err(FsErr::NotFound);
        }
        let m = if follow { std::fs::metadata(&f.real) } else { std::fs::symlink_metadata(&f.real) }.map_err(io_err)?;
        Ok(Self::attrs_of(&m))
    }

    /// All entries of a folder (the folder `/` also lists the `+name` folders), without those whose link leads out of the tree.
    pub fn list(&self, v: &str) -> Result<Vec<(String, String, Attrs)>, FsErr> {
        let f = self.find(v)?;
        let md = std::fs::metadata(&f.real).map_err(io_err)?;
        if !md.is_dir() {
            return Err(FsErr::Failure("Not a directory".into()));
        }
        let mut out = Vec::new();
        if f.virt == "/" {
            for (n, p) in &self.extra {
                if let Ok(m) = std::fs::metadata(p) {
                    let a = Self::attrs_of(&m);
                    out.push((n.clone(), long_name(n, &a), a));
                }
            }
        }
        for e in std::fs::read_dir(&f.real).map_err(io_err)? {
            let Ok(e) = e else { continue };
            if out.len() >= MAX_ENTRIES {
                break;
            }
            let name = e.file_name().to_string_lossy().into_owned();
            if name.contains('\0') || name.len() > 255 {
                continue;
            }
            let Ok(lm) = std::fs::symlink_metadata(e.path()) else { continue };
            if lm.file_type().is_symlink() {
                match std::fs::canonicalize(e.path()) {
                    Ok(t) if (self.read_ok)(&t) => {}
                    _ => continue,
                }
            }
            let a = Self::attrs_of(&lm);
            out.push((name.clone(), long_name(&name, &a), a));
        }
        out.sort_by(|a, b| a.0.cmp(&b.0));
        Ok(out)
    }

    /// Open a file. Returns the file and its virtual path.
    pub fn open(&self, v: &str, flags: u32, attrs: &Attrs) -> Result<(File, String), FsErr> {
        let writing = flags & (OPEN_WRITE | OPEN_APPEND | OPEN_CREAT | OPEN_TRUNC) != 0;
        let f = if flags & OPEN_CREAT != 0 { self.find_new(v)? } else { self.find(v)? };
        if f.is_root {
            return Err(FsErr::Failure("Is a directory".into()));
        }
        if flags & (OPEN_READ | OPEN_WRITE | OPEN_APPEND) == 0 {
            return Err(FsErr::Failure("open for neither reading nor writing".into()));
        }
        if writing {
            self.may_write(&f)?;
        }
        match std::fs::symlink_metadata(&f.real) {
            Ok(m) if !m.file_type().is_file() => return Err(FsErr::Failure(if m.is_dir() { "Is a directory" } else { "Not a regular file" }.into())),
            Ok(_) if flags & OPEN_CREAT != 0 && flags & OPEN_EXCL != 0 => return Err(FsErr::Exists),
            Err(e) if e.kind() != ErrorKind::NotFound => return Err(io_err(e)),
            Err(_) if flags & OPEN_CREAT == 0 => return Err(FsErr::NotFound),
            _ => {}
        }
        let mut o = OpenOptions::new();
        o.read(flags & OPEN_READ != 0).write(flags & (OPEN_WRITE | OPEN_APPEND) != 0 && flags & OPEN_APPEND == 0).append(flags & OPEN_APPEND != 0);
        if flags & OPEN_CREAT != 0 {
            o.create(true);
            if flags & OPEN_EXCL != 0 {
                o.create_new(true);
            }
            o.mode(attrs.perms.map_or(0o644, |p| p & 0o777));
        }
        if flags & OPEN_TRUNC != 0 {
            o.truncate(true);
        }
        o.custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK);
        let file = o.open(&f.real).map_err(io_err)?;
        let m = file.metadata().map_err(io_err)?;
        if !m.file_type().is_file() {
            return Err(FsErr::Failure("Not a regular file".into()));
        }
        Ok((file, f.virt))
    }

    // ------------------------------------------------------------------ changing

    pub fn mkdir(&self, v: &str, attrs: &Attrs) -> Result<String, FsErr> {
        let f = self.find_new(v)?;
        self.may_write(&f)?;
        if std::fs::symlink_metadata(&f.real).is_ok() {
            return Err(FsErr::Exists);
        }
        std::fs::create_dir(&f.real).map_err(io_err)?;
        let _ = std::fs::set_permissions(&f.real, std::fs::Permissions::from_mode(attrs.perms.map_or(0o755, |p| p & 0o777)));
        Ok(f.virt)
    }

    pub fn rmdir(&self, v: &str) -> Result<String, FsErr> {
        let f = self.find_link(v)?;
        self.may_write(&f)?;
        let m = std::fs::symlink_metadata(&f.real).map_err(io_err)?;
        if !m.is_dir() {
            return Err(FsErr::Failure("Not a directory".into()));
        }
        std::fs::remove_dir(&f.real).map_err(io_err)?;
        Ok(f.virt)
    }

    /// Remove a file or a link (the link itself, never its target).
    pub fn remove(&self, v: &str) -> Result<String, FsErr> {
        let f = self.find_link(v)?;
        self.may_write(&f)?;
        let m = std::fs::symlink_metadata(&f.real).map_err(io_err)?;
        if m.is_dir() {
            return Err(FsErr::Failure("Is a directory".into()));
        }
        std::fs::remove_file(&f.real).map_err(io_err)?;
        Ok(f.virt)
    }

    /// Rename; never replaces an existing file, as SFTP version 3 says.
    pub fn rename(&self, from: &str, to: &str) -> Result<(String, String), FsErr> {
        let a = self.find_link(from)?;
        let b = self.find_new(to)?;
        self.may_write(&a)?;
        self.may_write(&b)?;
        if std::fs::symlink_metadata(&b.real).is_ok() {
            return Err(FsErr::Failure("Target exists".into()));
        }
        // a folder must not move into itself
        if b.real.starts_with(&a.real) {
            return Err(FsErr::Failure("Cannot move a folder into itself".into()));
        }
        std::fs::rename(&a.real, &b.real).map_err(io_err)?;
        Ok((a.virt, b.virt))
    }

    /// Permissions (the lower 9 bits only), size and times of a path. The owner is never changed.
    pub fn setstat(&self, v: &str, a: &Attrs) -> Result<String, FsErr> {
        let f = self.find(v)?;
        self.may_write(&f)?;
        if let Some(p) = a.perms {
            std::fs::set_permissions(&f.real, std::fs::Permissions::from_mode(p & 0o777)).map_err(io_err)?;
        }
        if let Some(s) = a.size {
            let file = OpenOptions::new().write(true).custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK).open(&f.real).map_err(io_err)?;
            file.set_len(s).map_err(io_err)?;
        }
        if let Some((at, mt)) = a.times {
            let file = File::open(&f.real).map_err(io_err)?;
            let t = |s: u32| std::time::UNIX_EPOCH + std::time::Duration::from_secs(s as u64);
            file.set_times(std::fs::FileTimes::new().set_accessed(t(at)).set_modified(t(mt))).map_err(io_err)?;
        }
        Ok(f.virt)
    }
}

/// Apply attributes to an open file (FSETSTAT). The file was opened through the jail, so it is already inside.
pub fn fset(file: &File, writable: bool, a: &Attrs) -> Result<(), FsErr> {
    if let Some(p) = a.perms {
        if !writable {
            return Err(FsErr::Denied);
        }
        file.set_permissions(std::fs::Permissions::from_mode(p & 0o777)).map_err(io_err)?;
    }
    if let Some(s) = a.size {
        if !writable {
            return Err(FsErr::Denied);
        }
        file.set_len(s).map_err(io_err)?;
    }
    Ok(())
}

pub fn read_at(file: &File, offset: u64, len: u32) -> Result<Vec<u8>, FsErr> {
    if offset > i64::MAX as u64 {
        return Err(FsErr::Failure("offset out of range".into()));
    }
    let mut buf = vec![0u8; len.min(MAX_READ) as usize];
    let mut got = 0;
    while got < buf.len() {
        match file.read_at(&mut buf[got..], offset + got as u64) {
            Ok(0) => break,
            Ok(n) => got += n,
            Err(e) if e.kind() == ErrorKind::Interrupted => {}
            Err(e) => return Err(io_err(e)),
        }
    }
    buf.truncate(got);
    Ok(buf)
}

pub fn write_at(file: &File, offset: u64, data: &[u8]) -> Result<(), FsErr> {
    if offset > i64::MAX as u64 - data.len() as u64 {
        return Err(FsErr::Failure("offset out of range".into()));
    }
    file.write_all_at(data, offset).map_err(io_err)
}

fn clamp32(v: i64) -> u32 {
    v.clamp(0, u32::MAX as i64) as u32
}

/// `drwxr-xr-x    1 1000     1000         4096 Nov  8 2026 name`: the line `ls -l` style clients show.
pub fn long_name(name: &str, a: &Attrs) -> String {
    let mode = a.perms.unwrap_or(0);
    let kind = match mode & 0o170000 {
        0o040000 => 'd',
        0o120000 => 'l',
        0o100000 => '-',
        _ => '?',
    };
    let mut p = String::new();
    for (i, c) in "rwxrwxrwx".chars().enumerate() {
        p.push(if mode & (1 << (8 - i)) != 0 { c } else { '-' });
    }
    let (u, g) = a.uid_gid.unwrap_or((0, 0));
    let (y, mo, d) = civil(a.times.map_or(0, |t| t.1) as i64 / 86400);
    const MON: [&str; 12] = ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"];
    format!("{kind}{p} 1 {u:<8} {g:<8} {:>10} {} {d:>2} {y} {name}", a.size.unwrap_or(0), MON[(mo - 1) as usize % 12])
}

/// Days since 1970-01-01 to (year, month, day) in UTC.
fn civil(z: i64) -> (i64, u32, u32) {
    let z = z + 719468;
    let era = z.div_euclid(146097);
    let doe = z.rem_euclid(146097);
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (yoe + era * 400 + i64::from(m <= 2), m, d)
}

#[cfg(test)]
pub(crate) fn civil_for_test(z: i64) -> (i64, u32, u32) {
    civil(z)
}
