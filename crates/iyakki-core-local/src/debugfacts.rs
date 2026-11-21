//! What the developer-mode debug view reads from the kernel (F-ARCH-07, moved from the agent's `devmode/debug.rs`): the raw kernel log
//! records, the block devices, the mount lines of real file systems, the graphics cards with their connected outputs, the sound cards, the
//! kernel's identity (release, version, command line, taint flags, module count), the board identity and the UEFI flag. The agent turns
//! these into the JSON the developer tools show and hides credential-looking text; the files are read here, from roots given as arguments
//! so a test uses a made-up tree.

use std::path::Path;

fn read(p: &Path) -> String {
    std::fs::read_to_string(p).unwrap_or_default()
}

fn one(p: &Path) -> String {
    read(p).trim().to_string()
}

/// The raw records of the kernel log. A character device (`/dev/kmsg`) is read without blocking, one record per read, until it is caught
/// up (a record overwritten under the reader is skipped); a plain file in the same format (a saved log, a test) is read line by line with
/// continuation lines (leading space) dropped. Nothing there: no records.
pub fn kmsg_records(path: &Path) -> Vec<String> {
    use std::io::Read;
    use std::os::unix::fs::{FileTypeExt, OpenOptionsExt};
    let mut raw: Vec<String> = Vec::new();
    let Ok(meta) = std::fs::metadata(path) else { return raw };
    if meta.file_type().is_char_device() {
        if let Ok(mut f) = std::fs::OpenOptions::new().read(true).custom_flags(libc::O_NONBLOCK).open(path) {
            let mut buf = vec![0u8; 16 * 1024];
            // one record per read; EAGAIN when caught up; EPIPE when records were overwritten under us (carry on)
            for _ in 0..200_000 {
                match f.read(&mut buf) {
                    Ok(0) => break,
                    Ok(n) => raw.push(String::from_utf8_lossy(&buf[..n]).into_owned()),
                    Err(e) if e.raw_os_error() == Some(libc::EPIPE) => continue,
                    Err(_) => break,
                }
            }
        }
    } else {
        for l in read(path).lines() {
            if l.starts_with(' ') {
                continue;
            }
            raw.push(l.to_string());
        }
    }
    raw
}

/// One entry of `<sys>/class/dmi/id/<file>`, trimmed (empty when the machine has none).
pub fn dmi_in(sys: &Path, file: &str) -> String {
    one(&sys.join("class/dmi/id").join(file))
}

/// A block device as the debug view lists it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BlockDevice {
    pub name: String,
    pub bytes: u64,
    pub model: String,
    pub removable: bool,
}

/// The real block devices under `<sys>/block` (loop, ram and zram devices left out), sorted by name.
pub fn block_devices_in(sys: &Path) -> Vec<BlockDevice> {
    let mut names: Vec<String> = std::fs::read_dir(sys.join("block")).into_iter().flatten().flatten().map(|e| e.file_name().to_string_lossy().into_owned()).collect();
    names.sort();
    names
        .into_iter()
        .filter(|n| !n.starts_with("loop") && !n.starts_with("ram") && !n.starts_with("zram"))
        .map(|n| {
            let d = sys.join("block").join(&n);
            let sectors: u64 = one(&d.join("size")).parse().unwrap_or(0);
            BlockDevice { bytes: sectors * 512, model: one(&d.join("device/model")), removable: one(&d.join("removable")) == "1", name: n }
        })
        .collect()
}

/// A mounted file system of a real kind.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MountedFs {
    pub dev: String,
    pub dir: String,
    pub fs: String,
    pub read_only: bool,
}

/// The mounts in `<proc_>/mounts` whose file system is a real one (ext2/3/4, vfat, btrfs, xfs, f2fs, exfat, ntfs3, iso9660).
pub fn real_mounts_in(proc_: &Path) -> Vec<MountedFs> {
    const REAL: [&str; 10] = ["ext4", "ext2", "ext3", "vfat", "btrfs", "xfs", "f2fs", "exfat", "ntfs3", "iso9660"];
    read(&proc_.join("mounts"))
        .lines()
        .filter_map(|l| {
            let f: Vec<&str> = l.split_whitespace().collect();
            (f.len() >= 4 && REAL.contains(&f[2])).then(|| MountedFs { dev: f[0].into(), dir: f[1].into(), fs: f[2].into(), read_only: f[3].split(',').any(|o| o == "ro") })
        })
        .collect()
}

/// A graphics card with its driver, PCI ids and the outputs that have a screen connected.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GpuCard {
    pub card: String,
    pub driver: String,
    pub vendor: String,
    pub device: String,
    pub connected: Vec<String>,
}

/// The cards under `<sys>/class/drm` (`cardN`, not the output entries), sorted by name.
pub fn gpu_cards_in(sys: &Path) -> Vec<GpuCard> {
    let drm = sys.join("class/drm");
    let names_in = |keep: &dyn Fn(&str) -> bool| -> Vec<String> { std::fs::read_dir(&drm).into_iter().flatten().flatten().map(|e| e.file_name().to_string_lossy().into_owned()).filter(|n| keep(n)).collect() };
    let mut names = names_in(&|n| n.starts_with("card") && !n.contains('-'));
    names.sort();
    names
        .into_iter()
        .map(|n| {
            let d = drm.join(&n).join("device");
            let driver = std::fs::read_link(d.join("driver")).ok().and_then(|l| l.file_name().map(|s| s.to_string_lossy().into_owned())).unwrap_or_default();
            let prefix = format!("{n}-");
            let connected = names_in(&|c| c.starts_with(&prefix)).into_iter().filter(|c| one(&drm.join(c).join("status")) == "connected").collect();
            GpuCard { driver, vendor: one(&d.join("vendor")), device: one(&d.join("device")), connected, card: n }
        })
        .collect()
}

/// The sound card lines of `<proc_>/asound/cards` (the ones with a `[name]`), trimmed.
pub fn sound_card_lines_in(proc_: &Path) -> Vec<String> {
    read(&proc_.join("asound/cards")).lines().filter(|l| l.contains(" [")).map(|l| l.trim().to_string()).collect()
}

/// The kernel's identity as the debug view shows it.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct KernelIdentity {
    pub release: String,
    /// The first line of `/proc/version`.
    pub version: String,
    /// The boot command line, not yet scrubbed.
    pub cmdline: String,
    /// The taint bit mask (`/proc/sys/kernel/tainted`).
    pub tainted: u64,
    /// The number of loaded modules (non-empty lines of `/proc/modules`).
    pub modules: usize,
}

/// [`KernelIdentity`] from `<proc_>`.
pub fn kernel_identity_in(proc_: &Path) -> KernelIdentity {
    KernelIdentity {
        release: one(&proc_.join("sys/kernel/osrelease")),
        version: read(&proc_.join("version")).lines().next().unwrap_or("").to_string(),
        cmdline: one(&proc_.join("cmdline")),
        tainted: one(&proc_.join("sys/kernel/tainted")).parse().unwrap_or(0),
        modules: read(&proc_.join("modules")).lines().filter(|l| !l.trim().is_empty()).count(),
    }
}

/// Whether the machine started through UEFI (`<sys>/firmware/efi` exists).
pub fn uefi_in(sys: &Path) -> bool {
    sys.join("firmware/efi").is_dir()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn tree(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("iyakki-debugfacts-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    fn put(d: &Path, rel: &str, text: &str) {
        let p = d.join(rel);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, text).unwrap();
    }

    #[test]
    fn a_saved_log_file_gives_its_records_without_continuation_lines_and_a_missing_one_none() {
        let d = tree("kmsg");
        put(&d, "kmsg", "6,1,100,-;first\n SUBSYSTEM=x\n4,2,200,-;second\n");
        assert_eq!(kmsg_records(&d.join("kmsg")), vec!["6,1,100,-;first".to_string(), "4,2,200,-;second".to_string()]);
        assert!(kmsg_records(&d.join("nothing")).is_empty());
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn block_devices_leave_out_loop_ram_and_zram_and_read_size_model_and_removable() {
        let d = tree("block");
        put(&d, "block/sda/size", "2048\n");
        put(&d, "block/sda/device/model", "Test SSD  \n");
        put(&d, "block/sda/removable", "0\n");
        put(&d, "block/sdb/size", "8\n");
        put(&d, "block/sdb/removable", "1\n");
        for n in ["loop0", "ram1", "zram0"] {
            put(&d, &format!("block/{n}/size"), "1\n");
        }
        assert_eq!(
            block_devices_in(&d),
            vec![
                BlockDevice { name: "sda".into(), bytes: 2048 * 512, model: "Test SSD".into(), removable: false },
                BlockDevice { name: "sdb".into(), bytes: 4096, model: String::new(), removable: true },
            ]
        );
        assert!(block_devices_in(&d.join("none")).is_empty());
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn only_real_file_systems_are_listed_with_their_read_only_flag() {
        let d = tree("mounts");
        put(&d, "mounts", "proc /proc proc rw 0 0\n/dev/sda2 /var/lib/iyakki ext4 rw,noatime 0 0\n/dev/sdb1 /media/x vfat ro,nosuid 0 0\nshort\n");
        assert_eq!(
            real_mounts_in(&d),
            vec![
                MountedFs { dev: "/dev/sda2".into(), dir: "/var/lib/iyakki".into(), fs: "ext4".into(), read_only: false },
                MountedFs { dev: "/dev/sdb1".into(), dir: "/media/x".into(), fs: "vfat".into(), read_only: true },
            ]
        );
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn a_card_lists_its_ids_and_only_the_outputs_with_a_screen() {
        let d = tree("gpu");
        put(&d, "class/drm/card0/device/vendor", "0x8086\n");
        put(&d, "class/drm/card0/device/device", "0x3e92\n");
        put(&d, "class/drm/card0-HDMI-A-1/status", "connected\n");
        put(&d, "class/drm/card0-DP-1/status", "disconnected\n");
        put(&d, "class/drm/card1/device/vendor", "0x10de\n");
        put(&d, "class/drm/version", "x");
        let g = gpu_cards_in(&d);
        assert_eq!(g.len(), 2);
        assert_eq!((g[0].card.as_str(), g[0].vendor.as_str(), g[0].device.as_str()), ("card0", "0x8086", "0x3e92"));
        assert_eq!(g[0].connected, vec!["card0-HDMI-A-1".to_string()]);
        assert_eq!(g[0].driver, "");
        assert!(g[1].connected.is_empty());
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn sound_cards_identity_dmi_and_uefi_come_from_the_files() {
        let d = tree("ident");
        put(&d, "asound/cards", " 0 [PCH            ]: HDA-Intel - HDA Intel PCH\n                      HDA Intel PCH at 0x1\n");
        assert_eq!(sound_card_lines_in(&d), vec!["0 [PCH            ]: HDA-Intel - HDA Intel PCH".to_string()]);
        put(&d, "sys/kernel/osrelease", "6.12.1\n");
        put(&d, "sys/kernel/tainted", "4096\n");
        put(&d, "version", "Linux version 6.12.1 (x)\nsecond\n");
        put(&d, "cmdline", "quiet splash\n");
        put(&d, "modules", "a 1 0 - Live 0x0\n\nb 2 0 - Live 0x0\n");
        assert_eq!(kernel_identity_in(&d), KernelIdentity { release: "6.12.1".into(), version: "Linux version 6.12.1 (x)".into(), cmdline: "quiet splash".into(), tainted: 4096, modules: 2 });
        assert_eq!(kernel_identity_in(&d.join("none")), KernelIdentity::default());
        put(&d, "class/dmi/id/product_name", "Test Box\n");
        assert_eq!(dmi_in(&d, "product_name"), "Test Box");
        assert_eq!(dmi_in(&d, "board_name"), "");
        assert!(!uefi_in(&d));
        std::fs::create_dir_all(d.join("firmware/efi")).unwrap();
        assert!(uefi_in(&d));
        let _ = std::fs::remove_dir_all(&d);
    }
}
