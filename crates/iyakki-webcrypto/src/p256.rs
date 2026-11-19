//! NIST P-256 key agreement for a private key given as bytes (`ring` only makes one-use keys, and a push receiver or a test vector needs
//! a fixed one). Montgomery arithmetic on four 64-bit limbs, Jacobian points. NOT constant time: it is used for the receiving side of Web
//! Push in loopback tests and for checks, where the key is not an attacker's target. A sender (the app server) uses ring's ephemeral key.

type U = [u64; 4];

struct Field {
    m: U,
    n0: u64,
    r2: U,
}

fn geq(a: &U, b: &U) -> bool {
    for i in (0..4).rev() {
        if a[i] != b[i] {
            return a[i] > b[i];
        }
    }
    true
}
fn sub(a: &U, b: &U) -> (U, bool) {
    let mut o = [0u64; 4];
    let mut br = false;
    for i in 0..4 {
        let (x, b1) = a[i].overflowing_sub(b[i]);
        let (y, b2) = x.overflowing_sub(br as u64);
        o[i] = y;
        br = b1 || b2;
    }
    (o, br)
}
fn add(a: &U, b: &U) -> (U, bool) {
    let mut o = [0u64; 4];
    let mut c = false;
    for i in 0..4 {
        let (x, c1) = a[i].overflowing_add(b[i]);
        let (y, c2) = x.overflowing_add(c as u64);
        o[i] = y;
        c = c1 || c2;
    }
    (o, c)
}

impl Field {
    fn new(m: U) -> Field {
        let mut inv = 1u64;
        for _ in 0..6 {
            inv = inv.wrapping_mul(2u64.wrapping_sub(m[0].wrapping_mul(inv)));
        }
        let mut f = Field { m, n0: inv.wrapping_neg(), r2: [0; 4] };
        let mut x: U = [1, 0, 0, 0];
        for _ in 0..512 {
            x = f.add(&x, &x);
        }
        f.r2 = x;
        f
    }
    fn add(&self, a: &U, b: &U) -> U {
        let (s, c) = add(a, b);
        if c || geq(&s, &self.m) {
            sub(&s, &self.m).0
        } else {
            s
        }
    }
    fn sub(&self, a: &U, b: &U) -> U {
        let (d, br) = sub(a, b);
        if br {
            add(&d, &self.m).0
        } else {
            d
        }
    }
    fn mul(&self, a: &U, b: &U) -> U {
        let mut t = [0u64; 6];
        for i in 0..4 {
            let mut c = 0u128;
            for j in 0..4 {
                let s = t[j] as u128 + a[j] as u128 * b[i] as u128 + c;
                t[j] = s as u64;
                c = s >> 64;
            }
            let s = t[4] as u128 + c;
            t[4] = s as u64;
            t[5] = (s >> 64) as u64;
            let mm = t[0].wrapping_mul(self.n0);
            let s = t[0] as u128 + mm as u128 * self.m[0] as u128;
            let mut c = s >> 64;
            for j in 1..4 {
                let s = t[j] as u128 + mm as u128 * self.m[j] as u128 + c;
                t[j - 1] = s as u64;
                c = s >> 64;
            }
            let s = t[4] as u128 + c;
            t[3] = s as u64;
            t[4] = t[5] + (s >> 64) as u64;
            t[5] = 0;
        }
        let r = [t[0], t[1], t[2], t[3]];
        if t[4] != 0 || geq(&r, &self.m) {
            sub(&r, &self.m).0
        } else {
            r
        }
    }
    fn to_m(&self, a: &U) -> U {
        self.mul(a, &self.r2)
    }
    fn from_m(&self, a: &U) -> U {
        self.mul(a, &[1, 0, 0, 0])
    }
    fn pow(&self, base: &U, e: &U) -> U {
        let mut r = self.to_m(&[1, 0, 0, 0]);
        for i in (0..256).rev() {
            r = self.mul(&r, &r);
            if e[i / 64] >> (i % 64) & 1 == 1 {
                r = self.mul(&r, base);
            }
        }
        r
    }
}

const P: U = [0xFFFF_FFFF_FFFF_FFFF, 0x0000_0000_FFFF_FFFF, 0, 0xFFFF_FFFF_0000_0001];
const N: U = [0xF3B9_CAC2_FC63_2551, 0xBCE6_FAAD_A717_9E84, 0xFFFF_FFFF_FFFF_FFFF, 0xFFFF_FFFF_0000_0000];
const B: U = [0x3BCE_3C3E_27D2_604B, 0x651D_06B0_CC53_B0F6, 0xB3EB_BD55_7698_86BC, 0x5AC6_35D8_AA3A_93E7];
const GX: U = [0xF4A1_3945_D898_C296, 0x7703_7D81_2DEB_33A0, 0xF8BC_E6E5_63A4_40F2, 0x6B17_D1F2_E12C_4247];
const GY: U = [0xCBB6_4068_37BF_51F5, 0x2BCE_3357_6B31_5ECE, 0x8EE7_EB4A_7C0F_9E16, 0x4FE3_42E2_FE1A_7F9B];

fn from_be(b: &[u8]) -> U {
    let mut o = [0u64; 4];
    for (i, w) in o.iter_mut().enumerate() {
        let mut x = [0u8; 8];
        x.copy_from_slice(&b[32 - 8 * (i + 1)..32 - 8 * i]);
        *w = u64::from_be_bytes(x);
    }
    o
}
fn to_be(a: &U) -> [u8; 32] {
    let mut o = [0u8; 32];
    for i in 0..4 {
        o[32 - 8 * (i + 1)..32 - 8 * i].copy_from_slice(&a[i].to_be_bytes());
    }
    o
}

/// Jacobian point in Montgomery form; z = 0 is infinity.
#[derive(Clone, Copy)]
struct Pt {
    x: U,
    y: U,
    z: U,
}

struct Curve {
    f: Field,
    b: U,
    one: U,
}

impl Curve {
    fn new() -> Curve {
        let f = Field::new(P);
        let b = f.to_m(&B);
        let one = f.to_m(&[1, 0, 0, 0]);
        Curve { f, b, one }
    }
    fn inf(&self) -> Pt {
        Pt { x: self.one, y: self.one, z: [0; 4] }
    }
    fn is_inf(p: &Pt) -> bool {
        p.z == [0; 4]
    }
    fn dbl(&self, p: &Pt) -> Pt {
        if Self::is_inf(p) || p.y == [0; 4] {
            return self.inf();
        }
        let f = &self.f;
        let delta = f.mul(&p.z, &p.z);
        let gamma = f.mul(&p.y, &p.y);
        let beta = f.mul(&p.x, &gamma);
        let t = f.mul(&f.sub(&p.x, &delta), &f.add(&p.x, &delta));
        let alpha = f.add(&f.add(&t, &t), &t);
        let b2 = f.add(&beta, &beta);
        let b4 = f.add(&b2, &b2);
        let b8 = f.add(&b4, &b4);
        let x3 = f.sub(&f.mul(&alpha, &alpha), &b8);
        let yz = f.add(&p.y, &p.z);
        let z3 = f.sub(&f.sub(&f.mul(&yz, &yz), &gamma), &delta);
        let g2 = f.mul(&gamma, &gamma);
        let g2x2 = f.add(&g2, &g2);
        let g2x4 = f.add(&g2x2, &g2x2);
        let g2x8 = f.add(&g2x4, &g2x4);
        let y3 = f.sub(&f.mul(&alpha, &f.sub(&b4, &x3)), &g2x8);
        Pt { x: x3, y: y3, z: z3 }
    }
    fn addp(&self, p: &Pt, q: &Pt) -> Pt {
        if Self::is_inf(p) {
            return *q;
        }
        if Self::is_inf(q) {
            return *p;
        }
        let f = &self.f;
        let z1z1 = f.mul(&p.z, &p.z);
        let z2z2 = f.mul(&q.z, &q.z);
        let u1 = f.mul(&p.x, &z2z2);
        let u2 = f.mul(&q.x, &z1z1);
        let s1 = f.mul(&f.mul(&p.y, &q.z), &z2z2);
        let s2 = f.mul(&f.mul(&q.y, &p.z), &z1z1);
        let h = f.sub(&u2, &u1);
        let r = f.sub(&s2, &s1);
        if h == [0; 4] {
            return if r == [0; 4] { self.dbl(p) } else { self.inf() };
        }
        let hh = f.mul(&h, &h);
        let hhh = f.mul(&h, &hh);
        let v = f.mul(&u1, &hh);
        let x3 = f.sub(&f.sub(&f.mul(&r, &r), &hhh), &f.add(&v, &v));
        let y3 = f.sub(&f.mul(&r, &f.sub(&v, &x3)), &f.mul(&s1, &hhh));
        let z3 = f.mul(&f.mul(&p.z, &q.z), &h);
        Pt { x: x3, y: y3, z: z3 }
    }
    fn mulp(&self, p: &Pt, k: &U) -> Pt {
        let mut r = self.inf();
        for i in (0..256).rev() {
            r = self.dbl(&r);
            if k[i / 64] >> (i % 64) & 1 == 1 {
                r = self.addp(&r, p);
            }
        }
        r
    }
    fn affine(&self, p: &Pt) -> Option<(U, U)> {
        if Self::is_inf(p) {
            return None;
        }
        let f = &self.f;
        let zi = f.pow(&p.z, &sub(&P, &[2, 0, 0, 0]).0);
        let zi2 = f.mul(&zi, &zi);
        Some((f.from_m(&f.mul(&p.x, &zi2)), f.from_m(&f.mul(&p.y, &f.mul(&zi2, &zi)))))
    }
    fn from_affine(&self, x: &U, y: &U) -> Pt {
        Pt { x: self.f.to_m(x), y: self.f.to_m(y), z: self.one }
    }
    fn on_curve(&self, x: &U, y: &U) -> bool {
        if geq(x, &P) || geq(y, &P) {
            return false;
        }
        let f = &self.f;
        let (xm, ym) = (f.to_m(x), f.to_m(y));
        let x3 = f.mul(&f.mul(&xm, &xm), &xm);
        let three_x = f.add(&f.add(&xm, &xm), &xm);
        f.mul(&ym, &ym) == f.add(&f.sub(&x3, &three_x), &self.b)
    }
}

fn private_ok(d: &[u8; 32]) -> Result<U, String> {
    let k = from_be(d);
    if k == [0; 4] || geq(&k, &N) {
        return Err("not a P-256 private key".into());
    }
    Ok(k)
}

/// The uncompressed public point (65 bytes, 0x04 first) of a private scalar.
pub fn public_from_private(d: &[u8; 32]) -> Result<[u8; 65], String> {
    let k = private_ok(d)?;
    let c = Curve::new();
    let (x, y) = c.affine(&c.mulp(&c.from_affine(&GX, &GY), &k)).ok_or("point at infinity")?;
    let mut o = [0u8; 65];
    o[0] = 4;
    o[1..33].copy_from_slice(&to_be(&x));
    o[33..].copy_from_slice(&to_be(&y));
    Ok(o)
}

/// ECDH: the x coordinate of d times the peer's uncompressed point. The point is checked to be on the curve.
pub fn ecdh(d: &[u8; 32], peer: &[u8]) -> Result<[u8; 32], String> {
    let k = private_ok(d)?;
    if peer.len() != 65 || peer[0] != 4 {
        return Err("the public key must be an uncompressed P-256 point".into());
    }
    let c = Curve::new();
    let (x, y) = (from_be(&peer[1..33]), from_be(&peer[33..]));
    if !c.on_curve(&x, &y) {
        return Err("the public key is not on the P-256 curve".into());
    }
    let (sx, _) = c.affine(&c.mulp(&c.from_affine(&x, &y), &k)).ok_or("point at infinity")?;
    Ok(to_be(&sx))
}

// ------------------------------------------------------------------------------------------------ group operations (F-HOTSPOT-22)
//
// The pieces a protocol that works on points needs (the WPA3 SAE exchange of the hotspot): add, multiply, negate, lift an x to a point,
// and addition and multiplication of scalars modulo the group order. Points are 64 bytes (x then y, big endian, no prefix). NOT constant
// time, like the rest of this file: SAE here runs on an access point whose password is the secret, so the caller compensates by running
// its search a fixed number of rounds.

fn pt_from_bytes(c: &Curve, p: &[u8; 64]) -> Option<Pt> {
    let (x, y) = (from_be(&p[..32]), from_be(&p[32..]));
    if c.on_curve(&x, &y) {
        Some(c.from_affine(&x, &y))
    } else {
        None
    }
}

fn pt_to_bytes(c: &Curve, p: &Pt) -> Option<[u8; 64]> {
    let (x, y) = c.affine(p)?;
    let mut o = [0u8; 64];
    o[..32].copy_from_slice(&to_be(&x));
    o[32..].copy_from_slice(&to_be(&y));
    Some(o)
}

/// The base point as 64 bytes.
pub fn generator() -> [u8; 64] {
    let mut o = [0u8; 64];
    o[..32].copy_from_slice(&to_be(&GX));
    o[32..].copy_from_slice(&to_be(&GY));
    o
}

/// The group order as 32 bytes, big endian.
pub fn order() -> [u8; 32] {
    to_be(&N)
}

/// The field prime as 32 bytes, big endian.
pub fn prime() -> [u8; 32] {
    to_be(&P)
}

/// Is this 64-byte point on the curve (coordinates below the prime, the equation holds)?
pub fn point_on_curve(p: &[u8; 64]) -> bool {
    pt_from_bytes(&Curve::new(), p).is_some()
}

/// `k * p`. `None` when the point is not on the curve, `k` is zero or not below the order, or the result is infinity.
pub fn point_mul(k: &[u8; 32], p: &[u8; 64]) -> Option<[u8; 64]> {
    let k = private_ok(k).ok()?;
    let c = Curve::new();
    let pt = pt_from_bytes(&c, p)?;
    pt_to_bytes(&c, &c.mulp(&pt, &k))
}

/// `a + b`. `None` when either is off the curve or the sum is infinity.
pub fn point_add(a: &[u8; 64], b: &[u8; 64]) -> Option<[u8; 64]> {
    let c = Curve::new();
    let (pa, pb) = (pt_from_bytes(&c, a)?, pt_from_bytes(&c, b)?);
    pt_to_bytes(&c, &c.addp(&pa, &pb))
}

/// `-p` (the y coordinate negated). `None` when the point is off the curve.
pub fn point_neg(p: &[u8; 64]) -> Option<[u8; 64]> {
    let c = Curve::new();
    let (x, y) = (from_be(&p[..32]), from_be(&p[32..]));
    if !c.on_curve(&x, &y) {
        return None;
    }
    let ny = if y == [0; 4] { y } else { sub(&P, &y).0 };
    let mut o = [0u8; 64];
    o[..32].copy_from_slice(&to_be(&x));
    o[32..].copy_from_slice(&to_be(&ny));
    Some(o)
}

/// The point with this x coordinate whose y has the wanted parity (`y_odd`), or `None` when x is not below the prime or no point has it.
pub fn lift_x(x: &[u8; 32], y_odd: bool) -> Option<[u8; 64]> {
    let xv = from_be(x);
    if geq(&xv, &P) {
        return None;
    }
    let c = Curve::new();
    let f = &c.f;
    let xm = f.to_m(&xv);
    let x3 = f.mul(&f.mul(&xm, &xm), &xm);
    let three_x = f.add(&f.add(&xm, &xm), &xm);
    let rhs = f.add(&f.sub(&x3, &three_x), &c.b);
    // the prime is 3 mod 4: a square root is rhs^((p+1)/4), checked by squaring
    let (p1, _) = add(&P, &[1, 0, 0, 0]);
    let e = [(p1[0] >> 2) | (p1[1] << 62), (p1[1] >> 2) | (p1[2] << 62), (p1[2] >> 2) | (p1[3] << 62), p1[3] >> 2];
    let root = f.pow(&rhs, &e);
    if f.mul(&root, &root) != rhs {
        return None;
    }
    let mut y = f.from_m(&root);
    if (y[0] & 1 == 1) != y_odd {
        y = sub(&P, &y).0;
    }
    let mut o = [0u8; 64];
    o[..32].copy_from_slice(x);
    o[32..].copy_from_slice(&to_be(&y));
    Some(o)
}

/// `(a + b) mod order`; both inputs must be below the order.
pub fn scalar_add(a: &[u8; 32], b: &[u8; 32]) -> Option<[u8; 32]> {
    let (a, b) = (from_be(a), from_be(b));
    if geq(&a, &N) || geq(&b, &N) {
        return None;
    }
    Some(to_be(&Field::new(N).add(&a, &b)))
}

/// `(a * b) mod order`; both inputs must be below the order.
pub fn scalar_mul(a: &[u8; 32], b: &[u8; 32]) -> Option<[u8; 32]> {
    let (a, b) = (from_be(a), from_be(b));
    if geq(&a, &N) || geq(&b, &N) {
        return None;
    }
    let f = Field::new(N);
    Some(to_be(&f.from_m(&f.mul(&f.to_m(&a), &f.to_m(&b)))))
}

/// `(-a) mod order`; the input must be below the order.
pub fn scalar_neg(a: &[u8; 32]) -> Option<[u8; 32]> {
    let a = from_be(a);
    if geq(&a, &N) {
        return None;
    }
    Some(if a == [0; 4] { to_be(&a) } else { to_be(&sub(&N, &a).0) })
}

/// Is this a usable private scalar (1 ..= order - 1)?
pub fn scalar_in_range(a: &[u8; 32]) -> bool {
    private_ok(a).is_ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    fn h(s: &str) -> Vec<u8> {
        (0..s.len() / 2).map(|i| u8::from_str_radix(&s[2 * i..2 * i + 2], 16).unwrap()).collect()
    }
    #[test]
    fn the_generator_is_on_the_curve_and_n_times_g_is_infinity() {
        let c = Curve::new();
        assert!(c.on_curve(&GX, &GY));
        assert!(Curve::is_inf(&c.mulp(&c.from_affine(&GX, &GY), &N)));
        let (x, y) = c.affine(&c.mulp(&c.from_affine(&GX, &GY), &[2, 0, 0, 0])).unwrap();
        assert_eq!(to_be(&x).to_vec(), h("7cf27b188d034f7e8a52380304b51ac3c08969e277f21b35a60b48fc47669978"));
        assert_eq!(to_be(&y).to_vec(), h("07775510db8ed040293d9ac69f7430dbba7dade63ce982299e04b79d227873d1"));
    }
    #[test]
    fn rfc5903_section_8_1_ecdh_vector() {
        let i = <[u8; 32]>::try_from(h("c88f01f510d9ac3f70a292daa2316de544e9aab8afe84049c62a9c57862d1433")).unwrap();
        let r = <[u8; 32]>::try_from(h("c6ef9c5d78ae012a011164acb397ce2088685d8f06bf9be0b283ab46476bee53")).unwrap();
        let gi = public_from_private(&i).unwrap();
        let gr = public_from_private(&r).unwrap();
        assert_eq!(gi[1..33], h("dad0b65394221cf9b051e1feca5787d098dfe637fc90b9ef945d0c3772581180")[..]);
        assert_eq!(gr[1..33], h("d12dfb5289c8d4f81208b70270398c342296970a0bccb74c736fc7554494bf63")[..]);
        let z = h("d6840f6b42f6edafd13116e0e12565202fef8e9ece7dce03812464d04b9442de");
        assert_eq!(ecdh(&i, &gr).unwrap().to_vec(), z);
        assert_eq!(ecdh(&r, &gi).unwrap().to_vec(), z);
    }
    #[test]
    fn a_point_off_the_curve_and_bad_scalars_are_refused() {
        let d = [7u8; 32];
        let mut p = public_from_private(&d).unwrap();
        p[64] ^= 1;
        assert!(ecdh(&d, &p).is_err());
        assert!(ecdh(&[0u8; 32], &p).is_err() && ecdh(&[0xff; 32], &p).is_err());
        assert!(ecdh(&d, &p[..64]).is_err());
    }

    #[test]
    fn group_operations_agree_with_each_other() {
        let g = generator();
        assert!(point_on_curve(&g));
        let mut two = [0u8; 32];
        two[31] = 2;
        let mut three = [0u8; 32];
        three[31] = 3;
        let g2 = point_mul(&two, &g).unwrap();
        assert_eq!(point_add(&g, &g).unwrap(), g2);
        let g3 = point_mul(&three, &g).unwrap();
        assert_eq!(point_add(&g2, &g).unwrap(), g3);
        let six = scalar_mul(&two, &three).unwrap();
        assert_eq!(point_mul(&six, &g).unwrap(), point_mul(&three, &g2).unwrap());
        assert!(point_add(&g, &point_neg(&g).unwrap()).is_none());
        let mut one = [0u8; 32];
        one[31] = 1;
        let n1 = scalar_neg(&one).unwrap();
        assert_eq!(point_mul(&n1, &g).unwrap(), point_neg(&g).unwrap());
        assert_eq!(&g2[..32], &h("7cf27b188d034f7e8a52380304b51ac3c08969e277f21b35a60b48fc47669978")[..]);
        assert_eq!(scalar_add(&three, &scalar_neg(&three).unwrap()).unwrap(), [0u8; 32]);
        assert!(scalar_add(&order(), &three).is_none());
        assert!(!scalar_in_range(&[0u8; 32]) && !scalar_in_range(&order()) && scalar_in_range(&three));
        assert!(point_mul(&[0u8; 32], &g).is_none());
        let mut off = g;
        off[63] ^= 1;
        assert!(!point_on_curve(&off) && point_mul(&three, &off).is_none() && point_neg(&off).is_none());
    }

    #[test]
    fn lift_x_finds_the_generator_with_either_parity_and_refuses_non_residues() {
        let g = generator();
        let mut x = [0u8; 32];
        x.copy_from_slice(&g[..32]);
        let odd = g[63] & 1 == 1;
        assert_eq!(lift_x(&x, odd).unwrap(), g);
        assert_eq!(lift_x(&x, !odd).unwrap(), point_neg(&g).unwrap());
        assert!(lift_x(&prime(), false).is_none());
        let mut missing = 0;
        for v in 1u8..40 {
            let mut xs = [0u8; 32];
            xs[31] = v;
            match lift_x(&xs, false) {
                Some(p) => assert!(point_on_curve(&p)),
                None => missing += 1,
            }
        }
        assert!(missing > 0 && missing < 39);
    }
}
