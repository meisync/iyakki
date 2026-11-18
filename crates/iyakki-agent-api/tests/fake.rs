//! The fake of the generated facade answers the default and records the call (F-SPLIT-03, pass 2).

use iyakki_agent_api::{AgentApi, AgentFiles, AgentOther, FakeAgentApi};

#[test]
fn the_fake_answers_the_default_and_records_which_call_was_made() {
    let f = FakeAgentApi::default();
    assert_eq!(f.devtools_scrub_url("https://x.test/?token=abc"), "");
    assert_eq!(f.gitconn_host_of("git@example.test:a/b.git"), "");
    assert_eq!(f.calls(), ["devtools::scrub_url", "gitconn::host_of"]);
}

#[test]
fn one_handle_serves_every_domain_trait() {
    fn takes(_: &dyn AgentApi) {}
    takes(&FakeAgentApi::default());
    // a trait of a domain is also a handle of its own
    fn only_devtools(a: &dyn AgentOther) -> String {
        a.devtools_scrub_text("secret")
    }
    assert_eq!(only_devtools(&FakeAgentApi::default()), "");
}
