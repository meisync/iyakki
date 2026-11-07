//! Tests of the person's own ratings file: set, replace, forget, the schema stamp, a damaged file, credentials, the length limit.

use super::*;
use std::sync::atomic::{AtomicU32, Ordering};

fn dir(tag: &str) -> PathBuf {
    static N: AtomicU32 = AtomicU32::new(0);
    let d = std::env::temp_dir().join(format!("iyakki-ratings-{}-{tag}-{}", std::process::id(), N.fetch_add(1, Ordering::Relaxed)));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

#[test]
fn ratings_are_set_replaced_read_back_and_forgotten_per_account() {
    let d = dir("basic");
    let mut r = Ratings::load(&d, "amy");
    assert!(r.is_empty() && r.get("tool:shout").is_none());
    r.set_stars("tool:shout", 4, "1.0.0").unwrap();
    r.set_comment("tool:shout", "  Handy   for\nloud messages ", "1.0.0").unwrap();
    let got = r.get("tool:shout").unwrap();
    assert_eq!((got.stars, got.comment.as_str(), got.version.as_str()), (4, "Handy for loud messages", "1.0.0"));
    // replace: the newest stars and comment win, the other part stays
    r.set_stars("tool:shout", 2, "1.1.0").unwrap();
    assert_eq!((r.get("tool:shout").unwrap().stars, r.get("tool:shout").unwrap().comment.as_str()), (2, "Handy for loud messages"));
    r.set_comment("tool:shout", "Changed my mind", "1.1.0").unwrap();
    // it is on disk for amy and nobody else
    let again = Ratings::load(&d, "amy");
    assert_eq!((again.get("tool:shout").unwrap().stars, again.get("tool:shout").unwrap().comment.as_str()), (2, "Changed my mind"));
    assert!(Ratings::load(&d, "bob").is_empty());
    assert_ne!(Ratings::path_of(&d, "amy"), Ratings::path_of(&d, "bob"));
    // taking the stars away keeps the comment; taking both away drops the entry
    r.set_stars("tool:shout", 0, "1.1.0").unwrap();
    assert_eq!(r.get("tool:shout").unwrap().stars, 0);
    r.set_comment("tool:shout", "", "1.1.0").unwrap();
    assert!(r.get("tool:shout").is_none());
    // forget removes both for good
    r.set_stars("skill:notes", 5, "2").unwrap();
    r.set_comment("skill:notes", "Great", "2").unwrap();
    assert!(r.forget("skill:notes").unwrap());
    assert!(!r.forget("skill:notes").unwrap(), "nothing left to forget");
    assert!(Ratings::load(&d, "amy").get("skill:notes").is_none());
    assert!(r.set_stars("tool:x", 6, "1").is_err(), "stars are 1 to 5");
}

#[test]
fn the_file_carries_the_schema_stamp_is_private_and_keeps_unknown_fields() {
    SCHEMA.check().unwrap();
    let d = dir("stamp");
    let mut r = Ratings::load(&d, "amy");
    r.set_stars("tool:shout", 3, "1").unwrap();
    let p = Ratings::path_of(&d, "amy");
    let v: serde_json::Value = serde_json::from_slice(&std::fs::read(&p).unwrap()).unwrap();
    assert_eq!(v["schema"], 1);
    assert_eq!(v["ratings"]["tool:shout"]["stars"], 3);
    use std::os::unix::fs::PermissionsExt;
    assert_eq!(std::fs::metadata(&p).unwrap().permissions().mode() & 0o777, 0o600);
    // a field a later release added is kept when this one writes again
    let mut v = v;
    v["future"] = serde_json::json!({"a": 1});
    std::fs::write(&p, serde_json::to_vec(&v).unwrap()).unwrap();
    let mut r = Ratings::load(&d, "amy");
    r.set_stars("tool:shout", 4, "1").unwrap();
    let v: serde_json::Value = serde_json::from_slice(&std::fs::read(&p).unwrap()).unwrap();
    assert_eq!(v["future"]["a"], 1);
    // a file from a newer release is read but never written over
    let mut newer = v.clone();
    newer["schema"] = serde_json::json!(9);
    std::fs::write(&p, serde_json::to_vec(&newer).unwrap()).unwrap();
    let mut r = Ratings::load(&d, "amy");
    assert_eq!(r.get("tool:shout").unwrap().stars, 4);
    assert!(r.set_stars("tool:shout", 1, "1").unwrap_err().contains("newer IYAKKI"));
    assert_eq!(serde_json::from_slice::<serde_json::Value>(&std::fs::read(&p).unwrap()).unwrap()["schema"], 9);
}

#[test]
fn a_damaged_file_is_moved_aside_and_the_account_starts_empty() {
    let d = dir("damaged");
    let p = Ratings::path_of(&d, "amy");
    std::fs::write(&p, b"{ not json at all").unwrap();
    let mut r = Ratings::load(&d, "amy");
    assert!(r.is_empty());
    assert!(!p.exists(), "the damaged file is not left in place");
    let kept: Vec<_> = std::fs::read_dir(&d).unwrap().flatten().filter(|e| e.file_name().to_string_lossy().contains(".corrupt-")).collect();
    assert_eq!(kept.len(), 1, "one copy is kept for the person");
    r.set_stars("tool:shout", 5, "1").unwrap();
    assert_eq!(Ratings::load(&d, "amy").get("tool:shout").unwrap().stars, 5);
    // values a hand-made file could not have come from the tile are cut back
    std::fs::write(&p, serde_json::json!({"schema": 1, "ratings": {"a:b": {"stars": 9}, "c:d": {"stars": 2, "comment": "x".repeat(900)}, "e:f": {"stars": 0}}}).to_string()).unwrap();
    let r = Ratings::load(&d, "amy");
    assert!(r.get("a:b").is_none() && r.get("e:f").is_none());
    assert_eq!(r.get("c:d").unwrap().comment.chars().count(), MAX_COMMENT);
}

#[test]
fn a_comment_with_a_credential_is_refused_with_a_plain_sentence_and_nothing_is_stored() {
    let d = dir("cred");
    let mut r = Ratings::load(&d, "amy");
    for bad in ["my password: hunter2hunter2 works", "key sk-abcdefghijklmnopqrstuvwxyz0123456789 here", "AKIAABCDEFGHIJKLMNOP is mine"] {
        let e = r.set_comment("tool:shout", bad, "1").unwrap_err();
        assert!(e.contains("credential") && !e.contains("hunter2") && !e.contains("abcdefghij") && !e.contains("AKIAABCDEF"), "the refusal never repeats the secret: {e}");
    }
    assert!(r.is_empty() && !Ratings::path_of(&d, "amy").exists());
    r.set_comment("tool:shout", "My password manager likes it", "1").unwrap();
}

#[test]
fn a_comment_is_at_most_500_characters_counted_as_characters() {
    let d = dir("len");
    let mut r = Ratings::load(&d, "amy");
    r.set_comment("tool:shout", &"é".repeat(MAX_COMMENT), "1").unwrap();
    assert_eq!(r.get("tool:shout").unwrap().comment.chars().count(), 500);
    let e = r.set_comment("tool:shout", &"é".repeat(MAX_COMMENT + 1), "1").unwrap_err();
    assert!(e.contains("500"), "{e}");
    assert_eq!(r.get("tool:shout").unwrap().comment.chars().count(), 500, "the refused text changes nothing");
}

#[test]
fn the_key_has_no_version_so_a_rating_is_the_same_before_and_after_an_update() {
    let d = dir("update");
    let mut r = Ratings::load(&d, "amy");
    let key = super::super::organize::item_key("tool", "shout");
    r.set_stars(&key, 4, "1.0.0").unwrap();
    r.set_comment(&key, "Good", "1.0.0").unwrap();
    // a new version of the package: same key, the rating is still there and still says which version it was given for
    let r = Ratings::load(&d, "amy");
    assert_eq!((r.get(&key).unwrap().stars, r.get(&key).unwrap().version.as_str()), (4, "1.0.0"));
}
