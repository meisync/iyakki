//! Shared project Context between two systems (F-FLEET-04): two hubs over real loopback sockets, each with its own workspace folder and the
//! real `notes.rs` files. Not real here: other computers, a person editing the Context tab (the notes are changed through the same `notes::edit`).

use std::path::PathBuf;
use std::sync::Arc;

use super::*;
use crate::fleet::{FixedSource, Heartbeat, HeartbeatSource};
use crate::fleet_hub::Hub;
use crate::notes::{NewNote, Who};

fn tmp(tag: &str) -> PathBuf {
    crate::testdir::outside_project("fleetctx", tag)
}

fn hb() -> Heartbeat {
    Heartbeat { disk_free_mb: 5000, disk_total_mb: 9000, cpu_load_pct: 10, cpus: 4, mem_used_pct: 20, mem_total_mb: 8000, tasks_running: 0, models: vec![], battery_pct: None }
}

struct Side {
    hub: Arc<Hub>,
    root: PathBuf,
    sync: Arc<ContextSync>,
}

fn side(tag: &str) -> (Side, String) {
    let hub = Arc::new(Hub::load(&tmp(&format!("{tag}-hub"))).unwrap());
    let src: Arc<dyn HeartbeatSource> = Arc::new(FixedSource(hb()));
    let addr = hub.start_node("127.0.0.1:0", src).unwrap().to_string();
    let root = tmp(&format!("{tag}-ws"));
    let sync = Arc::new(ContextSync::new(&root, &tmp(&format!("{tag}-base"))));
    let s2 = sync.clone();
    hub.set_context_handler(Arc::new(move |f, _from, name, m| s2.incoming(f, name, m)));
    (Side { hub, root, sync }, addr)
}

fn two(tag: &str) -> (Side, Side, String) {
    let (a, a_addr) = side(&format!("{tag}a"));
    let (b, _) = side(&format!("{tag}b"));
    let fid = a.hub.create_fleet("lab", "alpha").unwrap();
    let (code, _) = a.hub.offer_code(&fid).unwrap();
    let t = b.hub.join_start(&a_addr, &code, "bravo").unwrap();
    b.hub.poll_pending();
    let r = a.hub.status().requests;
    a.hub.approve_join(&r[0].id).unwrap();
    b.hub.poll_pending();
    b.hub.join_accept(&t).unwrap();
    a.hub.settle();
    for _ in 0..2 {
        a.hub.heartbeat_tick(&FixedSource(hb()));
        b.hub.heartbeat_tick(&FixedSource(hb()));
    }
    (a, b, fid)
}

fn add(s: &Side, title: &str, text: &str) -> String {
    notes::edit(&s.root, |n| n.add(NewNote::new(title, text, Kind::Decision), &Who::Person("ann".into()), 100).map(|a| a.id)).unwrap()
}

fn text_of(s: &Side, id: &str) -> String {
    notes::load(&s.root).0.get(id).unwrap().text.clone()
}

fn push(from: &Side, fid: &str) -> Result<(), String> {
    let (msg, pairs) = from.sync.outgoing(fid);
    let r = from.hub.broadcast(fid, "context", &msg)?;
    for (_, x) in r {
        x?;
    }
    from.sync.sent(fid, &pairs)
}

#[test]
fn nothing_is_taken_until_the_person_turns_sharing_on_and_what_arrives_is_untrusted_data() {
    let (a, b, fid) = two("c1");
    let id = add(&a, "Use Postgres", "We use Postgres 16 for storage.");
    // b has not turned sharing on: refused, and the notes of b are unchanged
    let e = push(&a, &fid).unwrap_err();
    assert!(e.contains("does not share"), "{e}");
    assert!(notes::load(&b.root).0.notes.is_empty());
    b.hub.set_context_sync(&fid, true).unwrap();
    push(&a, &fid).unwrap();
    let n = notes::load(&b.root).0;
    let got = n.get(&id).expect("the note arrived");
    assert_eq!(got.text, "We use Postgres 16 for storage.");
    assert!(got.by_agent && !got.trusted && !got.reviewed, "an incoming note is data until a person trusts it");
    assert!(got.author.contains("alpha"));
    // nothing changed since: nothing is sent again
    assert!(a.sync.outgoing(&fid).1.is_empty());
    // a note that looks like a secret is refused on arrival even if the sender's own check missed it
    let msg = serde_json::json!({"notes": [{"id": "N-077", "title": "Deploy", "text": "the api_key is sk-abcdefghijklmnopqrstuvwxyz", "kind": "gotcha", "pinned": false, "status": "active", "author": "x", "updated": 1, "base": "", "hash": ""}]});
    let line = b.sync.incoming(&fid, "alpha", &msg).unwrap();
    assert!(line.contains("1 refused"), "{line}");
    assert!(notes::load(&b.root).0.get("N-077").is_none());
}

#[test]
fn a_change_on_one_side_goes_across_and_a_change_on_both_keeps_both_versions_for_a_person_to_settle() {
    let (a, b, fid) = two("c2");
    b.hub.set_context_sync(&fid, true).unwrap();
    a.hub.set_context_sync(&fid, true).unwrap();
    let id = add(&a, "Naming", "Use snake_case.");
    push(&a, &fid).unwrap();
    // the person on b trusts the note; a later change by a keeps that trust
    notes::edit(&b.root, |n| n.set_trusted(&id, true)).unwrap();
    notes::edit(&a.root, |n| n.update(&id, None, Some("Use snake_case everywhere."), None, &Who::Person("ann".into()), 200)).unwrap();
    push(&a, &fid).unwrap();
    assert_eq!(text_of(&b, &id), "Use snake_case everywhere.", "a change on one side only is taken");
    assert!(notes::load(&b.root).0.get(&id).unwrap().trusted, "the trust the person gave is kept");
    // both change it
    notes::edit(&a.root, |n| n.update(&id, None, Some("Use snake_case for functions."), None, &Who::Person("ann".into()), 300)).unwrap();
    notes::edit(&b.root, |n| n.update(&id, None, Some("Use camelCase for types."), None, &Who::Person("bob".into()), 310)).unwrap();
    push(&a, &fid).unwrap();
    assert_eq!(text_of(&b, &id), "Use camelCase for types.", "the local version stays");
    let c = b.sync.conflicts(&fid);
    assert_eq!(c.len(), 1, "{c:?}");
    assert_eq!(text_of(&b, &c[0].theirs), "Use snake_case for functions.", "the other version is kept beside it");
    assert!(notes::load(&b.root).0.get(&c[0].theirs).unwrap().title.contains("from alpha"));
    // the copy is never sent on: b's next push carries b's own change but not the copy
    let (msg, _) = b.sync.outgoing(&fid);
    let ids: Vec<&str> = msg["notes"].as_array().unwrap().iter().map(|n| n["id"].as_str().unwrap()).collect();
    assert!(ids.contains(&id.as_str()) && !ids.contains(&c[0].theirs.as_str()), "{ids:?}");
    // a person settles it: keep theirs
    assert!(b.sync.resolve(&fid, &c[0].theirs, "theirs").unwrap().contains("settled"));
    assert_eq!(text_of(&b, &id), "Use snake_case for functions.");
    assert!(b.sync.conflicts(&fid).is_empty());
    assert_eq!(notes::load(&b.root).0.get(&c[0].theirs).unwrap().status, notes::Status::Archived);
    assert!(b.sync.resolve(&fid, "N-nope", "mine").is_err());
}

#[test]
fn two_notes_with_the_same_id_but_different_content_are_both_kept() {
    let (a, b, fid) = two("c3");
    b.hub.set_context_sync(&fid, true).unwrap();
    // both made "their first note" independently: the same id, different content
    let (ia, ib) = (add(&a, "Cache", "Use Redis."), add(&b, "Queue", "Use NATS."));
    assert_eq!(ia, ib, "both are the first note of their workspace");
    push(&a, &fid).unwrap();
    let n = notes::load(&b.root).0;
    assert_eq!(n.notes.len(), 2, "nothing was overwritten");
    assert_eq!(n.get(&ib).unwrap().text, "Use NATS.");
    assert_eq!(b.sync.conflicts(&fid).len(), 1);
}

#[test]
fn two_edits_of_one_note_can_be_merged_line_by_line_and_nothing_of_either_is_lost() {
    assert_eq!(merge_lines("a\nb\nc", "a\nb\nc"), "a\nb\nc");
    assert_eq!(merge_lines("a\nmine\nc", "a\ntheirs\nc"), "a\nmine\ntheirs\nc");
    assert_eq!(merge_lines("a\nc", "a\nb\nc\nd"), "a\nb\nc\nd");
    assert_eq!(merge_lines("", "x"), "x");
    let (a, b, fid) = two("c4");
    b.hub.set_context_sync(&fid, true).unwrap();
    a.hub.set_context_sync(&fid, true).unwrap();
    let id = add(&a, "Rules", "Use snake_case.\nTests first.");
    push(&a, &fid).unwrap();
    notes::edit(&a.root, |n| n.update(&id, None, Some("Use snake_case.\nTests first.\nNo unwrap in services."), None, &Who::Person("ann".into()), 300)).unwrap();
    notes::edit(&b.root, |n| n.update(&id, None, Some("Use snake_case.\nTests first.\nPrefer small crates."), None, &Who::Person("bob".into()), 310)).unwrap();
    push(&a, &fid).unwrap();
    let c = b.sync.conflicts(&fid);
    assert_eq!(c.len(), 1);
    assert!(b.sync.resolve(&fid, &c[0].theirs, "merge").unwrap().contains("settled"));
    assert_eq!(text_of(&b, &id), "Use snake_case.\nTests first.\nPrefer small crates.\nNo unwrap in services.");
    assert_eq!(notes::load(&b.root).0.get(&c[0].theirs).unwrap().status, notes::Status::Archived);
    assert!(b.sync.conflicts(&fid).is_empty());
}
