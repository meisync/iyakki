//! Fuzz test (deterministic, seeded) for the per-app SDK state file that holds the money limit and what was spent: a damaged or hostile file is
//! never overwritten by a save, never makes a spend overflow, and a readable one keeps its limit. `IYAKKI_FUZZ_CASES=20000` runs more.

use crate::fuzzkit::{cases, mutate, Rng};
use crate::store::sdkv2_money::AppFile;
use iyakki_core_api::sdkbridge::money::{check_spend, Limit};

#[test]
fn sdkmoney_fuzz_a_damaged_state_file_is_left_alone_and_a_spend_cannot_overflow() {
    let d = std::env::temp_dir().join(format!("iyakki-sdkmoney-fuzz-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    let p = d.join("app.sdk2.json");
    let mut good = AppFile::default();
    good.money.currency = "USD".into();
    good.money.per_day_minor = 5000;
    good.money.spent.insert("2026-11-12".into(), 1200);
    good.money.refs.insert("r1".into(), "receipt".into());
    let text = serde_json::to_vec_pretty(&good).unwrap();
    let mut r = Rng::new(0x5D2);
    let (mut readable, mut locked) = (0usize, 0usize);
    for case in 0..cases(3000) {
        let mut m = if case == 0 { text.clone() } else { mutate(&mut r, &text) };
        if r.chance(6) {
            // numbers at the edge of what a u64 holds, in the places that matter
            let t = String::from_utf8_lossy(&m).replace("5000", "18446744073709551615").replace("1200", "18446744073709551614");
            m = t.into_bytes();
        }
        std::fs::write(&p, &m).unwrap();
        let (read_only, cur, per_day, spent) = AppFile::edit(&p, true, |f| (f.read_only, f.money.currency.clone(), f.money.per_day_minor, f.money.spent.values().copied().max().unwrap_or(0)));
        let after = std::fs::read(&p).unwrap();
        if read_only {
            locked += 1;
            assert_eq!(after, m, "case {case}: a file that could not be read safely was written over");
        } else {
            readable += 1;
            assert!(serde_json::from_slice::<AppFile>(&after).is_ok(), "case {case}: the saved file cannot be read back");
        }
        // what a spend does with the numbers read: the check first, then the addition
        let lim = (!cur.is_empty()).then(|| Limit { currency: cur.clone(), per_day_minor: per_day });
        for amount in [1u64, 100, 5000, 1_000_000_000_000] {
            if check_spend(lim.as_ref(), spent, &cur, amount).is_ok() {
                assert!(spent.checked_add(amount).is_some_and(|t| t <= per_day), "case {case}: {spent} + {amount} against a limit of {per_day}");
            }
        }
    }
    assert!(readable > 0 && locked > 0, "readable {readable}, locked {locked}");
    let _ = std::fs::remove_dir_all(&d);
}
