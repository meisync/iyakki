//! Unpacking the archives that command-line tools are shipped in: `.tar.gz` and `.zip`. Written here (with the
//! deflate decompressor from `miniz_oxide`) so that nothing has to be run to unpack a download. Everything is streamed
//! through files, so a 150 MB tool does not need 600 MB of memory, and every name and link is checked so an archive
//! cannot write outside the folder it is unpacked into.

use std::fs::File;
use std::io::{BufReader, BufWriter, Read, Seek, SeekFrom, Write};
use std::path::{Component, Path, PathBuf};

use miniz_oxide::inflate::stream::{inflate, InflateState};
use miniz_oxide::{DataFormat, MZFlush, MZStatus};

pub const MAX_TOTAL: u64 = 2 << 30;
pub const MAX_FILES: usize = 40_000;

/// What was unpacked: how many files and bytes.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Unpacked {
    pub files: usize,
    pub bytes: u64,
}

fn err(m: impl Into<String>) -> String {
    m.into()
}

/// Inflates raw deflate data from `input` into `out`, never writing more than `limit` bytes.
fn inflate_stream(input: &mut dyn Read, out: &mut dyn Write, limit: u64) -> Result<u64, String> {
    let mut state = InflateState::new_boxed(DataFormat::Raw);
    let mut inbuf = vec![0u8; 64 * 1024];
    let mut outbuf = vec![0u8; 256 * 1024];
    let (mut have, mut pos) = (0usize, 0usize);
    let mut total = 0u64;
    loop {
        if pos == have {
            have = input.read(&mut inbuf).map_err(|e| e.to_string())?;
            pos = 0;
        }
        let flush = if have == 0 { MZFlush::Finish } else { MZFlush::None };
        let r = inflate(&mut state, &inbuf[pos..have], &mut outbuf, flush);
        pos += r.bytes_consumed;
        if r.bytes_written > 0 {
            total += r.bytes_written as u64;
            if total > limit {
                return Err(err("the archive expands to more than its limit"));
            }
            out.write_all(&outbuf[..r.bytes_written]).map_err(|e| e.to_string())?;
        }
        match r.status {
            Ok(MZStatus::StreamEnd) => return Ok(total),
            Ok(_) => {
                if r.bytes_consumed == 0 && r.bytes_written == 0 && have == 0 {
                    return Err(err("the compressed data ends early"));
                }
            }
            Err(miniz_oxide::MZError::Buf) if have != 0 => {}
            Err(e) => return Err(format!("the compressed data is damaged ({e:?})")),
        }
    }
}

/// `src` (a .gz file) → `dst` (the decompressed bytes).
pub fn gunzip_file(src: &Path, dst: &Path, limit: u64) -> Result<u64, String> {
    let mut f = BufReader::new(File::open(src).map_err(|e| e.to_string())?);
    let mut h = [0u8; 10];
    f.read_exact(&mut h).map_err(|_| err("not a gzip file"))?;
    if h[0] != 0x1f || h[1] != 0x8b || h[2] != 8 {
        return Err(err("not a gzip file"));
    }
    let flags = h[3];
    if flags & 4 != 0 {
        let mut l = [0u8; 2];
        f.read_exact(&mut l).map_err(|e| e.to_string())?;
        let mut skip = vec![0u8; u16::from_le_bytes(l) as usize];
        f.read_exact(&mut skip).map_err(|e| e.to_string())?;
    }
    for bit in [8u8, 16] {
        if flags & bit != 0 {
            let mut b = [0u8; 1];
            loop {
                f.read_exact(&mut b).map_err(|e| e.to_string())?;
                if b[0] == 0 {
                    break;
                }
            }
        }
    }
    if flags & 2 != 0 {
        let mut c = [0u8; 2];
        f.read_exact(&mut c).map_err(|e| e.to_string())?;
    }
    let mut out = BufWriter::new(File::create(dst).map_err(|e| e.to_string())?);
    let n = inflate_stream(&mut f, &mut out, limit)?;
    out.flush().map_err(|e| e.to_string())?;
    Ok(n)
}

/// A path from an archive, made safe to join onto the destination: relative, with no `..`.
fn safe_rel(name: &str) -> Result<PathBuf, String> {
    let mut out = PathBuf::new();
    for c in Path::new(name).components() {
        match c {
            Component::Normal(p) => out.push(p),
            Component::CurDir => {}
            _ => return Err(format!("the archive names a path outside its folder: {name}")),
        }
    }
    if out.as_os_str().is_empty() { Err(err("an empty name in the archive")) } else { Ok(out) }
}

/// A link target is only allowed if, followed from where the link sits, it stays inside the destination.
fn safe_link(link_rel: &Path, target: &str) -> Result<(), String> {
    if target.starts_with('/') {
        return Err(format!("the archive has a link to an absolute path ({target})"));
    }
    let mut depth = link_rel.components().count() as i64 - 1;
    for c in Path::new(target).components() {
        match c {
            Component::ParentDir => depth -= 1,
            Component::Normal(_) => depth += 1,
            Component::CurDir => {}
            _ => return Err(format!("the archive has a link out of its folder ({target})")),
        }
        if depth < 0 {
            return Err(format!("the archive has a link out of its folder ({target})"));
        }
    }
    Ok(())
}

fn make_dirs(p: &Path) -> Result<(), String> {
    std::fs::create_dir_all(p).map_err(|e| e.to_string())
}

fn write_file(path: &Path, mode: u32, data: &mut dyn Read, size: u64) -> Result<(), String> {
    use std::os::unix::fs::OpenOptionsExt;
    if let Some(p) = path.parent() {
        make_dirs(p)?;
    }
    let _ = std::fs::remove_file(path);
    let mut f = std::fs::OpenOptions::new().write(true).create_new(true).mode(mode & 0o777 | 0o600).open(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let n = std::io::copy(&mut data.take(size), &mut f).map_err(|e| e.to_string())?;
    if n != size {
        return Err(err("the archive ends early"));
    }
    Ok(())
}

fn make_link(dest: &Path, rel: &Path, target: &str) -> Result<(), String> {
    safe_link(rel, target)?;
    let path = dest.join(rel);
    if let Some(p) = path.parent() {
        make_dirs(p)?;
    }
    let _ = std::fs::remove_file(&path);
    std::os::unix::fs::symlink(target, &path).map_err(|e| format!("{}: {e}", path.display()))
}

fn octal(b: &[u8]) -> u64 {
    let s: String = b.iter().take_while(|c| **c != 0).map(|c| *c as char).collect();
    u64::from_str_radix(s.trim(), 8).unwrap_or(0)
}

fn cstr(b: &[u8]) -> String {
    String::from_utf8_lossy(&b[..b.iter().position(|c| *c == 0).unwrap_or(b.len())]).into_owned()
}

/// Unpacks a tar file (not compressed) into `dest`. Plain files, folders and links are kept; anything else is skipped.
pub fn untar_file(tar: &Path, dest: &Path) -> Result<Unpacked, String> {
    untar_stream(BufReader::new(File::open(tar).map_err(|e| e.to_string())?), dest, MAX_TOTAL)
}

/// Unpacks a tar stream (not compressed), reading it straight through; never more than `max_total` bytes of files.
pub fn untar_stream<R: Read>(mut f: R, dest: &Path, max_total: u64) -> Result<Unpacked, String> {
    make_dirs(dest)?;
    let mut un = Unpacked::default();
    let (mut long_name, mut pax_path): (Option<String>, Option<String>) = (None, None);
    let mut block = [0u8; 512];
    loop {
        if f.read_exact(&mut block).is_err() || block.iter().all(|b| *b == 0) {
            break;
        }
        let size = octal(&block[124..136]);
        let kind = block[156];
        let mode = octal(&block[100..108]) as u32;
        let mut name = cstr(&block[0..100]);
        if &block[257..262] == b"ustar" {
            let prefix = cstr(&block[345..500]);
            if !prefix.is_empty() {
                name = format!("{prefix}/{name}");
            }
        }
        let padded = size.div_ceil(512) * 512;
        let skip = |f: &mut R, n: u64| std::io::copy(&mut f.by_ref().take(n), &mut std::io::sink()).map(|_| ()).map_err(|e| e.to_string());
        match kind {
            b'L' | b'x' | b'g' => {
                if size > 1 << 20 {
                    return Err(err("an oversized header in the archive"));
                }
                let mut data = vec![0u8; padded as usize];
                f.read_exact(&mut data).map_err(|e| e.to_string())?;
                let text = String::from_utf8_lossy(&data[..size as usize]).into_owned();
                if kind == b'L' {
                    long_name = Some(text.trim_end_matches('\0').to_string());
                } else if kind == b'x' {
                    pax_path = text.lines().find_map(|l| l.split_once(' ').and_then(|(_, kv)| kv.strip_prefix("path=")).map(str::to_string)).or(pax_path);
                }
                continue;
            }
            _ => {}
        }
        if let Some(n) = long_name.take().or_else(|| pax_path.take()) {
            name = n;
        }
        pax_path = None;
        match kind {
            b'0' | 0 | b'7' => {
                let rel = safe_rel(&name)?;
                un.files += 1;
                un.bytes += size;
                if un.files > MAX_FILES || un.bytes > max_total {
                    return Err(err("the archive is larger than allowed"));
                }
                write_file(&dest.join(&rel), if mode == 0 { 0o644 } else { mode }, &mut f, size)?;
                skip(&mut f, padded - size)?;
            }
            b'5' => {
                if let Ok(rel) = safe_rel(name.trim_end_matches('/')) {
                    make_dirs(&dest.join(rel))?;
                }
                skip(&mut f, padded)?;
            }
            b'2' => {
                let rel = safe_rel(&name)?;
                let target = cstr(&block[157..257]);
                make_link(dest, &rel, &target)?;
                skip(&mut f, padded)?;
            }
            _ => skip(&mut f, padded)?,
        }
    }
    Ok(un)
}

/// Unpacks a `.tar.zst` into `dest` while decompressing it (no temporary tar file; the download can be several gigabytes).
pub fn untar_zst(src: &Path, dest: &Path, max_total: u64) -> Result<Unpacked, String> {
    let f = BufReader::new(File::open(src).map_err(|e| e.to_string())?);
    let dec = ruzstd::decoding::StreamingDecoder::new(f).map_err(|e| format!("not a zstd file ({e})"))?;
    untar_stream(dec, dest, max_total)
}

/// Unpacks a `.tar.gz` into `dest`, using `scratch` for the uncompressed tar (removed afterwards).
pub fn untar_gz(src: &Path, dest: &Path, scratch: &Path) -> Result<Unpacked, String> {
    let tar = scratch.join("archive.tar.partial");
    let r = gunzip_file(src, &tar, MAX_TOTAL + (64 << 20)).and_then(|_| untar_file(&tar, dest));
    let _ = std::fs::remove_file(&tar);
    r
}

fn le16(b: &[u8], i: usize) -> usize {
    u16::from_le_bytes([b[i], b[i + 1]]) as usize
}
fn le32(b: &[u8], i: usize) -> u64 {
    u32::from_le_bytes([b[i], b[i + 1], b[i + 2], b[i + 3]]) as u64
}

/// Unpacks a `.zip` into `dest`.
pub fn unzip(src: &Path, dest: &Path) -> Result<Unpacked, String> {
    let mut f = File::open(src).map_err(|e| e.to_string())?;
    let len = f.metadata().map_err(|e| e.to_string())?.len();
    // the end-of-central-directory record is in the last 64 KiB
    let tail_len = len.min(70_000);
    f.seek(SeekFrom::Start(len - tail_len)).map_err(|e| e.to_string())?;
    let mut tail = vec![0u8; tail_len as usize];
    f.read_exact(&mut tail).map_err(|e| e.to_string())?;
    let eocd = (0..tail.len().saturating_sub(21)).rev().find(|&i| tail[i..i + 4] == [0x50, 0x4b, 0x05, 0x06]).ok_or_else(|| err("not a zip file"))?;
    let (count, cd_size, cd_off) = (le16(&tail, eocd + 10), le32(&tail, eocd + 12), le32(&tail, eocd + 16));
    if cd_off == 0xFFFF_FFFF || count == 0xFFFF {
        return Err(err("zip64 archives are not supported"));
    }
    if cd_size > 64 << 20 || count > MAX_FILES {
        return Err(err("the archive is larger than allowed"));
    }
    f.seek(SeekFrom::Start(cd_off)).map_err(|e| e.to_string())?;
    let mut cd = vec![0u8; cd_size as usize];
    f.read_exact(&mut cd).map_err(|_| err("the zip directory is damaged"))?;
    make_dirs(dest)?;
    let mut un = Unpacked::default();
    let mut p = 0usize;
    for _ in 0..count {
        if p + 46 > cd.len() || cd[p..p + 4] != [0x50, 0x4b, 0x01, 0x02] {
            return Err(err("the zip directory is damaged"));
        }
        let (method, csize, usize_, nlen, xlen, clen, attr, off) = (le16(&cd, p + 10), le32(&cd, p + 20), le32(&cd, p + 24), le16(&cd, p + 28), le16(&cd, p + 30), le16(&cd, p + 32), le32(&cd, p + 38), le32(&cd, p + 42));
        let name = String::from_utf8_lossy(cd.get(p + 46..p + 46 + nlen).ok_or_else(|| err("the zip directory is damaged"))?).into_owned();
        p += 46 + nlen + xlen + clen;
        let unix_mode = (attr >> 16) as u32;
        if name.ends_with('/') {
            if let Ok(rel) = safe_rel(name.trim_end_matches('/')) {
                make_dirs(&dest.join(rel))?;
            }
            continue;
        }
        let rel = safe_rel(&name)?;
        un.files += 1;
        un.bytes += usize_;
        if un.bytes > MAX_TOTAL {
            return Err(err("the archive is larger than allowed"));
        }
        // the local header gives where the data starts
        f.seek(SeekFrom::Start(off)).map_err(|e| e.to_string())?;
        let mut lh = [0u8; 30];
        f.read_exact(&mut lh).map_err(|_| err("the zip entry is damaged"))?;
        if lh[..4] != [0x50, 0x4b, 0x03, 0x04] {
            return Err(err("the zip entry is damaged"));
        }
        f.seek(SeekFrom::Current((le16(&lh, 26) + le16(&lh, 28)) as i64)).map_err(|e| e.to_string())?;
        let mut data = (&mut f).take(csize);
        let is_link = unix_mode & 0o170000 == 0o120000;
        if is_link {
            let mut t = Vec::new();
            if method == 0 {
                data.read_to_end(&mut t).map_err(|e| e.to_string())?;
            } else {
                inflate_stream(&mut data, &mut t, 4096)?;
            }
            make_link(dest, &rel, &String::from_utf8_lossy(&t))?;
            continue;
        }
        let path = dest.join(&rel);
        let mode = if unix_mode & 0o777 == 0 { 0o644 } else { unix_mode };
        match method {
            0 => write_file(&path, mode, &mut data, usize_)?,
            8 => {
                use std::os::unix::fs::OpenOptionsExt;
                if let Some(parent) = path.parent() {
                    make_dirs(parent)?;
                }
                let _ = std::fs::remove_file(&path);
                let mut out = BufWriter::new(std::fs::OpenOptions::new().write(true).create_new(true).mode(mode & 0o777 | 0o600).open(&path).map_err(|e| format!("{}: {e}", path.display()))?);
                let n = inflate_stream(&mut data, &mut out, usize_.max(1))?;
                out.flush().map_err(|e| e.to_string())?;
                if n != usize_ {
                    return Err(err("a zip entry has the wrong size"));
                }
            }
            m => return Err(format!("the zip uses a compression method ({m}) that is not supported")),
        }
    }
    Ok(un)
}

#[cfg(test)]
pub mod tests {
    use super::*;
    use miniz_oxide::deflate::compress_to_vec;

    pub fn tmp(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("iyakki-archive-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    fn tar_header(name: &str, size: usize, kind: u8, mode: u32, link: &str) -> [u8; 512] {
        let mut h = [0u8; 512];
        h[..name.len().min(100)].copy_from_slice(&name.as_bytes()[..name.len().min(100)]);
        h[100..108].copy_from_slice(format!("{mode:07o}\0").as_bytes());
        h[124..136].copy_from_slice(format!("{size:011o}\0").as_bytes());
        h[156] = kind;
        h[157..157 + link.len()].copy_from_slice(link.as_bytes());
        h[257..263].copy_from_slice(b"ustar\0");
        h
    }

    /// A tar of (name, kind, content-or-link, mode).
    pub fn make_tar(entries: &[(&str, u8, &str, u32)]) -> Vec<u8> {
        let mut t = Vec::new();
        for (name, kind, content, mode) in entries {
            let (size, link) = if *kind == b'2' { (0, *content) } else { (content.len(), "") };
            t.extend_from_slice(&tar_header(name, size, *kind, *mode, link));
            if *kind != b'2' && *kind != b'5' {
                t.extend_from_slice(content.as_bytes());
                t.extend(std::iter::repeat_n(0u8, size.div_ceil(512) * 512 - size));
            }
        }
        t.extend(std::iter::repeat_n(0u8, 1024));
        t
    }

    pub fn gzip(data: &[u8]) -> Vec<u8> {
        let mut g = vec![0x1f, 0x8b, 8, 0, 0, 0, 0, 0, 0, 3];
        g.extend(compress_to_vec(data, 6));
        g.extend_from_slice(&[0; 8]); // the checksum and length are not read
        g
    }

    /// A zip of (name, content, mode, deflate?).
    pub fn make_zip(entries: &[(&str, &str, u32, bool)]) -> Vec<u8> {
        let (mut body, mut cd) = (Vec::new(), Vec::new());
        for (name, content, mode, deflate) in entries {
            let off = body.len() as u32;
            let (method, data) = if *deflate { (8u16, compress_to_vec(content.as_bytes(), 6)) } else { (0u16, content.as_bytes().to_vec()) };
            body.extend_from_slice(&[0x50, 0x4b, 3, 4, 20, 0, 0, 0]);
            body.extend_from_slice(&method.to_le_bytes());
            body.extend_from_slice(&[0, 0, 0, 0, 0, 0, 0, 0]);
            body.extend_from_slice(&(data.len() as u32).to_le_bytes());
            body.extend_from_slice(&(content.len() as u32).to_le_bytes());
            body.extend_from_slice(&(name.len() as u16).to_le_bytes());
            body.extend_from_slice(&[0, 0]);
            body.extend_from_slice(name.as_bytes());
            body.extend_from_slice(&data);
            cd.extend_from_slice(&[0x50, 0x4b, 1, 2, 20, 3, 20, 0, 0, 0]);
            cd.extend_from_slice(&method.to_le_bytes());
            cd.extend_from_slice(&[0, 0, 0, 0, 0, 0, 0, 0]);
            cd.extend_from_slice(&(data.len() as u32).to_le_bytes());
            cd.extend_from_slice(&(content.len() as u32).to_le_bytes());
            cd.extend_from_slice(&(name.len() as u16).to_le_bytes());
            cd.extend_from_slice(&[0, 0, 0, 0, 0, 0, 0, 0]);
            cd.extend_from_slice(&(mode << 16).to_le_bytes());
            cd.extend_from_slice(&off.to_le_bytes());
            cd.extend_from_slice(name.as_bytes());
        }
        let cd_off = body.len() as u32;
        body.extend_from_slice(&cd);
        body.extend_from_slice(&[0x50, 0x4b, 5, 6, 0, 0, 0, 0]);
        body.extend_from_slice(&(entries.len() as u16).to_le_bytes());
        body.extend_from_slice(&(entries.len() as u16).to_le_bytes());
        body.extend_from_slice(&(cd.len() as u32).to_le_bytes());
        body.extend_from_slice(&cd_off.to_le_bytes());
        body.extend_from_slice(&[0, 0]);
        body
    }

    #[test]
    fn a_tar_gz_is_unpacked_with_folders_modes_and_safe_links() {
        use std::os::unix::fs::PermissionsExt;
        let d = tmp("targz");
        let big = "x".repeat(300_000);
        let tar = make_tar(&[("tool/", b'5', "", 0o755), ("tool/bin/tool", b'0', "#!/bin/sh\necho hi\n", 0o755), ("tool/README", b'0', "read me", 0o644), ("tool/big.dat", b'0', &big, 0o600), ("tool/bin/t", b'2', "tool", 0o777), ("./tool/dot", b'0', "dot", 0o644)]);
        let gz = d.join("a.tar.gz");
        std::fs::write(&gz, gzip(&tar)).unwrap();
        let out = d.join("out");
        let r = untar_gz(&gz, &out, &d).unwrap();
        assert_eq!(r.files, 4);
        assert_eq!(std::fs::read_to_string(out.join("tool/bin/tool")).unwrap(), "#!/bin/sh\necho hi\n");
        assert_eq!(std::fs::metadata(out.join("tool/bin/tool")).unwrap().permissions().mode() & 0o777, 0o755, "the program is still executable");
        assert_eq!(std::fs::read_to_string(out.join("tool/big.dat")).unwrap().len(), 300_000);
        assert_eq!(std::fs::read_link(out.join("tool/bin/t")).unwrap(), PathBuf::from("tool"));
        assert_eq!(std::fs::read_to_string(out.join("tool/dot")).unwrap(), "dot");
        assert!(!d.join("archive.tar.partial").exists(), "the scratch tar is removed");
    }

    #[test]
    fn archives_cannot_write_or_link_outside_their_folder() {
        let d = tmp("evil");
        for (i, bad) in [("../escape", b'0', "x", 0o644u32), ("/etc/escape", b'0', "x", 0o644), ("a/../../escape", b'0', "x", 0o644), ("l", b'2', "/etc/passwd", 0o777), ("sub/l", b'2', "../../etc/passwd", 0o777)].into_iter().enumerate() {
            let out = d.join(format!("out{i}"));
            let r = untar_file_from(&make_tar(&[bad]), &d, &out);
            assert!(r.is_err(), "{bad:?}");
        }
        assert!(!d.join("escape").exists());
        // a link that stays inside is fine
        assert!(untar_file_from(&make_tar(&[("a/b", b'0', "x", 0o644), ("a/l", b'2', "b", 0o777), ("a/m", b'2', "../a/b", 0o777)]), &d, &d.join("ok")).is_ok());
        // zip names too
        let z = d.join("bad.zip");
        std::fs::write(&z, make_zip(&[("../zz", "x", 0o644, false)])).unwrap();
        assert!(unzip(&z, &d.join("zout")).is_err());
        assert!(!d.parent().unwrap().join("zz").exists());
    }

    fn untar_file_from(tar: &[u8], scratch: &Path, out: &Path) -> Result<Unpacked, String> {
        let p = scratch.join("t.tar");
        std::fs::write(&p, tar).unwrap();
        untar_file(&p, out)
    }

    #[test]
    fn a_zip_is_unpacked_stored_or_deflated_with_modes_and_links_and_damage_is_reported() {
        use std::os::unix::fs::PermissionsExt;
        let d = tmp("zip");
        let big = "terraform ".repeat(50_000);
        let z = d.join("a.zip");
        std::fs::write(&z, make_zip(&[("terraform", &big, 0o100755, true), ("docs/readme.txt", "hello", 0o100644, false), ("aws/dist/aws", "bin", 0o100755, true), ("aws/dist/link", "aws", 0o120777, false), ("empty/", "", 0o40755, false)])).unwrap();
        let out = d.join("out");
        let r = unzip(&z, &out).unwrap();
        assert_eq!(r.files, 4);
        assert_eq!(std::fs::read_to_string(out.join("terraform")).unwrap(), big);
        assert_eq!(std::fs::metadata(out.join("terraform")).unwrap().permissions().mode() & 0o777, 0o755);
        assert_eq!(std::fs::read_to_string(out.join("docs/readme.txt")).unwrap(), "hello");
        assert_eq!(std::fs::read_link(out.join("aws/dist/link")).unwrap(), PathBuf::from("aws"));
        assert!(out.join("empty").is_dir());
        // not a zip, a cut-off zip, and a bad gzip
        std::fs::write(d.join("junk.zip"), b"nope nope nope nope nope nope").unwrap();
        assert!(unzip(&d.join("junk.zip"), &d.join("o2")).unwrap_err().contains("not a zip"));
        let bytes = std::fs::read(&z).unwrap();
        std::fs::write(d.join("cut.zip"), &bytes[..bytes.len() / 2]).unwrap();
        assert!(unzip(&d.join("cut.zip"), &d.join("o3")).is_err());
        std::fs::write(d.join("junk.gz"), b"not gzip at all").unwrap();
        assert!(untar_gz(&d.join("junk.gz"), &d.join("o4"), &d).is_err());
        let g = gzip(&make_tar(&[("a", b'0', &"y".repeat(100_000), 0o644)]));
        std::fs::write(d.join("cut.gz"), &g[..g.len() / 2]).unwrap();
        assert!(untar_gz(&d.join("cut.gz"), &d.join("o5"), &d).is_err());
    }

    #[test]
    fn a_decompression_bomb_is_stopped_at_its_limit() {
        let d = tmp("bomb");
        let gz = d.join("bomb.gz");
        std::fs::write(&gz, gzip(&vec![0u8; 5_000_000])).unwrap();
        assert!(gunzip_file(&gz, &d.join("out"), 1_000_000).unwrap_err().contains("more than its limit"));
        assert_eq!(gunzip_file(&gz, &d.join("out2"), 6_000_000).unwrap(), 5_000_000);
    }

    #[test]
    fn a_zstd_compressed_tar_is_unpacked_while_it_is_decompressed_and_checked_like_the_others() {
        let tar = make_tar(&[("tool/", b'5', "", 0o755), ("tool/bin/run", b'0', "#!/bin/sh\necho hi\n", 0o755), ("tool/lib/data.bin", b'0', &"x".repeat(100_000), 0o644), ("tool/link", b'2', "bin/run", 0o777)]);
        let z = ruzstd::encoding::compress_to_vec(&tar[..], ruzstd::encoding::CompressionLevel::Fastest);
        let d = tmp("zst");
        let src = d.join("a.tar.zst");
        std::fs::write(&src, &z).unwrap();
        let out = d.join("out");
        let un = untar_zst(&src, &out, MAX_TOTAL).unwrap();
        assert_eq!(un.files, 2);
        assert_eq!(std::fs::read_to_string(out.join("tool/bin/run")).unwrap(), "#!/bin/sh\necho hi\n");
        assert_eq!(std::fs::read(out.join("tool/lib/data.bin")).unwrap().len(), 100_000);
        assert_eq!(std::fs::read_link(out.join("tool/link")).unwrap(), std::path::PathBuf::from("bin/run"));
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(std::fs::metadata(out.join("tool/bin/run")).unwrap().permissions().mode() & 0o111, 0o111, "the program stays runnable");
        // the size limit applies to what comes out, not what went in
        assert!(untar_zst(&src, &d.join("small"), 1000).unwrap_err().contains("larger than allowed"));
        // paths that climb out are refused, and a file that is not zstd is explained
        let evil = ruzstd::encoding::compress_to_vec(&make_tar(&[("../escape", b'0', "x", 0o644)])[..], ruzstd::encoding::CompressionLevel::Fastest);
        std::fs::write(d.join("evil.tar.zst"), evil).unwrap();
        assert!(untar_zst(&d.join("evil.tar.zst"), &d.join("e"), MAX_TOTAL).is_err());
        assert!(!d.join("escape").exists());
        std::fs::write(d.join("plain.txt"), "hello").unwrap();
        assert!(untar_zst(&d.join("plain.txt"), &d.join("p"), MAX_TOTAL).unwrap_err().contains("not a zstd file"));
        // a real zstd program's output is read too, when this machine has one
        if let Ok(o) = std::process::Command::new("zstd").args(["-q", "-c"]).stdin(std::fs::File::open(d.join("a.tar")).or_else(|_| { std::fs::write(d.join("a.tar"), &tar).map(|_| ()).and_then(|_| std::fs::File::open(d.join("a.tar"))) }).unwrap()).output() {
            if o.status.success() {
                std::fs::write(d.join("real.tar.zst"), &o.stdout).unwrap();
                assert_eq!(untar_zst(&d.join("real.tar.zst"), &d.join("real"), MAX_TOTAL).unwrap().files, 2);
            }
        }
        let _ = std::fs::remove_dir_all(&d);
    }
}
