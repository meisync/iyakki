//! Charter step 3, part 3 (docs/development/CORE-COMPATIBILITY-DESIGN.md section 8): the golden extension snapshot of alpha.2 and the helper
//! frame of `Ready`, and what they must keep doing. Fixtures are in `tests/compat/alpha2/` (`extension-snapshot.hex`,
//! `extension-snapshot-v9.hex`); re-record by hand with `IYAKKI_COMPAT_RECORD=1 cargo test -p iyakki-webview --test compat_alpha2 record -- --ignored`.
//!
//! Backward: this code reads what alpha.2 wrote. Forward: a hook kind nobody here knows, or one without code, is skipped and reported,
//! the rest of the snapshot still works; a newer snapshot version is refused as a whole with words that say which side to update.

use std::path::{Path, PathBuf};

use iyakki_webview::ext::{self, hook, ExtError, Entry, HeaderRule, Hook, Pat, Snapshot, Target};
use iyakki_webview::proto::{self, FromHelper};

fn dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/compat/alpha2")
}

fn unhex(s: &str) -> Vec<u8> {
    let s: String = s.split_whitespace().collect();
    (0..s.len() / 2).map(|i| u8::from_str_radix(&s[2 * i..2 * i + 2], 16).unwrap()).collect()
}

fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

fn snapshot() -> Snapshot {
    let rules = vec![
        HeaderRule { header: "X-Fixture".into(), value: "synthetic-value".into(), replace: false, domains: vec![Pat { scheme: 2, suffix: true, host: "example.com".into(), port: 0, segs: vec![] }], prefix: vec![] },
        HeaderRule { header: "X-Local".into(), value: "dev".into(), replace: true, domains: vec![Pat { scheme: 0, suffix: false, host: "localhost".into(), port: 8080, segs: vec!["api".into()] }], prefix: vec![] },
    ];
    Snapshot {
        entries: vec![
            Entry { id: "fixture.headers".into(), version: "1.0.0".into(), caps: vec!["web.request.headers".into()], hooks: vec![Hook { kind: hook::REQUEST_HEADERS, payload: ext::encode_header_rules(&rules) }] },
            Entry { id: "fixture.future".into(), version: "9.9.9".into(), caps: vec!["web.request.headers".into(), "web.content.script".into()], hooks: vec![Hook { kind: hook::CONTENT_SCRIPT, payload: vec![1, 2, 3] }, Hook { kind: 99, payload: vec![0xde, 0xad] }] },
        ],
    }
}

#[test]
#[ignore]
fn record() {
    assert!(std::env::var_os("IYAKKI_COMPAT_RECORD").is_some(), "set IYAKKI_COMPAT_RECORD=1");
    std::fs::write(dir().join("extension-snapshot.hex"), format!("{}\n", hex(&snapshot().encode().0))).unwrap();
    // a snapshot of a newer layout: version 9, no entries
    let mut v9 = 9u32.to_le_bytes().to_vec();
    v9.extend_from_slice(&0u32.to_le_bytes());
    std::fs::write(dir().join("extension-snapshot-v9.hex"), format!("{}\n", hex(&v9))).unwrap();
}

fn recorded() -> Vec<u8> {
    unhex(&std::fs::read_to_string(dir().join("extension-snapshot.hex")).expect("tests/compat/alpha2/extension-snapshot.hex"))
}

#[test]
fn backward_the_alpha2_snapshot_decodes_and_encodes_to_the_same_bytes() {
    let bytes = recorded();
    let s = Snapshot::decode(&bytes).unwrap();
    assert_eq!(s, snapshot());
    assert_eq!(s.encode().0, bytes, "the writer still produces the recorded layout");
    assert_eq!(u32::from_le_bytes(bytes[0..4].try_into().unwrap()), ext::SNAPSHOT_VERSION.min(1), "the recorded version is 1");
}

#[test]
fn backward_the_recorded_header_rules_still_add_the_same_headers() {
    let d = Snapshot::decode(&recorded()).unwrap().dispatch();
    assert_eq!(d.header_rules.len(), 2);
    let t = |https: bool, host: &str, port: u16, segs: &[&str]| Target { https, host: host.into(), port, segs: segs.iter().map(|s| s.to_string()).collect(), ambiguous: false };
    let h = ext::headers_for(&d.header_rules, &t(true, "www.example.com", 443, &[]));
    assert_eq!(h, vec![("X-Fixture", "synthetic-value", false)]);
    assert!(ext::headers_for(&d.header_rules, &t(true, "example.org", 443, &[])).is_empty());
    let h = ext::headers_for(&d.header_rules, &t(false, "localhost", 8080, &["api", "v1"]));
    assert_eq!(h, vec![("X-Local", "dev", true)]);
}

#[test]
fn forward_hook_kinds_nobody_here_knows_or_has_code_for_are_skipped_and_reported_the_rest_works() {
    let d = Snapshot::decode(&recorded()).unwrap().dispatch();
    assert_eq!(d.header_rules.len(), 2, "the known hook still works next to the unknown ones");
    assert_eq!(d.report.len(), 2, "{:?}", d.report);
    assert!(d.report.iter().any(|r| r.contains("fixture.future") && r.contains("content.script") && r.contains("not implemented")));
    assert!(d.report.iter().any(|r| r.contains("hook kind 99 is unknown, ignored")));
    assert!(d.report.iter().all(|r| !r.contains("synthetic-value")), "a report never shows a header value");
}

#[test]
fn the_hook_kind_numbers_names_and_capabilities_are_frozen() {
    let want = [(1, "request.headers", "web.request.headers"), (2, "response.headers", "web.response.headers"), (3, "request.block", "web.request.block"), (4, "content.script", "web.content.script"), (5, "page.action", "web.page.action"), (6, "user.agent", "web.user.agent")];
    for (k, name, cap) in want {
        assert_eq!(hook::name(k), Some(name));
        assert_eq!(hook::capability(k), Some(cap));
    }
    assert_eq!(hook::name(7), None);
}

#[test]
fn a_snapshot_of_a_newer_layout_is_refused_as_a_whole_and_the_words_say_who_updates() {
    let bytes = unhex(&std::fs::read_to_string(dir().join("extension-snapshot-v9.hex")).unwrap());
    let e = Snapshot::decode(&bytes).unwrap_err();
    assert_eq!(e, ExtError::Version(9));
    let words = e.to_string();
    assert!(words.contains("newer") && words.contains("update"), "{words}");
    // nothing partial: truncation of the good snapshot is an error, never a half snapshot
    let good = recorded();
    for cut in [1, 5, good.len() / 2, good.len() - 1] {
        assert!(Snapshot::decode(&good[..cut]).is_err(), "cut at {cut}");
    }
}

fn frames() -> Vec<(&'static str, Vec<u8>)> {
    let hello = proto::Hello { profile: "/profile".into(), locale: "en-US".into(), ..proto::Hello::default() };
    vec![
        ("to_helper_hello", proto::ToHelper::Hello(hello).encode()),
        ("to_helper_navigate", proto::ToHelper::Navigate("https://example.com/".into()).encode()),
        ("from_helper_ready", FromHelper::Ready { version: proto::VERSION, engine: "fixture".into(), engine_version: "1".into() }.encode()),
        ("from_helper_title", FromHelper::Title("A title".into()).encode()),
    ]
}

#[test]
#[ignore]
fn record_frames() {
    assert!(std::env::var_os("IYAKKI_COMPAT_RECORD").is_some(), "set IYAKKI_COMPAT_RECORD=1");
    std::fs::write(dir().join("helper-frames.txt"), frames().iter().map(|(n, b)| format!("{n}\t{}\n", hex(b))).collect::<String>()).unwrap();
}

#[test]
fn backward_the_recorded_helper_frames_are_read_and_written_the_same() {
    let text = std::fs::read_to_string(dir().join("helper-frames.txt")).expect("tests/compat/alpha2/helper-frames.txt");
    let now = frames();
    for l in text.lines() {
        let (name, h) = l.split_once('\t').unwrap();
        let bytes = unhex(h);
        let ours = &now.iter().find(|(n, _)| *n == name).unwrap_or_else(|| panic!("frame {name} is no longer produced")).1;
        assert_eq!(ours, &bytes, "{name}: the writer changed the layout");
        let (ty, payload) = proto::read_frame(&mut std::io::Cursor::new(bytes)).unwrap().unwrap();
        if name.starts_with("to_") {
            assert!(proto::ToHelper::decode(ty, &payload).is_ok(), "{name}");
        } else {
            assert!(FromHelper::decode(ty, &payload).is_ok(), "{name}");
        }
    }
}

#[test]
fn forward_a_frame_of_a_type_nobody_knows_is_an_error_value_the_reader_skips_and_extra_bytes_are_ignored() {
    // WEBVIEW-PROTOCOL section 2: unknown types are skipped, bytes after the last known field are ignored (room to add fields)
    assert!(FromHelper::decode(0xEE, &[1, 2, 3]).is_err());
    assert!(proto::ToHelper::decode(0x7E, &[1, 2, 3]).is_err());
    let (ty, mut payload) = proto::read_frame(&mut std::io::Cursor::new(FromHelper::Title("A title".into()).encode())).unwrap().unwrap();
    payload.extend_from_slice(&[9, 9, 9, 9]);
    assert_eq!(FromHelper::decode(ty, &payload).unwrap(), FromHelper::Title("A title".into()));
}
