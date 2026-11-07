//! A QR code encoder of our own (ISO/IEC 18004 is a public standard): byte mode, error correction levels L, M, Q and H, the mask
//! with the lowest penalty. [`encode`] makes level L symbols of versions 1 to 20 (up to 858 bytes); [`encode_level`] makes any
//! level up to version 40 (up to 2953 bytes at level L). It exists so a sign-in address can be scanned with a phone from a screen that has
//! no browser. No dependency; the tests check the codewords, the format bits and the Reed-Solomon remainder of what it draws.

/// An encoded symbol: `size` x `size` modules, `true` = dark.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Qr {
    pub size: usize,
    modules: Vec<bool>,
}

impl Qr {
    pub fn get(&self, x: usize, y: usize) -> bool {
        x < self.size && y < self.size && self.modules[y * self.size + x]
    }
    pub fn version(&self) -> usize {
        (self.size - 17) / 4
    }
    /// A symbol from its modules (row by row); the decoder's tests build symbols at other levels this way.
    #[cfg(test)]
    pub(crate) fn from_modules(size: usize, modules: Vec<bool>) -> Qr {
        Qr { size, modules }
    }
}

/// Most bytes [`encode`] can hold (level L, version 20).
pub const MAX_BYTES: usize = 858;
/// The largest version [`encode_level`] makes (177 x 177 modules).
pub const MAX_VERSION: usize = 40;

/// How much of the symbol may be damaged and still read: about 7%, 15%, 25% and 30%.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Level {
    L,
    M,
    Q,
    H,
}

impl Level {
    pub const ALL: [Level; 4] = [Level::L, Level::M, Level::Q, Level::H];
    pub fn letter(self) -> char {
        ['L', 'M', 'Q', 'H'][self as usize]
    }
    /// The two format bits the standard writes for the level (L = 01, M = 00, Q = 11, H = 10).
    fn format_bits(self) -> usize {
        [1, 0, 3, 2][self as usize]
    }
}

/// Error-correction codewords per block and number of blocks, by level (L, M, Q, H) and version (index 0 is unused).
pub(crate) const ECC_TABLE: [[usize; 41]; 4] = [
    [0, 7, 10, 15, 20, 26, 18, 20, 24, 30, 18, 20, 24, 26, 30, 22, 24, 28, 30, 28, 28, 28, 28, 30, 30, 26, 28, 30, 30, 30, 30, 30, 30, 30, 30, 30, 30, 30, 30, 30, 30],
    [0, 10, 16, 26, 18, 24, 16, 18, 22, 22, 26, 30, 22, 22, 24, 24, 28, 28, 26, 26, 26, 26, 28, 28, 28, 28, 28, 28, 28, 28, 28, 28, 28, 28, 28, 28, 28, 28, 28, 28, 28],
    [0, 13, 22, 18, 26, 18, 24, 18, 22, 20, 24, 28, 26, 24, 20, 30, 24, 28, 28, 26, 30, 28, 30, 30, 30, 30, 28, 30, 30, 30, 30, 30, 30, 30, 30, 30, 30, 30, 30, 30, 30],
    [0, 17, 28, 22, 16, 22, 28, 26, 26, 24, 28, 24, 28, 22, 24, 24, 30, 28, 28, 26, 28, 30, 24, 30, 30, 30, 30, 30, 30, 30, 30, 30, 30, 30, 30, 30, 30, 30, 30, 30, 30],
];
pub(crate) const BLOCK_TABLE: [[usize; 41]; 4] = [
    [0, 1, 1, 1, 1, 1, 2, 2, 2, 2, 4, 4, 4, 4, 4, 6, 6, 6, 6, 7, 8, 8, 9, 9, 10, 12, 12, 12, 13, 14, 15, 16, 17, 18, 19, 19, 20, 21, 22, 24, 25],
    [0, 1, 1, 1, 2, 2, 4, 4, 4, 5, 5, 5, 8, 9, 9, 10, 10, 11, 13, 14, 16, 17, 17, 18, 20, 21, 23, 25, 26, 28, 29, 31, 33, 35, 37, 38, 40, 43, 45, 47, 49],
    [0, 1, 1, 2, 2, 4, 4, 6, 6, 8, 8, 8, 10, 12, 16, 12, 17, 16, 18, 21, 20, 23, 23, 25, 27, 29, 34, 34, 35, 38, 40, 43, 45, 48, 51, 53, 56, 59, 62, 65, 68],
    [0, 1, 1, 2, 4, 4, 4, 5, 6, 8, 8, 11, 11, 16, 16, 18, 16, 19, 21, 25, 25, 25, 34, 30, 32, 35, 37, 40, 42, 45, 48, 51, 54, 57, 60, 63, 66, 70, 74, 77, 81],
];

pub(crate) fn raw_modules(ver: usize) -> usize {
    let mut r = (16 * ver + 128) * ver + 64;
    if ver >= 2 {
        let n = ver / 7 + 2;
        r -= (25 * n - 10) * n - 55;
        if ver >= 7 {
            r -= 36;
        }
    }
    r
}

/// The data codewords a symbol of this version and level holds.
pub fn data_codewords(ver: usize, level: Level) -> usize {
    raw_modules(ver) / 8 - ECC_TABLE[level as usize][ver] * BLOCK_TABLE[level as usize][ver]
}

/// The most bytes (byte mode) a symbol of this version and level holds.
pub fn capacity(ver: usize, level: Level) -> usize {
    (data_codewords(ver, level) * 8 - 4 - if ver >= 10 { 16 } else { 8 }) / 8
}

fn gf_mul(x: u8, y: u8) -> u8 {
    let mut z: u32 = 0;
    for i in (0..8).rev() {
        z = (z << 1) ^ ((z >> 7) * 0x11D);
        z ^= ((y as u32 >> i) & 1) * x as u32;
    }
    z as u8
}

pub(crate) fn rs_divisor(degree: usize) -> Vec<u8> {
    let mut result = vec![0u8; degree];
    result[degree - 1] = 1;
    let mut root = 1u8;
    for _ in 0..degree {
        for j in 0..degree {
            result[j] = gf_mul(result[j], root);
            if j + 1 < degree {
                result[j] ^= result[j + 1];
            }
        }
        root = gf_mul(root, 2);
    }
    result
}

pub(crate) fn rs_remainder(data: &[u8], divisor: &[u8]) -> Vec<u8> {
    let mut result = vec![0u8; divisor.len()];
    for &b in data {
        let factor = b ^ result.remove(0);
        result.push(0);
        for (x, &c) in result.iter_mut().zip(divisor) {
            *x ^= gf_mul(c, factor);
        }
    }
    result
}

/// The data codewords of `text` for version `ver` (mode, length, bytes, terminator, padding).
fn data_words(text: &[u8], ver: usize, level: Level) -> Vec<u8> {
    let mut bits: Vec<bool> = vec![];
    let put = |v: u32, n: usize, bits: &mut Vec<bool>| {
        for i in (0..n).rev() {
            bits.push((v >> i) & 1 == 1);
        }
    };
    put(0b0100, 4, &mut bits);
    put(text.len() as u32, if ver >= 10 { 16 } else { 8 }, &mut bits);
    for &b in text {
        put(b as u32, 8, &mut bits);
    }
    let cap = data_codewords(ver, level) * 8;
    let term = (cap - bits.len()).min(4);
    put(0, term, &mut bits);
    while bits.len() % 8 != 0 {
        bits.push(false);
    }
    let mut out: Vec<u8> = bits.chunks(8).map(|c| c.iter().fold(0u8, |a, &b| (a << 1) | b as u8)).collect();
    let mut pad = 0xECu8;
    while out.len() < data_codewords(ver, level) {
        out.push(pad);
        pad = if pad == 0xEC { 0x11 } else { 0xEC };
    }
    out
}

/// Data and error correction codewords, interleaved block by block.
fn all_words(data: &[u8], ver: usize, level: Level) -> Vec<u8> {
    let (nb, ecl) = (BLOCK_TABLE[level as usize][ver], ECC_TABLE[level as usize][ver]);
    let raw = raw_modules(ver) / 8;
    let short = nb - raw % nb;
    let short_len = raw / nb;
    let div = rs_divisor(ecl);
    let mut blocks: Vec<Vec<u8>> = vec![];
    let mut k = 0;
    for i in 0..nb {
        let len = short_len - ecl + usize::from(i >= short);
        let d = &data[k..k + len];
        k += len;
        let mut b = d.to_vec();
        if i < short {
            b.push(0); // placeholder so the data columns line up
        }
        b.extend(rs_remainder(d, &div));
        blocks.push(b);
    }
    let mut out = vec![];
    for i in 0..blocks[0].len() {
        for (j, b) in blocks.iter().enumerate() {
            if i != short_len - ecl || j >= short {
                out.push(b[i]);
            }
        }
    }
    out
}

pub(crate) struct Grid {
    pub(crate) size: usize,
    pub(crate) m: Vec<bool>,
    pub(crate) f: Vec<bool>,
}

impl Grid {
    pub(crate) fn set_fn(&mut self, x: usize, y: usize, dark: bool) {
        self.m[y * self.size + x] = dark;
        self.f[y * self.size + x] = true;
    }

    pub(crate) fn finder(&mut self, cx: i32, cy: i32) {
        for dy in -4..=4i32 {
            for dx in -4..=4i32 {
                let (x, y) = (cx + dx, cy + dy);
                if x >= 0 && y >= 0 && (x as usize) < self.size && (y as usize) < self.size {
                    let d = dx.abs().max(dy.abs());
                    self.set_fn(x as usize, y as usize, d != 2 && d != 4);
                }
            }
        }
    }

    pub(crate) fn alignment(&mut self, cx: usize, cy: usize) {
        for dy in -2..=2i32 {
            for dx in -2..=2i32 {
                self.set_fn((cx as i32 + dx) as usize, (cy as i32 + dy) as usize, dx.abs().max(dy.abs()) != 1);
            }
        }
    }

    /// The format bits for a mask and a level (L = 1, M = 0, Q = 3, H = 2, as the standard writes them).
    pub(crate) fn format_level(&mut self, mask: usize, level: usize) {
        let data = (level << 3) | mask;
        let mut rem = data;
        for _ in 0..10 {
            rem = (rem << 1) ^ ((rem >> 9) * 0x537);
        }
        let bits = ((data << 10) | rem) ^ 0x5412;
        let bit = |i: usize| (bits >> i) & 1 == 1;
        let s = self.size;
        for i in 0..=5 {
            self.set_fn(8, i, bit(i));
        }
        self.set_fn(8, 7, bit(6));
        self.set_fn(8, 8, bit(7));
        self.set_fn(7, 8, bit(8));
        for i in 9..15 {
            self.set_fn(14 - i, 8, bit(i));
        }
        for i in 0..8 {
            self.set_fn(s - 1 - i, 8, bit(i));
        }
        for i in 8..15 {
            self.set_fn(8, s - 15 + i, bit(i));
        }
        self.set_fn(8, s - 8, true);
    }

    pub(crate) fn version_info(&mut self, ver: usize) {
        if ver < 7 {
            return;
        }
        let mut rem = ver;
        for _ in 0..12 {
            rem = (rem << 1) ^ ((rem >> 11) * 0x1F25);
        }
        let bits = (ver << 12) | rem;
        for i in 0..18 {
            let b = (bits >> i) & 1 == 1;
            let (a, c) = (self.size - 11 + i % 3, i / 3);
            self.set_fn(a, c, b);
            self.set_fn(c, a, b);
        }
    }
}

pub(crate) fn align_positions(ver: usize) -> Vec<usize> {
    if ver == 1 {
        return vec![];
    }
    let n = ver / 7 + 2;
    let step = if ver == 32 { 26 } else { (ver * 4 + n * 2 + 1) / (n * 2 - 2) * 2 };
    let mut v = vec![6];
    let mut pos = ver * 4 + 10;
    for _ in 1..n {
        v.insert(1, pos);
        pos = pos.saturating_sub(step); // the last step is not used
    }
    v
}

pub(crate) fn masked(mask: usize, x: usize, y: usize) -> bool {
    match mask {
        0 => (x + y) % 2 == 0,
        1 => y % 2 == 0,
        2 => x % 3 == 0,
        3 => (x + y) % 3 == 0,
        4 => (x / 3 + y / 2) % 2 == 0,
        5 => x * y % 2 + x * y % 3 == 0,
        6 => (x * y % 2 + x * y % 3) % 2 == 0,
        _ => ((x + y) % 2 + x * y % 3) % 2 == 0,
    }
}

fn penalty(g: &Grid) -> i32 {
    let s = g.size;
    let at = |x: usize, y: usize| g.m[y * s + x];
    let mut p = 0i32;
    for horizontal in [true, false] {
        for a in 0..s {
            let line: Vec<bool> = (0..s).map(|b| if horizontal { at(b, a) } else { at(a, b) }).collect();
            let mut run = 1;
            for i in 1..=s {
                if i < s && line[i] == line[i - 1] {
                    run += 1;
                } else {
                    if run >= 5 {
                        p += 3 + (run - 5) as i32;
                    }
                    run = 1;
                }
            }
            // 1:1:3:1:1 with four light modules on either side
            let pat = [true, false, true, true, true, false, true];
            for i in 0..s.saturating_sub(6) {
                if line[i..i + 7] == pat {
                    let before = i >= 4 && line[i - 4..i].iter().all(|&b| !b);
                    let after = i + 11 <= s && line[i + 7..i + 11].iter().all(|&b| !b);
                    if before || after {
                        p += 40;
                    }
                }
            }
        }
    }
    for y in 0..s - 1 {
        for x in 0..s - 1 {
            let c = at(x, y);
            if c == at(x + 1, y) && c == at(x, y + 1) && c == at(x + 1, y + 1) {
                p += 3;
            }
        }
    }
    let dark = g.m.iter().filter(|&&b| b).count() as i32;
    let total = (s * s) as i32;
    let k = ((dark * 20 - total * 10).abs() + total - 1) / total - 1;
    p + k.max(0) * 10
}

/// How a Wi-Fi network is secured, as the Wi-Fi QR code names it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WifiSecurity {
    Wpa,
    Wep,
    Open,
}

/// The text phones read from a QR code to join a Wi-Fi network (WPA): `WIFI:T:WPA;S:<name>;P:<password>;;`, with `\ ; , : "` escaped by a backslash.
pub fn wifi_payload(ssid: &str, pass: &str) -> String {
    wifi_payload_with(ssid, pass, WifiSecurity::Wpa, false)
}

/// The Wi-Fi payload for any security (an open network has no password part) and, when `hidden`, `H:true;` for a network that does not announce its name.
pub fn wifi_payload_with(ssid: &str, pass: &str, security: WifiSecurity, hidden: bool) -> String {
    fn esc(s: &str) -> String {
        let mut o = String::with_capacity(s.len() + 4);
        for c in s.chars() {
            if matches!(c, '\\' | ';' | ',' | ':' | '"') {
                o.push('\\');
            }
            o.push(c);
        }
        o
    }
    let (kind, pw) = match security {
        WifiSecurity::Wpa => ("WPA", format!("P:{};", esc(pass))),
        WifiSecurity::Wep => ("WEP", format!("P:{};", esc(pass))),
        WifiSecurity::Open => ("nopass", String::new()),
    };
    format!("WIFI:T:{kind};S:{};{pw}{}", esc(ssid), if hidden { "H:true;;" } else { ";" })
}

/// Encode `text` (any bytes) as a level L QR symbol of version 1 to 20; `None` when it does not fit in [`MAX_BYTES`].
pub fn encode(text: &[u8]) -> Option<Qr> {
    encode_upto(text, Level::L, 20)
}

/// Encode `text` (any bytes) at an error-correction level, version 1 to [`MAX_VERSION`]; `None` when it does not fit
/// (see [`capacity`]).
pub fn encode_level(text: &[u8], level: Level) -> Option<Qr> {
    encode_upto(text, level, MAX_VERSION)
}

fn encode_upto(text: &[u8], level: Level, max_ver: usize) -> Option<Qr> {
    let ver = (1..=max_ver).find(|&v| text.len() <= capacity(v, level))?;
    let words = all_words(&data_words(text, ver, level), ver, level);
    let size = ver * 4 + 17;
    let mut g = Grid { size, m: vec![false; size * size], f: vec![false; size * size] };
    for i in 0..size {
        g.set_fn(6, i, i % 2 == 0);
        g.set_fn(i, 6, i % 2 == 0);
    }
    g.finder(3, 3);
    g.finder(size as i32 - 4, 3);
    g.finder(3, size as i32 - 4);
    let pos = align_positions(ver);
    let last = pos.len().saturating_sub(1);
    for (i, &cx) in pos.iter().enumerate() {
        for (j, &cy) in pos.iter().enumerate() {
            if !((i == 0 && j == 0) || (i == 0 && j == last) || (i == last && j == 0)) {
                g.alignment(cx, cy);
            }
        }
    }
    g.format_level(0, level.format_bits());
    g.version_info(ver);
    // the codewords, in the zigzag of the standard
    let mut i = 0usize;
    let mut right = size as i32 - 1;
    while right >= 1 {
        if right == 6 {
            right = 5;
        }
        for vert in 0..size {
            for j in 0..2 {
                let x = (right - j) as usize;
                let upward = ((right + 1) & 2) == 0;
                let y = if upward { size - 1 - vert } else { vert };
                if !g.f[y * size + x] && i < words.len() * 8 {
                    g.m[y * size + x] = (words[i >> 3] >> (7 - (i & 7))) & 1 == 1;
                    i += 1;
                }
            }
        }
        right -= 2;
    }
    let mut best: Option<(i32, Grid)> = None;
    for mask in 0..8 {
        let mut t = Grid { size, m: g.m.clone(), f: g.f.clone() };
        for y in 0..size {
            for x in 0..size {
                if !t.f[y * size + x] && masked(mask, x, y) {
                    t.m[y * size + x] = !t.m[y * size + x];
                }
            }
        }
        t.format_level(mask, level.format_bits());
        let p = penalty(&t);
        if best.as_ref().is_none_or(|(bp, _)| p < *bp) {
            best = Some((p, t));
        }
    }
    best.map(|(_, t)| Qr { size, modules: t.m })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Read the symbol back the way a scanner does: format bits, unmask, codewords in zigzag order, de-interleave, check every
    /// block's Reed-Solomon remainder, and return the bytes.
    fn decode(q: &Qr) -> Result<Vec<u8>, String> {
        decode_level(q).map(|(_, b)| b)
    }

    /// Like [`decode`], and also the level the format bits name.
    fn decode_level(q: &Qr) -> Result<(Level, Vec<u8>), String> {
        let s = q.size;
        let ver = q.version();
        // format information, first copy
        let mut bits = 0usize;
        for i in 0..=5 {
            bits |= usize::from(q.get(8, i)) << i;
        }
        bits |= usize::from(q.get(8, 7)) << 6 | usize::from(q.get(8, 8)) << 7 | usize::from(q.get(7, 8)) << 8;
        for i in 9..15 {
            bits |= usize::from(q.get(14 - i, 8)) << i;
        }
        let bits = bits ^ 0x5412;
        let mut rem = bits >> 10;
        for _ in 0..10 {
            rem = (rem << 1) ^ ((rem >> 9) * 0x537);
        }
        if (bits & 0x3FF) != (rem & 0x3FF) {
            return Err("format bits do not check".into());
        }
        let lv = Level::ALL.into_iter().find(|l| l.format_bits() == (bits >> 13) & 3).ok_or("no such level")?;
        let mask = (bits >> 10) & 7;
        // which modules are function modules: draw an empty symbol of the same version
        let mut g = Grid { size: s, m: vec![false; s * s], f: vec![false; s * s] };
        for i in 0..s {
            g.set_fn(6, i, false);
            g.set_fn(i, 6, false);
        }
        g.finder(3, 3);
        g.finder(s as i32 - 4, 3);
        g.finder(3, s as i32 - 4);
        let pos = align_positions(ver);
        let last = pos.len().saturating_sub(1);
        for (i, &cx) in pos.iter().enumerate() {
            for (j, &cy) in pos.iter().enumerate() {
                if !((i == 0 && j == 0) || (i == 0 && j == last) || (i == last && j == 0)) {
                    g.alignment(cx, cy);
                }
            }
        }
        g.format_level(0, lv.format_bits());
        g.version_info(ver);
        let mut words = vec![0u8; raw_modules(ver) / 8];
        let mut i = 0usize;
        let mut right = s as i32 - 1;
        while right >= 1 {
            if right == 6 {
                right = 5;
            }
            for vert in 0..s {
                for j in 0..2 {
                    let x = (right - j) as usize;
                    let y = if ((right + 1) & 2) == 0 { s - 1 - vert } else { vert };
                    if !g.f[y * s + x] && i < words.len() * 8 {
                        let bit = q.get(x, y) ^ masked(mask, x, y);
                        words[i >> 3] |= u8::from(bit) << (7 - (i & 7));
                        i += 1;
                    }
                }
            }
            right -= 2;
        }
        // de-interleave
        let (nb, ecl) = (BLOCK_TABLE[lv as usize][ver], ECC_TABLE[lv as usize][ver]);
        let short = nb - words.len() % nb;
        let short_len = words.len() / nb;
        let mut blocks: Vec<Vec<u8>> = vec![vec![]; nb];
        let mut k = 0;
        for col in 0..short_len + 1 {
            for (b, blk) in blocks.iter_mut().enumerate() {
                if col == short_len - ecl && b < short {
                    continue; // the short blocks have no data byte here
                }
                blk.push(words[k]);
                k += 1;
            }
        }
        let div = rs_divisor(ecl);
        let mut data = vec![];
        for blk in &blocks {
            let (d, e) = blk.split_at(blk.len() - ecl);
            if rs_remainder(d, &div) != e {
                return Err("a Reed-Solomon block does not check".into());
            }
            data.extend_from_slice(d);
        }
        if data[0] >> 4 != 0b0100 {
            return Err("not byte mode".into());
        }
        let (n, start) = if ver >= 10 { ((((data[0] & 15) as usize) << 12) | (data[1] as usize) << 4 | (data[2] >> 4) as usize, 2) } else { ((((data[0] & 15) as usize) << 4) | (data[1] >> 4) as usize, 1) };
        // the bytes follow the 4 bits of the mode and the count, nibble-shifted
        let mut out = vec![];
        for j in 0..n {
            out.push((data[start + j] << 4) | (data[start + j + 1] >> 4));
        }
        Ok((lv, out))
    }

    #[test]
    fn wifi_payload_escapes_the_special_characters() {
        assert_eq!(wifi_payload("iyakki-ab12", "pass word"), "WIFI:T:WPA;S:iyakki-ab12;P:pass word;;");
        assert_eq!(wifi_payload("a;b,c:d\\e\"f", "x;y"), "WIFI:T:WPA;S:a\\;b\\,c\\:d\\\\e\\\"f;P:x\\;y;;");
        assert!(encode(wifi_payload(&"s".repeat(32), &"p".repeat(63)).as_bytes()).is_some(), "the longest name and password fit");
    }

    #[test]
    fn short_text_is_version_one_and_reads_back() {
        let q = encode(b"HELLO").unwrap();
        assert_eq!((q.version(), q.size), (1, 21));
        assert_eq!(decode(&q).unwrap(), b"HELLO");
    }

    #[test]
    fn finder_timing_and_dark_module_are_in_place() {
        let q = encode(b"https://github.com/login/device").unwrap();
        let s = q.size;
        for (cx, cy) in [(0, 0), (s - 7, 0), (0, s - 7)] {
            assert!(q.get(cx, cy) && q.get(cx + 6, cy + 6) && !q.get(cx + 1, cy + 1) && q.get(cx + 3, cy + 3));
        }
        assert!(q.get(8, s - 8), "the dark module");
        assert!((8..s - 8).all(|i| q.get(i, 6) == (i % 2 == 0)));
    }

    #[test]
    fn every_size_boundary_round_trips() {
        for n in [1usize, 17, 18, 32, 53, 78, 106, 134, 154, 192, 230, 271, 321, 367, 425, 458, 520, 586, 644, 718, 792, 858] {
            let text: Vec<u8> = (0..n).map(|i| b"abcXYZ0123/:?=&-_.~"[(i * 7 + n) % 19]).collect();
            let q = encode(&text).unwrap_or_else(|| panic!("{n} bytes do not fit"));
            assert_eq!(decode(&q).unwrap(), text, "{n} bytes, version {}", q.version());
        }
        assert!(encode(&vec![b'a'; MAX_BYTES + 1]).is_none());
    }

    #[test]
    fn capacities_match_the_standard() {
        // byte capacity at level L for versions 1..20 (ISO/IEC 18004 table 7)
        let want = [17, 32, 53, 78, 106, 134, 154, 192, 230, 271, 321, 367, 425, 458, 520, 586, 644, 718, 792, 858];
        for (v, w) in want.iter().enumerate() {
            let cc = if v + 1 >= 10 { 2 } else { 1 };
            let cap = (data_codewords(v + 1, Level::L) * 8 - 4 - cc * 8) / 8;
            assert_eq!(cap, *w, "version {}", v + 1);
        }
    }

    #[test]
    fn a_corrupted_module_is_noticed_by_the_check() {
        let mut q = encode(b"https://example.test/a").unwrap();
        let i = q.size * 10 + 10;
        q.modules[i] = !q.modules[i];
        assert!(decode(&q).is_err());
    }

    #[test]
    fn wifi_payload_has_a_form_for_every_security_and_for_hidden_networks() {
        assert_eq!(wifi_payload_with("home", "pw", WifiSecurity::Wep, false), "WIFI:T:WEP;S:home;P:pw;;");
        assert_eq!(wifi_payload_with("cafe", "", WifiSecurity::Open, false), "WIFI:T:nopass;S:cafe;;");
        assert_eq!(wifi_payload_with("a;b", "x:y", WifiSecurity::Wpa, true), "WIFI:T:WPA;S:a\\;b;P:x\\:y;H:true;;");
        assert_eq!(wifi_payload_with("c", "", WifiSecurity::Open, true), "WIFI:T:nopass;S:c;H:true;;");
        assert_eq!(wifi_payload("n", "p"), wifi_payload_with("n", "p", WifiSecurity::Wpa, false));
    }

    #[test]
    fn reed_solomon_matches_the_worked_example_of_the_standard_guides() {
        // 1-M "HELLO WORLD" style example: 16 data codewords, 10 check codewords
        let data = [32u8, 91, 11, 120, 209, 114, 220, 77, 67, 64, 236, 17, 236, 17, 236, 17];
        assert_eq!(rs_remainder(&data, &rs_divisor(10)), [196u8, 35, 39, 119, 235, 215, 231, 226, 93, 23]);
    }

    #[test]
    fn format_information_matches_the_table_of_the_standard() {
        // mask 0 at each level (ISO/IEC 18004 table C.1), first copy read as a scanner does
        for (level, want) in [(Level::L, 0b111011111000100usize), (Level::M, 0b101010000010010), (Level::Q, 0b011010101011111), (Level::H, 0b001011010001001)] {
            let mut g = Grid { size: 21, m: vec![false; 441], f: vec![false; 441] };
            g.format_level(0, level.format_bits());
            let at = |x: usize, y: usize| usize::from(g.m[y * 21 + x]);
            let mut bits = 0usize;
            for i in 0..=5 {
                bits |= at(8, i) << i;
            }
            bits |= at(8, 7) << 6 | at(8, 8) << 7 | at(7, 8) << 8;
            for i in 9..15 {
                bits |= at(14 - i, 8) << i;
            }
            assert_eq!(bits, want, "level {level:?}");
        }
    }

    #[test]
    fn the_tables_give_the_capacities_of_the_standard_at_every_level() {
        // byte-mode capacity (ISO/IEC 18004 table 7) at a spread of versions
        let want = [
            (1, [17, 14, 11, 7]),
            (5, [106, 84, 60, 44]),
            (10, [271, 213, 151, 119]),
            (20, [858, 666, 482, 382]),
            (27, [1465, 1125, 805, 625]),
            (30, [1732, 1370, 982, 742]),
            (40, [2953, 2331, 1663, 1273]),
        ];
        for (v, caps) in want {
            for (l, c) in Level::ALL.into_iter().zip(caps) {
                assert_eq!(capacity(v, l), c, "version {v} level {l:?}");
            }
        }
        // the data and check codewords add up to the symbol's whole capacity at every version and level
        for v in 1..=MAX_VERSION {
            for l in Level::ALL {
                let (e, b) = (ECC_TABLE[l as usize][v], BLOCK_TABLE[l as usize][v]);
                assert!(data_codewords(v, l) > 0 && e * b + data_codewords(v, l) == raw_modules(v) / 8, "version {v} level {l:?}");
            }
        }
        assert_eq!(align_positions(7), [6, 22, 38]);
        assert_eq!(align_positions(32), [6, 34, 60, 86, 112, 138]);
        assert_eq!(align_positions(36), [6, 24, 50, 76, 102, 128, 154]);
        assert_eq!(align_positions(40), [6, 30, 58, 86, 114, 142, 170]);
    }

    #[test]
    fn every_level_round_trips_with_its_format_bits_and_check_codewords() {
        for l in Level::ALL {
            for n in [1usize, 14, 50, 120, 300, 700, 1200, 1663, capacity(40, l) / 2] {
                let text: Vec<u8> = (0..n).map(|i| (i * 31 + n) as u8).collect();
                let Some(q) = encode_level(&text, l) else { assert!(n > capacity(MAX_VERSION, l)); continue };
                let (got, bytes) = decode_level(&q).unwrap_or_else(|e| panic!("{n} bytes at {l:?}: {e}"));
                assert_eq!((got, bytes), (l, text), "{n} bytes, version {}", q.version());
                assert!(q.version() >= 1 && q.size == q.version() * 4 + 17);
            }
        }
    }

    #[test]
    fn the_biggest_symbols_fill_and_one_more_byte_does_not_fit() {
        for l in Level::ALL {
            let n = capacity(MAX_VERSION, l);
            let q = encode_level(&vec![0xA5; n], l).unwrap_or_else(|| panic!("{n} bytes at {l:?} do not fit"));
            assert_eq!((q.version(), q.size), (40, 177));
            assert_eq!(decode_level(&q).unwrap().1.len(), n);
            assert!(encode_level(&vec![0xA5; n + 1], l).is_none(), "{l:?}");
        }
        // a higher level never makes a smaller symbol; level L through encode_level is the same as encode
        let t = b"https://example.test/path?q=1";
        assert_eq!(encode_level(t, Level::L), encode(t));
        let sizes: Vec<usize> = Level::ALL.into_iter().map(|l| encode_level(&vec![b'z'; 100], l).unwrap().size).collect();
        assert!(sizes.windows(2).all(|w| w[0] <= w[1]), "{sizes:?}");
        assert!(encode(&vec![b'a'; MAX_BYTES + 1]).is_none(), "encode keeps its old limit");
    }

    /// Writes symbols as PBM files into the folder named by `QR_DUMP`, for an outside scanner (OpenCV, a phone) to read back.
    #[test]
    #[ignore]
    fn dump_for_an_outside_scanner() {
        let Ok(dir) = std::env::var("QR_DUMP") else { return };
        for (name, text) in [("short", "HELLO".to_string()), ("device", "https://github.com/login/device".to_string()), ("claude", format!("https://claude.ai/oauth/authorize?code=true&client_id=9d1c250a-e61b-44d9-88ed-5944d1962f5e&response_type=code&redirect_uri=https%3A%2F%2Fconsole.anthropic.com%2Foauth%2Fcode%2Fcallback&scope=org%3Acreate_api_key+user%3Aprofile+user%3Ainference&code_challenge={}&code_challenge_method=S256&state={}", "a".repeat(43), "b".repeat(43))), ("big", "x".repeat(800))] {
            let q = encode(text.as_bytes()).unwrap();
            let quiet = 4;
            let n = q.size + 2 * quiet;
            let mut out = format!("P1\n{n} {n}\n");
            for y in 0..n {
                for x in 0..n {
                    let (qx, qy) = (x as i32 - quiet as i32, y as i32 - quiet as i32);
                    let dark = qx >= 0 && qy >= 0 && q.get(qx as usize, qy as usize);
                    out.push(if dark { '1' } else { '0' });
                }
                out.push('\n');
            }
            std::fs::write(format!("{dir}/{name}.pbm"), out).unwrap();
        }
    }
}
