use super::*;
use crate::twophase::Phase;

fn tmp(tag: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("iyakki-wakestats-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    std::fs::canonicalize(&d).unwrap()
}

const V1: &str = "{\n  \"schema\": 1,\n  \"wakes\": 7,\n  \"false_wakes\": 2\n}";

fn old_file(d: &Path) {
    std::fs::write(d.join(FILE), V1).unwrap();
}

#[test]
fn counters_count_each_kind_and_say_it_in_one_sentence() {
    let d = tmp("count");
    assert_eq!(summary(&load_as(&d, "2")), "No wake phrase counts yet.");
    for c in [Count::Heard, Count::Heard, Count::Rejected, Count::FalseWake, Count::Miss, Count::Miss, Count::Miss] {
        bump_as(&d, "2", c).unwrap();
    }
    let s = load_as(&d, "2");
    assert_eq!((s.heard, s.rejected, s.false_wakes, s.misses, s.schema), (2, 1, 1, 3, 2));
    assert!(summary(&s).contains("heard 2") && summary(&s).contains("marked as missed 3"));
    assert_eq!(Count::parse("miss"), Some(Count::Miss));
    assert_eq!(Count::parse("heard"), None, "only a false wake and a miss are marked by a person");
    // a file from a newer release is read, never rewritten
    std::fs::write(d.join(FILE), "{\"schema\": 9, \"heard\": 1}").unwrap();
    assert!(bump_as(&d, "2", Count::Heard).unwrap_err().contains("newer"));
    assert!(std::fs::read_to_string(d.join(FILE)).unwrap().contains("\"schema\": 9"));
}

#[test]
fn the_old_layout_is_staged_read_by_the_new_version_and_committed_only_after_confirm_and_grace() {
    let d = tmp("migrate");
    old_file(&d);
    let st = stage_if_needed(&d, "new", "old", 100).expect("staged");
    assert_eq!(st.phase, Phase::Staged);
    // phase 1 left the old file byte for byte as it was
    assert_eq!(std::fs::read_to_string(d.join(FILE)).unwrap(), V1);
    // the new version reads the new layout and writes into it
    let s = load_as(&d, "new");
    assert_eq!((s.heard, s.false_wakes, s.rejected, s.misses, s.schema), (7, 2, 0, 0, 2));
    bump_as(&d, "new", Count::Heard).unwrap();
    assert_eq!(load_as(&d, "new").heard, 8);
    assert_eq!(std::fs::read_to_string(d.join(FILE)).unwrap(), V1, "still the old file while staged");
    // nothing is committed before the new version has confirmed a good start
    assert!(!commit_when_due(&d, 10_000_000, 0));
    assert_eq!(twophase::reconcile(&d, "new", 200), vec![(MIGRATION.to_string(), Phase::Confirmed)]);
    // not before the grace period is over
    assert!(!commit_when_due(&d, 300, GRACE_SECS));
    assert_eq!(std::fs::read_to_string(d.join(FILE)).unwrap(), V1);
    // after it: the new layout is THE file and the old bytes are kept next to it
    assert!(commit_when_due(&d, 200 + GRACE_SECS, GRACE_SECS));
    let now: Value = serde_json::from_slice(&std::fs::read(d.join(FILE)).unwrap()).unwrap();
    assert_eq!((now["schema"].clone(), now["heard"].clone(), now["false_wakes"].clone()), (json!(2), json!(8), json!(2)));
    assert!(now.get("wakes").is_none());
    assert_eq!(std::fs::read_to_string(d.join(format!("{FILE}.pre-{MIGRATION}"))).unwrap(), V1);
    assert_eq!(load_as(&d, "new").heard, 8, "after the commit the plain file is read");
}

#[test]
fn the_old_version_starting_again_rolls_the_counters_back_and_finds_its_own_file() {
    let d = tmp("rollback");
    old_file(&d);
    stage_if_needed(&d, "new", "old", 100).unwrap();
    bump_as(&d, "new", Count::Miss).unwrap();
    twophase::reconcile(&d, "new", 150);
    // the person went back: the old version starts, the staged data is put aside, nothing is lost
    assert_eq!(twophase::reconcile(&d, "old", 400), vec![(MIGRATION.to_string(), Phase::RolledBack)]);
    assert_eq!(std::fs::read_to_string(d.join(FILE)).unwrap(), V1);
    let kept = std::fs::read_to_string(d.join(format!("{FILE}.next.rolled-back"))).unwrap();
    assert!(kept.contains("\"misses\": 1"), "{kept}");
    // a rolled-back migration is not staged again behind the person's back, and committing it is refused
    assert_eq!(stage_if_needed(&d, "new", "old", 500).unwrap().phase, Phase::RolledBack);
    assert!(!commit_when_due(&d, 99_999_999, 0));
    assert!(twophase::commit(&d, MIGRATION, 99_999_999, 0).is_err());
}

#[test]
fn a_third_version_that_is_neither_the_old_nor_the_new_one_also_leaves_the_old_file_alone() {
    let d = tmp("third");
    old_file(&d);
    stage_if_needed(&d, "new", "old", 1).unwrap();
    assert_eq!(twophase::reconcile(&d, "other", 2), vec![(MIGRATION.to_string(), Phase::RolledBack)]);
    assert_eq!(std::fs::read_to_string(d.join(FILE)).unwrap(), V1);
}

#[test]
fn nothing_is_staged_for_a_missing_a_current_or_a_damaged_file() {
    let d = tmp("none");
    assert!(stage_if_needed(&d, "new", "old", 1).is_none());
    bump_as(&d, "new", Count::Heard).unwrap();
    assert!(stage_if_needed(&d, "new", "old", 1).is_none(), "already schema 2");
    std::fs::write(d.join(FILE), "not json at all").unwrap();
    assert!(stage_if_needed(&d, "new", "old", 1).is_none());
    assert_eq!(std::fs::read_to_string(d.join(FILE)).unwrap(), "not json at all");
    assert!(!d.join(format!("{FILE}.next")).exists() && twophase::load(&d, MIGRATION).is_none());
    // a conversion that fails changes nothing (the library's own rule, here with this store's converter)
    assert!(convert(FILE, b"{broken").is_err());
    assert_eq!(convert(FILE, b"").unwrap(), Vec::<u8>::new());
}

#[test]
fn the_boot_step_stages_first_and_never_commits_an_unconfirmed_migration() {
    let d = tmp("boot");
    old_file(&d);
    boot_step(&d, "new", "old", 100);
    assert_eq!(twophase::load(&d, MIGRATION).unwrap().phase, Phase::Staged);
    boot_step(&d, "new", "old", 100 + 10 * GRACE_SECS);
    assert_eq!(twophase::load(&d, MIGRATION).unwrap().phase, Phase::Staged, "no confirm, no commit, however late");
    assert_eq!(std::fs::read_to_string(d.join(FILE)).unwrap(), V1);
}
