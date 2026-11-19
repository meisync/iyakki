//! The JPEG and GIF decoders against pictures made by an independent encoder and decoder (Pillow, libjpeg, giflib):
//! test files in tests/data with the decoded pixels next to them.

use iyakki_web::image::{decode, sniff, Kind};

fn data(name: &str) -> Vec<u8> {
    std::fs::read(format!("{}/tests/data/{name}", env!("CARGO_MANIFEST_DIR"))).unwrap()
}

/// Mean and largest difference per channel between our RGBA and the expected raw RGB (or RGBA).
fn compare(name: &str, expected: &str, channels: usize) -> (f64, u8, f64) {
    let ours = decode(&data(name)).unwrap();
    assert_eq!((ours.width, ours.height), (67, 45), "{name}");
    let want = data(expected);
    assert_eq!(want.len(), 67 * 45 * channels);
    let (mut sum, mut max, mut big) = (0u64, 0u8, 0usize);
    let mut n = 0u64;
    for i in 0..(67 * 45) {
        for c in 0..3 {
            let d = (ours.rgba[i * 4 + c] as i32 - want[i * channels + c] as i32).unsigned_abs() as u8;
            sum += d as u64;
            max = max.max(d);
            if d > 12 {
                big += 1;
            }
            n += 1;
        }
    }
    (sum as f64 / n as f64, max, big as f64 / n as f64)
}

#[test]
fn baseline_jpegs_match_libjpeg() {
    for (jpg, raw, mean_limit, max_limit) in [("base444.jpg", "base444.rgb", 1.0, 6), ("base422.jpg", "base422.rgb", 3.0, 70), ("base420.jpg", "base420.rgb", 3.0, 70), ("opt420.jpg", "opt420.rgb", 3.0, 70), ("gray.jpg", "gray.rgb", 1.0, 4)] {
        let (mean, max, big) = compare(jpg, raw, 3);
        eprintln!("{jpg}: mean {mean:.2} max {max} over12 {big:.4}");
        assert!(mean < mean_limit && max <= max_limit, "{jpg}: mean {mean}, max {max}");
    }
}

#[test]
fn progressive_jpegs_match_libjpeg() {
    for (jpg, raw, mean_limit, max_limit) in [("prog444.jpg", "prog444.rgb", 1.0, 6), ("prog420.jpg", "prog420.rgb", 3.0, 70), ("gray_prog.jpg", "gray_prog.rgb", 1.0, 4)] {
        let (mean, max, big) = compare(jpg, raw, 3);
        eprintln!("{jpg}: mean {mean:.2} max {max} over12 {big:.4}");
        assert!(mean < mean_limit && max <= max_limit, "{jpg}: mean {mean}, max {max}");
    }
}

#[test]
fn gifs_match_exactly_including_transparency_and_interlacing() {
    for (gif, raw) in [("pal.gif", "pal_gif_expected.rgba"), ("pal_interlaced.gif", "pal_interlaced_expected.rgba")] {
        let ours = decode(&data(gif)).unwrap();
        let want = data(raw);
        assert_eq!(ours.rgba.len(), want.len(), "{gif}");
        let mut wrong = 0;
        for (a, b) in ours.rgba.chunks_exact(4).zip(want.chunks_exact(4)) {
            // transparent pixels: only the alpha has to agree
            let same = if b[3] == 0 { a[3] == 0 } else { a == b };
            wrong += !same as usize;
        }
        assert_eq!(wrong, 0, "{gif}: {wrong} pixels differ");
    }
}

#[test]
fn the_kind_of_a_file_is_told_from_its_first_bytes() {
    assert_eq!(sniff(&data("rgb.png")), Kind::Png);
    assert_eq!(sniff(&data("base444.jpg")), Kind::Jpeg);
    assert_eq!(sniff(&data("pal.gif")), Kind::Gif);
    assert_eq!(sniff(b"RIFF\0\0\0\0WEBPVP8 "), Kind::Webp);
    assert_eq!(sniff(b"<?xml version=\"1.0\"?><svg xmlns=\"http://www.w3.org/2000/svg\"></svg>"), Kind::Svg);
    assert_eq!(sniff(b"hello"), Kind::Unknown);
    assert!(decode(b"RIFF\0\0\0\0WEBPVP8 ").unwrap_err().contains("WebP"));
}

/// The picture size a JPEG or GIF header claims (0 when it has none we can find).
fn claimed_pixels(b: &[u8]) -> u64 {
    if b.starts_with(b"GIF") && b.len() > 10 {
        return u16::from_le_bytes([b[6], b[7]]) as u64 * u16::from_le_bytes([b[8], b[9]]) as u64;
    }
    match b.windows(2).position(|w| w[0] == 0xFF && matches!(w[1], 0xC0 | 0xC1 | 0xC2)) {
        Some(p) if p + 9 <= b.len() => ((b[p + 5] as u64) << 8 | b[p + 6] as u64) * ((b[p + 7] as u64) << 8 | b[p + 8] as u64),
        _ => 0,
    }
}

/// Decode only when the damaged file still claims a small picture: a header that now says 60000x60000 is checked
/// separately (it must be refused), and decoding a huge picture is slow in a debug build.
fn try_decode(b: &[u8]) {
    if claimed_pixels(b) <= 200_000 {
        let _ = decode(b);
    }
}

#[test]
fn damaged_and_hostile_files_never_panic() {
    let good = data("prog420.jpg");
    for cut in (0..good.len()).step_by(7) {
        try_decode(&good[..cut]);
    }
    let base = data("base420.jpg");
    for i in (0..base.len()).step_by(3) {
        let mut b = base.clone();
        b[i] ^= 0xFF;
        try_decode(&b);
        let mut b = good.clone();
        let j = i % good.len();
        b[j] = b[j].wrapping_add(41);
        try_decode(&b);
    }
    let gif = data("pal.gif");
    for cut in 0..gif.len() {
        try_decode(&gif[..cut]);
    }
    for i in 0..gif.len() {
        let mut b = gif.clone();
        b[i] ^= 0xA5;
        try_decode(&b);
    }
    // a header that claims a huge picture is refused before anything is allocated
    let mut huge = base.clone();
    let p = huge.windows(2).position(|w| w == [0xFF, 0xC0]).expect("baseline frame header");
    huge[p + 5..p + 9].copy_from_slice(&[0xFF, 0xF0, 0xFF, 0xF0]);
    assert!(decode(&huge).unwrap_err().contains("too large"));
    let mut huge_gif = gif.clone();
    huge_gif[6..10].copy_from_slice(&[0xFF, 0xFF, 0xFF, 0xFF]);
    assert!(decode(&huge_gif).is_err());
    assert!(decode(&[]).is_err() && decode(&[0xFF, 0xD8]).is_err() && decode(b"GIF89a").is_err());
}

#[test]
fn data_urls_are_read() {
    let (mime, bytes) = iyakki_web::image::parse_data_url("data:image/gif;base64,R0lGODlhAQABAIAAAAAAAP///yH5BAEAAAAALAAAAAABAAEAAAIBRAA7").unwrap();
    assert_eq!(mime, "image/gif");
    let g = decode(&bytes).unwrap();
    assert_eq!((g.width, g.height), (1, 1));
    let (m, b) = iyakki_web::image::parse_data_url("data:text/plain,a%20b").unwrap();
    assert_eq!((m.as_str(), b), ("text/plain", b"a b".to_vec()));
    assert!(iyakki_web::image::parse_data_url("https://example.org/x.png").is_none());
}

/// Developer check against a directory of `name.webp` + `name.rgba` pairs made by Pillow (set `WEBP_CMP_DIR`):
/// prints the largest per-channel difference for each and fails unless all are zero.
#[test]
#[ignore]
fn webp_matches_pillow_on_a_directory() {
    let Ok(dir) = std::env::var("WEBP_CMP_DIR") else { return };
    let mut names: Vec<_> = std::fs::read_dir(&dir).unwrap().filter_map(|e| e.ok()).map(|e| e.path()).filter(|p| p.extension().map_or(false, |x| x == "webp")).collect();
    names.sort();
    let mut bad = 0;
    for p in names {
        let want = std::fs::read(p.with_extension("rgba")).unwrap();
        let got = match decode(&std::fs::read(&p).unwrap()) {
            Ok(b) => b,
            Err(e) => {
                eprintln!("{}: ERROR {e}", p.display());
                bad += 1;
                continue;
            }
        };
        if got.rgba.len() != want.len() {
            eprintln!("{}: size mismatch", p.display());
            bad += 1;
            continue;
        }
        let (mut max, mut n) = (0i32, 0usize);
        for (a, b) in got.rgba.iter().zip(&want) {
            let d = (*a as i32 - *b as i32).abs();
            max = max.max(d);
            n += (d != 0) as usize;
        }
        eprintln!("{}: max diff {max}, {n} differing bytes", p.file_name().unwrap().to_string_lossy());
        bad += (max != 0) as usize;
    }
    assert_eq!(bad, 0, "{bad} files differ");
}

fn fnv1a(b: &[u8]) -> u64 {
    b.iter().fold(0xcbf2_9ce4_8422_2325u64, |h, &x| (h ^ x as u64).wrapping_mul(0x100_0000_01b3))
}

/// The WebP decoder against libwebp (through Pillow): `webp_expected.txt` lists each file, its size and the FNV-1a
/// hash of the RGBA pixels libwebp produced, so a match is bit-exact (lossy with the loop filter and fancy chroma
/// upsampling, lossless, alpha planes, and the first frame of an animation).
#[test]
fn webp_pictures_match_libwebp_exactly() {
    let list = String::from_utf8(data("webp_expected.txt")).unwrap();
    let mut n = 0;
    for line in list.lines() {
        let f: Vec<&str> = line.split(' ').collect();
        let bytes = data(f[0]);
        assert_eq!(sniff(&bytes), Kind::Webp, "{}", f[0]);
        let bm = decode(&bytes).unwrap_or_else(|e| panic!("{}: {e}", f[0]));
        assert_eq!((bm.width, bm.height), (f[1].parse().unwrap(), f[2].parse().unwrap()), "{}", f[0]);
        assert_eq!(format!("{:016x}", fnv1a(&bm.rgba)), f[3], "{}", f[0]);
        n += 1;
    }
    assert!(n >= 10);
}
