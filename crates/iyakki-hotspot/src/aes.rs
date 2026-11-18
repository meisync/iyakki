//! AES (FIPS-197) for 128, 192 and 256-bit keys: encrypt and decrypt one block. Only the key wrap (RFC 3394) uses it, so there
//! is no table-free constant-time claim here; the key wrap runs on the hotspot's own handshake data once per client.
//! The S-box is computed from the field arithmetic at first use instead of being typed in.

use std::sync::OnceLock;

struct Tables {
    sbox: [u8; 256],
    inv: [u8; 256],
}

fn tables() -> &'static Tables {
    static T: OnceLock<Tables> = OnceLock::new();
    T.get_or_init(|| {
        let mut sbox = [0u8; 256];
        let (mut p, mut q) = (1u8, 1u8);
        loop {
            // multiply p by 3, divide q by 3 (they stay inverse to each other)
            p = p ^ (p << 1) ^ if p & 0x80 != 0 { 0x1b } else { 0 };
            q ^= q << 1;
            q ^= q << 2;
            q ^= q << 4;
            if q & 0x80 != 0 {
                q ^= 0x09;
            }
            sbox[p as usize] = q ^ q.rotate_left(1) ^ q.rotate_left(2) ^ q.rotate_left(3) ^ q.rotate_left(4) ^ 0x63;
            if p == 1 {
                break;
            }
        }
        sbox[0] = 0x63;
        let mut inv = [0u8; 256];
        for (i, s) in sbox.iter().enumerate() {
            inv[*s as usize] = i as u8;
        }
        Tables { sbox, inv }
    })
}

fn xtime(x: u8) -> u8 {
    (x << 1) ^ if x & 0x80 != 0 { 0x1b } else { 0 }
}

fn mul(mut a: u8, mut b: u8) -> u8 {
    let mut r = 0;
    while b != 0 {
        if b & 1 != 0 {
            r ^= a;
        }
        a = xtime(a);
        b >>= 1;
    }
    r
}

/// An expanded AES key.
pub struct Aes {
    round_keys: Vec<[u8; 16]>,
}

impl Aes {
    /// `key` must be 16, 24 or 32 bytes; anything else gives `None`.
    pub fn new(key: &[u8]) -> Option<Aes> {
        if !matches!(key.len(), 16 | 24 | 32) {
            return None;
        }
        let t = tables();
        let nk = key.len() / 4;
        let nr = nk + 6;
        let mut w: Vec<[u8; 4]> = key.chunks(4).map(|c| [c[0], c[1], c[2], c[3]]).collect();
        let mut rcon = 1u8;
        for i in nk..4 * (nr + 1) {
            let mut tmp = w[i - 1];
            if i % nk == 0 {
                tmp = [t.sbox[tmp[1] as usize] ^ rcon, t.sbox[tmp[2] as usize], t.sbox[tmp[3] as usize], t.sbox[tmp[0] as usize]];
                rcon = xtime(rcon);
            } else if nk > 6 && i % nk == 4 {
                tmp = [t.sbox[tmp[0] as usize], t.sbox[tmp[1] as usize], t.sbox[tmp[2] as usize], t.sbox[tmp[3] as usize]];
            }
            let p = w[i - nk];
            w.push([p[0] ^ tmp[0], p[1] ^ tmp[1], p[2] ^ tmp[2], p[3] ^ tmp[3]]);
        }
        let round_keys = w
            .chunks(4)
            .map(|c| {
                let mut k = [0u8; 16];
                for (i, word) in c.iter().enumerate() {
                    k[4 * i..4 * i + 4].copy_from_slice(word);
                }
                k
            })
            .collect();
        Some(Aes { round_keys })
    }

    fn add(s: &mut [u8; 16], k: &[u8; 16]) {
        for (a, b) in s.iter_mut().zip(k) {
            *a ^= b;
        }
    }

    pub fn encrypt_block(&self, block: &[u8; 16]) -> [u8; 16] {
        let t = tables();
        let mut s = *block;
        let nr = self.round_keys.len() - 1;
        Self::add(&mut s, &self.round_keys[0]);
        for r in 1..=nr {
            for b in s.iter_mut() {
                *b = t.sbox[*b as usize];
            }
            // shift rows (state is column-major: index = 4 * column + row)
            let o = s;
            for c in 0..4 {
                for row in 0..4 {
                    s[4 * c + row] = o[4 * ((c + row) % 4) + row];
                }
            }
            if r != nr {
                for c in 0..4 {
                    let col = [s[4 * c], s[4 * c + 1], s[4 * c + 2], s[4 * c + 3]];
                    let all = col[0] ^ col[1] ^ col[2] ^ col[3];
                    for row in 0..4 {
                        s[4 * c + row] = col[row] ^ all ^ xtime(col[row] ^ col[(row + 1) % 4]);
                    }
                }
            }
            Self::add(&mut s, &self.round_keys[r]);
        }
        s
    }

    pub fn decrypt_block(&self, block: &[u8; 16]) -> [u8; 16] {
        let t = tables();
        let mut s = *block;
        let nr = self.round_keys.len() - 1;
        Self::add(&mut s, &self.round_keys[nr]);
        for r in (0..nr).rev() {
            // inverse shift rows
            let o = s;
            for c in 0..4 {
                for row in 0..4 {
                    s[4 * ((c + row) % 4) + row] = o[4 * c + row];
                }
            }
            for b in s.iter_mut() {
                *b = t.inv[*b as usize];
            }
            Self::add(&mut s, &self.round_keys[r]);
            if r != 0 {
                for c in 0..4 {
                    let col = [s[4 * c], s[4 * c + 1], s[4 * c + 2], s[4 * c + 3]];
                    for row in 0..4 {
                        s[4 * c + row] = mul(col[row], 14) ^ mul(col[(row + 1) % 4], 11) ^ mul(col[(row + 2) % 4], 13) ^ mul(col[(row + 3) % 4], 9);
                    }
                }
            }
        }
        s
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{hex, unhex};

    fn block(h: &str) -> [u8; 16] {
        unhex(h).try_into().unwrap()
    }

    #[test]
    fn fips_197_appendix_c_vectors_encrypt_and_decrypt_for_all_three_key_sizes() {
        let pt = block("00112233445566778899aabbccddeeff");
        for (key, ct) in [
            ("000102030405060708090a0b0c0d0e0f", "69c4e0d86a7b0430d8cdb78070b4c55a"),
            ("000102030405060708090a0b0c0d0e0f1011121314151617", "dda97ca4864cdfe06eaf70a0ec0d7191"),
            ("000102030405060708090a0b0c0d0e0f101112131415161718191a1b1c1d1e1f", "8ea2b7ca516745bfeafc49904b496089"),
        ] {
            let a = Aes::new(&unhex(key)).unwrap();
            assert_eq!(hex(&a.encrypt_block(&pt)), ct);
            assert_eq!(a.decrypt_block(&block(ct)), pt);
        }
    }

    #[test]
    fn fips_197_appendix_b_example_and_the_sbox_corners() {
        let a = Aes::new(&unhex("2b7e151628aed2a6abf7158809cf4f3c")).unwrap();
        assert_eq!(hex(&a.encrypt_block(&block("3243f6a8885a308d313198a2e0370734"))), "3925841d02dc09fbdc118597196a0b32");
        let t = tables();
        assert_eq!((t.sbox[0], t.sbox[1], t.sbox[0x53], t.sbox[0xff]), (0x63, 0x7c, 0xed, 0x16));
    }

    #[test]
    fn a_bad_key_length_is_refused() {
        assert!(Aes::new(&[0u8; 15]).is_none());
        assert!(Aes::new(&[]).is_none());
    }
}
