//! The b0 facets answer quietly when a core does not serve them (the fake and `NoB0`), so a UI test never needs the agent.

use std::sync::Arc;

use iyakki_agent_api::{AgentFacets as _, B0Services, FakeAgentApi, NoB0};
use iyakki_sec::policy::Policy;
use iyakki_shared::target::Target;

#[test]
fn the_fake_agent_serves_no_b0_facet_and_every_default_refuses_or_is_empty() {
    let fake = FakeAgentApi::default();
    let b = fake.b0();
    b.providers().set_thinking_sink(None);
    assert_eq!(b.quiet().system().own_pid(), 0);
    assert!(b.quiet().system().amixer(&[]).is_err());
    let plan = b.target().plan_change(std::path::Path::new("/nonexistent"), "x", Target::Skill);
    assert_eq!(plan.to, Target::Skill);
    assert!(plan.drop_files.is_empty() && plan.summary.starts_with("unavailable"));
    assert!(b.target().apply_change(std::path::Path::new("/nonexistent"), "x", Target::Skill).is_err());
    assert!(!b.idebrowser().offered(&Policy::resolve(iyakki_sec::policy::Role::User, &Default::default())));
    assert!(b.idebrowser().take_requests("u", "w").is_empty());
    assert!(b.ocipush().http().request("GET", "h:1", "/", &[], &[]).is_err());
    assert!(b.linuxapps().set_workspace_sandbox(std::path::Path::new("/"), "u", std::path::Path::new("/x"), std::path::Path::new("/"), None).is_err());
}

#[test]
fn the_services_facet_does_nothing_without_a_core_and_hands_the_tools_back() {
    let s: &dyn B0Services = &NoB0;
    assert!(!s.tz_refresh_in_background(true, Arc::new(|_| {})));
    struct T;
    impl iyakki_shared::agent::Tools for T {
        fn defs(&self) -> Vec<iyakki_shared::agent::ToolDef> {
            Vec::new()
        }
        fn call(&self, _name: &str, _args: &serde_json::Value) -> Result<String, String> {
            Err("none".into())
        }
        fn describe(&self, _name: &str, _args: &serde_json::Value) -> String {
            String::new()
        }
    }
    let inner: Arc<dyn iyakki_shared::agent::Tools> = Arc::new(T);
    let out = s.ide_browser_tools(inner.clone(), Policy::resolve(iyakki_sec::policy::Role::User, &Default::default()), "u", "w", std::path::Path::new("/"));
    assert!(Arc::ptr_eq(&inner, &out));
}
