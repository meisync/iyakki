//! The lost-drive banner in the shell: drawn on every screen and in panes at six sizes without crossing the screen's edge or itself,
//! its buttons, "cannot be dismissed but can be minimised", the reconnected card, the wording of toasts, and the audit flush.
//! The state is made up per test thread (`iyakki_storage::loss::set_thread_override`): no drive, mount or file of the machine is touched.

use super::tests::*;
use super::*;
use crate::apps::tests::MockHost;
use crate::lossbar as lb;
use iyakki_storage::loss::{self, Snapshot, State};

const SIZES: [(u32, u32); 6] = [(360, 740), (800, 480), (1280, 720), (1920, 1080), (2560, 1600), (2880, 1800)];

fn state(s: State) {
    loss::set_thread_override(Some(Snapshot { state: s, since: 3088, lost_at: Some(3088), ..Default::default() }));
}

fn healthy() {
    loss::set_thread_override(None);
}

struct Reset;
impl Drop for Reset {
    fn drop(&mut self) {
        loss::set_thread_override(None);
    }
}

fn signed(tag: &str) -> (Shell, Arc<MockHost>) {
    let (mut sh, host) = shell_with_users(tag);
    login(&mut sh, "root", "rootpass12");
    (sh, host)
}

fn banner_hits(sh: &Shell) -> Vec<(usize, R)> {
    sh.hits().iter().filter_map(|h| match h.ev {
        Event::Tab(n) if lb::is_click(n) => Some((n, h.rect)),
        _ => None,
    }).collect()
}

fn inside(a: R, outer: R) -> bool {
    a.x >= outer.x - 0.5 && a.y >= outer.y - 0.5 && a.x + a.w <= outer.x + outer.w + 0.5 && a.y + a.h <= outer.y + outer.h + 0.5
}

fn disjoint(a: R, b: R) -> bool {
    a.x + a.w <= b.x + 0.5 || b.x + b.w <= a.x + 0.5 || a.y + a.h <= b.y + 0.5 || b.y + b.h <= a.y + 0.5
}

fn check_layout(sh: &Shell, w: u32, h: u32, what: &str) {
    let card = sh.loss.rect.unwrap_or_else(|| panic!("{what}: no banner"));
    assert!(inside(card, R::new(0.0, 0.0, w as f32, h as f32)), "{what}: card {card:?} inside {w}x{h}");
    let hits = banner_hits(sh);
    assert!(!hits.is_empty(), "{what}: buttons");
    for (n, r) in &hits {
        assert!(inside(*r, card), "{what}: button {n} {r:?} inside card {card:?}");
    }
    for i in 0..hits.len() {
        for j in i + 1..hits.len() {
            assert!(disjoint(hits[i].1, hits[j].1), "{what}: buttons {} and {} overlap", hits[i].0, hits[j].0);
        }
    }
}

#[test]
fn nothing_is_drawn_while_the_drive_is_healthy() {
    let _r = Reset;
    let (mut sh, _) = signed("loss-ok");
    let mut g = Gfx::new(1280, 720);
    sh.render(&mut g);
    assert!(sh.loss.rect.is_none() && banner_hits(&sh).is_empty());
}

#[test]
fn the_banner_is_on_the_sign_in_screen_the_launcher_and_an_app_at_six_sizes() {
    let _r = Reset;
    for st in [State::Lost, State::Errors, State::ReadOnly, State::Reattachable, State::Reconnecting, State::Failed("the new device could not be mounted (EUCLEAN: journal replay failed)".into())] {
        let (mut login_sh, _) = shell_with_users("loss-login");
        let (mut sh, _) = signed("loss-in");
        state(st.clone());
        // the three states with different buttons at every size, the others at the smallest and a large one
        let every = matches!(st, State::Lost | State::Reattachable | State::Failed(_));
        for (w, h) in SIZES.into_iter().filter(|s| every || [(360, 740), (1920, 1080)].contains(s)) {
            // before anyone signed in
            let mut g = Gfx::new(w, h);
            login_sh.render(&mut g);
            check_layout(&login_sh, w, h, &format!("{:?} sign-in {w}x{h}", st.word()));
            // signed in: launcher and Settings
            for app in [false, true] {
                let mut g = Gfx::new(w, h);
                sh.screen = Screen::Launcher;
                if app {
                    assert!(sh.open_for_demo("settings"));
                }
                sh.render(&mut g);
                check_layout(&sh, w, h, &format!("{} app={app} {w}x{h}", st.word()));
            }
        }
    }
}

#[test]
fn the_banner_stays_whole_in_panes() {
    let _r = Reset;
    for (w, h) in [(1280u32, 720u32), (1920, 1080), (2560, 1600), (2880, 1800)] {
        let (mut sh, _) = signed("loss-panes");
        state(State::Lost);
        let mut g = Gfx::new(w, h);
        sh.render(&mut g);
        sh.handle(&Event::Layout(crate::panes::LayoutKey::Apply(2)));
        sh.render(&mut g);
        check_layout(&sh, w, h, &format!("panes {w}x{h}"));
        let rects = sh.panes_rects_for_test();
        // the banner is drawn over the panes, never inside a pane's own click regions: its card blocks them
        let card = sh.loss.rect.unwrap();
        let top = sh.hits().iter().rev().find(|hit| hit.rect.contains((card.x + 4.0) as i32, (card.y + 4.0) as i32)).map(|hit| hit.label.clone());
        assert_eq!(top.as_deref(), Some("drive banner"), "the card takes the clicks under it ({} panes)", rects.len());
    }
}

#[test]
fn the_text_is_the_plain_sentence_and_wraps_inside_the_card() {
    let _r = Reset;
    let (mut sh, _) = signed("loss-text");
    state(State::Lost);
    let mut g = Gfx::new(360, 740);
    sh.render(&mut g);
    let b = loss::banner(&loss::snapshot()).unwrap();
    assert!(b.body.starts_with("The IYAKKI drive was disconnected for a moment, so saving is not possible right now. Check the USB-C cable."));
    let labels: Vec<String> = sh.hits().iter().map(|h| h.label.clone()).collect();
    for want in ["Reconnect now", "Restart", "Minimise"] {
        assert!(labels.iter().any(|l| l == want), "{want} in {labels:?}");
    }
}

#[test]
fn it_cannot_be_dismissed_but_can_be_minimised_and_opened_again() {
    let _r = Reset;
    let (mut sh, _) = signed("loss-min");
    state(State::Lost);
    let mut g = Gfx::new(1280, 720);
    sh.render(&mut g);
    let full = sh.loss.rect.unwrap();
    // Esc and clicks elsewhere do not close it, and it has no close button
    sh.handle(&Event::Key(Key::Esc));
    sh.render(&mut g);
    assert!(sh.loss.rect.is_some() && !sh.loss.minimised);
    assert!(!sh.hits().iter().any(|h| h.label.to_lowercase().contains("close") && lb::is_click(match h.ev { Event::Tab(n) => n, _ => 0 })));
    // minimise: a slim strip, still on screen, still not gone
    sh.dispatch(&Event::Tab(lb::MINIMISE));
    sh.render(&mut g);
    let strip = sh.loss.rect.unwrap();
    assert!(sh.loss.minimised && strip.h < full.h && inside(strip, R::new(0.0, 0.0, 1280.0, 720.0)));
    assert_eq!(banner_hits(&sh).len(), 1, "the strip is one button");
    // the strip opens it again
    sh.dispatch(&Event::Tab(lb::EXPAND));
    sh.render(&mut g);
    assert!(!sh.loss.minimised && sh.loss.rect.unwrap().h > strip.h);
    // a NEW loss opens it in full even when it was minimised before
    sh.dispatch(&Event::Tab(lb::MINIMISE));
    healthy();
    sh.loss_tick();
    state(State::Lost);
    sh.loss_tick();
    assert!(!sh.loss.minimised);
}

#[test]
fn the_buttons_work() {
    let _r = Reset;
    let (mut sh, host) = signed("loss-btn");
    state(State::Lost);
    let mut g = Gfx::new(1280, 720);
    sh.render(&mut g);
    // Reconnect now: asks the watcher
    while loss::take_reattach_request() {}
    let r = banner_hits(&sh).into_iter().find(|(n, _)| *n == lb::RECONNECT).unwrap().1;
    sh.handle(&Event::MouseMove((r.x + r.w / 2.0) as i32, (r.y + r.h / 2.0) as i32));
    sh.handle(&Event::MouseDown((r.x + r.w / 2.0) as i32, (r.y + r.h / 2.0) as i32));
    sh.handle(&Event::MouseUp((r.x + r.w / 2.0) as i32, (r.y + r.h / 2.0) as i32));
    assert!(loss::take_reattach_request(), "the click asked for a reattach");
    // Restart: the host restarts (never powers off)
    sh.dispatch(&Event::Tab(lb::RESTART));
    assert_eq!(host.powered_off.lock().unwrap().as_deref(), Some("reboot"));
    // Check the drive (shown after a failed reattach)
    state(State::Failed("boom".into()));
    sh.render(&mut g);
    assert!(banner_hits(&sh).iter().any(|(n, _)| *n == lb::CHECK));
    while loss::take_check_request() {}
    sh.dispatch(&Event::Tab(lb::CHECK));
    assert!(loss::take_check_request());
}

#[test]
fn coming_back_shows_a_reconnected_card_that_goes_away() {
    let _r = Reset;
    let (mut sh, _) = signed("loss-back");
    state(State::Lost);
    sh.loss_tick();
    loss::set_thread_override(Some(Snapshot { state: State::Healthy, reattached: 1, ..Default::default() }));
    sh.loss_tick();
    let mut g = Gfx::new(1280, 720);
    sh.render(&mut g);
    assert!(sh.loss.card_visible() && sh.loss.card_text == "Drive reconnected, everything is saved again.");
    assert!(banner_hits(&sh).iter().any(|(n, _)| *n == lb::CARD_OK));
    check_layout(&sh, 1280, 720, "card");
    sh.dispatch(&Event::Tab(lb::CARD_OK));
    sh.render(&mut g);
    assert!(sh.loss.rect.is_none(), "dismissed");
    // by itself after a while
    state(State::Lost);
    sh.loss_tick();
    healthy();
    sh.loss_tick();
    assert!(sh.loss.card_visible());
    sh.tick += lb::CARD_TICKS + 1;
    sh.loss_tick();
    assert!(!sh.loss.card_visible());
    // a hiccup that healed by itself is said differently
    state(State::Errors);
    sh.loss_tick();
    healthy();
    sh.loss_tick();
    assert_eq!(sh.loss.card_text, "The IYAKKI drive works again.");
}

#[test]
fn a_toast_about_an_io_error_names_the_drive_only_while_it_is_not_healthy() {
    let _r = Reset;
    let (mut sh, _) = signed("loss-toast");
    sh.say("Could not save: Input/output error (os error 5)");
    assert!(sh.toast_text().unwrap().contains("Input/output error"));
    state(State::Lost);
    sh.say("Could not save: Input/output error (os error 5)");
    assert!(sh.toast_text().unwrap().contains("drive is not available"));
    sh.say("The model returned an empty answer.");
    assert_eq!(sh.toast_text(), Some("The model returned an empty answer."));
}

#[test]
fn what_happened_while_the_drive_was_away_is_written_to_the_audit_log_after_it_is_back() {
    let _r = Reset;
    let (mut sh, _) = signed("loss-audit");
    state(State::Lost);
    sh.loss_tick();
    // no flush while lost: the drive-lost line is still in the ring (published by the watcher in production)
    let w = {
        let mut w = loss::Watcher::new();
        w.ring.push(3088, "storage: drive lost at uptime 3088 s (the device is gone)");
        w.ring.push(3091, "storage: drive reconnected as sdb3 at uptime 3091 s");
        w
    };
    loss::set_thread_override(None);
    loss::publish(&w, None, None);
    sh.loss_tick();
    let recent = sh.env.audit.recent(10).join("\n");
    assert!(recent.contains("storage.event") && recent.contains("drive lost at uptime 3088 s") && recent.contains("reconnected as sdb3"), "{recent}");
    let n = sh.env.audit.recent(100).len();
    sh.loss_tick();
    assert_eq!(sh.env.audit.recent(100).len(), n, "each event once");
    loss::publish(&loss::Watcher::new(), None, None);
}

#[test]
fn the_banner_files_are_crash_proof() {
    for (name, src) in [("lossbar.rs", include_str!("lossbar.rs")), ("shell_loss.rs", include_str!("shell_loss.rs"))] {
        let code = src.split("#[cfg(test)]").next().unwrap_or(src);
        assert!(!code.contains(".clamp("), "{name}: use clamp_safe for computed bounds");
        assert!(!code.contains(".unwrap()"), "{name}: no unwrap on geometry");
        assert!(!code.contains("panic!("), "{name}");
    }
    assert!(include_str!("lossbar.rs").contains("clamp_safe"));
}

#[test]
fn random_events_while_the_drive_is_lost_never_crash() {
    let _r = Reset;
    let (mut sh, _) = signed("loss-monkey");
    let mut fails = Vec::new();
    for (si, size) in [(360u32, 740u32), (800, 480)].into_iter().enumerate() {
        for (i, st) in [State::Lost, State::Failed("x".into())].into_iter().enumerate() {
            state(st);
            for (ai, app) in [None, Some(iyakki_sec::policy::App::Settings)].into_iter().enumerate() {
                if let Err(e) = crate::monkey_tests::run_case(&mut sh, app, size, 9700 + 100 * si as u64 + 10 * i as u64 + ai as u64, 30, 5) {
                    fails.push(e);
                }
            }
        }
    }
    assert!(fails.is_empty(), "{} failures:\n{}", fails.len(), fails.join("\n---\n"));
}
