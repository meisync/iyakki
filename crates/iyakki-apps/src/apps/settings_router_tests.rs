//! Settings -> Model router (experimental): who sees it, that every change asks first and then does what the chat tool does, the script
//! review, the core's say over a strategy change, the run in progress with Cancel, and the page from a phone to a large window and at the
//! largest text. Fake machine, fake models; no real model was run.
use iyakki_agent::facets_h::RealServices as _;

use super::router_page::RouterAct;
use super::*;
use std::sync::Mutex;
use crate::apps::tests::{cx_with_admin_store, temp_dir};
use iyakki_agent::router::{ModelPool, RouterConfig};
use iyakki_shared::router_registry::Role;
use iyakki_agent::routertools::{self as rt, Backend as _};
use iyakki_agent_api::ServicesB6;

fn providers() -> Vec<ProviderCfg> {
    let mut l = ProviderCfg::new("lan", Kind::Compat);
    l.base_url = Some("http://127.0.0.1:9/v1".into());
    l.models = vec!["big-70b".into(), "small-7b".into()];
    let mut c = ProviderCfg::new("cloud", Kind::OpenAi);
    c.models = vec!["gpt-large".into()];
    vec![l, c]
}

struct Rig {
    cx: AppCx,
    a: SettingsApp,
    be: Arc<rt::FileBackend>,
    os: Arc<iyakki_core_fake::FakeOs>,
}

fn rig(tag: &str) -> Rig {
    let mut cx = cx_with_admin_store(tag);
    let os = Arc::new(iyakki_core_fake::FakeOs::new());
    cx.os = os.clone();
    let audit = cx.audit.clone();
    let user = cx.session.user.clone();
    let dir = temp_dir(&format!("{tag}-router"));
    let be = Arc::new(rt::FileBackend::new(dir, Arc::new(providers), Arc::new(move |w, d| audit.record(&user, w, d))).with_gate(os.router()));
    let mut a = SettingsApp::new(&cx).with_router_backend(cx.services.router_panel_over(Box::new(be.clone() as Arc<dyn rt::Backend>)).expect("a router backend"));
    go(&mut a, &cx);
    Rig { cx, a, be, os }
}

use iyakki_core_api::Os as _;

fn go(a: &mut SettingsApp, cx: &AppCx) {
    for _ in 0..80 {
        if a.tab() == Tab::ModelRouter {
            return;
        }
        a.handle(&Event::Key(Key::Right), cx);
    }
    panic!("no page ModelRouter");
}

fn key(r: &mut Rig, c: char) {
    r.a.handle(&Event::Key(Key::Char(c)), &r.cx);
}

fn yes(r: &mut Rig) {
    key(r, 'y');
}

fn draw_big(a: &mut SettingsApp, cx: &AppCx, w: u32, h: u32, big: f32) -> Gfx {
    let mut g = Gfx::new(w, h);
    g.clear(rgb(0, 0, 0));
    let s = (w as f32 / 1280.0).min(h as f32 / 720.0).clamp_safe(0.6, 3.0) * big;
    a.render(&mut g, R::new(0.0, 72.0, w as f32, h as f32 - 72.0), s, cx, 0);
    g
}

fn audit(cx: &AppCx) -> Vec<String> {
    cx.audit.recent(200)
}

#[test]
fn only_an_administrator_has_the_page_and_it_sits_under_the_assistant_group() {
    let r = rig("rtr-admin");
    assert!(r.a.tabs().contains(&Tab::ModelRouter));
    assert_eq!(Tab::ModelRouter.label(), "Model router (experimental)");
    assert_eq!(Tab::ModelRouter.group(), Tab::Assistant.group());
    let mut cx = cx_with_admin_store("rtr-user");
    cx.session.user = "amy".into();
    cx.session.role = iyakki_sec::policy::Role::User;
    cx.session.policy = iyakki_sec::policy::Role::User.template();
    assert!(!SettingsApp::new(&cx).tabs().contains(&Tab::ModelRouter));
    let cx = cx_with_admin_store("rtr-find");
    let mut a = SettingsApp::new(&cx);
    a.handle(&Event::Key(Key::Ctrl('f')), &cx);
    for c in "planner".chars() {
        a.handle(&Event::Key(Key::Char(c)), &cx);
    }
    let i = a.tabs().iter().position(|t| *t == Tab::ModelRouter).unwrap();
    assert!(a.nav_matches().contains(&i), "searching planner finds the page");
}

#[test]
fn every_change_asks_first_and_does_nothing_until_y_and_esc_leaves_it_as_it_was() {
    let mut r = rig("rtr-ask");
    assert!(!r.be.config().enabled);
    key(&mut r, 'o');
    assert!(matches!(r.a.modal, Some(Modal::Confirm(ConfirmKind::Router(_), _))), "a card first");
    assert!(!r.be.config().enabled, "nothing changed while the card is up");
    r.a.handle(&Event::Key(Key::Esc), &r.cx);
    assert!(r.a.modal.is_none() && !r.be.config().enabled);
    key(&mut r, 'o');
    yes(&mut r);
    assert!(r.be.config().enabled);
    assert!(audit(&r.cx).iter().any(|l| l.contains("router.set")), "the same audit line as the chat tool");
    // the farthest place a model may run goes to the next one
    key(&mut r, 'm');
    yes(&mut r);
    assert_eq!(r.be.config().max_zone, iyakki_shared::router_registry::Zone::Computer);
}

#[test]
fn a_models_role_cost_place_and_use_are_corrected_through_a_card_and_reset() {
    let mut r = rig("rtr-model");
    let models = r.a.router_models(&r.cx);
    assert_eq!(models.len(), 3);
    let first = models[0].id.clone();
    let before = models[0].clone();
    for (k, check) in [('r', 0), ('c', 1), ('z', 2), ('e', 3)] {
        key(&mut r, k);
        assert!(matches!(r.a.modal, Some(Modal::Confirm(ConfirmKind::Router(_), _))), "{k}");
        yes(&mut r);
        let o = r.be.config().overrides.get(&first).cloned().unwrap_or_default();
        match check {
            0 => assert!(o.role.is_some() && o.role != Some(before.role)),
            1 => assert!(o.cost.is_some() && o.cost != Some(before.cost)),
            2 => assert!(o.zone.is_some() && o.zone != Some(before.zone)),
            _ => assert_eq!(o.enabled, Some(false)),
        }
    }
    let after = r.a.router_models(&r.cx)[0].clone();
    assert!(!after.enabled && after.zone != before.zone && after.cost != before.cost && after.role != before.role);
    key(&mut r, 'u');
    yes(&mut r);
    assert!(r.be.config().overrides.is_empty(), "back to the hints");
    // Down chooses the next model
    r.a.handle(&Event::Key(Key::Down), &r.cx);
    key(&mut r, 'r');
    yes(&mut r);
    assert!(r.be.config().overrides.contains_key(&models[1].id));
    // a click on a row chooses it too
    r.a.handle(&Event::Row(2), &r.cx);
    key(&mut r, 'r');
    yes(&mut r);
    assert!(r.be.config().overrides.contains_key(&models[2].id));
    let _ = Role::Both;
}

#[test]
fn the_budget_form_checks_the_numbers_and_the_caps_and_asks_before_it_saves() {
    let mut r = rig("rtr-budget");
    let form = |t: &str, c: &str, a: &str, e: &str| Form::new("b").text("Tokens", t, "").text("Calls", c, "").text("Tries", a, "").text("Rounds", e, "");
    assert!(r.a.router_budget_submit(&r.cx, &form("many", "10", "3", "1")).is_err());
    assert!(r.a.router_budget_submit(&r.cx, &form("99999999999", "10", "3", "1")).unwrap_err().contains("too large"));
    assert!(r.a.router_budget_submit(&r.cx, &form("200000", "60", "4", "2")).unwrap_err().contains("Nothing changed"));
    assert!(r.a.router_budget_submit(&r.cx, &form("5000", "9", "3", "1")).is_ok());
    assert!(matches!(r.a.modal, Some(Modal::Confirm(ConfirmKind::Router(_), _))));
    assert_eq!(r.be.config().budget.max_tokens, 200_000, "not yet");
    yes(&mut r);
    let b = r.be.config().budget;
    assert_eq!((b.max_tokens, b.max_calls, b.max_attempts, b.max_escalations), (5000, 9, 3, 1));
    key(&mut r, 'b');
    assert!(matches!(r.a.modal, Some(Modal::Form(FormKind::RouterBudget, _))), "b opens the form");
}

const GOOD: &str = "function decide(i) { if (i.phase === 'plan') return {v: 1, planner: i.models[0].id}; return {v: 1, assign: [{step: i.ready[0], model: i.models[i.models.length - 1].id}]}; }";

fn home_file(r: &Rig, name: &str, text: &str) {
    let home = iyakki_agent::scope::home_root_for_writes(&r.cx.session.user, r.cx.data_dir());
    std::fs::create_dir_all(&home).unwrap();
    std::fs::write(home.join(name), text).unwrap();
}

#[test]
fn a_script_from_a_file_in_the_home_is_shown_in_full_tried_added_chosen_and_removed_each_after_a_card() {
    let mut r = rig("rtr-script");
    home_file(&r, "mine.js", GOOD);
    let form = |n: &str, f: &str| Form::new("s").text("Name", n, "").text("File", f, "");
    assert!(r.a.router_script_submit(&r.cx, &form("default", "mine.js")).is_err(), "the built-in name is taken");
    assert!(r.a.router_script_submit(&r.cx, &form("mine", "../etc/passwd")).unwrap_err().contains("inside your own folder"));
    assert!(r.a.router_script_submit(&r.cx, &form("mine", "missing.js")).unwrap_err().contains("not there"));
    assert!(r.a.router_script_submit(&r.cx, &form("mine", "mine.js")).is_ok());
    assert_eq!(r.a.router.review.as_ref().map(|x| x.1.as_str()), Some(GOOD), "the whole script is what is shown");
    assert_eq!(r.a.router_buttons().iter().map(|b| b.0).collect::<Vec<_>>(), ["Add this script", "Back"]);
    assert!(r.be.strategies().is_empty());
    yes(&mut r);
    assert!(matches!(r.a.modal, Some(Modal::Confirm(ConfirmKind::Router(RouterAct::Add(..)), _))), "then the card");
    assert!(r.be.strategies().is_empty(), "nothing kept before the card is answered");
    yes(&mut r);
    assert_eq!(r.be.strategies().len(), 1);
    // choose it, then remove it
    r.a.router_pick(&r.cx, PickKind::RouterStrategy, "mine");
    yes(&mut r);
    assert_eq!(r.be.config().strategy, "mine");
    r.a.router_pick(&r.cx, PickKind::RouterRemove, "mine");
    yes(&mut r);
    assert!(r.be.strategies().is_empty() && r.be.config().strategy == "default", "removing the one in use goes back to the default");
    let log = audit(&r.cx);
    for want in ["router.strategy.add", "router.strategy.set", "router.strategy.remove"] {
        assert!(log.iter().any(|l| l.contains(want)), "{want}: {log:?}");
    }
    // a script that fails the trial run is not kept
    home_file(&r, "bad.js", "function decide(i) { return 5; }");
    assert!(r.a.router_script_submit(&r.cx, &form("bad", "bad.js")).is_ok());
    yes(&mut r);
    yes(&mut r);
    assert!(r.be.strategies().is_empty(), "the trial run refused it");
    assert!(r.a.status.as_ref().is_some_and(|s| !s.1 && s.0.contains("trial run")), "{:?}", r.a.status);
    // x with nothing to remove says so
    key(&mut r, 'x');
    assert!(r.a.modal.is_none());
}

#[test]
fn the_core_can_refuse_a_strategy_change_and_nothing_changes() {
    let mut r = rig("rtr-core");
    home_file(&r, "mine.js", GOOD);
    r.be.save_strategy("mine", GOOD).unwrap();
    r.os.router_fake().refuse(true);
    r.a.router_pick(&r.cx, PickKind::RouterStrategy, "mine");
    yes(&mut r);
    assert_eq!(r.be.config().strategy, "default");
    assert!(r.a.status.as_ref().is_some_and(|s| !s.1), "{:?}", r.a.status);
    r.os.router_fake().refuse(false);
    r.a.router_pick(&r.cx, PickKind::RouterStrategy, "mine");
    yes(&mut r);
    assert_eq!(r.be.config().strategy, "mine");
    assert!(r.os.router_fake().calls().iter().any(|c| c == "allow_strategy(mine)"));
}

struct Held {
    open: Mutex<bool>,
    cv: std::sync::Condvar,
}

impl iyakki_shared::chat::ChatBackend for Held {
    fn complete(&self, _: &[iyakki_shared::chat::Message]) -> Result<String, String> {
        let mut g = self.open.lock().unwrap();
        while !*g {
            g = self.cv.wait(g).unwrap();
        }
        Ok(r#"{"status":"done","result":"ok"}"#.into())
    }
}

struct HeldPool(Arc<Held>);

impl ModelPool for HeldPool {
    fn backend(&self, _: &str) -> Option<Arc<dyn iyakki_shared::chat::ChatBackend>> {
        Some(self.0.clone())
    }
}

#[test]
fn a_run_in_progress_shows_on_the_page_with_a_cancel_button_that_stops_it() {
    let mut cx = cx_with_admin_store("rtr-run");
    let os = Arc::new(iyakki_core_fake::FakeOs::new());
    cx.os = os.clone();
    let held = Arc::new(Held { open: Mutex::new(false), cv: Default::default() });
    let audit = cx.audit.clone();
    let dir = temp_dir("rtr-run-router");
    let be: Arc<rt::FileBackend> = Arc::new(rt::FileBackend::new(dir, Arc::new(providers), Arc::new(move |w, d| audit.record("u", w, d))).with_gate(os.router()).with_pool(Arc::new(HeldPool(held.clone()))));
    let mut a = SettingsApp::new(&cx).with_router_backend(cx.services.router_panel_over(Box::new(be.clone() as Arc<dyn rt::Backend>)).expect("a router backend"));
    go(&mut a, &cx);
    be.save_config(&RouterConfig { enabled: true, planner: Some("lan/big-70b".into()), ..RouterConfig::default() }).unwrap();
    let dynbe: Arc<dyn rt::Backend> = be.clone();
    let plan: iyakki_agent::router::Plan = serde_json::from_str(r#"{"steps":[{"id":"a","task":"do a"},{"id":"b","task":"do b","after":["a"]}]}"#).unwrap();
    rt::start_run(&dynbe, "g", Some(plan)).unwrap();
    for _ in 0..500 {
        if be.slot().progress().is_some_and(|p| p.contains("0 of 2 steps")) {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    let g = draw_big(&mut a, &cx, 1280, 720, 1.0);
    let cancel = g.hits.iter().find(|h| h.label == "Cancel run").expect("a Cancel button");
    let ev = cancel.ev.clone();
    assert!(be.slot().progress().unwrap().contains("A run is in progress: 0 of 2 steps done"));
    a.handle(&ev, &cx);
    assert!(a.status.as_ref().is_some_and(|s| s.1 && s.0.contains("will stop")), "{:?}", a.status);
    *held.open.lock().unwrap() = true;
    held.cv.notify_all();
    be.slot().join();
    assert!(be.last_run().unwrap().starts_with("Outcome: cancelled"));
    let g = draw_big(&mut a, &cx, 1280, 720, 1.0);
    assert!(g.hits.iter().any(|h| h.label == "Cancel run"), "the button stays; with no run it says so");
    a.handle(&Event::Key(Key::Char('k')), &cx);
    assert!(a.status.as_ref().is_some_and(|s| s.0.contains("No run")));
}

#[test]
fn the_page_fits_the_window_from_a_phone_to_a_large_screen_and_at_the_largest_text() {
    for (w, h) in [(360u32, 740u32), (800, 480), (1280, 720), (1920, 1080)] {
        for big in [1.0f32, 2.0] {
            let mut r = rig(&format!("rtr-geo-{w}-{}", big as u32));
            home_file(&r, "mine.js", &GOOD.repeat(12));
            r.be.save_strategy("a-strategy-with-a-long-name", GOOD).unwrap();
            for review in [false, true] {
                if review {
                    let f = Form::new("s").text("Name", "mine", "").text("File", "mine.js", "");
                    r.a.router_script_submit(&r.cx, &f).unwrap();
                }
                let g = draw_big(&mut r.a, &r.cx, w, h, big);
                for hit in g.hits.iter().filter(|x| x.rect.w > 0.0) {
                    let q = hit.rect;
                    assert!(q.x >= -0.5 && q.x + q.w <= w as f32 + 0.5, "{w}x{h} x{big} review {review}: {:?} {q:?}", hit.label);
                }
                let buttons: Vec<_> = g.hits.iter().filter(|x| matches!(x.ev, Event::Key(Key::Char(_))) && x.rect.h < 90.0 * big).collect();
                assert!(buttons.len() >= 2, "{w}x{h} x{big} review {review}: {} buttons", buttons.len());
                let o = |p: R, q: R| p.x < q.x + q.w - 0.5 && q.x < p.x + p.w - 0.5 && p.y < q.y + q.h - 0.5 && q.y < p.y + p.h - 0.5;
                for (i, b) in buttons.iter().enumerate() {
                    assert!(buttons[i + 1..].iter().all(|n| !o(b.rect, n.rect)), "{w}x{h} x{big}: {} overlaps another button", b.label);
                }
            }
        }
    }
}
