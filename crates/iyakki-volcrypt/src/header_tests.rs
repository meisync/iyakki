use sha2::{Digest, Sha256};

use crate::header::*;
use crate::recovery::RecoveryKey;
use crate::{Random, VolError};

/// Repeatable "random" bytes for tests only.
pub struct TestRandom(pub u64);
impl Random for TestRandom {
    fn fill(&mut self, buf: &mut [u8]) -> Result<(), VolError> {
        for chunk in buf.chunks_mut(32) {
            self.0 += 1;
            let h = Sha256::digest(self.0.to_le_bytes());
            chunk.copy_from_slice(&h[..chunk.len()]);
        }
        Ok(())
    }
}

pub const CHEAP: KdfParams = KdfParams { m_kib: 64, t: 1, p: 1 };

fn new_header() -> (Header, [u8; KEY_LEN]) {
    let (h, k) = Header::create(&mut TestRandom(1), &NewVolume { sector_size: 4096, payload_bytes: 1 << 20, payload_offset: 8192 }, b"correct horse", SlotKind::Person, 1000, CHEAP).unwrap();
    (h, *k)
}

#[test]
fn a_header_is_4096_bytes_and_reads_back_identically() {
    let (h, _) = new_header();
    let b = h.to_bytes();
    assert_eq!(b.len(), HEADER_LEN);
    assert_eq!(&b[..8], b"IYAKVOL1");
    assert_eq!(Header::parse(&b).unwrap(), h);
}

#[test]
fn the_right_password_opens_and_a_wrong_one_does_not() {
    let (h, k) = new_header();
    let u = h.unlock_password(b"correct horse").unwrap();
    assert_eq!((*u.key, u.slot), (k, 0));
    assert_eq!(h.unlock_password(b"correct horsf").err(), Some(VolError::WrongSecret));
    assert_eq!(h.unlock_password(b"").err(), Some(VolError::WrongSecret));
    assert_eq!(h.unlock_recovery(&RecoveryKey::from_bytes([1; 32])).err(), Some(VolError::WrongSecret));
}

#[test]
fn right_and_wrong_secrets_cost_the_same_number_of_slow_hashes() {
    let (mut h, k) = new_header();
    let mut rng = TestRandom(50);
    h.add_password(&mut rng, &k, b"second", SlotKind::Admin, 0, CHEAP).unwrap();
    h.add_password(&mut rng, &k, b"third", SlotKind::Person, 0, CHEAP).unwrap();
    let runs = |pw: &[u8]| {
        let before = kdf_runs();
        let _ = h.unlock_password(pw);
        kdf_runs() - before
    };
    // all three slots are checked whichever one matches, and when none does
    assert_eq!(runs(b"correct horse"), 3);
    assert_eq!(runs(b"second"), 3);
    assert_eq!(runs(b"third"), 3);
    assert_eq!(runs(b"nope"), 3);
}

#[test]
fn recovery_key_opens_and_a_new_one_replaces_the_old() {
    let (mut h, k) = new_header();
    let mut rng = TestRandom(7);
    let (i, rk) = h.set_recovery(&mut rng, &k).unwrap();
    assert_eq!(h.unlock_recovery(&rk).unwrap().slot, i);
    assert_eq!(*h.unlock_recovery(&rk).unwrap().key, k);
    // the printed text works too
    let typed = RecoveryKey::parse(&rk.to_text()).unwrap();
    assert!(h.unlock_recovery(&typed).is_ok());
    // a password is not a recovery key and the other way round
    assert!(h.unlock_password(rk.to_text().as_bytes()).is_err());
    let (j, rk2) = h.set_recovery(&mut rng, &k).unwrap();
    assert_eq!(i, j);
    assert_eq!(h.list().iter().filter(|s| s.1 == SlotKind::Recovery).count(), 1);
    assert!(h.unlock_recovery(&rk).is_err());
    assert!(h.unlock_recovery(&rk2).is_ok());
}

#[test]
fn slots_can_be_added_changed_and_removed_and_the_last_one_stays() {
    let (mut h, k) = new_header();
    let mut rng = TestRandom(9);
    let g0 = h.generation;
    let admin = h.add_password(&mut rng, &k, b"admin pw", SlotKind::Admin, 1, CHEAP).unwrap();
    assert_eq!(*h.unlock_password(b"admin pw").unwrap().key, k);
    assert_eq!(h.unlock_password(b"admin pw").unwrap().slot, admin);
    assert!(h.generation > g0);
    h.change_password(&mut rng, &k, 0, b"new pw", CHEAP).unwrap();
    assert!(h.unlock_password(b"correct horse").is_err() || h.unlock_password(b"correct horse").unwrap().slot != 0);
    assert_eq!(h.unlock_password(b"new pw").unwrap().slot, 0);
    // the owner's tag is kept
    assert_eq!(h.list()[0], (0, SlotKind::Person, 1000));
    h.remove_slot(&k, 0).unwrap();
    assert!(h.unlock_password(b"new pw").is_err());
    assert_eq!(h.remove_slot(&k, admin).err(), Some(VolError::LastSlot));
    assert_eq!(h.remove_slot(&k, 5).err(), Some(VolError::NoSuchSlot));
    assert_eq!(h.remove_slot(&k, 99).err(), Some(VolError::NoSuchSlot));
    assert!(h.unlock_password(b"admin pw").is_ok());
}

#[test]
fn wrong_key_or_wrong_kind_changes_are_refused() {
    let (mut h, k) = new_header();
    let mut rng = TestRandom(3);
    let mut bad = k;
    bad[0] ^= 1;
    assert_eq!(h.add_password(&mut rng, &bad, b"x", SlotKind::Person, 0, CHEAP).err(), Some(VolError::WrongSecret));
    assert_eq!(h.add_password(&mut rng, &k, b"x", SlotKind::Recovery, 0, CHEAP).err(), Some(VolError::WrongSlotKind));
    let (i, _) = h.set_recovery(&mut rng, &k).unwrap();
    assert_eq!(h.change_password(&mut rng, &k, i, b"x", CHEAP).err(), Some(VolError::WrongSlotKind));
    assert_eq!(h.change_password(&mut rng, &k, 6, b"x", CHEAP).err(), Some(VolError::NoSuchSlot));
}

#[test]
fn eight_slots_at_most() {
    let (mut h, k) = new_header();
    let mut rng = TestRandom(4);
    for n in 1..8 {
        h.add_password(&mut rng, &k, format!("pw{n}").as_bytes(), SlotKind::Person, 0, CHEAP).unwrap();
    }
    assert_eq!(h.add_password(&mut rng, &k, b"one more", SlotKind::Person, 0, CHEAP).err(), Some(VolError::SlotsFull));
}

/// Fix the checksum after a deliberate edit so the edit reaches the later checks.
fn resum(b: &mut [u8]) {
    let s = Sha256::digest(&b[..1184]);
    b[1184..1216].copy_from_slice(&s);
}

#[test]
fn truncated_and_foreign_input_is_refused_at_every_length() {
    let (h, _) = new_header();
    let b = h.to_bytes();
    for n in 0..HEADER_LEN {
        let r = Header::parse(&b[..n]);
        assert!(r.is_err(), "length {n}");
    }
    assert_eq!(Header::parse(&[]).err(), Some(VolError::Truncated));
    assert_eq!(Header::parse(&b[..100]).err(), Some(VolError::Truncated));
    assert_eq!(Header::parse(&[0u8; 4096]).err(), Some(VolError::NotOurs));
    assert_eq!(Header::parse(&[0xffu8; 4096]).err(), Some(VolError::NotOurs));
}

#[test]
fn every_flipped_bit_in_the_header_is_noticed() {
    let (h, _) = new_header();
    let b = h.to_bytes();
    for i in 0..HEADER_LEN {
        let mut t = b.clone();
        t[i] ^= 0x10;
        assert!(Header::parse(&t).is_err(), "byte {i} accepted");
    }
}

#[test]
fn edits_with_a_fixed_checksum_are_caught_by_the_checks_or_the_mac() {
    let (h, k) = new_header();
    let b = h.to_bytes();
    // geometry nonsense
    for (at, val) in [(40usize, 1000u32), (44, 2), (12, 100)] {
        let mut t = b.clone();
        t[at..at + 4].copy_from_slice(&val.to_le_bytes());
        resum(&mut t);
        assert!(matches!(Header::parse(&t), Err(VolError::Invalid(_))), "{at}");
    }
    // a newer format
    let mut t = b.clone();
    t[8] = 2;
    assert_eq!(Header::parse(&t).err(), Some(VolError::Newer(2)));
    // a hostile memory cost is refused without trying it
    let mut t = b.clone();
    t[128 + 4..128 + 8].copy_from_slice(&u32::MAX.to_le_bytes());
    resum(&mut t);
    assert!(matches!(Header::parse(&t), Err(VolError::KdfTooBig(_))));
    for (at, val) in [(132usize, 0u32), (136, 0), (136, 1000), (140, 0), (140, 99)] {
        let mut t = b.clone();
        t[at..at + 4].copy_from_slice(&val.to_le_bytes());
        resum(&mut t);
        assert!(matches!(Header::parse(&t), Err(VolError::Invalid(_))), "{at}");
    }
    // an unknown slot kind, a used reserved field, data hidden in an empty slot or after the checksum
    let mut t = b.clone();
    t[128] = 9;
    resum(&mut t);
    assert!(matches!(Header::parse(&t), Err(VolError::Invalid(_))));
    for at in [128 + 20, 128 + 128 + 60, 70, 2000, 4095] {
        let mut t = b.clone();
        t[at] = 1;
        resum(&mut t);
        assert!(matches!(Header::parse(&t), Err(VolError::Invalid(_))), "{at}");
    }
    // removing every slot
    let mut t = b.clone();
    t[128..256].fill(0);
    resum(&mut t);
    assert!(matches!(Header::parse(&t), Err(VolError::Invalid("no key slot"))));
    // a weaker cost inside the allowed range parses but the key no longer comes out (and never opens by accident)
    let mut t = b.clone();
    t[136..140].copy_from_slice(&2u32.to_le_bytes());
    resum(&mut t);
    let weak = Header::parse(&t).unwrap();
    assert_eq!(weak.unlock_password(b"correct horse").err(), Some(VolError::WrongSecret));
    let _ = k;
}

#[test]
fn removing_a_slot_from_a_two_slot_header_is_caught_by_the_mac() {
    let (mut h, k) = new_header();
    let mut rng = TestRandom(11);
    h.add_password(&mut rng, &k, b"other", SlotKind::Person, 0, CHEAP).unwrap();
    let mut b = h.to_bytes();
    // an attacker without the key blanks slot 1 and fixes the plain checksum
    b[256..384].fill(0);
    resum(&mut b);
    let t = Header::parse(&b).unwrap();
    assert_eq!(t.unlock_password(b"correct horse").err(), Some(VolError::Tampered));
    // and edits the tag of slot 0
    let mut b = h.to_bytes();
    b[128 + 16] ^= 1;
    resum(&mut b);
    assert_eq!(Header::parse(&b).unwrap().unlock_password(b"correct horse").err(), Some(VolError::Tampered));
    // changing the generation (to make an old copy look new) too
    let mut b = h.to_bytes();
    b[16] ^= 1;
    resum(&mut b);
    assert_eq!(Header::parse(&b).unwrap().unlock_password(b"correct horse").err(), Some(VolError::Tampered));
}

#[test]
fn geometry_of_a_new_volume_is_checked() {
    let mk = |ss, off, len| Header::create(&mut TestRandom(1), &NewVolume { sector_size: ss, payload_offset: off, payload_bytes: len }, b"p", SlotKind::Person, 0, CHEAP).err();
    assert!(mk(1024, 8192, 4096).is_some());
    assert!(mk(4096, 8192, 100).is_some());
    assert!(mk(4096, 8192, 0).is_some());
    assert!(mk(4096, 100, 4096).is_some());
    assert!(mk(4096, u64::MAX - 4095, 8192).is_some());
    assert!(mk(512, 8192, 512).is_none());
    assert_eq!(Header::create(&mut TestRandom(1), &NewVolume { sector_size: 512, payload_offset: 8192, payload_bytes: 512 }, b"p", SlotKind::Recovery, 0, CHEAP).err(), Some(VolError::WrongSlotKind));
}

#[test]
fn password_costs_are_bounded_and_the_floor_is_reported() {
    for bad in [KdfParams { m_kib: 0, t: 1, p: 1 }, KdfParams { m_kib: 8, t: 1, p: 2 }, KdfParams { m_kib: 64, t: 0, p: 1 }, KdfParams { m_kib: 64, t: 1, p: 0 }, KdfParams { m_kib: 64, t: 17, p: 1 }] {
        assert!(derive_kek(b"x", &[0; 32], &bad).is_err());
    }
    assert_eq!(derive_kek(b"x", &[0; 32], &KdfParams { m_kib: u32::MAX, t: 1, p: 1 }).err(), Some(VolError::KdfTooBig(4194303)));
    assert!(!CHEAP.meets_floor());
    assert!(FLOOR.meets_floor());
    // same inputs give the same key; a different salt or password does not
    let a = derive_kek(b"x", &[1; 32], &CHEAP).unwrap();
    assert_eq!(*a, *derive_kek(b"x", &[1; 32], &CHEAP).unwrap());
    assert_ne!(*a, *derive_kek(b"y", &[1; 32], &CHEAP).unwrap());
    assert_ne!(*a, *derive_kek(b"x", &[2; 32], &CHEAP).unwrap());
}

#[test]
fn a_known_header_stays_readable() {
    // a header made with fixed random input must keep reading the same: it pins the format (change the format, change this on purpose)
    let (h, _) = new_header();
    let b = h.to_bytes();
    let digest: String = Sha256::digest(&b).iter().map(|x| format!("{x:02x}")).collect();
    assert_eq!(digest, KNOWN_HEADER_SHA256, "the on-disk format changed");
}

const KNOWN_HEADER_SHA256: &str = "3813b48a8f4dfe0e9977a6caec0995a251c448d7b8b26ee89748ad42bc999729";
