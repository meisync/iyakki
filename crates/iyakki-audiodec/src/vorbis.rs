//! A Vorbis I decoder written from the public format description: the three header packets, the codebooks, floor type 1, residue
//! types 0 to 2, channel coupling, the windows and the overlap-add. Floor type 0 (a rare old form) is refused with a plain sentence.
//!
//! Everything read from a stream is checked against a cap before it is used to size anything, a packet that ends early gives what
//! was decoded so far (zeros for the rest), and a bad packet gives silence for that packet; nothing panics.

use crate::mdct::Imdct;

const MAX_CODEBOOK_ENTRIES: usize = 1 << 20;
const MAX_CODEBOOK_DIMS: usize = 256;
const MAX_CHANNELS: usize = 8;

/// Bits come out least significant first.
struct Bits<'a> {
    d: &'a [u8],
    pos: usize,
}

impl<'a> Bits<'a> {
    fn new(d: &'a [u8]) -> Bits<'a> {
        Bits { d, pos: 0 }
    }
    fn left(&self) -> usize {
        (self.d.len() * 8).saturating_sub(self.pos)
    }
    /// `n` (0 to 32) bits, or None when the packet ends first.
    fn read(&mut self, n: usize) -> Option<u32> {
        if n == 0 {
            return Some(0);
        }
        if self.left() < n {
            self.pos = self.d.len() * 8;
            return None;
        }
        let mut v = 0u64;
        let mut got = 0;
        while got < n {
            let byte = self.d[self.pos / 8] as u64;
            let off = self.pos % 8;
            let take = (8 - off).min(n - got);
            v |= ((byte >> off) & ((1 << take) - 1)) << got;
            got += take;
            self.pos += take;
        }
        Some(v as u32)
    }
    fn bit(&mut self) -> Option<bool> {
        self.read(1).map(|v| v == 1)
    }
}

fn ilog(x: u32) -> usize {
    (32 - x.leading_zeros()) as usize
}

fn float32_unpack(x: u32) -> f32 {
    let mant = (x & 0x1f_ffff) as f64;
    let exp = ((x & 0x7fe0_0000) >> 21) as i32;
    let v = mant * 2f64.powi(exp - 788);
    (if x & 0x8000_0000 != 0 { -v } else { v }) as f32
}

// ------------------------------------------------------------------------------------------------ codebooks

const EMPTY: i32 = 0;

struct Codebook {
    dims: usize,
    entries: usize,
    /// binary tree: `[child for bit 0, child for bit 1]`; 0 empty, positive a node, negative `-(entry + 1)` a leaf
    nodes: Vec<[i32; 2]>,
    lookup: u8,
    min: f32,
    delta: f32,
    seq: bool,
    mults: Vec<u32>,
    lookup_values: usize,
}

fn place(nodes: &mut Vec<[i32; 2]>, full: &mut Vec<bool>, node: usize, depth: usize, len: usize, entry: usize) -> bool {
    for side in 0..2 {
        let c = nodes[node][side];
        if depth + 1 == len {
            if c == EMPTY {
                nodes[node][side] = -(entry as i32) - 1;
                full[node] = nodes[node].iter().all(|&c| c < 0 || (c > 0 && full[c as usize]));
                return true;
            }
        } else if c == EMPTY || (c > 0 && !full[c as usize]) {
            let child = if c == EMPTY {
                if nodes.len() > 4 * MAX_CODEBOOK_ENTRIES {
                    return false;
                }
                nodes.push([EMPTY, EMPTY]);
                full.push(false);
                let idx = nodes.len() - 1;
                nodes[node][side] = idx as i32;
                idx
            } else {
                c as usize
            };
            if place(nodes, full, child, depth + 1, len, entry) {
                full[node] = nodes[node].iter().all(|&c| c < 0 || (c > 0 && full[c as usize]));
                return true;
            }
        }
    }
    false
}

fn lookup1_values(entries: usize, dims: usize) -> usize {
    let mut v = 0usize;
    loop {
        let next = v + 1;
        let mut p: u128 = 1;
        for _ in 0..dims {
            p = p.saturating_mul(next as u128);
            if p > entries as u128 {
                return v;
            }
        }
        v = next;
    }
}

impl Codebook {
    fn parse(b: &mut Bits) -> Result<Codebook, String> {
        let bad = || "a codebook is cut short".to_string();
        if b.read(24).ok_or_else(bad)? != 0x564342 {
            return Err("a codebook has no sync pattern".into());
        }
        let dims = b.read(16).ok_or_else(bad)? as usize;
        let entries = b.read(24).ok_or_else(bad)? as usize;
        if dims == 0 || dims > MAX_CODEBOOK_DIMS || entries == 0 || entries > MAX_CODEBOOK_ENTRIES {
            return Err("a codebook has a size that is not allowed".into());
        }
        let ordered = b.bit().ok_or_else(bad)?;
        let mut lens = vec![0u8; entries];
        if !ordered {
            let sparse = b.bit().ok_or_else(bad)?;
            for l in lens.iter_mut() {
                if sparse && !b.bit().ok_or_else(bad)? {
                    continue;
                }
                *l = b.read(5).ok_or_else(bad)? as u8 + 1;
            }
        } else {
            let mut cur = b.read(5).ok_or_else(bad)? as usize + 1;
            let mut e = 0;
            while e < entries {
                let num = b.read(ilog((entries - e) as u32)).ok_or_else(bad)? as usize;
                if e + num > entries || cur > 32 {
                    return Err("a codebook's lengths do not add up".into());
                }
                lens[e..e + num].fill(cur as u8);
                e += num;
                cur += 1;
            }
        }
        let mut nodes = vec![[EMPTY, EMPTY]];
        let mut full = vec![false];
        let used = lens.iter().filter(|&&l| l > 0).count();
        if used > 0 {
            for (e, &l) in lens.iter().enumerate() {
                if l == 0 {
                    continue;
                }
                if !place(&mut nodes, &mut full, 0, 0, l as usize, e) {
                    return Err("a codebook's code lengths over-fill the code tree".into());
                }
            }
        }
        let lookup = b.read(4).ok_or_else(bad)? as u8;
        let (mut min, mut delta, mut seq, mut mults, mut lookup_values) = (0.0, 0.0, false, vec![], 0);
        if lookup > 2 {
            return Err("a codebook has an unknown lookup type".into());
        }
        if lookup > 0 {
            min = float32_unpack(b.read(32).ok_or_else(bad)?);
            delta = float32_unpack(b.read(32).ok_or_else(bad)?);
            let bits = b.read(4).ok_or_else(bad)? as usize + 1;
            seq = b.bit().ok_or_else(bad)?;
            lookup_values = if lookup == 1 { lookup1_values(entries, dims) } else { entries.checked_mul(dims).ok_or_else(bad)? };
            if lookup_values > 8 * MAX_CODEBOOK_ENTRIES || b.left() < lookup_values * bits {
                return Err("a codebook's value table is cut short".into());
            }
            for _ in 0..lookup_values {
                mults.push(b.read(bits).ok_or_else(bad)?);
            }
        }
        Ok(Codebook { dims, entries, nodes, lookup, min, delta, seq, mults, lookup_values })
    }

    fn scalar(&self, b: &mut Bits) -> Option<usize> {
        let mut node = 0usize;
        loop {
            let c = self.nodes[node][b.read(1)? as usize];
            if c < 0 {
                return Some((-c - 1) as usize);
            }
            if c == EMPTY {
                return None;
            }
            node = c as usize;
        }
    }

    fn has_vq(&self) -> bool {
        self.lookup != 0
    }

    /// The vector of an entry.
    fn vector(&self, entry: usize, out: &mut [f32]) {
        let mut last = 0.0f32;
        let mut div = 1usize;
        for (i, o) in out.iter_mut().enumerate().take(self.dims) {
            let m = if self.lookup == 1 {
                let off = (entry / div) % self.lookup_values.max(1);
                div = div.saturating_mul(self.lookup_values.max(1));
                self.mults.get(off).copied().unwrap_or(0)
            } else {
                self.mults.get(entry * self.dims + i).copied().unwrap_or(0)
            };
            *o = m as f32 * self.delta + self.min + last;
            if self.seq {
                last = *o;
            }
        }
    }

    fn vq(&self, b: &mut Bits, out: &mut [f32]) -> Option<()> {
        let e = self.scalar(b)?;
        if e >= self.entries {
            return None;
        }
        self.vector(e, out);
        Some(())
    }
}

// ------------------------------------------------------------------------------------------------ floor 1

struct Floor1 {
    partition_class: Vec<usize>,
    class_dims: Vec<usize>,
    class_subs: Vec<usize>,
    class_master: Vec<usize>,
    class_books: Vec<Vec<i32>>,
    mult: usize,
    rangebits: usize,
    xs: Vec<usize>,
    sorted: Vec<usize>,
    lo: Vec<usize>,
    hi: Vec<usize>,
}

fn db_table() -> &'static [f32; 256] {
    static T: std::sync::OnceLock<[f32; 256]> = std::sync::OnceLock::new();
    T.get_or_init(|| {
        // the table of the specification is a geometric series from 1.0649863e-07 to 1
        let lnr = (1.0f64 / 1.064_986_3e-7).ln() / 255.0;
        let mut t = [0f32; 256];
        for (i, e) in t.iter_mut().enumerate() {
            *e = (-((255 - i) as f64) * lnr).exp() as f32;
        }
        t
    })
}

impl Floor1 {
    fn parse(b: &mut Bits, nbooks: usize) -> Result<Floor1, String> {
        let bad = || "a floor is cut short".to_string();
        let parts = b.read(5).ok_or_else(bad)? as usize;
        let mut partition_class = vec![];
        let mut maxc = 0;
        for _ in 0..parts {
            let c = b.read(4).ok_or_else(bad)? as usize;
            maxc = maxc.max(c);
            partition_class.push(c);
        }
        let (mut class_dims, mut class_subs, mut class_master, mut class_books) = (vec![], vec![], vec![], vec![]);
        for _ in 0..=maxc {
            class_dims.push(b.read(3).ok_or_else(bad)? as usize + 1);
            let subs = b.read(2).ok_or_else(bad)? as usize;
            class_subs.push(subs);
            let master = if subs != 0 { b.read(8).ok_or_else(bad)? as usize } else { 0 };
            if master >= nbooks {
                return Err("a floor names a codebook that is not there".into());
            }
            class_master.push(master);
            let mut books = vec![];
            for _ in 0..(1usize << subs) {
                let v = b.read(8).ok_or_else(bad)? as i32 - 1;
                if v >= nbooks as i32 {
                    return Err("a floor names a codebook that is not there".into());
                }
                books.push(v);
            }
            class_books.push(books);
        }
        let mult = b.read(2).ok_or_else(bad)? as usize + 1;
        let rangebits = b.read(4).ok_or_else(bad)? as usize;
        let mut xs = vec![0usize, 1usize << rangebits];
        for &c in &partition_class {
            for _ in 0..class_dims[c] {
                xs.push(b.read(rangebits).ok_or_else(bad)? as usize);
            }
        }
        if xs.len() > 65 + 2 {
            return Err("a floor has too many points".into());
        }
        let mut sorted: Vec<usize> = (0..xs.len()).collect();
        sorted.sort_by_key(|&i| xs[i]);
        if sorted.windows(2).any(|w| xs[w[0]] == xs[w[1]]) {
            return Err("a floor has two points at the same place".into());
        }
        let n = xs.len();
        let (mut lo, mut hi) = (vec![0; n], vec![0; n]);
        for i in 2..n {
            let (mut l, mut h) = (0usize, 1usize);
            for j in 0..i {
                if xs[j] < xs[i] && xs[j] > xs[l] {
                    l = j;
                }
                if xs[j] > xs[i] && xs[j] < xs[h] {
                    h = j;
                }
            }
            lo[i] = l;
            hi[i] = h;
        }
        Ok(Floor1 { partition_class, class_dims, class_subs, class_master, class_books, mult, rangebits, xs, sorted, lo, hi })
    }

    /// The curve over `half` points, or None when the channel has no floor in this packet (or the packet ended).
    fn decode(&self, b: &mut Bits, books: &[Codebook], half: usize) -> Option<Vec<f32>> {
        if !b.bit()? {
            return None;
        }
        let range = [256i32, 128, 86, 64][self.mult - 1];
        let n = self.xs.len();
        let mut y = vec![0i32; n];
        y[0] = b.read(ilog(range as u32 - 1))? as i32;
        y[1] = b.read(ilog(range as u32 - 1))? as i32;
        let mut off = 2;
        for &c in &self.partition_class {
            let cdim = self.class_dims[c];
            let cbits = self.class_subs[c];
            let csub = (1usize << cbits) - 1;
            let mut cval = 0usize;
            if cbits > 0 {
                cval = books[self.class_master[c]].scalar(b)?;
            }
            for j in 0..cdim {
                let book = self.class_books[c][cval & csub];
                cval >>= cbits;
                y[off + j] = if book >= 0 { books[book as usize].scalar(b)? as i32 } else { 0 };
            }
            off += cdim;
        }
        // amplitude synthesis
        let mut fy = vec![0i32; n];
        let mut step2 = vec![false; n];
        fy[0] = y[0];
        fy[1] = y[1];
        step2[0] = true;
        step2[1] = true;
        for i in 2..n {
            let (lo, hi) = (self.lo[i], self.hi[i]);
            let predicted = render_point(self.xs[lo] as i32, fy[lo], self.xs[hi] as i32, fy[hi], self.xs[i] as i32);
            let val = y[i];
            let highroom = range - predicted;
            let lowroom = predicted;
            let room = if highroom < lowroom { highroom * 2 } else { lowroom * 2 };
            if val != 0 {
                step2[lo] = true;
                step2[hi] = true;
                step2[i] = true;
                fy[i] = if val >= room {
                    if highroom > lowroom { val - lowroom + predicted } else { predicted - val + highroom - 1 }
                } else if val & 1 == 1 {
                    predicted - (val + 1) / 2
                } else {
                    predicted + val / 2
                };
            } else {
                step2[i] = false;
                fy[i] = predicted;
            }
            fy[i] = fy[i].clamp(0, range - 1);
        }
        let db = db_table();
        let mut floor = vec![0f32; half];
        let m = self.mult as i32;
        let idx = |v: i32| (v * m).clamp(0, 255) as usize;
        let mut hx = 0usize;
        let mut lx = 0usize;
        let mut ly = idx(fy[self.sorted[0]]) as i32;
        let mut hy = ly;
        for &i in self.sorted.iter().skip(1) {
            if step2[i] {
                hy = idx(fy[i]) as i32;
                hx = self.xs[i];
                render_line(lx as i32, ly, hx as i32, hy, &mut floor, db);
                lx = hx;
                ly = hy;
            }
        }
        if hx < half {
            let v = db[hy.clamp(0, 255) as usize];
            for f in floor.iter_mut().skip(hx) {
                *f = v;
            }
        }
        let _ = self.rangebits;
        Some(floor)
    }
}

fn render_point(x0: i32, y0: i32, x1: i32, y1: i32, x: i32) -> i32 {
    let dy = y1 - y0;
    let adx = x1 - x0;
    if adx == 0 {
        return y0;
    }
    let ady = dy.abs();
    let err = ady * (x - x0);
    let off = err / adx;
    if dy < 0 { y0 - off } else { y0 + off }
}

fn render_line(x0: i32, y0: i32, x1: i32, y1: i32, out: &mut [f32], db: &[f32; 256]) {
    let dy = y1 - y0;
    let adx = x1 - x0;
    if adx <= 0 {
        return;
    }
    let mut ady = dy.abs();
    let base = dy / adx;
    let sy = if dy < 0 { base - 1 } else { base + 1 };
    ady -= base.abs() * adx;
    let (mut x, mut y, mut err) = (x0, y0, 0);
    let put = |x: i32, y: i32, out: &mut [f32]| {
        if x >= 0 && (x as usize) < out.len() {
            out[x as usize] = db[y.clamp(0, 255) as usize];
        }
    };
    put(x, y, out);
    x += 1;
    while x < x1.min(out.len() as i32) {
        err += ady;
        if err >= adx {
            err -= adx;
            y += sy;
        } else {
            y += base;
        }
        put(x, y, out);
        x += 1;
    }
}

// ------------------------------------------------------------------------------------------------ residue

struct Residue {
    kind: u8,
    begin: usize,
    end: usize,
    psize: usize,
    classes: usize,
    classbook: usize,
    books: Vec<[i32; 8]>,
}

impl Residue {
    fn parse(b: &mut Bits, kind: u8, nbooks: usize, books: &[Codebook]) -> Result<Residue, String> {
        let bad = || "a residue is cut short".to_string();
        let begin = b.read(24).ok_or_else(bad)? as usize;
        let end = b.read(24).ok_or_else(bad)? as usize;
        let psize = b.read(24).ok_or_else(bad)? as usize + 1;
        let classes = b.read(6).ok_or_else(bad)? as usize + 1;
        let classbook = b.read(8).ok_or_else(bad)? as usize;
        if classbook >= nbooks || end < begin {
            return Err("a residue is not valid".into());
        }
        // a class codeword must be able to name every class of its partitions
        if (books[classbook].dims as f64) * (classes as f64).log2() > 40.0 {
            return Err("a residue's class book is too large".into());
        }
        let mut cascade = vec![];
        for _ in 0..classes {
            let low = b.read(3).ok_or_else(bad)?;
            let high = if b.bit().ok_or_else(bad)? { b.read(5).ok_or_else(bad)? } else { 0 };
            cascade.push(high * 8 + low);
        }
        let mut bk = vec![];
        for c in &cascade {
            let mut a = [-1i32; 8];
            for (j, slot) in a.iter_mut().enumerate() {
                if c & (1 << j) != 0 {
                    let v = b.read(8).ok_or_else(bad)? as usize;
                    if v >= nbooks || !books[v].has_vq() {
                        return Err("a residue names a codebook it cannot use".into());
                    }
                    *slot = v as i32;
                }
            }
            bk.push(a);
        }
        Ok(Residue { kind, begin, end, psize, classes, classbook, books: bk })
    }

    /// Adds the residue of the channels in `vecs` (each `n` long); `skip[i]` channels are left as they are.
    fn decode(&self, b: &mut Bits, books: &[Codebook], skip: &[bool], vecs: &mut [Vec<f32>], n: usize) {
        let nch = vecs.len();
        let begin = self.begin.min(n);
        let end = self.end.min(n);
        let to_read = end.saturating_sub(begin);
        let parts = to_read / self.psize;
        if parts == 0 || skip.iter().all(|&s| s) {
            return;
        }
        let cb = &books[self.classbook];
        let cpc = cb.dims;
        let mut classes = vec![vec![0usize; parts + cpc]; nch];
        for pass in 0..8 {
            let mut p = 0;
            while p < parts {
                if pass == 0 {
                    for j in 0..nch {
                        if skip[j] {
                            continue;
                        }
                        let Some(mut t) = cb.scalar(b) else { return };
                        for i in (0..cpc).rev() {
                            classes[j][p + i] = t % self.classes;
                            t /= self.classes;
                        }
                    }
                }
                for _ in 0..cpc {
                    if p >= parts {
                        break;
                    }
                    for j in 0..nch {
                        if skip[j] {
                            continue;
                        }
                        let bk = self.books[classes[j][p]][pass];
                        if bk < 0 {
                            continue;
                        }
                        let book = &books[bk as usize];
                        let at = begin + p * self.psize;
                        let mut v = vec![0f32; book.dims];
                        if self.kind == 0 {
                            let step = self.psize / book.dims.max(1);
                            for i in 0..step {
                                if book.vq(b, &mut v).is_none() {
                                    return;
                                }
                                for (k, x) in v.iter().enumerate() {
                                    if let Some(o) = vecs[j].get_mut(at + i + k * step) {
                                        *o += x;
                                    }
                                }
                            }
                        } else {
                            let mut i = 0;
                            while i < self.psize {
                                if book.vq(b, &mut v).is_none() {
                                    return;
                                }
                                for (k, x) in v.iter().enumerate() {
                                    if let Some(o) = vecs[j].get_mut(at + i + k) {
                                        *o += x;
                                    }
                                }
                                i += book.dims;
                            }
                        }
                    }
                    p += 1;
                }
            }
        }
    }
}

// ------------------------------------------------------------------------------------------------ mappings and modes

struct Mapping {
    coupling: Vec<(usize, usize)>,
    mux: Vec<usize>,
    submaps: Vec<(usize, usize)>,
}

struct Mode {
    blockflag: bool,
    mapping: usize,
}

pub struct Decoder {
    channels: usize,
    rate: u32,
    bs: [usize; 2],
    books: Vec<Codebook>,
    floors: Vec<Floor1>,
    residues: Vec<Residue>,
    mappings: Vec<Mapping>,
    modes: Vec<Mode>,
    imdct: [Imdct; 2],
    slopes: [Vec<f32>; 2],
    /// per channel: the previous block's samples from its centre on
    tail: Vec<Vec<f32>>,
    prev_n: usize,
    produced: u64,
}

fn slope(len: usize) -> Vec<f32> {
    (0..len).map(|i| {
        let s = ((i as f64 + 0.5) / len as f64 * std::f64::consts::FRAC_PI_2).sin();
        (std::f64::consts::FRAC_PI_2 * s * s).sin() as f32
    }).collect()
}

impl Decoder {
    pub fn new(id: &[u8], comment: &[u8], setup: &[u8]) -> Result<Decoder, String> {
        // identification
        if id.len() < 30 || id[0] != 1 || &id[1..7] != b"vorbis" {
            return Err("this is not a Vorbis stream".into());
        }
        if u32::from_le_bytes([id[7], id[8], id[9], id[10]]) != 0 {
            return Err("this Vorbis version is not known".into());
        }
        let channels = id[11] as usize;
        let rate = u32::from_le_bytes([id[12], id[13], id[14], id[15]]);
        let (b0, b1) = ((id[28] & 15) as usize, (id[28] >> 4) as usize);
        if channels == 0 || channels > MAX_CHANNELS || rate == 0 || rate > 400_000 || !(6..=13).contains(&b0) || !(6..=13).contains(&b1) || b0 > b1 || id[29] & 1 == 0 {
            return Err("the Vorbis header has values that are not allowed".into());
        }
        if comment.len() < 7 || comment[0] != 3 || &comment[1..7] != b"vorbis" {
            return Err("the Vorbis comment header is missing".into());
        }
        if setup.len() < 7 || setup[0] != 5 || &setup[1..7] != b"vorbis" {
            return Err("the Vorbis setup header is missing".into());
        }
        let mut b = Bits::new(&setup[7..]);
        let bad = || "the Vorbis setup header is cut short".to_string();
        let nb = b.read(8).ok_or_else(bad)? as usize + 1;
        let mut books = vec![];
        for _ in 0..nb {
            books.push(Codebook::parse(&mut b)?);
        }
        let nt = b.read(6).ok_or_else(bad)? as usize + 1;
        for _ in 0..nt {
            if b.read(16).ok_or_else(bad)? != 0 {
                return Err("the Vorbis setup has a time transform that is not defined".into());
            }
        }
        let nf = b.read(6).ok_or_else(bad)? as usize + 1;
        let mut floors = vec![];
        for _ in 0..nf {
            match b.read(16).ok_or_else(bad)? {
                1 => floors.push(Floor1::parse(&mut b, books.len())?),
                0 => return Err("this stream uses floor type 0, an old form this decoder does not read".into()),
                _ => return Err("the Vorbis setup has an unknown floor type".into()),
            }
        }
        let nr = b.read(6).ok_or_else(bad)? as usize + 1;
        let mut residues = vec![];
        for _ in 0..nr {
            let t = b.read(16).ok_or_else(bad)?;
            if t > 2 {
                return Err("the Vorbis setup has an unknown residue type".into());
            }
            residues.push(Residue::parse(&mut b, t as u8, books.len(), &books)?);
        }
        let nm = b.read(6).ok_or_else(bad)? as usize + 1;
        let mut mappings = vec![];
        for _ in 0..nm {
            if b.read(16).ok_or_else(bad)? != 0 {
                return Err("the Vorbis setup has an unknown mapping type".into());
            }
            let nsub = if b.bit().ok_or_else(bad)? { b.read(4).ok_or_else(bad)? as usize + 1 } else { 1 };
            let mut coupling = vec![];
            if b.bit().ok_or_else(bad)? {
                let steps = b.read(8).ok_or_else(bad)? as usize + 1;
                for _ in 0..steps {
                    let m = b.read(ilog(channels as u32 - 1)).ok_or_else(bad)? as usize;
                    let a = b.read(ilog(channels as u32 - 1)).ok_or_else(bad)? as usize;
                    if m == a || m >= channels || a >= channels {
                        return Err("the Vorbis setup couples channels wrongly".into());
                    }
                    coupling.push((m, a));
                }
            }
            if b.read(2).ok_or_else(bad)? != 0 {
                return Err("the Vorbis setup has a reserved value set".into());
            }
            let mut mux = vec![0usize; channels];
            if nsub > 1 {
                for m in mux.iter_mut() {
                    *m = b.read(4).ok_or_else(bad)? as usize;
                    if *m >= nsub {
                        return Err("the Vorbis setup names a submap that is not there".into());
                    }
                }
            }
            let mut submaps = vec![];
            for _ in 0..nsub {
                b.read(8).ok_or_else(bad)?;
                let f = b.read(8).ok_or_else(bad)? as usize;
                let r = b.read(8).ok_or_else(bad)? as usize;
                if f >= floors.len() || r >= residues.len() {
                    return Err("the Vorbis setup names a floor or residue that is not there".into());
                }
                submaps.push((f, r));
            }
            mappings.push(Mapping { coupling, mux, submaps });
        }
        let nmo = b.read(6).ok_or_else(bad)? as usize + 1;
        let mut modes = vec![];
        for _ in 0..nmo {
            let blockflag = b.bit().ok_or_else(bad)?;
            if b.read(16).ok_or_else(bad)? != 0 || b.read(16).ok_or_else(bad)? != 0 {
                return Err("the Vorbis setup has a window or transform type that is not defined".into());
            }
            let mapping = b.read(8).ok_or_else(bad)? as usize;
            if mapping >= mappings.len() {
                return Err("the Vorbis setup names a mapping that is not there".into());
            }
            modes.push(Mode { blockflag, mapping });
        }
        if !b.bit().ok_or_else(bad)? {
            return Err("the Vorbis setup has no framing bit".into());
        }
        let bs = [1usize << b0, 1usize << b1];
        Ok(Decoder { channels, rate, bs, books, floors, residues, mappings, modes, imdct: [Imdct::new(bs[0]), Imdct::new(bs[1])], slopes: [slope(bs[0] / 2), slope(bs[1] / 2)], tail: vec![vec![]; channels], prev_n: 0, produced: 0 })
    }

    pub fn rate(&self) -> u32 {
        self.rate
    }
    pub fn channels(&self) -> usize {
        self.channels
    }
    /// Samples per channel returned so far.
    pub fn produced(&self) -> u64 {
        self.produced
    }

    /// One audio packet: interleaved samples (about -1 to 1); empty for the first packet and for a packet that is not audio.
    pub fn decode(&mut self, pkt: &[u8]) -> Vec<f32> {
        let mut b = Bits::new(pkt);
        if b.read(1) != Some(0) {
            return vec![];
        }
        let mode_bits = ilog(self.modes.len() as u32 - 1);
        let Some(mn) = b.read(mode_bits) else { return vec![] };
        let Some(mode) = self.modes.get(mn as usize) else { return vec![] };
        let flag = mode.blockflag;
        let (prev_flag, next_flag) = if flag {
            match (b.bit(), b.bit()) {
                (Some(p), Some(n)) => (p, n),
                _ => return vec![],
            }
        } else {
            (false, false)
        };
        let n = self.bs[flag as usize];
        let half = n / 2;
        let map = &self.mappings[mode.mapping];
        let ch = self.channels;
        // floors
        let mut floors: Vec<Option<Vec<f32>>> = Vec::with_capacity(ch);
        for c in 0..ch {
            let (f, _) = map.submaps[map.mux[c]];
            floors.push(self.floors[f].decode(&mut b, &self.books, half));
        }
        let mut skip: Vec<bool> = floors.iter().map(|f| f.is_none()).collect();
        for &(m, a) in &map.coupling {
            if !skip[m] || !skip[a] {
                skip[m] = false;
                skip[a] = false;
            }
        }
        let mut v: Vec<Vec<f32>> = vec![vec![0.0; half]; ch];
        for (s, &(_, r)) in map.submaps.iter().enumerate() {
            let chans: Vec<usize> = (0..ch).filter(|&c| map.mux[c] == s).collect();
            if chans.is_empty() {
                continue;
            }
            let res = &self.residues[r];
            if res.kind == 2 {
                let all = chans.iter().all(|&c| skip[c]);
                let mut one = vec![vec![0f32; chans.len() * half]];
                res.decode(&mut b, &self.books, &[all], &mut one, chans.len() * half);
                for (k, &c) in chans.iter().enumerate() {
                    for i in 0..half {
                        v[c][i] = one[0][i * chans.len() + k];
                    }
                }
            } else {
                let sk: Vec<bool> = chans.iter().map(|&c| skip[c]).collect();
                let mut vs: Vec<Vec<f32>> = chans.iter().map(|_| vec![0f32; half]).collect();
                res.decode(&mut b, &self.books, &sk, &mut vs, half);
                for (k, &c) in chans.iter().enumerate() {
                    v[c] = std::mem::take(&mut vs[k]);
                }
            }
        }
        // inverse coupling, last step first
        for &(m, a) in map.coupling.iter().rev() {
            for i in 0..half {
                let (mv, av) = (v[m][i], v[a][i]);
                let (nm, na) = if mv > 0.0 {
                    if av > 0.0 { (mv, mv - av) } else { (mv + av, mv) }
                } else if av > 0.0 {
                    (mv, mv + av)
                } else {
                    (mv - av, mv)
                };
                v[m][i] = nm;
                v[a][i] = na;
            }
        }
        // window
        let bs0 = self.bs[0];
        let mut w = vec![0f32; n];
        let long = flag;
        let (ls, le, llen) = if long && !prev_flag { (n / 4 - bs0 / 4, n / 4 + bs0 / 4, bs0 / 2) } else { (0, half, half) };
        let (rs, re, rlen) = if long && !next_flag { (3 * n / 4 - bs0 / 4, 3 * n / 4 + bs0 / 4, bs0 / 2) } else { (half, n, half) };
        let lsl = &self.slopes[if llen == self.bs[0] / 2 { 0 } else { 1 }];
        let rsl = &self.slopes[if rlen == self.bs[0] / 2 { 0 } else { 1 }];
        for (i, x) in w.iter_mut().enumerate().take(half) {
            *x = if i < ls { 0.0 } else if i < le { lsl[i - ls] } else { 1.0 };
        }
        for (i, x) in w.iter_mut().enumerate().skip(half) {
            *x = if i < rs { 1.0 } else if i < re { rsl[rlen - 1 - (i - rs)] } else { 0.0 };
        }
        // transform and overlap
        let mut cur = vec![vec![0f32; n]; ch];
        for c in 0..ch {
            if let Some(f) = &floors[c] {
                for i in 0..half {
                    v[c][i] *= f[i];
                }
                self.imdct[flag as usize].run(&v[c], &mut cur[c]);
                for i in 0..n {
                    cur[c][i] *= w[i];
                }
            }
        }
        let mut out = vec![];
        if self.prev_n != 0 {
            let emit = self.prev_n / 4 + n / 4;
            let off = self.prev_n as isize / 4 - n as isize / 4;
            out = vec![0f32; emit * ch];
            for c in 0..ch {
                for j in 0..emit {
                    let mut s = self.tail[c].get(j).copied().unwrap_or(0.0);
                    let k = j as isize - off;
                    if k >= 0 && (k as usize) < n {
                        s += cur[c][k as usize];
                    }
                    out[j * ch + c] = s * SCALE;
                }
            }
            self.produced += emit as u64;
        }
        for c in 0..ch {
            self.tail[c] = cur[c][half..].to_vec();
        }
        self.prev_n = n;
        out
    }
}

/// The transform's scale (calibrated against the reference decode in the tests).
const SCALE: f32 = 1.0;

#[cfg(test)]
#[path = "vorbis_tests.rs"]
mod tests;
