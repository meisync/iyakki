//! A zip writer (F-SHARE-07): entries are stored without compression (so the sizes are exact and nothing can be inflated into a
//! bomb on the other side), the sizes and checksum are patched into each local header after its data has been written, so a file is
//! read once and never held whole. Plain zip only: no zip64, so a file or the whole archive must stay under 4 GiB and there are at
//! most 65,534 entries; the writer says so instead of writing a damaged archive. Names are UTF-8 and may not leave the archive.

use std::io::{Read, Seek, SeekFrom, Write};

/// An incremental CRC-32 (the zip and gzip one).
pub struct Crc32(u32);

fn table() -> &'static [u32; 256] {
    static T: std::sync::OnceLock<[u32; 256]> = std::sync::OnceLock::new();
    T.get_or_init(|| {
        let mut t = [0u32; 256];
        for (i, e) in t.iter_mut().enumerate() {
            let mut c = i as u32;
            for _ in 0..8 {
                c = if c & 1 != 0 { 0xEDB8_8320 ^ (c >> 1) } else { c >> 1 };
            }
            *e = c;
        }
        t
    })
}

impl Default for Crc32 {
    fn default() -> Crc32 {
        Crc32::new()
    }
}

impl Crc32 {
    pub fn new() -> Crc32 {
        Crc32(0xFFFF_FFFF)
    }
    pub fn update(&mut self, data: &[u8]) {
        let t = table();
        for &b in data {
            self.0 = t[((self.0 ^ b as u32) & 0xff) as usize] ^ (self.0 >> 8);
        }
    }
    pub fn finish(&self) -> u32 {
        !self.0
    }
}

pub const MAX_ENTRIES: usize = 65_534;
pub const MAX_BYTES: u64 = 0xFFFF_FFFE;
const MAX_NAME: usize = 1000;

struct Central {
    name: String,
    crc: u32,
    size: u32,
    offset: u32,
    dos: (u16, u16),
    attr: u32,
}

pub struct ZipWriter<W: Write + Seek> {
    w: W,
    entries: Vec<Central>,
    pos: u64,
}

/// Days since 1970-01-01 to (year, month, day) in the proleptic Gregorian calendar.
fn civil(days: i64) -> (i64, u32, u32) {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (if m <= 2 { y + 1 } else { y }, m, d)
}

/// The MS-DOS (time, date) words of a unix time; before 1980 or after 2107 it is clamped.
pub fn dos_time(unix: i64) -> (u16, u16) {
    let unix = unix.clamp(315_532_800, 4_354_819_199);
    let (y, mo, d) = civil(unix.div_euclid(86_400));
    let s = unix.rem_euclid(86_400);
    let time = (((s / 3600) as u16) << 11) | ((((s % 3600) / 60) as u16) << 5) | ((s % 60 / 2) as u16);
    let date = (((y - 1980) as u16) << 9) | ((mo as u16) << 5) | d as u16;
    (time, date)
}

/// A name that stays inside the archive: relative, `/` separated, no `..`, no empty parts, no NUL, backslash or control characters.
pub fn valid_name(name: &str) -> Result<(), String> {
    if name.is_empty() || name.len() > MAX_NAME {
        return Err("a name in the zip is empty or too long".into());
    }
    if name.starts_with('/') || name.chars().any(|c| c == '\0' || c == '\\' || c.is_control()) {
        return Err(format!("{name:?} is not a name that can go in a zip"));
    }
    if name.trim_end_matches('/').split('/').any(|p| p.is_empty() || p == "." || p == "..") {
        return Err(format!("{name:?} leaves the folder"));
    }
    Ok(())
}

fn put16(v: &mut Vec<u8>, n: u16) {
    v.extend_from_slice(&n.to_le_bytes());
}
fn put32(v: &mut Vec<u8>, n: u32) {
    v.extend_from_slice(&n.to_le_bytes());
}

impl<W: Write + Seek> ZipWriter<W> {
    pub fn new(w: W) -> ZipWriter<W> {
        ZipWriter { w, entries: Vec::new(), pos: 0 }
    }

    pub fn entries(&self) -> usize {
        self.entries.len()
    }

    fn start(&mut self, name: &str) -> Result<u32, String> {
        valid_name(name)?;
        if self.entries.len() >= MAX_ENTRIES {
            return Err(format!("more than {MAX_ENTRIES} files cannot go in one zip"));
        }
        u32::try_from(self.pos).map_err(|_| "the zip would be larger than 4 GB".to_string())
    }

    /// A folder (`name` gets a trailing `/`).
    pub fn add_dir(&mut self, name: &str, mtime: i64) -> Result<(), String> {
        let name = format!("{}/", name.trim_end_matches('/'));
        let offset = self.start(&name)?;
        let dos = dos_time(mtime);
        let mut h = Vec::new();
        local_header(&mut h, &name, dos, 0, 0);
        self.w.write_all(&h).map_err(|e| e.to_string())?;
        self.pos += h.len() as u64;
        self.entries.push(Central { name, crc: 0, size: 0, offset, dos, attr: (0o040_755 << 16) | 0x10 });
        Ok(())
    }

    /// A file read from `src`; at most `max` bytes (more is an error and the archive must be thrown away). Returns the size.
    pub fn add_file(&mut self, name: &str, mtime: i64, mode: u32, src: &mut dyn Read, max: u64) -> Result<u64, String> {
        let offset = self.start(name)?;
        let dos = dos_time(mtime);
        let mut h = Vec::new();
        local_header(&mut h, name, dos, 0, 0);
        self.w.write_all(&h).map_err(|e| e.to_string())?;
        let data_at = self.pos + h.len() as u64;
        let mut crc = Crc32::new();
        let mut total: u64 = 0;
        let mut buf = vec![0u8; 64 * 1024];
        let limit = max.min(MAX_BYTES);
        loop {
            let n = src.read(&mut buf).map_err(|e| format!("{name}: {e}"))?;
            if n == 0 {
                break;
            }
            total += n as u64;
            if total > limit || data_at + total > MAX_BYTES {
                return Err(format!("{name} is too large to put in this zip"));
            }
            crc.update(&buf[..n]);
            self.w.write_all(&buf[..n]).map_err(|e| e.to_string())?;
        }
        let end = data_at + total;
        // patch crc, compressed size and size into the local header (offset 14), then go back to the end
        let mut patch = Vec::new();
        put32(&mut patch, crc.finish());
        put32(&mut patch, total as u32);
        put32(&mut patch, total as u32);
        self.w.seek(SeekFrom::Start(self.pos + 14)).map_err(|e| e.to_string())?;
        self.w.write_all(&patch).map_err(|e| e.to_string())?;
        self.w.seek(SeekFrom::Start(end)).map_err(|e| e.to_string())?;
        self.pos = end;
        self.entries.push(Central { name: name.to_string(), crc: crc.finish(), size: total as u32, offset, dos, attr: (0o100_000 | (mode & 0o777)) << 16 });
        Ok(total)
    }

    /// Write the directory and the end record; returns the writer.
    pub fn finish(mut self) -> Result<W, String> {
        let cd_start = u32::try_from(self.pos).map_err(|_| "the zip would be larger than 4 GB".to_string())?;
        let mut cd = Vec::new();
        for e in &self.entries {
            put32(&mut cd, 0x0201_4b50);
            put16(&mut cd, 0x031e); // made by: unix, version 3.0
            put16(&mut cd, 20);
            put16(&mut cd, 0x0800);
            put16(&mut cd, 0);
            put16(&mut cd, e.dos.0);
            put16(&mut cd, e.dos.1);
            put32(&mut cd, e.crc);
            put32(&mut cd, e.size);
            put32(&mut cd, e.size);
            put16(&mut cd, e.name.len() as u16);
            put16(&mut cd, 0);
            put16(&mut cd, 0);
            put16(&mut cd, 0);
            put16(&mut cd, 0);
            put32(&mut cd, e.attr);
            put32(&mut cd, e.offset);
            cd.extend_from_slice(e.name.as_bytes());
        }
        let cd_len = u32::try_from(cd.len()).map_err(|_| "the zip directory is too large".to_string())?;
        put32(&mut cd, 0x0605_4b50);
        put16(&mut cd, 0);
        put16(&mut cd, 0);
        put16(&mut cd, self.entries.len() as u16);
        put16(&mut cd, self.entries.len() as u16);
        put32(&mut cd, cd_len);
        put32(&mut cd, cd_start);
        put16(&mut cd, 0);
        self.w.write_all(&cd).map_err(|e| e.to_string())?;
        self.w.flush().map_err(|e| e.to_string())?;
        Ok(self.w)
    }
}

fn local_header(h: &mut Vec<u8>, name: &str, dos: (u16, u16), crc: u32, size: u32) {
    put32(h, 0x0403_4b50);
    put16(h, 20);
    put16(h, 0x0800); // names are UTF-8
    put16(h, 0); // stored
    put16(h, dos.0);
    put16(h, dos.1);
    put32(h, crc);
    put32(h, size);
    put32(h, size);
    put16(h, name.len() as u16);
    put16(h, 0);
    h.extend_from_slice(name.as_bytes());
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    fn tmpdir(tag: &str) -> std::path::PathBuf {
        let base = std::env::var("TMPDIR").unwrap_or_else(|_| std::env::temp_dir().to_string_lossy().into_owned());
        let d = std::path::PathBuf::from(base).join(format!("iyakki-zipw-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn the_checksum_and_the_dos_time_are_right() {
        let mut c = Crc32::new();
        c.update(b"1234");
        c.update(b"56789");
        assert_eq!(c.finish(), 0xCBF4_3926, "the standard check value");
        assert_eq!(Crc32::new().finish(), 0);
        // 2024-02-29 13:45:58 UTC (a leap day), then clamping at both ends
        let (t, d) = dos_time(1_709_214_358);
        assert_eq!((d >> 9) + 1980, 2024);
        assert_eq!(((d >> 5) & 15, d & 31), (2, 29));
        assert_eq!((t >> 11, (t >> 5) & 63, (t & 31) * 2), (13, 45, 58));
        assert_eq!(dos_time(0), dos_time(315_532_800), "before 1980 is 1980");
        assert_eq!(dos_time(i64::MAX), dos_time(4_354_819_199));
    }

    #[test]
    fn what_is_written_is_read_back_by_the_unzip_of_this_crate_with_names_modes_and_contents() {
        let mut z = ZipWriter::new(Cursor::new(Vec::new()));
        z.add_dir("docs", 1_700_000_000).unwrap();
        z.add_file("docs/a.txt", 1_700_000_000, 0o640, &mut &b"alpha"[..], 1 << 20).unwrap();
        z.add_file("empty", 1_700_000_000, 0o644, &mut &b""[..], 10).unwrap();
        z.add_file("docs/Grüße €.txt", 1_700_000_000, 0o755, &mut &b"unicode name"[..], 100).unwrap();
        let big: Vec<u8> = (0..300_000u32).map(|i| (i % 251) as u8).collect();
        z.add_file("big.bin", 1_700_000_000, 0o644, &mut &big[..], 1 << 20).unwrap();
        assert_eq!(z.entries(), 5);
        let bytes = z.finish().unwrap().into_inner();
        let d = tmpdir("roundtrip");
        std::fs::write(d.join("x.zip"), &bytes).unwrap();
        let out = d.join("out");
        let un = crate::archive::unzip(&d.join("x.zip"), &out).unwrap();
        assert_eq!(un.files, 4);
        assert_eq!(std::fs::read(out.join("docs/a.txt")).unwrap(), b"alpha");
        assert_eq!(std::fs::read(out.join("docs/Grüße €.txt")).unwrap(), b"unicode name");
        assert_eq!(std::fs::read(out.join("big.bin")).unwrap(), big);
        assert_eq!(std::fs::read(out.join("empty")).unwrap(), b"");
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(std::fs::metadata(out.join("docs/a.txt")).unwrap().permissions().mode() & 0o777, 0o640);
        // the stored checksum matches the data (read from the central directory by hand)
        let eocd = bytes.windows(4).rposition(|w| w == [0x50, 0x4b, 0x05, 0x06]).unwrap();
        assert_eq!(u16::from_le_bytes([bytes[eocd + 10], bytes[eocd + 11]]), 5);
        let mut c = Crc32::new();
        c.update(&big);
        assert!(bytes.windows(4).any(|w| w == c.finish().to_le_bytes()));
    }

    #[test]
    fn names_that_leave_the_archive_and_files_that_do_not_fit_are_refused() {
        for bad in ["", "/etc/passwd", "../x", "a/../b", "a//b", "a\\b", "a\0b", "./a", "a\nb", &"x".repeat(2000)] {
            assert!(valid_name(bad).is_err(), "{bad:?}");
        }
        assert!(valid_name("a/b c/d.txt").is_ok() && valid_name("dir/").is_ok());
        let mut z = ZipWriter::new(Cursor::new(Vec::new()));
        assert!(z.add_file("../escape", 0, 0o644, &mut &b"x"[..], 10).is_err());
        assert!(z.add_dir("/abs", 0).is_err());
        assert!(z.add_file("big", 0, 0o644, &mut &[0u8; 100][..], 50).unwrap_err().contains("too large"));
        // a reader that fails midway is an error, not a short file
        struct Broken;
        impl Read for Broken {
            fn read(&mut self, _: &mut [u8]) -> std::io::Result<usize> {
                Err(std::io::Error::other("disk gone"))
            }
        }
        assert!(z.add_file("b", 0, 0o644, &mut Broken, 10).unwrap_err().contains("disk gone"));
        // a hostile entry count stops at the limit
        let mut z = ZipWriter::new(Cursor::new(Vec::new()));
        for i in 0..MAX_ENTRIES {
            z.add_dir(&format!("d{i}"), 0).unwrap();
        }
        assert!(z.add_dir("one-more", 0).unwrap_err().contains("65534"));
        assert!(z.finish().is_ok());
    }
}
