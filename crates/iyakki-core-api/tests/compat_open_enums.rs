//! Compatibility charter rule 3 (finding F-5): the enums that cross a boundary are open. A value a newer release added decodes as `Unknown`
//! instead of failing the whole message, and `Unknown` is read the safest way wherever it decides something.

use iyakki_core_api::client::{self, decode_client, decode_server, ClientMsg, ServerMsg};
use iyakki_core_api::contract::{guard, CallContext, ConsentKind, ErrorCode, FlowEvent, Risk, RunState, Scope, SettingKind, SettingValue, StreamKind, Transport};

fn from<T: serde::de::DeserializeOwned>(json: &str) -> T {
    serde_json::from_str(json).unwrap_or_else(|e| panic!("{json}: {e}"))
}

#[test]
fn a_value_a_newer_release_added_decodes_as_unknown_in_every_boundary_enum() {
    assert_eq!(from::<ErrorCode>("\"quota_full\""), ErrorCode::Unknown);
    assert_eq!(from::<StreamKind>("\"thermal_image\""), StreamKind::Unknown);
    assert_eq!(from::<RunState>("\"paused\""), RunState::Unknown);
    assert_eq!(from::<ConsentKind>("\"payment\""), ConsentKind::Unknown);
    assert_eq!(from::<Transport>("\"carrier_pigeon\""), Transport::Unknown);
    assert_eq!(from::<Risk>("\"catastrophic\""), Risk::Unknown);
    assert_eq!(from::<SettingKind>(r#"{"type":"colour","palette":[1,2]}"#), SettingKind::Unknown);
    assert_eq!(from::<FlowEvent>(r#"{"type":"checkpoint","run":"r1","step":3}"#), FlowEvent::Unknown);
    // the values that exist still decode as themselves
    assert_eq!(from::<ErrorCode>("\"denied\""), ErrorCode::Denied);
    assert_eq!(from::<RunState>("\"running\""), RunState::Running);
    assert_eq!(from::<SettingKind>(r#"{"type":"int","min":1,"max":5}"#), SettingKind::Int { min: 1, max: 5 });
    // the known codes are all in ALL and `Unknown` is not (a release never sends it)
    assert!(!ErrorCode::ALL.contains(&ErrorCode::Unknown));
    assert_eq!(ErrorCode::Unknown.as_str(), "unknown");
}

#[test]
fn an_unknown_value_is_read_the_strict_way_where_it_decides_something() {
    // a risk this release does not know is as strict as the strictest: a remote caller is refused even with the scope
    let remote = CallContext::remote("amy", Transport::DevMode, &[Scope::SETTINGS_WRITE]);
    assert!(guard(&remote, Scope::SETTINGS_WRITE, Risk::Change).is_ok());
    let refused = guard(&remote, Scope::SETTINGS_WRITE, Risk::Unknown).unwrap_err();
    assert_eq!(refused.code, ErrorCode::Denied);
    assert!(guard(&CallContext::local("amy", false), Scope::SETTINGS_WRITE, Risk::Unknown).is_ok(), "the machine's own screen still may");
    // an unknown transport is never local
    assert!(!CallContext::remote("amy", Transport::Unknown, &[]).is_local());
    // an unknown setting kind accepts no value: it is shown, never written
    for v in [SettingValue::Bool(true), SettingValue::Int(1), SettingValue::Text("x".into())] {
        assert!(!SettingKind::Unknown.accepts(&v));
    }
    assert_eq!(Risk::Unknown.id(), "unknown");
}

#[test]
fn the_client_protocol_trio_is_open_too_and_an_unknown_message_is_never_an_error_or_a_close() {
    assert_eq!(decode_client(r#"{"t":"hologram"}"#).unwrap(), ClientMsg::Unknown);
    assert_eq!(decode_server(r#"{"t":"hologram"}"#).unwrap(), ServerMsg::Unknown);
    assert_eq!(serde_json::from_str::<client::ErrorCode>("\"quota_full\"").unwrap(), client::ErrorCode::Unknown);
    // a real message keeps decoding
    assert!(matches!(decode_client(r#"{"t":"ping","ts":5}"#).unwrap(), ClientMsg::Ping { ts: 5 }));
}
