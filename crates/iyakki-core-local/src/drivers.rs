//! `LocalDrivers`: the core's [`Drivers`] on this machine (F-ARCH-05). It reads the machine through the roots it was given (sysfs and proc)
//! and the installed packs through the driver file system of the data folder, so a test points it at a made-up tree. Nothing is written.

use std::path::Path;

use iyakki_core_api::drivers::{apply, known, Drivers, Machine, Record, Roots, Row};

pub struct LocalDrivers {
    pub fs: apply::Fs,
    pub roots: Roots,
}

impl LocalDrivers {
    /// This machine: the real roots and the driver file system of the data folder (the device image uses the real firmware folder, any
    /// other machine a sandbox folder inside the data folder).
    pub fn system(data: &Path) -> LocalDrivers {
        LocalDrivers { fs: apply::Fs::for_data(data).0, roots: Roots::real() }
    }

    /// A made-up machine under `root` (`sys`, `proc`, `data`, `lib/firmware`, ...).
    pub fn under(root: &Path) -> LocalDrivers {
        LocalDrivers { fs: apply::Fs::under(root), roots: Roots { sys: root.join("sys"), proc: root.join("proc") } }
    }
}

impl Drivers for LocalDrivers {
    fn machine(&self) -> Machine {
        Machine::collect(&self.roots)
    }
    fn rows(&self) -> Vec<Row> {
        known::for_machine(&self.machine(), &|p| apply::firmware_present(&self.fs, p), &apply::active_ids(&self.fs))
    }
    fn installed(&self) -> Vec<Record> {
        apply::installed(&self.fs)
    }
    fn is_disabled(&self, id: &str) -> bool {
        apply::is_disabled(&self.fs, id)
    }
    fn pack_files(&self, id: &str) -> Option<(usize, usize)> {
        apply::read_record(&self.fs, id).map(|r| (r.files.len(), r.files.iter().filter(|f| self.fs.fw_root.join(f).exists()).count()))
    }
    fn remove_pack(&self, id: &str) -> Result<String, String> {
        if apply::read_record(&self.fs, id).is_none() {
            return Err(format!("no driver pack `{id}` is installed"));
        }
        apply::remove_pack(&self.fs, id).map(|r| r.lines.join(" "))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_made_up_machine_with_nothing_installed_reads_empty_and_nothing_is_disabled() {
        let d = std::env::temp_dir().join(format!("iyakki-ldrv-{}", std::process::id()));
        std::fs::create_dir_all(d.join("sys/class/dmi/id")).unwrap();
        std::fs::write(d.join("sys/class/dmi/id/sys_vendor"), "Acme\n").unwrap();
        std::fs::write(d.join("sys/class/dmi/id/product_name"), "Box1\n").unwrap();
        let l = LocalDrivers::under(&d);
        let m = l.machine();
        assert_eq!((m.vendor.as_str(), m.product.as_str()), ("Acme", "Box1"));
        assert!(l.installed().is_empty());
        assert!(!l.is_disabled("anything"));
        assert!(l.rows().iter().all(|r| !r.installed), "nothing is installed on a made-up machine");
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn a_pack_that_is_not_recorded_has_no_files_and_cannot_be_removed() {
        let d = std::env::temp_dir().join(format!("iyakki-ldrv-pack-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        let l = LocalDrivers::under(&d);
        assert_eq!(l.pack_files("mac-wifi-bcm4360"), None);
        assert!(l.remove_pack("mac-wifi-bcm4360").unwrap_err().contains("no driver pack"));
        let _ = std::fs::remove_dir_all(&d);
    }
}
