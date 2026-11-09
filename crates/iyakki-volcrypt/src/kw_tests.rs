use crate::kw::*;

fn unhex(s: &str) -> Vec<u8> {
    (0..s.len()).step_by(2).map(|i| u8::from_str_radix(&s[i..i + 2], 16).unwrap()).collect()
}

#[test]
fn rfc_3394_vectors() {
    let kek128: [u8; 16] = std::array::from_fn(|i| i as u8);
    let data128 = unhex("00112233445566778899aabbccddeeff");
    let w = wrap128(&kek128, &data128).unwrap();
    assert_eq!(w, unhex("1fa68b0a8112b447aef34bd8fb5a7b829d3e862371d2cfe5"));
    assert_eq!(&unwrap128(&kek128, &w).unwrap()[..], &data128[..]);

    let kek: [u8; 32] = std::array::from_fn(|i| i as u8);
    let data = unhex("00112233445566778899aabbccddeeff000102030405060708090a0b0c0d0e0f");
    let w = wrap256(&kek, &data).unwrap();
    assert_eq!(w, unhex("28c9f404c4b810f4cbccb35cfb87f8263f5786e2d80ed326cbc7f0e71a99f43bfb988b9b7a02dd21"));
    assert_eq!(&unwrap256(&kek, &w).unwrap()[..], &data[..]);
    let w = wrap256(&kek, &data[..16]).unwrap();
    assert_eq!(w, unhex("64e8c3f9ce0f5ba263e9777905818a2a93c8191e7d6e8ae7"));
}

#[test]
fn a_wrong_key_or_any_changed_bit_fails_and_bad_sizes_are_refused() {
    let kek = [9u8; 32];
    let w = wrap256(&kek, &[7u8; 64]).unwrap();
    assert_eq!(w.len(), 72);
    assert!(unwrap256(&[8u8; 32], &w).is_none());
    for i in 0..w.len() {
        let mut t = w.clone();
        t[i] ^= 0x01;
        assert!(unwrap256(&kek, &t).is_none(), "byte {i}");
    }
    assert!(wrap256(&kek, &[0u8; 8]).is_none());
    assert!(wrap256(&kek, &[0u8; 20]).is_none());
    assert!(unwrap256(&kek, &w[..64]).is_none());
    assert!(unwrap256(&kek, &[]).is_none());
    assert!(unwrap256(&kek, &w[..71]).is_none());
}
