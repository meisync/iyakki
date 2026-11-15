//! Fuzz and property tests (deterministic, seeded) for the SDK bridge's Markdown analysis and money rules. Markdown: any text up to the limit is
//! analysed quickly with bounded results. Money: under random sequences of limit changes and spends (the way the host keeps its ledger), no
//! number overflows, nothing is spent without a limit, a spend is never accepted past the limit, and the answer the person sees is right.
//! `IYAKKI_FUZZ_CASES=100000 cargo test -p iyakki-core-api fuzz` runs a larger set.

use super::markdown::*;
use super::money::*;
use crate::fuzzkit::{cases, mutate, run, Rng};
use std::time::Duration;

#[test]
fn fuzz_markdown_analysis_is_bounded_and_quick() {
    let corpus: Vec<Vec<u8>> = [
        "# Title\n\nSome *emphasis*, **strong**, `code`, a [link](https://example.org/a) and ![img](x.png).\n\n- one\n- two\n1. three\n> quote\n---\n```\nfenced [x](y)\n```\n[bad](javascript:alert(1)) [data](DATA:text/html,x) [ok](https://e.org)\n",
        "[a](b) ".repeat(700).as_str(),
        "[[[[[[[[[[[[[[[[[[[[](((((((((((((((((((((!!!![[[]]](()))``` ` ` ` \\ \\\\ ***___~~~\n",
    ]
    .iter()
    .map(|s| s.as_bytes().to_vec())
    .collect();
    let n = run("markdown", 0x3D4, &corpus, 1000, Duration::from_secs(2), 64 << 20, |input| {
        let text = String::from_utf8_lossy(input);
        for w in [0u32, 8] {
            if let Ok(a) = analyze(&text, w) {
                assert!(a.links.len() <= MAX_LINKS && a.plain.chars().count() <= MAX_PLAIN, "unbounded answer");
                assert!(a.links.iter().all(|(_, u)| !u.is_empty()), "an empty link was listed");
                let _ = a.to_json();
            }
        }
    });
    assert!(n >= cases(1000));
}

#[test]
fn fuzz_markdown_regression_hostile_shapes_of_a_full_size_text_do_not_take_long() {
    // text near the limit made of the shapes that make a naive scanner search to the end of the line for every character
    let t = std::time::Instant::now();
    for unit in ["[", "![", "[a](", "`", "\\", "*", "[a]", "\n", " \n", "- ", "> ", "#"] {
        let text = unit.repeat(MAX_TEXT / unit.len() / 8);
        let a = analyze(&text, 0).unwrap();
        assert!(a.plain.chars().count() <= MAX_PLAIN);
        assert!(t.elapsed() < Duration::from_secs(20), "{unit:?}: still busy after {:?}", t.elapsed());
    }
    // a megabyte of newlines used to count the whole plain text again for every line
    let a = analyze(&"\n".repeat(MAX_TEXT), 0).unwrap();
    assert!(a.lines >= 1);
    assert!(t.elapsed() < Duration::from_secs(40), "{:?}", t.elapsed());
}

/// The host's ledger, as `sdkv2_money` keeps it: a limit, a currency, the amount spent today; spends reserve under the check.
struct Ledger {
    limit: Option<Limit>,
    spent: u64,
}

#[test]
fn fuzz_money_random_sequences_keep_the_daily_limit() {
    let mut r = Rng::new(0x3E7);
    let mut accepted = 0u64;
    for session in 0..cases(2000) {
        let mut l = Ledger { limit: None, spent: if r.chance(20) { r.next() } else { 0 } };
        let mut shadow_spent: u128 = l.spent as u128;
        for step in 0..60 {
            match r.below(10) {
                0 => {
                    // the person sets, raises, lowers or removes the limit
                    l.limit = match r.below(5) {
                        0 => None,
                        1 => Some(Limit { currency: "USD".into(), per_day_minor: 0 }),
                        2 => Some(Limit { currency: "USD".into(), per_day_minor: u64::MAX }),
                        3 => Some(Limit { currency: "JPY".into(), per_day_minor: r.below(100_000) as u64 }),
                        _ => Some(Limit { currency: "USD".into(), per_day_minor: r.below(10_000) as u64 }),
                    };
                }
                1 => {
                    // a new day
                    l.spent = 0;
                    shadow_spent = 0;
                }
                _ => {
                    let (e1, e2) = (r.below(20_000) as u64, r.next());
                    let amount = *r.pick(&[0, 1, 2, 99, 100, 101, 5000, MAX_MINOR, MAX_MINOR + 1, u64::MAX, u64::MAX - 1, e1, e2]);
                    let cur = *r.pick(&["USD", "USD", "USD", "JPY", "EUR", "usd", "US", "", "USDD", "\u{4e2d}\u{6587}x"]);
                    let before = l.spent;
                    match check_spend(l.limit.as_ref(), l.spent, cur, amount) {
                        Ok(left) => {
                            accepted += 1;
                            let lim = l.limit.as_ref().expect("a spend was accepted with no limit");
                            assert!(lim.per_day_minor > 0 && lim.currency == cur && valid_currency(cur), "session {session} step {step}: accepted against {lim:?} in {cur:?}");
                            assert!((1..=MAX_MINOR).contains(&amount), "session {session}: accepted amount {amount}");
                            // the sum cannot overflow and stays inside the limit
                            let total = before.checked_add(amount).unwrap_or_else(|| panic!("session {session} step {step}: {before} + {amount} overflows"));
                            assert!(total <= lim.per_day_minor, "session {session} step {step}: {total} is past the limit {}", lim.per_day_minor);
                            assert_eq!(left, lim.per_day_minor - total, "the amount left is wrong");
                            l.spent = total;
                            shadow_spent += amount as u128;
                            assert_eq!(l.spent as u128, shadow_spent);
                        }
                        Err(refusal) => {
                            assert_eq!(l.spent, before);
                            let words = refusal.message();
                            assert!(!words.is_empty());
                            if let Refusal::Over { remaining, .. } = &refusal {
                                // what the person is told is left is what is left
                                let lim = l.limit.as_ref().unwrap();
                                assert_eq!(*remaining, lim.per_day_minor.saturating_sub(before));
                                assert!(amount > *remaining);
                            }
                            if l.limit.as_ref().is_none_or(|x| x.per_day_minor == 0) && (1..=MAX_MINOR).contains(&amount) && valid_currency(cur) {
                                assert_eq!(refusal, Refusal::NoLimit);
                            }
                        }
                    }
                }
            }
        }
    }
    assert!(accepted > 1000, "the sequences must reach accepted spends ({accepted})");
}

#[test]
fn fuzz_money_formatting_never_panics() {
    let mut r = Rng::new(0x3E8);
    for _ in 0..cases(20000) {
        let e = r.next();
        let amount = *r.pick(&[0, 1, 5, 99, 100, 12345, MAX_MINOR, u64::MAX, e]);
        let cur: String = (0..r.below(5)).map(|_| char::from_u32(r.below(0x3000) as u32 + 0x20).unwrap_or('X')).collect();
        let cur = if r.chance(2) { ["USD", "JPY", "KWD", "BHD", "EUR"][r.below(5)].to_string() } else { cur };
        let s = format_minor(amount, &cur);
        assert!(s.ends_with(&cur) && !s.is_empty());
        let _ = exponent(&cur);
        let _ = valid_currency(&cur);
        let _ = mutate(&mut r, cur.as_bytes());
    }
}
