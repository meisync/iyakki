//! In-house deterministic fuzz kit for tests: a seeded mutator, a per-thread allocation meter and a case driver. No dependencies.
//! A panic in the code under test is a finding: nothing is caught here; a guard prints the failing seed and case while the panic unwinds.
#![allow(dead_code)]

use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;
use std::time::{Duration, Instant};

pub struct Meter;

thread_local! {
    static CUR: Cell<i64> = const { Cell::new(0) };
    static PEAK: Cell<i64> = const { Cell::new(0) };
}

unsafe impl GlobalAlloc for Meter {
    unsafe fn alloc(&self, l: Layout) -> *mut u8 {
        add(l.size() as i64);
        System.alloc(l)
    }
    unsafe fn alloc_zeroed(&self, l: Layout) -> *mut u8 {
        add(l.size() as i64);
        System.alloc_zeroed(l)
    }
    unsafe fn dealloc(&self, p: *mut u8, l: Layout) {
        add(-(l.size() as i64));
        System.dealloc(p, l)
    }
    unsafe fn realloc(&self, p: *mut u8, l: Layout, n: usize) -> *mut u8 {
        add(n as i64 - l.size() as i64);
        System.realloc(p, l, n)
    }
}

fn add(d: i64) {
    let _ = CUR.try_with(|c| {
        let v = c.get() + d;
        c.set(v);
        let _ = PEAK.try_with(|p| {
            if v > p.get() {
                p.set(v)
            }
        });
    });
}

#[global_allocator]
static METER: Meter = Meter;

/// Peak bytes live on this thread above the level at `reset_peak`.
pub fn reset_peak() {
    CUR.with(|c| PEAK.with(|p| p.set(c.get())));
}
pub fn peak_above_start(start: i64) -> i64 {
    PEAK.with(|p| p.get()) - start
}
pub fn live() -> i64 {
    CUR.with(|c| c.get())
}

pub struct Rng(pub u64);

impl Rng {
    pub fn new(seed: u64) -> Rng {
        Rng(seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) ^ 0xD1B5_4A32_D192_ED03)
    }
    pub fn next(&mut self) -> u64 {
        // xorshift64*
        let mut x = self.0;
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        self.0 = x;
        x.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }
    pub fn below(&mut self, n: usize) -> usize {
        if n == 0 {
            0
        } else {
            (self.next() % n as u64) as usize
        }
    }
    pub fn chance(&mut self, one_in: usize) -> bool {
        self.below(one_in) == 0
    }
    pub fn pick<'a, T>(&mut self, v: &'a [T]) -> &'a T {
        &v[self.below(v.len())]
    }
}

/// Case count: `default` unless `IYAKKI_FUZZ_CASES` is set.
pub fn cases(default: usize) -> usize {
    std::env::var("IYAKKI_FUZZ_CASES").ok().and_then(|s| s.parse().ok()).unwrap_or(default)
}

const INTERESTING: [u8; 9] = [0, 1, 0x7f, 0x80, 0xff, 0xfe, 0x20, 0x0a, 0x22];

/// One random mutation of `data` (or a stack of up to three).
pub fn mutate(r: &mut Rng, data: &[u8]) -> Vec<u8> {
    let mut d = data.to_vec();
    for _ in 0..1 + r.below(3) {
        match r.below(9) {
            0 if !d.is_empty() => {
                for _ in 0..1 + r.below(4) {
                    let i = r.below(d.len());
                    d[i] ^= 1 << r.below(8);
                }
            }
            1 if !d.is_empty() => {
                let i = r.below(d.len());
                d[i] = *r.pick(&INTERESTING);
            }
            2 => {
                let n = r.below(d.len() + 1);
                d.truncate(n);
            }
            3 if !d.is_empty() => {
                // inflate a length field: write all-ones / huge big- or little-endian numbers over 1..8 bytes
                let w = *r.pick(&[1usize, 2, 3, 4, 8]);
                let i = r.below(d.len());
                let v = match r.below(4) {
                    0 => u64::MAX,
                    1 => 0x7fff_ffff_ffff_ffff,
                    2 => 0x0100_0000,
                    _ => 0xffff_fff0,
                };
                let be = r.chance(2);
                for k in 0..w.min(d.len() - i) {
                    let sh = if be { 8 * (w - 1 - k) } else { 8 * k };
                    d[i + k] = if sh < 64 { (v >> sh) as u8 } else { 0xff };
                }
            }
            4 if !d.is_empty() => {
                // duplicate a chunk in place
                let a = r.below(d.len());
                let n = 1 + r.below((d.len() - a).min(64));
                let chunk: Vec<u8> = d[a..a + n].to_vec();
                let at = r.below(d.len() + 1);
                for (k, b) in chunk.iter().enumerate() {
                    d.insert(at + k, *b);
                }
            }
            5 => {
                let at = r.below(d.len() + 1);
                for _ in 0..1 + r.below(16) {
                    d.insert(at, r.next() as u8);
                }
            }
            6 if !d.is_empty() => {
                let a = r.below(d.len());
                let n = 1 + r.below((d.len() - a).min(32));
                d.drain(a..a + n);
            }
            7 if d.len() > 1 => {
                let (a, b) = (r.below(d.len()), r.below(d.len()));
                d.swap(a, b);
            }
            _ if !d.is_empty() => {
                // repeat a short chunk many times (deep nesting / long runs)
                let a = r.below(d.len());
                let n = 1 + r.below((d.len() - a).min(4));
                let chunk: Vec<u8> = d[a..a + n].to_vec();
                let times = 1 + r.below(300);
                let mut ins = Vec::new();
                for _ in 0..times {
                    ins.extend_from_slice(&chunk);
                }
                let at = r.below(d.len() + 1);
                d.splice(at..at, ins);
            }
            _ => {}
        }
    }
    d
}

pub struct Report(String);
impl Report {
    pub fn new(s: String) -> Report {
        Report(s)
    }
}
impl Drop for Report {
    fn drop(&mut self) {
        if std::thread::panicking() {
            crate::qeprintln!("FUZZ FINDING: {}", self.0);
        }
    }
}

/// Runs `f` on: every prefix of every corpus sample (truncation at every byte), every single byte set to 0x00/0xff/0x7f/0x80 at every position
/// of the first `walk` bytes, then `cases` seeded mutations. Each case is timed (`max_each`) and its peak heap is checked (`max_heap` bytes).
pub fn run(name: &str, seed: u64, corpus: &[Vec<u8>], default_cases: usize, max_each: Duration, max_heap: i64, mut f: impl FnMut(&[u8])) -> usize {
    let mut n = 0;
    let mut one = |label: String, input: &[u8]| {
        let _rep = Report(format!("{name}: seed {seed} {label} (len {}) first bytes {:02x?}", input.len(), &input[..input.len().min(48)]));
        let start = live();
        reset_peak();
        let t = Instant::now();
        f(input);
        let took = t.elapsed();
        assert!(took <= max_each, "{name}: {label} took {took:?} (limit {max_each:?})");
        let peak = peak_above_start(start);
        assert!(peak <= max_heap, "{name}: {label} used {peak} bytes of heap (limit {max_heap})");
        n += 1;
    };
    for (ci, c) in corpus.iter().enumerate() {
        // truncation at every byte (large samples: every byte of the first 400 and the last 60, then an even stride of about 1000 cuts)
        let stride = (c.len() / 1000).max(1);
        for len in 0..=c.len() {
            if stride == 1 || len < 400 || len + 60 > c.len() || len % stride == 0 {
                one(format!("corpus {ci} cut at {len}"), &c[..len]);
            }
        }
        let walk = c.len().min(120);
        for pos in 0..walk {
            for v in [0x00u8, 0xff, 0x7f, 0x80] {
                let mut m = c.clone();
                m[pos] = v;
                one(format!("corpus {ci} byte {pos} = {v:#x}"), &m);
            }
        }
    }
    let mut r = Rng::new(seed);
    for k in 0..cases(default_cases) {
        let base = r.pick(corpus).clone();
        let m = mutate(&mut r, &base);
        one(format!("case {k}"), &m);
    }
    n
}
