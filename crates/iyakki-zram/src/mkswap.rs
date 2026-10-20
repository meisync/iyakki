//! `mkswap` in Rust: the Linux swap header (version 1), so no program is needed to prepare zram or a swap file.
//!
//! Layout (include/linux/swap.h `union swap_header`, first page of the area): bytes 0..1024 boot sector (left zero),
//! 1024 version (u32, 1), 1028 last_page (u32), 1032 nr_badpages (u32, 0), 1036 uuid (16), 1052 volume name (16,
//! NUL padded), the rest zero, and the 10 bytes "SWAPSPACE2" at the very end of the page.

pub const MAGIC: &[u8; 10] = b"SWAPSPACE2";

/// The first page of a swap area of `bytes` bytes with `page` byte pages. `label` is cut to 15 bytes.
pub fn header(bytes: u64, page: usize, uuid: [u8; 16], label: &str) -> Result<Vec<u8>, String> {
    if page < 1100 || !page.is_power_of_two() {
        return Err(format!("odd page size {page}"));
    }
    let pages = bytes / page as u64;
    if pages < 10 {
        return Err("swap area too small (needs at least 10 pages)".into());
    }
    let last = pages - 1;
    if last > u32::MAX as u64 {
        return Err("swap area too large for this header format".into());
    }
    let mut h = vec![0u8; page];
    h[1024..1028].copy_from_slice(&1u32.to_ne_bytes());
    h[1028..1032].copy_from_slice(&(last as u32).to_ne_bytes());
    h[1036..1052].copy_from_slice(&uuid);
    let l = label.as_bytes();
    let n = l.len().min(15);
    h[1052..1052 + n].copy_from_slice(&l[..n]);
    h[page - 10..].copy_from_slice(MAGIC);
    Ok(h)
}

/// A random version-4 UUID from the kernel's entropy.
pub fn random_uuid() -> [u8; 16] {
    let mut u = [0u8; 16];
    let ok = std::fs::File::open("/dev/urandom").and_then(|mut f| std::io::Read::read_exact(&mut f, &mut u)).is_ok();
    if !ok {
        let t = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_nanos());
        u.copy_from_slice(&t.to_le_bytes());
    }
    u[6] = (u[6] & 0x0f) | 0x40;
    u[8] = (u[8] & 0x3f) | 0x80;
    u
}

/// The kernel's page size.
pub fn page_size() -> usize {
    // SAFETY: sysconf takes an integer and returns a value.
    let p = unsafe { libc::sysconf(libc::_SC_PAGESIZE) };
    if p >= 4096 { p as usize } else { 4096 }
}

/// Write the header to the start of `path` (a block device or a file already `bytes` long).
pub fn write(path: &std::path::Path, bytes: u64, label: &str) -> Result<(), String> {
    use std::io::Write;
    let h = header(bytes, page_size(), random_uuid(), label)?;
    let mut f = std::fs::OpenOptions::new().write(true).open(path).map_err(|e| format!("{}: {e}", path.display()))?;
    f.write_all(&h).map_err(|e| format!("{}: {e}", path.display()))?;
    f.sync_all().map_err(|e| format!("{}: {e}", path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn u32at(h: &[u8], o: usize) -> u32 {
        u32::from_ne_bytes(h[o..o + 4].try_into().unwrap())
    }

    #[test]
    fn header_layout_for_4k_and_16k_pages() {
        for page in [4096usize, 16384, 65536] {
            let uuid: [u8; 16] = std::array::from_fn(|i| i as u8 + 1);
            let h = header(1 << 30, page, uuid, "IYAKKI-ZRAM").unwrap();
            assert_eq!(h.len(), page);
            assert_eq!(&h[page - 10..], b"SWAPSPACE2", "magic ends the first page");
            assert_eq!(u32at(&h, 1024), 1, "version");
            assert_eq!(u32at(&h, 1028) as u64, (1u64 << 30) / page as u64 - 1, "last_page");
            assert_eq!(u32at(&h, 1032), 0, "no bad pages");
            assert_eq!(&h[1036..1052], &uuid);
            assert_eq!(&h[1052..1063], b"IYAKKI-ZRAM");
            assert!(h[1063..1068].iter().all(|b| *b == 0));
            assert!(h[..1024].iter().all(|b| *b == 0));
            assert!(h[1068..page - 10].iter().all(|b| *b == 0));
        }
    }

    #[test]
    fn refusals_and_label_cut() {
        assert!(header(4096 * 9, 4096, [0; 16], "x").is_err());
        assert!(header(1 << 30, 1000, [0; 16], "x").is_err());
        assert!(header(1 << 50, 4096, [0; 16], "x").is_err(), "last_page does not fit 32 bits");
        let h = header(1 << 20, 4096, [0; 16], "0123456789ABCDEFGHIJ").unwrap();
        assert_eq!(&h[1052..1067], b"0123456789ABCDE");
        assert_eq!(h[1067], 0);
        let u = random_uuid();
        assert_eq!((u[6] >> 4, u[8] >> 6), (4, 2), "version 4, RFC 4122 variant");
        assert_ne!(random_uuid(), random_uuid());
    }

    #[test]
    fn matches_the_hosts_mkswap_when_there_is_one() {
        let Some(mkswap) = ["/sbin/mkswap", "/usr/sbin/mkswap"].iter().find(|p| std::path::Path::new(p).exists()) else { return };
        let dir = std::env::temp_dir().join(format!("iyakki-mkswap-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let f = dir.join("swap.img");
        std::fs::File::create(&f).unwrap().set_len(64 << 20).unwrap();
        let ok = std::process::Command::new(mkswap).args(["-U", "01020304-0506-4708-8910-111213141516", "-L", "IYAKKI-ZRAM"]).arg(&f).output().map(|o| o.status.success()).unwrap_or(false);
        if ok {
            let theirs = std::fs::read(&f).unwrap();
            let uuid: [u8; 16] = [1, 2, 3, 4, 5, 6, 0x47, 8, 0x89, 0x10, 0x11, 0x12, 0x13, 0x14, 0x15, 0x16];
            let mine = header(64 << 20, page_size(), uuid, "IYAKKI-ZRAM").unwrap();
            assert_eq!(&theirs[..mine.len()], &mine[..], "byte for byte the same as util-linux mkswap");
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn writes_to_a_file() {
        let dir = std::env::temp_dir().join(format!("iyakki-mkswap-w-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let f = dir.join("s");
        std::fs::File::create(&f).unwrap().set_len(1 << 20).unwrap();
        write(&f, 1 << 20, "X").unwrap();
        let b = std::fs::read(&f).unwrap();
        let p = page_size();
        assert_eq!(&b[p - 10..p], b"SWAPSPACE2");
        assert_eq!(b.len(), 1 << 20, "the rest of the file is untouched");
        assert!(write(&dir.join("missing"), 1 << 20, "X").is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
