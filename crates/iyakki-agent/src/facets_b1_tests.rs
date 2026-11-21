//! The real implementation of the b1 facets answers from the agent (F-SPLIT-03).

use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use iyakki_agent_api::{AgentFacetsB1, AiMachine, HeaderValues};
use iyakki_shared::aiprofile::Vis;

use crate::agentapi_gen::AgentApiImpl;

fn tmp(tag: &str) -> PathBuf {
    let d = std::fs::canonicalize(std::env::temp_dir()).unwrap().join(format!("iyakki-b1-{}-{tag}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

#[test]
fn the_real_scene_facet_reads_hosts_and_asks_for_links_like_the_module() {
    let a = AgentApiImpl;
    assert_eq!(a.scene().host_of("https://pics.example.org/a/b.png?x=1"), "pics.example.org");
    assert_eq!(a.scene().asks_for_link("no link here", 2), crate::scene::reply::asks_for_link("no link here", 2));
}

#[test]
fn the_real_build_facet_says_no_for_a_map_without_this_version() {
    let a = AgentApiImpl;
    assert!(a.av_build().consent("").is_err());
}

#[test]
fn the_real_ai_machine_shows_an_unknown_item_and_names_hidden_ones_by_id() {
    let m = crate::aiprofile::this_machine(0);
    assert_eq!(AiMachine::visibility(&m, "no-such-item", false), Vis::Show);
    for (id, _) in AiMachine::hidden_items(&m) {
        assert!(crate::quickinstall::Item::parse(&id).is_some(), "{id}");
    }
}

struct Mem(Mutex<std::collections::HashMap<String, String>>);
impl HeaderValues for Mem {
    fn get(&self, id: &str) -> Result<Option<String>, String> {
        Ok(self.0.lock().unwrap().get(id).cloned())
    }
    fn set(&self, id: &str, v: &str) -> Result<(), String> {
        self.0.lock().unwrap().insert(id.into(), v.into());
        Ok(())
    }
    fn delete(&self, id: &str) -> Result<(), String> {
        self.0.lock().unwrap().remove(id);
        Ok(())
    }
}

#[test]
fn the_real_header_store_keeps_rules_in_a_file_and_values_behind_the_trait() {
    let _rules = iyakki_testkit::process_lock(iyakki_testkit::names::BROWSER_HEADERS);
    let a = AgentApiImpl;
    let dir = tmp("hdr");
    let s = a.browser_headers().open(&dir.join("rules.json"), Arc::new(Mem(Default::default())));
    assert!(s.list().is_empty());
    let r = iyakki_shared::browserheaders::HeaderRule { id: String::new(), name: "Token".into(), header: "X-Demo".into(), domains: "example.org".into(), prefix: String::new(), path_regex: String::new(), enabled: true, ack: false };
    let added = s.add(r, "secret").unwrap();
    assert_eq!(s.list().len(), 1);
    assert_eq!(s.activate(), 0, "the one enabled rule compiled");
    assert!(s.set_enabled(&added.id, false).is_ok());
    assert!(s.delete(&added.id).is_ok() && s.list().is_empty());
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn the_real_context_facet_ignores_ordinary_text_and_refuses_a_foreign_undo() {
    let a = AgentApiImpl;
    assert!(a.context_intent().detect("what is the weather").is_none());
    let foreign = iyakki_agent_api::ContextUndo(Arc::new(1u8));
    assert!(a.context_intent().undo(&std::env::temp_dir(), &foreign).is_err());
}

#[test]
fn the_real_registry_facet_is_the_process_wide_one() {
    let a = AgentApiImpl;
    let reg = a.sound_hold_registry();
    reg.announce("b1-test", Arc::new(|| Some("a test player".to_string())), Arc::new(|| {}));
    assert_eq!(crate::soundhold::global().holds("b1-test").as_deref(), Some("a test player"));
    assert!(a.sound_hold_registry().conflict_for_new_player("other", false).is_some());
    reg.forget("b1-test");
    assert!(crate::soundhold::global().holds("b1-test").is_none());
}

#[test]
fn the_real_host_engine_facet_speaks_directly_only_for_engines_that_do() {
    let a = AgentApiImpl;
    assert!(a.host_engines().direct("piper", None, None).is_none(), "piper makes a file, it does not speak by itself");
    assert!(a.host_engines().direct("no-such-engine", None, None).is_none());
}

#[test]
fn the_real_story_reader_starts_idle() {
    let a = AgentApiImpl;
    let r = a.story().new_reader();
    assert!(!r.is_active());
    assert_eq!(r.status(), iyakki_shared::story::Status::Idle);
}
