//! Settings -> Screen saver: the page is there for everyone, the choices save per person (and for the sign-in screen, for
//! administrators), guests use the standard ones, the keyboard works, Preview asks the shell, and the cards and buttons lie inside
//! the room at every size and in panes.

use super::screensaver_page::SsRow;
use super::*;
use crate::apps::tests::{cx_with_admin_store, services_in, temp_dir, Offline};
use iyakki_agent::screensaver::{self as ss, First, SaverConfig, Style};
use iyakki_sec::policy::Role;

fn admin(tag: &str) -> AppCx {
    let mut cx = cx_with_admin_store(tag);
    let d = temp_dir(&format!("{tag}-svc"));
    cx.services = services_in(&d, Arc::new(Offline));
    cx.services.on_login("root", "rootpass12", true);
    cx
}

fn as_role(mut cx: AppCx, user: &str, role: Role) -> AppCx {
    cx.session.user = user.into();
    cx.session.role = role;
    cx.session.policy = role.template();
    cx
}

fn page(cx: &AppCx) -> SettingsApp {
    let mut a = SettingsApp::new(cx);
    a.go_to(Tab::ScreenSaver);
    a
}

fn draw(a: &mut SettingsApp, cx: &AppCx, w: u32, h: u32) -> Gfx {
    let mut g = Gfx::new(w, h);
    let s = (w as f32 / 1280.0).min(h as f32 / 720.0).clamp(0.6, 3.0);
    g.clear(rgb(0, 0, 0));
    a.render(&mut g, R::new(0.0, 72.0 * s, w as f32, h as f32 - 72.0 * s), s, cx, 0);
    g
}

fn press(a: &mut SettingsApp, cx: &AppCx, k: Key) {
    a.handle(&Event::Key(k), cx);
}

/// Click the button `idx` of card `row`, as the shell would deliver it.
fn click(a: &mut SettingsApp, cx: &AppCx, row: u8, idx: usize) {
    a.sel[a.tab] = row as usize; // the card is in view (the page scrolls to the selected one)
    let g = draw(a, cx, 1920, 1080);
    let want = Event::Sub(row, Key::Char(char::from(b'a' + idx as u8)));
    let hit = g.hits.iter().find(|h| h.ev == want).unwrap_or_else(|| panic!("no button {idx} on card {row}"));
    a.handle(&hit.ev.clone(), cx);
}

fn mine(cx: &AppCx) -> SaverConfig {
    SaverConfig::load(cx.services.dir(), &cx.session.user)
}

#[test]
fn the_page_is_in_display_and_sound_for_everyone() {
    for role in [Role::Admin, Role::Operator, Role::User, Role::Guest] {
        let cx = as_role(admin("ss-list"), "somebody", role);
        let a = SettingsApp::new(&cx);
        assert!(a.tabs.contains(&Tab::ScreenSaver), "{role:?} has the page");
    }
    assert_eq!(Tab::ScreenSaver.group(), "Display & sound");
    assert_eq!(Tab::ScreenSaver.label(), "Screen saver");
    assert_eq!(Tab::ScreenSaver.cloud_page(), Some("display"), "hidden where there is no screen");
    let k = Tab::ScreenSaver.keywords();
    for w in ["screen saver", "screen off", "sleep", "blank", "dim", "oled", "battery"] {
        assert!(k.contains(w), "{w} finds the page");
    }
    let order = Tab::ORDER.iter().position(|t| *t == Tab::ScreenSaver).unwrap();
    assert!(Tab::ORDER[order - 1] == Tab::Displays && Tab::ORDER[order + 1] == Tab::HomeLayout);
}

#[test]
fn a_new_person_sees_the_standard_choices_and_a_click_changes_one_and_saves_it() {
    let cx = as_role(admin("ss-click"), "amy", Role::User);
    let mut a = page(&cx);
    assert_eq!(mine(&cx), SaverConfig::default());
    assert!(!SaverConfig::has_saved(cx.services.dir(), "amy"));
    click(&mut a, &cx, 1, 5); // idle for 15 min
    assert_eq!(mine(&cx).after_min, 15);
    assert!(SaverConfig::has_saved(cx.services.dir(), "amy"));
    click(&mut a, &cx, 3, 2); // small logo
    assert_eq!(mine(&cx).style, Style::Small);
    click(&mut a, &cx, 4, 2); // dim to 25 %
    assert_eq!(mine(&cx).dim_pct, 25);
    click(&mut a, &cx, 5, 1); // battery halving off
    assert!(!mine(&cx).battery_shorter);
    click(&mut a, &cx, 2, 3); // then off after 10 min
    assert_eq!(mine(&cx).then_off_min, 10);
    click(&mut a, &cx, 0, 2); // turn the screen off
    assert_eq!(mine(&cx).first, First::Off);
    // the earlier choices are still there
    let c = mine(&cx);
    assert_eq!((c.after_min, c.style, c.dim_pct, c.battery_shorter), (15, Style::Small, 25, false));
    // somebody else is not affected
    assert_eq!(SaverConfig::load(cx.services.dir(), "bob"), SaverConfig::default());
    assert!(a.status().is_some_and(|s| s.1), "Saved.");
}

#[test]
fn the_second_stage_only_applies_with_the_screen_saver() {
    let cx = as_role(admin("ss-then"), "amy", Role::User);
    let mut a = page(&cx);
    click(&mut a, &cx, 0, 2); // screen off directly
    let g = draw(&mut a, &cx, 1920, 1080);
    assert!(!g.hits.iter().any(|h| matches!(h.ev, Event::Sub(2, _))), "its buttons are not offered");
    a.handle(&Event::Sub(2, Key::Char('a')), &cx); // a stray one
    assert_eq!(mine(&cx).then_off_min, 10, "unchanged");
    click(&mut a, &cx, 0, 1);
    click(&mut a, &cx, 2, 0);
    assert_eq!(mine(&cx).then_off_min, 0);
}

#[test]
fn every_card_has_the_choices_it_says_and_marks_the_current_one() {
    let cx = as_role(admin("ss-chips"), "amy", Role::User);
    let c = SaverConfig::default();
    assert_eq!(SettingsApp::ss_chips(SsRow::After, &c).0, ["Never", "1 min", "2 min", "5 min", "10 min", "15 min", "30 min"]);
    assert_eq!(SettingsApp::ss_chips(SsRow::After, &c).1, Some(4));
    assert_eq!(SettingsApp::ss_chips(SsRow::Then, &c).0, ["Not at all", "1 min", "5 min", "10 min", "30 min"]);
    assert_eq!(SettingsApp::ss_chips(SsRow::First, &c).0, ["Do nothing", "Screen saver", "Turn the screen off"]);
    assert_eq!(SettingsApp::ss_chips(SsRow::First, &c).1, Some(1));
    assert_eq!(SettingsApp::ss_chips(SsRow::Dim, &c).0, ["Do not dim", "50%", "25%", "10%"]);
    assert_eq!(SettingsApp::ss_chips(SsRow::Style, &c).0.len(), 4);
    assert_eq!(SettingsApp::ss_chips(SsRow::Battery, &c).1, Some(0), "on by default");
    let _ = cx;
}

#[test]
fn the_keyboard_walks_the_cards_and_takes_the_next_or_previous_choice() {
    let cx = as_role(admin("ss-keys"), "amy", Role::User);
    let mut a = page(&cx);
    assert_eq!(a.sel(), 0);
    press(&mut a, &cx, Key::Down);
    press(&mut a, &cx, Key::Down);
    assert_eq!(a.sel(), 2);
    for _ in 0..20 {
        press(&mut a, &cx, Key::Down);
    }
    assert_eq!(a.sel(), 8, "the last card of a person's page (Lock the screen now)");
    press(&mut a, &cx, Key::Home);
    assert_eq!(a.sel(), 0);
    press(&mut a, &cx, Key::Up);
    assert_eq!(a.sel(), 0);
    // card 1 (idle for): Enter takes the next, N the previous, round the ends
    press(&mut a, &cx, Key::Down);
    press(&mut a, &cx, Key::Enter);
    assert_eq!(mine(&cx).after_min, 15, "10 -> 15");
    press(&mut a, &cx, Key::Char('N'));
    press(&mut a, &cx, Key::Char('N'));
    assert_eq!(mine(&cx).after_min, 5);
    for _ in 0..4 {
        press(&mut a, &cx, Key::Char('N'));
    }
    assert_eq!(mine(&cx).after_min, 30, "from Never back round to 30");
    press(&mut a, &cx, Key::Char(' '));
    assert_eq!(mine(&cx).after_min, 0, "30 -> Never");
    // the buttons bar
    let names: Vec<&str> = a.buttons_for(false).iter().map(|b| b.0).collect();
    assert_eq!(names, ["Next choice", "Previous", "Preview"]);
}

#[test]
fn preview_asks_the_shell_to_show_the_saver_now() {
    let cx = as_role(admin("ss-preview"), "amy", Role::User);
    let mut a = page(&cx);
    press(&mut a, &cx, Key::Char('p'));
    assert_eq!(ss::take_request(cx.services.dir(), "amy"), Some(ss::Request::Preview));
    // the Preview card's button
    a.handle(&Event::Sub(6, Key::Char('a')), &cx);
    assert_eq!(ss::take_request(cx.services.dir(), "amy"), Some(ss::Request::Preview));
    // a guest can preview too
    let g = as_role(admin("ss-preview-g"), "gus", Role::Guest);
    let mut ga = page(&g);
    press(&mut ga, &g, Key::Char('p'));
    assert_eq!(ss::take_request(g.services.dir(), "gus"), Some(ss::Request::Preview));
}

#[test]
fn a_guest_uses_the_standard_choices_and_changes_nothing() {
    let cx = as_role(admin("ss-guest"), "gus", Role::Guest);
    let mut a = page(&cx);
    let g = draw(&mut a, &cx, 1920, 1080);
    assert!(!g.hits.iter().any(|h| matches!(h.ev, Event::Sub(r, _) if r != 6)), "only the preview button is live");
    a.handle(&Event::Sub(1, Key::Char('b')), &cx);
    press(&mut a, &cx, Key::Enter);
    assert!(!SaverConfig::has_saved(cx.services.dir(), "gus"), "nothing was saved");
    assert_eq!(SettingsApp::ss_person(&cx), SaverConfig::default());
    assert!(a.status().is_some_and(|s| s.0.contains("Guests")));
}

#[test]
fn only_an_administrator_sees_and_changes_the_sign_in_screen() {
    let user = as_role(admin("ss-si-user"), "amy", Role::User);
    let mut ua = page(&user);
    assert_eq!(SettingsApp::ss_len(&user), 9);
    let g = draw(&mut ua, &user, 1920, 1080);
    assert!(!g.hits.iter().any(|h| matches!(h.ev, Event::Sub(r, _) if r >= 9)));
    ua.handle(&Event::Sub(9, Key::Char('b')), &user); // a stray event for a card that is not there
    assert_eq!(SettingsApp::ss_signin(&user), SaverConfig::signin_default());

    let cx = admin("ss-si-admin");
    let mut a = page(&cx);
    assert_eq!(SettingsApp::ss_len(&cx), 12);
    assert_eq!(SettingsApp::ss_signin(&cx), SaverConfig::signin_default(), "5 minutes, then off 10 minutes later");
    click(&mut a, &cx, 10, 1); // sign-in screen: idle for 1 min
    assert_eq!(SettingsApp::ss_signin(&cx).after_min, 1);
    click(&mut a, &cx, 11, 2); // then off after 5 min
    assert_eq!(SettingsApp::ss_signin(&cx).then_off_min, 5);
    click(&mut a, &cx, 9, 2); // sign-in screen goes off at once
    assert_eq!(SettingsApp::ss_signin(&cx).first, First::Off);
    assert_eq!(mine(&cx), SaverConfig::default(), "the administrator's own choices are separate");
    // the machine's settings file holds it
    assert_eq!(cx.services.settings.lock().unwrap().get().screensaver_signin.0.first, First::Off);
}

const SIZES: [(u32, u32); 6] = [(360, 740), (800, 480), (1280, 720), (1920, 1080), (2560, 1600), (2880, 1800)];

fn disjoint(a: R, b: R) -> bool {
    a.x + a.w <= b.x + 0.5 || b.x + b.w <= a.x + 0.5 || a.y + a.h <= b.y + 0.5 || b.y + b.h <= a.y + 0.5
}

#[test]
fn the_page_draws_at_six_sizes_with_every_hit_inside_the_room_and_no_two_buttons_over_each_other() {
    for (name, role, user) in [("admin", Role::Admin, "root"), ("user", Role::User, "amy"), ("guest", Role::Guest, "gus")] {
        let cx = as_role(admin("ss-sizes"), user, role);
        let mut a = page(&cx);
        for (w, h) in SIZES {
            for sel in 0..SettingsApp::ss_len(&cx) {
                a.sel[a.tab] = sel;
                let g = draw(&mut a, &cx, w, h);
                let s = (w as f32 / 1280.0).min(h as f32 / 720.0).clamp(0.6, 3.0);
                let room = R::new(0.0, 72.0 * s, w as f32, h as f32 - 72.0 * s);
                let mut chips: Vec<(R, String)> = Vec::new();
                for hit in &g.hits {
                    if matches!(hit.ev, Event::Sub(..) | Event::Row(..)) {
                        assert!(hit.rect.x >= -0.5 && hit.rect.x + hit.rect.w <= w as f32 + 0.5, "{name} {w}x{h}: {} inside the width", hit.label);
                        assert!(hit.rect.y >= room.y - 0.5 && hit.rect.y + hit.rect.h <= room.y + room.h + 0.5, "{name} {w}x{h} sel {sel}: {} inside the page room {room:?}: {:?}", hit.label, hit.rect);
                    }
                    if matches!(hit.ev, Event::Sub(..)) {
                        chips.push((hit.rect, hit.label.clone()));
                    }
                }
                for i in 0..chips.len() {
                    for j in i + 1..chips.len() {
                        assert!(disjoint(chips[i].0, chips[j].0), "{name} {w}x{h}: {} and {} overlap", chips[i].1, chips[j].1);
                    }
                }
                // the selected card is always on the page
                assert!(g.hits.iter().any(|h| h.ev == Event::Row(sel)), "{name} {w}x{h}: card {sel} is in view");
            }
        }
    }
}

#[test]
fn in_a_pane_the_cards_stay_inside_the_pane() {
    let cx = admin("ss-pane");
    let mut a = page(&cx);
    for (pw, ph, s) in [(420.0f32, 420.0f32, 0.7f32), (640.0, 300.0, 0.8), (900.0, 600.0, 1.0), (330.0, 700.0, 0.6)] {
        for sel in [0usize, 4, 9] {
            a.sel[a.tab] = sel;
            let mut g = Gfx::new(pw as u32 + 40, ph as u32 + 40);
            g.clear(rgb(0, 0, 0));
            let area = R::new(20.0, 20.0, pw, ph);
            a.render(&mut g, area, s, &cx, 0);
            for hit in g.hits.iter().filter(|h| matches!(h.ev, Event::Sub(..) | Event::Row(..))) {
                assert!(hit.rect.x >= area.x - 0.5 && hit.rect.x + hit.rect.w <= area.x + area.w + 0.5, "{pw}x{ph}: {} inside the pane", hit.label);
                assert!(hit.rect.y >= area.y - 0.5 && hit.rect.y + hit.rect.h <= area.y + area.h + 0.5, "{pw}x{ph}: {} inside the pane", hit.label);
            }
        }
    }
}

#[test]
fn a_small_screen_scrolls_to_the_selected_card_and_the_wheel_scrolls_the_cards_not_the_choice() {
    let cx = admin("ss-scroll");
    let mut a = page(&cx);
    a.sel[a.tab] = 9;
    let g = draw(&mut a, &cx, 800, 480);
    assert!(g.hits.iter().any(|h| h.ev == Event::Row(9)), "the last card scrolled into view");
    assert!(!g.hits.iter().any(|h| h.ev == Event::Row(0)), "the first one made room");
    // the wheel moves the cards, the choice stays; the cards above come back
    a.handle(&Event::Scroll(10), &cx);
    let g = draw(&mut a, &cx, 800, 480);
    assert_eq!(a.sel(), 9, "the wheel does not move the choice");
    assert!(g.hits.iter().any(|h| h.ev == Event::Row(0)), "the wheel scrolled back to the first card");
    // a click on a card selects it and no card moves
    let at = |g: &Gfx, i: usize| g.hits.iter().find(|h| h.ev == Event::Row(i)).map(|h| h.rect.y as i32);
    let shown: Vec<usize> = (0..10).filter(|i| at(&g, *i).is_some()).collect();
    let last = *shown.last().unwrap();
    a.handle(&Event::Row(last), &cx);
    assert_eq!(a.sel(), last, "a click on a card selects it");
    let after = draw(&mut a, &cx, 800, 480);
    for i in shown {
        assert_eq!(at(&after, i), at(&g, i), "card {i} stayed where it was after the click");
    }
}

#[test]
fn the_overview_does_not_ask_for_attention_about_the_saver() {
    let cx = admin("ss-overview");
    let a = SettingsApp::new(&cx);
    let before = a.attention(&cx).len();
    let c = SaverConfig { first: First::Nothing, ..SaverConfig::default() };
    c.save(cx.services.dir(), "root").unwrap();
    assert_eq!(a.attention(&cx).len(), before, "turning the saver off is a choice, not a problem");
}

/// Pictures of the page: `IYAKKI_PICTURES=/some/folder cargo test -p iyakki-shell --lib screensaver_tests::pictures -- --ignored`.
#[test]
#[ignore]
fn pictures() {
    let Ok(dir) = std::env::var("IYAKKI_PICTURES") else { return };
    let dir = std::path::PathBuf::from(dir);
    let _ = std::fs::create_dir_all(&dir);
    let cx = admin("ss-pictures");
    let mut a = page(&cx);
    for (w, h) in [(1280u32, 720u32), (800, 480), (360, 740)] {
        let mut g = Gfx::new(w, h);
        let s = (w as f32 / 1280.0).min(h as f32 / 720.0).clamp(0.6, 3.0);
        g.gradient(crate::ui::BG_TOP, crate::ui::BG_BOTTOM);
        a.render(&mut g, R::new(0.0, 0.0, w as f32, h as f32), s, &cx, 0);
        g.save_png(&dir.join(format!("settings-screensaver-{w}x{h}.png"))).expect("png");
    }
}

#[test]
fn the_lock_cards_keep_the_automatic_lock_per_person_and_ask_the_shell_to_lock_now() {
    use iyakki_agent::lockscreen::{self, LockConfig};
    let cx = admin("ss-lock");
    let (dir, user) = (cx.services.dir().to_path_buf(), cx.session.user.clone());
    let mut a = page(&cx);
    assert_eq!(LockConfig::load(&dir, &user).auto_min, 0, "never by itself until chosen");
    click(&mut a, &cx, 7, 3); // 15 min
    assert_eq!(LockConfig::load(&dir, &user).auto_min, 15);
    assert_eq!(SettingsApp::ss_chips_cx(SsRow::AutoLock, &SaverConfig::default(), &cx).1, Some(3), "the saved choice is the marked one");
    click(&mut a, &cx, 7, 0); // never
    assert_eq!(LockConfig::load(&dir, &user).auto_min, 0);
    assert_eq!(mine(&cx), SaverConfig::default(), "the screen saver's own choices are untouched");
    assert!(!lockscreen::take_request(&dir, &user));
    click(&mut a, &cx, 8, 0); // lock now
    assert!(lockscreen::take_request(&dir, &user), "the shell of this person is asked to lock");
    // the keyboard works too: Down to the card, Enter for the next choice
    a.sel[a.tab] = 7;
    press(&mut a, &cx, Key::Enter);
    assert_eq!(LockConfig::load(&dir, &user).auto_min, 5);
}

#[test]
fn a_guest_has_no_password_so_the_lock_cards_do_nothing() {
    use iyakki_agent::lockscreen::{self, LockConfig};
    let cx = as_role(admin("ss-lock-guest"), "gus", Role::Guest);
    let mut a = page(&cx);
    let g = draw(&mut a, &cx, 1920, 1080);
    assert!(!g.hits.iter().any(|h| matches!(h.ev, Event::Sub(r, _) if r == 7 || r == 8)), "no live buttons on the lock cards");
    a.handle(&Event::Sub(7, Key::Char('d')), &cx);
    a.handle(&Event::Sub(8, Key::Char('a')), &cx);
    assert_eq!(LockConfig::load(cx.services.dir(), "gus").auto_min, 0);
    assert!(!lockscreen::take_request(cx.services.dir(), "gus"));
}
