//! A UEFI boot entry for the installed system, written straight into the firmware's variables through `efivarfs` (no
//! `efibootmgr`; the image does not carry it).
//!
//! Rules (docs/INSTALLER.md):
//!
//! * only our own entry, described "IYAKKI", is ever created or replaced; other entries are never edited or removed;
//! * the entry is **appended** to `BootOrder`: the computer keeps starting the other system by default, and IYAKKI is picked
//!   from the firmware's boot menu (F12, F10, Esc or Option at power-on, depending on the make);
//! * the device path starts at the disk's partition (the "short form" the UEFI specification defines); a firmware that does not
//!   take it still lists the disk, and a partition with `EFI/BOOT/BOOTX64.EFI` is found by its own scan.

use std::path::Path;

use iyakki_storage::gpt::Guid;

/// EFI_GLOBAL_VARIABLE
pub const GLOBAL_GUID: &str = "8be4df61-93ca-11d2-aa0d-00e098032b8c";
/// NON_VOLATILE | BOOTSERVICE_ACCESS | RUNTIME_ACCESS
const ATTRS: u32 = 7;
pub const DESCRIPTION: &str = "IYAKKI";

/// Where the partition is, for the device path.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Signature {
    Gpt(Guid),
    Mbr([u8; 4]),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BootEntry {
    pub number: u32,
    pub start_lba: u64,
    pub sectors: u64,
    pub signature: Signature,
    /// File on that partition, with backslashes: `\EFI\BOOT\BOOTX64.EFI`.
    pub file: String,
    /// Kernel command line for an EFI-stub boot (UCS-2 in the entry's optional data), empty for a boot loader.
    pub args: String,
}

fn utf16z(s: &str) -> Vec<u8> {
    s.encode_utf16().chain(std::iter::once(0)).flat_map(|u| u.to_le_bytes()).collect()
}

/// The EFI_LOAD_OPTION bytes (without the variable attributes).
pub fn load_option(e: &BootEntry) -> Vec<u8> {
    let mut path = Vec::new();
    // Hard Drive media device path node
    path.extend_from_slice(&[0x04, 0x01, 42, 0]);
    path.extend_from_slice(&e.number.to_le_bytes());
    path.extend_from_slice(&e.start_lba.to_le_bytes());
    path.extend_from_slice(&e.sectors.to_le_bytes());
    match &e.signature {
        Signature::Gpt(g) => {
            path.extend_from_slice(g);
            path.extend_from_slice(&[2, 2]);
        }
        Signature::Mbr(s) => {
            path.extend_from_slice(s);
            path.extend_from_slice(&[0u8; 12]);
            path.extend_from_slice(&[1, 1]);
        }
    }
    // File Path media device path node
    let f = utf16z(&e.file);
    path.extend_from_slice(&[0x04, 0x04]);
    path.extend_from_slice(&((4 + f.len()) as u16).to_le_bytes());
    path.extend_from_slice(&f);
    // end of the device path
    path.extend_from_slice(&[0x7F, 0xFF, 0x04, 0x00]);
    let mut out = Vec::new();
    out.extend_from_slice(&1u32.to_le_bytes()); // LOAD_OPTION_ACTIVE
    out.extend_from_slice(&(path.len() as u16).to_le_bytes());
    out.extend_from_slice(&utf16z(DESCRIPTION));
    out.extend_from_slice(&path);
    if !e.args.is_empty() {
        out.extend_from_slice(&e.args.encode_utf16().flat_map(|u| u.to_le_bytes()).collect::<Vec<u8>>());
    }
    out
}

fn var_name(base: &str) -> String {
    format!("{base}-{GLOBAL_GUID}")
}

fn read_var(dir: &Path, base: &str) -> Option<Vec<u8>> {
    std::fs::read(dir.join(var_name(base))).ok().filter(|b| b.len() >= 4).map(|b| b[4..].to_vec())
}

fn write_var(dir: &Path, base: &str, data: &[u8]) -> Result<(), String> {
    let path = dir.join(var_name(base));
    clear_immutable(&path);
    let mut bytes = ATTRS.to_le_bytes().to_vec();
    bytes.extend_from_slice(data);
    // efivarfs takes the attributes and the value in one write
    std::fs::write(&path, bytes).map_err(|e| format!("writing the firmware variable {base}: {e}"))
}

#[cfg(target_os = "linux")]
fn clear_immutable(path: &Path) {
    use std::os::fd::AsRawFd;
    if let Ok(f) = std::fs::File::open(path) {
        let mut flags: libc::c_long = 0;
        // SAFETY: FS_IOC_GETFLAGS/SETFLAGS take a pointer to a long; errors are ignored (not an efivarfs file in tests).
        unsafe {
            if libc::ioctl(f.as_raw_fd(), 0x8008_6601u64 as libc::c_ulong as _, &mut flags) == 0 && flags & 0x10 != 0 {
                flags &= !0x10;
                libc::ioctl(f.as_raw_fd(), 0x4008_6602u64 as libc::c_ulong as _, &flags);
            }
        }
    }
}
#[cfg(not(target_os = "linux"))]
fn clear_immutable(_: &Path) {}

/// The load options already in `dir`: (number, description, bytes).
pub fn list_entries(dir: &Path) -> Vec<(u16, String, Vec<u8>)> {
    let mut out = Vec::new();
    for e in std::fs::read_dir(dir).into_iter().flatten().flatten() {
        let name = e.file_name().to_string_lossy().into_owned();
        let Some(hex) = name.strip_prefix("Boot").and_then(|r| r.strip_suffix(&format!("-{GLOBAL_GUID}"))) else { continue };
        if hex.len() != 4 {
            continue;
        }
        let Ok(n) = u16::from_str_radix(hex, 16) else { continue };
        if let Some(v) = read_var(dir, &format!("Boot{hex}")) {
            if v.len() >= 6 {
                let units: Vec<u16> = v[6..].chunks_exact(2).map(|c| u16::from_le_bytes([c[0], c[1]])).take_while(|u| *u != 0).collect();
                out.push((n, String::from_utf16_lossy(&units), v));
            }
        }
    }
    out.sort();
    out
}


/// Apple's firmware variable namespace (the startup disk the picker and `bless` use live here).
pub const APPLE_GUID: &str = "7c436110-ab2a-4bbb-a880-fe41995c9f82";

/// What the firmware says about which disk starts, read only: for the question "why did the stick start and not the internal disk?".
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct StartupInfo {
    /// `BootOrder` (entry numbers in the order the firmware tries them).
    pub boot_order: Vec<u16>,
    /// `BootCurrent`: the entry this system was started from, when the firmware sets it.
    pub boot_current: Option<u16>,
    /// The load options: (number, description).
    pub entries: Vec<(u16, String)>,
    /// Apple startup variables found (`efi-boot-device`, `efi-boot-next`, ...): (name, bytes of value).
    pub apple: Vec<(String, usize)>,
}

fn u16s(b: &[u8]) -> Vec<u16> {
    b.chunks_exact(2).map(|c| u16::from_le_bytes([c[0], c[1]])).collect()
}

/// Read the startup variables in `dir` (efivarfs). Nothing is written.
pub fn startup_info(dir: &Path) -> StartupInfo {
    let mut info = StartupInfo::default();
    info.boot_order = read_var(dir, "BootOrder").map(|v| u16s(&v)).unwrap_or_default();
    info.boot_current = read_var(dir, "BootCurrent").and_then(|v| u16s(&v).first().copied());
    info.entries = list_entries(dir).into_iter().map(|(n, d, _)| (n, d)).collect();
    for e in std::fs::read_dir(dir).into_iter().flatten().flatten() {
        let name = e.file_name().to_string_lossy().into_owned();
        if let Some(base) = name.strip_suffix(&format!("-{APPLE_GUID}")) {
            if base.starts_with("efi-boot") || base.starts_with("boot-") || base == "efi-startup-disk" {
                let len = std::fs::metadata(e.path()).map(|m| m.len() as usize).unwrap_or(0).saturating_sub(4);
                info.apple.push((base.to_string(), len));
            }
        }
    }
    info.apple.sort();
    info
}

impl StartupInfo {
    /// The lines for the log: one fact each, no device paths.
    pub fn lines(&self) -> Vec<String> {
        let name = |n: u16| self.entries.iter().find(|e| e.0 == n).map_or(format!("Boot{n:04X}"), |e| format!("Boot{n:04X} \"{}\"", e.1));
        let order = if self.boot_order.is_empty() { "empty".to_string() } else { self.boot_order.iter().map(|n| name(*n)).collect::<Vec<_>>().join(", ") };
        let mut v = vec![format!("startup disk: BootOrder: {order}"), format!("startup disk: BootCurrent: {}", self.boot_current.map_or("not set".to_string(), name))];
        v.push(format!("startup disk: Apple startup variables: {}", if self.apple.is_empty() { "none".to_string() } else { self.apple.iter().map(|(n, l)| format!("{n} ({l} bytes)")).collect::<Vec<_>>().join(", ") }));
        v
    }
}


/// Where the order before the change is kept, so that it can be put back (next to nothing else: a plain file in the variables' own folder is not
/// possible on efivarfs, so the caller passes the data folder).
pub const ORDER_BACKUP: &str = "startup-order.before";

/// Put the entry described "IYAKKI" first in `BootOrder`, so that this computer starts its own disk before a stick it finds. Only `BootOrder` is
/// written (never another entry, never Apple's variables); the order before is saved in `backup_dir/startup-order.before` the first time, so
/// [`revert_boot_order`] can put it back. `Ok(None)`: already first, or there is no entry of ours.
pub fn own_disk_first(dir: &Path, backup_dir: &Path) -> Result<Option<String>, String> {
    let ours: Vec<u16> = list_entries(dir).into_iter().filter(|(_, d, _)| d == DESCRIPTION).map(|(n, _, _)| n).collect();
    let Some(&mine) = ours.first() else { return Ok(None) };
    let order = read_var(dir, "BootOrder").map(|v| u16s(&v)).unwrap_or_default();
    if order.first() == Some(&mine) {
        return Ok(None);
    }
    let backup = backup_dir.join(ORDER_BACKUP);
    if !backup.exists() {
        std::fs::write(&backup, order.iter().map(|n| format!("{n:04X}")).collect::<Vec<_>>().join(",")).map_err(|e| format!("saving the old boot order: {e}"))?;
    }
    let mut next = vec![mine];
    next.extend(order.iter().copied().filter(|n| *n != mine));
    let bytes: Vec<u8> = next.iter().flat_map(|n| n.to_le_bytes()).collect();
    write_var(dir, "BootOrder", &bytes)?;
    Ok(Some(format!("Boot{mine:04X} is first in the boot order now")))
}

/// Put the saved order back (and forget the copy). `Ok(false)`: there was nothing saved.
pub fn revert_boot_order(dir: &Path, backup_dir: &Path) -> Result<bool, String> {
    let backup = backup_dir.join(ORDER_BACKUP);
    let Ok(text) = std::fs::read_to_string(&backup) else { return Ok(false) };
    let order: Vec<u16> = text.trim().split(',').filter(|t| !t.is_empty()).filter_map(|t| u16::from_str_radix(t, 16).ok()).collect();
    let bytes: Vec<u8> = order.iter().flat_map(|n| n.to_le_bytes()).collect();
    write_var(dir, "BootOrder", &bytes)?;
    let _ = std::fs::remove_file(&backup);
    Ok(true)
}

/// Create (or, when an entry described "IYAKKI" is already there, replace) our boot entry and append it to `BootOrder`.
/// Returns the entry's name, for example `Boot0007`.
pub fn add_boot_entry(dir: &Path, e: &BootEntry) -> Result<String, String> {
    let existing = list_entries(dir);
    let number = match existing.iter().find(|(_, d, _)| d == DESCRIPTION) {
        Some((n, _, _)) => *n,
        None => (0..=0xFFFFu32).map(|n| n as u16).find(|n| !existing.iter().any(|(x, _, _)| x == n)).ok_or("the firmware has no free boot entry number")?,
    };
    let name = format!("Boot{number:04X}");
    write_var(dir, &name, &load_option(e))?;
    let mut order = read_var(dir, "BootOrder").unwrap_or_default();
    let already = order.chunks_exact(2).any(|c| u16::from_le_bytes([c[0], c[1]]) == number);
    if !already {
        order.extend_from_slice(&number.to_le_bytes());
        write_var(dir, "BootOrder", &order)?;
    }
    Ok(name)
}

/// The entry for the boot partition of a disk with a plain (MBR) table: `None` for anything else (a GPT disk gets its entry from the installer).
pub fn mbr_boot_entry(sector0: &[u8], sector1: &[u8], number: u32, start_lba: u64, sectors: u64) -> Option<BootEntry> {
    if sector0.len() < 512 || sector0[510..512] != [0x55, 0xAA] || sector1.starts_with(b"EFI PART") || start_lba == 0 || sectors == 0 {
        return None;
    }
    let sig: [u8; 4] = sector0[440..444].try_into().ok()?;
    if sig == [0; 4] {
        return None;
    }
    Some(BootEntry { number, start_lba, sectors, signature: Signature::Mbr(sig), file: "\\EFI\\BOOT\\BOOTX64.EFI".into(), args: String::new() })
}

/// A raw copy of the install image starts as "UEFI OS" in the firmware's boot menu: the firmware names a removable disk's fallback file
/// itself. Once running, give this computer an entry described "IYAKKI" for the boot partition (partition 1) of the disk that holds the data
/// volume `data_name`. Only when the firmware has a variable store and no entry of ours; appended, never first (see the top of this file).
/// `Ok(Some(name))` when one was written.
pub fn name_running_disk(roots: &iyakki_storage::Roots, dir: &Path, data_name: &str) -> Result<Option<String>, String> {
    use std::io::Read;
    if !available(dir) || list_entries(dir).iter().any(|(_, d, _)| d == DESCRIPTION) {
        return Ok(None);
    }
    let Some(disk) = iyakki_storage::list(roots).into_iter().find(|d| d.name == data_name).and_then(|d| d.parent) else { return Ok(None) };
    let part = iyakki_storage::partition_name(&disk, 1);
    let num = |p: std::path::PathBuf| std::fs::read_to_string(p).ok().and_then(|t| t.trim().parse::<u64>().ok());
    let (Some(start), Some(sectors)) = (num(roots.sys_block.join(&disk).join(&part).join("start")), num(roots.sys_block.join(&disk).join(&part).join("size"))) else { return Ok(None) };
    let mut head = [0u8; 1024];
    std::fs::File::open(roots.dev.join(&disk)).and_then(|mut f| f.read_exact(&mut head)).map_err(|e| format!("reading the disk's table: {e}"))?;
    match mbr_boot_entry(&head[..512], &head[512..], 1, start, sectors) {
        Some(e) => add_boot_entry(dir, &e).map(Some),
        None => Ok(None),
    }
}

/// Is `dir` a writable firmware variable store (a UEFI boot with `efivarfs` mounted)?
pub fn available(dir: &Path) -> bool {
    dir.join(var_name("BootOrder")).exists() || list_entries(dir).first().is_some()
}

/// `\EFI\IYAKKI\KERNEL.EFI` style path from `/EFI/IYAKKI/KERNEL.EFI`.
pub fn backslashed(p: &str) -> String {
    p.replace('/', "\\")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn put(dir: &Path, base_guid: &str, data: &[u8]) {
        let mut b = ATTRS.to_le_bytes().to_vec();
        b.extend_from_slice(data);
        std::fs::write(dir.join(base_guid), b).unwrap();
    }

    #[test]
    fn the_own_disk_can_be_put_first_and_the_old_order_comes_back() {
        let d = std::env::temp_dir().join(format!("iyakki-ownfirst-{}", std::process::id()));
        let b = std::env::temp_dir().join(format!("iyakki-ownfirst-bak-{}", std::process::id()));
        for x in [&d, &b] {
            let _ = std::fs::remove_dir_all(x);
            std::fs::create_dir_all(x).unwrap();
        }
        let utf = |s: &str| -> Vec<u8> { s.encode_utf16().chain([0u16]).flat_map(|u| u.to_le_bytes()).collect() };
        let entry = |desc: &str| { let mut v = vec![1, 0, 0, 0, 0, 0]; v.extend(utf(desc)); v };
        put(&d, &format!("Boot0007-{GLOBAL_GUID}"), &entry("IYAKKI"));
        put(&d, &format!("Boot0080-{GLOBAL_GUID}"), &entry("Mac OS X"));
        put(&d, &format!("BootOrder-{GLOBAL_GUID}"), &[0x80, 0, 0x81, 0, 0x07, 0]);
        assert_eq!(own_disk_first(&d, &b).unwrap().unwrap(), "Boot0007 is first in the boot order now");
        assert_eq!(startup_info(&d).boot_order, vec![7, 0x80, 0x81], "ours first, the others keep their order");
        assert_eq!(own_disk_first(&d, &b).unwrap(), None, "already first: nothing to do");
        assert!(b.join(ORDER_BACKUP).exists());
        assert!(revert_boot_order(&d, &b).unwrap());
        assert_eq!(startup_info(&d).boot_order, vec![0x80, 0x81, 7], "the old order is back");
        assert!(!revert_boot_order(&d, &b).unwrap(), "nothing saved any more");
        // no entry of ours: nothing is written
        std::fs::remove_file(d.join(format!("Boot0007-{GLOBAL_GUID}"))).unwrap();
        assert_eq!(own_disk_first(&d, &b).unwrap(), None);
        for x in [&d, &b] {
            let _ = std::fs::remove_dir_all(x);
        }
    }

    #[test]
    fn the_startup_variables_are_read_and_said_in_words_and_nothing_is_written() {
        let d = std::env::temp_dir().join(format!("iyakki-startup-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        let utf = |s: &str| -> Vec<u8> { s.encode_utf16().chain([0u16]).flat_map(|u| u.to_le_bytes()).collect() };
        let entry = |desc: &str| { let mut v = vec![1, 0, 0, 0, 0, 0]; v.extend(utf(desc)); v };
        put(&d, &format!("Boot0000-{GLOBAL_GUID}"), &entry("IYAKKI"));
        put(&d, &format!("Boot0080-{GLOBAL_GUID}"), &entry("Mac OS X"));
        put(&d, &format!("BootOrder-{GLOBAL_GUID}"), &[0x80, 0, 0, 0]);
        put(&d, &format!("BootCurrent-{GLOBAL_GUID}"), &[0, 0]);
        put(&d, &format!("efi-boot-device-data-{APPLE_GUID}"), &[1, 2, 3, 4, 5]);
        put(&d, &format!("unrelated-{APPLE_GUID}"), &[9]);
        let before: Vec<_> = std::fs::read_dir(&d).unwrap().flatten().map(|e| (e.file_name(), std::fs::read(e.path()).unwrap())).collect();
        let info = startup_info(&d);
        assert_eq!(info.boot_order, vec![0x80, 0]);
        assert_eq!(info.boot_current, Some(0));
        assert_eq!(info.apple, vec![("efi-boot-device-data".to_string(), 5)], "only the startup variables, not every Apple one");
        let lines = info.lines();
        assert!(lines[0].contains("Boot0080 \"Mac OS X\", Boot0000 \"IYAKKI\""), "{lines:?}");
        assert!(lines[1].contains("BootCurrent: Boot0000 \"IYAKKI\""), "{lines:?}");
        assert!(lines[2].contains("efi-boot-device-data (5 bytes)"), "{lines:?}");
        let after: Vec<_> = std::fs::read_dir(&d).unwrap().flatten().map(|e| (e.file_name(), std::fs::read(e.path()).unwrap())).collect();
        assert_eq!(before.len(), after.len(), "nothing was created or removed");
        assert_eq!(startup_info(Path::new("/nonexistent")), StartupInfo::default());
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn a_plain_copy_of_the_image_gets_an_entry_named_iyakki_only_for_an_mbr_disk() {
        let mut s0 = [0u8; 512];
        s0[510..512].copy_from_slice(&[0x55, 0xAA]);
        s0[440..444].copy_from_slice(&[0x00, 0x53, 0x46, 0x41]);
        let blank = [0u8; 512];
        let e = mbr_boot_entry(&s0, &blank, 1, 2048, 524288).expect("entry");
        assert_eq!(e.signature, Signature::Mbr([0x00, 0x53, 0x46, 0x41]));
        let bytes = load_option(&e);
        let units: Vec<u16> = bytes[6..].chunks_exact(2).map(|c| u16::from_le_bytes([c[0], c[1]])).take_while(|u| *u != 0).collect();
        assert_eq!(String::from_utf16_lossy(&units), "IYAKKI");
        let mut gpt = [0u8; 512];
        gpt[..8].copy_from_slice(b"EFI PART");
        assert!(mbr_boot_entry(&s0, &gpt, 1, 2048, 524288).is_none(), "a GPT disk is left to the installer");
        assert!(mbr_boot_entry(&[0u8; 512], &blank, 1, 2048, 524288).is_none(), "no table");
    }


    fn entry() -> BootEntry {
        BootEntry { number: 3, start_lba: 2048, sectors: 1_048_576, signature: Signature::Gpt([7; 16]), file: "\\EFI\\BOOT\\BOOTX64.EFI".into(), args: String::new() }
    }

    #[test]
    fn a_load_option_has_the_documented_layout() {
        let b = load_option(&entry());
        assert_eq!(u32::from_le_bytes(b[0..4].try_into().unwrap()), 1, "active");
        let fp_len = u16::from_le_bytes(b[4..6].try_into().unwrap()) as usize;
        let desc = utf16z(DESCRIPTION).len();
        assert_eq!(&b[6..6 + desc], &utf16z("IYAKKI")[..]);
        let p = &b[6 + desc..];
        assert_eq!(p.len(), fp_len);
        assert_eq!(&p[..4], &[4, 1, 42, 0], "hard drive node, 42 bytes");
        assert_eq!(u32::from_le_bytes(p[4..8].try_into().unwrap()), 3);
        assert_eq!(u64::from_le_bytes(p[8..16].try_into().unwrap()), 2048);
        assert_eq!(&p[24..40], &[7u8; 16]);
        assert_eq!(&p[40..42], &[2, 2], "GPT, GUID signature");
        assert_eq!(&p[42..44], &[4, 4], "file path node");
        assert_eq!(&p[p.len() - 4..], &[0x7F, 0xFF, 4, 0], "end of path");
        let mbr = BootEntry { signature: Signature::Mbr([1, 2, 3, 4]), ..entry() };
        let m = load_option(&mbr);
        assert!(m.windows(2).any(|w| w == [1, 1]));
        let k = BootEntry { args: "initrd=\\EFI\\IYAKKI\\INITRD quiet".into(), ..entry() };
        assert!(load_option(&k).len() > b.len() + 40);
    }

    #[test]
    fn our_entry_is_appended_replaced_not_duplicated_and_other_entries_are_untouched() {
        let dir = std::env::temp_dir().join(format!("iyakki-efivars-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        // another system's entries
        let foreign = [vec![7u8, 0, 0, 0, 1, 0, 0, 0, 0, 0, b'W', 0, 0, 0], vec![7u8, 0, 0, 0, 1, 0, 0, 0, 0, 0, b'U', 0, 0, 0]];
        std::fs::write(dir.join(var_name("Boot0000")), &foreign[0]).unwrap();
        std::fs::write(dir.join(var_name("Boot0001")), &foreign[1]).unwrap();
        std::fs::write(dir.join(var_name("BootOrder")), [7u8, 0, 0, 0, 1, 0, 0, 0]).unwrap();
        assert!(available(&dir));
        let name = add_boot_entry(&dir, &entry()).unwrap();
        assert_eq!(name, "Boot0002");
        assert_eq!(std::fs::read(dir.join(var_name("Boot0000"))).unwrap(), foreign[0], "foreign entries are not touched");
        assert_eq!(std::fs::read(dir.join(var_name("Boot0001"))).unwrap(), foreign[1]);
        assert_eq!(std::fs::read(dir.join(var_name("BootOrder"))).unwrap(), [7, 0, 0, 0, 1, 0, 0, 0, 2, 0], "appended: the other system stays the default");
        // a second install replaces our entry and does not add it twice
        let again = add_boot_entry(&dir, &BootEntry { start_lba: 4096, ..entry() }).unwrap();
        assert_eq!(again, "Boot0002");
        assert_eq!(std::fs::read(dir.join(var_name("BootOrder"))).unwrap(), [7, 0, 0, 0, 1, 0, 0, 0, 2, 0]);
        let listed = list_entries(&dir);
        assert_eq!(listed.iter().filter(|(_, d, _)| d == "IYAKKI").count(), 1);
        assert_eq!(backslashed("/EFI/IYAKKI/KERNEL.EFI"), "\\EFI\\IYAKKI\\KERNEL.EFI");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
