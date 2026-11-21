//! `LocalDiskTools`: the core's [`DiskTools`] on this machine (F-ARCH-07). The BIOS plan and write are the installer's own `plan_cli`, the look at a
//! partition is `Located::of_machine`, the boot files of an image are `bootpack::extract`. The machine (sysfs, device folder, mounts, swaps) is a
//! field, so a test points it at a made-up tree. Moved here from the agent's `settingstools_install` (`RealBios`, `real_source`) and `devmode::cli_adb`.

use std::path::Path;

use iyakki_core_api::installer::{BootFile, DiskTools, Located};
use iyakki_usb::plan_cli::{self, Machine};

pub struct LocalDiskTools {
    /// Where the disks come from, made on every call: the mounts change between calls.
    machine: Box<dyn Fn() -> Machine + Send + Sync>,
}

impl LocalDiskTools {
    /// This computer.
    pub fn system() -> LocalDiskTools {
        LocalDiskTools { machine: Box::new(Machine::real) }
    }

    /// A made-up computer (tests): `sys`, `dev`, the mounts and the swaps as given.
    pub fn under(sys: &Path, dev: &Path, mounts: &str) -> LocalDiskTools {
        let (sys, dev, mounts) = (sys.to_path_buf(), dev.to_path_buf(), mounts.to_string());
        LocalDiskTools { machine: Box::new(move || Machine { sys: sys.clone(), dev: dev.clone(), proc_mounts: mounts.clone(), swaps: String::new() }) }
    }
}

impl DiskTools for LocalDiskTools {
    fn bios_plan(&self, args: &[String]) -> Result<(String, String), String> {
        let p = plan_cli::prepare(args, &(self.machine)())?;
        Ok((p.plan.render(), p.model.name.clone()))
    }

    fn bios_write(&self, args: &[String], confirm: &str) -> Result<String, String> {
        let m = (self.machine)();
        let p = plan_cli::prepare(args, &m)?;
        let r = plan_cli::write_prepared(&p, &m, confirm, &mut |_| {})?;
        Ok(r.notes.join("\n"))
    }

    fn locate(&self, disk: &str, part: u32) -> Result<Located, String> {
        Located::of_machine(&(self.machine)(), disk, part)
    }

    fn extract_boot_pack(&self, image: &Path, dir: &Path) -> Result<Vec<BootFile>, String> {
        iyakki_usb::bootpack::extract(image, dir)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tree(tag: &str) -> std::path::PathBuf {
        let d = std::env::temp_dir().join(format!("iyakki-disktools-{}-{tag}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(d.join("sys/block")).unwrap();
        std::fs::create_dir_all(d.join("dev")).unwrap();
        d
    }

    #[test]
    fn a_disk_that_is_not_there_is_refused_by_the_plan_the_write_and_the_look_before_anything_is_written() {
        let d = tree("nodisk");
        let t = LocalDiskTools::under(&d.join("sys"), &d.join("dev"), "");
        let args: Vec<String> = ["--plan", "--disk", "sdz", "--mode", "free-space", "--bios"].iter().map(|s| s.to_string()).collect();
        assert!(t.bios_plan(&args).is_err());
        assert!(t.bios_write(&args, "sdz").is_err());
        assert!(t.locate("sdz", 2).is_err());
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn a_file_that_is_not_an_install_image_gives_no_boot_files_and_writes_nothing_into_the_folder() {
        let d = tree("noimage");
        let img = d.join("not-an-image.img");
        std::fs::write(&img, vec![0u8; 4096]).unwrap();
        let out = d.join("out");
        std::fs::create_dir_all(&out).unwrap();
        let t = LocalDiskTools::under(&d.join("sys"), &d.join("dev"), "");
        assert!(t.extract_boot_pack(&img, &out).is_err());
        assert!(t.extract_boot_pack(&d.join("missing.img"), &out).is_err());
        assert_eq!(std::fs::read_dir(&out).unwrap().count(), 0);
        let _ = std::fs::remove_dir_all(&d);
    }
}
