//! Fuzz tests (deterministic, seeded) for the signed check-up result file: a saved file that was changed in any way that changes its meaning is
//! refused in words and never read as a result; nothing panics. A file signed with someone else's key is accepted by design (the signature
//! proves the file was not changed after it was written, not who wrote it), so the readers of its numbers are tried on extreme values too.
//! `IYAKKI_FUZZ_CASES=50000 cargo test -p iyakki-agent checkup_fuzz` runs a larger set.

use crate::checkup_result::*;
use crate::fuzzkit::{cases, run, Rng};
use std::time::Duration;

fn body(created: i64) -> Body {
    Body {
        schema: SCHEMA,
        machine: "Mac mini \"8,1\" <x> \u{4e2d}".into(),
        release: "0.1.0-alpha.3".into(),
        created,
        cells: vec![Cell { capability: "Wi-Fi".into(), status: "verified".into(), note: "fine".into() }, Cell { capability: "k3s start".into(), status: "untested".into(), note: "not asked".into() }],
    }
}

fn folder(tag: &str) -> std::path::PathBuf {
    let d = std::env::temp_dir().join(format!("iyakki-checkup-fuzz-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

#[test]
fn checkup_fuzz_a_changed_result_file_is_refused_or_means_the_same() {
    let d = folder("mut");
    let original = body(1_800_000_000);
    let text = sign(&d, &original).unwrap();
    assert_eq!(verify(&text).unwrap(), original);
    let corpus = vec![text.clone().into_bytes(), sign(&d, &body(0)).unwrap().into_bytes()];
    let mut same = 0usize;
    let n = run("checkup result", 0xC4E, &corpus, 5000, Duration::from_secs(1), 32 << 20, |input| {
        let t = String::from_utf8_lossy(input);
        match verify(&t) {
            // the only files that pass are ones whose signed text, key and signature are all as written (spacing outside the strings, the order of
            // the three entries, ...): the body read from them is one of the two that were signed
            Ok(b) => {
                same += 1;
                assert!(b == original || b == body(0), "a changed file was accepted with a different result: {b:?}");
                let _ = lines(&b);
            }
            Err(e) => assert!(!e.is_empty()),
        }
    });
    assert!(n >= cases(5000) && same > 0);
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn checkup_fuzz_a_file_signed_with_extreme_values_is_read_without_trouble() {
    let d = folder("ext");
    let mut r = Rng::new(0xC4F);
    for case in 0..cases(1500) {
        let extra = r.next() as i64;
        let created = *r.pick(&[i64::MIN, i64::MAX, -1, 0, 1, 253_402_300_800, extra]);
        let long: String = (0..r.below(3000)).map(|_| char::from_u32(r.below(0x3000) as u32 + 0x20).unwrap_or('x')).collect();
        let b = Body { schema: if r.chance(4) { r.next() as u32 } else { SCHEMA }, machine: long.clone(), release: long.clone(), created, cells: (0..r.below(50)).map(|i| Cell { capability: format!("c{i}"), status: long.chars().take(r.below(40)).collect(), note: long.clone() }).collect() };
        let text = sign(&d, &b).unwrap();
        match verify(&text) {
            Ok(got) => {
                assert_eq!(got, b, "case {case}");
                assert!(got.schema == SCHEMA);
                let _ = lines(&got);
            }
            Err(e) => assert!(b.schema != SCHEMA && e.contains("schema"), "case {case}: {e}"),
        }
    }
    let _ = std::fs::remove_dir_all(&d);
}
