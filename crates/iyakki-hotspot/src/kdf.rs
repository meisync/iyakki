//! Key derivation for WPA2-personal: PBKDF2-HMAC-SHA1 (RFC 2898, vectors in RFC 6070), the IEEE 802.11i pseudo-random function
//! (PRF over HMAC-SHA1, clause 8.5.1.1) and the pairwise transient key (PTK, clause 8.5.1.2). HMAC-SHA1 comes from `iyakki-sec`.

use crate::Mac;
use iyakki_sec::wpa::hmac_sha1;

/// PBKDF2 with HMAC-SHA1. `rounds` of 0 is treated as 1.
pub fn pbkdf2_hmac_sha1(password: &[u8], salt: &[u8], rounds: u32, out: &mut [u8]) {
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

/// The pairwise master key of a WPA2-personal network (4096 rounds, the SSID is the salt).
pub fn pmk_from_passphrase(passphrase: &str, ssid: &[u8]) -> [u8; 32] {
    let mut out = [0u8; 32];
    pbkdf2_hmac_sha1(passphrase.as_bytes(), ssid, 4096, &mut out);
    out
}

/// PRF-`bits` of IEEE 802.11i: HMAC-SHA1(key, a || 0 || b || counter), counter from 0, concatenated. `bits` is rounded up to bytes.
pub fn prf_sha1(key: &[u8], label: &str, data: &[u8], bits: usize) -> Vec<u8> {
    let want = bits.div_ceil(8);
    let mut out = Vec::with_capacity(want + 20);
    let mut counter = 0u8;
    while out.len() < want {
        let mut m = label.as_bytes().to_vec();
        m.push(0);
        m.extend_from_slice(data);
        m.push(counter);
        out.extend_from_slice(&hmac_sha1(key, &m));
        counter = counter.wrapping_add(1);
    }
    out.truncate(want);
    out
}

/// The three keys cut from the PTK for CCMP (384 bits).
#[derive(Clone, PartialEq, Eq)]
pub struct Ptk {
    /// Key confirmation key: the MIC of the handshake frames.
    pub kck: [u8; 16],
    /// Key encryption key: wraps the key data (the group key).
    pub kek: [u8; 16],
    /// Temporal key: the CCMP key the driver uses for this client.
    pub tk: [u8; 16],
}

impl std::fmt::Debug for Ptk {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Ptk(..)")
    }
}

/// PTK = PRF-384(PMK, "Pairwise key expansion", min(AA,SPA) || max(AA,SPA) || min(ANonce,SNonce) || max(ANonce,SNonce)).
pub fn derive_ptk(pmk: &[u8; 32], aa: &Mac, spa: &Mac, anonce: &[u8; 32], snonce: &[u8; 32]) -> Ptk {
    let mut data = Vec::with_capacity(76);
    data.extend_from_slice(std::cmp::min(aa, spa));
    data.extend_from_slice(std::cmp::max(aa, spa));
    data.extend_from_slice(std::cmp::min(anonce, snonce));
    data.extend_from_slice(std::cmp::max(anonce, snonce));
    let b = prf_sha1(pmk, "Pairwise key expansion", &data, 384);
    let mut p = Ptk { kck: [0; 16], kek: [0; 16], tk: [0; 16] };
    p.kck.copy_from_slice(&b[0..16]);
    p.kek.copy_from_slice(&b[16..32]);
    p.tk.copy_from_slice(&b[32..48]);
    p
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{hex, unhex};

    fn pb(p: &[u8], s: &[u8], c: u32, n: usize) -> String {
        let mut o = vec![0u8; n];
        pbkdf2_hmac_sha1(p, s, c, &mut o);
        hex(&o)
    }

    #[test]
    fn rfc_6070_pbkdf2_hmac_sha1_vectors() {
        assert_eq!(pb(b"password", b"salt", 1, 20), "0c60c80f961f0e71f3a9b524af6012062fe037a6");
        assert_eq!(pb(b"password", b"salt", 2, 20), "ea6c014dc72d6f8ccd1ed92ace1d41f0d8de8957");
        assert_eq!(pb(b"password", b"salt", 4096, 20), "4b007901b765489abead49d926f721d065a429c1");
        assert_eq!(pb(b"passwordPASSWORDpassword", b"saltSALTsaltSALTsaltSALTsaltSALTsalt", 4096, 25), "3d2eec4fe41c849b80c8d83662c0e44a8b291a964cf2f07038");
        assert_eq!(pb(b"pass\0word", b"sa\0lt", 4096, 16), "56fa6aa75548099dcc37d7f03425e0c3");
    }

    #[test]
    fn ieee_802_11i_annex_h_passphrase_to_pmk_vectors() {
        assert_eq!(hex(&pmk_from_passphrase("password", b"IEEE")), "f42c6fc52df0ebef9ebb4b90b38a5f902e83fe1b135a70e23aed762e9710a12e");
        assert_eq!(hex(&pmk_from_passphrase("ThisIsAPassword", b"ThisIsASSID")), "0dc0d6eb90555ed6419756b9a15ec3e3209b63df707dd508d14581f8982721af");
        // 63 characters and a 32-byte SSID, checked against an independent PBKDF2 (Python hashlib)
        assert_eq!(hex(&pmk_from_passphrase(&"a".repeat(63), &[b'Z'; 32])), "2d43d0dabfdd635377172efa1fc4b4b87dbfc4219193909ded9a7cfb89a3097b");
    }

    #[test]
    fn prf_matches_an_independent_implementation_and_cuts_to_length() {
        let key: Vec<u8> = (0..20).collect();
        assert_eq!(hex(&prf_sha1(&key, "prefix", b"data", 128)), "47cbe51469b527cd3aa53532138e365f");
        assert_eq!(hex(&prf_sha1(&key, "prefix", &[b'x'; 60], 384)), "4288e369669c4883deb49eb49974fc544e82a9916285a4a0656492926e6d4b4492708cda2405b69cf889faa075c734df");
        assert_eq!(prf_sha1(&key, "p", b"d", 100).len(), 13);
    }

    #[test]
    fn the_ptk_for_the_annex_h_pmk_and_the_c0_d0_nonces_matches_the_independent_value() {
        // PMK of the published "ThisIsAPassword"/"ThisIsASSID" vector; addresses and nonce patterns in the style of annex H.
        // The expected PTK was computed with an independent implementation (Python hmac), not copied from the standard.
        let pmk: [u8; 32] = unhex("0dc0d6eb90555ed6419756b9a15ec3e3209b63df707dd508d14581f8982721af").try_into().unwrap();
        let aa: Mac = [0xa0, 0xa1, 0xa1, 0xa3, 0xa4, 0xa5];
        let spa: Mac = [0xb0, 0xb1, 0xb2, 0xb3, 0xb4, 0xb5];
        let an: [u8; 32] = std::array::from_fn(|i| 0xc0 + i as u8);
        let sn: [u8; 32] = std::array::from_fn(|i| 0xd0 + i as u8);
        let p = derive_ptk(&pmk, &aa, &spa, &an, &sn);
        let all = format!("{}{}{}", hex(&p.kck), hex(&p.kek), hex(&p.tk));
        assert_eq!(all, "651caf71c271cb55bf18d344b4f2979ef2fbe819ca281513d10cb688633fcef5eb57ed09d0cb6392aa44f2049520ee2e");
        // both sides get the same keys whichever order the addresses and nonces are given in
        assert!(p == derive_ptk(&pmk, &spa, &aa, &sn, &an));
    }
}
