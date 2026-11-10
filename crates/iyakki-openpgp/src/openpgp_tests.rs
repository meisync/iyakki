use super::*;

// fixtures made once with GnuPG 2 and a throw-away RSA-2048 key ("IYAKKI test key"; its private half was deleted)
const KEY: &[u8] = include_bytes!("../tests/fixtures/openpgp/test-key.asc");
const OTHER_KEY: &[u8] = include_bytes!("../tests/fixtures/openpgp/other-key.asc");
const DATA: &[u8] = include_bytes!("../tests/fixtures/openpgp/payload.bin");
const SIG_ARMORED_SHA512: &[u8] = include_bytes!("../tests/fixtures/openpgp/payload.bin.asc.sig");
const SIG_BINARY_SHA256: &[u8] = include_bytes!("../tests/fixtures/openpgp/payload.bin.sha256.sig");

#[test]
fn a_good_armored_sha512_signature_verifies() {
    verify_detached(KEY, SIG_ARMORED_SHA512, DATA).unwrap();
}

#[test]
fn a_good_binary_sha256_signature_verifies() {
    verify_detached(KEY, SIG_BINARY_SHA256, DATA).unwrap();
}

#[test]
fn streaming_in_pieces_gives_the_same_answer() {
    let mut h = Hasher::start(SIG_BINARY_SHA256).unwrap();
    for c in DATA.chunks(5) {
        h.update(c);
    }
    h.finish().unwrap().check(KEY).unwrap();
}

#[test]
fn a_changed_file_is_refused() {
    let mut d = DATA.to_vec();
    d[0] ^= 1;
    assert!(verify_detached(KEY, SIG_ARMORED_SHA512, &d).is_err());
    assert!(verify_detached(KEY, SIG_BINARY_SHA256, &d).is_err());
}

#[test]
fn a_damaged_signature_is_refused() {
    let mut s = SIG_BINARY_SHA256.to_vec();
    let last = s.len() - 3;
    s[last] ^= 0x40;
    assert!(verify_detached(KEY, &s, DATA).is_err());
    // armor with a changed character fails its checksum
    let text = String::from_utf8(SIG_ARMORED_SHA512.to_vec()).unwrap().replacen("iQEz", "iQEy", 1);
    assert!(verify_detached(KEY, text.as_bytes(), DATA).is_err());
}

#[test]
fn another_key_does_not_verify() {
    let r = verify_detached(OTHER_KEY, SIG_BINARY_SHA256, DATA);
    assert!(r.is_err(), "{r:?}");
    // a file holding both keys still accepts the signature of the first
    let both = [KEY, OTHER_KEY].concat();
    verify_detached(&both, SIG_BINARY_SHA256, DATA).unwrap();
}

#[test]
fn garbage_and_empty_input_are_refused_without_panicking() {
    assert!(verify_detached(KEY, b"", DATA).is_err());
    assert!(verify_detached(b"", SIG_BINARY_SHA256, DATA).is_err());
    assert!(verify_detached(KEY, &[0x89, 0xff, 0xff], DATA).is_err());
    assert!(verify_detached(b"-----BEGIN PGP PUBLIC KEY BLOCK-----\n\nAAAA\n-----END PGP PUBLIC KEY BLOCK-----\n", SIG_BINARY_SHA256, DATA).is_err());
    for n in 0..SIG_BINARY_SHA256.len() {
        let _ = verify_detached(KEY, &SIG_BINARY_SHA256[..n], DATA);
    }
}

#[test]
fn the_big_number_power_is_right() {
    // 4^13 mod 497 = 445 (textbook example)
    let r = Big::from_be(&[4]).modpow(&Big::from_be(&[13]), &Big::from_be(&[0x01, 0xf1]));
    assert_eq!(r.to_be(2), vec![0x01, 0xbd]);
}
