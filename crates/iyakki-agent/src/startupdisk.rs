//! Logs what the firmware says about the startup disk once per boot and, on a built-in disk, puts this system first in the boot order.
//!
//! Why: a stick plugged in while the Mac starts takes over the boot, and the Apple firmware may use its own startup variables (`efi-boot-device`,
//! ...) rather than `BootOrder`. Before anything is written, the next image says what is there (`iyakki dev dmesg` shows these lines), so the choice of
//! what to change rests on the machine's own answer. Plan: docs/development/USB-DETECT-AND-STARTUP-DISK-PLAN.md, section 2.

use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};

static DONE: AtomicBool = AtomicBool::new(false);

/// The lines for the variables in `dir` (efivarfs), empty on a machine without them.
pub fn lines(dir: &Path) -> Vec<String> {
    if !dir.is_dir() {
        return Vec::new();
    }
    iyakki_usb::efi::startup_info(dir).lines()
}

/// Called every poll; acts once, at 12 s of uptime.
pub fn step(uptime_secs: u64) {
    if cfg!(test) {
        return;
    }
    let ctx = StepCtx { efivars: Path::new("/sys/firmware/efi/efivars"), cmdline: &iyakki_storage::cmdline_text(), mounts: &iyakki_storage::read_mounts().unwrap_or_default(), sys_block: Path::new("/sys/block"), data_dir: Path::new(DATA_DIR) };
    act(uptime_secs, &DONE, &ctx, &mut |level, m| match level {
        Level::Warn => log::warn!("{m}"),
        Level::Info => log::info!("{m}"),
    });
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Level {
    Info,
    Warn,
}

/// What the once-per-boot step looks at (all of it given, so a test can make it up).
pub struct StepCtx<'a> {
    pub efivars: &'a Path,
    pub cmdline: &'a str,
    pub mounts: &'a str,
    pub sys_block: &'a Path,
    pub data_dir: &'a Path,
}

/// The step itself: nothing before 12 s of uptime, then once (`done` is set): the variables are logged, and on a built-in disk, unless
/// `iyakki.startupfirst=off` is on the command line, our own entry is put first in the boot order.
pub fn act(uptime_secs: u64, done: &AtomicBool, ctx: &StepCtx, log: &mut dyn FnMut(Level, String)) {
    if uptime_secs < 12 || done.swap(true, Ordering::Relaxed) {
        return;
    }
    for l in lines(ctx.efivars) {
        log(Level::Info, l);
    }
    if own_first_off(ctx.cmdline) {
        return;
    }
    let Some(dev) = mount_source(ctx.mounts, DATA_DIR) else { return };
    if !on_internal_disk(ctx.sys_block, &dev) {
        log(Level::Info, "startup-disk: running from a removable disk; the boot order stays as it is".into());
        return;
    }
    match iyakki_usb::efi::own_disk_first(ctx.efivars, ctx.data_dir) {
        Ok(Some(m)) => log(Level::Info, format!("startup-disk: {m} (a disk plugged in at power-on no longer takes over; `iyakki startup-disk revert` undoes it)")),
        Ok(None) => {}
        Err(e) => log(Level::Warn, format!("startup-disk: could not put this disk first: {e}")),
    }
}

const DATA_DIR: &str = "/var/lib/iyakki";

/// `iyakki.startupfirst=off` on the kernel command line leaves the firmware's boot order alone.
pub fn own_first_off(cmdline: &str) -> bool {
    cmdline.split_whitespace().any(|w| w == "iyakki.startupfirst=off")
}

/// The device mounted at `dir` in a `/proc/mounts` text (the last mount wins).
pub fn mount_source(mounts: &str, dir: &str) -> Option<String> {
    mounts.lines().filter_map(|l| {
        let mut f = l.split_whitespace();
        let (src, at) = (f.next()?, f.next()?);
        (at == dir).then(|| src.strip_prefix("/dev/").unwrap_or(src).to_string())
    }).last()
}

/// Is partition `dev` (`nvme0n1p3`, `sda3`) on a disk that is built in: not flagged removable and not behind USB?
pub fn on_internal_disk(sys_block: &Path, dev: &str) -> bool {
    let Ok(entries) = std::fs::read_dir(sys_block) else { return false };
    for e in entries.flatten() {
        let disk = e.file_name().to_string_lossy().to_string();
        if !dev.starts_with(&disk) || !e.path().join(dev).exists() {
            continue;
        }
        let removable = std::fs::read_to_string(e.path().join("removable")).map(|v| v.trim() == "1").unwrap_or(true);
        let real = std::fs::canonicalize(e.path()).unwrap_or_else(|_| e.path());
        return !removable && !real.to_string_lossy().contains("/usb");
    }
    false
}

/// `iyakki startup-disk status | register | first | next | own-first | revert`: what the firmware starts first, and (deliberately, needs root) make
/// IYAKKI's own boot entry, put it first, or start it at the next restart only. `own-first` is the old name of `first` for a computer that already
/// has the entry; `revert` puts the earlier order back.
pub fn cli(args: &[String]) -> i32 {
    let env = Env::real();
    let reboot_setting = std::fs::read_to_string(env.backup.join("settings.json")).ok().and_then(|t| serde_json::from_str::<serde_json::Value>(&t).ok()).and_then(|v| v["startup_reboot_into_iyakki"].as_bool()).unwrap_or(false);
    let done = |r: Result<String, String>| match r {
        Ok(m) => {
            crate::qprintln!("{m}");
            0
        }
        Err(e) => {
            crate::qeprintln!("{e}");
            1
        }
    };
    match args.first().map(String::as_str) {
        None | Some("status") => {
            let l = status_lines(&env, reboot_setting);
            l.iter().for_each(|x| crate::qprintln!("{x}"));
            if efi::available(&env.efivars) { 0 } else { 1 }
        }
        Some("register") => done(register(&env)),
        Some("first") => done(first(&env)),
        Some("own-first") => match efi::own_disk_first(&env.efivars, &env.backup) {
            Ok(Some(m)) => {
                crate::qprintln!("{m}. `iyakki startup-disk revert` puts the old order back.");
                0
            }
            Ok(None) => {
                crate::qprintln!("Nothing to change: IYAKKI's own entry is already first, or this computer has no IYAKKI entry (`iyakki startup-disk register` makes one).");
                0
            }
            Err(e) => {
                crate::qeprintln!("{e}");
                1
            }
        },
        Some("next") => done(next(&env)),
        Some("revert") => done(revert(&env)),
        _ => {
            crate::qeprintln!("usage: iyakki startup-disk status | register | first | next | own-first | revert");
            2
        }
    }
}

// ---- register the running boot volume, first / next boot (M53) --------------------------------------------------------------------

use iyakki_storage::gpt;
use iyakki_usb::efi::{self, BootEntry, Signature};

#[cfg(not(test))]
const EFIVARS: &str = "/sys/firmware/efi/efivars";
/// Where the boot volume is mounted while the system runs, in the order they are tried.
const BOOT_MOUNTS: [&str; 3] = ["/.stage1/boot", "/boot", "/run/iyakki/boot"];
/// The file the firmware's own scan and our entry start (the installer's boot loader).
pub const LOADER: &str = "\\EFI\\BOOT\\BOOTX64.EFI";

/// Everything the startup-disk actions read or write, so that tests run on fake folders.
pub struct Env {
    pub efivars: std::path::PathBuf,
    /// The data folder: the old boot order is kept here for `revert`.
    pub backup: std::path::PathBuf,
    pub roots: iyakki_storage::Roots,
    /// The text of `/proc/mounts`.
    pub mounts: String,
}

impl Env {
    pub fn real() -> Env {
        // a test never reaches the real firmware: without an override it sees a computer that has no variables
        #[cfg(test)]
        return ENV_OVERRIDE.with(|c| c.borrow_mut().take()).unwrap_or_else(|| {
            let none = std::path::PathBuf::from("/nonexistent-iyakki-test");
            Env { efivars: none.join("efivars"), backup: none.clone(), roots: iyakki_storage::Roots { sys_block: none.join("sys"), dev: none.join("dev"), mounts: none.join("mounts"), require_block: false }, mounts: String::new() }
        });
        #[cfg(not(test))]
        {
            Env {
                efivars: EFIVARS.into(),
                backup: std::env::var("IYAKKI_DATA").unwrap_or_else(|_| DATA_DIR.into()).into(),
                roots: iyakki_storage::Roots::real(),
                mounts: iyakki_storage::read_mounts().unwrap_or_default(),
            }
        }
    }
}

#[cfg(test)]
thread_local! {
    pub static ENV_OVERRIDE: std::cell::RefCell<Option<Env>> = const { std::cell::RefCell::new(None) };
}

const NO_UEFI: &str = "This computer was not started through UEFI (no firmware variables), so there is no boot entry to set.";

fn guard(env: &Env) -> Result<(), String> {
    if !efi::available(&env.efivars) {
        return Err(NO_UEFI.into());
    }
    if efi::efivarfs_read_only(&env.mounts) {
        return Err("The firmware variables are mounted read-only (efivarfs is read-only), so nothing was changed. Remount it read-write as an administrator and try again.".into());
    }
    Ok(())
}

/// The disk and partition number of partition `dev` (`sda1`, `nvme0n1p1`) in `sys_block`.
fn split_partition(sys_block: &Path, dev: &str) -> Option<(String, u32)> {
    for e in std::fs::read_dir(sys_block).ok()?.flatten() {
        let disk = e.file_name().to_string_lossy().to_string();
        if !dev.starts_with(&disk) || dev == disk || !e.path().join(dev).exists() {
            continue;
        }
        let n = std::fs::read_to_string(e.path().join(dev).join("partition")).ok()?.trim().parse().ok()?;
        return Some((disk, n));
    }
    None
}

/// The partition the system was started from: what is mounted at the boot folder, else partition 1 of the disk that holds the data volume
/// (the installer's layout).
pub fn boot_partition(mounts: &str, sys_block: &Path) -> Option<String> {
    for m in BOOT_MOUNTS {
        if let Some(dev) = mount_source(mounts, m).filter(|d| split_partition(sys_block, d).is_some()) {
            return Some(dev);
        }
    }
    let data = mount_source(mounts, DATA_DIR)?;
    let (disk, _) = split_partition(sys_block, &data)?;
    let part = iyakki_storage::partition_name(&disk, 1);
    split_partition(sys_block, &part).map(|_| part)
}

/// The boot entry for the running boot volume: partition GUID (GPT) or disk signature (MBR), start and size in the disk's own sectors.
pub fn running_boot_entry(env: &Env) -> Result<BootEntry, String> {
    let sys = &env.roots.sys_block;
    let part = boot_partition(&env.mounts, sys).ok_or("Cannot tell which volume IYAKKI was started from (no boot volume is mounted and the data volume's disk was not found).")?;
    let (disk, number) = split_partition(sys, &part).ok_or("The boot volume's disk was not found.")?;
    let num = |p: std::path::PathBuf| std::fs::read_to_string(p).ok().and_then(|t| t.trim().parse::<u64>().ok());
    let ss: u32 = num(sys.join(&disk).join("queue/logical_block_size")).filter(|n| matches!(*n, 512 | 4096)).unwrap_or(512) as u32;
    let size_512 = num(sys.join(&disk).join("size")).ok_or("The size of the boot volume's disk is unknown.")?;
    let mut f = std::fs::File::open(env.roots.dev.join(&disk)).map_err(|e| format!("reading the disk's table: {e}"))?;
    if let Ok(p) = gpt::read_gpt(&mut f, ss, size_512 * 512 / ss as u64) {
        let e = p.table.entries.iter().find(|e| e.index == number).ok_or("The boot volume is not in the disk's partition table.")?;
        return Ok(BootEntry { number, start_lba: e.first_lba, sectors: e.sectors(), signature: Signature::Gpt(e.guid), file: LOADER.into(), args: String::new() });
    }
    let start = num(sys.join(&disk).join(&part).join("start")).ok_or("The start of the boot volume is unknown.")? * 512 / ss as u64;
    let sectors = num(sys.join(&disk).join(&part).join("size")).ok_or("The size of the boot volume is unknown.")? * 512 / ss as u64;
    let mut head = vec![0u8; 2 * ss as usize];
    use std::io::{Read, Seek};
    f.rewind().and_then(|_| f.read_exact(&mut head)).map_err(|e| format!("reading the disk's table: {e}"))?;
    efi::mbr_boot_entry(&head[..512], &head[ss as usize..ss as usize + 512], number, start, sectors).ok_or_else(|| "The disk has no usable partition table (neither GPT nor MBR).".into())
}

/// The lines `status` prints: the firmware's order, ours, BootNext, and the setting.
pub fn status_lines(env: &Env, reboot_setting: bool) -> Vec<String> {
    let mut v = lines(&env.efivars);
    if v.is_empty() {
        return vec![NO_UEFI.into()];
    }
    match efi::own_entry(&env.efivars) {
        Some(n) => v.push(format!("startup disk: IYAKKI's own entry: Boot{n:04X}{}", if efi::boot_next(&env.efivars) == Some(n) { " (set to start at the next restart)" } else { "" })),
        None => v.push("startup disk: IYAKKI has no entry of its own in this computer's boot menu (`iyakki startup-disk register` makes one)".into()),
    }
    v.push(format!("startup disk: restart through `iyakki dev reboot` goes to IYAKKI: {}", if reboot_setting { "on" } else { "off" }));
    v
}

/// One line for Settings.
pub fn status_line(env: &Env, reboot_setting: bool) -> String {
    if !efi::available(&env.efivars) {
        return "No firmware boot variables on this computer.".into();
    }
    let order = iyakki_usb::efi::startup_info(&env.efivars).boot_order;
    let (own, next) = (efi::own_entry(&env.efivars), efi::boot_next(&env.efivars));
    let state = match own {
        None => "no IYAKKI entry in the boot menu".to_string(),
        Some(n) if next == Some(n) => "starts at the next restart".to_string(),
        Some(n) if order.first() == Some(&n) => "IYAKKI starts first".to_string(),
        Some(_) => "IYAKKI has an entry, another system starts first".to_string(),
    };
    format!("{state}; restart from `iyakki dev reboot` into IYAKKI: {}", if reboot_setting { "on" } else { "off" })
}

/// Create or replace our own entry for the running boot volume (appended to `BootOrder`, never first).
pub fn register(env: &Env) -> Result<String, String> {
    guard(env)?;
    let entry = running_boot_entry(env)?;
    let had = efi::own_entry(&env.efivars).is_some();
    efi::save_order_backup(&env.efivars, &env.backup)?;
    let name = efi::add_boot_entry(&env.efivars, &entry)?;
    Ok(format!("{} the boot entry {name} \"IYAKKI\" for the volume this system started from ({}). The boot order was not changed apart from adding it last. `iyakki startup-disk first` makes it the default, `iyakki startup-disk next` starts it once, `iyakki startup-disk revert` undoes the changes.", if had { "Replaced" } else { "Made" }, LOADER))
}

/// Register when needed, then put our entry first (the others keep their order; the old order is saved for `revert`).
pub fn first(env: &Env) -> Result<String, String> {
    guard(env)?;
    let made = if efi::own_entry(&env.efivars).is_none() { Some(register(env)?) } else { None };
    let moved = efi::own_disk_first(&env.efivars, &env.backup)?;
    let mut msg = match (made, moved) {
        (_, Some(m)) => format!("{m}."),
        (_, None) => "IYAKKI's entry is already first.".to_string(),
    };
    msg.push_str(" `iyakki startup-disk revert` puts the old order back.");
    Ok(msg)
}

/// Start IYAKKI at the next restart only (registers when needed); the boot order stays as it is.
pub fn next(env: &Env) -> Result<String, String> {
    guard(env)?;
    if efi::own_entry(&env.efivars).is_none() {
        register(env)?;
    }
    match efi::set_boot_next_to_own(&env.efivars)? {
        Some(n) => Ok(format!("{n} is set to start at the next restart only; the boot order is unchanged. `iyakki startup-disk revert` takes it back before then.")),
        None => Err("IYAKKI has no boot entry to start.".into()),
    }
}

/// Put the old boot order back and drop a pending one-time start. Our entry stays in the menu.
pub fn revert(env: &Env) -> Result<String, String> {
    guard(env)?;
    let cleared = efi::clear_own_boot_next(&env.efivars);
    let order = efi::revert_boot_order(&env.efivars, &env.backup)?;
    Ok(match (order, cleared) {
        (true, true) => "The earlier boot order is back and the one-time start is cancelled.".into(),
        (true, false) => "The earlier boot order is back.".into(),
        (false, true) => "The one-time start is cancelled. There was no saved order to put back.".into(),
        (false, false) => "There is no saved order to put back.".into(),
    })
}

/// A restart that was asked for through `iyakki dev reboot`: with the setting on, the one-time start of IYAKKI is set first.
/// Returns what was done for the audit log, `None` when nothing was done.
pub fn before_dev_reboot(setting_on: bool, env: &Env) -> Option<String> {
    if !setting_on {
        return None;
    }
    match next(env) {
        Ok(m) => Some(m),
        Err(e) => Some(format!("not set: {e}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_a_built_in_disk_is_put_first_and_it_can_be_switched_off() {
        let d = std::env::temp_dir().join(format!("iyakki-startdisk-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        for (disk, rem, part) in [("nvme0n1", "0", "nvme0n1p3"), ("sda", "1", "sda3")] {
            std::fs::create_dir_all(d.join(disk).join(part)).unwrap();
            std::fs::write(d.join(disk).join("removable"), rem).unwrap();
        }
        assert!(on_internal_disk(&d, "nvme0n1p3"));
        assert!(!on_internal_disk(&d, "sda3"));
        assert!(!on_internal_disk(&d, "sdb1"));
        let m = "/dev/nvme0n1p3 /var/lib/iyakki ext4 rw 0 0\ntmpfs /tmp tmpfs rw 0 0\n";
        assert_eq!(mount_source(m, "/var/lib/iyakki").as_deref(), Some("nvme0n1p3"));
        assert_eq!(mount_source(m, "/nowhere"), None);
        assert!(own_first_off("quiet iyakki.startupfirst=off"));
        assert!(!own_first_off("quiet"));
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn a_machine_without_efi_variables_says_nothing() {
        assert!(lines(Path::new("/nonexistent-efivars")).is_empty());
    }

    // ---- register / first / next / revert on fake firmware variables and a fake disk ----

    const GLOBAL: &str = iyakki_usb::efi::GLOBAL_GUID;
    const ESP_GUID: [u8; 16] = [0xAB; 16];

    struct Fake {
        root: std::path::PathBuf,
    }

    impl Fake {
        fn new(name: &str, boot_mounted: bool) -> (Fake, Env) {
            let root = std::env::temp_dir().join(format!("iyakki-startup-{name}-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&root);
            let (sys, dev, efi, data) = (root.join("sys"), root.join("dev"), root.join("efivars"), root.join("data"));
            for d in [&sys, &dev, &efi, &data] {
                std::fs::create_dir_all(d).unwrap();
            }
            // a 2 MB disk: partition 1 (boot, GUID ESP_GUID) at sector 2048, partition 2 (data)
            let t = gpt::Table {
                sector_size: 512,
                total_sectors: 4096,
                disk_guid: [9; 16],
                entries: vec![
                    gpt::Entry { index: 1, type_guid: gpt::TYPE_ESP, guid: ESP_GUID, first_lba: 2048, last_lba: 3071, attrs: 0, name: "IYAKKI-BOOT".into() },
                    gpt::Entry { index: 2, type_guid: gpt::TYPE_LINUX_DATA, guid: [0xCD; 16], first_lba: 3072, last_lba: 4000, attrs: 0, name: "IYAKKI-DATA".into() },
                ],
            };
            let ss = 512usize;
            let mut img = vec![0u8; 4096 * ss];
            img[..ss].copy_from_slice(&t.protective_mbr(&[], [1, 2, 3, 4]));
            img[ss..2 * ss].copy_from_slice(&t.header_bytes(false));
            let e = t.entries_bytes();
            img[2 * ss..2 * ss + e.len()].copy_from_slice(&e);
            let bk = t.backup_entries_lba() as usize * ss;
            img[bk..bk + e.len()].copy_from_slice(&e);
            let l = t.last_lba() as usize * ss;
            img[l..l + ss].copy_from_slice(&t.header_bytes(true));
            std::fs::write(dev.join("sda"), img).unwrap();
            let w = |p: std::path::PathBuf, v: &str| {
                std::fs::create_dir_all(p.parent().unwrap()).unwrap();
                std::fs::write(p, v).unwrap();
            };
            w(sys.join("sda/size"), "4096");
            w(sys.join("sda/queue/logical_block_size"), "512");
            w(sys.join("sda/removable"), "1");
            for (n, start, size, i) in [("sda1", 2048, 1024, 1), ("sda2", 3072, 929, 2)] {
                w(sys.join("sda").join(n).join("partition"), &i.to_string());
                w(sys.join("sda").join(n).join("start"), &start.to_string());
                w(sys.join("sda").join(n).join("size"), &size.to_string());
            }
            let mut mounts = format!("/dev/sda2 {DATA_DIR} ext4 rw 0 0\n");
            if boot_mounted {
                mounts.push_str("/dev/sda1 /.stage1/boot vfat rw 0 0\n");
            }
            // other systems' entries and a boot order that starts with Windows
            let utf = |s: &str| -> Vec<u8> { s.encode_utf16().chain([0u16]).flat_map(|u| u.to_le_bytes()).collect() };
            for (n, d) in [("0000", "Windows Boot Manager"), ("0001", "Ubuntu")] {
                let mut v = vec![7, 0, 0, 0, 1, 0, 0, 0, 0, 0];
                v.extend(utf(d));
                std::fs::write(efi.join(format!("Boot{n}-{GLOBAL}")), v).unwrap();
            }
            std::fs::write(efi.join(format!("BootOrder-{GLOBAL}")), [7, 0, 0, 0, 0, 0, 1, 0]).unwrap();
            let roots = iyakki_storage::Roots { sys_block: sys, dev, mounts: root.join("mounts"), require_block: false };
            (Fake { root: root.clone() }, Env { efivars: efi, backup: data, roots, mounts })
        }
        fn read(&self, name: &str) -> Vec<u8> {
            std::fs::read(self.root.join("efivars").join(format!("{name}-{GLOBAL}"))).unwrap()
        }
        fn order(&self) -> Vec<u16> {
            self.read("BootOrder")[4..].chunks_exact(2).map(|c| u16::from_le_bytes([c[0], c[1]])).collect()
        }
    }

    impl Drop for Fake {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.root);
        }
    }

    fn run_step(env: &Env, uptime: u64, cmdline: &str, done: &AtomicBool) -> Vec<(Level, String)> {
        let mut out = Vec::new();
        let ctx = StepCtx { efivars: &env.efivars, cmdline, mounts: &env.mounts, sys_block: &env.roots.sys_block, data_dir: &env.backup };
        act(uptime, done, &ctx, &mut |l, m| out.push((l, m)));
        out
    }

    #[test]
    fn the_boot_step_waits_for_twelve_seconds_logs_the_variables_once_and_puts_our_disk_first_only_on_a_built_in_disk() {
        let (f, env) = Fake::new("step", true);
        register(&env).unwrap();
        // the boot order starts with Windows again (as after a firmware reset)
        std::fs::write(f.root.join("efivars").join(format!("BootOrder-{GLOBAL}")), [7, 0, 0, 0, 0, 0, 1, 0, 2, 0]).unwrap();
        // the fake disk is a stick: nothing is changed and the reason is logged
        let done = AtomicBool::new(false);
        assert!(run_step(&env, 11, "", &done).is_empty(), "before 12 s nothing happens and the step is not used up");
        let got = run_step(&env, 12, "", &done);
        assert!(got.iter().any(|(l, m)| *l == Level::Info && m.contains("BootOrder")), "the variables are logged: {got:?}");
        assert!(got.iter().any(|(_, m)| m.contains("running from a removable disk")), "{got:?}");
        assert_eq!(f.order(), vec![0, 1, 2]);
        assert!(run_step(&env, 30, "", &done).is_empty(), "once per boot");
        // the same disk marked as built in: our entry moves to the front, the old order is kept and the line says how to undo it
        std::fs::write(f.root.join("sys/sda/removable"), "0").unwrap();
        let done = AtomicBool::new(false);
        let got = run_step(&env, 12, "quiet", &done);
        assert!(got.iter().any(|(l, m)| *l == Level::Info && m.starts_with("startup-disk: Boot0002 is first in the boot order now") && m.contains("revert")), "{got:?}");
        assert_eq!(f.order(), vec![2, 0, 1]);
        assert!(env.backup.join(efi::ORDER_BACKUP).exists());
        // with iyakki.startupfirst=off the variables are still logged and the order is left alone
        std::fs::write(f.root.join("efivars").join(format!("BootOrder-{GLOBAL}")), [7, 0, 0, 0, 0, 0, 1, 0, 2, 0]).unwrap();
        let got = run_step(&env, 40, "quiet iyakki.startupfirst=off", &AtomicBool::new(false));
        assert!(got.iter().any(|(_, m)| m.contains("BootOrder")) && !got.iter().any(|(_, m)| m.contains("is first")), "{got:?}");
        assert_eq!(f.order(), vec![0, 1, 2]);
        // a read-only variable folder is a warning, not a panic
        let (g, env2) = Fake::new("step-ro", true);
        register(&env2).unwrap();
        std::fs::write(g.root.join("efivars").join(format!("BootOrder-{GLOBAL}")), [7, 0, 0, 0, 0, 0, 1, 0, 2, 0]).unwrap();
        std::fs::write(g.root.join("sys/sda/removable"), "0").unwrap();
        use std::os::unix::fs::PermissionsExt;
        let var = env2.efivars.join(format!("BootOrder-{GLOBAL}"));
        std::fs::set_permissions(&var, std::fs::Permissions::from_mode(0o400)).unwrap();
        let writable = std::fs::OpenOptions::new().write(true).open(&var).is_ok(); // a process allowed to write anyway (root) cannot show this
        let got = run_step(&env2, 12, "", &AtomicBool::new(false));
        std::fs::set_permissions(&var, std::fs::Permissions::from_mode(0o600)).unwrap();
        if !writable {
            assert!(got.iter().any(|(l, m)| *l == Level::Warn && m.contains("could not put this disk first")), "{got:?}");
        }
    }

    #[test]
    fn register_makes_our_entry_with_the_boot_partitions_guid_and_leaves_the_others_alone() {
        let (f, env) = Fake::new("register", true);
        let (w, u) = (f.read("Boot0000"), f.read("Boot0001"));
        let m = register(&env).unwrap();
        assert!(m.starts_with("Made the boot entry Boot0002 \"IYAKKI\""), "{m}");
        assert!(m.contains("revert"), "says how to undo: {m}");
        let want = efi::load_option(&BootEntry { number: 1, start_lba: 2048, sectors: 1024, signature: Signature::Gpt(ESP_GUID), file: LOADER.into(), args: String::new() });
        assert_eq!(&f.read("Boot0002")[4..], &want[..], "device path: partition 1, its GUID, start and size, and the loader file");
        assert!(want.windows(16).any(|w| w == ESP_GUID));
        assert_eq!((f.read("Boot0000"), f.read("Boot0001")), (w, u), "other systems' entries are not touched");
        assert_eq!(f.order(), vec![0, 1, 2], "appended last: Windows stays the default");
        assert!(env.backup.join(efi::ORDER_BACKUP).exists(), "the old order is kept for revert");
    }

    #[test]
    fn registering_again_replaces_the_one_entry_and_a_missing_boot_mount_falls_back_to_partition_one_of_the_data_disk() {
        let (f, env) = Fake::new("replace", false);
        register(&env).unwrap();
        let first = f.read("Boot0002");
        let m = register(&env).unwrap();
        assert!(m.starts_with("Replaced the boot entry Boot0002"), "{m}");
        assert_eq!(f.read("Boot0002"), first, "same bytes, one entry");
        assert_eq!(efi::list_entries(&env.efivars).iter().filter(|(_, d, _)| d == "IYAKKI").count(), 1);
        assert_eq!(f.order(), vec![0, 1, 2], "not added twice");
        assert_eq!(boot_partition(&env.mounts, &env.roots.sys_block).as_deref(), Some("sda1"));
    }

    #[test]
    fn first_next_and_revert_change_only_the_order_and_the_one_time_variable() {
        let (f, env) = Fake::new("firstnext", true);
        let m = first(&env).unwrap();
        assert!(m.contains("Boot0002 is first in the boot order now"), "{m}");
        assert_eq!(f.order(), vec![2, 0, 1], "ours first, the others keep their order");
        assert!(first(&env).unwrap().contains("already first"), "idempotent");
        assert_eq!(f.order(), vec![2, 0, 1]);
        assert!(revert(&env).unwrap().contains("earlier boot order is back"));
        assert_eq!(f.order(), vec![0, 1], "the order from before the first change (our entry was not in it)");
        // next: BootNext only
        let m = next(&env).unwrap();
        assert!(m.contains("Boot0002") && m.contains("next restart only"), "{m}");
        assert_eq!(&f.read("BootNext")[4..], &[2, 0]);
        assert_eq!(f.order(), vec![0, 1], "the order is unchanged by next");
        assert!(revert(&env).unwrap().contains("one-time start is cancelled"));
        assert!(!env.efivars.join(format!("BootNext-{GLOBAL}")).exists());
        // a BootNext that is not ours is left alone
        std::fs::write(env.efivars.join(format!("BootNext-{GLOBAL}")), [7, 0, 0, 0, 1, 0]).unwrap();
        revert(&env).unwrap();
        assert!(env.efivars.join(format!("BootNext-{GLOBAL}")).exists(), "somebody else's BootNext is not ours to remove");
        assert!(status_lines(&env, false).iter().any(|l| l.contains("IYAKKI's own entry: Boot0002")));
    }

    #[test]
    fn a_read_only_variable_folder_and_a_machine_without_uefi_say_so_and_change_nothing() {
        let (f, mut env) = Fake::new("readonly", true);
        env.mounts.push_str("efivarfs /sys/firmware/efi/efivars efivarfs ro,nosuid,nodev,noexec,relatime 0 0\n");
        let before = f.order();
        for r in [register(&env), first(&env), next(&env), revert(&env)] {
            assert!(r.unwrap_err().contains("read-only"));
        }
        assert_eq!(f.order(), before);
        assert!(efi::list_entries(&env.efivars).iter().all(|(_, d, _)| d != "IYAKKI"));
        assert!(efi::write_error("Boot0002", &std::io::Error::from_raw_os_error(30)).contains("read-only"));
        assert!(efi::write_error("Boot0002", &std::io::Error::from_raw_os_error(1)).contains("nothing was changed"));
        let none = Env { efivars: f.root.join("nonexistent"), ..Env { efivars: env.efivars.clone(), backup: env.backup.clone(), roots: iyakki_storage::Roots { sys_block: env.roots.sys_block.clone(), dev: env.roots.dev.clone(), mounts: env.roots.mounts.clone(), require_block: false }, mounts: String::new() } };
        assert!(register(&none).unwrap_err().contains("not started through UEFI"));
        assert_eq!(status_lines(&none, false).len(), 1);
    }

    #[test]
    fn a_restart_through_dev_reboot_sets_boot_next_only_with_the_setting_on() {
        let (f, env) = Fake::new("devreboot", true);
        assert_eq!(before_dev_reboot(false, &env), None);
        assert!(!env.efivars.join(format!("BootNext-{GLOBAL}")).exists(), "setting off: nothing is written");
        assert!(efi::list_entries(&env.efivars).iter().all(|(_, d, _)| d != "IYAKKI"), "and no entry is made");
        let m = before_dev_reboot(true, &env).unwrap();
        assert!(m.contains("Boot0002"), "{m}");
        assert_eq!(&f.read("BootNext")[4..], &[2, 0]);
        assert_eq!(f.order(), vec![0, 1, 2], "the order only gained our entry at the end");
    }
}
