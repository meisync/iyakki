//! The partition planner: read a disk's partition table, and turn a request ("use the free space", "use these two partitions")
//! into a PLAN, a table of every change with sizes, before anything is written.
//!
//! Everything here is pure: it works on a [`DiskModel`] (made by [`read_model`] from any `Read + Seek`, so a regular file stands
//! in for a disk in the tests) and on a [`Request`]. The rules that keep other systems safe are in [`build_plan`]; the tests in
//! `planner_tests.rs` pin each of them. Writing the plan is [`crate::apply`].
//!
//! Modes (docs/INSTALLER.md):
//!
//! * `whole-disk`: erase the disk, make the IYAKKI layout (the installer's long-standing behaviour).
//! * `free-space`: create the IYAKKI boot and data partitions in a gap of unallocated space. Nothing else is touched.
//! * `custom`: the same with sizes chosen by the person.
//! * `reuse`: make two existing partitions the boot (FAT32) and data partitions; only those are formatted. With `keep_esp`
//!   the boot partition is the disk's existing EFI System Partition, which is not formatted: only an `EFI/IYAKKI` folder and
//!   a boot entry are added (dual boot with another system).
//!
//! No shrinking and no resizing: free space is what the disk already has (shrink with the other system's own tool first).

use std::io::{Read, Seek, SeekFrom};
use std::path::PathBuf;

use iyakki_storage::gpt::{self, Guid};
use sha2::{Digest, Sha256};

use crate::drives;
use crate::gpt_install::{resolve_table, TableChoice, TableKind, ESP_4K_BYTES, MIN_DATA_BYTES};
use crate::install::{partition_name, MIB};
use crate::json::Json;
use crate::ui::human_size;

/// An IYAKKI install on a shared disk needs this much for the data partition to be of any use (a warning below it).
pub const COMFORTABLE_DATA_BYTES: u64 = 8 * 1024 * MIB;

// ---------------------------------------------------------------------------------------------------------------------------
// reading a disk

/// One partition as the disk's table lists it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Part {
    /// 1-based slot in the table (the number in `sda3`).
    pub index: u32,
    /// Kernel name (`sda3`, `nvme0n1p3`).
    pub name: String,
    /// In sectors of the disk.
    pub first_lba: u64,
    pub sectors: u64,
    /// "EFI System", "Microsoft basic data", "Linux", "extended", ...
    pub kind: String,
    pub type_guid: Option<Guid>,
    pub mbr_type: Option<u8>,
    pub label: String,
    /// Filesystem by signature: "ext4", "ntfs", "fat32", "swap", "" when unknown or blank.
    pub fs: String,
    /// Free bytes of a FAT32 volume when its FSInfo sector says.
    pub fat_free: Option<u64>,
    /// Mount points of this partition (from /proc/mounts).
    pub mounts: Vec<String>,
    pub swap: bool,
    pub extended: bool,
}

impl Part {
    pub fn bytes(&self, ss: u32) -> u64 {
        self.sectors * ss as u64
    }
    pub fn end_lba(&self) -> u64 {
        self.first_lba + self.sectors
    }
    pub fn is_esp(&self) -> bool {
        self.type_guid == Some(gpt::TYPE_ESP) || self.mbr_type == Some(0xEF)
    }
    /// In use right now (mounted or swap): never touched.
    pub fn in_use(&self) -> Option<String> {
        if let Some(m) = self.mounts.first() {
            return Some(format!("mounted at {m}"));
        }
        self.swap.then(|| "in use as swap space".to_string())
    }
}

/// Unallocated space, aligned inwards to 1 MiB.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Gap {
    pub first_lba: u64,
    pub sectors: u64,
}

impl Gap {
    pub fn bytes(&self, ss: u32) -> u64 {
        self.sectors * ss as u64
    }
}

/// What the caller knows about the disk besides its contents.
#[derive(Clone, Debug, Default)]
pub struct DiskMeta {
    pub name: String,
    pub path: String,
    pub model: String,
    pub serial: String,
    pub removable: bool,
    pub usb: bool,
    pub read_only: bool,
    /// Names of partitions used as swap (`/proc/swaps`).
    pub swap_parts: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DiskModel {
    pub name: String,
    pub path: String,
    pub model: String,
    pub serial: String,
    pub size_bytes: u64,
    pub sector: u32,
    /// `None`: no partition table found.
    pub table: Option<TableKind>,
    pub notes: Vec<String>,
    pub parts: Vec<Part>,
    pub removable: bool,
    pub usb: bool,
    pub read_only: bool,
    /// The disk holds the running system (`/`, `/boot`, the IYAKKI data folder, ... are mounted from it).
    pub running_system: bool,
    /// The stick IYAKKI booted from: it is the source of an install and never a target.
    pub booted_from: bool,
    /// No table, but the first sectors hold something (a filesystem on the whole disk).
    pub raw_data: bool,
    /// A GPT with an entry array other than 128 x 128 bytes: rewriting it would change its geometry.
    pub unusual_gpt: bool,
    pub disk_guid: Option<Guid>,
    /// SHA-256 over the sizes and the table sectors: any change in the table changes it.
    pub fingerprint: String,
}

impl DiskModel {
    pub fn al(&self) -> u64 {
        MIB / self.sector as u64
    }
    pub fn total_sectors(&self) -> u64 {
        self.size_bytes / self.sector as u64
    }
    pub fn mounted(&self) -> Vec<(String, String)> {
        self.parts.iter().flat_map(|p| p.mounts.iter().map(|m| (p.name.clone(), m.clone()))).collect()
    }
    pub fn part(&self, index: u32) -> Option<&Part> {
        self.parts.iter().find(|p| p.index == index)
    }
    pub fn internal(&self) -> bool {
        !(self.usb || self.removable)
    }

    /// The first and one past the last sector a partition may use (1 MiB aligned inwards).
    pub fn usable(&self, kind: TableKind) -> (u64, u64) {
        let al = self.al();
        let total = self.total_sectors();
        match kind {
            TableKind::Gpt => {
                let array = gpt::ENTRY_ARRAY_BYTES.div_ceil(self.sector as u64);
                let first = (2 + array).div_ceil(al).max(1) * al;
                let last_usable = total.saturating_sub(array + 2);
                (first, (last_usable + 1) / al * al)
            }
            TableKind::Mbr => (al, total.min(0xFFFF_FFFF) / al * al),
        }
    }

    /// Unallocated gaps for a table of kind `kind` (the disk's own, or the one that would be made).
    pub fn free_gaps(&self, kind: TableKind) -> Vec<Gap> {
        let al = self.al();
        let (lo, hi) = self.usable(kind);
        let mut used: Vec<(u64, u64)> = self.parts.iter().map(|p| (p.first_lba, p.end_lba())).collect();
        used.sort();
        let mut gaps = Vec::new();
        let mut cursor = lo;
        for (s, e) in used {
            let end = s / al * al;
            if end > cursor {
                gaps.push(Gap { first_lba: cursor, sectors: end - cursor });
            }
            cursor = cursor.max(e.div_ceil(al) * al);
        }
        if hi > cursor {
            gaps.push(Gap { first_lba: cursor, sectors: hi - cursor });
        }
        gaps.retain(|g| g.sectors * self.sector as u64 >= MIB);
        gaps
    }

    pub fn largest_gap(&self, kind: TableKind) -> Option<Gap> {
        self.free_gaps(kind).into_iter().max_by_key(|g| g.sectors)
    }
}

fn le16(b: &[u8], o: usize) -> u64 {
    u16::from_le_bytes([b[o], b[o + 1]]) as u64
}
fn le32(b: &[u8], o: usize) -> u64 {
    u32::from_le_bytes(b[o..o + 4].try_into().unwrap()) as u64
}

fn read_at<R: Read + Seek>(r: &mut R, off: u64, len: usize) -> Vec<u8> {
    let mut v = vec![0u8; len];
    let mut got = 0;
    if r.seek(SeekFrom::Start(off)).is_ok() {
        while got < len {
            match r.read(&mut v[got..]) {
                Ok(0) | Err(_) => break,
                Ok(n) => got += n,
            }
        }
    }
    v.truncate(got);
    v.resize(len, 0);
    v
}

/// The filesystem at `off` by its signature ("" for blank or unknown).
pub fn sniff_fs<R: Read + Seek>(r: &mut R, off: u64, len: u64) -> String {
    let head = read_at(r, off, 4096.min(len as usize));
    let at = |o: usize, n: usize| head.get(o..o + n);
    if at(0, 20) == Some(&iyakki_storage::PENDING_MARKER[..]) {
        return "iyakki-pending".into();
    }
    if at(3, 8) == Some(b"NTFS    ") {
        return "ntfs".into();
    }
    if at(3, 8) == Some(b"EXFAT   ") {
        return "exfat".into();
    }
    if at(82, 5) == Some(b"FAT32") {
        return "fat32".into();
    }
    if at(54, 5) == Some(b"FAT16") {
        return "fat16".into();
    }
    if at(54, 5) == Some(b"FAT12") {
        return "fat12".into();
    }
    if at(0, 4) == Some(b"XFSB") {
        return "xfs".into();
    }
    if at(0, 6) == Some(b"LUKS\xba\xbe") {
        return "luks".into();
    }
    if at(32, 4) == Some(b"NXSB") {
        return "apfs".into();
    }
    if at(4086, 10) == Some(b"SWAPSPACE2") || at(4086, 10) == Some(b"SWAP-SPACE") {
        return "swap".into();
    }
    if at(1024 + 56, 2) == Some(&[0x53, 0xEF]) {
        let compat = le32(&head, 1024 + 0x5C);
        let incompat = le32(&head, 1024 + 0x60);
        return if incompat & 0x40 != 0 || incompat & 0x200 != 0 { "ext4" } else if compat & 0x4 != 0 { "ext3" } else { "ext2" }.into();
    }
    if at(1024, 2) == Some(b"H+") || at(1024, 2) == Some(b"HX") {
        return "hfs+".into();
    }
    if len > 0x10048 && read_at(r, off + 0x10040, 8) == b"_BHRfS_M" {
        return "btrfs".into();
    }
    if len > 0x8006 && read_at(r, off + 0x8001, 5) == b"CD001" {
        return "iso9660".into();
    }
    String::new()
}

/// Free bytes of the FAT32 volume at `off`, from its FSInfo sector, when that sector is valid.
pub fn fat32_free<R: Read + Seek>(r: &mut R, off: u64) -> Option<u64> {
    let b = read_at(r, off, 512);
    if b[510..512] != [0x55, 0xAA] || &b[82..87] != b"FAT32" {
        return None;
    }
    let (bps, spc, info) = (le16(&b, 11), b[13] as u64, le16(&b, 48));
    if !matches!(bps, 512 | 1024 | 2048 | 4096) || spc == 0 || info == 0 || info == 0xFFFF {
        return None;
    }
    let f = read_at(r, off + info * bps, 512.max(bps as usize));
    if le32(&f, 0) != 0x4161_5252 || le32(&f, 484) != 0x6141_7272 {
        return None;
    }
    let free = le32(&f, 488);
    (free != 0xFFFF_FFFF).then(|| free * spc * bps)
}

const MBR_TYPES: &[(u8, &str)] = &[
    (0x01, "FAT12"),
    (0x04, "FAT16"),
    (0x06, "FAT16"),
    (0x0E, "FAT16"),
    (0x05, "extended"),
    (0x0F, "extended"),
    (0x85, "extended"),
    (0x07, "NTFS/exFAT"),
    (0x0B, "FAT32"),
    (0x0C, "FAT32"),
    (0x27, "Windows recovery"),
    (0x82, "Linux swap"),
    (0x83, "Linux"),
    (0x8E, "Linux LVM"),
    (0xAF, "Apple"),
    (0xEE, "GPT protective"),
    (0xEF, "EFI System"),
    (0xFD, "Linux RAID"),
];

/// Read the table and the contents of a disk. `mounts` is `/proc/mounts` parsed ([`drives::parse_proc_mounts`]); `boot_media`
/// the name of the disk IYAKKI booted from, when known. Never fails: oddities become `notes`.
pub fn read_model<R: Read + Seek>(r: &mut R, meta: &DiskMeta, size_bytes: u64, sector: u32, mounts: &[(String, String)], boot_media: Option<&str>) -> DiskModel {
    let ss = sector as u64;
    let info = gpt::read_disk_table(r, sector, size_bytes);
    let mut m = DiskModel {
        name: meta.name.clone(),
        path: meta.path.clone(),
        model: meta.model.clone(),
        serial: meta.serial.clone(),
        size_bytes,
        sector,
        table: None,
        notes: info.notes.clone(),
        parts: vec![],
        removable: meta.removable,
        usb: meta.usb,
        read_only: meta.read_only,
        running_system: false,
        booted_from: boot_media == Some(meta.name.as_str()),
        raw_data: false,
        unusual_gpt: false,
        disk_guid: None,
        fingerprint: String::new(),
    };
    let s0 = read_at(r, 0, 512.max(sector as usize));
    let mut hash = Sha256::new();
    hash.update(size_bytes.to_le_bytes());
    hash.update(sector.to_le_bytes());
    hash.update(&s0);
    let push = |m: &mut DiskModel, r: &mut R, index: u32, first_lba: u64, sectors: u64, kind: String, type_guid: Option<Guid>, mbr_type: Option<u8>, label: String, extended: bool| {
        let name = partition_name(&meta.name, index);
        let mut p = Part {
            index,
            first_lba,
            sectors,
            kind,
            type_guid,
            mbr_type,
            label,
            fs: if extended { String::new() } else { sniff_fs(r, first_lba * ss, sectors * ss) },
            fat_free: None,
            mounts: mounts.iter().filter(|(d, _)| d.rsplit('/').next() == Some(name.as_str())).map(|(_, mp)| mp.clone()).collect(),
            swap: meta.swap_parts.iter().any(|s| *s == name),
            name,
            extended,
        };
        if p.fs == "fat32" {
            p.fat_free = fat32_free(r, first_lba * ss);
        }
        m.parts.push(p);
    };
    match info.kind {
        gpt::Kind::Gpt => {
            m.table = Some(TableKind::Gpt);
            match gpt::read_gpt(r, sector, size_bytes / ss) {
                Ok(parsed) => {
                    m.disk_guid = Some(parsed.table.disk_guid);
                    for p in &parsed.problems {
                        m.notes.push(p.clone());
                    }
                    for e in &parsed.table.entries {
                        push(&mut m, r, e.index, e.first_lba, e.sectors(), gpt::type_name(&e.type_guid), Some(e.type_guid), None, e.name.clone(), false);
                    }
                    // the header's entry array geometry
                    let hdr = read_at(r, ss, 92);
                    if hdr.len() >= 92 && &hdr[..8] == gpt::SIGNATURE && (le32(&hdr, 80) != 128 || le32(&hdr, 84) != 128) {
                        m.unusual_gpt = true;
                    }
                    let array = gpt::ENTRY_ARRAY_BYTES.div_ceil(ss);
                    hash.update(read_at(r, ss, sector as usize));
                    hash.update(read_at(r, 2 * ss, (array * ss) as usize));
                }
                Err(e) => m.notes.push(format!("the GPT cannot be used: {e}")),
            }
        }
        gpt::Kind::Mbr => {
            m.table = Some(TableKind::Mbr);
            for i in 0..4usize {
                let e = 446 + 16 * i;
                let (t, start, n) = (s0[e + 4], le32(&s0, e + 8), le32(&s0, e + 12));
                if t == 0 || n == 0 {
                    continue;
                }
                let name = MBR_TYPES.iter().find(|(c, _)| *c == t).map_or("other", |(_, n)| *n).to_string();
                let ext = matches!(t, 0x05 | 0x0F | 0x85);
                push(&mut m, r, i as u32 + 1, start, n, name, None, Some(t), String::new(), ext);
            }
        }
        gpt::Kind::None => {
            m.raw_data = !sniff_fs(r, 0, size_bytes).is_empty() || read_at(r, 0, 4096).iter().any(|b| *b != 0);
        }
    }
    m.parts.sort_by_key(|p| p.first_lba);
    m.running_system = mounts.iter().any(|(d, mp)| drives::belongs_to(d, &meta.name) && (drives::critical_mount(mp) || mp.contains("iyakki")));
    m.fingerprint = Sha256::digest(hash.finalize()).iter().map(|b| format!("{b:02x}")).collect();
    m
}

// ---------------------------------------------------------------------------------------------------------------------------
// the request

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    WholeDisk,
    FreeSpace,
    Reuse,
    Custom,
}

impl Mode {
    pub fn parse(s: &str) -> Result<Mode, String> {
        match s.trim().to_ascii_lowercase().as_str() {
            "whole-disk" | "whole" | "erase" => Ok(Mode::WholeDisk),
            "free-space" | "free" => Ok(Mode::FreeSpace),
            "reuse" | "partitions" => Ok(Mode::Reuse),
            "custom" => Ok(Mode::Custom),
            o => Err(format!("mode \"{o}\": use whole-disk, free-space, reuse or custom")),
        }
    }
    pub fn id(self) -> &'static str {
        match self {
            Mode::WholeDisk => "whole-disk",
            Mode::FreeSpace => "free-space",
            Mode::Reuse => "reuse",
            Mode::Custom => "custom",
        }
    }
    pub fn title(self) -> &'static str {
        match self {
            Mode::WholeDisk => "Whole disk: erase everything on the disk",
            Mode::FreeSpace => "Free space only: add IYAKKI in unallocated space, touch nothing else",
            Mode::Reuse => "Use existing partitions: choose which become BOOT and DATA (only those are formatted)",
            Mode::Custom => "Custom sizes in free space",
        }
    }
    pub const ALL: [Mode; 4] = [Mode::WholeDisk, Mode::FreeSpace, Mode::Reuse, Mode::Custom];
}

/// What the person asked for. Serialised into the plan file; the privileged writer recomputes the plan from it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Request {
    pub mode: Mode,
    /// Free space to use: its first sector (as listed). `None`: the largest.
    pub gap_start: Option<u64>,
    /// Existing partitions (table slot numbers) for `reuse`.
    pub boot_part: Option<u32>,
    pub data_part: Option<u32>,
    /// Dual boot: leave the existing EFI System Partition alone and add only `EFI/IYAKKI` and a boot entry.
    pub keep_esp: bool,
    /// Custom sizes in MiB (`None`: the image's need for boot, the rest of the gap for data).
    pub boot_mib: Option<u64>,
    pub data_mib: Option<u64>,
    pub table: TableChoice,
    /// The person typed the confirmation for a disk that holds the running system.
    pub confirm_running_disk: bool,
    pub zram: String,
    pub swapfile: String,
    pub hostname: Option<String>,
}

impl Default for Request {
    fn default() -> Request {
        Request { mode: Mode::WholeDisk, gap_start: None, boot_part: None, data_part: None, keep_esp: false, boot_mib: None, data_mib: None, table: TableChoice::Auto, confirm_running_disk: false, zram: "auto".into(), swapfile: "off".into(), hostname: None }
    }
}

impl Request {
    pub fn choices(&self) -> crate::install::Choices {
        crate::install::Choices { zram: self.zram.clone(), swapfile: self.swapfile.clone(), hostname: self.hostname.clone() }
    }

    pub fn to_json(&self) -> Json {
        let opt = |v: Option<u64>| v.map_or(Json::Null, Json::num);
        Json::obj(vec![
            ("mode", Json::str(self.mode.id())),
            ("gap_start", opt(self.gap_start)),
            ("boot_part", opt(self.boot_part.map(u64::from))),
            ("data_part", opt(self.data_part.map(u64::from))),
            ("keep_esp", Json::Bool(self.keep_esp)),
            ("boot_mib", opt(self.boot_mib)),
            ("data_mib", opt(self.data_mib)),
            ("table", Json::str(self.table.label().to_ascii_lowercase())),
            ("confirm_running_disk", Json::Bool(self.confirm_running_disk)),
            ("zram", Json::str(&self.zram)),
            ("swapfile", Json::str(&self.swapfile)),
            ("hostname", self.hostname.as_deref().map_or(Json::Null, Json::str)),
        ])
    }

    pub fn from_json(j: &Json) -> Result<Request, String> {
        let s = |k: &str| j.get(k).and_then(Json::as_str);
        let n = |k: &str| j.get(k).and_then(Json::as_u64);
        let b = |k: &str| j.get(k).and_then(Json::as_bool).unwrap_or(false);
        let mut r = Request { mode: Mode::parse(s("mode").ok_or("the plan file has no mode")?)?, ..Request::default() };
        r.gap_start = n("gap_start");
        r.boot_part = n("boot_part").map(|v| v as u32);
        r.data_part = n("data_part").map(|v| v as u32);
        r.keep_esp = b("keep_esp");
        r.boot_mib = n("boot_mib");
        r.data_mib = n("data_mib");
        r.table = TableChoice::parse(s("table").unwrap_or("auto"))?;
        r.confirm_running_disk = b("confirm_running_disk");
        if let Some(z) = s("zram") {
            r.zram = z.into();
        }
        if let Some(z) = s("swapfile") {
            r.swapfile = z.into();
        }
        r.hostname = s("hostname").map(String::from);
        r.choices().validate()?;
        Ok(r)
    }
}

/// What the image needs, read from it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SourceSpec {
    pub path: PathBuf,
    /// The boot partition of the image, in bytes (the PC image needs 512 MB).
    pub boot_bytes: u64,
    /// Kernel + initial RAM disk, the files "keep the existing ESP" copies.
    pub payload_bytes: u64,
    pub secure_boot: bool,
}

impl SourceSpec {
    /// The boot partition size a disk with `sector`-byte sectors needs.
    pub fn boot_required(&self, sector: u32) -> u64 {
        if sector == 512 { self.boot_bytes } else { ESP_4K_BYTES }.div_ceil(MIB) * MIB
    }
}

// ---------------------------------------------------------------------------------------------------------------------------
// the plan

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Action {
    Create,
    Format,
    Delete,
    Keep,
    /// Add files to an existing FAT volume (keep the ESP).
    AddFiles,
}

impl Action {
    pub fn word(self) -> &'static str {
        match self {
            Action::Create => "create",
            Action::Format => "format",
            Action::Delete => "delete",
            Action::Keep => "keep",
            Action::AddFiles => "add files",
        }
    }
    pub fn destructive(self) -> bool {
        matches!(self, Action::Format | Action::Delete)
    }
}

impl Step {
    /// Deletes a partition, or formats one whose filesystem is recognised (a blank one says "its contents (<kind>)").
    pub fn erases_data(&self) -> bool {
        match self.action {
            Action::Delete => true,
            Action::Format => !self.loses.starts_with("its contents ("),
            _ => false,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Role {
    Bios,
    Boot,
    Data,
}

/// One row of the plan table.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Step {
    pub action: Action,
    /// `sda3` for an existing partition, `new 4` for a partition that will be created.
    pub part: String,
    pub index: u32,
    pub first_lba: u64,
    pub sectors: u64,
    pub bytes: u64,
    pub role: Option<Role>,
    /// What it holds or becomes.
    pub what: String,
    /// For destructive steps: what is lost.
    pub loses: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Plan {
    pub disk_name: String,
    pub disk_path: String,
    pub disk_model: String,
    pub disk_size: u64,
    pub sector: u32,
    pub request: Request,
    pub fingerprint: String,
    pub table_before: Option<TableKind>,
    pub table_after: TableKind,
    pub table_why: String,
    /// The disk gets a new partition table (blank disk or whole-disk mode).
    pub new_table: bool,
    pub steps: Vec<Step>,
    /// Slot numbers of the partitions that become BOOT and DATA.
    pub boot_index: u32,
    pub data_index: u32,
    pub keep_esp: bool,
    pub boot_required: u64,
    /// Partitions to unmount first (all non-critical).
    pub unmount: Vec<String>,
    pub warnings: Vec<String>,
    /// The disk holds the running system: the typed confirmation is mandatory.
    pub running_disk: bool,
    pub internal: bool,
    pub source: PathBuf,
}

impl Plan {
    pub fn destructive(&self) -> bool {
        self.steps.iter().any(|s| s.action.destructive())
    }

    /// The plan deletes a partition or reformats one that holds data: the second deliberate step (the erase acknowledgement)
    /// is required. Formatting a partition that holds no filesystem (blank space the person chose) needs only the one button.
    pub fn erases_data(&self) -> bool {
        self.steps.iter().any(Step::erases_data)
    }

    /// Names of every partition the plan formats or deletes.
    pub fn destroyed(&self) -> Vec<String> {
        self.steps.iter().filter(|s| s.action.destructive()).map(|s| s.part.clone()).collect()
    }

    /// The plan as the person reads it.
    pub fn render(&self) -> String {
        let mut s = format!("Disk {} ({}, {}, {}-byte sectors){}\n", self.disk_path, self.disk_model, human_size(self.disk_size), self.sector, if self.internal { ", INTERNAL" } else { "" });
        s.push_str(&format!("Mode: {}\n", self.request.mode.title()));
        s.push_str(&format!("Partition table: {} -> {}{}\n\n", self.table_before.map_or("none", |t| t.label()), self.table_after.label(), if self.new_table { " (written new)" } else { " (existing entries kept as they are)" }));
        s.push_str("  #  action      partition      start        size        what\n");
        for st in &self.steps {
            let what = if st.loses.is_empty() { st.what.clone() } else { format!("{}  [loses: {}]", st.what, st.loses) };
            s.push_str(&format!("  {:<2} {:<11} {:<14} {:>10}  {:>10}  {what}\n", st.index, st.action.word(), st.part, human_size(st.first_lba * self.sector as u64), human_size(st.bytes)));
        }
        if !self.unmount.is_empty() {
            s.push_str(&format!("\nUnmounted first: {}\n", self.unmount.join(", ")));
        }
        let d = self.destroyed();
        if d.is_empty() {
            s.push_str("\nNothing that exists now is erased or changed.\n");
        } else {
            s.push_str(&format!("\nErased or reformatted: {}\n", d.join(", ")));
        }
        for w in &self.warnings {
            s.push_str(&format!("\nNote: {w}\n"));
        }
        s
    }

    /// Digest of what the person confirmed: the privileged writer refuses a plan whose digest changed.
    pub fn digest(&self) -> String {
        let mut h = Sha256::new();
        h.update(self.fingerprint.as_bytes());
        h.update(self.render().as_bytes());
        h.finalize().iter().map(|b| format!("{b:02x}")).collect()
    }

    pub fn to_json(&self) -> Json {
        let steps = self
            .steps
            .iter()
            .map(|s| {
                Json::obj(vec![
                    ("action", Json::str(s.action.word())),
                    ("partition", Json::str(&s.part)),
                    ("number", Json::num(s.index as u64)),
                    ("first_sector", Json::num(s.first_lba)),
                    ("sectors", Json::num(s.sectors)),
                    ("bytes", Json::num(s.bytes)),
                    ("role", s.role.map_or(Json::Null, |r| Json::str(match r { Role::Bios => "bios", Role::Boot => "boot", Role::Data => "data" }))),
                    ("what", Json::str(&s.what)),
                    ("loses", Json::str(&s.loses)),
                    ("destructive", Json::Bool(s.action.destructive())),
                ])
            })
            .collect();
        Json::obj(vec![
            ("format", Json::str("iyakki-install-plan-1")),
            ("disk", Json::obj(vec![("name", Json::str(&self.disk_name)), ("path", Json::str(&self.disk_path)), ("model", Json::str(&self.disk_model)), ("size_bytes", Json::num(self.disk_size)), ("sector_size", Json::num(self.sector as u64)), ("fingerprint", Json::str(&self.fingerprint)), ("internal", Json::Bool(self.internal)), ("running_system", Json::Bool(self.running_disk))])),
            ("request", self.request.to_json()),
            ("table_before", Json::str(self.table_before.map_or("none", |t| t.label()))),
            ("table_after", Json::str(self.table_after.label())),
            ("new_table", Json::Bool(self.new_table)),
            ("boot_partition", Json::num(self.boot_index as u64)),
            ("data_partition", Json::num(self.data_index as u64)),
            ("keep_esp", Json::Bool(self.keep_esp)),
            ("boot_required_bytes", Json::num(self.boot_required)),
            ("source", Json::str(self.source.display().to_string())),
            ("steps", Json::Arr(steps)),
            ("unmount", Json::Arr(self.unmount.iter().map(Json::str).collect())),
            ("destroys", Json::Arr(self.destroyed().iter().map(Json::str).collect())),
            ("warnings", Json::Arr(self.warnings.iter().map(Json::str).collect())),
            ("digest", Json::str(self.digest())),
        ])
    }
}

/// What the person must type for this plan (the drive's name), and the check of what was typed.
pub fn confirmation_ok(plan: &Plan, name_line: &str, size_line: &str) -> bool {
    let norm = |s: &str| s.chars().filter(|c| !c.is_whitespace()).collect::<String>().to_lowercase();
    let n = name_line.trim();
    !n.is_empty() && (n == plan.disk_name || n == plan.disk_path) && norm(size_line) == norm(&human_size(plan.disk_size))
}

/// What proves that the person confirmed a plan (the `confirmed` object of the plan file).
///
/// `Typed` is the original form (the drive name and its size typed; still used by the command line and old plan files).
/// `Button` is the screen's form: one press of the Install button, plus `erase_ack` (the ticked "everything on this drive will be
/// erased" box) when the plan erases data. Owner's decision, 2026-10-24: the screens no longer ask for typing.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Confirmation {
    Typed { name: String, size: String },
    Button { drive: String, size: u64, erase_ack: bool },
}

impl Confirmation {
    pub fn to_json(&self) -> Json {
        match self {
            Confirmation::Typed { name, size } => Json::obj(vec![("name", Json::str(name)), ("size", Json::str(size))]),
            Confirmation::Button { drive, size, erase_ack } => Json::obj(vec![("method", Json::str("button")), ("drive", Json::str(drive)), ("size", Json::num(*size)), ("erase_ack", Json::Bool(*erase_ack))]),
        }
    }

    /// `None` when the object has neither form.
    pub fn from_json(j: &Json) -> Option<Confirmation> {
        if j.get("method").and_then(Json::as_str) == Some("button") {
            return Some(Confirmation::Button { drive: j.get("drive").and_then(Json::as_str)?.to_string(), size: j.get("size").and_then(Json::as_u64)?, erase_ack: j.get("erase_ack").and_then(Json::as_bool).unwrap_or(false) });
        }
        Some(Confirmation::Typed { name: j.get("name").and_then(Json::as_str)?.to_string(), size: j.get("size").and_then(Json::as_str)?.to_string() })
    }
}

/// Does `c` prove that the person confirmed `plan`? The `Err` says why not. Typed confirmations are checked as before (typing the
/// name and size is at least as deliberate as the erase acknowledgement); a button confirmation must name this drive and its exact
/// size, and must carry `erase_ack` when the plan erases data.
pub fn check_confirmation(plan: &Plan, c: &Confirmation) -> Result<(), String> {
    match c {
        Confirmation::Typed { name, size } => {
            if confirmation_ok(plan, name, size) {
                Ok(())
            } else {
                Err("the typed confirmation in the plan file does not match the drive's name and size; nothing was written".into())
            }
        }
        Confirmation::Button { drive, size, erase_ack } => {
            let d = drive.trim();
            if d.is_empty() || !(d == plan.disk_name || d == plan.disk_path) || *size != plan.disk_size {
                return Err("the confirmation in the plan file names a different drive or size than the plan; nothing was written".into());
            }
            if plan.erases_data() && !*erase_ack {
                return Err("this plan erases data and the confirmation does not carry the erase acknowledgement; nothing was written".into());
            }
            Ok(())
        }
    }
}

fn mib_up(b: u64) -> u64 {
    b.div_ceil(MIB) * MIB
}

fn describe_part(p: &Part, ss: u32) -> String {
    let fs = if p.fs.is_empty() { String::new() } else { format!("{} ", p.fs) };
    let label = if p.label.is_empty() { String::new() } else { format!("“{}” ", p.label) };
    format!("{}{}{}", fs, label, if p.kind.is_empty() { human_size(p.bytes(ss)) } else { p.kind.clone() }).trim().to_string()
}

/// The refusals every mode shares. Returns the warnings.
fn common_checks(d: &DiskModel, req: &Request) -> Result<Vec<String>, String> {
    let mut warnings = Vec::new();
    if d.size_bytes == 0 {
        return Err(format!("{} cannot be used: no media in the drive", d.path));
    }
    if d.read_only {
        return Err(format!("{} cannot be used: it is write-protected", d.path));
    }
    if !matches!(d.sector, 512 | 4096) {
        return Err(format!("{} cannot be used: {}-byte logical sectors are not supported (512 and 4096 are)", d.path, d.sector));
    }
    if d.booted_from {
        return Err(format!("{} is the disk IYAKKI started from (the installation source); it is never a target", d.path));
    }
    if d.parts.iter().any(|p| p.swap) && req.mode == Mode::WholeDisk {
        return Err(format!("{} cannot be used: it is in use as swap space", d.path));
    }
    if d.unusual_gpt {
        return Err(format!("{} has a GPT with an unusual entry array (not 128 entries of 128 bytes); rewriting it could break the other system, so it is refused", d.path));
    }
    if d.table.is_some() && d.notes.iter().any(|n| n.contains("cannot be used")) {
        return Err(format!("{}: {}. Repair the partition table first (for example with the other system's disk tool).", d.path, d.notes.iter().find(|n| n.contains("cannot be used")).unwrap()));
    }
    if d.running_system {
        if req.mode == Mode::WholeDisk {
            return Err(format!("{} holds the running system (a partition of it is mounted as the system, boot or data volume); erasing it is refused", d.path));
        }
        if !req.confirm_running_disk {
            return Err(format!("{} holds the running system. Changing its free space or partitions is possible only after you type the disk's name and size to confirm (--confirm-running-disk); the mounted partitions are never touched", d.path));
        }
        warnings.push("this disk holds the system that is running right now: only free space or partitions that are not mounted are used".into());
    }
    if d.internal() {
        warnings.push("this is an internal disk.".into());
    }
    Ok(warnings)
}

fn pick_part<'a>(d: &'a DiskModel, idx: u32, what: &str) -> Result<&'a Part, String> {
    let p = d.part(idx).ok_or_else(|| format!("{what}: partition {idx} does not exist on {}", d.path))?;
    if p.extended {
        return Err(format!("{what}: {} is an extended partition (a container); choose a primary partition", p.name));
    }
    Ok(p)
}

fn refuse_in_use(p: &Part, what: &str) -> Result<(), String> {
    if let Some(m) = p.mounts.iter().find(|m| drives::critical_mount(m) || m.contains("iyakki")) {
        return Err(format!("{what}: {} is the running system's own volume (mounted at {m}); it is never used", p.name));
    }
    if p.swap {
        return Err(format!("{what}: {} is in use as swap space", p.name));
    }
    Ok(())
}

/// Build the plan for `req` on disk `d`. Pure; refuses anything unsafe with the reason.
pub fn build_plan(d: &DiskModel, req: &Request, src: &SourceSpec, uefi: bool) -> Result<Plan, String> {
    let mut warnings = common_checks(d, req)?;
    let required = src.boot_required(d.sector);
    let mut plan = Plan {
        disk_name: d.name.clone(),
        disk_path: d.path.clone(),
        disk_model: d.model.clone(),
        disk_size: d.size_bytes,
        sector: d.sector,
        request: req.clone(),
        fingerprint: d.fingerprint.clone(),
        table_before: d.table,
        table_after: d.table.unwrap_or(TableKind::Gpt),
        table_why: String::new(),
        new_table: false,
        steps: vec![],
        boot_index: 0,
        data_index: 0,
        keep_esp: false,
        boot_required: required,
        unmount: vec![],
        warnings: vec![],
        running_disk: d.running_system,
        internal: d.internal(),
        source: src.path.clone(),
    };
    if src.secure_boot {
        warnings.push(crate::install::SECURE_BOOT_NOTE.to_string());
    }
    let ss = d.sector as u64;
    match req.mode {
        Mode::WholeDisk => whole_disk(d, req, src, uefi, &mut plan, &mut warnings)?,
        Mode::FreeSpace | Mode::Custom => free_space(d, req, src, uefi, &mut plan, &mut warnings)?,
        Mode::Reuse => reuse(d, req, src, &mut plan, &mut warnings)?,
    }
    plan.steps.sort_by_key(|s| (s.first_lba, s.index));
    // the partitions that must be unmounted: mounted ones the plan destroys (critical mounts were refused above)
    for st in plan.steps.iter().filter(|s| s.action.destructive()) {
        if let Some(p) = d.parts.iter().find(|p| p.name == st.part) {
            if p.in_use().is_some() {
                plan.unmount.push(p.name.clone());
            }
        }
    }
    // the keep-ESP target is not touched by a format, but it must not be mounted either
    plan.warnings = warnings;
    let _ = ss;
    check_plan(d, &plan)?;
    Ok(plan)
}

fn whole_disk(d: &DiskModel, req: &Request, src: &SourceSpec, uefi: bool, plan: &mut Plan, warnings: &mut Vec<String>) -> Result<(), String> {
    use crate::gpt_install::GptLayout;
    if d.size_bytes < crate::install::MIN_DISK {
        return Err(format!("{} is only {}: IYAKKI needs at least {} for a whole-disk install", d.path, human_size(d.size_bytes), human_size(crate::install::MIN_DISK)));
    }
    let (kind, why) = resolve_table(req.table, d.size_bytes, d.sector, uefi).map_err(|e| format!("{} cannot be used: {e}", d.path))?;
    plan.table_after = kind;
    plan.table_why = why;
    plan.new_table = true;
    for p in &d.parts {
        plan.steps.push(Step { action: Action::Delete, part: p.name.clone(), index: p.index, first_lba: p.first_lba, sectors: p.sectors, bytes: p.bytes(d.sector), role: None, what: describe_part(p, d.sector), loses: if p.fs.is_empty() { "its contents".into() } else { format!("its {} contents", p.fs) } });
    }
    let ss = d.sector as u64;
    match kind {
        TableKind::Gpt => {
            let g = GptLayout::new(d.size_bytes, d.sector, src.boot_bytes).map_err(|e| format!("{} cannot be used: {e}", d.path))?;
            let mk = |i: u32, span: (u64, u64), role: Role, what: &str| Step { action: Action::Create, part: format!("new {i}"), index: i, first_lba: span.0, sectors: span.1, bytes: span.1 * ss, role: Some(role), what: what.into(), loses: String::new() };
            plan.steps.push(mk(1, g.bios, Role::Bios, if d.sector == 512 { "BIOS boot (GRUB for legacy BIOS PCs)" } else { "BIOS boot (kept free: 4096-byte sectors start through UEFI)" }));
            plan.steps.push(mk(2, g.esp, Role::Boot, "IYAKKI-BOOT FAT32, EFI System"));
            plan.steps.push(mk(3, g.data, Role::Data, "IYAKKI-DATA (ext4 on first start)"));
            plan.boot_index = 2;
            plan.data_index = 3;
            plan.boot_required = g.bytes(g.esp.1);
        }
        TableKind::Mbr => {
            let boot = mib_up(src.boot_bytes) / 512;
            let first = 2048u64;
            let total = d.size_bytes / 512;
            let data_sectors = ((total - first - boot).min(0xFFFF_FFFF - first - boot)) / 2048 * 2048;
            if total <= first + boot + 131072 {
                return Err(format!("{} cannot be used: this disk is too small", d.path));
            }
            plan.steps.push(Step { action: Action::Create, part: "new 1".into(), index: 1, first_lba: first, sectors: boot, bytes: boot * 512, role: Some(Role::Boot), what: "IYAKKI-BOOT FAT32, bootable (GRUB for BIOS before it)".into(), loses: String::new() });
            plan.steps.push(Step { action: Action::Create, part: "new 2".into(), index: 2, first_lba: first + boot, sectors: data_sectors, bytes: data_sectors * 512, role: Some(Role::Data), what: "IYAKKI-DATA (ext4 on first start)".into(), loses: String::new() });
            plan.boot_index = 1;
            plan.data_index = 2;
            if first + boot + data_sectors < total - 4096 {
                warnings.push("this disk is larger than 2 TiB; only the first 2 TiB are used (choose GPT to use it all).".into());
            }
        }
    }
    if d.table.is_some() {
        warnings.push(format!("the disk has {} partition(s) now; they are all deleted.", d.parts.len()));
    }
    Ok(())
}

/// Existing partitions as Keep rows.
fn keep_rows(d: &DiskModel, plan: &mut Plan, except: &[u32]) {
    for p in &d.parts {
        if !except.contains(&p.index) {
            plan.steps.push(Step { action: Action::Keep, part: p.name.clone(), index: p.index, first_lba: p.first_lba, sectors: p.sectors, bytes: p.bytes(d.sector), role: None, what: describe_part(p, d.sector), loses: String::new() });
        }
    }
}

fn find_esp<'a>(d: &'a DiskModel, req: &Request) -> Result<&'a Part, String> {
    if let Some(i) = req.boot_part {
        let p = pick_part(d, i, "boot partition")?;
        if !p.is_esp() {
            return Err(format!("keep-ESP: {} is not an EFI System Partition ({})", p.name, p.kind));
        }
        return Ok(p);
    }
    let all: Vec<&Part> = d.parts.iter().filter(|p| p.is_esp()).collect();
    match all.len() {
        0 => Err(format!("{} has no EFI System Partition to keep. Use the free-space mode, which creates IYAKKI's own boot partition.", d.path)),
        1 => Ok(all[0]),
        _ => Err(format!("{} has {} EFI System Partitions ({}); say which one with --boot-part", d.path, all.len(), all.iter().map(|p| p.name.as_str()).collect::<Vec<_>>().join(", "))),
    }
}

fn check_keep_esp(esp: &Part, src: &SourceSpec, d: &DiskModel, warnings: &mut Vec<String>) -> Result<(), String> {
    if let Some(why) = esp.in_use() {
        return Err(format!("keep-ESP: {} is {why}. A mounted FAT volume is not written behind the system's back; start the installer from the IYAKKI USB stick (boot-time install mode) or unmount it first.", esp.name));
    }
    if esp.fs != "fat32" {
        return Err(format!("keep-ESP: {} is {} (only FAT32 EFI System Partitions are supported in this version)", esp.name, if esp.fs.is_empty() { "not a recognisable filesystem".to_string() } else { esp.fs.clone() }));
    }
    let need = src.payload_bytes + 4 * MIB;
    match esp.fat_free {
        Some(f) if f < need => return Err(format!("keep-ESP: {} has {} free but the IYAKKI kernel and RAM disk need {}. Use the free-space mode (own boot partition) instead.", esp.name, human_size(f), human_size(need))),
        Some(_) => {}
        None => warnings.push(format!("the free space of {} could not be read; the writer checks it again before writing.", esp.name)),
    }
    if src.secure_boot {
        warnings.push("keep-ESP starts the kernel directly from the firmware's boot entry; with Secure Boot on the firmware will refuse the unsigned entry. Turn Secure Boot off, or use free-space mode.".into());
    }
    let _ = d;
    Ok(())
}

fn data_warn(bytes: u64, warnings: &mut Vec<String>) {
    if bytes < COMFORTABLE_DATA_BYTES {
        warnings.push(format!("the data partition is {}; 8 GiB or more is comfortable.", human_size(bytes)));
    }
}

fn free_space(d: &DiskModel, req: &Request, src: &SourceSpec, uefi: bool, plan: &mut Plan, warnings: &mut Vec<String>) -> Result<(), String> {
    if d.raw_data {
        return Err(format!("{} has data but no partition table (a filesystem on the whole disk). Free-space mode will not guess; use whole-disk mode or clear it yourself.", d.path));
    }
    let kind = match d.table {
        Some(k) => k,
        None => {
            let (k, why) = resolve_table(req.table, d.size_bytes, d.sector, uefi).map_err(|e| format!("{} cannot be used: {e}", d.path))?;
            plan.table_why = why;
            plan.new_table = true;
            plan.table_after = k;
            k
        }
    };
    if d.table.is_some() {
        plan.table_why = "the disk's own table is kept".into();
        if kind == TableKind::Mbr && d.sector != 512 {
            return Err(format!("{} cannot be used: MBR needs 512-byte sectors", d.path));
        }
    }
    let ss = d.sector as u64;
    let esp = if req.keep_esp { Some(find_esp(d, req)?.clone()) } else { None };
    if let Some(e) = &esp {
        check_keep_esp(e, src, d, warnings)?;
    }
    let gaps = d.free_gaps(kind);
    let gap = match req.gap_start {
        Some(s) => gaps.iter().find(|g| g.first_lba == s).cloned().ok_or_else(|| format!("no free space starts at sector {s} on {} (see the list of free space)", d.path))?,
        None => d.largest_gap(kind).ok_or_else(|| format!("{} has no free space (1 MiB or more). Installing next to another system needs unallocated space: shrink a partition with the other system's disk tool first (this version never resizes existing filesystems).", d.path))?,
    };
    let boot_bytes = match (&esp, req.boot_mib) {
        (Some(_), _) => 0,
        (None, Some(m)) => {
            if m * MIB < required_floor(src, d.sector) {
                return Err(format!("the boot partition must be at least {} for this image", human_size(required_floor(src, d.sector))));
            }
            m * MIB
        }
        (None, None) => plan.boot_required,
    };
    let room = gap.bytes(d.sector);
    let data_bytes = match req.data_mib {
        Some(m) => m * MIB,
        None => (room.saturating_sub(boot_bytes)) / MIB * MIB,
    };
    if data_bytes < MIN_DATA_BYTES {
        return Err(format!("the free space ({}) is too small: IYAKKI needs {} for boot plus at least {} for data. Free more space with the other system's disk tool (shrink a partition); this version never resizes existing filesystems.", human_size(room), human_size(boot_bytes), human_size(MIN_DATA_BYTES)));
    }
    if boot_bytes + data_bytes > room {
        return Err(format!("the sizes ({} boot + {} data) do not fit the free space ({}); nothing existing is shrunk", human_size(boot_bytes), human_size(data_bytes), human_size(room)));
    }
    // partition slots
    let new_count = if esp.is_some() { 1 } else { 2 };
    let used: Vec<u32> = d.parts.iter().map(|p| p.index).collect();
    let mut free_slots: Vec<u32> = Vec::new();
    let max_slot = if kind == TableKind::Gpt { 128 } else { 4 };
    for i in 1..=max_slot {
        if !used.contains(&i) {
            free_slots.push(i);
        }
    }
    if free_slots.len() < new_count {
        return Err(if kind == TableKind::Mbr {
            format!("{} uses {} of the 4 primary partitions an MBR disk can hold and IYAKKI needs {new_count} more. Remove or merge a partition with the other system's disk tool, or use the reuse mode.", d.path, used.len())
        } else {
            format!("{} has no free partition slot", d.path)
        });
    }
    if kind == TableKind::Mbr {
        let end = gap.first_lba * ss / 512 + (boot_bytes + data_bytes) / 512;
        if end > 0xFFFF_FFFF {
            return Err("the new partitions would end beyond 2 TiB, which an MBR cannot address".into());
        }
    }
    let mut cursor = gap.first_lba;
    let mut slots = free_slots.into_iter();
    if esp.is_none() {
        let i = slots.next().unwrap();
        let sectors = boot_bytes / ss;
        plan.steps.push(Step { action: Action::Create, part: format!("new {i}"), index: i, first_lba: cursor, sectors, bytes: boot_bytes, role: Some(Role::Boot), what: "IYAKKI-BOOT FAT32, EFI System (UEFI loader, GRUB, kernel, initial RAM disk)".into(), loses: String::new() });
        plan.boot_index = i;
        cursor += sectors;
    }
    let i = slots.next().unwrap();
    let sectors = data_bytes / ss;
    plan.steps.push(Step { action: Action::Create, part: format!("new {i}"), index: i, first_lba: cursor, sectors, bytes: data_bytes, role: Some(Role::Data), what: "IYAKKI-DATA (ext4 on first start)".into(), loses: String::new() });
    plan.data_index = i;
    let mut except = vec![];
    if let Some(e) = &esp {
        plan.boot_index = e.index;
        plan.keep_esp = true;
        plan.steps.push(Step { action: Action::AddFiles, part: e.name.clone(), index: e.index, first_lba: e.first_lba, sectors: e.sectors, bytes: e.bytes(d.sector), role: Some(Role::Boot), what: "existing EFI System Partition: only the folder EFI/IYAKKI (kernel, RAM disk) is added".into(), loses: String::new() });
        except.push(e.index);
        warnings.push("the existing EFI System Partition keeps its boot loaders; IYAKKI adds its own folder EFI/IYAKKI and its own UEFI boot entry. Nothing of the other system is overwritten.".into());
    }
    keep_rows(d, plan, &except);
    data_warn(data_bytes, warnings);
    if gaps.len() > 1 {
        warnings.push(format!("{} separate areas of free space exist; the one starting at sector {} is used.", gaps.len(), gap.first_lba));
    }
    if d.table.is_some() && kind == TableKind::Gpt && d.notes.iter().any(|n| n.contains("backup")) {
        warnings.push("the backup copy of the partition table was missing or damaged; it is rewritten at the end of the disk.".into());
    }
    if !plan.new_table {
        warnings.push("the BIOS (legacy) boot of this shared disk is not set up: IYAKKI starts through UEFI, from the firmware's boot menu. (Legacy BIOS on a shared disk is planned.)".into());
    }
    Ok(())
}

fn required_floor(src: &SourceSpec, sector: u32) -> u64 {
    // the boot volume holds the kernel and RAM disk at least; the image's own size is the comfortable value
    (src.payload_bytes * 5 / 4 + 40 * MIB).max(96 * MIB).min(src.boot_required(sector))
}

fn reuse(d: &DiskModel, req: &Request, src: &SourceSpec, plan: &mut Plan, warnings: &mut Vec<String>) -> Result<(), String> {
    if d.table.is_none() {
        return Err(format!("{} has no partition table, so there are no partitions to reuse", d.path));
    }
    let data_i = req.data_part.ok_or("reuse mode needs the DATA partition (--data-part N)")?;
    let data = pick_part(d, data_i, "data partition")?.clone();
    refuse_in_use(&data, "data partition")?;
    if data.bytes(d.sector) < MIN_DATA_BYTES {
        return Err(format!("data partition: {} is only {}; at least {} is needed", data.name, human_size(data.bytes(d.sector)), human_size(MIN_DATA_BYTES)));
    }
    if req.keep_esp {
        let esp = find_esp(d, req)?.clone();
        if esp.index == data.index {
            return Err("the boot and data partitions must be different".into());
        }
        check_keep_esp(&esp, src, d, warnings)?;
        plan.boot_index = esp.index;
        plan.keep_esp = true;
        plan.steps.push(Step { action: Action::AddFiles, part: esp.name.clone(), index: esp.index, first_lba: esp.first_lba, sectors: esp.sectors, bytes: esp.bytes(d.sector), role: Some(Role::Boot), what: "existing EFI System Partition: only the folder EFI/IYAKKI (kernel, RAM disk) is added".into(), loses: String::new() });
        warnings.push("the existing EFI System Partition keeps its boot loaders; IYAKKI adds its own folder EFI/IYAKKI and its own UEFI boot entry.".into());
        plan.steps.push(format_step(&data, d.sector, Role::Data, "IYAKKI-DATA (ext4 on first start)"));
        plan.data_index = data.index;
        keep_rows(d, plan, &[esp.index, data.index]);
    } else {
        let boot_i = req.boot_part.ok_or("reuse mode needs the BOOT partition (--boot-part N), or --keep-esp")?;
        if boot_i == data_i {
            return Err("the boot and data partitions must be different".into());
        }
        let boot = pick_part(d, boot_i, "boot partition")?.clone();
        refuse_in_use(&boot, "boot partition")?;
        if boot.bytes(d.sector) < plan.boot_required {
            return Err(format!("boot partition: {} is {} but this image needs {} for the boot volume", boot.name, human_size(boot.bytes(d.sector)), human_size(plan.boot_required)));
        }
        if boot.is_esp() && d.parts.iter().filter(|p| p.is_esp()).count() == 1 {
            warnings.push(format!("{} is the disk's only EFI System Partition: formatting it removes the other system's boot loader. Use --keep-esp to add IYAKKI beside it instead.", boot.name));
        }
        plan.steps.push(format_step(&boot, d.sector, Role::Boot, "IYAKKI-BOOT FAT32, EFI System (UEFI loader, GRUB, kernel, initial RAM disk)"));
        plan.steps.push(format_step(&data, d.sector, Role::Data, "IYAKKI-DATA (ext4 on first start)"));
        plan.boot_index = boot.index;
        plan.data_index = data.index;
        keep_rows(d, plan, &[boot.index, data.index]);
    }
    plan.table_why = "the disk's own table is kept; only the two entries' types and names change".into();
    data_warn(data.bytes(d.sector), warnings);
    warnings.push("the BIOS (legacy) boot of this shared disk is not set up: IYAKKI starts through UEFI, from the firmware's boot menu.".into());
    Ok(())
}

fn format_step(p: &Part, ss: u32, role: Role, what: &str) -> Step {
    let loses = if p.fs.is_empty() { format!("its contents ({})", p.kind) } else { format!("its {} filesystem{}", p.fs, if p.label.is_empty() { String::new() } else { format!(" “{}”", p.label) }) };
    Step { action: Action::Format, part: p.name.clone(), index: p.index, first_lba: p.first_lba, sectors: p.sectors, bytes: p.bytes(ss), role: Some(role), what: what.into(), loses }
}

/// The plan's own consistency, checked again after building (and by the writer against a fresh read of the disk): the rules
/// that protect other systems, as code.
pub fn check_plan(d: &DiskModel, plan: &Plan) -> Result<(), String> {
    let al = d.al();
    let mut spans: Vec<(u64, u64, String)> = Vec::new();
    for st in &plan.steps {
        let existing = d.parts.iter().find(|p| p.name == st.part);
        match st.action {
            Action::Create => {
                if existing.is_some() {
                    return Err(format!("internal error: {} is created but exists", st.part));
                }
                if st.first_lba % al != 0 || st.sectors % al != 0 {
                    return Err(format!("internal error: new partition {} is not aligned to 1 MiB", st.index));
                }
            }
            Action::Format | Action::Delete | Action::Keep | Action::AddFiles => {
                let p = existing.ok_or_else(|| format!("internal error: {} {} is not on the disk", st.action.word(), st.part))?;
                if (p.first_lba, p.sectors) != (st.first_lba, st.sectors) {
                    return Err(format!("internal error: {} changes position", st.part));
                }
                if matches!(st.action, Action::Format | Action::Delete) && p.extended {
                    return Err(format!("{} is an extended partition; refused", st.part));
                }
                if matches!(st.action, Action::Format | Action::Delete) {
                    if let Some(m) = p.mounts.iter().find(|m| drives::critical_mount(m) || m.contains("iyakki")) {
                        return Err(format!("{} is mounted at {m} (the running system) and is never changed", st.part));
                    }
                }
            }
        }
        if st.action != Action::Delete {
            spans.push((st.first_lba, st.first_lba + st.sectors, st.part.clone()));
        }
    }
    spans.sort();
    if let Some(w) = spans.windows(2).find(|w| w[1].0 < w[0].1) {
        return Err(format!("internal error: {} and {} overlap", w[0].2, w[1].2));
    }
    // every existing partition is either in the plan (keep/format/delete/add-files) or the plan is wrong
    for p in &d.parts {
        if !plan.steps.iter().any(|s| s.part == p.name) && !(plan.request.mode == Mode::WholeDisk) {
            return Err(format!("internal error: {} is missing from the plan", p.name));
        }
    }
    // deletes only in whole-disk mode, and then every partition
    let deletes = plan.steps.iter().filter(|s| s.action == Action::Delete).count();
    if plan.request.mode != Mode::WholeDisk && deletes > 0 {
        return Err("internal error: only the whole-disk mode deletes partitions".into());
    }
    if plan.request.mode == Mode::WholeDisk && deletes != d.parts.len() {
        return Err("internal error: whole-disk mode must name every partition it deletes".into());
    }
    if plan.table_after == TableKind::Gpt && !plan.new_table {
        let (lo, hi) = d.usable(TableKind::Gpt);
        if let Some(st) = plan.steps.iter().find(|s| s.action == Action::Create && (s.first_lba < lo || s.first_lba + s.sectors > hi)) {
            return Err(format!("internal error: new partition {} is outside the usable area", st.index));
        }
    }
    Ok(())
}

// ---------------------------------------------------------------------------------------------------------------------------
// the disk list as JSON (the privileged helper hands it to the window installer, which plans in its own process)

fn hex16(g: &Guid) -> String {
    g.iter().map(|b| format!("{b:02x}")).collect()
}

fn unhex16(s: &str) -> Option<Guid> {
    if s.len() != 32 {
        return None;
    }
    let mut g = [0u8; 16];
    for (i, b) in g.iter_mut().enumerate() {
        *b = u8::from_str_radix(s.get(2 * i..2 * i + 2)?, 16).ok()?;
    }
    Some(g)
}

impl Part {
    pub fn to_json(&self) -> Json {
        Json::obj(vec![
            ("index", Json::num(self.index as u64)),
            ("name", Json::str(&self.name)),
            ("first_lba", Json::num(self.first_lba)),
            ("sectors", Json::num(self.sectors)),
            ("kind", Json::str(&self.kind)),
            ("type_guid", self.type_guid.as_ref().map_or(Json::Null, |g| Json::str(hex16(g)))),
            ("mbr_type", self.mbr_type.map_or(Json::Null, |t| Json::num(t as u64))),
            ("label", Json::str(&self.label)),
            ("fs", Json::str(&self.fs)),
            ("fat_free", self.fat_free.map_or(Json::Null, Json::num)),
            ("mounts", Json::Arr(self.mounts.iter().map(Json::str).collect())),
            ("swap", Json::Bool(self.swap)),
            ("extended", Json::Bool(self.extended)),
        ])
    }

    pub fn from_json(j: &Json) -> Result<Part, String> {
        let n = |k: &str| j.get(k).and_then(Json::as_u64).ok_or_else(|| format!("partition: {k} missing"));
        let s = |k: &str| j.get(k).and_then(Json::as_str).unwrap_or("").to_string();
        Ok(Part {
            index: n("index")? as u32,
            name: s("name"),
            first_lba: n("first_lba")?,
            sectors: n("sectors")?,
            kind: s("kind"),
            type_guid: j.get("type_guid").and_then(Json::as_str).and_then(unhex16),
            mbr_type: j.get("mbr_type").and_then(Json::as_u64).map(|t| t as u8),
            label: s("label"),
            fs: s("fs"),
            fat_free: j.get("fat_free").and_then(Json::as_u64),
            mounts: j.get("mounts").and_then(Json::as_arr).map(|a| a.iter().filter_map(|m| m.as_str().map(String::from)).collect()).unwrap_or_default(),
            swap: j.get("swap").and_then(Json::as_bool).unwrap_or(false),
            extended: j.get("extended").and_then(Json::as_bool).unwrap_or(false),
        })
    }
}

impl DiskModel {
    pub fn to_json(&self) -> Json {
        Json::obj(vec![
            ("name", Json::str(&self.name)),
            ("path", Json::str(&self.path)),
            ("model", Json::str(&self.model)),
            ("serial", Json::str(&self.serial)),
            ("size_bytes", Json::num(self.size_bytes)),
            ("sector", Json::num(self.sector as u64)),
            ("table", Json::str(match self.table { None => "none", Some(TableKind::Gpt) => "gpt", Some(TableKind::Mbr) => "mbr" })),
            ("notes", Json::Arr(self.notes.iter().map(Json::str).collect())),
            ("parts", Json::Arr(self.parts.iter().map(Part::to_json).collect())),
            ("removable", Json::Bool(self.removable)),
            ("usb", Json::Bool(self.usb)),
            ("read_only", Json::Bool(self.read_only)),
            ("running_system", Json::Bool(self.running_system)),
            ("booted_from", Json::Bool(self.booted_from)),
            ("raw_data", Json::Bool(self.raw_data)),
            ("unusual_gpt", Json::Bool(self.unusual_gpt)),
            ("disk_guid", self.disk_guid.as_ref().map_or(Json::Null, |g| Json::str(hex16(g)))),
            ("fingerprint", Json::str(&self.fingerprint)),
        ])
    }

    pub fn from_json(j: &Json) -> Result<DiskModel, String> {
        let s = |k: &str| j.get(k).and_then(Json::as_str).unwrap_or("").to_string();
        let b = |k: &str| j.get(k).and_then(Json::as_bool).unwrap_or(false);
        Ok(DiskModel {
            name: s("name"),
            path: s("path"),
            model: s("model"),
            serial: s("serial"),
            size_bytes: j.get("size_bytes").and_then(Json::as_u64).ok_or("disk: size_bytes missing")?,
            sector: j.get("sector").and_then(Json::as_u64).ok_or("disk: sector missing")? as u32,
            table: match j.get("table").and_then(Json::as_str) {
                Some("gpt") => Some(TableKind::Gpt),
                Some("mbr") => Some(TableKind::Mbr),
                _ => None,
            },
            notes: j.get("notes").and_then(Json::as_arr).map(|a| a.iter().filter_map(|m| m.as_str().map(String::from)).collect()).unwrap_or_default(),
            parts: j.get("parts").and_then(Json::as_arr).map(|a| a.iter().map(Part::from_json).collect::<Result<Vec<_>, _>>()).transpose()?.unwrap_or_default(),
            removable: b("removable"),
            usb: b("usb"),
            read_only: b("read_only"),
            running_system: b("running_system"),
            booted_from: b("booted_from"),
            raw_data: b("raw_data"),
            unusual_gpt: b("unusual_gpt"),
            disk_guid: j.get("disk_guid").and_then(Json::as_str).and_then(unhex16),
            fingerprint: s("fingerprint"),
        })
    }
}

impl SourceSpec {
    pub fn to_json(&self) -> Json {
        Json::obj(vec![("path", Json::str(self.path.display().to_string())), ("boot_bytes", Json::num(self.boot_bytes)), ("payload_bytes", Json::num(self.payload_bytes)), ("secure_boot", Json::Bool(self.secure_boot))])
    }
    pub fn from_json(j: &Json) -> Result<SourceSpec, String> {
        Ok(SourceSpec { path: PathBuf::from(j.get("path").and_then(Json::as_str).ok_or("source: path missing")?), boot_bytes: j.get("boot_bytes").and_then(Json::as_u64).ok_or("source: boot_bytes missing")?, payload_bytes: j.get("payload_bytes").and_then(Json::as_u64).unwrap_or(0), secure_boot: j.get("secure_boot").and_then(Json::as_bool).unwrap_or(false) })
    }
}
