//! The knowledge base as a trait object (F-SPLIT-03): the store behind its lock answers through `KnowledgeBaseApi`, and the workspace history runs over
//! the adapter `KbHandle` exactly as it runs over the store itself.

use std::sync::{Arc, Mutex};

use iyakki_agent_api::{KbHandle, KnowledgeBaseApi};
use iyakki_memx::history::History;
use iyakki_memx::kb::{default_embedder, KbSource, KnowledgeBase};

fn store() -> (Arc<Mutex<KnowledgeBase>>, Arc<dyn KnowledgeBaseApi>) {
    let raw = Arc::new(Mutex::new(KnowledgeBase::in_memory(default_embedder())));
    let api: Arc<dyn KnowledgeBaseApi> = Arc::new(raw.clone());
    (raw, api)
}

#[test]
fn documents_added_and_searched_through_the_trait_are_the_stores_own() {
    let (raw, api) = store();
    api.add_document("user/amy", "plan", "plan.txt", "quokkas are small marsupials from Rottnest", Default::default()).unwrap();
    assert_eq!(raw.lock().unwrap().docs("user/amy").len(), 1, "the trait wrote into the real store");
    assert_eq!(api.namespaces(), vec!["user/amy".to_string()]);
    assert_eq!(api.stats("user/amy").map(|s| s.docs), Some(1));
    let hits = api.search_in(&["user/amy".to_string()], "quokkas marsupials", 3);
    assert_eq!(hits.len(), 1);
    assert!(api.search_in(&["user/bob".to_string()], "quokkas marsupials", 3).is_empty(), "another namespace is another person's");
    let weighted = api.search_weighted_in(&[("user/amy".to_string(), 2.0)], "quokkas", 3);
    assert_eq!(weighted.len(), 1);
    // a bad namespace is the store's own error, not a panic
    assert!(api.add_document("../x", "t", "s", "text", Default::default()).is_err());
    let id = hits[0].doc_id.clone();
    api.remove_document("user/amy", &id).unwrap();
    assert!(api.docs("user/amy").is_empty());
}

#[test]
fn the_workspace_history_works_over_the_adapter_and_forgets_through_it() {
    let (raw, api) = store();
    let src: Arc<dyn KbSource> = Arc::new(KbHandle(api.clone()));
    let h = History::new(src.clone(), "alice", "Shop", Some("s_one"));
    h.record("answer", "r1", "the quote for the shop is ready");
    h.record("answer", "r2", "the second answer");
    assert_eq!(h.len(), 2);
    assert_eq!(History::workspaces_of(&*src, "alice").iter().map(|w| w.entries).sum::<usize>(), 2);
    assert_eq!(raw.lock().unwrap().namespaces().len(), 1, "the entries are in the real store");
    assert_eq!(History::forget_session_of(&*src, "alice", "Shop", "s_one"), 2);
    assert!(History::workspaces_of(&*src, "alice").is_empty());
}
