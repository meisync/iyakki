//! ECDSA over NIST P-521 with SHA-512 (`ecdsa-sha2-nistp521`), written here because `ring` has no P-521 (F-FILE-14). Used to CHECK login
//! signatures (public data, so the lack of constant time does not matter); a signer exists for the tests only. Points are kept in
//! Jacobian form with a = -3, on the integer type of `bignum.rs`.

use crate::bignum::Big;
use sha2::{Digest, Sha512};
use std::cmp::Ordering;

const B_HEX: &str = "0051953eb9618e1c9a1f929a21a0b68540eea2da725b99b315f3b8b489918ef109e156193951ec7e937b1652c0bd3bb1bf073573df883d2c34f1ef451fd46b503f00";
const GX_HEX: &str = "00c6858e06b70404e9cd9e3ecb662395b4429c648139053fb521f828af606b4d3dbaa14b5e77efe75928fe1dc127a2ffa8de3348b3c1856a429bf97e7e31c2e5bd66";
const GY_HEX: &str = "011839296a789a3bc0045c8a5fb42c7d1bd998f54449579b446817afbd17273e662c97ee72995ef42640c550b9013fad0761353c7086a272c24088be94769fd16650";
const N_HEX: &str = "01fffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffa51868783bf2f966b7fcc0148f709a5d03bb5c9b8899c47aebb6fb71e91386409";

/// Bytes of a field element or a scalar.
pub const FIELD_BYTES: usize = 66;
/// An uncompressed point: 0x04, x, y.
pub const POINT_BYTES: usize = 1 + 2 * FIELD_BYTES;

struct Curve {
    p: Big,
    n: Big,
    b: Big,
    g: (Big, Big),
}

fn curve() -> Curve {
    let p = Big::from_u64(1).shl(521).sub(&Big::from_u64(1));
    Curve { p, n: Big::from_hex(N_HEX), b: Big::from_hex(B_HEX), g: (Big::from_hex(GX_HEX), Big::from_hex(GY_HEX)) }
}

/// A Jacobian point; z = 0 is the point at infinity.
#[derive(Clone)]
struct Jac {
    x: Big,
    y: Big,
    z: Big,
}

impl Jac {
    fn inf() -> Jac {
        Jac { x: Big::from_u64(1), y: Big::from_u64(1), z: Big::zero() }
    }
    fn from_affine(p: &(Big, Big)) -> Jac {
        Jac { x: p.0.clone(), y: p.1.clone(), z: Big::from_u64(1) }
    }
    fn is_inf(&self) -> bool {
        self.z.is_zero()
    }
}

fn dbl(c: &Curve, a: &Jac) -> Jac {
    if a.is_inf() || a.y.is_zero() {
        return Jac::inf();
    }
    let p = &c.p;
    let m = |x: &Big, y: &Big| x.mod_mul(y, p);
    let delta = m(&a.z, &a.z);
    let gamma = m(&a.y, &a.y);
    let beta = m(&a.x, &gamma);
    let t = m(&a.x.mod_sub(&delta, p), &a.x.mod_add(&delta, p));
    let alpha = t.mod_add(&t, p).mod_add(&t, p);
    let beta4 = beta.shl(2).rem(p);
    let beta8 = beta.shl(3).rem(p);
    let x3 = m(&alpha, &alpha).mod_sub(&beta8, p);
    let yz = a.y.mod_add(&a.z, p);
    let z3 = m(&yz, &yz).mod_sub(&gamma, p).mod_sub(&delta, p);
    let g2 = m(&gamma, &gamma).shl(3).rem(p);
    let y3 = m(&alpha, &beta4.mod_sub(&x3, p)).mod_sub(&g2, p);
    Jac { x: x3, y: y3, z: z3 }
}

fn add(c: &Curve, a: &Jac, b: &Jac) -> Jac {
    if a.is_inf() {
        return b.clone();
    }
    if b.is_inf() {
        return a.clone();
    }
    let p = &c.p;
    let m = |x: &Big, y: &Big| x.mod_mul(y, p);
    let (z1z1, z2z2) = (m(&a.z, &a.z), m(&b.z, &b.z));
    let u1 = m(&a.x, &z2z2);
    let u2 = m(&b.x, &z1z1);
    let s1 = m(&m(&a.y, &b.z), &z2z2);
    let s2 = m(&m(&b.y, &a.z), &z1z1);
    if u1 == u2 {
        return if s1 == s2 { dbl(c, a) } else { Jac::inf() };
    }
    let h = u2.mod_sub(&u1, p);
    let r = s2.mod_sub(&s1, p);
    let h2 = m(&h, &h);
    let h3 = m(&h2, &h);
    let u1h2 = m(&u1, &h2);
    let x3 = m(&r, &r).mod_sub(&h3, p).mod_sub(&u1h2.shl(1).rem(p), p);
    let y3 = m(&r, &u1h2.mod_sub(&x3, p)).mod_sub(&m(&s1, &h3), p);
    let z3 = m(&m(&a.z, &b.z), &h);
    Jac { x: x3, y: y3, z: z3 }
}

fn mul(c: &Curve, k: &Big, pt: &Jac) -> Jac {
    let mut acc = Jac::inf();
    for i in (0..k.bit_len()).rev() {
        acc = dbl(c, &acc);
        if k.bit(i) {
            acc = add(c, &acc, pt);
        }
    }
    acc
}

fn to_affine(c: &Curve, a: &Jac) -> Option<(Big, Big)> {
    let zi = a.z.mod_inv(&c.p)?;
    let zi2 = zi.mod_mul(&zi, &c.p);
    Some((a.x.mod_mul(&zi2, &c.p), a.y.mod_mul(&zi2.mod_mul(&zi, &c.p), &c.p)))
}

/// Is this 133-byte uncompressed point a point of the curve (coordinates below p, y^2 = x^3 - 3x + b)?
pub fn decode_point(q: &[u8]) -> Option<(Big, Big)> {
    if q.len() != POINT_BYTES || q[0] != 4 {
        return None;
    }
    let c = curve();
    let (x, y) = (Big::from_be(&q[1..1 + FIELD_BYTES]), Big::from_be(&q[1 + FIELD_BYTES..]));
    if x.cmp_big(&c.p) != Ordering::Less || y.cmp_big(&c.p) != Ordering::Less {
        return None;
    }
    let lhs = y.mod_mul(&y, &c.p);
    let x3 = x.mod_mul(&x, &c.p).mod_mul(&x, &c.p);
    let rhs = x3.mod_sub(&x.shl(1).add(&x).rem(&c.p), &c.p).mod_add(&c.b, &c.p);
    (lhs == rhs).then_some((x, y))
}

pub fn valid_point(q: &[u8]) -> bool {
    decode_point(q).is_some()
}

fn hash_to_scalar(data: &[u8]) -> Big {
    // SHA-512 is 512 bits, below the 521-bit order, so the integer is the hash itself
    Big::from_be(&Sha512::digest(data))
}

/// Check an ECDSA signature (`r`, `s` as big-endian integers) over `data` for the public point `q`.
pub fn verify(q: &[u8], r: &[u8], s: &[u8], data: &[u8]) -> bool {
    let Some(pubkey) = decode_point(q) else { return false };
    let c = curve();
    let (r, s) = (Big::from_be(r), Big::from_be(s));
    if r.is_zero() || s.is_zero() || r.cmp_big(&c.n) != Ordering::Less || s.cmp_big(&c.n) != Ordering::Less {
        return false;
    }
    let Some(w) = s.mod_inv(&c.n) else { return false };
    let e = hash_to_scalar(data);
    let u1 = e.mod_mul(&w, &c.n);
    let u2 = r.mod_mul(&w, &c.n);
    let sum = add(&c, &mul(&c, &u1, &Jac::from_affine(&c.g)), &mul(&c, &u2, &Jac::from_affine(&pubkey)));
    match to_affine(&c, &sum) {
        Some((x, _)) => x.rem(&c.n) == r,
        None => false,
    }
}

#[cfg(test)]
pub mod signer {
    use super::*;
    use crate::bignum::{random_below, Fill};

    /// A key for the tests: the private scalar and the public point.
    pub struct TestKey {
        pub d: Big,
        pub q: Vec<u8>,
    }

    pub fn generate(rng: &mut dyn Fill) -> TestKey {
        let c = curve();
        let d = random_below(rng, &c.n);
        let (x, y) = to_affine(&c, &mul(&c, &d, &Jac::from_affine(&c.g))).unwrap();
        let mut q = vec![4];
        q.extend(x.to_be(FIELD_BYTES));
        q.extend(y.to_be(FIELD_BYTES));
        TestKey { d, q }
    }

    pub fn sign(k: &TestKey, data: &[u8], rng: &mut dyn Fill) -> (Vec<u8>, Vec<u8>) {
        let c = curve();
        let e = hash_to_scalar(data);
        loop {
            let nonce = random_below(rng, &c.n);
            let (x, _) = to_affine(&c, &mul(&c, &nonce, &Jac::from_affine(&c.g))).unwrap();
            let r = x.rem(&c.n);
            if r.is_zero() {
                continue;
            }
            let s = nonce.mod_inv(&c.n).unwrap().mod_mul(&e.mod_add(&r.mod_mul(&k.d, &c.n), &c.n), &c.n);
            if !s.is_zero() {
                return (r.to_be_min(), s.to_be_min());
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bignum::SystemFill;

    // A key and a signature made by OpenSSL 3.5 (`openssl dgst -sha512 -sign`) over the text below: an outside check of the curve.
    const Q: &str = "04012f813e7a9df9268df1a2c1cc5cb171a82d03738e752e25a555f0a2e0258c4b3a2970ac50a8ac5bb1523f639e09f009e37722e8efc8b8b676b01b10c42b47ab6bb70024c1a95089999b5e25983973799af3619fad4c5764bfdc99e20e46c5528e8ea1580d3d85f4f7e96c207e9e5386e9ff899ec1206f099b7e731ce57f3b01aa020d9c";
    const R: &str = "15F1BB4F074547EAAA9191A3CA28C30D89B3CB8B0FEF2AB8CEFFC7CD33B96E5C27830E088499F652B54A5AAAEBC7CC40D65ADE4022A6C23D4953693A102C494597";
    const S: &str = "F5885C3CB1A08D14715817AB854BE09B1CCB97E7359ABA1158FC17E84E5B10639FD8DF529BCAB9D0E5610CB2C91C925E297D8E36D152EA354B4568A8D73090EB93";
    const TEXT: &[u8] = b"iyakki p521 vector";

    fn hex(s: &str) -> Vec<u8> {
        Big::from_hex(s).to_be_min()
    }

    #[test]
    fn the_generator_is_on_the_curve_and_has_the_published_order() {
        let c = curve();
        let mut q = vec![4];
        q.extend(c.g.0.to_be(FIELD_BYTES));
        q.extend(c.g.1.to_be(FIELD_BYTES));
        assert!(valid_point(&q));
        assert!(to_affine(&c, &mul(&c, &c.n, &Jac::from_affine(&c.g))).is_none(), "n times G is the point at infinity");
        let one_less = c.n.sub(&Big::from_u64(1));
        let (x, y) = to_affine(&c, &mul(&c, &one_less, &Jac::from_affine(&c.g))).unwrap();
        assert_eq!(x, c.g.0);
        assert_eq!(y, c.p.sub(&c.g.1), "(n-1)G is -G");
    }

    #[test]
    fn an_openssl_signature_checks_and_nothing_else_does() {
        let (q, r, s) = (hex(Q), hex(R), hex(S));
        assert!(verify(&q, &r, &s, TEXT));
        assert!(!verify(&q, &r, &s, b"another text"));
        assert!(!verify(&q, &s, &r, TEXT), "r and s swapped");
        let mut bad_q = q.clone();
        bad_q[70] ^= 1;
        assert!(!verify(&bad_q, &r, &s, TEXT), "a point off the curve");
        assert!(!verify(&q[..132], &r, &s, TEXT));
        assert!(!verify(&q, &[], &s, TEXT) && !verify(&q, &r, &[], TEXT));
        let n = hex(N_HEX);
        assert!(!verify(&q, &n, &s, TEXT) && !verify(&q, &r, &n, TEXT), "values not below the order");
        let mut bad_r = r.clone();
        bad_r[10] ^= 4;
        assert!(!verify(&q, &bad_r, &s, TEXT));
    }

    #[test]
    fn our_own_signatures_check_and_are_not_transferable() {
        let mut rng = crate::bignum::tests_stream(9);
        let k = signer::generate(&mut rng);
        let other = signer::generate(&mut rng);
        for i in 0..3 {
            let data = format!("message {i}");
            let (r, s) = signer::sign(&k, data.as_bytes(), &mut rng);
            assert!(verify(&k.q, &r, &s, data.as_bytes()));
            assert!(!verify(&other.q, &r, &s, data.as_bytes()));
            assert!(!verify(&k.q, &r, &s, b"x"));
        }
        let _ = SystemFill;
    }
}
