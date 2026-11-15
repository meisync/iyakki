//! Legacy-BIOS boot of a disk that is shared with another system (F-INST-13): the planner side.
//!
//! The whole-disk install puts GRUB's boot code into sector 0 and its core image into the gap behind it (MBR) or the BIOS boot partition
//! (GPT). On a shared disk that area may belong to someone else, so nothing is written unless this module says the area is free, and
//! before anything is written the sectors that will change are saved to a file that can put them back. This module only reads and
//! decides; the writing step (and the `--bios` option that asks for it) uses [`merged_sector0`], [`Backup`] and [`verify_after`].
//!
//! What it checks (each failure is a plain sentence, none of them is silent):
//! * the disk has 512-byte sectors (GRUB's BIOS boot sector assumes them; 4096-byte-sector disks start through UEFI);
//! * sector 0 holds a valid partition table (signature `55 AA`);
//! * MBR: the gap between sector 0 and the first partition is big enough for the core image and every byte of the part we use is zero
//!   (a non-zero gap belongs to another boot loader or a disk-protection tool);
//! * GPT: a BIOS boot partition exists, is big enough and is all zero;
//! * the boot code in sector 0 is empty or GRUB's own, or the caller said in so many words that replacing another system's boot code is wanted.

use std::io::{Read, Seek, SeekFrom, Write};

use iyakki_storage::gpt::{self, Kind};
use sha2::{Digest, Sha256};

/// GRUB's boot sector keeps the core image's first sector here (8 bytes, little endian).
pub const BOOT_START_OFFSET: usize = 0x5C;
/// Bytes of boot code in sector 0; the disk signature (440..444), the reserved word and the partition table (446..510) are never touched.
pub const BOOT_CODE_BYTES: usize = 440;
/// Where the core image goes on an MBR disk: the sector right after sector 0.
pub const MBR_CORE_LBA: u64 = 1;
/// A core image smaller than this is not a GRUB core image.
pub const MIN_CORE_BYTES: u64 = 4096;
/// How much of the gap is read to judge it (the usual gap is 1 MiB minus one sector).
pub const GAP_READ_MAX: u64 = 1 << 20;

/// Who owns the boot code in sector 0.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum BootCode {
    Empty,
    Grub,
    Windows,
    Other,
}

impl BootCode {
    pub fn classify(code: &[u8]) -> BootCode {
        let code = &code[..code.len().min(BOOT_CODE_BYTES)];
        let has = |needle: &[u8]| code.windows(needle.len()).any(|w| w == needle);
        if code.iter().all(|b| *b == 0) {
            BootCode::Empty
        } else if has(b"GRUB ") && has(b"Geom") {
            BootCode::Grub
        } else if has(b"Invalid partition table") || has(b"Missing operating system") || has(b"Error loading operating system") {
            BootCode::Windows
        } else {
            BootCode::Other
        }
    }
    pub fn label(&self) -> &'static str {
        match self {
            BootCode::Empty => "empty",
            BootCode::Grub => "GRUB",
            BootCode::Windows => "Windows or DOS",
            BootCode::Other => "another boot loader",
        }
    }
}

/// What was read from the disk.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Facts {
    pub kind: Kind,
    pub sector_size: u32,
    /// Sector 0, 512 bytes.
    pub sector0: Vec<u8>,
    /// MBR: the sectors from 1 up to the first partition (at most [`GAP_READ_MAX`] bytes). GPT: empty.
    pub gap: Vec<u8>,
    /// MBR: start of the first partition (in sectors); `None` for no partition.
    pub first_part_lba: Option<u64>,
    /// GPT: the first BIOS boot partition (first sector, sector count) and its contents (up to [`GAP_READ_MAX`] bytes).
    pub bios_boot: Option<(u64, u64)>,
    pub bios_boot_content: Vec<u8>,
    /// MBR: is any partition marked active?
    pub active_partition: bool,
}

/// Read the facts from a disk or an image file.
pub fn read_facts<R: Read + Seek>(r: &mut R, sector_size: u32, size_bytes: u64) -> Result<Facts, String> {
    let info = gpt::read_disk_table(r, sector_size, size_bytes);
    let mut sector0 = vec![0u8; 512];
    r.seek(SeekFrom::Start(0)).and_then(|_| r.read_exact(&mut sector0)).map_err(|e| format!("reading sector 0: {e}"))?;
    let mut f = Facts { kind: info.kind, sector_size, sector0, gap: vec![], first_part_lba: None, bios_boot: None, bios_boot_content: vec![], active_partition: false };
    match info.kind {
        Kind::Mbr => {
            f.active_partition = (0..4).any(|i| f.sector0[446 + 16 * i] == 0x80);
            f.first_part_lba = info.parts.iter().map(|p| p.first_lba).filter(|l| *l > 0).min();
            let end = f.first_part_lba.unwrap_or(size_bytes / 512);
            let bytes = (end.saturating_sub(1) * 512).min(GAP_READ_MAX);
            let mut gap = vec![0u8; bytes as usize];
            r.seek(SeekFrom::Start(512)).and_then(|_| r.read_exact(&mut gap)).map_err(|e| format!("reading the gap behind sector 0: {e}"))?;
            f.gap = gap;
        }
        Kind::Gpt => {
            f.bios_boot = info.parts.iter().find(|p| p.type_name.contains("BIOS boot")).map(|p| (p.first_lba, p.sectors));
            if let Some((lba, sectors)) = f.bios_boot {
                let bytes = (sectors * sector_size as u64).min(GAP_READ_MAX);
                let mut c = vec![0u8; bytes as usize];
                r.seek(SeekFrom::Start(lba * sector_size as u64)).and_then(|_| r.read_exact(&mut c)).map_err(|e| format!("reading the BIOS boot partition: {e}"))?;
                f.bios_boot_content = c;
            }
        }
        Kind::None => {}
    }
    Ok(f)
}

/// Where the core image goes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Method {
    /// MBR disk: sectors `core_lba ..` in the gap, `gap_sectors` available.
    MbrGap { core_lba: u64, gap_sectors: u64 },
    /// GPT disk: the BIOS boot partition.
    BiosBootPartition { start_lba: u64, sectors: u64 },
}

impl Method {
    pub fn core_lba(self) -> u64 {
        match self {
            Method::MbrGap { core_lba, .. } => core_lba,
            Method::BiosBootPartition { start_lba, .. } => start_lba,
        }
    }
}

/// The verdict.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Check {
    pub method: Option<Method>,
    /// Why legacy BIOS boot cannot be set up here. Empty when `method` is `Some`.
    pub refusals: Vec<String>,
    /// Things the person should know before saying yes.
    pub warnings: Vec<String>,
}

impl Check {
    pub fn ok(&self) -> bool {
        self.method.is_some()
    }
}

fn sectors_of(bytes: u64) -> u64 {
    bytes.div_ceil(512)
}

/// Decide whether legacy BIOS boot can be set up. `core_bytes` is the size of GRUB's core image to place; `replace_foreign_boot_code`
/// is true only when the person asked, in so many words, to replace another system's boot code.
pub fn evaluate(f: &Facts, core_bytes: u64, replace_foreign_boot_code: bool) -> Check {
    let mut refusals = Vec::new();
    let mut warnings = Vec::new();
    if core_bytes < MIN_CORE_BYTES {
        refusals.push(format!("the GRUB core image ({core_bytes} bytes) is too small to be one; the image is damaged"));
    }
    if f.sector_size != 512 {
        refusals.push(format!("this disk has {}-byte sectors; legacy BIOS boot needs 512-byte sectors, such PCs start through UEFI", f.sector_size));
    }
    if f.sector0.len() < 512 || f.sector0[510..512] != [0x55, 0xAA] {
        refusals.push("sector 0 has no partition table signature; this is not a partitioned disk".into());
    }
    let need = sectors_of(core_bytes);
    let mut method = None;
    match f.kind {
        Kind::None => refusals.push("the disk has no partition table".into()),
        Kind::Mbr => {
            let gap_sectors = f.first_part_lba.map_or(0, |l| l.saturating_sub(1));
            if gap_sectors < need {
                refusals.push(format!("the gap before the first partition is {gap_sectors} sectors ({} KiB) and GRUB needs {need} sectors ({} KiB); the other system's partitions would have to move",
                    gap_sectors / 2, need.div_ceil(2)));
            } else if let Some(i) = f.gap.iter().take((need * 512) as usize).position(|b| *b != 0) {
                refusals.push(format!("the gap behind sector 0 is not empty (first data at byte {}); another boot loader or a disk tool keeps something there", 512 + i));
            } else {
                method = Some(Method::MbrGap { core_lba: MBR_CORE_LBA, gap_sectors });
            }
            if !f.active_partition {
                warnings.push("no partition is marked active; some old BIOSes refuse to boot such a disk".into());
            }
        }
        Kind::Gpt => match f.bios_boot {
            None => refusals.push("the disk is GPT and has no BIOS boot partition; making one means changing the partition table, which this option does not do".into()),
            Some((start_lba, sectors)) => {
                if sectors < need {
                    refusals.push(format!("the BIOS boot partition has {sectors} sectors and GRUB needs {need}"));
                } else if f.bios_boot_content.iter().take((need * 512) as usize).any(|b| *b != 0) {
                    refusals.push("the BIOS boot partition is not empty; another boot loader uses it".into());
                } else {
                    method = Some(Method::BiosBootPartition { start_lba, sectors });
                }
            }
        },
    }
    let owner = BootCode::classify(&f.sector0);
    match owner {
        BootCode::Empty | BootCode::Grub => {}
        _ if replace_foreign_boot_code => warnings.push(format!("the boot code in sector 0 ({}) will be replaced; it is saved first and GRUB must chainload that system", owner.label())),
        _ => {
            refusals.push(format!("sector 0 holds the boot code of {}; it is replaced only when you ask for that explicitly", owner.label()));
        }
    }
    if !refusals.is_empty() {
        method = None;
    }
    Check { method, refusals, warnings }
}

/// Sector 0 for the shared disk: GRUB's 440 bytes of boot code (with the core image's address filled in) on top of the old sector,
/// whose disk signature, reserved word, partition table and `55 AA` stay exactly as they were.
pub fn merged_sector0(old: &[u8], grub_boot_code: &[u8], core_lba: u64) -> Result<Vec<u8>, String> {
    if old.len() != 512 || old[510..512] != [0x55, 0xAA] {
        return Err("sector 0 is not a valid partition-table sector".into());
    }
    if grub_boot_code.len() < BOOT_CODE_BYTES {
        return Err(format!("GRUB's boot code is {} bytes, expected {BOOT_CODE_BYTES}", grub_boot_code.len()));
    }
    let mut s = old.to_vec();
    s[..BOOT_CODE_BYTES].copy_from_slice(&grub_boot_code[..BOOT_CODE_BYTES]);
    s[BOOT_START_OFFSET..BOOT_START_OFFSET + 8].copy_from_slice(&core_lba.to_le_bytes());
    Ok(s)
}

/// The sectors the write will change: sector 0 and the core image's area. (First sector, sector count) pairs.
pub fn regions_to_save(method: Method, core_bytes: u64) -> Vec<(u64, u64)> {
    vec![(0, 1), (method.core_lba(), sectors_of(core_bytes))]
}

const MAGIC: &[u8; 16] = b"IYAKKI-BIOSSAVE1";
/// A save holds sector 0 and the core image's area; more than this is not a save of ours.
const MAX_REGIONS: usize = 16;

/// Does the area (first sector, byte count) lie inside a disk of `disk_bytes`? (No wrapping: an offset that does not fit is outside.)
fn area_inside(lba: u64, len: u64, sector_size: u32, disk_bytes: u64) -> bool {
    lba.checked_mul(sector_size as u64).and_then(|o| o.checked_add(len)).is_some_and(|end| end <= disk_bytes)
}

/// The saved sectors, with enough to find the disk again and a checksum.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Backup {
    pub disk_bytes: u64,
    pub sector_size: u32,
    /// (first sector, bytes)
    pub regions: Vec<(u64, Vec<u8>)>,
}

impl Backup {
    /// Read the regions from the disk.
    pub fn take<R: Read + Seek>(r: &mut R, sector_size: u32, disk_bytes: u64, regions: &[(u64, u64)]) -> Result<Backup, String> {
        let mut out = Vec::new();
        for &(lba, n) in regions {
            let mut b = vec![0u8; (n * sector_size as u64) as usize];
            r.seek(SeekFrom::Start(lba * sector_size as u64)).and_then(|_| r.read_exact(&mut b)).map_err(|e| format!("saving sectors {lba}..{}: {e}", lba + n))?;
            out.push((lba, b));
        }
        Ok(Backup { disk_bytes, sector_size, regions: out })
    }

    /// `IYAKKI-BIOSSAVE1`, disk bytes, sector size, region count, each region (first sector, byte count, bytes), then the SHA-256 of all before it.
    pub fn encode(&self) -> Vec<u8> {
        let mut v = MAGIC.to_vec();
        v.extend(self.disk_bytes.to_le_bytes());
        v.extend(self.sector_size.to_le_bytes());
        v.extend((self.regions.len() as u32).to_le_bytes());
        for (lba, b) in &self.regions {
            v.extend(lba.to_le_bytes());
            v.extend((b.len() as u64).to_le_bytes());
            v.extend(b);
        }
        let sum = Sha256::digest(&v);
        v.extend(sum);
        v
    }

    pub fn decode(bytes: &[u8]) -> Result<Backup, String> {
        if bytes.len() < MAGIC.len() + 16 + 32 || &bytes[..16] != MAGIC {
            return Err("not a BIOS boot save file".into());
        }
        let (body, sum) = bytes.split_at(bytes.len() - 32);
        if Sha256::digest(body).as_slice() != sum {
            return Err("the save file is damaged (checksum does not match)".into());
        }
        let u64_at = |o: usize| u64::from_le_bytes(body[o..o + 8].try_into().unwrap());
        let (disk_bytes, sector_size) = (u64_at(16), u32::from_le_bytes(body[24..28].try_into().unwrap()));
        let n = u32::from_le_bytes(body[28..32].try_into().unwrap()) as usize;
        // the checksum has no secret in it, so the numbers are checked as well (found by the fuzz test): a real disk's sector size, a few areas,
        // each a whole number of sectors lying inside the disk it was saved from; nothing here can overflow
        if !matches!(sector_size, 512 | 1024 | 2048 | 4096) {
            return Err(format!("the save file names a sector size of {sector_size} bytes, which no disk has"));
        }
        if n > MAX_REGIONS {
            return Err(format!("the save file holds too many areas ({n}; at most {MAX_REGIONS})"));
        }
        let (mut at, mut regions) = (32usize, Vec::new());
        for _ in 0..n {
            if body.len() - at < 16 {
                return Err("the save file is cut short".into());
            }
            let (lba, len) = (u64_at(at), u64_at(at + 8));
            at += 16;
            if len > (body.len() - at) as u64 {
                return Err("the save file is cut short".into());
            }
            let len = len as usize;
            if len == 0 || len as u64 % sector_size as u64 != 0 {
                return Err("a saved area is not a whole number of sectors".into());
            }
            if !area_inside(lba, len as u64, sector_size, disk_bytes) {
                return Err(format!("a saved area (sector {lba}, {len} bytes) lies outside the disk it was saved from ({disk_bytes} bytes)"));
            }
            regions.push((lba, body[at..at + len].to_vec()));
            at += len;
        }
        if at != body.len() {
            return Err("the save file has extra bytes".into());
        }
        Ok(Backup { disk_bytes, sector_size, regions })
    }

    /// Put the saved sectors back. Refuses a different disk (size or sector size) so a save cannot be applied to the wrong one.
    pub fn restore<W: Write + Seek>(&self, w: &mut W, sector_size: u32, disk_bytes: u64) -> Result<(), String> {
        if sector_size != self.sector_size || disk_bytes != self.disk_bytes {
            return Err(format!("this save is from a disk of {} bytes with {}-byte sectors, not this one ({disk_bytes} bytes, {sector_size})", self.disk_bytes, self.sector_size));
        }
        // all areas are checked before the first byte is written
        for (lba, b) in &self.regions {
            if !area_inside(*lba, b.len() as u64, sector_size, disk_bytes) {
                return Err(format!("the saved area at sector {lba} lies outside the disk; nothing was written"));
            }
        }
        for (lba, b) in &self.regions {
            w.seek(SeekFrom::Start(lba * sector_size as u64)).and_then(|_| w.write_all(b)).map_err(|e| format!("restoring sector {lba}: {e}"))?;
        }
        Ok(())
    }
}

/// After the write: read the disk again and report everything that is not as it should be (empty = all good).
/// `before` is sector 0 as it was; `boot_code` and `core` are what was written.
pub fn verify_after<R: Read + Seek>(r: &mut R, before_sector0: &[u8], method: Method, boot_code: &[u8], core: &[u8]) -> Vec<String> {
    let mut problems = Vec::new();
    let mut s0 = vec![0u8; 512];
    if r.seek(SeekFrom::Start(0)).and_then(|_| r.read_exact(&mut s0)).is_err() {
        return vec!["sector 0 cannot be read back".into()];
    }
    if s0[BOOT_CODE_BYTES..] != before_sector0[BOOT_CODE_BYTES..] {
        problems.push("the disk signature or the partition table in sector 0 changed; restore the save at once".into());
    }
    let mut want = boot_code[..BOOT_CODE_BYTES].to_vec();
    want[BOOT_START_OFFSET..BOOT_START_OFFSET + 8].copy_from_slice(&method.core_lba().to_le_bytes());
    if s0[..BOOT_CODE_BYTES] != want[..] {
        problems.push("the boot code in sector 0 is not what was written".into());
    }
    let mut got = vec![0u8; core.len()];
    if r.seek(SeekFrom::Start(method.core_lba() * 512)).and_then(|_| r.read_exact(&mut got)).is_err() {
        problems.push("the core image cannot be read back".into());
    } else if got != core {
        problems.push("the core image on the disk differs from the one written".into());
    }
    problems
}

/// Put a save file's sectors back on `target` (an image file, or a device by its path): the way to undo `--bios`. The save is refused for
/// any disk of another size or sector size; only the saved sectors are written.
pub fn restore_file(save: &std::path::Path, target: &std::path::Path) -> Result<String, String> {
    let backup = Backup::decode(&std::fs::read(save).map_err(|e| format!("{}: {e}", save.display()))?)?;
    let mut f = std::fs::OpenOptions::new().read(true).write(true).open(target).map_err(|e| format!("{}: {e}", target.display()))?;
    let size = f.seek(SeekFrom::End(0)).map_err(|e| format!("{}: {e}", target.display()))?;
    backup.restore(&mut f, backup.sector_size, size)?;
    f.sync_all().map_err(|e| format!("flushing {}: {e}", target.display()))?;
    Ok(format!("put back {} saved area(s) on {}", backup.regions.len(), target.display()))
}

/// The lines for the plan page.
pub fn describe(check: &Check) -> Vec<String> {
    let mut v = Vec::new();
    match check.method {
        Some(Method::MbrGap { core_lba, gap_sectors }) => v.push(format!("Legacy BIOS boot: GRUB goes into the gap behind sector 0 (sector {core_lba}, {gap_sectors} sectors free); the disk's partitions are not touched")),
        Some(Method::BiosBootPartition { start_lba, sectors }) => v.push(format!("Legacy BIOS boot: GRUB goes into the BIOS boot partition (sector {start_lba}, {sectors} sectors)")),
        None => v.push("Legacy BIOS boot: not set up on this disk (UEFI only)".into()),
    }
    if check.ok() {
        v.push("Sector 0 and the area GRUB uses are saved to a file first, so they can be put back".into());
    }
    v.extend(check.refusals.iter().map(|r| format!("  not possible: {r}")));
    v.extend(check.warnings.iter().map(|w| format!("  note: {w}")));
    v
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    const MIB: usize = 1 << 20;

    fn grub_code() -> Vec<u8> {
        let mut c = vec![0x90u8; BOOT_CODE_BYTES];
        c[0x188..0x18D].copy_from_slice(b"GRUB ");
        c[0x190..0x194].copy_from_slice(b"Geom");
        c
    }

    fn windows_code() -> Vec<u8> {
        let mut c = vec![0x33u8; BOOT_CODE_BYTES];
        c[0x100..0x100 + 23].copy_from_slice(b"Invalid partition table");
        c
    }

    /// An MBR disk image of `mib` MiB: one NTFS partition at `start` (sectors), a signature and an active flag.
    fn mbr_disk(mib: usize, start: u32, boot: &[u8], active: bool) -> Vec<u8> {
        let mut d = vec![0u8; mib * MIB];
        d[..boot.len()].copy_from_slice(boot);
        d[440..444].copy_from_slice(&[0xDE, 0xAD, 0xBE, 0xEF]);
        let e = 446;
        d[e] = if active { 0x80 } else { 0 };
        d[e + 4] = 0x07;
        d[e + 8..e + 12].copy_from_slice(&start.to_le_bytes());
        d[e + 12..e + 16].copy_from_slice(&((mib * MIB / 512) as u32 - start).to_le_bytes());
        d[510] = 0x55;
        d[511] = 0xAA;
        let at = start as usize * 512;
        d[at..at + 16].copy_from_slice(b"NTFS partition!!");
        d
    }

    fn facts(d: &[u8]) -> Facts {
        read_facts(&mut Cursor::new(d.to_vec()), 512, d.len() as u64).unwrap()
    }

    const CORE: u64 = 140 * 1024;

    #[test]
    fn an_empty_gap_and_empty_boot_code_are_free_and_the_plan_says_so() {
        let d = mbr_disk(8, 2048, &[], true);
        let c = evaluate(&facts(&d), CORE, false);
        assert_eq!(c.method, Some(Method::MbrGap { core_lba: 1, gap_sectors: 2047 }), "{c:?}");
        assert!(c.refusals.is_empty() && c.warnings.is_empty());
        let text = describe(&c).join("\n");
        assert!(text.contains("gap behind sector 0") && text.contains("saved to a file first"), "{text}");
    }

    #[test]
    fn a_gap_that_is_too_small_or_not_empty_is_refused_with_the_reason() {
        let small = mbr_disk(8, 63, &[], true);
        let c = evaluate(&facts(&small), CORE, false);
        assert!(!c.ok() && c.refusals[0].contains("62 sectors") && c.refusals[0].contains("would have to move"), "{c:?}");
        let mut used = mbr_disk(8, 2048, &[], true);
        used[512 + 100] = 7;
        let c = evaluate(&facts(&used), CORE, false);
        assert!(!c.ok() && c.refusals[0].contains("byte 612"), "{c:?}");
        // data beyond what GRUB needs does not matter
        let mut far = mbr_disk(8, 2048, &[], true);
        far[900 * 1024] = 9;
        assert!(evaluate(&facts(&far), CORE, false).ok());
    }

    #[test]
    fn another_systems_boot_code_is_replaced_only_when_asked_for() {
        let d = mbr_disk(8, 2048, &windows_code(), true);
        assert_eq!(BootCode::classify(&d), BootCode::Windows);
        let c = evaluate(&facts(&d), CORE, false);
        assert!(!c.ok() && c.refusals[0].contains("Windows or DOS"), "{c:?}");
        let c = evaluate(&facts(&d), CORE, true);
        assert!(c.ok() && c.warnings.iter().any(|w| w.contains("saved first") && w.contains("chainload")), "{c:?}");
        let g = mbr_disk(8, 2048, &grub_code(), true);
        assert_eq!(BootCode::classify(&g), BootCode::Grub);
        assert!(evaluate(&facts(&g), CORE, false).ok(), "an old GRUB of ours may be replaced");
        let other = mbr_disk(8, 2048, &[0xEBu8; 100], true);
        assert_eq!(BootCode::classify(&other), BootCode::Other);
    }

    #[test]
    fn no_signature_4k_sectors_and_a_tiny_core_are_refused() {
        let mut d = mbr_disk(8, 2048, &[], true);
        d[510] = 0;
        let c = evaluate(&facts(&d), CORE, false);
        assert!(!c.ok());
        let d = mbr_disk(8, 2048, &[], true);
        let mut f = facts(&d);
        f.sector_size = 4096;
        assert!(evaluate(&f, CORE, false).refusals.iter().any(|r| r.contains("4096-byte sectors")));
        assert!(evaluate(&facts(&d), 100, false).refusals.iter().any(|r| r.contains("too small")));
        let na = mbr_disk(8, 2048, &[], false);
        assert!(evaluate(&facts(&na), CORE, false).warnings.iter().any(|w| w.contains("active")));
    }

    fn gpt_disk(bios_part: bool, dirty: bool) -> Vec<u8> {
        use gpt::*;
        let total = 16 * MIB as u64 / 512;
        let mut t = Table { sector_size: 512, total_sectors: total, disk_guid: guid_from_str("11111111-2222-3333-4444-555555555555").unwrap(), entries: vec![] };
        let mut idx = 1;
        let mut at = 2048;
        if bios_part {
            t.entries.push(Entry { index: idx, type_guid: TYPE_BIOS_BOOT, guid: [1; 16], first_lba: at, last_lba: at + 2047, attrs: 0, name: "BIOS".into() });
            idx += 1;
            at += 2048;
        }
        let last = t.last_usable();
        t.entries.push(Entry { index: idx, type_guid: TYPE_LINUX_DATA, guid: [2; 16], first_lba: at, last_lba: last, attrs: 0, name: "data".into() });
        let mut d = vec![0u8; 16 * MIB];
        d[..512].copy_from_slice(&t.protective_mbr(&[], [1, 2, 3, 4]));
        d[512..1024].copy_from_slice(&t.header_bytes(false));
        let e = t.entries_bytes();
        d[1024..1024 + e.len()].copy_from_slice(&e);
        let bl = t.backup_entries_lba() as usize * 512;
        d[bl..bl + e.len()].copy_from_slice(&e);
        let hl = (total as usize - 1) * 512;
        d[hl..hl + 512].copy_from_slice(&t.header_bytes(true));
        if dirty {
            d[2048 * 512 + 10] = 1;
        }
        d
    }

    #[test]
    fn a_gpt_disk_needs_an_empty_bios_boot_partition() {
        let c = evaluate(&facts(&gpt_disk(true, false)), CORE, false);
        assert_eq!(c.method, Some(Method::BiosBootPartition { start_lba: 2048, sectors: 2048 }), "{c:?}");
        let c = evaluate(&facts(&gpt_disk(false, false)), CORE, false);
        assert!(!c.ok() && c.refusals[0].contains("no BIOS boot partition"), "{c:?}");
        let c = evaluate(&facts(&gpt_disk(true, true)), CORE, false);
        assert!(!c.ok() && c.refusals[0].contains("not empty"), "{c:?}");
    }

    #[test]
    fn the_merged_sector_keeps_the_signature_and_the_partition_table() {
        let d = mbr_disk(8, 2048, &windows_code(), true);
        let old = &d[..512];
        let mut grub = grub_code();
        grub[BOOT_START_OFFSET..BOOT_START_OFFSET + 8].copy_from_slice(&99u64.to_le_bytes());
        let s = merged_sector0(old, &grub, 1).unwrap();
        assert_eq!(&s[440..], &old[440..], "signature, table and 55 AA untouched");
        assert_eq!(u64::from_le_bytes(s[BOOT_START_OFFSET..BOOT_START_OFFSET + 8].try_into().unwrap()), 1);
        assert_eq!(&s[..BOOT_START_OFFSET], &grub[..BOOT_START_OFFSET]);
        assert!(merged_sector0(&old[..511], &grub, 1).is_err() && merged_sector0(old, &grub[..100], 1).is_err());
    }

    #[test]
    fn save_write_verify_and_restore_round_trip_on_an_image() {
        let original = mbr_disk(8, 2048, &windows_code(), true);
        let f = facts(&original);
        let check = evaluate(&f, CORE, true);
        let method = check.method.unwrap();
        let mut disk = Cursor::new(original.clone());
        let regions = regions_to_save(method, CORE);
        assert_eq!(regions, vec![(0, 1), (1, 280)]);
        let save = Backup::take(&mut disk, 512, original.len() as u64, &regions).unwrap();
        // the save survives a trip through bytes
        let save = Backup::decode(&save.encode()).unwrap();
        // write GRUB
        let (boot, core) = (grub_code(), vec![0xC3u8; CORE as usize]);
        let s0 = merged_sector0(&f.sector0, &boot, method.core_lba()).unwrap();
        disk.seek(SeekFrom::Start(0)).unwrap();
        disk.write_all(&s0).unwrap();
        disk.seek(SeekFrom::Start(512)).unwrap();
        disk.write_all(&core).unwrap();
        assert_eq!(verify_after(&mut disk, &f.sector0, method, &boot, &core), Vec::<String>::new());
        assert_ne!(disk.get_ref(), &original);
        assert_eq!(&disk.get_ref()[2048 * 512..2048 * 512 + 16], b"NTFS partition!!", "the other system's partition is untouched");
        // a damaged write is noticed
        let mut bad = disk.clone();
        bad.get_mut()[446] = 0;
        assert!(verify_after(&mut bad, &f.sector0, method, &boot, &core).iter().any(|p| p.contains("partition table")));
        bad.get_mut()[600] ^= 1;
        assert!(verify_after(&mut bad, &f.sector0, method, &boot, &core).iter().any(|p| p.contains("core image")));
        // restore puts every byte back
        save.restore(&mut disk, 512, original.len() as u64).unwrap();
        assert_eq!(disk.get_ref(), &original);
    }

    #[test]
    fn a_save_file_puts_the_old_boot_code_back_on_the_named_image_and_refuses_another_size() {
        let dir = std::env::temp_dir().join(format!("iyakki-biosrestore-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let original = mbr_disk(8, 2048, &windows_code(), true);
        let img = dir.join("d.img");
        std::fs::write(&img, &original).unwrap();
        let save = Backup::take(&mut Cursor::new(original.clone()), 512, original.len() as u64, &[(0, 1), (1, 280)]).unwrap();
        std::fs::write(dir.join("s.bin"), save.encode()).unwrap();
        let mut changed = original.clone();
        changed[..440].copy_from_slice(&grub_code());
        changed[512..512 + 4096].fill(0xC3);
        std::fs::write(&img, &changed).unwrap();
        restore_file(&dir.join("s.bin"), &img).unwrap();
        assert_eq!(std::fs::read(&img).unwrap(), original);
        let small = dir.join("small.img");
        std::fs::write(&small, vec![0u8; 4 << 20]).unwrap();
        assert!(restore_file(&dir.join("s.bin"), &small).unwrap_err().contains("not this one"));
        assert!(restore_file(&dir.join("missing.bin"), &img).is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_save_cannot_be_used_on_another_disk_and_damage_is_caught() {
        let d = mbr_disk(8, 2048, &[], true);
        let save = Backup::take(&mut Cursor::new(d.clone()), 512, d.len() as u64, &[(0, 1)]).unwrap();
        let mut other = Cursor::new(vec![0u8; 4 * MIB]);
        assert!(save.restore(&mut other, 512, 4 * MIB as u64).unwrap_err().contains("not this one"));
        assert!(save.restore(&mut other, 4096, d.len() as u64).is_err());
        let mut bytes = save.encode();
        assert!(Backup::decode(&bytes[..bytes.len() - 5]).is_err());
        bytes[40] ^= 1;
        assert!(Backup::decode(&bytes).unwrap_err().contains("damaged"));
        assert!(Backup::decode(b"short").is_err());
    }

    fn compat_path() -> std::path::PathBuf {
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/compat/alpha3/bios-save-v1.hex")
    }

    fn sample() -> Backup {
        Backup { disk_bytes: 1 << 20, sector_size: 512, regions: vec![(0, vec![0xAA; 512]), (34, vec![0x55; 1024])] }
    }

    /// Writes `tests/compat/alpha3/bios-save-v1.hex` (once, when the release is cut; never edited after).
    #[test]
    #[ignore]
    fn record_alpha3_bios_save() {
        if std::env::var("IYAKKI_COMPAT_RECORD").is_ok() {
            let hex: String = sample().encode().iter().map(|b| format!("{b:02x}")).collect();
            std::fs::write(compat_path(), hex + "\n").unwrap();
        }
    }

    #[test]
    fn the_bios_save_file_of_alpha3_is_still_read_and_other_magic_or_a_flipped_byte_is_refused() {
        let text = std::fs::read_to_string(compat_path()).unwrap();
        let bytes: Vec<u8> = (0..text.trim().len() / 2).map(|i| u8::from_str_radix(&text.trim()[2 * i..2 * i + 2], 16).unwrap()).collect();
        assert_eq!(&bytes[..16], b"IYAKKI-BIOSSAVE1");
        assert_eq!(Backup::decode(&bytes).unwrap(), sample(), "what alpha.3 wrote reads back the same");
        // a newer layout (another magic) is named, never half-read
        let mut newer = bytes.clone();
        newer[15] = b'2';
        assert_eq!(Backup::decode(&newer).unwrap_err(), "not a BIOS boot save file");
        // any flipped byte fails the checksum
        let mut bad = bytes.clone();
        bad[40] ^= 1;
        assert!(Backup::decode(&bad).unwrap_err().contains("damaged"));
        assert!(Backup::decode(&bytes[..bytes.len() - 40]).is_err());
    }
}
