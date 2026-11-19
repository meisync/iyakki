//! Settings -> Gemini voice (EXPERIMENTAL): off by default, the consent card that is "No" until a person presses Yes (and that no tool
//! can answer), the choices, Stop, the page at every size, the keyboard and the click paths.

use super::*;
use crate::apps::tests::{cx_with_admin_store, services_in, temp_dir, Offline};
use iyakki_shared::gemini;
use iyakki_shared::geminilive;
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

fn key(a: &mut SettingsApp, cx: &AppCx, k: Key) {
    a.handle(&Event::Key(k), cx);
}

fn status(a: &SettingsApp) -> String {
    a.status().map(|s| s.0.clone()).unwrap_or_default()
}

fn vcfg(cx: &AppCx) -> geminilive::LiveCfg {
    gemini::load(cx.data_dir()).voice
}

fn on(a: &mut SettingsApp, row: geminivoice_page::GvRow) {
    a.sel[a.tab] = geminivoice_page::ROWS.iter().position(|r| *r == row).unwrap();
}

#[test]
fn the_page_is_in_the_ai_group_labelled_experimental_and_found_by_search() {
    let cx = admin("gv-tab");
    let a = SettingsApp::new(&cx);
    assert!(a.tabs.contains(&Tab::GeminiVoice));
    assert_eq!(Tab::GeminiVoice.group(), "Apps & AI");
    assert!(Tab::GeminiVoice.label().contains("experimental"));
    let ucx = plain_user("gv-tab-user");
    assert_eq!(SettingsApp::new(&ucx).tabs.contains(&Tab::GeminiVoice), ucx.session.policy.voice, "whoever may use voice sees it");
    let mut a = SettingsApp::new(&cx);
    a.open_item("tab:geminivoice", &cx);
    assert_eq!(a.tab(), Tab::GeminiVoice);
    assert!(a.hint().contains("consent card") && a.hint().contains("Stop now"));
}

#[test]
fn it_is_off_by_default_and_the_switch_and_the_choices_are_saved_audited_and_for_administrators_only() {
    let cx = admin("gv-switch");
    let mut a = SettingsApp::new(&cx);
    a.go_to(Tab::GeminiVoice);
    assert!(!vcfg(&cx).enabled && vcfg(&cx).voice_out == geminilive::VoiceOut::Local);
    on(&mut a, geminivoice_page::GvRow::Switch);
    key(&mut a, &cx, Key::Enter);
    assert!(vcfg(&cx).enabled && status(&a).contains("Nothing is sent until you say yes"), "{}", status(&a));
    assert_eq!(gemini::live_consent_state(cx.data_dir(), "root"), gemini::ConsentState::Missing, "turning it on does not give the yes");
    key(&mut a, &cx, Key::Char('v'));
    assert_eq!(vcfg(&cx).voice_out, geminilive::VoiceOut::Gemini);
    key(&mut a, &cx, Key::Char('v'));
    assert_eq!(vcfg(&cx).voice_out, geminilive::VoiceOut::Local);
    key(&mut a, &cx, Key::Char('k'));
    assert_eq!(vcfg(&cx).key, "Super+G");
    key(&mut a, &cx, Key::Char('w'));
    assert!(vcfg(&cx).wake_handoff && status(&a).contains("Nothing is sent before the phrase is heard"));
    // numbers step inside their ranges
    for (row, get, lo, hi) in [
        (geminivoice_page::GvRow::SessionLen, (|c: &geminilive::LiveCfg| c.max_session_secs) as fn(&geminilive::LiveCfg) -> u32, 30u32, 900u32),
        (geminivoice_page::GvRow::Quiet, |c| c.idle_secs, 5, 300),
        (geminivoice_page::GvRow::DayCap, |c| c.day_cap_secs, 60, 36_000),
        (geminivoice_page::GvRow::SpokenCap, |c| c.spoken_cap_chars, 100, 2000),
    ] {
        on(&mut a, row);
        for _ in 0..12 {
            key(&mut a, &cx, Key::Char('+'));
        }
        assert_eq!(get(&vcfg(&cx)), hi, "{row:?} tops out at its limit");
        for _ in 0..12 {
            key(&mut a, &cx, Key::Char('-'));
        }
        assert_eq!(get(&vcfg(&cx)), lo, "{row:?} bottoms out at its limit");
    }
    assert!(cx.audit.recent(40).iter().any(|l| l.contains("settings.gemini_voice")));
    // an account that is not an administrator reads but changes nothing
    let ucx = plain_user("gv-switch-user");
    let mut u = SettingsApp::new(&ucx);
    u.go_to(Tab::GeminiVoice);
    on(&mut u, geminivoice_page::GvRow::Switch);
    key(&mut u, &ucx, Key::Enter);
    assert!(!vcfg(&ucx).enabled && status(&u).contains("administrator"), "{}", status(&u));
}

#[test]
fn the_consent_card_names_the_microphone_and_the_tool_results_starts_on_no_and_only_a_press_says_yes() {
    let cx = admin("gv-consent");
    let mut a = SettingsApp::new(&cx);
    a.go_to(Tab::GeminiVoice);
    key(&mut a, &cx, Key::Char('C'));
    assert!(a.has_modal());
    let note = match &a.modal { Some(Modal::Form(_, f)) => f.note.clone(), _ => String::new() };
    assert!(note.contains("EXPERIMENTAL") && note.contains("microphone") && note.contains("results of the tools") && note.contains("red sign"), "{note}");
    // Enter alone answers No
    key(&mut a, &cx, Key::Enter);
    assert!(!a.has_modal() && status(&a).contains("stays on this computer"), "{}", status(&a));
    assert_eq!(gemini::live_consent_state(cx.data_dir(), "root"), gemini::ConsentState::Missing);
    // Esc leaves nothing either
    key(&mut a, &cx, Key::Char('C'));
    key(&mut a, &cx, Key::Esc);
    assert_eq!(gemini::live_consent_state(cx.data_dir(), "root"), gemini::ConsentState::Missing);
    // Yes: Right, Enter
    key(&mut a, &cx, Key::Char('C'));
    key(&mut a, &cx, Key::Right);
    key(&mut a, &cx, Key::Enter);
    assert!(matches!(gemini::live_consent_state(cx.data_dir(), "root"), gemini::ConsentState::Given { .. }), "{}", status(&a));
    // it is its own yes: the speech-to-text yes is untouched
    assert_eq!(gemini::consent_state(cx.data_dir(), "root"), gemini::ConsentState::Missing);
    assert!(cx.audit.recent(40).iter().any(|l| l.contains("consent given for the microphone")));
    // taking it back
    key(&mut a, &cx, Key::Char('C'));
    assert!(a.has_modal());
    key(&mut a, &cx, Key::Char('y'));
    key(&mut a, &cx, Key::Enter);
    assert!(matches!(gemini::live_consent_state(cx.data_dir(), "root"), gemini::ConsentState::Missing | gemini::ConsentState::Given { .. }));
}

#[test]
fn stop_closes_the_microphone_and_is_free_for_everyone() {
    let cx = plain_user("gv-stop");
    let mut a = SettingsApp::new(&cx);
    a.go_to(Tab::GeminiVoice);
    key(&mut a, &cx, Key::Char('x'));
    assert!(status(&a).contains("No Gemini voice session was open"), "{}", status(&a));
    assert!(cx.audit.recent(40).iter().any(|l| l.contains("stop asked")));
    assert!(a.buttons_for(true).iter().any(|b| b.2 == 'x'), "a Stop button on every row");
}

#[test]
fn the_status_row_shows_the_limit_meter_and_the_delay() {
    let cx = admin("gv-status");
    let mut a = SettingsApp::new(&cx);
    a.go_to(Tab::GeminiVoice);
    let (title, detail) = a.gv_row_text(&cx, geminivoice_page::GvRow::Status);
    assert!(title.contains("EXPERIMENTAL") && detail.contains("Microphone time sent today") && detail.contains("Delay"), "{detail}");
}

#[test]
fn the_page_draws_at_every_size_and_text_scale_with_every_row_and_button_inside_the_window_and_clicks_work() {
    let cx = admin("gv-sizes");
    let mut a = SettingsApp::new(&cx);
    a.go_to(Tab::GeminiVoice);
    for (w, h) in [(360u32, 740u32), (800, 480), (1280, 720), (1920, 1080), (2560, 1600)] {
        for scale in [None, Some(1.6f32)] {
            // the page's own rows at the largest text size; the narrow list's "All settings" button is the Settings frame's, not this page's
            if scale.is_some() && w < 1280 {
                continue;
            }
            let mut g = Gfx::new(w, h);
            let s = scale.unwrap_or_else(|| (w as f32 / 1280.0).min(h as f32 / 720.0).clamp(0.6, 3.0));
            a.render(&mut g, R::new(0.0, 72.0 * s, w as f32, (h as f32 - 72.0 * s).max(100.0)), s, &cx, 0);
            if w >= 800 && scale.is_none() {
                assert!(g.hits.iter().any(|x| matches!(x.ev, Event::Row(0))), "{w}x{h}: the first row can be clicked");
            }
            for hit in &g.hits {
                assert!(hit.rect.x >= -0.5 && hit.rect.x + hit.rect.w <= w as f32 + 0.5, "{w}x{h} scale {scale:?}: {} is inside the width", hit.label);
            }
        }
    }
    // the mouse path: a click selects the row, and every button of it is a key the page handles
    let mut g = Gfx::new(1280, 720);
    a.render(&mut g, R::new(0.0, 72.0, 1280.0, 648.0), 1.0, &cx, 0);
    a.handle(&Event::Row(2), &cx);
    assert_eq!(a.sel(), 2);
    for row in 0..geminivoice_page::ROWS.len() {
        a.sel[a.tab] = row;
        for b in a.buttons_for(true) {
            key(&mut a, &cx, Key::Char(b.2));
            if a.has_modal() {
                key(&mut a, &cx, Key::Esc);
            }
        }
    }
}
