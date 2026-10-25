//! DISKS partition manager: make a new partition table (GPT or MBR), add, delete, format and rename partitions (docs/DISKS.md).
//!
//! Built from our own code: the GPT writer and reader of `iyakki-storage`, an MBR writer, a FAT32 formatter, label changes for
//! ext2/3/4 and FAT, and `mke2fs` (already part of the image, BOUNDARY.md) for ext4. exFAT and NTFS formatting and any
//! resizing are not supported yet, and say so: nothing is faked.
//!
//! Every change goes through [`Disks::apply_partition`], which checks again, on a fresh look at the machine, that:
//! the person is an administrator; the drive is not the system disk (never offered, never written: the engine refuses); nothing
//! on it is mounted, used as swap or held by another driver; the drive is not write-protected; the person typed the drive's name
//! or the last four characters of its serial number; and pressed the second confirmation. The audit log gets the drive's name,
//! size and serial and what was done, never contents. [`Disks::plan_partition`] is the dry run: the exact steps, nothing written.

use iyakki_sec::policy::Role;
use iyakki_storage::gpt::{self, Entry, Guid};

use crate::disks::{Actor, AuditFn, Disk, Disks, Fs, Snapshot, Table};

/// One mebibyte: partitions start and end on it.
const MIB: u64 = 1 << 20;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FormatFs {
    Ext4,
    Fat32,
    Exfat,
    Ntfs,
}

impl FormatFs {
    pub fn name(self) -> &'static str {
        match self {
            FormatFs::Ext4 => "ext4",
            FormatFs::Fat32 => "FAT32",
            FormatFs::Exfat => "exFAT",
            FormatFs::Ntfs => "NTFS",
        }
    }

    /// Why this cannot be done (the honest answer), or `None`.
    pub fn unsupported(self) -> Option<&'static str> {
        match self {
            FormatFs::Exfat => Some("Formatting as exFAT is not supported yet: IYAKKI has no permissive in-house exFAT formatter. Use FAT32 (files up to 4 GB) or ext4, or format it on a Mac or Windows computer."),
            FormatFs::Ntfs => Some("Formatting as NTFS is not supported yet: IYAKKI has no permissive in-house NTFS formatter. Use FAT32 or ext4, or format it on a Windows computer."),
            _ => None,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PartOp {
    NewTable(Table),
    /// A new partition in the largest free space that fits (`None` = all of the largest free space), optionally formatted.
    Add { size_bytes: Option<u64>, fs: Option<FormatFs>, label: String },
    Delete { number: u32 },
    Format { number: u32, fs: FormatFs, label: String },
    Rename { number: u32, label: String },
    Resize { number: u32, size_bytes: u64 },
    Reread,
}

impl PartOp {
    pub fn name(&self) -> &'static str {
        match self {
            PartOp::NewTable(_) => "new_table",
            PartOp::Add { .. } => "add",
            PartOp::Delete { .. } => "delete",
            PartOp::Format { .. } => "format",
            PartOp::Rename { .. } => "rename",
            PartOp::Resize { .. } => "resize",
            PartOp::Reread => "reread",
        }
    }

    /// Does it change what is on the drive?
    pub fn destructive(&self) -> bool {
        !matches!(self, PartOp::Reread)
    }
}

/// What the person did to confirm: typed the drive's name or the last four characters of its serial, and pressed the second button.
#[derive(Clone, Debug, Default)]
pub struct Confirm {
    pub typed: String,
    pub second: bool,
}

pub fn check_confirmation(d: &Disk, c: &Confirm) -> Result<(), String> {
    let t = c.typed.trim();
    if t.is_empty() {
        return Err(format!("Type the drive's name ({}){} to confirm.", d.name, if d.serial_tail().is_empty() { String::new() } else { format!(" or the last 4 characters of its serial number ({})", d.serial_tail()) }));
    }
    let tail = d.serial_tail();
    if !(t.eq_ignore_ascii_case(&d.name) || t.eq_ignore_ascii_case(&d.path) || (!tail.is_empty() && t.eq_ignore_ascii_case(&tail))) {
        return Err(format!("\"{t}\" is not this drive's name ({}){}. Nothing was changed.", d.name, if tail.is_empty() { String::new() } else { format!(" or the last 4 characters of its serial ({tail})") }));
    }
    if !c.second {
        return Err("Press the second confirmation button to go on. Nothing was changed.".into());
    }
    Ok(())
}

// ---------------------------------------------------------------------------------------------------------------------
// the table as it is on the drive
// ---------------------------------------------------------------------------------------------------------------------

/// One partition as the table says (sectors of the drive's logical size).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LPart {
    pub number: u32,
    pub first: u64,
    pub last: u64,
    pub type_name: String,
    pub name: String,
    /// MBR type byte, or 0 for GPT.
    pub mbr_type: u8,
    pub type_guid: Guid,
    pub guid: Guid,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Layout {
    pub table: Table,
    pub sector: u32,
    pub total: u64,
    pub disk_guid: Guid,
    pub mbr_sig: [u8; 4],
    pub parts: Vec<LPart>,
}

impl Layout {
    pub fn bytes(&self, sectors: u64) -> u64 {
        sectors * self.sector as u64
    }

    fn align(&self) -> u64 {
        (MIB / self.sector as u64).max(1)
    }

    /// First and last sector a partition may use.
    fn usable(&self) -> (u64, u64) {
        match self.table {
            Table::Gpt => {
                let t = gpt::Table { sector_size: self.sector, total_sectors: self.total, disk_guid: self.disk_guid, entries: vec![] };
                (t.first_usable(), t.last_usable())
            }
            _ => (1, self.total.saturating_sub(1).min(0xFFFF_FFFF)),
        }
    }

    /// The free stretches (first, last sector), 1 MiB aligned at their start.
    pub fn free(&self) -> Vec<(u64, u64)> {
        let (lo, hi) = self.usable();
        let a = self.align();
        let up = |x: u64| x.div_ceil(a) * a;
        let mut parts: Vec<&LPart> = self.parts.iter().collect();
        parts.sort_by_key(|p| p.first);
        let mut out = Vec::new();
        let mut cur = up(lo);
        for p in parts {
            if p.first > cur {
                out.push((cur, p.first - 1));
            }
            cur = cur.max(up(p.last + 1));
        }
        let end = ((hi + 1) / a * a).saturating_sub(1);
        if end > cur && end >= cur {
            out.push((cur, end));
        }
        out.retain(|(f, l)| l >= f && (l - f + 1) >= a);
        out
    }
}

fn mbr_type_name(t: u8) -> &'static str {
    match t {
        0x0B | 0x0C => "FAT32",
        0x83 => "Linux",
        0x07 => "NTFS/exFAT",
        0x82 => "Linux swap",
        0xEF => "EFI System",
        _ => "other",
    }
}

fn le32(b: &[u8], o: usize) -> u32 {
    u32::from_le_bytes([b[o], b[o + 1], b[o + 2], b[o + 3]])
}

/// Read the real table of a drive (not the kernel's idea of it).
pub fn read_layout(d: &Disks, disk: &Disk) -> Result<Layout, String> {
    let env = &*d.env;
    let len = env.dev_len(&disk.path);
    let size = if len > 0 { len } else { disk.size_bytes };
    let total = size / disk.sector as u64;
    let mut l = Layout { table: Table::None, sector: disk.sector, total, disk_guid: [0; 16], mbr_sig: [0; 4], parts: vec![] };
    if total < 4096 {
        return Err("The drive is too small to partition.".into());
    }
    let s0 = env.dev_read(&disk.path, 0, 512)?;
    if s0.len() < 512 {
        return Err("Could not read the start of the drive.".into());
    }
    let mbr_ok = s0[510..512] == [0x55, 0xAA];
    let protective = mbr_ok && (0..4).any(|i| s0[446 + 16 * i + 4] == 0xEE);
    let hdr = env.dev_read(&disk.path, disk.sector as u64, 8)?;
    if protective || &hdr[..] == gpt::SIGNATURE {
        let mut r = crate::disks::dev_reader(env, &disk.path, size);
        let p = gpt::read_gpt(&mut r, disk.sector, total).map_err(|e| format!("The GPT on this drive is damaged ({e}). Make a new partition table to start over."))?;
        l.table = Table::Gpt;
        l.disk_guid = p.table.disk_guid;
        l.mbr_sig = [s0[440], s0[441], s0[442], s0[443]];
        l.parts = p.table.entries.iter().map(|e| LPart { number: e.index, first: e.first_lba, last: e.last_lba, type_name: gpt::type_name(&e.type_guid), name: e.name.clone(), mbr_type: 0, type_guid: e.type_guid, guid: e.guid }).collect();
        return Ok(l);
    }
    if mbr_ok {
        l.table = Table::Mbr;
        l.mbr_sig = [s0[440], s0[441], s0[442], s0[443]];
        for i in 0..4 {
            let e = 446 + 16 * i;
            let (t, start, n) = (s0[e + 4], le32(&s0, e + 8) as u64, le32(&s0, e + 12) as u64);
            if t == 0 || n == 0 {
                continue;
            }
            if matches!(t, 0x05 | 0x0F | 0x85) {
                return Err("This MBR drive has an extended partition. Changing such a table is not supported yet; use a new partition table (this erases everything), or change it on a computer that has a partition editor.".into());
            }
            l.parts.push(LPart { number: i as u32 + 1, first: start, last: start + n - 1, type_name: mbr_type_name(t).into(), name: String::new(), mbr_type: t, type_guid: [0; 16], guid: [0; 16] });
        }
    }
    Ok(l)
}

// ---------------------------------------------------------------------------------------------------------------------
// building tables
// ---------------------------------------------------------------------------------------------------------------------

/// "Microsoft basic data" (FAT32, exFAT, NTFS).
fn type_basic_data() -> Guid {
    gpt::guid_from_str("EBD0A0A2-B9E5-4433-87C0-68B6B72699C7").unwrap_or(gpt::TYPE_LINUX_DATA)
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Step {
    Zero { dev: String, offset: u64, len: u64, what: String },
    Write { dev: String, offset: u64, data: Vec<u8>, what: String },
    Mke2fs { dev: String, label: String, uuid: String, what: String },
    Reread { dev: String },
    WaitNode { dev: String },
    Sync,
}

impl Step {
    fn text(&self) -> String {
        match self {
            Step::Zero { offset, len, what, .. } => format!("{what} (clear bytes {offset} to {})", offset + len),
            Step::Write { offset, data, what, .. } => format!("{what} (write {} bytes at byte {offset})", data.len()),
            Step::Mke2fs { what, .. } => what.clone(),
            Step::Reread { dev } => format!("Ask the kernel to read the partition table of {dev} again"),
            Step::WaitNode { dev } => format!("Wait for {dev} to appear"),
            Step::Sync => "Flush everything to the drive".into(),
        }
    }
}

fn gpt_steps(disk: &Disk, l: &Layout, entries: Vec<Entry>, fresh: bool, boot_sig: [u8; 4]) -> Result<Vec<Step>, String> {
    let t = gpt::Table { sector_size: l.sector, total_sectors: l.total, disk_guid: l.disk_guid, entries };
    t.check()?;
    let dev = disk.path.clone();
    let ss = l.sector as u64;
    let mut v = Vec::new();
    if fresh {
        v.push(Step::Write { dev: dev.clone(), offset: 0, data: t.protective_mbr(&[], boot_sig), what: "Write a protective MBR".into() });
    }
    v.push(Step::Write { dev: dev.clone(), offset: t.primary_entries_lba() * ss, data: t.entries_bytes(), what: "Write the GPT partition entries".into() });
    v.push(Step::Write { dev: dev.clone(), offset: t.backup_entries_lba() * ss, data: t.entries_bytes(), what: "Write the backup partition entries".into() });
    v.push(Step::Write { dev: dev.clone(), offset: ss, data: t.header_bytes(false), what: "Write the GPT header".into() });
    v.push(Step::Write { dev, offset: t.last_lba() * ss, data: t.header_bytes(true), what: "Write the backup GPT header".into() });
    Ok(v)
}

fn mbr_sector(l: &Layout, parts: &[LPart], sig: [u8; 4], keep_code: &[u8]) -> Result<Vec<u8>, String> {
    let mut m = vec![0u8; 512];
    let n = keep_code.len().min(440);
    m[..n].copy_from_slice(&keep_code[..n]);
    m[440..444].copy_from_slice(&sig);
    for p in parts {
        if !(1..=4).contains(&p.number) {
            return Err("An MBR holds four partitions at most.".into());
        }
        if p.last > 0xFFFF_FFFF {
            return Err("An MBR cannot describe space beyond 2 TB; use GPT for this drive.".into());
        }
        let e = 446 + 16 * (p.number as usize - 1);
        m[e + 1..e + 4].copy_from_slice(&[0xFE, 0xFF, 0xFF]);
        m[e + 4] = p.mbr_type;
        m[e + 5..e + 8].copy_from_slice(&[0xFE, 0xFF, 0xFF]);
        m[e + 8..e + 12].copy_from_slice(&(p.first as u32).to_le_bytes());
        m[e + 12..e + 16].copy_from_slice(&((p.last - p.first + 1) as u32).to_le_bytes());
    }
    let _ = l;
    m[510] = 0x55;
    m[511] = 0xAA;
    Ok(m)
}

fn random_bytes<const N: usize>() -> [u8; N] {
    let g = gpt::random_guid();
    let mut out = [0u8; N];
    for (i, o) in out.iter_mut().enumerate() {
        *o = g[i % 16];
    }
    out
}

fn uuid_text() -> String {
    gpt::guid_to_string(&gpt::random_guid()).to_lowercase()
}

// ---------------------------------------------------------------------------------------------------------------------
// FAT32, written in-house
// ---------------------------------------------------------------------------------------------------------------------

fn put16(b: &mut [u8], o: usize, v: u16) {
    b[o..o + 2].copy_from_slice(&v.to_le_bytes());
}
fn put32(b: &mut [u8], o: usize, v: u32) {
    b[o..o + 4].copy_from_slice(&v.to_le_bytes());
}

/// A valid FAT volume label (up to 11 characters, upper case, none of `* ? . , ; : / \ | + = < > [ ] "`), or the reason it is not.
pub fn fat_label(label: &str) -> Result<String, String> {
    let l = label.trim().to_uppercase();
    if l.chars().count() > 11 {
        return Err("A FAT label has 11 characters at most.".into());
    }
    if l.chars().any(|c| !c.is_ascii() || c.is_ascii_control() || "*?.,;:/\\|+=<>[]\"".contains(c)) {
        return Err("A FAT label may use letters, digits, spaces and - _ only.".into());
    }
    Ok(l)
}

/// The writes that make an empty FAT32 volume of `size_bytes` (one partition starting at `start_lba` of the disk).
pub fn fat32_steps(dev: &str, size_bytes: u64, sector: u32, start_lba: u64, label: &str, serial: u32) -> Result<Vec<Step>, String> {
    let bps = sector as u64;
    let total = size_bytes / bps;
    if total > u32::MAX as u64 {
        return Err("FAT32 cannot be larger than 2 TB here; use ext4.".into());
    }
    let label = fat_label(label)?;
    let want = if size_bytes <= 260 * MIB { 512 } else if size_bytes <= (8 << 30) { 4096 } else if size_bytes <= (16 << 30) { 8192 } else if size_bytes <= (32 << 30) { 16384 } else { 32768 };
    let mut spc = (want / bps).max(1);
    let reserved = 32u64;
    let fat_for = |spc: u64| -> (u64, u64) {
        let mut fat = 1u64;
        loop {
            let clusters = total.saturating_sub(reserved + 2 * fat) / spc;
            let need = ((clusters + 2) * 4).div_ceil(bps);
            if need <= fat {
                return (fat, clusters);
            }
            fat = need;
        }
    };
    let (mut fat, mut clusters) = fat_for(spc);
    while clusters < 65_525 && spc > 1 {
        spc /= 2;
        (fat, clusters) = fat_for(spc);
    }
    if clusters < 65_525 {
        return Err("This partition is too small for FAT32 (about 33 MB at least). Use ext4.".into());
    }
    let mut bs = vec![0u8; bps as usize];
    bs[0..3].copy_from_slice(&[0xEB, 0x58, 0x90]);
    bs[3..11].copy_from_slice(b"IYAKKI  ");
    put16(&mut bs, 11, bps as u16);
    bs[13] = spc as u8;
    put16(&mut bs, 14, reserved as u16);
    bs[16] = 2;
    bs[21] = 0xF8;
    put16(&mut bs, 24, 63);
    put16(&mut bs, 26, 255);
    put32(&mut bs, 28, start_lba.min(u32::MAX as u64) as u32);
    put32(&mut bs, 32, total as u32);
    put32(&mut bs, 36, fat as u32);
    put32(&mut bs, 44, 2);
    put16(&mut bs, 48, 1);
    put16(&mut bs, 50, 6);
    bs[64] = 0x80;
    bs[66] = 0x29;
    put32(&mut bs, 67, serial);
    let mut lab = [b' '; 11];
    if label.is_empty() {
        lab.copy_from_slice(b"NO NAME    ");
    } else {
        lab[..label.len()].copy_from_slice(label.as_bytes());
    }
    bs[71..82].copy_from_slice(&lab);
    bs[82..90].copy_from_slice(b"FAT32   ");
    bs[510] = 0x55;
    bs[511] = 0xAA;
    let mut fi = vec![0u8; bps as usize];
    put32(&mut fi, 0, 0x4161_5252);
    put32(&mut fi, 484, 0x6141_7272);
    put32(&mut fi, 488, (clusters - 1) as u32);
    put32(&mut fi, 492, 3);
    fi[510] = 0x55;
    fi[511] = 0xAA;
    let mut fat0 = vec![0u8; bps as usize];
    put32(&mut fat0, 0, 0x0FFF_FFF8);
    put32(&mut fat0, 4, 0x0FFF_FFFF);
    put32(&mut fat0, 8, 0x0FFF_FFFF);
    let data_start = reserved + 2 * fat;
    let meta = (data_start + spc) * bps;
    let d = dev.to_string();
    let mut v = vec![Step::Zero { dev: d.clone(), offset: 0, len: meta, what: "Clear the FAT32 structures".into() }];
    for (off, data, what) in [(0, bs.clone(), "Write the FAT32 boot sector"), (bps, fi.clone(), "Write the FAT32 information sector"), (6 * bps, bs, "Write the backup boot sector"), (7 * bps, fi, "Write the backup information sector"), (reserved * bps, fat0.clone(), "Write the first file allocation table"), ((reserved + fat) * bps, fat0, "Write the second file allocation table")] {
        v.push(Step::Write { dev: d.clone(), offset: off, data, what: what.into() });
    }
    if !label.is_empty() {
        let mut e = vec![0u8; 32];
        e[..11].copy_from_slice(&lab);
        e[11] = 0x08;
        v.push(Step::Write { dev: d, offset: data_start * bps, data: e, what: "Write the volume label".into() });
    }
    Ok(v)
}

/// CRC-32C as the ext4 superblock uses it (no final inversion).
pub(crate) fn crc32c_raw(mut crc: u32, data: &[u8]) -> u32 {
    for b in data {
        crc ^= *b as u32;
        for _ in 0..8 {
            crc = if crc & 1 != 0 { (crc >> 1) ^ 0x82F6_3B78 } else { crc >> 1 };
        }
    }
    crc
}

// ---------------------------------------------------------------------------------------------------------------------
// the plan
// ---------------------------------------------------------------------------------------------------------------------

#[derive(Clone, Debug)]
pub struct Plan {
    pub disk: String,
    pub identity: String,
    pub op: PartOp,
    /// What will be lost, one line each.
    pub lost: Vec<String>,
    /// The layout afterwards, one line each.
    pub after: Vec<String>,
    pub steps: Vec<Step>,
    pub warnings: Vec<String>,
    audit: String,
}

impl Plan {
    /// Everything a person is shown before confirming (the same shape as the installer's plan).
    pub fn render(&self) -> String {
        let mut s = format!("{} on {}\n", self.headline(), self.identity);
        if !self.lost.is_empty() {
            s.push_str("What will be lost:\n");
            for l in &self.lost {
                s.push_str(&format!("  {l}\n"));
            }
        }
        if !self.after.is_empty() {
            s.push_str("\nThe drive will look like this:\n");
            for l in &self.after {
                s.push_str(&format!("  {l}\n"));
            }
        }
        s.push_str("\nThe plan:\n");
        for (i, st) in self.steps.iter().enumerate() {
            s.push_str(&format!("  {}. {}\n", i + 1, st.text()));
        }
        for w in &self.warnings {
            s.push_str(&format!("\nNote: {w}\n"));
        }
        s
    }

    pub fn headline(&self) -> String {
        match &self.op {
            PartOp::NewTable(t) => format!("Make a new {} partition table, erasing EVERYTHING", t.name()),
            PartOp::Add { .. } => "Add a partition".into(),
            PartOp::Delete { number } => format!("Delete partition {number}"),
            PartOp::Format { number, fs, .. } => format!("Format partition {number} as {}", fs.name()),
            PartOp::Rename { number, .. } => format!("Rename partition {number}"),
            PartOp::Resize { .. } => "Resize a partition".into(),
            PartOp::Reread => "Read the partition table again".into(),
        }
    }

    pub fn writes_anything(&self) -> bool {
        self.steps.iter().any(|s| matches!(s, Step::Zero { .. } | Step::Write { .. } | Step::Mke2fs { .. }))
    }
}

fn layout_lines(l: &Layout) -> Vec<String> {
    let mut v: Vec<String> = l.parts.iter().map(|p| format!("{}  {:>9}  {}{}", p.number, iyakki_storage::human(l.bytes(p.last - p.first + 1)), p.type_name, if p.name.is_empty() { String::new() } else { format!("  “{}”", p.name) })).collect();
    for (f, last) in l.free() {
        v.push(format!("free  {:>7}", iyakki_storage::human(l.bytes(last - f + 1))));
    }
    v
}

impl Disks {
    /// Why this drive may not be changed, or `None`. The engine's own, hard rules: administrators only, never the system disk,
    /// nothing mounted, no swap, nobody holding it, not write-protected.
    pub fn refusal(&self, who: &Actor, d: &Disk) -> Option<String> {
        if who.role != Role::Admin {
            return Some("Only an administrator can change partitions.".into());
        }
        if d.system {
            return Some(format!("{} {}. The drive that holds the running system is never changed.", d.name, d.system_why));
        }
        if d.read_only {
            return Some(format!("{} is write-protected.", d.name));
        }
        let mounted: Vec<String> = d.parts.iter().flat_map(|p| p.mountpoints.iter().cloned()).collect();
        if !mounted.is_empty() {
            return Some(format!("{} is open at {}. Unmount (or eject) it first.", d.name, mounted.join(", ")));
        }
        if let Some(p) = d.parts.iter().find(|p| p.swap_in_use) {
            return Some(format!("{} is in use as swap space.", p.name));
        }
        if !d.holders.is_empty() {
            return Some(format!("{} is in use by {} (a RAID, LVM or encrypted volume). Close that first.", d.name, d.holders.join(", ")));
        }
        None
    }

    /// The dry run: what would be done to `disk_name`, step by step. Nothing is written.
    pub fn plan_partition(&self, who: &Actor, disk_name: &str, op: &PartOp) -> Result<Plan, String> {
        let snap = self.scan();
        self.plan_in(&snap, who, disk_name, op)
    }

    fn plan_in(&self, snap: &Snapshot, who: &Actor, disk_name: &str, op: &PartOp) -> Result<Plan, String> {
        let d = snap.disk(disk_name).ok_or_else(|| format!("{disk_name}: there is no such drive."))?;
        if let Some(r) = self.refusal(who, d) {
            return Err(r);
        }
        let identity = d.identity();
        let audit = |extra: &str| format!("{} op={}{}", d.audit_id(), op.name(), extra);
        let mut plan = Plan { disk: d.name.clone(), identity: identity.clone(), op: op.clone(), lost: vec![], after: vec![], steps: vec![], warnings: vec![], audit: audit("") };
        let part_dev = |n: u32| iyakki_storage::partition_name(&d.name, n);
        match op {
            PartOp::Reread => {
                plan.steps = vec![Step::Reread { dev: d.path.clone() }];
            }
            PartOp::Resize { .. } => {
                return Err("Resizing is not supported yet: IYAKKI has no safe, in-house way to grow or shrink a file system, so it does not pretend. Copy the files off, delete the partition and add a new one.".into());
            }
            PartOp::NewTable(kind) => {
                if *kind == Table::None {
                    return Err("Choose GPT or MBR.".into());
                }
                let mut l = read_layout(self, d).unwrap_or(Layout { table: Table::None, sector: d.sector, total: d.size_bytes / d.sector as u64, disk_guid: [0; 16], mbr_sig: [0; 4], parts: vec![] });
                if l.total < 4096 {
                    return Err("The drive is too small to partition.".into());
                }
                for p in d.parts.iter() {
                    plan.lost.push(format!("{}  {}  {}", p.name, iyakki_storage::human(p.size_bytes), p.title()));
                }
                if plan.lost.is_empty() {
                    plan.lost.push("Nothing: the drive has no partitions.".into());
                }
                plan.lost.push("Every file on every partition becomes unreachable.".into());
                let ss = l.sector as u64;
                let clear = MIB.min(l.total * ss / 4);
                plan.steps.push(Step::Zero { dev: d.path.clone(), offset: 0, len: clear, what: "Erase the start of the drive".into() });
                plan.steps.push(Step::Zero { dev: d.path.clone(), offset: l.total * ss - clear, len: clear, what: "Erase the end of the drive (the old backup table)".into() });
                let sig = random_bytes::<4>();
                l.parts.clear();
                match kind {
                    Table::Gpt => {
                        l.disk_guid = gpt::random_guid();
                        plan.steps.extend(gpt_steps(d, &l, vec![], true, sig)?);
                    }
                    _ => plan.steps.push(Step::Write { dev: d.path.clone(), offset: 0, data: mbr_sector(&l, &[], sig, &[])?, what: "Write an empty MBR partition table".into() }),
                }
                l.table = *kind;
                plan.steps.push(Step::Sync);
                plan.steps.push(Step::Reread { dev: d.path.clone() });
                plan.after = layout_lines(&l);
            }
            PartOp::Add { size_bytes, fs, label } => {
                let mut l = read_layout(self, d)?;
                if l.table == Table::None {
                    return Err("The drive has no partition table yet. Make a new partition table first.".into());
                }
                if let Some(f) = fs.and_then(|f| f.unsupported()) {
                    return Err(f.into());
                }
                let free = l.free();
                let a = l.align();
                let want = size_bytes.map(|b| (b.div_ceil(MIB) * MIB).div_ceil(l.sector as u64));
                let pick = match want {
                    Some(w) => free.iter().copied().filter(|(f, last)| last - f + 1 >= w).min_by_key(|(f, last)| last - f + 1),
                    None => free.iter().copied().max_by_key(|(f, last)| last - f + 1),
                };
                let Some((first, last)) = pick else {
                    return Err(match size_bytes {
                        Some(b) => format!("There is no free space of {} on the drive.", iyakki_storage::human(*b)),
                        None => "There is no free space on the drive.".into(),
                    });
                };
                let last = match want {
                    Some(w) => first + w - 1,
                    None => last,
                };
                let _ = a;
                let max_parts = if l.table == Table::Mbr { 4 } else { gpt::ENTRY_COUNT };
                let number = (1..=max_parts).find(|n| !l.parts.iter().any(|p| p.number == *n)).ok_or("The partition table is full.")?;
                let (mbr_type, type_guid) = match fs {
                    Some(FormatFs::Ext4) => (0x83, gpt::TYPE_LINUX_DATA),
                    Some(_) => (0x0C, type_basic_data()),
                    None => (0x83, gpt::TYPE_LINUX_DATA),
                };
                let new = LPart { number, first, last, type_name: if l.table == Table::Gpt { gpt::type_name(&type_guid) } else { mbr_type_name(mbr_type).into() }, name: label.clone(), mbr_type, type_guid, guid: gpt::random_guid() };
                l.parts.push(new.clone());
                l.parts.sort_by_key(|p| p.number);
                plan.lost.push(format!("Nothing is lost: the new partition uses free space ({}).", iyakki_storage::human(l.bytes(last - first + 1))));
                plan.steps = self.table_steps(d, &l)?;
                plan.steps.push(Step::Sync);
                plan.steps.push(Step::Reread { dev: d.path.clone() });
                if let Some(f) = fs {
                    let pd = format!("/dev/{}", part_dev(number));
                    plan.steps.push(Step::WaitNode { dev: pd.clone() });
                    plan.steps.extend(self.format_steps(&pd, l.bytes(last - first + 1), l.sector, first, *f, label)?);
                    plan.steps.push(Step::Sync);
                }
                plan.after = layout_lines(&l);
                plan.audit = audit(&format!(" part={number} size={} fs={}", l.bytes(last - first + 1), fs.map_or("none", |f| f.name())));
            }
            PartOp::Delete { number } => {
                let mut l = read_layout(self, d)?;
                let Some(i) = l.parts.iter().position(|p| p.number == *number) else { return Err(format!("There is no partition {number} on {}.", d.name)) };
                let gone = l.parts.remove(i);
                let known = d.parts.iter().find(|p| p.number == *number);
                plan.lost.push(format!("Partition {number}  {}  {}", iyakki_storage::human(l.bytes(gone.last - gone.first + 1)), known.map_or(gone.type_name.clone(), |p| p.title())));
                plan.lost.push("Every file on it becomes unreachable.".into());
                plan.steps = self.table_steps(d, &l)?;
                plan.steps.push(Step::Zero { dev: d.path.clone(), offset: gone.first * l.sector as u64, len: (MIB).min(l.bytes(gone.last - gone.first + 1)), what: format!("Erase the start of partition {number} so it is not found again") });
                plan.steps.push(Step::Sync);
                plan.steps.push(Step::Reread { dev: d.path.clone() });
                plan.after = layout_lines(&l);
                plan.audit = audit(&format!(" part={number} size={}", l.bytes(gone.last - gone.first + 1)));
            }
            PartOp::Format { number, fs, label } => {
                if let Some(f) = fs.unsupported() {
                    return Err(f.into());
                }
                let l = read_layout(self, d)?;
                let Some(p) = l.parts.iter().find(|p| p.number == *number) else { return Err(format!("There is no partition {number} on {}.", d.name)) };
                let known = d.parts.iter().find(|q| q.number == *number);
                plan.lost.push(format!("Partition {number}  {}  {}", iyakki_storage::human(l.bytes(p.last - p.first + 1)), known.map_or(p.type_name.clone(), |q| q.title())));
                plan.lost.push("Every file on it becomes unreachable.".into());
                let pd = format!("/dev/{}", part_dev(*number));
                plan.steps = self.format_steps(&pd, l.bytes(p.last - p.first + 1), l.sector, p.first, *fs, label)?;
                plan.steps.push(Step::Sync);
                plan.after = layout_lines(&l);
                plan.audit = audit(&format!(" part={number} size={} fs={}", l.bytes(p.last - p.first + 1), fs.name()));
            }
            PartOp::Rename { number, label } => {
                let l = read_layout(self, d)?;
                if !l.parts.iter().any(|p| p.number == *number) {
                    return Err(format!("There is no partition {number} on {}.", d.name));
                }
                let known = d.parts.iter().find(|q| q.number == *number).ok_or("The partition is not known to the kernel; re-read the table first.")?;
                plan.lost.push(format!("Nothing is lost: only the name of partition {number} changes (now “{}”).", known.label));
                plan.steps = self.rename_steps(&format!("/dev/{}", known.name), known.fs, label)?;
                plan.steps.push(Step::Sync);
                plan.after = layout_lines(&l);
                plan.audit = audit(&format!(" part={number}"));
            }
        }
        Ok(plan)
    }

    /// The writes that put `l` on the drive (the table of its kind).
    fn table_steps(&self, d: &Disk, l: &Layout) -> Result<Vec<Step>, String> {
        match l.table {
            Table::Gpt => {
                let entries: Vec<Entry> = l.parts.iter().map(|p| Entry { index: p.number, type_guid: p.type_guid, guid: p.guid, first_lba: p.first, last_lba: p.last, attrs: 0, name: p.name.clone() }).collect();
                gpt_steps(d, l, entries, false, l.mbr_sig)
            }
            Table::Mbr => {
                let code = self.env.dev_read(&d.path, 0, 440)?;
                Ok(vec![Step::Write { dev: d.path.clone(), offset: 0, data: mbr_sector(l, &l.parts, l.mbr_sig, &code)?, what: "Write the MBR partition table".into() }])
            }
            Table::None => Err("The drive has no partition table.".into()),
        }
    }

    /// Wipe the start of the partition (so no old file system is found) and make the new one.
    fn format_steps(&self, part_dev: &str, size: u64, sector: u32, start_lba: u64, fs: FormatFs, label: &str) -> Result<Vec<Step>, String> {
        if let Some(f) = fs.unsupported() {
            return Err(f.into());
        }
        let mut v = vec![Step::Zero { dev: part_dev.to_string(), offset: 0, len: MIB.min(size), what: "Erase the start of the partition".into() }];
        match fs {
            FormatFs::Ext4 => {
                if label.len() > 16 {
                    return Err("An ext4 label has 16 characters at most.".into());
                }
                v.push(Step::Mke2fs { dev: part_dev.to_string(), label: label.to_string(), uuid: uuid_text(), what: format!("Create an ext4 file system on {part_dev} (mke2fs)") });
            }
            _ => {
                let serial = u32::from_le_bytes(random_bytes::<4>());
                v.extend(fat32_steps(part_dev, size, sector, start_lba, label, serial)?);
            }
        }
        Ok(v)
    }

    /// Change a label in place: ext2/3/4 (superblock, with its checksum) and FAT (boot sectors and the label entry).
    pub(crate) fn rename_steps(&self, part_dev: &str, fs: Fs, label: &str) -> Result<Vec<Step>, String> {
        match fs {
            Fs::Ext2 | Fs::Ext3 | Fs::Ext4 => {
                if label.len() > 16 {
                    return Err("An ext4 label has 16 characters at most.".into());
                }
                let mut sb = self.env.dev_read(part_dev, 1024, 1024)?;
                if sb.len() < 1024 || sb[56..58] != [0x53, 0xEF] {
                    return Err("The superblock could not be read.".into());
                }
                sb[120..136].fill(0);
                sb[120..120 + label.len()].copy_from_slice(label.as_bytes());
                if le32(&sb, 0x64) & 0x400 != 0 {
                    let c = crc32c_raw(0xFFFF_FFFF, &sb[..1020]);
                    put32(&mut sb, 1020, c);
                }
                Ok(vec![Step::Write { dev: part_dev.to_string(), offset: 1024, data: sb, what: format!("Change the {} label to “{label}”", fs.name()) }])
            }
            Fs::Fat12 | Fs::Fat16 | Fs::Fat32 => {
                let lab = fat_label(label)?;
                let mut padded = [b' '; 11];
                if lab.is_empty() {
                    padded.copy_from_slice(b"NO NAME    ");
                } else {
                    padded[..lab.len()].copy_from_slice(lab.as_bytes());
                }
                let bs = self.env.dev_read(part_dev, 0, 512)?;
                if bs.len() < 512 {
                    return Err("The boot sector could not be read.".into());
                }
                let off = if fs == Fs::Fat32 { 71 } else { 43 };
                let mut v = vec![Step::Write { dev: part_dev.to_string(), offset: off, data: padded.to_vec(), what: "Change the label in the boot sector".into() }];
                if fs == Fs::Fat32 {
                    let bps = u16::from_le_bytes([bs[11], bs[12]]) as u64;
                    let backup = u16::from_le_bytes([bs[50], bs[51]]) as u64;
                    if backup != 0 && backup != 0xFFFF {
                        v.push(Step::Write { dev: part_dev.to_string(), offset: backup * bps + off, data: padded.to_vec(), what: "Change the label in the backup boot sector".into() });
                    }
                    let (spc, res, nfats, fatsz, rootc) = (bs[13] as u64, u16::from_le_bytes([bs[14], bs[15]]) as u64, bs[16] as u64, le32(&bs, 36) as u64, le32(&bs, 44) as u64);
                    let root = (res + nfats * fatsz + rootc.saturating_sub(2) * spc) * bps;
                    let dir = self.env.dev_read(part_dev, root, (spc * bps).min(32 * 1024) as usize)?;
                    let slot = dir.chunks(32).position(|e| e.len() == 32 && e[0] != 0 && e[0] != 0xE5 && e[11] == 0x08);
                    let free = dir.chunks(32).position(|e| e.len() == 32 && (e[0] == 0 || e[0] == 0xE5));
                    match (slot, free) {
                        (Some(i), _) if !lab.is_empty() => v.push(Step::Write { dev: part_dev.to_string(), offset: root + 32 * i as u64, data: padded.to_vec(), what: "Change the label entry of the root folder".into() }),
                        (Some(i), _) => v.push(Step::Write { dev: part_dev.to_string(), offset: root + 32 * i as u64, data: vec![0xE5], what: "Remove the label entry of the root folder".into() }),
                        (None, Some(i)) if !lab.is_empty() => {
                            let mut e = padded.to_vec();
                            e.extend_from_slice(&[0x08]);
                            e.extend_from_slice(&[0u8; 20]);
                            v.push(Step::Write { dev: part_dev.to_string(), offset: root + 32 * i as u64, data: e, what: "Add a label entry to the root folder".into() });
                        }
                        _ => {}
                    }
                }
                Ok(v)
            }
            other => Err(format!("Renaming a {} volume is not supported yet.", other.name())),
        }
    }

    /// Carry out a change: all the checks again on a fresh look, the confirmations, the audit, then the writes.
    pub fn apply_partition(&self, who: &Actor, disk_name: &str, op: &PartOp, confirm: &Confirm, audit: AuditFn) -> Result<String, String> {
        let snap = self.scan();
        let Some(d) = snap.disk(disk_name) else { return Err(format!("{disk_name}: there is no such drive.")) };
        let refused = |why: &str| {
            audit("disks.partition.refused", &format!("{} op={} why={why}", d.audit_id(), op.name()));
            why.to_string()
        };
        if let Some(r) = self.refusal(who, d) {
            return Err(refused(&r));
        }
        if op.destructive() {
            check_confirmation(d, confirm).map_err(|e| refused(&e))?;
        }
        let plan = self.plan_in(&snap, who, disk_name, op).map_err(|e| refused(&e))?;
        audit("disks.partition.apply", &plan.audit);
        // the last look before the first write: the same drive, still not the system disk, still nothing mounted
        let again = self.scan();
        match again.disk(disk_name) {
            Some(d2) if d2.serial == d.serial && d2.size_bytes == d.size_bytes && self.refusal(who, d2).is_none() => {}
            _ => return Err(refused("the drive changed while the plan was made; nothing was written")),
        }
        for st in &plan.steps {
            let r = match st {
                Step::Zero { dev, offset, len, .. } => self.env.dev_zero(dev, *offset, *len),
                Step::Write { dev, offset, data, .. } => self.env.dev_write(dev, *offset, data),
                Step::Mke2fs { dev, label, uuid, .. } => self.env.mke2fs(dev, label, uuid),
                Step::Reread { dev } => self.env.reread_table(dev),
                Step::WaitNode { dev } => {
                    if self.env.wait_node(dev) { Ok(()) } else { Err(format!("{dev} did not appear")) }
                }
                Step::Sync => {
                    self.env.sync();
                    Ok(())
                }
            };
            if let Err(e) = r {
                audit("disks.partition.failed", &format!("{} error={e}", plan.audit));
                self.forget();
                return Err(format!("Stopped: {e}. Some steps were already done; look at the drive before using it."));
            }
        }
        self.forget();
        audit("disks.partition.done", &plan.audit);
        Ok(format!("Done: {} on {}.", plan.headline(), d.name))
    }
}
