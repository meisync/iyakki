//! Writing an image to a drive and checking that what is on the drive is what we meant to write.

use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use sha2::{Digest, Sha256};

pub const BLOCK: usize = 4 * 1024 * 1024;
/// How much is flushed to the device at a time, so the progress bar shows the device's speed, not the page cache's.
const FLUSH_EVERY: u64 = 8 * 1024 * 1024;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Phase {
    Checking,
    Writing,
    Syncing,
    Verifying,
}

impl Phase {
    pub fn label(self) -> &'static str {
        match self {
            Phase::Checking => "Checking image",
            Phase::Writing => "Writing",
            Phase::Syncing => "Flushing",
            Phase::Verifying => "Verifying",
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub struct Progress {
    pub phase: Phase,
    pub done: u64,
    pub total: u64,
    pub elapsed: Duration,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ImageKind {
    /// ISO 9660 (the IYAKKI x86 image, which is also bootable as a hybrid disk).
    Iso9660,
    /// A raw disk image with a partition table (the IYAKKI aarch64 image).
    DiskImage,
    /// The IYAKKI install template: partitioned disk (MBR boot code, GRUB gap, FAT boot partition, marked data
    /// partition). Writing it also stretches the data partition to fill the drive.
    InstallTemplate,
    Unknown,
}

#[derive(Clone, Debug)]
pub struct ImageInfo {
    pub path: PathBuf,
    pub size: u64,
    pub kind: ImageKind,
}

pub fn inspect_image(path: &Path) -> Result<ImageInfo, String> {
    let mut f = std::fs::File::open(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let size = f.metadata().map_err(|e| e.to_string())?.len();
    if size == 0 {
        return Err(format!("{} is empty", path.display()));
    }
    let mut head = vec![0u8; 0x8006.min(size as usize)];
    f.read_exact(&mut head).map_err(|e| e.to_string())?;
    let kind = if head.get(0x8001..0x8006) == Some(b"CD001") {
        ImageKind::Iso9660
    } else if is_install_template(&head) {
        ImageKind::InstallTemplate
    } else if head.get(510..512) == Some(&[0x55, 0xAA]) || head.get(512..520) == Some(b"EFI PART") {
        ImageKind::DiskImage
    } else {
        ImageKind::Unknown
    };
    Ok(ImageInfo { path: path.into(), size, kind })
}

/// The template carries a fixed disk signature (`00 53 46 41`, "\0SFA") and a valid MBR.
pub fn is_install_template(head: &[u8]) -> bool {
    head.len() >= 512 && head[440..444] == [0x00, 0x53, 0x46, 0x41] && head[510..512] == [0x55, 0xAA]
}

/// Where the data partition ended up (in 512-byte sectors).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct InstallLayout {
    pub data_start: u64,
    pub data_sectors: u64,
}

/// The smallest data partition worth creating.
pub const MIN_DATA_SECTORS: u64 = 64 * 1024 * 1024 / 512;

/// Rewrite the template's boot sector for a drive of `device_bytes`: partition 2 (IYAKKI data) grows to fill the
/// drive (1 MiB aligned, within what an MBR can address), and the disk gets a fresh signature so two sticks
/// never look alike. Everything else, the boot code and partition 1, is left exactly as built.
pub fn patch_install_mbr(sector0: &mut [u8], device_bytes: u64, new_signature: [u8; 4]) -> Result<InstallLayout, String> {
    if !is_install_template(sector0) {
        return Err("not an IYAKKI install template".into());
    }
    let e = 446 + 16; // partition 2
    if sector0[e + 4] != 0x83 {
        return Err("the template's second partition is not a data partition".into());
    }
    let start = u32::from_le_bytes([sector0[e + 8], sector0[e + 9], sector0[e + 10], sector0[e + 11]]) as u64;
    let total = device_bytes / 512;
    if total <= start + MIN_DATA_SECTORS {
        return Err(format!("this drive is too small for an install (needs about {} MB)", (start + MIN_DATA_SECTORS) * 512 / 1_000_000 + 1));
    }
    let max = 0xFFFF_FFFFu64 - start; // MBR sector counts are 32-bit
    let sectors = ((total - start).min(max) / 2048) * 2048;
    sector0[e + 12..e + 16].copy_from_slice(&(sectors as u32).to_le_bytes());
    // CHS fields are meaningless on big disks: use the "use LBA" values
    sector0[e + 1..e + 4].copy_from_slice(&[0xFE, 0xFF, 0xFF]);
    sector0[e + 5..e + 8].copy_from_slice(&[0xFE, 0xFF, 0xFF]);
    sector0[440..444].copy_from_slice(&new_signature);
    Ok(InstallLayout { data_start: start, data_sectors: sectors })
}

/// Applied to each block just before it is hashed and written: `(offset in the image, block)`.
pub type Patch<'a> = &'a dyn Fn(u64, &mut [u8]) -> Result<(), String>;

pub(crate) fn hex(d: &[u8]) -> String {
    d.iter().map(|b| format!("{b:02x}")).collect()
}

pub fn file_sha256(path: &Path, progress: &mut dyn FnMut(Progress)) -> Result<String, String> {
    let mut f = std::fs::File::open(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let total = f.metadata().map_err(|e| e.to_string())?.len();
    let (mut h, mut buf, mut done, t0) = (Sha256::new(), vec![0u8; BLOCK], 0u64, Instant::now());
    loop {
        let n = f.read(&mut buf).map_err(|e| e.to_string())?;
        if n == 0 {
            break;
        }
        h.update(&buf[..n]);
        done += n as u64;
        progress(Progress { phase: Phase::Checking, done, total, elapsed: t0.elapsed() });
    }
    Ok(hex(&h.finalize()))
}

/// The expected SHA-256 from `<image>.sha256` ("<hex>  name" or just "<hex>").
pub fn read_sidecar(image: &Path) -> Option<String> {
    let mut name = image.as_os_str().to_owned();
    name.push(".sha256");
    let text = std::fs::read_to_string(PathBuf::from(name)).ok()?;
    let tok = text.split_whitespace().next()?.to_lowercase();
    (tok.len() == 64 && tok.chars().all(|c| c.is_ascii_hexdigit())).then_some(tok)
}

/// The expected SHA-256 of `image` from a release manifest (`release.json`, written by `iyakki release sign`) in the same folder,
/// or from `manifest` when given. The manifest is read as text (`"name": ...` followed by its `"sha256": ...`; the keys are
/// written in that order); its signature is checked with `iyakki release verify`, not here.
pub fn read_manifest_sum(image: &Path, manifest: Option<&Path>) -> Option<String> {
    let path = manifest.map(Path::to_path_buf).unwrap_or_else(|| image.with_file_name("release.json"));
    let text = std::fs::read_to_string(path).ok()?;
    manifest_sum_in(&text, &image.file_name()?.to_string_lossy())
}

pub fn manifest_sum_in(text: &str, name: &str) -> Option<String> {
    let at = text.find(&format!("\"name\": \"{name}\""))?;
    let rest = &text[at..];
    let end = rest.find('}').unwrap_or(rest.len());
    let rest = &rest[..end];
    let i = rest.find("\"sha256\": \"")? + "\"sha256\": \"".len();
    let tok = rest[i..].split('"').next()?.to_lowercase();
    (tok.len() == 64 && tok.chars().all(|c| c.is_ascii_hexdigit())).then_some(tok)
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WriteReport {
    pub bytes: u64,
    pub sha256: String,
}

/// Write `image` to `target` (a block device, or a regular file for testing). `device_size` is the capacity of
/// the drive when known: an image that does not fit is refused before anything is written.
pub fn write_image(image: &Path, target: &Path, device_size: Option<u64>, patch: Option<Patch>, progress: &mut dyn FnMut(Progress)) -> Result<WriteReport, String> {
    use std::os::unix::fs::FileTypeExt;
    let mut src = std::fs::File::open(image).map_err(|e| format!("{}: {e}", image.display()))?;
    let total = src.metadata().map_err(|e| e.to_string())?.len();
    if let Some(cap) = device_size {
        if cap < total {
            return Err(format!("the image is {} bytes but the drive holds only {cap}", total));
        }
    }
    let existing = std::fs::metadata(target).ok();
    let is_block = existing.as_ref().is_some_and(|m| m.file_type().is_block_device());
    let mut dst = std::fs::OpenOptions::new().write(true).create(!is_block).truncate(!is_block && existing.is_none()).open(target).map_err(|e| format!("{}: {e}", target.display()))?;
    let (mut h, mut buf, mut done, mut since_flush, t0) = (Sha256::new(), vec![0u8; BLOCK], 0u64, 0u64, Instant::now());
    loop {
        let n = src.read(&mut buf).map_err(|e| format!("reading the image: {e}"))?;
        if n == 0 {
            break;
        }
        if let Some(p) = patch {
            p(done, &mut buf[..n])?;
        }
        h.update(&buf[..n]);
        dst.write_all(&buf[..n]).map_err(|e| format!("writing {}: {e}", target.display()))?;
        done += n as u64;
        since_flush += n as u64;
        if since_flush >= FLUSH_EVERY {
            dst.sync_data().map_err(|e| format!("flushing {}: {e}", target.display()))?;
            since_flush = 0;
        }
        progress(Progress { phase: Phase::Writing, done, total, elapsed: t0.elapsed() });
    }
    progress(Progress { phase: Phase::Syncing, done, total, elapsed: t0.elapsed() });
    dst.sync_all().map_err(|e| format!("flushing {}: {e}", target.display()))?;
    Ok(WriteReport { bytes: done, sha256: hex(&h.finalize()) })
}

/// Erase the last `len` bytes of the drive: a stale backup partition table left from the stick's previous life
/// would otherwise confuse the kernel about where the new image's tables are.
pub fn zero_tail(target: &Path, device_size: u64, len: u64) -> Result<(), String> {
    let len = len.min(device_size);
    let mut f = std::fs::OpenOptions::new().write(true).open(target).map_err(|e| format!("{}: {e}", target.display()))?;
    f.seek(SeekFrom::Start(device_size - len)).map_err(|e| e.to_string())?;
    f.write_all(&vec![0u8; len as usize]).map_err(|e| e.to_string())?;
    f.sync_all().map_err(|e| e.to_string())
}

/// Open for verification so reads come from the device, not from the cache that just absorbed our writes.
fn open_uncached(path: &Path) -> Result<std::fs::File, String> {
    #[cfg(target_os = "linux")]
    {
        use std::os::unix::fs::OpenOptionsExt;
        if let Ok(f) = std::fs::OpenOptions::new().read(true).custom_flags(libc::O_DIRECT).open(path) {
            return Ok(f);
        }
    }
    let f = std::fs::File::open(path).map_err(|e| format!("{}: {e}", path.display()))?;
    #[cfg(target_os = "macos")]
    {
        use std::os::fd::AsRawFd;
        // SAFETY: F_NOCACHE takes an integer flag.
        unsafe { libc::fcntl(f.as_raw_fd(), libc::F_NOCACHE, 1) };
    }
    Ok(f)
}

/// Read back `bytes` bytes from `target` and compare their SHA-256 with `expected`.
pub fn verify(target: &Path, bytes: u64, expected: &str, progress: &mut dyn FnMut(Progress)) -> Result<(), String> {
    verify_range(target, 0, bytes, expected, progress)
}

/// Like [`verify`] for `bytes` bytes starting at byte `offset` (a multiple of the drive's sector size, as is `bytes` for
/// direct reads). Progress reports within the range.
pub fn verify_range(target: &Path, offset: u64, bytes: u64, expected: &str, progress: &mut dyn FnMut(Progress)) -> Result<(), String> {
    let mut f = open_uncached(target)?;
    f.seek(SeekFrom::Start(offset)).map_err(|e| format!("reading back {}: {e}", target.display()))?;
    // O_DIRECT needs a buffer aligned to the device's block size
    let mut raw = vec![0u8; BLOCK + 4096];
    let off = raw.as_ptr().align_offset(4096);
    let buf = &mut raw[off..off + BLOCK];
    let (mut h, mut done, t0) = (Sha256::new(), 0u64, Instant::now());
    while done < bytes {
        let want = BLOCK.min(((bytes - done) as usize).div_ceil(4096) * 4096).max(4096);
        let mut got = 0;
        while got < want {
            match f.read(&mut buf[got..want]) {
                Ok(0) => break,
                Ok(n) => got += n,
                Err(e) => return Err(format!("reading back {}: {e}", target.display())),
            }
        }
        let use_n = got.min((bytes - done) as usize);
        if use_n == 0 {
            return Err("the drive returned less data than was written".into());
        }
        h.update(&buf[..use_n]);
        done += use_n as u64;
        progress(Progress { phase: Phase::Verifying, done, total: bytes, elapsed: t0.elapsed() });
    }
    let got = hex(&h.finalize());
    if got == expected { Ok(()) } else { Err(format!("verification FAILED: the drive holds different data than the image (expected {}…, read {}…). Try another USB port or stick.", &expected[..12], &got[..12])) }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("iyakki-usb-{}-{tag}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    /// A fake image: an ISO header at the right offset followed by pseudo-random data.
    fn image(path: &Path, mib: usize, iso: bool) {
        let mut s = 0x9e3779b97f4a7c15u64;
        let mut data: Vec<u8> = (0..mib * 1024 * 1024).map(|_| {
            s ^= s << 13;
            s ^= s >> 7;
            s ^= s << 17;
            (s >> 24) as u8
        }).collect();
        if iso {
            data[0x8001..0x8006].copy_from_slice(b"CD001");
        } else {
            data[510] = 0x55;
            data[511] = 0xAA;
        }
        std::fs::write(path, data).unwrap();
    }

    #[test]
    fn images_are_recognised_and_junk_is_flagged() {
        let d = tmp("inspect");
        image(&d.join("a.iso"), 1, true);
        image(&d.join("b.img"), 1, false);
        std::fs::write(d.join("c.txt"), vec![b'x'; 70_000]).unwrap();
        std::fs::write(d.join("empty"), b"").unwrap();
        assert_eq!(inspect_image(&d.join("a.iso")).unwrap().kind, ImageKind::Iso9660);
        assert_eq!(inspect_image(&d.join("b.img")).unwrap().kind, ImageKind::DiskImage);
        assert_eq!(inspect_image(&d.join("c.txt")).unwrap().kind, ImageKind::Unknown);
        assert!(inspect_image(&d.join("empty")).unwrap_err().contains("empty"));
        assert!(inspect_image(&d.join("missing")).is_err());
        std::fs::write(d.join("tiny"), b"hi").unwrap();
        assert_eq!(inspect_image(&d.join("tiny")).unwrap().kind, ImageKind::Unknown, "files shorter than a header do not panic");
    }

    #[test]
    fn write_then_verify_roundtrips_with_monotonic_progress() {
        let d = tmp("write");
        image(&d.join("a.iso"), 10, true);
        let mut seen: Vec<(Phase, u64)> = Vec::new();
        let rep = write_image(&d.join("a.iso"), &d.join("stick"), Some(64 * 1024 * 1024), None, &mut |p| seen.push((p.phase, p.done))).unwrap();
        assert_eq!(rep.bytes, 10 * 1024 * 1024);
        assert_eq!(std::fs::read(d.join("stick")).unwrap(), std::fs::read(d.join("a.iso")).unwrap());
        assert_eq!(seen.first().unwrap().0, Phase::Writing);
        assert_eq!(seen.last().unwrap().0, Phase::Syncing);
        let writes: Vec<u64> = seen.iter().filter(|(p, _)| *p == Phase::Writing).map(|(_, d)| *d).collect();
        assert!(writes.len() >= 3 && writes.windows(2).all(|w| w[0] < w[1]), "{writes:?}");
        assert_eq!(*writes.last().unwrap(), rep.bytes);
        assert_eq!(rep.sha256, file_sha256(&d.join("a.iso"), &mut |_| {}).unwrap());
        let mut v = Vec::new();
        verify(&d.join("stick"), rep.bytes, &rep.sha256, &mut |p| v.push(p.done)).unwrap();
        assert_eq!(*v.last().unwrap(), rep.bytes);
        assert!(v.windows(2).all(|w| w[0] < w[1]));
    }

    #[test]
    fn verification_catches_a_wrong_byte_and_a_short_drive() {
        let d = tmp("verify");
        image(&d.join("a.iso"), 6, true);
        let rep = write_image(&d.join("a.iso"), &d.join("stick"), None, None, &mut |_| {}).unwrap();
        let mut bytes = std::fs::read(d.join("stick")).unwrap();
        bytes[5 * 1024 * 1024 + 123] ^= 0x01;
        std::fs::write(d.join("stick"), &bytes).unwrap();
        let e = verify(&d.join("stick"), rep.bytes, &rep.sha256, &mut |_| {}).unwrap_err();
        assert!(e.contains("FAILED"), "{e}");
        std::fs::write(d.join("stick"), &bytes[..2 * 1024 * 1024]).unwrap();
        assert!(verify(&d.join("stick"), rep.bytes, &rep.sha256, &mut |_| {}).is_err(), "truncated");
        assert!(verify(&d.join("missing"), 10, "00", &mut |_| {}).is_err());
    }

    #[test]
    fn an_image_that_does_not_fit_is_refused_before_anything_is_written() {
        let d = tmp("toobig");
        image(&d.join("a.iso"), 2, true);
        let e = write_image(&d.join("a.iso"), &d.join("stick"), Some(1024 * 1024), None, &mut |_| {}).unwrap_err();
        assert!(e.contains("holds only"), "{e}");
        assert!(!d.join("stick").exists(), "nothing was created or written");
        assert!(write_image(&d.join("nope.iso"), &d.join("stick"), None, None, &mut |_| {}).is_err());
    }

    #[test]
    fn the_tail_of_the_drive_is_wiped_without_touching_the_image() {
        let d = tmp("tail");
        std::fs::write(d.join("stick"), vec![0xFFu8; 4 * 1024 * 1024]).unwrap();
        zero_tail(&d.join("stick"), 4 * 1024 * 1024, 1024 * 1024).unwrap();
        let b = std::fs::read(d.join("stick")).unwrap();
        assert!(b[..3 * 1024 * 1024].iter().all(|x| *x == 0xFF));
        assert!(b[3 * 1024 * 1024..].iter().all(|x| *x == 0));
        assert_eq!(b.len(), 4 * 1024 * 1024);
    }

    #[test]
    fn checksums_come_from_a_sidecar_file_in_either_format() {
        let d = tmp("sidecar");
        image(&d.join("a.iso"), 1, true);
        let sum = file_sha256(&d.join("a.iso"), &mut |_| {}).unwrap();
        assert_eq!(read_sidecar(&d.join("a.iso")), None);
        std::fs::write(d.join("a.iso.sha256"), format!("{}  a.iso\n", sum.to_uppercase())).unwrap();
        assert_eq!(read_sidecar(&d.join("a.iso")), Some(sum.clone()));
        std::fs::write(d.join("a.iso.sha256"), format!("{sum}\n")).unwrap();
        assert_eq!(read_sidecar(&d.join("a.iso")), Some(sum));
        std::fs::write(d.join("a.iso.sha256"), "not a checksum\n").unwrap();
        assert_eq!(read_sidecar(&d.join("a.iso")), None);
        // a release manifest next to the image
        let h = "ab".repeat(32);
        let m = format!("{{\n  \"files\": [\n    {{\n      \"bytes\": 5,\n      \"name\": \"other.iso\",\n      \"sha256\": \"{}\"\n    }},\n    {{\n      \"bytes\": 1,\n      \"name\": \"a.iso\",\n      \"sha256\": \"{h}\"\n    }}\n  ]\n}}\n", "cd".repeat(32));
        assert_eq!(manifest_sum_in(&m, "a.iso"), Some(h.clone()));
        assert_eq!(manifest_sum_in(&m, "b.iso"), None);
        assert_eq!(read_manifest_sum(&d.join("a.iso"), None), None);
        std::fs::write(d.join("release.json"), &m).unwrap();
        assert_eq!(read_manifest_sum(&d.join("a.iso"), None), Some(h));
        // known answer, so a broken hash would be noticed
        std::fs::write(d.join("abc"), b"abc").unwrap();
        assert_eq!(file_sha256(&d.join("abc"), &mut |_| {}).unwrap(), "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad");
    }

    /// A synthetic install template: MBR with boot code, partition 1 (EFI, bootable) and partition 2 (data, 1 MiB).
    fn template(path: &Path) -> Vec<u8> {
        let mut t = vec![0u8; 4 * 1024 * 1024];
        t[..440].iter_mut().enumerate().for_each(|(i, b)| *b = (i % 251) as u8 + 1); // stands in for GRUB's boot code
        t[440..444].copy_from_slice(&[0x00, 0x53, 0x46, 0x41]);
        let p1 = 446;
        t[p1] = 0x80;
        t[p1 + 4] = 0xEF;
        t[p1 + 8..p1 + 12].copy_from_slice(&2048u32.to_le_bytes());
        t[p1 + 12..p1 + 16].copy_from_slice(&4096u32.to_le_bytes());
        let p2 = 446 + 16;
        t[p2 + 4] = 0x83;
        t[p2 + 8..p2 + 12].copy_from_slice(&6144u32.to_le_bytes());
        t[p2 + 12..p2 + 16].copy_from_slice(&2048u32.to_le_bytes());
        t[510] = 0x55;
        t[511] = 0xAA;
        t[2048 * 512..2048 * 512 + 8].copy_from_slice(b"FATBOOT!");
        t[6144 * 512..6144 * 512 + 20].copy_from_slice(b"IYAKKI-FORMAT-ME-v1\0");
        std::fs::write(path, &t).unwrap();
        t
    }

    #[test]
    fn the_template_is_recognised_and_its_data_partition_is_stretched_to_fill_the_drive() {
        let d = tmp("template");
        let t = template(&d.join("t.img"));
        assert_eq!(inspect_image(&d.join("t.img")).unwrap().kind, ImageKind::InstallTemplate);
        assert!(!is_install_template(&t[..100]) && !is_install_template(&vec![0u8; 512]));
        let mut s0 = t[..512].to_vec();
        let l = patch_install_mbr(&mut s0, 64 * 1024 * 1024 * 1024, [1, 2, 3, 4]).unwrap();
        assert_eq!(l.data_start, 6144);
        assert_eq!(l.data_sectors, ((64u64 * 1024 * 1024 * 1024 / 512 - 6144) / 2048) * 2048, "fills the drive, 1 MiB aligned");
        let e = 446 + 16;
        assert_eq!(u32::from_le_bytes(s0[e + 12..e + 16].try_into().unwrap()) as u64, l.data_sectors);
        assert_eq!(&s0[440..444], &[1, 2, 3, 4], "fresh disk signature");
        assert_eq!(&s0[..440], &t[..440], "the boot code is untouched");
        assert_eq!(&s0[446..446 + 16], &t[446..446 + 16], "partition 1 is untouched");
        assert_eq!(&s0[510..512], &[0x55, 0xAA]);
        assert!(!is_install_template(&s0), "a written disk no longer looks like the template");
        // limits: too small, and a huge disk is capped at what 32-bit sector counts can address
        assert!(patch_install_mbr(&mut t[..512].to_vec(), 64 * 1024 * 1024, [0; 4]).unwrap_err().contains("too small"));
        assert!(patch_install_mbr(&mut t[..512].to_vec(), 100 * 1024 * 1024, [0; 4]).is_ok(), "a small data partition is allowed down to 64 MiB");
        let big = patch_install_mbr(&mut t[..512].to_vec(), 10 * 1024 * 1024 * 1024 * 1024, [0; 4]).unwrap();
        assert!(big.data_start + big.data_sectors <= 0xFFFF_FFFF);
        assert!(patch_install_mbr(&mut vec![0u8; 512], 1 << 40, [0; 4]).is_err(), "not a template");
        let mut wrong = t[..512].to_vec();
        wrong[446 + 16 + 4] = 0x0B;
        assert!(patch_install_mbr(&mut wrong, 1 << 40, [0; 4]).unwrap_err().contains("not a data partition"));
    }

    #[test]
    fn installing_writes_the_patched_boot_sector_and_verifies_the_patched_data() {
        let d = tmp("install");
        let t = template(&d.join("t.img"));
        let dev_size = 256u64 * 1024 * 1024;
        let patch = move |off: u64, buf: &mut [u8]| -> Result<(), String> {
            if off == 0 {
                patch_install_mbr(&mut buf[..512], dev_size, [9, 9, 9, 9]).map(|_| ())
            } else {
                Ok(())
            }
        };
        let rep = write_image(&d.join("t.img"), &d.join("stick"), Some(dev_size), Some(&patch), &mut |_| {}).unwrap();
        let out = std::fs::read(d.join("stick")).unwrap();
        assert_eq!(out.len(), t.len());
        assert_eq!(&out[440..444], &[9, 9, 9, 9]);
        assert_ne!(&out[..512], &t[..512]);
        assert_eq!(&out[512..], &t[512..], "only the boot sector differs from the template");
        assert_eq!(&out[2048 * 512..2048 * 512 + 8], b"FATBOOT!");
        assert_eq!(&out[6144 * 512..6144 * 512 + 19], b"IYAKKI-FORMAT-ME-v1", "the marker survives");
        let e = 446 + 16;
        assert_eq!(u32::from_le_bytes(out[e + 12..e + 16].try_into().unwrap()) as u64, (dev_size / 512 - 6144) / 2048 * 2048);
        // the checksum is of what was written (patched), so verification passes; the unpatched hash would not
        verify(&d.join("stick"), rep.bytes, &rep.sha256, &mut |_| {}).unwrap();
        assert_ne!(rep.sha256, file_sha256(&d.join("t.img"), &mut |_| {}).unwrap());
        // a failing patch stops the write
        let bad = |_: u64, _: &mut [u8]| -> Result<(), String> { Err("nope".into()) };
        assert_eq!(write_image(&d.join("t.img"), &d.join("stick2"), None, Some(&bad), &mut |_| {}).unwrap_err(), "nope");
    }
}
