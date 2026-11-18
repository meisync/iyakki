//! The WPA/WPA2-personal pre-shared key: a passphrase and the network's name are stretched into the 256-bit key the access
//! point and the supplicant actually use (PBKDF2 with HMAC-SHA1, 4096 rounds, IEEE 802.11i annex H). Saving that key instead of
//! the passphrase means the passphrase itself never has to be kept.

fn sha1(data: &[u8]) -> [u8; 20] {
    let mut h: [u32; 5] = [0x67452301, 0xEFCDAB89, 0x98BADCFE, 0x10325476, 0xC3D2E1F0];
    let mut msg = data.to_vec();
    msg.push(0x80);
    while msg.len() % 64 != 56 {
        msg.push(0);
    }
    msg.extend_from_slice(&((data.len() as u64) * 8).to_be_bytes());
    for chunk in msg.chunks(64) {
        let mut w = [0u32; 80];
        for i in 0..16 {
            w[i] = u32::from_be_bytes([chunk[4 * i], chunk[4 * i + 1], chunk[4 * i + 2], chunk[4 * i + 3]]);
        }
        for i in 16..80 {
            w[i] = (w[i - 3] ^ w[i - 8] ^ w[i - 14] ^ w[i - 16]).rotate_left(1);
        }
        let [mut a, mut b, mut c, mut d, mut e] = h;
        for (i, wi) in w.iter().enumerate() {
            let (f, k) = match i {
                0..=19 => ((b & c) | (!b & d), 0x5A827999),
                20..=39 => (b ^ c ^ d, 0x6ED9EBA1),
                40..=59 => ((b & c) | (b & d) | (c & d), 0x8F1BBCDC),
                _ => (b ^ c ^ d, 0xCA62C1D6),
            };
            let t = a.rotate_left(5).wrapping_add(f).wrapping_add(e).wrapping_add(k).wrapping_add(*wi);
            e = d;
            d = c;
            c = b.rotate_left(30);
            b = a;
            a = t;
        }
        for (x, y) in h.iter_mut().zip([a, b, c, d, e]) {
            *x = x.wrapping_add(y);
        }
    }
    let mut out = [0u8; 20];
    for (i, x) in h.iter().enumerate() {
        out[4 * i..4 * i + 4].copy_from_slice(&x.to_be_bytes());
    }
    out
}

/// HMAC-SHA1 (RFC 2104); also the keyed hash behind the private MAC addresses.
pub fn hmac_sha1(key: &[u8], msg: &[u8]) -> [u8; 20] {
    let mut k = [0u8; 64];
    if key.len() > 64 {
        k[..20].copy_from_slice(&sha1(key));
    } else {
        k[..key.len()].copy_from_slice(key);
    }
    let mut inner: Vec<u8> = k.iter().map(|b| b ^ 0x36).collect();
    inner.extend_from_slice(msg);
    let mut outer: Vec<u8> = k.iter().map(|b| b ^ 0x5c).collect();
    outer.extend_from_slice(&sha1(&inner));
    sha1(&outer)
}

/// PBKDF2-HMAC-SHA1.
fn pbkdf2_sha1(password: &[u8], salt: &[u8], rounds: u32, out: &mut [u8]) {
    for (block, chunk) in out.chunks_mut(20).enumerate() {
        let mut s = salt.to_vec();
        s.extend_from_slice(&(block as u32 + 1).to_be_bytes());
        let mut u = hmac_sha1(password, &s);
        let mut t = u;
        for _ in 1..rounds {
            u = hmac_sha1(password, &u);
            for (a, b) in t.iter_mut().zip(u) {
                *a ^= b;
            }
        }
        chunk.copy_from_slice(&t[..chunk.len()]);
    }
}

/// The 32-byte key for a passphrase (8 to 63 characters) on the network called `ssid`.
pub fn wpa_psk(passphrase: &str, ssid: &str) -> [u8; 32] {
    let mut out = [0u8; 32];
    pbkdf2_sha1(passphrase.as_bytes(), ssid.as_bytes(), 4096, &mut out);
    out
}

pub fn wpa_psk_hex(passphrase: &str, ssid: &str) -> String {
    wpa_psk(passphrase, ssid).iter().map(|b| format!("{b:02x}")).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hex(b: &[u8]) -> String {
        b.iter().map(|x| format!("{x:02x}")).collect()
    }

    #[test]
    fn sha1_and_hmac_match_the_standard_test_vectors() {
        assert_eq!(hex(&sha1(b"abc")), "a9993e364706816aba3e25717850c26c9cd0d89d");
        assert_eq!(hex(&sha1(b"")), "da39a3ee5e6b4b0d3255bfef95601890afd80709");
        // RFC 2202 test case 2
        assert_eq!(hex(&hmac_sha1(b"Jefe", b"what do ya want for nothing?")), "effcdf6ae5eb2fa2d27416d5f184df9c259a7c79");
        // a key longer than a block is hashed first (RFC 2202 test case 6)
        assert_eq!(hex(&hmac_sha1(&[0xaa; 80], b"Test Using Larger Than Block-Size Key - Hash Key First")), "aa4ae5e15272d00e95705637ce8a3b55ed402112");
    }

    #[test]
    fn the_key_for_a_passphrase_matches_the_ieee_802_11i_test_vectors() {
        assert_eq!(wpa_psk_hex("password", "IEEE"), "f42c6fc52df0ebef9ebb4b90b38a5f902e83fe1b135a70e23aed762e9710a12e");
        assert_eq!(wpa_psk_hex("ThisIsAPassword", "ThisIsASSID"), "0dc0d6eb90555ed6419756b9a15ec3e3209b63df707dd508d14581f8982721af");
        // (checked against an independent PBKDF2 implementation)
        assert_eq!(wpa_psk_hex(&"a".repeat(63), &"Z".repeat(32)), "2d43d0dabfdd635377172efa1fc4b4b87dbfc4219193909ded9a7cfb89a3097b");
        assert_ne!(wpa_psk_hex("password", "IEEE"), wpa_psk_hex("password", "IEEF"), "the network name is part of the key");
    }
}
