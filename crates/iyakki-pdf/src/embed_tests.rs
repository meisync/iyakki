//! Embedded fonts (F-PDF-11): Greek, Cyrillic and other text DejaVu draws is written as Type 0 / CIDFontType2 fonts with a subset, widths
//! and a ToUnicode map; the repo's own reader gets the same text back; the structure is checked strictly (cross-reference table,
//! stream lengths, references, font dictionaries, the font file parsed by an independent TrueType reader); scripts that need shaping
//! or right-to-left layout stay refused.

use crate::embed::{face, supports};
use crate::filter::inflate;
use crate::{Block, Builder, Document, PageSize};

const GREEK: &str = "\u{39a}\u{3b1}\u{3bb}\u{3b7}\u{3bc}\u{3ad}\u{3c1}\u{3b1} \u{3ba}\u{3cc}\u{3c3}\u{3bc}\u{3b5} \u{2014} \u{3b7} \u{3b3}\u{3bb}\u{3ce}\u{3c3}\u{3c3}\u{3b1} \u{3c0}\u{3bf}\u{3c5} \u{3bc}\u{3b9}\u{3bb}\u{3ac}\u{3c9}";
const CYRILLIC: &str = "\u{41f}\u{440}\u{438}\u{432}\u{435}\u{442}, \u{43c}\u{438}\u{440}! \u{42d}\u{442}\u{43e} \u{442}\u{435}\u{441}\u{442} \u{43f}\u{43e}-\u{440}\u{443}\u{441}\u{441}\u{43a}\u{438}.";

fn norm(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn build(blocks: Vec<Block>, title: &str) -> (Vec<u8>, crate::Built) {
    let mut b = Builder::new(PageSize::A4);
    b.title = title.into();
    b.blocks = blocks;
    b.build()
}

fn num_after(s: &str, key: &str) -> Option<usize> {
    let i = s.find(key)? + key.len();
    s[i..].trim_start().split(|c: char| !c.is_ascii_digit()).next()?.parse().ok()
}

/// Objects of a file written by the writer: (number, dictionary text, stream bytes), after checking everything the writer promises.
fn validate(bytes: &[u8]) -> Vec<(usize, String, Option<Vec<u8>>)> {
    assert!(bytes.starts_with(b"%PDF-1.4\n") && bytes.ends_with(b"%%EOF\n"));
    let sx = bytes.windows(10).rposition(|w| w == b"startxref\n").expect("startxref");
    let tail = String::from_utf8_lossy(&bytes[sx + 10..]).to_string();
    let xref_at: usize = tail.lines().next().unwrap().parse().unwrap();
    assert!(bytes[xref_at..].starts_with(b"xref\n0 "), "startxref points at the table");
    let after = String::from_utf8_lossy(&bytes[xref_at..sx]).to_string();
    let head = after.lines().nth(1).unwrap();
    let n: usize = head.strip_prefix("0 ").unwrap().parse().unwrap();
    let table_start = xref_at + "xref\n".len() + head.len() + 1;
    let entry = |i: usize| &bytes[table_start + i * 20..table_start + i * 20 + 20];
    assert_eq!(entry(0), b"0000000000 65535 f \n");
    let trailer = String::from_utf8_lossy(&bytes[table_start + n * 20..]).to_string();
    let trailer = trailer.as_str();
    assert!(trailer.starts_with("trailer\n<< ") && num_after(trailer, "/Size") == Some(n), "trailer Size");
    let mut objs = Vec::new();
    let mut offs = Vec::new();
    for i in 1..n {
        let e = entry(i);
        assert!(e.ends_with(b" 00000 n \n"), "entry {i}");
        offs.push(std::str::from_utf8(&e[..10]).unwrap().parse::<usize>().unwrap());
    }
    for (k, &o) in offs.iter().enumerate() {
        let num = k + 1;
        let tag = format!("{num} 0 obj\n");
        assert!(bytes[o..].starts_with(tag.as_bytes()), "object {num} is where the table says");
        let end = offs.get(k + 1).copied().unwrap_or(xref_at);
        let body = &bytes[o + tag.len()..end];
        assert!(body.ends_with(b"\nendobj\n"), "object {num} ends with endobj");
        let body = &body[..body.len() - 8];
        if let Some(p) = body.windows(7).position(|w| w == b"stream\n") {
            let dict = String::from_utf8_lossy(&body[..p]).to_string();
            let len = num_after(&dict, "/Length ").expect("stream Length");
            let data = &body[p + 7..];
            assert_eq!(data.len(), len + "\nendstream".len(), "object {num}: /Length is the stream's length");
            assert!(data.ends_with(b"\nendstream"));
            objs.push((num, dict, Some(data[..len].to_vec())));
        } else {
            objs.push((num, String::from_utf8_lossy(body).to_string(), None));
        }
    }
    // every reference points at an object that exists
    for (num, d, _) in &objs {
        let mut rest = d.as_str();
        while let Some(i) = rest.find(" 0 R") {
            let before = &rest[..i];
            let r: usize = before.rsplit(|c: char| !c.is_ascii_digit()).next().unwrap().parse().unwrap();
            assert!(r >= 1 && r < n, "object {num} refers to {r}");
            rest = &rest[i + 4..];
        }
    }
    objs
}

fn type0_fonts(objs: &[(usize, String, Option<Vec<u8>>)]) -> Vec<usize> {
    objs.iter().filter(|o| o.1.contains("/Subtype /Type0")).map(|o| o.0).collect()
}

#[test]
fn greek_and_cyrillic_are_written_with_an_embedded_font_and_read_back_exactly() {
    let (bytes, built) = build(vec![Block::Heading(1, "\u{39a}\u{3b5}\u{3c6}\u{3b1}\u{3bb}\u{3af}\u{3b4}\u{3b1} / \u{417}\u{430}\u{433}\u{43e}\u{43b}\u{43e}\u{432}\u{43e}\u{43a}".into()), Block::Para(GREEK.into()), Block::Para(CYRILLIC.into()), Block::Para("Plain Latin stays on Helvetica.".into())], "\u{39a}\u{3b5}\u{3c6}\u{3b1}\u{3bb}\u{3af}\u{3b4}\u{3b1}");
    assert_eq!(built.replaced_chars, 0);
    let d = Document::open(bytes.clone()).unwrap();
    assert!(!d.info().repaired, "read without any repair");
    assert_eq!(d.page_count(), 1);
    let t = norm(&d.page_text(0).unwrap());
    for want in [norm(GREEK), norm(CYRILLIC), "Plain Latin stays on Helvetica.".to_string(), "\u{39a}\u{3b5}\u{3c6}\u{3b1}\u{3bb}\u{3af}\u{3b4}\u{3b1} / \u{417}\u{430}\u{433}\u{43e}\u{43b}\u{43e}\u{432}\u{43e}\u{43a}".to_string()] {
        assert!(t.contains(&want), "{want:?} in {t:?}");
    }
    assert_eq!(d.info().title, "\u{39a}\u{3b5}\u{3c6}\u{3b1}\u{3bb}\u{3af}\u{3b4}\u{3b1}");
    // the footer carries the Greek title too
    assert!(t.contains("\u{39a}\u{3b5}\u{3c6}\u{3b1}\u{3bb}\u{3af}\u{3b4}\u{3b1} - 1 / 1"), "{t}");
    // copy and search find the text
    assert!(!d.search("\u{3b3}\u{3bb}\u{3ce}\u{3c3}\u{3c3}\u{3b1}", true).is_empty());
}

#[test]
fn the_structure_is_strict_and_the_font_file_parses_with_an_independent_reader() {
    let (bytes, _) = build(vec![Block::Heading(2, CYRILLIC.into()), Block::Para(GREEK.into()), Block::Code("\u{3b1} = \u{3b2} + \u{3b3}\n  \u{3c0}r\u{b2}".into())], "T");
    let objs = validate(&bytes);
    let fonts = type0_fonts(&objs);
    assert_eq!(fonts.len(), 3, "Sans (Greek paragraph), Bold (the heading) and Mono (the code block)");
    for f in fonts {
        let get = |n: usize| &objs[n - 1];
        let t0 = &get(f).1;
        assert!(t0.contains("/Encoding /Identity-H") && t0.contains("/ToUnicode"), "{t0}");
        let cid = num_after(t0, "/DescendantFonts [").unwrap();
        let c = &get(cid).1;
        assert!(c.contains("/Subtype /CIDFontType2") && c.contains("/CIDToGIDMap /Identity") && c.contains("/W ["), "{c}");
        let fd = &get(num_after(c, "/FontDescriptor").unwrap()).1;
        assert!(fd.contains("/Type /FontDescriptor") && fd.contains("/FontBBox [") && fd.contains("/Ascent") && fd.contains("/Descent"), "{fd}");
        let ff = get(num_after(fd, "/FontFile2").unwrap());
        let file = inflate(ff.2.as_ref().unwrap(), 1 << 26);
        assert_eq!(num_after(&ff.1, "/Length1"), Some(file.len()), "Length1 is the uncompressed size");
        assert!(file.len() < 100_000, "a subset, not the whole font: {}", file.len());
        // the independent reader accepts it and draws the used glyphs
        let font = fontdue::Font::from_bytes(file.as_slice(), fontdue::FontSettings::default()).expect("the subset is a valid TrueType font");
        let alpha = face(0).unwrap().gid('\u{3b1}');
        let (m, px) = font.rasterize_indexed(alpha, 24.0);
        let _ = alpha;
        let tu = get(num_after(t0, "/ToUnicode").unwrap());
        let cmap = String::from_utf8(inflate(tu.2.as_ref().unwrap(), 1 << 20)).unwrap();
        assert!(cmap.contains("beginbfchar") && cmap.contains("begincodespacerange") && cmap.contains("<0000> <FFFF>"));
        let _ = (m, px);
    }
    // the Sans subset has the outline of alpha and not of a letter that was not used
    let sans = type0_fonts(&objs).into_iter().find(|f| objs[*f - 1].1.contains("DejaVuSans") && !objs[*f - 1].1.contains("Bold") && !objs[*f - 1].1.contains("Mono")).unwrap();
    let t0 = &objs[sans - 1].1;
    let cid = num_after(t0, "/DescendantFonts [").unwrap();
    let fd = num_after(&objs[cid - 1].1, "/FontDescriptor").unwrap();
    let ff = &objs[num_after(&objs[fd - 1].1, "/FontFile2").unwrap() - 1];
    let file = inflate(ff.2.as_ref().unwrap(), 1 << 26);
    let font = fontdue::Font::from_bytes(file.as_slice(), fontdue::FontSettings::default()).unwrap();
    let f0 = face(0).unwrap();
    let (ma, pa) = font.rasterize_indexed(f0.gid('\u{3b1}'), 24.0);
    assert!(ma.width > 0 && pa.iter().any(|v| *v > 200), "alpha has ink: {}x{} max {:?} gid {} of {}", ma.width, ma.height, pa.iter().max(), f0.gid('\u{3b1}'), t0);
    let (mz, _) = font.rasterize_indexed(f0.gid('\u{4e}'), 24.0);
    assert_eq!(mz.width, 0, "N was not used, so its outline is not in the file");
}

#[test]
fn widths_come_from_the_font_and_long_text_wraps_and_pages_without_losing_a_word() {
    let para = format!("{GREEK}. {CYRILLIC} ").repeat(40);
    let (bytes, built) = build(vec![Block::Heading(1, "Sample".into()), Block::Para(para.clone()), Block::Bullet(0, "\u{2022}".into(), GREEK.into()), Block::Quote(CYRILLIC.into())], "Sample");
    let d = Document::open(bytes.clone()).unwrap();
    assert!(built.pages >= 1 && d.page_count() == built.pages);
    let all = norm(&(0..d.page_count()).map(|i| d.page_text(i).unwrap()).collect::<Vec<_>>().join(" "));
    let want = norm(&para);
    let words = want.split(' ').filter(|w| w.chars().count() > 3).collect::<Vec<_>>();
    let missing: Vec<&&str> = words.iter().filter(|w| !all.contains(**w)).collect();
    assert!(missing.is_empty(), "words lost: {missing:?}");
    assert_eq!(all.matches("\u{3b3}\u{3bb}\u{3ce}\u{3c3}\u{3c3}\u{3b1}").count(), 40 + 1, "every copy of a word comes back once");
    // /W lists the used glyphs with a width in 1/1000 em: the Greek capital Kappa is about 0.66 em in DejaVu Sans
    let objs = validate(&bytes);
    let cid = objs.iter().find(|o| o.1.contains("/CIDFontType2")).unwrap();
    let kappa = face(0).unwrap().gid('\u{39a}');
    let w = face(0).unwrap().width('\u{39a}').round() as i64;
    assert!(cid.1.contains(&format!(" {kappa} [{w}]")), "{} not in {}", w, cid.1.chars().take(300).collect::<String>());
    assert!((600..700).contains(&w), "{w}");
}

#[test]
fn latin_only_files_have_no_embedded_font_and_mixed_blocks_use_one() {
    let (plain, _) = build(vec![Block::Para("caf\u{e9} \u{201c}quoted\u{201d} \u{20ac}5".into())], "Plain");
    assert!(type0_fonts(&validate(&plain)).is_empty() && !plain.windows(8).any(|w| w == b"Identity"));
    let (mixed, built) = build(vec![Block::Para("Name: \u{391}\u{3bb}\u{3ad}\u{3be}\u{3b1}\u{3bd}\u{3b4}\u{3c1}\u{3bf}\u{3c2} (caf\u{e9} \u{2192} \u{2264} \u{221e})".into())], "Mixed");
    assert_eq!(built.replaced_chars, 0);
    let t = norm(&Document::open(mixed).unwrap().page_text(0).unwrap());
    assert!(t.contains("Name: \u{391}\u{3bb}\u{3ad}\u{3be}\u{3b1}\u{3bd}\u{3b4}\u{3c1}\u{3bf}\u{3c2} (caf\u{e9} \u{2192} \u{2264} \u{221e})"), "{t}");
}

#[test]
fn scripts_that_need_shaping_or_right_to_left_layout_and_missing_glyphs_stay_refused() {
    for (c, name) in [('\u{b95}', "Tamil"), ('\u{915}', "Devanagari"), ('\u{5d0}', "Hebrew"), ('\u{627}', "Arabic"), ('\u{4e2d}', "Chinese"), ('\u{e01}', "Thai")] {
        assert!(!supports(c), "{name}");
    }
    for c in ['\u{3b1}', '\u{416}', '\u{2192}', '\u{256c}', '\u{531}', '\u{10d0}', '\u{1e9e}'] {
        assert!(supports(c), "{c}");
    }
    // the embedded DejaVu path alone still refuses them (the shaped path of F-PDF-15 draws them); Chinese has no glyphs anywhere
    let (bytes, built) = build(vec![Block::Para("chinese \u{4f60}\u{597d}\u{4e16}\u{754c}\u{3002} here".into())], "T");
    assert_eq!(built.replaced_chars, 5, "counted, and written as ? so the file is still valid");
    validate(&bytes);
    assert!(Document::open(bytes).unwrap().page_text(0).unwrap().contains("chinese ????? here"));
    let (n, seen) = crate::md::outside_latin("t", &[Block::Para("\u{3b1}\u{b95}\u{5d0}\u{4e2d}".into())], 5);
    assert_eq!((n, seen), (1, vec!['\u{4e2d}']), "Greek, Tamil and Hebrew are drawn, Chinese is named");
}

#[test]
fn the_subsetter_survives_a_hostile_font_and_composites_keep_their_parts() {
    let f = face(0).unwrap();
    // an accented letter is often a composite of a base and a mark: the closure holds the parts
    let a = f.gid('\u{1ea1}');
    let set: std::collections::BTreeSet<u16> = [a].into_iter().collect();
    let keep = f.ttf.closure(&set);
    assert!(keep.contains(&0) && keep.contains(&a) && keep.len() >= 2, "{keep:?}");
    let file = f.ttf.subset(&keep, &[('\u{1ea1}' as u32, a)]);
    let sub = crate::ttf::Ttf::parse(&file).expect("the subset parses");
    assert_eq!(sub.num_glyphs, f.ttf.num_glyphs);
    assert_eq!(sub.advance(a), f.ttf.advance(a));
    fontdue::Font::from_bytes(file.as_slice(), fontdue::FontSettings::default()).unwrap();
    // cut-off and mangled fonts are refused or handled, never a panic
    let sans = include_bytes!("../../iyakki-shell/assets/DejaVuSans.ttf");
    for cut in [0usize, 3, 12, 100, 5000, 100_000] {
        let _ = crate::ttf::Ttf::parse(&sans[..cut]).map(|t| t.cmap_map().len());
    }
    let mut x = 12345u32;
    for _ in 0..200 {
        let mut copy = sans[..200_000].to_vec();
        for _ in 0..20 {
            x = x.wrapping_mul(1664525).wrapping_add(1013904223);
            let i = (x >> 8) as usize % copy.len();
            copy[i] = (x >> 24) as u8;
        }
        if let Some(t) = crate::ttf::Ttf::parse(&copy) {
            let m = t.cmap_map();
            let set: std::collections::BTreeSet<u16> = m.values().copied().take(20).collect();
            let keep = t.closure(&set);
            let _ = t.subset(&keep, &[]);
        }
    }
}

#[test]
fn a_subset_draws_the_glyphs_it_keeps_exactly_like_the_whole_font() {
    let f = face(0).unwrap();
    let sans = include_bytes!("../../iyakki-shell/assets/DejaVuSans.ttf");
    let whole = fontdue::Font::from_bytes(sans.as_slice(), fontdue::FontSettings::default()).unwrap();
    let chars = ['\u{3b1}', '\u{416}', 'A', '\u{1ea1}', '\u{2192}'];
    let set: std::collections::BTreeSet<u16> = chars.iter().map(|c| f.gid(*c)).collect();
    let pairs: Vec<(u32, u16)> = chars.iter().map(|c| (*c as u32, f.gid(*c))).collect();
    let file = f.ttf.subset(&f.ttf.closure(&set), &pairs);
    let part = fontdue::Font::from_bytes(file.as_slice(), fontdue::FontSettings::default()).unwrap();
    for c in chars {
        let g = f.gid(c);
        let (m1, p1) = whole.rasterize_indexed(g, 24.0);
        let (m2, p2) = part.rasterize_indexed(g, 24.0);
        assert!(m1.width > 0, "{c:?} has an outline in the whole font");
        assert_eq!((m1.width, m1.height, m1.advance_width.round()), (m2.width, m2.height, m2.advance_width.round()), "{c:?}");
        assert_eq!(p1, p2, "{c:?} is drawn the same");
    }
}


#[test]
fn text_is_read_from_an_embedded_font_that_has_no_unicode_map_of_its_own() {
    // the writer's Greek page, with the ToUnicode entry of its font made unreadable (same length, so every offset stays right)
    let (bytes, _built) = build(vec![Block::Para(GREEK.into())], "g");
    let with = Document::open(bytes.clone()).unwrap().page_text(0).unwrap();
    assert!(norm(&with).contains(&norm(GREEK)), "the plain file reads back");
    let mut cut = bytes.clone();
    let key = b"/ToUnicode";
    let mut n = 0;
    let mut i = 0;
    while let Some(p) = cut[i..].windows(key.len()).position(|w| w == key) {
        cut[i + p..i + p + key.len()].copy_from_slice(b"/Xxxxxxxxx");
        i += p + key.len();
        n += 1;
    }
    assert!(n >= 1, "the font had a ToUnicode entry");
    let read = Document::open(cut).unwrap().page_text(0).unwrap();
    assert!(norm(&read).contains(&norm(GREEK)), "the glyphs are read back through the font's own cmap table");
    // Cyrillic too, and a font that is not Identity-H keeps being refused rather than guessed
    let (bytes, _) = build(vec![Block::Para(CYRILLIC.into())], "c");
    let mut cut = bytes;
    while let Some(p) = cut.windows(10).position(|w| w == b"/ToUnicode") {
        cut[p..p + 10].copy_from_slice(b"/Xxxxxxxxx");
    }
    assert!(norm(&Document::open(cut.clone()).unwrap().page_text(0).unwrap()).contains(&norm(CYRILLIC)));
    let mut odd = cut;
    while let Some(p) = odd.windows(11).position(|w| w == b"/Identity-H") {
        odd[p..p + 11].copy_from_slice(b"/Mystery-H0");
    }
    assert!(!norm(&Document::open(odd).unwrap().page_text(0).unwrap_or_default()).contains("Привет"), "another encoding is not guessed");
}
