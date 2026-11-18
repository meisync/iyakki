//! Compatibility fixtures of the media half of the client protocol (docs/development/CORE-COMPATIBILITY-DESIGN.md; docs/development/CLIENT-SPLIT-DESIGN.md
//! sections 6 and 7b). EXPERIMENTAL: the folder is `tests/compat/alpha3/`, written once from this release's own code and never edited; later
//! releases must read it.
//!
//!   * `client-media.jsonl`        a transcript of sound, clipboard, camera and device-offer messages, one `{"dir":"c"|"s","msg":{..}}` per line
//!   * `capability-catalogue-added.txt`  the capabilities this release added to the catalogue, in the layout of the alpha2 file
//!   * `binary-frames.txt`         the binary payloads of the four media streams as hex, with what they decode to
//!
//! Re-record (only when making a release folder, never to make a failing test pass):
//! `IYAKKI_COMPAT_RECORD=1 cargo test -p iyakki-core-api --test compat_alpha3_media record -- --ignored`.

use std::path::{Path, PathBuf};

use iyakki_core_api::appcaps;
use iyakki_core_api::client::media;
use iyakki_core_api::client::{self as client, decode_client, decode_server, encode_client, encode_server, ClientMsg, ErrorCode, ServerMsg};
use serde_json::{json, Value};

fn dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/compat/alpha3")
}

fn read(name: &str) -> String {
    std::fs::read_to_string(dir().join(name)).unwrap_or_else(|e| panic!("tests/compat/alpha3/{name}: {e}"))
}

fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

fn unhex(s: &str) -> Vec<u8> {
    (0..s.len() / 2).map(|i| u8::from_str_radix(&s[2 * i..2 * i + 2], 16).unwrap()).collect()
}

fn transcript() -> Vec<(&'static str, Value)> {
    let c = |m: ClientMsg| ("c", serde_json::from_str::<Value>(&encode_client(&m)).unwrap());
    let s = |m: ServerMsg| ("s", serde_json::from_str::<Value>(&encode_server(&m)).unwrap());
    vec![
        c(ClientMsg::Hello { v: 1, min_v: 1, offered: client::cap::SERVER_MEDIA.iter().map(|x| x.to_string()).collect(), needed: vec![], client: "compat-fixture".into() }),
        s(ServerMsg::HelloOk { v: 1, caps: client::cap::SERVER_MEDIA.iter().map(|x| x.to_string()).collect(), challenge: "00112233445566778899aabbccddeeff".into(), tile: 64, max_message: 65536, deprecations: vec![] }),
        c(ClientMsg::AudioOpen { side: None }),
        s(ServerMsg::AudioOk { rate: 48000, channels: 2, chunk_ms: 20, side: "client".into() }),
        c(ClientMsg::AudioOpen { side: Some("both".into()) }),
        s(ServerMsg::AudioFlush),
        c(ClientMsg::AudioClose),
        s(ServerMsg::AudioClosed { reason: "closed".into() }),
        c(ClientMsg::ClipboardPut { text: "hello".into(), confirmed: true }),
        s(ServerMsg::ClipboardOk),
        c(ClientMsg::ClipboardGet { confirmed: true }),
        s(ServerMsg::ClipboardText { text: "from the machine".into() }),
        c(ClientMsg::CameraOpen { unit: "usb:video0".into() }),
        s(ServerMsg::CameraOk { unit: "usb:video0".into(), fps: 5 }),
        s(ServerMsg::CameraClosed { unit: "usb:video0".into(), reason: "in_use: Recorder".into() }),
        c(ClientMsg::CameraClose { unit: "usb:video0".into() }),
        c(ClientMsg::OfferOpen { kind: "camera".into() }),
        s(ServerMsg::OfferOk { kind: "camera".into(), source: "Fixture laptop camera".into() }),
        c(ClientMsg::OfferClose { kind: "camera".into() }),
        s(ServerMsg::OfferClosed { kind: "camera".into(), reason: "closed".into() }),
        s(ServerMsg::MediaErr { what: "camera".into(), code: "in_use".into(), message: "in use by Recorder".into() }),
    ]
}

fn frames() -> String {
    let pcm: Vec<i16> = (0..8).map(|i| i * 1000 - 3000).collect();
    let jpeg = [0xFFu8, 0xD8, 1, 2, 3, 0xFF, 0xD9];
    let mut out = String::new();
    out.push_str(&format!("audio\t{}\n", hex(&client::binary(client::STREAM_AUDIO, &media::encode_audio(7, &pcm)))));
    out.push_str(&format!("camera\t{}\n", hex(&client::binary(client::STREAM_CAMERA, &media::encode_camera("usb:video0", 9, &jpeg).unwrap()))));
    out.push_str(&format!("mic_in\t{}\n", hex(&client::binary(client::STREAM_MIC_IN, &[1, 0, 2, 0]))));
    out.push_str(&format!("camera_in\t{}\n", hex(&client::binary(client::STREAM_CAMERA_IN, &jpeg))));
    out
}

#[test]
#[ignore]
fn record() {
    if std::env::var("IYAKKI_COMPAT_RECORD").is_err() {
        return;
    }
    std::fs::create_dir_all(dir()).unwrap();
    let lines: String = transcript().into_iter().map(|(d, m)| format!("{}\n", json!({"dir": d, "msg": m}))).collect();
    std::fs::write(dir().join("client-media.jsonl"), lines).unwrap();
    let added: String = ["clipboard.client", "client.camera.offer", "client.microphone.offer"].iter().map(|n| {
        let d = appcaps::lookup(n).unwrap();
        format!("{}\t{:?}\t{:?}\t{}\n", d.name, d.param, d.builtin, d.hard_ask)
    }).collect();
    std::fs::write(dir().join("capability-catalogue-added.txt"), added).unwrap();
    std::fs::write(dir().join("binary-frames.txt"), frames()).unwrap();
}

fn recorded() -> Vec<(char, Value)> {
    read("client-media.jsonl")
        .lines()
        .map(|l| {
            let v: Value = serde_json::from_str(l).unwrap();
            (v["dir"].as_str().unwrap().chars().next().unwrap(), v["msg"].clone())
        })
        .collect()
}

#[test]
fn backward_the_recorded_media_messages_are_read_and_written_back_unchanged() {
    let all = recorded();
    assert!(all.len() >= 20);
    for (dir, msg) in &all {
        let text = msg.to_string();
        let again = if *dir == 'c' { encode_client(&decode_client(&text).unwrap_or_else(|e| panic!("{text}: {e}"))) } else { encode_server(&decode_server(&text).unwrap_or_else(|e| panic!("{text}: {e}"))) };
        assert_eq!(serde_json::from_str::<Value>(&again).unwrap(), *msg, "{text}");
    }
}

#[test]
fn forward_unknown_fields_in_media_messages_are_ignored() {
    for (dir, msg) in recorded() {
        let mut richer = msg.clone();
        richer.as_object_mut().unwrap().insert("added_in_a_newer_release".into(), json!([1]));
        let (a, b) = if dir == 'c' { (decode_client(&msg.to_string()).map(|m| encode_client(&m)), decode_client(&richer.to_string()).map(|m| encode_client(&m))) } else { (decode_server(&msg.to_string()).map(|m| encode_server(&m)), decode_server(&richer.to_string()).map(|m| encode_server(&m))) };
        assert_eq!(a.unwrap(), b.unwrap_or_else(|e| panic!("{msg}: {e}")));
    }
}

#[test]
fn an_older_client_without_the_media_capabilities_still_negotiates_and_gets_none_of_them() {
    // the alpha.2 hello (offers four capabilities) against an instance that now serves the media ones too
    let mut server: Vec<&str> = client::cap::SERVER.to_vec();
    server.extend_from_slice(client::cap::SERVER_MEDIA);
    let agreed = client::negotiate(1, 1, &["picture".into(), "input".into(), "events".into(), "calls".into()], &["picture".into()], &server).unwrap();
    assert_eq!(agreed, vec!["picture", "input", "events", "calls"]);
    // a new client against an instance without media: the media capabilities only shrink the feature set
    let agreed = client::negotiate(1, 1, &client::cap::SERVER_MEDIA.iter().map(|s| s.to_string()).chain(["picture".to_string()]).collect::<Vec<_>>(), &[], client::cap::SERVER).unwrap();
    assert_eq!(agreed, vec!["picture"]);
    // and one the instance has never heard of is a refusal that names it
    let e = client::negotiate(1, 1, &[], &["audio.surround".into()], &server).unwrap_err();
    assert_eq!(e.code, ErrorCode::CapabilityMissing);
}

#[test]
fn backward_the_added_capabilities_keep_their_shape_and_stay_hard_asks() {
    for l in read("capability-catalogue-added.txt").lines() {
        let c: Vec<&str> = l.split('\t').collect();
        let d = appcaps::lookup(c[0]).unwrap_or_else(|| panic!("{} was removed or renamed without an alias", c[0]));
        assert_eq!(format!("{:?}", d.param), c[1]);
        assert_eq!(format!("{:?}", d.builtin), c[2]);
        assert!(c[3] != "true" || d.hard_ask, "{}: a hard ask became a soft one", c[0]);
        assert!(d.hard_ask, "{} must stay a hard ask", c[0]);
    }
}

#[test]
fn backward_the_recorded_binary_payloads_decode_and_the_stream_tags_never_change() {
    assert_eq!((client::STREAM_PICTURE, client::STREAM_AUDIO, client::STREAM_CAMERA, client::STREAM_MIC_IN, client::STREAM_CAMERA_IN), (1, 2, 3, 4, 5));
    for l in read("binary-frames.txt").lines() {
        let (kind, h) = l.split_once('\t').unwrap();
        let b = unhex(h);
        let (tag, payload) = client::split_binary(&b).unwrap();
        match kind {
            "audio" => {
                assert_eq!(tag, client::STREAM_AUDIO);
                let (seq, pcm) = media::decode_audio(payload, 2).unwrap();
                assert_eq!((seq, pcm.len(), pcm[0], pcm[7]), (7, 8, -3000, 4000));
            }
            "camera" => {
                assert_eq!(tag, client::STREAM_CAMERA);
                let (unit, seq, jpeg) = media::decode_camera(payload).unwrap();
                assert_eq!((unit.as_str(), seq), ("usb:video0", 9));
                assert!(media::looks_like_jpeg(jpeg));
            }
            "mic_in" => assert_eq!((tag, payload), (client::STREAM_MIC_IN, &[1u8, 0, 2, 0][..])),
            "camera_in" => assert!(tag == client::STREAM_CAMERA_IN && media::looks_like_jpeg(payload)),
            other => panic!("{other}"),
        }
    }
}

#[test]
fn the_clipboard_message_may_be_longer_than_other_text_but_nothing_else_may() {
    let big = "a".repeat(media::MAX_CLIPBOARD);
    let put = encode_client(&ClientMsg::ClipboardPut { text: big.clone(), confirmed: true });
    assert!(put.len() > client::MAX_TEXT, "a 64 KB clipboard does not fit the ordinary limit");
    assert!(decode_client(&put).is_ok());
    // escaped control characters are six bytes each in JSON: the worst case still fits
    let worst = encode_client(&ClientMsg::ClipboardPut { text: "\u{1}".repeat(media::MAX_CLIPBOARD), confirmed: true });
    assert!(worst.len() <= client::MAX_CLIPBOARD_MESSAGE && decode_client(&worst).is_ok());
    // any other long message is still refused; so is a clipboard far past its cap
    assert_eq!(decode_client(&encode_client(&ClientMsg::Input { msg: "x".repeat(client::MAX_TEXT) })).unwrap_err().code, ErrorCode::TooBig);
    assert_eq!(decode_client(&format!(r#"{{"t":"clipboard_put","text":"{}"}}"#, "a".repeat(client::MAX_CLIPBOARD_MESSAGE))).unwrap_err().code, ErrorCode::TooBig);
    let text = encode_server(&ServerMsg::ClipboardText { text: big });
    assert!(decode_server(&text).is_ok());
}

#[test]
fn a_debug_print_of_a_clipboard_or_an_offer_never_shows_the_text() {
    assert_eq!(format!("{:?}", ClientMsg::ClipboardPut { text: "hunter2".into(), confirmed: true }), "ClipboardPut(7 bytes)");
}
