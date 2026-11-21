//! Which machine kind a boot volume is for, read from the files it holds (F-INST-28). The x86-64 image keeps `/boot/bzImage` and
//! `/boot/initrd` (GRUB reads them); the aarch64 image's boot volume (`board/iyakki-aarch64/genimage.cfg`) keeps the kernel `Image`
//! (an EFI stub, it starts without GRUB) and `rootfs.cpio.gz` at its root, with the loader `EFI/BOOT/BOOTAA64.EFI`. The planner and the
//! writer ask this module for the paths instead of naming the x86 ones, so one install code serves both images.

/// The kind of machine an image boots on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Arch {
    X86_64,
    Aarch64,
}

impl Arch {
    pub fn name(self) -> &'static str {
        match self {
            Arch::X86_64 => "x86-64",
            Arch::Aarch64 => "aarch64",
        }
    }
}

/// The files the installer copies (the kernel and the RAM disk) and the firmware loader of a boot volume.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BootFiles {
    pub arch: Arch,
    pub kernel: &'static str,
    pub initrd: &'static str,
    pub loader: &'static str,
    /// The boot menu file whose first `linux` line gives the kernel arguments for an EFI-stub boot.
    pub grub_cfg: &'static str,
}

const X86: BootFiles = BootFiles { arch: Arch::X86_64, kernel: "/boot/bzImage", initrd: "/boot/initrd", loader: "/EFI/BOOT/BOOTX64.EFI", grub_cfg: "/boot/grub/grub.cfg" };
const ARM: BootFiles = BootFiles { arch: Arch::Aarch64, kernel: "/Image", initrd: "/rootfs.cpio.gz", loader: "/EFI/BOOT/BOOTAA64.EFI", grub_cfg: "/EFI/BOOT/grub.cfg" };

/// Decide from the paths of the files on the boot volume (case does not matter, the volume is FAT). Both layouts present, or neither,
/// is an error: the installer does not guess which one to copy.
pub fn boot_files<'a>(paths: impl IntoIterator<Item = &'a str>) -> Result<BootFiles, String> {
    let have: Vec<String> = paths.into_iter().map(str::to_ascii_lowercase).collect();
    let has = |p: &str| have.iter().any(|h| h == &p.to_ascii_lowercase());
    let x86 = has(X86.kernel) && has(X86.initrd);
    let arm = has(ARM.kernel) && has(ARM.initrd);
    match (x86, arm) {
        (true, false) => Ok(X86),
        (false, true) => Ok(ARM),
        (true, true) => Err("the image's boot volume holds both an x86-64 and an aarch64 kernel; it is not clear which one to install".into()),
        (false, false) => Err("the image's boot volume has no kernel and RAM disk this installer knows (/boot/bzImage and /boot/initrd, or /Image and /rootfs.cpio.gz)".into()),
    }
}

/// The files of the boot volume at `src_path`'s first partition (read from the image or the stick it is).
pub fn of_boot_volume(src_path: &std::path::Path, p1_start_sector: u64) -> Result<BootFiles, String> {
    let mut f = std::fs::File::open(src_path).map_err(|e| format!("{}: {e}", src_path.display()))?;
    let mut vol = crate::fatclone::Fat32::open(&mut f, p1_start_sector * 512)?;
    let items = vol.walk_long()?;
    boot_files(items.iter().filter(|i| !i.dir).map(|i| i.path.as_str()))
}

/// True for the paths whose bytes count as the image's payload when the space it needs is worked out.
pub fn is_payload(path: &str) -> bool {
    let p = path.to_ascii_lowercase();
    [X86.kernel, X86.initrd, ARM.kernel, ARM.initrd].iter().any(|k| k.to_ascii_lowercase() == p)
}

/// The machine kind the running installer itself is built for (what a stick it writes can be started on without an emulator).
pub fn host_arch() -> Arch {
    if cfg!(target_arch = "aarch64") {
        Arch::Aarch64
    } else {
        Arch::X86_64
    }
}

/// A calm sentence when the image is for another machine kind than the disk's computer is known to be, `None` when it is fine or unknown.
/// `target` is `None` when the installer cannot tell (it writes a stick for another computer).
pub fn mismatch_note(image: Arch, target: Option<Arch>) -> Option<String> {
    let t = target?;
    (t != image).then(|| format!("This image is for {} computers but this one is {}: the installed disk would not start here.", image.name(), t.name()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_x86_image_is_known_by_its_grub_files_and_the_aarch64_one_by_its_stub_files() {
        assert_eq!(boot_files(["/EFI/BOOT/BOOTX64.EFI", "/BOOT/BZIMAGE", "/boot/INITRD"]).unwrap().arch, Arch::X86_64);
        let b = boot_files(["/EFI", "/Image", "/ROOTFS.CPIO.GZ", "/EFI/BOOT/BOOTAA64.EFI"]).unwrap();
        assert_eq!((b.arch, b.kernel, b.initrd, b.loader), (Arch::Aarch64, "/Image", "/rootfs.cpio.gz", "/EFI/BOOT/BOOTAA64.EFI"));
    }

    #[test]
    fn both_layouts_or_none_are_refused_with_a_reason_and_half_a_layout_is_none() {
        assert!(boot_files(["/boot/bzImage", "/boot/initrd", "/Image", "/rootfs.cpio.gz"]).unwrap_err().contains("both"));
        assert!(boot_files(["/boot/bzImage"]).unwrap_err().contains("no kernel"));
        assert!(boot_files(["/Image"]).is_err());
        assert!(boot_files(Vec::<&str>::new()).is_err());
    }

    #[test]
    fn each_image_names_its_loader_and_the_menu_file_the_kernel_arguments_come_from() {
        let x = boot_files(["/boot/bzImage", "/boot/initrd"]).unwrap();
        assert_eq!((x.loader, x.grub_cfg), ("/EFI/BOOT/BOOTX64.EFI", "/boot/grub/grub.cfg"));
        let a = boot_files(["/Image", "/rootfs.cpio.gz"]).unwrap();
        assert_eq!((a.loader, a.grub_cfg), ("/EFI/BOOT/BOOTAA64.EFI", "/EFI/BOOT/grub.cfg"));
    }

    #[test]
    fn the_payload_counts_the_kernel_and_ram_disk_of_either_image_and_nothing_else() {
        for p in ["/BOOT/BZIMAGE", "/boot/initrd", "/IMAGE", "/rootfs.cpio.gz"] {
            assert!(is_payload(p), "{p}");
        }
        for p in ["/boot/grub/grub.cfg", "/EFI/BOOT/BOOTAA64.EFI", "/imagex"] {
            assert!(!is_payload(p), "{p}");
        }
    }

    #[test]
    fn an_image_for_another_machine_kind_is_named_and_an_unknown_target_is_not_blamed() {
        assert!(mismatch_note(Arch::Aarch64, Some(Arch::X86_64)).unwrap().contains("aarch64 computers but this one is x86-64"));
        assert_eq!(mismatch_note(Arch::Aarch64, Some(Arch::Aarch64)), None);
        assert_eq!(mismatch_note(Arch::X86_64, None), None);
    }
}
