//! Unpacking a base root file system (Debian or Ubuntu) for the Linux apps sandbox (`linuxapps.rs`, `docs/LINUX-APPS.md`).
//!
//! A root file system differs from a tool archive (`archive.rs`): it holds absolute symbolic links (`/bin/sh -> /usr/bin/dash` is
//! normal), hard links, and file owners other than the person. So this reader is stricter about the one thing that matters and
//! looser about what is normal:
//!
//! * every name must be relative with no `..`; the archive is refused as a whole otherwise;
//! * **no entry is ever created through a symbolic link**: before an entry is written, each folder of its path that already exists
//!   must be a real folder (a link named `etc` pointing at `/etc` followed by `etc/passwd` is the classic way out);
//! * a symbolic link may point anywhere (it is only ever followed *inside* the sandbox, after `pivot_root`, never by IYAKKI);
//! * a hard link must name a regular file already unpacked in this archive (never a link, never a path outside);
//! * device nodes, pipes and sockets are never created (they are listed in the report); setuid and setgid bits are removed;
//! * the size of all files and their number are capped, so a small download cannot fill the disk ("zip bomb").
//!
//! Compression is detected from the first bytes: gzip (`archive.rs`), zstd (`ruzstd`), xz (`xz.rs`), or none.

use std::fs::{self, File};
use std::io::{BufReader, BufWriter, Read, Write};
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::{Component, Path, PathBuf};

/// What an unpacking may do.
#[derive(Clone, Debug)]
pub struct Policy {
    /// The most bytes of file contents.
    pub max_bytes: u64,
    pub max_files: usize,
    /// Added to every owner and group found in the archive (`lchown`), for a sandbox whose user namespace maps its ids to a host
    /// range. `None`: files stay owned by whoever unpacks.
    pub id_shift: Option<u32>,
    /// Container layers: `.wh.name` removes `name`, `.wh..wh..opq` empties its folder.
    pub whiteouts: bool,
}

impl Policy {
    pub fn base() -> Policy {
        Policy { max_bytes: 6 << 30, max_files: 600_000, id_shift: None, whiteouts: false }
    }
}

/// What was unpacked.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Report {
    pub files: usize,
    pub bytes: u64,
    pub dirs: usize,
    pub symlinks: usize,
    pub hardlinks: usize,
    /// Device nodes, pipes and sockets that were left out (names).
    pub skipped: Vec<String>,
}

fn bad(m: impl Into<String>) -> String {
    m.into()
}

/// A name from the archive as a relative path with only normal parts (a leading `./` is fine).
pub fn safe_rel(name: &str) -> Result<PathBuf, String> {
    if name.contains('\0') {
        return Err(bad("a name with a NUL in the archive"));
    }
    let mut out = PathBuf::new();
    for c in Path::new(name).components() {
        match c {
            Component::Normal(p) => out.push(p),
            Component::CurDir => {}
            _ => return Err(format!("the archive names a path outside its folder: {name}")),
        }
    }
    Ok(out)
}

/// Every folder between `dest` and the entry must be a real folder (or missing, then it is made): no link may be followed.
pub(crate) fn make_parents(dest: &Path, rel: &Path, id_shift: Option<u32>) -> Result<(), String> {
    let mut cur = dest.to_path_buf();
    let parent = rel.parent().unwrap_or(Path::new(""));
    for c in parent.components() {
        cur.push(c);
        match fs::symlink_metadata(&cur) {
            Ok(m) if m.file_type().is_symlink() => return Err(format!("the archive writes through a link ({})", rel.display())),
            Ok(m) if m.is_dir() => {}
            Ok(_) => return Err(format!("the archive uses a file as a folder ({})", rel.display())),
            Err(_) => {
                fs::create_dir(&cur).map_err(|e| format!("{}: {e}", cur.display()))?;
                let _ = fs::set_permissions(&cur, fs::Permissions::from_mode(0o755));
                chown(&cur, 0, 0, id_shift)?;
            }
        }
    }
    Ok(())
}

fn chown(p: &Path, uid: u32, gid: u32, shift: Option<u32>) -> Result<(), String> {
    let Some(base) = shift else { return Ok(()) };
    let c = std::ffi::CString::new(p.as_os_str().as_bytes()).map_err(|_| bad("a name with a NUL"))?;
    // SAFETY: `c` is a valid NUL-terminated path; lchown does not follow a link.
    let r = unsafe { libc::lchown(c.as_ptr(), base.saturating_add(uid), base.saturating_add(gid)) };
    if r != 0 {
        return Err(format!("{}: could not set the owner ({})", p.display(), std::io::Error::last_os_error()));
    }
    Ok(())
}

/// The tar number fields: octal text, or base-256 when the top bit of the first byte is set.
fn num(b: &[u8]) -> u64 {
    if b.first().is_some_and(|c| c & 0x80 != 0) {
        let mut v = (b[0] & 0x7f) as u64;
        for c in &b[1..] {
            v = v.checked_mul(256).unwrap_or(u64::MAX) | *c as u64;
        }
        return v;
    }
    let s: String = b.iter().take_while(|c| **c != 0).map(|c| *c as char).collect();
    u64::from_str_radix(s.trim(), 8).unwrap_or(0)
}

fn cstr(b: &[u8]) -> String {
    String::from_utf8_lossy(&b[..b.iter().position(|c| *c == 0).unwrap_or(b.len())]).into_owned()
}

/// pax extended header records: `<len> <key>=<value>\n`.
fn pax(data: &[u8]) -> Vec<(String, String)> {
    let (mut out, mut i) = (Vec::new(), 0);
    while i < data.len() {
        let Some(sp) = data[i..].iter().position(|c| *c == b' ') else { break };
        let Ok(len) = String::from_utf8_lossy(&data[i..i + sp]).parse::<usize>() else { break };
        if len == 0 || i + len > data.len() || len <= sp + 1 {
            break;
        }
        let rec = String::from_utf8_lossy(&data[i + sp + 1..i + len - 1]).into_owned();
        if let Some((k, v)) = rec.split_once('=') {
            out.push((k.to_string(), v.to_string()));
        }
        i += len;
    }
    out
}

/// Unpack a tar stream (not compressed) into `dest`. `progress(files, bytes)` is called now and then.
pub fn untar<R: Read>(mut f: R, dest: &Path, pol: &Policy, progress: &mut dyn FnMut(usize, u64)) -> Result<Report, String> {
    fs::create_dir_all(dest).map_err(|e| format!("{}: {e}", dest.display()))?;
    let mut rep = Report::default();
    let (mut long_name, mut long_link): (Option<String>, Option<String>) = (None, None);
    let (mut px_path, mut px_link, mut px_size, mut px_uid, mut px_gid): (Option<String>, Option<String>, Option<u64>, Option<u64>, Option<u64>) = (None, None, None, None, None);
    let mut block = [0u8; 512];
    loop {
        if f.read_exact(&mut block).is_err() || block.iter().all(|b| *b == 0) {
            break;
        }
        let mut size = num(&block[124..136]);
        let kind = block[156];
        let mode = (num(&block[100..108]) as u32) & 0o1777;
        let (mut uid, mut gid) = (num(&block[108..116]), num(&block[116..124]));
        let mut name = cstr(&block[0..100]);
        if &block[257..262] == b"ustar" {
            let prefix = cstr(&block[345..500]);
            if !prefix.is_empty() {
                name = format!("{prefix}/{name}");
            }
        }
        let mut link = cstr(&block[157..257]);
        let skip = |f: &mut R, n: u64| std::io::copy(&mut f.by_ref().take(n), &mut std::io::sink()).map(|_| ()).map_err(|e| e.to_string());
        if matches!(kind, b'L' | b'K' | b'x' | b'g') {
            if size > 1 << 20 {
                return Err(bad("an oversized header in the archive"));
            }
            let mut data = vec![0u8; size.div_ceil(512) as usize * 512];
            f.read_exact(&mut data).map_err(|e| e.to_string())?;
            data.truncate(size as usize);
            match kind {
                b'L' => long_name = Some(cstr(&data)),
                b'K' => long_link = Some(cstr(&data)),
                b'x' => {
                    for (k, v) in pax(&data) {
                        match k.as_str() {
                            "path" => px_path = Some(v),
                            "linkpath" => px_link = Some(v),
                            "size" => px_size = v.parse().ok(),
                            "uid" => px_uid = v.parse().ok(),
                            "gid" => px_gid = v.parse().ok(),
                            _ => {}
                        }
                    }
                }
                _ => {}
            }
            continue;
        }
        if let Some(n) = px_path.take().or_else(|| long_name.take()) {
            name = n;
        }
        long_name = None;
        if let Some(l) = px_link.take().or_else(|| long_link.take()) {
            link = l;
        }
        long_link = None;
        size = px_size.take().unwrap_or(size);
        uid = px_uid.take().unwrap_or(uid);
        gid = px_gid.take().unwrap_or(gid);
        let (uid, gid) = (uid.min(u32::MAX as u64 / 2) as u32, gid.min(u32::MAX as u64 / 2) as u32);
        let padded = size.div_ceil(512) * 512;
        let rel = safe_rel(name.trim_end_matches('/'))?;
        let leaf = rel.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        // layers: whiteouts delete what a lower layer made
        if pol.whiteouts && leaf.starts_with(".wh.") {
            skip(&mut f, padded)?;
            let parent = rel.parent().unwrap_or(Path::new("")).to_path_buf();
            make_parents(dest, &rel, None)?;
            if leaf == ".wh..wh..opq" {
                let dir = dest.join(&parent);
                if let Ok(rd) = fs::read_dir(&dir) {
                    for e in rd.flatten() {
                        remove_any(&e.path());
                    }
                }
            } else {
                remove_any(&dest.join(parent).join(&leaf[4..]));
            }
            continue;
        }
        if rel.as_os_str().is_empty() {
            // "./" itself
            skip(&mut f, padded)?;
            continue;
        }
        let path = dest.join(&rel);
        match kind {
            b'0' | 0 | b'7' => {
                rep.files += 1;
                rep.bytes = rep.bytes.saturating_add(size);
                if rep.files > pol.max_files || rep.bytes > pol.max_bytes {
                    return Err(bad("the archive is larger than allowed (too many files or too many bytes)"));
                }
                make_parents(dest, &rel, pol.id_shift)?;
                remove_any_file(&path)?;
                let mut out = fs::OpenOptions::new().write(true).create_new(true).custom_flags(libc::O_NOFOLLOW).mode(0o600).open(&path).map_err(|e| format!("{}: {e}", path.display()))?;
                let n = std::io::copy(&mut (&mut f).take(size), &mut BufWriter::new(&mut out)).map_err(|e| e.to_string())?;
                if n != size {
                    return Err(bad("the archive ends early"));
                }
                out.flush().map_err(|e| e.to_string())?;
                let _ = fs::set_permissions(&path, fs::Permissions::from_mode(mode & 0o1777 & !0o6000 | 0o200));
                chown(&path, uid, gid, pol.id_shift)?;
                skip(&mut f, padded - size)?;
                if rep.files % 2000 == 0 {
                    progress(rep.files, rep.bytes);
                }
            }
            b'5' => {
                make_parents(dest, &rel, pol.id_shift)?;
                match fs::symlink_metadata(&path) {
                    Ok(m) if m.is_dir() && !m.file_type().is_symlink() => {}
                    Ok(_) => {
                        remove_any(&path);
                        fs::create_dir(&path).map_err(|e| format!("{}: {e}", path.display()))?;
                    }
                    Err(_) => fs::create_dir(&path).map_err(|e| format!("{}: {e}", path.display()))?,
                }
                let _ = fs::set_permissions(&path, fs::Permissions::from_mode(mode | 0o700));
                chown(&path, uid, gid, pol.id_shift)?;
                rep.dirs += 1;
                skip(&mut f, padded)?;
            }
            b'2' => {
                if link.is_empty() || link.len() > 4096 || link.contains('\0') {
                    return Err(format!("a link without a usable target ({})", rel.display()));
                }
                make_parents(dest, &rel, pol.id_shift)?;
                remove_any(&path);
                std::os::unix::fs::symlink(&link, &path).map_err(|e| format!("{}: {e}", path.display()))?;
                chown(&path, uid, gid, pol.id_shift)?;
                rep.symlinks += 1;
                skip(&mut f, padded)?;
            }
            b'1' => {
                let target_rel = safe_rel(&link).map_err(|_| format!("a hard link out of the archive's folder ({} -> {link})", rel.display()))?;
                if target_rel.as_os_str().is_empty() {
                    return Err(format!("a hard link without a target ({})", rel.display()));
                }
                make_parents(dest, &target_rel, None).map_err(|_| format!("a hard link through a link ({} -> {link})", rel.display()))?;
                let target = dest.join(&target_rel);
                let meta = fs::symlink_metadata(&target).map_err(|_| format!("a hard link to a file that is not in the archive ({} -> {link})", rel.display()))?;
                if !meta.is_file() {
                    return Err(format!("a hard link to something that is not a plain file ({} -> {link})", rel.display()));
                }
                make_parents(dest, &rel, pol.id_shift)?;
                remove_any_file(&path)?;
                fs::hard_link(&target, &path).map_err(|e| format!("{}: {e}", path.display()))?;
                rep.hardlinks += 1;
                skip(&mut f, padded)?;
            }
            b'3' | b'4' | b'6' => {
                rep.skipped.push(format!("{} ({})", rel.display(), match kind { b'3' => "character device", b'4' => "block device", _ => "pipe" }));
                skip(&mut f, padded)?;
            }
            _ => skip(&mut f, padded)?,
        }
    }
    progress(rep.files, rep.bytes);
    Ok(rep)
}

/// Remove whatever is at `p` without following a link.
fn remove_any(p: &Path) {
    match fs::symlink_metadata(p) {
        Ok(m) if m.is_dir() && !m.file_type().is_symlink() => {
            let _ = fs::remove_dir_all(p);
        }
        Ok(_) => {
            let _ = fs::remove_file(p);
        }
        Err(_) => {}
    }
}

/// Make room for a file: an older file or link of that name goes; a folder is an error (a later layer cannot make a folder a file by accident).
fn remove_any_file(p: &Path) -> Result<(), String> {
    match fs::symlink_metadata(p) {
        Ok(m) if m.is_dir() && !m.file_type().is_symlink() => Err(format!("{}: a folder is in the way of a file", p.display())),
        Ok(_) => fs::remove_file(p).map_err(|e| format!("{}: {e}", p.display())),
        Err(_) => Ok(()),
    }
}

/// How an archive is compressed, from its first bytes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Compression {
    None,
    Gzip,
    Zstd,
    Xz,
}

pub fn detect(head: &[u8]) -> Compression {
    if head.starts_with(&[0x1f, 0x8b]) {
        Compression::Gzip
    } else if head.starts_with(&[0x28, 0xb5, 0x2f, 0xfd]) {
        Compression::Zstd
    } else if head.starts_with(&[0xfd, b'7', b'z', b'X', b'Z', 0]) {
        Compression::Xz
    } else {
        Compression::None
    }
}

/// Unpack a (compressed) tar file into `dest`. `scratch` is a folder for the uncompressed tar of a gzip or xz file (removed after).
pub fn unpack(src: &Path, dest: &Path, scratch: &Path, pol: &Policy, progress: &mut dyn FnMut(usize, u64)) -> Result<Report, String> {
    let mut head = [0u8; 8];
    let n = File::open(src).and_then(|mut f| f.read(&mut head)).map_err(|e| format!("{}: {e}", src.display()))?;
    let open = || File::open(src).map(BufReader::new).map_err(|e| e.to_string());
    match detect(&head[..n]) {
        Compression::None => untar(open()?, dest, pol, progress),
        Compression::Zstd => {
            let dec = ruzstd::decoding::StreamingDecoder::new(open()?).map_err(|e| format!("not a zstd file ({e})"))?;
            untar(LimitRead { inner: dec, left: pol.max_bytes.saturating_add(pol.max_bytes / 8 + (64 << 20)) }, dest, pol, progress)
        }
        c @ (Compression::Gzip | Compression::Xz) => {
            fs::create_dir_all(scratch).map_err(|e| e.to_string())?;
            let tar = scratch.join("rootfs.tar.partial");
            let limit = pol.max_bytes.saturating_add(pol.max_bytes / 8 + (64 << 20));
            let r = (|| {
                if c == Compression::Gzip {
                    crate::archive::gunzip_file(src, &tar, limit)?;
                } else {
                    let data = fs::read(src).map_err(|e| e.to_string())?;
                    let mut w = BufWriter::new(File::create(&tar).map_err(|e| e.to_string())?);
                    crate::xz::decompress(&data, &mut w, limit)?;
                    w.flush().map_err(|e| e.to_string())?;
                }
                untar(BufReader::new(File::open(&tar).map_err(|e| e.to_string())?), dest, pol, progress)
            })();
            let _ = fs::remove_file(&tar);
            r
        }
    }
}

/// A reader that stops with an error after `left` bytes (a zstd stream that expands without end).
struct LimitRead<R> {
    inner: R,
    left: u64,
}

impl<R: Read> Read for LimitRead<R> {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        if self.left == 0 {
            return Err(std::io::Error::other("the archive expands to more than its limit"));
        }
        let take = (buf.len() as u64).min(self.left) as usize;
        let n = self.inner.read(&mut buf[..take])?;
        self.left -= n as u64;
        Ok(n)
    }
}

/// Bytes of the files under `p` (links not followed).
pub fn tree_size(p: &Path) -> u64 {
    let mut total = 0;
    let mut stack = vec![p.to_path_buf()];
    while let Some(d) = stack.pop() {
        for e in fs::read_dir(&d).into_iter().flatten().flatten() {
            let Ok(m) = fs::symlink_metadata(e.path()) else { continue };
            if m.is_dir() {
                stack.push(e.path());
            } else {
                total += m.len();
            }
        }
    }
    total
}

/// Write a file inside an unpacked root file system without following any link: every folder on the way must be a real folder (made
/// when missing) and an older file or link of that name is replaced. `owner` is the id inside the sandbox; `shift` the host offset.
pub fn write_file_in(root: &Path, rel: &str, data: &[u8], mode: u32, owner: (u32, u32), shift: Option<u32>) -> Result<(), String> {
    let rel = safe_rel(rel)?;
    if rel.as_os_str().is_empty() {
        return Err(bad("an empty name"));
    }
    make_parents(root, &rel, shift)?;
    let path = root.join(&rel);
    remove_any_file(&path)?;
    let mut f = fs::OpenOptions::new().write(true).create_new(true).custom_flags(libc::O_NOFOLLOW).mode(0o600).open(&path).map_err(|e| format!("{}: {e}", path.display()))?;
    f.write_all(data).map_err(|e| e.to_string())?;
    drop(f);
    let _ = fs::set_permissions(&path, fs::Permissions::from_mode(mode & 0o777));
    chown(&path, owner.0, owner.1, shift)
}

/// Make a folder inside an unpacked root file system (no link followed), owned by `owner` inside.
pub fn make_dir_in(root: &Path, rel: &str, mode: u32, owner: (u32, u32), shift: Option<u32>) -> Result<(), String> {
    let rel = safe_rel(rel)?;
    if rel.as_os_str().is_empty() {
        return Ok(());
    }
    make_parents(root, &rel, shift)?;
    let path = root.join(&rel);
    match fs::symlink_metadata(&path) {
        Ok(m) if m.is_dir() && !m.file_type().is_symlink() => {}
        Ok(_) => return Err(format!("{} exists and is not a folder", rel.display())),
        Err(_) => fs::create_dir(&path).map_err(|e| format!("{}: {e}", path.display()))?,
    }
    let _ = fs::set_permissions(&path, fs::Permissions::from_mode(mode & 0o7777));
    chown(&path, owner.0, owner.1, shift)
}

/// Read a file inside an unpacked root file system, only if no link is on the way (None otherwise or when missing).
pub fn read_file_in(root: &Path, rel: &str) -> Option<Vec<u8>> {
    let rel = safe_rel(rel).ok()?;
    let mut cur = root.to_path_buf();
    for c in rel.components() {
        cur.push(c);
        let m = fs::symlink_metadata(&cur).ok()?;
        if m.file_type().is_symlink() {
            return None;
        }
    }
    fs::read(cur).ok()
}

/// Delete a sandbox's folder tree even when its modes forbid it (folders made read-only by packages); links are not followed.
pub fn remove_tree(p: &Path) -> Result<(), String> {
    let mut stack = vec![p.to_path_buf()];
    while let Some(d) = stack.pop() {
        if let Ok(m) = fs::symlink_metadata(&d) {
            if m.is_dir() && !m.file_type().is_symlink() {
                let _ = fs::set_permissions(&d, fs::Permissions::from_mode(0o700));
                for e in fs::read_dir(&d).into_iter().flatten().flatten() {
                    stack.push(e.path());
                }
            }
        }
    }
    match fs::remove_dir_all(p) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(format!("{}: {e}", p.display())),
    }
}
