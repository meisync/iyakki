//! Fake-engine tests: the fake answers what it has canned, records every call in order, and can be held behind the trait object the screens use.

use std::sync::Arc;

use crate::engine::StoreEngine;
use crate::fake::{FakeStoreEngine, NOT_CANNED};

#[test]
fn the_canned_answers_come_back_and_every_call_is_recorded_in_order() {
    let fake = FakeStoreEngine::with(1_700_000_000, "/data/store", Some("https://store.example/"), true, Some("approved by the owner"));
    assert_eq!(fake.now(), 1_700_000_000);
    assert_eq!(fake.dir(), std::path::Path::new("/data/store"));
    assert_eq!(fake.official_store_url(), Some("https://store.example/"));
    assert!(fake.catalog_visible());
    assert_eq!(fake.approval_audit("app.one", "1.0"), Some("approved by the owner".to_string()));
    assert_eq!(fake.calls(), vec!["now", "dir", "official_store_url", "catalog_visible", "approval_audit"]);
}

#[test]
fn a_call_without_a_canned_answer_gives_the_not_canned_error() {
    let fake = FakeStoreEngine::default();
    assert!(!fake.catalog_visible());
    assert_eq!(fake.official_store_url(), None);
    assert_eq!(fake.approval_audit("app.one", "1.0"), None);
    assert!(fake.pin("app.one", "1.0").unwrap_err().contains(NOT_CANNED));
    assert!(fake.trust_key(&iyakki_store_model::store_install::Actor::system(), "k", "l").unwrap_err().contains(NOT_CANNED));
    assert_eq!(fake.calls().last().copied(), Some("trust_key"));
}

#[test]
fn the_engine_is_held_behind_a_shared_trait_object_and_answers_through_it() {
    let engine: Arc<dyn StoreEngine> = Arc::new(FakeStoreEngine::with(42, "/s", None, false, None));
    assert_eq!(engine.now(), 42);
    assert!(engine.items(&iyakki_store_model::store_install::Actor::system(), &[]).is_empty());
    assert!(engine.local_catalogs().is_empty());
    assert!(!engine.is_disabled("app", "one", &iyakki_store_model::skills::Scope::Shared));
}
