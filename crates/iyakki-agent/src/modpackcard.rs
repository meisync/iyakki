//! The optional kernel-module pack as a card the person approves (F-PKG-11): which rarely used drivers this computer's devices call for
//! and that the image left out, from where they would be downloaded, and the install itself.
//!
//! Fails closed: no address is built in (D-MODPACK-URL), so without an address the card says so and nothing is fetched. The address
//! comes from the person's call (shown on the card, https only) or from the file they set earlier. Whatever the address serves, every
//! file is checked against the manifest that is inside the image (size and SHA-256) before the first byte is written, so a wrong or
//! hostile server can at most refuse to serve.

use std::path::{Path, PathBuf};

use iyakki_core_api::drivers::modpack::{self, Depmod};

/// What the card and the install need from the machine (a fake in tests).
pub trait Machine {
    /// The `lib/modules/RELEASE` folder.
    fn root(&self) -> PathBuf;
    /// The device alias strings of the devices now present.
    fn modaliases(&self) -> Vec<String>;
    /// The address the person set earlier, if any.
    fn saved_address(&self) -> Option<String>;
    fn save_address(&self, address: &str) -> Result<(), String>;
    /// Download one file (`want` bytes expected).
    fn fetch(&self, url: &str, want: u64) -> Result<Vec<u8>, String>;
    fn depmod(&self) -> &dyn Depmod;
}

fn address(m: &dyn Machine, given: Option<&str>) -> Result<String, String> {
    let a = given.map(str::to_string).or_else(|| m.saved_address()).ok_or(
        "No download address is set for the optional drivers, so nothing can be fetched. Give the address (https) of the pack to the tool, or set it in Settings.",
    )?;
    modpack::module_url(&a, "kernel/x.ko").map(|_| a)
}

/// The Store row: `None` when this image has no optional pack, else a state word (what is waiting for this computer).
pub fn store_state(m: &dyn Machine) -> Option<String> {
    let root = m.root();
    let (manifest, map) = modpack::read_pack(&root).ok()??;
    let n = modpack::needed(&manifest, &map, &m.modaliases(), &|p| root.join(p).exists()).len();
    Some(if n == 0 { "nothing needed".to_string() } else { format!("{n} driver(s) available for this computer") })
}

/// The card: lines for the person and the id of exactly this list.
pub fn card(m: &dyn Machine, given: Option<&str>) -> Result<(Vec<String>, String), String> {
    let root = m.root();
    let (manifest, map) = modpack::read_pack(&root)?.ok_or("This image has no optional driver pack: every driver is already in it.")?;
    let wanted = modpack::needed(&manifest, &map, &m.modaliases(), &|p| root.join(p).exists());
    if wanted.is_empty() {
        return Err("No optional driver is needed for the devices of this computer.".into());
    }
    let addr = address(m, given)?;
    let bytes: u64 = wanted.iter().filter_map(|p| manifest.modules.get(p)).map(|i| i.size).sum();
    let id = modpack::list_id(&manifest, &wanted);
    let mut lines = vec![format!("Install {} optional driver module(s), {} KB, downloaded from {addr} on approval.", wanted.len(), bytes.div_ceil(1024))];
    lines.push("Every file is checked against the checksums inside this image before it is written; a file that does not match is never written.".into());
    lines.extend(wanted.iter().map(|p| format!("  {p}")));
    lines.push(format!("List id {}. If the list changes, you are asked again.", &id[..16]));
    Ok((lines, id))
}

/// After the person approved the card with list id `list`.
pub fn install(m: &dyn Machine, given: Option<&str>, list: &str) -> Result<String, String> {
    let root = m.root();
    let (manifest, map) = modpack::read_pack(&root)?.ok_or("This image has no optional driver pack.")?;
    let wanted = modpack::needed(&manifest, &map, &m.modaliases(), &|p| root.join(p).exists());
    if modpack::list_id(&manifest, &wanted) != list {
        return Err("the list of optional drivers changed since the card was shown, so nothing was fetched; ask again to see the new list".into());
    }
    let addr = address(m, given)?;
    let sizes: std::collections::BTreeMap<&str, u64> = manifest.modules.iter().map(|(p, i)| (p.as_str(), i.size)).collect();
    let fetch = |p: &str| -> Result<Vec<u8>, String> { m.fetch(&modpack::module_url(&addr, p)?, sizes.get(p).copied().unwrap_or(0)) };
    let done = modpack::install(&root, &manifest, &wanted, &fetch, m.depmod())?;
    m.save_address(&addr)?;
    Ok(format!("Installed {} optional driver module(s). A restart or a replug may be needed for a device to pick its driver up.", done.written.len()))
}

/// The real machine: sysfs, the running kernel's module folder, the person's file for the address, the proxy streamer for the download.
pub struct RealMachine {
    pub data: PathBuf,
    pub streamer: std::sync::Arc<dyn iyakki_proxy::Streamer>,
}

impl RealMachine {
    fn url_file(&self) -> PathBuf {
        self.data.join("optional-pack-url")
    }
}

impl Machine for RealMachine {
    fn root(&self) -> PathBuf {
        let rel = iyakki_core_local::kernelfacts::read().osrelease.unwrap_or_default();
        Path::new("/lib/modules").join(rel.trim())
    }
    fn modaliases(&self) -> Vec<String> {
        iyakki_core_local::kernelfacts::modaliases_in(Path::new("/sys"))
    }
    fn saved_address(&self) -> Option<String> {
        std::fs::read_to_string(self.url_file()).ok().map(|s| s.trim().to_string()).filter(|s| !s.is_empty())
    }
    fn save_address(&self, address: &str) -> Result<(), String> {
        std::fs::write(self.url_file(), format!("{address}\n")).map_err(|e| format!("saving the address: {e}"))
    }
    fn fetch(&self, url: &str, want: u64) -> Result<Vec<u8>, String> {
        let tmp = self.data.join("optional-pack-download.part");
        let _ = std::fs::remove_file(&tmp);
        crate::fwpack::fetch_resume(&*self.streamer, url, want, &tmp, 3, std::time::Duration::from_secs(3), &mut |_| {}, &|| false)?;
        let b = std::fs::read(&tmp).map_err(|e| e.to_string());
        let _ = std::fs::remove_file(&tmp);
        b
    }
    fn depmod(&self) -> &dyn Depmod {
        &iyakki_core_local::LocalDepmod
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use sha2::{Digest, Sha256};
    use std::cell::RefCell;

    struct Fake {
        root: PathBuf,
        saved: RefCell<Option<String>>,
        asked: RefCell<Vec<String>>,
        body: Vec<u8>,
        dm: Dm,
    }
    struct Dm;
    impl Depmod for Dm {
        fn run(&self, _: &Path, _: &str) -> Result<(), String> {
            Ok(())
        }
    }
    impl Machine for Fake {
        fn root(&self) -> PathBuf {
            self.root.clone()
        }
        fn modaliases(&self) -> Vec<String> {
            vec!["usb:v1234p5678d0001".into()]
        }
        fn saved_address(&self) -> Option<String> {
            self.saved.borrow().clone()
        }
        fn save_address(&self, a: &str) -> Result<(), String> {
            *self.saved.borrow_mut() = Some(a.into());
            Ok(())
        }
        fn fetch(&self, url: &str, _: u64) -> Result<Vec<u8>, String> {
            self.asked.borrow_mut().push(url.into());
            Ok(self.body.clone())
        }
        fn depmod(&self) -> &dyn Depmod {
            &self.dm
        }
    }

    fn machine(tag: &str, body: &[u8]) -> Fake {
        let root = std::env::temp_dir().join(format!("iyakki-modcard-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("optional-pack")).unwrap();
        let sum: String = Sha256::digest(b"AAAA").iter().map(|x| format!("{x:02x}")).collect();
        std::fs::write(root.join("optional-pack/manifest.txt"), format!("release 6.1.0\nmodule 4 {sum} kernel/drivers/dvbu.ko\n")).unwrap();
        std::fs::write(root.join("optional-pack/optional.map"), "alias usb:v1234p5678* kernel/drivers/dvbu.ko\n").unwrap();
        Fake { root, saved: RefCell::new(None), asked: RefCell::new(vec![]), body: body.to_vec(), dm: Dm }
    }

    #[test]
    fn without_an_address_the_card_refuses_and_nothing_is_fetched() {
        let m = machine("noaddr", b"AAAA");
        let e = card(&m, None).unwrap_err();
        assert!(e.contains("No download address"), "{e}");
        assert!(card(&m, Some("http://plain.example.org/")).is_err());
        assert!(m.asked.borrow().is_empty());
    }

    #[test]
    fn the_card_lists_the_modules_and_the_install_writes_them_only_for_that_list() {
        let m = machine("ok", b"AAAA");
        let (lines, id) = card(&m, Some("https://packs.example.org/p")).unwrap();
        assert!(lines[0].contains("1 optional driver module") && lines.iter().any(|l| l.contains("dvbu.ko")), "{lines:?}");
        assert!(install(&m, Some("https://packs.example.org/p"), "wrong").unwrap_err().contains("changed"));
        assert!(m.asked.borrow().is_empty() && !m.root.join("kernel/drivers/dvbu.ko").exists());
        let said = install(&m, Some("https://packs.example.org/p"), &id).unwrap();
        assert!(said.starts_with("Installed 1"), "{said}");
        assert_eq!(std::fs::read(m.root.join("kernel/drivers/dvbu.ko")).unwrap(), b"AAAA");
        assert_eq!(m.asked.borrow().as_slice(), ["https://packs.example.org/p/kernel/drivers/dvbu.ko"]);
        assert_eq!(m.saved.borrow().as_deref(), Some("https://packs.example.org/p"));
    }

    #[test]
    fn a_server_that_sends_other_bytes_changes_nothing() {
        let m = machine("bad", b"EVIL");
        let (_, id) = card(&m, Some("https://packs.example.org/p")).unwrap();
        let e = install(&m, Some("https://packs.example.org/p"), &id).unwrap_err();
        assert!(e.contains("checksum"), "{e}");
        assert!(!m.root.join("kernel/drivers/dvbu.ko").exists() && m.saved.borrow().is_none());
    }

    #[test]
    fn the_store_row_exists_only_with_a_pack_and_counts_what_waits() {
        let m = machine("store", b"AAAA");
        assert_eq!(store_state(&m).unwrap(), "1 driver(s) available for this computer");
        std::fs::remove_dir_all(m.root.join("optional-pack")).unwrap();
        assert!(store_state(&m).is_none());
    }

    #[test]
    fn an_image_without_a_pack_or_with_nothing_needed_says_so() {
        let m = machine("none", b"");
        std::fs::remove_dir_all(m.root.join("optional-pack")).unwrap();
        assert!(card(&m, Some("https://x.org")).unwrap_err().contains("no optional driver pack"));
    }
}
