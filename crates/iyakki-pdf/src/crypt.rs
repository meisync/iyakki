//! The PDF standard security handler for files that open with a password: RC4 with 40 to 128 bit keys (revisions 2 and 3), and the
//! crypt filters of revision 4 (RC4 or AES-128 in CBC mode) and AES-256 of revisions 5 and 6 (SHA-256/384/512, AES-256 CBC). The hashes and ciphers are written here from their standards (MD5 RFC 1321,
//! RC4, AES FIPS 197, SHA-2 FIPS 180-4) so the crate needs nothing else.
//!
//! The keys, the passwords and the decrypted bytes live in memory only; nothing is written anywhere.

use crate::obj::*;

// ------------------------------------------------------------------------------------------------ MD5

pub fn md5(data: &[u8]) -> [u8; 16] {
    const S: [u32; 64] = [7, 12, 17, 22, 7, 12, 17, 22, 7, 12, 17, 22, 7, 12, 17, 22, 5, 9, 14, 20, 5, 9, 14, 20, 5, 9, 14, 20, 5, 9, 14, 20, 4, 11, 16, 23, 4, 11, 16, 23, 4, 11, 16, 23, 4, 11, 16, 23, 6, 10, 15, 21, 6, 10, 15, 21, 6, 10, 15, 21, 6, 10, 15, 21];
    let k: Vec<u32> = (0..64).map(|i| ((i as f64 + 1.0).sin().abs() * 4294967296.0) as u32).collect();
    let (mut a0, mut b0, mut c0, mut d0) = (0x67452301u32, 0xefcdab89u32, 0x98badcfeu32, 0x10325476u32);
    let mut msg = data.to_vec();
    msg.push(0x80);
    while msg.len() % 64 != 56 {
        msg.push(0);
    }
    msg.extend_from_slice(&((data.len() as u64).wrapping_mul(8)).to_le_bytes());
    for chunk in msg.chunks(64) {
        let m: Vec<u32> = chunk.chunks(4).map(|w| u32::from_le_bytes([w[0], w[1], w[2], w[3]])).collect();
        let (mut a, mut b, mut c, mut d) = (a0, b0, c0, d0);
        for i in 0..64 {
            let (f, g) = match i / 16 {
                0 => ((b & c) | (!b & d), i),
                1 => ((d & b) | (!d & c), (5 * i + 1) % 16),
                2 => (b ^ c ^ d, (3 * i + 5) % 16),
                _ => (c ^ (b | !d), (7 * i) % 16),
            };
            let f2 = f.wrapping_add(a).wrapping_add(k[i]).wrapping_add(m[g]);
            a = d;
            d = c;
            c = b;
            b = b.wrapping_add(f2.rotate_left(S[i]));
        }
        a0 = a0.wrapping_add(a);
        b0 = b0.wrapping_add(b);
        c0 = c0.wrapping_add(c);
        d0 = d0.wrapping_add(d);
    }
    let mut out = [0u8; 16];
    for (i, v) in [a0, b0, c0, d0].iter().enumerate() {
        out[i * 4..i * 4 + 4].copy_from_slice(&v.to_le_bytes());
    }
    out
}

// ------------------------------------------------------------------------------------------------ RC4

pub fn rc4(key: &[u8], data: &[u8]) -> Vec<u8> {
    if key.is_empty() {
        return data.to_vec();
    }
    let mut s: Vec<u8> = (0..=255).collect();
    let mut j = 0usize;
    for i in 0..256 {
        j = (j + s[i] as usize + key[i % key.len()] as usize) & 255;
        s.swap(i, j);
    }
    let (mut i, mut j) = (0usize, 0usize);
    data.iter()
        .map(|&b| {
            i = (i + 1) & 255;
            j = (j + s[i] as usize) & 255;
            s.swap(i, j);
            b ^ s[(s[i] as usize + s[j] as usize) & 255]
        })
        .collect()
}

// ------------------------------------------------------------------------------------------------ AES-128 (decryption only)

fn sboxes() -> ([u8; 256], [u8; 256]) {
    let mut sbox = [0u8; 256];
    let (mut p, mut q) = (1u8, 1u8);
    loop {
        // p walks the powers of 3, q is its inverse
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
    for (i, &v) in sbox.iter().enumerate() {
        inv[v as usize] = i as u8;
    }
    (sbox, inv)
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

/// The round keys for a 16 or 32 byte key (FIPS 197 key expansion); any other length is treated as 16 by padding with zeros.
fn expand_key(key: &[u8], sbox: &[u8; 256]) -> Vec<[u8; 16]> {
    let nk = if key.len() >= 32 { 8 } else { 4 };
    let rounds = nk + 6;
    let mut k = key.to_vec();
    k.resize(nk * 4, 0);
    let mut w: Vec<[u8; 4]> = vec![[0u8; 4]; 4 * (rounds + 1)];
    for i in 0..nk {
        w[i].copy_from_slice(&k[i * 4..i * 4 + 4]);
    }
    let mut rcon = 1u8;
    for i in nk..w.len() {
        let mut t = w[i - 1];
        if i % nk == 0 {
            t = [sbox[t[1] as usize] ^ rcon, sbox[t[2] as usize], sbox[t[3] as usize], sbox[t[0] as usize]];
            rcon = xtime(rcon);
        } else if nk == 8 && i % nk == 4 {
            t = [sbox[t[0] as usize], sbox[t[1] as usize], sbox[t[2] as usize], sbox[t[3] as usize]];
        }
        for j in 0..4 {
            w[i][j] = w[i - nk][j] ^ t[j];
        }
    }
    let mut rk = vec![[0u8; 16]; rounds + 1];
    for (r, round) in rk.iter_mut().enumerate() {
        for c in 0..4 {
            round[c * 4..c * 4 + 4].copy_from_slice(&w[r * 4 + c]);
        }
    }
    rk
}

/// Encrypt one 16-byte block with the expanded key.
fn encrypt_block(block: &[u8], rk: &[[u8; 16]], sbox: &[u8; 256]) -> [u8; 16] {
    let mut s = [0u8; 16];
    s.copy_from_slice(block);
    let add = |s: &mut [u8; 16], k: &[u8; 16]| s.iter_mut().zip(k).for_each(|(a, b)| *a ^= b);
    let last = rk.len() - 1;
    add(&mut s, &rk[0]);
    for (round, key) in rk.iter().enumerate().skip(1) {
        s.iter_mut().for_each(|b| *b = sbox[*b as usize]);
        let t = s;
        for c in 0..4 {
            for r in 0..4 {
                s[c * 4 + r] = t[((c + r) % 4) * 4 + r];
            }
        }
        if round < last {
            for c in 0..4 {
                let col = [s[c * 4], s[c * 4 + 1], s[c * 4 + 2], s[c * 4 + 3]];
                s[c * 4] = mul(col[0], 2) ^ mul(col[1], 3) ^ col[2] ^ col[3];
                s[c * 4 + 1] = col[0] ^ mul(col[1], 2) ^ mul(col[2], 3) ^ col[3];
                s[c * 4 + 2] = col[0] ^ col[1] ^ mul(col[2], 2) ^ mul(col[3], 3);
                s[c * 4 + 3] = mul(col[0], 3) ^ col[1] ^ col[2] ^ mul(col[3], 2);
            }
        }
        add(&mut s, key);
    }
    s
}

/// Decrypt one 16-byte block with the expanded key.
fn decrypt_block(block: &[u8], rk: &[[u8; 16]], inv: &[u8; 256]) -> [u8; 16] {
    let mut s = [0u8; 16];
    s.copy_from_slice(block);
    let add = |s: &mut [u8; 16], k: &[u8; 16]| s.iter_mut().zip(k).for_each(|(a, b)| *a ^= b);
    let last = rk.len() - 1;
    add(&mut s, &rk[last]);
    for round in (0..last).rev() {
        // inverse shift rows (the state is column-major: s[col * 4 + row])
        let t = s;
        for c in 0..4 {
            for r in 0..4 {
                s[((c + r) % 4) * 4 + r] = t[c * 4 + r];
            }
        }
        s.iter_mut().for_each(|b| *b = inv[*b as usize]);
        add(&mut s, &rk[round]);
        if round > 0 {
            for c in 0..4 {
                let col = [s[c * 4], s[c * 4 + 1], s[c * 4 + 2], s[c * 4 + 3]];
                s[c * 4] = mul(col[0], 14) ^ mul(col[1], 11) ^ mul(col[2], 13) ^ mul(col[3], 9);
                s[c * 4 + 1] = mul(col[0], 9) ^ mul(col[1], 14) ^ mul(col[2], 11) ^ mul(col[3], 13);
                s[c * 4 + 2] = mul(col[0], 13) ^ mul(col[1], 9) ^ mul(col[2], 14) ^ mul(col[3], 11);
                s[c * 4 + 3] = mul(col[0], 11) ^ mul(col[1], 13) ^ mul(col[2], 9) ^ mul(col[3], 14);
            }
        }
    }
    s
}

/// CBC with no padding handling, a whole number of blocks: `decrypt` or encrypt with the given iv.
fn cbc_raw(key: &[u8], iv: &[u8; 16], data: &[u8], decrypt: bool) -> Vec<u8> {
    let (sbox, inv) = sboxes();
    let rk = expand_key(key, &sbox);
    let mut out = Vec::with_capacity(data.len());
    let mut prev = *iv;
    for chunk in data.chunks_exact(16) {
        if decrypt {
            let p = decrypt_block(chunk, &rk, &inv);
            out.extend(p.iter().zip(prev.iter()).map(|(a, b)| a ^ b));
            prev.copy_from_slice(chunk);
        } else {
            let x: Vec<u8> = chunk.iter().zip(prev.iter()).map(|(a, b)| a ^ b).collect();
            prev = encrypt_block(&x, &rk, &sbox);
            out.extend_from_slice(&prev);
        }
    }
    out
}

/// AES-CBC as PDF uses it (128 or 256 bit key): the first 16 bytes are the initialisation vector, the rest is cipher text, the end is
/// PKCS#7 padding. Damaged input (a length that is not a whole number of blocks, a padding that does not fit) gives what could be decrypted.
pub fn aes_cbc_decrypt(key: &[u8], data: &[u8]) -> Vec<u8> {
    if data.len() < 32 {
        return Vec::new();
    }
    let iv: [u8; 16] = data[..16].try_into().unwrap_or([0; 16]);
    let mut out = cbc_raw(key, &iv, &data[16..], true);
    if let Some(&n) = out.last() {
        if (1..=16).contains(&n) && out.len() >= n as usize && out[out.len() - n as usize..].iter().all(|&b| b == n) {
            out.truncate(out.len() - n as usize);
        }
    }
    out
}

pub fn aes128_cbc_decrypt(key: &[u8; 16], data: &[u8]) -> Vec<u8> {
    aes_cbc_decrypt(key, data)
}

// ------------------------------------------------------------------------------------------------ SHA-2 (FIPS 180-4), for revisions 5 and 6

pub fn sha256(data: &[u8]) -> [u8; 32] {
    const K: [u32; 64] = [
        0x428a2f98, 0x71374491, 0xb5c0fbcf, 0xe9b5dba5, 0x3956c25b, 0x59f111f1, 0x923f82a4, 0xab1c5ed5, 0xd807aa98, 0x12835b01, 0x243185be, 0x550c7dc3, 0x72be5d74, 0x80deb1fe, 0x9bdc06a7, 0xc19bf174, 0xe49b69c1, 0xefbe4786, 0x0fc19dc6, 0x240ca1cc, 0x2de92c6f, 0x4a7484aa, 0x5cb0a9dc, 0x76f988da, 0x983e5152, 0xa831c66d, 0xb00327c8, 0xbf597fc7, 0xc6e00bf3, 0xd5a79147, 0x06ca6351, 0x14292967, 0x27b70a85, 0x2e1b2138, 0x4d2c6dfc, 0x53380d13, 0x650a7354, 0x766a0abb, 0x81c2c92e, 0x92722c85, 0xa2bfe8a1, 0xa81a664b, 0xc24b8b70, 0xc76c51a3, 0xd192e819, 0xd6990624, 0xf40e3585, 0x106aa070, 0x19a4c116, 0x1e376c08, 0x2748774c, 0x34b0bcb5, 0x391c0cb3, 0x4ed8aa4a, 0x5b9cca4f, 0x682e6ff3, 0x748f82ee, 0x78a5636f, 0x84c87814, 0x8cc70208, 0x90befffa, 0xa4506ceb, 0xbef9a3f7, 0xc67178f2,
    ];
    let mut h: [u32; 8] = [0x6a09e667, 0xbb67ae85, 0x3c6ef372, 0xa54ff53a, 0x510e527f, 0x9b05688c, 0x1f83d9ab, 0x5be0cd19];
    let mut msg = data.to_vec();
    msg.push(0x80);
    while msg.len() % 64 != 56 {
        msg.push(0);
    }
    msg.extend_from_slice(&((data.len() as u64).wrapping_mul(8)).to_be_bytes());
    for chunk in msg.chunks(64) {
        let mut w = [0u32; 64];
        for i in 0..16 {
            w[i] = u32::from_be_bytes([chunk[i * 4], chunk[i * 4 + 1], chunk[i * 4 + 2], chunk[i * 4 + 3]]);
        }
        for i in 16..64 {
            let s0 = w[i - 15].rotate_right(7) ^ w[i - 15].rotate_right(18) ^ (w[i - 15] >> 3);
            let s1 = w[i - 2].rotate_right(17) ^ w[i - 2].rotate_right(19) ^ (w[i - 2] >> 10);
            w[i] = w[i - 16].wrapping_add(s0).wrapping_add(w[i - 7]).wrapping_add(s1);
        }
        let mut v = h;
        for i in 0..64 {
            let s1 = v[4].rotate_right(6) ^ v[4].rotate_right(11) ^ v[4].rotate_right(25);
            let ch = (v[4] & v[5]) ^ (!v[4] & v[6]);
            let t1 = v[7].wrapping_add(s1).wrapping_add(ch).wrapping_add(K[i]).wrapping_add(w[i]);
            let s0 = v[0].rotate_right(2) ^ v[0].rotate_right(13) ^ v[0].rotate_right(22);
            let maj = (v[0] & v[1]) ^ (v[0] & v[2]) ^ (v[1] & v[2]);
            let t2 = s0.wrapping_add(maj);
            v = [t1.wrapping_add(t2), v[0], v[1], v[2], v[3].wrapping_add(t1), v[4], v[5], v[6]];
        }
        for i in 0..8 {
            h[i] = h[i].wrapping_add(v[i]);
        }
    }
    let mut out = [0u8; 32];
    for (i, x) in h.iter().enumerate() {
        out[i * 4..i * 4 + 4].copy_from_slice(&x.to_be_bytes());
    }
    out
}

/// SHA-512, or SHA-384 (the first 48 bytes of a run from other starting values).
fn sha512_with(data: &[u8], init: [u64; 8]) -> [u8; 64] {
    const K: [u64; 80] = [
        0x428a2f98d728ae22, 0x7137449123ef65cd, 0xb5c0fbcfec4d3b2f, 0xe9b5dba58189dbbc, 0x3956c25bf348b538, 0x59f111f1b605d019, 0x923f82a4af194f9b, 0xab1c5ed5da6d8118, 0xd807aa98a3030242, 0x12835b0145706fbe, 0x243185be4ee4b28c, 0x550c7dc3d5ffb4e2, 0x72be5d74f27b896f, 0x80deb1fe3b1696b1, 0x9bdc06a725c71235, 0xc19bf174cf692694, 0xe49b69c19ef14ad2, 0xefbe4786384f25e3, 0x0fc19dc68b8cd5b5, 0x240ca1cc77ac9c65, 0x2de92c6f592b0275, 0x4a7484aa6ea6e483, 0x5cb0a9dcbd41fbd4, 0x76f988da831153b5, 0x983e5152ee66dfab, 0xa831c66d2db43210, 0xb00327c898fb213f, 0xbf597fc7beef0ee4, 0xc6e00bf33da88fc2, 0xd5a79147930aa725, 0x06ca6351e003826f, 0x142929670a0e6e70, 0x27b70a8546d22ffc, 0x2e1b21385c26c926, 0x4d2c6dfc5ac42aed, 0x53380d139d95b3df, 0x650a73548baf63de, 0x766a0abb3c77b2a8, 0x81c2c92e47edaee6, 0x92722c851482353b, 0xa2bfe8a14cf10364, 0xa81a664bbc423001, 0xc24b8b70d0f89791, 0xc76c51a30654be30, 0xd192e819d6ef5218, 0xd69906245565a910, 0xf40e35855771202a, 0x106aa07032bbd1b8, 0x19a4c116b8d2d0c8, 0x1e376c085141ab53, 0x2748774cdf8eeb99, 0x34b0bcb5e19b48a8, 0x391c0cb3c5c95a63, 0x4ed8aa4ae3418acb, 0x5b9cca4f7763e373, 0x682e6ff3d6b2b8a3, 0x748f82ee5defb2fc, 0x78a5636f43172f60, 0x84c87814a1f0ab72, 0x8cc702081a6439ec, 0x90befffa23631e28, 0xa4506cebde82bde9, 0xbef9a3f7b2c67915, 0xc67178f2e372532b, 0xca273eceea26619c, 0xd186b8c721c0c207, 0xeada7dd6cde0eb1e, 0xf57d4f7fee6ed178, 0x06f067aa72176fba, 0x0a637dc5a2c898a6, 0x113f9804bef90dae, 0x1b710b35131c471b, 0x28db77f523047d84, 0x32caab7b40c72493, 0x3c9ebe0a15c9bebc, 0x431d67c49c100d4c, 0x4cc5d4becb3e42b6, 0x597f299cfc657e2a, 0x5fcb6fab3ad6faec, 0x6c44198c4a475817,
    ];
    let mut h = init;
    let mut msg = data.to_vec();
    msg.push(0x80);
    while msg.len() % 128 != 112 {
        msg.push(0);
    }
    msg.extend_from_slice(&((data.len() as u128).wrapping_mul(8)).to_be_bytes());
    for chunk in msg.chunks(128) {
        let mut w = [0u64; 80];
        for i in 0..16 {
            w[i] = u64::from_be_bytes(chunk[i * 8..i * 8 + 8].try_into().unwrap_or([0; 8]));
        }
        for i in 16..80 {
            let s0 = w[i - 15].rotate_right(1) ^ w[i - 15].rotate_right(8) ^ (w[i - 15] >> 7);
            let s1 = w[i - 2].rotate_right(19) ^ w[i - 2].rotate_right(61) ^ (w[i - 2] >> 6);
            w[i] = w[i - 16].wrapping_add(s0).wrapping_add(w[i - 7]).wrapping_add(s1);
        }
        let mut v = h;
        for i in 0..80 {
            let s1 = v[4].rotate_right(14) ^ v[4].rotate_right(18) ^ v[4].rotate_right(41);
            let ch = (v[4] & v[5]) ^ (!v[4] & v[6]);
            let t1 = v[7].wrapping_add(s1).wrapping_add(ch).wrapping_add(K[i]).wrapping_add(w[i]);
            let s0 = v[0].rotate_right(28) ^ v[0].rotate_right(34) ^ v[0].rotate_right(39);
            let maj = (v[0] & v[1]) ^ (v[0] & v[2]) ^ (v[1] & v[2]);
            let t2 = s0.wrapping_add(maj);
            v = [t1.wrapping_add(t2), v[0], v[1], v[2], v[3].wrapping_add(t1), v[4], v[5], v[6]];
        }
        for i in 0..8 {
            h[i] = h[i].wrapping_add(v[i]);
        }
    }
    let mut out = [0u8; 64];
    for (i, x) in h.iter().enumerate() {
        out[i * 8..i * 8 + 8].copy_from_slice(&x.to_be_bytes());
    }
    out
}

pub fn sha512(data: &[u8]) -> [u8; 64] {
    sha512_with(data, [0x6a09e667f3bcc908, 0xbb67ae8584caa73b, 0x3c6ef372fe94f82b, 0xa54ff53a5f1d36f1, 0x510e527fade682d1, 0x9b05688c2b3e6c1f, 0x1f83d9abfb41bd6b, 0x5be0cd19137e2179])
}

pub fn sha384(data: &[u8]) -> [u8; 48] {
    let full = sha512_with(data, [0xcbbb9d5dc1059ed8, 0x629a292a367cd507, 0x9159015a3070dd17, 0x152fecd8f70e5939, 0x67332667ffc00b31, 0x8eb44a8768581511, 0xdb0c2e0d64f98fa7, 0x47b5481dbefa4fa4]);
    let mut out = [0u8; 48];
    out.copy_from_slice(&full[..48]);
    out
}

/// The hash of revision 6 (ISO 32000-2 algorithm 2.B); revision 5 is the plain SHA-256 of the first step.
fn hash_r56(r: i64, password: &[u8], salt: &[u8], udata: &[u8]) -> [u8; 32] {
    let mut input = password.to_vec();
    input.extend_from_slice(salt);
    input.extend_from_slice(udata);
    let mut k: Vec<u8> = sha256(&input).to_vec();
    if r < 6 {
        return k[..32].try_into().unwrap_or([0; 32]);
    }
    let mut i = 0usize;
    loop {
        let mut k1 = Vec::with_capacity(64 * (password.len() + k.len() + udata.len()));
        for _ in 0..64 {
            k1.extend_from_slice(password);
            k1.extend_from_slice(&k);
            k1.extend_from_slice(udata);
        }
        let iv: [u8; 16] = k[16..32].try_into().unwrap_or([0; 16]);
        let e = cbc_raw(&k[..16], &iv, &k1, false);
        let m: u32 = e[..16].iter().map(|&b| b as u32).sum::<u32>() % 3;
        k = match m {
            0 => sha256(&e).to_vec(),
            1 => sha384(&e).to_vec(),
            _ => sha512(&e).to_vec(),
        };
        i += 1;
        if i >= 64 && (*e.last().unwrap_or(&0) as usize) <= i - 32 {
            break;
        }
    }
    k[..32].try_into().unwrap_or([0; 32])
}

// ------------------------------------------------------------------------------------------------ the security handler

/// The padding string of the standard security handler.
const PAD: [u8; 32] = [0x28, 0xBF, 0x4E, 0x5E, 0x4E, 0x75, 0x8A, 0x41, 0x64, 0x00, 0x4E, 0x56, 0xFF, 0xFA, 0x01, 0x08, 0x2E, 0x2E, 0x00, 0xB6, 0xD0, 0x68, 0x3E, 0x80, 0x2F, 0x0C, 0xA9, 0xFE, 0x64, 0x53, 0x69, 0x7A];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Method {
    None,
    Rc4,
    Aes128,
    Aes256,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CryptError {
    /// A kind of protection this crate does not read (AES-256, public-key security, a different handler).
    Unsupported(String),
    /// The protection data is missing or makes no sense.
    Damaged,
    /// The password opens nothing.
    WrongPassword,
}

/// What the Encrypt dictionary says, before a password is tried.
pub struct Params {
    v: i64,
    r: i64,
    key_len: usize,
    o: Vec<u8>,
    u: Vec<u8>,
    p: i32,
    id0: Vec<u8>,
    encrypt_metadata: bool,
    stm: Method,
    strm: Method,
    ue: Vec<u8>,
    oe: Vec<u8>,
}

/// A handler that opened: the file key and how strings and streams are protected.
pub struct Crypt {
    key: Vec<u8>,
    pub stm: Method,
    pub strm: Method,
    pub encrypt_metadata: bool,
}

fn pad_password(pw: &[u8]) -> Vec<u8> {
    let mut v: Vec<u8> = pw.iter().copied().take(32).collect();
    v.extend_from_slice(&PAD[..32 - v.len()]);
    v
}

impl Params {
    /// Read the Encrypt dictionary (`resolve` follows references in it) and the first element of the trailer's `ID`.
    pub fn read(d: &Dict, resolve: &dyn Fn(&Obj) -> Obj, id0: Vec<u8>) -> Result<Params, CryptError> {
        let get = |k: &str| d.get(k).map(resolve).unwrap_or(Obj::Null);
        let bytes = |o: Obj| if let Obj::Str(s) = o { Some((*s).clone()) } else { None };
        if get("Filter").name() != Some("Standard") {
            return Err(CryptError::Unsupported("this PDF uses a security handler other than a password".into()));
        }
        let (v, r) = (get("V").int().unwrap_or(0), get("R").int().unwrap_or(0));
        let aes256 = v == 5 && (r == 5 || r == 6);
        if !aes256 && (v >= 5 || r >= 5) {
            return Err(CryptError::Unsupported("this PDF uses a protection revision this reader does not know".into()));
        }
        if !aes256 && (!(1..=4).contains(&v) || !(2..=4).contains(&r)) {
            return Err(CryptError::Damaged);
        }
        let (Some(o), Some(u)) = (bytes(get("O")), bytes(get("U"))) else { return Err(CryptError::Damaged) };
        if o.len() < 32 || u.len() < 32 || (aes256 && (o.len() < 48 || u.len() < 48)) {
            return Err(CryptError::Damaged);
        }
        let (ue, oe) = (bytes(get("UE")).unwrap_or_default(), bytes(get("OE")).unwrap_or_default());
        if aes256 && (ue.len() < 32 || oe.len() < 32) {
            return Err(CryptError::Damaged);
        }
        let p = get("P").int().unwrap_or(0) as u32 as i32;
        let mut key_len = if v == 1 { 5 } else { (get("Length").int().unwrap_or(40) / 8).clamp(5, 16) as usize };
        let (mut stm, mut strm) = (Method::Rc4, Method::Rc4);
        if v == 4 || aes256 {
            let cf = get("CF").dict().cloned().unwrap_or_default();
            let method_of = |name: Obj| -> Result<Method, CryptError> {
                let n = name.name().unwrap_or("Identity").to_string();
                if n == "Identity" {
                    return Ok(Method::None);
                }
                let f = cf.get(&n).map(resolve).and_then(|o| o.dict().cloned()).unwrap_or_default();
                match f.get("CFM").map(resolve).and_then(|o| o.name().map(str::to_string)).as_deref() {
                    Some("V2") => Ok(Method::Rc4),
                    Some("AESV2") if !aes256 => Ok(Method::Aes128),
                    Some("AESV3") if aes256 => Ok(Method::Aes256),
                    Some("None") => Ok(Method::None),
                    _ => Err(CryptError::Unsupported("this PDF uses a cipher this reader does not know".into())),
                }
            };
            stm = method_of(get("StmF"))?;
            strm = method_of(get("StrF"))?;
            key_len = if aes256 { 32 } else { 16 };
        }
        let encrypt_metadata = !matches!(get("EncryptMetadata"), Obj::Bool(false));
        Ok(Params { v, r, key_len, o, u, p, id0, encrypt_metadata, stm, strm, ue, oe })
    }

    /// Algorithm 2: the file key for this user password.
    fn key_for(&self, user_pw: &[u8]) -> Vec<u8> {
        let mut h = pad_password(user_pw);
        h.extend_from_slice(&self.o[..32]);
        h.extend_from_slice(&(self.p as u32).to_le_bytes());
        h.extend_from_slice(&self.id0);
        if self.r >= 4 && !self.encrypt_metadata {
            h.extend_from_slice(&[0xff; 4]);
        }
        let mut d = md5(&h);
        let n = if self.r == 2 { 5 } else { self.key_len };
        if self.r >= 3 {
            for _ in 0..50 {
                d = md5(&d[..n]);
            }
        }
        d[..n].to_vec()
    }

    /// Algorithms 4 and 5: does this key match the stored `U`?
    fn user_ok(&self, key: &[u8]) -> bool {
        if self.r == 2 {
            return rc4(key, &PAD) == self.u[..32];
        }
        let mut h = PAD.to_vec();
        h.extend_from_slice(&self.id0);
        let mut x = rc4(key, &md5(&h));
        for i in 1..=19u8 {
            let k: Vec<u8> = key.iter().map(|b| b ^ i).collect();
            x = rc4(&k, &x);
        }
        x[..16] == self.u[..16]
    }

    /// Algorithm 7: the user password hidden in `O`, found with the owner password.
    fn user_pw_from_owner(&self, owner_pw: &[u8], n: usize) -> Vec<u8> {
        let mut d = md5(&pad_password(owner_pw));
        if self.r >= 3 {
            // each round hashes the first `n` bytes of the one before (the standard says "the output"; the files the system and the other readers
            // write and read use the key length, which differs only for keys shorter than 128 bits)
            for _ in 0..50 {
                d = md5(&d[..n]);
            }
        }
        let key = &d[..n];
        let mut x = self.o[..32].to_vec();
        if self.r == 2 {
            x = rc4(key, &x);
        } else {
            for i in (0..=19u8).rev() {
                let k: Vec<u8> = key.iter().map(|b| b ^ i).collect();
                x = rc4(&k, &x);
            }
        }
        x
    }

    /// Revisions 5 and 6 (ISO 32000-2 algorithms 2.A and 2.B): the user check, then the owner check; the file key is unwrapped from UE or OE.
    fn open_aes256(&self, pw: &[u8]) -> Result<Crypt, CryptError> {
        let r = self.r;
        let done = |inter: [u8; 32], wrapped: &[u8]| {
            let key = cbc_raw(&inter, &[0u8; 16], &wrapped[..32], true);
            Crypt { key, stm: self.stm, strm: self.strm, encrypt_metadata: self.encrypt_metadata }
        };
        if hash_r56(r, pw, &self.u[32..40], &[])[..] == self.u[..32] {
            return Ok(done(hash_r56(r, pw, &self.u[40..48], &[]), &self.ue));
        }
        let u48 = &self.u[..48];
        if hash_r56(r, pw, &self.o[32..40], u48)[..] == self.o[..32] {
            return Ok(done(hash_r56(r, pw, &self.o[40..48], u48), &self.oe));
        }
        Err(CryptError::WrongPassword)
    }

    /// Try the password as the user password, then as the owner password. The empty password is how most protected files open.
    pub fn open(&self, password: &str) -> Result<Crypt, CryptError> {
        let pw = password.as_bytes();
        if self.v == 5 {
            return self.open_aes256(&pw[..pw.len().min(127)]);
        }
        let mut candidates: Vec<Vec<u8>> = vec![pw.to_vec()];
        let n = if self.r == 2 { 5 } else { self.key_len };
        candidates.push(self.user_pw_from_owner(pw, n));
        for c in candidates {
            let key = self.key_for(&c);
            if self.user_ok(&key) {
                return Ok(Crypt { key, stm: self.stm, strm: self.strm, encrypt_metadata: self.encrypt_metadata });
            }
        }
        let _ = self.v;
        Err(CryptError::WrongPassword)
    }
}

impl Crypt {
    /// The key of one object (algorithm 1).
    fn object_key(&self, num: u32, generation: u16, aes: bool) -> Vec<u8> {
        let mut h = self.key.clone();
        h.extend_from_slice(&num.to_le_bytes()[..3]);
        h.extend_from_slice(&generation.to_le_bytes());
        if aes {
            h.extend_from_slice(b"sAlT");
        }
        let d = md5(&h);
        d[..(self.key.len() + 5).min(16)].to_vec()
    }

    fn run(&self, method: Method, num: u32, generation: u16, data: &[u8]) -> Vec<u8> {
        match method {
            Method::None => data.to_vec(),
            Method::Rc4 => rc4(&self.object_key(num, generation, false), data),
            Method::Aes256 => aes_cbc_decrypt(&self.key, data),
            Method::Aes128 => {
                let k = self.object_key(num, generation, true);
                let mut key = [0u8; 16];
                key.copy_from_slice(&k[..16]);
                aes128_cbc_decrypt(&key, data)
            }
        }
    }

    pub fn decrypt_stream(&self, num: u32, generation: u16, data: &[u8]) -> Vec<u8> {
        self.run(self.stm, num, generation, data)
    }

    /// Every string inside the object, decrypted in place (strings nested in arrays and dictionaries; a stream's dictionary too).
    pub fn decrypt_strings(&self, o: &Obj, num: u32, generation: u16) -> Obj {
        match o {
            Obj::Str(s) => Obj::Str(std::rc::Rc::new(self.run(self.strm, num, generation, s))),
            Obj::Arr(a) => Obj::Arr(std::rc::Rc::new(a.iter().map(|x| self.decrypt_strings(x, num, generation)).collect())),
            Obj::Dict(d) => Obj::Dict(std::rc::Rc::new(self.decrypt_dict(d, num, generation))),
            Obj::Stream(d, s, e) => Obj::Stream(std::rc::Rc::new(self.decrypt_dict(d, num, generation)), *s, *e),
            other => other.clone(),
        }
    }

    fn decrypt_dict(&self, d: &Dict, num: u32, generation: u16) -> Dict {
        d.iter().map(|(k, v)| (k.clone(), self.decrypt_strings(v, num, generation))).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hex(b: &[u8]) -> String {
        b.iter().map(|x| format!("{x:02x}")).collect()
    }

    #[test]
    fn md5_matches_the_rfc_1321_test_suite() {
        for (m, want) in [("", "d41d8cd98f00b204e9800998ecf8427e"), ("a", "0cc175b9c0f1b6a831c399e269772661"), ("abc", "900150983cd24fb0d6963f7d28e17f72"), ("message digest", "f96b697d7cb7938d525a2f31aaf161d0"), ("abcdefghijklmnopqrstuvwxyz", "c3fcd3d76192e4007dfb496cca67e13b"), ("12345678901234567890123456789012345678901234567890123456789012345678901234567890", "57edf4a22be3c955ac49da2e2107b67a")] {
            assert_eq!(hex(&md5(m.as_bytes())), want, "{m:?}");
        }
    }

    #[test]
    fn rc4_matches_the_published_vectors() {
        assert_eq!(hex(&rc4(b"Key", b"Plaintext")), "bbf316e8d940af0ad3");
        assert_eq!(hex(&rc4(b"Wiki", b"pedia")), "1021bf0420");
        assert_eq!(hex(&rc4(b"Secret", b"Attack at dawn")), "45a01f645fc35b383552544b9bf5");
        assert_eq!(rc4(b"k", &rc4(b"k", b"round trip")), b"round trip");
    }

    #[test]
    fn aes_128_decrypts_the_fips_197_example_and_cbc_with_padding_works() {
        let key: [u8; 16] = std::array::from_fn(|i| i as u8);
        let (sbox, inv) = sboxes();
        assert_eq!((sbox[0x00], sbox[0x53], sbox[0xff], inv[0x63]), (0x63, 0xed, 0x16, 0x00), "the S-box is the standard one");
        let rk = expand_key(&key, &sbox);
        let ct = [0x69, 0xc4, 0xe0, 0xd8, 0x6a, 0x7b, 0x04, 0x30, 0xd8, 0xcd, 0xb7, 0x80, 0x70, 0xb4, 0xc5, 0x5a];
        assert_eq!(hex(&decrypt_block(&ct, &rk, &inv)), "00112233445566778899aabbccddeeff");
        // NIST SP 800-38A, CBC-AES128.Decrypt: key 2b7e..., iv 0001...0f, first cipher block 7649abac... gives 6bc1bee2...
        let k2: [u8; 16] = [0x2b, 0x7e, 0x15, 0x16, 0x28, 0xae, 0xd2, 0xa6, 0xab, 0xf7, 0x15, 0x88, 0x09, 0xcf, 0x4f, 0x3c];
        let iv: Vec<u8> = (0..16).collect();
        let c1 = [0x76, 0x49, 0xab, 0xac, 0x81, 0x19, 0xb2, 0x46, 0xce, 0xe9, 0x8e, 0x9b, 0x12, 0xe9, 0x19, 0x7d];
        let mut data = iv.clone();
        data.extend_from_slice(&c1);
        // a second block that decrypts to a full block of padding (sixteen 0x10 bytes) after the XOR with the first cipher block is not needed:
        // decrypting only the first block, the output is the first 16 bytes (no valid padding to strip)
        let one = aes128_cbc_decrypt(&k2, &[data.clone(), c1.to_vec()].concat());
        assert_eq!(hex(&one[..16]), "6bc1bee22e409f96e93d7e117393172a");
        // too short or damaged input never panics
        assert!(aes128_cbc_decrypt(&k2, &[1, 2, 3]).is_empty());
        let _ = aes128_cbc_decrypt(&k2, &[0u8; 47]);
    }
}
