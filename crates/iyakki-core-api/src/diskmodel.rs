//! The drive model and the drive engine's handle (F-ARCH-05, F-ARCH-07): the plain data of a scan (disks, partitions, file systems), who is
//! asking, what a mount does, and the [`DiskEngine`] trait the UI asks. No IO here: the engine that reads `/sys` and mounts is in
//! `iyakki-core-local`, the canned one in `iyakki-core-fake`. Moved from `iyakki-shared::disks` and `iyakki-core-local::disks`; both re-export.

use iyakki_sec::policy::Role;

/// The flags a mount can carry (the kernel's `MS_*` bits are chosen in `disks_sys.rs`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct MountFlags {
    pub rdonly: bool,
    pub nosuid: bool,
    pub nodev: bool,
    pub noexec: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Fs {
    Ext2,
    Ext3,
    Ext4,
    Fat12,
    Fat16,
    Fat32,
    Exfat,
    Ntfs,
    Btrfs,
    Xfs,
    F2fs,
    Iso9660,
    Swap,
    Luks,
    BitLocker,
    Apfs,
    HfsPlus,
    /// Blank (all zero) as far as we looked.
    Empty,
    Unknown,
}

impl Fs {
    /// The name a person sees.
    pub fn name(self) -> &'static str {
        match self {
            Fs::Ext2 => "ext2",
            Fs::Ext3 => "ext3",
            Fs::Ext4 => "ext4",
            Fs::Fat12 => "FAT12",
            Fs::Fat16 => "FAT16",
            Fs::Fat32 => "FAT32",
            Fs::Exfat => "exFAT",
            Fs::Ntfs => "NTFS (Windows)",
            Fs::Btrfs => "Btrfs",
            Fs::Xfs => "XFS",
            Fs::F2fs => "F2FS",
            Fs::Iso9660 => "ISO 9660 (disc image)",
            Fs::Swap => "swap space",
            Fs::Luks => "LUKS (encrypted)",
            Fs::BitLocker => "BitLocker (encrypted)",
            Fs::Apfs => "APFS (Mac)",
            Fs::HfsPlus => "HFS+ (Mac)",
            Fs::Empty => "empty",
            Fs::Unknown => "unknown",
        }
    }

    /// The kernel's name for it, when a plain mount can open it.
    pub fn driver(self) -> Option<&'static str> {
        match self {
            Fs::Ext2 | Fs::Ext3 | Fs::Ext4 => Some("ext4"),
            Fs::Fat12 | Fs::Fat16 | Fs::Fat32 => Some("vfat"),
            Fs::Exfat => Some("exfat"),
            Fs::Ntfs => Some("ntfs3"),
            Fs::Btrfs => Some("btrfs"),
            Fs::Xfs => Some("xfs"),
            Fs::F2fs => Some("f2fs"),
            Fs::Iso9660 => Some("iso9660"),
            Fs::HfsPlus => Some("hfsplus"),
            _ => None,
        }
    }

    /// Always opened read-only (a disc image, and Mac volumes: Linux does not write them safely).
    pub fn read_only_always(self) -> bool {
        matches!(self, Fs::Iso9660 | Fs::HfsPlus)
    }

    /// FAT, exFAT and NTFS carry no owner: the person who mounts gets them through mount options.
    pub fn owner_options(self) -> bool {
        matches!(self, Fs::Fat12 | Fs::Fat16 | Fs::Fat32 | Fs::Exfat | Fs::Ntfs | Fs::Iso9660)
    }

    /// Why a plain mount is not possible (encrypted, swap, nothing to mount), or `None`.
    pub fn never_mountable(self) -> Option<&'static str> {
        match self {
            Fs::Luks => Some("Encrypted (LUKS): unlock needs the key. Choose Unlock and type the password; it opens read-only first."),
            Fs::BitLocker => Some("Encrypted (BitLocker): unlock needs the key. Choose Unlock and type the password or the 48-digit recovery key; it opens read-only first."),
            Fs::Apfs => Some("Mac APFS: shown, but Linux cannot read it safely. Use a Mac, or format it as exFAT there."),
            Fs::Swap => Some("Swap space: nothing to open."),
            Fs::Empty => Some("Empty: there is no file system on it."),
            Fs::Unknown => Some("Unknown format: nothing IYAKKI knows how to open."),
            _ => None,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Transport {
    Usb,
    Nvme,
    Sata,
    Mmc,
    Virtual,
    Other,
}

impl Transport {
    pub fn name(self) -> &'static str {
        match self {
            Transport::Usb => "USB",
            Transport::Nvme => "NVMe",
            Transport::Sata => "SATA",
            Transport::Mmc => "SD / eMMC",
            Transport::Virtual => "virtual",
            Transport::Other => "disk",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Table {
    None,
    Mbr,
    Gpt,
}

impl Table {
    pub fn name(self) -> &'static str {
        match self {
            Table::None => "no partition table",
            Table::Mbr => "MBR",
            Table::Gpt => "GPT",
        }
    }
}

#[derive(Clone, Debug)]
pub struct Actor {
    pub user: String,
    pub role: Role,
    /// The owner the person's files get on FAT, exFAT and NTFS.
    pub uid: u32,
    pub gid: u32,
    /// The person's home folder: their drives are opened in `<home>/media`.
    pub home: String,
}

impl Actor {
    pub fn new(user: &str, role: Role) -> Actor {
        Actor { user: user.into(), role, uid: 0, gid: 0, home: format!("/home/{user}") }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct MountOpts {
    pub read_only: bool,
    /// "Allow running programs" (administrators only).
    pub allow_exec: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MountPlan {
    pub dev: String,
    pub target: String,
    pub fstype: &'static str,
    pub flags: MountFlags,
    pub data: String,
    /// Opened read-only although it was not asked for, and why.
    pub forced_ro: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Part {
    /// `sdb1`, `nvme0n1p2`; the disk's own name for a file system straight on the disk.
    pub name: String,
    pub path: String,
    /// 1-based; 0 for a file system straight on the disk.
    pub number: u32,
    pub start_bytes: u64,
    pub size_bytes: u64,
    pub fs: Fs,
    pub label: String,
    pub uuid: String,
    pub mountpoints: Vec<String>,
    /// Space of the first mount: (free, total).
    pub space: Option<(u64, u64)>,
    /// The mount is read-only.
    pub mounted_ro: bool,
    pub swap_in_use: bool,
    /// Holds the running system or its boot or data volume.
    pub system: bool,
    /// What the partition table calls it ("Microsoft basic data", "EFI System").
    pub type_name: String,
    /// The header of an encrypted container (LUKS, BitLocker), when that is what this is.
    pub crypt: Option<iyakki_diskcrypt::CryptInfo>,
    /// An encrypted container that is unlocked right now: its opened device `/dev/mapper/iyakki-<name>` exists.
    pub unlocked: bool,
}

impl Part {
    pub fn mounted(&self) -> bool {
        !self.mountpoints.is_empty()
    }

    /// "SANDISK", "Backup (ext4)" ...
    pub fn title(&self) -> String {
        if self.label.is_empty() { format!("{} ({})", self.name, self.fs.name()) } else { format!("{} ({})", self.label, self.fs.name()) }
    }

    pub fn used_bytes(&self) -> Option<u64> {
        self.space.map(|(free, total)| total.saturating_sub(free))
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Disk {
    /// `sdb`, `nvme0n1`
    pub name: String,
    pub path: String,
    pub vendor: String,
    pub model: String,
    pub serial: String,
    pub size_bytes: u64,
    pub sector: u32,
    pub removable: bool,
    pub transport: Transport,
    pub read_only: bool,
    pub table: Table,
    pub table_notes: Vec<String>,
    pub parts: Vec<Part>,
    /// Holds the running system: never offered for anything destructive.
    pub system: bool,
    pub system_why: String,
    /// Another driver (device-mapper, RAID, LVM) uses it.
    pub holders: Vec<String>,
}

impl Disk {
    /// What a person calls it: "SanDisk Ultra" or the device name.
    pub fn title(&self) -> String {
        let t = [self.vendor.as_str(), self.model.as_str()].iter().filter(|s| !s.is_empty()).copied().collect::<Vec<_>>().join(" ");
        if t.is_empty() { self.name.clone() } else { t }
    }

    pub fn mounted(&self) -> bool {
        self.parts.iter().any(Part::mounted)
    }

    /// A USB stick, an SD card or another drive meant to be taken out.
    pub fn removable_media(&self) -> bool {
        self.removable || self.transport == Transport::Usb
    }

    /// The last four characters of the serial number (what a person may type to confirm), or empty without a serial.
    pub fn serial_tail(&self) -> String {
        let s = self.serial.trim();
        let n = s.chars().count();
        if n < 4 { String::new() } else { s.chars().skip(n - 4).collect() }
    }

    pub fn part(&self, name: &str) -> Option<&Part> {
        self.parts.iter().find(|p| p.name == name)
    }

    /// The one line that says exactly which drive this is: name, size, serial.
    pub fn identity(&self) -> String {
        format!("{} ({}, {}, serial {})", self.name, self.title(), iyakki_storage::human(self.size_bytes), if self.serial.is_empty() { "none".to_string() } else { self.serial.clone() })
    }

    /// The audit detail for this drive: name, size, serial. Never contents.
    pub fn audit_id(&self) -> String {
        format!("drive={} size={} serial={}", self.name, self.size_bytes, if self.serial.is_empty() { "-" } else { &self.serial })
    }
}

/// A file system the kernel may or may not have.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FsSupport {
    pub fs: Fs,
    pub kernel: bool,
    pub driver: &'static str,
}

#[derive(Clone, Debug, Default)]
pub struct Snapshot {
    pub disks: Vec<Disk>,
    pub support: Vec<FsSupport>,
}

impl Snapshot {
    pub fn disk(&self, name: &str) -> Option<&Disk> {
        self.disks.iter().find(|d| d.name == name)
    }

    /// The disk and the partition called `name`.
    pub fn part(&self, name: &str) -> Option<(&Disk, &Part)> {
        self.disks.iter().find_map(|d| d.part(name).map(|p| (d, p)))
    }

    pub fn kernel_has(&self, driver: &str) -> bool {
        self.support.iter().any(|s| s.driver == driver && s.kernel)
    }
}

/// May this person mount, unmount or eject `d`? The same answer the tile uses to show or grey the buttons.
pub fn may_use(who: &Actor, d: &Disk) -> Result<(), String> {
    match who.role {
        Role::Guest => Err("Guests cannot use drives. Ask an administrator.".into()),
        Role::User if !d.removable_media() => Err("People with the User role can use removable drives (USB sticks, SD cards) only. Ask an administrator for this one.".into()),
        _ => Ok(()),
    }
}



/// Why this drive may not be changed (partitioned, formatted), or `None`. The engine's own hard rules: administrators only, never the system
/// disk, nothing mounted, no swap, nobody holding it, not write-protected. Pure: the tile greys its buttons with the same answer the engine
/// refuses with.
pub fn partition_refusal(who: &Actor, d: &Disk) -> Option<String> {
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

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Mounted {
    pub target: String,
    pub read_only: bool,
    /// What the person should know ("opened read-only because ...").
    pub notes: Vec<String>,
}


// ---------------------------------------------------------------------------------------------------------------------
// The partition manager's plain data (F-ARCH-06): what the person asks (PartOp), what the engine plans (Plan, Step), what they confirm
// (Confirm). Moved from `iyakki-core-local::disks_part` and `disks_fmt`; both re-export. No IO here.
// ---------------------------------------------------------------------------------------------------------------------

/// What the person did to confirm: typed the drive's name or the last four characters of its serial, and pressed the second button.
#[derive(Clone, Debug, Default)]
pub struct Confirm {
    pub typed: String,
    pub second: bool,
}

/// The programs of this feature.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tool {
    MkfsExfat,
    Mkntfs,
    E2fsck,
    Resize2fs,
    Ntfsresize,
}

impl Tool {
    /// The program's file name.
    pub fn prog(self) -> &'static str {
        match self {
            Tool::MkfsExfat => "mkfs.exfat",
            Tool::Mkntfs => "mkntfs",
            Tool::E2fsck => "e2fsck",
            Tool::Resize2fs => "resize2fs",
            Tool::Ntfsresize => "ntfsresize",
        }
    }

    /// The CLI tools entry that provides it, and its licence.
    pub fn package(self) -> (&'static str, &'static str) {
        match self {
            Tool::MkfsExfat => ("exfatprogs", "GPL-2.0"),
            Tool::Mkntfs | Tool::Ntfsresize => ("ntfs-3g", "GPL-2.0"),
            Tool::E2fsck | Tool::Resize2fs => ("e2fsprogs", "GPL-2.0"),
        }
    }

    /// The words when the program is not there. Says plainly that IYAKKI does not ship it and that nothing was changed.
    pub fn missing_words(self, doing: &str) -> String {
        let (pkg, lic) = self.package();
        if pkg == "e2fsprogs" {
            return format!("{doing} needs the program {} (e2fsprogs), which should be part of the system image but was not found. Nothing was changed on the drive.", self.prog());
        }
        format!("{doing} needs the program {} ({pkg}, {lic} licence), which is not installed. IYAKKI does not ship it: install it from CLI tools (you give the download address and its SHA-256; nothing is fetched until you do) or with this system's package manager, then try again. Nothing was changed on the drive.", self.prog())
    }
}

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

    /// Why this cannot be done in principle, or `None`. exFAT and NTFS need a person-installed program, which the planner checks.
    pub fn unsupported(self) -> Option<&'static str> {
        None
    }

    /// The program that makes this file system when it is not made in-house.
    pub fn tool(self) -> Option<Tool> {
        match self {
            FormatFs::Exfat => Some(Tool::MkfsExfat),
            FormatFs::Ntfs => Some(Tool::Mkntfs),
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


/// The partition a change is about, if it is about one.
pub fn op_partition(op: &PartOp) -> Option<u32> {
    match op {
        PartOp::Delete { number } | PartOp::Format { number, .. } | PartOp::Rename { number, .. } | PartOp::Resize { number, .. } => Some(*number),
        _ => None,
    }
}

/// What the person may type to confirm `op` on `d`: the drive's name, the last four of its serial, and for a change to one partition
/// also that partition's device name and its label (an exact match for the label).
pub fn accepted_names(d: &Disk, op: &PartOp) -> Vec<String> {
    let mut v = vec![d.name.clone()];
    let tail = d.serial_tail();
    if !tail.is_empty() {
        v.push(tail);
    }
    if let Some(p) = op_partition(op).and_then(|n| d.parts.iter().find(|p| p.number == n)) {
        v.push(p.name.clone());
        if !p.label.trim().is_empty() {
            v.push(p.label.trim().to_string());
        }
    }
    v
}

/// Like [`check_confirmation`], but a change to one partition may also be confirmed with the partition's device name (`sdb2`) or its
/// label, typed exactly. Never a plain Yes.
pub fn check_confirmation_for(d: &Disk, op: &PartOp, c: &Confirm) -> Result<(), String> {
    let part = op_partition(op).and_then(|n| d.parts.iter().find(|p| p.number == n));
    let t = c.typed.trim();
    if let Some(p) = part {
        let hit = t.eq_ignore_ascii_case(&p.name) || t.eq_ignore_ascii_case(&p.path) || (!p.label.trim().is_empty() && t == p.label.trim());
        if hit {
            return if c.second { Ok(()) } else { Err("Press the second confirmation button to go on. Nothing was changed.".into()) };
        }
        let others = accepted_names(d, op).join(", ");
        if t.is_empty() {
            return Err(format!("Type one of these exactly to confirm: {others}."));
        }
        if !(t.eq_ignore_ascii_case(&d.name) || t.eq_ignore_ascii_case(&d.path) || (!d.serial_tail().is_empty() && t.eq_ignore_ascii_case(&d.serial_tail()))) {
            return Err(format!("\"{t}\" is not one of: {others}. Nothing was changed."));
        }
    }
    check_confirmation(d, c)
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Step {
    Zero { dev: String, offset: u64, len: u64, what: String },
    Write { dev: String, offset: u64, data: Vec<u8>, what: String },
    Mke2fs { dev: String, label: String, uuid: String, what: String },
    /// A person-installed program (exFAT, NTFS, resize) with a fixed argument list.
    Tool { tool: Tool, args: Vec<String>, what: String },
    /// Ask a program how small the file system can get and stop unless `need` bytes is enough (reads only). `block` is the ext block size.
    MinSize { tool: Tool, args: Vec<String>, block: u64, need: u64, what: String },
    Reread { dev: String },
    WaitNode { dev: String },
    Sync,
}

impl Step {
    pub fn text(&self) -> String {
        match self {
            Step::Zero { offset, len, what, .. } => format!("{what} (clear bytes {offset} to {})", offset + len),
            Step::Write { offset, data, what, .. } => format!("{what} (write {} bytes at byte {offset})", data.len()),
            Step::Mke2fs { what, .. } | Step::Tool { what, .. } | Step::MinSize { what, .. } => what.clone(),
            Step::Reread { dev } => format!("Ask the kernel to read the partition table of {dev} again"),
            Step::WaitNode { dev } => format!("Wait for {dev} to appear"),
            Step::Sync => "Flush everything to the drive".into(),
        }
    }
}

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
    /// What must be true on the drive afterwards, checked by looking again.
    pub expect: Vec<Expect>,
    pub audit: String,
}

/// One thing the executor checks after the last step.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Expect {
    /// The partition shows this file system.
    Fs { part: String, fs: Fs },
    /// The partition is this many bytes.
    Size { part: String, bytes: u64 },
    /// The partition is gone.
    Gone { part: String },
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
        self.steps.iter().any(|s| matches!(s, Step::Zero { .. } | Step::Write { .. } | Step::Mke2fs { .. } | Step::Tool { .. }))
    }
}

// ---------------------------------------------------------------------------------------------------------------------
// The handle the UI asks
// ---------------------------------------------------------------------------------------------------------------------

/// Where an audit line goes: `(what, detail)`. The caller names the person; the engine never reads their contents.
pub type AuditFn<'a> = &'a dyn Fn(&str, &str);

/// The drive engine as the core serves it (F-ARCH-05, F-ARCH-07), next to [`crate::devices::Disks`] (block devices, mounts, free space): what
/// is on each drive (a scan, with file system, label, mount points, encryption header), and the plan and apply side of mounting, unmounting and
/// ejecting. Every answer is plain data. Every change names the person ([`Actor`]) and the engine refuses it by role ([`may_use`]: guests none,
/// users removable drives only), never forces a busy drive, and leaves an audit line with the drive's name, size and serial, never contents.
/// A machine that does not say otherwise ([`NoDiskEngine`]) sees no drive and changes none. Partition changes, formatting and unlocking
/// move through the same handle (KI-729 step 1): [`DiskEngine::plan_partition`] and [`DiskEngine::apply_partition`] (administrators only,
/// typed confirmation checked again), [`DiskEngine::plan_unlock`], [`DiskEngine::unlock`], [`DiskEngine::allow_writing`] and [`DiskEngine::lock`]
/// (the key goes to the helper only, never into an audit line). [`DiskEngine::use_programs`] tells the engine where the person-installed
/// programs (exFAT, NTFS, resize, the unlock helpers) are.
pub trait DiskEngine: Send + Sync {
    // ---- read side (`system.read`)
    /// Every drive of this machine with its partitions and the file systems the kernel supports.
    fn scan(&self) -> Snapshot {
        Snapshot::default()
    }
    /// A cheap number that changes when a drive or partition appears, goes, or is mounted or unmounted (two small reads, no device opened).
    fn fingerprint(&self) -> u64 {
        0
    }
    /// Forget what was read from the partitions (after a change).
    fn forget(&self) {}
    /// The people and programs using a mounted folder: `(process id, name)`.
    fn users_of(&self, mountpoint: &str) -> Vec<(u32, String)> {
        let _ = mountpoint;
        Vec::new()
    }
    /// The partitions open under `home`: `(partition name, folder)`.
    fn mounted_in(&self, home: &str) -> Vec<(String, String)> {
        let _ = home;
        Vec::new()
    }

    // ---- plan and apply side (every use is checked against the role and audited)
    /// What a mount of `part` would do (device, folder, options), without doing it.
    fn plan_mount(&self, snap: &Snapshot, who: &Actor, part: &str, opts: &MountOpts) -> Result<MountPlan, String> {
        let _ = (snap, who, part, opts);
        Err("this machine has no drives to open".into())
    }
    /// Open a partition in the person's home folder.
    fn mount(&self, who: &Actor, part: &str, opts: &MountOpts, audit: AuditFn) -> Result<Mounted, String> {
        let _ = (who, part, opts, audit);
        Err("this machine has no drives to open".into())
    }
    /// Close a partition (plain unmount, never forced).
    fn unmount(&self, who: &Actor, part: &str, audit: AuditFn) -> Result<String, String> {
        let _ = (who, part, audit);
        Err("this machine has no drives to close".into())
    }
    /// Close everything of the drive and switch it off so it can be taken out.
    fn eject(&self, who: &Actor, disk: &str, audit: AuditFn) -> Result<String, String> {
        let _ = (who, disk, audit);
        Err("this machine has no drives to eject".into())
    }
    /// At sign-out: close the person's drives when their switch is on, otherwise say which stay open. The sentence for the person, or `None`.
    fn at_sign_out(&self, who: &Actor, audit: AuditFn) -> Option<String> {
        let _ = (who, audit);
        None
    }

    // ---- partitions and formatting (F-ARCH-06; `system.disk.partition`, every use asks)
    /// Why this drive may not be changed, or `None` ([`partition_refusal`]).
    fn refusal(&self, who: &Actor, d: &Disk) -> Option<String> {
        partition_refusal(who, d)
    }
    /// The dry run: what would be done to `disk` for `op`, step by step. Nothing is written.
    fn plan_partition(&self, who: &Actor, disk: &str, op: &PartOp) -> Result<Plan, String> {
        let _ = (who, disk, op);
        Err("this machine has no drives to change".into())
    }
    /// Carry out a change: every check again on a fresh look, the typed drive name and the second confirmation, the audit, then the writes.
    fn apply_partition(&self, who: &Actor, disk: &str, op: &PartOp, confirm: &Confirm, audit: AuditFn) -> Result<String, String> {
        let _ = (who, disk, op, confirm, audit);
        Err("this machine has no drives to change".into())
    }

    // ---- encrypted volumes (F-ARCH-06; `system.disk.unlock`)
    /// Whether `part` can be unlocked now, and as what. Nothing is changed.
    fn plan_unlock(&self, snap: &Snapshot, who: &Actor, part: &str) -> Result<iyakki_diskcrypt::CryptKind, String> {
        let _ = (snap, who, part);
        Err("this machine has no drives to unlock".into())
    }
    /// Unlock with the typed key and open it read-only. The key is only handed to the helper; a wrong key costs a growing wait.
    fn unlock(&self, who: &Actor, part: &str, secret: &iyakki_diskcrypt::Secret, audit: AuditFn) -> Result<Mounted, String> {
        let _ = (who, part, secret, audit);
        Err("this machine has no drives to unlock".into())
    }
    /// Reopen an unlocked LUKS volume for writing (a BitLocker volume always stays read-only).
    fn allow_writing(&self, who: &Actor, part: &str, audit: AuditFn) -> Result<Mounted, String> {
        let _ = (who, part, audit);
        Err("this machine has no drives to unlock".into())
    }
    /// Unmount an unlocked volume and close it again.
    fn lock(&self, who: &Actor, part: &str, audit: AuditFn) -> Result<String, String> {
        let _ = (who, part, audit);
        Err("this machine has no drives to lock".into())
    }

    // ---- the tools seam
    /// Where the person-installed programs are (the CLI tools folder): exFAT and NTFS formatters, the resizers, the unlock helpers. Called once
    /// when the Disks tile opens; an engine that starts no programs ignores it.
    fn use_programs(&self, bin_dir: &str) {
        let _ = bin_dir;
    }
    /// What the assistant asked the tile to format or resize for `user`, taken once (the request waits in the data folder `dir`; anything that
    /// does not parse is dropped). The tile shows the review for it: the typed name and the second button still come first.
    fn take_format_request(&self, dir: &std::path::Path, user: &str) -> Option<FormatRequest> {
        let _ = (dir, user);
        None
    }
}

/// What the assistant asks the Disks tile to format or resize (data from a model: checked before it is used).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FormatRequest {
    Format { part: String, fs: FormatFs, label: String },
    Resize { part: String, bytes: u64 },
}

impl FormatRequest {
    pub fn part(&self) -> &str {
        match self {
            FormatRequest::Format { part, .. } | FormatRequest::Resize { part, .. } => part,
        }
    }
}

/// Turn a request into the engine's change: the drive and partition number the name belongs to.
pub fn request_op(snap: &Snapshot, req: &FormatRequest) -> Result<(String, PartOp), String> {
    let (disk, p) = snap.part(req.part()).map(|(d, p)| (d.name.clone(), p.number)).ok_or_else(|| format!("{}: there is no such partition (the drive may have been unplugged).", req.part()))?;
    if p == 0 {
        return Err(format!("{} is a file system straight on the drive without a partition table; make a partition table in the DISKS tile first.", req.part()));
    }
    Ok((disk, match req {
        FormatRequest::Format { fs, label, .. } => PartOp::Format { number: p, fs: *fs, label: label.clone() },
        FormatRequest::Resize { bytes, .. } => PartOp::Resize { number: p, size_bytes: *bytes },
    }))
}

/// The drive engine of a machine that does not say otherwise: no drives, no changes.
pub struct NoDiskEngine;

impl DiskEngine for NoDiskEngine {}
