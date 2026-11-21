//! Early mount of the side image (F-BOOT-25, plan in docs/MAC-HARDWARE.md "a side image read by Linux's USB driver").
//!
//! The idea: GRUB reads the kernel and a SMALL initial RAM disk through the firmware's slow USB driver (about 6 MB/s on a Mac); the big
//! parts (`/lib/firmware`, `/lib/modules`) travel on the FAT boot partition as one squashfs file, `side-<kernel release>.sqfs`, which
//! Linux's own xHCI/UAS driver reads at tens of MB/s. This module is the device side: find the boot volume (a FAT volume labelled
//! `IYAKKI-BOOT`), check that the file for THIS kernel is there, loop-mount it read-only and lay it under `/lib/firmware` and
//! `/lib/modules` with overlayfs (the small initramfs's own files stay visible underneath), and check that `modules.dep` is there.
//!
//! Safe by construction: it runs only when the kernel command line says `iyakki.side=1` (the boot menu sets that for the install image
//! that carries the file), every step is behind the [`Host`] trait (tested with a fake), and ANY failure undoes what was mounted and
//! returns [`Outcome::NotUsed`] with the reason: boot then goes on with the initial RAM disk as it is. Nothing is written anywhere.

use std::path::{Path, PathBuf};

/// The label of the FAT boot volume.
pub const LABEL: &str = "IYAKKI-BOOT";
/// Where the pieces are mounted.
pub const BOOT_MOUNT: &str = "/run/iyakki-boot";
pub const SIDE_MOUNT: &str = "/run/iyakki-side";
/// The folders the side image lays under the root.
pub const FOLDERS: [&str; 2] = ["lib/firmware", "lib/modules"];

/// The command line asks for it.
pub fn requested(cmdline: &str) -> bool {
    cmdline.split_whitespace().any(|w| w == "iyakki.side=1")
}

/// The file name for a kernel release: `side-<release>.sqfs`. The release is part of the name so a kernel updated without its side
/// image never reads modules built for another one. Only plain release characters are accepted (it names a file on a FAT volume).
pub fn file_name(release: &str) -> Option<String> {
    let r = release.trim();
    (!r.is_empty() && r.len() <= 64 && r.bytes().all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-' | b'+'))).then(|| format!("side-{r}.sqfs"))
}

/// The volume label of a FAT boot sector (FAT32 at byte 71, FAT12/16 at byte 43), trailing spaces cut.
pub fn fat_label(sector: &[u8]) -> Option<String> {
    let at = |o: usize| sector.get(o..o + 11).map(|b| String::from_utf8_lossy(b).trim_end().to_string());
    if sector.len() < 512 || sector[510] != 0x55 || sector[511] != 0xAA {
        return None;
    }
    // FAT32 has "FAT32   " at byte 82, FAT12/16 has "FAT" at byte 54
    if sector.get(82..85) == Some(b"FAT") {
        at(71)
    } else if sector.get(54..57) == Some(b"FAT") {
        at(43)
    } else {
        None
    }
}

/// What the early mount needs from the machine (a fake in tests).
pub trait Host {
    fn kernel_release(&self) -> String;
    /// The device node of a FAT volume with this label, if one is there now.
    fn find_labelled(&self, label: &str) -> Option<PathBuf>;
    /// Give a slow USB disk time to show up (one step of the wait).
    fn wait(&self);
    fn exists(&self, p: &Path) -> bool;
    fn mount(&self, src: &str, target: &Path, fstype: &str, options: &str) -> Result<(), String>;
    fn unmount(&self, target: &Path);
    fn loop_attach(&self, file: &Path) -> Result<String, String>;
    fn loop_detach(&self, dev: &str);
}

#[derive(Debug, PartialEq, Eq)]
pub enum Outcome {
    /// Nothing was changed; the reason is one line for the boot log.
    NotUsed(String),
    /// The side image lies under these folders.
    Used { file: String, folders: Vec<String> },
}

/// How many times the wait for the boot volume is repeated (each step is the host's `wait`).
pub const WAIT_STEPS: u32 = 30;

/// Mount the side image over the initial RAM disk's folders. Never leaves a half-done state.
pub fn attach(host: &dyn Host) -> Outcome {
    let release = host.kernel_release();
    let Some(name) = file_name(&release) else { return Outcome::NotUsed(format!("the kernel release {release:?} cannot name a side image")) };
    let mut dev = host.find_labelled(LABEL);
    let mut n = 0;
    while dev.is_none() && n < WAIT_STEPS {
        host.wait();
        dev = host.find_labelled(LABEL);
        n += 1;
    }
    let Some(dev) = dev else { return Outcome::NotUsed(format!("no volume labelled {LABEL} appeared")) };
    let (boot, side) = (Path::new(BOOT_MOUNT), Path::new(SIDE_MOUNT));
    if let Err(e) = host.mount(&dev.display().to_string(), boot, "vfat", "ro") {
        return Outcome::NotUsed(format!("the boot volume could not be mounted: {e}"));
    }
    let file = boot.join("boot").join(&name);
    if !host.exists(&file) {
        host.unmount(boot);
        return Outcome::NotUsed(format!("no {name} on the boot volume (a stick made without it, or another kernel)"));
    }
    let lo = match host.loop_attach(&file) {
        Ok(l) => l,
        Err(e) => {
            host.unmount(boot);
            return Outcome::NotUsed(format!("{name} could not be attached: {e}"));
        }
    };
    let mut mounted: Vec<PathBuf> = Vec::new();
    let undo = |mounted: &[PathBuf]| {
        for m in mounted.iter().rev() {
            host.unmount(m);
        }
        host.unmount(side);
        host.loop_detach(&lo);
        host.unmount(boot);
    };
    if let Err(e) = host.mount(&lo, side, "squashfs", "ro") {
        undo(&mounted);
        return Outcome::NotUsed(format!("{name} is not a readable squashfs: {e}"));
    }
    let mut folders = Vec::new();
    for f in FOLDERS {
        let lower = side.join(f);
        if !host.exists(&lower) {
            undo(&mounted);
            return Outcome::NotUsed(format!("{name} has no {f}"));
        }
        let target = Path::new("/").join(f);
        // the side image is the lower layer, the small initial RAM disk's own folder stays visible on top of it
        let opts = format!("lowerdir={}:{}", target.display(), lower.display());
        if let Err(e) = host.mount("overlay", &target, "overlay", &opts) {
            undo(&mounted);
            return Outcome::NotUsed(format!("{f} could not be layered: {e}"));
        }
        mounted.push(target);
        folders.push(f.to_string());
    }
    if !host.exists(Path::new("/lib/modules").join(&release).join("modules.dep").as_path()) {
        undo(&mounted);
        return Outcome::NotUsed(format!("after layering there is no modules.dep for {release}"));
    }
    Outcome::Used { file: name, folders }
}

/// The real machine (Linux): sysfs for the block devices, the FAT boot sector for the label, `mount(2)`, the loop device of `iyakki-storage`.
#[cfg(target_os = "linux")]
pub struct RealHost {
    /// The kernel release, read by the boot code (`/proc/sys/kernel/osrelease` has one reader).
    pub release: String,
}

#[cfg(target_os = "linux")]
impl Host for RealHost {
    fn kernel_release(&self) -> String {
        self.release.trim().to_string()
    }
    fn find_labelled(&self, label: &str) -> Option<PathBuf> {
        use std::io::Read;
        let roots = iyakki_storage::Roots::real();
        for d in iyakki_storage::list(&roots) {
            let mut sector = [0u8; 512];
            if std::fs::File::open(&d.path).and_then(|mut f| f.read_exact(&mut sector)).is_ok() && fat_label(&sector).as_deref() == Some(label) {
                return Some(d.path);
            }
        }
        None
    }
    fn wait(&self) {
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
    fn exists(&self, p: &Path) -> bool {
        p.exists()
    }
    fn mount(&self, src: &str, target: &Path, fstype: &str, options: &str) -> Result<(), String> {
        let ro = if options.split(',').any(|o| o == "ro") { libc::MS_RDONLY } else { 0 };
        let data = options.split(',').filter(|o| *o != "ro").collect::<Vec<_>>().join(",");
        crate::boot::mount_raw(src, &target.display().to_string(), fstype, ro, &data)
    }
    fn unmount(&self, target: &Path) {
        use std::os::unix::ffi::OsStrExt;
        if let Ok(t) = std::ffi::CString::new(target.as_os_str().as_bytes()) {
            // SAFETY: a valid NUL-terminated path.
            unsafe { libc::umount(t.as_ptr()) };
        }
    }
    fn loop_attach(&self, file: &Path) -> Result<String, String> {
        use iyakki_storage::dm::DmKernel;
        iyakki_storage::dm::SysKernel.loop_attach(file).map_err(|e| e.to_string())
    }
    fn loop_detach(&self, dev: &str) {
        use iyakki_storage::dm::DmKernel;
        let _ = iyakki_storage::dm::SysKernel.loop_detach(dev);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;
    use std::collections::HashSet;

    struct Fake {
        release: String,
        label_after: u32,
        calls: RefCell<u32>,
        files: HashSet<String>,
        log: RefCell<Vec<String>>,
        fail_mount: Option<&'static str>,
    }

    fn fake(files: &[&str]) -> Fake {
        Fake { release: "6.12.1-iyakki".into(), label_after: 0, calls: RefCell::new(0), files: files.iter().map(|s| s.to_string()).collect(), log: RefCell::new(vec![]), fail_mount: None }
    }

    const ALL: [&str; 5] = ["/run/iyakki-boot/boot/side-6.12.1-iyakki.sqfs", "/run/iyakki-side/lib/firmware", "/run/iyakki-side/lib/modules", "/lib/modules/6.12.1-iyakki/modules.dep", "/x"];

    impl Host for Fake {
        fn kernel_release(&self) -> String {
            self.release.clone()
        }
        fn find_labelled(&self, _: &str) -> Option<PathBuf> {
            let mut c = self.calls.borrow_mut();
            *c += 1;
            (*c > self.label_after).then(|| PathBuf::from("/dev/sdb1"))
        }
        fn wait(&self) {
            self.log.borrow_mut().push("wait".into());
        }
        fn exists(&self, p: &Path) -> bool {
            self.files.contains(&p.display().to_string())
        }
        fn mount(&self, src: &str, target: &Path, fstype: &str, options: &str) -> Result<(), String> {
            if self.fail_mount == Some(fstype_static(fstype)) {
                return Err("refused".into());
            }
            self.log.borrow_mut().push(format!("mount {fstype} {src} {} {options}", target.display()));
            Ok(())
        }
        fn unmount(&self, t: &Path) {
            self.log.borrow_mut().push(format!("umount {}", t.display()));
        }
        fn loop_attach(&self, f: &Path) -> Result<String, String> {
            self.log.borrow_mut().push(format!("loop {}", f.display()));
            Ok("/dev/loop0".into())
        }
        fn loop_detach(&self, d: &str) {
            self.log.borrow_mut().push(format!("unloop {d}"));
        }
    }
    fn fstype_static(s: &str) -> &'static str {
        match s {
            "vfat" => "vfat",
            "squashfs" => "squashfs",
            _ => "overlay",
        }
    }

    #[test]
    fn it_is_asked_for_by_the_command_line_and_the_file_is_named_by_the_kernel_release() {
        assert!(requested("quiet iyakki.side=1 console=tty0") && !requested("iyakki.side=0 quiet") && !requested("iyakki.side=11"));
        assert_eq!(file_name("6.12.1-iyakki+rc1").as_deref(), Some("side-6.12.1-iyakki+rc1.sqfs"));
        assert_eq!(file_name("../etc"), None);
        assert_eq!(file_name(""), None);
        assert_eq!(file_name(&"a".repeat(65)), None);
    }

    #[test]
    fn the_label_is_read_from_a_fat32_or_a_fat16_boot_sector() {
        let mut s = vec![0u8; 512];
        s[510] = 0x55;
        s[511] = 0xAA;
        s[82..90].copy_from_slice(b"FAT32   ");
        s[71..82].copy_from_slice(b"IYAKKI-BOOT");
        assert_eq!(fat_label(&s).as_deref(), Some("IYAKKI-BOOT"));
        let mut t = vec![0u8; 512];
        t[510] = 0x55;
        t[511] = 0xAA;
        t[54..62].copy_from_slice(b"FAT16   ");
        t[43..54].copy_from_slice(b"SMALL      ");
        assert_eq!(fat_label(&t).as_deref(), Some("SMALL"));
        assert_eq!(fat_label(&[0u8; 512]), None);
        assert_eq!(fat_label(&[0u8; 10]), None);
    }

    #[test]
    fn a_present_side_image_is_loop_mounted_and_layered_under_the_firmware_and_modules_folders() {
        let mut h = fake(&ALL);
        h.label_after = 3; // the USB disk shows up after three tries
        let out = attach(&h);
        assert_eq!(out, Outcome::Used { file: "side-6.12.1-iyakki.sqfs".into(), folders: vec!["lib/firmware".into(), "lib/modules".into()] });
        let l = h.log.borrow().join("\n");
        assert_eq!(l.matches("wait").count(), 3);
        let order = ["mount vfat /dev/sdb1 /run/iyakki-boot ro", "loop /run/iyakki-boot/boot/side-6.12.1-iyakki.sqfs", "mount squashfs /dev/loop0 /run/iyakki-side ro", "mount overlay overlay /lib/firmware lowerdir=/lib/firmware:/run/iyakki-side/lib/firmware", "mount overlay overlay /lib/modules lowerdir=/lib/modules:/run/iyakki-side/lib/modules"];
        let mut at = 0;
        for o in order {
            let i = l[at..].find(o).unwrap_or_else(|| panic!("{o} is missing or out of order in\n{l}"));
            at += i + o.len();
        }
        assert!(!l.contains("umount"), "nothing is undone on success: {l}");
    }

    #[test]
    fn any_failure_undoes_everything_and_says_why_so_boot_goes_on_with_the_initramfs() {
        // the volume never appears
        let mut h = fake(&ALL);
        h.label_after = 1000;
        assert!(matches!(attach(&h), Outcome::NotUsed(r) if r.contains("no volume labelled")));
        assert_eq!(h.log.borrow().iter().filter(|l| *l == "wait").count(), WAIT_STEPS as usize);
        // a stick made without the file
        let h = fake(&["/lib/modules/6.12.1-iyakki/modules.dep"]);
        assert!(matches!(attach(&h), Outcome::NotUsed(r) if r.contains("side-6.12.1-iyakki.sqfs")));
        assert_eq!(h.log.borrow().last().map(String::as_str), Some("umount /run/iyakki-boot"));
        // not a squashfs
        let mut h = fake(&ALL);
        h.fail_mount = Some("squashfs");
        assert!(matches!(attach(&h), Outcome::NotUsed(r) if r.contains("not a readable squashfs")));
        let l = h.log.borrow().join("|");
        assert!(l.contains("unloop /dev/loop0") && l.ends_with("umount /run/iyakki-boot"), "{l}");
        // the overlay of the modules folder fails: the first one is taken off again
        let mut h = fake(&ALL);
        h.fail_mount = Some("overlay");
        assert!(matches!(attach(&h), Outcome::NotUsed(r) if r.contains("could not be layered")));
        // layered, but no modules.dep for this kernel
        let h = fake(&ALL[..3]);
        let o = attach(&h);
        assert!(matches!(&o, Outcome::NotUsed(r) if r.contains("modules.dep")), "{o:?}");
        let l = h.log.borrow().join("|");
        assert!(l.contains("umount /lib/modules|umount /lib/firmware"), "the layers come off in reverse order: {l}");
        // a release that cannot name a file
        let mut h = fake(&ALL);
        h.release = "bad/release".into();
        assert!(matches!(attach(&h), Outcome::NotUsed(_)) && h.log.borrow().is_empty());
    }
}
