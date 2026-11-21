//! A few plain facts about the running kernel and firmware that the About screen and the web engine show (F-ARCH-07): the release string,
//! whether the Apple bridge module is loaded, the Secure Boot variable and the lockdown line, the taint number and the graphics cards'
//! vendor and device numbers. This is the one reader of those files; every function has a `_in` form that takes the roots so a test
//! uses a made-up tree.

use std::path::{Path, PathBuf};

/// Where the files are (the machine's `/proc`, `/sys`).
#[derive(Clone, Debug)]
pub struct Roots {
    pub proc_: PathBuf,
    pub sys: PathBuf,
}

impl Roots {
    pub fn system() -> Roots {
        Roots { proc_: PathBuf::from("/proc"), sys: PathBuf::from("/sys") }
    }
}

/// The EFI variable that holds the Secure Boot state.
const SECURE_BOOT_VAR: &str = "firmware/efi/efivars/SecureBoot-8be4df61-93ca-11d2-aa0d-00e098032b8c";

/// What the kernel and firmware say, each `None` when the file is not there.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Facts {
    /// `/proc/sys/kernel/osrelease`, as the file holds it.
    pub osrelease: Option<String>,
    /// The `apple_bce` module is loaded.
    pub apple_bce: bool,
    /// The raw Secure Boot variable (four attribute bytes, then the value) and the lockdown line, when the variable exists.
    pub secure_boot: Option<(Vec<u8>, Option<String>)>,
    /// The kernel's taint number.
    pub taint: Option<u64>,
}

pub fn read_in(r: &Roots) -> Facts {
    Facts {
        osrelease: std::fs::read_to_string(r.proc_.join("sys/kernel/osrelease")).ok(),
        apple_bce: r.sys.join("module/apple_bce").exists(),
        secure_boot: std::fs::read(r.sys.join(SECURE_BOOT_VAR)).ok().map(|v| (v, std::fs::read_to_string(r.sys.join("kernel/security/lockdown")).ok())),
        taint: std::fs::read_to_string(r.proc_.join("sys/kernel/tainted")).ok().and_then(|t| t.trim().parse::<u64>().ok()),
    }
}

/// [`read_in`] on this machine.
pub fn read() -> Facts {
    read_in(&Roots::system())
}

/// The kernel's id of this boot (`/proc/sys/kernel/random/boot_id` under `proc_`), trimmed; empty when it cannot be read.
pub fn boot_id_in(proc_: &Path) -> String {
    std::fs::read_to_string(proc_.join("sys/kernel/random/boot_id")).map(|s| s.trim().to_string()).unwrap_or_default()
}

/// [`boot_id_in`] on this machine.
pub fn boot_id() -> String {
    boot_id_in(Path::new("/proc"))
}

/// The text of the kernel's boot command line (`<proc_>/cmdline`), empty when it cannot be read.
pub fn cmdline_in(proc_: &Path) -> String {
    iyakki_storage::cmdline_in(proc_)
}

/// [`cmdline_in`] on this machine: the one place the agent reads the boot command line.
pub fn cmdline() -> String {
    iyakki_storage::cmdline_text()
}

/// The text of the kernel's mount list on this machine, empty when it cannot be read.
pub fn mounts_text() -> String {
    iyakki_storage::mounts_text()
}

/// Is the data folder `data` on memory (a volatile live session) rather than on a disk? `roots` are the storage roots (`/proc`, `/sys`, `/dev`
/// of the machine, or a made-up tree).
pub fn data_in_memory_in(roots: &iyakki_storage::Roots, data: &Path) -> bool {
    iyakki_storage::data_location(roots, data) == iyakki_storage::DataLocation::Memory
}

/// [`data_in_memory_in`] on this machine.
pub fn data_in_memory(data: &Path) -> bool {
    data_in_memory_in(&iyakki_storage::Roots::real(), data)
}

/// The storage roots of this machine (for the startup-disk code, which takes them as an argument so a test can pass a made-up tree).
pub fn storage_roots() -> iyakki_storage::Roots {
    iyakki_storage::Roots::real()
}

/// Ask the kernel to drop its page, dentry and inode caches (`3`), after the caller has synced the disks. The device patch trial uses it
/// to test a patched program from cold caches.
pub fn drop_caches() -> Result<(), String> {
    drop_caches_in(Path::new("/proc"))
}

fn drop_caches_in(proc_: &Path) -> Result<(), String> {
    std::fs::write(proc_.join("sys/vm/drop_caches"), "3").map_err(|e| format!("drop caches: {e}"))
}

/// Does the kernel run the unified (v2) control-group hierarchy? True when `<sys>/fs/cgroup/cgroup.controllers` exists. The container cluster
/// refuses to start without it.
pub fn cgroup_v2_in(sys: &Path) -> bool {
    sys.join("fs/cgroup/cgroup.controllers").exists()
}

/// [`cgroup_v2_in`] on this machine.
pub fn cgroup_v2() -> bool {
    cgroup_v2_in(Path::new("/sys"))
}

/// How deep under `devices` the scan for `modalias` files looks (a device behind a bridge chain is about ten levels down).
const MODALIAS_DEPTH: u32 = 16;

/// The `modalias` strings of every device under `<sys>/devices` (symbolic links are not followed), sorted and without duplicates: each
/// names the driver module a device wants. The optional driver pack asks which of its modules this computer's devices call for.
pub fn modaliases_in(sys: &Path) -> Vec<String> {
    fn walk(dir: &Path, out: &mut Vec<String>, depth: u32) {
        if depth > MODALIAS_DEPTH {
            return;
        }
        let Ok(rd) = std::fs::read_dir(dir) else { return };
        for e in rd.flatten() {
            let Ok(ft) = e.file_type() else { continue };
            if ft.is_symlink() {
                continue;
            }
            if ft.is_dir() {
                walk(&e.path(), out, depth + 1);
            } else if e.file_name() == "modalias" {
                if let Ok(s) = std::fs::read_to_string(e.path()) {
                    if !s.trim().is_empty() {
                        out.push(s.trim().to_string());
                    }
                }
            }
        }
    }
    let mut v = Vec::new();
    walk(&sys.join("devices"), &mut v, 0);
    v.sort();
    v.dedup();
    v
}

/// `(vendor, device)` file contents of the graphics cards (`class/drm/card0..3/device/`), as the files hold them (`0x1002`). A card with
/// no `vendor` file is left out; a missing `device` file is an empty string.
pub fn drm_gpus_in(sys: &Path) -> Vec<(String, String)> {
    (0..4)
        .filter_map(|i| {
            let d = sys.join(format!("class/drm/card{i}/device"));
            let v = std::fs::read_to_string(d.join("vendor")).ok()?;
            Some((v, std::fs::read_to_string(d.join("device")).unwrap_or_default()))
        })
        .collect()
}

/// The kernel's process and device tree roots on this machine. The agent names these through here, never by literal (F-ARCH-07).
pub const PROC: &str = "/proc";
/// See [`PROC`]: the hardware tree.
pub const SYS: &str = "/sys";
/// See [`PROC`]: the device nodes.
pub const DEV: &str = "/dev";
/// The display connectors of every graphics card (`/sys/class/drm`): the shell's display loop and its screen-change check read them
/// under this name, never by literal.
pub const DRM_CLASS: &str = "/sys/class/drm";
/// The framebuffer devices' attributes (`/sys/class/graphics`): name, size, stride and depth of a screen.
pub const GRAPHICS_CLASS: &str = "/sys/class/graphics";

/// The kernel's device tree (`/sys/devices`): the boot's module-alias pass and the late-device settle read it under this name, never by literal.
pub const SYS_DEVICES: &str = "/sys/devices";
/// The USB devices as the kernel lists them (`/sys/bus/usb/devices`).
pub const USB_DEVICES: &str = "/sys/bus/usb/devices";
/// The kernel's debug file system mount point (`/sys/kernel/debug`), in the boot mount table.
pub const DEBUGFS: &str = "/sys/kernel/debug";
/// The first framebuffer device: the default screen the shell and the core's UI program are told about when nothing says otherwise.
pub const DEFAULT_FB: &str = "/dev/fb0";

/// The console device nodes the shell tries for its virtual-terminal work, in order.
pub const CONSOLE_NODES: [&str; 2] = ["/dev/tty0", "/dev/console"];

/// The text of `/proc/meminfo` on this machine, empty when it cannot be read.
pub fn meminfo_text() -> String {
    std::fs::read_to_string(Path::new(PROC).join("meminfo")).unwrap_or_default()
}

/// The kernel release (`/proc/sys/kernel/osrelease`) on this machine, empty when it cannot be read (not trimmed, as the file holds it).
pub fn osrelease_text() -> String {
    read().osrelease.unwrap_or_default()
}

/// The id of this boot with the dashes left out (the form the crash log and the self test use), empty when it cannot be read.
pub fn boot_id_compact() -> String {
    boot_id().replace('-', "")
}

/// Let programs write kernel messages without the rate limit (`printk_devkmsg` = on): the boot lines a program writes to the log all arrive.
pub fn allow_unlimited_kmsg() {
    let _ = std::fs::write(Path::new(PROC).join("sys/kernel/printk_devkmsg"), "on\n");
}

/// Name `helper` as the program the kernel runs for a hotplugged device (`/proc/sys/kernel/hotplug`).
pub fn set_hotplug_helper(helper: &str) {
    let _ = std::fs::write(Path::new(PROC).join("sys/kernel/hotplug"), format!("{helper}\n"));
}

/// Open the first virtual console (`/dev/tty0`) for reading and writing; `None` while the node is not there yet.
pub fn open_console() -> Option<std::fs::File> {
    std::fs::OpenOptions::new().read(true).write(true).open(Path::new(DEV).join("tty0")).ok()
}

/// Does the routing table (`<proc_>/net/route`) have a default route (a destination of `00000000`)?
pub fn has_default_route_in(proc_: &Path) -> bool {
    std::fs::read_to_string(proc_.join("net/route")).is_ok_and(|r| r.lines().skip(1).any(|l| l.split_whitespace().nth(1) == Some("00000000")))
}

/// [`has_default_route_in`] on this machine.
pub fn has_default_route() -> bool {
    has_default_route_in(Path::new(PROC))
}

/// A kernel setting under `<proc_>/sys/vm/` (`swappiness`, `page-cluster`), as the file holds it, or `missing: <reason>`: the self test prints both.
pub fn vm_setting_in(proc_: &Path, name: &str) -> String {
    text_or_missing(&proc_.join("sys/vm").join(name), name)
}

/// [`vm_setting_in`] on this machine.
pub fn vm_setting(name: &str) -> String {
    vm_setting_in(Path::new(PROC), name)
}

/// An attribute of the first compressed-memory block device (`<sys>/block/zram0/<name>`), as the file holds it, or `missing: <reason>`.
pub fn zram_attr_in(sys: &Path, name: &str) -> String {
    text_or_missing(&sys.join("block/zram0").join(name), name)
}

/// [`zram_attr_in`] on this machine.
pub fn zram_attr(name: &str) -> String {
    zram_attr_in(Path::new(SYS), name)
}

fn text_or_missing(p: &Path, name: &str) -> String {
    if name.is_empty() || name.contains('/') {
        return format!("missing: `{name}` is not a setting name");
    }
    std::fs::read_to_string(p).unwrap_or_else(|e| format!("missing: {e}"))
}

/// The kernel message buffer device under the given device root.
pub fn kmsg_in(dev: &Path) -> PathBuf {
    dev.join("kmsg")
}

/// The USB device controller list under the given hardware root (`class/udc`).
pub fn udc_in(sys: &Path) -> PathBuf {
    sys.join("class/udc")
}

/// The kernel's input device list (`bus/input/devices`), as the file holds it; empty when it is not there.
pub fn input_devices_in(proc_: &Path) -> String {
    std::fs::read_to_string(proc_.join("bus/input/devices")).unwrap_or_default()
}

/// [`input_devices_in`] on this machine.
pub fn input_devices() -> String {
    input_devices_in(Path::new(PROC))
}

/// The kernel message buffer opened for reading without blocking under the given device root; none when it cannot be opened.
pub fn open_kmsg_in(dev: &Path) -> Option<std::fs::File> {
    use std::os::unix::fs::OpenOptionsExt;
    std::fs::OpenOptions::new().read(true).custom_flags(libc::O_NONBLOCK).open(kmsg_in(dev)).ok()
}

/// [`open_kmsg_in`] on this machine.
pub fn open_kmsg() -> Option<std::fs::File> {
    open_kmsg_in(Path::new(DEV))
}

/// [`drm_gpus_in`] on this machine.
pub fn drm_gpus() -> Vec<(String, String)> {
    drm_gpus_in(Path::new("/sys"))
}

#[cfg(test)]
mod tests {
    #[test]
    fn the_input_list_the_message_buffer_and_the_controller_dir_follow_the_given_roots() {
        let d = std::env::temp_dir().join(format!("iyakki-kf-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(d.join("bus/input")).unwrap();
        std::fs::write(d.join("bus/input/devices"), "N: Name=\"Made Up Keyboard\"\n").unwrap();
        assert_eq!(super::input_devices_in(&d), "N: Name=\"Made Up Keyboard\"\n");
        assert_eq!(super::input_devices_in(&d.join("missing")), "");
        assert_eq!(super::kmsg_in(&d), d.join("kmsg"));
        assert_eq!(super::udc_in(&d), d.join("class/udc"));
        assert_eq!((super::PROC, super::SYS, super::DEV), ("/proc", "/sys", "/dev"));
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn the_message_buffer_opens_under_the_given_root_and_a_missing_one_gives_none() {
        let d = std::env::temp_dir().join(format!("iyakki-kmsg-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        assert!(super::open_kmsg_in(&d).is_none());
        std::fs::write(d.join("kmsg"), "6,1,1,-;hello\n").unwrap();
        let mut text = String::new();
        std::io::Read::read_to_string(&mut super::open_kmsg_in(&d).unwrap(), &mut text).unwrap();
        assert_eq!(text, "6,1,1,-;hello\n");
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn the_boot_id_is_trimmed_empty_when_missing_and_the_cache_drop_writes_a_three() {
        let d = std::env::temp_dir().join(format!("iyakki-bootid-{}", std::process::id()));
        std::fs::create_dir_all(d.join("sys/kernel/random")).unwrap();
        std::fs::create_dir_all(d.join("sys/vm")).unwrap();
        assert_eq!(super::boot_id_in(&d), "");
        std::fs::write(d.join("sys/kernel/random/boot_id"), "abc-123\n").unwrap();
        assert_eq!(super::boot_id_in(&d), "abc-123");
        super::drop_caches_in(&d).unwrap();
        assert_eq!(std::fs::read_to_string(d.join("sys/vm/drop_caches")).unwrap(), "3");
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn the_command_line_and_the_data_place_are_read_from_the_given_roots() {
        let d = std::env::temp_dir().join(format!("iyakki-kfacts-data-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        assert_eq!(super::cmdline_in(&d), "");
        std::fs::write(d.join("cmdline"), "quiet iyakki.x=1\n").unwrap();
        assert!(super::cmdline_in(&d).contains("iyakki.x=1"));
        let roots = iyakki_storage::Roots { sys_block: d.join("sys"), dev: d.join("dev"), mounts: d.join("mounts"), require_block: false };
        std::fs::write(d.join("mounts"), "tmpfs /var/lib/iyakki tmpfs rw 0 0\n").unwrap();
        assert!(super::data_in_memory_in(&roots, std::path::Path::new("/var/lib/iyakki")), "a tmpfs data folder is volatile");
        std::fs::write(d.join("mounts"), "/dev/sda2 /var/lib/iyakki ext4 rw 0 0\n").unwrap();
        assert!(!super::data_in_memory_in(&roots, std::path::Path::new("/var/lib/iyakki")), "a disk data folder is not");
        assert!(!super::data_in_memory_in(&roots, std::path::Path::new("/elsewhere")), "no mount found: not called volatile");
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn cgroup_v2_is_told_by_the_controllers_file() {
        let d = std::env::temp_dir().join(format!("iyakki-cgv2-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(d.join("fs/cgroup")).unwrap();
        assert!(!cgroup_v2_in(&d));
        std::fs::write(d.join("fs/cgroup/cgroup.controllers"), "cpu memory\n").unwrap();
        assert!(cgroup_v2_in(&d));
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn modaliases_are_collected_from_every_depth_sorted_once_and_links_are_not_followed() {
        let d = std::env::temp_dir().join(format!("iyakki-modalias-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        let a = d.join("devices/pci0000:00/0000:00:14.0/usb1/1-4");
        std::fs::create_dir_all(&a).unwrap();
        std::fs::write(d.join("devices/pci0000:00/0000:00:14.0/modalias"), "pci:v1\n").unwrap();
        std::fs::write(a.join("modalias"), "usb:v2\n").unwrap();
        std::fs::write(d.join("devices/pci0000:00/modalias"), "usb:v2\n").unwrap();
        std::fs::write(d.join("devices/modalias"), "\n").unwrap();
        #[cfg(unix)]
        std::os::unix::fs::symlink(&a, d.join("devices/loop")).unwrap();
        assert_eq!(modaliases_in(&d), ["pci:v1", "usb:v2"]);
        assert!(modaliases_in(&d.join("nothing")).is_empty());
        let _ = std::fs::remove_dir_all(&d);
    }

    use super::*;

    fn tree(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("iyakki-kfacts-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn the_display_and_framebuffer_class_folders_are_named_once_for_the_shell() {
        assert_eq!(DRM_CLASS, "/sys/class/drm");
        assert_eq!(GRAPHICS_CLASS, "/sys/class/graphics");
        assert!(DRM_CLASS.starts_with(SYS) && GRAPHICS_CLASS.starts_with(SYS));
    }

    #[test]
    fn the_facts_come_from_the_given_roots_and_a_missing_file_is_none() {
        let d = tree("facts");
        let (p, s) = (d.join("proc"), d.join("sys"));
        std::fs::create_dir_all(p.join("sys/kernel")).unwrap();
        std::fs::write(p.join("sys/kernel/osrelease"), "6.12.1\n").unwrap();
        std::fs::write(p.join("sys/kernel/tainted"), "12288\n").unwrap();
        std::fs::create_dir_all(s.join("module/apple_bce")).unwrap();
        let r = Roots { proc_: p, sys: s.clone() };
        let f = read_in(&r);
        assert_eq!(f.osrelease.as_deref(), Some("6.12.1\n"));
        assert!(f.apple_bce);
        assert_eq!(f.taint, Some(12288));
        assert_eq!(f.secure_boot, None, "no firmware variable, no row");
        std::fs::create_dir_all(s.join("firmware/efi/efivars")).unwrap();
        std::fs::write(s.join(SECURE_BOOT_VAR), [7u8, 0, 0, 0, 1]).unwrap();
        std::fs::create_dir_all(s.join("kernel/security")).unwrap();
        std::fs::write(s.join("kernel/security/lockdown"), "none [integrity] confidentiality\n").unwrap();
        let f = read_in(&r);
        assert_eq!(f.secure_boot, Some((vec![7, 0, 0, 0, 1], Some("none [integrity] confidentiality\n".to_string()))));
        assert_eq!(read_in(&Roots { proc_: d.join("nothing"), sys: d.join("nothing") }), Facts::default());
        let _ = std::fs::remove_dir_all(d);
    }

    #[test]
    fn graphics_cards_are_listed_by_their_vendor_and_device_files() {
        let d = tree("drm");
        for (i, v, dev) in [(0, "0x8086\n", Some("0x0412\n")), (2, "0x1002\n", None)] {
            let c = d.join(format!("class/drm/card{i}/device"));
            std::fs::create_dir_all(&c).unwrap();
            std::fs::write(c.join("vendor"), v).unwrap();
            if let Some(x) = dev {
                std::fs::write(c.join("device"), x).unwrap();
            }
        }
        std::fs::create_dir_all(d.join("class/drm/card1/device")).unwrap();
        assert_eq!(drm_gpus_in(&d), vec![("0x8086\n".to_string(), "0x0412\n".to_string()), ("0x1002\n".to_string(), String::new())]);
        let _ = std::fs::remove_dir_all(d);
    }

    #[test]
    fn the_boot_settings_and_the_route_are_read_under_the_given_roots_and_a_missing_file_says_so() {
        let d = std::env::temp_dir().join(format!("iyakki-kf-boot-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(d.join("proc/sys/vm")).unwrap();
        std::fs::create_dir_all(d.join("proc/net")).unwrap();
        std::fs::create_dir_all(d.join("sys/block/zram0")).unwrap();
        std::fs::write(d.join("proc/sys/vm/swappiness"), "100\n").unwrap();
        std::fs::write(d.join("sys/block/zram0/disksize"), "1073741824\n").unwrap();
        assert_eq!(super::vm_setting_in(&d.join("proc"), "swappiness").trim(), "100");
        assert!(super::vm_setting_in(&d.join("proc"), "page-cluster").starts_with("missing: "));
        assert!(super::vm_setting_in(&d.join("proc"), "../x").starts_with("missing: "), "a name with a slash is refused");
        assert_eq!(super::zram_attr_in(&d.join("sys"), "disksize").trim(), "1073741824");
        assert!(super::zram_attr_in(&d.join("sys"), "comp_algorithm").starts_with("missing: "));
        assert!(!super::has_default_route_in(&d.join("proc")), "no table: no default route");
        std::fs::write(d.join("proc/net/route"), "Iface\tDestination\tGateway\neth0\t0001A8C0\t00000000\n").unwrap();
        assert!(!super::has_default_route_in(&d.join("proc")), "a local route is not a default route");
        std::fs::write(d.join("proc/net/route"), "Iface\tDestination\tGateway\neth0\t00000000\t0101A8C0\n").unwrap();
        assert!(super::has_default_route_in(&d.join("proc")));
        assert_eq!((super::SYS_DEVICES, super::USB_DEVICES, super::DEBUGFS, super::DEFAULT_FB), ("/sys/devices", "/sys/bus/usb/devices", "/sys/kernel/debug", "/dev/fb0"));
        assert_eq!(super::CONSOLE_NODES, ["/dev/tty0", "/dev/console"]);
        let _ = std::fs::remove_dir_all(&d);
    }
}
