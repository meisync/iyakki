//! Legacy-BIOS boot of a shared GPT disk, the missing pieces of F-INST-13 (planner side, no device): making the BIOS boot partition
//! when the disk has none, writing both copies of the table in the safe order, and the GRUB entry that hands over to the other system
//! (chainload). `biosshared` only refused a GPT disk without that partition; this module decides whether one can be made in free space
//! and builds the new table. Nothing is written without the saved sectors of [`table_regions`] being taken first (`Backup::take`).

use std::io::{Seek, SeekFrom, Write};

use iyakki_storage::gpt::{self, Entry, Table, ENTRY_COUNT};

use crate::biosshared::BootCode;

/// Size of the new partition: 1 MiB, the usual size (GRUB's core image is about 140 KB) and the usual alignment.
pub const NEW_BIOS_PART_BYTES: u64 = 1 << 20;
/// Name written into the new entry.
pub const NEW_BIOS_PART_NAME: &str = "IYAKKI-BIOS";
/// GRUB's BIOS code reads its core image with 32-bit sector numbers in the worst case, so the partition must start below this sector.
const MAX_START_LBA: u64 = 1 << 32;

/// What making the partition would do.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NewBiosPart {
    pub entry: Entry,
    pub table_after: Table,
    /// Plain lines for the plan screen.
    pub lines: Vec<String>,
}

/// Free extents (first, last) of the usable area, in LBA order.
fn free_extents(t: &Table) -> Vec<(u64, u64)> {
    let mut used: Vec<(u64, u64)> = t.entries.iter().map(|e| (e.first_lba, e.last_lba)).collect();
    used.sort_unstable();
    let mut out = Vec::new();
    let mut at = t.first_usable();
    for (a, b) in used {
        if a > at {
            out.push((at, a - 1));
        }
        at = at.max(b.saturating_add(1));
    }
    if at <= t.last_usable() {
        out.push((at, t.last_usable()));
    }
    out
}

/// Can a BIOS boot partition be added in free space? Never moves, shrinks or touches an existing partition. `guid` is the new
/// partition's unique id (the caller supplies it so a test is repeatable).
pub fn plan_new_bios_boot(t: &Table, guid: [u8; 16]) -> Result<NewBiosPart, String> {
    if t.sector_size != 512 {
        return Err(format!("this disk has {}-byte sectors; legacy BIOS boot needs 512-byte sectors, such PCs start through UEFI", t.sector_size));
    }
    t.check().map_err(|e| format!("the partition table is not sound ({e}); it is not changed"))?;
    if t.entries.iter().any(|e| e.type_guid == gpt::TYPE_BIOS_BOOT) {
        return Err("the disk already has a BIOS boot partition; use it instead of making another".into());
    }
    let index = (1..=ENTRY_COUNT).find(|i| t.entries.iter().all(|e| e.index != *i)).ok_or("all 128 partition slots are used")?;
    let need = NEW_BIOS_PART_BYTES / 512;
    // 1 MiB aligned start inside a free extent; the lowest one wins (the first MiB before a first partition is the usual place)
    let spot = free_extents(t).into_iter().find_map(|(a, b)| {
        let start = a.div_ceil(need) * need;
        (start.checked_add(need - 1)? <= b && start < MAX_START_LBA).then_some(start)
    });
    let Some(start) = spot else {
        return Err("there is no free 1 MiB between the partitions for a BIOS boot partition; the other system's partitions would have to move or shrink, which this option never does".into());
    };
    let entry = Entry { index, type_guid: gpt::TYPE_BIOS_BOOT, guid, first_lba: start, last_lba: start + need - 1, attrs: 0, name: NEW_BIOS_PART_NAME.into() };
    let mut table_after = t.clone();
    table_after.entries.push(entry.clone());
    table_after.entries.sort_by_key(|e| e.index);
    table_after.check().map_err(|e| format!("the new table would not be sound ({e}); nothing is changed"))?;
    let lines = vec![
        format!("A BIOS boot partition of 1 MiB is added as partition {index} at sectors {start}-{} (free space; no partition moves or shrinks).", entry.last_lba),
        "Both copies of the partition table are rewritten; the old table is saved to the save file first and can be put back.".into(),
    ];
    Ok(NewBiosPart { entry, table_after, lines })
}

/// The (first sector, sector count) pairs a table change touches, for `Backup::take`: the primary header and entries, the backup
/// entries and header.
pub fn table_regions(t: &Table) -> Vec<(u64, u64)> {
    vec![(1, 1), (t.primary_entries_lba(), t.array_sectors()), (t.backup_entries_lba(), t.array_sectors()), (t.last_lba(), 1)]
}

/// Write the table: the backup copy first, then the primary entries, then the primary header, so a power cut leaves one good copy.
pub fn write_table<W: Write + Seek>(w: &mut W, t: &Table) -> Result<(), String> {
    let ss = u64::from(t.sector_size);
    let (entries, hb, hp) = (t.entries_bytes(), t.header_bytes(true), t.header_bytes(false));
    let mut put = |lba: u64, b: &[u8]| w.seek(SeekFrom::Start(lba * ss)).and_then(|_| w.write_all(b)).map_err(|e| format!("writing the partition table at sector {lba}: {e}"));
    put(t.backup_entries_lba(), &entries)?;
    put(t.last_lba(), &hb)?;
    put(t.primary_entries_lba(), &entries)?;
    put(1, &hp)
}

/// How the other system is handed over to.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Chainload {
    /// Hand over to the boot sector of a partition (Windows and most others): `chainloader +1`.
    Partition { uuid: String },
    /// Hand over to the saved boot code of the whole disk (sector 0 as it was).
    DiskBootCode,
}

/// A `grub.cfg` entry that starts the other system through GRUB, built only from characters that are safe in a grub script.
pub fn chainload_entry(title: &str, how: &Chainload) -> Result<String, String> {
    let safe = |s: &str| !s.is_empty() && s.len() <= 64 && s.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | ' ' | '.'));
    if !safe(title) {
        return Err("the entry title may only hold letters, digits, space, dash, underscore and dot".into());
    }
    let body = match how {
        Chainload::Partition { uuid } => {
            if !uuid.chars().all(|c| c.is_ascii_hexdigit() || c == '-') || uuid.len() < 8 || uuid.len() > 36 {
                return Err("the partition id is not a file system UUID or volume serial".into());
            }
            format!("    insmod part_msdos\n    insmod part_gpt\n    insmod ntfs\n    insmod chain\n    search --no-floppy --fs-uuid --set=root {uuid}\n    chainloader +1\n")
        }
        Chainload::DiskBootCode => "    insmod chain\n    set root=(hd1)\n    chainloader +1\n".to_string(),
    };
    Ok(format!("menuentry \"{title}\" {{\n{body}}}\n"))
}

/// The entry a shared install adds for the system whose boot code was replaced, or why none can be made.
pub fn entry_for_replaced_boot_code(old: &BootCode, windows_uuid: Option<&str>) -> Result<Option<String>, String> {
    match old {
        BootCode::Empty | BootCode::Grub => Ok(None),
        BootCode::Windows => match windows_uuid {
            Some(u) => chainload_entry("Windows", &Chainload::Partition { uuid: u.to_string() }).map(Some),
            None => Err("Windows boot code is being replaced but its partition could not be identified; without an entry to start it, it would be unreachable".into()),
        },
        BootCode::Other => Err("another boot loader's code is being replaced and IYAKKI does not know how to hand over to it; keep its boot code (do not replace it)".into()),
    }
}

/// The file-system UUID GRUB's `search --fs-uuid` prints for an NTFS volume: the 64-bit volume serial of its boot sector (offset 0x48,
/// little endian) as 16 lower-case hex digits. `None` when the sector is not an NTFS boot sector.
pub fn ntfs_uuid(boot_sector: &[u8]) -> Option<String> {
    if boot_sector.len() < 512 || &boot_sector[3..11] != b"NTFS    " || boot_sector[510] != 0x55 || boot_sector[511] != 0xAA {
        return None;
    }
    let mut serial = [0u8; 8];
    serial.copy_from_slice(&boot_sector[0x48..0x50]);
    Some(format!("{:016x}", u64::from_le_bytes(serial)))
}

/// The menu text to add to the boot menu of a shared legacy-BIOS install: the entry that starts the system whose boot code is replaced.
/// `first_sectors` gives the first 512 bytes of every NTFS partition (the first one that is an NTFS boot sector is the Windows volume).
/// `Ok(None)` when no boot code is replaced; an error says why a replacement would leave the other system unreachable.
pub fn menu_for_install(replace: bool, old: &BootCode, first_sectors: &[Vec<u8>]) -> Result<Option<String>, String> {
    if !replace {
        return Ok(None);
    }
    let uuid = first_sectors.iter().find_map(|s| ntfs_uuid(s));
    entry_for_replaced_boot_code(old, uuid.as_deref())
}
