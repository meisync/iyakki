//! The media half of the engine against a fake instance: what it offers in hello, what it sends for sound, the clipboard (only with the
//! page's confirmation), camera frames and the device offers, and how it hands the instance's media messages to the page.

use iyakki_client_engine::engine::{Config, Engine, Out, Phase};
use iyakki_client_engine::host::Host;
use iyakki_client_engine::sign;
use iyakki_core_api::client::{self as proto, ClientMsg, ServerMsg};

const SEED: [u8; 32] = [3; 32];
const NONCE: &str = "00112233445566778899aabbccddeeff";

fn cfg() -> Config {
    Config { token: None, public: sign::hex(&sign::public_from_seed(&SEED)), name: "Maya's phone".into(), device: Some("dev1".into()), client: "test".into() }
}

fn sent(e: &mut Engine) -> (Vec<ClientMsg>, Vec<Out>) {
    let (mut msgs, mut rest) = (vec![], vec![]);
    for o in e.take_out() {
        match o {
            Out::Send { text } => msgs.push(proto::decode_client(&text).unwrap()),
            other => rest.push(other),
        }
    }
    (msgs, rest)
}

fn tell(e: &mut Engine, m: ServerMsg) {
    e.on_text(&proto::encode_server(&m));
}

fn hello_ok(caps: &[&str]) -> ServerMsg {
    ServerMsg::HelloOk { v: 1, caps: caps.iter().map(|c| c.to_string()).collect(), challenge: "chal1".into(), tile: 64, max_message: proto::MAX_TEXT, deprecations: vec![] }
}

fn live(caps: &[&str]) -> Engine {
    let mut e = Engine::new(cfg());
    e.connected();
    sent(&mut e);
    tell(&mut e, hello_ok(caps));
    sent(&mut e);
    assert!(e.sign_with_seed(&sign::hex(&SEED), 1_700_000_000, NONCE));
    sent(&mut e);
    tell(&mut e, ServerMsg::Authed { device: "dev1".into(), name: "Maya's phone".into() });
    assert!(e.sign_in("ann", "right"));
    sent(&mut e);
    tell(&mut e, ServerMsg::SignedIn { person: "ann".into() });
    sent(&mut e);
    tell(&mut e, ServerMsg::PictureOk);
    assert_eq!(e.phase(), Phase::Live);
    e
}

const BASE: &[&str] = &["picture", "input", "events", "calls"];

#[test]
fn hello_offers_the_media_capabilities_but_needs_none_of_them() {
    let mut e = Engine::new(cfg());
    e.connected();
    let (m, _) = sent(&mut e);
    let ClientMsg::Hello { offered, needed, .. } = &m[0] else { panic!() };
    for c in ["audio.out", "clipboard.text", "camera.frames", "client.camera", "client.microphone"] {
        assert!(offered.iter().any(|o| o == c), "{c}");
        assert!(!needed.iter().any(|o| o == c), "an older machine must still be usable: {c} is not needed");
    }
}

#[test]
fn the_page_is_told_what_was_agreed_so_it_can_hide_what_this_machine_cannot_do() {
    let mut e = Engine::new(cfg());
    e.connected();
    sent(&mut e);
    tell(&mut e, hello_ok(&["picture", "input", "audio.out"]));
    let (_, rest) = sent(&mut e);
    assert!(rest.iter().any(|o| matches!(o, Out::Caps { caps } if caps.contains(&"audio.out".to_string()) && !caps.contains(&"clipboard.text".to_string()))));
}

#[test]
fn sound_is_asked_for_by_side_and_only_when_signed_in() {
    let mut e = Engine::new(cfg());
    assert!(!e.audio_open("client"), "not before signing in");
    let mut e = live(BASE);
    assert!(!e.audio_open("machine"), "the machine alone needs no stream");
    assert!(!e.audio_open("everywhere"));
    assert!(e.audio_open("client"));
    assert!(e.audio_open("both"));
    assert!(e.audio_close());
    let (m, _) = sent(&mut e);
    assert_eq!(m, vec![ClientMsg::AudioOpen { side: Some("client".into()) }, ClientMsg::AudioOpen { side: Some("both".into()) }, ClientMsg::AudioClose]);
}

#[test]
fn the_clipboard_is_sent_only_with_the_confirmation_of_a_button_press_and_within_64_kb() {
    let mut e = live(BASE);
    assert!(!e.clip_put("secret", false), "no press, no send");
    assert!(!e.clip_get(false));
    assert!(!e.clip_put("", true));
    assert!(!e.clip_put(&"a".repeat(proto::media::MAX_CLIPBOARD + 1), true), "the cap is checked before anything leaves the page");
    assert!(sent(&mut e).0.is_empty(), "nothing was sent by the refused ones");
    assert!(e.clip_put("hello", true) && e.clip_get(true));
    let (m, _) = sent(&mut e);
    assert_eq!(m, vec![ClientMsg::ClipboardPut { text: "hello".into(), confirmed: true }, ClientMsg::ClipboardGet { confirmed: true }]);
}

#[test]
fn camera_views_and_device_offers_are_asked_for_by_name() {
    let mut e = live(BASE);
    assert!(e.camera_open("usb:video0") && e.camera_close("usb:video0") && !e.camera_open(""));
    assert!(e.offer_open("camera") && e.offer_open("microphone") && !e.offer_open("screen") && e.offer_close("camera"));
    let (m, _) = sent(&mut e);
    assert_eq!(m.len(), 5);
    assert!(matches!(&m[2], ClientMsg::OfferOpen { kind } if kind == "camera"));
}

#[test]
fn media_messages_of_the_instance_reach_the_page_as_records() {
    let mut e = live(BASE);
    tell(&mut e, ServerMsg::AudioOk { rate: 48_000, channels: 2, chunk_ms: 20, side: "client".into() });
    tell(&mut e, ServerMsg::AudioFlush);
    tell(&mut e, ServerMsg::ClipboardText { text: "from the machine".into() });
    tell(&mut e, ServerMsg::MediaErr { what: "camera".into(), code: "in_use".into(), message: "in use by Recorder".into() });
    tell(&mut e, ServerMsg::OfferOk { kind: "camera".into(), source: "Maya's phone camera".into() });
    let (_, rest) = sent(&mut e);
    let evs: Vec<&str> = rest.iter().filter_map(|o| if let Out::Media { ev, .. } = o { Some(ev.as_str()) } else { None }).collect();
    assert_eq!(evs, vec!["audio_ok", "audio_flush", "clipboard_text", "media_err", "offer_ok"]);
    let Some(Out::Media { body, .. }) = rest.iter().find(|o| matches!(o, Out::Media { ev, .. } if ev == "media_err")) else { panic!() };
    assert_eq!((body["code"].as_str(), body["what"].as_str()), (Some("in_use"), Some("camera")));
}

#[test]
fn the_json_host_layer_has_the_media_commands_and_refuses_unconfirmed_clipboard() {
    let mut h = Host::new();
    let public = sign::hex(&sign::public_from_seed(&SEED));
    assert!(h.init(&format!(r#"{{"public":"{public}","name":"n","device":"dev1"}}"#)));
    assert_eq!(h.command(r#"{"c":"audio_open","side":"client"}"#), -1, "not signed in");
    // drive it to Live through the engine inside the host
    {
        let e = h.engine().unwrap();
        e.connected();
        e.take_out();
        tell(e, hello_ok(BASE));
        e.take_out();
        assert!(e.sign_with_seed(&sign::hex(&SEED), 1_700_000_000, NONCE));
        tell(e, ServerMsg::Authed { device: "dev1".into(), name: "n".into() });
        assert!(e.sign_in("a", "b"));
        tell(e, ServerMsg::SignedIn { person: "a".into() });
        tell(e, ServerMsg::PictureOk);
        e.take_out();
    }
    assert_eq!(h.command(r#"{"c":"audio_open","side":"client"}"#), 0);
    assert_eq!(h.command(r#"{"c":"audio_open","side":"nowhere"}"#), -1);
    assert_eq!(h.command(r#"{"c":"clip_put","text":"x"}"#), -1, "unconfirmed");
    assert_eq!(h.command(r#"{"c":"clip_put","text":"x","confirmed":true}"#), 0);
    assert_eq!(h.command(r#"{"c":"clip_get","confirmed":true}"#), 0);
    assert_eq!(h.command(r#"{"c":"camera_open","unit":"usb:video0"}"#), 0);
    assert_eq!(h.command(r#"{"c":"offer_open","kind":"microphone"}"#), 0);
    assert_eq!(h.command(r#"{"c":"offer_open","kind":"x"}"#), -1);
}
