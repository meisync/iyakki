//! Settings -> Assistant: the history switch, Forget per workspace and per chat (twice-pressed), the stall limits for all models and per
//! provider, the page at four sizes, the keyboard path and the mouse path, and the chat tools that do the same.

use super::*;
use crate::apps::tests::{cx_with_admin_store, services_in, temp_dir, Offline};
use iyakki_memx::history::History;
use iyakki_sec::policy::Role;

fn admin(tag: &str) -> AppCx {
    let mut cx = cx_with_admin_store(tag);
    let d = temp_dir(&format!("{tag}-svc"));
    cx.services = services_in(&d, Arc::new(Offline));
    cx.services.on_login("root", "rootpass12", true);
    cx
}

fn plain_user(tag: &str) -> AppCx {
    let mut cx = admin(tag);
    cx.session.user = "amy".into();
    cx.session.role = Role::User;
    cx.session.policy = Role::User.template();
    cx
}

fn remember(cx: &AppCx, ws: &str, chat: &str, n: usize) {
    let h = History::new(cx.kb().clone(), &cx.session.user, ws, Some(chat));
    for i in 0..n {
        h.record("turn", "", &format!("{ws} {chat} step {i}: the registry address is 10.0.0.{i}"));
    }
}

fn key(a: &mut SettingsApp, cx: &AppCx, k: Key) {
    a.handle(&Event::Key(k), cx);
}

fn status(a: &SettingsApp) -> String {
    a.status().map(|s| s.0.clone()).unwrap_or_default()
}

fn rows(a: &mut SettingsApp, cx: &AppCx) -> Vec<assistant_page::Row> {
    a.asst_rows(cx)
}

#[test]
fn everyone_has_the_page_and_the_search_finds_it() {
    let cx = plain_user("asst-tab");
    let mut a = SettingsApp::new(&cx);
    assert!(a.tabs.contains(&Tab::Assistant));
    let pos = |t: Tab| a.tabs.iter().position(|x| *x == t).unwrap();
    assert!(pos(Tab::Rag) < pos(Tab::Assistant) && pos(Tab::Assistant) < pos(Tab::Voice), "in the AI group, after the documents");
    assert_eq!(Tab::Assistant.group(), "Apps & AI");
    a.open_item("tab:assistant", &cx);
    assert_eq!(a.tab(), Tab::Assistant);
    assert!(Tab::Assistant.keywords().contains("forget") && Tab::Assistant.keywords().contains("stall"));
}

#[test]
fn the_switch_turns_the_history_off_and_on_and_is_audited() {
    let cx = admin("asst-switch");
    let mut a = SettingsApp::new(&cx);
    a.go_to(Tab::Assistant);
    assert!(!cx.settings().lock().unwrap().get().no_workspace_history);
    key(&mut a, &cx, Key::Char('h'));
    assert!(cx.settings().lock().unwrap().get().no_workspace_history);
    assert!(status(&a).contains("no longer keeps"));
    key(&mut a, &cx, Key::Enter); // the first row is the switch
    assert!(!cx.settings().lock().unwrap().get().no_workspace_history);
    assert!(cx.audit.recent(10).iter().any(|l| l.contains("settings.assistant.history")));
}

#[test]
fn forgetting_a_workspace_or_one_chat_needs_the_same_key_twice_and_touches_only_that() {
    let cx = admin("asst-forget");
    remember(&cx, "web-shop", "s_one", 3);
    remember(&cx, "web-shop", "s_two", 2);
    remember(&cx, "data-lab", "s_one", 4);
    let mut a = SettingsApp::new(&cx);
    a.go_to(Tab::Assistant);
    let r = rows(&mut a, &cx);
    let ws: Vec<String> = r.iter().filter_map(|x| if let assistant_page::Row::Workspace(w) = x { Some(format!("{}:{}:{}", w.name, w.entries, w.chats)) } else { None }).collect();
    assert_eq!(ws, vec!["data-lab:4:1", "web-shop:5:2"]);
    // select web-shop, show its chats
    key(&mut a, &cx, Key::Down);
    key(&mut a, &cx, Key::Down);
    key(&mut a, &cx, Key::Enter);
    let r = rows(&mut a, &cx);
    assert!(r.iter().any(|x| matches!(x, assistant_page::Row::Chat { chat, entries: 3, .. } if chat == "s-one")), "{r:?}");
    // one chat: the first F asks, anything else cancels, the second F forgets
    key(&mut a, &cx, Key::Down);
    key(&mut a, &cx, Key::Char('f'));
    assert!(status(&a).contains("Press F again"), "{}", status(&a));
    key(&mut a, &cx, Key::Down);
    key(&mut a, &cx, Key::Up);
    key(&mut a, &cx, Key::Char('f'));
    assert!(status(&a).contains("Press F again"), "a key in between cancelled the first press: {}", status(&a));
    key(&mut a, &cx, Key::Char('f'));
    assert!(status(&a).contains("Forgot 3 entries"), "{}", status(&a));
    let w = |name: &str| History::workspaces_of(&cx.kb(), "root").into_iter().find(|w| w.name == name).map(|w| w.entries);
    assert_eq!(w("web-shop"), Some(2));
    assert_eq!(w("data-lab"), Some(4), "another workspace is untouched");
    // a whole workspace
    a.asst.open = None;
    a.sel[a.tab] = 1; // data-lab
    key(&mut a, &cx, Key::Char('f'));
    key(&mut a, &cx, Key::Char('f'));
    assert_eq!(w("data-lab"), None);
    assert_eq!(w("web-shop"), Some(2));
    assert!(cx.audit.recent(10).iter().any(|l| l.contains("settings.assistant.forget")));
    // another person's history is never listed or forgotten here
    let amy = plain_user("asst-forget-amy");
    remember(&amy, "web-shop", "s_x", 2);
    assert_eq!(History::workspaces_of(&cx.kb(), "root").len(), 1);
}

#[test]
fn the_limits_move_in_steps_for_all_models_or_one_provider_and_a_plain_account_cannot() {
    let cx = admin("asst-limits");
    cx.settings().lock().unwrap().update(|s| s.providers.push(iyakki_shared::providers::ProviderCfg::new("local", iyakki_shared::providers::Kind::Ollama))).ok();
    let mut a = SettingsApp::new(&cx);
    a.go_to(Tab::Assistant);
    let n = rows(&mut a, &cx).len();
    let all = rows(&mut a, &cx).iter().position(|r| matches!(r, assistant_page::Row::Limits(None))).unwrap();
    a.sel[a.tab] = all;
    key(&mut a, &cx, Key::Char('t'));
    key(&mut a, &cx, Key::Char('t'));
    key(&mut a, &cx, Key::Char('q'));
    {
        let st = cx.settings().lock().unwrap();
        assert_eq!((st.get().think_limit_secs, st.get().silence_limit_secs), (60, 30));
    }
    key(&mut a, &cx, Key::Char('T'));
    assert_eq!(cx.settings().lock().unwrap().get().think_limit_secs, 30);
    // a provider of its own, over the machine-wide numbers
    if n > all + 1 {
        a.sel[a.tab] = all + 1;
        key(&mut a, &cx, Key::Char('t'));
        let p = match &rows_after(&a, &cx)[all + 1] { assistant_page::Row::Limits(Some(p)) => p.clone(), r => panic!("{r:?}") };
        let st = cx.settings().lock().unwrap();
        assert_eq!(st.get().stall_limits(&p), (30, 30), "its own think limit, the machine-wide silence limit: {:?}", st.get().provider_limits);
        assert_eq!(st.get().stall_limits("another"), (30, 30));
        drop(st);
        key(&mut a, &cx, Key::Char('0'));
        assert!(cx.settings().lock().unwrap().get().provider_limits.is_empty(), "back to following all models");
    }
    key(&mut a, &cx, Key::Char('0'));
    // steps stop at the ends
    assert_eq!(assistant_page::step_up(900), 900);
    assert_eq!(assistant_page::step_down(0), 0);
    assert_eq!(assistant_page::step_up(61), 120);
    assert_eq!(assistant_page::secs_words(120), "2 min");
    // a plain account sees the numbers but cannot change them
    let ucx = plain_user("asst-limits-user");
    let mut u = SettingsApp::new(&ucx);
    u.go_to(Tab::Assistant);
    let all = rows(&mut u, &ucx).iter().position(|r| matches!(r, assistant_page::Row::Limits(None))).unwrap();
    u.sel[u.tab] = all;
    key(&mut u, &ucx, Key::Char('t'));
    assert_eq!(ucx.services.settings().lock().unwrap().get().think_limit_secs, 0);
    assert!(status(&u).contains("administrator"), "{}", status(&u));
}

fn rows_after(a: &SettingsApp, cx: &AppCx) -> Vec<assistant_page::Row> {
    a.asst_rows(cx)
}

#[test]
fn the_page_draws_at_four_sizes_with_every_row_and_button_clickable_inside_the_window() {
    let cx = admin("asst-sizes");
    for (w, n) in [("a", 3), ("b", 2), ("c", 5)] {
        remember(&cx, &format!("ws-{w}"), "s_1", n);
    }
    let mut a = SettingsApp::new(&cx);
    a.go_to(Tab::Assistant);
    for (w, h) in [(1280u32, 720u32), (1920, 1080), (1024, 600), (800, 480)] {
        let mut g = Gfx::new(w, h);
        let s = (w as f32 / 1280.0).min(h as f32 / 720.0).clamp(0.6, 3.0);
        a.render(&mut g, R::new(0.0, 72.0 * s, w as f32, h as f32 - 72.0 * s), s, &cx, 0);
        assert!(g.hits.iter().any(|x| matches!(x.ev, Event::Row(0))), "{w}x{h}: the switch row can be clicked");
        for hit in &g.hits {
            assert!(hit.rect.x >= -0.5 && hit.rect.x + hit.rect.w <= w as f32 + 0.5, "{w}x{h}: {} is inside the width", hit.label);
        }
    }
    // the mouse path: a click on a workspace row selects it, the button shows its chats
    let mut g = Gfx::new(1280, 720);
    a.render(&mut g, R::new(0.0, 72.0, 1280.0, 648.0), 1.0, &cx, 0);
    a.handle(&Event::Row(1), &cx);
    assert_eq!(a.sel(), 1);
    assert!(a.buttons_for(true).iter().any(|b| b.2 == 'f'), "the Forget button is offered on a workspace row: {:?}", a.buttons_for(true).iter().map(|b| b.0).collect::<Vec<_>>());
}

struct NoTools;
impl iyakki_shared::agent::Tools for NoTools {
    fn defs(&self) -> Vec<iyakki_shared::agent::ToolDef> {
        vec![]
    }
    fn call(&self, name: &str, _a: &serde_json::Value) -> Result<String, String> {
        Err(format!("no tool {name}"))
    }
    fn describe(&self, name: &str, _a: &serde_json::Value) -> String {
        name.to_string()
    }
}

#[test]
fn the_chat_tools_do_what_the_page_does_with_the_same_asking_rules() {
    use iyakki_shared::agent::{Risk};
    use iyakki_agent::agent::{hard_ask};
    use serde_json::json;
    let cx = admin("asst-tools");
    remember(&cx, "web-shop", "s_one", 3);
    remember(&cx, "web-shop", "s_two", 2);
    let t = crate::apps::chat_settings::wrap(&cx, Arc::new(NoTools));
    let risk = |n: &str| t.defs().into_iter().find(|d| d.spec.name == n).map(|d| d.risk);
    for n in ["assistant_memory_status", "assistant_limits_status"] {
        assert_eq!(risk(n), Some(Risk::Safe), "{n}");
    }
    for n in ["assistant_memory_set", "assistant_memory_forget", "assistant_limits_set"] {
        assert_eq!(risk(n), Some(Risk::Ask), "{n}");
    }
    assert!(hard_ask("assistant_memory_forget", &json!({})).is_some(), "Auto never forgets and a grant never covers it");
    let status = t.call("assistant_memory_status", &json!({})).unwrap();
    assert!(status.contains("web-shop: 5 entries from 2 chat(s)") && status.contains("chat s-one: 3 entries"), "{status}");
    // forgetting: a preview first, a refusal for what is not there, then one chat only
    let one = json!({"workspace": "web-shop", "chat": "s-one"});
    assert!(t.preview("assistant_memory_forget", &one).unwrap().contains("Forget the 3 entries chat s-one left in web-shop"));
    assert!(t.refusal("assistant_memory_forget", &json!({"workspace": "nowhere"})).unwrap().contains("no history"));
    assert!(t.refusal("assistant_memory_forget", &json!({"workspace": "web-shop", "chat": "zzz"})).is_some());
    assert_eq!(t.call("assistant_memory_forget", &one).unwrap(), "Forgot 3 entries.");
    assert!(t.call("assistant_memory_status", &json!({})).unwrap().contains("web-shop: 2 entries"));
    assert_eq!(t.call("assistant_memory_forget", &json!({"workspace": "web-shop"})).unwrap(), "Forgot 2 entries.");
    assert!(t.call("assistant_memory_status", &json!({})).unwrap().contains("No workspace has any history"));
    // the switch and the limits
    t.call("assistant_memory_set", &json!({"remember": false})).unwrap();
    assert!(cx.settings().lock().unwrap().get().no_workspace_history);
    assert!(t.call("assistant_memory_set", &json!({})).is_err());
    assert!(t.refusal("assistant_limits_set", &json!({})).unwrap().contains("think_secs"));
    assert!(t.refusal("assistant_limits_set", &json!({"think_secs": 99999})).is_some());
    assert!(t.refusal("assistant_limits_set", &json!({"think_secs": 90, "provider": "nope"})).unwrap().contains("no provider"));
    t.call("assistant_limits_set", &json!({"think_secs": 90, "silence_secs": 200})).unwrap();
    let st = t.call("assistant_limits_status", &json!({})).unwrap();
    assert!(st.contains("All models: think 90 s, silent 200 s"), "{st}");
    assert!(cx.audit.recent(20).iter().any(|l| l.contains("settings.assistant.limits")) && cx.audit.recent(20).iter().any(|l| l.contains("settings.assistant.forget")));
    // an ordinary account gets the memory tools but not the limits change
    let u = plain_user("asst-tools-user");
    let ut = crate::apps::chat_settings::wrap(&u, Arc::new(NoTools));
    assert!(ut.defs().iter().any(|d| d.spec.name == "assistant_memory_forget"));
    assert!(!ut.defs().iter().any(|d| d.spec.name == "assistant_limits_set"));
}

#[test]
fn the_waiting_rows_are_drawn_and_switch_and_step_the_ledger_settings_with_an_audit() {
    let cx = admin("waitrows");
    let mut a = SettingsApp::new(&cx);
    a.open_item("tab:assistant", &cx);
    let rows = a.asst_rows(&cx);
    let first = rows.iter().position(|r| matches!(r, assistant_page::Row::Waiting("auto_continue"))).expect("the Waiting rows are listed");
    assert!(matches!(rows[first + 3], assistant_page::Row::Waiting("separate_thread")));
    let ledger = || iyakki_agent::pending::Ledger::open(&cx.data_dir().join("pending"), &cx.session.user);
    assert!(ledger().settings().auto_continue);
    let mut g = Gfx::new(1280, 1400);
    a.render(&mut g, R::new(0.0, 72.0, 1280.0, 1300.0), 1.0, &cx, 0);
    assert!(g.hits.iter().any(|h| h.label == "Continue by itself when waiting ends"), "{:?}", g.hits.iter().map(|h| h.label.clone()).collect::<Vec<_>>());
    for _ in 0..first {
        key(&mut a, &cx, Key::Down);
    }
    key(&mut a, &cx, Key::Enter);
    assert!(!ledger().settings().auto_continue, "Enter switches it off");
    key(&mut a, &cx, Key::Enter);
    assert!(ledger().settings().auto_continue);
    key(&mut a, &cx, Key::Down);
    let before = ledger().settings().max_wakes_per_day;
    key(&mut a, &cx, Key::Char('+'));
    assert_eq!(ledger().settings().max_wakes_per_day, before + 5);
    key(&mut a, &cx, Key::Char('-'));
    assert_eq!(ledger().settings().max_wakes_per_day, before);
    key(&mut a, &cx, Key::Down);
    key(&mut a, &cx, Key::Char('+'));
    assert_eq!(ledger().settings().max_wait_secs / 3600, 12);
    key(&mut a, &cx, Key::Down);
    key(&mut a, &cx, Key::Enter);
    assert!(ledger().settings().separate_thread);
}
