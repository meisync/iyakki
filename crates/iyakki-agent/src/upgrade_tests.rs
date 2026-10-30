//! Upgrade tests (v1.0.0 gate item 5): what an install made by alpha.1 left on the data volume is read by this code without losing
//! anything. The fixtures are `tests/fixtures/upgrade/alpha1` (synthetic; the layout is what alpha.1 wrote, taken from the code at
//! the tag `v0.1.0-alpha.1`) and `tests/fixtures/upgrade/current` (what this release writes). Every store is tested for the same
//! five things: nothing lost and new fields defaulted; one backup `*.pre-vN`; a second run changes nothing; a file from a NEWER
//! release is never written; a damaged file is moved aside and only that store starts empty.
//! docs/development/DATA-LAYOUT-AND-UPGRADE.md has the table of stores.

use std::fs;
use std::path::{Path, PathBuf};

use iyakki_schema::sibling;

fn fixtures(which: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/fixtures/upgrade").join(which)
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

/// A fresh copy of a fixture tree standing in for `/var/lib/iyakki`.
fn data(which: &str, tag: &str) -> PathBuf {
    let d = crate::testdir::unique("upg", &format!("{which}-{tag}"));
    let _ = fs::remove_dir_all(&d);
    copy_tree(&fixtures(which), &d);
    d
}

fn bytes(p: &Path) -> Vec<u8> {
    fs::read(p).unwrap()
}

fn json(p: &Path) -> serde_json::Value {
    serde_json::from_slice(&bytes(p)).unwrap()
}

fn corrupt_copies(dir: &Path, name: &str) -> Vec<PathBuf> {
    fs::read_dir(dir).unwrap().flatten().map(|e| e.path()).filter(|p| p.file_name().unwrap().to_string_lossy().starts_with(&format!("{name}.corrupt-"))).collect()
}

// ------------------------------------------------------------------------------------------------ settings

#[test]
fn settings_of_alpha1_load_with_nothing_lost_new_fields_defaulted_one_backup_and_a_quiet_second_run() {
    use crate::settings::SettingsStore;
    let d = data("alpha1", "settings");
    let p = d.join("settings.json");
    let original = bytes(&p);
    let s = SettingsStore::open(&p).unwrap();
    let g = s.get();
    assert_eq!(g.providers.len(), 2);
    assert_eq!(g.providers[1].key_ref.as_deref(), Some("vault:cloud-key"));
    assert_eq!((g.chat.as_ref().unwrap().model.as_str(), g.embedding.as_ref().unwrap().model.as_str()), ("qwen3:8b", "nomic-embed-text"));
    assert_eq!((g.rag.chunk_max, g.rag.top_k, g.rag.use_external), (900, 7, false));
    assert_eq!((g.token_price_cents_per_mtok, g.tool_log_days, g.media_download_max_mb, g.think_limit_secs, g.silence_limit_secs), (150, 5, 512, 90, 150));
    assert!(g.no_workspace_history && g.vault.lock_on_admin_logout && g.time.zone == "Europe/Paris");
    // what alpha.1 did not have starts at its default
    let fresh = crate::settings::Settings::default();
    assert_eq!((&g.sounds, &g.keyboard, &g.battery, &g.cloud, g.show_all_ai, g.no_tool_routing), (&fresh.sounds, &fresh.keyboard, &fresh.battery, &fresh.cloud, false, false));
    assert!(s.read_only().is_none());
    // the old file is kept as it was; the new file carries the schema
    assert_eq!(bytes(&sibling(&p, "pre-v1")), original);
    assert_eq!(json(&p)["schema"], 1);
    // the second run changes nothing
    let after = bytes(&p);
    let again = SettingsStore::open(&p).unwrap();
    assert_eq!(again.get(), s.get());
    assert_eq!(bytes(&p), after);
    assert_eq!(bytes(&sibling(&p, "pre-v1")), original, "the backup is made once and never replaced");
    // and a change saves normally, keeping everything else
    let mut s = s;
    s.update(|x| x.tool_log_days = 6).unwrap();
    let t = SettingsStore::open(&p).unwrap();
    assert_eq!((t.get().tool_log_days, t.get().providers.len()), (6, 2));
}

#[test]
fn settings_from_a_newer_release_are_read_but_never_written() {
    use crate::settings::SettingsStore;
    let d = data("alpha1", "settings-newer");
    let p = d.join("settings.json");
    let mut v = json(&p);
    v["schema"] = 9.into();
    v["a_field_of_the_future"] = serde_json::json!({"keep": [1, 2, 3]});
    fs::write(&p, serde_json::to_vec_pretty(&v).unwrap()).unwrap();
    let before = bytes(&p);
    let mut s = SettingsStore::open(&p).unwrap();
    assert_eq!(s.get().providers.len(), 2, "what can be read is shown");
    assert!(s.read_only().unwrap().contains("newer IYAKKI"));
    let e = s.update(|x| x.tool_log_days = 9).unwrap_err().to_string();
    assert!(e.contains("newer IYAKKI") && e.contains("update IYAKKI first"), "{e}");
    assert!(s.upsert_provider(crate::providers::ProviderCfg::new("x", crate::providers::Kind::Ollama)).is_err());
    assert_eq!(bytes(&p), before, "the file is exactly as the newer release left it");
    assert!(!sibling(&p, "pre-v1").exists() && !sibling(&p, "pre-v10").exists());
}

#[test]
fn a_damaged_settings_file_is_moved_aside_and_the_system_starts_with_defaults_and_an_audit_line() {
    use crate::settings::SettingsStore;
    let d = data("alpha1", "settings-bad");
    let p = d.join("settings.json");
    fs::write(&p, b"{\"providers\": [ {\"id\": ").unwrap();
    let (out, st, note) = iyakki_schema::load_typed::<crate::settings::Settings>(&crate::settings::SCHEMA, &p, 1234);
    assert!(matches!(out, iyakki_schema::Outcome::Corrupt { .. }) && st == crate::settings::Settings::default());
    let note = note.unwrap();
    assert!(note.contains("settings") && note.contains("damaged") && note.contains("settings.json.corrupt-1234") && note.contains("started with the defaults"), "{note}");
    assert_eq!(bytes(&d.join("settings.json.corrupt-1234")), b"{\"providers\": [ {\"id\": ");
    // the store opens (the system starts) and works; the other stores of the volume are not touched
    let mut s = SettingsStore::open(&p).unwrap();
    assert!(s.get().providers.is_empty());
    s.update(|x| x.tool_log_days = 4).unwrap();
    assert_eq!(SettingsStore::open(&p).unwrap().get().tool_log_days, 4);
    assert_eq!(bytes(&d.join("inbox/amy.json")), bytes(&fixtures("alpha1").join("inbox/amy.json")));
}

// ------------------------------------------------------------------------------------------------ inbox

#[test]
fn the_inbox_of_alpha1_is_grouped_with_every_note_and_task_counted_one_backup_and_a_quiet_second_run() {
    use crate::inbox::Inbox;
    let d = data("alpha1", "inbox");
    let p = d.join("inbox/amy.json");
    let original = bytes(&p);
    let inbox = Inbox::new(&d.join("inbox"));
    let now = 1_700_000_100;
    inbox.maintain("amy", now).unwrap();
    let after = json(&p);
    assert_eq!(after["version"], 2);
    let notes = inbox.list("amy");
    let total: u32 = notes.iter().map(|n| n.count).sum();
    assert_eq!(total, 5, "five notes went in, five are accounted for (three device notes are now one entry)");
    assert!(notes.iter().any(|n| n.title.contains("Disk is 90% full") && n.flagged), "a flagged note stays on its own");
    assert_eq!(inbox.tasks("amy", crate::inbox::TaskSort::Due).len(), 2);
    assert_eq!(bytes(&sibling(&p, "pre-v2")), original);
    // second run: nothing to do, nothing rewritten
    let stamp = bytes(&p);
    inbox.maintain("amy", now + 1).unwrap();
    assert_eq!(bytes(&p), stamp);
    assert_eq!(bytes(&sibling(&p, "pre-v2")), original);
}

#[test]
fn an_inbox_from_a_newer_release_is_not_overwritten() {
    use crate::inbox::Inbox;
    let d = data("alpha1", "inbox-newer");
    let p = d.join("inbox/amy.json");
    let mut v = json(&p);
    v["version"] = 7.into();
    v["a_field_of_the_future"] = "x".into();
    fs::write(&p, serde_json::to_vec(&v).unwrap()).unwrap();
    let before = bytes(&p);
    let inbox = Inbox::new(&d.join("inbox"));
    assert!(inbox.maintain("amy", 1_700_000_100).is_ok(), "nothing to change, nothing to refuse");
    let e = inbox.add("amy", crate::inbox::Kind::Info, "t", "b", "test", None, 1_700_000_200).unwrap_err();
    assert!(e.contains("newer IYAKKI"), "{e}");
    assert_eq!(bytes(&p), before);
}

#[test]
fn a_damaged_inbox_is_moved_aside_and_only_that_person_starts_empty() {
    use crate::inbox::Inbox;
    let d = data("alpha1", "inbox-bad");
    fs::write(d.join("inbox/amy.json"), b"{ truncated").unwrap();
    fs::write(d.join("inbox/bob.json"), bytes(&fixtures("alpha1").join("inbox/amy.json"))).unwrap();
    let inbox = Inbox::new(&d.join("inbox"));
    assert!(inbox.list("amy").is_empty());
    let n = inbox.add("amy", crate::inbox::Kind::Info, "fresh", "", "test", None, 1_700_000_300).unwrap();
    assert_eq!(n.title, "fresh");
    let moved = corrupt_copies(&d.join("inbox"), "amy.json");
    assert_eq!(moved.len(), 1);
    assert_eq!(bytes(&moved[0]), b"{ truncated");
    assert_eq!(inbox.list("bob").iter().map(|n| n.count).sum::<u32>(), 5, "another person's inbox is untouched");
}

// ------------------------------------------------------------------------------------------------ dashboard sources

#[test]
fn dashboard_sources_of_alpha1_are_kept_wrapped_with_one_backup_and_new_ones_are_added_beside_them() {
    use crate::dash::store::Sources;
    let d = data("alpha1", "dash");
    let p = d.join("dash/sources-amy.json");
    let original = bytes(&p);
    let st = Sources::open(&d, "amy");
    let list = st.list();
    assert_eq!(list.iter().map(|s| s.id.as_str()).collect::<Vec<_>>(), vec!["prometheus", "logs"], "her own two; the shared one has the same id as her first and is replaced by it");
    assert_eq!(list[0].secrets["token"], "uvault:svc.prometheus");
    let now = json(&p);
    assert_eq!((now["schema"].clone(), now["sources"].as_array().unwrap().len()), (1.into(), 2));
    assert_eq!(bytes(&sibling(&p, "pre-v1")), original);
    assert_eq!(json(&d.join("dash/sources-system.json"))["schema"], 1, "the shared file is lifted the same way");
    let stamp = bytes(&p);
    assert_eq!(st.list().len(), 2);
    assert_eq!(bytes(&p), stamp, "reading again changes nothing");
    let mut c = list[1].clone();
    c.id = "logs-2".into();
    c.name = "More logs".into();
    st.save(c).unwrap();
    assert_eq!(Sources::open(&d, "amy").own().len(), 3);
}

#[test]
fn dashboard_sources_from_a_newer_release_are_not_overwritten_and_a_damaged_file_is_moved_aside() {
    use crate::dash::store::Sources;
    let d = data("alpha1", "dash-newer");
    let p = d.join("dash/sources-amy.json");
    let text = r#"{"schema":4,"sources":[{"id":"prometheus","kind":"prometheus","name":"P","values":{"url":"http://192.0.2.1:9090"},"secrets":{}}],"tomorrow":true}"#;
    fs::write(&p, text).unwrap();
    let st = Sources::open(&d, "amy");
    assert_eq!(st.own().len(), 1);
    let mut c = st.own()[0].clone();
    c.id = "other".into();
    let e = st.save(c).unwrap_err();
    assert!(e.contains("newer IYAKKI"), "{e}");
    assert!(st.remove("prometheus").is_err());
    assert_eq!(fs::read_to_string(&p).unwrap(), text);
    // damaged
    let q = d.join("dash/sources-bob.json");
    fs::write(&q, b"[ {").unwrap();
    let bob = Sources::open(&d, "bob");
    assert!(bob.own().is_empty());
    assert_eq!(corrupt_copies(&d.join("dash"), "sources-bob.json").len(), 1);
    let mut c = Sources::open(&d, "amy").own()[0].clone();
    c.id = "mine".into();
    bob.save(c).unwrap();
    assert_eq!(bob.own().len(), 1);
}

// ------------------------------------------------------------------------------------------------ dev cluster state

#[test]
fn the_dev_cluster_state_of_alpha1_keeps_every_field_and_a_damaged_one_is_moved_aside() {
    use crate::k3sdev::{load_state, save_state, Layout};
    let d = data("alpha1", "k3s");
    let l = Layout::new(&d);
    let p = l.state();
    let original = bytes(&p);
    let st = load_state(&l).unwrap();
    assert_eq!((st.version.as_str(), st.installed_by.as_str(), st.restarts, st.autostart, st.last_restart_reason.as_str()), ("v1.30.0+k3s1", "admin", 2, true, "synthetic"));
    assert_eq!((st.node_ip.as_str(), st.last_exit_at, st.last_exit_reason.as_str()), ("", 0, ""), "fields added after alpha.1 start empty");
    assert_eq!(bytes(&sibling(&p, "pre-v1")), original);
    assert_eq!(json(&p)["schema"], 1);
    let stamp = bytes(&p);
    assert_eq!(load_state(&l).unwrap(), st);
    assert_eq!(bytes(&p), stamp);
    // a newer file is not replaced
    let mut v = json(&p);
    v["schema"] = 3.into();
    fs::write(&p, serde_json::to_vec(&v).unwrap()).unwrap();
    let before = bytes(&p);
    assert!(save_state(&l, &st).unwrap_err().contains("newer IYAKKI"));
    assert_eq!(bytes(&p), before);
    // a damaged one: "no cluster managed here", the file kept beside it, the cluster's own folders untouched
    fs::write(&p, b"{{{").unwrap();
    fs::create_dir_all(l.root.join("data")).unwrap();
    assert!(load_state(&l).is_none());
    assert_eq!(corrupt_copies(&l.root, "state.json").len(), 1);
    assert!(l.root.join("data").is_dir());
}

// ------------------------------------------------------------------------------------------------ devices

#[test]
fn the_paired_devices_of_alpha1_stay_paired_and_a_newer_file_is_not_overwritten() {
    use crate::devmode::client::Home;
    let d = data("alpha1", "devhome");
    let h = Home::at(&d.join("devhome"));
    let p = d.join("devhome/devices.json");
    let original = bytes(&p);
    let devs = h.devices();
    assert_eq!(devs.iter().map(|x| (x.name.as_str(), x.port, x.account.as_str())).collect::<Vec<_>>(), vec![("board", 7979, "admin"), ("mac", 7979, "")]);
    assert_eq!(h.default_name(), "board");
    // the first change lifts the file; the old one is kept
    h.set_default("mac").unwrap();
    assert_eq!(bytes(&sibling(&p, "pre-v1")), original);
    assert_eq!((json(&p)["schema"].clone(), h.devices().len(), h.default_name()), (1.into(), 2, "mac".to_string()));
    // newer
    let mut v = json(&p);
    v["schema"] = 5.into();
    v["tomorrow"] = true.into();
    fs::write(&p, serde_json::to_vec(&v).unwrap()).unwrap();
    let before = bytes(&p);
    assert!(h.set_default("board").unwrap_err().contains("newer IYAKKI"));
    assert!(h.remove("mac").is_err());
    assert_eq!(bytes(&p), before);
    // damaged: nothing paired, the old file is kept beside the new one
    fs::write(&p, b"[not json").unwrap();
    assert!(h.devices().is_empty());
    assert_eq!(corrupt_copies(&d.join("devhome"), "devices.json").len(), 1);
}

#[test]
fn the_network_watch_of_alpha1_keeps_its_trusted_devices_and_a_damaged_file_is_not_taken_as_a_clean_slate() {
    use crate::devwatch::DeviceWatch;
    let d = data("alpha1", "watch");
    let p = d.join("devices.json");
    let original = bytes(&p);
    let w = DeviceWatch::open(&p);
    let l = w.list();
    assert_eq!(l.len(), 2);
    assert_eq!((l[0].mac.as_str(), l[0].trusted, l[1].name.as_str()), ("de:ad:be:ef:00:09", false, "Router"), "the stranger is still a stranger");
    assert_eq!(bytes(&sibling(&p, "pre-v1")), original);
    assert_eq!(json(&p)["schema"], 1);
    assert_eq!(w.untrusted_count(), 1);
    // newer: never written
    let mut v = json(&p);
    v["schema"] = 8.into();
    fs::write(&p, serde_json::to_vec(&v).unwrap()).unwrap();
    let before = bytes(&p);
    let w2 = DeviceWatch::open(&p);
    assert!(w2.trust_all() >= 1);
    assert_eq!(bytes(&p), before);
    // damaged: moved aside; the first look afterwards is a new baseline, but the old list is still on disk to read
    fs::write(&p, b"nope").unwrap();
    let w3 = DeviceWatch::open(&p);
    assert!(w3.list().is_empty());
    assert_eq!(corrupt_copies(&d, "devices.json").len(), 1);
}

// ------------------------------------------------------------------------------------------------ flows

#[test]
fn the_flows_of_alpha1_open_with_their_versions_and_runs_and_a_newer_flow_folder_is_not_written() {
    use iyakki_flow::store::FlowStore;
    use std::sync::Arc;
    let d = data("alpha1", "flows");
    let root = d.join("home/amy/Flows");
    let meta = root.join("orders/meta.json");
    let original = bytes(&meta);
    let store = FlowStore::new(&root, Arc::new(|| 1_700_000_500_000));
    assert_eq!(store.ids(), vec!["orders".to_string()]);
    let m = store.meta_of("orders");
    assert_eq!((m.published, m.versions.len(), m.tags["stable"], m.schema), (1, 1, 1, 1));
    assert_eq!(bytes(&sibling(&meta, "pre-v1")), original);
    assert_eq!(store.published("orders").unwrap().nodes.len(), 2);
    assert_eq!(store.get_draft("orders").unwrap().description, "synthetic flow, draft edit");
    let stamp = bytes(&meta);
    let _ = store.meta_of("orders");
    assert_eq!(bytes(&meta), stamp);
    // a draft change keeps the versions
    let mut f = store.get_draft("orders").unwrap();
    f.description = "edited after the upgrade".into();
    store.save_draft(&f, None).unwrap();
    assert_eq!(store.meta_of("orders").versions.len(), 1);
    assert!(root.join("orders/versions/v1.json").is_file());
    // newer
    let mut v = json(&meta);
    v["schema"] = 6.into();
    v["tomorrow"] = true.into();
    fs::write(&meta, serde_json::to_vec(&v).unwrap()).unwrap();
    let (mb, fb) = (bytes(&meta), bytes(&root.join("orders/flow.json")));
    let e = store.save_draft(&f, None).unwrap_err();
    assert!(e.contains("newer IYAKKI"), "{e}");
    assert_eq!((bytes(&meta), bytes(&root.join("orders/flow.json"))), (mb, fb));
}

#[test]
fn a_damaged_flow_meta_file_is_moved_aside_and_the_flow_itself_is_still_there() {
    use iyakki_flow::store::FlowStore;
    use std::sync::Arc;
    let d = data("alpha1", "flows-bad");
    let root = d.join("home/amy/Flows");
    fs::write(root.join("orders/meta.json"), b"{ half").unwrap();
    let store = FlowStore::new(&root, Arc::new(|| 1));
    let _ = store.meta_of("orders");
    assert_eq!(corrupt_copies(&root.join("orders"), "meta.json").len(), 1);
    assert!(store.get_draft("orders").is_some() && root.join("orders/versions/v1.json").is_file(), "the flow and its version files are untouched");
}

// ------------------------------------------------------------------------------------------------ the schema registries

#[test]
fn every_registry_is_complete() {
    for s in [crate::settings::SCHEMA, crate::dash::store::SOURCES_SCHEMA, crate::k3sdev::STATE_SCHEMA, crate::devmode::client::DEVICES_SCHEMA, crate::devwatch::SCHEMA, iyakki_flow::store::SCHEMA] {
        s.check().unwrap();
    }
}

// ------------------------------------------------------------------------------------------------ the current fixtures

#[test]
fn the_files_this_release_writes_load_without_any_change() {
    use crate::dash::store::Sources;
    use crate::settings::SettingsStore;
    let d = data("current", "all");
    let all: Vec<PathBuf> = ["settings.json", "dash/sources-amy.json", "k3s-dev/state.json", "devices.json", "devhome/devices.json", "home/amy/Flows/orders/meta.json"].iter().map(|f| d.join(f)).collect();
    let before: Vec<Vec<u8>> = all.iter().map(|p| bytes(p)).collect();
    assert_eq!(SettingsStore::open(&d.join("settings.json")).unwrap().get().providers.len(), 2);
    assert_eq!(Sources::open(&d, "amy").own().len(), 2);
    assert!(crate::k3sdev::load_state(&crate::k3sdev::Layout::new(&d)).is_some());
    assert_eq!(crate::devwatch::DeviceWatch::open(&d.join("devices.json")).list().len(), 2);
    assert_eq!(crate::devmode::client::Home::at(&d.join("devhome")).devices().len(), 2);
    let fs_ = iyakki_flow::store::FlowStore::new(&d.join("home/amy/Flows"), std::sync::Arc::new(|| 1));
    assert_eq!(fs_.meta_of("orders").versions.len(), 1);
    let crate_inbox = crate::inbox::Inbox::new(&d.join("inbox"));
    crate_inbox.maintain("amy", 1_700_000_100).unwrap();
    for (p, b) in all.iter().zip(&before) {
        assert_eq!(&bytes(p), b, "{} was rewritten", p.display());
    }
    let mut pre: Vec<String> = Vec::new();
    for e in walk(&d) {
        if e.to_string_lossy().contains(".pre-v") {
            pre.push(e.display().to_string());
        }
    }
    assert!(pre.is_empty(), "no backup is made for a file that needs no change: {pre:?}");
}

fn walk(dir: &Path) -> Vec<PathBuf> {
    let mut v = Vec::new();
    for e in fs::read_dir(dir).unwrap().flatten() {
        if e.path().is_dir() {
            v.extend(walk(&e.path()));
        } else {
            v.push(e.path());
        }
    }
    v
}

/// Rewrites `tests/fixtures/upgrade/current` from the alpha.1 fixtures with this release's code (run it by hand after a schema
/// change, then read the diff): `IYAKKI_REGEN_FIXTURES=1 cargo test -p iyakki-agent --lib regenerate_current_fixtures -- --ignored`.
#[test]
#[ignore]
fn regenerate_current_fixtures() {
    assert!(std::env::var_os("IYAKKI_REGEN_FIXTURES").is_some(), "set IYAKKI_REGEN_FIXTURES=1");
    let d = data("alpha1", "regen");
    let mut s = crate::settings::SettingsStore::open(&d.join("settings.json")).unwrap();
    s.update(|_| {}).unwrap();
    let _ = crate::dash::store::Sources::open(&d, "amy").list();
    let l = crate::k3sdev::Layout::new(&d);
    let st = crate::k3sdev::load_state(&l).unwrap();
    crate::k3sdev::save_state(&l, &st).unwrap();
    let w = crate::devwatch::DeviceWatch::open(&d.join("devices.json"));
    w.set_enabled(true);
    let h = crate::devmode::client::Home::at(&d.join("devhome"));
    h.set_default("board").unwrap();
    let fl = iyakki_flow::store::FlowStore::new(&d.join("home/amy/Flows"), std::sync::Arc::new(|| 1));
    let _ = fl.meta_of("orders");
    crate::inbox::Inbox::new(&d.join("inbox")).maintain("amy", 1_700_000_100).unwrap();
    let out = fixtures("current");
    let _ = fs::remove_dir_all(&out);
    for p in walk(&d) {
        let rel = p.strip_prefix(&d).unwrap();
        if rel.to_string_lossy().contains(".pre-v") {
            continue;
        }
        let to = out.join(rel);
        fs::create_dir_all(to.parent().unwrap()).unwrap();
        fs::copy(&p, &to).unwrap();
    }
}
