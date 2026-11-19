//! Fuzz tests (deterministic, seeded) for the terminal: random byte streams and mutated escape-sequence corpora go through the parser and
//! the screen; the screen must stay valid, bounded and quick, and a stream cut into two reads at any byte must give the same screen as one.
//! `IYAKKI_FUZZ_CASES=100000 cargo test -p iyakki-vt fuzz` runs a larger set.

use crate::fuzzkit::{cases, mutate, run, Rng};
use crate::{Parser, Screen};
use std::time::Duration;

const SCROLLBACK: usize = 40;

fn corpus() -> Vec<Vec<u8>> {
    let s: Vec<Vec<u8>> = vec![
        b"\x1b[2J\x1b[H\x1b[1;31mred\x1b[0m \x1b[38;5;200mpal\x1b[48;2;1;2;3mtrue\x1b[m\r\nsecond line\x1b[3A\x1b[5C\x1b[K\x1b[2K\x1b[1J".to_vec(),
        b"\x1b[?1049h\x1b[2;10r\x1b[5;5H\x1b[3L\x1b[2M\x1b[4@\x1b[4P\x1b[3X\x1b[2S\x1b[2T\x1b[?1049l\x1b7\x1b8\x1bM\x1bD\x1bE\x1bH\x1b[g\x1b[3g".to_vec(),
        b"\x1b]0;a window title\x07\x1b]2;another\x1b\\\x1b]8;;https://example.org\x07link\x1b]8;;\x07\x1bP1$r0m\x1b\\\x1b_apc\x1b\\\x1b^pm\x1b\\\x1bXsos\x1b\\".to_vec(),
        b"\x1b[6n\x1b[c\x1b[>c\x1b[5n\x1b[?25l\x1b[?25h\x1b[?2004h\x1b[?7l\x1b[?7h\x1b[4h\x1b[4l\x1b[20h\x1b(0lqqk\x1b(B\x1b)0\x0e\x0f\x1bc".to_vec(),
        "wide \u{4e2d}\u{6587} \u{1F600} caf\u{e9} e\u{301} \u{200d} \u{fffd}\t\t\x08\x08\x0b\x0c\x07 tab\ttab\r\n".as_bytes().to_vec(),
        b"\x1b[99999999999999999999;99999999999999999999H\x1b[999999999@\x1b[999999999L\x1b[999999999S\x1b[999999999b\x1b[999999999C\x1b[999999999A\x1b[0;0r\x1b[1;999999r".to_vec(),
        b"line of output that goes on and on\r\n".repeat(40),
        b"\x1b[".repeat(300),
        b"\x1b[1;2;3;4;5;6;7;8;9;10;11;12;13;14;15;16;17;18;19;20;21;22;23;24;25;26;27;28;29;30m".repeat(8),
    ];
    s
}

/// Whatever was fed, the screen is in a valid state.
fn check(s: &Screen, fed: usize) {
    assert!(s.cols() >= 1 && s.rows() >= 1 && s.cols() <= crate::screen::MAX_COLS && s.rows() <= crate::screen::MAX_ROWS);
    assert!(s.x < s.cols() && s.y < s.rows(), "cursor ({}, {}) outside {}x{}", s.x, s.y, s.cols(), s.rows());
    let (top, bottom) = s.region();
    assert!(top <= bottom && bottom < s.rows(), "scroll region {top}..{bottom} in {} rows", s.rows());
    assert!(s.scrollback_len() <= SCROLLBACK, "scrollback holds {} lines (limit {SCROLLBACK})", s.scrollback_len());
    assert!(s.title.len() <= 4096, "title of {} bytes", s.title.len());
    assert!(s.replies.len() <= fed * 24 + 64, "{} reply bytes for {fed} bytes fed", s.replies.len());
    for y in 0..s.rows() {
        let _ = s.row_text(y);
    }
    let _ = s.text();
    for back in [0, 1, SCROLLBACK] {
        let _ = s.view_row(back, s.rows() - 1);
    }
}

fn snapshot(s: &Screen) -> (String, usize, usize, String, usize, bool, Vec<u8>) {
    (s.text(), s.x, s.y, s.title.clone(), s.scrollback_len(), s.alt_screen, s.replies.clone())
}

fn fresh(cols: usize, rows: usize) -> Screen {
    let mut s = Screen::new(cols, rows);
    s.set_scrollback_limit(SCROLLBACK);
    s
}

#[test]
fn fuzz_vt_parser_keeps_the_screen_valid_and_bounded() {
    let corpus = corpus();
    let mut k = 0usize;
    let n = run("vt", 0x7E, &corpus, 700, Duration::from_secs(2), 64 << 20, |input| {
        k += 1;
        let (c, r) = [(80, 24), (5, 3), (1, 1), (40, 12)][k % 4];
        let mut s = fresh(c, r);
        let mut p = Parser::new();
        p.feed(&mut s, input);
        check(&s, input.len());
        // a resize in the middle of the program's life, then more output
        s.resize(c / 2 + 1, r / 2 + 1);
        check(&s, input.len());
        p.feed(&mut s, input);
        check(&s, input.len() * 2);
        s.resize(200, 60);
        check(&s, input.len() * 2);
    });
    assert!(n >= cases(700));
}

#[test]
fn fuzz_vt_pure_random_bytes_and_biased_escape_soup() {
    let mut r = Rng::new(0x7E2);
    let alphabet: Vec<u8> = b"\x1b[]();?>0123456789;:\x07\x18\x1a\x1b\\PXHm@ABCDJKLMSTfhlnrgbcdt \r\n\t".to_vec();
    for case in 0..cases(500) {
        let len = r.below(3000);
        let bytes: Vec<u8> = if r.chance(3) { (0..len).map(|_| r.next() as u8).collect() } else { (0..len).map(|_| if r.chance(8) { r.next() as u8 } else { *r.pick(&alphabet) }).collect() };
        let mut s = fresh(1 + r.below(120), 1 + r.below(50));
        let mut p = Parser::new();
        let t = std::time::Instant::now();
        // arbitrary read sizes
        let mut at = 0;
        while at < bytes.len() {
            let n = 1 + r.below(64);
            let end = (at + n).min(bytes.len());
            p.feed(&mut s, &bytes[at..end]);
            at = end;
            if r.chance(40) {
                s.resize(1 + r.below(150), 1 + r.below(60));
            }
            let _ = s.take_replies();
        }
        check(&s, bytes.len());
        assert!(t.elapsed() < Duration::from_secs(2), "case {case}: {:?}", t.elapsed());
    }
}

#[test]
fn fuzz_vt_a_stream_split_at_every_position_gives_the_same_screen() {
    let corpus = corpus();
    for (ci, c) in corpus.iter().enumerate() {
        let mut whole = fresh(20, 6);
        Parser::new().feed(&mut whole, c);
        let want = snapshot(&whole);
        for cut in 0..=c.len() {
            let mut s = fresh(20, 6);
            let mut p = Parser::new();
            p.feed(&mut s, &c[..cut]);
            p.feed(&mut s, &c[cut..]);
            assert!(snapshot(&s) == want, "corpus {ci}: a stream split at byte {cut} gives a different screen");
        }
        // one byte at a time
        let mut s = fresh(20, 6);
        let mut p = Parser::new();
        for b in c {
            p.feed(&mut s, std::slice::from_ref(b));
        }
        assert!(snapshot(&s) == want, "corpus {ci}: byte-at-a-time differs");
    }
    // the same for mutated streams, at a random cut
    let mut r = Rng::new(0x7E3);
    for case in 0..cases(500) {
        let base = r.pick(&corpus).clone();
        let m = mutate(&mut r, &base);
        let mut whole = fresh(20, 6);
        Parser::new().feed(&mut whole, &m);
        let cut = r.below(m.len() + 1);
        let mut s = fresh(20, 6);
        let mut p = Parser::new();
        p.feed(&mut s, &m[..cut]);
        p.feed(&mut s, &m[cut..]);
        assert!(snapshot(&s) == snapshot(&whole), "case {case}: split at {cut} of {} differs", m.len());
    }
}

#[test]
fn fuzz_vt_scrollback_stays_at_its_limit_under_a_flood() {
    let mut s = fresh(80, 24);
    let mut p = Parser::new();
    let line = b"0123456789012345678901234567890123456789\r\n";
    for _ in 0..20_000 {
        p.feed(&mut s, line);
    }
    assert_eq!(s.scrollback_len(), SCROLLBACK);
    // an unterminated OSC or DCS that never ends does not grow without bound
    let mut p = Parser::new();
    p.feed(&mut s, b"\x1b]0;");
    for _ in 0..2000 {
        p.feed(&mut s, &[b'x'; 1000]);
    }
    assert!(s.title.len() <= 4096);
    p.feed(&mut s, b"\x07\x1bP");
    for _ in 0..2000 {
        p.feed(&mut s, &[b'y'; 1000]);
    }
    check(&s, 0 + 4_000_000);
}
