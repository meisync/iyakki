//! The consent card for sending microphone sound to a cloud machine (F-VOICE-38): shown only when the Settings switch is on, answered only by a
//! tap or a key, asked again when the switch goes off and on, and the chat tool that flips the switch is the same hard ask as everywhere.
use iyakki_agent::facets_h::RealServices as _;

use super::*;
use crate::apps::tests::cx_with_admin_store;
use crate::apps::voice_mine::T_MINE_STREAM_NO;
use iyakki_agent::cloudstream::Verdict;

fn tile(tag: &str, stream: bool) -> (AppCx, VoiceApp) {
    let cx = cx_with_admin_store(tag);
    cx.settings().lock().unwrap().update(|s| {
        s.voice.my_on = true;
        s.voice.my_cloud_stream = stream;
    }).unwrap();
    let mut v = VoiceApp::new(&cx);
    v.mine_tick(&cx);
    (cx, v)
}

#[test]
fn with_the_switch_off_there_is_no_card_and_nothing_may_be_sent() {
    let (cx, _v) = tile("vs-off", false);
    assert_eq!(cx.services.real().cloud_stream_verdict(), Verdict::Off);
}

#[test]
fn with_the_switch_on_a_card_asks_and_only_a_tap_or_a_key_answers_it() {
    let (cx, mut v) = tile("vs-on", true);
    assert!(matches!(cx.services.real().cloud_stream_verdict(), Verdict::Card(_)));
    // a key that is not the answer changes nothing
    assert!(v.mine_key(&Key::Char('x'), &cx).is_none());
    assert!(matches!(cx.services.real().cloud_stream_verdict(), Verdict::Card(_)));
    // the key
    assert!(v.mine_key(&Key::Char('y'), &cx).is_some());
    assert_eq!(cx.services.real().cloud_stream_verdict(), Verdict::Allowed);
    // the card does not come back within the session, and muting stops the stream at once
    v.mine_tick(&cx);
    assert_eq!(cx.services.real().cloud_stream_verdict(), Verdict::Allowed);
    cx.settings().lock().unwrap().update(|s| s.voice.my_muted = true).unwrap();
    v.mine_tick(&cx);
    assert_eq!(cx.services.real().cloud_stream_verdict(), Verdict::Muted);
    cx.settings().lock().unwrap().update(|s| s.voice.my_muted = false).unwrap();
    v.mine_tick(&cx);
    assert_eq!(cx.services.real().cloud_stream_verdict(), Verdict::Allowed);
    // off and on again asks again
    cx.settings().lock().unwrap().update(|s| s.voice.my_cloud_stream = false).unwrap();
    v.mine_tick(&cx);
    assert_eq!(cx.services.real().cloud_stream_verdict(), Verdict::Off);
    cx.settings().lock().unwrap().update(|s| s.voice.my_cloud_stream = true).unwrap();
    v.mine_tick(&cx);
    assert!(matches!(cx.services.real().cloud_stream_verdict(), Verdict::Card(_)));
    // the tap on "Do not allow" refuses it for the session
    v.mine_click(T_MINE_STREAM_NO, &cx);
    assert_eq!(cx.services.real().cloud_stream_verdict(), Verdict::Denied);
}
