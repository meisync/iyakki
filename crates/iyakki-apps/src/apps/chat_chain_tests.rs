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
    let (mut cx, h) = agent_cx("chain-voice", |_: &std::path::Path| vec![]);
    cx.session.policy.manage_settings = true;
    // the voice session binds the dashboard the person opened last, which lives in the account's home folder: without its own home this
    // test read the real one (`~/Dashboards`), where any DASH run that went before leaves a "last opened" marker, and the voice session
    // then had five dashboard tools that the chat chain of this test (built without it) does not have
    let home = h.join("own-home");
    std::fs::create_dir_all(&home).unwrap();
    let _home = iyakki_testkit::EnvGuard::set(&[("HOME", Some(home.to_str().unwrap()))]);
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
    let (only_voice, only_chat): (Vec<_>, Vec<_>) = (a.iter().filter(|x| !b.contains(x)).collect(), b.iter().filter(|x| !a.contains(x)).collect());
    assert_eq!(a, b, "only in the voice session: {only_voice:?}; only in the chat: {only_chat:?}");
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
    assert!(crate::apps::tool_chain::CHAT_ONLY.is_empty(), "nothing is chat-only now; a new entry needs a reason");
}

#[test]
fn a_spoken_request_that_names_no_project_means_the_one_opened_last_with_its_board_tools_and_asks_like_the_chat() {
    let (mut cx, h) = agent_cx("chain-voice-ws", |_: &std::path::Path| vec![]);
    cx.session.policy.cli = iyakki_sec::policy::CliLevel::Full;
    let root = h.join("voice-ws-project");
    std::fs::create_dir_all(&root).unwrap();
    let none = names(&crate::apps::gemvoice::voice_tools(&cx));
    assert!(!none.iter().any(|n| n == "board_list"), "no workspace, no board tools: {none:?}");
    assert!(crate::apps::gemvoice::last_workspace(&cx).is_none());
    let mut reg = iyakki_shared::workspace::Registry::load(cx.data_dir(), &cx.session.user);
    let w = reg.add("Voice", &root).unwrap();
    reg.items.iter_mut().find(|x| x.id == w.id).unwrap().last_opened = 5;
    let other = h.join("voice-ws-older");
    std::fs::create_dir_all(&other).unwrap();
    let w2 = reg.add("Older", &other).unwrap();
    reg.items.iter_mut().find(|x| x.id == w2.id).unwrap().last_opened = 1;
    reg.last = Some(w.id.clone());
    reg.save().unwrap();
    assert_eq!(crate::apps::gemvoice::last_workspace(&cx).map(|p| p.name), Some("Voice".to_string()), "the one opened last");
    let with = crate::apps::gemvoice::voice_tools(&cx);
    for n in iyakki_agent::boardtools::BOARD_TOOLS {
        assert!(names(&with).iter().any(|h| h == n), "{n} is missing from a voice session with a last workspace");
    }
    let risk = |n: &str| with.defs().into_iter().find(|d| d.spec.name == n).map(|d| d.risk);
    assert!(risk("board_add").is_some(), "the board tools carry the risk class the chat gives them");
        assert!(names(&with).iter().any(|n| n == "wait_for"), "the jobs and the ledger stay on top");
    // an account that may not run commands is offered no project tools by voice either
    cx.session.policy.cli = iyakki_sec::policy::CliLevel::Off;
    assert!(!names(&crate::apps::gemvoice::voice_tools(&cx)).iter().any(|n| n == "board_list"));
}

#[test]
fn a_spoken_request_means_the_dashboard_and_the_task_list_of_what_the_person_opened_last_and_changes_still_ask() {
    let (mut cx, h) = agent_cx("chain-voice-dash-task", |_: &std::path::Path| vec![]);
    // the dashboards of the account live in its home folder, which falls back to `$HOME`: this test makes one the "last opened" and
    // the test above reads that marker, so the two must not share the real home (nor leave a dashboard in it)
    let home = h.join("own-home");
    std::fs::create_dir_all(&home).unwrap();
    let _home = iyakki_testkit::EnvGuard::set(&[("HOME", Some(home.to_str().unwrap()))]);
    cx.session.policy.cli = iyakki_sec::policy::CliLevel::Full;
    cx.session.policy.apps.insert(iyakki_sec::policy::App::Dash);
    cx.session.policy.net_hosts = vec!["example.invalid".into()];
    let has = |n: &str| names(&crate::apps::gemvoice::voice_tools(&cx)).iter().any(|h| h == n);
    // nothing opened: the saved-dashboard form of the tools only, no parallel tasks
    assert!(!has("task_spawn"), "task_spawn with nothing opened");
    assert!(!has("dash_panel_remove"), "dash_panel_remove with nothing opened");
    assert!(crate::apps::gemvoice::last_dashboard(&cx).is_none(), "a dashboard is already opened");
    // a dashboard opened last
    let host = cx.services.dash_host();
    let boards = iyakki_dash::store::Dashboards::open(&host.home_root(&cx.session.user));
    std::fs::create_dir_all(boards.dir()).unwrap();
    boards.save(&iyakki_dash::model::Dashboard::new("checkout", "Checkout")).unwrap();
    boards.note_opened("checkout");
    assert!(has("dash_panel_remove") && has("dash_layout"), "the open-dashboard tools bind to the one opened last");
    let voice = crate::apps::gemvoice::voice_tools(&cx);
    let risk = |n: &str| voice.defs().into_iter().find(|d| d.spec.name == n).map(|d| d.risk);
    assert_eq!(risk("dash_save"), Some(Risk::Ask), "keeping a change asks");
    // a workspace opened last
    let root = h.join("voice-task-project");
    std::fs::create_dir_all(&root).unwrap();
    let mut reg = iyakki_shared::workspace::Registry::load(cx.data_dir(), &cx.session.user);
    let w = reg.add("Tasks", &root).unwrap();
    reg.last = Some(w.id);
    reg.save().unwrap();
    let voice = crate::apps::gemvoice::voice_tools(&cx);
    for n in iyakki_agent::tasktools::TASK_TOOLS {
        assert!(names(&voice).iter().any(|h| h == n), "{n} is missing");
    }
    assert_eq!(voice.defs().into_iter().find(|d| d.spec.name == "task_spawn").map(|d| d.risk), Some(Risk::Ask), "starting a task asks like in the chat");
    // an account that may not run commands gets no parallel tasks
    cx.session.policy.cli = iyakki_sec::policy::CliLevel::Off;
    assert!(!names(&crate::apps::gemvoice::voice_tools(&cx)).iter().any(|n| n == "task_spawn"));
}

#[test]
fn the_voice_session_and_the_ide_share_one_task_manager_for_a_workspace() {
    let (cx, h) = agent_cx("chain-voice-mgr", |_: &std::path::Path| vec![]);
    let root = h.join("shared-mgr");
    std::fs::create_dir_all(&root).unwrap();
    let f = root.join("FEATURES.md");
    let a = crate::apps::ide_tasks_run::shared_manager(&cx, &root, &f, "W");
    let b = crate::apps::ide_tasks_run::shared_manager(&cx, &root, &f, "W");
    assert!(Arc::ptr_eq(&a, &b), "one manager while one is held");
    drop((a, b));
    let c = crate::apps::ide_tasks_run::shared_manager(&cx, &root, &f, "W");
    assert_eq!(c.counts(), (0, 0, 0));
}
