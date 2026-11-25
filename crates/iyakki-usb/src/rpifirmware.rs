//! Vendor firmware for Raspberry Pi boards, fetched only at the person's click (F-ARM-14; docs/RESTRICTED-FIRMWARE.md, the rule
//! "firmware by button"). Nothing in this file is a firmware file: it holds the pinned list of what to download (repository, commit, file,
//! size, SHA-256), shows the licence first, refuses to download without the acceptance, checks every byte against the pin and deletes
//! everything on one wrong byte.
//!
//! * `rpi5-wifi`: the Wi-Fi firmware of the Pi 5's CYW43455 chip (the board's own Wi-Fi). The Pi 5 itself needs NO boot blob on the card: its EEPROM
//!   carries the VideoCore firmware, and IYAKKI never writes the EEPROM.
//! * `rpi3-boot`: `bootcode.bin`, `start.elf`, `fixup.dat` for the Pi 3 "IYAKKI lite" target. Pinned now so the licence question is answered
//!   in advance; the Pi 3 image is a design (plan section 5), so the pack is listed as not ready and `fetch` refuses it.
//!
//! The pins were taken on 2026-11-21 by downloading each file once from the named commit and hashing it; the licence texts were fetched the
//! same way. A person must still read them (`docs/FIRMWARE-LICENCES.md`): the project has no lawyer, so an unread text is never a licence.

use std::io::Read;
use std::path::{Path, PathBuf};

use sha2::{Digest, Sha256};

use crate::fetch::{url_allowed, Opener};

/// One file of a pack.
#[derive(Clone, Copy, Debug)]
pub struct PackFile {
    /// Path under the vendor repository at the pinned commit.
    pub url_path: &'static str,
    /// Where the file lands under the pack's destination folder.
    pub dest: &'static str,
    pub size: u64,
    pub sha256: &'static str,
}

#[derive(Clone, Copy, Debug)]
pub struct Pack {
    pub id: &'static str,
    pub title: &'static str,
    pub vendor: &'static str,
    /// The licence in words a person can read before accepting (the text file is downloaded and kept next to the files).
    pub licence_summary: &'static str,
    /// `https://.../<commit>/` (the pin is the commit in the URL).
    pub base_url: &'static str,
    pub commit: &'static str,
    pub licence: PackFile,
    pub files: &'static [PackFile],
    /// Boards the pack is for (`iyakki_stage1::board_of_model` ids).
    pub boards: &'static [&'static str],
    /// `false`: pinned but not offered (the target is not built).
    pub ready: bool,
}

const RPI_FIRMWARE: &str = "https://raw.githubusercontent.com/raspberrypi/firmware/6f0881cba8bea8ec24956a5718714730e9936ec5/";
const RPI_NONFREE: &str = "https://raw.githubusercontent.com/RPi-Distro/firmware-nonfree/5d2295b633d94641253e16ba90fc847436fc9d4a/";

pub const PACKS: [Pack; 2] = [
    Pack {
        id: "rpi5-wifi",
        title: "Raspberry Pi 5 Wi-Fi firmware (Cypress CYW43455)",
        vendor: "Cypress Semiconductor (Infineon), packaged by Raspberry Pi Ltd",
        licence_summary: "Binary files under the Cypress redistribution licence ('binary-redist-Cypress' in the repository's debian/copyright): use is limited to Cypress/Broadcom chips, which is the chip on the board. The licence text of this pin is downloaded with the files and kept next to them. The three small text and CLM files are the board's calibration data. Not yet read by a person at the project: read it before you accept.",
        base_url: RPI_NONFREE,
        commit: "5d2295b633d94641253e16ba90fc847436fc9d4a",
        licence: PackFile { url_path: "debian/copyright", dest: "LICENCE.cypress.copyright", size: 430_523, sha256: "07082fd0bb65e32c73af39a2292dd6c83f169be7427d338ba1fe34f9b86c0e30" },
        files: &[
            PackFile { url_path: "debian/added-firmware/cypress/cyfmac43455-sdio-standard.bin", dest: "brcm/brcmfmac43455-sdio.bin", size: 609_309, sha256: "d608f866582519c0a28d86db43040f4f1b98dd1d153e72e9752586546b4a36c3" },
            PackFile { url_path: "debian/added-firmware/cypress/cyfmac43455-sdio.clm_blob", dest: "brcm/brcmfmac43455-sdio.clm_blob", size: 2_676, sha256: "9823842cae9fb9a5dd1e5fb31f595516ec7deee341354bef30bb3026eee29cc1" },
            PackFile { url_path: "debian/added-firmware/brcm/brcmfmac43455-sdio.txt", dest: "brcm/brcmfmac43455-sdio.txt", size: 2_074, sha256: "ca709be81a78bdb6932936374f39943acbd7af07fae6151011127599a3ce9e3d" },
        ],
        boards: &["rpi5"],
        ready: true,
    },
    Pack {
        id: "rpi3-boot",
        title: "Raspberry Pi 3 boot firmware (bootcode.bin, start.elf, fixup.dat)",
        vendor: "Broadcom Corporation and Raspberry Pi (Trading) Ltd",
        licence_summary: "LICENCE.broadcom: redistribution and use in binary form, without modification, only for developing for, running or using a Raspberry Pi device; the notice must be reproduced with the files; Broadcom's name may not be used to endorse. The Pi 3 target is a design: this pack is pinned for the plan and cannot be fetched yet.",
        base_url: RPI_FIRMWARE,
        commit: "6f0881cba8bea8ec24956a5718714730e9936ec5",
        licence: PackFile { url_path: "boot/LICENCE.broadcom", dest: "LICENCE.broadcom", size: 1_594, sha256: "c7283ff51f863d93a275c66e3b4cb08021a5dd4d8c1e7acc47d872fbe52d3d6b" },
        files: &[
            PackFile { url_path: "boot/bootcode.bin", dest: "bootcode.bin", size: 52_624, sha256: "4245ec23b58158dc3e55f3e065eb4ecc0c0705d76c8db506e9f824941feee32e" },
            PackFile { url_path: "boot/start.elf", dest: "start.elf", size: 3_023_104, sha256: "32f990801e7765bae6fe5cb940ba5eeff02dd0dac8dc5474db5b3fd345281790" },
            PackFile { url_path: "boot/fixup.dat", dest: "fixup.dat", size: 7_381, sha256: "7853f2b51e4489f8b3ce6f9c52fd0afdda38c7383961524e0883d34afcb4827b" },
        ],
        boards: &["rpi3"],
        ready: false,
    },
];

pub fn find(id: &str) -> Option<&'static Pack> {
    PACKS.iter().find(|p| p.id == id)
}

impl Pack {
    /// The text shown before anything is downloaded: what, from where, how big, which licence, and the sentence that nothing is shipped.
    pub fn consent_text(&self) -> String {
        let total: u64 = self.files.iter().map(|f| f.size).sum::<u64>() + self.licence.size;
        let mut t = format!("{}\nFrom: {}\nCommit: {}\nFiles ({} KB in all):\n", self.title, self.vendor, self.commit, total.div_ceil(1024));
        for f in self.files {
            t.push_str(&format!("  {}  ({} bytes, sha256 {})\n", f.dest, f.size, &f.sha256[..16]));
        }
        t.push_str(&format!("Licence: {}\n", self.licence_summary));
        t.push_str("IYAKKI does not ship these files. They are downloaded from the vendor's repository only when you accept, checked against the sizes and checksums above, and the licence text is kept next to them.\n");
        t
    }
    fn url(&self, f: &PackFile) -> String {
        format!("{}{}", self.base_url, f.url_path)
    }
}

/// Downloads one file through `open`, refusing anything but its pinned size and SHA-256; written through a `.part` file.
fn get_file(open: Opener, url: &str, f: &PackFile, dest: &Path) -> Result<(), String> {
    url_allowed(url)?;
    let mut head = crate::fetch::open_following(open, url)?;
    if head.status != 200 {
        return Err(format!("{url} answered HTTP {}", head.status));
    }
    let mut body = Vec::new();
    // one byte more than the pin is read, so a longer answer is noticed without reading an unbounded body
    head.body.by_ref().take(f.size + 1).read_to_end(&mut body).map_err(|e| format!("the download of {} broke off: {e}", f.dest))?;
    if body.len() as u64 != f.size {
        return Err(format!("{} is {} bytes; the pinned file is {} bytes (the vendor changed it or the answer was cut); nothing was installed", f.dest, body.len(), f.size));
    }
    let got: String = Sha256::digest(&body).iter().map(|b| format!("{b:02x}")).collect();
    if got != f.sha256 {
        return Err(format!("{} does not match its pinned checksum (the file changed or was altered)\n  pinned {}\n  got    {got}", f.dest, f.sha256));
    }
    if let Some(d) = dest.parent() {
        std::fs::create_dir_all(d).map_err(|e| format!("{}: {e}", d.display()))?;
    }
    let part = dest.with_extension("part");
    std::fs::write(&part, &body).map_err(|e| format!("{}: {e}", part.display()))?;
    std::fs::rename(&part, dest).map_err(|e| format!("{}: {e}", dest.display()))
}

/// The licence text first, then the files, into `out/<pack id>/`. `accepted` must be true (the person said so after the consent text); a
/// pack that is not ready is refused. On any error everything this call wrote is deleted. Returns the paths written.
pub fn fetch_pack(open: Opener, pack: &Pack, out: &Path, accepted: bool) -> Result<Vec<PathBuf>, String> {
    if !pack.ready {
        return Err(format!("{} is pinned for the plan but not offered: the target it is for is not built yet", pack.id));
    }
    if !accepted {
        return Err("the licence was not accepted: nothing was downloaded".into());
    }
    let root = out.join(pack.id);
    let mut written = Vec::new();
    let all = std::iter::once(&pack.licence).chain(pack.files.iter());
    for f in all {
        let dest = root.join(f.dest);
        if let Err(e) = get_file(open, &pack.url(f), f, &dest) {
            for w in &written {
                let _ = std::fs::remove_file(w);
            }
            let _ = std::fs::remove_file(dest.with_extension("part"));
            return Err(e);
        }
        written.push(dest);
    }
    Ok(written)
}

/// A real HTTPS transport for the command line: `curl` (https only, no redirect to another scheme), the body from a temporary file. The answer
/// is verified byte for byte against the pin, so the transport is not trusted for anything.
fn curl_open(url: &str) -> Result<crate::fetch::Head, String> {
    let tmp = std::env::temp_dir().join(format!("iyakki-rpi-fw-{}-{}", std::process::id(), url.len()));
    let out = std::process::Command::new("curl").args(["-sS", "--proto", "=https", "--max-time", "300", "-o"]).arg(&tmp).args(["-w", "%{http_code}", url]).output().map_err(|e| format!("curl is needed for the download: {e}"))?;
    let status: u16 = String::from_utf8_lossy(&out.stdout).trim().parse().unwrap_or(0);
    let file = std::fs::File::open(&tmp).map_err(|e| format!("the download left no file: {e}"))?;
    let _ = std::fs::remove_file(&tmp);
    Ok(crate::fetch::Head { status, headers: vec![], body: Box::new(file) })
}

/// `iyakki-usb rpi ...` on a computer: the boot volume texts, the table, the firmware packs.
pub fn run_rpi(args: &[String]) -> Result<(), String> {
    if args.first().is_some_and(|a| a == "firmware") {
        let flag = |name: &str| args.iter().position(|a| a == name).and_then(|i| args.get(i + 1)).map(String::as_str);
        match args.get(1).map(String::as_str) {
            Some("list") => {
                for p in &PACKS {
                    println!("{:<10} {}{}", p.id, p.title, if p.ready { "" } else { "  (pinned, not offered yet)" });
                }
                return Ok(());
            }
            Some("fetch") => {
                let pack = find(flag("--pack").ok_or("--pack ID is needed (see: iyakki-usb rpi firmware list)")?).ok_or("no such pack (see: iyakki-usb rpi firmware list)")?;
                let out = Path::new(flag("--out").ok_or("--out DIR is needed")?);
                print!("{}", pack.consent_text());
                // the acceptance is a flag the person types after reading the text above; there is no default
                let accepted = args.iter().any(|a| a == "--accept-licence");
                let written = fetch_pack(&curl_open, pack, out, accepted)?;
                for w in written {
                    println!("wrote {}", w.display());
                }
                return Ok(());
            }
            _ => {}
        }
        print!("{}", crate::rpiboot::RPI_HELP);
        return Ok(());
    }
    let mut write = |p: &Path, b: &[u8]| -> Result<(), String> {
        if let Some(d) = p.parent() {
            std::fs::create_dir_all(d).map_err(|e| format!("{}: {e}", d.display()))?;
        }
        std::fs::write(p, b).map_err(|e| format!("{}: {e}", p.display()))
    };
    let read = |p: &Path| std::fs::read_to_string(p).map_err(|e| format!("{}: {e}", p.display()));
    print!("{}", crate::rpiboot::run_cli(args, &mut write, &read)?);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fetch::Head;

    fn serve(files: Vec<(String, Vec<u8>)>) -> impl Fn(&str) -> Result<Head, String> {
        move |url: &str| {
            let hit = files.iter().find(|(u, _)| u == url);
            let (status, body) = match hit {
                Some((_, b)) => (200, b.clone()),
                None => (404, Vec::new()),
            };
            Ok(Head { status, headers: vec![], body: Box::new(std::io::Cursor::new(body)) })
        }
    }

    /// A made-up pack whose files are `n` bytes of a letter, with the real SHA-256 computed here.
    fn made_up() -> (Pack, Vec<(String, Vec<u8>)>) {
        fn leak(s: String) -> &'static str {
            Box::leak(s.into_boxed_str())
        }
        let sha = |b: &[u8]| -> &'static str { leak(Sha256::digest(b).iter().map(|x| format!("{x:02x}")).collect()) };
        let (lic, a, b) = (b"licence text".to_vec(), vec![b'a'; 100], vec![b'b'; 50]);
        let files: &'static [PackFile] = Box::leak(Box::new([
            PackFile { url_path: "boot/a.bin", dest: "a.bin", size: 100, sha256: sha(&a) },
            PackFile { url_path: "boot/b.dat", dest: "sub/b.dat", size: 50, sha256: sha(&b) },
        ]));
        let pack = Pack {
            id: "test-pack",
            title: "test",
            vendor: "nobody",
            licence_summary: "a test licence",
            base_url: "https://example.invalid/c0ffee/",
            commit: "c0ffee",
            licence: PackFile { url_path: "LICENCE", dest: "LICENCE", size: lic.len() as u64, sha256: sha(&lic) },
            files,
            boards: &["rpi5"],
            ready: true,
        };
        let served = vec![("https://example.invalid/c0ffee/LICENCE".to_string(), lic), ("https://example.invalid/c0ffee/boot/a.bin".to_string(), a), ("https://example.invalid/c0ffee/boot/b.dat".to_string(), b)];
        (pack, served)
    }

    fn scratch(tag: &str) -> PathBuf {
        let p = std::env::temp_dir().join(format!("iyakki-rpifw-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&p);
        std::fs::create_dir_all(&p).unwrap();
        p
    }

    #[test]
    fn the_pins_are_complete_and_no_pack_file_is_empty() {
        for p in &PACKS {
            assert!(p.base_url.starts_with("https://") && p.base_url.contains(p.commit) && p.commit.len() == 40, "{}: the commit is in the URL", p.id);
            for f in p.files.iter().chain(std::iter::once(&p.licence)) {
                assert!(f.size > 0 && f.sha256.len() == 64 && f.sha256.bytes().all(|b| b.is_ascii_hexdigit()), "{} {}", p.id, f.dest);
                assert!(!f.dest.starts_with('/') && !f.dest.contains(".."), "{}: destinations stay inside the pack folder", f.dest);
            }
            assert!(!p.files.is_empty() && !p.licence_summary.is_empty());
        }
        assert!(find("rpi5-wifi").unwrap().ready && !find("rpi3-boot").unwrap().ready, "only the Pi 5 pack is offered");
        assert!(find("rpi5").is_none());
    }

    #[test]
    fn every_pack_has_a_row_in_the_download_inventory_with_the_same_commit() {
        let text = std::fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join("../../legal/downloads.txt")).expect("legal/downloads.txt");
        for p in &PACKS {
            let row: Vec<&str> = text.lines().filter(|l| l.starts_with(p.id)).map(|l| l.split('|').map(str::trim).collect::<Vec<_>>()).next().unwrap_or_else(|| panic!("no row for {} in legal/downloads.txt", p.id));
            assert_eq!(row[0], p.id);
            assert_eq!(row[2], "download-only", "{}: never shipped", p.id);
            assert_eq!(row[3], "commit");
            assert_eq!(row[4], p.commit, "{}: the inventory pin is the code's pin", p.id);
        }
    }

    #[test]
    fn the_pi_5_has_no_boot_blob_pack() {
        // the Pi 5 boots from its EEPROM: no pack carries bootcode/start/fixup for it
        for p in PACKS.iter().filter(|p| p.boards.contains(&"rpi5")) {
            assert!(p.files.iter().all(|f| crate::rpiboot::forbidden([f.dest]).is_empty()), "{} must not carry a boot blob", p.id);
        }
        // and the Pi 3 pack is exactly the blob set the boot-volume check forbids on a Pi 5 volume
        let pi3 = find("rpi3-boot").unwrap();
        assert!(pi3.files.iter().all(|f| !crate::rpiboot::forbidden([f.dest]).is_empty()));
    }

    #[test]
    fn the_consent_text_names_the_files_the_licence_and_that_nothing_is_shipped() {
        let t = find("rpi5-wifi").unwrap().consent_text();
        assert!(t.contains("brcm/brcmfmac43455-sdio.bin") && t.contains("5d2295b633d94641253e16ba90fc847436fc9d4a") && t.contains("binary-redist-Cypress"));
        assert!(t.contains("does not ship these files") && t.contains("only when you accept"));
        assert!(t.contains("Not yet read by a person"), "the unread licence is said out loud");
    }

    #[test]
    fn nothing_is_downloaded_without_the_acceptance_and_a_pack_that_is_not_ready_is_refused() {
        let (pack, served) = made_up();
        let out = scratch("noaccept");
        let hits = std::cell::Cell::new(0);
        let s = serve(served);
        let counting = |u: &str| {
            hits.set(hits.get() + 1);
            s(u)
        };
        let e = fetch_pack(&counting, &pack, &out, false).unwrap_err();
        assert!(e.contains("not accepted") && e.contains("nothing was downloaded"));
        assert_eq!(hits.get(), 0, "no request is made before the acceptance");
        assert!(!out.join("test-pack").exists());
        let mut not_ready = pack;
        not_ready.ready = false;
        assert!(fetch_pack(&counting, &not_ready, &out, true).unwrap_err().contains("not offered"));
        assert_eq!(hits.get(), 0);
        let _ = std::fs::remove_dir_all(&out);
    }

    #[test]
    fn an_accepted_fetch_writes_the_licence_and_every_file_checked_against_the_pin() {
        let (pack, served) = made_up();
        let out = scratch("ok");
        let written = fetch_pack(&serve(served), &pack, &out, true).unwrap();
        assert_eq!(written.len(), 3);
        assert_eq!(std::fs::read(out.join("test-pack/LICENCE")).unwrap(), b"licence text", "the licence travels with the files");
        assert_eq!(std::fs::read(out.join("test-pack/a.bin")).unwrap(), vec![b'a'; 100]);
        assert_eq!(std::fs::read(out.join("test-pack/sub/b.dat")).unwrap().len(), 50, "a nested destination is created");
        assert!(std::fs::read_dir(out.join("test-pack")).unwrap().flatten().all(|e| e.path().extension().is_none_or(|x| x != "part")), "no .part file is left");
        let _ = std::fs::remove_dir_all(&out);
    }

    #[test]
    fn one_wrong_byte_deletes_everything_the_call_wrote() {
        let (pack, mut served) = made_up();
        served[2].1[10] ^= 1; // the last file is altered
        let out = scratch("badbyte");
        let e = fetch_pack(&serve(served), &pack, &out, true).unwrap_err();
        assert!(e.contains("sub/b.dat") && e.contains("pinned checksum"), "{e}");
        assert!(!out.join("test-pack/LICENCE").exists() && !out.join("test-pack/a.bin").exists(), "the files that were fine are removed too");
        assert!(!out.join("test-pack/sub/b.dat").exists());
        let _ = std::fs::remove_dir_all(&out);
    }

    #[test]
    fn a_longer_or_shorter_answer_a_missing_file_and_plain_http_are_all_refused() {
        let (pack, served) = made_up();
        let out = scratch("sizes");
        let mut longer = served.clone();
        longer[1].1.push(b'x');
        assert!(fetch_pack(&serve(longer), &pack, &out, true).unwrap_err().contains("101 bytes"));
        let mut shorter = served.clone();
        shorter[1].1.pop();
        assert!(fetch_pack(&serve(shorter), &pack, &out, true).unwrap_err().contains("99 bytes"));
        let missing = served[..2].to_vec();
        assert!(fetch_pack(&serve(missing), &pack, &out, true).unwrap_err().contains("HTTP 404"));
        let mut http = pack;
        http.base_url = "http://example.invalid/c0ffee/";
        assert!(fetch_pack(&serve(served), &http, &out, true).unwrap_err().contains("https"));
        assert!(!out.join("test-pack/LICENCE").exists(), "a refused first file leaves nothing");
        let _ = std::fs::remove_dir_all(&out);
    }
}
