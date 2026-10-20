//! "Install to this computer": copy IYAKKI from the stick it is running from onto an internal disk.
//!
//! The installed disk has the layout `iyakki-usb --install` makes, in one of two flavours (`--table auto|gpt|mbr`):
//!
//! * **MBR**: boot code in sector 0, GRUB's core image in the gap, partition 1 (FAT32 "IYAKKI-BOOT", type EF: GRUB for UEFI,
//!   the kernel and the initramfs), partition 2 filling the rest up to 2 TiB. 512-byte sectors only.
//! * **GPT** ([`crate::gpt_install`]): protective MBR with GRUB's boot code, a BIOS boot partition, an EFI System Partition
//!   "IYAKKI-BOOT" and the data partition "IYAKKI-DATA" filling the rest of the disk, primary and backup tables. Any size, 512 or
//!   4096-byte sectors.
//!
//! The data partition is marked "format me" (the first boot makes it the IYAKKI-DATA volume). The choices made in the
//! installer (zram, swap file, host name) travel inside that marked partition and become `system.conf` on the first boot.
//! Whole-disk erase only; "install next to another system" and data encryption are planned.
//!
//! Everything before [`execute`] is pure (it works on [`DiskInfo`] values), so the plan, the refusals and the layout are
//! tested without a disk; [`execute`] with `dry_run` writes nothing.

use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::time::Instant;

use sha2::{Digest, Sha256};

use crate::drives::{self, Drive, Verdict};
use crate::ui::human_size;
use crate::gpt_install::{self, GptLayout};
pub use crate::gpt_install::{booted_uefi, resolve_table, TableChoice, TableKind};
use crate::writer::{self, InstallLayout, Phase, Progress, BLOCK, MIN_DATA_SECTORS};
use iyakki_storage::gpt as gptfmt;

/// The smallest disk we install on, and the size from which it is comfortable.
pub const MIN_DISK: u64 = 4_000_000_000;
pub const COMFORTABLE_DISK: u64 = 16_000_000_000;
/// MBR sector counts are 32 bit: the data partition ends at 2 TiB.
pub const MBR_LIMIT: u64 = 2 * 1024 * 1024 * 1024 * 1024;
pub const MIB: u64 = 1024 * 1024;

/// The questions the installer asks.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Choices {
    /// `auto`, `off`, `4G`, `50%`, `4G,zstd` (same syntax as `iyakki.zram=`).
    pub zram: String,
    /// `off` or a size.
    pub swapfile: String,
    pub hostname: Option<String>,
}

impl Default for Choices {
    fn default() -> Choices {
        Choices { zram: "auto".into(), swapfile: "off".into(), hostname: None }
    }
}

pub fn valid_hostname(h: &str) -> bool {
    !h.is_empty() && h.len() <= 63 && !h.starts_with('-') && !h.ends_with('-') && h.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
}

impl Choices {
    pub fn validate(&self) -> Result<(), String> {
        if iyakki_zram::config::parse_zram_value(&self.zram).is_none() {
            return Err(format!("zram: \"{}\" is not auto, off, a size like 4G, or a percentage like 50%", self.zram));
        }
        if !matches!(self.swapfile.as_str(), "off" | "0") && iyakki_zram::config::parse_bytes(&self.swapfile).is_none() {
            return Err(format!("swap file: \"{}\" is not off or a size like 2G", self.swapfile));
        }
        if let Some(h) = &self.hostname {
            if !valid_hostname(h) {
                return Err(format!("host name \"{h}\": use lower-case letters, digits and '-' (up to 63 characters)"));
            }
        }
        Ok(())
    }

    /// The `system.conf` text the first boot starts from.
    pub fn conf_text(&self) -> String {
        let mut s = format!("# written by the IYAKKI installer\nzram={}\nswapfile={}\n", self.zram, self.swapfile);
        if let Some(h) = &self.hostname {
            s.push_str(&format!("hostname={h}\n"));
        }
        s
    }
}

/// A disk with everything the installer shows and decides on.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DiskInfo {
    pub drive: Drive,
    pub serial: String,
    pub logical_sector: u32,
    /// (name, bytes, what is on it when known)
    pub partitions: Vec<(String, u64, String)>,
    /// The disk already carries a GUID partition table.
    pub gpt: bool,
    /// What the table on the disk is, for the confirmation: "GPT", "MBR", "none" (empty when not looked at yet), with
    /// damage noted ("GPT, backup table missing").
    pub table: String,
}

fn read(p: &Path) -> String {
    std::fs::read_to_string(p).unwrap_or_default().trim().to_string()
}

/// Disks from `sys` (`/sys`), their nodes under `dev` (`/dev`; tests point both at a made-up machine so nothing real is ever
/// named), `proc_mounts` and `swaps` the text of `/proc/mounts` and `/proc/swaps`.
pub fn list_disks(sys: &Path, dev: &Path, proc_mounts: &str, swaps: &str) -> Vec<DiskInfo> {
    drives::list_linux(sys, proc_mounts, swaps)
        .into_iter()
        .map(|mut drive| {
            drive.path = dev.join(&drive.name).display().to_string();
            let d = sys.join("block").join(&drive.name);
            let serial = ["device/serial", "serial", "device/wwid", "wwid"].iter().map(|f| read(&d.join(f))).find(|s| !s.is_empty()).unwrap_or_default();
            let logical_sector = read(&d.join("queue/logical_block_size")).parse().unwrap_or(512);
            let mut partitions: Vec<(String, u64, String)> = std::fs::read_dir(&d)
                .into_iter()
                .flatten()
                .flatten()
                .filter(|e| e.path().join("partition").exists())
                .map(|e| {
                    let n = e.file_name().to_string_lossy().into_owned();
                    let sectors: u64 = read(&e.path().join("size")).parse().unwrap_or(0);
                    (n, sectors * 512, String::new())
                })
                .collect();
            partitions.sort_by_key(|p| (p.0.len(), p.0.clone()));
            DiskInfo { drive, serial, logical_sector, partitions, gpt: false, table: String::new() }
        })
        .collect()
}

/// Fill in what is on each partition and what the disk's partition table is (reads the disks; run as root). The table is read with
/// the disk's own sector size: GPT (both copies checked) or MBR, with each partition's type and label.
pub fn probe_contents(disks: &mut [DiskInfo], dev: &Path) {
    for d in disks {
        let node = dev.join(&d.drive.name);
        if let Ok(mut f) = std::fs::File::open(&node) {
            let size = if d.drive.size_bytes > 0 { d.drive.size_bytes } else { f.metadata().map_or(0, |m| m.len()) };
            let info = gptfmt::read_disk_table(&mut f, d.logical_sector, size);
            d.gpt = info.kind == gptfmt::Kind::Gpt;
            d.table = match info.kind {
                gptfmt::Kind::Gpt => "GPT",
                gptfmt::Kind::Mbr => "MBR",
                gptfmt::Kind::None => "none",
            }
            .to_string();
            if !info.notes.is_empty() {
                d.table = format!("{} ({})", d.table, info.notes.join("; "));
            }
            for p in &mut d.partitions {
                p.2 = iyakki_storage::probe(&dev.join(&p.0)).map(|c| c.describe()).unwrap_or_default();
                let number = p.0.strip_prefix(d.drive.name.as_str()).map(|r| r.trim_start_matches('p')).and_then(|r| r.parse::<u32>().ok());
                if let Some(t) = number.and_then(|n| info.parts.iter().find(|x| x.index == n)) {
                    let label = if t.label.is_empty() { String::new() } else { format!(" “{}”", t.label) };
                    p.2 = if p.2.is_empty() { format!("{}{label}", t.type_name) } else { format!("{} · {}{label}", p.2, t.type_name) };
                }
            }
        } else {
            for p in &mut d.partitions {
                p.2 = iyakki_storage::probe(&dev.join(&p.0)).map(|c| c.describe()).unwrap_or_default();
            }
        }
    }
}

/// Name of partition `n` of `disk` (`sda` -> `sda1`; `nvme0n1`, `mmcblk0` -> `nvme0n1p1`).
pub fn partition_name(disk: &str, n: u32) -> String {
    if disk.chars().last().is_some_and(|c| c.is_ascii_digit()) { format!("{disk}p{n}") } else { format!("{disk}{n}") }
}

/// What the installer copies from: the boot area of a disk (or template file) made by `iyakki-usb --install`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SourceInfo {
    pub path: PathBuf,
    pub sector0: Vec<u8>,
    pub p1_start: u64,
    pub p1_sectors: u64,
    pub p2_start: u64,
    /// MBR, gap and partition 1: this many bytes are copied.
    pub copy_bytes: u64,
}

fn entry(s0: &[u8], i: usize) -> (u8, u8, u64, u64) {
    let e = 446 + 16 * i;
    (s0[e], s0[e + 4], u32::from_le_bytes(s0[e + 8..e + 12].try_into().unwrap()) as u64, u32::from_le_bytes(s0[e + 12..e + 16].try_into().unwrap()) as u64)
}

/// Is `path` (a disk or a template file) an IYAKKI boot area? Checks the MBR layout and the FAT label "IYAKKI-BOOT".
pub fn read_source(path: &Path) -> Result<SourceInfo, String> {
    let mut f = std::fs::File::open(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let mut s0 = vec![0u8; 512];
    f.read_exact(&mut s0).map_err(|e| format!("{}: {e}", path.display()))?;
    if s0[510..512] != [0x55, 0xAA] {
        return Err(format!("{} has no partition table", path.display()));
    }
    let (_, t1, p1_start, p1_sectors) = entry(&s0, 0);
    let (_, t2, p2_start, _) = entry(&s0, 1);
    if t1 != 0xEF || t2 != 0x83 || p1_start == 0 || p1_sectors == 0 || p2_start < p1_start + p1_sectors {
        return Err(format!("{} is not an IYAKKI boot disk (expected a FAT boot partition then a data partition)", path.display()));
    }
    let mut bs = vec![0u8; 512];
    f.seek(SeekFrom::Start(p1_start * 512)).and_then(|_| f.read_exact(&mut bs)).map_err(|e| format!("{}: {e}", path.display()))?;
    let label = |o: usize| String::from_utf8_lossy(&bs[o..o + 11]).trim().to_string();
    if bs[510..512] != [0x55, 0xAA] || (label(71) != "IYAKKI-BOOT" && label(43) != "IYAKKI-BOOT") {
        return Err(format!("{}: the first partition is not the IYAKKI-BOOT volume", path.display()));
    }
    Ok(SourceInfo { path: path.into(), sector0: s0, p1_start, p1_sectors, p2_start, copy_bytes: (p1_start + p1_sectors) * 512 })
}

/// The certificate a Secure Boot image carries on its boot partition (`dist/buildroot/secureboot.sh`, `docs/SECURE-BOOT.md`).
pub const MOK_CERT_PATH: &str = "/EFI/IYAKKI/iyakki-mok.cer";

/// Does the template's boot partition hold the Secure Boot certificate (is it a signed image)? A template that cannot be read as
/// FAT32 answers no.
pub fn is_secure_boot_image(source: &SourceInfo) -> bool {
    let Ok(mut f) = std::fs::File::open(&source.path) else { return false };
    let Ok(mut fat) = crate::fatclone::Fat32::open(&mut f, source.p1_start * 512) else { return false };
    fat.walk().map(|items| has_mok_cert(items.iter().map(|i| i.path.as_str()))).unwrap_or(false)
}

fn has_mok_cert<'a>(mut paths: impl Iterator<Item = &'a str>) -> bool {
    paths.any(|p| p.eq_ignore_ascii_case(MOK_CERT_PATH))
}

/// What the person is told when the image is signed: Secure Boot works, after a one-time key enrolment.
pub const SECURE_BOOT_NOTE: &str = "this IYAKKI image is signed for Secure Boot. With Secure Boot on, the first start shows a blue key screen: choose \"Enroll key from disk\", EFI, IYAKKI, iyakki-mok.cer, Continue, Yes, Reboot (once per computer; docs/SECURE-BOOT.md). With Secure Boot off nothing is needed.";

/// The boot disk to copy from: not the target, an IYAKKI boot disk; the removable one when several.
pub fn find_source(disks: &[DiskInfo], dev: &Path, not: Option<&str>) -> Result<SourceInfo, String> {
    let mut found: Vec<(&DiskInfo, SourceInfo)> = disks.iter().filter(|d| Some(d.drive.name.as_str()) != not).filter_map(|d| read_source(&dev.join(&d.drive.name)).ok().map(|s| (d, s))).collect();
    if found.len() > 1 {
        found.sort_by_key(|(d, _)| !(d.drive.usb || d.drive.removable));
        if found[0].0.drive.usb || found[0].0.drive.removable {
            found.truncate(1);
        }
    }
    match found.len() {
        0 => Err("no IYAKKI boot disk found to copy from. Run this from an IYAKKI USB stick made with `iyakki-usb --install`, or name a template with --template FILE".into()),
        1 => Ok(found.remove(0).1),
        _ => Err(format!("several IYAKKI boot disks found ({}); name the one to copy from with --source", found.iter().map(|(d, _)| d.drive.name.as_str()).collect::<Vec<_>>().join(", "))),
    }
}

/// The new boot sector: the source's, with partition 2 grown to fill `device_bytes` (1 MiB aligned) and a fresh signature.
pub fn patch_mbr(sector0: &mut [u8], device_bytes: u64, signature: [u8; 4]) -> Result<InstallLayout, String> {
    if sector0.len() < 512 || sector0[510..512] != [0x55, 0xAA] {
        return Err("not a boot sector".into());
    }
    let (_, t2, start, _) = entry(sector0, 1);
    if t2 != 0x83 {
        return Err("the second partition is not a data partition".into());
    }
    let total = device_bytes / 512;
    if total <= start + MIN_DATA_SECTORS {
        return Err("this disk is too small".into());
    }
    let max = 0xFFFF_FFFFu64 - start;
    let sectors = ((total - start).min(max) / 2048) * 2048;
    let e = 446 + 16;
    sector0[e + 12..e + 16].copy_from_slice(&(sectors as u32).to_le_bytes());
    sector0[e + 1..e + 4].copy_from_slice(&[0xFE, 0xFF, 0xFF]);
    sector0[e + 5..e + 8].copy_from_slice(&[0xFE, 0xFF, 0xFF]);
    sector0[440..444].copy_from_slice(&signature);
    Ok(InstallLayout { data_start: start, data_sectors: sectors })
}

#[derive(Clone, Debug)]
pub struct Plan {
    pub target: DiskInfo,
    pub source: SourceInfo,
    /// Where the data partition goes, in 512-byte sectors (either table).
    pub layout: InstallLayout,
    pub choices: Choices,
    pub steps: Vec<String>,
    pub warnings: Vec<String>,
    /// The partition table the disk gets, and why.
    pub table: TableKind,
    pub table_why: String,
    /// The GPT layout (when `table` is GPT).
    pub gpt: Option<GptLayout>,
}

impl Plan {
    /// The planned partitions, one line each.
    pub fn layout_lines(&self) -> Vec<String> {
        match &self.gpt {
            Some(g) => g.lines(),
            None => vec![
                format!("1  IYAKKI-BOOT  {:>9}  FAT32, bootable: GRUB for UEFI, kernel, initial RAM disk (GRUB for legacy BIOS sits before it)", human_size(self.source.p1_sectors * 512)),
                format!("2  IYAKKI-DATA  {:>9}  formatted as ext4 on the first start", human_size(self.layout.data_sectors * 512)),
            ],
        }
    }

    /// Everything a person is shown before confirming.
    pub fn render(&self) -> String {
        let d = &self.target.drive;
        let mut s = format!("Install IYAKKI on {} ({}, {}{}), erasing EVERYTHING on it.\n", d.path, human_size(d.size_bytes), d.model, if self.target.serial.is_empty() { String::new() } else { format!(", serial {}", self.target.serial) });
        if !self.target.partitions.is_empty() || !self.target.table.is_empty() {
            s.push_str(&format!("It holds now{}:\n", if self.target.table.is_empty() || self.target.table == "none" { String::new() } else { format!(" (partition table: {})", self.target.table) }));
            for (n, b, what) in &self.target.partitions {
                s.push_str(&format!("  {n:<14} {:>9}  {what}\n", human_size(*b)));
            }
        }
        s.push_str(&format!("\nPartition table: {} ({}){}\n", self.table.label(), self.table_why, if self.target.logical_sector != 512 { format!(", {}-byte sectors", self.target.logical_sector) } else { String::new() }));
        s.push_str("The disk will be partitioned like this:\n");
        for l in self.layout_lines() {
            s.push_str(&format!("  {l}\n"));
        }
        s.push_str("\nThe plan:\n");
        for (i, st) in self.steps.iter().enumerate() {
            s.push_str(&format!("  {}. {st}\n", i + 1));
        }
        for w in &self.warnings {
            s.push_str(&format!("\nNote: {w}\n"));
        }
        s
    }
}

/// Plan the installation of `source` on the disk called `target` with the classic MBR layout. Refuses anything unsafe with the
/// reason. (The installer proper uses [`make_plan_with`], where the table is a choice.)
pub fn make_plan(disks: &[DiskInfo], target: &str, source: &SourceInfo, choices: &Choices) -> Result<Plan, String> {
    make_plan_with(disks, target, source, choices, TableChoice::Mbr, false)
}

/// Plan the installation with a partition table `table` (`uefi`: this machine was started through UEFI, which `Auto` takes into
/// account). Refuses anything unsafe with the reason.
pub fn make_plan_with(disks: &[DiskInfo], target: &str, source: &SourceInfo, choices: &Choices, table: TableChoice, uefi: bool) -> Result<Plan, String> {
    choices.validate()?;
    let t = target.trim().trim_start_matches("/dev/");
    let disk = disks.iter().find(|d| d.drive.name == t).ok_or_else(|| format!("{target} is not a disk of this computer (see the list)"))?;
    let d = &disk.drive;
    match drives::verdict(d, &source.path) {
        Verdict::Refused(r) => return Err(format!("{} cannot be used: {r}", d.path)),
        Verdict::Ok | Verdict::Internal => {}
    }
    if let Some((dev, mp)) = d.mounts.first() {
        return Err(format!("{} cannot be used: {dev} is mounted at {mp}. Unmount it first.", d.path));
    }
    if source.path.file_name() == Path::new(&d.path).file_name() {
        return Err(format!("{} is the disk IYAKKI is being copied from", d.path));
    }
    if d.size_bytes < MIN_DISK {
        return Err(format!("{} is only {}: IYAKKI needs at least {} for an install", d.path, human_size(d.size_bytes), human_size(MIN_DISK)));
    }
    let (kind, table_why) = resolve_table(table, d.size_bytes, disk.logical_sector, uefi).map_err(|e| format!("{} cannot be used: {e}", d.path))?;
    let mut warnings = Vec::new();
    if is_secure_boot_image(source) {
        warnings.push(SECURE_BOOT_NOTE.to_string());
    }
    if d.size_bytes < COMFORTABLE_DISK {
        warnings.push(format!("{} is small; 16 GB or more is comfortable.", human_size(d.size_bytes)));
    }
    let (layout, gpt) = match kind {
        TableKind::Mbr => {
            let mut sector0 = source.sector0.clone();
            let layout = patch_mbr(&mut sector0, d.size_bytes, [0, 0, 0, 0])?;
            if layout.data_start + layout.data_sectors < d.size_bytes / 512 - 4096 {
                warnings.push("this disk is larger than 2 TiB; only the first 2 TiB are used (the MBR partition table cannot address more). Choose GPT to use it all.".to_string());
            }
            (layout, None)
        }
        TableKind::Gpt => {
            let g = GptLayout::new(d.size_bytes, disk.logical_sector, source.p1_sectors * 512).map_err(|e| format!("{} cannot be used: {e}", d.path))?;
            if g.sector_size != 512 {
                warnings.push(format!("this disk has {}-byte sectors: it starts through UEFI only (legacy BIOS boot is not supported there), and the boot volume is rebuilt for 4096-byte sectors.", g.sector_size));
            }
            (g.data_layout(), Some(g))
        }
    };
    if disk.gpt {
        warnings.push("the disk has a GPT partition table; it is replaced.".into());
    } else if disk.table.starts_with("MBR") {
        warnings.push("the disk has an MBR partition table; it is replaced.".into());
    }
    if !(d.usb || d.removable) {
        warnings.push("this is an internal disk. Other systems on it (Windows, Linux, macOS) will be gone; there is no \"install next to\" mode yet (planned).".into());
    }
    let boot_bytes = gpt.as_ref().map_or(source.p1_sectors * 512, |g| g.bytes(g.esp.1));
    let tables = match kind {
        TableKind::Mbr => "the partition table (sector 0) last".to_string(),
        TableKind::Gpt => "the backup GPT first, the primary GPT (protective MBR, entries, header) last".to_string(),
    };
    let steps = vec![
        match kind {
            TableKind::Mbr => format!("write the boot area ({} MB: boot code, GRUB, partition 1 \"IYAKKI-BOOT\" FAT32 {}) copied from {}", source.copy_bytes / 1_000_000, human_size(boot_bytes), source.path.display()),
            TableKind::Gpt => format!("write GRUB into the BIOS boot partition and the boot volume \"IYAKKI-BOOT\" (FAT32, {}{}) from {}", human_size(boot_bytes), if gpt.as_ref().is_some_and(|g| g.esp_rebuilt) { ", rebuilt for 4096-byte sectors" } else { "" }, source.path.display()),
        },
        format!("create the data partition \"IYAKKI-DATA\" ({}), formatted on the first boot", human_size(layout.data_sectors * 512)),
        format!("save your choices for the first boot: zram {}, swap file {}{}", choices.zram, choices.swapfile, choices.hostname.as_ref().map_or(String::new(), |h| format!(", host name {h}"))),
        format!("wipe the old partition tables and the end of the disk; write {tables}"),
        "read everything back and compare checksums".to_string() + if kind == TableKind::Gpt { "; check both GPT copies (CRC32, locations)" } else { "" },
        "the first account is created on the first start, as always".to_string(),
    ];
    Ok(Plan { target: disk.clone(), source: source.clone(), layout, choices: choices.clone(), steps, warnings, table: kind, table_why, gpt })
}

/// The GPT plan for a drive `iyakki-usb --install` has already vetted (it checks mounts, swap and the rest itself and takes
/// sticks smaller than the installer's 4 GB minimum): no refusals of its own beyond "does it fit".
pub fn plan_stick_gpt(drive: &Drive, sector: u32, source: &SourceInfo, why: &str) -> Result<Plan, String> {
    let g = GptLayout::new(drive.size_bytes, sector, source.p1_sectors * 512).map_err(|e| format!("{}: {e}", drive.path))?;
    let disk = DiskInfo { drive: drive.clone(), serial: String::new(), logical_sector: sector, partitions: vec![], gpt: false, table: String::new() };
    Ok(Plan {
        target: disk,
        source: source.clone(),
        layout: g.data_layout(),
        choices: Choices::default(),
        steps: vec![],
        warnings: vec![],
        table: TableKind::Gpt,
        table_why: why.to_string(),
        gpt: Some(g),
    })
}

/// The typed confirmations: the drive name, then its size as shown ("238.5 GB", spaces and case do not matter).
pub fn confirmation_ok(name_line: &str, size_line: &str, d: &Drive) -> bool {
    let norm = |s: &str| s.chars().filter(|c| !c.is_whitespace()).collect::<String>().to_lowercase();
    let n = name_line.trim();
    !n.is_empty() && (n == d.name || n == d.path) && norm(size_line) == norm(&human_size(d.size_bytes))
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct InstallReport {
    pub written: u64,
    pub head_sha256: String,
    pub dry_run: bool,
}

fn random_signature() -> [u8; 4] {
    let mut b = [0u8; 4];
    let ok = std::fs::File::open("/dev/urandom").and_then(|mut f| f.read_exact(&mut b)).is_ok();
    if !ok || b == [0; 4] {
        b = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(1, |d| d.subsec_nanos()).to_le_bytes();
    }
    b
}

/// Do it. With `dry_run` nothing is opened for writing. Order: data marker and settings, end-of-disk wipe, the boot area
/// without its first sector, then the first sector (the partition table) last, so a half-finished install never looks
/// bootable. `mounts` is the current text of /proc/mounts, checked again just before writing.
pub fn execute(plan: &Plan, dry_run: bool, proc_mounts: &str, progress: &mut dyn FnMut(Progress)) -> Result<InstallReport, String> {
    let d = &plan.target.drive;
    if drives::mounts_of(&d.name, &drives::parse_proc_mounts(proc_mounts)).first().is_some() {
        return Err(format!("{} was mounted while the installer was waiting; nothing was written", d.path));
    }
    if dry_run {
        return Ok(InstallReport { written: 0, head_sha256: String::new(), dry_run: true });
    }
    if plan.table == TableKind::Gpt {
        return gpt_install::execute(plan, progress);
    }
    let t0 = Instant::now();
    let total = plan.source.copy_bytes + MIB;
    let mut s0 = plan.source.sector0.clone();
    patch_mbr(&mut s0, d.size_bytes, random_signature())?;
    let mut src = std::fs::File::open(&plan.source.path).map_err(|e| format!("{}: {e}", plan.source.path.display()))?;
    let mut dst = std::fs::OpenOptions::new().write(true).open(&d.path).map_err(|e| format!("{}: {e}", d.path))?;
    let fail = |e: std::io::Error| format!("writing {}: {e}\nThe disk may be half-written; do not boot it.", d.path);
    // 1. the data partition's marker and the settings
    let mut marker = vec![0u8; MIB as usize];
    marker[..iyakki_storage::PENDING_MARKER.len()].copy_from_slice(iyakki_storage::PENDING_MARKER);
    let conf = iyakki_storage::pending_conf_bytes(&plan.choices.conf_text())?;
    let off = iyakki_storage::PENDING_CONF_OFFSET as usize;
    marker[off..off + conf.len()].copy_from_slice(&conf);
    let data_off = plan.layout.data_start * 512;
    dst.seek(SeekFrom::Start(data_off)).and_then(|_| dst.write_all(&marker)).map_err(fail)?;
    // 2. the end of the disk: a stale backup GPT would confuse the kernel
    let tail = d.size_bytes.min(MIB);
    dst.seek(SeekFrom::Start(d.size_bytes - tail)).and_then(|_| dst.write_all(&vec![0u8; tail as usize])).map_err(fail)?;
    progress(Progress { phase: Phase::Writing, done: MIB, total, elapsed: t0.elapsed() });
    // 3. the boot area, sector 0 excluded
    let mut h = Sha256::new();
    h.update(&s0);
    src.seek(SeekFrom::Start(512)).map_err(|e| e.to_string())?;
    dst.seek(SeekFrom::Start(512)).map_err(fail)?;
    let mut buf = vec![0u8; BLOCK];
    let (mut done, mut since_flush) = (512u64, 0u64);
    while done < plan.source.copy_bytes {
        let want = ((plan.source.copy_bytes - done) as usize).min(BLOCK);
        src.read_exact(&mut buf[..want]).map_err(|e| format!("reading {}: {e}", plan.source.path.display()))?;
        h.update(&buf[..want]);
        dst.write_all(&buf[..want]).map_err(fail)?;
        done += want as u64;
        since_flush += want as u64;
        if since_flush >= 8 * MIB {
            dst.sync_data().map_err(fail)?;
            since_flush = 0;
        }
        progress(Progress { phase: Phase::Writing, done: MIB + done, total, elapsed: t0.elapsed() });
    }
    dst.sync_all().map_err(fail)?;
    // 4. the partition table, last
    dst.seek(SeekFrom::Start(0)).and_then(|_| dst.write_all(&s0)).map_err(fail)?;
    progress(Progress { phase: Phase::Syncing, done: total, total, elapsed: t0.elapsed() });
    dst.sync_all().map_err(fail)?;
    drop(dst);
    let sha = h.finalize().iter().map(|b| format!("{b:02x}")).collect::<String>();
    // 5. read it back
    writer::verify(Path::new(&d.path), plan.source.copy_bytes, &sha, progress).map_err(|e| format!("{e}\nDo not boot this disk."))?;
    let mut back = std::fs::File::open(&d.path).map_err(|e| e.to_string())?;
    let mut m = vec![0u8; 8192];
    back.seek(SeekFrom::Start(data_off)).and_then(|_| back.read_exact(&mut m)).map_err(|e| format!("reading back the data partition: {e}"))?;
    if iyakki_storage::classify(&m) != iyakki_storage::Content::Pending {
        return Err("verification FAILED: the data partition marker did not arrive. Do not boot this disk.".into());
    }
    drives::reread_partitions(&d.path);
    Ok(InstallReport { written: total, head_sha256: sha, dry_run: false })
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    pub(crate) fn tmp(tag: &str) -> PathBuf {
        static N: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
        let d = std::env::temp_dir().join(format!("iyakki-inst-{}-{tag}-{}", std::process::id(), N.fetch_add(1, std::sync::atomic::Ordering::Relaxed)));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    /// A small fake install template: MBR + gap + FAT boot partition "IYAKKI-BOOT" + data partition.
    pub(crate) fn template(path: &Path) -> Vec<u8> {
        let mut t = vec![0u8; 8 * 1024 * 1024];
        for (i, b) in t[..440].iter_mut().enumerate() {
            *b = (i % 251) as u8 + 1;
        }
        t[440..444].copy_from_slice(&[0x11, 0x22, 0x33, 0x44]);
        // like GRUB's boot.img and core.img: the boot code names the core image's sector (1), whose first sector ends with a
        // block list {start 2, 10 sectors, segment 0x820}
        t[0x5C..0x64].copy_from_slice(&1u64.to_le_bytes());
        for (i, x) in t[512..512 + 11 * 512].iter_mut().enumerate() {
            *x = (i * 13 % 251) as u8 + 1;
        }
        t[512 + 500..512 + 508].copy_from_slice(&2u64.to_le_bytes());
        t[512 + 508..512 + 510].copy_from_slice(&10u16.to_le_bytes());
        t[512 + 510..512 + 512].copy_from_slice(&0x820u16.to_le_bytes());
        let (p1, p2) = (446, 462);
        t[p1] = 0x80;
        t[p1 + 4] = 0xEF;
        t[p1 + 8..p1 + 12].copy_from_slice(&2048u32.to_le_bytes());
        t[p1 + 12..p1 + 16].copy_from_slice(&8192u32.to_le_bytes()); // 4 MiB
        t[p2 + 4] = 0x83;
        t[p2 + 8..p2 + 12].copy_from_slice(&10240u32.to_le_bytes());
        t[p2 + 12..p2 + 16].copy_from_slice(&2048u32.to_le_bytes());
        t[510] = 0x55;
        t[511] = 0xAA;
        let b = 2048 * 512;
        t[b + 71..b + 82].copy_from_slice(b"IYAKKI-BOOT");
        t[b + 510] = 0x55;
        t[b + 511] = 0xAA;
        for (i, x) in t[b + 4096..b + 4096 + 100000].iter_mut().enumerate() {
            *x = (i * 7 % 256) as u8;
        }
        std::fs::write(path, &t).unwrap();
        t
    }

    pub(crate) fn disk(name: &str, gb: u64) -> DiskInfo {
        DiskInfo {
            drive: Drive { path: format!("/dev/{name}"), name: name.into(), model: "Samsung SSD".into(), size_bytes: gb * 1_000_000_000, removable: false, usb: false, read_only: false, mounts: vec![], swap: false },
            serial: "S123".into(),
            logical_sector: 512,
            partitions: vec![(partition_name(name, 1), 500_000_000, "vfat".into()), (partition_name(name, 2), 200_000_000_000, "ntfs".into())],
            gpt: true,
            table: "GPT".into(),
        }
    }

    pub(crate) fn src() -> SourceInfo {
        let d = tmp("srcinfo");
        template(&d.join("t.img"));
        read_source(&d.join("t.img")).unwrap()
    }

    #[test]
    fn the_secure_boot_certificate_is_recognised_by_its_path_and_a_non_fat_template_is_not_signed() {
        assert!(has_mok_cert(["/EFI", "/efi/iyakki/IYAKKI-MOK.CER"].into_iter()));
        assert!(!has_mok_cert(["/EFI/BOOT/bootx64.efi"].into_iter()));
        assert!(SECURE_BOOT_NOTE.contains("Enroll key from disk") && SECURE_BOOT_NOTE.contains("iyakki-mok.cer"));
        let junk = SourceInfo { path: "/nonexistent/template.img".into(), sector0: vec![], p1_start: 1, p1_sectors: 1, p2_start: 2, copy_bytes: 512 };
        assert!(!is_secure_boot_image(&junk));
    }

    #[test]
    fn partition_names_follow_the_kernel() {
        assert_eq!(partition_name("sda", 2), "sda2");
        assert_eq!(partition_name("nvme0n1", 1), "nvme0n1p1");
        assert_eq!(partition_name("mmcblk0", 2), "mmcblk0p2");
        assert_eq!(partition_name("vda", 1), "vda1");
    }

    #[test]
    fn the_source_must_be_an_iyakki_boot_disk() {
        let d = tmp("source");
        let t = template(&d.join("t.img"));
        let s = read_source(&d.join("t.img")).unwrap();
        assert_eq!((s.p1_start, s.p1_sectors, s.p2_start, s.copy_bytes), (2048, 8192, 10240, 10240 * 512));
        assert_eq!(s.sector0, t[..512]);
        let mut bad = t.clone();
        bad[2048 * 512 + 71..2048 * 512 + 82].copy_from_slice(b"SOMETHINGEL");
        std::fs::write(d.join("bad.img"), &bad).unwrap();
        assert!(read_source(&d.join("bad.img")).unwrap_err().contains("IYAKKI-BOOT"));
        std::fs::write(d.join("zero.img"), vec![0u8; 4096]).unwrap();
        assert!(read_source(&d.join("zero.img")).unwrap_err().contains("no partition table"));
        let mut wrong = t.clone();
        wrong[446 + 4] = 0x0B;
        std::fs::write(d.join("wrong.img"), &wrong).unwrap();
        assert!(read_source(&d.join("wrong.img")).unwrap_err().contains("not an IYAKKI boot disk"));
        assert!(read_source(&d.join("missing")).is_err());
        // the FAT16 label position works too
        let mut f16 = t.clone();
        f16[2048 * 512 + 71..2048 * 512 + 82].copy_from_slice(&[b' '; 11]);
        f16[2048 * 512 + 43..2048 * 512 + 54].copy_from_slice(b"IYAKKI-BOOT");
        std::fs::write(d.join("f16.img"), &f16).unwrap();
        assert!(read_source(&d.join("f16.img")).is_ok());
    }

    #[test]
    fn find_source_prefers_the_removable_boot_disk_and_skips_the_target() {
        let dev = tmp("devdir");
        let d = tmp("findsrc");
        template(&dev.join("sdb"));
        template(&dev.join("sdc"));
        let mut usb = disk("sdb", 16);
        usb.drive.usb = true;
        usb.drive.removable = true;
        let internal = disk("sdc", 500);
        let none = disk("sdd", 500);
        assert_eq!(find_source(&[usb.clone(), internal.clone(), none.clone()], &dev, None).unwrap().path, dev.join("sdb"), "the stick wins");
        assert_eq!(find_source(&[usb.clone(), internal.clone()], &dev, Some("sdb")).unwrap().path, dev.join("sdc"), "never the target");
        assert!(find_source(&[none.clone()], &dev, None).unwrap_err().contains("no IYAKKI boot disk"));
        let mut other = internal.clone();
        other.drive.name = "sde".into();
        template(&dev.join("sde"));
        assert!(find_source(&[internal, other], &dev, None).unwrap_err().contains("several"));
        let _ = d;
    }

    #[test]
    fn plan_for_an_internal_disk_shows_everything_and_warns() {
        let p = make_plan(&[disk("nvme0n1", 512)], "/dev/nvme0n1", &src(), &Choices::default()).unwrap();
        assert_eq!(p.layout.data_start, 10240);
        assert_eq!(p.layout.data_sectors % 2048, 0);
        assert!(p.layout.data_start + p.layout.data_sectors <= 512_000_000_000 / 512);
        let text = p.render();
        for want in ["/dev/nvme0n1", "512.0 GB", "serial S123", "nvme0n1p2", "ntfs", "IYAKKI-BOOT", "IYAKKI-DATA", "zram auto", "internal disk", "GPT", "read everything back"] {
            assert!(text.contains(want), "{want} in:\n{text}");
        }
    }

    #[test]
    fn unsafe_targets_are_refused_with_the_reason() {
        let s = src();
        let c = Choices::default();
        let mut sys = disk("nvme0n1", 512);
        sys.drive.mounts = vec![("/dev/nvme0n1p2".into(), "/".into())];
        assert!(make_plan(&[sys], "nvme0n1", &s, &c).unwrap_err().contains("running system"));
        let mut mounted = disk("sda", 512);
        mounted.drive.mounts = vec![("/dev/sda1".into(), "/mnt/x".into())];
        assert!(make_plan(&[mounted], "sda", &s, &c).unwrap_err().contains("Unmount it first"));
        let mut ro = disk("sdb", 512);
        ro.drive.read_only = true;
        assert!(make_plan(&[ro], "sdb", &s, &c).unwrap_err().contains("write-protected"));
        let mut swap = disk("sdc", 512);
        swap.drive.swap = true;
        assert!(make_plan(&[swap], "sdc", &s, &c).unwrap_err().contains("swap"));
        let mut fourk = disk("sdd", 512);
        fourk.logical_sector = 4096;
        assert!(make_plan(&[fourk], "sdd", &s, &c).unwrap_err().contains("4Kn"));
        assert!(make_plan(&[disk("sde", 2)], "sde", &s, &c).unwrap_err().contains("at least"));
        assert!(make_plan(&[disk("sdf", 512)], "sdz", &s, &c).unwrap_err().contains("not a disk"));
        let mut empty = disk("sdg", 512);
        empty.drive.size_bytes = 0;
        assert!(make_plan(&[empty], "sdg", &s, &c).unwrap_err().contains("no media"));
        // the disk we copy from
        let mut from = src();
        from.path = PathBuf::from("/dev/sdh");
        assert!(make_plan(&[disk("sdh", 512)], "sdh", &from, &c).unwrap_err().contains("copied from"));
        // the image file lives on a mount of the target
        let mut holder = disk("sdi", 512);
        holder.drive.mounts = vec![("/dev/sdi1".into(), "/mnt".into())];
        assert!(make_plan(&[holder], "sdi", &s, &c).is_err());
    }

    #[test]
    fn small_and_huge_disks_get_notes() {
        let s = src();
        let small = make_plan(&[disk("sda", 8)], "sda", &s, &Choices::default()).unwrap();
        assert!(small.warnings.iter().any(|w| w.contains("small")));
        let mut big = disk("sdb", 4096);
        big.gpt = false;
        let p = make_plan(&[big], "sdb", &s, &Choices::default()).unwrap();
        assert!(p.warnings.iter().any(|w| w.contains("2 TiB")), "{:?}", p.warnings);
        assert!(p.layout.data_start + p.layout.data_sectors <= 0xFFFF_FFFF);
        let mut usb = disk("sdc", 32);
        usb.drive.usb = true;
        usb.gpt = false;
        assert!(make_plan(&[usb], "sdc", &s, &Choices::default()).unwrap().warnings.is_empty());
    }

    #[test]
    fn choices_are_validated_and_become_a_config() {
        assert!(Choices::default().validate().is_ok());
        let c = Choices { zram: "4G,zstd".into(), swapfile: "2G".into(), hostname: Some("laptop-1".into()) };
        assert!(c.validate().is_ok());
        let t = c.conf_text();
        let r = iyakki_zram::config::resolve("", &t);
        assert_eq!((r.config.size, r.config.swapfile), (iyakki_zram::Size::Bytes(4 << 30), Some(2 << 30)));
        assert!(t.contains("hostname=laptop-1"));
        for bad in [Choices { zram: "potato".into(), ..Default::default() }, Choices { swapfile: "lots".into(), ..Default::default() }, Choices { hostname: Some("Bad Name".into()), ..Default::default() }, Choices { hostname: Some("-x".into()), ..Default::default() }] {
            assert!(bad.validate().is_err(), "{bad:?}");
        }
        assert!(Choices { zram: "off".into(), swapfile: "off".into(), hostname: None }.validate().is_ok());
    }

    #[test]
    fn typed_confirmation_needs_the_name_and_the_size() {
        let d = disk("sda", 512).drive;
        assert!(confirmation_ok("sda", "512.0 GB", &d));
        assert!(confirmation_ok(" /dev/sda ", "512.0gb", &d));
        assert!(!confirmation_ok("sda", "512 GB", &d), "the size as shown");
        assert!(!confirmation_ok("sdb", "512.0 GB", &d));
        assert!(!confirmation_ok("", "", &d));
        assert!(!confirmation_ok("sd", "512.0 GB", &d));
    }

    #[test]
    fn disks_are_listed_with_serial_sector_size_and_partitions() {
        let root = tmp("sys");
        let w = |rel: &str, v: &str| {
            let p = root.join(rel);
            std::fs::create_dir_all(p.parent().unwrap()).unwrap();
            std::fs::write(p, v).unwrap();
        };
        let base = "devices/pci0000:00/0000:00:1d.0/nvme/nvme0/nvme0n1";
        w(&format!("{base}/size"), "1000215216\n");
        w(&format!("{base}/removable"), "0\n");
        w(&format!("{base}/device/serial"), "S4EVNX0\n");
        w(&format!("{base}/device/model"), "Samsung SSD 970\n");
        w(&format!("{base}/queue/logical_block_size"), "4096\n");
        w(&format!("{base}/nvme0n1p1/size"), "1048576\n");
        w(&format!("{base}/nvme0n1p1/partition"), "1\n");
        w(&format!("{base}/nvme0n1p2/size"), "999000000\n");
        w(&format!("{base}/nvme0n1p2/partition"), "2\n");
        std::fs::create_dir_all(root.join("block")).unwrap();
        std::os::unix::fs::symlink(root.join(base), root.join("block/nvme0n1")).unwrap();
        let v = list_disks(&root, Path::new("/dev"), "", "");
        assert_eq!(v.len(), 1);
        assert_eq!((v[0].serial.as_str(), v[0].logical_sector, v[0].partitions.len()), ("S4EVNX0", 4096, 2));
        assert_eq!(v[0].partitions[0].0, "nvme0n1p1");
        assert_eq!(v[0].drive.model, "Samsung SSD 970");
    }

    fn file_disk(dir: &Path, name: &str, mib: u64) -> (PathBuf, DiskInfo) {
        let f = dir.join(name);
        std::fs::File::create(&f).unwrap().set_len(mib * MIB).unwrap();
        let mut d = disk(name, 8);
        d.drive.path = f.display().to_string();
        d.drive.size_bytes = mib * MIB;
        d.drive.usb = true;
        d.gpt = false;
        d.table = String::new();
        d.partitions.clear();
        (f, d)
    }

    #[test]
    fn dry_run_writes_nothing_and_refuses_a_disk_mounted_meanwhile() {
        let dir = tmp("dry");
        let t = template(&dir.join("t.img"));
        let s = read_source(&dir.join("t.img")).unwrap();
        let (f, mut d) = file_disk(&dir, "target", 4200);
        d.drive.size_bytes = 8_000_000_000; // planned as an 8 GB disk, the file stays sparse
        let plan = make_plan(&[d.clone()], "target", &s, &Choices::default()).unwrap();
        let before = std::fs::read(&f).unwrap();
        let r = execute(&plan, true, "", &mut |_| panic!("no progress when nothing is written")).unwrap();
        assert!(r.dry_run && r.written == 0);
        assert_eq!(std::fs::read(&f).unwrap(), before, "the target is untouched");
        assert!(std::fs::read(&f).unwrap().iter().all(|b| *b == 0));
        let _ = t;
        let mut named = plan.clone();
        named.target.drive.name = "sda".into();
        named.target.drive.path = "/dev/sda".into();
        assert!(execute(&named, false, "/dev/sda1 /mnt vfat rw 0 0\n", &mut |_| {}).unwrap_err().contains("was mounted"));
    }

    #[test]
    fn installing_onto_an_image_file_gives_the_template_layout_and_a_marked_data_partition() {
        let dir = tmp("real");
        let t = template(&dir.join("t.img"));
        let s = read_source(&dir.join("t.img")).unwrap();
        let (f, mut disk) = file_disk(&dir, "target", 600);
        disk.drive.size_bytes = 600 * MIB;
        // pre-existing junk: an old GPT at the start and a stale backup at the end
        let mut old = vec![0u8; 600 * MIB as usize];
        old[512..520].copy_from_slice(b"EFI PART");
        let n = old.len();
        old[n - 512..n - 504].copy_from_slice(b"EFI PART");
        std::fs::write(&f, &old).unwrap();
        // 4 GB minimum applies to real disks: bypass by planning with a bigger nominal size, then write the small file
        let mut nominal = disk.clone();
        nominal.drive.size_bytes = 4_100_000_000;
        let mut plan = make_plan(&[nominal], "target", &s, &Choices { zram: "2G,lz4".into(), swapfile: "1G".into(), hostname: Some("mybox".into()) }).unwrap();
        plan.target.drive.size_bytes = 600 * MIB;
        plan.layout = patch_mbr(&mut s.sector0.clone(), 600 * MIB, [0; 4]).unwrap();
        let mut phases = Vec::new();
        let r = execute(&plan, false, "", &mut |p| phases.push(p.phase)).unwrap();
        assert!(!r.dry_run && r.written > 0);
        assert!(phases.contains(&Phase::Writing) && phases.contains(&Phase::Verifying));
        let out = std::fs::read(&f).unwrap();
        assert_eq!(out.len(), 600 * MIB as usize);
        assert_eq!(&out[512..s.copy_bytes as usize], &t[512..s.copy_bytes as usize], "boot area copied verbatim (except sector 0)");
        assert_eq!(&out[..440], &t[..440], "boot code identical");
        assert_ne!(&out[440..444], &t[440..444], "a fresh disk signature");
        assert_eq!(&out[446..462], &t[446..462], "partition 1 as built");
        let (_, ty, start, sectors) = entry(&out, 1);
        assert_eq!((ty, start), (0x83, 10240));
        assert_eq!(sectors, (600 * MIB / 512 - 10240) / 2048 * 2048);
        assert_ne!(&out[512..520], b"EFI PART", "the old GPT header is gone");
        assert!(out[n - 1024..].iter().all(|b| *b == 0), "the stale backup table is wiped");
        let part = &out[start as usize * 512..];
        assert_eq!(iyakki_storage::classify(&part[..65536]), iyakki_storage::Content::Pending);
        let tmpf = dir.join("part");
        std::fs::write(&tmpf, &part[..65536]).unwrap();
        let conf = iyakki_storage::read_pending_conf(&tmpf).unwrap();
        let rz = iyakki_zram::config::resolve("", &conf);
        assert_eq!((rz.config.algo.as_deref(), rz.config.swapfile), (Some("lz4"), Some(1 << 30)));
        assert!(conf.contains("hostname=mybox"));
        // fdisk agrees about the table, when it exists on this machine
        if let Ok(o) = std::process::Command::new("fdisk").arg("-l").arg(&f).output() {
            if o.status.success() {
                let text = String::from_utf8_lossy(&o.stdout);
                assert!(text.contains("EFI") && text.contains("Linux"), "{text}");
            }
        }
    }

    #[test]
    fn a_failed_read_of_the_source_is_reported() {
        let dir = tmp("failsrc");
        template(&dir.join("t.img"));
        let mut s = read_source(&dir.join("t.img")).unwrap();
        let (_, disk) = file_disk(&dir, "target", 600);
        let mut nominal = disk.clone();
        nominal.drive.size_bytes = 4_100_000_000;
        let mut plan = make_plan(&[nominal], "target", &s, &Choices::default()).unwrap();
        plan.target.drive.size_bytes = 600 * MIB;
        s.path = dir.join("gone.img");
        plan.source = s;
        assert!(execute(&plan, false, "", &mut |_| {}).is_err());
    }
}
