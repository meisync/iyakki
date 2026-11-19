//! A small seeded random generator (splitmix64 seeding, xorshift64* stream) so training and the data generator are repeatable.

#[derive(Clone, Debug)]
pub struct Rng(u64);

impl Rng {
    pub fn new(seed: u64) -> Rng {
        let mut z = seed.wrapping_add(0x9E37_79B9_7F4A_7C15);
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^= z >> 31;
        Rng(if z == 0 { 0x1234_5678_9ABC_DEF1 } else { z })
    }

    pub fn next_u64(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        self.0 = x;
        x.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }

    /// Uniform in [0, 1).
    pub fn f32(&mut self) -> f32 {
        (self.next_u64() >> 40) as f32 / (1u64 << 24) as f32
    }

    /// Uniform in [lo, hi).
    pub fn range(&mut self, lo: f32, hi: f32) -> f32 {
        lo + (hi - lo) * self.f32()
    }

    /// Uniform integer in [0, n).
    pub fn below(&mut self, n: usize) -> usize {
        if n == 0 { 0 } else { (self.next_u64() % n as u64) as usize }
    }

    pub fn chance(&mut self, p: f32) -> bool {
        self.f32() < p
    }

    /// Standard normal (Box-Muller).
    pub fn normal(&mut self) -> f32 {
        let u1 = self.f32().max(1e-7);
        let u2 = self.f32();
        (-2.0 * u1.ln()).sqrt() * (2.0 * std::f32::consts::PI * u2).cos()
    }

    pub fn shuffle<T>(&mut self, v: &mut [T]) {
        for i in (1..v.len()).rev() {
            let j = self.below(i + 1);
            v.swap(i, j);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_same_seed_gives_the_same_stream_and_the_numbers_are_spread() {
        let (mut a, mut b) = (Rng::new(7), Rng::new(7));
        assert!((0..100).all(|_| a.next_u64() == b.next_u64()));
        assert_ne!(Rng::new(7).next_u64(), Rng::new(8).next_u64());
        let mut r = Rng::new(1);
        let m: f32 = (0..10_000).map(|_| r.f32()).sum::<f32>() / 10_000.0;
        assert!((m - 0.5).abs() < 0.02);
        let n: Vec<f32> = (0..10_000).map(|_| r.normal()).collect();
        let mean = n.iter().sum::<f32>() / 10_000.0;
        let var = n.iter().map(|v| (v - mean) * (v - mean)).sum::<f32>() / 10_000.0;
        assert!(mean.abs() < 0.05 && (var - 1.0).abs() < 0.08);
    }
}
