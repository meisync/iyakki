//! Install to disk, as the core serves it (UI/OS split plan, F-ARCH-05). Design note:
//!
//! * The page needs five things from the machine: the disks (with what is on them), the source it copies from (the boot stick, or a
//!   template), whether the computer started through UEFI, whether anything can be installed at all, and the writing itself with its
//!   progress. All five are [`Installer`]. Everything else (the plan, the refusals, the layout, the confirmation words) is pure and is
//!   re-exported from `iyakki-usb` so a tile names it through the core interface.
//! * `iyakki_core_local::LocalInstaller` reads sysfs and the devices (roots as fields, so a test uses a made-up tree) and writes the disk;
//!   `iyakki_core_fake::FakeInstaller` holds canned disks and a canned source and records the plans it was asked to write.
//! * Listing is a plain read (`system.read`); writing a disk needs `system.disk.install` and asks every time (see `guarded.rs`).
//! * A machine that does not say otherwise has [`NoInstaller`]: nothing to install from, nothing is written.

pub use iyakki_usb::install::{confirmation_ok, make_plan_with, partition_name, valid_hostname, Choices, DiskInfo, InstallReport, Plan, SourceInfo, TableChoice, MIN_DISK};
pub use iyakki_usb::writer::{Phase, Progress};
pub use iyakki_usb::bootpack::Extracted as BootFile;
pub use iyakki_usb::resize::{FsTool, HostTools, Located, ShrinkPlan};

/// The disk work of the install chat tools and of `iyakki dev pack` that is not the Install page's own write: the plan and the write of the
/// legacy BIOS boot setup on a shared disk (F-INST-13, F-INST-29), the look at one partition before it is shrunk (F-INST-30), and the boot
/// files of an install image (F-PKG). `iyakki_core_local::LocalDiskTools` is the machine, `iyakki_core_fake::FakeDiskTools` a test. The cards,
/// the administrator rule and the second typed confirmation stay with the callers (the tools); every error is a sentence for the person.
pub trait DiskTools: Send + Sync {
    /// The plan text and the drive's name for the installer arguments. Writes nothing.
    fn bios_plan(&self, args: &[String]) -> Result<(String, String), String>;
    /// Plan again from a fresh read of the disk and write; `confirm` is the drive's name. The answer is the notes of the writer.
    fn bios_write(&self, args: &[String], confirm: &str) -> Result<String, String>;
    /// The partition `part` of disk `disk` as the shrink tools need to see it (the table, the device node, the mounts).
    fn locate(&self, disk: &str, part: u32) -> Result<Located, String>;
    /// Extract the boot files of an install image into `dir` (the image is only read) and say what was written.
    fn extract_boot_pack(&self, image: &std::path::Path, dir: &std::path::Path) -> Result<Vec<BootFile>, String>;
}

/// The handle of a machine that does not say otherwise: nothing can be planned or written.
pub struct NoDiskTools;

impl DiskTools for NoDiskTools {
    fn bios_plan(&self, _: &[String]) -> Result<(String, String), String> {
        Err("this machine cannot install to a disk".into())
    }
    fn bios_write(&self, _: &[String], _: &str) -> Result<String, String> {
        Err("this machine cannot install to a disk".into())
    }
    fn locate(&self, _: &str, _: u32) -> Result<Located, String> {
        Err("this machine has no disks to look at".into())
    }
    fn extract_boot_pack(&self, _: &std::path::Path, _: &std::path::Path) -> Result<Vec<BootFile>, String> {
        Err("this machine cannot read an install image".into())
    }
}

/// What the Install page does to the machine. Errors are sentences for the person.
pub trait Installer: Send + Sync {
    /// The disks of this computer, with what is on each (refused ones included: the plan says why).
    fn disks(&self) -> Vec<DiskInfo>;
    /// What to copy: the template when one is set, else the disk this system runs from, else a removable IYAKKI boot disk.
    fn source(&self, disks: &[DiskInfo]) -> Result<SourceInfo, String>;
    /// This computer was started through UEFI (what the "Auto" partition table looks at, besides the disk).
    fn uefi(&self) -> bool;
    /// Is there anything to install from?
    fn available(&self) -> bool {
        self.source(&self.disks()).is_ok()
    }
    /// Write the plan to its disk. The mounts are looked at again just before writing. `progress` is called as the phases advance.
    fn execute(&self, plan: &Plan, progress: &mut dyn FnMut(Progress)) -> Result<InstallReport, String>;
}

/// The installer of a machine that has none: nothing to install from, nothing written.
pub struct NoInstaller;

impl Installer for NoInstaller {
    fn disks(&self) -> Vec<DiskInfo> {
        Vec::new()
    }
    fn source(&self, _: &[DiskInfo]) -> Result<SourceInfo, String> {
        Err("this machine cannot install to a disk".into())
    }
    fn uefi(&self) -> bool {
        false
    }
    fn execute(&self, _: &Plan, _: &mut dyn FnMut(Progress)) -> Result<InstallReport, String> {
        Err("this machine cannot install to a disk".into())
    }
}

/// The disk that holds the running system: the one with the IYAKKI boot or data volume mounted from it (`/proc/mounts` text).
pub fn running_disk(mounts: &str, names: &[&str]) -> Option<String> {
    let dev = mounts.lines().filter_map(|l| {
        let mut f = l.split_whitespace();
        let (src, at) = (f.next()?, f.next()?);
        matches!(at, "/.stage1/boot" | "/.stage1/data" | "/var/lib/iyakki").then(|| src.rsplit('/').next().unwrap_or(src).to_string())
    }).next()?;
    names.iter().find(|d| (1..=16).any(|n| partition_name(d, n) == dev)).map(|d| d.to_string())
}

#[cfg(test)]
mod disk_tools_tests {
    use super::*;

    #[test]
    fn a_machine_without_the_handle_plans_and_writes_nothing() {
        let n = NoDiskTools;
        assert!(n.bios_plan(&[]).is_err() && n.bios_write(&[], "sda").is_err());
        assert!(n.locate("sda", 1).is_err());
        assert!(n.extract_boot_pack(std::path::Path::new("a.img"), std::path::Path::new("out")).is_err());
    }
}
