//! The facets of lane q-split-b3 answer "nothing / unavailable" by default and the fake needs no agent (F-SPLIT-03).

use std::path::Path;
use std::sync::Arc;

use iyakki_agent_api::{AgentApi, AgentFacetsB3, FakeAgentApi, NoB3, ServicesFacetB3, VpnManager};
use iyakki_shared::avaddon::Addon;
use iyakki_shared::hwprofile::Category;

#[test]
fn the_fake_agent_has_every_b3_facet() {
    fn takes(_: &dyn AgentApi) {}
    let f = FakeAgentApi::default();
    takes(&f);
    assert_eq!(f.avaddon().detect(Path::new("/nonexistent")), Addon::NotInImage);
    assert_eq!(f.profile_lock().on_screen_lock(Path::new("/x"), "ann"), Ok(false));
    assert_eq!(f.profile_lock().on_screen_unlock(Path::new("/x"), "ann", Some("00")), Ok(false));
    assert!(f.client_media().device_holds(Path::new("/x"), "d1").is_empty());
    assert!(f.client_media().set_permission(Path::new("/x"), "d1", "audio.out", true, "ann", 1).is_err());
    assert!(!f.client_media().indicator().active());
    assert!(!f.browser_source().browser_session_active("ann"));
    assert_eq!(f.computer_hooks().display_size_get(Path::new("/x"), "ann"), None);
}

#[test]
fn tool_wrappers_hand_back_the_tools_they_were_given() {
    let f = FakeAgentApi::default();
    let inner: Arc<dyn iyakki_shared::agent::Tools> = Arc::new(iyakki_shared::agent::NoTools);
    let policy = iyakki_sec::policy::Role::Admin.template();
    let audit: iyakki_agent_api::AuditLine = Arc::new(|_, _| {});
    let w = f.tool_wraps_b3().wrap_device_tools(inner.clone(), Path::new("/x"), "ann", true, audit.clone());
    assert!(Arc::ptr_eq(&w, &inner));
    let w = f.tool_wraps_b3().wrap_restart_tools(inner.clone(), Path::new("/x"), &policy, "ann", audit);
    assert!(Arc::ptr_eq(&w, &inner));
}

#[test]
fn the_services_facet_without_services_says_unavailable_and_keeps_no_computer() {
    let s: &dyn ServicesFacetB3 = &NoB3;
    assert!(s.startup_retry("k3s", "ann").is_err());
    assert!(!s.computer_attach() && !s.computer_announce(&["ann".into()]));
    assert!(s.computer_page().is_none());
    assert!(s.computer_set_sharing(Category::Sound, true).is_err());
    assert!(s.computer_copy_from("x").is_err() && s.computer_reset().is_err() && s.computer_forget("x").is_err());
    let be = s.wizard_backend("ann", Arc::new(|_, _| {}));
    assert!(be.installed().is_empty() && be.pending().is_empty());
    s.kube_resolver(Some("ann"));
    s.kube_resolver(None);
}

#[test]
fn a_person_without_a_vpn_core_has_no_profiles() {
    let m: Arc<dyn VpnManager> = NoB3.vpn_manager("ann", Arc::new(|_, _| {}));
    assert!(m.profiles().is_empty() && m.rows().is_empty() && m.summary().is_none() && m.guard_line().is_none());
    assert!(!m.is_enabled("v1"));
    assert!(m.import("x", b"").is_err() && m.remove("v1").is_err());
}
