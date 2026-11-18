//! AES key wrap (RFC 3394), used to carry the group key to the client inside the third handshake message.

use crate::aes::Aes;

const IV: [u8; 8] = [0xa6; 8];

/// Wrap `plain` (a multiple of 8 bytes, at least 16) under `kek`. The output is 8 bytes longer.
pub fn wrap(kek: &[u8], plain: &[u8]) -> Option<Vec<u8>> {
    if plain.len() < 16 || plain.len() % 8 != 0 {
        return None;
    }
    let aes = Aes::new(kek)?;
    let n = plain.len() / 8;
    let mut a = IV;
    let mut r: Vec<[u8; 8]> = plain.chunks(8).map(|c| c.try_into().unwrap_or([0; 8])).collect();
    for j in 0..6u64 {
        for (i, ri) in r.iter_mut().enumerate() {
            let mut b = [0u8; 16];
            b[..8].copy_from_slice(&a);
            b[8..].copy_from_slice(ri);
            let b = aes.encrypt_block(&b);
            let t = (n as u64) * j + i as u64 + 1;
            a.copy_from_slice(&b[..8]);
            for (x, y) in a.iter_mut().zip(t.to_be_bytes()) {
                *x ^= y;
            }
            ri.copy_from_slice(&b[8..]);
        }
    }
    let mut out = a.to_vec();
    for ri in r {
        out.extend_from_slice(&ri);
    }
    Some(out)
}

/// Unwrap; `None` when the length is wrong or the integrity value does not match (wrong key or damaged data).
pub fn unwrap(kek: &[u8], wrapped: &[u8]) -> Option<Vec<u8>> {
    if wrapped.len() < 24 || wrapped.len() % 8 != 0 {
        return None;
    }
    let aes = Aes::new(kek)?;
    let n = wrapped.len() / 8 - 1;
    let mut a: [u8; 8] = wrapped[..8].try_into().ok()?;
    let mut r: Vec<[u8; 8]> = wrapped[8..].chunks(8).map(|c| c.try_into().unwrap_or([0; 8])).collect();
    for j in (0..6u64).rev() {
        for i in (0..n).rev() {
            let t = (n as u64) * j + i as u64 + 1;
            let mut b = [0u8; 16];
            b[..8].copy_from_slice(&a);
            for (x, y) in b[..8].iter_mut().zip(t.to_be_bytes()) {
                *x ^= y;
            }
            b[8..].copy_from_slice(&r[i]);
            let b = aes.decrypt_block(&b);
            a.copy_from_slice(&b[..8]);
            r[i].copy_from_slice(&b[8..]);
        }
    }
    if !crate::ct_eq(&a, &IV) {
        return None;
    }
    Some(r.concat())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{hex, unhex};

    fn case(kek: &str, data: &str, expect: &str) {
        let k = unhex(kek);
        let d = unhex(data);
        let w = wrap(&k, &d).unwrap();
        assert_eq!(hex(&w), expect.to_lowercase().replace(' ', ""));
        assert_eq!(unwrap(&k, &w).unwrap(), d);
    }

    #[test]
    fn rfc_3394_section_4_vectors() {
        let d128 = "00112233445566778899AABBCCDDEEFF";
        let d192 = "00112233445566778899AABBCCDDEEFF0001020304050607";
        let d256 = "00112233445566778899AABBCCDDEEFF000102030405060708090A0B0C0D0E0F";
        let k128 = "000102030405060708090A0B0C0D0E0F";
        let k192 = "000102030405060708090A0B0C0D0E0F1011121314151617";
        let k256 = "000102030405060708090A0B0C0D0E0F101112131415161718191A1B1C1D1E1F";
        case(k128, d128, "1FA68B0A8112B447AEF34BD8FB5A7B829D3E862371D2CFE5");
        case(k192, d128, "96778B25AE6CA435F92B5B97C050AED2468AB8A17AD84E5D");
        case(k256, d128, "64E8C3F9CE0F5BA263E9777905818A2A93C8191E7D6E8AE7");
        case(k192, d192, "031D33264E15D33268F24EC260743EDCE1C6C7DDEE725A936BA814915C6762D2");
        case(k256, d192, "A8F9BC1612C68B3FF6E6F4FBE30E71E4769C8B80A32CB8958CD5D17D6B254DA1");
        case(k256, d256, "28C9F404C4B810F4CBCCB35CFB87F8263F5786E2D80ED326CBC7F0E71A99F43BFB988B9B7A02DD21");
    }

    #[test]
    fn a_wrong_key_or_a_changed_byte_is_detected() {
        let k = [7u8; 16];
        let mut w = wrap(&k, &[1u8; 32]).unwrap();
        assert_eq!(unwrap(&[8u8; 16], &w), None);
        w[9] ^= 1;
        assert_eq!(unwrap(&k, &w), None);
        assert_eq!(unwrap(&k, &w[..23]), None);
        assert!(wrap(&k, &[0u8; 8]).is_none());
        assert!(wrap(&k, &[0u8; 17]).is_none());
    }
}
