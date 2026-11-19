//! A reader for the file list of a `newc` cpio archive (what Buildroot writes as `rootfs.cpio`), and a walker for an unpacked tree.
//! Reading only: nothing is extracted or written; file data is skipped.

use std::io::{self, Read};
use std::path::Path;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Kind {
    File,
    Dir,
    Symlink,
    Other,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    /// Without a leading `./` or `/`.
    pub path: String,
    pub kind: Kind,
    pub size: u64,
    pub mode: u32,
    /// Where a symbolic link points (empty for anything else).
    pub target: String,
}

pub fn norm(p: &str) -> String {
    p.trim_start_matches("./").trim_start_matches('/').to_string()
}

fn skip<R: Read>(r: &mut R, n: u64) -> io::Result<()> {
    let got = io::copy(&mut r.take(n), &mut io::sink())?;
    if got == n { Ok(()) } else { Err(io::Error::new(io::ErrorKind::UnexpectedEof, "archive ends inside a file")) }
}

/// List a `newc` (`070701`) or `crc` (`070702`) archive from any reader (a file, or `zstd -dc rootfs.cpio.zst |` on stdin).
pub fn list<R: Read>(mut r: R) -> io::Result<Vec<Entry>> {
    let mut out = Vec::new();
    loop {
        let mut h = [0u8; 110];
        r.read_exact(&mut h)?;
        if h[0..4] == [0x28, 0xb5, 0x2f, 0xfd] {
            return Err(io::Error::new(io::ErrorKind::InvalidData, "this is zstd-compressed; pipe it through `zstd -dc` and pass `-`"));
        }
        if &h[0..6] != b"070701" && &h[0..6] != b"070702" {
            return Err(io::Error::new(io::ErrorKind::InvalidData, "not a newc cpio archive"));
        }
        let field = |i: usize| -> io::Result<u64> {
            let s = std::str::from_utf8(&h[6 + i * 8..14 + i * 8]).map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "bad header"))?;
            u64::from_str_radix(s, 16).map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "bad header number"))
        };
        let (mode, size, namesize) = (field(1)? as u32, field(6)?, field(11)?);
        let mut name = vec![0u8; namesize as usize];
        r.read_exact(&mut name)?;
        let name = String::from_utf8_lossy(&name[..name.len().saturating_sub(1)]).to_string();
        skip(&mut r, (4 - (110 + namesize) % 4) % 4)?;
        if name == "TRAILER!!!" {
            return Ok(out);
        }
        let kind = match mode & 0o170000 {
            0o100000 => Kind::File,
            0o040000 => Kind::Dir,
            0o120000 => Kind::Symlink,
            _ => Kind::Other,
        };
        let mut target = String::new();
        if kind == Kind::Symlink && size <= 4096 {
            let mut t = vec![0u8; size as usize];
            r.read_exact(&mut t)?;
            target = String::from_utf8_lossy(&t).to_string();
        } else {
            skip(&mut r, size)?;
        }
        skip(&mut r, (4 - size % 4) % 4)?;
        let path = norm(&name);
        if !path.is_empty() && path != "." {
            out.push(Entry { path, kind, size, mode, target });
        }
    }
}

/// An unpacked tree (a Buildroot `target/` folder, the install image's `install-root`). Symbolic links are not followed.
pub fn walk(root: &Path) -> io::Result<Vec<Entry>> {
    fn rec(base: &Path, dir: &Path, out: &mut Vec<Entry>) -> io::Result<()> {
        let mut names: Vec<_> = std::fs::read_dir(dir)?.flatten().collect();
        names.sort_by_key(|e| e.file_name());
        for e in names {
            let md = e.metadata().or_else(|_| std::fs::symlink_metadata(e.path()))?;
            let sym = std::fs::symlink_metadata(e.path())?;
            let ft = sym.file_type();
            let rel = e.path().strip_prefix(base).unwrap_or(&e.path()).to_string_lossy().to_string();
            let kind = if ft.is_symlink() { Kind::Symlink } else if ft.is_dir() { Kind::Dir } else if ft.is_file() { Kind::File } else { Kind::Other };
            let target = if kind == Kind::Symlink { std::fs::read_link(e.path()).map(|t| t.to_string_lossy().to_string()).unwrap_or_default() } else { String::new() };
            out.push(Entry { path: rel, kind: kind.clone(), size: md.len(), mode: 0, target });
            if kind == Kind::Dir {
                rec(base, &e.path(), out)?;
            }
        }
        Ok(())
    }
    let mut out = Vec::new();
    rec(root, root, &mut out)?;
    Ok(out)
}

/// A names-only list (the output of `cpio -t` or `find`), every line a regular file.
pub fn from_names(text: &str) -> Vec<Entry> {
    text.lines().map(norm).filter(|p| !p.is_empty() && p != ".").map(|path| Entry { path, kind: Kind::File, size: 0, mode: 0, target: String::new() }).collect()
}

#[cfg(test)]
pub fn build_newc(entries: &[(&str, u32, &[u8])]) -> Vec<u8> {
    let mut o = Vec::new();
    let mut add = |name: &str, mode: u32, data: &[u8]| {
        o.extend(format!("070701{:08x}{:08x}{:08x}{:08x}{:08x}{:08x}{:08x}{:08x}{:08x}{:08x}{:08x}{:08x}{:08x}", 1, mode, 0, 0, 1, 0, data.len(), 0, 0, 0, 0, name.len() + 1, 0).bytes());
        o.extend(name.bytes());
        o.push(0);
        while o.len() % 4 != 0 {
            o.push(0);
        }
        o.extend(data);
        while o.len() % 4 != 0 {
            o.push(0);
        }
    };
    for (n, m, d) in entries {
        add(n, *m, d);
    }
    add("TRAILER!!!", 0, b"");
    o
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lists_a_newc_archive_without_extracting() {
        let a = build_newc(&[(".", 0o040755, b""), ("./usr", 0o040755, b""), ("./usr/bin/tool", 0o100755, b"abcde"), ("lib/libx.so", 0o120777, b"libx.so.1"), ("etc/x", 0o100644, b"")]);
        let l = list(&a[..]).unwrap();
        let v: Vec<_> = l.iter().map(|e| (e.path.as_str(), e.kind.clone(), e.size)).collect();
        assert_eq!(v, vec![("usr", Kind::Dir, 0), ("usr/bin/tool", Kind::File, 5), ("lib/libx.so", Kind::Symlink, 9), ("etc/x", Kind::File, 0)]);
        assert_eq!(l[2].target, "libx.so.1");
        assert!(list(&b"not a cpio archive at all, nothing like it, padding padding padding padding padding padding padding padding"[..]).is_err());
        let mut z = vec![0x28, 0xb5, 0x2f, 0xfd];
        z.resize(200, 0);
        assert!(list(&z[..]).unwrap_err().to_string().contains("zstd"));
    }
}
