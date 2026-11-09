//! DISKS engine: see every drive of the machine, mount and unmount partitions, eject safely (docs/DISKS.md).
//!
//! Everything that touches the machine goes through the [`DiskEnv`] seam (reading `/sys` and `/proc`, reading and writing a
//! device, `mount`, `umount`, `mke2fs`, switching a USB drive off), so the tests run on a made-up machine and never see a real
//! disk. The real seam is `disks_sys.rs`. The partition manager (`disks_part.rs`) is built on the same listing and the same
//! safety rules.
//!
//! Rules that hold everywhere:
//! * the drive that holds the running system (`/`, `/boot`, the `IYAKKI-BOOT` and `IYAKKI-DATA` volumes, swap in use) is never
//!   unmounted, ejected or changed;
//! * nothing is ever forced: a busy drive is not unmounted, and the people or programs using it are named;
//! * who may do what follows the role (administrators and operators: any drive; users: removable drives only; guests: none);
//! * every mount, unmount, eject and partition change leaves an audit entry with the drive's name, size and serial, never contents.

use std::collections::HashMap;
use std::io::{Read, Seek, SeekFrom};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use iyakki_sec::policy::Role;
use iyakki_storage::gpt;

/// Where a person's drives are opened: `<home>/media/<label>`. Folders there are created on mount and removed (when empty) after
/// unmount by IYAKKI only. Every drive goes into the home of the signed-in account, so every role needs only its own home.
pub fn media_dir(home: &str) -> String {
    format!("{}/media", home.trim_end_matches('/'))
}

/// Is `mp` a folder under some person's `media` folder (where IYAKKI mounts)?
pub fn is_media_mount(mp: &str) -> bool {
    mp.contains("/media/") && !mp.contains("/../")
}

/// `<home>/.config/iyakki/disks-unmount-on-signout`: its presence is the Settings switch "Unmount my drives when I sign out" (default off).
fn signout_marker(home: &str) -> std::path::PathBuf {
    std::path::Path::new(home).join(".config/iyakki/disks-unmount-on-signout")
}

pub fn signout_pref(home: &str) -> bool {
    signout_marker(home).exists()
}

pub fn set_signout_pref(home: &str, on: bool) -> Result<(), String> {
    let m = signout_marker(home);
    if on {
        if let Some(d) = m.parent() {
            std::fs::create_dir_all(d).map_err(|e| e.to_string())?;
        }
        std::fs::write(&m, b"1\n").map_err(|e| e.to_string())
    } else {
        match std::fs::remove_file(&m) {
            Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(e.to_string()),
            _ => Ok(()),
        }
    }
}

/// The owner of an account from `/etc/passwd` (user id, group id), when it is not root's.
pub fn account_owner(user: &str, passwd: &str) -> Option<(u32, u32)> {
    std::fs::read_to_string(passwd).ok().and_then(|t| {
        t.lines().filter(|l| !l.starts_with('#')).find_map(|l| {
            let f: Vec<&str> = l.split(':').collect();
            (f.len() >= 4 && f[0] == user).then(|| (f[2].parse::<u32>().ok(), f[3].parse::<u32>().ok())).and_then(|(u, g)| Some((u.filter(|u| *u != 0)?, g?)))
        })
    })
}

/// What the audit log gets: (action, detail).
pub type AuditFn<'a> = &'a dyn Fn(&str, &str);

// ---------------------------------------------------------------------------------------------------------------------
// The seam
// ---------------------------------------------------------------------------------------------------------------------

/// The flags a mount can carry (the kernel's `MS_*` bits are chosen in `disks_sys.rs`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct MountFlags {
    pub rdonly: bool,
    pub nosuid: bool,
    pub nodev: bool,
    pub noexec: bool,
}

pub trait DiskEnv: Send + Sync {
    /// A text file (`/sys/...`, `/proc/...`); `None` when absent.
    fn read_text(&self, path: &str) -> Option<String>;
    /// The names inside a folder, sorted.
    fn list(&self, path: &str) -> Vec<String>;
    /// Where a symbolic link points.
    fn read_link(&self, path: &str) -> Option<String>;
    /// The real path of a `/sys` entry (links resolved).
    fn canonical(&self, path: &str) -> Option<String>;
    /// Bytes of a device (shorter at its end).
    fn dev_read(&self, dev: &str, offset: u64, len: usize) -> Result<Vec<u8>, String>;
    /// Only ever called by the partition manager, after every safety check.
    fn dev_write(&self, dev: &str, offset: u64, data: &[u8]) -> Result<(), String>;
    fn dev_zero(&self, dev: &str, offset: u64, len: u64) -> Result<(), String>;
    fn dev_len(&self, dev: &str) -> u64;
    /// (free, total) bytes of a mounted file system.
    fn space(&self, mountpoint: &str) -> Option<(u64, u64)>;
    fn mount(&self, dev: &str, target: &str, fstype: &str, flags: MountFlags, data: &str) -> Result<(), String>;
    /// A plain unmount: never forced, never lazy. A busy folder answers with "busy" in the text.
    fn umount(&self, target: &str) -> Result<(), String>;
    fn make_dir(&self, path: &str) -> Result<(), String>;
    /// Give a folder we made to the person (their user and group id).
    fn chown(&self, path: &str, uid: u32, gid: u32) -> Result<(), String>;
    /// Removes an empty folder only.
    fn remove_dir(&self, path: &str) -> Result<(), String>;
    fn sync(&self);
    /// Write a short text to a `/sys` knob.
    fn write_sys(&self, path: &str, text: &str) -> Result<(), String>;
    /// Ask the kernel to read the partition table again (BLKRRPART).
    fn reread_table(&self, disk_dev: &str) -> Result<(), String>;
    /// `mke2fs -t ext4` with fixed arguments (no shell).
    fn mke2fs(&self, dev: &str, label: &str, uuid: &str) -> Result<(), String>;
    /// Wait a little for a device node to appear (after the table was re-read).
    fn wait_node(&self, dev: &str) -> bool;
}

// ---------------------------------------------------------------------------------------------------------------------
// What is on a partition: superblock magic, read in-house
// ---------------------------------------------------------------------------------------------------------------------

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

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Probe {
    pub fs: Fs,
    pub label: String,
    pub uuid: String,
    /// What the header of an encrypted container says (LUKS, BitLocker); `None` for everything else.
    pub crypt: Option<crate::disks_crypt::CryptInfo>,
}

/// How many bytes from the start of a partition [`detect`] wants (Btrfs keeps its magic and label 64 KiB in).
pub const PROBE_BYTES: usize = 0x10230;

fn text(b: &[u8], off: usize, len: usize) -> String {
    let s = b.get(off..off + len).unwrap_or(&[]);
    let s: Vec<u8> = s.iter().copied().take_while(|c| *c != 0).collect();
    String::from_utf8_lossy(&s).trim_end_matches([' ', '\0']).trim().to_string()
}

fn hex_uuid(b: &[u8]) -> String {
    if b.len() != 16 || b.iter().all(|x| *x == 0) {
        return String::new();
    }
    let h: Vec<String> = b.iter().map(|x| format!("{x:02x}")).collect();
    format!("{}-{}-{}-{}-{}", h[0..4].concat(), h[4..6].concat(), h[6..8].concat(), h[8..10].concat(), h[10..16].concat())
}

fn le32(b: &[u8], off: usize) -> u32 {
    b.get(off..off + 4).map_or(0, |s| u32::from_le_bytes([s[0], s[1], s[2], s[3]]))
}

/// Read the file system of a partition from its first bytes (at least 4 KiB; more finds Btrfs). Pure.
pub fn detect(b: &[u8]) -> Probe {
    let at = |off: usize, len: usize| b.get(off..off + len);
    let p = |fs: Fs, label: String, uuid: String| Probe { fs, label, uuid, crypt: None };
    // encrypted containers first: their header is not a file system
    if let Some(c) = crate::disks_crypt::parse(b) {
        let fs = if c.kind == crate::disks_crypt::CryptKind::BitLocker { Fs::BitLocker } else { Fs::Luks };
        let (label, uuid) = (c.label.clone(), c.uuid.clone());
        return Probe { fs, label, uuid, crypt: Some(c) };
    }
    if at(32, 4) == Some(b"NXSB") {
        return p(Fs::Apfs, String::new(), hex_uuid(at(72, 16).unwrap_or(&[])));
    }
    if at(3, 8) == Some(b"NTFS    ") {
        let serial = at(0x48, 8).map(|s| s.iter().rev().map(|x| format!("{x:02X}")).collect::<String>()).unwrap_or_default();
        return p(Fs::Ntfs, String::new(), serial);
    }
    if at(3, 8) == Some(b"EXFAT   ") {
        return p(Fs::Exfat, String::new(), format!("{:08X}", le32(b, 100)));
    }
    if at(1024 + 56, 2) == Some(&[0x53, 0xEF]) {
        let compat = le32(b, 1024 + 0x5C);
        let incompat = le32(b, 1024 + 0x60);
        let ro = le32(b, 1024 + 0x64);
        let fs = if incompat & (0x40 | 0x80 | 0x200) != 0 || ro & (0x8 | 0x20 | 0x40) != 0 {
            Fs::Ext4
        } else if compat & 0x4 != 0 {
            Fs::Ext3
        } else {
            Fs::Ext2
        };
        return p(fs, text(b, 1024 + 120, 16), hex_uuid(at(1024 + 104, 16).unwrap_or(&[])));
    }
    if at(0, 4) == Some(b"XFSB") {
        return p(Fs::Xfs, text(b, 108, 12), hex_uuid(at(32, 16).unwrap_or(&[])));
    }
    if at(0x10040, 8) == Some(b"_BHRfS_M") {
        return p(Fs::Btrfs, text(b, 0x1012B, 256), hex_uuid(at(0x10020, 16).unwrap_or(&[])));
    }
    if at(1024, 4) == Some(&[0x10, 0x20, 0xF5, 0xF2]) {
        // the label is UTF-16 at 1024 + 0x6C
        let lab: Vec<u16> = (0..16).map_while(|i| at(1024 + 0x6C + 2 * i, 2).map(|s| u16::from_le_bytes([s[0], s[1]]))).take_while(|c| *c != 0).collect();
        return p(Fs::F2fs, String::from_utf16_lossy(&lab), hex_uuid(at(1024 + 0x6C - 16, 16).unwrap_or(&[])));
    }
    if at(0x8001, 5) == Some(b"CD001") {
        return p(Fs::Iso9660, text(b, 0x8028, 32), String::new());
    }
    if at(4086, 10) == Some(b"SWAPSPACE2") {
        return p(Fs::Swap, text(b, 0x41C, 16), hex_uuid(at(0x40C, 16).unwrap_or(&[])));
    }
    if at(1024, 2) == Some(b"H+") || at(1024, 2) == Some(b"HX") {
        return p(Fs::HfsPlus, String::new(), String::new());
    }
    // FAT: the boot sector ends with 0x55AA and names its type
    if at(510, 2) == Some(&[0x55, 0xAA]) {
        if at(82, 5) == Some(b"FAT32") {
            return p(Fs::Fat32, text(b, 71, 11), format!("{:04X}-{:04X}", le32(b, 67) >> 16, le32(b, 67) & 0xFFFF));
        }
        if at(54, 3) == Some(b"FAT") {
            let fs = match at(54, 5) {
                Some(b"FAT12") => Fs::Fat12,
                _ => Fs::Fat16,
            };
            return p(fs, text(b, 43, 11), format!("{:04X}-{:04X}", le32(b, 39) >> 16, le32(b, 39) & 0xFFFF));
        }
    }
    if b.len() >= 4096 && b.iter().all(|x| *x == 0) {
        return p(Fs::Empty, String::new(), String::new());
    }
    p(Fs::Unknown, String::new(), String::new())
}

// ---------------------------------------------------------------------------------------------------------------------
// The picture of the machine
// ---------------------------------------------------------------------------------------------------------------------

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
    pub crypt: Option<crate::disks_crypt::CryptInfo>,
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

// ---------------------------------------------------------------------------------------------------------------------
// Who is asking
// ---------------------------------------------------------------------------------------------------------------------

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

/// May this person mount, unmount or eject `d`? The same answer the tile uses to show or grey the buttons.
pub fn may_use(who: &Actor, d: &Disk) -> Result<(), String> {
    match who.role {
        Role::Guest => Err("Guests cannot use drives. Ask an administrator.".into()),
        Role::User if !d.removable_media() => Err("People with the User role can use removable drives (USB sticks, SD cards) only. Ask an administrator for this one.".into()),
        _ => Ok(()),
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
pub struct Mounted {
    pub target: String,
    pub read_only: bool,
    /// What the person should know ("opened read-only because ...").
    pub notes: Vec<String>,
}

// ---------------------------------------------------------------------------------------------------------------------
// The engine
// ---------------------------------------------------------------------------------------------------------------------

pub struct Disks {
    pub env: Arc<dyn DiskEnv>,
    cache: Mutex<HashMap<String, (Instant, Probe)>>,
    /// Wrong keys for encrypted volumes (`disks_unlock.rs`).
    pub(crate) limiter: Mutex<crate::disks_crypt::Limiter>,
    /// Tests: the clock of the wrong-key wait stands still at this many seconds (`u64::MAX` = the real clock).
    pub(crate) held_clock: std::sync::atomic::AtomicU64,
    /// The person-installed file system programs (exFAT, NTFS, resizing); none until [`Disks::set_tools`].
    tools: Mutex<Arc<dyn crate::disks_fmt::FsTools>>,
}

const SKIP: [&str; 8] = ["loop", "ram", "zram", "dm-", "md", "fd", "nbd", "sr"];
const PROBE_TTL: Duration = Duration::from_secs(30);

#[derive(Clone, Debug)]
struct MountEnt {
    majmin: String,
    mp: String,
    opts: String,
    source: String,
}

fn unescape(s: &str) -> String {
    s.replace("\\040", " ").replace("\\011", "\t").replace("\\012", "\n").replace("\\134", "\\")
}

fn parse_mountinfo(text: &str) -> Vec<MountEnt> {
    text.lines()
        .filter_map(|l| {
            let (left, right) = l.split_once(" - ")?;
            let f: Vec<&str> = left.split_whitespace().collect();
            let r: Vec<&str> = right.split_whitespace().collect();
            if f.len() < 6 || r.len() < 2 {
                return None;
            }
            Some(MountEnt { majmin: f[2].to_string(), mp: unescape(f[4]), opts: f[5].to_string(), source: unescape(r[1]) })
        })
        .collect()
}

fn base(p: &str) -> &str {
    p.rsplit('/').next().unwrap_or(p)
}

fn is_system_label(l: &str) -> bool {
    l == "IYAKKI-BOOT" || l == "IYAKKI-DATA"
}

fn is_system_mount(mp: &str) -> bool {
    iyakki_usb::drives::critical_mount(mp) || mp.starts_with("/var/lib/iyakki") || mp.starts_with("/run/iyakki")
}

fn fnv(h: &mut u64, s: &str) {
    for b in s.bytes() {
        *h ^= b as u64;
        *h = h.wrapping_mul(0x100000001b3);
    }
}

/// A `Read + Seek` over a device read through the seam (for the partition-table reader).
struct DevReader<'a> {
    env: &'a dyn DiskEnv,
    dev: &'a str,
    pos: u64,
    len: u64,
}

impl Read for DevReader<'_> {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        let b = self.env.dev_read(self.dev, self.pos, buf.len()).map_err(std::io::Error::other)?;
        buf[..b.len()].copy_from_slice(&b);
        self.pos += b.len() as u64;
        Ok(b.len())
    }
}

impl Seek for DevReader<'_> {
    fn seek(&mut self, to: SeekFrom) -> std::io::Result<u64> {
        let np = match to {
            SeekFrom::Start(n) => n as i128,
            SeekFrom::End(d) => self.len as i128 + d as i128,
            SeekFrom::Current(d) => self.pos as i128 + d as i128,
        };
        if np < 0 {
            return Err(std::io::Error::other("seek before the start"));
        }
        self.pos = np as u64;
        Ok(self.pos)
    }
}

/// A `Read + Seek` over a device read through the seam.
pub fn dev_reader<'a>(env: &'a dyn DiskEnv, dev: &'a str, len: u64) -> impl Read + Seek + 'a {
    DevReader { env, dev, pos: 0, len }
}

impl Disks {
    pub fn new(env: Arc<dyn DiskEnv>) -> Disks {
        Disks { env, cache: Mutex::new(HashMap::new()), limiter: Mutex::new(crate::disks_crypt::Limiter::default()), held_clock: std::sync::atomic::AtomicU64::new(u64::MAX), tools: Mutex::new(Arc::new(crate::disks_fmt::NoTools)) }
    }

    /// Where the exFAT, NTFS and resize programs come from.
    pub fn set_tools(&self, t: Arc<dyn crate::disks_fmt::FsTools>) {
        if let Ok(mut g) = self.tools.lock() {
            *g = t;
        }
    }

    pub fn tools(&self) -> Arc<dyn crate::disks_fmt::FsTools> {
        self.tools.lock().map(|g| g.clone()).unwrap_or_else(|_| Arc::new(crate::disks_fmt::NoTools))
    }

    /// The engine on this machine.
    pub fn real() -> Disks {
        Disks::new(Arc::new(crate::disks_sys::RealEnv))
    }

    /// Forget what was read from the partitions (after a change we made).
    pub fn forget(&self) {
        if let Ok(mut c) = self.cache.lock() {
            c.clear();
        }
    }

    /// A cheap number that changes when a drive or partition appears or goes, or something is mounted or unmounted:
    /// two small text reads, no device is opened. The tile polls this and scans only when it changed.
    pub fn fingerprint(&self) -> u64 {
        let mut h = 0xcbf29ce484222325u64;
        fnv(&mut h, &self.env.read_text("/proc/partitions").unwrap_or_default());
        fnv(&mut h, &self.env.read_text("/proc/self/mountinfo").unwrap_or_default());
        fnv(&mut h, &self.env.read_text("/proc/swaps").unwrap_or_default());
        h
    }

    fn read_sys(&self, path: &str) -> String {
        self.env.read_text(path).unwrap_or_default().trim().to_string()
    }

    fn probe(&self, dev_name: &str, start: u64, size: u64) -> Probe {
        let key = format!("{dev_name}:{start}:{size}");
        if let Ok(c) = self.cache.lock() {
            if let Some((at, p)) = c.get(&key) {
                if at.elapsed() < PROBE_TTL {
                    return p.clone();
                }
            }
        }
        let want = PROBE_BYTES.min(size as usize);
        let p = match self.env.dev_read(&format!("/dev/{dev_name}"), 0, want) {
            Ok(b) => detect(&b),
            Err(_) => Probe { fs: Fs::Unknown, label: String::new(), uuid: String::new(), crypt: None },
        };
        if let Ok(mut c) = self.cache.lock() {
            c.insert(key, (Instant::now(), p.clone()));
        }
        p
    }

    /// Look at the machine: every drive with its partitions, what is on them and where they are mounted.
    pub fn scan(&self) -> Snapshot {
        let env = &*self.env;
        let mounts = parse_mountinfo(&env.read_text("/proc/self/mountinfo").unwrap_or_default());
        let swaps = env.read_text("/proc/swaps").unwrap_or_default();
        let mut majmin: HashMap<String, String> = HashMap::new();
        let mut names = env.list("/sys/block");
        names.retain(|n| !SKIP.iter().any(|p| n.starts_with(p)));
        names.sort();
        for n in &names {
            if let Some(d) = env.read_text(&format!("/sys/block/{n}/dev")) {
                majmin.insert(d.trim().to_string(), n.clone());
            }
            for sub in env.list(&format!("/sys/block/{n}")) {
                if let Some(d) = env.read_text(&format!("/sys/block/{n}/{sub}/dev")) {
                    majmin.insert(d.trim().to_string(), sub.clone());
                }
            }
        }
        let mount_of = |name: &str| -> Vec<&MountEnt> { mounts.iter().filter(|m| majmin.get(&m.majmin).is_some_and(|n| n == name) || (m.source.starts_with("/dev/") && (base(&m.source) == name || base(&m.source) == crate::disks_crypt::mapper_name(name)))).collect() };
        let swap_of = |name: &str| swaps.lines().skip(1).any(|l| l.split_whitespace().next().is_some_and(|d| base(d) == name));
        let mut disks = Vec::new();
        for n in names {
            let sd = format!("/sys/block/{n}");
            let sectors: u64 = self.read_sys(&format!("{sd}/size")).parse().unwrap_or(0);
            let size = sectors * 512;
            let sector = self.read_sys(&format!("{sd}/queue/logical_block_size")).parse().ok().filter(|s: &u32| *s >= 512).unwrap_or(512);
            let real = env.canonical(&sd).unwrap_or_default();
            let transport = if real.contains("/usb") {
                Transport::Usb
            } else if n.starts_with("nvme") {
                Transport::Nvme
            } else if n.starts_with("mmcblk") {
                Transport::Mmc
            } else if n.starts_with("vd") || n.starts_with("xvd") || real.contains("/virtio") {
                Transport::Virtual
            } else if real.contains("/ata") {
                Transport::Sata
            } else {
                Transport::Other
            };
            let sd_card = self.read_sys(&format!("{sd}/device/type")) == "SD";
            let removable = self.read_sys(&format!("{sd}/removable")) == "1" || sd_card;
            // serial: the first `serial` file found going up from the device (NVMe keeps it next to the model, USB on the device)
            let mut serial = self.read_sys(&format!("{sd}/device/serial"));
            if serial.is_empty() {
                let mut p = real.clone();
                while serial.is_empty() && p.len() > 5 && p != "/sys" {
                    serial = self.read_sys(&format!("{p}/serial"));
                    match p.rfind('/') {
                        Some(i) => p.truncate(i),
                        None => break,
                    }
                }
            }
            let (mut vendor, mut model) = (self.read_sys(&format!("{sd}/device/vendor")), self.read_sys(&format!("{sd}/device/model")));
            if model.is_empty() {
                model = self.read_sys(&format!("{sd}/device/name"));
            }
            vendor = vendor.split_whitespace().collect::<Vec<_>>().join(" ");
            model = model.split_whitespace().collect::<Vec<_>>().join(" ");
            let dev = format!("/dev/{n}");
            let ti = if size >= 4096 {
                let mut r = DevReader { env, dev: &dev, pos: 0, len: size };
                gpt::read_disk_table(&mut r, sector, size)
            } else {
                gpt::TableInfo { kind: gpt::Kind::None, sector_size: sector, parts: vec![], notes: vec![] }
            };
            let table = match ti.kind {
                gpt::Kind::None => Table::None,
                gpt::Kind::Mbr => Table::Mbr,
                gpt::Kind::Gpt => Table::Gpt,
            };
            let mut table = table;
            let mut parts = Vec::new();
            let mut subs: Vec<(u32, String)> = env.list(&sd).into_iter().filter_map(|s| env.read_text(&format!("{sd}/{s}/partition")).and_then(|t| t.trim().parse::<u32>().ok()).map(|num| (num, s))).collect();
            subs.sort();
            let mut holders: Vec<String> = env.list(&format!("{sd}/holders"));
            for (num, pn) in &subs {
                let psd = format!("{sd}/{pn}");
                let start = self.read_sys(&format!("{psd}/start")).parse::<u64>().unwrap_or(0) * 512;
                let psize = self.read_sys(&format!("{psd}/size")).parse::<u64>().unwrap_or(0) * 512;
                holders.extend(env.list(&format!("{psd}/holders")));
                parts.push(self.make_part(pn, *num, start, psize, &mount_of(pn), swap_of(pn), ti.parts.iter().find(|p| p.index == *num).map(|p| p.type_name.clone())));
            }
            if parts.is_empty() && size > 0 {
                // a file system straight on the disk (a "superfloppy": many USB sticks and SD cards)
                let p = self.make_part(&n, 0, 0, size, &mount_of(&n), swap_of(&n), None);
                if !matches!(p.fs, Fs::Empty | Fs::Unknown) || p.mounted() {
                    parts.push(p);
                    // a volume's boot sector ends in 55 AA like a partition table does: it is not one
                    table = Table::None;
                }
            }
            let mut system = false;
            let mut why = String::new();
            for p in &parts {
                if p.system {
                    system = true;
                    why = if let Some(mp) = p.mountpoints.iter().find(|m| is_system_mount(m)) {
                        format!("it holds the running system ({mp} is mounted from {})", p.name)
                    } else if p.swap_in_use {
                        format!("{} is in use as swap space", p.name)
                    } else {
                        format!("{} is an IYAKKI system volume ({})", p.name, p.label)
                    };
                    break;
                }
            }
            holders.sort();
            holders.dedup();
            disks.push(Disk { name: n.clone(), path: dev, vendor, model, serial, size_bytes: size, sector, removable, transport, read_only: self.read_sys(&format!("{sd}/ro")) == "1", table, table_notes: ti.notes, parts, system, system_why: why, holders });
        }
        Snapshot { disks, support: self.support() }
    }

    #[allow(clippy::too_many_arguments)]
    fn make_part(&self, name: &str, number: u32, start: u64, size: u64, mounts: &[&MountEnt], swap: bool, type_name: Option<String>) -> Part {
        let probe = self.probe(name, start, size);
        let mut mountpoints: Vec<String> = mounts.iter().map(|m| m.mp.clone()).collect();
        mountpoints.dedup();
        let space = mountpoints.first().and_then(|m| self.env.space(m));
        let mounted_ro = mounts.first().is_some_and(|m| m.opts.split(',').any(|o| o == "ro"));
        let system = mountpoints.iter().any(|m| is_system_mount(m)) || is_system_label(&probe.label) || swap;
        let unlocked = probe.crypt.is_some() && self.env.canonical(&crate::disks_crypt::mapper_path(name)).is_some();
        Part { name: name.to_string(), path: format!("/dev/{name}"), number, start_bytes: start, size_bytes: size, fs: probe.fs, label: probe.label, uuid: probe.uuid, mountpoints, space, mounted_ro, swap_in_use: swap, system, type_name: type_name.unwrap_or_default(), crypt: probe.crypt, unlocked }
    }

    /// Which file systems this kernel can open: `/proc/filesystems`, or a module of that name in `modules.dep`.
    pub fn support(&self) -> Vec<FsSupport> {
        let env = &*self.env;
        let fss = env.read_text("/proc/filesystems").unwrap_or_default();
        let rel = env.read_text("/proc/sys/kernel/osrelease").unwrap_or_default();
        let deps = if rel.trim().is_empty() { String::new() } else { env.read_text(&format!("/lib/modules/{}/modules.dep", rel.trim())).unwrap_or_default() };
        let builtin = if rel.trim().is_empty() { String::new() } else { env.read_text(&format!("/lib/modules/{}/modules.builtin", rel.trim())).unwrap_or_default() };
        let has = |d: &str| fss.lines().any(|l| l.split_whitespace().last() == Some(d)) || deps.contains(&format!("/{d}.ko")) || builtin.contains(&format!("/{d}.ko"));
        let mut out: Vec<FsSupport> = Vec::new();
        for fs in [Fs::Ext4, Fs::Fat32, Fs::Exfat, Fs::Ntfs, Fs::Btrfs, Fs::Xfs, Fs::F2fs, Fs::Iso9660, Fs::HfsPlus] {
            let d = fs.driver().unwrap_or("");
            out.push(FsSupport { fs, kernel: has(d), driver: d });
        }
        out
    }

    // -----------------------------------------------------------------------------------------------------------------
    // mounting
    // -----------------------------------------------------------------------------------------------------------------

    /// A free folder name under the person's `media` folder for this partition: its label, else its id, else its device name.
    pub fn pick_target(&self, p: &Part, taken: &[String], home: &str) -> String {
        let root = media_dir(home);
        let raw = if !p.label.is_empty() {
            p.label.clone()
        } else if !p.uuid.is_empty() {
            p.uuid.clone()
        } else {
            p.name.clone()
        };
        let mut s: String = raw.chars().map(|c| if c.is_alphanumeric() || c == '-' || c == '_' || c == '.' { c } else { '_' }).collect();
        s = s.trim_matches('.').chars().take(32).collect();
        if s.is_empty() {
            s = p.name.clone();
        }
        let existing = self.env.list(&root);
        let mut n = 1;
        let mut t = format!("{root}/{s}");
        while taken.contains(&t) || existing.iter().any(|e| format!("{root}/{e}") == t) {
            n += 1;
            t = format!("{root}/{s}-{n}");
        }
        t
    }

    /// What mounting `part` would do, or why it cannot be done. Pure: nothing is changed.
    pub fn plan_mount(&self, snap: &Snapshot, who: &Actor, part: &str, opts: &MountOpts) -> Result<MountPlan, String> {
        let (d, p) = snap.part(part).ok_or_else(|| format!("{part}: there is no such partition (the drive may have been unplugged)."))?;
        // an unlocked container that is not open yet is planned as the file system inside it
        if p.unlocked && !p.mounted() {
            if let Some(inner) = self.inner_part(p) {
                return self.plan_for(snap, who, d, &inner, opts);
            }
        }
        self.plan_for(snap, who, d, p, opts)
    }

    /// [`plan_mount`](Self::plan_mount) for a partition already looked up (an unlocked volume is planned as the file system inside it).
    pub(crate) fn plan_for(&self, snap: &Snapshot, who: &Actor, d: &Disk, p: &Part, opts: &MountOpts) -> Result<MountPlan, String> {
        may_use(who, d)?;
        if p.system {
            return Err(format!("{} is part of the running system and stays as it is.", p.name));
        }
        if p.mounted() {
            return Err(format!("{} is already open at {}.", p.name, p.mountpoints[0]));
        }
        if let Some(why) = p.fs.never_mountable() {
            return Err(why.into());
        }
        let driver = p.fs.driver().ok_or_else(|| "Nothing IYAKKI knows how to open.".to_string())?;
        if !snap.kernel_has(driver) {
            return Err(format!("This system has no {} driver, so it cannot open {} yet. (It is not part of this image; nothing is wrong with the drive.)", driver, p.fs.name()));
        }
        if opts.allow_exec && who.role != Role::Admin {
            return Err("Only an administrator can allow running programs from a drive.".into());
        }
        let mut forced_ro = None;
        let mut rdonly = opts.read_only;
        if d.read_only {
            rdonly = true;
            forced_ro = Some("the drive is write-protected".to_string());
        } else if p.fs.read_only_always() {
            rdonly = true;
            forced_ro = Some(format!("{} is only ever opened for reading here", p.fs.name()));
        }
        let flags = MountFlags { rdonly, nosuid: true, nodev: true, noexec: !opts.allow_exec };
        let data = match p.fs {
            Fs::Fat12 | Fs::Fat16 | Fs::Fat32 => format!("uid={},gid={},umask=0077,utf8=1,flush,shortname=mixed", who.uid, who.gid),
            Fs::Exfat => format!("uid={},gid={},umask=0077,iocharset=utf8", who.uid, who.gid),
            Fs::Ntfs => format!("uid={},gid={},umask=0077,windows_names", who.uid, who.gid),
            Fs::Iso9660 => format!("uid={},gid={},iocharset=utf8", who.uid, who.gid),
            _ => String::new(),
        };
        let taken: Vec<String> = snap.disks.iter().flat_map(|d| d.parts.iter().flat_map(|p| p.mountpoints.clone())).collect();
        Ok(MountPlan { dev: p.path.clone(), target: self.pick_target(p, &taken, &who.home), fstype: driver, flags, data, forced_ro })
    }

    /// Plain words for what a failed mount means.
    pub fn explain_mount_error(fs: Fs, err: &str) -> String {
        let e = err.to_lowercase();
        if e.contains("no such device") || e.contains("nodev") {
            format!("This system has no driver for {}. Nothing is wrong with the drive.", fs.name())
        } else if e.contains("busy") {
            "The drive is busy: something else already uses it.".into()
        } else if e.contains("invalid argument") || e.contains("wrong fs type") {
            format!("The drive could not be opened as {}: it is damaged, was not shut down cleanly, or is a different kind of volume. Check it on the computer it came from.", fs.name())
        } else if e.contains("permission") || e.contains("not permitted") {
            "This system is not allowed to open it (a read-only or locked drive).".into()
        } else {
            format!("Could not open it: {err}")
        }
    }

    /// Mount a partition. Never changes what is on it; a Windows volume that was not shut down cleanly is opened read-only.
    pub fn mount(&self, who: &Actor, part: &str, opts: &MountOpts, audit: AuditFn) -> Result<Mounted, String> {
        let snap = self.scan();
        let plan = self.plan_mount(&snap, who, part, opts).inspect_err(|e| audit("disks.mount.refused", &format!("part={part} why={e}")))?;
        let (d, p) = snap.part(part).ok_or("The drive is gone.")?;
        self.run_mount(who, d, p, plan, audit)
    }

    /// The steps of a mount once it is planned (shared with unlocked volumes).
    pub(crate) fn run_mount(&self, who: &Actor, d: &Disk, p: &Part, plan: MountPlan, audit: AuditFn) -> Result<Mounted, String> {
        let (fs, ident) = (p.fs, format!("{} part={} fs={} ro={} exec={}", d.audit_id(), p.name, p.fs.name(), plan.flags.rdonly, !plan.flags.noexec));
        self.env.make_dir(&plan.target).map_err(|e| format!("Could not create the folder {}: {e}", plan.target))?;
        if who.uid != 0 {
            // the folders are the person's (the drive's own files follow the mount options or the file system)
            let _ = self.env.chown(&media_dir(&who.home), who.uid, who.gid);
            let _ = self.env.chown(&plan.target, who.uid, who.gid);
        }
        let mut notes = Vec::new();
        let mut flags = plan.flags;
        let mut res = self.env.mount(&plan.dev, &plan.target, plan.fstype, flags, &plan.data);
        if let (Err(first), true, Fs::Ntfs) = (&res, !flags.rdonly, fs) {
            // a Windows volume left dirty or hibernated is not written to: try again for reading
            flags.rdonly = true;
            let retry = self.env.mount(&plan.dev, &plan.target, plan.fstype, flags, &plan.data);
            if retry.is_ok() {
                notes.push(format!("Opened read-only: Windows did not shut this drive down cleanly (fast start-up or hibernation, or it needs a check). To write to it, shut Windows down completely or let Windows repair it ('repair needs Windows'), then plug it in again. ({first})"));
            }
            res = retry;
        }
        match res {
            Ok(()) => {
                if let Some(why) = &plan.forced_ro {
                    notes.push(format!("Opened read-only: {why}."));
                }
                if flags.noexec {
                    notes.push("Programs on it cannot be run from it (the safe default).".into());
                }
                self.forget();
                audit("disks.mount", &format!("{ident} at={}", plan.target));
                Ok(Mounted { target: plan.target, read_only: flags.rdonly, notes })
            }
            Err(e) => {
                let _ = self.env.remove_dir(&plan.target);
                audit("disks.mount.failed", &format!("{ident} error={e}"));
                Err(Self::explain_mount_error(fs, &e))
            }
        }
    }

    // -----------------------------------------------------------------------------------------------------------------
    // what keeps a drive busy
    // -----------------------------------------------------------------------------------------------------------------

    /// Programs using anything under `mp`: (process id, name), by looking at `/proc/<pid>/{cwd,root,exe,fd/*}`.
    pub fn users_of(&self, mp: &str) -> Vec<(u32, String)> {
        let env = &*self.env;
        let under = |p: &str| p == mp || p.strip_prefix(mp).is_some_and(|r| r.starts_with('/'));
        let mut out = Vec::new();
        for pid in env.list("/proc") {
            let Ok(n) = pid.parse::<u32>() else { continue };
            let base_p = format!("/proc/{pid}");
            let mut hit = ["cwd", "root", "exe"].iter().any(|l| env.read_link(&format!("{base_p}/{l}")).is_some_and(|t| under(&t)));
            if !hit {
                hit = env.list(&format!("{base_p}/fd")).iter().any(|f| env.read_link(&format!("{base_p}/fd/{f}")).is_some_and(|t| under(&t)));
            }
            if hit {
                let name = env.read_text(&format!("{base_p}/comm")).unwrap_or_default().trim().to_string();
                out.push((n, if name.is_empty() { "a program".into() } else { name }));
            }
        }
        out
    }

    /// "Close bash (process 12), vlc (process 90) and try again."
    pub fn busy_message(&self, what: &str, mp: &str) -> String {
        let users = self.users_of(mp);
        if users.is_empty() {
            return format!("{what} is busy: something is still using it, but IYAKKI could not tell what. Close any windows or programs that may be open on it, wait a few seconds and try again. Nothing was forced.");
        }
        let mut names: Vec<String> = users.iter().take(5).map(|(p, n)| format!("{n} (process {p})")).collect();
        if users.len() > 5 {
            names.push(format!("{} more", users.len() - 5));
        }
        format!("{what} is busy: {} is using it. Close {} and try again. Nothing was forced.", names.join(", "), if users.len() == 1 { "it" } else { "them" })
    }

    // -----------------------------------------------------------------------------------------------------------------
    // unmount and eject
    // -----------------------------------------------------------------------------------------------------------------

    /// A person with the User role closes only what is open in their own `media` folder.
    pub(crate) fn own_mounts_only(who: &Actor, p: &Part) -> Result<(), String> {
        let mine = media_dir(&who.home) + "/";
        if who.role == Role::User && p.mountpoints.iter().any(|m| is_media_mount(m) && !m.starts_with(&mine)) {
            return Err(format!("{} is open in another person's folder. Only that person or an administrator can close it.", p.name));
        }
        Ok(())
    }

    /// The drives open in `home`'s `media` folder: (partition, folder).
    pub fn mounted_in(&self, home: &str) -> Vec<(String, String)> {
        let prefix = media_dir(home) + "/";
        let mounts = parse_mountinfo(&self.env.read_text("/proc/self/mountinfo").unwrap_or_default());
        mounts.into_iter().filter(|m| m.mp.starts_with(&prefix)).map(|m| (base(&m.source).to_string(), m.mp)).collect()
    }

    /// At sign-out: with the switch on, close this person's drives (plainly, never forced); with it off, close nothing and say which
    /// drives stay open. Returns the sentence for the person, or `None` when nothing is open.
    pub fn at_sign_out(&self, who: &Actor, audit: AuditFn) -> Option<String> {
        let open = self.mounted_in(&who.home);
        if open.is_empty() {
            return None;
        }
        let names: Vec<String> = open.iter().map(|(_, m)| m.rsplit('/').next().unwrap_or(m).to_string()).collect();
        if !signout_pref(&who.home) {
            audit("disks.signout.left_open", &format!("user={} folders={}", who.user, open.iter().map(|(_, m)| m.as_str()).collect::<Vec<_>>().join(",")));
            return Some(format!("A drive is still open in your home folder: {}. Signing out does not close it; open DISKS to eject it before you remove it.", names.join(", ")));
        }
        let mut failed = Vec::new();
        for (part, _) in &open {
            if let Err(e) = self.unmount(who, part, audit) {
                failed.push(e);
            }
        }
        if failed.is_empty() { None } else { Some(format!("Some drives could not be closed at sign-out and stay open: {}", failed.join(" "))) }
    }

    pub(crate) fn unmount_part(&self, d: &Disk, p: &Part, audit: AuditFn) -> Result<(), String> {
        for mp in &p.mountpoints {
            if is_system_mount(mp) {
                return Err(format!("{mp} is part of the running system and stays mounted."));
            }
        }
        self.env.sync();
        for mp in &p.mountpoints {
            if let Err(e) = self.env.umount(mp) {
                let m = if e.to_lowercase().contains("busy") { self.busy_message(&format!("{} ({mp})", p.title()), mp) } else { format!("Could not unmount {mp}: {e}") };
                audit("disks.unmount.failed", &format!("{} part={} error={e}", d.audit_id(), p.name));
                return Err(m);
            }
            if is_media_mount(mp) {
                // only an empty folder goes (remove_dir refuses anything else)
                let _ = self.env.remove_dir(mp);
            }
        }
        audit("disks.unmount", &format!("{} part={}", d.audit_id(), p.name));
        Ok(())
    }

    /// Unmount a partition: flushes first, names what keeps it busy, never forces.
    pub fn unmount(&self, who: &Actor, part: &str, audit: AuditFn) -> Result<String, String> {
        let snap = self.scan();
        let (d, p) = snap.part(part).ok_or_else(|| format!("{part}: there is no such partition."))?;
        may_use(who, d).inspect_err(|e| audit("disks.unmount.refused", &format!("{} part={part} why={e}", d.audit_id())))?;
        if !p.mounted() {
            return Err(format!("{} is not open.", p.name));
        }
        Self::own_mounts_only(who, p).inspect_err(|e| audit("disks.unmount.refused", &format!("{} part={part} why={e}", d.audit_id())))?;
        self.unmount_part(d, p, audit).inspect_err(|e| {
            if p.mountpoints.iter().any(|m| is_system_mount(m)) {
                audit("disks.unmount.refused", &format!("{} part={part} why={e}", d.audit_id()));
            }
        })?;
        self.forget();
        Ok(format!("{} is closed. Nothing was lost; it is safe to eject.", p.title()))
    }

    /// Safe eject: flush, unmount everything on the drive, switch a USB drive off. The message says when it can be removed.
    pub fn eject(&self, who: &Actor, disk: &str, audit: AuditFn) -> Result<String, String> {
        let snap = self.scan();
        let d = snap.disk(disk).ok_or_else(|| format!("{disk}: there is no such drive (it may have been unplugged already)."))?;
        if let Err(e) = may_use(who, d) {
            audit("disks.eject.refused", &format!("{} why={e}", d.audit_id()));
            return Err(e);
        }
        if d.system {
            let m = format!("{} {}: it cannot be ejected.", d.name, d.system_why);
            audit("disks.eject.refused", &format!("{} why=system disk", d.audit_id()));
            return Err(m);
        }
        if let Some(p) = d.parts.iter().find(|p| p.unlocked) {
            let m = format!("{} is unlocked. Lock it first (the Lock button), then eject the drive.", p.title());
            audit("disks.eject.refused", &format!("{} why=unlocked volume {}", d.audit_id(), p.name));
            return Err(m);
        }
        for p in d.parts.iter().filter(|p| p.mounted()) {
            if let Err(e) = Self::own_mounts_only(who, p) {
                audit("disks.eject.refused", &format!("{} why={e}", d.audit_id()));
                return Err(e);
            }
        }
        let mut done = Vec::new();
        for p in d.parts.iter().filter(|p| p.mounted()) {
            if let Err(e) = self.unmount_part(d, p, audit) {
                let tail = if done.is_empty() { String::new() } else { format!(" ({} already closed.)", done.join(", ")) };
                return Err(format!("{e}{tail} The drive was not ejected."));
            }
            done.push(p.name.clone());
        }
        self.env.sync();
        self.forget();
        let msg = if d.transport == Transport::Usb {
            match self.env.write_sys(&format!("/sys/block/{}/device/delete", d.name), "1") {
                Ok(()) => "You can remove it now.".to_string(),
                Err(e) => format!("Everything on it is closed and saved. It could not be switched off ({e}), but it is safe to remove."),
            }
        } else if d.removable_media() {
            "Everything on it is closed and saved. You can remove it now.".to_string()
        } else {
            "Everything on it is closed and saved. This is an internal drive: it stays powered and in place.".to_string()
        };
        audit("disks.eject", &d.audit_id());
        Ok(msg)
    }
}
