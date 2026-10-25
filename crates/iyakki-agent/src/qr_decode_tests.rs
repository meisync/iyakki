use super::*;
use crate::qr::Qr;

// ------------------------------------------------------------------------------------------------ helpers

struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }
    fn below(&mut self, n: usize) -> usize {
        (self.next() % n as u64) as usize
    }
}

enum Seg<'a> {
    Num(&'a str),
    Alnum(&'a str),
    Bytes(&'a [u8]),
    Eci(u32),
}

fn put(bits: &mut Vec<bool>, v: usize, n: usize) {
    for i in (0..n).rev() {
        bits.push((v >> i) & 1 == 1);
    }
}

/// The function patterns of a version, drawn as the encoder draws them (flags `f` mark the modules that are not data).
fn grid_for(ver: usize) -> Grid {
    let s = ver * 4 + 17;
    let mut g = Grid { size: s, m: vec![false; s * s], f: vec![false; s * s] };
    for i in 0..s {
        g.set_fn(6, i, i % 2 == 0);
        g.set_fn(i, 6, i % 2 == 0);
    }
    for (cx, cy) in [(3, 3), (s as i32 - 4, 3), (3, s as i32 - 4)] {
        g.finder(cx, cy);
    }
    let pos = qr::align_positions(ver);
    let last = pos.len().saturating_sub(1);
    for (i, &cx) in pos.iter().enumerate() {
        for (j, &cy) in pos.iter().enumerate() {
            if !((i == 0 && j == 0) || (i == 0 && j == last) || (i == last && j == 0)) {
                g.alignment(cx, cy);
            }
        }
    }
    g.format_level(0, 0);
    g.version_info(ver);
    g
}

/// A symbol made independently of the library's encoder: any version 1..=10, level (0 L, 1 M, 2 Q, 3 H), mask and mix of segments.
fn encode_at(segs: &[Seg], ver: usize, level: usize, mask: usize) -> Option<Qr> {
    let mut bits: Vec<bool> = vec![];
    let cc = |a: usize, b: usize| if ver <= 9 { a } else { b };
    for s in segs {
        match s {
            Seg::Num(t) => {
                put(&mut bits, 1, 4);
                put(&mut bits, t.len(), cc(10, 12));
                for ch in t.as_bytes().chunks(3) {
                    let v: usize = std::str::from_utf8(ch).unwrap().parse().unwrap();
                    put(&mut bits, v, [4, 7, 10][ch.len() - 1]);
                }
            }
            Seg::Alnum(t) => {
                put(&mut bits, 2, 4);
                put(&mut bits, t.len(), cc(9, 11));
                let idx = |c: u8| ALNUM.iter().position(|&a| a == c).unwrap();
                for ch in t.as_bytes().chunks(2) {
                    if ch.len() == 2 {
                        put(&mut bits, idx(ch[0]) * 45 + idx(ch[1]), 11);
                    } else {
                        put(&mut bits, idx(ch[0]), 6);
                    }
                }
            }
            Seg::Bytes(b) => {
                put(&mut bits, 4, 4);
                put(&mut bits, b.len(), cc(8, 16));
                for &x in *b {
                    put(&mut bits, x as usize, 8);
                }
            }
            Seg::Eci(n) => {
                put(&mut bits, 7, 4);
                put(&mut bits, *n as usize, 8);
            }
        }
    }
    let cap = data_codewords(ver, level);
    if bits.len() > cap * 8 {
        return None;
    }
    let term = (cap * 8 - bits.len()).min(4);
    put(&mut bits, 0, term);
    while bits.len() % 8 != 0 {
        bits.push(false);
    }
    let mut data: Vec<u8> = bits.chunks(8).map(|c| c.iter().fold(0u8, |a, &b| (a << 1) | b as u8)).collect();
    let mut pad = 0xECu8;
    while data.len() < cap {
        data.push(pad);
        pad = if pad == 0xEC { 0x11 } else { 0xEC };
    }
    // blocks, check codewords, interleaving
    let (nb, ecl) = (BLOCKS[level][ver - 1], ECC_PER_BLOCK[level][ver - 1]);
    let total = qr::raw_modules(ver) / 8;
    let (short, short_len) = (nb - total % nb, total / nb);
    let div = qr::rs_divisor(ecl);
    let (mut dblocks, mut eblocks) = (vec![], vec![]);
    let mut k = 0;
    for b in 0..nb {
        let len = short_len - ecl + usize::from(b >= short);
        let d = data[k..k + len].to_vec();
        k += len;
        eblocks.push(qr::rs_remainder(&d, &div));
        dblocks.push(d);
    }
    let mut words = vec![];
    for i in 0..short_len - ecl + 1 {
        for d in &dblocks {
            if i < d.len() {
                words.push(d[i]);
            }
        }
    }
    for i in 0..ecl {
        for e in &eblocks {
            words.push(e[i]);
        }
    }
    assert_eq!(words.len(), total);
    let mut g = grid_for(ver);
    let s = g.size;
    let (mut i, mut right) = (0usize, s as i32 - 1);
    while right >= 1 {
        if right == 6 {
            right = 5;
        }
        for vert in 0..s {
            for j in 0..2 {
                let x = (right - j) as usize;
                let y = if ((right + 1) & 2) == 0 { s - 1 - vert } else { vert };
                if !g.f[y * s + x] && i < total * 8 {
                    g.m[y * s + x] = (((words[i >> 3] >> (7 - (i & 7))) & 1) == 1) ^ qr::masked(mask, x, y);
                    i += 1;
                }
            }
        }
        right -= 2;
    }
    g.format_level(mask, [1, 0, 3, 2][level]);
    Some(Qr::from_modules(s, g.m))
}

struct Look {
    scale: f64,
    angle: f64,
    mirror: bool,
    noise: u32,
    gradient: bool,
    /// Perspective: the symbol looks about this much wider per symbol-height towards the bottom (0.1 = 10 %).
    keystone: f64,
}

impl Default for Look {
    fn default() -> Look {
        Look { scale: 4.0, angle: 0.0, mirror: false, noise: 0, gradient: false, keystone: 0.0 }
    }
}

/// The symbol drawn as a camera would see it: dark modules on a white card with a 4-module quiet zone on a mid-grey table.
fn render(q: &Qr, look: &Look, seed: u64) -> (usize, usize, Vec<u8>) {
    let total = (q.size + 8) as f64;
    let side = total * look.scale;
    let (w, h) = ((side * 1.5) as usize + 60, (side * 1.5) as usize + 60);
    let (cx, cy) = (w as f64 / 2.0, h as f64 / 2.0);
    let (sin, cos) = look.angle.to_radians().sin_cos();
    let mut rng = Rng(seed | 1);
    let mut out = vec![0u8; w * h];
    for y in 0..h {
        for x in 0..w {
            let mut acc = 0.0;
            for (ox, oy) in [(0.25, 0.25), (0.75, 0.25), (0.25, 0.75), (0.75, 0.75)] {
                let (dx, dy) = (x as f64 + ox - cx, y as f64 + oy - cy);
                // a true perspective: the divisor grows down the picture, so the symbol is wider at the bottom
                let k = 1.0 + look.keystone * (dy / side);
                let (dx, dy) = (dx / k, dy / k);
                let (rx, ry) = (dx * cos + dy * sin, -dx * sin + dy * cos);
                let (mut u, v) = (rx / look.scale + total / 2.0, ry / look.scale + total / 2.0);
                if look.mirror {
                    u = total - u;
                }
                acc += if u < 0.0 || v < 0.0 || u >= total || v >= total {
                    120.0
                } else {
                    let (mx, my) = (u as i64 - 4, v as i64 - 4);
                    if mx >= 0 && my >= 0 && q.get(mx as usize, my as usize) {
                        35.0
                    } else {
                        235.0
                    }
                };
            }
            let mut v = acc / 4.0;
            if look.gradient {
                v *= 0.4 + 0.6 * x as f64 / w as f64;
            }
            if look.noise > 0 {
                v += rng.below(2 * look.noise as usize + 1) as f64 - look.noise as f64;
            }
            out[y * w + x] = v.clamp(0.0, 255.0) as u8;
        }
    }
    (w, h, out)
}

fn read(w: usize, h: usize, img: &[u8]) -> Option<Vec<u8>> {
    // a long budget: a slow debug build must not cut the search short
    decode_with(w, h, img, Duration::from_secs(10))
}

fn text(n: usize, seed: u64) -> Vec<u8> {
    let mut r = Rng(seed | 1);
    (0..n).map(|_| b"abcdefXYZ0123456789/:?=&-_.~#"[r.below(29)]).collect()
}

fn round_trip(q: &Qr, look: &Look, want: &[u8], what: &str) {
    let (w, h, img) = render(q, look, 7);
    assert_eq!(read(w, h, &img).as_deref(), Some(want), "{what}");
}

// ------------------------------------------------------------------------------------------------ tables and pieces

#[test]
fn tables_match_the_standard_and_the_encoder() {
    // total data codewords per version at L, M, Q and H (ISO/IEC 18004 table 7)
    let want = [
        [19, 34, 55, 80, 108, 136, 156, 194, 232, 274],
        [16, 28, 44, 64, 86, 108, 124, 154, 182, 216],
        [13, 22, 34, 48, 62, 76, 88, 110, 132, 154],
        [9, 16, 26, 36, 46, 60, 66, 86, 100, 122],
    ];
    for (level, row) in want.iter().enumerate() {
        for (v, &d) in row.iter().enumerate() {
            assert_eq!(data_codewords(v + 1, level), d, "level {level} version {}", v + 1);
        }
    }
    for v in 1..=MAX_VERSION {
        assert_eq!((ECC_PER_BLOCK[0][v - 1], BLOCKS[0][v - 1]), (qr::ECC_PER_BLOCK[v], qr::BLOCKS[v]), "version {v} level L agrees with the encoder");
    }
}

#[test]
fn format_words_follow_the_standard_and_survive_three_bit_errors() {
    assert_eq!(format_bits(0b01000), 0x77C4, "level L, mask 0");
    assert_eq!(format_bits(0b00000), 0x5412, "level M, mask 0");
    assert_eq!(format_bits(0b11000), 0x355F, "level Q, mask 0");
    assert_eq!(format_bits(0b10000), 0x1689, "level H, mask 0");
    for data in 0..32usize {
        let want_level = match data >> 3 {
            1 => 0,
            0 => 1,
            3 => 2,
            _ => 3,
        };
        let w = format_bits(data);
        assert_eq!(match_format(w), Some((want_level, data & 7, 0)));
        // any three wrong bits still point at the same word
        assert_eq!(match_format(w ^ 0b100_0100_0000_0001).map(|m| (m.0, m.1)), Some((want_level, data & 7)));
        // four wrong bits are refused (the words are at least eight apart)
        assert_eq!(match_format(w ^ 0b100_0100_0100_0001), None);
    }
}

#[test]
fn reed_solomon_repairs_up_to_half_the_check_words_and_no_more() {
    let mut rng = Rng(99);
    for nsym in [7usize, 10, 17, 28] {
        for _ in 0..20 {
            let data: Vec<u8> = (0..40).map(|_| rng.below(256) as u8).collect();
            let mut block = data.clone();
            block.extend(qr::rs_remainder(&data, &qr::rs_divisor(nsym)));
            let clean = block.clone();
            let mut hit = vec![];
            while hit.len() < nsym / 2 {
                let p = rng.below(block.len());
                if !hit.contains(&p) {
                    hit.push(p);
                    block[p] ^= 1 + rng.below(255) as u8;
                }
            }
            assert!(rs_correct(&mut block, nsym));
            assert_eq!(block, clean, "nsym {nsym}");
        }
        // one more error than it can mend: refused (or, rarely, mended to a different word that still checks: never the original)
        let data: Vec<u8> = (0..40).map(|i| i as u8 * 3).collect();
        let mut block = data.clone();
        block.extend(qr::rs_remainder(&data, &qr::rs_divisor(nsym)));
        let clean = block.clone();
        for p in 0..nsym / 2 + 2 {
            block[p * 2] ^= 0x55;
        }
        let ok = rs_correct(&mut block, nsym);
        assert!(!ok || block != clean, "too many errors cannot give the original back by luck");
    }
}

#[test]
fn segments_of_every_kind_read_back() {
    let q = encode_at(&[Seg::Num("0123456789012345")], 1, 0, 3).unwrap();
    round_trip(&q, &Look::default(), b"0123456789012345", "numeric");
    let q = encode_at(&[Seg::Alnum("HELLO WORLD $%*+-./:")], 2, 1, 5).unwrap();
    round_trip(&q, &Look::default(), b"HELLO WORLD $%*+-./:", "alphanumeric, odd and even groups");
    let q = encode_at(&[Seg::Alnum("ABC")], 1, 2, 0).unwrap();
    round_trip(&q, &Look::default(), b"ABC", "alphanumeric with a single left over");
    let q = encode_at(&[Seg::Num("7"), Seg::Alnum("A1"), Seg::Bytes(b"-x_y#z")], 3, 3, 6).unwrap();
    round_trip(&q, &Look::default(), b"7A1-x_y#z", "three segments");
    let q = encode_at(&[Seg::Eci(26), Seg::Bytes("caf\u{e9} \u{2713}".as_bytes())], 2, 0, 1).unwrap();
    round_trip(&q, &Look::default(), "caf\u{e9} \u{2713}".as_bytes(), "ECI marker skipped, UTF-8 bytes kept");
    // the longer count field of version 10
    let t = text(200, 5);
    let q = encode_at(&[Seg::Bytes(&t)], 10, 0, 2).unwrap();
    round_trip(&q, &Look::default(), &t, "version 10 byte count");
    let q = encode_at(&[Seg::Num(&"9".repeat(300))], 10, 1, 4).unwrap();
    round_trip(&q, &Look::default(), "9".repeat(300).as_bytes(), "version 10 numeric count");
}

#[test]
fn unknown_modes_are_refused_not_guessed() {
    // a Kanji segment (mode 8) and FNC1 (mode 5)
    assert_eq!(parse_segments(&[0x80, 0, 0, 0], 1), None);
    assert_eq!(parse_segments(&[0x50, 0, 0, 0], 1), None);
    // a numeric group that is not a number, a count that runs past the data
    assert_eq!(parse_segments(&[0x10, 0x3F, 0xFF, 0xF0], 1), None);
    assert_eq!(parse_segments(&[0x4F, 0xF0], 1), None);
    assert_eq!(parse_segments(&[0x40, 0x00], 1), Some(vec![]));
}

// ------------------------------------------------------------------------------------------------ pictures

#[test]
fn every_version_and_level_with_every_mask_reads_back() {
    for ver in 1..=MAX_VERSION {
        for level in 0..4 {
            let n = data_codewords(ver, level) * 6 / 10;
            let t = text(n.max(1), (ver * 10 + level) as u64);
            let mask = (ver + level * 3) % 8;
            let q = encode_at(&[Seg::Bytes(&t)], ver, level, mask).unwrap_or_else(|| panic!("{n} bytes in version {ver} level {level}"));
            round_trip(&q, &Look { scale: 3.0, ..Look::default() }, &t, &format!("version {ver} level {level} mask {mask}"));
        }
    }
    for mask in 0..8 {
        let q = encode_at(&[Seg::Bytes(b"https://example.test/login/abc")], 3, 1, mask).unwrap();
        round_trip(&q, &Look::default(), b"https://example.test/login/abc", &format!("mask {mask}"));
    }
}

#[test]
fn our_own_encoder_round_trips_at_several_scales() {
    for n in [5usize, 40, 100, 180, 271] {
        let t = text(n, n as u64);
        let q = qr::encode(&t).unwrap();
        for scale in [3.0, 5.0, 9.0] {
            if q.version() >= 8 && scale > 5.0 {
                continue;
            }
            round_trip(&q, &Look { scale, ..Look::default() }, &t, &format!("{n} bytes, version {}, {scale} px a module", q.version()));
        }
    }
    let q = qr::encode(b"HELLO").unwrap();
    round_trip(&q, &Look { scale: 6.5, ..Look::default() }, b"HELLO", "a fractional scale");
}

#[test]
fn rotation_by_quarter_turns_and_by_other_angles() {
    let t = text(70, 3);
    for ver_text in [&t[..10], &t[..]] {
        let q = qr::encode(ver_text).unwrap();
        for angle in [90.0, 180.0, 270.0, 5.0, -8.0, 15.0, 30.0, 45.0, 100.0, 200.0] {
            round_trip(&q, &Look { angle, scale: 4.0, ..Look::default() }, ver_text, &format!("version {} turned {angle} degrees", q.version()));
        }
    }
}

#[test]
fn a_mirrored_picture_is_read_too() {
    let t = text(60, 11);
    let q = qr::encode(&t).unwrap();
    round_trip(&q, &Look { mirror: true, ..Look::default() }, &t, "mirrored");
    round_trip(&q, &Look { mirror: true, angle: 90.0, ..Look::default() }, &t, "mirrored and turned");
    round_trip(&q, &Look { mirror: true, angle: 12.0, scale: 3.0, ..Look::default() }, &t, "mirrored and slightly turned");
}

#[test]
fn noise_and_uneven_light_are_survived() {
    let t = text(90, 21);
    let q = qr::encode(&t).unwrap();
    round_trip(&q, &Look { noise: 25, ..Look::default() }, &t, "noise");
    round_trip(&q, &Look { gradient: true, ..Look::default() }, &t, "gradient light");
    round_trip(&q, &Look { gradient: true, noise: 20, angle: 10.0, scale: 5.0, ..Look::default() }, &t, "gradient, noise and a turn");
    round_trip(&q, &Look { gradient: true, noise: 15, mirror: true, scale: 4.0, ..Look::default() }, &t, "gradient, noise, mirrored");
}

#[test]
fn a_tilted_symbol_is_read_by_way_of_the_alignment_pattern() {
    let t = text(120, 31);
    let q = qr::encode(&t).unwrap();
    assert!(q.version() >= 4);
    round_trip(&q, &Look { keystone: 0.10, scale: 5.0, ..Look::default() }, &t, "keystone 10 %");
    round_trip(&q, &Look { keystone: -0.12, scale: 5.0, angle: 8.0, ..Look::default() }, &t, "keystone and a turn");
}

#[test]
fn damaged_modules_are_repaired_up_to_the_limit_and_refused_beyond() {
    let mut rng = Rng(1234);
    for (ver, level) in [(1, 0), (2, 1), (3, 2), (5, 3), (7, 1), (10, 3), (10, 0)] {
        let t = text(data_codewords(ver, level) / 2, ver as u64);
        let q = encode_at(&[Seg::Bytes(&t)], ver, level, 2).unwrap();
        let g = grid_for(ver);
        let data_modules: Vec<usize> = (0..q.size * q.size).filter(|&i| !g.f[i]).collect();
        // each flipped module spoils one codeword, so this many are always mendable
        let k = ECC_PER_BLOCK[level][ver - 1] / 2;
        let mut flips = vec![];
        while flips.len() < k {
            let i = data_modules[rng.below(data_modules.len())];
            if !flips.contains(&i) {
                flips.push(i);
            }
        }
        let mut modules: Vec<bool> = (0..q.size * q.size).map(|i| q.get(i % q.size, i / q.size)).collect();
        for &i in &flips {
            modules[i] = !modules[i];
        }
        let damaged = Qr::from_modules(q.size, modules.clone());
        round_trip(&damaged, &Look { scale: 5.0, ..Look::default() }, &t, &format!("version {ver} level {level}, {k} damaged modules"));
        // a third of the data modules wrong: no answer at all
        let mut bad = modules;
        for &i in data_modules.iter().filter(|_| rng.below(3) == 0) {
            bad[i] = !bad[i];
        }
        let (w, h, img) = render(&Qr::from_modules(q.size, bad), &Look { scale: 5.0, ..Look::default() }, 3);
        assert_eq!(read(w, h, &img), None, "version {ver} level {level} too damaged");
    }
}

#[test]
fn rgb_and_rgba_pictures_and_the_time_budget() {
    let t = text(50, 41);
    let q = qr::encode(&t).unwrap();
    let (w, h, gray) = render(&q, &Look::default(), 5);
    let rgb: Vec<u8> = gray.iter().flat_map(|&g| [g, g, g]).collect();
    let rgba: Vec<u8> = gray.iter().flat_map(|&g| [g, g, g, 255]).collect();
    assert_eq!(decode_rgb(w, h, &rgb).as_deref(), Some(&t[..]));
    assert_eq!(decode_rgba(w, h, &rgba).as_deref(), Some(&t[..]));
    assert_eq!(decode_gray(w, h, &gray).as_deref(), Some(&t[..]));
    // a zero budget still tries the first candidate, so a clean picture reads
    assert_eq!(decode_with(w, h, &gray, Duration::ZERO).as_deref(), Some(&t[..]));
    assert_eq!(default_budget(640, 480), Duration::from_millis(30));
    assert_eq!(default_budget(1280, 720), Duration::from_millis(90));
    assert_eq!(default_budget(2000, 1200), Duration::from_millis(150));
    // red on blue still has a contrast in its luma
    let red_blue: Vec<u8> = gray.iter().flat_map(|&g| if g < 100 { [200, 0, 0] } else { [0, 120, 255] }).collect();
    assert_eq!(decode_rgb(w, h, &red_blue).as_deref(), Some(&t[..]));
}

// ------------------------------------------------------------------------------------------------ nothing to read

#[test]
fn pictures_without_a_code_give_none_quickly() {
    let mut rng = Rng(77);
    let t = std::time::Instant::now();
    for (w, h) in [(640usize, 480usize), (320, 240)] {
        assert_eq!(decode_gray(w, h, &vec![255; w * h]), None, "white");
        assert_eq!(decode_gray(w, h, &vec![0; w * h]), None, "black");
        assert_eq!(decode_gray(w, h, &vec![128; w * h]), None, "grey");
        let noise: Vec<u8> = (0..w * h).map(|_| rng.below(256) as u8).collect();
        assert_eq!(decode_gray(w, h, &noise), None, "noise");
        let bars: Vec<u8> = (0..w * h).map(|i| if (i % w / 7) % 2 == 0 { 0 } else { 255 }).collect();
        assert_eq!(decode_gray(w, h, &bars), None, "bars");
        let ramp: Vec<u8> = (0..w * h).map(|i| (i % w * 255 / w) as u8).collect();
        assert_eq!(decode_gray(w, h, &ramp), None, "ramp");
        // a checkerboard of 1:1:3:1:1 rows everywhere
        let board: Vec<u8> = (0..w * h).map(|i| if (i % w / 5 + i / w / 5) % 2 == 0 { 0 } else { 255 }).collect();
        assert_eq!(decode_gray(w, h, &board), None, "checkerboard");
    }
    // generous: this is a debug build running 14 frames of 640x480 and 320x240
    assert!(t.elapsed() < Duration::from_secs(20), "took {:?}", t.elapsed());
}

#[test]
fn a_symbol_with_its_corner_hidden_or_cut_is_none() {
    let t = text(60, 51);
    let q = qr::encode(&t).unwrap();
    let (w, h, mut img) = render(&q, &Look::default(), 5);
    // cover the top-left finder with a mid-grey patch: only two finders are left
    for y in 0..h / 2 - 20 {
        for x in 0..w / 2 - 20 {
            img[y * w + x] = 120;
        }
    }
    assert_eq!(read(w, h, &img), None);
    // the same picture with the code light on dark (inverted) is not read either
    let (w, h, img) = render(&q, &Look::default(), 5);
    let inv: Vec<u8> = img.iter().map(|&v| 255 - v).collect();
    assert_eq!(read(w, h, &inv), None);
}

#[test]
fn frames_that_are_too_small_too_big_or_short_are_refused_without_work() {
    assert_eq!(decode_gray(0, 0, &[]), None);
    assert_eq!(decode_gray(20, 20, &[0; 400]), None);
    assert_eq!(decode_gray(640, 480, &[0; 100]), None, "a short buffer");
    assert_eq!(decode_gray(5000, 5000, &[]), None, "too many pixels, nothing allocated");
    assert_eq!(decode_gray(usize::MAX, 2, &[]), None, "the size overflows");
    assert_eq!(decode_rgb(100, 100, &[0; 100]), None);
    assert_eq!(decode_rgba(100, 100, &[0; 100]), None);
    assert!(MAX_PIXELS >= 1920 * 1080 && MAX_PIXELS <= 4_000_000);
}

#[test]
fn random_frames_and_damaged_symbols_never_panic() {
    let mut rng = Rng(2026);
    for round in 0..120 {
        let (w, h) = (21 + rng.below(90), 21 + rng.below(90));
        let mut img: Vec<u8> = (0..w * h).map(|_| rng.below(256) as u8).collect();
        match round % 4 {
            0 => {}
            // mostly flat with a few random blobs
            1 => {
                for v in img.iter_mut() {
                    *v = if *v > 200 { 0 } else { 255 };
                }
            }
            // a real symbol with a random rectangle of noise over it
            2 => {
                let q = qr::encode(&text(10 + rng.below(100), round as u64)).unwrap();
                let (qw, qh, mut p) = render(&q, &Look { scale: 2.0 + rng.below(3) as f64, angle: rng.below(360) as f64, mirror: round % 8 == 2, ..Look::default() }, round as u64);
                let (x0, y0) = (rng.below(qw), rng.below(qh));
                for y in y0..(y0 + rng.below(qh / 2)).min(qh) {
                    for x in x0..(x0 + rng.below(qw / 2)).min(qw) {
                        p[y * qw + x] = rng.below(256) as u8;
                    }
                }
                let _ = decode_with(qw, qh, &p, Duration::from_millis(300));
                continue;
            }
            // thin lines and corners
            _ => {
                for i in 0..w * h {
                    img[i] = if i % w < 3 || i / w < 3 || (i % w + i / w) % 11 == 0 { 0 } else { 255 };
                }
            }
        }
        let _ = decode_with(w, h, &img, Duration::from_millis(100));
        let _ = decode_gray(w, h, &img[..img.len() / 2]);
    }
}
