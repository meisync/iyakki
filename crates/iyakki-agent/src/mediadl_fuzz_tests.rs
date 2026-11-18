//! Fuzz tests (deterministic, seeded) for the note that lets a broken download go on: mutated `.part.src` notes next to a `.part` file. A note
//! that is not exactly ours is not used, a note's validator is only ever plain text that can go into a request header, and the clean-up of old
//! parts touches nothing but `.part` and `.part.src` files. `IYAKKI_FUZZ_CASES=20000 cargo test -p iyakki-agent mediadl_fuzz` runs more.

use super::*;
use std::time::Duration;
use crate::fuzzkit::{cases, mutate, Rng};

const URL: &str = "https://media.example.org/a/b.mp4?x=1";

fn folder(tag: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("iyakki-mediadl-fuzz-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

#[test]
fn mediadl_fuzz_a_hostile_resume_note_is_not_used() {
    let d = folder("note");
    let good = format!("iyakki-resume 1\n{URL}\n\"abc123\"\n").into_bytes();
    let corpus = vec![good.clone(), format!("iyakki-resume 1\n{URL}\nWed, 21 Oct 2026 07:28:00 GMT\n").into_bytes(), format!("iyakki-resume 1\n{URL}\n\n").into_bytes()];
    let mut r = Rng::new(0x3ED1);
    let (mut used, mut refused) = (0usize, 0usize);
    for case in 0..cases(4000) {
        let base = r.pick(&corpus).clone();
        let note = if case == 0 { good.clone() } else { mutate(&mut r, &base) };
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        let name = if r.chance(8) { "..".to_string() } else { format!("movie{}", r.below(3)) };
        std::fs::write(d.join(format!("{name}.part.src")), &note).unwrap();
        if !r.chance(5) {
            std::fs::write(d.join(format!("{name}.part")), vec![1u8; r.below(4)]).unwrap();
        }
        let url = if r.chance(6) { "https://other.example.org/".to_string() } else { URL.to_string() };
        let t = std::time::Instant::now();
        match find_resume(&[d.clone()], &url) {
            Some(res) => {
                used += 1;
                let text = String::from_utf8_lossy(&note).into_owned();
                let mut lines = text.lines();
                assert_eq!((lines.next(), lines.next()), (Some("iyakki-resume 1"), Some(url.as_str())), "case {case}: a note that is not ours was used: {text:?}");
                assert!(res.offset > 0 && !res.validator.is_empty(), "case {case}");
                // the validator goes into a request header: plain visible text only, so it cannot add a header line
                assert!(res.validator.len() <= 200 && res.validator.bytes().all(|b| (0x20..0x7f).contains(&b)), "case {case}: validator {:?} is not plain header text", res.validator);
                assert!(!res.name.contains('/') && res.name != "." && res.name != "..", "case {case}: name {:?}", res.name);
            }
            None => refused += 1,
        }
        assert!(t.elapsed() < Duration::from_secs(2));
    }
    assert!(used > 0 && refused > 0, "used {used}, refused {refused}");
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn mediadl_fuzz_clean_up_touches_only_part_files() {
    let d = folder("clean");
    let mut r = Rng::new(0x3ED2);
    for case in 0..cases(300) {
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        let others = ["keep.mp4", "keep.part2", "x.part.srcx", ".part", "a.part.src.bak", "song.part.keep"];
        for o in others {
            std::fs::write(d.join(o), b"mine").unwrap();
        }
        let n = r.below(4);
        for i in 0..n {
            std::fs::write(d.join(format!("dl{i}.part.src")), b"iyakki-resume 1\nu\nv\n").unwrap();
            if r.chance(2) {
                std::fs::write(d.join(format!("dl{i}.part")), b"data").unwrap();
            }
        }
        let now = std::time::SystemTime::now() + Duration::from_secs(r.below(40 * 24 * 3600) as u64);
        let removed = clean_stale_parts(&[d.clone(), d.join("missing")], Duration::from_secs(r.below(30 * 24 * 3600) as u64), now);
        assert!(removed <= n, "case {case}: removed {removed} of {n}");
        for o in others {
            assert_eq!(std::fs::read(d.join(o)).unwrap(), b"mine", "case {case}: {o} was touched");
        }
    }
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn mediadl_fuzz_regression_a_validator_with_a_bare_carriage_return_is_not_used() {
    let d = folder("cr");
    for (i, validator) in ["\"abc\"", "\"abc\"\rX-Evil: 1", "tab\there", "\u{e9}tag", &"v".repeat(300)].iter().enumerate() {
        std::fs::write(d.join(format!("m{i}.part.src")), format!("iyakki-resume 1\n{URL}\n{validator}\n")).unwrap();
        std::fs::write(d.join(format!("m{i}.part")), b"bytes").unwrap();
    }
    let found = find_resume(&[d.clone()], URL).expect("the plain note is still used");
    assert_eq!((found.name.as_str(), found.validator.as_str()), ("m0", "\"abc\""));
    let _ = std::fs::remove_dir_all(&d);
}
