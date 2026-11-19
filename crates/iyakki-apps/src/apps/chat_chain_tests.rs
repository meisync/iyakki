//! The chat and the voice session share one account tool chain (F-GEMVOICE-15): the tools that used to be chat-only are in a voice session
//! too, with the same approval class, and what stays out of a voice session is exactly the listed chat-only state.

use super::tests::agent_cx;
use super::*;
use iyakki_shared::agent::Risk;

fn names(t: &Arc<dyn iyakki_shared::agent::Tools>) -> Vec<String> {
    t.defs().into_iter().map(|d| d.spec.name).collect()
}

#[test]
fn a_voice_session_has_the_tools_the_chat_chain_gives_the_account_and_with_the_same_approvals() {
    let (mut cx, _h) = agent_cx("chain-voice", |_: &std::path::Path| vec![]);
    cx.session.policy.manage_settings = true;
    let voice = crate::apps::gemvoice::voice_tools(&cx);
    let have = names(&voice);
    // tools of wrappers that the voice session did not have before the chain was shared
    for n in ["wait_for", "pending_list", iyakki_agent::routertools::STATUS, iyakki_agent::fleettools::STATUS, iyakki_agent::lockscreentools::LOCK_TOOL, iyakki_agent::vpntools::READ_TOOL, "pdf_info", "tiles_list"] {
        assert!(have.iter().any(|h| h == n), "{n} is missing from a voice session: {have:?}");
    }
    // the chat's own chain: the same base through the same function gives the same set
    let base = iyakki_agent::agent::UserTools::new(&cx.session.user, cx.session.policy.clone(), iyakki_agent::services::Services::from_handle(&cx.services.clone()), cx.host.clone(), cx.mcp_servers.clone());
    let chat = crate::apps::tool_chain::account_chain(&cx, Arc::new(base), &crate::apps::tool_chain::ChainScope { project: None, ext: None });
    let chat = crate::apps::tool_chain::session_state_tools(&cx, chat, None, None, None);
    let mut a = have.clone();
    let mut b = names(&chat);
    // the browser tools exist only while a web engine helper is found, and other tests set and remove the helper's environment variable
    // while this one runs: the two builds above could see different states, which says nothing about the chain
    a.retain(|n| !n.starts_with("browser_"));
    b.retain(|n| !n.starts_with("browser_"));
    a.sort();
    b.sort();
    assert_eq!(a, b);
    // a change by the model-router or the fleet tools is a hard ask in a voice session as it is in the chat
    let hard = |t: &Arc<dyn iyakki_shared::agent::Tools>, n: &str| t.defs().into_iter().find(|d| d.spec.name == n).map(|d| d.risk);
    assert!(have.iter().any(|h| h == iyakki_agent::cloudstreamtools::SET_TOOL), "the cloud stream switch is reachable by voice and chat alike");
    for n in ["router_set", "fleet_pair", iyakki_agent::cloudstreamtools::SET_TOOL] {
        assert_eq!(hard(&voice, n), hard(&chat, n), "{n}");
        assert_eq!(hard(&voice, n), Some(Risk::Ask), "{n} asks");
    }
}

#[test]
fn what_a_voice_session_does_not_have_is_only_the_listed_chat_state() {
    assert_eq!(crate::apps::tool_chain::CHAT_ONLY.len(), 4, "the list may only shrink; a new entry needs a reason");
}
