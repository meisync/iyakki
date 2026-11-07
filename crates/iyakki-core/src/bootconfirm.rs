//! Telling IYAKKI's boot loader that a start worked.
//!
//! The loader (`iyakki-boot`, docs/BOOT-LOADER.md) counts starts nobody confirmed; after too many in a row it opens its menu and
//! waits for a person instead of starting by itself. The running system confirms a start by writing one UEFI variable,
//! `BootConfirm` under the loader's vendor GUID, once the sign-in screen is on the display. The loader reads it at the next start,
//! refills its tries and deletes it. Nothing else is shared: the loader's own counter format stays inside the loader.
//!
//! The write goes through a tiny trait so the logic is tested without a firmware; the real implementation uses `efivarfs`.

use std::io;
use std::path::{Path, PathBuf};

/// The loader's vendor GUID (the loader's `vars::VENDOR`).
pub const LOADER_VENDOR: &str = "7b5a3c1e-2d84-4f6b-9a0e-5c1f8d3b2a47";
/// Variable name the loader reads.
pub const CONFIRM_NAME: &str = "BootConfirm";
/// What is written (the loader only checks that the variable exists).
pub const CONFIRM_VALUE: &[u8] = b"ok";
/// Attributes: non-volatile, boot-service and runtime access (the same as the loader's own variables).
pub const ATTRIBUTES: u32 = 0x7;

/// Where a variable is written.
pub trait VariableSink {
    /// Is this a machine started through UEFI (variables exist)?
    fn available(&self) -> bool;
    /// Write (create or replace) a variable.
    fn write(&self, vendor: &str, name: &str, attributes: u32, data: &[u8]) -> io::Result<()>;
}

/// What happened.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Confirmed {
    /// The variable was written.
    Written,
    /// Not a UEFI start (BIOS, a container, a test): nothing to do.
    NotUefi,
    /// The write failed (the loader keeps counting; the only harm is a menu after many starts in a row).
    Failed(String),
}

/// Confirm the start that is running now. Call it once the sign-in screen has been drawn.
pub fn confirm_good_start(sink: &dyn VariableSink) -> Confirmed {
    if !sink.available() {
        return Confirmed::NotUefi;
    }
    match sink.write(LOADER_VENDOR, CONFIRM_NAME, ATTRIBUTES, CONFIRM_VALUE) {
        Ok(()) => Confirmed::Written,
        Err(e) => Confirmed::Failed(e.to_string()),
    }
}

/// The real variables: `/sys/firmware/efi/efivars/NAME-GUID` (4 bytes of attributes, then the data).
pub struct Efivarfs {
    root: PathBuf,
}

impl Efivarfs {
    /// The system's efivarfs.
    pub fn system() -> Efivarfs {
        Efivarfs { root: PathBuf::from("/sys/firmware/efi/efivars") }
    }

    /// Another directory (tests).
    pub fn at(root: &Path) -> Efivarfs {
        Efivarfs { root: root.to_path_buf() }
    }
}

impl VariableSink for Efivarfs {
    fn available(&self) -> bool {
        self.root.is_dir()
    }

    fn write(&self, vendor: &str, name: &str, attributes: u32, data: &[u8]) -> io::Result<()> {
        let path = self.root.join(format!("{name}-{vendor}"));
        // efivarfs marks an existing variable immutable; a write needs the flag cleared (best effort: a plain directory has none)
        #[cfg(target_os = "linux")]
        if path.exists() {
            let _ = std::process::Command::new("chattr").arg("-i").arg(&path).stderr(std::process::Stdio::null()).status();
        }
        let mut bytes = attributes.to_le_bytes().to_vec();
        bytes.extend_from_slice(data);
        std::fs::write(&path, bytes)
    }
}

/// Confirm in the background a moment after the shell started drawing: the sign-in screen is the first thing it draws, and the
/// write must never slow it down. The delay is the time one frame needs plus a margin.
pub fn confirm_soon(delay: std::time::Duration) {
    let spawn = std::thread::Builder::new().name("boot-confirm".into()).spawn(move || {
        std::thread::sleep(delay);
        match confirm_good_start(&Efivarfs::system()) {
            Confirmed::Written => log::info!("boot: the loader was told this start worked"),
            Confirmed::NotUefi => log::debug!("boot: not a UEFI start, nothing to confirm"),
            Confirmed::Failed(e) => log::warn!("boot: could not tell the loader this start worked: {e}"),
        }
    });
    if let Err(e) = spawn {
        log::warn!("boot: could not start the confirm thread: {e}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;

    struct Fake {
        up: bool,
        fail: bool,
        seen: RefCell<Vec<(String, String, u32, Vec<u8>)>>,
    }

    impl VariableSink for Fake {
        fn available(&self) -> bool {
            self.up
        }
        fn write(&self, v: &str, n: &str, a: u32, d: &[u8]) -> io::Result<()> {
            if self.fail {
                return Err(io::Error::other("read-only"));
            }
            self.seen.borrow_mut().push((v.into(), n.into(), a, d.to_vec()));
            Ok(())
        }
    }

    #[test]
    fn writes_the_variable_the_loader_reads() {
        let f = Fake { up: true, fail: false, seen: Default::default() };
        assert_eq!(confirm_good_start(&f), Confirmed::Written);
        let s = f.seen.borrow();
        assert_eq!(s.len(), 1);
        assert_eq!(s[0], (LOADER_VENDOR.into(), "BootConfirm".into(), 7, b"ok".to_vec()));
    }

    #[test]
    fn no_uefi_no_write_and_a_failure_is_reported_not_fatal() {
        let f = Fake { up: false, fail: false, seen: Default::default() };
        assert_eq!(confirm_good_start(&f), Confirmed::NotUefi);
        assert!(f.seen.borrow().is_empty());
        let f = Fake { up: true, fail: true, seen: Default::default() };
        assert!(matches!(confirm_good_start(&f), Confirmed::Failed(m) if m.contains("read-only")));
    }

    #[test]
    fn efivarfs_file_is_attributes_then_data() {
        let d = std::env::temp_dir().join(format!("iyakki-efivars-{}", std::process::id()));
        std::fs::create_dir_all(&d).unwrap();
        assert_eq!(confirm_good_start(&Efivarfs::at(&d)), Confirmed::Written);
        let b = std::fs::read(d.join(format!("BootConfirm-{LOADER_VENDOR}"))).unwrap();
        assert_eq!(b, [7, 0, 0, 0, b'o', b'k']);
        std::fs::remove_dir_all(&d).unwrap();
        assert_eq!(confirm_good_start(&Efivarfs::at(&d)), Confirmed::NotUefi);
    }

    #[test]
    fn the_guid_matches_the_loader() {
        let src = std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/../../iyakki-boot/crates/boot-uefi/src/vars.rs")).unwrap();
        let g = LOADER_VENDOR.replace('-', "");
        // Guid::new(0x7b5a3c1e, 0x2d84, 0x4f6b, [0x9a, 0x0e, 0x5c, 0x1f, 0x8d, 0x3b, 0x2a, 0x47])
        for part in ["0x7b5a3c1e", "0x2d84", "0x4f6b", "0x9a, 0x0e, 0x5c, 0x1f, 0x8d, 0x3b, 0x2a, 0x47"] {
            assert!(src.contains(part), "{part}");
        }
        assert_eq!(g.len(), 32);
        assert!(src.contains("\"BootConfirm\""));
    }
}
