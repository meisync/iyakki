//! Legacy BIOS boot of a shared GPT disk whose sectors are 4096 bytes (F-INST-29): the byte-level handling, with no hardware.
//!
//! GRUB's stock `boot.img` and `diskboot.img` count in 512-byte sectors. On a 4Kn disk the BIOS reads 4096-byte sectors, so three
//! things change, and this module does exactly those and nothing else:
//!
//! 1. the 4096-byte sector 0 holds the 512-byte protective-MBR sector first (disk signature and partition table kept as they were),
//!    the rest of that sector is kept as found;
//! 2. the boot code names the core image's first sector in 4096-byte units (field 0x5C, the same field, a different unit);
//! 3. the core image's first sector is padded to 4096 bytes and its block list (start, length, segment, at offset 500) is rewritten in
//!    4096-byte units: the rest of the core follows at the next 4096-byte sector, and its load segment moves by (4096 - 512) / 16 = 0xE0
//!    paragraphs because the first sector now takes 4096 bytes of memory.
//!
//! The boot CODE itself (16-bit real mode, reading in 4096-byte units) is `dist/boot4k/boot4k.S` (embedded as [`BUILTIN_CODE`], chosen
//! with `--bios-4k-boot-code builtin`) or another build handed in as a file (`--bios-4k-boot-code`). It must carry [`MARKER`] in the last bytes of the 440 bytes of boot code,
//! which says it reads 4096-byte sectors and steps its load segment by 0x100 paragraphs per sector. Without that file, or with stock
//! GRUB code, the planner refuses in plain words and nothing is written (the default of D-4KN-BIOS).

use std::io::{Read, Seek, SeekFrom, Write};

use crate::biosshared::Backup;

/// Bytes in one sector of the disks this module is for.
pub const SECTOR: usize = 4096;
/// The boot code's size inside sector 0.
pub const CODE_BYTES: usize = 440;
/// Marker the 4096-byte-sector boot code carries at its end (bytes 424..440).
pub const MARKER: &[u8; 16] = b"IYAKKI-4K-BOOT\x01\0";
const KERNEL_SECTOR: usize = 0x5C;
const BLOCKLIST: usize = 512 - 12;

/// Our own boot code (source `dist/boot4k/boot4k.S`, assembled by `dist/boot4k/build.sh`): 440 bytes ending in [`MARKER`].
pub const BUILTIN_CODE: &[u8; CODE_BYTES] = include_bytes!("../../../dist/boot4k/boot4k.bin");
/// The file name that stands for [`BUILTIN_CODE`] in `--bios-4k-boot-code`. Nothing uses it unless the person asks for it.
pub const BUILTIN_NAME: &str = "builtin";

/// The boot code for a `--bios-4k-boot-code` value: the built-in code for [`BUILTIN_NAME`], else the named file.
pub fn load_code(name: &str) -> Result<Vec<u8>, String> {
    if name == BUILTIN_NAME {
        return Ok(BUILTIN_CODE.to_vec());
    }
    std::fs::read(name).map_err(|e| format!("--bios-4k-boot-code {name}: {e}"))
}

/// Is this a 4096-byte-sector boot code from our loader build (and not GRUB's stock code)?
pub fn check_boot_code(code: &[u8]) -> Result<(), String> {
    if code.len() < CODE_BYTES {
        return Err(format!("the boot code is {} bytes, expected {CODE_BYTES}", code.len()));
    }
    if &code[CODE_BYTES - MARKER.len()..CODE_BYTES] != MARKER {
        return Err("this boot code does not say it reads 4096-byte sectors (it is GRUB's stock code, which reads 512-byte sectors and would not start); the 4096-byte-sector boot code comes from the iyakki-boot build".into());
    }
    Ok(())
}

/// What is written for one disk.
#[derive(Debug, PartialEq, Eq)]
pub struct Layout {
    /// The whole first sector of the disk (4096 bytes).
    pub sector0: Vec<u8>,
    /// The core image in whole 4096-byte sectors.
    pub core: Vec<u8>,
    /// Where it goes, in 4096-byte sectors from the start of the disk.
    pub core_lba: u64,
}

/// The core image as the 4096-byte-sector boot code reads it. `core` is GRUB's core image as built for 512-byte sectors: its first
/// 512 bytes end in one block list entry that names the rest.
pub fn core_for_4k(core: &[u8], core_lba: u64) -> Result<Vec<u8>, String> {
    if core.len() < 1024 {
        return Err("the core image is too small to be one".into());
    }
    let entry = |at: usize| (u64::from_le_bytes(core[at..at + 8].try_into().unwrap()), u16::from_le_bytes(core[at + 8..at + 10].try_into().unwrap()), u16::from_le_bytes(core[at + 10..at + 12].try_into().unwrap()));
    let (_, len, seg) = entry(BLOCKLIST);
    if len == 0 {
        return Err("the core image has no block list".into());
    }
    if BLOCKLIST >= 12 && entry(BLOCKLIST - 12).1 != 0 {
        return Err("the core image's block list has more than one entry; only one contiguous run can be moved to 4096-byte sectors".into());
    }
    let rest_bytes = len as usize * 512;
    if core.len() < 512 + rest_bytes {
        return Err("the core image is shorter than its block list says".into());
    }
    if seg < 0x20 {
        return Err("the core image's load segment is not after its first sector".into());
    }
    let sectors_4k = rest_bytes.div_ceil(SECTOR);
    let new_len = u16::try_from(sectors_4k).map_err(|_| "the core image is too long for one block list entry".to_string())?;
    let new_seg = u32::from(seg) - 0x20 + 0x100;
    let new_seg = u16::try_from(new_seg).map_err(|_| "the core image would load beyond the first megabyte".to_string())?;
    let mut out = vec![0u8; SECTOR + sectors_4k * SECTOR];
    out[..512].copy_from_slice(&core[..512]);
    out[SECTOR..SECTOR + rest_bytes].copy_from_slice(&core[512..512 + rest_bytes]);
    out[BLOCKLIST..BLOCKLIST + 8].copy_from_slice(&(core_lba + 1).to_le_bytes());
    out[BLOCKLIST + 8..BLOCKLIST + 10].copy_from_slice(&new_len.to_le_bytes());
    out[BLOCKLIST + 10..BLOCKLIST + 12].copy_from_slice(&new_seg.to_le_bytes());
    Ok(out)
}

/// Sector 0 for the shared disk: the old 4096 bytes with the boot code (core address in 4096-byte units) in front of the table.
pub fn sector0_for_4k(old: &[u8], code: &[u8], core_lba: u64) -> Result<Vec<u8>, String> {
    if old.len() != SECTOR {
        return Err(format!("sector 0 is {} bytes, expected {SECTOR}", old.len()));
    }
    check_boot_code(code)?;
    // the 512-byte part follows the usual rule (signature and table kept); the rest of the 4096 bytes stays as it was
    let mut s = old.to_vec();
    let first = crate::biosshared::merged_sector0(&old[..512], &code[..CODE_BYTES], core_lba)?;
    s[..512].copy_from_slice(&first);
    debug_assert_eq!(&s[KERNEL_SECTOR..KERNEL_SECTOR + 8], &core_lba.to_le_bytes());
    Ok(s)
}

/// Everything for one disk, or why not. `part_sectors` is the BIOS boot partition's length in 4096-byte sectors.
pub fn build(old_sector0: &[u8], code: &[u8], core512: &[u8], core_lba: u64, part_sectors: u64) -> Result<Layout, String> {
    let core = core_for_4k(core512, core_lba)?;
    let need = (core.len() / SECTOR) as u64;
    if need > part_sectors {
        return Err(format!("the BIOS boot partition has {part_sectors} sectors of 4096 bytes and GRUB needs {need}"));
    }
    if core_lba == 0 {
        return Err("the core image cannot start at sector 0".into());
    }
    Ok(Layout { sector0: sector0_for_4k(old_sector0, code, core_lba)?, core, core_lba })
}

/// The sectors a write changes, for [`Backup::take`]: sector 0 and the core's area (first sector, count in 4096-byte sectors).
pub fn regions_to_save(core_lba: u64, core_sectors: u64) -> Vec<(u64, u64)> {
    vec![(0, 1), (core_lba, core_sectors)]
}

/// Write order: the core image first, then sector 0, so a power cut leaves the old boot code pointing at nothing new. Reads back and
/// reports what differs (empty = good). The caller took the [`Backup`] first.
pub fn write<F: Read + Write + Seek>(f: &mut F, l: &Layout) -> Result<Vec<String>, String> {
    let put = |f: &mut F, lba: u64, b: &[u8]| f.seek(SeekFrom::Start(lba * SECTOR as u64)).and_then(|_| f.write_all(b)).map_err(|e| format!("writing sector {lba}: {e}"));
    put(f, l.core_lba, &l.core)?;
    f.flush().map_err(|e| e.to_string())?;
    put(f, 0, &l.sector0)?;
    f.flush().map_err(|e| e.to_string())?;
    let mut problems = Vec::new();
    for (lba, want, what) in [(l.core_lba, &l.core, "the core image"), (0, &l.sector0, "sector 0")] {
        let mut got = vec![0u8; want.len()];
        if f.seek(SeekFrom::Start(lba * SECTOR as u64)).and_then(|_| f.read_exact(&mut got)).is_err() {
            problems.push(format!("{what} cannot be read back"));
        } else if &got != want {
            problems.push(format!("{what} on the disk differs from the one written"));
        }
    }
    Ok(problems)
}

/// Undo: put the saved sectors back (the backup was taken with 4096-byte sectors).
pub fn undo<F: Write + Seek>(f: &mut F, save: &Backup, disk_bytes: u64) -> Result<(), String> {
    save.restore(f, SECTOR as u32, disk_bytes)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    fn code() -> Vec<u8> {
        let mut c = vec![0x90u8; CODE_BYTES];
        c[CODE_BYTES - 16..].copy_from_slice(MARKER);
        c
    }
    /// A core image as built for 512-byte sectors: first sector with one block list entry (start 2, 9 sectors, segment 0x820).
    fn core512() -> Vec<u8> {
        let mut c: Vec<u8> = (0..512 * 10).map(|i| (i % 251) as u8 | 1).collect();
        c[500..508].copy_from_slice(&2u64.to_le_bytes());
        c[508..510].copy_from_slice(&9u16.to_le_bytes());
        c[510..512].copy_from_slice(&0x820u16.to_le_bytes());
        c[488..500].fill(0);
        c
    }
    fn old0() -> Vec<u8> {
        let mut s = vec![0u8; SECTOR];
        s[440..444].copy_from_slice(&[9, 8, 7, 6]);
        s[446 + 4] = 0xEE;
        s[510] = 0x55;
        s[511] = 0xAA;
        s[600] = 0x77; // something kept in the rest of the sector
        s
    }

    #[test]
    fn only_boot_code_that_says_it_reads_4096_byte_sectors_is_taken() {
        assert!(check_boot_code(&code()).is_ok());
        let mut stock = code();
        stock[CODE_BYTES - 1] = 0x55;
        assert!(check_boot_code(&stock).unwrap_err().contains("512-byte sectors"));
        assert!(check_boot_code(&code()[..100]).is_err());
    }

    #[test]
    fn the_core_moves_to_4096_byte_units_with_its_block_list_and_segment_rewritten() {
        let c = core512();
        let out = core_for_4k(&c, 300).unwrap();
        // 9 * 512 = 4608 bytes of rest = 2 sectors of 4096; first sector 4096 bytes
        assert_eq!(out.len(), SECTOR * 3);
        assert_eq!(u64::from_le_bytes(out[500..508].try_into().unwrap()), 301);
        assert_eq!(u16::from_le_bytes(out[508..510].try_into().unwrap()), 2);
        assert_eq!(u16::from_le_bytes(out[510..512].try_into().unwrap()), 0x900);
        assert_eq!(&out[..500], &c[..500], "the diskboot code is untouched");
        assert!(out[512..SECTOR].iter().all(|b| *b == 0), "first sector padded with zeros");
        assert_eq!(&out[SECTOR..SECTOR + 4608], &c[512..512 + 4608]);
        assert!(out[SECTOR + 4608..].iter().all(|b| *b == 0));
    }

    #[test]
    fn a_core_that_does_not_fit_the_one_entry_shape_is_refused() {
        let mut c = core512();
        c[488..500].copy_from_slice(&[1; 12]); // a second entry
        assert!(core_for_4k(&c, 10).unwrap_err().contains("more than one entry"));
        let mut c = core512();
        c[508..510].copy_from_slice(&0u16.to_le_bytes());
        assert!(core_for_4k(&c, 10).is_err());
        let mut c = core512();
        c[508..510].copy_from_slice(&50u16.to_le_bytes());
        assert!(core_for_4k(&c, 10).unwrap_err().contains("shorter"));
        assert!(core_for_4k(&[0; 100], 10).is_err());
    }

    #[test]
    fn sector_zero_keeps_the_table_and_the_rest_and_names_the_core_in_4096_units() {
        let s = sector0_for_4k(&old0(), &code(), 300).unwrap();
        assert_eq!(s.len(), SECTOR);
        assert_eq!(&s[440..512], &old0()[440..512]);
        assert_eq!(s[600], 0x77);
        assert_eq!(u64::from_le_bytes(s[0x5C..0x64].try_into().unwrap()), 300);
        assert_eq!(&s[..0x5C], &code()[..0x5C]);
        assert!(sector0_for_4k(&old0()[..512], &code(), 300).is_err());
        let mut bad = old0();
        bad[511] = 0;
        assert!(sector0_for_4k(&bad, &code(), 300).is_err(), "no partition table signature");
    }

    #[test]
    fn a_partition_too_small_or_a_start_at_zero_is_refused() {
        assert!(build(&old0(), &code(), &core512(), 300, 2).unwrap_err().contains("needs 3"));
        assert!(build(&old0(), &code(), &core512(), 0, 10).is_err());
        assert!(build(&old0(), &code(), &core512(), 300, 3).is_ok());
    }

    #[test]
    fn written_to_a_byte_image_read_back_and_undone() {
        let mut disk = vec![0u8; SECTOR * 400];
        disk[..SECTOR].copy_from_slice(&old0());
        for (i, b) in disk[SECTOR * 300..SECTOR * 304].iter_mut().enumerate() {
            *b = (i % 7) as u8; // junk in the BIOS boot partition before
        }
        let before = disk.clone();
        let l = build(&old0(), &code(), &core512(), 300, 4).unwrap();
        let mut f = Cursor::new(disk);
        let save = Backup::take(&mut f, SECTOR as u32, (SECTOR * 400) as u64, &regions_to_save(300, 4)).unwrap();
        assert!(write(&mut f, &l).unwrap().is_empty());
        let after = f.get_ref().clone();
        assert_eq!(&after[..SECTOR], &l.sector0[..]);
        assert_eq!(&after[SECTOR * 300..SECTOR * 303], &l.core[..]);
        assert_eq!(&after[SECTOR * 1..SECTOR * 300], &before[SECTOR..SECTOR * 300], "nothing else changed");
        undo(&mut f, &save, (SECTOR * 400) as u64).unwrap();
        assert_eq!(f.get_ref(), &before);
    }
}
