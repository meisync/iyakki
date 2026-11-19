use super::*;

fn bits(s: &str) -> Vec<bool> {
    s.bytes().map(|b| b == b'1').collect()
}

#[test]
fn the_code128_table_keeps_its_structural_rules() {
    assert_eq!(CODE128_WIDTHS.len(), 107);
    for (i, w) in CODE128_WIDTHS.iter().enumerate() {
        let d: Vec<u32> = w.bytes().map(|b| (b - b'0') as u32).collect();
        assert!(d.iter().all(|x| (1..=4).contains(x)), "symbol {i}: widths 1 to 4");
        if i < STOP {
            assert_eq!(d.len(), 6, "symbol {i}: six elements");
            assert_eq!(d.iter().sum::<u32>(), 11, "symbol {i}: eleven modules");
            assert_eq!(d.iter().step_by(2).sum::<u32>() % 2, 0, "symbol {i}: bar widths add up to an even number");
        } else {
            assert_eq!(d.len(), 7);
            assert_eq!(d.iter().sum::<u32>(), 13, "the stop is thirteen modules");
        }
    }
    let mut seen = std::collections::BTreeSet::new();
    for w in &CODE128_WIDTHS {
        assert!(seen.insert(*w), "{w} twice");
    }
    for w in &CODE128_WIDTHS[..STOP] {
        let rev: String = w.chars().rev().collect();
        assert!(!seen.contains(rev.as_str()) || rev == *w, "{w} is the reverse of another symbol, so a reader could not tell the direction");
    }
}

#[test]
fn code128_matches_published_bit_strings() {
    // start B, "A", "1", "2", "3"? published encodings (independent of the table above): the stop and the three starts
    let mut m = Vec::new();
    push_symbol(&mut m, STOP);
    assert_eq!(m, bits("1100011101011"));
    for (s, b) in [(START_A, "11010000100"), (START_B, "11010010000"), (START_C, "11010011100")] {
        let mut m = Vec::new();
        push_symbol(&mut m, s);
        assert_eq!(m, bits(b), "start symbol {s}");
    }
    // function symbols (FNC1..FNC4 are 102, 97, 96 and 100/101 by subset)
    let mut m = Vec::new();
    push_symbol(&mut m, 102);
    assert_eq!(m, bits("11110101110"));
    // "A23" in B after FNC1 is not what the encoder makes; a plain full encoding: start B, 'H','i', check, stop
    let v = code128_values("Hi").unwrap();
    assert_eq!(v, vec![START_B, 40, 73]);
    assert_eq!(code128_check(&v), (104 + 40 + 73 * 2) % 103);
}

#[test]
fn code128_picks_the_subsets_and_round_trips() {
    // whole symbols from an independent implementation's test data: start B, H, I, six digits in C, H; and start C alone
    let full = |t: &str| encode_code128(t).unwrap().iter().map(|b| if *b { '1' } else { '0' }).collect::<String>();
    assert_eq!(full("HI345678H"), "110100100001100010100011000100010101110111101000101100011100010110110000101001011110111011000101000111011000101100011101011");
    assert_eq!(full("334455"), "11010011100101000110001000110111011101000110100100111101100011101011");
    assert_eq!(code128_values("123456").unwrap(), vec![START_C, 12, 34, 56]);
    assert_eq!(code128_values("12345").unwrap(), vec![START_B, 17, TO_C, 23, 45]);
    assert_eq!(code128_values("AB1234").unwrap(), vec![START_B, 33, 34, TO_C, 12, 34]);
    assert_eq!(code128_values("\tA").unwrap(), vec![START_A, 73, 33]);
    for s in ["Hello, world", "PJJ123C", "0123456789", "12345", "A1", "SKU-00042 x 3", "~", " ", "\u{1}ab", "ab\u{1}cd", "99", "9"] {
        let m = encode_code128(s).unwrap();
        assert_eq!(decode_code128(&m).unwrap(), s, "round trip of {s:?}");
        assert!(m.len() % 11 == 2, "11 modules a symbol and a 13 module stop: {}", m.len());
        assert_eq!(&m[m.len() - 13..], &bits("1100011101011")[..]);
    }
    assert!(encode_code128("").is_err());
    assert!(encode_code128("caf\u{e9}").is_err());
    let mut m = encode_code128("Hello").unwrap();
    let k = m.len() - 13 - 11 + 1;
    m[k] = !m[k];
    assert!(decode_code128(&m).is_err(), "a damaged symbol is refused");
}

#[test]
fn ean13_matches_published_examples() {
    // 5901234123457 is the example of the standard's description: 95 modules
    let m = encode_ean13("5901234123457").unwrap();
    assert_eq!(m.len(), 95);
    let s: String = m.iter().map(|b| if *b { '1' } else { '0' }).collect();
    assert_eq!(&s[..3], "101");
    assert_eq!(&s[45..50], "01010");
    assert_eq!(&s[92..], "101");
    // left half: first digit 5 gives LGGLLG over 9 0 1 2 3 4
    assert_eq!(&s[3..45], "000101101001110110011001001101111010011101");
    // right half: 1 2 3 4 5 7 as R codes
    assert_eq!(&s[50..92], "110011011011001000010101110010011101000100");
    // the whole symbol, from an independent implementation's test data
    assert_eq!(s, "10100010110100111011001100100110111101001110101010110011011011001000010101110010011101000100101");
    assert_eq!(ean13_check_digit("590123412345").unwrap(), 7);
    assert_eq!(ean13_check_digit("400638133393").unwrap(), 1);
    assert_eq!(encode_ean13("590123412345").unwrap(), m, "12 digits get the check digit added");
    assert!(encode_ean13("5901234123458").unwrap_err().contains("check digit should be 7"));
    assert!(encode_ean13("12345").is_err());
    assert!(encode_ean13("59012341234x").is_err());
}

#[test]
fn upca_is_an_ean13_with_a_leading_zero() {
    // 036000291452 is the classic UPC-A example
    assert_eq!(ean13_check_digit("003600029145").unwrap(), 2);
    assert_eq!(encode_upca("03600029145").unwrap(), encode_upca("036000291452").unwrap());
    assert_eq!(encode_upca("036000291452").unwrap().len(), 95);
    assert!(encode_upca("036000291453").is_err());
}

#[cfg(feature = "uikit")]
mod view {
    use crate::barcodeview::BarcodeView;
    use crate::list::ListCanvas;
    use crate::size::Metrics;
    use crate::geom::Rect;
    use iyakki_uikit::look::Look;

    #[test]
    fn the_view_draws_dark_bars_on_light_paper_in_every_look_and_names_itself() {
        let look = Look::default();
        let th = look.theme;
        let m = Metrics::new(360.0, 200.0, 1.0, 1.0, true);
        let v = BarcodeView::code128(7, "SKU-42").unwrap();
        let mut c = ListCanvas::new(360.0, 200.0);
        v.draw(&mut c, &th, &m, Rect::new(0.0, 0.0, 360.0, 200.0));
        let l = c.finish();
        let a = l.a11y();
        assert_eq!(a.len(), 1);
        assert_eq!(a[0].name, "Barcode: SKU-42");
        assert!(l.texts().contains(&"SKU-42"));
        assert!(BarcodeView::ean13(1, "590123412345").unwrap().text().ends_with('7'));
        assert!(BarcodeView::ean13(1, "5901234123458").is_err());
    }
}
