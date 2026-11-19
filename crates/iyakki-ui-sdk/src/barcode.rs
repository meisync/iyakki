//! Barcode encoders, in-house and without a dependency: Code 128 (subsets A, B and C, with the check symbol and the stop) and EAN-13 / UPC-A.
//! They turn text into modules (`true` = a bar, `false` = a space, one entry per module); [`crate::barcodeview`] draws them. The 107-entry
//! Code 128 table is written below as bar and space widths and is checked in the tests against its structural rules (every symbol is 11
//! modules, starts with a bar, its bar widths add up to an even number, no two are equal, none is the reverse of another) and, once, against an
//! independent table; encoding is checked against published bit strings and a decoder round trip.
//!
//! Code 128: [`encode_code128`] picks the subsets itself: runs of four or more digits go in subset C (two digits a symbol), control characters in
//! subset A, the rest in subset B. Text must be ASCII (0 to 126).
//! EAN-13: [`encode_ean13`] takes 12 digits (the check digit is added) or 13 (the check digit is verified). [`encode_upca`] takes 11 or 12.

/// Bar and space widths of symbols 0 to 102 (the data symbols), 103 to 105 (Start A, B, C) and 106 (Stop, seven elements).
pub const CODE128_WIDTHS: [&str; 107] = [
    "212222", "222122", "222221", "121223", "121322", "131222", "122213", "122312", "132212", "221213", "221312", "231212", "112232", "122132", "122231",
    "113222", "123122", "123221", "223211", "221132", "221231", "213212", "223112", "312131", "311222", "321122", "321221", "312212", "322112", "322211",
    "212123", "212321", "232121", "111323", "131123", "131321", "112313", "132113", "132311", "211313", "231113", "231311", "112133", "112331", "132131",
    "113123", "113321", "133121", "313121", "211331", "231131", "213113", "213311", "213131", "311123", "311321", "331121", "312113", "312311", "332111",
    "314111", "221411", "431111", "111224", "111422", "121124", "121421", "141122", "141221", "112214", "112412", "122114", "122411", "142112", "142211",
    "241211", "221114", "413111", "241112", "134111", "111242", "121142", "121241", "114212", "124112", "124211", "411212", "421112", "421211", "212141",
    "214121", "412121", "111143", "111341", "131141", "114113", "114311", "411113", "411311", "113141", "114131", "311141", "411131", "211412", "211214",
    "211232", "2331112",
];
const START_A: usize = 103;
const START_B: usize = 104;
const START_C: usize = 105;
const STOP: usize = 106;
const TO_C: usize = 99;
const TO_B: usize = 100;
const TO_A: usize = 101;

#[derive(Clone, Copy, PartialEq)]
enum Set {
    A,
    B,
    C,
}

fn push_symbol(out: &mut Vec<bool>, symbol: usize) {
    let mut bar = true;
    for w in CODE128_WIDTHS[symbol].bytes() {
        for _ in 0..(w - b'0') {
            out.push(bar);
        }
        bar = !bar;
    }
}

fn value_in(set: Set, c: u8) -> Option<usize> {
    match set {
        Set::A if c < 96 => Some(if c < 32 { c as usize + 64 } else { c as usize - 32 }),
        Set::B if (32..127).contains(&c) => Some(c as usize - 32),
        _ => None,
    }
}

/// The symbol values of `text` (start, data, switches), without the check symbol and the stop.
fn code128_values(text: &str) -> Result<Vec<usize>, String> {
    let b = text.as_bytes();
    if b.is_empty() {
        return Err("Code 128 needs some text".into());
    }
    if let Some(bad) = b.iter().find(|c| **c > 126) {
        return Err(format!("Code 128 here takes ASCII up to 126; byte {bad} is outside"));
    }
    let mut v = Vec::new();
    let mut set: Option<Set> = None;
    let mut i = 0;
    while i < b.len() {
        let run = b[i..].iter().take_while(|c| c.is_ascii_digit()).count();
        if run >= 4 || (run >= 2 && i == 0 && run == b.len()) {
            let mut k = i;
            if run % 2 == 1 {
                // an odd run: its first digit goes in B (or the current subset), the rest in pairs in C
                let s = match set {
                    Some(Set::A) => Set::A,
                    _ => Set::B,
                };
                if set != Some(s) {
                    v.push(if set.is_none() { if s == Set::A { START_A } else { START_B } } else if s == Set::A { TO_A } else { TO_B });
                    set = Some(s);
                }
                v.push(value_in(s, b[k]).ok_or("a digit is not encodable")?);
                k += 1;
            }
            if set != Some(Set::C) {
                v.push(if set.is_none() { START_C } else { TO_C });
                set = Some(Set::C);
            }
            while k < i + run {
                v.push((b[k] - b'0') as usize * 10 + (b[k + 1] - b'0') as usize);
                k += 2;
            }
            i += run;
            continue;
        }
        // a single character in A (controls) or B (the rest); stay in the current subset when it can hold the character
        let want = if b[i] < 32 { Set::A } else { Set::B };
        let s = match set {
            Some(Set::A) if value_in(Set::A, b[i]).is_some() => Set::A,
            Some(Set::B) if value_in(Set::B, b[i]).is_some() => Set::B,
            _ => want,
        };
        if set != Some(s) {
            v.push(if set.is_none() { if s == Set::A { START_A } else { START_B } } else if s == Set::A { TO_A } else { TO_B });
            set = Some(s);
        }
        v.push(value_in(s, b[i]).ok_or_else(|| format!("character {} cannot be encoded", b[i]))?);
        i += 1;
    }
    Ok(v)
}

/// The check symbol of a list of symbol values (the first is the start symbol): the weighted sum modulo 103.
pub fn code128_check(values: &[usize]) -> usize {
    values.iter().enumerate().map(|(i, v)| v * i.max(1)).sum::<usize>() % 103
}

/// Code 128 modules of `text`: start, data, check symbol and stop (the quiet zone is the drawer's).
pub fn encode_code128(text: &str) -> Result<Vec<bool>, String> {
    let mut v = code128_values(text)?;
    let check = code128_check(&v);
    v.push(check);
    v.push(STOP);
    let mut out = Vec::new();
    for s in v {
        push_symbol(&mut out, s);
    }
    Ok(out)
}

/// Read Code 128 modules back to text (for tests and for checking what was drawn). Refuses a wrong length, an unknown symbol or a wrong check.
pub fn decode_code128(modules: &[bool]) -> Result<String, String> {
    let mut symbols = Vec::new();
    let mut i = 0;
    while i + 11 <= modules.len() {
        let w = &modules[i..i + 11];
        let sym = (0..STOP).find(|s| {
            let mut m = Vec::new();
            push_symbol(&mut m, *s);
            m == w
        });
        match sym {
            Some(s) => {
                symbols.push(s);
                i += 11;
            }
            None => break,
        }
    }
    let mut stop = Vec::new();
    push_symbol(&mut stop, STOP);
    if modules.len() != i + 13 || modules[i..] != stop[..] {
        return Err("no stop symbol where one is expected".into());
    }
    if symbols.len() < 3 {
        return Err("too short".into());
    }
    let check = symbols.pop().unwrap_or(0);
    if code128_check(&symbols) != check {
        return Err("the check symbol is wrong".into());
    }
    let mut set = match symbols[0] {
        START_A => Set::A,
        START_B => Set::B,
        START_C => Set::C,
        _ => return Err("it does not begin with a start symbol".into()),
    };
    let mut out = String::new();
    for &s in &symbols[1..] {
        match (set, s) {
            // in subset C the values 0 to 99 are digit pairs (99 included); 100 and 101 switch to B and A
            (Set::C, 0..=99) => out.push_str(&format!("{s:02}")),
            (Set::C, TO_B) | (Set::A, TO_B) => set = Set::B,
            (Set::C, TO_A) | (Set::B, TO_A) => set = Set::A,
            (Set::A, TO_C) | (Set::B, TO_C) => set = Set::C,
            (Set::A, 0..=63) => out.push((s as u8 + 32) as char),
            (Set::A, 64..=95) => out.push((s as u8 - 64) as char),
            (Set::B, 0..=94) => out.push((s as u8 + 32) as char),
            _ => return Err(format!("symbol {s} is not data in this subset")),
        }
    }
    Ok(out)
}

const EAN_L: [&str; 10] = ["0001101", "0011001", "0010011", "0111101", "0100011", "0110001", "0101111", "0111011", "0110111", "0001011"];
const EAN_FIRST: [&str; 10] = ["LLLLLL", "LLGLGG", "LLGGLG", "LLGGGL", "LGLLGG", "LGGLLG", "LGGGLL", "LGLGLG", "LGLGGL", "LGGLGL"];

/// The EAN-13 check digit of the first 12 digits.
pub fn ean13_check_digit(first12: &str) -> Result<u8, String> {
    let d: Vec<u32> = first12.chars().map(|c| c.to_digit(10).ok_or_else(|| format!("`{c}` is not a digit"))).collect::<Result<_, _>>()?;
    if d.len() != 12 {
        return Err(format!("EAN-13 needs 12 digits before the check digit, got {}", d.len()));
    }
    let sum: u32 = d.iter().enumerate().map(|(i, v)| if i % 2 == 0 { *v } else { v * 3 }).sum();
    Ok(((10 - sum % 10) % 10) as u8)
}

/// EAN-13 modules (95): 12 digits get their check digit added, 13 digits are verified.
pub fn encode_ean13(digits: &str) -> Result<Vec<bool>, String> {
    let digits = digits.trim();
    let full = match digits.len() {
        12 => format!("{digits}{}", ean13_check_digit(digits)?),
        13 => {
            let c = ean13_check_digit(&digits[..12])?;
            if digits.as_bytes()[12] - b'0' != c {
                return Err(format!("the check digit should be {c}"));
            }
            digits.to_string()
        }
        n => return Err(format!("EAN-13 takes 12 or 13 digits, got {n}")),
    };
    let d: Vec<usize> = full.bytes().map(|b| (b - b'0') as usize).collect();
    let bits = |s: &str, out: &mut Vec<bool>| out.extend(s.bytes().map(|b| b == b'1'));
    let mut out = Vec::new();
    bits("101", &mut out);
    let parity = EAN_FIRST[d[0]].as_bytes();
    for (k, &digit) in d[1..7].iter().enumerate() {
        let l = EAN_L[digit];
        if parity[k] == b'L' {
            bits(l, &mut out);
        } else {
            // G is the reverse of the complement of L
            let g: String = l.chars().rev().map(|c| if c == '1' { '0' } else { '1' }).collect();
            bits(&g, &mut out);
        }
    }
    bits("01010", &mut out);
    for &digit in &d[7..13] {
        let r: String = EAN_L[digit].chars().map(|c| if c == '1' { '0' } else { '1' }).collect();
        bits(&r, &mut out);
    }
    bits("101", &mut out);
    Ok(out)
}

/// UPC-A modules: 11 digits (the check digit is added) or 12 (verified); a UPC-A is an EAN-13 with a leading zero.
pub fn encode_upca(digits: &str) -> Result<Vec<bool>, String> {
    let d = digits.trim();
    if d.len() != 11 && d.len() != 12 {
        return Err(format!("UPC-A takes 11 or 12 digits, got {}", d.len()));
    }
    encode_ean13(&format!("0{d}"))
}

#[cfg(test)]
#[path = "barcode_tests.rs"]
mod tests;
