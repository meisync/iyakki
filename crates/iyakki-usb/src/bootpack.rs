//! Reading the boot partition of an IYAKKI install image (or of a stick) WITHOUT mounting it, to make a system-update payload
//! (`iyakki dev update-pack IMAGE -o DIR`): the MBR or GPT partition table is read, the FAT32 volume is read with the in-house
//! reader (long names understood), and every file under `boot/` and `EFI/` is written to a folder with its SHA-256. Nothing is
//! ever written to the image.

use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};

use sha2::{Digest, Sha256};

use crate::fatclone::Fat32;

/// The biggest single file taken (the initramfs is about 180 MB).
pub const MAX_FILE: u64 = 700 << 20;
/// All files together.
pub const MAX_TOTAL: u64 = 1 << 30;
pub const MAX_FILES: usize = 400;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Extracted {
    /// `boot/bzImage`, `EFI/BOOT/bootx64.efi`...
    pub path: String,
    pub size: u64,
    pub sha256: String,
}

fn le32(b: &[u8], o: usize) -> u64 {
    u32::from_le_bytes(b[o..o + 4].try_into().unwrap()) as u64
}

/// Where the FAT32 boot volume starts in an image (bytes): the whole file when it is itself a FAT32 volume, else the first FAT
/// partition of an MBR table, else the EFI System Partition of a GPT.
pub fn boot_volume_offset<R: Read + Seek>(r: &mut R) -> Result<u64, String> {
    let mut mbr = [0u8; 512];
    r.seek(SeekFrom::Start(0)).and_then(|_| r.read_exact(&mut mbr)).map_err(|e| format!("reading the image: {e}"))?;
    if mbr[510..512] != [0x55, 0xAA] {
        return Err("this is not a disk image (no partition table signature)".into());
    }
    // a bare FAT32 volume (genimage's iyakki-boot.vfat)
    let bps = u16::from_le_bytes([mbr[11], mbr[12]]);
    let (spc, reserved, nfats) = (mbr[13], u16::from_le_bytes([mbr[14], mbr[15]]), mbr[16]);
    if matches!(bps, 512 | 1024 | 2048 | 4096) && spc.is_power_of_two() && reserved > 0 && matches!(nfats, 1 | 2) && mbr[22] == 0 && mbr[23] == 0 && le32(&mbr, 36) > 0 && le32(&mbr, 32) > 0 {
        return Ok(0);
    }
    let mut gpt = false;
    for i in 0..4 {
        let e = &mbr[446 + 16 * i..462 + 16 * i];
        let (ty, lba) = (e[4], le32(e, 8));
        if ty == 0xEE {
            gpt = true;
        } else if matches!(ty, 0xEF | 0x0B | 0x0C) && lba > 0 {
            return Ok(lba * 512);
        }
    }
    if gpt {
        let mut h = [0u8; 512];
        r.seek(SeekFrom::Start(512)).and_then(|_| r.read_exact(&mut h)).map_err(|e| e.to_string())?;
        if &h[0..8] != b"EFI PART" {
            return Err("the GPT header is damaged".into());
        }
        let (first, n, size) = (le32(&h, 72) | (le32(&h, 76) << 32), le32(&h, 80).min(128), le32(&h, 84).clamp(128, 512));
        let mut table = vec![0u8; (n * size) as usize];
        r.seek(SeekFrom::Start(first * 512)).and_then(|_| r.read_exact(&mut table)).map_err(|e| e.to_string())?;
        const ESP: [u8; 16] = [0x28, 0x73, 0x2A, 0xC1, 0x1F, 0xF8, 0xD2, 0x11, 0xBA, 0x4B, 0x00, 0xA0, 0xC9, 0x3E, 0xC9, 0x3B];
        for e in table.chunks_exact(size as usize) {
            if e[0..16] == ESP {
                return Ok((le32(e, 32) | (le32(e, 36) << 32)) * 512);
            }
        }
    }
    Err("no FAT32 boot partition was found in this image".into())
}

fn clean_component(c: &str) -> bool {
    !c.is_empty() && c.len() <= 100 && c != "." && c != ".." && c.chars().all(|ch| ch.is_ascii_alphanumeric() || " ._-+~@()".contains(ch))
}

/// Map the volume's top-level folder to the payload's name (`BOOT`/`boot` -> `boot`, `EFI` -> `EFI`); anything else is not part of an update.
fn top(c: &str) -> Option<&'static str> {
    match c.to_ascii_lowercase().as_str() {
        "boot" => Some("boot"),
        "efi" => Some("EFI"),
        _ => None,
    }
}

/// Write every file of the image's `boot/` and `EFI/` folders into `out` and return what was written.
pub fn extract(image: &Path, out: &Path) -> Result<Vec<Extracted>, String> {
    let mut f = File::open(image).map_err(|e| format!("{}: {e}", image.display()))?;
    let base = boot_volume_offset(&mut f)?;
    let mut fat = Fat32::open(&mut f, base)?;
    let items = fat.walk_long()?;
    let mut done = Vec::new();
    let mut total = 0u64;
    for it in items.iter().filter(|i| !i.dir) {
        let comps: Vec<&str> = it.path.trim_start_matches('/').split('/').collect();
        let Some(t) = comps.first().and_then(|c| top(c)) else { continue };
        if comps.len() < 2 || crate::fatclone::is_install_media_marker(&it.path) {
            continue;
        }
        if comps.iter().any(|c| !clean_component(c)) {
            return Err(format!("{} has a name that is not safe to copy", it.path));
        }
        if it.size as u64 > MAX_FILE {
            return Err(format!("{} is {} MB: more than the {} MB an update carries", it.path, it.size / 1_000_000, MAX_FILE >> 20));
        }
        total += it.size as u64;
        if total > MAX_TOTAL || done.len() >= MAX_FILES {
            return Err("the boot partition holds more than an update may carry".into());
        }
        let rel: PathBuf = std::iter::once(t).chain(comps[1..].iter().copied()).collect();
        let dest = out.join(&rel);
        if let Some(d) = dest.parent() {
            std::fs::create_dir_all(d).map_err(|e| format!("{}: {e}", d.display()))?;
        }
        let data = fat.read_file(it)?;
        std::fs::write(&dest, &data).map_err(|e| format!("{}: {e}", dest.display()))?;
        done.push(Extracted { path: rel.to_string_lossy().into_owned(), size: data.len() as u64, sha256: Sha256::digest(&data).iter().map(|b| format!("{b:02x}")).collect() });
    }
    done.sort_by(|a, b| a.path.cmp(&b.path));
    Ok(done)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fatclone::tests::mini_fat;

    fn tmp(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("iyakki-bootpack-{}-{tag}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    /// A disk image: an MBR with one FAT partition at 1 MiB holding `fat`.
    fn disk(fat: &[u8], ptype: u8) -> Vec<u8> {
        let mut img = vec![0u8; 1 << 20];
        img[446 + 4] = ptype;
        img[446 + 8..446 + 12].copy_from_slice(&2048u32.to_le_bytes());
        img[446 + 12..446 + 16].copy_from_slice(&((fat.len() / 512) as u32).to_le_bytes());
        img[510] = 0x55;
        img[511] = 0xAA;
        img.extend_from_slice(fat);
        img
    }

    #[test]
    fn the_boot_volume_is_found_in_an_mbr_image_a_bare_volume_and_a_gpt_image_and_nothing_else_is_accepted() {
        let fat = mini_fat(512, 8, 40_000_000, &[("BZIMAGE", vec![1; 100])]);
        assert_eq!(boot_volume_offset(&mut std::io::Cursor::new(disk(&fat, 0xEF))), Ok(1 << 20));
        assert_eq!(boot_volume_offset(&mut std::io::Cursor::new(disk(&fat, 0x0C))), Ok(1 << 20));
        assert_eq!(boot_volume_offset(&mut std::io::Cursor::new(fat.clone())), Ok(0), "a bare FAT32 volume");
        assert!(boot_volume_offset(&mut std::io::Cursor::new(disk(&fat, 0x83))).unwrap_err().contains("no FAT32"), "a Linux partition is not a boot volume");
        assert!(boot_volume_offset(&mut std::io::Cursor::new(vec![0u8; 4096])).unwrap_err().contains("not a disk image"));
        assert!(boot_volume_offset(&mut std::io::Cursor::new(vec![0u8; 10])).is_err());
        // GPT: protective MBR, header at LBA 1, one ESP entry at LBA 2
        let mut img = vec![0u8; 1 << 20];
        img[446 + 4] = 0xEE;
        img[510] = 0x55;
        img[511] = 0xAA;
        img[512..520].copy_from_slice(b"EFI PART");
        img[512 + 72..512 + 80].copy_from_slice(&2u64.to_le_bytes());
        img[512 + 80..512 + 84].copy_from_slice(&1u32.to_le_bytes());
        img[512 + 84..512 + 88].copy_from_slice(&128u32.to_le_bytes());
        img[1024..1040].copy_from_slice(&[0x28, 0x73, 0x2A, 0xC1, 0x1F, 0xF8, 0xD2, 0x11, 0xBA, 0x4B, 0x00, 0xA0, 0xC9, 0x3E, 0xC9, 0x3B]);
        img[1024 + 32..1024 + 40].copy_from_slice(&2048u64.to_le_bytes());
        assert_eq!(boot_volume_offset(&mut std::io::Cursor::new(img)), Ok(1 << 20));
    }

    #[test]
    fn long_names_are_assembled_only_when_their_checksum_and_order_fit_the_short_entry() {
        let short = *b"BZIMAGE    ";
        let sum = short.iter().fold(0u8, |sum, c| (sum >> 1).wrapping_add((sum & 1) << 7).wrapping_add(*c));
        let units = |s: &str| {
            let mut u = [0xFFFFu16; 13];
            for (i, c) in s.encode_utf16().enumerate() {
                u[i] = c;
            }
            if s.encode_utf16().count() < 13 {
                u[s.encode_utf16().count()] = 0;
            }
            u
        };
        assert_eq!(super::super::fatclone::long_name_for_tests(&[(0x41, units("bzImage"), sum)], &short), Some("bzImage".to_string()));
        // wrong checksum, no "last" flag, a gap in the order, a slash in the name: all fall back to the short name
        assert_eq!(super::super::fatclone::long_name_for_tests(&[(0x41, units("bzImage"), sum ^ 1)], &short), None);
        assert_eq!(super::super::fatclone::long_name_for_tests(&[(0x01, units("bzImage"), sum)], &short), None);
        assert_eq!(super::super::fatclone::long_name_for_tests(&[(0x42, units("bzImage"), sum)], &short), None);
        assert_eq!(super::super::fatclone::long_name_for_tests(&[(0x41, units("a/b"), sum)], &short), None);
        // two parts, stored last first
        let long = "a-long-file-name.bin";
        let u: Vec<u16> = long.encode_utf16().collect();
        let mut p1 = [0u16; 13];
        p1.copy_from_slice(&u[..13]);
        let mut p2 = [0xFFFFu16; 13];
        p2[..u.len() - 13].copy_from_slice(&u[13..]);
        p2[u.len() - 13] = 0;
        assert_eq!(super::super::fatclone::long_name_for_tests(&[(0x42, p2, sum), (0x01, p1, sum)], &short), Some(long.to_string()));
    }

    #[test]
    fn the_boot_files_come_out_with_their_hashes_nothing_else_does_and_the_image_is_never_written() {
        let kernel: Vec<u8> = (0..300_000u32).map(|i| (i % 251) as u8).collect();
        let fat = mini_fat(512, 8, 40_000_000, &[("BZIMAGE", kernel.clone()), ("BOOT/INITRD", vec![9; 70_000]), ("BOOT/GRUB/GRUB.CFG", b"set default=0\n".to_vec()), ("EMPTY.TXT", vec![])]);
        // the fixture's root folder entry has a made-up long name "b": take that entry out so the folder is BOOT
        let mut fat = fat;
        let at = fat.windows(32).position(|w| w[0] == 0x41 && w[11] == 0x0F).unwrap();
        fat[at] = 0xE5;
        let d = tmp("extract");
        let img = d.join("disk.img");
        let bytes = disk(&fat, 0xEF);
        std::fs::write(&img, &bytes).unwrap();
        let out = d.join("payload");
        let files = extract(&img, &out).unwrap();
        let names: Vec<&str> = files.iter().map(|f| f.path.as_str()).collect();
        assert_eq!(names, vec!["boot/GRUB/GRUB.CFG", "boot/INITRD"], "only boot/ and EFI/ are taken (BZIMAGE and EMPTY.TXT are in the root)");
        assert_eq!(std::fs::read(out.join("boot/INITRD")).unwrap(), vec![9; 70_000]);
        assert_eq!(files[1].sha256.len(), 64);
        assert_eq!(std::fs::read(&img).unwrap(), bytes, "the image is untouched");
        assert!(extract(&d.join("missing.img"), &out).is_err());
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn names_that_could_escape_are_refused() {
        for bad in ["..", ".", "", "a/b", "a\\b", "x\u{7}y", "caf\u{e9}"] {
            assert!(!clean_component(bad), "{bad:?}");
        }
        for ok in ["bzImage", "grub.cfg", "bootx64.efi", "iyakki-bg.png", "unicode.pf2", "a b"] {
            assert!(clean_component(ok), "{ok:?}");
        }
        assert_eq!(top("BOOT"), Some("boot"));
        assert_eq!(top("efi"), Some("EFI"));
        assert_eq!(top("System Volume Information"), None);
    }
}
