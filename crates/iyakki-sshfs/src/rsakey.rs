//! An RSA host key for the SSH server, made here (F-FILE-14): `ring` cannot generate RSA keys. Key generation (two random primes, CRT
//! parts), the PKCS#1 v1.5 signature with SHA-256 or SHA-512 (`rsa-sha2-256`, `rsa-sha2-512`; never SHA-1) and the PKCS#1 DER form the
//! key is kept in. The signature is checked in the tests by `ring`, which is an independent implementation.

use crate::bignum::{random_prime, Big, Fill};
use crate::sshkey::{fingerprint_of_blob, put_str};
use sha2::{Digest, Sha256, Sha512};

pub const DEFAULT_BITS: usize = 3072;
const E: u64 = 65537;

pub struct RsaHostKey {
    n: Big,
    e: Big,
    d: Big,
    p: Big,
    q: Big,
    dp: Big,
    dq: Big,
    qinv: Big,
}

fn mpint(v: &Big) -> Vec<u8> {
    let mut t = v.to_be_min();
    if t.first().is_none_or(|b| b & 0x80 != 0) {
        t.insert(0, 0);
    }
    t
}

fn der_len(o: &mut Vec<u8>, n: usize) {
    if n < 0x80 {
        o.push(n as u8);
    } else if n < 0x100 {
        o.extend([0x81, n as u8]);
    } else {
        o.extend([0x82, (n >> 8) as u8, n as u8]);
    }
}

fn der_int(o: &mut Vec<u8>, v: &Big) {
    let m = mpint(v);
    o.push(2);
    der_len(o, m.len());
    o.extend(m);
}

/// One DER element: (tag, content, rest).
fn der_next(b: &[u8]) -> Option<(u8, &[u8], &[u8])> {
    let (&tag, b) = b.split_first()?;
    let (&l0, b) = b.split_first()?;
    let (len, b) = if l0 < 0x80 {
        (l0 as usize, b)
    } else {
        let k = (l0 & 0x7f) as usize;
        if k == 0 || k > 2 || b.len() < k {
            return None;
        }
        (b[..k].iter().fold(0usize, |a, x| (a << 8) | *x as usize), &b[k..])
    };
    if b.len() < len {
        return None;
    }
    Some((tag, &b[..len], &b[len..]))
}

impl RsaHostKey {
    /// A new key of `bits` bits (2048 to 8192).
    pub fn generate(bits: usize, rng: &mut dyn Fill) -> Result<RsaHostKey, String> {
        if !(2048..=8192).contains(&bits) || bits % 2 != 0 {
            return Err("an RSA host key needs 2048 to 8192 bits".into());
        }
        let e = Big::from_u64(E);
        let one = Big::from_u64(1);
        loop {
            let p = random_prime(rng, bits / 2, &e);
            let q = random_prime(rng, bits / 2, &e);
            // primes too close together make the key easy to factor
            let diff = if p.cmp_big(&q).is_gt() { p.sub(&q) } else { q.sub(&p) };
            if diff.bit_len() < bits / 2 - 100 {
                continue;
            }
            let (p, q) = if p.cmp_big(&q).is_gt() { (p, q) } else { (q, p) };
            let n = p.mul(&q);
            if n.bit_len() != bits {
                continue;
            }
            let (p1, q1) = (p.sub(&one), q.sub(&one));
            let lambda = p1.mul(&q1).divrem(&p1.gcd(&q1)).0;
            let Some(d) = e.mod_inv(&lambda) else { continue };
            let Some(qinv) = q.mod_inv(&p) else { continue };
            return Ok(RsaHostKey { dp: d.rem(&p1), dq: d.rem(&q1), n, e, d, p, q, qinv });
        }
    }

    pub fn bits(&self) -> usize {
        self.n.bit_len()
    }

    pub fn public_blob(&self) -> Vec<u8> {
        let mut b = Vec::new();
        put_str(&mut b, b"ssh-rsa");
        put_str(&mut b, &mpint(&self.e));
        put_str(&mut b, &mpint(&self.n));
        b
    }

    pub fn fingerprint(&self) -> String {
        fingerprint_of_blob(&self.public_blob())
    }

    /// PKCS#1 `RSAPrivateKey` (version 0, n, e, d, p, q, dp, dq, qinv), the form it is stored in.
    pub fn to_der(&self) -> Vec<u8> {
        let mut body = vec![2, 1, 0];
        for v in [&self.n, &self.e, &self.d, &self.p, &self.q, &self.dp, &self.dq, &self.qinv] {
            der_int(&mut body, v);
        }
        let mut out = vec![0x30];
        der_len(&mut out, body.len());
        out.extend(body);
        out
    }

    pub fn from_der(der: &[u8]) -> Result<RsaHostKey, String> {
        let bad = || "the RSA host key is damaged".to_string();
        let (tag, body, rest) = der_next(der).ok_or_else(bad)?;
        if tag != 0x30 || !rest.is_empty() {
            return Err(bad());
        }
        let mut cur = body;
        let mut ints = Vec::new();
        while !cur.is_empty() {
            let (t, c, r) = der_next(cur).ok_or_else(bad)?;
            if t != 2 {
                return Err(bad());
            }
            ints.push(Big::from_be(c));
            cur = r;
        }
        if ints.len() != 9 || !ints[0].is_zero() {
            return Err(bad());
        }
        let mut it = ints.into_iter().skip(1);
        let mut next = || it.next().ok_or_else(bad);
        let key = RsaHostKey { n: next()?, e: next()?, d: next()?, p: next()?, q: next()?, dp: next()?, dq: next()?, qinv: next()? };
        // the parts must agree, or a damaged file would sign nonsense
        let one = Big::from_u64(1);
        if key.p.mul(&key.q) != key.n || !(2048..=8192).contains(&key.n.bit_len()) || key.dp != key.d.rem(&key.p.sub(&one)) || key.dq != key.d.rem(&key.q.sub(&one)) || key.q.mod_mul(&key.qinv, &key.p) != one {
            return Err(bad());
        }
        Ok(key)
    }

    /// `string algo`, `string signature` for `rsa-sha2-256` or `rsa-sha2-512`; None for any other name.
    pub fn sign_wire(&self, algo: &str, data: &[u8]) -> Option<Vec<u8>> {
        let (prefix, hash): (&[u8], Vec<u8>) = match algo {
            "rsa-sha2-256" => (&[0x30, 0x31, 0x30, 0x0d, 0x06, 0x09, 0x60, 0x86, 0x48, 0x01, 0x65, 0x03, 0x04, 0x02, 0x01, 0x05, 0x00, 0x04, 0x20], Sha256::digest(data).to_vec()),
            "rsa-sha2-512" => (&[0x30, 0x51, 0x30, 0x0d, 0x06, 0x09, 0x60, 0x86, 0x48, 0x01, 0x65, 0x03, 0x04, 0x02, 0x03, 0x05, 0x00, 0x04, 0x40], Sha512::digest(data).to_vec()),
            _ => return None,
        };
        let k = self.n.bit_len().div_ceil(8);
        let t_len = prefix.len() + hash.len();
        if k < t_len + 11 {
            return None;
        }
        let mut em = vec![0u8, 1];
        em.extend(std::iter::repeat_n(0xff, k - t_len - 3));
        em.push(0);
        em.extend_from_slice(prefix);
        em.extend_from_slice(&hash);
        let m = Big::from_be(&em);
        // the CRT form (about four times faster), then a check against the public key so a fault never leaves the machine
        let m1 = m.rem(&self.p).mod_pow(&self.dp, &self.p);
        let m2 = m.rem(&self.q).mod_pow(&self.dq, &self.q);
        let h = m1.mod_sub(&m2, &self.p).mod_mul(&self.qinv, &self.p);
        let s = m2.add(&h.mul(&self.q));
        if s.mod_pow(&self.e, &self.n) != m {
            return None;
        }
        let mut sig = Vec::new();
        put_str(&mut sig, algo.as_bytes());
        put_str(&mut sig, &s.to_be(k));
        Some(sig)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bignum::SystemFill;

    #[test]
    fn a_2048_bit_key_is_made_signs_and_ring_checks_the_signature() {
        let key = RsaHostKey::generate(2048, &mut SystemFill).unwrap();
        assert_eq!(key.bits(), 2048);
        let blob = key.public_blob();
        assert!(crate::sshalg::valid_user_blob(&blob));
        for algo in ["rsa-sha2-256", "rsa-sha2-512"] {
            for n in 0..3 {
                let data = format!("exchange hash {n}");
                let sig = key.sign_wire(algo, data.as_bytes()).unwrap();
                assert!(crate::sshalg::verify_user(algo.as_bytes(), &blob, &sig, data.as_bytes()), "{algo} {n}");
                assert!(!crate::sshalg::verify_user(algo.as_bytes(), &blob, &sig, b"another"));
            }
        }
        assert!(key.sign_wire("ssh-rsa", b"x").is_none(), "never the SHA-1 signature");
        assert!(key.fingerprint().starts_with("SHA256:"));
        // the stored form comes back the same and signs the same bytes (PKCS#1 v1.5 is deterministic)
        let again = RsaHostKey::from_der(&key.to_der()).unwrap();
        assert_eq!(again.public_blob(), blob);
        assert_eq!(again.sign_wire("rsa-sha2-256", b"x"), key.sign_wire("rsa-sha2-256", b"x"));
    }

    #[test]
    fn a_damaged_stored_key_is_refused_and_small_sizes_are_not_made() {
        let key = RsaHostKey::generate(2048, &mut SystemFill).unwrap();
        let der = key.to_der();
        assert!(RsaHostKey::from_der(&[]).is_err() && RsaHostKey::from_der(&der[..der.len() - 1]).is_err());
        let mut bad = der.clone();
        let last = bad.len() - 3;
        bad[last] ^= 1; // inside qinv
        assert!(RsaHostKey::from_der(&bad).is_err());
        assert!(RsaHostKey::generate(1024, &mut SystemFill).is_err());
        assert!(RsaHostKey::generate(2047, &mut SystemFill).is_err());
    }
}
