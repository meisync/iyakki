//! Types moved out of `iyakki_agent::disks` (F-SPLIT-03, scripted by dist/split/move_types.py); the agent re-exports them.

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
