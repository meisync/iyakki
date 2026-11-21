//! The fax decoder against pictures coded by the real encoder (libtiff through Pillow; see `tests/ccitt/gen.py`): every picture is decoded
//! from its committed Group 4 and Group 3 stream and compared sample by sample with the picture itself.

use crate::ccitt::{decode, Params};

fn pbm(b: &[u8]) -> (usize, usize, Vec<u8>) {
    let w = u32::from_be_bytes(b[0..4].try_into().unwrap()) as usize;
    let h = u32::from_be_bytes(b[4..8].try_into().unwrap()) as usize;
    (w, h, b[8..].to_vec())
}

macro_rules! fixture {
    ($n:literal) => {
        (include_bytes!(concat!("../tests/ccitt/", $n, ".pbm")).as_slice(), include_bytes!(concat!("../tests/ccitt/", $n, ".g4")).as_slice(), include_bytes!(concat!("../tests/ccitt/", $n, ".g3")).as_slice())
    };
}

fn all() -> Vec<(&'static str, &'static [u8], &'static [u8], &'static [u8])> {
    let t = fixture!("text");
    let r = fixture!("runs");
    let n = fixture!("noise");
    let b = fixture!("blank");
    vec![("text", t.0, t.1, t.2), ("runs", r.0, r.1, r.2), ("noise", n.0, n.1, n.2), ("blank", b.0, b.1, b.2)]
}

#[test]
fn group_4_pictures_made_by_the_real_encoder_decode_to_the_same_samples() {
    for (name, pic, g4, _) in all() {
        let (w, h, want) = pbm(pic);
        let p = Params { k: -1, columns: w, rows: h, black_is_1: true, byte_align: false };
        let (got, rows) = decode(g4, &p, 1 << 26);
        assert_eq!(rows, h, "{name}: rows decoded");
        assert_eq!(got, want, "{name}: samples");
    }
}

#[test]
fn group_3_one_dimensional_pictures_with_end_of_line_codes_decode_to_the_same_samples() {
    for (name, pic, _, g3) in all() {
        let (w, h, want) = pbm(pic);
        let p = Params { k: 0, columns: w, rows: h, black_is_1: true, byte_align: false };
        let (got, rows) = decode(g3, &p, 1 << 26);
        assert_eq!(rows, h, "{name}: rows decoded");
        assert_eq!(got, want, "{name}: samples");
    }
}

#[test]
fn black_is_one_off_gives_the_inverse_and_damage_keeps_the_rows_read_so_far() {
    let (name, pic, g4, _) = all()[0];
    let (w, h, want) = pbm(pic);
    let p = Params { k: -1, columns: w, rows: h, black_is_1: false, byte_align: false };
    let (got, _) = decode(g4, &p, 1 << 26);
    let row_bytes = w.div_ceil(8);
    for y in 0..h {
        for x in 0..w {
            let a = (want[y * row_bytes + x / 8] >> (7 - x % 8)) & 1;
            let b = (got[y * row_bytes + x / 8] >> (7 - x % 8)) & 1;
            assert_eq!(a ^ b, 1, "{name} {x},{y}");
        }
    }
    // a cut stream: the first rows are right, the rest is white and the size is still the page's
    let p = Params { k: -1, columns: w, rows: h, black_is_1: true, byte_align: false };
    let (cut, rows) = decode(&g4[..g4.len() / 2], &p, 1 << 26);
    assert!(rows > 0 && rows < h);
    assert_eq!(cut.len(), want.len());
    assert_eq!(&cut[..row_bytes * (rows - 1)], &want[..row_bytes * (rows - 1)]);
}

#[test]
fn any_bytes_decode_without_panic_in_every_mode() {
    let mut seed = 12345u32;
    for len in [0usize, 1, 5, 40, 300, 2000] {
        let data: Vec<u8> = (0..len)
            .map(|_| {
                seed = seed.wrapping_mul(1664525).wrapping_add(1013904223);
                (seed >> 24) as u8
            })
            .collect();
        for k in [-1i64, 0, 4] {
            for align in [false, true] {
                let p = Params { k, columns: 77, rows: 0, black_is_1: false, byte_align: align };
                let (out, rows) = decode(&data, &p, 1 << 20);
                assert_eq!(out.len(), rows * 10);
            }
        }
    }
}

#[test]
fn a_ccitt_picture_in_a_pdf_is_drawn_as_its_samples_and_a_stencil_works() {
    use crate::tests_support::{assemble, stream};
    let (pic, g4, _) = fixture!("text");
    let (w, h, want) = pbm(pic);
    for stencil in [false, true] {
        let dict = if stencil {
            format!("/Type /XObject /Subtype /Image /Width {w} /Height {h} /ImageMask true /BitsPerComponent 1 /Filter /CCITTFaxDecode /DecodeParms << /K -1 /Columns {w} /Rows {h} >>")
        } else {
            format!("/Type /XObject /Subtype /Image /Width {w} /Height {h} /ColorSpace /DeviceGray /BitsPerComponent 1 /Filter /CCITTFaxDecode /DecodeParms << /K -1 /Columns {w} /Rows {h} >>")
        };
        let objs = vec![
            (1, b"<< /Type /Catalog /Pages 2 0 R >>".to_vec()),
            (2, b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_vec()),
            (3, format!("<< /Type /Page /Parent 2 0 R /MediaBox [0 0 {w} {h}] /Resources << /XObject << /Im0 5 0 R >> >> /Contents 4 0 R >>").into_bytes()),
            (4, stream("", format!("q {w} 0 0 {h} 0 0 cm /Im0 Do Q").as_bytes())),
            (5, stream(&dict, g4)),
        ];
        let doc = crate::Document::open(assemble(&objs, 1, "")).unwrap();
        let r = doc.render(0, 1.0, &crate::BoxGlyphs).unwrap();
        // the sample at a black pixel of the picture is dark, at a white one light
        let row_bytes = w.div_ceil(8);
        let mut dark_ok = 0;
        let mut dark_all = 0;
        let mut light_ok = 0;
        let mut light_all = 0;
        for y in 0..h {
            for x in 0..w {
                let black = (want[y * row_bytes + x / 8] >> (7 - x % 8)) & 1 == 1;
                let i = (y * r.width as usize + x) * 4;
                let lum = r.rgba[i] as u32 + r.rgba[i + 1] as u32 + r.rgba[i + 2] as u32;
                if black {
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

#[test]
fn group_3_mixed_pictures_made_by_the_real_encoder_decode_to_the_same_samples_for_any_k_above_zero() {
    // the same encoder (libtiff through Pillow, T4Options bit 0), the same pictures: each end-of-line is followed by a tag bit that
    // says whether the next row is coded on its own (one dimensional) or against the row above (two dimensional)
    let fixtures: [(&str, &[u8], &[u8]); 4] = [
        ("text", include_bytes!("../tests/ccitt/text.pbm"), include_bytes!("../tests/ccitt/text.g3m")),
        ("runs", include_bytes!("../tests/ccitt/runs.pbm"), include_bytes!("../tests/ccitt/runs.g3m")),
        ("noise", include_bytes!("../tests/ccitt/noise.pbm"), include_bytes!("../tests/ccitt/noise.g3m")),
        ("blank", include_bytes!("../tests/ccitt/blank.pbm"), include_bytes!("../tests/ccitt/blank.g3m")),
    ];
    for (name, pic, g3m) in fixtures {
        let (w, h, want) = pbm(pic);
        for k in [1, 4, 100] {
            let p = Params { k, columns: w, rows: h, black_is_1: true, byte_align: false };
            let (got, rows) = decode(g3m, &p, 1 << 26);
            assert_eq!(rows, h, "{name}, K {k}: rows decoded");
            assert_eq!(got, want, "{name}, K {k}: samples");
        }
        // read as one dimensional the tag bits are not rows: the picture is not the same (the stream really is the mixed form)
        let p = Params { k: 0, columns: w, rows: h, black_is_1: true, byte_align: false };
        let (flat, _) = decode(g3m, &p, 1 << 26);
        assert!(name == "blank" || flat != want, "{name}: the stream is mixed, a one dimensional read must differ");
        // damage never panics
        let mut bad = g3m.to_vec();
        for i in (0..bad.len()).step_by(7) {
            bad[i] ^= 0x5A;
        }
        let p = Params { k: 4, columns: w, rows: h, black_is_1: true, byte_align: false };
        let _ = decode(&bad, &p, 1 << 26);
    }
}
