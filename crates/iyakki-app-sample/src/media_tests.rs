//! The Media pane of SAMPLE (F-SDK-146, F-SDK-149): an embedded `MediaView` (sound and video) over the SDK's in-memory media port. The core-backed port and the real
//! player are tested where they live (`iyakki-apps` `sdk_media`, `iyakki-agent` `embedmedia`).

use iyakki_ui_sdk::desktop::{Arrangement, MemDock, MemWallpaper};
use iyakki_ui_sdk::event::{Phase, UiEvent, UiKey};
use iyakki_ui_sdk::host::Host;
use iyakki_ui_sdk::list::{DisplayList, Op};
use iyakki_ui_sdk::look::Look;
use iyakki_ui_sdk::media::{MemMedia, PlayState};

use super::*;

fn media() -> MemMedia {
    let m = MemMedia::new();
    m.put_file("music/one.mp3", 60_000, false);
    m.put_file("two.ogg", 30_000, false);
    m
}

fn host(w: u32, h: u32, m: &MemMedia) -> Host<SampleApp> {
    Host::new(SampleApp::new().with_media(Box::new(m.clone())), Look::default(), Arrangement::single(w, h), Box::new(MemDock::knowing(&[ID])), Box::new(MemWallpaper::default()))
}

fn centre(l: &DisplayList, name: &str) -> (f32, f32) {
    l.ops.iter().find_map(|o| if let Op::Hit(r, hit) = o { (hit.name == name).then_some((r.x + r.w / 2.0, r.y + r.h / 2.0)) } else { None }).unwrap_or_else(|| panic!("no control named {name}: {:?}", l.a11y().iter().map(|n| n.name.clone()).collect::<Vec<_>>()))
}

fn tap(h: &mut Host<SampleApp>, name: &str) {
    let l = h.record();
    let (x, y) = centre(&l, name);
    h.mouse(x, y, Phase::Down, 0);
    h.mouse(x, y, Phase::Up, 60);
}

fn texts(l: &DisplayList) -> Vec<String> {
    l.ops.iter().filter_map(|o| if let Op::Text { text, .. } = o { Some(text.clone()) } else { None }).collect()
}

#[test]
fn an_app_without_a_player_has_no_media_tab() {
    let mut h = Host::new(SampleApp::new(), Look::default(), Arrangement::single(800, 480), Box::new(MemDock::knowing(&[ID])), Box::new(MemWallpaper::default()));
    assert!(!h.record().a11y().iter().any(|n| n.name == "Media"));
    assert!(!h.app.has_audio());
}

#[test]
fn the_media_tab_lists_the_files_and_a_tap_plays_one() {
    let m = media();
    let mut h = host(800, 480, &m);
    h.record();
    tap(&mut h, "Media");
    assert!(h.app.audio_open());
    h.event(&UiEvent::Tick);
    let l = h.record();
    assert!(texts(&l).iter().any(|t| t == "two.ogg"), "{:?}", texts(&l));
    tap(&mut h, "two.ogg");
    assert_eq!(h.app.audio().unwrap().state(), PlayState::Playing);
    assert_eq!(m.calls()[0], "open home two.ogg");
    m.advance(10_000);
    h.event(&UiEvent::Tick);
    assert!(texts(&h.record()).contains(&"0:10".to_string()));
}

#[test]
fn the_sound_goes_on_in_the_background_and_the_keys_belong_to_the_pane_that_is_shown() {
    let m = media();
    let mut h = host(800, 480, &m);
    h.record();
    h.event(&UiEvent::Key(UiKey::Char('u')));
    h.event(&UiEvent::Tick);
    h.event(&UiEvent::Key(UiKey::Enter));
    assert_eq!(h.app.audio().unwrap().state(), PlayState::Playing);
    // Esc stops the sound first; a second Esc goes back to the notes
    h.event(&UiEvent::Key(UiKey::Esc));
    assert_eq!(h.app.audio().unwrap().state(), PlayState::Idle);
    assert!(h.app.audio_open());
    h.event(&UiEvent::Key(UiKey::Esc));
    assert!(!h.app.audio_open());
    // start again, leave the pane: the clock still moves and the pane shows it on return
    h.event(&UiEvent::Key(UiKey::Char('u')));
    h.event(&UiEvent::Key(UiKey::Enter));
    h.event(&UiEvent::Key(UiKey::Esc));
    h.event(&UiEvent::Key(UiKey::Esc));
    h.event(&UiEvent::Key(UiKey::Char('u')));
    h.event(&UiEvent::Key(UiKey::Enter));
    m.advance(7_000);
    tap(&mut h, "Notes");
    m.advance(3_000);
    h.event(&UiEvent::Tick);
    assert_eq!(h.app.audio().unwrap().position_ms(), 10_000, "ticks are taken while another pane shows");
}

#[test]
fn the_pane_fits_a_phone_a_window_and_a_desktop_at_three_times_the_text() {
    for (w, hh) in [(360u32, 740u32), (800, 480), (1280, 720)] {
        let m = media();
        let mut h = host(w, hh, &m);
        h.record();
        h.event(&UiEvent::Key(UiKey::Char('u')));
        h.event(&UiEvent::Tick);
        h.event(&UiEvent::Key(UiKey::Enter));
        for scale in [1.0f32, 3.0] {
            h.text_scale = scale;
            let l = h.record();
            let mut seen = 0;
            for o in &l.ops {
                if let Op::Hit(r, hit) = o {
                    seen += 1;
                    assert!(!hit.name.is_empty());
                    assert!(r.x >= -0.5 && r.y >= -0.5 && r.right() <= w as f32 + 0.5 && r.bottom() <= hh as f32 + 0.5, "{} outside at {w}x{hh} x{scale}: {r:?}", hit.name);
                }
            }
            assert!(seen >= 8, "{seen} controls at {w}x{hh} x{scale}");
        }
    }
}

#[test]
fn the_media_pane_shows_the_picture_of_a_video_and_full_screen_fills_the_pane() {
    let m = media();
    m.put_video("clips/v.avi", 20_000, 10, 64, 48);
    let mut h = host(800, 480, &m);
    h.record();
    h.event(&UiEvent::Key(UiKey::Char('u')));
    h.event(&UiEvent::Tick);
    tap(&mut h, "clips/v.avi");
    h.event(&UiEvent::Tick);
    let images = |l: &DisplayList| l.ops.iter().filter_map(|o| if let Op::Image(r, i) = o { Some((*r, i.seq)) } else { None }).collect::<Vec<_>>();
    let l = h.record();
    assert_eq!(images(&l).len(), 1, "{:?}", l.a11y().iter().map(|n| n.name.clone()).collect::<Vec<_>>());
    m.advance(2_000);
    h.event(&UiEvent::Tick);
    assert_eq!(images(&h.record())[0].1, 21);
    tap(&mut h, "Full screen");
    let l = h.record();
    assert!(l.a11y().iter().any(|n| n.name == "Exit full screen"));
    assert!(images(&l)[0].0.h > 300.0);
    // Esc leaves full screen, the next Esc stops, the next goes back to the notes
    h.event(&UiEvent::Key(UiKey::Esc));
    assert!(!h.record().a11y().iter().any(|n| n.name == "Exit full screen"));
    h.event(&UiEvent::Key(UiKey::Esc));
    assert_eq!(h.app.audio().unwrap().state(), PlayState::Idle);
    h.event(&UiEvent::Key(UiKey::Esc));
    assert!(!h.app.audio_open());
}
