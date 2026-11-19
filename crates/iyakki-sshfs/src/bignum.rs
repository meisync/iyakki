//! A small unsigned big-integer type written for the SSH key code (F-FILE-14): RSA key generation (Miller-Rabin, extended Euclid) and
//! ECDSA P-521 (`p521.rs`), which `ring` cannot do. Not constant time: it is used to CHECK public data (signatures) and to make a
//! host key once, where the person's machine is the only party that sees the timing. Limbs are 32 bits, least significant first, and
//! a value never carries a zero top limb.

use std::cmp::Ordering;

#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct Big(Vec<u32>);

impl Big {
    pub fn zero() -> Big {
        Big(Vec::new())
    }

    pub fn from_u64(v: u64) -> Big {
        let mut b = Big(vec![v as u32, (v >> 32) as u32]);
        b.trim();
        b
    }

    pub fn from_be(bytes: &[u8]) -> Big {
        let mut limbs = Vec::with_capacity(bytes.len() / 4 + 1);
        let mut i = bytes.len();
        while i > 0 {
            let lo = i.saturating_sub(4);
            let mut w = 0u32;
            for &b in &bytes[lo..i] {
                w = (w << 8) | b as u32;
            }
            limbs.push(w);
            i = lo;
        }
        let mut b = Big(limbs);
        b.trim();
        b
    }

    pub fn from_hex(s: &str) -> Big {
        let s = if s.len() % 2 == 1 { format!("0{s}") } else { s.to_string() };
        let bytes: Vec<u8> = (0..s.len() / 2).map(|i| u8::from_str_radix(&s[2 * i..2 * i + 2], 16).unwrap_or(0)).collect();
        Big::from_be(&bytes)
    }

    /// Big-endian bytes, left-padded with zeros to `len` (cut from the left when the value is longer).
    pub fn to_be(&self, len: usize) -> Vec<u8> {
        let mut out = vec![0u8; len];
        for (i, limb) in self.0.iter().enumerate() {
            for j in 0..4 {
                let pos = i * 4 + j;
                if pos < len {
                    out[len - 1 - pos] = (limb >> (8 * j)) as u8;
                }
            }
        }
        out
    }

    /// The shortest big-endian form (empty for zero).
    pub fn to_be_min(&self) -> Vec<u8> {
        self.to_be(self.bit_len().div_ceil(8))
    }

    fn trim(&mut self) {
        while self.0.last() == Some(&0) {
            self.0.pop();
        }
    }

    pub fn is_zero(&self) -> bool {
        self.0.is_empty()
    }

    pub fn is_odd(&self) -> bool {
        self.0.first().is_some_and(|l| l & 1 == 1)
    }

    pub fn is_one(&self) -> bool {
        self.0 == [1]
    }

    pub fn bit_len(&self) -> usize {
        match self.0.last() {
            None => 0,
            Some(top) => self.0.len() * 32 - top.leading_zeros() as usize,
        }
    }

    pub fn bit(&self, i: usize) -> bool {
        self.0.get(i / 32).is_some_and(|l| (l >> (i % 32)) & 1 == 1)
    }

    pub fn low_u32(&self) -> u32 {
        self.0.first().copied().unwrap_or(0)
    }

    pub fn cmp_big(&self, o: &Big) -> Ordering {
        if self.0.len() != o.0.len() {
            return self.0.len().cmp(&o.0.len());
        }
        for i in (0..self.0.len()).rev() {
            if self.0[i] != o.0[i] {
                return self.0[i].cmp(&o.0[i]);
            }
        }
        Ordering::Equal
    }

    pub fn add(&self, o: &Big) -> Big {
        let n = self.0.len().max(o.0.len());
        let mut out = Vec::with_capacity(n + 1);
        let mut carry = 0u64;
        for i in 0..n {
            let s = *self.0.get(i).unwrap_or(&0) as u64 + *o.0.get(i).unwrap_or(&0) as u64 + carry;
            out.push(s as u32);
            carry = s >> 32;
        }
        if carry > 0 {
            out.push(carry as u32);
        }
        Big(out)
    }

    /// `self - o`; the caller guarantees `self >= o` (otherwise the result is zero).
    pub fn sub(&self, o: &Big) -> Big {
        if self.cmp_big(o) != Ordering::Greater {
            return Big::zero();
        }
        let mut out = Vec::with_capacity(self.0.len());
        let mut borrow = 0i64;
        for i in 0..self.0.len() {
            let mut d = self.0[i] as i64 - *o.0.get(i).unwrap_or(&0) as i64 - borrow;
            if d < 0 {
                d += 1 << 32;
                borrow = 1;
            } else {
                borrow = 0;
            }
            out.push(d as u32);
        }
        let mut b = Big(out);
        b.trim();
        b
    }

    pub fn mul(&self, o: &Big) -> Big {
        if self.is_zero() || o.is_zero() {
            return Big::zero();
        }
        let mut out = vec![0u32; self.0.len() + o.0.len()];
        for (i, &a) in self.0.iter().enumerate() {
            let mut carry = 0u64;
            for (j, &b) in o.0.iter().enumerate() {
                let t = a as u64 * b as u64 + out[i + j] as u64 + carry;
                out[i + j] = t as u32;
                carry = t >> 32;
            }
            out[i + o.0.len()] = carry as u32;
        }
        let mut b = Big(out);
        b.trim();
        b
    }

    pub fn shl(&self, bits: usize) -> Big {
        if self.is_zero() {
            return Big::zero();
        }
        let (words, rem) = (bits / 32, bits % 32);
        let mut out = vec![0u32; words];
        let mut carry = 0u32;
        for &l in &self.0 {
            if rem == 0 {
                out.push(l);
            } else {
                out.push((l << rem) | carry);
                carry = l >> (32 - rem);
            }
        }
        if carry > 0 {
            out.push(carry);
        }
        Big(out)
    }

    pub fn shr(&self, bits: usize) -> Big {
        let (words, rem) = (bits / 32, bits % 32);
        if words >= self.0.len() {
            return Big::zero();
        }
        let src = &self.0[words..];
        let mut out = Vec::with_capacity(src.len());
        for i in 0..src.len() {
            let lo = src[i] >> rem;
            let hi = if rem > 0 && i + 1 < src.len() { src[i + 1] << (32 - rem) } else { 0 };
            out.push(lo | hi);
        }
        let mut b = Big(out);
        b.trim();
        b
    }

    /// Quotient and remainder (Knuth, algorithm D). Division by zero gives (0, self) and never panics.
    pub fn divrem(&self, d: &Big) -> (Big, Big) {
        if d.is_zero() || self.cmp_big(d) == Ordering::Less {
            return (Big::zero(), self.clone());
        }
        if d.0.len() == 1 {
            let dv = d.0[0] as u64;
            let mut q = vec![0u32; self.0.len()];
            let mut rem = 0u64;
            for i in (0..self.0.len()).rev() {
                let cur = (rem << 32) | self.0[i] as u64;
                q[i] = (cur / dv) as u32;
                rem = cur % dv;
            }
            let mut q = Big(q);
            q.trim();
            return (q, Big::from_u64(rem));
        }
        let s = d.0.last().map_or(0, |t| t.leading_zeros() as usize);
        let v = d.shl(s).0;
        let mut u = self.shl(s).0;
        let n = v.len();
        if u.len() == self.0.len() {
            u.push(0);
        }
        while u.len() < self.0.len() + 1 {
            u.push(0);
        }
        let m = u.len() - n - 1;
        let mut q = vec![0u32; m + 1];
        let base = 1u64 << 32;
        for j in (0..=m).rev() {
            let top = ((u[j + n] as u64) << 32) | u[j + n - 1] as u64;
            let mut qhat = top / v[n - 1] as u64;
            let mut rhat = top % v[n - 1] as u64;
            while qhat >= base || qhat * v[n - 2] as u64 > ((rhat << 32) | u[j + n - 2] as u64) {
                qhat -= 1;
                rhat += v[n - 1] as u64;
                if rhat >= base {
                    break;
                }
            }
            // multiply and subtract
            let mut borrow = 0i64;
            let mut carry = 0u64;
            for i in 0..n {
                let p = qhat * v[i] as u64 + carry;
                carry = p >> 32;
                let t = u[i + j] as i64 - borrow - (p & 0xffff_ffff) as i64;
                u[i + j] = t as u32;
                borrow = if t < 0 { 1 } else { 0 };
            }
            let t = u[j + n] as i64 - borrow - carry as i64;
            u[j + n] = t as u32;
            if t < 0 {
                // qhat was one too large: add the divisor back
                qhat -= 1;
                let mut c = 0u64;
                for i in 0..n {
                    let s2 = u[i + j] as u64 + v[i] as u64 + c;
                    u[i + j] = s2 as u32;
                    c = s2 >> 32;
                }
                u[j + n] = (u[j + n] as u64 + c) as u32;
            }
            q[j] = qhat as u32;
        }
        let mut q = Big(q);
        q.trim();
        u.truncate(n);
        let r = Big(u);
        let mut r = r;
        r.trim();
        (q, r.shr(s))
    }

    pub fn rem(&self, m: &Big) -> Big {
        self.divrem(m).1
    }

    pub fn mod_add(&self, o: &Big, m: &Big) -> Big {
        self.add(o).rem(m)
    }

    pub fn mod_sub(&self, o: &Big, m: &Big) -> Big {
        let (a, b) = (self.rem(m), o.rem(m));
        if a.cmp_big(&b) == Ordering::Less {
            a.add(m).sub(&b)
        } else {
            a.sub(&b)
        }
    }

    pub fn mod_mul(&self, o: &Big, m: &Big) -> Big {
        self.mul(o).rem(m)
    }

    pub fn mod_pow(&self, exp: &Big, m: &Big) -> Big {
        if m.is_one() {
            return Big::zero();
        }
        let mut result = Big::from_u64(1);
        let base = self.rem(m);
        for i in (0..exp.bit_len()).rev() {
            result = result.mod_mul(&result, m);
            if exp.bit(i) {
                result = result.mod_mul(&base, m);
            }
        }
        result
    }

    /// The inverse of `self` modulo `m`, or None when they share a factor.
    pub fn mod_inv(&self, m: &Big) -> Option<Big> {
        if m.is_zero() || m.is_one() {
            return None;
        }
        let (mut r0, mut r1) = (m.clone(), self.rem(m));
        let (mut t0, mut t1) = (Big::zero(), Big::from_u64(1));
        while !r1.is_zero() {
            let (q, r2) = r0.divrem(&r1);
            let t2 = t0.mod_sub(&q.mod_mul(&t1, m), m);
            r0 = r1;
            r1 = r2;
            t0 = t1;
            t1 = t2;
        }
        if r0.is_one() {
            Some(t0)
        } else {
            None
        }
    }

    pub fn gcd(&self, o: &Big) -> Big {
        let (mut a, mut b) = (self.clone(), o.clone());
        while !b.is_zero() {
            let r = a.rem(&b);
            a = b;
            b = r;
        }
        a
    }
}

/// Small primes for the quick trial division before Miller-Rabin.
const SMALL_PRIMES: [u32; 54] = [
    3, 5, 7, 11, 13, 17, 19, 23, 29, 31, 37, 41, 43, 47, 53, 59, 61, 67, 71, 73, 79, 83, 89, 97, 101, 103, 107, 109, 113, 127, 131, 137, 139, 149, 151, 157, 163, 167, 173, 179, 181, 191, 193, 197, 199, 211, 223, 227, 229, 233, 239, 241, 251, 257,
];

/// Where random bytes come from (the system generator in the server, a fixed stream in tests).
pub trait Fill {
    fn fill(&mut self, out: &mut [u8]);
}

pub struct SystemFill;

impl Fill for SystemFill {
    fn fill(&mut self, out: &mut [u8]) {
        use ring::rand::SecureRandom;
        // a failing system generator would make a predictable key: stop instead
        ring::rand::SystemRandom::new().fill(out).expect("the system random generator failed");
    }
}

/// A random value of exactly `bits` bits (top bit set).
pub fn random_bits(rng: &mut dyn Fill, bits: usize) -> Big {
    let bytes = bits.div_ceil(8);
    let mut b = vec![0u8; bytes];
    rng.fill(&mut b);
    let extra = bytes * 8 - bits;
    b[0] &= 0xffu8 >> extra;
    b[0] |= 0x80u8 >> extra;
    Big::from_be(&b)
}

/// A random value in 1..n (rejection sampling).
pub fn random_below(rng: &mut dyn Fill, n: &Big) -> Big {
    let bytes = n.bit_len().div_ceil(8).max(1);
    loop {
        let mut b = vec![0u8; bytes];
        rng.fill(&mut b);
        let v = Big::from_be(&b);
        if !v.is_zero() && v.cmp_big(n) == Ordering::Less {
            return v;
        }
    }
}

/// Miller-Rabin with `rounds` random bases after trial division by small primes.
pub fn is_probable_prime(n: &Big, rounds: usize, rng: &mut dyn Fill) -> bool {
    if n.bit_len() < 2 {
        return false;
    }
    if !n.is_odd() {
        return n == &Big::from_u64(2);
    }
    for &p in &SMALL_PRIMES {
        let pb = Big::from_u64(p as u64);
        if n == &pb {
            return true;
        }
        if n.rem(&pb).is_zero() {
            return false;
        }
    }
    let one = Big::from_u64(1);
    let n1 = n.sub(&one);
    let mut s = 0;
    while !n1.bit(s) {
        s += 1;
    }
    let d = n1.shr(s);
    'witness: for _ in 0..rounds {
        let a = random_below(rng, &n1);
        if a.is_one() {
            continue;
        }
        let mut x = a.mod_pow(&d, n);
        if x.is_one() || x == n1 {
            continue;
        }
        for _ in 1..s {
            x = x.mod_mul(&x, n);
            if x == n1 {
                continue 'witness;
            }
        }
        return false;
    }
    true
}

/// A random prime of exactly `bits` bits whose value minus one shares no factor with `e`.
pub fn random_prime(rng: &mut dyn Fill, bits: usize, e: &Big) -> Big {
    loop {
        let mut c = random_bits(rng, bits);
        // top two bits set so the product of two primes has the full length
        if !c.bit(bits - 2) {
            c = c.add(&Big::from_u64(1).shl(bits - 2));
        }
        if !c.is_odd() {
            c = c.add(&Big::from_u64(1));
        }
        if !c.sub(&Big::from_u64(1)).gcd(e).is_one() {
            continue;
        }
        if is_probable_prime(&c, 24, rng) {
            return c;
        }
    }
}

#[cfg(test)]
pub fn tests_stream(seed: u64) -> tests::Stream {
    tests::Stream(seed)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A deterministic byte stream (SplitMix64), so the tests repeat.
    pub struct Stream(pub u64);
    impl Fill for Stream {
        fn fill(&mut self, out: &mut [u8]) {
            for b in out {
                self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
                let mut z = self.0;
                z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
                z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
                *b = (z ^ (z >> 31)) as u8;
            }
        }
    }

    #[test]
    fn bytes_round_trip_and_trim() {
        let b = Big::from_be(&[0, 0, 1, 2, 3, 4, 5]);
        assert_eq!(b.to_be_min(), [1, 2, 3, 4, 5]);
        assert_eq!(b.to_be(8), [0, 0, 0, 1, 2, 3, 4, 5]);
        assert!(Big::from_be(&[0, 0]).is_zero());
        assert_eq!(Big::from_hex("0100").to_be_min(), [1, 0]);
        assert_eq!(Big::from_u64(u64::MAX).bit_len(), 64);
    }

    #[test]
    fn arithmetic_agrees_with_u128() {
        let mut r = Stream(7);
        for _ in 0..300 {
            let mut b = [0u8; 32];
            r.fill(&mut b);
            let a = u128::from_be_bytes(b[..16].try_into().unwrap()) >> (b[16] % 100);
            let c = (u128::from_be_bytes(b[16..].try_into().unwrap()) >> (b[0] % 110)).max(1);
            let (ba, bc) = (Big::from_be(&a.to_be_bytes()), Big::from_be(&c.to_be_bytes()));
            let (q, rem) = ba.divrem(&bc);
            assert_eq!(q.to_be(16), (a / c).to_be_bytes(), "{a} / {c}");
            assert_eq!(rem.to_be(16), (a % c).to_be_bytes(), "{a} % {c}");
            let s = ba.add(&bc);
            assert_eq!(s.to_be(17)[1..], a.wrapping_add(c).to_be_bytes()[..]);
            if a >= c {
                assert_eq!(ba.sub(&bc).to_be(16), (a - c).to_be_bytes());
            }
            let (x, y) = (a >> 64, c >> 64);
            assert_eq!(Big::from_be(&x.to_be_bytes()).mul(&Big::from_be(&y.to_be_bytes())).to_be(16), (x * y).to_be_bytes());
        }
    }

    #[test]
    fn division_of_wide_numbers_satisfies_the_identity() {
        let mut r = Stream(11);
        for round in 0..200 {
            let a = random_bits(&mut r, 100 + round * 7);
            let d = random_bits(&mut r, 40 + (round * 5) % 300);
            let (q, rem) = a.divrem(&d);
            assert_eq!(q.mul(&d).add(&rem), a);
            assert_eq!(rem.cmp_big(&d), Ordering::Less);
        }
        // the add-back case of Knuth D: a divisor with a top limb of all ones
        let d = Big::from_hex("ffffffff00000000ffffffff");
        let a = Big::from_hex("ffffffffffffffffffffffffffffffff0000000100000001");
        let (q, rem) = a.divrem(&d);
        assert_eq!(q.mul(&d).add(&rem), a);
        let (q, rem) = a.divrem(&Big::zero());
        assert!(q.is_zero() && rem == a, "division by zero does not panic");
    }

    #[test]
    fn shifts_pow_and_inverse() {
        let a = Big::from_hex("0123456789abcdef0123456789abcdef");
        assert_eq!(a.shl(37).shr(37), a);
        assert_eq!(a.shr(200), Big::zero());
        // Fermat: 3^(p-1) = 1 mod p for the prime 2^127 - 1
        let p = Big::from_u64(1).shl(127).sub(&Big::from_u64(1));
        assert!(Big::from_u64(3).mod_pow(&p.sub(&Big::from_u64(1)), &p).is_one());
        let inv = a.mod_inv(&p).unwrap();
        assert!(a.mod_mul(&inv, &p).is_one());
        assert!(Big::from_u64(6).mod_inv(&Big::from_u64(9)).is_none());
        assert_eq!(Big::from_u64(48).gcd(&Big::from_u64(180)), Big::from_u64(12));
    }

    #[test]
    fn primes_are_told_from_composites() {
        let mut r = Stream(3);
        for p in [2u64, 3, 65537, 2147483647, 18446744073709551557] {
            assert!(is_probable_prime(&Big::from_u64(p), 16, &mut r), "{p}");
        }
        // Carmichael numbers and a strong pseudoprime to base 2
        for c in [561u64, 41041, 825265, 3215031751, 1000000007 * 3] {
            assert!(!is_probable_prime(&Big::from_u64(c), 16, &mut r), "{c}");
        }
        let m127 = Big::from_u64(1).shl(127).sub(&Big::from_u64(1));
        assert!(is_probable_prime(&m127, 16, &mut r));
        let p = random_prime(&mut r, 256, &Big::from_u64(65537));
        assert_eq!(p.bit_len(), 256);
        assert!(is_probable_prime(&p, 16, &mut r));
    }
}
