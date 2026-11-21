use crate::luks1::LuksError;
use crate::luks2::*;
use sha2::{Digest, Sha256};

/// A volume made by cryptsetup 2.8.4 (`luksFormat --type luks2 --pbkdf argon2id --pbkdf-memory 4096 --pbkdf-parallel 2
/// --pbkdf-force-iterations 4 --cipher aes-xts-plain64 --key-size 512 --hash sha256`), cut after the first key slot's area (290816 bytes);
/// the passphrase and the master key (from `luksDump --dump-master-key`) are the ones below.
const FIXTURE: &[u8] = include_bytes!("../tests/fixtures/luks2-argon2id-aes-xts-512.img");
const PASS: &[u8] = b"correct horse battery";
const MASTER: &str = "f7f46d429fa5e2edff5c38d59e55fb60d738b8fc7636f861e5528e8aaaa66bc799637dc13407f0a34ae121cf82a10e927a4a278f7fdcb29a0b28b245f489289a";

fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

pub(crate) fn reader(d: &'static [u8]) -> impl FnMut(u64, usize) -> Option<Vec<u8>> {
    move |off, n| d.get(off as usize..off as usize + n).map(<[u8]>::to_vec)
}

#[test]
fn a_volume_made_by_cryptsetup_opens_with_its_passphrase_and_gives_its_master_key() {
    let size = header_size(FIXTURE).unwrap() as usize;
    assert_eq!(size, 16384);
    let l = parse_header(&FIXTURE[..size]).unwrap();
    assert_eq!(l.uuid, "eebac079-31db-4409-bce4-c027187a4a97");
    assert_eq!(l.slots.len(), 1);
    assert!(matches!(&l.slots[0].kdf, Kdf::Argon { time: 4, memory_kib: 4096, cpus: 2, .. }));
    assert_eq!((l.slots[0].area_offset, l.slots[0].area_size, l.slots[0].stripes), (32768, 258048, 4000));
    assert_eq!(l.data_segment().map(|s| (s.offset, s.sector_size)), Some((16777216, 4096)));
    let key = l.unlock(PASS, &mut reader(FIXTURE)).unwrap();
    assert_eq!(hex(&key), MASTER);
    assert_eq!(l.unlock(b"wrong", &mut reader(FIXTURE)), Err(LuksError::WrongPassword));
    assert_eq!(l.unlock(PASS, &mut |_, _| None), Err(LuksError::Unreadable));
}

#[test]
fn damaged_or_foreign_headers_are_plain_errors_never_panics() {
    assert_eq!(header_size(&FIXTURE[..100]), Err(LuksError::Truncated));
    let mut v1 = FIXTURE[..4096].to_vec();
    v1[7] = 1;
    assert_eq!(header_size(&v1), Err(LuksError::NotLuks2));
    assert_eq!(header_size(&[0u8; 4096]), Err(LuksError::NotLuks2));
    assert_eq!(parse_header(&FIXTURE[..8000]), Err(LuksError::Truncated));
    // every flipped byte is caught: the checksum covers the whole header
    for at in (0..16384).step_by(61) {
        let mut h = FIXTURE[..16384].to_vec();
        h[at] ^= 0x55;
        assert!(parse_header(&h).is_err(), "byte {at}");
    }
    // an absurd memory cost (with a checksum made to fit) is refused before it is spent
    let mut h = FIXTURE[..16384].to_vec();
    let end = h[4096..].iter().position(|c| *c == 0).unwrap() + 4096;
    let mut j: serde_json::Value = serde_json::from_slice(&h[4096..end]).unwrap();
    j["keyslots"]["0"]["kdf"]["memory"] = serde_json::json!(99_999_999);
    let text = serde_json::to_vec(&j).unwrap();
    h[4096..].fill(0);
    h[4096..4096 + text.len()].copy_from_slice(&text);
    h[448..512].fill(0);
    let sum = Sha256::digest(&h);
    h[448..480].copy_from_slice(&sum);
    assert_eq!(parse_header(&h), Err(LuksError::Invalid("argon2 costs")));
}

const PBKDF2_ONLY: &[u8] = include_bytes!("../tests/fixtures/luks2-pbkdf2-sha512-aes-xts-256.bin");
const TWO_SLOTS: &[u8] = include_bytes!("../tests/fixtures/luks2-two-slots-argon2i-pbkdf2.bin");

fn open(d: &'static [u8]) -> Luks2 {
    let size = header_size(d).unwrap() as usize;
    parse_header(&d[..size]).unwrap()
}

#[test]
fn a_pbkdf2_only_volume_with_a_256_bit_key_and_sha512_made_by_cryptsetup_opens() {
    // cryptsetup 2.8.4: luksFormat --type luks2 --pbkdf pbkdf2 --pbkdf-force-iterations 1000 --key-size 256 --hash sha512
    let l = open(PBKDF2_ONLY);
    assert_eq!(l.slots.len(), 1);
    assert!(matches!(&l.slots[0].kdf, Kdf::Pbkdf2 { iterations: 1000, .. }));
    assert_eq!((l.slots[0].key_size, l.slots[0].area_size), (32, 131072));
    let key = l.unlock(b"pbkdf2 only pass", &mut reader(PBKDF2_ONLY)).unwrap();
    assert_eq!(hex(&key), "b69a244b4001ebf70ac43c534c2435f8a2b187d4286e61a67a50c0d802160450");
    assert_eq!(l.unlock(b"pbkdf2 only pas", &mut reader(PBKDF2_ONLY)), Err(LuksError::WrongPassword));
}

#[test]
fn a_volume_with_an_argon2i_slot_and_a_pbkdf2_slot_opens_with_either_passphrase_and_gives_the_same_key() {
    // cryptsetup 2.8.4: luksFormat with --pbkdf argon2i (2 MiB, 4 passes), then luksAddKey --pbkdf pbkdf2 (1000 iterations)
    let l = open(TWO_SLOTS);
    assert_eq!(l.slots.len(), 2);
    assert!(matches!(&l.slots[0].kdf, Kdf::Argon { alg: argon2::Algorithm::Argon2i, time: 4, memory_kib: 2048, cpus: 1, .. }));
    assert!(matches!(&l.slots[1].kdf, Kdf::Pbkdf2 { iterations: 1000, .. }));
    assert_eq!((l.slots[0].area_offset, l.slots[1].area_offset), (32768, 290816));
    let want = "6ba275903e1629c8648f841539 00e31200dfb907e2cc020582bcbfa4e0274e76d445ba31c10bf4146d00e4ce42dd877574 25b51c10c9f2bdca23b6698f911ab8".replace(' ', "");
    let a = l.unlock(b"first one", &mut reader(TWO_SLOTS)).unwrap();
    let b = l.unlock(b"second one", &mut reader(TWO_SLOTS)).unwrap();
    assert_eq!(hex(&a), want);
    assert_eq!(hex(&b), want);
    assert_eq!(l.unlock(b"third one", &mut reader(TWO_SLOTS)), Err(LuksError::WrongPassword));
}

mod export_tests {
    use crate::luks2::{header_size, parse_header};
    use crate::luks2_export::*;

    fn counter() -> impl FnMut(&mut [u8]) {
        let mut n = 0u8;
        move |b: &mut [u8]| {
            for x in b.iter_mut() {
                n = n.wrapping_mul(31).wrapping_add(7);
                *x = n;
            }
        }
    }

    const UUID: &str = "11111111-2222-4333-8444-555555555555";

    fn made(key: &[u8; 64], pw: &[u8]) -> Vec<u8> {
        export_header(key, pw, UUID, "home", Costs::light(), &mut counter()).unwrap()
    }

    fn reader(img: Vec<u8>) -> impl FnMut(u64, usize) -> Option<Vec<u8>> {
        move |o, n| img.get(o as usize..o as usize + n).map(<[u8]>::to_vec)
    }

    #[test]
    fn an_exported_header_opens_with_the_password_and_gives_the_volume_key_back() {
        let key: [u8; 64] = std::array::from_fn(|i| (i * 3 + 1) as u8);
        let img = made(&key, b"correct horse");
        assert_eq!(img.len() as u64, DATA_OFFSET);
        let h = parse_header(&img[..HEADER_COPY]).unwrap();
        assert_eq!((h.uuid.as_str(), h.label.as_str()), (UUID, "home"));
        assert_eq!(h.data_segment().unwrap().offset, DATA_OFFSET);
        assert_eq!(&*h.unlock(b"correct horse", &mut reader(img.clone())).unwrap(), &key[..]);
        assert!(h.unlock(b"wrong", &mut reader(img)).is_err());
    }

    #[test]
    fn the_second_copy_is_complete_and_has_the_other_signature_and_its_own_offset() {
        let key = [9u8; 64];
        let img = made(&key, b"pw");
        assert_eq!(&img[HEADER_COPY..HEADER_COPY + 6], b"SKUL\xba\xbe");
        assert_eq!(header_size(&img[HEADER_COPY..HEADER_COPY + 4096]).unwrap(), HEADER_COPY as u64);
        let h = parse_header(&img[HEADER_COPY..2 * HEADER_COPY]).unwrap();
        assert_eq!(&*h.unlock(b"pw", &mut reader(img)).unwrap(), &key[..]);
    }

    #[test]
    fn one_flipped_bit_in_either_copy_breaks_its_checksum() {
        let img = made(&[1u8; 64], b"pw");
        for at in [4200usize, HEADER_COPY + 4200, 30, HEADER_COPY + 30] {
            let mut bad = img.clone();
            bad[at] ^= 1;
            let copy = at / HEADER_COPY * HEADER_COPY;
            assert!(parse_header(&bad[copy..copy + HEADER_COPY]).is_err(), "{at}");
        }
    }

    #[test]
    fn the_same_inputs_give_the_same_bytes_and_other_random_gives_other_bytes() {
        let key = [5u8; 64];
        assert_eq!(made(&key, b"pw"), made(&key, b"pw"));
        let mut n = 99u8;
        let mut other = move |b: &mut [u8]| b.iter_mut().for_each(|x| { n = n.wrapping_add(13); *x = n });
        assert_ne!(made(&key, b"pw"), export_header(&key, b"pw", UUID, "home", Costs::light(), &mut other).unwrap());
    }

    #[test]
    fn a_bad_uuid_label_or_empty_password_is_refused_and_the_key_is_not_in_the_header() {
        let key: [u8; 64] = std::array::from_fn(|i| 200u8.wrapping_add(i as u8));
        let mut r = counter();
        assert!(export_header(&key, b"pw", "nope", "", Costs::light(), &mut r).is_err());
        assert!(export_header(&key, b"pw", UUID, "bad\nlabel", Costs::light(), &mut r).is_err());
        assert!(export_header(&key, b"", UUID, "", Costs::light(), &mut r).is_err());
        let img = made(&key, b"pw");
        assert!(!img.windows(16).any(|w| w == &key[..16]), "the volume key never appears in the clear");
    }
}
