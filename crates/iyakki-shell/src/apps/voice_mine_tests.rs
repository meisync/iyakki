//! VOICE, ASSISTANT (My IYAKKI) with a fake microphone: off by default, nothing opens the microphone before the opt-in and a tap,
//! the indicator follows the open microphone, mute closes it, and the tab draws at four sizes. (The state machine itself is tested in
//! `iyakki_agent::voiceassist_tests` with a scripted model; here only the tile around it.)

use super::*;
use crate::apps::voice_mine::*;

fn mine_app(b: &Bench) -> VoiceApp {
    let mut a = b.app();
    key(&mut a, &b.cx, Key::F(5));
    a
}

fn wait_until(a: &mut VoiceApp, cx: &AppCx, what: impl Fn(&VoiceApp) -> bool) -> bool {
    for _ in 0..300 {
        tick(a, cx);
        if what(a) {
            return true;
        }
    }
    false
}

#[test]
fn off_by_default_and_a_tap_opens_nothing() {
    let b = bench("mine-off");
    let mut a = mine_app(&b);
    assert_eq!(a.area(), Area::Mine);
    assert!(!b.cx.services.voice_settings().my_on, "opt-in: off by default");
    click(&mut a, &b.cx, T_MINE_TALK);
    key(&mut a, &b.cx, Key::Char(' '));
    for _ in 0..10 {
        tick(&mut a, &b.cx);
    }
    assert!(b.recs.started.lock().unwrap().is_empty(), "no microphone before the person turned it on");
    let g = draw(&mut a, &b.cx, 900, 600);
    drop(g);
}

#[test]
fn on_then_a_tap_opens_the_microphone_and_stop_and_mute_close_it() {
    let b = bench("mine-on");
    let mut a = mine_app(&b);
    click(&mut a, &b.cx, T_MINE_ON);
    assert!(b.cx.services.voice_settings().my_on);
    assert!(b.recs.started.lock().unwrap().is_empty(), "turning it on opens nothing");
    click(&mut a, &b.cx, T_MINE_TALK);
    assert!(wait_until(&mut a, &b.cx, |a| a.mine.st.as_ref().is_some_and(|s| s.mic_open)), "a tap opens the microphone");
    assert_eq!(b.recs.started.lock().unwrap().len(), 1);
    // the red indicator and the label are drawn from the real state
    let _ = draw(&mut a, &b.cx, 900, 600);
    click(&mut a, &b.cx, T_MINE_STOP);
    assert!(wait_until(&mut a, &b.cx, |a| a.mine.st.as_ref().is_some_and(|s| !s.mic_open)), "stop closes it");
    // mute is a setting that survives a restart, and a muted assistant never opens the microphone
    click(&mut a, &b.cx, T_MINE_MUTE);
    assert!(b.cx.services.voice_settings().my_muted);
    let before = b.recs.started.lock().unwrap().len();
    assert!(wait_until(&mut a, &b.cx, |a| a.mine.st.as_ref().is_some_and(|s| s.muted)));
    click(&mut a, &b.cx, T_MINE_TALK);
    for _ in 0..20 {
        tick(&mut a, &b.cx);
    }
    assert_eq!(b.recs.started.lock().unwrap().len(), before, "muted: no new microphone");
    // turning it off drops the worker
    click(&mut a, &b.cx, T_MINE_OFF);
    assert!(!b.cx.services.voice_settings().my_on);
    assert!(a.mine.st.is_none());
}

#[test]
fn the_tab_draws_at_four_sizes() {
    let b = bench("mine-draw");
    let mut a = mine_app(&b);
    for (w, h) in [(420, 700), (900, 600), (1280, 800), (1920, 1080)] {
        let _ = draw(&mut a, &b.cx, w, h);
    }
    click(&mut a, &b.cx, T_MINE_ON);
    for (w, h) in [(420, 700), (900, 600), (1280, 800), (1920, 1080)] {
        let _ = draw(&mut a, &b.cx, w, h);
    }
}
