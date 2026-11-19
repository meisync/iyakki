//! Unlocking LUKS volumes made by real `cryptsetup` (the fixtures of `iyakki-volcrypt`) and handing the key to a recording kernel: the
//! table that reaches device-mapper, the read-only flag, a wrong passphrase touching nothing, and what is refused.

use std::path::Path;
use std::sync::Mutex;

use super::*;
use iyakki_storage::dm::{decode_table_load, DmCmd, DmKernel, TableLoad, DM_READONLY_FLAG};

const ARGON: &[u8] = include_bytes!("../../iyakki-volcrypt/tests/fixtures/luks2-argon2id-aes-xts-512.img");
const PBKDF2: &[u8] = include_bytes!("../../iyakki-volcrypt/tests/fixtures/luks2-pbkdf2-sha512-aes-xts-256.bin");
const MASTER512: &str = "f7f46d429fa5e2edff5c38d59e55fb60d738b8fc7636f861e5528e8aaaa66bc799637dc13407f0a34ae121cf82a10e927a4a278f7fdcb29a0b28b245f489289a";
const MASTER256: &str = "b69a244b4001ebf70ac43c534c2435f8a2b187d4286e61a67a50c0d802160450";

/// The first bytes of a real image, and a length as if the rest was there.
struct Mem(&'static [u8], u64);

impl Device for Mem {
    fn read_at(&self, off: u64, len: usize) -> Option<Vec<u8>> {
        self.0.get(off as usize..off as usize + len).map(<[u8]>::to_vec)
    }
    fn len_bytes(&self) -> Option<u64> {
        Some(self.1)
    }
}

#[derive(Default)]
struct Rec {
    calls: Mutex<Vec<String>>,
    tables: Mutex<Vec<TableLoad>>,
    fail_load: bool,
}

impl DmKernel for Rec {
    fn dm_ioctl(&self, cmd: DmCmd, buf: &mut Vec<u8>) -> std::io::Result<()> {
        self.calls.lock().unwrap().push(format!("{cmd:?}"));
        match cmd {
            DmCmd::TableLoad => {
                if self.fail_load {
                    return Err(std::io::Error::from_raw_os_error(libc::EINVAL));
                }
                self.tables.lock().unwrap().push(decode_table_load(buf).unwrap());
                Ok(())
            }
            DmCmd::DevStatus => Err(std::io::Error::from_raw_os_error(libc::ENXIO)),
            _ => Ok(()),
        }
    }
    fn loop_attach(&self, _: &Path) -> std::io::Result<String> {
        Err(std::io::ErrorKind::Unsupported.into())
    }
    fn loop_detach(&self, _: &str) -> std::io::Result<()> {
        Ok(())
    }
    fn make_fs(&self, _: &str, _: &str) -> std::io::Result<()> {
        Ok(())
    }
    fn mount(&self, _: &str, _: &Path) -> std::io::Result<()> {
        Ok(())
    }
    fn umount(&self, _: &Path) -> std::io::Result<()> {
        Ok(())
    }
    fn is_mounted(&self, _: &Path) -> bool {
        false
    }
}

#[test]
fn a_real_luks2_volume_is_opened_by_our_code_and_the_master_key_goes_to_device_mapper_only() {
    let k = Rec::default();
    open_luks(&k, &Mem(ARGON, 64 << 20), "/dev/sdb1", "iyakki-sdb1", b"correct horse battery", false).unwrap();
    assert_eq!(*k.calls.lock().unwrap(), ["DevCreate", "TableLoad", "DevSuspend"]);
    let t = &k.tables.lock().unwrap()[0];
    assert_eq!((t.name.as_str(), t.target_type.as_str()), ("iyakki-sdb1", "crypt"));
    assert_eq!(t.params, format!("aes-xts-plain64 {MASTER512} 0 /dev/sdb1 32768 1 sector_size:4096"), "the data starts at 16 MiB, in 4096-byte sectors");
    assert_eq!(t.sectors, (64 << 20) / 512 - 32768, "the rest of the drive after the header");
    assert_eq!(t.flags & DM_READONLY_FLAG, 0);
    assert!(t.flags & iyakki_storage::dm::DM_SECURE_DATA_FLAG != 0, "the kernel wipes its copy of the key");
}

#[test]
fn a_256_bit_key_volume_and_a_read_only_open() {
    let k = Rec::default();
    open_luks(&k, &Mem(PBKDF2, 32 << 20), "/dev/sdc2", "iyakki-sdc2", b"pbkdf2 only pass", true).unwrap();
    let t = &k.tables.lock().unwrap()[0];
    assert!(t.params.starts_with(&format!("aes-xts-plain64 {MASTER256} 0 /dev/sdc2 ")), "{}", t.params);
    assert!(t.flags & DM_READONLY_FLAG != 0, "the kernel refuses every write");
}

#[test]
fn a_wrong_passphrase_touches_nothing_and_a_refused_table_leaves_nothing_behind() {
    let k = Rec::default();
    assert_eq!(open_luks(&k, &Mem(ARGON, 64 << 20), "/dev/sdb1", "iyakki-sdb1", b"wrong", false), Err(DmOpenError::WrongKey));
    assert!(k.calls.lock().unwrap().is_empty(), "no mapping was made");
    let k = Rec { fail_load: true, ..Rec::default() };
    let e = open_luks(&k, &Mem(ARGON, 64 << 20), "/dev/sdb1", "iyakki-sdb1", b"correct horse battery", false).unwrap_err();
    assert!(matches!(e, DmOpenError::Failed(m) if m.contains("kernel refused")));
    assert_eq!(*k.calls.lock().unwrap(), ["DevCreate", "TableLoad", "DevRemove"], "the half-made mapping is removed");
}

#[test]
fn volumes_that_are_not_ours_to_open_are_said_in_plain_words() {
    let k = Rec::default();
    assert!(matches!(open_luks(&k, &Mem(&[0u8; 8192], 1 << 20), "/dev/sdb1", "iyakki-sdb1", b"x", false), Err(DmOpenError::Unsupported(_))));
    assert!(matches!(open_luks(&k, &Mem(&ARGON[..2000], 1 << 20), "/dev/sdb1", "iyakki-sdb1", b"x", false), Err(DmOpenError::Failed(_))));
    // the data segment lies beyond the drive
    assert!(matches!(open_luks(&k, &Mem(ARGON, 1 << 20), "/dev/sdb1", "iyakki-sdb1", b"correct horse battery", false), Err(DmOpenError::Failed(_))));
    // a name the kernel layer does not allow
    assert!(open_luks(&k, &Mem(ARGON, 64 << 20), "/dev/sdb1", "Bad Name", b"correct horse battery", false).is_err());
    assert!(k.calls.lock().unwrap().is_empty());
    // closing something that is not there says so; is_open asks the kernel
    assert_eq!(close(&k, "iyakki-sdb1"), Ok(false));
    assert!(!is_open(&k, "iyakki-sdb1"));
}

#[test]
fn the_runner_opens_and_closes_a_luks_drive_without_cryptsetup_and_falls_back_only_for_what_it_cannot_read() {
    use crate::disks_crypt::CryptKind;
    use crate::disks_helper::{CryptRunner, OpenError, OpenReq, ProcRunner};
    use std::sync::Arc;
    // a machine with no helper programs at all
    let sys = crate::disks_helper::FakeSys::with_programs(&[]);
    let k = Arc::new(Rec::default());
    let r = ProcRunner::new(sys.clone(), "/nonexistent/bin", "/tmp/none").with_dm(k.clone(), Arc::new(|_| Box::new(Mem(ARGON, 64 << 20)) as Box<dyn Device>));
    assert!(r.available(CryptKind::Luks2).is_ok(), "no download needed for a LUKS drive");
    assert!(r.available(CryptKind::BitLocker).is_err(), "BitLocker still needs its helper");
    let req = OpenReq { kind: CryptKind::Luks2, dev: "/dev/sdb1", part: "sdb1", read_only: false };
    let secret = crate::disks_crypt::Secret::new("wrong");
    assert_eq!(r.open(&req, &secret), Err(OpenError::WrongKey));
    let secret = crate::disks_crypt::Secret::new("correct horse battery");
    r.open(&req, &secret).unwrap();
    assert!(k.tables.lock().unwrap()[0].params.contains("/dev/sdb1"));
    assert!(sys.calls().is_empty(), "no program was run");
}
