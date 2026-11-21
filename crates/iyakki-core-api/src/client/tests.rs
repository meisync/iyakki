use super::*;

fn s(v: &[&str]) -> Vec<String> {
    v.iter().map(|x| x.to_string()).collect()
}

#[test]
fn every_client_message_survives_encode_and_decode() {
    let all = vec![
        ClientMsg::Hello { v: 1, min_v: 1, offered: s(&["picture"]), needed: s(&["calls"]), client: "web".into() },
        ClientMsg::Enrol { token: "t".into(), public: "ab".repeat(32), name: "Maya's laptop".into() },
        ClientMsg::Auth { authorization: "IYAKKI-SIG id=x".into() },
        ClientMsg::SignIn { user: "ann".into(), password: "pw".into() },
        ClientMsg::Call { id: 7, capability: "camera.view".into(), unit: Some("usb:1".into()) },
        ClientMsg::Input { msg: r#"{"t":"key"}"#.into() },
        ClientMsg::PictureOpen { deflate: true },
        ClientMsg::Ack { seq: 9 },
        ClientMsg::Resume { last_seq: 3 },
        ClientMsg::Ping { ts: 5 },
        ClientMsg::Bye,
        ClientMsg::AppOpen { app: "sample".into(), w: 800, h: 600 },
        ClientMsg::AppInput { lines: "key enter\naction 3".into() },
        ClientMsg::AppClose,
    ];
    for m in all {
        assert_eq!(decode_client(&encode_client(&m)).unwrap(), m);
    }
}

#[test]
fn every_server_message_survives_encode_and_decode() {
    let all = vec![
        ServerMsg::HelloOk { v: 1, caps: s(&["picture"]), challenge: "ff".into(), tile: 64, max_message: 10, deprecations: vec![] },
        ServerMsg::Enrolled { device: "d".into(), challenge: "c".into() },
        ServerMsg::Authed { device: "d".into(), name: "n".into() },
        ServerMsg::SignedIn { person: "ann".into() },
        ServerMsg::CallOk { id: 1, detail: String::new() },
        ServerMsg::CallErr { id: 2, code: "not_granted".into(), message: "no".into() },
        ServerMsg::Event { seq: 4, topic: "t".into(), body: "b".into() },
        ServerMsg::ResumeOk { from: 1, count: 2 },
        ServerMsg::ResumeGap { oldest: 8 },
        ServerMsg::PictureOk,
        ServerMsg::Pong { ts: 1 },
        ServerMsg::Error { code: ErrorCode::BadSignature, message: "x".into() },
        ServerMsg::Bye { reason: "revoked".into() },
    ];
    for m in all {
        assert_eq!(decode_server(&encode_server(&m)).unwrap(), m);
    }
}

#[test]
fn error_codes_are_stable_strings_and_match_the_wire() {
    for (c, w) in [(ErrorCode::VersionMismatch, "version_mismatch"), (ErrorCode::TokenUsed, "token_used"), (ErrorCode::DeviceRevoked, "device_revoked"), (ErrorCode::Replayed, "replayed"), (ErrorCode::NotSignedIn, "not_signed_in")] {
        assert_eq!(c.as_str(), w);
        assert_eq!(serde_json::to_string(&c).unwrap(), format!("\"{w}\""));
    }
}

#[test]
fn a_hello_with_only_the_version_is_enough_and_unknown_messages_are_refused() {
    let m = decode_client(r#"{"t":"hello","v":1}"#).unwrap();
    assert_eq!(m, ClientMsg::Hello { v: 1, min_v: 1, offered: vec![], needed: vec![], client: String::new() });
    assert_eq!(decode_client(r#"{"t":"nope"}"#).unwrap(), ClientMsg::Unknown, "a type a newer client added is an open case, not an error");
    assert_eq!(decode_client("not json").unwrap_err().code, ErrorCode::BadMessage);
    assert_eq!(decode_client(&format!(r#"{{"t":"ping","ts":1,"x":"{}"}}"#, "a".repeat(MAX_TEXT))).unwrap_err().code, ErrorCode::TooBig);
}

#[test]
fn version_mismatch_is_refused_both_ways() {
    let ok = negotiate(1, 1, &[], &[], cap::SERVER).unwrap();
    assert!(ok.is_empty());
    // a client that only speaks a newer protocol
    let e = negotiate(PROTOCOL_VERSION + 3, PROTOCOL_VERSION + 1, &[], &[], cap::SERVER).unwrap_err();
    assert_eq!(e.code, ErrorCode::VersionMismatch);
    assert!(e.message.contains(&PROTOCOL_VERSION.to_string()), "the answer names what the instance speaks");
    // a client that only speaks an older protocol than the oldest kept
    let e = negotiate(0, 0, &[], &[], cap::SERVER).unwrap_err();
    assert_eq!(e.code, ErrorCode::VersionMismatch);
    // a nonsense range
    assert_eq!(negotiate(1, 5, &[], &[], cap::SERVER).unwrap_err().code, ErrorCode::VersionMismatch);
    // a newer client that can also speak ours is fine
    assert!(negotiate(PROTOCOL_VERSION + 2, MIN_PROTOCOL_VERSION, &[], &[], cap::SERVER).is_ok());
}

#[test]
fn needed_capabilities_must_exist_and_offered_ones_are_intersected() {
    let agreed = negotiate(1, 1, &s(&["picture", "teleport"]), &s(&["calls"]), cap::SERVER).unwrap();
    assert_eq!(agreed, s(&["picture", "calls"]));
    let e = negotiate(1, 1, &[], &s(&["audio", "calls"]), cap::SERVER).unwrap_err();
    assert_eq!(e.code, ErrorCode::CapabilityMissing);
    assert!(e.message.contains("audio") && !e.message.contains("calls"));
}

#[test]
fn a_debug_print_never_shows_a_password_token_or_signature() {
    let shown = format!(
        "{:?} {:?} {:?}",
        ClientMsg::SignIn { user: "ann".into(), password: "hunter2".into() },
        ClientMsg::Enrol { token: "SECRETTOKEN".into(), public: "00".into(), name: "x".into() },
        ClientMsg::Auth { authorization: "sig=DEADBEEF".into() }
    );
    assert!(!shown.contains("hunter2") && !shown.contains("SECRETTOKEN") && !shown.contains("DEADBEEF"), "{shown}");
}

#[test]
fn binary_messages_carry_a_stream_tag() {
    let m = binary(STREAM_PICTURE, &[9, 8, 7]);
    assert_eq!(split_binary(&m), Some((STREAM_PICTURE, &[9u8, 8, 7][..])));
    assert_eq!(split_binary(&[]), None);
}

#[test]
fn events_resume_after_a_sequence_number_and_a_hole_is_a_gap() {
    let mut r = EventRing::new(3);
    assert_eq!(r.latest(), 0);
    assert_eq!(r.since(0, "ann"), Resumed::Events(vec![]));
    for i in 0..5 {
        r.push(if i == 3 { "bob" } else { "" }, "t", &format!("b{i}"));
    }
    assert_eq!(r.latest(), 5);
    // kept: 3, 4, 5. From 2 is still contiguous; 3 is bob's own
    match r.since(2, "ann") {
        Resumed::Events(v) => assert_eq!(v.iter().map(|k| k.seq).collect::<Vec<_>>(), vec![3, 5], "bob's event (seq 4) is not shown to ann"),
        other => panic!("{other:?}"),
    }
    assert_eq!(r.since(1, "ann"), Resumed::Gap { oldest: 3 });
    assert_eq!(r.since(5, "ann"), Resumed::Events(vec![]));
    assert_eq!(r.since(99, "ann"), Resumed::Gap { oldest: 3 }, "a client ahead of the ring (the instance restarted) starts again");
}

#[test]
fn the_display_list_pair_is_additive_a_list_may_be_long_and_typed_text_is_never_printed() {
    // advertised by its own capability; the old lists are unchanged, so an older client that offers nothing new agrees on nothing new
    assert!(cap::SERVER_UI.contains(&cap::DISPLAY_LIST) && !cap::SERVER.contains(&cap::DISPLAY_LIST) && !cap::SERVER_MEDIA.contains(&cap::DISPLAY_LIST));
    let mut server: Vec<&str> = cap::SERVER.to_vec();
    assert!(negotiate(1, 1, &s(&["picture"]), &[], &server).unwrap() == s(&["picture"]), "without the list capability an old client's agreement is as before");
    server.extend_from_slice(cap::SERVER_UI);
    assert_eq!(negotiate(1, 1, &s(&["picture", "display.list"]), &[], &server).unwrap(), s(&["picture", "display.list"]));
    assert_eq!(negotiate(1, 1, &[], &s(&["display.list"]), cap::SERVER).unwrap_err().code, ErrorCode::CapabilityMissing, "an instance without it says so");
    // every message of the instance's side survives encode and decode, and none of them is a ServerMsg (that reader is unchanged)
    for m in [
        AppMsg::AppList { app: "sample".into(), text: "iyakki-display-list 1 800 600\n".into() },
        AppMsg::AppClosed { app: "sample".into(), reason: "left".into() },
        AppMsg::AppErr { code: "not_found".into(), message: "no such app".into() },
    ] {
        let text = encode_app(&m);
        assert_eq!(decode_app(&text).unwrap(), Some(m));
        assert_eq!(decode_server(&text).unwrap(), ServerMsg::Unknown, "the old reader does not know it (a type it has no name for is Unknown): {text}");
    }
    // a text that is not one of these is not this reader's
    assert_eq!(decode_app(&encode_server(&ServerMsg::PictureOk)).unwrap(), None);
    assert_eq!(decode_app("not json").unwrap(), None);
    assert_eq!(decode_app(r#"{"t":"app_nope"}"#).unwrap_err().code, ErrorCode::BadMessage);
    assert_eq!(decode_app(r#"{"t":"app_list","app":"x"}"#).unwrap_err().code, ErrorCode::BadMessage, "a field missing");
    assert_eq!(decode_app(r#"{"t":"app_list","app":1,"text":"x"}"#).unwrap_err().code, ErrorCode::BadMessage, "a field of the wrong kind");
    // a list is longer than an ordinary message but not longer than its own bound; any other long message is still refused
    let long = AppMsg::AppList { app: "sample".into(), text: "x".repeat(MAX_TEXT * 2) };
    assert_eq!(decode_app(&encode_app(&long)).unwrap(), Some(long));
    let too_long = AppMsg::AppList { app: "sample".into(), text: "x".repeat(MAX_APP_LIST + 512) };
    assert_eq!(decode_app(&encode_app(&too_long)).unwrap_err().code, ErrorCode::TooBig);
    let other = AppMsg::AppClosed { app: "x".repeat(MAX_TEXT), reason: String::new() };
    assert_eq!(decode_app(&encode_app(&other)).unwrap_err().code, ErrorCode::TooBig);
    // the client's own input lines are ordinary messages: bounded by MAX_TEXT
    let big = ClientMsg::AppInput { lines: "t".repeat(MAX_TEXT) };
    assert_eq!(decode_client(&encode_client(&big)).unwrap_err().code, ErrorCode::TooBig);
    let shown = format!("{:?}", ClientMsg::AppInput { lines: "text hunter2".into() });
    assert!(!shown.contains("hunter2"), "{shown}");
    // the old wire is untouched: an old message decodes exactly as before
    assert_eq!(decode_client(r#"{"t":"picture_open"}"#).unwrap(), ClientMsg::PictureOpen { deflate: false });
    assert_eq!(decode_server(r#"{"t":"picture_ok"}"#).unwrap(), ServerMsg::PictureOk);
}
