//! The real core always serves the app store handle (the UI's `store_of` relies on it), and the handle answers from this install's folder.

use std::sync::Arc;

use iyakki_agent_api::ServicesFacetB4;

struct Dead;
impl iyakki_proxy::Upstream for Dead {
    fn request(&self, _: &str, _: &str, _: &[(String, String)], _: &[u8]) -> Result<iyakki_proxy::UpstreamResponse, String> {
        Err("no network in tests".into())
    }
}

#[test]
fn the_real_core_always_returns_the_store_handle_and_it_works_over_this_install() {
    let d = std::fs::canonicalize(std::env::temp_dir()).unwrap().join(format!("iyakki-store-api-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    let svc = crate::services::Services::open(&d.join("svc"), Arc::new(Dead), Arc::new(Dead)).unwrap();
    let handle = svc.b4().store();
    assert!(handle.is_some(), "the agent core serves the store; the UI's `store_of` depends on it");
    let store = handle.unwrap();
    assert!(store.dir().starts_with(&d), "the handle works over this install's folder");
    assert!(store.config().sources.is_empty(), "a fresh install has added no store");
    // the default of a core without a store is None
    assert!(iyakki_agent_api::NoB4.store().is_none());
    let _ = std::fs::remove_dir_all(&d);
}
