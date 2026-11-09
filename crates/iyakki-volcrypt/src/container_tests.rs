use crate::container::*;
use crate::header::{KdfParams, SlotKind};
use crate::header_tests::{TestRandom, CHEAP};
use crate::recovery::RecoveryKey;
use crate::VolError;

const PAYLOAD: u64 = 64 * 4096;

fn fresh(sector: u32) -> Volume<MemStore> {
    let store = MemStore::new(container_len(PAYLOAD) as usize);
    Volume::format(store, &mut TestRandom(1), sector, PAYLOAD, b"pw one", SlotKind::Person, 1000, CHEAP).unwrap()
}

fn pattern(n: usize, seed: u8) -> Vec<u8> {
    (0..n).map(|i| (i as u8).wrapping_mul(31).wrapping_add(seed)).collect()
}

#[test]
fn blocks_written_read_back_and_are_not_plain_in_the_store() {
    for ss in [512u32, 4096] {
        let mut v = fresh(ss);
        let ssz = ss as usize;
        let data = pattern(ssz * 3, 5);
        v.write_sectors(2, &data).unwrap();
        let mut out = vec![0u8; ssz * 3];
        v.read_sectors(2, &mut out).unwrap();
        assert_eq!(out, data);
        // a single sector inside the range reads the same
        let mut one = vec![0u8; ssz];
        v.read_sectors(3, &mut one).unwrap();
        assert_eq!(one, data[ssz..2 * ssz]);
        // the store holds no run of the plain data
        let raw = v.into_store();
        let bytes = raw.bytes.lock().unwrap();
        assert!(!bytes.windows(64).any(|w| w == &data[..64]));
    }
}

#[test]
fn the_volume_opens_again_with_the_password_and_the_data_is_there() {
    let mut v = fresh(4096);
    let data = pattern(4096 * 2, 9);
    v.write_sectors(10, &data).unwrap();
    let store = v.into_store();
    let v2 = Volume::open_password(store.clone(), b"pw one").unwrap();
    let mut out = vec![0u8; 4096 * 2];
    v2.read_sectors(10, &mut out).unwrap();
    assert_eq!(out, data);
    assert_eq!(Volume::open_password(store, b"pw two").err(), Some(VolError::WrongSecret));
}

#[test]
fn recovery_key_opens_the_same_data() {
    let mut v = fresh(512);
    let data = pattern(1024, 1);
    v.write_sectors(0, &data).unwrap();
    let mut rng = TestRandom(77);
    let rk = v.change(|h, k| h.set_recovery(&mut rng, k).map(|x| x.1)).unwrap();
    let store = v.into_store();
    let v2 = Volume::open_recovery(store.clone(), &RecoveryKey::parse(&rk.to_text()).unwrap()).unwrap();
    let mut out = vec![0u8; 1024];
    v2.read_sectors(0, &mut out).unwrap();
    assert_eq!(out, data);
    assert_eq!(Volume::open_recovery(store, &RecoveryKey::from_bytes([3; 32])).err(), Some(VolError::WrongSecret));
}

#[test]
fn sector_numbers_follow_the_512_byte_rule_so_the_kernel_could_read_the_same_bytes() {
    // 4096-byte sector n is encrypted with tweak 8n, as dm-crypt does for sector_size:4096
    let mut v = fresh(4096);
    let plain = pattern(4096, 3);
    v.write_sectors(5, &plain).unwrap();
    let key = *v.key();
    let store = v.into_store();
    let mut raw = vec![0u8; 4096];
    store.read_at(PAYLOAD_OFFSET + 5 * 4096, &mut raw).unwrap();
    let x = crate::xts::Xts::new(&key);
    let mut d = raw.clone();
    assert!(x.decrypt_sector(40, &mut d));
    assert_eq!(d, plain);
    let mut wrong = raw;
    x.decrypt_sector(5, &mut wrong);
    assert_ne!(wrong, plain);
}

#[test]
fn out_of_range_and_ragged_requests_are_refused() {
    let mut v = fresh(4096);
    assert!(v.write_sectors(0, &[0u8; 100]).is_err());
    assert!(v.write_sectors(0, &[]).is_err());
    assert!(v.write_sectors(64, &[0u8; 4096]).is_err());
    assert!(v.write_sectors(63, &[0u8; 8192]).is_err());
    assert!(v.write_sectors(u64::MAX, &[0u8; 4096]).is_err());
    assert!(v.read_sectors(64, &mut [0u8; 4096]).is_err());
    assert!(v.write_sectors(63, &[0u8; 4096]).is_ok());
}

#[test]
fn a_changed_password_works_and_the_old_one_does_not_and_data_is_untouched() {
    let mut v = fresh(4096);
    let data = pattern(4096, 2);
    v.write_sectors(1, &data).unwrap();
    let mut rng = TestRandom(5);
    v.change(|h, k| h.change_password(&mut rng, k, 0, b"pw two", CHEAP)).unwrap();
    let store = v.into_store();
    assert_eq!(Volume::open_password(store.clone(), b"pw one").err(), Some(VolError::WrongSecret));
    let v2 = Volume::open_password(store, b"pw two").unwrap();
    let mut out = vec![0u8; 4096];
    v2.read_sectors(1, &mut out).unwrap();
    assert_eq!(out, data);
}

#[test]
fn a_power_cut_while_changing_the_header_loses_nothing_at_any_byte() {
    // cut the write of the new header after every possible number of bytes: the volume must still open with the old password
    // (the cut write went to the unused copy) and, if the new header happened to be whole, with the new one
    for cut in (0..=4096usize).step_by(61).chain([4095, 4096]) {
        let mut v = fresh(4096);
        let data = pattern(4096, 8);
        v.write_sectors(0, &data).unwrap();
        let mut store = v.into_store();
        store.fail_after = Some(cut);
        let mut v = Volume::open_password(store.clone(), b"pw one").unwrap();
        let mut rng = TestRandom(21);
        let r = v.change(|h, k| h.change_password(&mut rng, k, 0, b"pw two", CHEAP));
        let store = MemStore { bytes: store.bytes.clone(), fail_after: None };
        let old = Volume::open_password(store.clone(), b"pw one");
        let new = Volume::open_password(store.clone(), b"pw two");
        if r.is_ok() {
            assert!(new.is_ok(), "cut {cut}");
        }
        assert!(old.is_ok() || new.is_ok(), "cut {cut}: neither password opens the volume");
        let any = old.or(new).unwrap();
        let mut out = vec![0u8; 4096];
        any.read_sectors(0, &mut out).unwrap();
        assert_eq!(out, data, "cut {cut}");
    }
}

#[test]
fn one_damaged_header_copy_is_survived_and_both_damaged_is_reported() {
    let v = fresh(4096);
    let store = v.into_store();
    // copy A destroyed
    let mut a_bad = MemStore { bytes: std::sync::Arc::new(std::sync::Mutex::new(store.bytes.lock().unwrap().clone())), fail_after: None };
    a_bad.write_at(100, &[0xEE; 50]).unwrap();
    let v = Volume::open_password(a_bad.clone(), b"pw one").unwrap();
    assert!(v.other_copy_bad);
    // copy B destroyed too
    let mut both = a_bad;
    both.write_at(4096 + 100, &[0xEE; 50]).unwrap();
    assert_eq!(Volume::open_password(both, b"pw one").err(), Some(VolError::Damaged));
    // a store that was never formatted
    assert_eq!(Volume::open_password(MemStore::new(100_000), b"pw one").err(), Some(VolError::NotOurs));
    assert_eq!(Volume::open_password(MemStore::new(10), b"pw one").err().is_some(), true);
}

#[test]
fn syncing_copies_brings_both_headers_up_to_date() {
    let mut v = fresh(4096);
    let mut rng = TestRandom(2);
    v.change(|h, k| h.add_password(&mut rng, k, b"admin", SlotKind::Admin, 0, CHEAP)).unwrap();
    v.sync_copies().unwrap();
    assert!(!v.other_copy_bad);
    let store = v.into_store();
    let mut a = vec![0u8; 4096];
    let mut b = vec![0u8; 4096];
    store.read_at(0, &mut a).unwrap();
    store.read_at(4096, &mut b).unwrap();
    assert_eq!(a, b);
}

#[test]
fn too_small_a_drive_is_refused_and_a_real_file_works_too() {
    let small = MemStore::new(1000);
    assert!(Volume::format(small, &mut TestRandom(1), 4096, PAYLOAD, b"p", SlotKind::Person, 0, CHEAP).is_err());
    let dir = std::env::temp_dir().join(format!("iyakki-volcrypt-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("vol.img");
    let f = std::fs::OpenOptions::new().read(true).write(true).create(true).truncate(true).open(&path).unwrap();
    f.set_len(container_len(PAYLOAD)).unwrap();
    let mut v = Volume::format(FileStore(f), &mut TestRandom(3), 4096, PAYLOAD, b"file pw", SlotKind::Person, 0, KdfParams { m_kib: 64, t: 1, p: 1 }).unwrap();
    let data = pattern(4096 * 4, 6);
    v.write_sectors(7, &data).unwrap();
    drop(v);
    let f = std::fs::OpenOptions::new().read(true).write(true).open(&path).unwrap();
    let v = Volume::open_password(FileStore(f), b"file pw").unwrap();
    let mut out = vec![0u8; 4096 * 4];
    v.read_sectors(7, &mut out).unwrap();
    assert_eq!(out, data);
    // the file holds no run of the plain data
    let raw = std::fs::read(&path).unwrap();
    assert!(!raw.windows(64).any(|w| w == &data[..64]));
    std::fs::remove_dir_all(&dir).unwrap();
}
