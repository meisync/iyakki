//! FaceTime HD camera firmware, extracted from Apple's `AppleCameraInterface` driver file that the person supplies.
//! Apple's licence does not let anyone redistribute the firmware, so it is made on the person's own machine from
//! their own copy, in Rust, with checksums at both ends: the driver file must be a build we know (SHA-256), and the
//! result must be a firmware we know (SHA-256). Anything else is refused, with the numbers it found.
//!
//! The recipe is the one the facetimehd-firmware project documents (patjak/facetimehd-firmware): the firmware is a
//! gzip stream at a fixed offset of the Mach-O driver. The upstream tool also unpacks Apple's update disk images
//! (xar, pbzx, cpio); this does not: give it the driver file itself.
//!
//! Checked 2026-10-22 against the raw `extract-firmware.sh` and `Makefile` of facetimehd-firmware and against the real bytes
//! Apple serves (driver sha256 e959244d..., firmware sha256 240ef2e9..., set files 9112, 1771 and 1675 verified): see
//! docs/MAC-CAMERA.md. The device can also fetch the driver file itself (`PARTS`, used by the agent's `camfw`): three byte ranges
//! of Apple's macOS 10.12.6 combo update, about 17 MB instead of the 1.9 GB image.
//!
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

/// Apple's macOS 10.12.6 combo update on Apple's update CDN (1,974,788,755 bytes; the same address facetimehd-firmware's Makefile uses).
/// The update's payload is a series of xz streams stored as they are in the image, so the pieces the camera needs can be fetched
/// by byte range. The whole image is never downloaded and its checksum is not known to us; every piece, the driver file and the
/// firmware are checked against the values below instead.
pub const SOURCE_URL: &str = "https://updates.cdn-apple.com/2019/cert/041-90765-20191011-837e856d-b522-4865-b64c-641048ed77c4/macOSUpdCombo10.12.6.dmg";
pub const SOURCE_BYTES: u64 = 1_974_788_755;
/// Hosts the pieces may come from (a name or a parent domain).
pub const SOURCE_HOSTS: &[&str] = &["cdn-apple.com", "apple.com"];

/// One byte range of the image: an xz stream of 16 MiB of the update payload.
pub struct Part {
    pub name: &'static str,
    /// First and last byte (inclusive).
    pub from: u64,
    pub to: u64,
    /// SHA-256 of the range as downloaded (the xz stream).
    pub sha256: &'static str,
}

impl Part {
    pub fn len(&self) -> u64 {
        self.to - self.from + 1
    }
}

/// `DRV_RANGE1`, `DRV_RANGE2` and `ASST_RANGE` of the Makefile.
pub const PARTS: [Part; 3] = [
    Part { name: "driver-1", from: 699_186_570, to: 703_316_225, sha256: "d9431f0fa8a2b5d2a31037cfe12c4f0abcb0a597739d925a0b8c834ddb6cb647" },
    Part { name: "driver-2", from: 703_316_242, to: 707_986_973, sha256: "562c8b86a69e59ef4cc2eed45ba380b8572908a8232e91c258a45e13f6a2c127" },
    Part { name: "assistant", from: 430_282_546, to: 439_105_037, sha256: "4c9c26a7ce607d749351428aee698e4fd487972392cc7ad06b69e2501dfe215a" },
];
/// The driver file `AppleCameraInterface` is the last `DRV_TAIL1` bytes of the first decoded chunk followed by the first `DRV_HEAD2` of the second.
pub const DRIVER_TAIL1: usize = 249_719;
pub const DRIVER_HEAD2: usize = 507_353;
/// `AppleCameraAssistant` (carries the sensor files): `ASST_SIZE` bytes after `ASST_SKIP` of the decoded chunk.
pub const ASSISTANT_SKIP: usize = 7_477_241;
pub const ASSISTANT_SIZE: usize = 515_632;
pub const ASSISTANT_SHA256: &str = "af6a9de00472657e925f546753c818f9e33636030892b14ca96a1b4817ea58c7";
/// The sensor calibration ("set") files inside the assistant: name, offset, size, SHA-256. The driver asks for `facetimehd/<name>_01XX.dat`
/// by the sensor it finds; without its file the camera still starts but runs without its factory calibration (colour and lens shading).
pub const SETFILES: &[(&str, usize, usize, &str)] = &[
    ("9112", 217_088, 33_060, "4dd756fa8460d8dc3d78d0d76944b2f92275d1fe9c83181bbc8292c81c005f1a"),
    ("1771", 253_952, 19_040, "756c2bb7c5e55b395449e43a0be1cb7c40c37dfc6c2b5abfaffb8ae70ff0fc4b"),
    ("1871", 274_432, 19_040, "bf36fbde0668ab7e44368b584f9fa64b5945b01003d04c6e3c6f22c0be0fd5f3"),
    ("1874", 294_912, 19_040, "ffde89e7819ac16a9eb1c8f0bc6dba0e980b508b2022507679d901c190f7cef8"),
    ("1222", 315_392, 20_076, "04a6aa0d67c0353505a56187c573b27dfdef703dfb4b98329b1ee74f59e4ba7e"),
    ("8221", 335_872, 30_240, "2e041686cf2484345b08b18207266abe725f41f8869e04d427aa092071d9edde"),
    ("1571", 368_640, 18_652, "0f73f550b65121115fe0b999f016fb3be3d109057597863df9fe01fd4678c300"),
    ("1575", 389_120, 18_652, "31068eab65ba25a480fd4d0463e86f8e2807828a2e251a20b6f107499e0f7936"),
    ("1674", 409_600, 18_044, "32377ac603d764f33f1466b5e4a7e3e08780d7becc50ad5b00292e29e2dd0374"),
    ("1675", 430_080, 18_044, "b7a38aef2755721bb28c92d84a15654b17e9fb3b0a0f088a384a981fc8fe16d6"),
    ("1671", 450_560, 18_044, "0b90133936bf0bbcdde4b85df8d3fc18722b58d2f8a1afc2564c6c242b6c57fa"),
];

/// The driver file from the two decoded chunks.
pub fn assemble_driver(chunk1: &[u8], chunk2: &[u8]) -> Result<Vec<u8>, String> {
    if chunk1.len() < DRIVER_TAIL1 || chunk2.len() < DRIVER_HEAD2 {
        return Err("a downloaded piece decoded to less than expected".into());
    }
    let mut d = Vec::with_capacity(DRIVER_TAIL1 + DRIVER_HEAD2);
    d.extend_from_slice(&chunk1[chunk1.len() - DRIVER_TAIL1..]);
    d.extend_from_slice(&chunk2[..DRIVER_HEAD2]);
    Ok(d)
}

/// The sensor files out of the decoded assistant chunk, each checked; the assistant itself is checked first.
pub fn setfiles_from_chunk(chunk: &[u8]) -> Result<Vec<(String, Vec<u8>)>, String> {
    let asst = chunk.get(ASSISTANT_SKIP..ASSISTANT_SKIP + ASSISTANT_SIZE).ok_or("the downloaded piece decoded to less than expected")?;
    let sum = sha256_hex(asst);
    if sum != ASSISTANT_SHA256 {
        return Err(format!("the camera assistant came out with sha256 {sum}, not the expected {ASSISTANT_SHA256}"));
    }
    setfiles(asst)
}

/// The eleven `<name>_01XX.dat` files of an assistant file (its checksum is the caller's business).
pub fn setfiles(asst: &[u8]) -> Result<Vec<(String, Vec<u8>)>, String> {
    let mut out = Vec::new();
    for (name, off, size, want) in SETFILES {
        let b = asst.get(*off..off + size).ok_or_else(|| format!("{name}_01XX.dat lies beyond the end of the file"))?;
        let got = sha256_hex(b);
        if got != *want {
            return Err(format!("{name}_01XX.dat came out with sha256 {got}, not the expected {want}"));
        }
        out.push((format!("{name}_01XX.dat"), b.to_vec()));
    }
    Ok(out)
}

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

/// [`pack_for`] with the sensor files (`facetimehd/<name>_01XX.dat`) added.
pub fn pack_with_setfiles(e: &Extracted, sets: &[(String, Vec<u8>)]) -> crate::pack::Pack {
    let mut p = pack_for(e);
    for (n, b) in sets {
        p.files.push(crate::pack::FwFile { path: format!("facetimehd/{n}"), data: crate::b64::encode(b), sha256: sha256_hex(b) });
    }
    p
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
    fn the_driver_is_put_together_from_two_chunks_and_the_sensor_files_are_cut_and_checked() {
        let c1: Vec<u8> = (0..400_000u32).map(|i| (i % 251) as u8).collect();
        let c2: Vec<u8> = (0..600_000u32).map(|i| (i % 241) as u8).collect();
        let d = assemble_driver(&c1, &c2).unwrap();
        assert_eq!(d.len(), DRIVER_TAIL1 + DRIVER_HEAD2);
        assert_eq!(&d[..DRIVER_TAIL1], &c1[c1.len() - DRIVER_TAIL1..]);
        assert_eq!(&d[DRIVER_TAIL1..], &c2[..DRIVER_HEAD2]);
        assert!(assemble_driver(&c1[..1000], &c2).is_err() && assemble_driver(&c1, &c2[..1000]).is_err());
        // an assistant of the right size but the wrong bytes is refused with the numbers
        let wrong = vec![0u8; ASSISTANT_SIZE];
        let e = setfiles(&wrong).unwrap_err();
        assert!(e.contains("9112_01XX.dat") && e.contains("not the expected"), "{e}");
        let mut chunk = vec![0u8; ASSISTANT_SKIP + ASSISTANT_SIZE];
        chunk[ASSISTANT_SKIP] = 1;
        assert!(setfiles_from_chunk(&chunk).unwrap_err().contains("camera assistant"));
        assert!(setfiles_from_chunk(&chunk[..1000]).is_err());
        // every set file lies inside the assistant and the names are the ones the driver asks for
        for (n, off, size, _) in SETFILES {
            assert!(off + size <= ASSISTANT_SIZE, "{n}");
            assert!(n.len() == 4 && n.chars().all(|c| c.is_ascii_digit()));
        }
    }

    #[test]
    fn the_pack_with_the_sensor_files_validates_and_matches_the_camera() {
        let e = Extracted { firmware: vec![1, 2, 3], version: "5.60.0", from: "t", sha256: sha256_hex(&[1, 2, 3]) };
        let p = pack_with_setfiles(&e, &[("1771_01XX.dat".into(), vec![7; 40]), ("1675_01XX.dat".into(), vec![8; 40])]);
        assert!(p.validate().is_ok());
        let paths: Vec<&str> = p.files.iter().map(|f| f.path.as_str()).collect();
        assert_eq!(paths, ["facetimehd/firmware.bin", "facetimehd/1771_01XX.dat", "facetimehd/1675_01XX.dat"]);
        assert_eq!(p.reload, ["facetimehd"]);
        assert!(p.matches.iter().any(|m| m.pci == "14e4:1570" && m.dmi_vendor == "Apple*"));
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
