//! F-CORE-19 (charter step 4): both `hello` answers carry `deprecations`, `negotiate` names the side that must update, and the compat
//! report says which versions met. Old transcripts and fixtures must still be read.

use iyakki_core_api::client::{self, ServerMsg};
use iyakki_core_api::compat::{self, Deprecation, UpdateSide};
use iyakki_core_api::contract::catalog::hello_for;
use iyakki_core_api::contract::hello::{negotiate, ClientHello, ContractVersion, Hello};
use iyakki_core_api::identity::Capabilities;

fn caps() -> Capabilities {
    serde_json::from_str("{}").unwrap_or_default()
}

fn machine() -> Hello {
    hello_for("box", "alpha.3", caps())
}

fn client_at(major: u16, minor: u16, wants: &[&str]) -> ClientHello {
    ClientHello { contract: ContractVersion { major, minor }, wants: wants.iter().map(|s| s.to_string()).collect() }
}

#[test]
fn the_refusal_names_the_side_that_must_update_for_every_way_two_versions_can_fail_to_meet() {
    let m = machine();
    let (maj, min) = (m.contract.major, m.contract.minor);
    // all well: nobody updates
    let ok = negotiate(&m, &client_at(maj, min, &[]));
    assert_eq!((ok.version.is_some(), ok.update), (true, None));
    assert!(ok.say(&m, &client_at(maj, min, &[])).is_none());
    // a newer major on the client: the machine is the older one
    let newer = client_at(maj + 1, 0, &[]);
    let n = negotiate(&m, &newer);
    assert_eq!((n.version, n.update), (None, Some(UpdateSide::Machine)));
    assert!(n.say(&m, &newer).unwrap().contains("update this machine"), "{:?}", n.say(&m, &newer));
    // a client older than the minor the machine still serves: the client updates
    let mut strict = machine();
    strict.oldest_minor = min; // pretend the machine dropped everything below its own minor
    if min > 0 {
        let old = client_at(maj, min - 1, &[]);
        let n = negotiate(&strict, &old);
        assert_eq!((n.version, n.update), (None, Some(UpdateSide::Client)));
        assert!(n.say(&strict, &old).unwrap().contains("update the client"));
    }
    // an older major on the client (only possible once the machine is at major 1; shown with a made-up machine)
    let mut m1 = machine();
    m1.contract = ContractVersion { major: maj + 1, minor: 0 };
    m1.oldest_minor = 0;
    let n = negotiate(&m1, &client_at(maj, 5, &[]));
    assert_eq!((n.version, n.update), (None, Some(UpdateSide::Client)));
    // a newer client that wants a domain only a newer minor has: the machine updates, the client still gets the calls it can use
    let wants = client_at(maj, min + 2, &["a-domain-from-the-future"]);
    let n = negotiate(&m, &wants);
    assert_eq!(n.version, Some(ContractVersion { major: maj, minor: min }), "the lower minor is agreed");
    assert_eq!(n.update, Some(UpdateSide::Machine));
    assert!(n.say(&m, &wants).unwrap().contains("a-domain-from-the-future"));
}

#[test]
fn the_typed_hello_carries_deprecations_and_an_old_hello_without_them_is_still_read() {
    let mut m = machine();
    assert!(m.deprecations.iter().any(|d| d.name == "stream.chunk.data_as_numbers"), "the table feeds the hello");
    m.deprecations.clear();
    // an empty list is not written at all, so the recorded fixture still matches byte for byte in shape
    let text = serde_json::to_string(&m).unwrap();
    assert!(!text.contains("deprecations"), "{text}");
    // a hello without the field (every release before this one) reads, with an empty list
    let back: Hello = serde_json::from_str(&text).unwrap();
    assert_eq!(back, m);
    // with a deprecation it is written, read back and shown
    m.deprecations = vec![Deprecation { name: "camera.view".into(), kind: "capability".into(), since: "0.1.0-alpha.3".into(), not_before: "2027-02-01".into(), replacement: "camera.watch".into() }];
    let text = serde_json::to_string(&m).unwrap();
    assert!(text.contains("\"deprecations\"") && text.contains("camera.view"));
    assert_eq!(serde_json::from_str::<Hello>(&text).unwrap().deprecations[0].replacement, "camera.watch");
    // a field this release does not know inside a deprecation is ignored (unknown is data)
    let richer = text.replace("\"replacement\"", "\"added_later\":1,\"replacement\"");
    assert_eq!(serde_json::from_str::<Hello>(&richer).unwrap().deprecations.len(), 1);
}

#[test]
fn the_client_session_hello_ok_lists_deprecated_names_only_when_there_are_some_and_old_ones_still_parse() {
    let none = ServerMsg::HelloOk { v: 1, caps: vec!["picture".into()], challenge: "ff".into(), tile: 64, max_message: 10, deprecations: vec![] };
    let line = client::encode_server(&none);
    assert!(!line.contains("deprecations"), "{line}");
    // the same line parses to the same message (a line recorded before the field existed)
    assert_eq!(client::decode_server(&line).unwrap(), none);
    let some = ServerMsg::HelloOk { v: 1, caps: vec![], challenge: "ff".into(), tile: 64, max_message: 10, deprecations: vec!["camera.view".into()] };
    let line = client::encode_server(&some);
    assert!(line.contains("\"deprecations\":[\"camera.view\"]"), "{line}");
    assert_eq!(client::decode_server(&line).unwrap(), some);
    // an older client reading a hello_ok with the new field: `serde` ignores what it does not know, shown with a stand-in of the old shape
    #[derive(serde::Deserialize)]
    #[allow(dead_code)]
    #[serde(tag = "t", rename_all = "snake_case")]
    enum Old {
        HelloOk { v: u32, caps: Vec<String>, challenge: String, tile: u32, max_message: usize },
    }
    assert!(serde_json::from_str::<Old>(&line).is_ok(), "an older reader ignores the new field");
}

#[test]
fn the_committed_tables_feed_hello_and_the_report() {
    // the live list is the table the gate reads: empty today, and each row it ever has is announced
    assert_eq!(compat::deprecations(), compat::parse_deprecations(compat::DEPRECATION_TABLE));
    assert_eq!(machine().deprecations, compat::deprecations());
    let r = compat::Report::new("0.1.0-alpha.3", vec![], vec![]);
    assert!(r.surfaces.len() > 30, "{}", r.surfaces.len());
    assert!(r.contract.starts_with("0.2 (serves from 0.1)"), "{}", r.contract);
    assert_eq!(r.client_protocol, format!("{} to {}", client::MIN_PROTOCOL_VERSION, client::PROTOCOL_VERSION));
}

// ---- the contract 0.2 hello as a golden fixture (F-CORE-18): a version bump needs a fixture folder entry (tests/compat/README.md)

fn hello_fixture() -> std::path::PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/compat/alpha3/contract-hello-0.2.json")
}

/// Writes `tests/compat/alpha3/contract-hello-0.2.json` (once, when the release is cut; never edited after).
#[test]
#[ignore]
fn record_alpha3_contract_hello_0_2() {
    if std::env::var("IYAKKI_COMPAT_RECORD").is_ok() {
        let mut h = machine();
        h.deprecations.clear();
        std::fs::write(hello_fixture(), serde_json::to_string_pretty(&h).unwrap() + "\n").unwrap();
    }
}

#[test]
fn the_contract_0_2_hello_of_alpha3_is_still_read_and_a_0_1_client_still_negotiates_with_it() {
    let text = std::fs::read_to_string(hello_fixture()).unwrap();
    let h: Hello = serde_json::from_str(&text).unwrap();
    assert_eq!((h.contract.major, h.contract.minor, h.oldest_minor), (0, 2, 1));
    // a client of contract 0.1 (every release before this one) still gets calls, at its own minor
    let old = client_at(0, 1, &["settings"]);
    let n = negotiate(&h, &old);
    assert_eq!(n.version.map(|v| (v.major, v.minor)), Some((0, 1)));
    // a client of 0.2 gets 0.2; a field a newer release adds to the hello is kept as unknown, not an error
    let mut v: serde_json::Value = serde_json::from_str(&text).unwrap();
    v["a_field_of_the_future"] = serde_json::json!(true);
    assert!(serde_json::from_value::<Hello>(v).is_ok());
    assert_eq!(negotiate(&h, &client_at(0, 2, &["settings"])).version.map(|v| (v.major, v.minor)), Some((0, 2)));
}
