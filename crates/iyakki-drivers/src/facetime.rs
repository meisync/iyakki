//! FaceTime HD camera firmware, extracted from Apple's `AppleCameraInterface` driver file that the person supplies.
//! Apple's licence does not let anyone redistribute the firmware, so it is made on the person's own machine from
//! their own copy, in Rust, with checksums at both ends: the driver file must be a build we know (SHA-256), and the
//! result must be a firmware we know (SHA-256). Anything else is refused, with the numbers it found.
//!
//! The recipe is the one the facetimehd-firmware project documents (patjak/facetimehd-firmware): the firmware is a
//! gzip stream at a fixed offset of the Mach-O driver. The upstream tool also unpacks Apple's update disk images
//! (xar, pbzx, cpio); this does not: give it the driver file itself.
//!
//! UNVERIFIED: the table below was transcribed from the upstream script through a summarising tool, not from the raw
//! file. If a value is wrong the extraction fails closed (no output); compare the printed checksums with
//! `extract-firmware.sh` upstream before relying on it. See docs/MAC-HARDWARE.md.

use crate::pack::sha256_hex;

pub struct Recipe {
    pub label: &'static str,
    pub driver_sha256: &'static str,
    pub offset: usize,
    pub size: usize,
    /// Accepted SHA-256 of the extracted firmware.
    pub firmware_sha256: &'static str,
    pub firmware_version: &'static str,
}

pub const RECIPES: &[Recipe] = &[
    Recipe { label: "OS X 10.11 (El Capitan)", driver_sha256: "387097b5133e980196ac51504a60ae1ad8bab736eb0070a55774925ca0194892", offset: 81920, size: 603715, firmware_sha256: "e3e6034a67dfdaa27672dd547698bbc5b33f47f1fc7f5572a2fb68ea09d32d3d", firmware_version: "1.43.0" },
    Recipe { label: "OS X 10.11 (El Capitan)", driver_sha256: "4667e6828f6bfc690a39cf9d561369a525f44394f48d0a98d750931b2f3f278b", offset: 81920, size: 603715, firmware_sha256: "e3e6034a67dfdaa27672dd547698bbc5b33f47f1fc7f5572a2fb68ea09d32d3d", firmware_version: "1.43.0" },
    Recipe { label: "OS X 10.11.2", driver_sha256: "d4650346c940dafdc50e5fcbeeeffe074ec359726773e79c0cfa601cec6b1f08", offset: 81920, size: 603715, firmware_sha256: "e3e6034a67dfdaa27672dd547698bbc5b33f47f1fc7f5572a2fb68ea09d32d3d", firmware_version: "1.43.0" },
    Recipe { label: "OS X 10.11.3", driver_sha256: "dfac86799c6cf0aceb59bb4e732be8f030e7943eb1146830c7136f62621c9853", offset: 81920, size: 603715, firmware_sha256: "e3e6034a67dfdaa27672dd547698bbc5b33f47f1fc7f5572a2fb68ea09d32d3d", firmware_version: "1.43.0" },
    Recipe { label: "OS X 10.11.5", driver_sha256: "f56e68a880b65767335071531a1c75f3cfd4958adc6d871adf8dbf3b788e8ee1", offset: 81920, size: 603715, firmware_sha256: "e3e6034a67dfdaa27672dd547698bbc5b33f47f1fc7f5572a2fb68ea09d32d3d", firmware_version: "1.43.0" },
    Recipe { label: "macOS 10.12.6 (Sierra)", driver_sha256: "e959244db1e0561f6d5590c8e5000a36816c592e2820bafe89af0bea75556aca", offset: 81920, size: 602903, firmware_sha256: "240ef2e991f1d089d8228ce11d92b66bfa4b3d7289ec4fee228b64a713024330", firmware_version: "5.60.0" },
];

/// Where the extracted firmware goes under the firmware folder (what the facetimehd driver asks for).
pub const FIRMWARE_PATH: &str = "facetimehd/firmware.bin";

#[derive(Debug, PartialEq, Eq)]
pub struct Extracted {
    pub firmware: Vec<u8>,
    pub version: &'static str,
    pub from: &'static str,
    pub sha256: String,
}

/// The licence notice the person must accept before the device extracts anything.
pub const NOTICE: &str = "The camera firmware is Apple's software. Apple's licence for it does not allow it to be copied to other people, so IYAKKI does not carry it. This step reads the Apple driver file you give it, on this computer, and writes the firmware for your own camera only. Do it only with a file from a Mac you own and under the terms Apple gave you for it.";

/// Take the gzip member apart: header, deflate body (the CRC and length trailer are not needed: the checksum of the
/// result is checked instead).
fn gunzip(data: &[u8], max: usize) -> Result<Vec<u8>, String> {
    if data.len() < 18 || data[0] != 0x1f || data[1] != 0x8b || data[2] != 8 {
        return Err("the firmware area is not a gzip stream".into());
    }
    let flags = data[3];
    let mut p = 10usize;
    if flags & 4 != 0 {
        let n = u16::from_le_bytes([*data.get(p).ok_or("short gzip")?, *data.get(p + 1).ok_or("short gzip")?]) as usize;
        p += 2 + n;
    }
    for bit in [8u8, 16] {
        if flags & bit != 0 {
            while *data.get(p).ok_or("short gzip")? != 0 {
                p += 1;
            }
            p += 1;
        }
    }
    if flags & 2 != 0 {
        p += 2;
    }
    let body = data.get(p..data.len() - 8).ok_or("short gzip")?;
    miniz_oxide::inflate::decompress_to_vec_with_limit(body, max).map_err(|e| format!("the firmware could not be unpacked ({:?})", e.status))
}

pub fn extract(driver: &[u8]) -> Result<Extracted, String> {
    extract_with(RECIPES, driver)
}

pub fn extract_with(recipes: &'static [Recipe], driver: &[u8]) -> Result<Extracted, String> {
    if driver.starts_with(b"xar!") || driver.windows(4).rev().take(600).any(|w| w == b"koly") {
        return Err("this is a macOS disk image or installer package, not the driver file. Extract AppleCameraInterface from it first (on another computer, with a tool such as 7-Zip) and give that file.".into());
    }
    let sum = sha256_hex(driver);
    let Some(r) = recipes.iter().find(|r| r.driver_sha256 == sum) else {
        return Err(format!("this driver file is not a build this tool knows (sha256 {sum}). Supported: {}. Nothing was extracted.", recipes.iter().map(|r| r.label).collect::<Vec<_>>().join(", ")));
    };
    let blob = driver.get(r.offset..r.offset + r.size).ok_or("the driver file is shorter than its checksum says: it is damaged")?;
    let fw = gunzip(blob, 8 * 1024 * 1024)?;
    let out = sha256_hex(&fw);
    if out != r.firmware_sha256 {
        return Err(format!("the firmware came out with sha256 {out}, not the expected {}. Nothing was written.", r.firmware_sha256));
    }
    Ok(Extracted { firmware: fw, version: r.firmware_version, from: r.label, sha256: out })
}

/// A ready pack for the extracted firmware (matches the camera, asks the driver to reload).
pub fn pack_for(e: &Extracted) -> crate::pack::Pack {
    let known = crate::known::by_id("mac-facetimehd").expect("the camera entry exists");
    crate::pack::Pack {
        v: 1,
        matches: known.match_rules(),
        files: vec![crate::pack::FwFile { path: FIRMWARE_PATH.into(), data: crate::b64::encode(&e.firmware), sha256: e.sha256.clone() }],
        reload: vec!["facetimehd".into()],
        why: known.why.into(),
        ..Default::default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A synthetic "driver" whose firmware area is a gzip stream (stored deflate blocks) of `fw`, with a recipe injected for the test.
    fn gz(fw: &[u8]) -> Vec<u8> {
        let mut g = vec![0x1f, 0x8b, 8, 0, 0, 0, 0, 0, 0, 3];
        let comp = miniz_oxide::deflate::compress_to_vec(fw, 6);
        g.extend_from_slice(&comp);
        g.extend_from_slice(&[0u8; 8]); // crc and size (not checked)
        g
    }

    fn driver_with(fw: &[u8], offset: usize) -> (Vec<u8>, usize) {
        let g = gz(fw);
        let mut d = vec![0xAAu8; offset];
        d.extend_from_slice(&g);
        d.extend_from_slice(&[0xBB; 100]);
        (d, g.len())
    }

    fn run(driver: &[u8], offset: usize, size: usize, fw_sum: &str) -> Result<Vec<u8>, String> {
        // the real `extract_with`, with a one-off recipe (the real table holds Apple's hashes)
        let r: &'static [Recipe] = Box::leak(vec![Recipe { label: "test", driver_sha256: Box::leak(sha256_hex(driver).into_boxed_str()), offset, size, firmware_sha256: Box::leak(fw_sum.to_string().into_boxed_str()), firmware_version: "t" }].into_boxed_slice());
        extract_with(r, driver).map(|e| e.firmware)
    }

    #[test]
    fn the_gzip_area_at_the_offset_is_unpacked_and_checked_against_the_known_checksum() {
        let fw: Vec<u8> = (0..5000u32).map(|i| (i * 31 % 251) as u8).collect();
        let (d, glen) = driver_with(&fw, 81920);
        assert_eq!(run(&d, 81920, glen, &sha256_hex(&fw)).unwrap(), fw, "good checksum");
        assert!(run(&d, 81920, glen, &sha256_hex(b"other")).unwrap_err().contains("Nothing was written"), "bad checksum");
        assert!(run(&d, 81921, glen, &sha256_hex(&fw)).is_err(), "wrong offset is not gzip");
        assert!(run(&d, 81920, glen + 1_000_000, "x").is_err(), "area beyond the file");
    }

    #[test]
    fn an_unknown_driver_build_is_refused_with_its_checksum_and_the_supported_list() {
        let (d, _) = driver_with(b"firmware", 81920);
        let e = extract(&d).unwrap_err();
        assert!(e.contains(&sha256_hex(&d)) && e.contains("OS X 10.11.5") && e.contains("Nothing was extracted"), "{e}");
    }

    #[test]
    fn a_disk_image_is_recognised_and_explained() {
        let mut dmg = vec![0u8; 2000];
        dmg.extend_from_slice(b"koly");
        dmg.extend_from_slice(&[0u8; 500]);
        assert!(extract(&dmg).unwrap_err().contains("disk image"));
        assert!(extract(b"xar!....").unwrap_err().contains("disk image"));
    }

    #[test]
    fn gzip_headers_with_names_and_extras_are_skipped_and_garbage_is_refused() {
        let comp = miniz_oxide::deflate::compress_to_vec(b"hello firmware", 6);
        let mut g = vec![0x1f, 0x8b, 8, 4 | 8, 0, 0, 0, 0, 0, 3, 2, 0, 9, 9];
        g.extend_from_slice(b"name\0");
        g.extend_from_slice(&comp);
        g.extend_from_slice(&[0u8; 8]);
        assert_eq!(gunzip(&g, 1000).unwrap(), b"hello firmware");
        assert!(gunzip(b"not gzip at all, just text here", 1000).is_err());
        assert!(gunzip(&g, 3).is_err(), "output limit");
    }

    #[test]
    fn the_table_is_well_formed_and_the_pack_it_makes_validates() {
        for r in RECIPES {
            assert!(r.driver_sha256.len() == 64 && r.firmware_sha256.len() == 64 && r.driver_sha256.chars().all(|c| c.is_ascii_hexdigit()) && r.firmware_sha256.chars().all(|c| c.is_ascii_hexdigit()), "{}", r.label);
        }
        let mut ids: Vec<_> = RECIPES.iter().map(|r| r.driver_sha256).collect();
        ids.sort();
        ids.dedup();
        assert_eq!(ids.len(), RECIPES.len(), "no driver listed twice");
        let p = pack_for(&Extracted { firmware: vec![1, 2, 3], version: "t", from: "t", sha256: sha256_hex(&[1, 2, 3]) });
        assert!(p.validate().is_ok() && p.files[0].path == FIRMWARE_PATH && p.reload == vec!["facetimehd".to_string()]);
    }
}
