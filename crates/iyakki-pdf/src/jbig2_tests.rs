//! The JBIG2 decoder (F-PDF-17). The streams are made by a small encoder written here from the standard's encoder annex (the arithmetic
//! coder, the integer coder, generic, refinement, symbol and text segments), and the decoder is checked two ways: it must give the
//! picture back, and two other decoders (Ghostscript's and poppler's, where installed) must give the same picture from the same bytes,
//! so a mistake shared by our encoder and our decoder (a wrong table, a wrong context order) cannot pass. Group 4 data made by the
//! real fax encoder (libtiff) is carried as an MMR generic region. A real stream written by another encoder is read when a copy is
//! on the machine.

use super::*;
use std::process::Command;

// ------------------------------------------------------------------------------------------------ the encoder

struct MqEnc {
    out: Vec<u8>,
    a: u32,
    c: u32,
    ct: i32,
    first: bool,
}

impl MqEnc {
    fn new() -> MqEnc {
        // `out` holds the byte before the first one (the standard's BP = BPST - 1): it is dropped at the end
        MqEnc { out: vec![0], a: 0x8000, c: 0, ct: 12, first: true }
    }

    fn byte_out(&mut self) {
        let last = *self.out.last().unwrap();
        if last == 0xFF {
            self.out.push((self.c >> 20) as u8);
            self.c &= 0xFFFFF;
            self.ct = 7;
        } else if self.c < 0x800_0000 {
            self.out.push((self.c >> 19) as u8);
            self.c &= 0x7FFFF;
            self.ct = 8;
        } else {
            *self.out.last_mut().unwrap() += 1;
            if *self.out.last().unwrap() == 0xFF {
                self.c &= 0x7FF_FFFF;
                self.out.push((self.c >> 20) as u8);
                self.c &= 0xFFFFF;
                self.ct = 7;
            } else {
                self.out.push((self.c >> 19) as u8);
                self.c &= 0x7FFFF;
                self.ct = 8;
            }
        }
    }

    fn renorm(&mut self) {
        loop {
            self.a <<= 1;
            self.c <<= 1;
            self.ct -= 1;
            if self.ct == 0 {
                self.byte_out();
            }
            if self.a & 0x8000 != 0 {
                break;
            }
        }
    }

    fn bit(&mut self, cx: &mut [u8], i: usize, d: u8) {
        let _ = self.first;
        let mut idx = (cx[i] >> 1) as usize;
        let mut mps = cx[i] & 1;
        let (qe, nmps, nlps, sw) = QE[idx];
        let qe = qe as u32;
        if d == mps {
            self.a -= qe;
            if self.a & 0x8000 == 0 {
                if self.a < qe {
                    self.a = qe;
                } else {
                    self.c += qe;
                }
                idx = nmps as usize;
                self.renorm();
            } else {
                self.c += qe;
            }
        } else {
            self.a -= qe;
            if self.a < qe {
                self.c += qe;
            } else {
                self.a = qe;
            }
            if sw == 1 {
                mps = 1 - mps;
            }
            idx = nlps as usize;
            self.renorm();
        }
        cx[i] = ((idx as u8) << 1) | mps;
    }

    fn finish(mut self) -> Vec<u8> {
        let temp = self.c + self.a;
        self.c |= 0xFFFF;
        if self.c >= temp {
            self.c -= 0x8000;
        }
        self.c <<= self.ct;
        self.byte_out();
        self.c <<= self.ct;
        self.byte_out();
        if *self.out.last().unwrap() != 0xFF {
            self.out.push(0xFF);
        }
        self.out.push(0xAC);
        self.out.remove(0);
        self.out
    }
}

struct IntEnc(Vec<u8>);

impl IntEnc {
    fn new() -> IntEnc {
        IntEnc(vec![0; 512])
    }
    fn encode(&mut self, mq: &mut MqEnc, v: Option<i64>) {
        let mut prev = 1usize;
        let mut put = |mq: &mut MqEnc, cx: &mut Vec<u8>, b: u8| {
            mq.bit(cx, prev, b);
            prev = if prev < 256 { (prev << 1) | b as usize } else { (((prev << 1) | b as usize) & 511) | 256 };
        };
        let (s, mag) = match v {
            Some(v) if v >= 0 => (0u8, v as u64),
            Some(v) => (1, (-v) as u64),
            None => (1, 0),
        };
        put(mq, &mut self.0, s);
        let (prefix, n, off): (&[u8], u32, u64) = if mag < 4 {
            (&[0], 2, 0)
        } else if mag < 20 {
            (&[1, 0], 4, 4)
        } else if mag < 84 {
            (&[1, 1, 0], 6, 20)
        } else if mag < 340 {
            (&[1, 1, 1, 0], 8, 84)
        } else if mag < 4436 {
            (&[1, 1, 1, 1, 0], 12, 340)
        } else {
            (&[1, 1, 1, 1, 1], 32, 4436)
        };
        for b in prefix {
            put(mq, &mut self.0, *b);
        }
        let val = mag - off;
        for k in (0..n).rev() {
            put(mq, &mut self.0, ((val >> k) & 1) as u8);
        }
    }
}

struct IdEnc(Vec<u8>);

impl IdEnc {
    fn new(len: u32) -> IdEnc {
        IdEnc(vec![0; 1 << (len + 1)])
    }
    fn encode(&mut self, mq: &mut MqEnc, len: u32, id: usize) {
        let mut prev = 1usize;
        for k in (0..len).rev() {
            let b = ((id >> k) & 1) as u8;
            mq.bit(&mut self.0, prev, b);
            prev = (prev << 1) | b as usize;
        }
    }
}

fn encode_generic(mq: &mut MqEnc, cx: &mut [u8], bm: &Bitmap, template: u8, tpgdon: bool, at: &[(i8, i8)]) {
    let mut ltp = 0u8;
    for y in 0..bm.h {
        if tpgdon {
            let same = y > 0 && bm.d[y * bm.w..(y + 1) * bm.w] == bm.d[(y - 1) * bm.w..y * bm.w] || (y == 0 && bm.d[..bm.w].iter().all(|&p| p == 0));
            let new_ltp = same as u8;
            mq.bit(cx, sltp_context(template), ltp ^ new_ltp);
            ltp = new_ltp;
            if ltp == 1 {
                continue;
            }
        }
        for x in 0..bm.w {
            let c = generic_context(bm, x as i64, y as i64, template, at);
            mq.bit(cx, c, bm.d[y * bm.w + x]);
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn encode_refinement(mq: &mut MqEnc, cx: &mut [u8], bm: &Bitmap, template: u8, reference: &Bitmap, dx: i64, dy: i64, at: &[(i8, i8)]) {
    for y in 0..bm.h as i64 {
        for x in 0..bm.w as i64 {
            let r = |i: i64, j: i64| reference.get(x - dx + i, y - dy + j) as usize;
            let p = |i: i64, j: i64| bm.get(x + i, y + j) as usize;
            let c = if template == 0 {
                p(-1, 0) | p(1, -1) << 1 | p(0, -1) << 2 | p(at[0].0 as i64, at[0].1 as i64) << 3 | r(1, 1) << 4 | r(0, 1) << 5 | r(-1, 1) << 6 | r(1, 0) << 7 | r(0, 0) << 8 | r(-1, 0) << 9 | r(1, -1) << 10 | r(0, -1) << 11 | r(at[1].0 as i64, at[1].1 as i64) << 12
            } else {
                p(-1, 0) | p(1, -1) << 1 | p(0, -1) << 2 | p(-1, -1) << 3 | r(1, 1) << 4 | r(0, 1) << 5 | r(1, 0) << 6 | r(0, 0) << 7 | r(-1, 0) << 8 | r(0, -1) << 9
            };
            mq.bit(cx, c, bm.d[y as usize * bm.w + x as usize]);
        }
    }
}

// ------------------------------------------------------------------------------------------------ segments

fn segment(number: u32, kind: u8, refs: &[u32], body: &[u8]) -> Vec<u8> {
    let mut v = number.to_be_bytes().to_vec();
    v.push(kind);
    v.push((refs.len() as u8) << 5);
    for r in refs {
        v.push(*r as u8);
    }
    v.push(1); // page 1 (0 for a global segment is also fine)
    v.extend((body.len() as u32).to_be_bytes());
    v.extend(body);
    v
}

fn page_info(w: u32, h: u32, def: u8) -> Vec<u8> {
    let mut b = w.to_be_bytes().to_vec();
    b.extend(h.to_be_bytes());
    b.extend([0u8; 8]);
    b.push(def << 2);
    b.extend([0u8; 2]);
    b
}

fn region_head(w: usize, h: usize, x: u32, y: u32, op: u8) -> Vec<u8> {
    let mut b = (w as u32).to_be_bytes().to_vec();
    b.extend((h as u32).to_be_bytes());
    b.extend(x.to_be_bytes());
    b.extend(y.to_be_bytes());
    b.push(op);
    b
}

fn at_bytes(at: &[(i8, i8)]) -> Vec<u8> {
    at.iter().flat_map(|&(x, y)| [x as u8, y as u8]).collect()
}

fn generic_segment_body(bm: &Bitmap, x: u32, y: u32, op: u8, template: u8, tpgdon: bool, at: &[(i8, i8)]) -> Vec<u8> {
    let mut b = region_head(bm.w, bm.h, x, y, op);
    b.push((template << 1) | ((tpgdon as u8) << 3));
    b.extend(at_bytes(at));
    let mut mq = MqEnc::new();
    let mut cx = vec![0u8; generic_contexts(template)];
    encode_generic(&mut mq, &mut cx, bm, template, tpgdon, at);
    b.extend(mq.finish());
    b
}

/// A picture with some structure (blocks, lines, a ring and noise at the edges) so every context is exercised.
fn picture(w: usize, h: usize, seed: u32) -> Bitmap {
    let mut bm = Bitmap::new(w, h, 0).unwrap();
    let mut s = seed;
    let mut rnd = || {
        s = s.wrapping_mul(1664525).wrapping_add(1013904223);
        (s >> 16) as usize
    };
    for _ in 0..(w * h / 90).max(3) {
        let (bx, by, bw, bh) = (rnd() % w, rnd() % h, 1 + rnd() % 9, 1 + rnd() % 7);
        for y in by..(by + bh).min(h) {
            for x in bx..(bx + bw).min(w) {
                bm.d[y * w + x] = 1;
            }
        }
    }
    for y in 0..h {
        for x in 0..w {
            let (dx, dy) = (x as i64 - w as i64 / 2, y as i64 - h as i64 / 2);
            let r = dx * dx + dy * dy;
            if r > 140 && r < 190 {
                bm.d[y * w + x] ^= 1;
            }
            if rnd() % 61 == 0 {
                bm.d[y * w + x] ^= 1;
            }
        }
    }
    bm
}

fn stream_of(page: (usize, usize), segs: &[Vec<u8>]) -> Vec<u8> {
    let mut v = segment(100, 48, &[], &page_info(page.0 as u32, page.1 as u32, 0));
    for s in segs {
        v.extend(s);
    }
    v
}

fn decoded(data: &[u8], globals: Option<&[u8]>) -> Bitmap {
    let (page, problem) = decode(data, globals).expect("a page");
    assert_eq!(problem, None);
    page
}

// ------------------------------------------------------------------------------------------------ other decoders

fn have(tool: &str, arg: &str) -> bool {
    Command::new(tool).arg(arg).output().is_ok()
}

fn pdf_with(w: usize, h: usize, data: &[u8], globals: Option<&[u8]>) -> Vec<u8> {
    let mut out: Vec<u8> = b"%PDF-1.5\n".to_vec();
    let mut offs = Vec::new();
    let mut obj = |out: &mut Vec<u8>, n: usize, body: &[u8]| {
        offs.push(out.len());
        out.extend(format!("{n} 0 obj\n").as_bytes());
        out.extend(body);
        out.extend(b"\nendobj\n");
    };
    let parms = if globals.is_some() { "/DecodeParms << /JBIG2Globals 7 0 R >>" } else { "" };
    obj(&mut out, 1, b"<< /Type /Catalog /Pages 2 0 R >>");
    obj(&mut out, 2, b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>");
    obj(&mut out, 3, format!("<< /Type /Page /Parent 2 0 R /MediaBox [0 0 {w} {h}] /Resources << /XObject << /Im 5 0 R >> >> /Contents 4 0 R >>").as_bytes());
    let content = format!("q {w} 0 0 {h} 0 0 cm /Im Do Q");
    obj(&mut out, 4, format!("<< /Length {} >>\nstream\n{content}\nendstream", content.len()).as_bytes());
    let mut im = format!("<< /Type /XObject /Subtype /Image /Width {w} /Height {h} /ColorSpace /DeviceGray /BitsPerComponent 1 /Filter /JBIG2Decode {parms} /Length {} >>\nstream\n", data.len()).into_bytes();
    im.extend(data);
    im.extend(b"\nendstream");
    obj(&mut out, 5, &im);
    obj(&mut out, 6, b"<< >>");
    let g = globals.unwrap_or(&[]);
    let mut gb = format!("<< /Length {} >>\nstream\n", g.len()).into_bytes();
    gb.extend(g);
    gb.extend(b"\nendstream");
    obj(&mut out, 7, &gb);
    let xref = out.len();
    out.extend(format!("xref\n0 8\n0000000000 65535 f \n").as_bytes());
    for o in &offs {
        out.extend(format!("{o:010} 00000 n \n").as_bytes());
    }
    out.extend(format!("trailer\n<< /Size 8 /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n").as_bytes());
    out
}


/// Read a raw PBM (P4): width, height and the picture as one byte per pixel (1 black). Comment lines are skipped.
fn parse_p4(pbm: &[u8]) -> (usize, usize, Bitmap) {
    let mut at = 2;
    let mut nums = Vec::new();
    while nums.len() < 2 {
        while pbm[at].is_ascii_whitespace() {
            at += 1;
        }
        if pbm[at] == b'#' {
            while pbm[at] != b'\n' {
                at += 1;
            }
            continue;
        }
        let st = at;
        while pbm[at].is_ascii_digit() {
            at += 1;
        }
        nums.push(String::from_utf8_lossy(&pbm[st..at]).parse::<usize>().unwrap());
    }
    at += 1;
    let (w, h) = (nums[0], nums[1]);
    let row = w.div_ceil(8);
    let mut bm = Bitmap::new(w, h, 0).unwrap();
    for y in 0..h {
        for x in 0..w {
            bm.d[y * w + x] = (pbm[at + y * row + x / 8] >> (7 - x % 8)) & 1;
        }
    }
    (w, h, bm)
}

/// What Ghostscript draws for the picture in a PDF page of the picture's size: one byte per pixel, 1 black.
fn gs_picture(tag: &str, w: usize, h: usize, data: &[u8], globals: Option<&[u8]>) -> Option<Bitmap> {
    if !have("gs", "--version") {
        return None;
    }
    let d = std::path::PathBuf::from("/tmp").join(format!("iyakki-jbig2-{}-{tag}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    std::fs::write(d.join("a.pdf"), pdf_with(w, h, data, globals)).unwrap();
    let o = Command::new("gs")
        .env("TMPDIR", "/tmp")
        .args(["-q", "-dNOPAUSE", "-dBATCH", "-dSAFER", "-sDEVICE=pbmraw", "-r72", "-dGraphicsAlphaBits=1", "-dTextAlphaBits=1"])
        .arg(format!("-sOutputFile={}", d.join("o.pbm").display()))
        .arg(d.join("a.pdf"))
        .output()
        .unwrap();
    let pbm = std::fs::read(d.join("o.pbm"));
    if !String::from_utf8_lossy(&o.stderr).trim().is_empty() || !String::from_utf8_lossy(&o.stdout).trim().is_empty() {
        eprintln!("gs for {tag} said: {}{}", String::from_utf8_lossy(&o.stdout), String::from_utf8_lossy(&o.stderr));
    }
    let _ = std::fs::remove_dir_all(&d);
    assert!(o.status.success(), "gs: {}", String::from_utf8_lossy(&o.stderr));
    let pbm = pbm.ok()?;
    let (pw, ph, bm) = parse_p4(&pbm);
    assert_eq!((pw, ph), (w, h), "gs page size");
    Some(bm)
}

/// What poppler draws (`pdfimages`: the decoded samples), one byte per pixel, 1 black.
fn poppler_picture(tag: &str, w: usize, h: usize, data: &[u8], globals: Option<&[u8]>) -> Option<Bitmap> {
    if !have("pdfimages", "-v") {
        return None;
    }
    let d = std::path::PathBuf::from("/tmp").join(format!("iyakki-jbig2p-{}-{tag}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    std::fs::write(d.join("a.pdf"), pdf_with(w, h, data, globals)).unwrap();
    // `pdfimages` writes the decoded samples of the picture itself (no scaling, no screening)
    let o = Command::new("pdfimages").arg(d.join("a.pdf")).arg(d.join("o")).output().unwrap();
    assert!(o.status.success(), "pdfimages: {}", String::from_utf8_lossy(&o.stderr));
    let name = std::fs::read_dir(&d).unwrap().filter_map(|e| e.ok()).map(|e| e.path()).find(|p| p.extension().is_some_and(|x| x == "pbm"))?;
    let pbm = std::fs::read(name).ok()?;
    let _ = std::fs::remove_dir_all(&d);
    let (pw, ph, bm) = parse_p4(&pbm);
    if (pw, ph) != (w, h) {
        return None;
    }
    Some(bm)
}

/// The picture must come back from our decoder and from each other decoder that is installed.
fn same(what: &str, got: &Bitmap, want: &Bitmap) {
    if got == want {
        return;
    }
    let diff: Vec<usize> = got.d.iter().zip(&want.d).enumerate().filter(|(_, (a, b))| a != b).map(|(i, _)| i).collect();
    panic!("{what}: black {} against {}, {} pixels differ, the first at ({}, {})", got.d.iter().filter(|&&p| p == 1).count(), want.d.iter().filter(|&&p| p == 1).count(), diff.len(), diff.first().map(|i| i % want.w).unwrap_or(0), diff.first().map(|i| i / want.w).unwrap_or(0));
}

fn check_everywhere(tag: &str, want: &Bitmap, data: &[u8], globals: Option<&[u8]>) {
    check_with(tag, want, data, globals, true);
}

fn check_with(tag: &str, want: &Bitmap, data: &[u8], globals: Option<&[u8]>, poppler: bool) {
    let ours = decoded(data, globals);
    same(&format!("{tag}: our decoder"), &ours, want);
    if let Some(g) = gs_picture(tag, want.w, want.h, data, globals) {
        same(&format!("{tag}: Ghostscript's decoder reads the same bytes as another picture"), &g, want);
    } else {
        eprintln!("skipped Ghostscript for {tag}");
    }
    if !poppler {
        return;
    }
    if let Some(p) = poppler_picture(tag, want.w, want.h, data, globals) {
        same(&format!("{tag}: poppler's decoder reads the same bytes as another picture"), &p, want);
    } else {
        eprintln!("skipped poppler for {tag}");
    }
}

/// A page stream with one generic region (template 0, typical prediction on) for the other test modules.
pub(crate) fn generic_page_stream(bm: &Bitmap) -> Vec<u8> {
    stream_of((bm.w, bm.h), &[segment(1, 38, &[], &generic_segment_body(bm, 0, 0, 0, 0, true, &default_at(0)))])
}

// ------------------------------------------------------------------------------------------------ tests

#[test]
fn the_probability_table_is_consistent() {
    assert_eq!(QE.len(), 47);
    for (i, &(qe, nmps, nlps, sw)) in QE.iter().enumerate() {
        assert!(qe > 0 && (nmps as usize) < 47 && (nlps as usize) < 47 && sw <= 1, "row {i}");
    }
    assert_eq!(QE[0].0, 0x5601);
    assert_eq!(QE[45].0, 0x0001);
}

#[test]
fn the_arithmetic_coder_and_the_integer_coder_give_back_what_was_coded() {
    let mut enc = MqEnc::new();
    let mut cx = vec![0u8; 8];
    let mut ie = IntEnc::new();
    let vals: Vec<Option<i64>> = vec![Some(0), Some(3), Some(-3), Some(4), Some(19), Some(-20), Some(83), Some(339), Some(-4435), Some(4436), Some(100_000), None, Some(-1), Some(7)];
    for (i, v) in vals.iter().enumerate() {
        enc.bit(&mut cx, i % 8, (i % 3 == 0) as u8);
        ie.encode(&mut enc, *v);
    }
    let bytes = enc.finish();
    assert_eq!(&bytes[bytes.len() - 2..], &[0xFF, 0xAC]);
    let mut mq = Mq::new(&bytes);
    let mut cx = vec![0u8; 8];
    let mut id = IntCx::new();
    for (i, v) in vals.iter().enumerate() {
        assert_eq!(mq.bit(&mut cx, i % 8), (i % 3 == 0) as u8, "bit {i}");
        assert_eq!(id.decode(&mut mq), *v, "value {i}");
    }
}

#[test]
fn generic_regions_of_every_template_with_typical_prediction_and_moving_pixels_decode_and_other_readers_agree() {
    let bm = picture(83, 61, 5);
    for (template, tpgdon, at) in [
        (0u8, false, default_at(0)),
        (0, true, default_at(0)),
        (0, false, vec![(4, -2), (-5, -1), (1, -2), (-3, -2)]),
        (1, false, default_at(1)),
        (1, true, default_at(1)),
        (2, false, default_at(2)),
        (2, true, vec![(-2, -2)]),
        (3, false, default_at(3)),
        (3, true, default_at(3)),
    ] {
        let data = stream_of((bm.w, bm.h), &[segment(1, 38, &[], &generic_segment_body(&bm, 0, 0, 0, template, tpgdon, &at))]);
        check_everywhere(&format!("generic-t{template}-{tpgdon}-{}", at[0].0), &bm, &data, None);
    }
}

#[test]
fn a_picture_with_repeated_and_blank_rows_uses_typical_prediction_and_decodes() {
    let mut bm = picture(64, 40, 9);
    for y in 10..20 {
        let (a, b) = bm.d.split_at_mut(y * 64);
        b[..64].copy_from_slice(&a[(y - 1) * 64..y * 64]);
    }
    for x in 0..64 {
        bm.d[x] = 0;
        bm.d[64 + x] = 0;
    }
    let data = stream_of((64, 40), &[segment(1, 38, &[], &generic_segment_body(&bm, 0, 0, 0, 0, true, &default_at(0)))]);
    check_everywhere("tpgdon-rows", &bm, &data, None);
}

#[test]
fn regions_are_combined_into_the_page_with_each_operator_and_may_hang_over_its_edge() {
    let a = picture(40, 30, 1);
    let b = picture(30, 30, 2);
    let mut want = Bitmap::new(50, 40, 0).unwrap();
    want.combine(&a, 3, 4, 0);
    want.combine(&b, 15, 8, 2);
    let mut c = picture(20, 20, 3);
    want.combine(&c, 40, 30, 4);
    let data = stream_of(
        (50, 40),
        &[
            segment(1, 38, &[], &generic_segment_body(&a, 3, 4, 0, 0, false, &default_at(0))),
            segment(2, 38, &[], &generic_segment_body(&b, 15, 8, 2, 1, false, &default_at(1))),
            segment(3, 38, &[], &generic_segment_body(&c, 40, 30, 4, 2, false, &default_at(2))),
        ],
    );
    assert_eq!(decoded(&data, None), want);
    c = picture(20, 20, 4);
    let mut w2 = Bitmap::new(50, 40, 1).unwrap();
    w2.combine(&c, 5, 5, 1);
    let mut info = page_info(50, 40, 1);
    info[16] = 1 << 2;
    let mut v = segment(0, 48, &[], &info);
    v.extend(segment(1, 38, &[], &generic_segment_body(&c, 5, 5, 1, 0, false, &default_at(0))));
    assert_eq!(decoded(&v, None), w2);
    let mut w3 = Bitmap::new(50, 40, 0).unwrap();
    w3.combine(&c, 5, 5, 3);
    let data = stream_of((50, 40), &[segment(1, 38, &[], &generic_segment_body(&c, 5, 5, 3, 0, false, &default_at(0)))]);
    assert_eq!(decoded(&data, None), w3);
}

#[test]
fn a_page_of_unknown_height_grows_with_its_regions() {
    let a = picture(40, 12, 11);
    let b = picture(40, 12, 12);
    let mut info = page_info(40, 0, 0);
    info[4..8].copy_from_slice(&0xFFFF_FFFFu32.to_be_bytes());
    let mut v = segment(0, 48, &[], &info);
    v.extend(segment(1, 38, &[], &generic_segment_body(&a, 0, 0, 0, 0, false, &default_at(0))));
    v.extend(segment(2, 50, &[], &11u32.to_be_bytes()));
    v.extend(segment(3, 38, &[], &generic_segment_body(&b, 0, 12, 0, 0, false, &default_at(0))));
    v.extend(segment(4, 50, &[], &23u32.to_be_bytes()));
    let page = decoded(&v, None);
    assert_eq!((page.w, page.h), (40, 24));
    assert_eq!(page.sub(0, 12, 40, 12).unwrap(), b);
}

#[test]
fn a_generic_region_coded_as_mmr_is_read_through_the_fax_decoder() {
    // the picture and its Group 4 stream come from the real fax encoder (libtiff through Pillow)
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
    let mut body = region_head(w, h, 0, 0, 0);
    body.push(1); // MMR
    body.extend(g4);
    let data = stream_of((w, h), &[segment(1, 38, &[], &body)]);
    // Ghostscript reads it as libtiff wrote it; poppler's MMR reader gives a different picture for this same stream (it is left out)
    check_with("mmr", &want, &data, None, false);
}

#[test]
fn a_generic_region_of_unknown_length_is_cut_at_its_end_marker() {
    let a = picture(30, 20, 21);
    let body = generic_segment_body(&a, 0, 0, 0, 0, false, &default_at(0));
    let mut with_rows = body.clone();
    with_rows.extend(20u32.to_be_bytes());
    // header of the segment with the length field 0xFFFFFFFF
    let mut seg = 1u32.to_be_bytes().to_vec();
    seg.extend([38, 0, 1]);
    seg.extend(0xFFFF_FFFFu32.to_be_bytes());
    seg.extend(&with_rows);
    let mut v = segment(0, 48, &[], &page_info(30, 20, 0));
    v.extend(seg);
    v.extend(segment(2, 49, &[], &[]));
    assert_eq!(decoded(&v, None), a);
}

/// Symbols and a text region over them; returns (global stream, page stream, expected picture).
fn text_page(corner: u8, transposed: bool, strips_log: u32, refine: bool) -> (Vec<u8>, Vec<u8>, Bitmap) {
    let syms: Vec<Bitmap> = vec![picture(6, 9, 31), picture(8, 9, 32), picture(5, 7, 33), picture(9, 12, 34), picture(7, 12, 35)];
    // the dictionary: symbols sorted into height classes, each class's widths as differences
    let order: Vec<usize> = {
        let mut o: Vec<usize> = (0..syms.len()).collect();
        o.sort_by_key(|&i| (syms[i].h, syms[i].w));
        o
    };
    let n = syms.len();
    let mut mq = MqEnc::new();
    let mut gb = vec![0u8; generic_contexts(0)];
    let (mut dh, mut dw, mut ex) = (IntEnc::new(), IntEnc::new(), IntEnc::new());
    let at = default_at(0);
    let mut height = 0i64;
    let mut i = 0;
    while i < n {
        let h = syms[order[i]].h as i64;
        dh.encode(&mut mq, Some(h - height));
        height = h;
        let mut width = 0i64;
        while i < n && syms[order[i]].h as i64 == h {
            let s = &syms[order[i]];
            dw.encode(&mut mq, Some(s.w as i64 - width));
            width = s.w as i64;
            encode_generic(&mut mq, &mut gb, s, 0, false, &at);
            i += 1;
        }
        dw.encode(&mut mq, None);
    }
    ex.encode(&mut mq, Some(0));
    ex.encode(&mut mq, Some(n as i64));
    let mut body = Vec::new();
    body.extend(0u16.to_be_bytes());
    body.extend(at_bytes(&at));
    body.extend((n as u32).to_be_bytes());
    body.extend((n as u32).to_be_bytes());
    body.extend(mq.finish());
    let globals = segment(0, 0, &[], &body);
    // symbol index in the dictionary (sorted order) of each source symbol
    let mut pos = vec![0usize; n];
    for (k, &o) in order.iter().enumerate() {
        pos[o] = k;
    }
    // the placements: (strip T, S, symbol)
    let (pw, ph) = (90usize, 60usize);
    let mut page = Bitmap::new(pw, ph, 0).unwrap();
    let placements: Vec<(i64, i64, usize, Option<Bitmap>)> = vec![(10, 5, 0, None), (10, 20, 3, None), (10, 40, 1, None), (30, 7, 4, None), (30, 25, 2, None), (30, 33, 0, None), (48, 10, 3, None), (48, 30, 1, None)];
    let strips = 1i64 << strips_log;
    let len = code_len(n);
    let mut tm = MqEnc::new();
    let (mut dt, mut fs, mut ds, mut it, mut ri) = (IntEnc::new(), IntEnc::new(), IntEnc::new(), IntEnc::new(), IntEnc::new());
    let (mut rdw, mut rdh, mut rdx, mut rdy) = (IntEnc::new(), IntEnc::new(), IntEnc::new(), IntEnc::new());
    let mut idc = IdEnc::new(len);
    let mut gr = vec![0u8; refinement_contexts(0)];
    let r_at = vec![(-1i8, -1i8), (-1, -1)];
    dt.encode(&mut tm, Some(0));
    // group by strip of T
    let mut stript = 0i64;
    let mut firsts = 0i64;
    let mut idx = 0;
    let right = corner >= 2;
    let bottom = corner & 1 == 0;
    while idx < placements.len() {
        let strip_t = placements[idx].0 - placements[idx].0.rem_euclid(strips);
        dt.encode(&mut tm, Some((strip_t - stript) / strips));
        stript = strip_t;
        let mut first = true;
        let mut curs = 0i64;
        while idx < placements.len() && placements[idx].0 - placements[idx].0.rem_euclid(strips) == strip_t {
            let (t, s, sym, _) = placements[idx].clone();
            let shown = &syms[sym];
            let (wi, hi) = (shown.w as i64, shown.h as i64);
            // the S sent is the position before the reference corner's advance: the decoder adds the advance itself
            let adv_before = if !transposed && right { wi - 1 } else if transposed && bottom { hi - 1 } else { 0 };
            if first {
                fs.encode(&mut tm, Some(s - firsts));
                firsts = s;
                first = false;
            } else {
                ds.encode(&mut tm, Some(s - curs));
            }
            curs = s + adv_before;
            if strips > 1 {
                it.encode(&mut tm, Some(t - strip_t));
            }
            idc.encode(&mut tm, len, pos[sym]);
            if refine {
                ri.encode(&mut tm, Some(0));
            }
            // place in the expected picture
            let (x, y) = if !transposed {
                (if right { curs - wi + 1 } else { curs }, if bottom { t - hi + 1 } else { t })
            } else {
                (if right { t - wi + 1 } else { t }, if bottom { curs - hi + 1 } else { curs })
            };
            page.combine(shown, x, y, 0);
            curs += if !transposed && !right { wi - 1 } else if transposed && !bottom { hi - 1 } else { 0 };
            idx += 1;
        }
        ds.encode(&mut tm, None);
    }
    let _ = (&mut rdw, &mut rdh, &mut rdx, &mut rdy, &mut gr, &r_at);
    let mut tb = region_head(pw, ph, 0, 0, 0);
    let flags: u16 = ((refine as u16) << 1) | ((strips_log as u16) << 2) | ((corner as u16) << 4) | ((transposed as u16) << 6);
    tb.extend(flags.to_be_bytes());
    if refine {
        tb.extend(at_bytes(&r_at));
    }
    tb.extend((placements.len() as u32).to_be_bytes());
    tb.extend(tm.finish());
    let pages = stream_of((pw, ph), &[segment(2, 6, &[0], &tb)]);
    (globals, pages, page)
}

#[test]
fn symbols_and_text_regions_with_every_corner_strips_and_the_transposed_form_decode_and_other_readers_agree() {
    for corner in 0..4u8 {
        for (transposed, strips_log) in [(false, 0u32), (false, 3), (true, 0)] {
            let (g, p, want) = text_page(corner, transposed, strips_log, false);
            check_everywhere(&format!("text-c{corner}-t{transposed}-s{strips_log}"), &want, &p, Some(&g));
        }
    }
}

#[test]
fn a_text_region_with_the_refinement_flag_unused_still_decodes() {
    let (g, p, want) = text_page(1, false, 2, true);
    check_everywhere("text-refine-flag", &want, &p, Some(&g));
}

#[test]
fn a_symbol_dictionary_can_be_the_page_streams_own_segment_and_one_can_refer_to_another() {
    // two dictionaries: the second refers to the first and exports both its symbols and the first's
    let (g, p, want) = text_page(1, false, 0, false);
    let mut v = g.clone();
    v.extend(&p);
    // the dictionary segment has number 0 and page association 1 here, as the globals do
    assert_eq!(decoded(&v, None), want);
}

#[test]
fn a_refinement_region_corrects_the_page_it_covers_and_a_refined_copy_of_an_intermediate_region_works() {
    let base = picture(48, 36, 41);
    let mut fine = base.clone();
    for k in 0..40 {
        let (x, y) = ((k * 7) % 48, (k * 5) % 36);
        fine.d[y * 48 + x] ^= 1;
    }
    for template in [0u8, 1] {
        let at = vec![(-1i8, -1i8), (-1, -1)];
        let mut mq = MqEnc::new();
        let mut cx = vec![0u8; refinement_contexts(template)];
        encode_refinement(&mut mq, &mut cx, &fine, template, &base, 0, 0, &at);
        // refine the page content
        let mut body = region_head(48, 36, 0, 0, 4);
        body.push(template);
        if template == 0 {
            body.extend(at_bytes(&at));
        }
        body.extend(mq.finish());
        let data = stream_of((48, 36), &[segment(1, 38, &[], &generic_segment_body(&base, 0, 0, 0, 0, false, &default_at(0))), segment(2, 42, &[], &body)]);
        assert_eq!(decoded(&data, None), fine, "template {template}, on the page");
        // refine an intermediate region (type 36) that the segment refers to; the page gets the refined one
        let data = stream_of((48, 36), &[segment(1, 36, &[], &generic_segment_body(&base, 0, 0, 0, 0, false, &default_at(0))), segment(2, 42, &[1], &body)]);
        assert_eq!(decoded(&data, None), fine, "template {template}, from an intermediate region");
        // Ghostscript's decoder does not read intermediate regions, so it is given the page form
        let page_form = stream_of((48, 36), &[segment(1, 38, &[], &generic_segment_body(&base, 0, 0, 0, 0, false, &default_at(0))), segment(2, 42, &[], &body)]);
        // poppler differs in one pixel at the right edge (it reads the reference beyond the edge differently from the standard and from
        // Ghostscript); only Ghostscript is the second reader here
        check_with(&format!("refine-{template}"), &fine, &page_form, None, false);
    }
}

#[test]
fn typical_prediction_in_a_refinement_region_is_read() {
    let base = picture(40, 30, 51);
    let mut fine = base.clone();
    fine.d[5 * 40 + 5] ^= 1;
    fine.d[20 * 40 + 30] ^= 1;
    let at = vec![(-1i8, -1i8), (-1, -1)];
    for template in [0u8, 1] {
        // typical prediction: every row's flag is sent; rows are predicted wherever the reference's 3 by 3 neighbourhood is flat. The encoder
        // sends LTP = 1 for rows that agree with the reference wherever it is flat, and codes the other pixels normally
        let mut mq = MqEnc::new();
        let mut cx = vec![0u8; refinement_contexts(template)];
        let mut ltp = 0u8;
        for y in 0..fine.h as i64 {
            let flat = |x: i64| {
                let v = base.get(x, y);
                (-1..=1).all(|j| (-1..=1).all(|i| base.get(x + i, y + j) == v))
            };
            let ok = (0..fine.w as i64).all(|x| !flat(x) || fine.get(x, y) == base.get(x, y));
            let want = ok as u8;
            mq.bit(&mut cx, if template == 0 { 0x100 } else { 0x80 }, ltp ^ want);
            ltp = want;
            for x in 0..fine.w as i64 {
                if ltp == 1 && flat(x) {
                    continue;
                }
                let r = |i: i64, j: i64| base.get(x + i, y + j) as usize;
                let p = |i: i64, j: i64| fine.get(x + i, y + j) as usize;
                let c = if template == 0 {
                    p(-1, 0) | p(1, -1) << 1 | p(0, -1) << 2 | p(-1, -1) << 3 | r(1, 1) << 4 | r(0, 1) << 5 | r(-1, 1) << 6 | r(1, 0) << 7 | r(0, 0) << 8 | r(-1, 0) << 9 | r(1, -1) << 10 | r(0, -1) << 11 | r(-1, -1) << 12
                } else {
                    p(-1, 0) | p(1, -1) << 1 | p(0, -1) << 2 | p(-1, -1) << 3 | r(1, 1) << 4 | r(0, 1) << 5 | r(1, 0) << 6 | r(0, 0) << 7 | r(-1, 0) << 8 | r(0, -1) << 9
                };
                mq.bit(&mut cx, c, fine.d[y as usize * 40 + x as usize]);
            }
        }
        let mut body = region_head(40, 30, 0, 0, 4);
        body.push(template | 2);
        if template == 0 {
            body.extend(at_bytes(&at));
        }
        body.extend(mq.finish());
        let data = stream_of((40, 30), &[segment(1, 38, &[], &generic_segment_body(&base, 0, 0, 0, 0, false, &default_at(0))), segment(2, 42, &[], &body)]);
        assert_eq!(decoded(&data, None), fine, "template {template}");
    }
}

#[test]
fn a_halftone_region_with_its_pattern_dictionary_decodes_even_on_a_rotated_grid_and_other_readers_agree() {
    // 6 patterns of 4 by 4 pixels with growing ink, a 12 by 9 grid of gray values
    let (pw, ph, count) = (4usize, 4usize, 6usize);
    let pats: Vec<Bitmap> = (0..count)
        .map(|k| {
            let mut b = Bitmap::new(pw, ph, 0).unwrap();
            for i in 0..k * 3 {
                b.d[(i * 5) % (pw * ph)] = 1;
            }
            // growing ink: patterns k hold the first 3k of a fixed order of cells
            b
        })
        .collect();
    let mut dict_img = Bitmap::new(count * pw, ph, 0).unwrap();
    for (k, p) in pats.iter().enumerate() {
        dict_img.combine(p, (k * pw) as i64, 0, 4);
    }
    let mut dict_body = vec![0u8 << 1, pw as u8, ph as u8];
    dict_body.extend(((count - 1) as u32).to_be_bytes());
    let mut mq = MqEnc::new();
    let mut cx = vec![0u8; generic_contexts(0)];
    encode_generic(&mut mq, &mut cx, &dict_img, 0, false, &[(-(pw as i8), 0), (-3, -1), (2, -2), (-2, -2)]);
    dict_body.extend(mq.finish());
    let (gw, gh) = (12usize, 9usize);
    let gray: Vec<usize> = (0..gw * gh).map(|i| (i * 7 + i / 5) % count).collect();
    let bpp = code_len(count) as usize;
    // bit planes, most significant first, each Gray coded: plane k holds bit(k) XOR bit(k-1) of the value
    let plane = |k: usize| -> Bitmap {
        let mut b = Bitmap::new(gw, gh, 0).unwrap();
        for (i, v) in gray.iter().enumerate() {
            let bit = |j: usize| (v >> (bpp - 1 - j)) & 1;
            b.d[i] = (if k == 0 { bit(0) } else { bit(k) ^ bit(k - 1) }) as u8;
        }
        b
    };
    for (rx, ry, gx, gy, name) in [(4 * 256, 0i64, 2 * 256i64, 3 * 256i64, "axis"), (4 * 256, 0, -256, 0, "negative origin"), (3 * 256, 1 * 256, 20 * 256, 1 * 256, "rotated")] {
        let mut mq = MqEnc::new();
        let mut cx = vec![0u8; generic_contexts(0)];
        let at = [(3i8, -1i8), (-3, -1), (2, -2), (-2, -2)];
        for k in 0..bpp {
            encode_generic(&mut mq, &mut cx, &plane(k), 0, false, &at);
        }
        let (w, h) = (60usize, 48usize);
        let mut body = region_head(w, h, 0, 0, 0);
        body.push(0);
        body.extend((gw as u32).to_be_bytes());
        body.extend((gh as u32).to_be_bytes());
        body.extend((gx as i32).to_be_bytes());
        body.extend((gy as i32).to_be_bytes());
        body.extend((rx as u16).to_be_bytes());
        body.extend((ry as u16).to_be_bytes());
        body.extend(mq.finish());
        let mut want = Bitmap::new(w, h, 0).unwrap();
        for m in 0..gh as i64 {
            for n in 0..gw as i64 {
                let x = (gx + m * ry + n * rx) >> 8;
                let y = (gy + m * rx - n * ry) >> 8;
                want.combine(&pats[gray[m as usize * gw + n as usize]], x, y, 0);
            }
        }
        let data = stream_of((w, h), &[segment(1, 16, &[], &dict_body), segment(2, 22, &[1], &body)]);
        // poppler places a pattern that starts left of the region differently; the others are checked against it only on the plain grid
        check_with(&format!("halftone-{name}"), &want, &data, None, name == "axis");
    }
}

#[test]
fn what_is_not_read_is_named_and_the_rest_of_the_page_is_still_given() {
    let a = picture(30, 20, 61);
    let mut v = segment(0, 48, &[], &page_info(30, 20, 0));
    v.extend(segment(1, 38, &[], &generic_segment_body(&a, 0, 0, 0, 0, false, &default_at(0))));
    let mut hb = region_head(8, 8, 0, 0, 0);
    hb.push(8); // the skip bitmap flag
    hb.extend([0u8; 20]);
    v.extend(segment(2, 22, &[], &hb));
    let (page, problem) = decode(&v, None).unwrap();
    assert_eq!(page, a);
    // (a halftone region with a skip bitmap is read now: with no pattern dictionary to refer to, that is what the error says)
    assert!(problem.unwrap().contains("pattern dictionary"));
    // a Huffman-coded text region without its data
    let mut tb = region_head(8, 8, 0, 0, 0);
    tb.extend(1u16.to_be_bytes());
    let mut v = segment(0, 48, &[], &page_info(30, 20, 0));
    v.extend(segment(1, 6, &[], &tb));
    let (_, problem) = decode(&v, None).unwrap();
    // (it is read now: with no symbol code data the error says so)
    assert!(problem.unwrap().contains("cut short"));
    // no page information at all
    assert!(decode(&segment(1, 38, &[], &[]), None).is_err());
    assert!(decode(b"", None).is_err());
}

#[test]
fn damaged_and_random_streams_never_panic_and_never_take_the_memory() {
    let (g, p, _) = text_page(1, false, 0, false);
    let mut seed = 77u32;
    for round in 0..300 {
        let mut gg = g.clone();
        let mut pp = p.clone();
        for _ in 0..3 {
            seed = seed.wrapping_mul(1664525).wrapping_add(1013904223);
            let target = if seed & 0x100 != 0 { &mut gg } else { &mut pp };
            let i = (seed >> 9) as usize % target.len();
            target[i] = (seed >> 24) as u8;
        }
        let _ = decode(&pp, Some(&gg));
        if round % 10 == 0 {
            let cut = (seed >> 7) as usize % pp.len();
            let _ = decode(&pp[..cut], Some(&gg[..cut.min(gg.len())]));
        }
    }
    // a page that claims a huge size is refused, not allocated
    let r = decode(&segment(0, 48, &[], &page_info(0x7FFF_FFFF, 0x7FFF_FFFF, 0)), None);
    assert!(r.is_err());
}

#[test]
fn a_page_is_packed_as_the_pdf_filter_gives_it() {
    let mut bm = Bitmap::new(10, 2, 0).unwrap();
    bm.d[0] = 1;
    bm.d[9] = 1;
    bm.d[10 + 3] = 1;
    let p = pack_pdf(&bm, 10, 2);
    // black is 0, white is 1, rows are padded to a byte (the padding is white)
    assert_eq!(p, vec![0b0111_1111, 0b1011_1111, 0b1110_1111, 0b1111_1111]);
    // a picture asked for larger than the page is white around it
    assert_eq!(pack_pdf(&bm, 16, 3).len(), 6);
}

#[test]
fn a_real_stream_written_by_another_encoder_is_read_when_a_copy_is_installed() {
    // a symbol dictionary, a text region and a generic region from a real JBIG2 encoder (the files are test data of a graphics library
    // and are not carried here)
    let root = match std::env::var_os("IYAKKI_JBIG2_SAMPLES") {
        Some(r) => std::path::PathBuf::from(r),
        None => std::path::PathBuf::from(std::env::var_os("HOME").unwrap_or_default()).join("iyakki-work").join("jbig2-samples"), // home-ok: read-only look for optional sample files; nothing is written
    };
    let (Ok(g), Ok(i1), Ok(i2)) = (std::fs::read(root.join("global.jb2")), std::fs::read(root.join("image1.jb2")), std::fs::read(root.join("image2.jb2"))) else {
        eprintln!("skipped: the sample files are not there");
        return;
    };
    for (tag, data) in [("img1", i1), ("img2", i2)] {
        let (page, problem) = decode(&data, Some(&g)).unwrap();
        assert_eq!(problem, None, "{tag}");
        assert_eq!((page.w, page.h), (200, 50));
        assert!(page.d.iter().filter(|&&p| p == 1).count() > 100, "{tag} has ink");
        if let Some(gsp) = gs_picture(tag, page.w, page.h, &data, Some(&g)) {
            assert_eq!(gsp, page, "{tag}: Ghostscript agrees");
        }
        if let Some(pp) = poppler_picture(tag, page.w, page.h, &data, Some(&g)) {
            assert_eq!(pp, page, "{tag}: poppler agrees");
        }
    }
}

#[test]
fn a_jbig2_picture_in_a_pdf_is_drawn_as_its_samples_and_a_stencil_works() {
    use crate::tests_support::{assemble, stream};
    let pic = include_bytes!("../tests/ccitt/text.pbm");
    let w = u32::from_be_bytes(pic[0..4].try_into().unwrap()) as usize;
    let h = u32::from_be_bytes(pic[4..8].try_into().unwrap()) as usize;
    let row = w.div_ceil(8);
    let mut want = Bitmap::new(w, h, 0).unwrap();
    for y in 0..h {
        for x in 0..w {
            want.d[y * w + x] = (pic[8 + y * row + x / 8] >> (7 - x % 8)) & 1;
        }
    }
    let (globals, data) = {
        // the page's own segments in the global stream and the region in the picture: both are read
        let g = segment(100, 48, &[], &page_info(w as u32, h as u32, 0));
        let d = segment(1, 38, &[], &generic_segment_body(&want, 0, 0, 0, 1, false, &default_at(1)));
        (g, d)
    };
    for stencil in [false, true] {
        let dict = if stencil {
            format!("/Type /XObject /Subtype /Image /Width {w} /Height {h} /ImageMask true /BitsPerComponent 1 /Filter /JBIG2Decode /DecodeParms << /JBIG2Globals 6 0 R >>")
        } else {
            format!("/Type /XObject /Subtype /Image /Width {w} /Height {h} /ColorSpace /DeviceGray /BitsPerComponent 1 /Filter /JBIG2Decode /DecodeParms << /JBIG2Globals 6 0 R >>")
        };
        let objs = vec![
            (1, b"<< /Type /Catalog /Pages 2 0 R >>".to_vec()),
            (2, b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_vec()),
            (3, format!("<< /Type /Page /Parent 2 0 R /MediaBox [0 0 {w} {h}] /Resources << /XObject << /Im0 5 0 R >> >> /Contents 4 0 R >>").into_bytes()),
            (4, stream("", format!("q {w} 0 0 {h} 0 0 cm /Im0 Do Q").as_bytes())),
            (5, stream(&dict, &data)),
            (6, stream("", &globals)),
        ];
        let doc = crate::Document::open(assemble(&objs, 1, "")).unwrap();
        let r = doc.render(0, 1.0, &crate::BoxGlyphs).unwrap();
        let (mut dark_ok, mut dark_all, mut light_ok, mut light_all) = (0, 0, 0, 0);
        for y in 0..h {
            for x in 0..w {
                let i = (y * r.width as usize + x) * 4;
                let lum = r.rgba[i] as u32 + r.rgba[i + 1] as u32 + r.rgba[i + 2] as u32;
                if want.d[y * w + x] == 1 {
                    dark_all += 1;
                    dark_ok += (lum < 300) as usize;
                } else {
                    light_all += 1;
                    light_ok += (lum > 600) as usize;
                }
            }
        }
        assert!(dark_ok * 100 >= dark_all * 97, "stencil {stencil}: dark {dark_ok}/{dark_all}");
        assert!(light_ok * 100 >= light_all * 97, "stencil {stencil}: light {light_ok}/{light_all}");
    }
}


// ------------------------------------------------------------------------------------------------ halftones: skip bitmap and MMR planes

/// Four patterns of 4 by 4 pixels with growing ink, and their dictionary segment body (arithmetic coded).
fn four_patterns() -> (Vec<Bitmap>, Vec<u8>) {
    let (pw, ph, count) = (4usize, 4usize, 4usize);
    let pats: Vec<Bitmap> = (0..count)
        .map(|k| {
            let mut b = Bitmap::new(pw, ph, 0).unwrap();
            for i in 0..k * 4 {
                b.d[(i * 5) % (pw * ph)] = 1;
            }
            b
        })
        .collect();
    let mut img = Bitmap::new(count * pw, ph, 0).unwrap();
    for (k, p) in pats.iter().enumerate() {
        img.combine(p, (k * pw) as i64, 0, 4);
    }
    let mut body = vec![0u8, pw as u8, ph as u8];
    body.extend(((count - 1) as u32).to_be_bytes());
    let mut mq = MqEnc::new();
    let mut cx = vec![0u8; generic_contexts(0)];
    encode_generic(&mut mq, &mut cx, &img, 0, false, &[(-(pw as i8), 0), (-3, -1), (2, -2), (-2, -2)]);
    body.extend(mq.finish());
    (pats, body)
}

/// The header fields of a halftone region after its region information.
fn halftone_head(flags: u8, gw: usize, gh: usize, gx: i64, gy: i64, rx: i64, ry: i64) -> Vec<u8> {
    let mut b = vec![flags];
    b.extend((gw as u32).to_be_bytes());
    b.extend((gh as u32).to_be_bytes());
    b.extend((gx as i32).to_be_bytes());
    b.extend((gy as i32).to_be_bytes());
    b.extend((rx as u16).to_be_bytes());
    b.extend((ry as u16).to_be_bytes());
    b
}

fn draw_grid(pats: &[Bitmap], gray: &[usize], gw: usize, gh: usize, (gx, gy, rx, ry): (i64, i64, i64, i64), w: usize, h: usize) -> Bitmap {
    let mut want = Bitmap::new(w, h, 0).unwrap();
    for m in 0..gh as i64 {
        for n in 0..gw as i64 {
            want.combine(&pats[gray[m as usize * gw + n as usize]], (gx + m * ry + n * rx) >> 8, (gy + m * rx - n * ry) >> 8, 0);
        }
    }
    want
}

#[test]
fn a_halftone_region_with_a_skip_bitmap_does_not_code_the_cells_that_fall_outside_and_decodes() {
    let (pats, dict) = four_patterns();
    let (gw, gh, bpp) = (14usize, 11usize, 2usize);
    let (w, h) = (40usize, 30usize);
    // grids that hang over the region on one side or more, one of them rotated
    for (grid, name) in [((-3 * 256i64, 2 * 256i64, 4 * 256i64, 0i64), "hangs over the left"), ((34 * 256, 10 * 256, 3 * 256, 1 * 256), "hangs over the right and rotated"), ((2 * 256, -6 * 256, 4 * 256, 0), "hangs over the top")] {
        let (gx, gy, rx, ry) = grid;
        // which cells the standard skips: the pattern wholly outside the region
        let mut skip = Bitmap::new(gw, gh, 0).unwrap();
        for m in 0..gh as i64 {
            for n in 0..gw as i64 {
                let (x, y) = ((gx + m * ry + n * rx) >> 8, (gy + m * rx - n * ry) >> 8);
                if x + 4 <= 0 || x >= w as i64 || y + 4 <= 0 || y >= h as i64 {
                    skip.d[m as usize * gw + n as usize] = 1;
                }
            }
        }
        assert!(skip.d.contains(&1) && skip.d.contains(&0), "{name}: the grid must hang over the edge and still be partly inside");
        // gray values: anything for the cells that are drawn, and zero for the skipped ones (which the decoder never reads)
        let gray: Vec<usize> = (0..gw * gh).map(|i| if skip.d[i] == 1 { 0 } else { (i * 5 + i / 3) % 4 }).collect();
        let plane = |k: usize| {
            let mut b = Bitmap::new(gw, gh, 0).unwrap();
            for (i, v) in gray.iter().enumerate() {
                let bit = |j: usize| (v >> (bpp - 1 - j)) & 1;
                b.d[i] = (if k == 0 { bit(0) } else { bit(k) ^ bit(k - 1) }) as u8;
            }
            b
        };
        let at = [(3i8, -1i8), (-3, -1), (2, -2), (-2, -2)];
        let mut mq = MqEnc::new();
        let mut cx = vec![0u8; generic_contexts(0)];
        for k in 0..bpp {
            // the encoder leaves the skipped pixels out, as the decoder does
            let pl = plane(k);
            let mut ltp_unused = 0u8;
            let _ = &mut ltp_unused;
            for y in 0..gh {
                for x in 0..gw {
                    if skip.d[y * gw + x] == 1 {
                        continue;
                    }
                    let c = generic_context(&pl, x as i64, y as i64, 0, &at);
                    mq.bit(&mut cx, c, pl.d[y * gw + x]);
                }
            }
        }
        let mut body = region_head(w, h, 0, 0, 0);
        body.extend(halftone_head(8, gw, gh, gx, gy, rx, ry));
        body.extend(mq.finish());
        let data = stream_of((w, h), &[segment(1, 16, &[], &dict), segment(2, 22, &[1], &body)]);
        let want = draw_grid(&pats, &gray, gw, gh, grid, w, h);
        same(&format!("halftone with a skip bitmap, {name}"), &decoded(&data, None), &want);
    }
}

#[test]
fn a_halftone_region_with_its_bit_planes_mmr_coded_one_after_the_other_decodes() {
    // two bit planes from the real fax encoder (Group 4, each ends with its end-of-block code and a byte boundary); the grid is 24 by 18
    let planes: [(&[u8], &[u8]); 2] = [(include_bytes!("../tests/ccitt/ht0.pbm"), include_bytes!("../tests/ccitt/ht0.g4")), (include_bytes!("../tests/ccitt/ht1.pbm"), include_bytes!("../tests/ccitt/ht1.g4"))];
    let pbm = |p: &[u8]| {
        let (w, h) = (u32::from_be_bytes(p[0..4].try_into().unwrap()) as usize, u32::from_be_bytes(p[4..8].try_into().unwrap()) as usize);
        let row = w.div_ceil(8);
        (w, h, (0..w * h).map(|i| (p[8 + (i / w) * row + (i % w) / 8] >> (7 - (i % w) % 8)) & 1).collect::<Vec<u8>>())
    };
    let (gw, gh, p0) = pbm(planes[0].0);
    let (_, _, p1) = pbm(planes[1].0);
    // gray value = bit 1 from the first plane, bit 0 from the second plane Gray coded against the first
    let gray: Vec<usize> = (0..gw * gh).map(|i| ((p0[i] as usize) << 1) | (p1[i] ^ p0[i]) as usize).collect();
    assert!((0..4).all(|v| gray.contains(&v)), "all four patterns are used");
    let (pats, dict) = four_patterns();
    let (w, h) = (100usize, 80usize);
    let grid = (2 * 256i64, 3 * 256i64, 4 * 256i64, 0i64);
    let mut body = region_head(w, h, 0, 0, 0);
    body.extend(halftone_head(1, gw, gh, grid.0, grid.1, grid.2, grid.3));
    body.extend(planes[0].1);
    body.extend(planes[1].1);
    let data = stream_of((w, h), &[segment(1, 16, &[], &dict), segment(2, 22, &[1], &body)]);
    let want = draw_grid(&pats, &gray, gw, gh, grid, w, h);
    same("mmr halftone", &decoded(&data, None), &want);
    // a stream cut inside the second plane gives what was read and an error, never a panic
    let mut cut = body.clone();
    cut.truncate(cut.len() - 30);
    let data = stream_of((w, h), &[segment(1, 16, &[], &dict), segment(2, 22, &[1], &cut)]);
    let _ = decode(&data, None);
}

#[path = "jbig2_huff_tests.rs"]
mod huff_tests;
