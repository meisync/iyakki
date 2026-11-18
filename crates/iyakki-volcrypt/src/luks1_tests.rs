use crate::luks1::*;
use crate::xts::{Xts, Xts128};

fn hex(s: &str) -> Vec<u8> {
    (0..s.len()).step_by(2).map(|i| u8::from_str_radix(&s[i..i + 2], 16).unwrap()).collect()
}

#[test]
fn sha1_pbkdf2_and_hmac_match_the_published_vectors() {
    assert_eq!(LuksHash::Sha1.digest(&[b"abc"]), hex("a9993e364706816aba3e25717850c26c9cd0d89d"));
    assert_eq!(LuksHash::Sha1.digest(&[b"abcdbcdecdefdefgefghfghighijhijkijkljklmklmnlmnomnopnopq"]), hex("84983e441c3bd26ebaae4aa1f95129e5e54670f1"));
    let million = vec![b'a'; 1_000_000];
    assert_eq!(LuksHash::Sha1.digest(&[&million[..300_000], &million[300_000..]]), hex("34aa973cd4c4daa4f61eeb2bdbad27316534016f"));
    // RFC 2202 test case 2
    assert_eq!(hmac(LuksHash::Sha1, b"Jefe", &[b"what do ya want ", b"for nothing?"]), hex("effcdf6ae5eb2fa2d27416d5f184df9c259a7c79"));
    // RFC 6070 vectors
    assert_eq!(*pbkdf2(LuksHash::Sha1, b"password", b"salt", 1, 20), hex("0c60c80f961f0e71f3a9b524af6012062fe037a6"));
    assert_eq!(*pbkdf2(LuksHash::Sha1, b"password", b"salt", 2, 20), hex("ea6c014dc72d6f8ccd1ed92ace1d41f0d8de8957"));
    assert_eq!(*pbkdf2(LuksHash::Sha1, b"password", b"salt", 4096, 20), hex("4b007901b765489abead49d926f721d065a429c1"));
    assert_eq!(*pbkdf2(LuksHash::Sha1, b"passwordPASSWORDpassword", b"saltSALTsaltSALTsaltSALTsaltSALTsalt", 4096, 25), hex("3d2eec4fe41c849b80c8d83662c0e44a8b291a964cf2f07038"));
    // SHA-256 PBKDF2 (RFC 7914 section 11, c = 1)
    assert_eq!(*pbkdf2(LuksHash::Sha256, b"passwd", b"salt", 1, 64), hex("55ac046e56e3089fec1691c22544b605f94185216dde0465e68b9d57c20dacbc49ca9cccf179b645991664b39d77ef317c71b845b1e30bd509112041d3a19783"));
    // SHA-512 PBKDF2 (well-known vector, c = 1, dkLen 64)
    assert_eq!(*pbkdf2(LuksHash::Sha512, b"password", b"salt", 1, 64), hex("867f70cf1ade02cff3752599a3a53dc4af34c7a669815ae5d513554e1c8cf252c02d470a285a0501bad999bfe943c08f050235d7d68b1da55e63f73b60a57fce"));
}

/// Build a LUKS1 volume the way cryptsetup lays it out: header, key material, payload offset.
fn make(hash: LuksHash, key_bytes: usize, password: &[u8], master: &[u8], iters: u32, mk_iters: u32, slot: usize) -> (Vec<u8>, Vec<u8>) {
    let spec = match hash {
        LuksHash::Sha1 => "sha1",
        LuksHash::Sha256 => "sha256",
        LuksHash::Sha512 => "sha512",
    };
    let mut seed = 7u64;
    let mut rnd = |b: &mut [u8]| {
        for x in b.iter_mut() {
            seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
            *x = (seed >> 33) as u8;
        }
    };
    let stripes = 4000usize;
    let mat_sectors = (stripes * key_bytes).div_ceil(512);
    let mut h = vec![0u8; HEADER_LEN];
    h[..6].copy_from_slice(b"LUKS\xba\xbe");
    h[6..8].copy_from_slice(&1u16.to_be_bytes());
    h[8..11].copy_from_slice(b"aes");
    h[40..51].copy_from_slice(b"xts-plain64");
    h[72..72 + spec.len()].copy_from_slice(spec.as_bytes());
    h[104..108].copy_from_slice(&4096u32.to_be_bytes());
    h[108..112].copy_from_slice(&(key_bytes as u32).to_be_bytes());
    let mut mk_salt = [0u8; 32];
    rnd(&mut mk_salt);
    h[132..164].copy_from_slice(&mk_salt);
    h[164..168].copy_from_slice(&mk_iters.to_be_bytes());
    let dg = pbkdf2(hash, master, &mk_salt, mk_iters, 20);
    h[112..132].copy_from_slice(&dg);
    h[168..168 + 36].copy_from_slice(b"01234567-89ab-cdef-0123-456789abcdef");
    for i in 0..8 {
        let o = 208 + i * 48;
        h[o..o + 4].copy_from_slice(&0x0000_DEADu32.to_be_bytes());
    }
    let o = 208 + slot * 48;
    let mut salt = [0u8; 32];
    rnd(&mut salt);
    h[o..o + 4].copy_from_slice(&0x00AC_71F3u32.to_be_bytes());
    h[o + 4..o + 8].copy_from_slice(&iters.to_be_bytes());
    h[o + 8..o + 40].copy_from_slice(&salt);
    h[o + 40..o + 44].copy_from_slice(&8u32.to_be_bytes());
    h[o + 44..o + 48].copy_from_slice(&(stripes as u32).to_be_bytes());
    let mut mat = af_split(hash, master, stripes, &mut rnd);
    mat.resize(mat_sectors * 512, 0);
    let sk = pbkdf2(hash, password, &salt, iters, key_bytes);
    for (i, sec) in mat.chunks_mut(512).enumerate() {
        if key_bytes == 32 {
            let mut k = [0u8; 32];
            k.copy_from_slice(&sk);
            assert!(Xts128::new_128(&k).encrypt_sector(i as u64, sec));
        } else {
            let mut k = [0u8; 64];
            k.copy_from_slice(&sk);
            assert!(Xts::new(&k).encrypt_sector(i as u64, sec));
        }
    }
    let mut disk = vec![0u8; 8 * 512 + mat.len()];
    disk[..HEADER_LEN].copy_from_slice(&h);
    disk[8 * 512..].copy_from_slice(&mat);
    (h, disk)
}

fn reader(disk: Vec<u8>) -> impl FnMut(u64, usize) -> Option<Vec<u8>> {
    move |s, n| disk.get((s as usize) * 512..(s as usize) * 512 + n).map(<[u8]>::to_vec)
}

#[test]
fn the_right_password_gives_the_master_key_for_every_hash_and_key_size() {
    for (hash, kb) in [(LuksHash::Sha256, 64usize), (LuksHash::Sha1, 32), (LuksHash::Sha512, 64), (LuksHash::Sha256, 32)] {
        let master: Vec<u8> = (0..kb as u8).map(|i| i.wrapping_mul(7).wrapping_add(3)).collect();
        let (h, disk) = make(hash, kb, b"correct horse", &master, 3, 2, 3);
        let hdr = parse_header(&h).unwrap();
        assert_eq!((hdr.hash, hdr.key_bytes, hdr.payload_sector), (hash, kb as u32, 4096));
        assert_eq!(hdr.uuid, "01234567-89ab-cdef-0123-456789abcdef");
        assert_eq!(hdr.slots.iter().filter(|s| s.active).count(), 1);
        assert_eq!(*hdr.unlock(b"correct horse", &mut reader(disk.clone())).unwrap(), master);
        assert_eq!(hdr.unlock(b"correct hors", &mut reader(disk)).unwrap_err(), LuksError::WrongPassword);
    }
}

#[test]
fn a_damaged_key_slot_or_a_short_read_is_a_plain_error_not_a_panic() {
    let master = vec![9u8; 64];
    let (h, mut disk) = make(LuksHash::Sha256, 64, b"pw", &master, 2, 2, 0);
    let hdr = parse_header(&h).unwrap();
    disk[8 * 512 + 100] ^= 1;
    assert_eq!(hdr.unlock(b"pw", &mut reader(disk.clone())).unwrap_err(), LuksError::WrongPassword);
    assert_eq!(hdr.unlock(b"pw", &mut |_, _| None).unwrap_err(), LuksError::Unreadable);
    assert_eq!(hdr.unlock(b"pw", &mut |_, n| Some(vec![0; n / 2])).unwrap_err(), LuksError::Unreadable);
}

#[test]
fn hostile_headers_are_refused_at_every_length_and_with_flipped_values() {
    let master = vec![1u8; 64];
    let (h, _) = make(LuksHash::Sha256, 64, b"pw", &master, 2, 2, 0);
    for n in 0..HEADER_LEN {
        let r = parse_header(&h[..n]);
        assert!(r.is_err(), "length {n}");
    }
    let mut bad = h.clone();
    bad[108..112].copy_from_slice(&7u32.to_be_bytes());
    assert_eq!(parse_header(&bad).unwrap_err(), LuksError::Invalid("key size"));
    let mut bad = h.clone();
    bad[164..168].copy_from_slice(&u32::MAX.to_be_bytes());
    assert!(matches!(parse_header(&bad), Err(LuksError::Invalid(_))));
    let mut bad = h.clone();
    bad[208 + 4..208 + 8].copy_from_slice(&u32::MAX.to_be_bytes());
    assert!(matches!(parse_header(&bad), Err(LuksError::Invalid(_))));
    let mut bad = h.clone();
    bad[208..212].copy_from_slice(&5u32.to_be_bytes());
    assert!(matches!(parse_header(&bad), Err(LuksError::Invalid(_))));
    let mut bad = h.clone();
    bad[72..80].copy_from_slice(b"whirlpoo");
    assert!(matches!(parse_header(&bad), Err(LuksError::Unsupported(_))));
    let mut bad = h.clone();
    bad[7] = 2;
    assert_eq!(parse_header(&bad).unwrap_err(), LuksError::NotLuks1);
    // every single byte flip must give Ok or Err, never a panic
    for i in 0..HEADER_LEN {
        let mut x = h.clone();
        x[i] ^= 0xff;
        let _ = parse_header(&x);
    }
}

#[test]
fn other_ciphers_are_refused_with_their_name() {
    let master = vec![1u8; 32];
    let (mut h, disk) = make(LuksHash::Sha256, 32, b"pw", &master, 2, 2, 0);
    h[40..51].copy_from_slice(b"cbc-essiv:s");
    let hdr = parse_header(&h).unwrap();
    assert!(matches!(hdr.unlock(b"pw", &mut reader(disk)), Err(LuksError::Unsupported(s)) if s.starts_with("aes-cbc")));
}

#[test]
fn the_anti_forensic_split_merges_back_and_one_changed_stripe_changes_the_result() {
    for h in [LuksHash::Sha1, LuksHash::Sha256, LuksHash::Sha512] {
        let key = vec![0xA5u8; 48];
        let mut n = 0u8;
        let mut rnd = |b: &mut [u8]| {
            for x in b.iter_mut() {
                n = n.wrapping_mul(31).wrapping_add(17);
                *x = n;
            }
        };
        let mut m = af_split(h, &key, 50, &mut rnd);
        assert_eq!(*af_merge(h, &m, 48, 50).unwrap(), key);
        m[5] ^= 1;
        assert_ne!(*af_merge(h, &m, 48, 50).unwrap(), key);
    }
    assert!(af_merge(LuksHash::Sha1, &[0; 10], 48, 50).is_none());
}
