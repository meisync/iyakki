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
