//! `LocalInstaller`: the core's [`Installer`] on this machine (F-ARCH-05). The places it looks (sysfs, the device folder, the mounts and swaps
//! files) are fields, so a test points it at a made-up tree; the writing is `iyakki_usb::install::execute`, which refuses a disk that got mounted
//! while the person was typing.

use std::path::{Path, PathBuf};

use iyakki_core_api::installer::{running_disk, DiskInfo, InstallReport, Installer, Plan, Progress, SourceInfo};
use iyakki_usb::install;

#[derive(Clone, Debug)]
pub struct LocalInstaller {
    pub sys: PathBuf,
    pub dev: PathBuf,
    pub mounts: PathBuf,
    pub swaps: PathBuf,
    /// Copy from this template file instead of the boot disk.
    pub template: Option<PathBuf>,
    /// This computer was started through UEFI.
    pub uefi: bool,
}

impl LocalInstaller {
    /// This machine.
    pub fn system() -> LocalInstaller {
        let p = install::Paths::real();
        LocalInstaller { sys: p.sys, dev: p.dev, mounts: p.mounts, swaps: p.swaps, template: None, uefi: install::booted_uefi() }
    }

    /// A made-up machine under `root` (`sys`, `dev`, `proc/mounts`, `proc/swaps`), booted by BIOS.
    pub fn under(root: &Path) -> LocalInstaller {
        LocalInstaller { sys: root.join("sys"), dev: root.join("dev"), mounts: root.join("proc/mounts"), swaps: root.join("proc/swaps"), template: None, uefi: false }
    }

    fn rd(p: &Path) -> String {
        std::fs::read_to_string(p).unwrap_or_default()
    }
}

impl Installer for LocalInstaller {
    fn disks(&self) -> Vec<DiskInfo> {
        let mut d = install::list_disks(&self.sys, &self.dev, &Self::rd(&self.mounts), &Self::rd(&self.swaps));
        install::probe_contents(&mut d, &self.dev);
        d
    }

    fn source(&self, disks: &[DiskInfo]) -> Result<SourceInfo, String> {
        if let Some(t) = &self.template {
            return install::read_source(t);
        }
        // the disk this system runs from is the one to copy (and is never offered as the target); only when that is not an IYAKKI
        // boot disk does the older rule apply (a removable boot disk wins)
        let names: Vec<&str> = disks.iter().map(|d| d.drive.name.as_str()).collect();
        if let Some(run) = running_disk(&Self::rd(&self.mounts), &names) {
            if let Ok(s) = install::read_source(&self.dev.join(&run)) {
                return Ok(s);
            }
        }
        install::find_source(disks, &self.dev, None)
    }

    fn uefi(&self) -> bool {
        self.uefi
    }

    fn execute(&self, plan: &Plan, progress: &mut dyn FnMut(Progress)) -> Result<InstallReport, String> {
        install::execute(plan, false, &Self::rd(&self.mounts), progress)
    }
}
