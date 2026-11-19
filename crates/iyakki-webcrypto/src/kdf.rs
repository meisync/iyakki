//! HMAC-SHA-256 (RFC 2104), HKDF (RFC 5869) and AES-256-CBC with PKCS#7 padding, on `ring` and `aes`.

use aes::cipher::{generic_array::GenericArray, BlockDecrypt, BlockEncrypt, KeyInit};
use ring::hmac;

pub fn hmac_sha256(key: &[u8], data: &[u8]) -> [u8; 32] {
    let k = hmac::Key::new(hmac::HMAC_SHA256, key);
    let t = hmac::sign(&k, data);
    let mut o = [0u8; 32];
    o.copy_from_slice(t.as_ref());
    o
}

/// HKDF-SHA-256 extract then expand to `len` bytes (at most 255 * 32).
pub fn hkdf(salt: &[u8], ikm: &[u8], info: &[u8], len: usize) -> Vec<u8> {
    let prk = hmac_sha256(if salt.is_empty() { &[0u8; 32] } else { salt }, ikm);
    let mut out = Vec::with_capacity(len);
    let mut t: Vec<u8> = Vec::new();
    let mut i = 1u8;
    while out.len() < len {
        let mut m = t.clone();
        m.extend_from_slice(info);
        m.push(i);
        t = hmac_sha256(&prk, &m).to_vec();
        out.extend_from_slice(&t);
        i = i.wrapping_add(1);
    }
    out.truncate(len);
    out
}

pub fn cbc_encrypt(key: &[u8; 32], iv: &[u8; 16], plain: &[u8]) -> Vec<u8> {
    let c = aes::Aes256::new(GenericArray::from_slice(key));
    let pad = 16 - plain.len() % 16;
    let mut data = plain.to_vec();
    data.extend(std::iter::repeat_n(pad as u8, pad));
    let mut prev = *iv;
    for b in data.chunks_mut(16) {
        for (x, p) in b.iter_mut().zip(prev) {
            *x ^= p;
        }
        c.encrypt_block(GenericArray::from_mut_slice(b));
        prev.copy_from_slice(b);
    }
    data
}

pub fn cbc_decrypt(key: &[u8; 32], iv: &[u8; 16], ct: &[u8]) -> Result<Vec<u8>, String> {
    if ct.is_empty() || ct.len() % 16 != 0 {
        return Err("bad cipher length".into());
    }
    let c = aes::Aes256::new(GenericArray::from_slice(key));
    let mut data = ct.to_vec();
    let mut prev = *iv;
    for b in data.chunks_mut(16) {
        let mut cur = [0u8; 16];
        cur.copy_from_slice(b);
        c.decrypt_block(GenericArray::from_mut_slice(b));
        for (x, p) in b.iter_mut().zip(prev) {
            *x ^= p;
        }
        prev = cur;
    }
    let pad = *data.last().unwrap_or(&0) as usize;
    if pad == 0 || pad > 16 || pad > data.len() || data[data.len() - pad..].iter().any(|&b| b as usize != pad) {
        return Err("bad padding".into());
    }
    data.truncate(data.len() - pad);
    Ok(data)
}

#[cfg(test)]
mod tests {
    use super::*;
    fn h(s: &str) -> Vec<u8> {
        (0..s.len() / 2).map(|i| u8::from_str_radix(&s[2 * i..2 * i + 2], 16).unwrap()).collect()
    }
    #[test]
    fn hmac_rfc4231_case_2() {
        assert_eq!(hmac_sha256(b"Jefe", b"what do ya want for nothing?").to_vec(), h("5bdcc146bf60754e6a042426089575c75a003f089d2739839dec58b964ec3843"));
    }
    #[test]
    fn hkdf_rfc5869_cases_1_and_3() {
        let okm = hkdf(&h("000102030405060708090a0b0c"), &[0x0b; 22], &h("f0f1f2f3f4f5f6f7f8f9"), 42);
        assert_eq!(okm, h("3cb25f25faacd57a90434f64d0362f2a2d2d0a90cf1a5a4c5db02d56ecc4c5bf34007208d5b887185865"));
        let okm = hkdf(&[], &[0x0b; 22], &[], 42);
        assert_eq!(okm, h("8da4e775a563c18f715f802a063c5a31b8a11f5c5ee1879ec3454e5f3c738d2d9d201395faa4b61a96c8"));
    }
    #[test]
    fn cbc_nist_sp800_38a_f_2_5_first_block_and_round_trip() {
        let key: [u8; 32] = h("603deb1015ca71be2b73aef0857d77811f352c073b6108d72d9810a30914dff4").try_into().unwrap();
        let iv: [u8; 16] = h("000102030405060708090a0b0c0d0e0f").try_into().unwrap();
        let ct = cbc_encrypt(&key, &iv, &h("6bc1bee22e409f96e93d7e117393172a"));
        assert_eq!(&ct[..16], &h("f58c4c04d6e5f1ba779eabfb5f7bfbd6")[..]);
        assert_eq!(cbc_decrypt(&key, &iv, &ct).unwrap(), h("6bc1bee22e409f96e93d7e117393172a"));
        let mut bad = ct.clone();
        *bad.last_mut().unwrap() ^= 1;
        assert!(cbc_decrypt(&key, &iv, &bad).is_err() || cbc_decrypt(&key, &iv, &bad).unwrap() != h("6bc1bee22e409f96e93d7e117393172a"));
    }
}
