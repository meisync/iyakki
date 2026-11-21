//! The fake [`DiskTools`]: a canned plan, a canned write answer, an image file that stands in for a disk, canned boot files; every call recorded and
//! nothing written to a disk.

use std::path::{Path, PathBuf};
use std::sync::Mutex;

use iyakki_core_api::installer::{BootFile, DiskTools, Located};

#[derive(Default)]
struct State {
    plan: Option<(String, String)>,
    write: Option<String>,
    disk_file: Option<PathBuf>,
    boot_files: Vec<BootFile>,
    calls: Vec<String>,
}

#[derive(Default)]
pub struct FakeDiskTools {
    state: Mutex<State>,
}

impl FakeDiskTools {
    pub fn new() -> FakeDiskTools {
        FakeDiskTools::default()
    }
    /// What `bios_plan` answers: the plan text and the drive's name (default: refused).
    pub fn set_plan(&self, text: &str, drive: &str) {
        self.state.lock().unwrap().plan = Some((text.into(), drive.into()));
    }
    /// What `bios_write` answers when the confirmation is the drive's name (default: refused).
    pub fn set_write(&self, notes: &str) {
        self.state.lock().unwrap().write = Some(notes.into());
    }
    /// An image file that `locate` reads as the disk (default: no disk).
    pub fn set_disk_file(&self, f: &Path) {
        self.state.lock().unwrap().disk_file = Some(f.to_path_buf());
    }
    pub fn set_boot_files(&self, f: Vec<BootFile>) {
        self.state.lock().unwrap().boot_files = f;
    }
    pub fn calls(&self) -> Vec<String> {
        self.state.lock().unwrap().calls.clone()
    }
    fn rec(&self, what: String) {
        self.state.lock().unwrap().calls.push(what);
    }
}

impl DiskTools for FakeDiskTools {
    fn bios_plan(&self, args: &[String]) -> Result<(String, String), String> {
        self.rec(format!("bios_plan({})", args.join(" ")));
        self.state.lock().unwrap().plan.clone().ok_or_else(|| "nothing to plan on the fake machine".into())
    }
    fn bios_write(&self, args: &[String], confirm: &str) -> Result<String, String> {
        let (plan, write) = {
            let s = self.state.lock().unwrap();
            (s.plan.clone(), s.write.clone())
        };
        let (_, drive) = plan.ok_or("nothing to plan on the fake machine")?;
        if confirm != drive {
            return Err(format!("the confirmation must be the drive's name, {drive}"));
        }
        self.rec(format!("bios_write({}, {confirm})", args.join(" ")));
        write.ok_or_else(|| "nothing to write on the fake machine".into())
    }
    fn locate(&self, disk: &str, part: u32) -> Result<Located, String> {
        self.rec(format!("locate({disk}, {part})"));
        match self.state.lock().unwrap().disk_file.clone() {
            Some(f) => Located::of_file(&f),
            None => Err(format!("no disk {disk} on the fake machine")),
        }
    }
    fn extract_boot_pack(&self, image: &Path, dir: &Path) -> Result<Vec<BootFile>, String> {
        self.rec(format!("extract_boot_pack({}, {})", image.display(), dir.display()));
        Ok(self.state.lock().unwrap().boot_files.clone())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_fake_plans_what_it_was_given_writes_only_with_the_drives_name_and_records_the_writes() {
        let f = FakeDiskTools::new();
        assert!(f.bios_plan(&[]).is_err() && f.locate("sda", 1).is_err());
        f.set_plan("write sector 0", "sda");
        f.set_write("saved sector 0");
        assert_eq!(f.bios_plan(&["--plan".into()]).unwrap(), ("write sector 0".into(), "sda".into()));
        assert!(f.bios_write(&[], "sdb").unwrap_err().contains("sda"));
        assert_eq!(f.bios_write(&["--plan".into()], "sda").unwrap(), "saved sector 0");
        f.set_boot_files(vec![BootFile { path: "boot/bzImage".into(), size: 3, sha256: "ab".into() }]);
        let files = f.extract_boot_pack(Path::new("a.img"), Path::new("out")).unwrap();
        assert_eq!(files[0].path, "boot/bzImage");
        assert_eq!(f.calls(), vec!["bios_plan()", "locate(sda, 1)", "bios_plan(--plan)", "bios_write(--plan, sda)", "extract_boot_pack(a.img, out)"]);
    }
}
