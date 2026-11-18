//! The address-bar short word chat tools with a fake backend and with the real file: the list is free, changes ask on a card, and the
//! other settings in the person's browser file are left as they were.

use std::sync::{Arc, Mutex};

use serde_json::json;

use crate::agent::{Risk, Tools};
use crate::browsershort::*;

#[derive(Default)]
struct Fake {
    words: Mutex<Vec<(String, String)>>,
}
impl Backend for Fake {
    fn list(&self) -> Vec<(String, String)> {
        self.words.lock().unwrap().clone()
    }
    fn add(&self, w: &str, t: &str) -> Result<(), String> {
        let mut v = self.words.lock().unwrap();
        v.retain(|x| x.0 != w);
        v.push((w.into(), t.into()));
        Ok(())
    }
    fn remove(&self, w: &str) -> Result<(), String> {
        let mut v = self.words.lock().unwrap();
        let n = v.len();
        v.retain(|x| x.0 != w);
        if v.len() == n { Err("no such word".into()) } else { Ok(()) }
    }
}

fn rig() -> (ShortTools, Arc<Fake>) {
    let f = Arc::new(Fake::default());
    (ShortTools::new(Arc::new(crate::assign::tests_support::NoTools), f.clone()), f)
}

#[test]
fn listing_is_free_and_changes_ask_every_time() {
    let (t, _) = rig();
    let defs = t.defs();
    let risk = |n: &str| defs.iter().find(|d| d.spec.name == n).map(|d| d.risk);
    assert_eq!(risk(LIST_TOOL), Some(Risk::Safe));
    assert_eq!(risk(ADD_TOOL), Some(Risk::Ask));
    assert_eq!(risk(REMOVE_TOOL), Some(Risk::Ask));
    for n in WRITE_TOOLS {
        assert!(!t.pre_approved(n) && !t.approved_always(n));
    }
    assert!(t.call(LIST_TOOL, &json!({})).unwrap().contains("No short words of your own") );
}

#[test]
fn a_card_names_the_word_and_address_and_the_call_changes_the_list() {
    let (t, f) = rig();
    let a = json!({"word": "YT2", "address": "https://example.org/s?q=%s"});
    let card = t.preview(ADD_TOOL, &a).unwrap();
    assert!(card.contains("yt2") && card.contains("https://example.org/s?q=%s"), "{card}");
    assert!(t.describe(ADD_TOOL, &a).starts_with("Add the short word yt2"));
    let out = t.call(ADD_TOOL, &a).unwrap();
    assert!(out.starts_with("Done.") && out.contains("yt2 = https://example.org/s?q=%s"), "{out}");
    assert_eq!(f.list().len(), 1);
    // changing an existing one says what it was; a built-in name says it replaces the built-in
    let card = t.preview(ADD_TOOL, &json!({"word": "yt2", "address": "https://other.org/?q=%s"})).unwrap();
    assert!(card.contains("now https://example.org/s?q=%s"), "{card}");
    assert!(t.preview(ADD_TOOL, &json!({"word": "yt", "address": "https://other.org/?q=%s"})).unwrap().contains("built-in"));
    let card = t.preview(REMOVE_TOOL, &json!({"word": "yt2"})).unwrap();
    assert!(card.contains("Remove the short word yt2"));
    t.call(REMOVE_TOOL, &json!({"word": "yt2"})).unwrap();
    assert!(f.list().is_empty());
}

#[test]
fn bad_words_and_addresses_are_refused_before_anything_changes() {
    let (t, f) = rig();
    for a in [
        json!({"word": "Y T", "address": "https://a.org/?q=%s"}),
        json!({"word": "toolongtoolongword", "address": "https://a.org/?q=%s"}),
        json!({"word": "ok", "address": "ftp://a.org/?q=%s"}),
        json!({"word": "ok", "address": "https://a.org/?q="}),
        json!({"word": "ok", "address": "https://a.org/?q=%s%s"}),
        json!({"word": "ok", "address": "https://a.org/ ?q=%s"}),
        json!({}),
    ] {
        assert!(t.refusal(ADD_TOOL, &a).is_some(), "{a}");
        assert!(t.call(ADD_TOOL, &a).is_err(), "{a}");
    }
    assert!(t.refusal(REMOVE_TOOL, &json!({"word": "yt"})).unwrap().contains("built in"));
    assert!(t.call(REMOVE_TOOL, &json!({"word": "nope"})).is_err());
    assert!(f.list().is_empty());
}

#[test]
fn the_real_backend_edits_only_the_short_words_key_audits_and_keeps_fifty_at_most() {
    let dir = std::env::temp_dir().join(format!("iyakki-short-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("browser-ann.json"), r#"{"js_all":false,"images":true,"search":"duck"}"#).unwrap();
    let log = Arc::new(Mutex::new(Vec::<String>::new()));
    let l2 = log.clone();
    let b = FileBackend { data: dir.clone(), user: "ann".into(), audit: Arc::new(move |a, d| l2.lock().unwrap().push(format!("{a}: {d}"))) };
    b.add("yt2", "https://example.org/s?q=%s").unwrap();
    let v: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(dir.join("browser-ann.json")).unwrap()).unwrap();
    assert_eq!(v["js_all"], false, "other settings are untouched");
    assert_eq!(v["search"], "duck");
    assert_eq!(v["short_words"]["yt2"], "https://example.org/s?q=%s");
    assert_eq!(b.list(), vec![("yt2".to_string(), "https://example.org/s?q=%s".to_string())]);
    assert!(log.lock().unwrap()[0].contains("yt2"));
    b.remove("yt2").unwrap();
    assert!(b.list().is_empty());
    assert!(b.remove("yt2").is_err());
    for i in 0..MAX_WORDS {
        b.add(&format!("w{i}"), "https://a.org/?q=%s").unwrap();
    }
    assert!(b.add("extra", "https://a.org/?q=%s").unwrap_err().contains("at most"));
    b.add("w0", "https://b.org/?q=%s").unwrap(); // changing one is fine at the limit
    // a damaged file is not overwritten
    std::fs::write(dir.join("browser-ann.json"), "{not json").unwrap();
    assert!(b.remove("w1").is_err());
    assert_eq!(std::fs::read_to_string(dir.join("browser-ann.json")).unwrap(), "{not json");
    let _ = std::fs::remove_dir_all(&dir);
}
