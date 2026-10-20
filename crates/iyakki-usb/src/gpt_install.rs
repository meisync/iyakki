//! The GPT flavour of the install: choosing the partition table, the layout, the GRUB pieces, and writing and checking it all.
//!
//! The installed disk looks like this (sector numbers are in the disk's own sectors, partitions start on 1 MiB marks):
//!
//! | Part | Content |
//! |---|---|
//! | sector 0 | protective MBR with GRUB's boot code (legacy BIOS starts here) |
//! | sector 1, 2.. | primary GPT header and entry array |
//! | partition 1, 1 MiB, BIOS boot | GRUB's core image (legacy BIOS reads it from here) |
//! | partition 2, FAT32 `IYAKKI-BOOT`, EFI System | `EFI/BOOT/BOOTX64.EFI`, `boot/grub/grub.cfg`, kernel, initial RAM disk |
//! | partition 3, the rest, `IYAKKI-DATA` | marker "format me" until the first boot makes it ext4 |
//! | last sectors | backup GPT entry array and header, at the true end of the disk |
//!
//! The boot assets are the install template's own: its boot sector's code, the GRUB core image from the gap before partition 1
//! (that core image already contains `part_gpt` and `biosdisk`, see `BR2_TARGET_GRUB2_BUILTIN_MODULES_PC`), and the FAT volume.
//! Only two numbers in them depend on where they sit and are patched: the core image's own sector in the boot code, and the
//! block list in the core image's first sector.

use std::io::{Read, Seek, SeekFrom};
use std::os::unix::fs::FileExt;
use std::time::Instant;

use iyakki_storage::gpt::{self, Entry, Guid, Table};
use sha2::{Digest, Sha256};

use crate::fatclone;
use crate::install::{InstallReport, Plan, SourceInfo, MBR_LIMIT, MIB};
use crate::ui::human_size;
use crate::writer::{self, InstallLayout, Phase, Progress, BLOCK};

/// Size of the BIOS boot partition: GRUB's core image is 140 KB; 1 MiB is the usual size and the partition alignment.
pub const BIOS_BOOT_BYTES: u64 = MIB;
/// Boot partition size when the volume has to be rebuilt for 4096-byte sectors (FAT32 needs 65525 clusters, which is
/// 256 MiB of 4 KiB clusters plus the FATs).
pub const ESP_4K_BYTES: u64 = 320 * MIB;
/// The smallest data partition worth creating.
pub const MIN_DATA_BYTES: u64 = 64 * MIB;

pub const NAME_BIOS: &str = "IYAKKI-BIOS";
pub const NAME_BOOT: &str = "IYAKKI-BOOT";
pub const NAME_DATA: &str = "IYAKKI-DATA";

/// Which partition table to put on the disk.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TableChoice {
    Auto,
    Gpt,
    Mbr,
}

impl TableChoice {
    pub fn parse(s: &str) -> Result<TableChoice, String> {
        match s.trim().to_ascii_lowercase().as_str() {
            "auto" | "" => Ok(TableChoice::Auto),
            "gpt" => Ok(TableChoice::Gpt),
            "mbr" | "msdos" | "dos" => Ok(TableChoice::Mbr),
            other => Err(format!("partition table \"{other}\": use auto, gpt or mbr")),
        }
    }
    pub fn label(self) -> &'static str {
        match self {
            TableChoice::Auto => "Auto",
            TableChoice::Gpt => "GPT",
            TableChoice::Mbr => "MBR",
        }
    }
    /// Auto, then GPT, then MBR, then Auto again (the install page's button).
    pub fn next(self) -> TableChoice {
        match self {
            TableChoice::Auto => TableChoice::Gpt,
            TableChoice::Gpt => TableChoice::Mbr,
            TableChoice::Mbr => TableChoice::Auto,
        }
    }
    /// One line saying what the choice means.
    pub fn explain(self) -> &'static str {
        match self {
            TableChoice::Auto => "Auto: GPT for disks over 2 TiB, disks with 4096-byte sectors and PCs started through UEFI; MBR otherwise, which any old BIOS-only PC boots.",
            TableChoice::Gpt => "GPT: boots on legacy BIOS (BIOS boot partition) and on UEFI (EFI System Partition); needed above 2 TiB and for 4096-byte sectors.",
            TableChoice::Mbr => "MBR: the classic table; the data partition is limited to 2 TiB and the disk needs 512-byte sectors.",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TableKind {
    Mbr,
    Gpt,
}

impl TableKind {
    pub fn label(self) -> &'static str {
        match self {
            TableKind::Mbr => "MBR",
            TableKind::Gpt => "GPT",
        }
    }
}

/// Is this machine running under UEFI firmware?
pub fn booted_uefi() -> bool {
    std::path::Path::new("/sys/firmware/efi").exists()
}

/// The table a disk gets and why. `uefi`: the machine doing the install was started through UEFI.
pub fn resolve_table(choice: TableChoice, size_bytes: u64, sector: u32, uefi: bool) -> Result<(TableKind, String), String> {
    if !matches!(sector, 512 | 4096) {
        return Err(format!("{sector}-byte logical sectors are not supported (512 and 4096 are)"));
    }
    match choice {
        TableChoice::Gpt => Ok((TableKind::Gpt, "chosen: GPT".into())),
        TableChoice::Mbr if sector != 512 => Err(format!("MBR needs 512-byte sectors; this disk has {sector}-byte sectors (4Kn). Choose GPT or Auto.")),
        TableChoice::Mbr => Ok((TableKind::Mbr, "chosen: MBR".into())),
        TableChoice::Auto if size_bytes > MBR_LIMIT => Ok((TableKind::Gpt, "auto: the disk is larger than 2 TiB".into())),
        TableChoice::Auto if sector == 4096 => Ok((TableKind::Gpt, "auto: the disk has 4096-byte sectors".into())),
        TableChoice::Auto if uefi => Ok((TableKind::Gpt, "auto: this computer was started through UEFI".into())),
        TableChoice::Auto => Ok((TableKind::Mbr, "auto: MBR keeps old BIOS-only PCs working".into())),
    }
}

/// Where the three partitions go (sector numbers in sectors of `sector_size`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GptLayout {
    pub sector_size: u32,
    pub total_sectors: u64,
    /// (first sector, sectors)
    pub bios: (u64, u64),
    pub esp: (u64, u64),
    pub data: (u64, u64),
    /// The boot volume is rebuilt for this sector size instead of copied.
    pub esp_rebuilt: bool,
}

impl GptLayout {
    /// `source_esp_bytes`: the boot volume of the template (copied as it is when the disk has 512-byte sectors).
    pub fn new(size_bytes: u64, sector_size: u32, source_esp_bytes: u64) -> Result<GptLayout, String> {
        if !matches!(sector_size, 512 | 4096) {
            return Err(format!("{sector_size}-byte logical sectors are not supported (512 and 4096 are)"));
        }
        let ss = sector_size as u64;
        let (total, al) = (size_bytes / ss, MIB / ss);
        let rebuilt = sector_size != 512;
        let esp_bytes = if rebuilt { ESP_4K_BYTES } else { source_esp_bytes };
        let esp_secs = esp_bytes.div_ceil(MIB) * al;
        let array = gpt::ENTRY_ARRAY_BYTES.div_ceil(ss);
        if total < 2 * al + esp_secs + 2 * array + 8 {
            return Err("this disk is too small".into());
        }
        let last_usable = total - 1 - array - 1;
        let data_first = 2 * al + esp_secs;
        let end = (last_usable + 1) / al * al;
        if end <= data_first || (end - data_first) * ss < MIN_DATA_BYTES {
            return Err("this disk is too small".into());
        }
        Ok(GptLayout { sector_size, total_sectors: total, bios: (al, al), esp: (2 * al, esp_secs), data: (data_first, end - data_first), esp_rebuilt: rebuilt })
    }

    pub fn bytes(&self, sectors: u64) -> u64 {
        sectors * self.sector_size as u64
    }

    /// The table for these GUIDs (disk, then one per partition).
    pub fn table(&self, disk: Guid, parts: [Guid; 3]) -> Table {
        let mk = |index: u32, type_guid: Guid, guid: Guid, span: (u64, u64), name: &str| Entry { index, type_guid, guid, first_lba: span.0, last_lba: span.0 + span.1 - 1, attrs: 0, name: name.into() };
        Table {
            sector_size: self.sector_size,
            total_sectors: self.total_sectors,
            disk_guid: disk,
            entries: vec![
                mk(1, gpt::TYPE_BIOS_BOOT, parts[0], self.bios, NAME_BIOS),
                mk(2, gpt::TYPE_ESP, parts[1], self.esp, NAME_BOOT),
                mk(3, gpt::TYPE_LINUX_DATA, parts[2], self.data, NAME_DATA),
            ],
        }
    }

    /// What the install page and the terminal show.
    pub fn lines(&self) -> Vec<String> {
        let bios_note = if self.sector_size == 512 { "GRUB for legacy BIOS PCs" } else { "kept free: PCs with 4096-byte sectors start through UEFI" };
        vec![
            format!("1  BIOS boot    {:>9}  {bios_note}", human_size(self.bytes(self.bios.1))),
            format!("2  {NAME_BOOT}  {:>9}  FAT32, EFI System Partition: UEFI loader, GRUB, kernel, initial RAM disk{}", human_size(self.bytes(self.esp.1)), if self.esp_rebuilt { " (rebuilt for 4096-byte sectors)" } else { "" }),
            format!("3  {NAME_DATA}  {:>9}  formatted as ext4 on the first start", human_size(self.bytes(self.data.1))),
        ]
    }

    /// The same numbers in 512-byte sectors, for code that thinks in them.
    pub fn data_layout(&self) -> InstallLayout {
        let k = self.sector_size as u64 / 512;
        InstallLayout { data_start: self.data.0 * k, data_sectors: self.data.1 * k }
    }
}

/// GRUB's two pieces patched for sitting at `core_lba` (in 512-byte units): the first 440 bytes of the boot sector and the
/// core image (the gap before the template's first partition, padded).
#[derive(Debug)]
pub struct GrubBits {
    pub boot_code: Vec<u8>,
    pub core: Vec<u8>,
}

const BOOT_START_OFFSET: usize = 0x5C; // GRUB_BOOT_MACHINE_KERNEL_SECTOR: where boot.img looks for the core image (8 bytes)
const BLOCKLIST_OFFSET: usize = 512 - 12; // the core image's first sector ends with {start: u64, len: u16, segment: u16}

pub fn grub_bits(source: &SourceInfo, core_lba: u64, part_bytes: u64) -> Result<GrubBits, String> {
    let gap = source.p1_start.saturating_sub(1) * 512;
    if gap < 4096 || gap > part_bytes {
        return Err(format!("the template's GRUB gap ({gap} bytes) does not fit the BIOS boot partition ({part_bytes} bytes)"));
    }
    let mut f = std::fs::File::open(&source.path).map_err(|e| format!("{}: {e}", source.path.display()))?;
    let mut core = vec![0u8; part_bytes as usize];
    f.seek(SeekFrom::Start(512)).and_then(|_| f.read_exact(&mut core[..gap as usize])).map_err(|e| format!("reading GRUB from {}: {e}", source.path.display()))?;
    let mut code = source.sector0[..440].to_vec();
    let boot_start = u64::from_le_bytes(code[BOOT_START_OFFSET..BOOT_START_OFFSET + 8].try_into().unwrap());
    let list_start = u64::from_le_bytes(core[BLOCKLIST_OFFSET..BLOCKLIST_OFFSET + 8].try_into().unwrap());
    let list_len = u16::from_le_bytes(core[BLOCKLIST_OFFSET + 8..BLOCKLIST_OFFSET + 10].try_into().unwrap());
    if boot_start != 1 || list_start != 2 || list_len == 0 || list_len as u64 * 512 > gap {
        return Err(format!("the template's GRUB is laid out differently than expected (boot sector points to sector {boot_start}, block list {list_start}+{list_len}); rebuild the image"));
    }
    code[BOOT_START_OFFSET..BOOT_START_OFFSET + 8].copy_from_slice(&core_lba.to_le_bytes());
    core[BLOCKLIST_OFFSET..BLOCKLIST_OFFSET + 8].copy_from_slice(&(core_lba + 1).to_le_bytes());
    Ok(GrubBits { boot_code: code, core })
}

fn hex(d: &[u8]) -> String {
    d.iter().map(|b| format!("{b:02x}")).collect()
}

struct Region {
    off: u64,
    len: u64,
    sha: String,
}

fn sha_of(b: &[u8]) -> String {
    hex(&Sha256::digest(b))
}

/// Put `data` at `off`.
fn put(dst: &std::fs::File, d: &str, off: u64, data: &[u8]) -> Result<(), String> {
    dst.write_all_at(data, off).map_err(|e| format!("writing {d}: {e}\nThe disk may be half-written; do not boot it."))
}

/// Write the GPT install. Order: the old table is made unreadable, the data marker, the end of the disk is wiped, the backup
/// table, the boot partitions, then the primary entries, the protective MBR and, last of all, the primary header. Then every
/// piece is read back (direct I/O) and compared, and both GPT copies are parsed and their checksums checked.
pub(crate) fn execute(plan: &Plan, progress: &mut dyn FnMut(Progress)) -> Result<InstallReport, String> {
    let d = &plan.target.drive;
    let g = plan.gpt.as_ref().ok_or("internal error: no GPT layout in the plan")?;
    let ss = g.sector_size as u64;
    let t0 = Instant::now();
    let table = g.table(gpt::random_guid(), [gpt::random_guid(), gpt::random_guid(), gpt::random_guid()]);
    table.check()?;
    let bits = if g.sector_size == 512 { Some(grub_bits(&plan.source, g.bios.0 * ss / 512, g.bytes(g.bios.1))?) } else { None };
    let mut src = std::fs::File::open(&plan.source.path).map_err(|e| format!("{}: {e}", plan.source.path.display()))?;
    let mut sig = [0u8; 4];
    sig.copy_from_slice(&gpt::random_guid()[..4]);
    let mbr = table.protective_mbr(bits.as_ref().map_or(&[][..], |b| &b.boot_code[..]), sig);
    let (hdr_p, hdr_b, entries) = (table.header_bytes(false), table.header_bytes(true), table.entries_bytes());
    let esp_src_bytes = plan.source.p1_sectors * 512;
    let esp_bytes = if g.esp_rebuilt { g.bytes(g.esp.1) } else { esp_src_bytes };
    let total_work = 3 * MIB + BIOS_BOOT_BYTES + esp_bytes;
    let mut done = 0u64;
    let mut say = |phase: Phase, add: u64, done: &mut u64| {
        *done += add;
        progress(Progress { phase, done: (*done).min(total_work), total: total_work, elapsed: t0.elapsed() });
    };

    let dst = std::fs::OpenOptions::new().write(true).open(&d.path).map_err(|e| format!("{}: {e}", d.path))?;
    let mut regions: Vec<Region> = Vec::new();
    // 0. the old tables stop being tables (a half-finished install must not look like the old system)
    put(&dst, &d.path, 0, &vec![0u8; 2 * ss as usize])?;
    // 1. the data partition's marker and the settings
    let mut marker = vec![0u8; MIB as usize];
    marker[..iyakki_storage::PENDING_MARKER.len()].copy_from_slice(iyakki_storage::PENDING_MARKER);
    let conf = iyakki_storage::pending_conf_bytes(&plan.choices.conf_text())?;
    let off = iyakki_storage::PENDING_CONF_OFFSET as usize;
    marker[off..off + conf.len()].copy_from_slice(&conf);
    let data_off = g.data.0 * ss;
    put(&dst, &d.path, data_off, &marker)?;
    regions.push(Region { off: data_off, len: 8192, sha: sha_of(&marker[..8192]) });
    say(Phase::Writing, MIB, &mut done);
    // 2. the end of the disk: whatever was there (an old backup table) goes
    let tail = d.size_bytes.min(MIB);
    put(&dst, &d.path, d.size_bytes - tail, &vec![0u8; tail as usize])?;
    say(Phase::Writing, MIB, &mut done);
    // 3. the backup table: entries first, then its header in the very last sector
    let (b_ents, b_hdr) = (table.backup_entries_lba() * ss, table.last_lba() * ss);
    put(&dst, &d.path, b_ents, &entries)?;
    put(&dst, &d.path, b_hdr, &hdr_b)?;
    regions.push(Region { off: b_ents, len: entries.len() as u64, sha: sha_of(&entries) });
    regions.push(Region { off: b_hdr, len: ss, sha: sha_of(&hdr_b) });
    say(Phase::Writing, MIB, &mut done);
    // 4. the BIOS boot partition (GRUB's core image; blank on 4096-byte-sector disks) and the boot volume
    let core = bits.as_ref().map_or_else(|| vec![0u8; BIOS_BOOT_BYTES as usize], |b| b.core.clone());
    put(&dst, &d.path, g.bios.0 * ss, &core)?;
    regions.push(Region { off: g.bios.0 * ss, len: core.len() as u64, sha: sha_of(&core) });
    say(Phase::Writing, BIOS_BOOT_BYTES, &mut done);
    let esp_off = g.esp.0 * ss;
    let mut h = Sha256::new();
    let mut pos = 0u64;
    let mut since_flush = 0u64;
    let mut sink = |b: &[u8], done: &mut u64| -> Result<(), String> {
        h.update(b);
        put(&dst, &d.path, esp_off + pos, b)?;
        pos += b.len() as u64;
        since_flush += b.len() as u64;
        if since_flush >= 8 * MIB {
            dst.sync_data().map_err(|e| format!("flushing {}: {e}", d.path))?;
            since_flush = 0;
        }
        say(Phase::Writing, b.len() as u64, done);
        Ok(())
    };
    if g.esp_rebuilt {
        let mut input = std::fs::File::open(&plan.source.path).map_err(|e| e.to_string())?;
        let mut vol = fatclone::Fat32::open(&mut input, plan.source.p1_start * 512)?;
        let mut buf: Vec<u8> = Vec::with_capacity(4 * MIB as usize);
        let id = u32::from_le_bytes(sig) ^ 0x4959_414B;
        fatclone::clone_to(&mut vol, g.sector_size, esp_bytes, NAME_BOOT, id, &mut |b| {
            buf.extend_from_slice(b);
            if buf.len() >= 4 * MIB as usize {
                sink(&buf, &mut done)?;
                buf.clear();
            }
            Ok(())
        })?;
        sink(&buf, &mut done)?;
    } else {
        src.seek(SeekFrom::Start(plan.source.p1_start * 512)).map_err(|e| e.to_string())?;
        let mut buf = vec![0u8; BLOCK];
        let mut left = esp_bytes;
        while left > 0 {
            let want = (left as usize).min(BLOCK);
            src.read_exact(&mut buf[..want]).map_err(|e| format!("reading {}: {e}", plan.source.path.display()))?;
            sink(&buf[..want], &mut done)?;
            left -= want as u64;
        }
    }
    let esp_sha = hex(&h.finalize());
    regions.push(Region { off: esp_off, len: esp_bytes, sha: esp_sha });
    dst.sync_all().map_err(|e| format!("flushing {}: {e}", d.path))?;
    // 5. the primary table, last: entries, protective MBR (with GRUB's boot code), and the header
    put(&dst, &d.path, 2 * ss, &entries)?;
    put(&dst, &d.path, 0, &mbr)?;
    dst.sync_all().map_err(|e| format!("flushing {}: {e}", d.path))?;
    put(&dst, &d.path, ss, &hdr_p)?;
    regions.push(Region { off: 2 * ss, len: entries.len() as u64, sha: sha_of(&entries) });
    regions.push(Region { off: 0, len: ss, sha: sha_of(&mbr) });
    regions.push(Region { off: ss, len: ss, sha: sha_of(&hdr_p) });
    progress(Progress { phase: Phase::Syncing, done: total_work, total: total_work, elapsed: t0.elapsed() });
    dst.sync_all().map_err(|e| format!("flushing {}: {e}", d.path))?;
    drop(dst);
    // 6. read everything back from the disk and compare
    let mut all = Sha256::new();
    let mut verified = 0u64;
    let total_verify: u64 = regions.iter().map(|r| r.len).sum();
    for r in &regions {
        let base = verified;
        writer::verify_range(std::path::Path::new(&d.path), r.off, r.len, &r.sha, &mut |p| progress(Progress { done: base + p.done, total: total_verify, ..p })).map_err(|e| format!("{e}\nDo not boot this disk."))?;
        verified += r.len;
        all.update(r.sha.as_bytes());
    }
    // 7. both GPT copies, parsed from the disk the way a reader would: signatures, CRC32s, locations, no overlaps
    let mut back = std::fs::File::open(&d.path).map_err(|e| e.to_string())?;
    let parsed = gpt::read_gpt(&mut back, g.sector_size, g.total_sectors).map_err(|e| format!("verification FAILED: the partition table on the disk is not valid ({e}). Do not boot this disk."))?;
    if !parsed.primary_ok || !parsed.backup_ok || !parsed.problems.is_empty() || parsed.table != table {
        return Err(format!("verification FAILED: the two copies of the partition table are not as written ({}). Do not boot this disk.", parsed.problems.join("; ")));
    }
    drop(back);
    crate::drives::reread_partitions(&d.path);
    Ok(InstallReport { written: total_work, head_sha256: hex(&all.finalize()), dry_run: false })
}
