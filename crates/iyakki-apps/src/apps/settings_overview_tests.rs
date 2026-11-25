//! Settings -> Overview and Last session log on a fake port: what needs attention comes from the port (model, wizards, update words, data in
//! memory, the unexpected end of the last session), "I have seen it" clears the note and audits it, the copy is for administrators and goes
//! to the home folder of the port, and both pages draw at several sizes. No sleeps, no real machine.

use super::system_port::{CrashPort, FakeSys};
use super::*;
use crate::apps::tests::cx_with_admin_store;
use iyakki_core_api::crashlog::Unclean;
use std::sync::atomic::Ordering;

fn app(tab: Tab) -> SettingsApp {
    let cx = cx_with_admin_store("overview-fake");
    let mut a = SettingsApp::new(&cx);
    a.go_to(tab);
    a
}

fn unclean(seen: bool) -> Unclean {
    Unclean { boot_id: "b1".into(), started: 1_792_771_000, last_line: "kernel panic".into(), uptime_s: 42, file: "kmsg-b1.log".into(), audited: false, seen, marker: true }
}

fn titles(a: &SettingsApp, p: &FakeSys) -> Vec<String> {
    a.attention(p).into_iter().map(|t| t.title).collect()
}

#[test]
fn nothing_needs_attention_when_the_port_reports_a_healthy_machine() {
    let p = FakeSys::new(true);
    let a = app(Tab::Overview);
    assert!(titles(&a, &p).iter().all(|t| !t.contains("model") && !t.contains("wizard") && !t.contains("update") && !t.contains("memory")), "{:?}", titles(&a, &p));
}

#[test]
fn what_the_port_reports_becomes_a_line_with_the_page_that_fixes_it() {
    let p = FakeSys::new(true);
    p.chat_chosen.store(false, Ordering::SeqCst);
    p.wizards.store(2, Ordering::SeqCst);
    *p.interrupted.lock().unwrap() = Some("it stopped at step 3".into());
    *p.badge.lock().unwrap() = Some("3 updates".into());
    p.in_memory.store(true, Ordering::SeqCst);
    p.crash.set(|s| s.unclean = Some(unclean(false)));
    let a = app(Tab::Overview);
    let t = titles(&a, &p);
    for want in ["No AI model is chosen", "2 setup wizards waiting", "A system update did not finish", "3 updates available", "Your data is kept in memory only", "The last session ended unexpectedly"] {
        assert!(t.iter().any(|x| x.starts_with(want)), "{want}: {t:?}");
    }
    let at = a.attention(&p);
    assert_eq!(at.iter().find(|x| x.title.contains("wizard")).map(|x| x.tab), Some(Tab::Updates));
    assert_eq!(at.iter().find(|x| x.title.contains("memory")).map(|x| x.tab), Some(Tab::Storage));
    assert!(at.iter().find(|x| x.title.contains("did not finish")).is_some_and(|x| x.why == "it stopped at step 3"));
}

#[test]
fn a_fallen_back_update_is_told_when_none_was_interrupted_and_one_wizard_is_singular() {
    let p = FakeSys::new(true);
    p.fell_back.store(true, Ordering::SeqCst);
    p.wizards.store(1, Ordering::SeqCst);
    let a = app(Tab::Overview);
    let t = titles(&a, &p);
    assert!(t.iter().any(|x| x == "1 setup wizard waiting"), "{t:?}");
    assert!(t.iter().any(|x| x.contains("did not start")), "{t:?}");
}

#[test]
fn enter_goes_to_the_page_of_the_chosen_line() {
    let p = FakeSys::new(true);
    p.wizards.store(1, Ordering::SeqCst);
    p.chat_chosen.store(false, Ordering::SeqCst);
    let mut a = app(Tab::Overview);
    let first = a.attention(&p)[0].tab;
    a.handle_overview(&Key::Enter, &p);
    assert_eq!(a.tab(), first);
}

#[test]
fn the_overview_draws_with_and_without_lines_and_offers_diagnostics_only_to_who_may() {
    for (lines, doctor) in [(false, true), (true, true), (true, false)] {
        let p = FakeSys::new(true);
        p.doctor.store(doctor, Ordering::SeqCst);
        p.chat_chosen.store(!lines, Ordering::SeqCst);
        let mut a = app(Tab::Overview);
        for (w, h) in [(360u32, 740u32), (800, 480), (1280, 720), (2560, 1600)] {
            let mut g = Gfx::new(w, h);
            g.clear(rgb(0, 0, 0));
            a.render_overview(&mut g, R::new(0.0, 72.0, w as f32, h as f32 - 72.0), 1.0, &p);
            if w >= 1280 {
                assert_eq!(g.hits.iter().any(|x| x.ev == Event::Key(Key::Char('s'))), doctor, "{w}x{h} lines={lines}");
            }
        }
    }
}

// ---- the last session log ----

fn log(p: &FakeSys) {
    p.crash.set(|s| {
        s.unclean = Some(unclean(false));
        s.latest = Some("kmsg-b1.log".into());
        s.files.insert("kmsg-b1.log".into(), (0..300).map(|i| format!("line {i}")).collect());
    });
}

#[test]
fn the_viewer_shows_the_last_200_lines_of_the_kept_log_and_scrolls() {
    let p = FakeSys::new(true);
    log(&p);
    let mut a = app(Tab::SessionLog);
    a.handle_crash(&Key::Char('r'), &p);
    assert_eq!(a.crash.lines.len(), 200);
    assert_eq!(a.crash.lines.last().map(String::as_str), Some("line 299"));
    a.handle_crash(&Key::Home, &p);
    assert_eq!(a.crash.scroll, 0);
    a.handle_crash(&Key::PageDown, &p);
    assert_eq!(a.crash.scroll, 10);
    // another boot has no kept file here
    a.handle_crash(&Key::Char('p'), &p);
    assert!(a.crash.lines.is_empty());
}

#[test]
fn i_have_seen_it_clears_the_note_through_the_port_and_audits_it_once() {
    let p = FakeSys::new(true);
    log(&p);
    let mut a = app(Tab::SessionLog);
    a.handle_crash(&Key::Char('y'), &p);
    assert!(p.crash.calls().contains(&"mark_seen".to_string()));
    assert_eq!(p.audited().len(), 1);
    assert_eq!(p.audited()[0].0, "system.unclean-shutdown.seen");
    assert!(a.crash_note(&p).is_none());
    a.handle_crash(&Key::Char('y'), &p);
    assert_eq!(p.audited().len(), 1, "no second audit line when there is nothing to clear");
    assert!(a.status().is_some_and(|s| s.0.contains("no note")));
}

#[test]
fn the_copy_is_for_administrators_goes_to_the_home_of_the_port_and_is_audited() {
    let p = FakeSys::new(false);
    log(&p);
    let mut a = app(Tab::SessionLog);
    a.handle_crash(&Key::Char('c'), &p);
    assert!(a.status().is_some_and(|s| s.0.contains("Only an administrator")));
    assert!(p.audited().is_empty());
    p.admin.store(true, Ordering::SeqCst);
    a.handle_crash(&Key::Char('c'), &p);
    assert!(a.status().is_some_and(|s| s.0.contains("fake-home") && s.0.contains("kmsg-b1.log")), "{:?}", a.status());
    assert_eq!(p.audited()[0].0, "system.unclean-shutdown.copy");
    assert_eq!(p.home_for_writes(), std::path::PathBuf::from("fake-home"));
}

#[test]
fn a_copy_with_no_kept_log_says_so_and_the_page_draws_at_several_sizes() {
    let p = FakeSys::new(true);
    let mut a = app(Tab::SessionLog);
    a.handle_crash(&Key::Char('c'), &p);
    assert!(a.status().is_some_and(|s| s.0.contains("no kept log")));
    log(&p);
    for (w, h) in [(360u32, 740u32), (800, 480), (1280, 720), (2560, 1600)] {
        let mut g = Gfx::new(w, h);
        g.clear(rgb(0, 0, 0));
        a.render_crash(&mut g, R::new(0.0, 72.0, w as f32, h as f32 - 72.0), 1.0, &p);
    }
}
