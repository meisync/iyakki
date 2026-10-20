//! The text frontend of the Indic voices: what must happen to a story's text before the models see it.
//!
//! The AI4Bharat FastPitch models read **characters** (no phonemes, no transliteration inside the model). Each language has its own
//! character set (Tamil: its letters, the digit `0`, a few punctuation marks; no Latin letters at all), the model's own cleaner only
//! lower-cases, turns `; : -` into `, , space`, and drops `< > ( ) [ ] "`, and every other character is **silently dropped**. So
//! this module does what a person reading aloud does:
//!
//! 1. symbols and punctuation are made the ones the models know (curly quotes, dashes, the danda `।`, ellipsis, ...);
//! 2. native-script digits and ASCII digits become words (`numbers`), with decimals, percent, rupee sign and thousands commas;
//! 3. text in another Indic script is mapped letter by letter into the language's script when the letter exists there;
//! 4. Latin words are cut out as segments for an English voice when one is installed, else written approximately in the
//!    language's own script (`transliterate_latin`), which is a rough, rule-based sound-alike, not a dictionary;
//! 5. the result is cut into pieces of a length the model handles well, with the pause that should follow each piece.

use super::numbers;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Script {
    Latin,
    Devanagari,
    Bengali,
    Gurmukhi,
    Gujarati,
    Odia,
    Tamil,
    Telugu,
    Kannada,
    Malayalam,
    Other,
}

impl Script {
    /// First code point of the script's Unicode block (the Indic blocks share one layout).
    pub fn base(self) -> Option<u32> {
        Some(match self {
            Script::Devanagari => 0x0900,
            Script::Bengali => 0x0980,
            Script::Gurmukhi => 0x0A00,
            Script::Gujarati => 0x0A80,
            Script::Odia => 0x0B00,
            Script::Tamil => 0x0B80,
            Script::Telugu => 0x0C00,
            Script::Kannada => 0x0C80,
            Script::Malayalam => 0x0D00,
            _ => return None,
        })
    }
}

pub fn script_of(c: char) -> Script {
    let u = c as u32;
    match u {
        0x0900..=0x097F => Script::Devanagari,
        0x0980..=0x09FF => Script::Bengali,
        0x0A00..=0x0A7F => Script::Gurmukhi,
        0x0A80..=0x0AFF => Script::Gujarati,
        0x0B00..=0x0B7F => Script::Odia,
        0x0B80..=0x0BFF => Script::Tamil,
        0x0C00..=0x0C7F => Script::Telugu,
        0x0C80..=0x0CFF => Script::Kannada,
        0x0D00..=0x0D7F => Script::Malayalam,
        _ if c.is_ascii_alphabetic() => Script::Latin,
        0xC0..=0x24F if c.is_alphabetic() => Script::Latin,
        _ => Script::Other,
    }
}

/// The script a language is written in.
pub fn lang_script(lang: &str) -> Script {
    match lang {
        "ta" => Script::Tamil,
        "te" => Script::Telugu,
        "kn" => Script::Kannada,
        "ml" => Script::Malayalam,
        "hi" | "mr" | "brx" | "raj" => Script::Devanagari,
        "bn" | "as" | "mni" => Script::Bengali,
        "gu" => Script::Gujarati,
        "pa" => Script::Gurmukhi,
        "or" => Script::Odia,
        _ => Script::Latin,
    }
}

/// A native digit (any Indic script) as its value.
fn native_digit(c: char) -> Option<char> {
    let u = c as u32;
    for base in [0x0966u32, 0x09E6, 0x0A66, 0x0AE6, 0x0B66, 0x0BE6, 0x0C66, 0x0CE6, 0x0D66] {
        if (base..base + 10).contains(&u) {
            return char::from_digit(u - base, 10);
        }
    }
    None
}

/// Letters of one Indic script written in another. The blocks are laid out alike, so a letter is moved by the difference of the
/// block starts; letters the target script lacks are dropped, except in Tamil, which has no voiced or aspirated consonants and
/// writes them as the plain ones.
pub fn map_indic_char(c: char, from: Script, to: Script) -> Option<char> {
    let (fb, tb) = (from.base()?, to.base()?);
    if from == to {
        return Some(c);
    }
    let dev = 0x0900 + (c as u32 - fb);
    // a letter that every Indic block shares: consonants 0x15..=0x39, vowels 0x05..=0x14, signs 0x3E..=0x4C, virama 0x4D
    let off = dev - 0x0900;
    let ok = matches!(off, 0x01..=0x03 | 0x05..=0x39 | 0x3E..=0x4D);
    if !ok {
        return None;
    }
    if to == Script::Tamil {
        let t = tamil_of(off)?;
        return char::from_u32(0x0B80 + t);
    }
    let cp = tb + off;
    let ch = char::from_u32(cp)?;
    (script_of(ch) == to).then_some(ch)
}

/// Offset in the Tamil block of the letter that stands for the Devanagari-block offset `off`.
fn tamil_of(off: u32) -> Option<u32> {
    Some(match off {
        0x01..=0x03 => return None,
        // vowels: अ आ इ ई उ ऊ ऋ(drop) ऌ(drop) ऍ ऎ ए ऐ ऑ ऒ ओ औ
        0x05..=0x0B => off,
        0x0C | 0x0D | 0x11 => return None,
        0x0E..=0x10 | 0x12..=0x14 => off,
        // consonants: ka kha ga gha nga
        0x15 | 0x16 | 0x17 | 0x18 => 0x15,
        0x19 => 0x19,
        // ca cha ja jha nya
        0x1A | 0x1B => 0x1A,
        0x1C | 0x1D => 0x1C,
        0x1E => 0x1E,
        // tta ttha dda ddha nna
        0x1F | 0x20 | 0x21 | 0x22 => 0x1F,
        0x23 => 0x23,
        // ta tha da dha na
        0x24 | 0x25 | 0x26 | 0x27 => 0x24,
        0x28 => 0x28,
        // pa pha ba bha ma
        0x2A | 0x2B | 0x2C | 0x2D => 0x2A,
        0x2E => 0x2E,
        // ya ra la lla (devanagari 0x33) -> Tamil ள, ḻ none
        0x2F => 0x2F,
        0x30 => 0x30,
        0x32 => 0x32,
        0x33 => 0x33,
        0x35 => 0x35,
        0x36 | 0x37 => 0x37,
        0x38 => 0x38,
        0x39 => 0x39,
        // signs: aa i ii u uu (ṛ) e ai o au and virama
        0x3E | 0x3F | 0x40 | 0x41 | 0x42 => off,
        0x46 | 0x47 | 0x48 | 0x4A | 0x4B | 0x4C | 0x4D => off,
        0x45 | 0x49 => return None,
        _ => return None,
    })
}

// ------------------------------------------------------------------------------------------------------------ symbols

/// Replace the symbols a story is full of with ones the models know. Line breaks stay (the pieces are cut there).
fn tidy_symbols(text: &str) -> String {
    let mut o = String::with_capacity(text.len());
    for c in text.chars() {
        match c {
            '\u{2018}' | '\u{2019}' | '\u{201A}' | '\u{2032}' => o.push('\''),
            '\u{201C}' | '\u{201D}' | '\u{201E}' | '\u{00AB}' | '\u{00BB}' | '\u{2033}' => {}
            '\u{2026}' => o.push_str(". "),
            '\u{2013}' | '\u{2014}' | '\u{2015}' => o.push_str(", "),
            '\u{2010}' | '\u{2011}' | '\u{2212}' => o.push('-'),
            '\u{0964}' | '\u{0965}' | '|' | '\u{3002}' => o.push('.'),
            '\u{061F}' => o.push('?'),
            '\u{00A0}' | '\u{2009}' | '\u{202F}' | '\t' => o.push(' '),
            '\r' => {}
            '\u{FEFF}' | '\u{200B}' => {}
            '*' | '#' | '_' | '~' | '^' | '`' | '\\' | '=' | '<' | '>' | '[' | ']' | '{' | '}' | '@' | '\u{2022}' | '\u{00B7}' | '\u{25CF}' | '\u{25AA}' => o.push(' '),
            '/' => o.push(' '),
            '\u{00A1}' | '\u{00BF}' => {}
            c if c.is_control() && c != '\n' => o.push(' '),
            c if (c as u32) >= 0x1F000 || (0x2600..=0x27BF).contains(&(c as u32)) || (0xFE00..=0xFE0F).contains(&(c as u32)) => {} // emoji and symbols
            c => o.push(c),
        }
    }
    o
}

/// Digits, decimals, percent and rupee signs as words (`lang` decides the words).
fn spell_numbers(text: &str, lang: &str) -> String {
    let chars: Vec<char> = text.chars().map(|c| native_digit(c).unwrap_or(c)).collect();
    let mut o = String::with_capacity(text.len() + 32);
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        // a currency sign before a number: the rupee word goes after it
        let rupee_before = (c == '\u{20B9}' || (c == 'R' && chars.get(i + 1) == Some(&'s') && chars.get(i + 2) == Some(&'.'))) && {
            let mut j = i + if c == 'R' { 3 } else { 1 };
            while chars.get(j) == Some(&' ') {
                j += 1;
            }
            chars.get(j).is_some_and(char::is_ascii_digit)
        };
        if rupee_before {
            i += if c == 'R' { 3 } else { 1 };
            while chars.get(i) == Some(&' ') {
                i += 1;
            }
            let (words, next) = read_number(&chars, i, lang);
            o.push_str(&words);
            o.push(' ');
            o.push_str(numbers::rupee_word(lang));
            o.push(' ');
            i = next;
            continue;
        }
        if c.is_ascii_digit() {
            let (words, mut next) = read_number(&chars, i, lang);
            o.push(' ');
            o.push_str(&words);
            o.push(' ');
            if chars.get(next) == Some(&'%') {
                o.push_str(numbers::percent_word(lang));
                o.push(' ');
                next += 1;
            }
            i = next;
            continue;
        }
        if c == ':' && i > 0 && chars[i - 1].is_ascii_digit() && chars.get(i + 1).is_some_and(char::is_ascii_digit) {
            o.push(' ');
        } else {
            o.push(c);
        }
        i += 1;
    }
    o
}

/// Read the number that starts at `i`: thousands commas (`1,234` or `12,34,567`), a decimal part, and words for it all.
fn read_number(chars: &[char], start: usize, lang: &str) -> (String, usize) {
    let mut i = start;
    let mut int = String::new();
    while i < chars.len() {
        if chars[i].is_ascii_digit() {
            int.push(chars[i]);
            i += 1;
        } else if chars[i] == ',' && chars.get(i + 1).is_some_and(char::is_ascii_digit) && chars.get(i + 2).is_some_and(char::is_ascii_digit) && !int.is_empty() {
            // a group separator only when two or three digits follow and then a non-digit or another comma
            let mut j = i + 1;
            let mut n = 0;
            while chars.get(j).is_some_and(char::is_ascii_digit) {
                j += 1;
                n += 1;
            }
            if (n == 2 || n == 3) && !chars.get(j).is_some_and(|c| c.is_ascii_digit()) {
                i += 1;
            } else {
                break;
            }
        } else {
            break;
        }
    }
    let mut out = numbers::digits_to_words(&int, lang);
    if chars.get(i) == Some(&'.') && chars.get(i + 1).is_some_and(char::is_ascii_digit) {
        let mut frac = String::new();
        let mut j = i + 1;
        while chars.get(j).is_some_and(char::is_ascii_digit) {
            frac.push(chars[j]);
            j += 1;
        }
        let names = numbers::digit_names(lang);
        if let Some(names) = names {
            out.push(' ');
            out.push_str(numbers::point_word(lang));
            for d in frac.chars() {
                out.push(' ');
                out.push_str(names[d.to_digit(10).unwrap_or(0) as usize]);
            }
            i = j;
        }
    }
    (out, i)
}

/// Two-part vowel signs written as their parts (and Malayalam chillu letters written with ZWJ) in the single characters the models were
/// trained with: the character sets hold `ொ` `ോ` `ൻ` but not the parts, so the parts would be read wrongly or dropped.
pub fn compose(text: &str, lang: &str) -> String {
    let pairs: &[(&str, &str)] = match lang {
        "ta" => &[("\u{0BC6}\u{0BBE}", "\u{0BCA}"), ("\u{0BC7}\u{0BBE}", "\u{0BCB}"), ("\u{0BC6}\u{0BD7}", "\u{0BCC}")],
        "ml" => &[
            ("\u{0D46}\u{0D3E}", "\u{0D4A}"),
            ("\u{0D47}\u{0D3E}", "\u{0D4B}"),
            ("\u{0D46}\u{0D57}", "\u{0D4C}"),
            ("\u{0D23}\u{0D4D}\u{200D}", "\u{0D7A}"),
            ("\u{0D28}\u{0D4D}\u{200D}", "\u{0D7B}"),
            ("\u{0D30}\u{0D4D}\u{200D}", "\u{0D7C}"),
            ("\u{0D32}\u{0D4D}\u{200D}", "\u{0D7D}"),
            ("\u{0D33}\u{0D4D}\u{200D}", "\u{0D7E}"),
            ("\u{0D15}\u{0D4D}\u{200D}", "\u{0D7F}"),
        ],
        "kn" => &[("\u{0CC6}\u{0CC2}", "\u{0CCA}")],
        "bn" | "as" | "mni" => &[("\u{09C7}\u{09BE}", "\u{09CB}"), ("\u{09C7}\u{09D7}", "\u{09CC}")],
        _ => return text.to_string(),
    };
    let mut t = text.to_string();
    for (from, to) in pairs {
        t = t.replace(from, to);
    }
    t
}

/// Symbols and numbers done: the text the models can take (Latin words are still in it).
pub fn normalise(text: &str, lang: &str) -> String {
    let t = compose(&tidy_symbols(text), lang);
    let t = spell_numbers(&t, lang);
    // the same line breaks, no runs of spaces, no space before , . ? !
    let mut lines: Vec<String> = Vec::new();
    for line in t.split('\n') {
        let mut l = line.split_whitespace().collect::<Vec<_>>().join(" ");
        for p in [" ,", " .", " ?", " !"] {
            while l.contains(p) {
                l = l.replace(p, &p[1..]);
            }
        }
        lines.push(l);
    }
    lines.join("\n").trim().to_string()
}

// ------------------------------------------------------------------------------------------------------------ Latin words

fn consonant_dev(w: &[char], i: usize) -> Option<(u32, usize)> {
    // returns (Devanagari-block offset of the consonant, letters used)
    let c = w[i];
    let n = w.get(i + 1).copied();
    let n2 = w.get(i + 2).copied();
    Some(match (c, n) {
        ('s', Some('h')) => (0x36, 2),
        ('c', Some('h')) => (0x1A, 2),
        ('k', Some('h')) => (0x16, 2),
        ('g', Some('h')) => (0x18, 2),
        ('j', Some('h')) => (0x1D, 2),
        ('t', Some('h')) => (0x25, 2),
        ('d', Some('h')) => (0x27, 2),
        ('p', Some('h')) => (0x2B, 2),
        ('b', Some('h')) => (0x2D, 2),
        ('z', Some('h')) => (0x1D, 2),
        ('w', Some('h')) => (0x35, 2),
        ('c', Some('k')) => (0x15, 2),
        ('q', Some('u')) => (0x15, 2),
        ('n', Some('g')) if n2.is_none() || !is_vowel(n2.unwrap_or('a')) => (0x19, 2),
        ('c', Some('e' | 'i' | 'y')) => (0x38, 1),
        ('c', _) => (0x15, 1),
        ('k', _) | ('q', _) => (0x15, 1),
        ('g', _) => (0x17, 1),
        ('j', _) => (0x1C, 1),
        ('t', _) => (0x1F, 1),
        ('d', _) => (0x21, 1),
        ('p', _) => (0x2A, 1),
        ('b', _) => (0x2C, 1),
        ('m', _) => (0x2E, 1),
        ('n', _) => (0x28, 1),
        ('l', _) => (0x32, 1),
        ('r', _) => (0x30, 1),
        ('s', _) => (0x38, 1),
        ('h', _) => (0x39, 1),
        ('f', _) => (0x2B, 1),
        ('v', _) | ('w', _) => (0x35, 1),
        ('y', _) if i > 0 || n.is_some_and(is_vowel) => (0x2F, 1),
        ('z', _) => (0x1C, 1),
        ('x', _) => (0x15, 1),
        _ => return None,
    })
}

fn is_vowel(c: char) -> bool {
    matches!(c, 'a' | 'e' | 'i' | 'o' | 'u')
}

/// Vowel at `i`: (independent letter offset, matra offset, letters used).
fn vowel_dev(w: &[char], i: usize, short_vowels: bool) -> Option<(u32, u32, usize)> {
    let c = w[i];
    let n = w.get(i + 1).copied();
    Some(match (c, n) {
        ('a', Some('a')) => (0x06, 0x3E, 2),
        ('e', Some('e')) | ('e', Some('a')) | ('i', Some('e')) => (0x08, 0x40, 2),
        ('o', Some('o')) => (0x0A, 0x42, 2),
        ('o', Some('a')) => (0x13, 0x4B, 2),
        ('a', Some('i')) | ('a', Some('y')) => (0x10, 0x48, 2),
        ('a', Some('u')) | ('a', Some('w')) | ('o', Some('u')) | ('o', Some('w')) => (0x14, 0x4C, 2),
        ('o', Some('i')) | ('o', Some('y')) => (0x13, 0x4B, 2),
        ('a', _) => (0x05, 0x3E, 1),
        ('e', _) if short_vowels => (0x0E, 0x46, 1),
        ('e', _) => (0x0F, 0x47, 1),
        ('i', _) => (0x07, 0x3F, 1),
        ('o', _) if short_vowels => (0x12, 0x4A, 1),
        ('o', _) => (0x13, 0x4B, 1),
        ('u', _) => (0x09, 0x41, 1),
        ('y', _) => (0x07, 0x3F, 1),
        _ => return None,
    })
}

/// A Latin word written approximately in `lang`'s script. Rough sound-alike by rules (a trailing silent `e` dropped, `a` as
/// long ā after a consonant, clusters joined by virama); good enough to be understood, not a dictionary.
pub fn transliterate_latin(word: &str, lang: &str) -> String {
    let to = lang_script(lang);
    let Some(tb) = to.base() else { return String::new() };
    let mut w: Vec<char> = word.chars().filter(|c| c.is_ascii_alphabetic() || *c == '\'').map(|c| c.to_ascii_lowercase()).filter(|c| *c != '\'').collect();
    if w.len() > 3 && w.last() == Some(&'e') && !is_vowel(w[w.len() - 2]) {
        w.pop(); // silent e: "game"
    }
    // abstract output as Devanagari-block offsets, then moved to the target block
    let mut out: Vec<u32> = Vec::new();
    let mut after_consonant = false;
    let mut i = 0;
    while i < w.len() {
        if let Some((off, used)) = consonant_dev(&w, i) {
            if after_consonant {
                out.push(0x4D); // virama joins the cluster
            }
            out.push(off);
            if off == 0x15 && w[i] == 'x' {
                out.push(0x4D);
                out.push(0x38);
            }
            if w[i] == 'q' && w.get(i + 1) == Some(&'u') {
                out.push(0x4D);
                out.push(0x35);
            }
            after_consonant = true;
            i += used;
            continue;
        }
        if let Some((ind, matra, used)) = vowel_dev(&w, i, matches!(to, Script::Tamil | Script::Telugu | Script::Kannada | Script::Malayalam)) {
            if after_consonant {
                out.push(matra);
            } else {
                out.push(ind);
            }
            after_consonant = false;
            i += used;
            continue;
        }
        i += 1;
    }
    // Dravidian scripts pronounce a final bare consonant: mark it with the virama; North Indian scripts drop the schwa.
    if after_consonant && matches!(to, Script::Tamil | Script::Telugu | Script::Kannada | Script::Malayalam) {
        out.push(0x4D);
    }
    let mut s = String::new();
    for off in out {
        let ch = if to == Script::Tamil {
            tamil_of(off).and_then(|t| char::from_u32(0x0B80 + t))
        } else {
            char::from_u32(tb + off).filter(|c| script_of(*c) == to)
        };
        if let Some(c) = ch {
            s.push(c);
        }
    }
    s
}

/// Letter names, for words in capitals (USB, TV): spelt as the English names in the language's script.
pub fn spell_letters(word: &str, lang: &str) -> String {
    const NAMES: [&str; 26] = ["ei", "bee", "see", "dee", "ee", "ef", "jee", "eych", "ai", "jay", "kay", "el", "em", "en", "o", "pee", "kyoo", "ar", "es", "tee", "yoo", "vee", "dabalyoo", "eks", "wai", "zed"];
    word.chars().filter(char::is_ascii_alphabetic).map(|c| transliterate_latin(NAMES[(c.to_ascii_lowercase() as u8 - b'a') as usize], lang)).collect::<Vec<_>>().join(" ")
}

/// A piece of text for one voice.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Seg {
    pub lang: String,
    pub text: String,
}

fn is_word_char(c: char) -> bool {
    c.is_alphanumeric() || c == '\'' || c == '\u{200c}' || c == '\u{200d}' || (0x0900..=0x0DFF).contains(&(c as u32))
}

/// Cut normalised text into segments by voice. Text in `lang`'s script (and other Indic scripts, mapped into it) stays in `lang`;
/// Latin words go to `en` when `english` is true and are written in the language's script otherwise.
pub fn segments(text: &str, lang: &str, english: bool) -> Vec<Seg> {
    let target = lang_script(lang);
    if target == Script::Latin {
        return if text.trim().is_empty() { Vec::new() } else { vec![Seg { lang: lang.to_string(), text: text.to_string() }] };
    }
    let mut segs: Vec<Seg> = Vec::new();
    let push = |segs: &mut Vec<Seg>, l: &str, t: &str| {
        if t.is_empty() {
            return;
        }
        match segs.last_mut() {
            Some(last) if last.lang == l => last.text.push_str(t),
            _ => segs.push(Seg { lang: l.to_string(), text: t.to_string() }),
        }
    };
    let chars: Vec<char> = text.chars().collect();
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        let sc = script_of(c);
        if sc == Script::Latin {
            let mut j = i;
            while j < chars.len() && (script_of(chars[j]) == Script::Latin || (chars[j] == '\'' && chars.get(j + 1).is_some_and(|c| script_of(*c) == Script::Latin))) {
                j += 1;
            }
            let word: String = chars[i..j].iter().collect();
            if english {
                push(&mut segs, "en", &word);
            } else {
                let caps = word.len() >= 2 && word.len() <= 5 && word.chars().all(|c| c.is_ascii_uppercase());
                let t = if caps { spell_letters(&word, lang) } else { transliterate_latin(&word, lang) };
                push(&mut segs, lang, &t);
            }
            i = j;
        } else if sc != Script::Other && sc != target {
            let m = map_indic_char(c, sc, target);
            if let Some(m) = m {
                push(&mut segs, lang, &m.to_string());
            }
            i += 1;
        } else {
            // native text and punctuation. In an English run, punctuation and spaces stay with it so the voice breathes right.
            let in_en = segs.last().is_some_and(|s| s.lang == "en") && !is_word_char(c) && c != '\n';
            let next_is_latin = chars[i + 1..].iter().find(|c| !c.is_whitespace() && !matches!(**c, ',' | '.' | '?' | '!')).is_some_and(|c| script_of(*c) == Script::Latin);
            let l = if in_en && next_is_latin { "en" } else { lang };
            push(&mut segs, l, &c.to_string());
            i += 1;
        }
    }
    // spaces only: nothing to say
    segs.retain(|s| s.text.chars().any(|c| c.is_alphanumeric() || (0x0900..=0x0DFF).contains(&(c as u32))));
    segs
}

// ------------------------------------------------------------------------------------------------------------ pieces

/// A stretch of text for one request, and the quiet that follows it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Piece {
    pub text: String,
    pub pause_ms: u32,
}

/// How long one piece may be, in characters. The models were trained on sentences up to a few hundred characters; the long
/// words of Tamil, Malayalam, Telugu and Kannada make a piece long in characters and long in sound, so they are cut sooner.
pub fn max_chars(lang: &str) -> usize {
    match lang {
        "ta" | "ml" => 110,
        "te" | "kn" => 130,
        _ => 170,
    }
}

pub const PAUSE_SENTENCE: u32 = 320;
pub const PAUSE_QUESTION: u32 = 360;
pub const PAUSE_COMMA: u32 = 150;
pub const PAUSE_SPLIT: u32 = 70;
pub const PAUSE_LINE: u32 = 520;

fn end_pause(s: &str) -> u32 {
    match s.trim_end().chars().last() {
        Some('?') => PAUSE_QUESTION,
        Some('.' | '!') => PAUSE_SENTENCE,
        Some(',') => PAUSE_COMMA,
        _ => PAUSE_SENTENCE,
    }
}

/// Split `s` at the clause marks (`,`), keeping the comma with the clause before it.
fn clauses(s: &str) -> Vec<String> {
    let mut v = Vec::new();
    let mut cur = String::new();
    for c in s.chars() {
        cur.push(c);
        if c == ',' {
            v.push(std::mem::take(&mut cur));
        }
    }
    if !cur.trim().is_empty() {
        v.push(cur);
    }
    v
}

/// Cut `s` (one sentence) into pieces of at most `max` characters: at commas where possible, else at the space nearest the middle
/// of what is left. A word is never cut.
fn cut_long(s: &str, max: usize) -> Vec<String> {
    if s.chars().count() <= max {
        return vec![s.trim().to_string()];
    }
    let mut out: Vec<String> = Vec::new();
    let mut cur = String::new();
    for cl in clauses(s) {
        let cl = cl.trim().to_string();
        if cl.chars().count() > max {
            if !cur.is_empty() {
                out.push(std::mem::take(&mut cur));
            }
            out.extend(cut_at_spaces(&cl, max));
        } else if cur.is_empty() {
            cur = cl;
        } else if cur.chars().count() + 1 + cl.chars().count() <= max {
            cur.push(' ');
            cur.push_str(&cl);
        } else {
            out.push(std::mem::take(&mut cur));
            cur = cl;
        }
    }
    if !cur.is_empty() {
        out.push(cur);
    }
    out
}

fn cut_at_spaces(s: &str, max: usize) -> Vec<String> {
    let words: Vec<&str> = s.split_whitespace().collect();
    let total: usize = s.chars().count();
    let parts = total.div_ceil(max).max(1);
    let target = total.div_ceil(parts);
    let mut out = Vec::new();
    let mut cur = String::new();
    for w in words {
        if !cur.is_empty() && cur.chars().count() + 1 + w.chars().count() > max.min(target + target / 4) {
            out.push(std::mem::take(&mut cur));
        }
        if !cur.is_empty() {
            cur.push(' ');
        }
        cur.push_str(w);
    }
    if !cur.is_empty() {
        out.push(cur);
    }
    out
}

/// Cut normalised text into pieces with their pauses. A line break ends a piece with a longer pause; sentences end at `. ? !`;
/// long sentences are cut (see [`cut_long`]); a very short piece is joined to the one after it.
pub fn pieces(text: &str, max: usize) -> Vec<Piece> {
    let mut out: Vec<Piece> = Vec::new();
    for line in text.split('\n') {
        let line = line.trim();
        if line.is_empty() {
            if let Some(last) = out.last_mut() {
                last.pause_ms = last.pause_ms.max(PAUSE_LINE);
            }
            continue;
        }
        // a new line after a piece: a little longer pause than between sentences
        if let Some(last) = out.last_mut() {
            last.pause_ms = last.pause_ms.max(PAUSE_LINE.min(last.pause_ms + 160));
        }
        let mut sentences: Vec<String> = Vec::new();
        let mut cur = String::new();
        let chars: Vec<char> = line.chars().collect();
        for (k, &c) in chars.iter().enumerate() {
            cur.push(c);
            let end = matches!(c, '.' | '?' | '!') && chars.get(k + 1).is_none_or(|n| n.is_whitespace());
            if end {
                sentences.push(std::mem::take(&mut cur));
            }
        }
        if !cur.trim().is_empty() {
            sentences.push(cur);
        }
        for s in sentences {
            let s = s.trim().to_string();
            if !s.chars().any(char::is_alphanumeric) {
                continue;
            }
            let parts = cut_long(&s, max);
            let n = parts.len();
            for (k, p) in parts.into_iter().enumerate() {
                let pause = if k + 1 == n { end_pause(&p) } else if p.trim_end().ends_with(',') { PAUSE_COMMA } else { PAUSE_SPLIT };
                out.push(Piece { text: p, pause_ms: pause });
            }
        }
    }
    // very short pieces ("Yes.") join the next one
    let mut merged: Vec<Piece> = Vec::new();
    let mut carry: Option<Piece> = None;
    for p in out {
        let p = match carry.take() {
            Some(c) => Piece { text: format!("{} {}", c.text, p.text), pause_ms: p.pause_ms },
            None => p,
        };
        if p.text.chars().filter(|c| c.is_alphanumeric() || (0x0900..=0x0DFF).contains(&(*c as u32))).count() < 4 {
            carry = Some(p);
        } else {
            merged.push(p);
        }
    }
    if let Some(c) = carry {
        match merged.last_mut() {
            Some(l) => {
                l.text.push(' ');
                l.text.push_str(&c.text);
            }
            None => merged.push(c),
        }
    }
    merged
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn symbols_become_the_ones_the_models_know() {
        assert_eq!(normalise("வணக்கம்… “நலமா”?", "ta"), "வணக்கம். நலமா?");
        assert_eq!(normalise("राम ने कहा।  अच्छा", "hi"), "राम ने कहा. अच्छा");
        assert_eq!(normalise("a\u{2014}b", "en"), "a, b");
        assert_eq!(normalise("ok 😀 *bold* #tag", "en"), "ok bold tag");
        assert_eq!(normalise("line one\r\n\r\nline two", "en"), "line one\n\nline two");
    }

    #[test]
    fn split_vowel_signs_and_chillu_letters_are_made_the_single_characters_of_the_models() {
        assert_eq!(compose("\u{0B95}\u{0BC6}\u{0BBE}", "ta"), "\u{0B95}\u{0BCA}");
        assert_eq!(compose("\u{0B95}\u{0BC7}\u{0BBE}", "ta"), "\u{0B95}\u{0BCB}");
        assert_eq!(compose("\u{0D15}\u{0D46}\u{0D3E}", "ml"), "\u{0D15}\u{0D4A}");
        assert_eq!(compose("\u{0D05}\u{0D35}\u{0D28}\u{0D4D}\u{200D}", "ml"), "\u{0D05}\u{0D35}\u{0D7B}", "ZWJ chillu to the atomic letter");
        assert_eq!(compose("\u{0C95}\u{0CC6}\u{0CC2}", "kn"), "\u{0C95}\u{0CCA}");
        assert_eq!(compose("\u{0995}\u{09C7}\u{09BE}", "bn"), "\u{0995}\u{09CB}");
        assert_eq!(compose("தமிழ்", "ta"), "தமிழ்", "already composed text is untouched");
        assert_eq!(compose("hello", "en"), "hello");
        assert_eq!(normalise("\u{0D15}\u{0D46}\u{0D3E}\u{2026}", "ml"), "\u{0D15}\u{0D4A}.");
    }

    #[test]
    fn tamil_and_native_digits_are_spelt() {
        assert_eq!(normalise("இன்று 25 டிகிரி", "ta"), "இன்று இருபத்தி ஐந்து டிகிரி");
        assert_eq!(normalise("௨௫ பேர்", "ta"), "இருபத்தி ஐந்து பேர்", "Tamil numerals");
        assert_eq!(normalise("१२ बजे", "hi"), "बारह बजे", "Devanagari numerals");
        assert_eq!(normalise("விலை 1,234 ரூபாய்", "ta"), "விலை ஆயிரத்து இருநூற்று முப்பத்தி நான்கு ரூபாய்");
        assert_eq!(normalise("50% தள்ளுபடி", "ta"), "ஐம்பது சதவீதம் தள்ளுபடி");
        assert_eq!(normalise("₹ 300", "hi"), "तीन सौ रुपये");
        assert_eq!(normalise("3.14", "en"), "three point one four");
        assert_eq!(normalise("7:30 மணி", "ta"), "ஏழு முப்பது மணி");
        assert_eq!(normalise("12,34,567", "en"), "one million two hundred thirty four thousand five hundred sixty seven");
        assert_eq!(normalise("1, 2, 3", "en"), "one, two, three", "a list is not a thousands group");
    }

    #[test]
    fn latin_words_go_to_english_or_are_written_in_the_script() {
        let text = "என் கணினியில் Wi-Fi வேலை செய்யவில்லை";
        let n = normalise(text, "ta");
        let with_en = segments(&n, "ta", true);
        assert!(with_en.iter().any(|s| s.lang == "en" && s.text.trim() == "Wi-Fi"), "{with_en:?}");
        assert!(with_en.iter().all(|s| s.lang == "en" || s.text.chars().all(|c| script_of(c) != Script::Latin)));
        let no_en = segments(&n, "ta", false);
        assert_eq!(no_en.len(), 1);
        assert!(no_en[0].text.chars().all(|c| c == ' ' || c == '-' || script_of(c) == Script::Tamil), "{:?}", no_en[0].text);
        let wifi = transliterate_latin("wifi", "ta");
        assert!(!wifi.is_empty() && wifi.chars().all(|c| script_of(c) == Script::Tamil));
        let usb = segments("USB கேபிள்", "ta", false);
        assert!(usb[0].text.chars().all(|c| c == ' ' || script_of(c) == Script::Tamil), "{usb:?}");
    }

    #[test]
    fn transliteration_stays_in_the_script_for_every_language() {
        for lang in ["ta", "te", "kn", "ml", "hi", "mr", "bn", "gu", "pa", "or"] {
            let want = lang_script(lang);
            for w in ["computer", "wifi", "school", "banana", "quick", "xylophone", "rhythm", "a", "e", "street", "phone", "chair", "queue"] {
                let t = transliterate_latin(w, lang);
                assert!(!t.is_empty(), "{lang} {w}");
                assert!(t.chars().all(|c| script_of(c) == want), "{lang} {w}: {t}");
            }
        }
        assert_eq!(transliterate_latin("net", "ta"), "நெட்");
    }

    #[test]
    fn another_indic_script_is_mapped_into_the_language() {
        let s = segments("राम", "ta", false);
        assert_eq!(s[0].text, "ராம");
        let s = segments("ఇది", "kn", false);
        assert_eq!(s[0].text, "ಇದಿ");
        // a Tamil text is left alone
        assert_eq!(segments("வணக்கம்", "ta", false)[0].text, "வணக்கம்");
    }

    #[test]
    fn pieces_follow_sentences_commas_and_the_length_limit() {
        let p = pieces("ஒரு ஊரிலே ஒரு சிறிய முயல் இருந்தது. அது தினமும் காட்டில் விளையாடியது.", 110);
        assert_eq!(p.len(), 2);
        assert_eq!(p[0].pause_ms, PAUSE_SENTENCE);
        assert_eq!(p[1].pause_ms, PAUSE_SENTENCE);
        let q = pieces("Is it raining? Yes.", 100);
        assert_eq!(q.len(), 1, "a very short piece joins the one before: {q:?}");
        // a long sentence is cut at commas, not in words
        let long = "alpha beta gamma delta, epsilon zeta eta theta, iota kappa lambda mu, nu xi omicron pi rho sigma tau upsilon phi chi psi omega.";
        let parts = pieces(long, 50);
        assert!(parts.len() >= 3, "{parts:?}");
        for x in &parts {
            assert!(x.text.chars().count() <= 60, "{x:?}");
            assert!(!x.text.starts_with(' ') && !x.text.ends_with(' '));
        }
        assert_eq!(parts.iter().map(|x| x.text.as_str()).collect::<Vec<_>>().join(" ").replace(',', ""), long.replace(',', ""));
        assert_eq!(parts[0].pause_ms, PAUSE_COMMA);
        assert_eq!(parts.last().unwrap().pause_ms, PAUSE_SENTENCE);
        // one huge word-run without commas is cut at spaces near the middle
        let words = vec!["word"; 80].join(" ");
        let cut = pieces(&words, 100);
        assert!(cut.len() >= 4 && cut.iter().all(|x| x.text.chars().count() <= 100), "{cut:?}");
        assert!(cut[..cut.len() - 1].iter().all(|x| x.pause_ms == PAUSE_SPLIT));
        // line breaks give the longer pause; empty text gives nothing
        let l = pieces("First line here.\n\nSecond line here.", 100);
        assert_eq!(l.len(), 2);
        assert!(l[0].pause_ms >= PAUSE_LINE);
        assert!(pieces("   \n  ", 100).is_empty() && pieces("... ,", 100).is_empty());
    }

    #[test]
    fn limits_follow_the_language() {
        assert!(max_chars("ta") < max_chars("hi"));
        assert!(pieces(&"ஆ".repeat(500), 110).iter().all(|p| p.text.chars().count() <= 500), "a single word longer than the limit is kept whole");
    }
}
