use crate::mac::*;

fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

#[test]
fn rfc_4231_vectors() {
    assert_eq!(hex(&hmac_sha256(&[0x0b; 20], &[b"Hi There"])), "b0344c61d8db38535ca8afceaf0bf12b881dc200c9833da726e9376c2e32cff7");
    assert_eq!(hex(&hmac_sha256(b"Jefe", &[b"what do ya want for nothing?"])), "5bdcc146bf60754e6a042426089575c75a003f089d2739839dec58b964ec3843");
    // a key longer than the block is hashed first (test case 6)
    assert_eq!(hex(&hmac_sha256(&[0xaa; 131], &[b"Test Using Larger Than Block-Size Key - Hash Key First"])), "60e431591ee0b67f0d8a26aacbf5b77f8e0bc6213728c5140546040f0ee37f54");
    // pieces are joined
    assert_eq!(hmac_sha256(b"Jefe", &[b"what do ya ", b"want for nothing?"]), hmac_sha256(b"Jefe", &[b"what do ya want for nothing?"]));
}

#[test]
fn constant_time_compare_is_exact() {
    assert!(ct_eq(b"abc", b"abc"));
    assert!(!ct_eq(b"abc", b"abd"));
    assert!(!ct_eq(b"abc", b"ab"));
    assert!(ct_eq(b"", b""));
}

#[test]
fn argon2id_matches_the_rfc_9106_test_vector() {
    // RFC 9106 section 5.3: t=3, m=32 KiB, p=4, password 32x01, salt 16x02, secret 8x03, associated data 12x04
    use argon2::{Algorithm, Argon2, ParamsBuilder, Version};
    let params = ParamsBuilder::new().m_cost(32).t_cost(3).p_cost(4).data(argon2::AssociatedData::new(&[4u8; 12]).unwrap()).output_len(32).build().unwrap();
    let a = Argon2::new_with_secret(&[3u8; 8], Algorithm::Argon2id, Version::V0x13, params).unwrap();
    let mut out = [0u8; 32];
    a.hash_password_into(&[1u8; 32], &[2u8; 16], &mut out).unwrap();
    assert_eq!(hex(&out), "0d640df58d78766c08c037a34a8b53c9d01ef0452d75b65eb52520e96b01e659");
}
