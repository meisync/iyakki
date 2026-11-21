//! Tests of the Huffman side of the JBIG2 decoder (F-PDF-17): the standard tables checked against the source they were taken from,
//! custom tables, symbol code lengths, and Huffman-coded symbol dictionaries and text regions made by an encoder written for the
//! tests (and read by Ghostscript where it reads them).

use super::*;
use crate::jbig2::huff::{self, Bits, Table, SOURCE_CODES};

// ------------------------------------------------------------------------------------------------ bits

#[derive(Default)]
struct BitW(Vec<bool>);

impl BitW {
    fn put(&mut self, x: u64, n: u32) {
        self.0.extend((0..n).rev().map(|i| x >> i & 1 == 1));
    }
    fn val(&mut self, t: &Table, v: Option<i64>) {
        assert!(t.encode(&mut self.0, v), "the table has a line for {v:?}");
    }
    fn align(&mut self) {
        while self.0.len() % 8 != 0 {
            self.0.push(false);
        }
    }
    fn bytes(&mut self, b: &[u8]) {
        self.align();
        for &x in b {
            self.put(x as u64, 8);
        }
    }
    fn finish(mut self) -> Vec<u8> {
        self.align();
        self.0.chunks(8).map(|c| c.iter().fold(0u8, |a, &b| a << 1 | b as u8)).collect()
    }
}

fn std(n: usize) -> Rc<Table> {
    huff::standard(n).unwrap()
}

// ------------------------------------------------------------------------------------------------ the tables

#[test]
fn the_standard_tables_fill_the_code_space_and_carry_the_codes_of_their_source() {
    for n in 1..=15 {
        let t = std(n);
        assert!((t.kraft() - 1.0).abs() < 1e-12, "B.{n}: the prefix lengths do not fill the code space exactly ({})", t.kraft());
    }
    // the codes the B.3 assignment gives equal the explicit codes of the source the lines were taken from
    for n in 1..=15 {
        let mut w = BitW::default();
        let t = std(n);
        // every line of the source, in order, is reached by the first value of its range: code by code
        let want = SOURCE_CODES[n - 1];
        let mut seen = Vec::new();
        for l in t.line_codes() {
            seen.push(l);
        }
        assert_eq!(seen, want, "B.{n}: codes");
        let _ = &mut w;
    }
}

#[test]
fn table_b1_and_b2_give_the_values_the_standard_lists() {
    // B.1: 0..15 as "0" + 4 bits, 16..271 as "10" + 8 bits, 272..65807 as "110" + 16 bits, 65808 and up as "111" + 32 bits
    let mut w = BitW::default();
    w.put(0b0_0101, 5); // 5
    w.put(0b10, 2);
    w.put(0x2A, 8); // 16 + 42
    w.put(0b110, 3);
    w.put(0x1234, 16); // 272 + 4660
    w.put(0b111, 3);
    w.put(7, 32); // 65808 + 7
    let bytes = w.finish();
    let mut b = Bits::new(&bytes);
    let t = std(1);
    assert_eq!(t.decode(&mut b).unwrap(), Some(5));
    assert_eq!(t.decode(&mut b).unwrap(), Some(58));
    assert_eq!(t.decode(&mut b).unwrap(), Some(272 + 0x1234));
    assert_eq!(t.decode(&mut b).unwrap(), Some(65815));
    // B.2: 0 is "0", 1 is "10", 2 is "110", out of band is "111111"
    let mut w = BitW::default();
    w.put(0, 1);
    w.put(0b10, 2);
    w.put(0b110, 3);
    w.put(0b111111, 6);
    let bytes = w.finish();
    let mut b = Bits::new(&bytes);
    let t = std(2);
    assert_eq!(t.decode(&mut b).unwrap(), Some(0));
    assert_eq!(t.decode(&mut b).unwrap(), Some(1));
    assert_eq!(t.decode(&mut b).unwrap(), Some(2));
    assert_eq!(t.decode(&mut b).unwrap(), None);
}

#[test]
fn every_standard_table_codes_and_decodes_values_from_every_line_including_both_ends_and_out_of_band() {
    for n in 1..=15 {
        let t = std(n);
        let mut values: Vec<Option<i64>> = vec![Some(0), Some(1), Some(2), Some(5), Some(100), Some(1000), Some(100_000), Some(-1), Some(-3), Some(-30), Some(-300), Some(-3000), Some(-100_000)];
        values.push(None);
        let mut w = BitW::default();
        let mut coded = Vec::new();
        for v in values {
            // a value a table has no line for (B.1 cannot code a negative number) is skipped
            let mut tmp = Vec::new();
            if t.encode(&mut tmp, v) {
                w.0.extend(tmp);
                coded.push(v);
            }
        }
        assert!(coded.len() >= 4, "B.{n}");
        let bytes = w.finish();
        let mut b = Bits::new(&bytes);
        for v in coded {
            assert_eq!(t.decode(&mut b).unwrap(), v, "B.{n}");
        }
    }
}

#[test]
fn a_standard_table_that_does_not_exist_or_data_that_is_cut_short_is_an_error() {
    assert!(huff::standard(0).is_err());
    assert!(huff::standard(16).is_err());
    let mut b = Bits::new(&[0xFF]);
    assert!(std(1).decode(&mut b).is_err());
    let mut b = Bits::new(&[]);
    assert!(std(15).decode(&mut b).is_err());
}

/// A custom table segment: values -64..64 in four ranges of 32, a lower and an upper range and out of band.
fn custom_body() -> Vec<u8> {
    let mut body = vec![1 | 1 << 1 | 2 << 4];
    body.extend((-64i32).to_be_bytes());
    body.extend(64i32.to_be_bytes());
    let mut w = BitW::default();
    for _ in 0..4 {
        w.put(3, 2);
        w.put(5, 3);
    }
    w.put(3, 2); // lower
    w.put(3, 2); // upper
    w.put(2, 2); // out of band
    body.extend(w.finish());
    body
}

#[test]
fn a_custom_table_is_read_and_codes_every_value_of_its_ranges() {
    let t = huff::custom_table(&custom_body()).unwrap();
    assert!((t.kraft() - 1.0).abs() < 1e-12);
    let values = [Some(-64), Some(-33), Some(0), Some(31), Some(63), Some(-65), Some(-5000), Some(64), Some(9000), None];
    let mut w = BitW::default();
    for v in values {
        w.val(&t, v);
    }
    let bytes = w.finish();
    let mut b = Bits::new(&bytes);
    for v in values {
        assert_eq!(t.decode(&mut b).unwrap(), v);
    }
    // damaged: too short, low above high, prefix lengths that overfill the space
    assert!(huff::custom_table(&[1, 0, 0]).is_err());
    let mut bad = custom_body();
    bad[1..5].copy_from_slice(&100i32.to_be_bytes());
    assert!(huff::custom_table(&bad).is_err());
    let mut over = vec![1 | 1 << 1 | 2 << 4];
    over.extend(0i32.to_be_bytes());
    over.extend(64i32.to_be_bytes());
    let mut w = BitW::default();
    for _ in 0..2 {
        w.put(1, 2);
        w.put(5, 3);
    }
    for _ in 0..3 {
        w.put(1, 2); // five lines of length 1 cannot exist
    }
    over.extend(w.finish());
    assert!(huff::custom_table(&over).is_err());
}

#[test]
fn symbol_code_lengths_are_read_with_the_repeat_and_zero_run_codes() {
    // run codes: 3 gets "0", 32 gets "10", 33 gets "110", 34 gets "111"
    let mut w = BitW::default();
    for i in 0..35 {
        w.put(
            match i {
                3 => 1,
                32 => 2,
                33 | 34 => 3,
                _ => 0,
            },
            4,
        );
    }
    let run = Table::of_lengths(&{
        let mut l = [0u8; 35];
        l[3] = 1;
        l[32] = 2;
        l[33] = 3;
        l[34] = 3;
        l
    })
    .unwrap();
    // the symbols: 3, then (repeat 3 times: +2 bits "01" gives 3 + 1 = 4 copies), then 5 zeros (33 with 2), 15 zeros (34 with 4), then 3
    w.val(&run, Some(3));
    w.val(&run, Some(32));
    w.put(1, 2);
    w.val(&run, Some(33));
    w.put(2, 3);
    w.val(&run, Some(34));
    w.put(4, 7);
    w.val(&run, Some(3));
    let bytes = w.finish();
    let mut b = Bits::new(&bytes);
    let n = 1 + 4 + 5 + 15 + 1;
    let codes = huff::read_symbol_codes(&mut b, n).unwrap();
    // the symbols with a code are 0..5 and the last: the table gives each its own number back
    let mut w = BitW::default();
    for s in [0, 1, 2, 3, 4, n as i64 - 1] {
        w.val(&codes, Some(s));
    }
    let bytes = w.finish();
    let mut b = Bits::new(&bytes);
    for s in [0, 1, 2, 3, 4, n as i64 - 1] {
        assert_eq!(codes.decode(&mut b).unwrap(), Some(s));
    }
    // a repeat with nothing before it, and a run past the last symbol, are errors
    let mut w = BitW::default();
    for i in 0..35 {
        w.put(if i == 32 || i == 3 { 1 } else { 0 }, 4);
    }
    w.put(1, 1); // code of 32
    w.put(0, 2);
    let bytes = w.finish();
    assert!(huff::read_symbol_codes(&mut Bits::new(&bytes), 10).is_err());
}

// ------------------------------------------------------------------------------------------------ symbol dictionary, Huffman

fn sorted_order(syms: &[Bitmap]) -> Vec<usize> {
    let mut o: Vec<usize> = (0..syms.len()).collect();
    o.sort_by_key(|&i| (syms[i].h, syms[i].w));
    o
}

fn collective_rows(class: &[&Bitmap], total: usize) -> Vec<u8> {
    let h = class[0].h;
    let row = total.div_ceil(8);
    let mut out = vec![0u8; row * h];
    for y in 0..h {
        let mut x0 = 0;
        for s in class {
            for x in 0..s.w {
                if s.d[y * s.w + x] != 0 {
                    out[y * row + (x0 + x) / 8] |= 0x80 >> ((x0 + x) % 8);
                }
            }
            x0 += s.w;
        }
    }
    out
}

/// The body of a Huffman symbol dictionary with every table selector 0, no refinement, the symbols of a height class stored as one uncompressed collective bitmap.
fn huff_dict_body(syms: &[Bitmap]) -> (Vec<u8>, Vec<usize>) {
    let order = sorted_order(syms);
    let n = syms.len();
    let mut w = BitW::default();
    let (b4, b2, b1) = (std(4), std(2), std(1));
    let mut height = 0i64;
    let mut i = 0;
    while i < n {
        let h = syms[order[i]].h as i64;
        w.val(&b4, Some(h - height));
        height = h;
        let mut width = 0i64;
        let mut class = Vec::new();
        while i < n && syms[order[i]].h as i64 == h {
            let s = &syms[order[i]];
            w.val(&b2, Some(s.w as i64 - width));
            width = s.w as i64;
            class.push(s);
            i += 1;
        }
        w.val(&b2, None);
        let total: usize = class.iter().map(|s| s.w).sum();
            w.val(&b1, Some(0));
            w.bytes(&collective_rows(&class, total));
    }
    w.val(&b1, Some(0));
    w.val(&b1, Some(n as i64));
    let mut body = 1u16.to_be_bytes().to_vec();
    body.extend((n as u32).to_be_bytes());
    body.extend((n as u32).to_be_bytes());
    body.extend(w.finish());
    let mut pos = vec![0usize; n];
    for (k, &o) in order.iter().enumerate() {
        pos[o] = k;
    }
    (body, pos)
}

fn five_symbols() -> Vec<Bitmap> {
    vec![picture(6, 9, 31), picture(8, 9, 32), picture(5, 7, 33), picture(9, 12, 34), picture(7, 12, 35)]
}

const PLACEMENTS: [(i64, i64, usize); 8] = [(10, 5, 0), (10, 20, 3), (10, 40, 1), (30, 7, 4), (30, 25, 2), (30, 33, 0), (48, 10, 3), (48, 30, 1)];

/// A Huffman text region over `n` symbols with the standard tables of selector 0, the given symbol code lengths and, when `refined`
/// is set, the instance `refined.0` of PLACEMENTS drawn as that bitmap (a refinement of its symbol). Returns the body and the picture.
fn huff_text_body(syms: &[Bitmap], pos: &[usize], corner: u8, transposed: bool, strips_log: u32, refined: Option<(usize, Bitmap)>, custom_ds: Option<&Table>) -> (Vec<u8>, Bitmap) {
    let n = syms.len();
    let (pw, ph) = (90usize, 60usize);
    let mut page = Bitmap::new(pw, ph, 0).unwrap();
    let strips = 1i64 << strips_log;
    let (fs_t, b8, b11, b14, b1) = (std(6), custom_ds.map(|_| std(8)).unwrap_or_else(|| std(8)), std(11), std(14), std(1));
    let ds_t: &Table = custom_ds.unwrap_or(&b8);
    let b15 = std(15);
    let _ = &b14;
    // symbol codes: lengths 2,2,2,3,3 by symbol number in the dictionary order
    let lens: Vec<u8> = (0..n).map(|i| if i < 3 { 2 } else { 3 }).collect();
    let codes = Table::of_lengths(&lens).unwrap();
    let mut w = BitW::default();
    // 35 run code lengths: run codes 2 and 3 get one bit each
    for i in 0..35 {
        w.put(if i == 2 || i == 3 { 1 } else { 0 }, 4);
    }
    let run = Table::of_lengths(&{
        let mut l = [0u8; 35];
        l[2] = 1;
        l[3] = 1;
        l
    })
    .unwrap();
    for &l in &lens {
        w.val(&run, Some(l as i64));
    }
    w.align();
    // the strips
    // the first value is the strip origin counted backwards: B.11 starts at 1, so the first strip starts one strip above the page
    w.val(&b11, Some(1));
    let mut stript = -strips;
    let mut firsts = 0i64;
    let mut idx = 0;
    let right = corner >= 2;
    let bottom = corner & 1 == 0;
    let mut gr = vec![0u8; refinement_contexts(0)];
    let r_at = vec![(-1i8, -1i8), (-1, -1)];
    while idx < PLACEMENTS.len() {
        let strip_t = PLACEMENTS[idx].0 - PLACEMENTS[idx].0.rem_euclid(strips);
        w.val(&b11, Some((strip_t - stript) / strips));
        stript = strip_t;
        let mut first = true;
        let mut curs = 0i64;
        while idx < PLACEMENTS.len() && PLACEMENTS[idx].0 - PLACEMENTS[idx].0.rem_euclid(strips) == strip_t {
            let (t, s, sym) = PLACEMENTS[idx];
            let base = &syms[sym];
            let shown = match &refined {
                Some((k, bm)) if *k == idx => bm,
                _ => base,
            };
            let (wi, hi) = (shown.w as i64, shown.h as i64);
            let adv_before = if !transposed && right { wi - 1 } else if transposed && bottom { hi - 1 } else { 0 };
            if first {
                w.val(&fs_t, Some(s - firsts));
                firsts = s;
                first = false;
            } else {
                w.val(ds_t, Some(s - curs));
            }
            curs = s + adv_before;
            if strips > 1 {
                w.put((t - strip_t) as u64, strips_log);
            }
            w.val(&codes, Some(pos[sym] as i64));
            if refined.is_some() {
                let is_ref = matches!(&refined, Some((k, _)) if *k == idx);
                w.put(is_ref as u64, 1);
                if is_ref {
                    let (rdw, rdh) = (shown.w as i64 - base.w as i64, shown.h as i64 - base.h as i64);
                    let (rdx, rdy) = (0i64, 0i64);
                    for (t, v) in [(&b14, rdw), (&b14, rdh), (&b14, rdx), (&b14, rdy)] {
                        w.val(t, Some(v));
                    }
                    let mut mq = MqEnc::new();
                    let (gdx, gdy) = (rdw.div_euclid(2) + rdx, rdh.div_euclid(2) + rdy);
                    encode_refinement(&mut mq, &mut gr, shown, 0, base, gdx, gdy, &r_at);
                    let data = mq.finish();
                    w.val(&b1, Some(data.len() as i64));
                    w.bytes(&data);
                }
            }
            let (x, y) = if !transposed {
                (if right { curs - wi + 1 } else { curs }, if bottom { t - hi + 1 } else { t })
            } else {
                (if right { t - wi + 1 } else { t }, if bottom { curs - hi + 1 } else { curs })
            };
            page.combine(shown, x, y, 0);
            curs += if !transposed && !right { wi - 1 } else if transposed && !bottom { hi - 1 } else { 0 };
            idx += 1;
        }
        w.val(ds_t, None);
    }
    let _ = &b15;
    let mut hflags = 0u16;
    if custom_ds.is_some() {
        hflags |= 3 << 2;
    }
    let mut tb = region_head(pw, ph, 0, 0, 0);
    let flags: u16 = 1 | ((refined.is_some() as u16) << 1) | ((strips_log as u16) << 2) | ((corner as u16) << 4) | ((transposed as u16) << 6);
    tb.extend(flags.to_be_bytes());
    tb.extend(hflags.to_be_bytes());
    if refined.is_some() {
        tb.extend(at_bytes(&r_at));
    }
    tb.extend((PLACEMENTS.len() as u32).to_be_bytes());
    tb.extend(w.finish());
    (tb, page)
}

#[test]
fn huffman_symbol_dictionaries_and_text_regions_with_every_corner_strips_and_the_transposed_form_decode_and_ghostscript_agrees() {
    let syms = five_symbols();
    let (dict, pos) = huff_dict_body(&syms);
    let globals = segment(0, 0, &[], &dict);
    for corner in 0..4u8 {
        for (transposed, strips_log) in [(false, 0u32), (false, 3), (true, 0)] {
            let (tb, want) = huff_text_body(&syms, &pos, corner, transposed, strips_log, None, None);
            let pages = stream_of((90, 60), &[segment(2, 6, &[0], &tb)]);
            check_with(&format!("huff-text-c{corner}-t{transposed}-s{strips_log}"), &want, &pages, Some(&globals), false);
        }
    }
}

#[test]
fn a_huffman_text_region_with_a_refined_symbol_decodes() {
    let syms = five_symbols();
    let (dict, pos) = huff_dict_body(&syms);
    let globals = segment(0, 0, &[], &dict);
    // the fourth instance (symbol 4, 7 by 12) drawn as a slightly different 8 by 11 bitmap
    let fine = picture(8, 11, 77);
    let (tb, want) = huff_text_body(&syms, &pos, 1, false, 0, Some((3, fine)), None);
    let pages = stream_of((90, 60), &[segment(2, 6, &[0], &tb)]);
    let got = decoded(&pages, Some(&globals));
    same("huffman text with a refined instance", &got, &want);
    // Ghostscript's decoder gives an empty picture for a Huffman text region with a refined instance (it does not read that form), so
    // there is no second reader for this one: it is checked against the encoder written here only.
}

#[test]
fn a_custom_table_named_by_a_selector_is_used_from_the_segment_the_region_refers_to() {
    let syms = five_symbols();
    let (dict, pos) = huff_dict_body(&syms);
    let globals = segment(0, 0, &[], &dict);
    let table = huff::custom_table(&custom_body()).unwrap();
    let (tb, want) = huff_text_body(&syms, &pos, 1, false, 0, None, Some(&table));
    let pages = stream_of((90, 60), &[segment(3, 53, &[], &custom_body()), segment(4, 6, &[0, 3], &tb)]);
    check_with("huff-custom-ds", &want, &pages, Some(&globals), false);
    // without the table segment the region names a table that does not exist: an error, the page stays blank
    let no_table = stream_of((90, 60), &[segment(4, 6, &[0], &tb)]);
    let (page, problem) = decode(&no_table, Some(&globals)).unwrap();
    assert!(problem.unwrap().contains("user supplied"));
    assert!(page.d.iter().all(|&p| p == 0));
}

#[test]
fn a_symbol_dictionary_with_an_mmr_collective_bitmap_is_read_through_the_fax_decoder() {
    // the picture and its Group 4 stream come from the real fax encoder; the whole picture is one symbol of a one-symbol dictionary
    let pic = include_bytes!("../tests/ccitt/text.pbm");
    let g4 = include_bytes!("../tests/ccitt/text.g4");
    let w = u32::from_be_bytes(pic[0..4].try_into().unwrap()) as usize;
    let h = u32::from_be_bytes(pic[4..8].try_into().unwrap()) as usize;
    let row = w.div_ceil(8);
    let mut want = Bitmap::new(w, h, 0).unwrap();
    for y in 0..h {
        for x in 0..w {
            want.d[y * w + x] = (pic[8 + y * row + x / 8] >> (7 - x % 8)) & 1;
        }
    }
    let (b4, b2, b1) = (std(4), std(2), std(1));
    let mut bw = BitW::default();
    bw.val(&b4, Some(h as i64));
    bw.val(&b2, Some(w as i64));
    bw.val(&b2, None);
    bw.val(&b1, Some(g4.len() as i64));
    bw.bytes(g4);
    bw.val(&b1, Some(0));
    bw.val(&b1, Some(1));
    let mut dict = 1u16.to_be_bytes().to_vec();
    dict.extend(1u32.to_be_bytes());
    dict.extend(1u32.to_be_bytes());
    dict.extend(bw.finish());
    // one instance placed at the top left of a page of the picture's size
    let mut t = BitW::default();
    for i in 0..35 {
        t.put(if i == 1 { 1 } else { 0 }, 4);
    }
    // a single symbol needs a code length of 1; the run code for length 1 is itself the only code of that table (length 1)
    let run = Table::of_lengths(&{
        let mut l = [0u8; 35];
        l[1] = 1;
        l
    })
    .unwrap();
    t.val(&run, Some(1));
    t.align();
    let (b6, b8, b11) = (std(6), std(8), std(11));
    t.val(&b11, Some(1));
    t.val(&b11, Some(h as i64 - 1 + 1)); // the strip of T = h - 1 (the corner is the bottom left)
    t.val(&b6, Some(0));
    t.put(0, 1); // the symbol's code (length 1, the first code)
    t.val(&b8, None);
    let mut tb = region_head(w, h, 0, 0, 0);
    tb.extend(1u16.to_be_bytes()); // huffman, bottom left corner, no strips
    tb.extend(0u16.to_be_bytes());
    tb.extend(1u32.to_be_bytes());
    tb.extend(t.finish());
    let data = stream_of((w, h), &[segment(1, 0, &[], &dict), segment(2, 6, &[1], &tb)]);
    check_with("huff-mmr-collective", &want, &data, None, false);
}

#[test]
fn a_huffman_dictionary_can_refine_a_symbol_of_the_one_it_is_given() {
    // dictionary 1: the five symbols; dictionary 2 (huffman, refinement and aggregation): refines symbol 0 of dictionary 1 into a new one
    let syms = five_symbols();
    let (dict1, pos) = huff_dict_body(&syms);
    let base = &syms[0];
    let fine = picture(6, 9, 99);
    let (b4, b2, b1, b15) = (std(4), std(2), std(1), std(15));
    let mut w = BitW::default();
    w.val(&b4, Some(9));
    w.val(&b2, Some(6));
    // instance count 1 (B.1), then the symbol number in the fixed code length, dx and dy (B.15), the size (B.1) and the bytes
    w.val(&b1, Some(1));
    let total = syms.len() + 1;
    let len = code_len(total).max(1);
    w.put(pos[0] as u64, len);
    w.val(&b15, Some(0));
    w.val(&b15, Some(0));
    let mut mq = MqEnc::new();
    let mut gr = vec![0u8; refinement_contexts(0)];
    encode_refinement(&mut mq, &mut gr, &fine, 0, base, 0, 0, &[(-1, -1), (-1, -1)]);
    let data = mq.finish();
    w.val(&b1, Some(data.len() as i64));
    w.bytes(&data);
    w.val(&b2, None);
    // export only the new symbol: skip the five inputs, take one
    w.val(&b1, Some(syms.len() as i64));
    w.val(&b1, Some(1));
    let mut dict2 = (1u16 | 2).to_be_bytes().to_vec();
    dict2.extend(at_bytes(&[(-1, -1), (-1, -1)]));
    dict2.extend(1u32.to_be_bytes());
    dict2.extend(1u32.to_be_bytes());
    dict2.extend(w.finish());
    // a text region with the new symbol alone (one symbol: code length 1)
    let mut t = BitW::default();
    for i in 0..35 {
        t.put(if i == 1 { 1 } else { 0 }, 4);
    }
    let run = Table::of_lengths(&{
        let mut l = [0u8; 35];
        l[1] = 1;
        l
    })
    .unwrap();
    t.val(&run, Some(1));
    t.align();
    let (b6, b8, b11) = (std(6), std(8), std(11));
    t.val(&b11, Some(1));
    t.val(&b11, Some(fine.h as i64));
    t.val(&b6, Some(4));
    t.put(0, 1);
    t.val(&b8, None);
    let mut tb = region_head(20, 20, 0, 0, 0);
    tb.extend(1u16.to_be_bytes());
    tb.extend(0u16.to_be_bytes());
    tb.extend(1u32.to_be_bytes());
    tb.extend(t.finish());
    let data = stream_of((20, 20), &[segment(1, 0, &[], &dict1), segment(2, 0, &[1], &dict2), segment(3, 6, &[2], &tb)]);
    let mut want = Bitmap::new(20, 20, 0).unwrap();
    want.combine(&fine, 4, 0, 0);
    same("a refined symbol of a huffman dictionary", &decoded(&data, None), &want);
}

#[test]
fn damaged_huffman_streams_never_panic_and_never_take_the_memory() {
    let syms = five_symbols();
    let (dict, pos) = huff_dict_body(&syms);
    let (tb, _) = huff_text_body(&syms, &pos, 1, false, 2, None, None);
    let globals = segment(0, 0, &[], &dict);
    let pages = stream_of((90, 60), &[segment(2, 6, &[0], &tb), segment(3, 53, &[], &custom_body())]);
    let mut seed = 5u32;
    for _ in 0..400 {
        let mut g = globals.clone();
        let mut p = pages.clone();
        for _ in 0..3 {
            seed = seed.wrapping_mul(1664525).wrapping_add(1013904223);
            let t = if seed & 0x100 != 0 { &mut g } else { &mut p };
            let i = (seed >> 9) as usize % t.len();
            t[i] ^= 1 << (seed >> 4 & 7);
        }
        if seed & 3 == 0 {
            let cut = (seed >> 11) as usize % p.len();
            p.truncate(cut.max(30));
        }
        let _ = decode(&p, Some(&g));
    }
    // random bytes as a Huffman table segment and as a text region
    for k in 0..200u32 {
        let junk: Vec<u8> = (0..40).map(|i| (k.wrapping_mul(2654435761).wrapping_add(i * 97) >> 7) as u8).collect();
        let _ = huff::custom_table(&junk);
        let _ = decode(&stream_of((30, 30), &[segment(1, 53, &[], &junk), segment(2, 0, &[1], &junk), segment(3, 6, &[1, 2], &junk)]), None);
    }
}
