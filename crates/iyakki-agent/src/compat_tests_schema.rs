//! Compatibility findings F-2 (approvals file), F-3 (access log) and F-4 (views file) of docs/development/CORE-COMPATIBILITY-DESIGN.md
//! section 9: schema stamp, unknown fields kept, a damaged file is never read as empty and overwritten, a newer file is read-only.
//! The golden alpha.2 files in `tests/compat/alpha2/data` are copied and never edited.

use std::fs;
use std::path::{Path, PathBuf};

use iyakki_core_api::appcaps::ApprovalBook;
use serde_json::{json, Value};

fn data(tag: &str) -> PathBuf {
    fn copy(from: &Path, to: &Path) {
        fs::create_dir_all(to).unwrap();
        for e in fs::read_dir(from).unwrap().flatten() {
            if e.path().is_dir() {
                copy(&e.path(), &to.join(e.file_name()));
            } else {
                fs::copy(e.path(), to.join(e.file_name())).unwrap();
            }
        }
    }
    let d = crate::testdir::unique("compat-schema", tag);
    let _ = fs::remove_dir_all(&d);
    copy(&Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/compat/alpha2/data"), &d);
    d
}

fn json_of(p: &Path) -> Value {
    serde_json::from_slice(&fs::read(p).unwrap()).unwrap()
}

fn names(dir: &Path, prefix: &str) -> Vec<PathBuf> {
    fs::read_dir(dir).unwrap().flatten().map(|e| e.path()).filter(|p| p.file_name().unwrap().to_string_lossy().starts_with(prefix)).collect()
}

fn an_approval(app: &str) -> iyakki_core_api::appcaps::Approval {
    let r = iyakki_core_api::appcaps::CapabilityRequest { name: "notify".into(), why: "x".into(), param: None, extra: Default::default() };
    iyakki_core_api::appcaps::record_approval("amy", app, "1", "00", 5, &[r])
}

// ---------------------------------------------------------------------------------------------- F-2 approvals

#[test]
fn approvals_alpha2_bare_map_migrates_to_schema_1_keeping_a_copy_and_every_entry() {
    let d = data("ap-migrate");
    let p = d.join("store/approvals.json");
    let before = fs::read(&p).unwrap();
    let store = crate::store::Store::open(&d);
    let mut book = crate::store::approvals::FileBook(&store);
    assert!(book.get("tile/user/amy/radio").is_some(), "a read of the old layout works");
    assert_eq!(fs::read(&p).unwrap(), before, "reading changes nothing");
    book.put("tile/user/amy/new", an_approval("new"));
    let after = json_of(&p);
    assert_eq!(after["schema"], 1);
    assert_eq!(after["approvals"].as_object().unwrap().len(), 3, "the two old entries and the new one");
    assert_eq!(fs::read(d.join("store/approvals.json.pre-v1")).unwrap(), before, "the old bytes are kept");
    // the broker lookup and the settings list read the new layout
    assert!(crate::accessbroker::held_list(&d).len() == 3);
}

#[test]
fn approvals_damaged_file_is_set_aside_with_its_bytes_and_not_silently_replaced() {
    let d = data("ap-corrupt");
    let p = d.join("store/approvals.json");
    let good = fs::read(&p).unwrap();
    let broken = [&good[..good.len() / 2], b"\x00{{"].concat();
    fs::write(&p, &broken).unwrap();
    let store = crate::store::Store::open(&d);
    let mut book = crate::store::approvals::FileBook(&store);
    book.put("tile/user/amy/new", an_approval("new"));
    let aside = names(&d.join("store"), "approvals.json.corrupt-");
    assert_eq!(aside.len(), 1, "the damaged file is moved aside");
    assert_eq!(fs::read(&aside[0]).unwrap(), broken, "with its original bytes, so the other approvals can be recovered");
    // (the audit sentence goes through the process-wide queue of iyakki_schema, which parallel tests also drain: not asserted here)
    assert_eq!(json_of(&p)["approvals"].as_object().unwrap().len(), 1);
}

#[test]
fn approvals_file_of_a_newer_schema_is_read_but_never_written() {
    let d = data("ap-newer");
    let p = d.join("store/approvals.json");
    let mut v = json!({"schema": 7, "approvals": json_of(&p), "added_later": true});
    v["approvals"]["tile/user/amy/radio"]["extra_field"] = json!(1);
    fs::write(&p, serde_json::to_vec_pretty(&v).unwrap()).unwrap();
    let before = fs::read(&p).unwrap();
    let store = crate::store::Store::open(&d);
    let mut book = crate::store::approvals::FileBook(&store);
    assert!(book.get("tile/user/amy/camera-notes").is_some(), "readable");
    book.put("tile/user/amy/new", an_approval("new"));
    book.remove("tile/user/amy/radio");
    assert_eq!(fs::read(&p).unwrap(), before, "bytes unchanged");
    assert!(names(&d.join("store"), "approvals.json.").is_empty(), "no copy, not treated as damaged");
}

#[test]
fn approvals_unknown_fields_of_the_root_and_of_entries_survive_a_change() {
    let d = data("ap-extra");
    let store = crate::store::Store::open(&d);
    let mut book = crate::store::approvals::FileBook(&store);
    book.put("tile/user/amy/a", an_approval("a")); // migrates
    let p = d.join("store/approvals.json");
    let mut v = json_of(&p);
    v["added_later"] = json!({"x": 1});
    v["approvals"]["tile/user/amy/a"]["scope_hint"] = json!("h");
    fs::write(&p, serde_json::to_vec_pretty(&v).unwrap()).unwrap();
    book.put("tile/user/amy/b", an_approval("b"));
    let after = json_of(&p);
    assert_eq!(after["added_later"], json!({"x": 1}));
    assert_eq!(after["approvals"]["tile/user/amy/a"]["scope_hint"], "h");
}

#[test]
fn approvals_entries_that_do_not_fit_are_set_aside_not_read_as_empty() {
    let d = data("ap-nofit");
    let p = d.join("store/approvals.json");
    fs::write(&p, br#"{"tile/user/amy/x": {"nonsense": true}}"#).unwrap();
    let store = crate::store::Store::open(&d);
    crate::store::approvals::FileBook(&store).put("tile/user/amy/new", an_approval("new"));
    assert!(!names(&d.join("store"), "approvals.json.corrupt-").is_empty());
}

// ---------------------------------------------------------------------------------------------- F-3 access log

#[test]
fn access_log_lines_carry_a_version_and_a_trim_keeps_fields_of_newer_lines() {
    use iyakki_core_api::broker::{AccessEvent, AccessSink};
    let d = data("log");
    let p = d.join("access/log.jsonl");
    // a newer line with an unknown field, then enough new lines to force a trim
    let mut text = fs::read_to_string(&p).unwrap();
    text.push_str("{\"at\":1700000090,\"app\":\"keep\",\"person\":\"amy\",\"device\":\"d\",\"capability\":\"notify\",\"target\":\"\",\"allowed\":true,\"reason\":\"\",\"v\":9,\"from_a_newer_release\":[1]}\n");
    fs::write(&p, text).unwrap();
    let log = crate::accessbroker::FileLog::with_cap(&d, 10);
    for i in 0..30 {
        log.record(AccessEvent { at: 1_700_001_000 + i, app: "n".into(), person: "amy".into(), device: "d".into(), capability: "notify".into(), target: String::new(), allowed: true, reason: String::new() });
    }
    let text = fs::read_to_string(&p).unwrap();
    assert!(text.lines().all(|l| serde_json::from_str::<Value>(l).unwrap()["v"].as_u64().is_some()), "every line written by this release has v");
    let log2 = crate::accessbroker::FileLog::with_cap(&d, 3);
    log2.record(AccessEvent { at: 1_700_002_000, app: "last".into(), person: "amy".into(), device: "d".into(), capability: "notify".into(), target: String::new(), allowed: true, reason: String::new() });
    // the golden alpha.2 lines (no v) and the newer line are read; a trim of a log holding a newer line keeps its extra field
    let d2 = data("log2");
    let p2 = d2.join("access/log.jsonl");
    let mut t = String::from("{\"at\":1,\"app\":\"newer\",\"person\":\"amy\",\"device\":\"d\",\"capability\":\"notify\",\"target\":\"\",\"allowed\":true,\"reason\":\"\",\"v\":9,\"from_a_newer_release\":[1]}\n");
    t.push_str(&fs::read_to_string(&p2).unwrap());
    fs::write(&p2, t).unwrap();
    let l = crate::accessbroker::FileLog::with_cap(&d2, 1);
    for i in 0..3 {
        l.record(AccessEvent { at: 10 + i, app: "z".into(), person: "amy".into(), device: "d".into(), capability: "notify".into(), target: String::new(), allowed: true, reason: String::new() });
    }
    // cap 1: trim at 2 lines; the surviving lines are written back byte for byte
    let kept = fs::read_to_string(&p2).unwrap();
    assert!(kept.lines().count() <= 3);
    let d3 = data("log3");
    let p3 = d3.join("access/log.jsonl");
    let newer = "{\"at\":99,\"app\":\"newer\",\"person\":\"amy\",\"device\":\"d\",\"capability\":\"notify\",\"target\":\"\",\"allowed\":true,\"reason\":\"\",\"v\":9,\"from_a_newer_release\":[1]}\n";
    fs::write(&p3, newer.repeat(12)).unwrap();
    let l3 = crate::accessbroker::FileLog::with_cap(&d3, 10);
    l3.record(AccessEvent { at: 100, app: "x".into(), person: "amy".into(), device: "d".into(), capability: "notify".into(), target: String::new(), allowed: true, reason: String::new() });
    let out = fs::read_to_string(&p3).unwrap();
    assert!(out.lines().count() <= 11);
    assert!(out.contains("from_a_newer_release"), "a trim keeps the fields of lines it did not write");
}

// ---------------------------------------------------------------------------------------------- F-4 views

#[test]
fn views_file_is_stamped_on_save_and_the_alpha2_layout_migrates_with_a_copy() {
    use crate::views::Views;
    let d = data("views");
    let p = d.join("views-amy.json");
    let before = fs::read(&p).unwrap();
    let v = Views::load(&d, "amy");
    assert_eq!(v.views.len(), 2);
    assert_eq!(json_of(&p)["schema"], 1, "lifted on first load");
    assert_eq!(fs::read(d.join("views-amy.json.pre-v1")).unwrap(), before, "old bytes kept");
    v.save(&d, "amy");
    assert_eq!(json_of(&p)["schema"], 1);
}

#[test]
fn views_file_of_a_newer_schema_is_read_and_not_written_and_a_damaged_one_is_set_aside() {
    use crate::views::Views;
    let d = data("views-newer");
    let p = d.join("views-amy.json");
    let mut j = json_of(&p);
    j["schema"] = json!(5);
    j["later"] = json!(true);
    fs::write(&p, serde_json::to_vec_pretty(&j).unwrap()).unwrap();
    let before = fs::read(&p).unwrap();
    let mut v = Views::load(&d, "amy");
    assert_eq!(v.views.len(), 2);
    v.set_locked(2, true);
    v.save(&d, "amy");
    assert_eq!(fs::read(&p).unwrap(), before);
    fs::write(&p, b"{ broken").unwrap();
    assert_eq!(Views::load(&d, "amy"), Views::default());
    let aside = names(&d, "views-amy.json.corrupt-");
    assert_eq!(aside.len(), 1);
    assert_eq!(fs::read(&aside[0]).unwrap(), b"{ broken");
}
