//! The engine's state machine against a fake instance (no socket): every step of a connection, the error codes, resume, the picture and
//! the input rules. The fake speaks the protocol with the same types the instance uses (`iyakki_core_api::client`).

use iyakki_client_engine::engine::{describe, Action, Config, Engine, Out, Phase};
use iyakki_client_engine::host::Host;
use iyakki_client_engine::sign;
use iyakki_core_api::client::{self as proto, ClientMsg, ErrorCode, ServerMsg};
use iyakki_link::pixelcodec::{Caps, Level, Opts, Streamer};

const SEED: [u8; 32] = [3; 32];
const NONCE: &str = "00112233445566778899aabbccddeeff";

fn cfg(token: Option<&str>, device: Option<&str>) -> Config {
    Config { token: token.map(str::to_string), public: sign::hex(&sign::public_from_seed(&SEED)), name: "Maya's phone".into(), device: device.map(str::to_string), client: "test".into() }
}

/// What the engine sent, decoded.
fn sent(e: &mut Engine) -> (Vec<ClientMsg>, Vec<Out>) {
    let (mut msgs, mut rest) = (vec![], vec![]);
    for o in e.take_out() {
        match o {
            Out::Send { text } => msgs.push(proto::decode_client(&text).expect("the engine only sends messages of the protocol")),
            other => rest.push(other),
        }
    }
    (msgs, rest)
}

fn tell(e: &mut Engine, m: ServerMsg) {
    e.on_text(&proto::encode_server(&m));
}

fn hello_ok(challenge: &str) -> ServerMsg {
    ServerMsg::HelloOk { v: 1, caps: vec!["picture".into(), "input".into(), "events".into(), "calls".into()], challenge: challenge.into(), tile: 64, max_message: proto::MAX_TEXT, deprecations: vec![] }
}

/// Drive a returning device to the sign-in step.
fn to_sign_in(e: &mut Engine) {
    e.connected();
    let (m, _) = sent(e);
    assert!(matches!(m[0], ClientMsg::Hello { v: 1, .. }));
    tell(e, hello_ok("chal1"));
    let (m, rest) = sent(e);
    assert!(m.is_empty());
    assert!(rest.iter().any(|o| matches!(o, Out::Sign { device, challenge } if device == "dev1" && challenge == "chal1")));
    assert!(e.sign_with_seed(&sign::hex(&SEED), 1_700_000_000, NONCE));
    let (m, _) = sent(e);
    let ClientMsg::Auth { authorization } = &m[0] else { panic!("{m:?}") };
    // exactly what the instance verifies
    let parsed = iyakki_link::sigscheme::parse_authorization(authorization).unwrap();
    assert!(parsed.verify(&sign::public_from_seed(&SEED), "GET", proto::WS_PATH, b"chal1"));
    tell(e, ServerMsg::Authed { device: "dev1".into(), name: "Maya's phone".into() });
    assert_eq!(e.phase(), Phase::SignIn);
}

fn picture_msg() -> Vec<u8> {
    let rgba: Vec<u8> = (0..128 * 64).flat_map(|_| [10u8, 20, 30, 255]).collect();
    let mut st = Streamer::new();
    st.set_source(128, 64, rgba);
    let e = st.encode(&Opts { level: Level::Lossless, caps: Caps { deflate: true }, budget: usize::MAX, now_ms: 0 }).unwrap();
    proto::binary(proto::STREAM_PICTURE, &e.msg)
}

fn live(e: &mut Engine) {
    to_sign_in(e);
    assert!(e.sign_in("ann", "right"));
    let (m, _) = sent(e);
    assert!(matches!(&m[0], ClientMsg::SignIn { user, .. } if user == "ann"));
    tell(e, ServerMsg::SignedIn { person: "ann".into() });
    let (m, _) = sent(e);
    assert!(m.iter().any(|x| matches!(x, ClientMsg::PictureOpen { deflate: true })), "the engine can inflate, so it says so: no PNG tiles");
    tell(e, ServerMsg::PictureOk);
    assert_eq!(e.phase(), Phase::Live);
}

#[test]
fn a_new_device_enrols_with_the_link_then_proves_itself_signs_in_and_gets_a_picture() {
    let mut e = Engine::new(cfg(Some("the-link-token"), None));
    e.connected();
    let (m, _) = sent(&mut e);
    let ClientMsg::Hello { offered, needed, .. } = &m[0] else { panic!() };
    assert!(needed.iter().any(|n| n == "picture") && offered.iter().any(|n| n == "events"));
    tell(&mut e, hello_ok("c0"));
    let (m, _) = sent(&mut e);
    assert!(matches!(&m[0], ClientMsg::Enrol { token, public, name } if token == "the-link-token" && public.len() == 64 && name == "Maya's phone"));
    assert_eq!(e.phase(), Phase::Enrolling);
    tell(&mut e, ServerMsg::Enrolled { device: "dev1".into(), challenge: "c1".into() });
    let (_, rest) = sent(&mut e);
    assert!(rest.contains(&Out::Device { id: "dev1".into() }), "the page is told to keep the device id");
    assert!(rest.iter().any(|o| matches!(o, Out::Sign { .. })));
    assert!(e.sign_with_seed(&sign::hex(&SEED), 1_700_000_000, NONCE));
    let (m, _) = sent(&mut e);
    assert!(matches!(m[0], ClientMsg::Auth { .. }));
    tell(&mut e, ServerMsg::Authed { device: "dev1".into(), name: "Maya's phone".into() });
    assert!(e.sign_in("ann", "right"));
    sent(&mut e);
    tell(&mut e, ServerMsg::SignedIn { person: "ann".into() });
    tell(&mut e, ServerMsg::PictureOk);
    e.on_binary(&picture_msg());
    let (m, rest) = sent(&mut e);
    assert!(m.iter().any(|x| matches!(x, ClientMsg::Ack { .. })), "every drawn picture is acknowledged");
    assert!(rest.iter().any(|o| matches!(o, Out::Frame { w: 128, h: 64, screen_w: 128, screen_h: 64, .. })));
    let (px, w, h, v) = e.framebuffer();
    assert_eq!((w, h, v), (128, 64, 1));
    assert_eq!(&px[..4], &[10, 20, 30, 255]);
    // the link token is not kept anywhere in the engine: a second hello on the same engine cannot use it again
    e.connected();
    sent(&mut e);
    tell(&mut e, hello_ok("c2"));
    let (m, _) = sent(&mut e);
    assert!(m.is_empty(), "a known device signs, it does not enrol again: {m:?}");
    assert!(format!("{:?}", cfg(Some("the-link-token"), None)).contains("<set>") && !format!("{:?}", cfg(Some("the-link-token"), None)).contains("the-link"), "Debug never shows the token");
}

#[test]
fn a_returning_device_with_no_link_and_no_device_stops_with_a_clear_message() {
    let mut e = Engine::new(cfg(None, None));
    e.connected();
    sent(&mut e);
    tell(&mut e, hello_ok("c"));
    let (_, rest) = sent(&mut e);
    assert!(rest.iter().any(|o| matches!(o, Out::Fail { code, fatal: true, action: Action::NewLink, .. } if code == "no_link")));
    assert!(!e.wants_reconnect());
}

#[test]
fn a_wrong_password_asks_again_and_the_third_try_works() {
    let mut e = Engine::new(cfg(None, Some("dev1")));
    to_sign_in(&mut e);
    for _ in 0..2 {
        assert!(e.sign_in("ann", "wrong"));
        sent(&mut e);
        tell(&mut e, ServerMsg::Error { code: ErrorCode::SignInRefused, message: "x".into() });
        let (_, rest) = sent(&mut e);
        assert!(rest.iter().any(|o| matches!(o, Out::Fail { code, fatal: false, action: Action::SignIn, .. } if code == "sign_in_refused")));
        assert_eq!(e.phase(), Phase::SignIn);
        assert!(e.wants_reconnect());
    }
    assert!(e.sign_in("ann", "right"));
}

#[test]
fn a_password_is_only_sent_at_the_sign_in_step_and_never_shown_by_debug() {
    let mut e = Engine::new(cfg(None, Some("dev1")));
    assert!(!e.sign_in("ann", "right"), "not before the device is proven");
    e.connected();
    assert!(!e.sign_in("ann", "right"));
    let m = ClientMsg::SignIn { user: "ann".into(), password: "hunter2".into() };
    assert!(!format!("{m:?}").contains("hunter2"));
    let c = cfg(Some("tok-123"), None);
    assert!(!format!("{c:?}").contains("tok-123"));
}

#[test]
fn every_error_code_has_words_and_a_decision_and_the_page_never_loops_on_a_fatal_one() {
    let all = [
        "bad_message", "too_big", "bad_sequence", "version_mismatch", "capability_missing", "token_unknown", "token_expired", "token_used", "bad_enrolment", "bad_signature",
        "replayed", "device_unknown", "device_revoked", "rate_limited", "not_authenticated", "sign_in_refused", "not_signed_in", "role_denied", "unsupported", "closed", "internal",
    ];
    assert_eq!(all.len(), 21, "the protocol has 21 stable codes");
    for name in all {
        let code: ErrorCode = serde_json::from_str(&format!("\"{name}\"")).unwrap_or_else(|_| panic!("{name} is not a code"));
        assert_eq!(code.as_str(), name);
        let d = describe(code);
        assert!(d.title.len() > 3 && d.message.len() > 20, "{name}");
        let mut e = Engine::new(cfg(None, Some("dev1")));
        to_sign_in(&mut e);
        tell(&mut e, ServerMsg::Error { code, message: "free text from the machine".into() });
        let (_, rest) = sent(&mut e);
        let fail = rest.iter().find_map(|o| if let Out::Fail { code: c, fatal, action, message, .. } = o { Some((c.clone(), *fatal, *action, message.clone())) } else { None }).unwrap_or_else(|| panic!("{name}: no Fail"));
        assert_eq!(fail.0, name);
        assert_eq!(fail.1, d.fatal);
        assert_eq!(e.wants_reconnect(), !d.fatal, "{name}");
        assert!(!fail.3.contains("free text"), "the page shows its own words");
    }
    // a few that matter
    assert_eq!(describe(ErrorCode::TokenExpired).action, Action::NewLink);
    assert_eq!(describe(ErrorCode::TokenUsed).action, Action::NewLink);
    assert_eq!(describe(ErrorCode::DeviceRevoked).action, Action::ForgetDevice);
    assert_eq!(describe(ErrorCode::DeviceUnknown).action, Action::ForgetDevice);
    assert!(describe(ErrorCode::BadSignature).message.contains("clock"));
    assert!(!describe(ErrorCode::RateLimited).fatal && describe(ErrorCode::RateLimited).retry_s >= 60);
}

#[test]
fn bye_reasons_are_told_and_a_revoked_device_does_not_reconnect() {
    for (reason, reconnect, action) in [("revoked", false, Action::ForgetDevice), ("disabled", false, Action::None), ("signed_out", false, Action::SignIn), ("restart", true, Action::None)] {
        let mut e = Engine::new(cfg(None, Some("dev1")));
        live(&mut e);
        tell(&mut e, ServerMsg::Bye { reason: reason.into() });
        let (_, rest) = sent(&mut e);
        assert!(rest.iter().any(|o| matches!(o, Out::Bye { reason: r, action: a, .. } if r == reason && *a == action)), "{reason}");
        assert_eq!(e.wants_reconnect(), reconnect, "{reason}");
        e.closed();
        if !reconnect {
            assert_eq!(e.phase(), Phase::Failed, "the end stays visible: {reason}");
        }
    }
}

#[test]
fn a_dropped_connection_resumes_events_and_keeps_the_picture() {
    let mut e = Engine::new(cfg(None, Some("dev1")));
    live(&mut e);
    e.on_binary(&picture_msg());
    tell(&mut e, ServerMsg::Event { seq: 4, topic: "note".into(), body: "one".into() });
    tell(&mut e, ServerMsg::Event { seq: 9, topic: "note".into(), body: "two".into() });
    tell(&mut e, ServerMsg::Event { seq: 7, topic: "late".into(), body: "old".into() });
    assert_eq!(e.last_seq(), 9, "never goes back");
    sent(&mut e);
    e.closed();
    assert_eq!(e.phase(), Phase::Closed);
    assert!(e.wants_reconnect());
    let (px0, ..) = e.framebuffer();
    assert_eq!(&px0[..4], &[10, 20, 30, 255], "the last picture stays on screen while reconnecting");
    // connect again: hello, signature, sign-in, then resume before the picture
    to_sign_in(&mut e);
    assert!(e.sign_in("ann", "right"));
    sent(&mut e);
    tell(&mut e, ServerMsg::SignedIn { person: "ann".into() });
    let (m, _) = sent(&mut e);
    let resume = m.iter().position(|x| matches!(x, ClientMsg::Resume { last_seq: 9 })).expect("resume from the last event");
    let open = m.iter().position(|x| matches!(x, ClientMsg::PictureOpen { .. })).unwrap();
    assert!(resume < open);
    tell(&mut e, ServerMsg::ResumeOk { from: 9, count: 2 });
    tell(&mut e, ServerMsg::ResumeGap { oldest: 50 });
    assert_eq!(e.last_seq(), 49);
    let (_, rest) = sent(&mut e);
    assert!(rest.iter().filter(|o| matches!(o, Out::Notice { .. })).count() == 2);
}

#[test]
fn input_becomes_messages_only_once_signed_in_and_pointer_positions_follow_the_machines_screen() {
    let mut e = Engine::new(cfg(None, Some("dev1")));
    to_sign_in(&mut e);
    e.key("a", "KeyA", false, false, false);
    e.pointer("down", 1, 2);
    e.size(390, 700);
    assert!(sent(&mut e).0.is_empty(), "nothing before sign-in");
    assert!(e.sign_in("ann", "right"));
    sent(&mut e);
    tell(&mut e, ServerMsg::SignedIn { person: "ann".into() });
    let (m, _) = sent(&mut e);
    assert!(m.iter().any(|x| matches!(x, ClientMsg::Input { msg } if msg.contains("\"size\"") && msg.contains("390"))), "the size seen before sign-in is sent when the picture opens: {m:?}");
    tell(&mut e, ServerMsg::PictureOk);
    e.on_binary(&picture_msg());
    sent(&mut e);
    // the picture is 128x64 on a 256x128 box: a click at the middle of the box is the middle of the screen
    assert_eq!(e.map_point(128.0, 64.0, 256.0, 128.0), Some((64, 32)));
    assert_eq!(e.map_point(-5.0, 9999.0, 256.0, 128.0), Some((0, 63)), "clamped to the screen");
    assert_eq!(e.map_point(f64::NAN, 0.0, 256.0, 128.0), None);
    e.pointer("move", 64, 32);
    e.pointer("bogus", 1, 1);
    e.wheel(-300);
    e.shift(true);
    e.key("Enter", "Enter", false, false, false);
    e.key("", "", false, false, false);
    e.text("hi");
    e.full();
    let (m, _) = sent(&mut e);
    let inputs: Vec<&str> = m.iter().filter_map(|x| if let ClientMsg::Input { msg } = x { Some(msg.as_str()) } else { None }).collect();
    // every input is a message the instance's own parser accepts
    for i in &inputs {
        let v: serde_json::Value = serde_json::from_str(i).unwrap();
        assert!(["move", "wheel", "shift", "key", "full"].contains(&v["t"].as_str().unwrap()), "{i}");
    }
    assert_eq!(inputs.len(), 1 + 1 + 1 + 1 + 2 + 1, "move, wheel, shift, Enter, two typed keys, full: {inputs:?}");
    assert!(inputs[1].contains("\"d\":-20"), "a wheel is clamped as the instance does: {}", inputs[1]);
}

#[test]
fn a_broken_picture_asks_for_the_whole_picture_a_few_times_then_stops_asking() {
    let mut e = Engine::new(cfg(None, Some("dev1")));
    live(&mut e);
    sent(&mut e);
    let mut bad = picture_msg();
    bad.truncate(30);
    for i in 0..6 {
        e.on_binary(&bad);
        let (m, rest) = sent(&mut e);
        assert!(rest.iter().any(|o| matches!(o, Out::Notice { .. })));
        let asked = m.iter().any(|x| matches!(x, ClientMsg::Input { msg } if msg.contains("full")));
        assert_eq!(asked, i < 3, "try {i}");
    }
    e.on_binary(&picture_msg());
    let (m, _) = sent(&mut e);
    assert!(m.iter().any(|x| matches!(x, ClientMsg::Ack { .. })), "and it recovers");
    // unknown streams and pictures before the picture is open are ignored
    e.on_binary(&[9, 1, 2, 3]);
    e.on_binary(&[]);
    assert!(sent(&mut e).0.is_empty());
    let mut idle = Engine::new(cfg(None, Some("dev1")));
    idle.on_binary(&picture_msg());
    assert!(idle.take_out().is_empty());
}

#[test]
fn calls_pings_and_capability_checks() {
    let mut e = Engine::new(cfg(None, Some("dev1")));
    live(&mut e);
    e.call(5, "camera.view", Some("usb:1"));
    e.ping(77);
    let (m, _) = sent(&mut e);
    assert!(matches!(&m[0], ClientMsg::Call { id: 5, capability, unit: Some(u) } if capability == "camera.view" && u == "usb:1"));
    assert!(matches!(m[1], ClientMsg::Ping { ts: 77 }));
    tell(&mut e, ServerMsg::CallErr { id: 5, code: "not_granted".into(), message: "no".into() });
    tell(&mut e, ServerMsg::Pong { ts: 77 });
    let (_, rest) = sent(&mut e);
    assert!(matches!(&rest[0], Out::CallErr { id: 5, code, .. } if code == "not_granted") && matches!(rest[1], Out::Pong { ts: 77 }));
    // an instance that cannot serve pictures is refused before anything is sent
    let mut e = Engine::new(cfg(None, Some("dev1")));
    e.connected();
    sent(&mut e);
    tell(&mut e, ServerMsg::HelloOk { v: 1, caps: vec!["events".into()], challenge: "c".into(), tile: 64, max_message: 1, deprecations: vec![] });
    let (m, rest) = sent(&mut e);
    assert!(m.is_empty() && rest.iter().any(|o| matches!(o, Out::Fail { code, .. } if code == "capability_missing")));
    // garbage from the machine is a notice, not a crash
    e.on_text("not json");
    e.on_text("{\"t\":\"nothing\"}");
    assert!(sent(&mut e).1.iter().all(|o| matches!(o, Out::Notice { .. })));
}

#[test]
fn the_json_host_layer_drives_the_same_machine_and_refuses_bad_commands() {
    let mut h = Host::new();
    assert_eq!(h.command(r#"{"c":"connected"}"#), -1, "nothing before init");
    assert!(!h.init(r#"{"public":"short"}"#));
    let public = sign::hex(&sign::public_from_seed(&SEED));
    assert!(h.init(&format!(r#"{{"public":"{public}","name":"n","device":"dev1","token":null}}"#)));
    assert_eq!(h.command(r#"{"c":"connected"}"#), 0);
    assert!(h.next_out() > 0 && std::str::from_utf8(h.reply()).unwrap().contains("\"k\":\"phase\""));
    assert!(h.next_out() > 0 && std::str::from_utf8(h.reply()).unwrap().contains("\"k\":\"send\""));
    h.on_text(&proto::encode_server(&hello_ok("zz")));
    assert!(h.next_out() > 0);
    assert!(std::str::from_utf8(h.reply()).unwrap().contains("\"k\":\"phase\""));
    assert!(h.next_out() > 0 && std::str::from_utf8(h.reply()).unwrap().contains("\"k\":\"sign\""));
    // sign through the text path, as the WebCrypto page does
    let n = h.command(&format!(r#"{{"c":"begin_sign","ts":1700000000,"nonce":"{NONCE}"}}"#));
    assert!(n > 0);
    let text = serde_json::from_slice::<serde_json::Value>(h.reply()).unwrap()["text"].as_str().unwrap().to_string();
    assert_eq!(text, sign::request_text("dev1", "zz", 1_700_000_000, NONCE));
    use ed25519_dalek::Signer;
    let sig = sign::hex(&ed25519_dalek::SigningKey::from_bytes(&SEED).sign(text.as_bytes()).to_bytes());
    assert_eq!(h.command(&format!(r#"{{"c":"finish_sign","sig":"{sig}"}}"#)), 0);
    assert_eq!(h.command(r#"{"c":"finish_sign","sig":"00"}"#), -1, "a second answer is refused");
    // bad commands
    for bad in ["", "[]", r#"{"c":"nope"}"#, r#"{"c":"pointer"}"#, r#"{"c":"begin_sign","ts":1,"nonce":"zz"}"#, r#"{"c":"sign_in","user":"a","password":"b"}"#, r#"{"c":"public_from_seed","seed":"12"}"#] {
        assert_eq!(h.command(bad), -1, "{bad}");
    }
    assert!(h.command(&format!(r#"{{"c":"public_from_seed","seed":"{}"}}"#, sign::hex(&SEED))) > 0);
    assert!(h.command(r#"{"c":"version"}"#) > 0);
}

#[test]
fn begin_sign_refuses_a_bad_nonce_and_a_missing_challenge() {
    let mut e = Engine::new(cfg(None, Some("dev1")));
    assert!(e.begin_sign(1, NONCE).is_none(), "no challenge yet");
    e.connected();
    sent(&mut e);
    tell(&mut e, hello_ok("c"));
    assert!(e.begin_sign(1, "short").is_none());
    assert!(e.begin_sign(1, "zzzzzzzzzzzzzzzzzzzz").is_none());
    assert!(!e.finish_sign(&"00".repeat(64)), "no text was begun");
    assert!(e.begin_sign(1, NONCE).is_some());
    assert!(!e.finish_sign("00"), "a signature is 64 bytes");
}

#[test]
fn the_state_command_tells_the_page_whether_to_reconnect() {
    let mut h = Host::new();
    let public = sign::hex(&sign::public_from_seed(&SEED));
    assert!(h.init(&format!(r#"{{"public":"{public}","name":"n","device":"dev1"}}"#)));
    assert!(h.command(r#"{"c":"state"}"#) > 0);
    let v: serde_json::Value = serde_json::from_slice(h.reply()).unwrap();
    assert_eq!((v["phase"].as_str(), v["reconnect"].as_bool(), v["device"].as_str()), (Some("idle"), Some(true), Some("dev1")));
    h.on_text(&proto::encode_server(&ServerMsg::Error { code: ErrorCode::TokenUsed, message: String::new() }));
    h.command(r#"{"c":"state"}"#);
    let v: serde_json::Value = serde_json::from_slice(h.reply()).unwrap();
    assert_eq!((v["phase"].as_str(), v["reconnect"].as_bool()), (Some("failed"), Some(false)));
}
