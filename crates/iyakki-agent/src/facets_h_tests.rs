//! The facets that take the person's services as `Any` must work when the UI hands them the `Arc<dyn ServicesHandle>` it holds (alpha.6 split H), not
//! only the concrete `Arc<Services>`: the gate of 2026-11-18 failed because one of them downcast to the concrete type only and answered "no services handle".

use std::any::Any;
use std::sync::Arc;

use iyakki_agent_api::{AgentFacetsB2, ServicesHandle};

use crate::agentapi_gen::AgentApiImpl;
use crate::services::Services;

#[derive(Debug)]
struct Dead;
impl iyakki_proxy::Upstream for Dead {
    fn request(&self, _: &str, _: &str, _: &[(String, String)], _: &[u8]) -> Result<iyakki_proxy::UpstreamResponse, String> {
        Err("no network in tests".into())
    }
}

fn handle(tag: &str) -> Arc<dyn ServicesHandle> {
    let d = crate::testdir::short("iyakki-handle", tag);
    let svc: Arc<dyn ServicesHandle> = Services::open(&d.join("svc"), Arc::new(Dead), Arc::new(Dead)).unwrap();
    svc
}

const NO_SVC: &str = "no services handle";

#[test]
fn the_facets_that_take_the_services_as_any_work_through_the_handle_the_ui_holds() {
    let h = handle("a");
    assert!(h.is_real());
    let a = AgentApiImpl;
    // what the UI passes as `&cx.services`
    let any: &dyn Any = &h;
    // the browser profile store
    let r = a.profile_store().reset_for(any, "ann");
    assert!(!r.contains(NO_SVC), "reset_for: {r}");
    let r = a.profile_store().protect_for(any, "ann");
    assert!(r.as_ref().err().is_none_or(|e| !e.contains(NO_SVC)), "protect_for: {r:?}");
    let r = a.profile_store().unprotect_for(any, "ann");
    assert!(r.as_ref().err().is_none_or(|e| !e.contains(NO_SVC)), "unprotect_for: {r:?}");
    // "correct my wording"
    assert!(a.dict_fix().set_enabled(any, true).is_ok(), "set_enabled");
    let f = a.dict_fix().fix_wording(any, "turn on the lights", &iyakki_shared::dictfix::Context::default());
    assert!(!format!("{f:?}").contains(NO_SVC), "fix_wording: {f:?}");
    // the facets that take an `Arc<dyn Any>`: the concrete Arc from `as_any_arc`, or the handle wrapped
    let inner: Arc<dyn iyakki_shared::agent::Tools> = Arc::new(crate::extagent::NoTools);
    for s in [h.clone().as_any_arc(), Arc::new(h.clone()) as Arc<dyn Any + Send + Sync>] {
        let wrapped = a.tool_wraps().linux_appimage_tools(inner.clone(), s.clone(), iyakki_sec::policy::Role::Admin.template(), "ann".into(), Arc::new(|_, _| {}));
        assert!(wrapped.defs().len() > inner.defs().len(), "linux_appimage_tools left the tools alone");
        assert!(Services::from_any(s).is_some(), "from_any");
    }
    // anything else is refused, not mistaken for the services
    assert!(Services::from_any(Arc::new(())).is_none());
}
