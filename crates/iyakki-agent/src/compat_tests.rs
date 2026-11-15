//! Charter step 3, part 2 (docs/development/CORE-COMPATIBILITY-DESIGN.md section 8, items 2 and 3): the golden data fixtures of release alpha.2
//! (`tests/compat/alpha2/data`, one tree standing in for `/var/lib/iyakki`) and what this code must keep doing with them.
//!
//! For every versioned JSON store that has an `iyakki_schema::Spec` in this crate and in `iyakki-flow` (the approval pins of the shell have
//! the same three tests in `iyakki-shell`), plus the approvals file, the access log and the views file:
//!   * backward: the current code reads the alpha.2 file and finds what was written;
//!   * forward, unknown fields: a field a newer release added is still in the file after the store has written it;
//!   * forward, newer schema: a file with a higher `schema` is read, a write is refused with the sentence that says to update IYAKKI, and the
//!     bytes on disk do not change.
//!
//! The files are synthetic. `IYAKKI_COMPAT_RECORD=1 cargo test -p iyakki-agent --lib record_alpha2 -- --ignored` writes them from the code
//! that is in the tree (only when the release folder is made; never to make a failing test pass).

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use serde_json::{json, Value};

fn fixtures() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/compat/alpha2/data")
}

fn copy_tree(from: &Path, to: &Path) {
    fs::create_dir_all(to).unwrap();
    for e in fs::read_dir(from).unwrap().flatten() {
        let (src, dst) = (e.path(), to.join(e.file_name()));
        if src.is_dir() {
            copy_tree(&src, &dst);
        } else {
            fs::copy(&src, &dst).unwrap();
        }
    }
}

/// A fresh copy of the alpha.2 data tree.
fn data(tag: &str) -> PathBuf {
    let d = crate::testdir::unique("compat", tag);
    let _ = fs::remove_dir_all(&d);
    copy_tree(&fixtures(), &d);
    d
}

fn json_of(p: &Path) -> Value {
    serde_json::from_slice(&fs::read(p).unwrap()).unwrap()
}

// ------------------------------------------------------------------------------------------------ recording

fn put(root: &Path, rel: &str, v: &Value) {
    let p = root.join(rel);
    fs::create_dir_all(p.parent().unwrap()).unwrap();
    fs::write(p, serde_json::to_string_pretty(v).unwrap() + "\n").unwrap();
}

#[test]
#[ignore]
fn record_alpha2() {
    assert!(std::env::var_os("IYAKKI_COMPAT_RECORD").is_some(), "set IYAKKI_COMPAT_RECORD=1");
    let out = fixtures();
    let _ = fs::remove_dir_all(&out);
    fs::create_dir_all(&out).unwrap();
    // 1. the files this release writes for the stores that other tests already keep in `tests/fixtures/upgrade/current`
    let cur = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/fixtures/upgrade/current");
    for rel in ["settings.json", "dash/sources-amy.json", "dash/sources-system.json", "k3s-dev/state.json", "devices.json", "devhome/devices.json", "home/amy/Flows/orders/meta.json", "home/amy/Flows/orders/flow.json", "home/amy/Flows/orders/versions/v1.json", "inbox/amy.json"] {
        let to = out.join(rel);
        fs::create_dir_all(to.parent().unwrap()).unwrap();
        fs::copy(cur.join(rel), to).unwrap();
    }
    // 2. written by this code: the browser graphics setting, the model options, the access log, the approvals, the views
    crate::webgraphics::set_mode(&out, crate::webgraphics::Mode::Software).unwrap();
    let opts = crate::modelopts::ModelOptions { max_tokens: Some(2048), temperature: Some(0.5), top_p: None, reasoning_budget: Some(0) };
    crate::modelopts::set(&out, "provider-a", "model-x", &opts).unwrap();
    let mut v = crate::views::Views::default();
    v.add("Media", Some("media")).unwrap();
    v.save(&out, "amy");
    use iyakki_core_api::appcaps::{record_approval, CapabilityRequest};
    let req = |n: &str, why: &str, p: Option<&str>| CapabilityRequest { name: n.into(), why: why.into(), param: p.map(str::to_string), extra: Default::default() };
    let mut book = std::collections::BTreeMap::new();
    book.insert("tile/user/amy/camera-notes".to_string(), record_approval("amy", "camera-notes", "1.2.0", &"0".repeat(64), 1_700_000_000, &[req("camera.view", "show the live view", Some("front")), req("notify", "leave a note", None)]));
    let mut revoked = record_approval("amy", "radio", "2.0.0", &"1".repeat(64), 1_700_000_100, &[req("net.connect", "reach the station", Some("radio.example.org:443"))]);
    revoked.revoke("net.connect");
    book.insert("tile/user/amy/radio".to_string(), revoked);
    put(&out, "store/approvals.json", &serde_json::to_value(&book).unwrap());
    let ev = |at: i64, app: &str, cap: &str, target: &str, allowed: bool, reason: &str| iyakki_core_api::broker::AccessEvent { at, app: app.into(), person: "amy".into(), device: "dev-000001".into(), capability: cap.into(), target: target.into(), allowed, reason: reason.into() };
    let lines = [ev(1_700_000_010, "camera-notes", "camera.view", "front", true, ""), ev(1_700_000_020, "radio", "net.connect", "radio.example.org:443", false, "revoked")];
    fs::create_dir_all(out.join("access")).unwrap();
    fs::write(out.join("access/log.jsonl"), lines.iter().map(|e| serde_json::to_string(e).unwrap() + "\n").collect::<String>()).unwrap();
    // 3. small files written by hand in the layout the code documents (no public writer with a fixed clock)
    put(&out, "web-profile/web-graphics-state.json", &json!({"schema": 1, "fallbacks": [{"gpu": "1002:1681", "since": 1_700_000_000, "reason": "synthetic"}], "judged_boot": "boot-1", "offered_boot": "", "offer": true}));
    put(&out, "store/ratings-amy.json", &json!({"schema": 1, "ratings": {"tile:camera-notes": {"stars": 4, "comment": "works", "version": "1.2.0", "at": 1_700_000_000}}}));
    put(&out, "approval-pins-amy.json", &json!({"schema": 1, "pins": ["tool:read_file"]}));
    put(&out, "home/amy/Flows/_assist.json", &json!({"schema": 1, "enabled": true, "max_publish_per_day": 3, "publishes": []}));
    put(&out, "home/amy/Flows/orders/assist.json", &json!({"schema": 1, "paused": false, "cooldown_until_ms": 0, "next": 1, "events": [], "watch": null, "seen_runs": []}));
}

// ------------------------------------------------------------------------------------------------ the table of stores

struct Case {
    name: &'static str,
    /// The file in the data tree that the store owns.
    file: &'static str,
    /// Read through the store's own code; the answer is a short summary compared with `want`.
    read: fn(&Path) -> String,
    want: &'static str,
    /// Make the store write its file (a small real change). `Err` carries the store's refusal.
    write: fn(&Path) -> Result<(), String>,
    /// Whether a refused write reports an error (some stores write best-effort and stay silent).
    reports: bool,
}

fn settings_open(d: &Path) -> crate::settings::SettingsStore {
    crate::settings::SettingsStore::open(&d.join("settings.json")).unwrap()
}

fn flow_store(d: &Path) -> iyakki_flow::store::FlowStore {
    iyakki_flow::store::FlowStore::new(&d.join("home/amy/Flows"), Arc::new(|| 1_700_000_500_000))
}

fn cases() -> Vec<Case> {
    vec![
        Case {
            name: "settings",
            file: "settings.json",
            read: |d| format!("{} providers", settings_open(d).get().providers.len()),
            want: "2 providers",
            write: |d| settings_open(d).update(|_| {}).map_err(|e| e.to_string()),
            reports: true,
        },
        Case {
            name: "dashboard sources",
            file: "dash/sources-amy.json",
            read: |d| format!("{} own sources", crate::dash::store::Sources::open(d, "amy").own().len()),
            want: "2 own sources",
            write: |d| {
                let st = crate::dash::store::Sources::open(d, "amy");
                let mut c = st.own()[0].clone();
                c.name = "Renamed by the compat test".into();
                st.save(c)
            },
            reports: true,
        },
        Case {
            name: "dev cluster state",
            file: "k3s-dev/state.json",
            read: |d| {
                let st = crate::k3sdev::load_state(&crate::k3sdev::Layout::new(d)).unwrap();
                format!("{} {}", st.version, st.installed_by)
            },
            want: "v1.30.0+k3s1 admin",
            write: |d| {
                let l = crate::k3sdev::Layout::new(d);
                let st = crate::k3sdev::load_state(&l).unwrap();
                crate::k3sdev::save_state(&l, &st)
            },
            reports: true,
        },
        Case {
            name: "network devices",
            file: "devices.json",
            read: |d| format!("{} devices", crate::devwatch::DeviceWatch::open(&d.join("devices.json")).list().len()),
            want: "2 devices",
            write: |d| {
                let w = crate::devwatch::DeviceWatch::open(&d.join("devices.json"));
                w.rename("aa:bb:cc:00:00:01", "Renamed");
                Ok(())
            },
            reports: false,
        },
        Case {
            name: "paired devices",
            file: "devhome/devices.json",
            read: |d| {
                let h = crate::devmode::client::Home::at(&d.join("devhome"));
                format!("{} paired, default {}", h.devices().len(), h.default_name())
            },
            want: "2 paired, default board",
            write: |d| crate::devmode::client::Home::at(&d.join("devhome")).set_default("mac"),
            reports: true,
        },
        Case {
            name: "flow",
            file: "home/amy/Flows/orders/meta.json",
            read: |d| {
                let m = flow_store(d).meta_of("orders");
                format!("published {}, {} versions", m.published, m.versions.len())
            },
            want: "published 1, 1 versions",
            write: |d| {
                let s = flow_store(d);
                let mut f = s.get_draft("orders").unwrap();
                f.description = "edited by the compat test".into();
                s.save_draft(&f, None)
            },
            reports: true,
        },
        Case {
            name: "flow assist (one flow)",
            file: "home/amy/Flows/orders/assist.json",
            read: |d| format!("paused {}", iyakki_flow::assist_store::AssistStore::new(&d.join("home/amy/Flows"), Arc::new(|| 1_700_000_500_000)).paused("orders")),
            want: "paused false",
            write: |d| iyakki_flow::assist_store::AssistStore::new(&d.join("home/amy/Flows"), Arc::new(|| 1_700_000_500_000)).set_paused("orders", true),
            reports: true,
        },
        Case {
            name: "flow assist (global)",
            file: "home/amy/Flows/_assist.json",
            read: |d| format!("cap {}", iyakki_flow::assist_store::AssistStore::new(&d.join("home/amy/Flows"), Arc::new(|| 1_700_000_500_000)).global().max_publish_per_day),
            want: "cap 3",
            write: |d| iyakki_flow::assist_store::AssistStore::new(&d.join("home/amy/Flows"), Arc::new(|| 1_700_000_500_000)).set_global_cap(5),
            reports: true,
        },
        Case {
            name: "browser graphics setting",
            file: "web-profile/web-graphics.json",
            read: |d| {
                let (m, ro) = crate::webgraphics::load_mode(d);
                format!("{} ro={}", m.id(), ro.is_some())
            },
            want: "software ro=false",
            write: |d| crate::webgraphics::set_mode(d, crate::webgraphics::Mode::Hardware),
            reports: true,
        },
        Case {
            name: "browser graphics memory",
            file: "web-profile/web-graphics-state.json",
            read: |d| {
                let s = crate::webgraphics::load_state(d);
                format!("{} fallbacks, offer {}", s.fallbacks.len(), s.offer)
            },
            want: "1 fallbacks, offer true",
            // a change of the setting forgets the remembered fall backs and writes the memory file
            write: |d| crate::webgraphics::set_mode(d, crate::webgraphics::Mode::Automatic),
            reports: false,
        },
        Case {
            name: "model options",
            file: "model-options.json",
            read: |d| format!("{:?}", crate::modelopts::get(d, "provider-a", "model-x").max_tokens),
            want: "Some(2048)",
            write: |d| {
                let mut o = crate::modelopts::get(d, "provider-a", "model-x");
                o.max_tokens = Some(1024);
                crate::modelopts::set(d, "provider-a", "model-x", &o)
            },
            reports: true,
        },
        Case {
            name: "package ratings",
            file: "store/ratings-amy.json",
            read: |d| {
                let r = crate::store::ratings::Ratings::load(&d.join("store"), "amy");
                format!("{} ratings, {} stars", r.len(), r.get("tile:camera-notes").map_or(0, |x| x.stars))
            },
            want: "1 ratings, 4 stars",
            write: |d| crate::store::ratings::Ratings::load(&d.join("store"), "amy").set_stars("tile:camera-notes", 5, "1.2.0"),
            reports: true,
        },
    ]
}

#[test]
fn backward_every_store_reads_the_alpha2_file_and_finds_what_was_written() {
    for c in cases() {
        let d = data(&format!("read-{}", c.name.replace([' ', '(', ')'], "-")));
        assert!(d.join(c.file).is_file(), "{}: the fixture {} is missing", c.name, c.file);
        let before = fs::read(d.join(c.file)).unwrap();
        assert_eq!((c.read)(&d), c.want, "{}", c.name);
        assert_eq!(json_of(&d.join(c.file))["schema"], 1, "{}: the alpha.2 file carries schema 1", c.name);
        assert_eq!(fs::read(d.join(c.file)).unwrap(), before, "{}: reading a current file changes nothing", c.name);
        let backups: Vec<_> = fs::read_dir(d.join(c.file).parent().unwrap()).unwrap().flatten().filter(|e| e.file_name().to_string_lossy().contains(".pre-v")).collect();
        assert!(backups.is_empty(), "{}: a current file needs no backup", c.name);
    }
}

#[test]
fn every_spec_in_the_table_is_current_and_valid() {
    for s in [crate::settings::SCHEMA, crate::dash::store::SOURCES_SCHEMA, crate::k3sdev::STATE_SCHEMA, crate::devmode::client::DEVICES_SCHEMA, crate::devwatch::SCHEMA, crate::webgraphics::SETTING_SCHEMA, crate::webgraphics::STATE_SCHEMA, crate::modelopts::SCHEMA, crate::store::ratings::SCHEMA, iyakki_flow::store::SCHEMA, iyakki_flow::assist_store::ASSIST_SCHEMA] {
        s.check().unwrap();
        assert_eq!(s.current, 1, "{}: a bump needs an alpha.3 folder of fixtures and a migration test", s.name);
    }
}

#[test]
fn forward_a_field_a_newer_release_added_is_still_there_after_the_store_wrote_its_file() {
    for c in cases() {
        let d = data(&format!("extra-{}", c.name.replace([' ', '(', ')'], "-")));
        let p = d.join(c.file);
        let mut v = json_of(&p);
        v.as_object_mut().unwrap().insert("zz_added_in_a_newer_release".into(), json!({"n": [1, 2, 3]}));
        fs::write(&p, serde_json::to_vec_pretty(&v).unwrap()).unwrap();
        (c.write)(&d).unwrap_or_else(|e| panic!("{}: {e}", c.name));
        let after = json_of(&p);
        assert_eq!(after["zz_added_in_a_newer_release"], json!({"n": [1, 2, 3]}), "{}: an unknown field was dropped on write (charter rule 2)", c.name);
        assert_eq!(after["schema"], 1, "{}", c.name);
    }
}

#[test]
fn forward_a_file_with_a_newer_schema_is_read_never_written_and_the_refusal_says_to_update() {
    for c in cases() {
        let d = data(&format!("newer-{}", c.name.replace([' ', '(', ')'], "-")));
        let p = d.join(c.file);
        let mut v = json_of(&p);
        v["schema"] = json!(99);
        v.as_object_mut().unwrap().insert("zz_added_in_a_newer_release".into(), json!(true));
        fs::write(&p, serde_json::to_vec_pretty(&v).unwrap()).unwrap();
        let before = fs::read(&p).unwrap();
        let _ = (c.read)(&d);
        let res = (c.write)(&d);
        if c.reports {
            let e = res.expect_err(&format!("{}: a write over a newer file must be refused", c.name));
            assert!(e.contains("newer IYAKKI") && e.contains("update IYAKKI"), "{}: the refusal must say to update: {e}", c.name);
        }
        assert_eq!(fs::read(&p).unwrap(), before, "{}: a newer file was rewritten", c.name);
        let copies: Vec<_> = fs::read_dir(p.parent().unwrap()).unwrap().flatten().filter(|e| e.file_name().to_string_lossy().contains(".corrupt-")).collect();
        assert!(copies.is_empty(), "{}: a newer file must not be treated as damaged", c.name);
    }
}

// ------------------------------------------------------------------------------------------------ approvals, access log, views

#[test]
fn backward_the_approvals_file_is_read_by_the_broker_and_by_the_settings_list() {
    use iyakki_core_api::broker::ApprovalLookup;
    let d = data("approvals");
    let look = crate::accessbroker::FileLookup::for_data(&d);
    let a = look.approval("tile/user/amy/camera-notes").expect("the alpha.2 approval is found");
    assert!(a.holds("camera.view", Some("front")) && a.holds("notify", None));
    assert!(!a.holds("camera.view", Some("back")), "a parameter that was not approved is not held");
    let r = look.approval("tile/user/amy/radio").unwrap();
    assert!(!r.holds("net.connect", Some("radio.example.org:443")), "a revoked capability stays revoked");
    let held = crate::accessbroker::held_list(&d);
    assert_eq!(held.iter().map(|h| (h.app.as_str(), h.holds.len(), h.revoked.len())).collect::<Vec<_>>(), vec![("camera-notes", 2, 0), ("radio", 0, 1)]);
}

#[test]
fn forward_an_approval_entry_with_an_added_field_is_read_and_kept_by_a_revoke() {
    let d = data("approvals-extra");
    let p = d.join("store/approvals.json");
    let mut v = json_of(&p);
    v["tile/user/amy/camera-notes"]["added_in_a_newer_release"] = json!({"since": "alpha.9"});
    fs::write(&p, serde_json::to_vec_pretty(&v).unwrap()).unwrap();
    assert!(crate::accessbroker::revoke(&d, "tile/user/amy/camera-notes", "notify").unwrap());
    // the change lifts the alpha.2 bare map to schema 1 (`{schema, approvals}`), every entry and field kept
    let after = json_of(&p);
    assert_eq!(after["schema"], 1);
    let after = after["approvals"].clone();
    assert_eq!(after["tile/user/amy/camera-notes"]["added_in_a_newer_release"], json!({"since": "alpha.9"}));
    assert_eq!(after["tile/user/amy/camera-notes"]["revoked"][0]["name"], "notify");
    assert!(after.get("tile/user/amy/radio").is_some(), "the other entries are untouched");
}

#[test]
fn backward_the_access_log_lines_of_alpha2_are_read_and_a_newer_line_with_extra_fields_is_too() {
    let d = data("accesslog");
    let log = crate::accessbroker::FileLog::open(&d);
    let mut rows = log.recent(10);
    rows.reverse();
    assert_eq!(rows.len(), 2);
    assert_eq!((rows[0].app.as_str(), rows[0].allowed, rows[1].reason.as_str()), ("camera-notes", true, "revoked"));
    // a line from a newer release (extra field) and one that is not JSON at all: the reader skips what it cannot read, keeps the rest
    let p = d.join("access/log.jsonl");
    let mut text = fs::read_to_string(&p).unwrap();
    text.push_str("{\"at\":1700000030,\"app\":\"x\",\"person\":\"amy\",\"device\":\"d\",\"capability\":\"notify\",\"target\":\"\",\"allowed\":true,\"reason\":\"\",\"added_in_a_newer_release\":1}\nnot json\n");
    fs::write(&p, text).unwrap();
    let rows = crate::accessbroker::FileLog::open(&d).recent(10);
    assert_eq!(rows.len(), 3);
}

#[test]
fn backward_the_views_file_of_alpha2_loads_and_forward_added_fields_survive_a_save() {
    use crate::views::Views;
    let d = data("views");
    let v = Views::load(&d, "amy");
    assert_eq!(v.views.iter().map(|x| x.name.as_str()).collect::<Vec<_>>(), vec!["Main", "Media"]);
    assert_eq!(v.views[1].tiles, vec!["media".to_string(), "radio".to_string()]);
    let p = d.join("views-amy.json");
    let mut j = json_of(&p);
    j["added_in_a_newer_release"] = json!(7);
    j["views"][1]["added_too"] = json!("x");
    fs::write(&p, serde_json::to_vec_pretty(&j).unwrap()).unwrap();
    let mut v = Views::load(&d, "amy");
    v.set_locked(2, true);
    v.save(&d, "amy");
    let after = json_of(&p);
    assert_eq!(after["added_in_a_newer_release"], 7, "an unknown field of the file survives a save");
    assert_eq!(after["views"][1]["added_too"], "x", "an unknown field of a view survives a save");
    assert_eq!(after["views"][1]["locked"], true);
}

#[test]
fn a_damaged_file_of_alpha2_is_set_aside_and_the_store_starts_with_defaults() {
    let d = data("damaged");
    let p = d.join("model-options.json");
    fs::write(&p, b"{{ not json").unwrap();
    assert_eq!(crate::modelopts::get(&d, "provider-a", "model-x").max_tokens, None);
    let copies = fs::read_dir(&d).unwrap().flatten().filter(|e| e.file_name().to_string_lossy().starts_with("model-options.json.corrupt-")).count();
    assert_eq!(copies, 1, "the bytes are kept beside the new file");
}

#[test]
fn backward_the_inbox_of_alpha2_is_kept_as_it_is_and_a_newer_one_is_not_overwritten() {
    let d = data("inbox");
    let p = d.join("inbox/amy.json");
    let before = fs::read(&p).unwrap();
    crate::inbox::Inbox::new(&d.join("inbox")).maintain("amy", 1_700_000_100).unwrap();
    assert_eq!(fs::read(&p).unwrap(), before, "a current inbox needs no change");
    let mut v = json_of(&p);
    v["version"] = json!(99);
    fs::write(&p, serde_json::to_vec_pretty(&v).unwrap()).unwrap();
    let newer = fs::read(&p).unwrap();
    let _ = crate::inbox::Inbox::new(&d.join("inbox")).maintain("amy", 1_700_000_200);
    assert_eq!(fs::read(&p).unwrap(), newer, "an inbox from a newer release is never rewritten");
}
