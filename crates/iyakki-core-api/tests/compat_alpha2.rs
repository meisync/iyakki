//! Charter step 3, part 1 (docs/development/CORE-COMPATIBILITY-DESIGN.md section 8, items 2 and 3): the golden fixtures of release alpha.2 that
//! belong to the core interface crate, and the backward and forward tests over them. The data stores, the approvals, access log and
//! views files are in `iyakki-agent` (`compat_tests`), the extension snapshot is in `iyakki-webview` (`tests/compat_alpha2.rs`).
//!
//! Fixtures: `tests/compat/alpha2/` (all synthetic: fixed ids, keys, times; no real token, password or key).
//!   * `client-session.jsonl`        a recorded transcript of the client session protocol, one `{"dir":"c"|"s","msg":{..}}` per line
//!   * `contract-hello.json`         what a machine answers to the typed contract `hello`
//!   * `capability-catalogue.txt`    the capability names with parameter kind, built-in default and hard-ask flag
//!   * `app.json`                    an app manifest with a `capabilities` list
//!
//! Re-record (only when the release folder is being made, never to make a failing test pass): `IYAKKI_COMPAT_RECORD=1 cargo test -p
//! iyakki-core-api --test compat_alpha2 record -- --ignored`.

use std::path::{Path, PathBuf};

use iyakki_core_api::appcaps::{self, Approval, CapabilityRequest};
use iyakki_core_api::client::{self, decode_client, decode_server, encode_client, encode_server, ClientMsg, ErrorCode, ServerMsg};
use iyakki_core_api::contract::{catalog, hello};
use iyakki_core_api::identity::Capabilities;
use serde_json::{json, Value};

fn dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/compat/alpha2")
}

fn read(name: &str) -> String {
    std::fs::read_to_string(dir().join(name)).unwrap_or_else(|e| panic!("tests/compat/alpha2/{name}: {e}"))
}

fn s(v: &[&str]) -> Vec<String> {
    v.iter().map(|x| x.to_string()).collect()
}

/// The transcript as the code writes it today: hello, enrol, auth, sign in, calls (allowed and refused), ping, bye, and the refusals
/// a too-new and a too-old client get.
fn transcript() -> Vec<(char, String)> {
    let key = "ab".repeat(32);
    let c = |m: ClientMsg| ('c', encode_client(&m));
    let sv = |m: ServerMsg| ('s', encode_server(&m));
    let agreed = client::negotiate(1, 1, &s(&["picture", "input", "events", "calls"]), &s(&["calls"]), client::cap::SERVER).unwrap();
    let too_new = client::negotiate(9, 8, &[], &[], client::cap::SERVER).unwrap_err();
    let too_old = client::negotiate(0, 0, &[], &[], client::cap::SERVER);
    let missing = client::negotiate(1, 1, &[], &s(&["hologram"]), client::cap::SERVER).unwrap_err();
    let mut v = vec![
        c(ClientMsg::Hello { v: 1, min_v: 1, offered: s(&["picture", "input", "events", "calls"]), needed: s(&["calls"]), client: "compat-fixture".into() }),
        sv(ServerMsg::HelloOk { v: client::PROTOCOL_VERSION, caps: agreed, challenge: "00112233445566778899aabbccddeeff".into(), tile: 64, max_message: client::MAX_TEXT, deprecations: vec![] }),
        c(ClientMsg::Enrol { token: "synthetic-one-time-token".into(), public: key, name: "Fixture laptop".into() }),
        sv(ServerMsg::Enrolled { device: "dev-000001".into(), challenge: "ffeeddccbbaa99887766554433221100".into() }),
        c(ClientMsg::Auth { authorization: "IYAKKI-SIG synthetic-signature".into() }),
        sv(ServerMsg::Authed { device: "dev-000001".into(), name: "Fixture laptop".into() }),
        c(ClientMsg::SignIn { user: "amy".into(), password: "synthetic-password-not-real".into() }),
        sv(ServerMsg::SignedIn { person: "amy".into() }),
        c(ClientMsg::Call { id: 1, capability: "camera.view".into(), unit: Some("front".into()) }),
        sv(ServerMsg::CallOk { id: 1, detail: String::new() }),
        c(ClientMsg::Call { id: 2, capability: "camera.record".into(), unit: None }),
        sv(ServerMsg::CallErr { id: 2, code: "not_granted".into(), message: "this app was not given that access".into() }),
        c(ClientMsg::PictureOpen { deflate: true }),
        sv(ServerMsg::PictureOk),
        c(ClientMsg::Resume { last_seq: 0 }),
        sv(ServerMsg::ResumeOk { from: 1, count: 1 }),
        sv(ServerMsg::Event { seq: 1, topic: "notify".into(), body: "hello".into() }),
        c(ClientMsg::Ping { ts: 5 }),
        sv(ServerMsg::Pong { ts: 5 }),
        sv(ServerMsg::Error { code: too_new.code, message: too_new.message }),
        sv(ServerMsg::Error { code: missing.code, message: missing.message }),
        c(ClientMsg::Bye),
        sv(ServerMsg::Bye { reason: "closed".into() }),
    ];
    assert!(too_old.is_err());
    v.shrink_to_fit();
    v
}

fn line((dir, text): &(char, String)) -> String {
    format!("{}\n", json!({"dir": dir.to_string(), "msg": serde_json::from_str::<Value>(text).unwrap()}))
}

/// Writes the fixtures of this crate. Ignored: run by hand when a release folder is made.
#[test]
#[ignore]
fn record() {
    assert!(std::env::var_os("IYAKKI_COMPAT_RECORD").is_some(), "set IYAKKI_COMPAT_RECORD=1");
    std::fs::create_dir_all(dir()).unwrap();
    std::fs::write(dir().join("client-session.jsonl"), transcript().iter().map(line).collect::<String>()).unwrap();
    let h = catalog::hello_for("fixture-machine", "alpha.2", Capabilities { audio: true, display: true, camera: true, keyboard: true, battery: false, bluetooth: true });
    std::fs::write(dir().join("contract-hello.json"), serde_json::to_string_pretty(&h).unwrap() + "\n").unwrap();
    let cat: String = appcaps::CATALOGUE.iter().map(|d| format!("{}\t{:?}\t{:?}\t{}\n", d.name, d.param, d.builtin, d.hard_ask)).collect();
    std::fs::write(dir().join("capability-catalogue.txt"), cat).unwrap();
    let app = json!({
        "id": "fixture-camera-notes",
        "name": "Fixture camera notes",
        "version": "1.2.0",
        "icon": "\u{1F4F7}",
        "group": "Media",
        "capabilities": [
            {"name": "camera.view", "why": "show the live view", "param": "front"},
            {"name": "notify", "why": "leave a note when the picture changes"},
            {"name": "files.read", "why": "open a picture you pick", "param": "/home/amy/Pictures"}
        ]
    });
    std::fs::write(dir().join("app.json"), serde_json::to_string_pretty(&app).unwrap() + "\n").unwrap();
}

// ------------------------------------------------------------------------------------------------ client session protocol

fn recorded() -> Vec<(char, Value)> {
    read("client-session.jsonl")
        .lines()
        .map(|l| {
            let v: Value = serde_json::from_str(l).unwrap();
            (v["dir"].as_str().unwrap().chars().next().unwrap(), v["msg"].clone())
        })
        .collect()
}

#[test]
fn backward_the_recorded_client_session_is_read_and_written_back_byte_for_byte() {
    let all = recorded();
    assert!(all.len() >= 20, "the transcript is not short");
    for (dir, msg) in &all {
        let text = msg.to_string();
        // serde_json::Value orders keys alphabetically, so compare as values, and the typed round trip as text of the typed value
        match dir {
            'c' => {
                let m = decode_client(&text).unwrap_or_else(|e| panic!("client message no longer read: {text}: {e}"));
                assert_eq!(serde_json::from_str::<Value>(&encode_client(&m)).unwrap(), *msg, "re-encoded differently: {text}");
            }
            _ => {
                let m = decode_server(&text).unwrap_or_else(|e| panic!("server message no longer read: {text}: {e}"));
                assert_eq!(serde_json::from_str::<Value>(&encode_server(&m)).unwrap(), *msg, "re-encoded differently: {text}");
            }
        }
    }
}

#[test]
fn backward_the_recorded_hello_still_negotiates_to_the_same_capabilities_and_the_refusals_keep_their_codes() {
    let all = recorded();
    let ClientMsg::Hello { v, min_v, offered, needed, .. } = decode_client(&all[0].1.to_string()).unwrap() else { panic!("first message is a hello") };
    let agreed = client::negotiate(v, min_v, &offered, &needed, client::cap::SERVER).unwrap();
    let ServerMsg::HelloOk { caps, v: answered, .. } = decode_server(&all[1].1.to_string()).unwrap() else { panic!("second message is hello_ok") };
    assert_eq!(agreed, caps);
    assert!(answered >= client::MIN_PROTOCOL_VERSION && answered <= client::PROTOCOL_VERSION);
    let codes: Vec<&str> = all.iter().filter(|(d, m)| *d == 's' && m["t"] == "error").map(|(_, m)| m["code"].as_str().unwrap()).collect();
    assert_eq!(codes, vec!["version_mismatch", "capability_missing"]);
    for c in codes {
        assert!(serde_json::from_value::<ErrorCode>(json!(c)).is_ok(), "{c} is no longer an error code");
    }
}

#[test]
fn forward_unknown_fields_in_every_recorded_message_are_ignored_and_change_nothing() {
    for (dir, msg) in recorded() {
        let mut richer = msg.clone();
        richer.as_object_mut().unwrap().insert("added_in_a_newer_release".into(), json!({"x": [1, 2, 3]}));
        let (a, b) = if dir == 'c' { (decode_client(&msg.to_string()).map(|m| encode_client(&m)), decode_client(&richer.to_string()).map(|m| encode_client(&m))) } else { (decode_server(&msg.to_string()).map(|m| encode_server(&m)), decode_server(&richer.to_string()).map(|m| encode_server(&m))) };
        assert_eq!(a.unwrap(), b.unwrap_or_else(|e| panic!("an unknown field broke {msg}: {e}")));
    }
}

#[test]
fn forward_a_message_type_this_release_does_not_know_is_a_bad_message_value_never_a_panic_or_a_close_by_itself() {
    // The decoder returns a value the caller can report and skip; open message enums (an Unknown case) are the charter's target and are
    // finding F-5 in its section 9.
    for text in [r#"{"t":"hologram","id":1}"#, r#"{"t":"call","id":"not a number","capability":"x"}"#, r#"{"no_tag":true}"#, "[1,2]", "not json", ""] {
        assert_eq!(decode_client(text).unwrap_err().code, ErrorCode::BadMessage, "{text}");
        assert_eq!(decode_server(text).unwrap_err().code, ErrorCode::BadMessage, "{text}");
    }
    let huge = "x".repeat(client::MAX_TEXT + 1);
    assert_eq!(decode_client(&huge).unwrap_err().code, ErrorCode::TooBig);
}

#[test]
fn a_version_mismatch_names_which_side_must_update() {
    let newer_client = client::negotiate(client::PROTOCOL_VERSION + 5, client::PROTOCOL_VERSION + 4, &[], &[], client::cap::SERVER).unwrap_err();
    assert_eq!(newer_client.code, ErrorCode::VersionMismatch);
    assert!(newer_client.message.contains("update this IYAKKI instance"), "{}", newer_client.message);
    assert!(newer_client.message.contains(&client::PROTOCOL_VERSION.to_string()), "it names what the instance speaks");
    if client::MIN_PROTOCOL_VERSION > 0 {
        let older = client::negotiate(client::MIN_PROTOCOL_VERSION - 1, 0, &[], &[], client::cap::SERVER).unwrap_err();
        assert!(older.message.contains("update the client"), "{}", older.message);
    }
    let missing = client::negotiate(1, 1, &[], &s(&["hologram", "teleport"]), client::cap::SERVER).unwrap_err();
    assert_eq!(missing.code, ErrorCode::CapabilityMissing);
    assert!(missing.message.contains("hologram") && missing.message.contains("teleport"), "the refusal lists what is missing");
    // an offered capability the instance lacks only shrinks the feature set
    let ok = client::negotiate(1, 1, &s(&["hologram", "picture"]), &[], client::cap::SERVER).unwrap();
    assert_eq!(ok, s(&["picture"]));
}

// ------------------------------------------------------------------------------------------------ typed contract hello

#[test]
fn backward_the_recorded_contract_hello_is_read_and_nothing_it_offered_has_gone() {
    let old: hello::Hello = serde_json::from_str(&read("contract-hello.json")).unwrap();
    let now = catalog::hello_for("fixture-machine", "alpha.2", old.capabilities);
    assert!(now.contract >= old.contract, "the contract version never goes down");
    assert!(now.oldest_minor <= now.contract.minor);
    for d in &old.domains {
        let n = now.domains.iter().find(|x| x.domain == d.domain).unwrap_or_else(|| panic!("domain {} is gone", d.domain));
        for call in &d.calls {
            assert!(n.calls.contains(call), "{}.{call} is gone", d.domain);
        }
        for ev in &d.events {
            assert!(n.events.contains(ev), "event {}.{ev} is gone", d.domain);
        }
        assert!(n.since_minor <= d.since_minor, "{} first appeared in a later minor than recorded", d.domain);
    }
    // an unknown field in a newer machine's hello is ignored
    let mut richer: Value = serde_json::from_str(&read("contract-hello.json")).unwrap();
    richer["added_in_a_newer_release"] = json!(true);
    richer["domains"][0]["added_too"] = json!([1]);
    assert_eq!(serde_json::from_value::<hello::Hello>(richer).unwrap(), old);
}

#[test]
fn the_contract_negotiation_serves_the_overlap_and_refuses_the_rest_without_failing_the_client() {
    let old: hello::Hello = serde_json::from_str(&read("contract-hello.json")).unwrap();
    let wants = s(&[&old.domains[0].domain, "domain-from-the-future"]);
    let same = hello::negotiate(&old, &hello::ClientHello { contract: old.contract, wants: wants.clone() });
    assert_eq!(same.version, Some(old.contract));
    assert_eq!(same.missing, s(&["domain-from-the-future"]), "a missing domain hides a screen, it does not fail");
    let newer_minor = hello::negotiate(&old, &hello::ClientHello { contract: hello::ContractVersion { major: old.contract.major, minor: old.contract.minor + 3 }, wants: wants.clone() });
    assert_eq!(newer_minor.version, Some(old.contract), "the lower minor is agreed");
    let other_major = hello::negotiate(&old, &hello::ClientHello { contract: hello::ContractVersion { major: old.contract.major + 1, minor: 0 }, wants });
    assert_eq!(other_major.version, None, "a different major is not served (finding F-6: the refusal does not name the side to update)");
}

// ------------------------------------------------------------------------------------------------ capability catalogue and app.json

#[test]
fn backward_no_capability_name_of_alpha2_is_gone_or_changed_and_no_hard_ask_became_soft() {
    for l in read("capability-catalogue.txt").lines() {
        let c: Vec<&str> = l.split('\t').collect();
        let d = appcaps::lookup(c[0]).unwrap_or_else(|| panic!("capability {} was removed or renamed without an alias (charter rule 5 and 8)", c[0]));
        assert_eq!(format!("{:?}", d.param), c[1], "{}: the parameter kind changed: that is a new meaning, so a new name", c[0]);
        if c[3] == "true" {
            assert!(d.hard_ask, "{}: a hard ask became a soft one", c[0]);
        }
    }
}

#[test]
fn backward_the_alpha2_app_manifest_validates_and_forward_an_unknown_field_in_it_is_found_and_reported() {
    let app: Value = serde_json::from_str(&read("app.json")).unwrap();
    let list: Vec<CapabilityRequest> = serde_json::from_value(app["capabilities"].clone()).unwrap();
    appcaps::validate_requests(&list).unwrap();
    assert_eq!(list.len(), 3);
    // approve, then the card for an update that asks the same: no question
    let a = appcaps::record_approval("amy", "fixture-camera-notes", "1.2.0", &"0".repeat(64), 1_700_000_000, &list);
    assert_eq!(appcaps::plan_ask(Some(&a), &list), appcaps::AskPlan::Nothing);
    // forward (finding F-1): an entry with a field this release does not know is accepted, kept, written back and reported
    let mut richer = app["capabilities"].clone();
    richer[0]["optional"] = json!(true);
    let kept: Vec<CapabilityRequest> = serde_json::from_value(richer.clone()).unwrap();
    appcaps::validate_requests(&kept).unwrap();
    assert_eq!(appcaps::unknown_request_fields(&kept), vec![format!("{}: optional", kept[0].name)]);
    assert_eq!(serde_json::to_value(&kept).unwrap()[0]["optional"], json!(true), "the unknown field is written back");
    assert!(appcaps::unknown_request_fields(&list).is_empty());
    // an unknown capability name is refused naming the known ones (the Store shows this to the author)
    let bad = vec![CapabilityRequest { name: "camera.teleport".into(), why: "x".into(), param: None, extra: Default::default() }];
    assert!(appcaps::validate_requests(&bad).unwrap_err().to_string().contains("camera.view"));
}

#[test]
fn forward_an_approval_keeps_fields_a_newer_release_wrote() {
    let text = r#"{"who":"amy","app":"a","app_version":"1","app_sha256":"00","at":1,"grants":[{"name":"notify"}],"revoked":[],"scope_hint":{"since":"alpha.9"}}"#;
    let a: Approval = serde_json::from_str(text).unwrap();
    assert!(a.holds("notify", None));
    let back: Value = serde_json::from_str(&serde_json::to_string(&a).unwrap()).unwrap();
    assert_eq!(back["scope_hint"], json!({"since": "alpha.9"}), "an unknown field is written back");
    // and an approval of alpha.1 (no extra) writes nothing new
    let plain = appcaps::record_approval("amy", "a", "1", "00", 1, &[]);
    assert!(serde_json::to_value(plain).unwrap().get("extra").is_none());
}
