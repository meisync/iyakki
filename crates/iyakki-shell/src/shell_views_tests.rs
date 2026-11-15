//! Desktop views in the shell: switching keeps each view's apps and panes, the keys (a raw key stream through to the shell), the
//! locked view that asks for the password, the template that opens its apps, the strip, persistence across sign-out, and the limits.

use super::tests::*;
use super::*;
use crate::input::{InputState, Raw, EV_KEY};
use crate::memgate::{MemFacts, GIB};
use crate::panes::LayoutKey;
use iyakki_agent::views::{KeySet, Views, MAX_VIEWS};
use std::sync::atomic::{AtomicUsize, Ordering};

/// A stand-in app that counts how many of it were dropped (closed).
struct Dummy {
    name: &'static str,
    dropped: Arc<AtomicUsize>,
}

impl Drop for Dummy {
    fn drop(&mut self) {
        self.dropped.fetch_add(1, Ordering::SeqCst);
    }
}

impl App for Dummy {
    fn title(&self) -> &'static str {
        self.name
    }
    fn icon(&self) -> Icon {
        Icon::Chat
    }
    fn accent(&self) -> [u8; 4] {
        [90, 140, 220, 255]
    }
    fn hint(&self) -> String {
        "dummy".into()
    }
    fn handle(&mut self, _ev: &Event, _cx: &AppCx) -> Flow {
        Flow::Stay
    }
    fn render(&mut self, g: &mut Gfx, area: R, _s: f32, _cx: &AppCx, _tick: u64) {
        g.rect(area.x, area.y, area.w, area.h, [30, 30, 60, 255]);
        g.text(area.x + 8.0, area.y + 8.0, 16.0, [255, 255, 255, 255], self.name, Face::Bold);
    }
}

fn dummy(name: &'static str, dropped: &Arc<AtomicUsize>) -> Box<dyn App> {
    Box::new(Dummy { name, dropped: dropped.clone() })
}

fn signed_in(tag: &str, user: &str) -> (Shell, Gfx) {
    let (mut sh, _) = shell_with_users(tag);
    let mut g = Gfx::new(1280, 720);
    sh.render(&mut g);
    login(&mut sh, user, if user == "root" { "rootpass12" } else { "alicepass1" });
    sh.set_mem_for_test(Some(MemFacts { total: 16 * GIB, available: 12 * GIB }));
    sh.render(&mut g);
    (sh, g)
}

fn add_view(sh: &mut Shell, name: &str, tpl: Option<&str>) -> u32 {
    let id = sh.views.model.add(name, tpl).unwrap();
    sh.views_save();
    id
}

fn chord(sh: &mut Shell, k: LayoutKey) {
    sh.handle(&Event::Layout(k));
}

fn audit_text(sh: &Shell) -> String {
    std::fs::read_to_string(sh.env.audit.path()).unwrap_or_default()
}

#[test]
fn a_new_person_has_one_view_called_main_and_no_strip() {
    let (mut sh, mut g) = signed_in("vw-main", "alice");
    assert_eq!(sh.view_names(), vec![("Main".to_string(), false)]);
    assert_eq!(sh.view_name(), "Main");
    sh.render(&mut g);
    assert!(!sh.hits().iter().any(|h| h.label.starts_with("view ")), "no strip with a single view");
    chord(&mut sh, LayoutKey::ViewStep { back: false, mac: false });
    assert_eq!(sh.view_name(), "Main", "nothing to step to");
}

#[test]
fn switching_keeps_the_apps_and_the_panes_of_every_view() {
    let (mut sh, mut g) = signed_in("vw-keep", "alice");
    let closed = Arc::new(AtomicUsize::new(0));
    let two = add_view(&mut sh, "Docs", None);
    // view Main: two panes with two apps
    sh.show_app_for_test(dummy("ALPHA", &closed), PApp::Chat);
    chord(&mut sh, LayoutKey::Apply(2));
    sh.panes_put_for_test(1, dummy("BRAVO", &closed), PApp::Edit);
    sh.render(&mut g);
    assert_eq!(sh.panes_layout_for_test(), "cols-50-50");
    // go to Docs: a fresh launcher, no panes
    chord(&mut sh, LayoutKey::View(2));
    assert_eq!(sh.views.cur, two);
    assert_eq!(sh.view_name(), "Docs");
    assert_eq!(sh.screen_name(), "launcher");
    assert_eq!(sh.panes_layout_for_test(), "single");
    assert_eq!(closed.load(Ordering::SeqCst), 0, "nothing of Main was closed");
    sh.show_app_for_test(dummy("CHARLIE", &closed), PApp::Radio);
    sh.render(&mut g);
    // back to Main with the key for the first view: both panes are there as they were
    chord(&mut sh, LayoutKey::View(1));
    sh.render(&mut g);
    assert_eq!(sh.view_name(), "Main");
    assert_eq!(sh.panes_layout_for_test(), "cols-50-50");
    let titles = sh.panes_titles_for_test();
    assert!(titles.iter().any(|t| t == "ALPHA") && titles.iter().any(|t| t == "BRAVO"), "{titles:?}");
    assert_eq!(closed.load(Ordering::SeqCst), 0);
    // and Docs still has its app
    chord(&mut sh, LayoutKey::ViewStep { back: false, mac: false });
    assert_eq!(sh.screen_name(), "app:radio");
    assert_eq!(closed.load(Ordering::SeqCst), 0, "still nothing closed after two round trips");
    // signing out ends them all
    sh.handle(&Event::Layout(LayoutKey::View(1)));
    sh.sign_out();
    assert_eq!(closed.load(Ordering::SeqCst), 3);
}

#[test]
fn previous_and_next_wrap_around_and_a_missing_number_says_so() {
    let (mut sh, _g) = signed_in("vw-step", "alice");
    add_view(&mut sh, "B", None);
    add_view(&mut sh, "C", None);
    let next = LayoutKey::ViewStep { back: false, mac: false };
    let prev = LayoutKey::ViewStep { back: true, mac: false };
    chord(&mut sh, next);
    chord(&mut sh, next);
    assert_eq!(sh.view_name(), "C");
    chord(&mut sh, next);
    assert_eq!(sh.view_name(), "Main", "next wraps to the first");
    chord(&mut sh, prev);
    assert_eq!(sh.view_name(), "C", "previous wraps to the last");
    chord(&mut sh, LayoutKey::View(7));
    assert_eq!(sh.view_name(), "C");
    assert!(sh.toast.as_ref().is_some_and(|t| t.0.contains("no view 7")), "{:?}", sh.toast);
}

#[test]
fn the_person_chooses_which_step_keys_are_on() {
    let (mut sh, _g) = signed_in("vw-keyset", "alice");
    add_view(&mut sh, "B", None);
    sh.views.model.key_set = KeySet::Mac;
    chord(&mut sh, LayoutKey::ViewStep { back: false, mac: false });
    assert_eq!(sh.view_name(), "Main", "the browser-style key is off");
    assert!(sh.toast.as_ref().is_some_and(|t| t.0.contains("off")));
    chord(&mut sh, LayoutKey::ViewStep { back: false, mac: true });
    assert_eq!(sh.view_name(), "B");
    sh.views.model.key_set = KeySet::Browser;
    chord(&mut sh, LayoutKey::ViewStep { back: true, mac: true });
    assert_eq!(sh.view_name(), "B", "the Mac-style key is off");
    chord(&mut sh, LayoutKey::ViewStep { back: true, mac: false });
    assert_eq!(sh.view_name(), "Main");
    // the jump keys do not depend on the set
    sh.views.model.key_set = KeySet::Mac;
    chord(&mut sh, LayoutKey::View(2));
    assert_eq!(sh.view_name(), "B");
}

#[test]
fn raw_keys_reach_the_shell_as_view_switches_and_the_lock_chord_is_unchanged() {
    let (mut sh, _g) = signed_in("vw-raw", "alice");
    add_view(&mut sh, "B", None);
    add_view(&mut sh, "C", None);
    let mut st = InputState::new(800, 600);
    let mut feed = |sh: &mut Shell, code: u16, value: i32| {
        for ev in st.feed(Raw { typ: EV_KEY, code, value }, None) {
            sh.handle(&ev);
        }
    };
    // Ctrl+Alt+PageDown / PageUp (browser-style)
    feed(&mut sh, 29, 1);
    feed(&mut sh, 56, 1);
    feed(&mut sh, 109, 1);
    feed(&mut sh, 109, 0);
    assert_eq!(sh.view_name(), "B");
    feed(&mut sh, 104, 1);
    feed(&mut sh, 104, 0);
    assert_eq!(sh.view_name(), "Main");
    // Ctrl+Alt+Super+3 jumps to view 3; the same digit without Super is still a pane layout
    feed(&mut sh, 125, 1);
    feed(&mut sh, 4, 1);
    feed(&mut sh, 4, 0);
    assert_eq!(sh.view_name(), "C");
    feed(&mut sh, 125, 0);
    feed(&mut sh, 56, 0);
    feed(&mut sh, 29, 0);
    // Control+Command+Left / Right (Mac-style)
    feed(&mut sh, 29, 1);
    feed(&mut sh, 125, 1);
    feed(&mut sh, 106, 1);
    feed(&mut sh, 106, 0);
    assert_eq!(sh.view_name(), "Main", "Right from the last view wraps to the first");
    feed(&mut sh, 105, 1);
    feed(&mut sh, 105, 0);
    assert_eq!(sh.view_name(), "C");
    // the lock chord still locks: Control+Command+L (Control+Command+Q now asks to sign out)
    assert!(!sh.is_locked());
    feed(&mut sh, 38, 1);
    feed(&mut sh, 38, 0);
    assert!(sh.is_locked(), "Control+Command+L locks as before");
}

#[test]
fn a_locked_view_asks_for_the_password_and_only_then_opens() {
    let (mut sh, _g) = signed_in("vw-lock", "alice");
    let two = add_view(&mut sh, "Private", None);
    sh.views.model.set_locked(two, true);
    sh.views_save();
    chord(&mut sh, LayoutKey::View(2));
    assert!(sh.is_locked(), "the lock screen asks for the password");
    assert_eq!(sh.view_name(), "Main", "still the old view behind the lock");
    // a wrong password does not open it
    typ(&mut sh, "not the password");
    key(&mut sh, Key::Enter);
    assert!(sh.is_locked() && sh.view_name() == "Main");
    // the right one opens the lock and the view
    typ(&mut sh, "alicepass1");
    key(&mut sh, Key::Enter);
    assert!(!sh.is_locked());
    assert_eq!(sh.view_name(), "Private");
    assert!(sh.views.pending.is_none());
    let log = audit_text(&sh);
    assert!(log.contains("view.switch") && !log.contains("alicepass1") && !log.contains("not the password"), "no password in the audit log");
    // leaving a locked view is free; coming back asks again
    chord(&mut sh, LayoutKey::View(1));
    assert!(!sh.is_locked() && sh.view_name() == "Main");
    chord(&mut sh, LayoutKey::ViewStep { back: false, mac: false });
    assert!(sh.is_locked(), "asks again");
}

#[test]
fn an_unlock_without_a_view_request_stays_or_returns_to_the_first_view_by_choice() {
    let (mut sh, _g) = signed_in("vw-reset", "alice");
    add_view(&mut sh, "B", None);
    chord(&mut sh, LayoutKey::View(2));
    assert!(sh.lock_screen("test"));
    typ(&mut sh, "alicepass1");
    key(&mut sh, Key::Enter);
    assert_eq!(sh.view_name(), "B", "the default is to stay on the active view");
    sh.views.model.reset_on_lock = true;
    assert!(sh.lock_screen("test"));
    typ(&mut sh, "alicepass1");
    key(&mut sh, Key::Enter);
    assert_eq!(sh.view_name(), "Main", "the person's choice: back to the first view");
}

#[test]
fn the_active_view_and_the_views_are_kept_across_sign_out_and_belong_to_one_person() {
    let (mut sh, _g) = signed_in("vw-persist", "alice");
    add_view(&mut sh, "Second", None);
    chord(&mut sh, LayoutKey::View(2));
    assert_eq!(Views::load(sh.env.services.dir(), "alice").active, sh.views.cur, "saved when switched");
    sh.sign_out();
    assert_eq!(sh.view_names().len(), 1, "nothing of the last person stays");
    login(&mut sh, "alice", "alicepass1");
    assert_eq!(sh.view_name(), "Second", "signed in on the view that was active");
    sh.sign_out();
    login(&mut sh, "root", "rootpass12");
    assert_eq!(sh.view_names(), vec![("Main".to_string(), false)], "another person has their own views");
}

#[test]
fn a_template_view_opens_its_apps_in_panes_the_first_time() {
    let (mut sh, mut g) = signed_in("vw-tpl", "root");
    add_view(&mut sh, "Desk", Some("work"));
    chord(&mut sh, LayoutKey::View(2));
    sh.render(&mut g);
    let titles = sh.panes_titles_for_test();
    assert_eq!(sh.panes_layout_for_test(), "cols-50-50", "{titles:?}");
    assert_eq!(titles.len(), 2, "{titles:?}");
    // back and forth does not open them again
    chord(&mut sh, LayoutKey::View(1));
    chord(&mut sh, LayoutKey::View(2));
    sh.render(&mut g);
    assert_eq!(sh.panes_titles_for_test(), titles);
}

#[test]
fn a_template_leaves_out_apps_the_role_may_not_open() {
    // alice is a plain user; whatever she may not open is skipped without a message about a denied app
    let (mut sh, _g) = signed_in("vw-tpl-role", "alice");
    add_view(&mut sh, "Mon", Some("monitoring"));
    let before = audit_text(&sh);
    chord(&mut sh, LayoutKey::View(2));
    let after = audit_text(&sh);
    assert!(!after[before.len().min(after.len())..].contains("app.denied"), "no denied attempts: {after}");
}

#[test]
fn the_strip_shows_with_two_views_and_a_click_switches() {
    let (mut sh, mut g) = signed_in("vw-strip", "alice");
    add_view(&mut sh, "Other", None);
    sh.render(&mut g);
    let labels: Vec<String> = sh.hits().iter().map(|h| h.label.clone()).collect();
    assert!(labels.iter().any(|l| l == "view 1 Main") && labels.iter().any(|l| l == "view 2 Other"), "{labels:?}");
    let h = sh.hits().iter().rev().find(|h| h.label == "view 2 Other").unwrap().clone();
    let (x, y) = ((h.rect.x + h.rect.w / 2.0) as i32, (h.rect.y + h.rect.h / 2.0) as i32);
    sh.handle(&Event::MouseMove(x, y));
    sh.handle(&Event::MouseDown(x, y));
    sh.handle(&Event::MouseUp(x, y));
    assert_eq!(sh.view_name(), "Other");
    sh.views.model.set_locked(sh.views.cur, true);
    sh.render(&mut g);
    assert!(sh.hits().iter().any(|h| h.label.contains("(locked)")), "a locked view says so on its chip");
}

#[test]
fn the_count_is_bounded_and_the_last_view_cannot_go() {
    let (mut sh, _g) = signed_in("vw-limit", "alice");
    for i in 1..MAX_VIEWS {
        add_view(&mut sh, &format!("V{i}"), None);
    }
    assert_eq!(sh.view_names().len(), MAX_VIEWS);
    assert!(sh.views.model.add("Too many", None).is_err());
    chord(&mut sh, LayoutKey::View(9));
    assert_eq!(sh.view_name(), "V8");
}

#[test]
fn deleting_the_live_view_in_settings_puts_the_first_view_on_screen() {
    let (mut sh, _g) = signed_in("vw-del", "alice");
    let two = add_view(&mut sh, "Gone", None);
    chord(&mut sh, LayoutKey::View(2));
    assert_eq!(sh.views.cur, two);
    // Settings writes the file; leaving an app reads it again
    let mut m = Views::load(sh.env.services.dir(), "alice");
    m.delete(two).unwrap();
    m.save(sh.env.services.dir(), "alice");
    sh.views_reload();
    assert_eq!(sh.view_name(), "Main");
    assert_eq!(sh.view_names().len(), 1);
}

// ---- the strip on the real screen sizes, after the real ways of leaving Settings

fn strip_chip(sh: &Shell, label: &str) -> Option<(i32, i32)> {
    sh.hits().iter().rev().find(|h| h.label == label).map(|h| ((h.rect.x + h.rect.w / 2.0) as i32, (h.rect.y + h.rect.h / 2.0) as i32))
}

fn click_at(sh: &mut Shell, x: i32, y: i32) {
    sh.handle(&Event::MouseMove(x, y));
    sh.handle(&Event::MouseDown(x, y));
    sh.handle(&Event::MouseUp(x, y));
}

fn made_in_settings(sh: &mut Shell, user: &str, name: &str) {
    // what Settings -> Desktop views does: write the file, the shell is not told
    let mut m = Views::load(sh.env.services.dir(), user);
    m.add(name, None).unwrap();
    m.save(sh.env.services.dir(), user);
}

#[test]
fn the_strip_is_there_and_clickable_after_settings_made_a_view_on_both_sizes() {
    for (w, h) in [(1920u32, 1080u32), (2560, 1600)] {
        for way in ["home-icon", "back-click", "esc"] {
            let (mut sh, _) = signed_in(&format!("vw-real-{w}-{way}"), "alice");
            let mut g = Gfx::new(w, h);
            sh.render(&mut g);
            sh.open(PApp::Settings);
            sh.render(&mut g);
            made_in_settings(&mut sh, "alice", "Media");
            match way {
                "home-icon" => sh.handle(&Event::Tab(volbar_impl::HOME_TAB)),
                "back-click" => {
                    let s = scale_for(w, h);
                    let r = back_rect(w, s);
                    click_at(&mut sh, (r.x + r.w / 2.0) as i32, (r.y + r.h / 2.0) as i32);
                }
                _ => sh.handle(&Event::Key(Key::Esc)),
            }
            sh.render(&mut g);
            assert!(matches!(sh.screen, Screen::Launcher), "{way} {w}: back at Home");
            let (x, y) = strip_chip(&sh, "view 2 Media").unwrap_or_else(|| panic!("{way} {w}x{h}: no chip; {:?}", sh.hits().iter().map(|h| h.label.clone()).collect::<Vec<_>>()));
            click_at(&mut sh, x, y);
            assert_eq!(sh.view_name(), "Media", "{way} {w}x{h}");
        }
    }
}

#[test]
fn in_a_split_layout_the_chips_and_the_pane_strip_buttons_answer_to_the_mouse() {
    let mut ran = 0;
    for (w, h) in [(1920u32, 1080u32), (2560, 1600)] {
        for key in 2u8..=5 {
            let (mut sh, _) = signed_in(&format!("vw-split-{w}-{key}"), "root");
            let mut g = Gfx::new(w, h);
            sh.render(&mut g);
            add_view(&mut sh, "Media", None);
            chord(&mut sh, LayoutKey::Apply(1));
            chord(&mut sh, LayoutKey::Apply(key));
            sh.render(&mut g);
            sh.render(&mut g);
            if !sh.ws_active() {
                continue; // the template needs more than this test machine has
            }
            // a pane strip button by mouse: Layout of the second pane opens the picker
            let (x, y) = strip_chip(&sh, "pane 2 layout").unwrap_or_else(|| panic!("layout {key} {w}: no pane button"));
            click_at(&mut sh, x, y);
            sh.render(&mut g);
            assert!(sh.panes.picker.is_some(), "layout {key} {w}: the pane's Layout button opened the picker");
            sh.handle(&Event::Key(Key::Esc));
            sh.render(&mut g);
            // the view chip by mouse
            let (x, y) = strip_chip(&sh, "view 2 Media").unwrap_or_else(|| panic!("layout {key} {w}: no chip"));
            click_at(&mut sh, x, y);
            assert_eq!(sh.view_name(), "Media", "layout {key} {w}x{h}");
            sh.render(&mut g);
            let (x, y) = strip_chip(&sh, "view 1 Main").unwrap();
            click_at(&mut sh, x, y);
            assert_eq!(sh.view_name(), "Main");
            assert!(sh.ws_active(), "the split layout is back with its view");
            ran += 1;
        }
    }
    assert!(ran >= 4, "the split layouts really ran ({ran})");
}

#[test]
fn the_strip_is_a_visible_bar_and_the_home_hint_says_to_click() {
    let (mut sh, _) = signed_in("vw-vis", "alice");
    let mut g = Gfx::new(1920, 1080);
    sh.render(&mut g);
    assert!(!sh.hits().iter().any(|h| h.label.starts_with("view ")));
    add_view(&mut sh, "Media", None);
    sh.render(&mut g);
    g.text_log.clear();
    sh.render(&mut g);
    let h = sh.hits().iter().find(|h| h.label == "view 2 Media").unwrap().clone();
    // the bar around the chips is lighter at its edge than the wall beside it, and the idle chip is not the old near-black
    let px = |x: f32, y: f32| g.pm.pixels()[(y as usize) * 1920 + x as usize];
    let chip = px(h.rect.x + 3.0, h.rect.y + h.rect.h / 2.0);
    assert!(chip.red() as u32 + chip.green() as u32 + chip.blue() as u32 > 3 * 28 + 40, "idle chip is clearly lighter than the wall");
    let edge = px(h.rect.x + h.rect.w + 7.0, h.rect.y + h.rect.h / 2.0);
    let wall = px(h.rect.x - 400.0, h.rect.y + h.rect.h / 2.0);
    assert!(edge.blue() > wall.blue() + 10, "the bar has an edge that stands out from the wall");
    assert!(g.text_log.iter().any(|(_, _, t)| t.contains("click a view")), "the Home hint mentions clicking a view");
}

#[test]
fn a_view_gets_its_window_layout_back_after_a_switch_and_after_sign_out_not_its_template() {
    let (mut sh, mut g) = signed_in("vw-layout", "root");
    let desk = add_view(&mut sh, "Desk", Some("work"));
    chord(&mut sh, LayoutKey::View(2));
    sh.render(&mut g);
    let titles = sh.panes_titles_for_test();
    assert_eq!((sh.panes_layout_for_test().as_str(), titles.len()), ("cols-50-50", 2));
    // the person moves the focus to the second pane; leaving the view keeps layout, apps and focus
    sh.ws_focus(1);
    chord(&mut sh, LayoutKey::View(1));
    let saved = sh.views.model.get(desk).unwrap().layout.clone().expect("kept when the view was left");
    assert_eq!((saved.layout.as_str(), saved.apps.as_slice(), saved.focus), ("cols-50-50", &["edit".to_string(), "cli".to_string()][..], 1));
    // the template changes meanwhile: the arrangement that was left wins after a sign-out and a new sign-in
    sh.views.model.views[1].tiles = vec!["chat".into()];
    sh.views_save();
    sh.sign_out();
    sh.set_mem_for_test(Some(MemFacts { total: 16 * GIB, available: 12 * GIB }));
    login(&mut sh, "root", "rootpass12");
    sh.render(&mut g);
    chord(&mut sh, LayoutKey::View(2));
    sh.render(&mut g);
    assert_eq!(sh.panes_titles_for_test(), titles, "the same two apps in the same two panes");
    assert_eq!(sh.panes_layout_for_test(), "cols-50-50");
}

#[test]
fn the_view_that_is_showing_at_sign_out_is_remembered_too_and_remembering_can_be_switched_off() {
    let (mut sh, mut g) = signed_in("vw-layout-live", "root");
    add_view(&mut sh, "Desk", Some("work"));
    chord(&mut sh, LayoutKey::View(2));
    sh.render(&mut g);
    let titles = sh.panes_titles_for_test();
    sh.views.model.views[1].tiles = Vec::new();
    sh.sign_out();
    let saved = Views::load(sh.env.services.dir(), "root");
    assert_eq!(saved.get(saved.active).unwrap().layout.as_ref().map(|l| l.apps.len()), Some(2), "written at sign-out");
    sh.set_mem_for_test(Some(MemFacts { total: 16 * GIB, available: 12 * GIB }));
    login(&mut sh, "root", "rootpass12");
    sh.render(&mut g);
    assert_eq!(sh.view_name(), "Desk");
    assert_eq!(sh.panes_titles_for_test(), titles, "signed in on the view with its panes back");
    // off: nothing is kept and the next view shows its template again
    sh.views.model.set_remember_layout(false);
    sh.views_save();
    sh.sign_out();
    assert!(Views::load(sh.env.services.dir(), "root").views.iter().all(|v| v.layout.is_none()));
}

#[test]
fn the_one_window_rule_goes_to_the_open_app_in_a_pane_or_another_view_and_is_off_by_default() {
    let (mut sh, mut g) = signed_in("vw-single", "root");
    add_view(&mut sh, "Desk", Some("work"));
    chord(&mut sh, LayoutKey::View(2));
    sh.render(&mut g);
    let titles = sh.panes_titles_for_test();
    assert_eq!(sh.screen_name(), "app:edit".to_string(), "pane 0 (the editor) has the focus: {titles:?}");
    // off: opening the terminal tile again opens a second one in the focused pane
    sh.open(PApp::Cli);
    assert_eq!(sh.screen_name(), "app:cli");
    sh.open_for_test(PApp::Edit);
    sh.views.model.set_single(&["cli".to_string()]).unwrap();
    // on for CLI: it is open in the other pane, so the focus goes there and nothing new opens
    sh.open(PApp::Cli);
    sh.render(&mut g);
    assert_eq!(sh.screen_name(), "app:cli");
    assert_eq!(sh.panes_titles_for_test().len(), 2, "still two panes");
    // another view: Main shows the launcher; open Calendar there with the rule on, then ask for it from Desk
    sh.views.model.set_single(&["*".to_string()]).unwrap();
    chord(&mut sh, LayoutKey::View(1));
    sh.open(PApp::Calendar);
    chord(&mut sh, LayoutKey::View(2));
    sh.open(PApp::Calendar);
    sh.render(&mut g);
    assert_eq!(sh.view_name(), "Main", "went to the view that has it");
    assert_eq!(sh.screen_name(), "app:calendar");
}
