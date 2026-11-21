use super::*;
use crate::store::sdkv2_backends::Backends;
use crate::store::tile::{AppInfo, Caps, Outbox, TileHost, TileStorage};
use crate::store::tilesdk::{FakeLlm, FakeSpeech, LogMeter, TileSdk};
use crate::usage::UsageLog;
use serde_json::json;

fn services(tag: &str) -> (Arc<WasmServices>, Arc<crate::store::sdkv2_backends::FakeBackend>) {
    let dir = std::env::temp_dir().join(format!("iyakki-wasmsvc-{}-{tag}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let out = Arc::new(Mutex::new(Outbox::default()));
    let (b, fake) = Backends::fake();
    let sdk = Arc::new(TileSdk::new("t", "tester", Arc::new(FakeLlm::default()), Arc::new(FakeSpeech::default()), Arc::new(LogMeter(UsageLog::open(&dir.join("usage.jsonl")))), out.clone(), Some(&dir.join("t.json"))).with_backends(b));
    let info = AppInfo { id: "t".into(), name: "T".into(), version: "1.0.0".into(), publisher: "acme".into(), trust: "Community".into(), dev: false };
    (WasmServices::new(TileHost::new(info, Caps::all_v2(), TileStorage::memory(), out).with_sdk(sdk)), fake)
}

#[test]
fn a_wasm_tiles_sdk_calls_go_through_the_same_bridge_as_a_page_and_other_domains_are_not_served() {
    let (s, fake) = services("bridge");
    let info: Value = serde_json::from_str(&s.call("sdk", "app.info", "").unwrap()).unwrap();
    assert_eq!(info["id"], "t");
    // a new-namespace call reaches the backend through the bridge's own checks
    let r = s.call("sdk", "flows.list", "{}").unwrap();
    assert!(r.contains("backup"), "{r}");
    assert_eq!(fake.calls_of("flows.list").len(), 1);
    // arguments the bridge refuses are refused
    assert!(s.call("sdk", "markdown.view", &json!({"text": 5}).to_string()).is_err());
    // junk and other domains
    assert!(s.call("sdk", "app.info", "{not json").is_err());
    assert!(s.call("tools", "run", "{}").unwrap_err().contains("no tools service"));
}

#[test]
fn a_first_use_card_waits_for_the_person_through_the_backend_and_an_answer_lets_the_tile_ask_again() {
    let dir = std::env::temp_dir().join(format!("iyakki-wasmsvc-{}-card", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let out = Arc::new(Mutex::new(Outbox::default()));
    let sdk = Arc::new(TileSdk::fake("t", out.clone(), &dir.join("t.json")));
    let info = AppInfo { id: "t".into(), name: "T".into(), version: "1.0.0".into(), publisher: "acme".into(), trust: "Community".into(), dev: false };
    let s = WasmServices::new(TileHost::new(info, Caps { llm: true, speech: true, ..Caps::all_v2() }, TileStorage::memory(), out).with_sdk(sdk));
    assert!(s.requests().is_empty());
    s.call("sdk", "__test.approve.set", &json!({"mode": "ask"}).to_string()).unwrap();
    let chat = json!({"messages": [{"role": "user", "content": "hi"}]}).to_string();
    let r: Value = serde_json::from_str(&s.call("sdk", "llm.chat", &chat).unwrap()).unwrap();
    let id = r["pending"].as_u64().expect("the call waits for the person") as u32;
    let waiting = s.requests();
    assert_eq!((waiting.len(), waiting[0].0, waiting[0].1.as_str()), (1, id, "sdk.approve"));
    let args: Value = serde_json::from_str(&waiting[0].2).unwrap();
    assert_eq!(args["ns"], "llm");
    // a no settles the card and nothing runs; asking again asks again
    assert!(s.answer(id, false));
    assert!(s.requests().is_empty());
    assert!(!s.answer(id, true), "an answer settles a card once");
    let r: Value = serde_json::from_str(&s.call("sdk", "llm.chat", &chat).unwrap()).unwrap();
    let id2 = r["pending"].as_u64().expect("asked again") as u32;
    // a yes is kept: the tile's next call does not ask
    assert!(s.answer(id2, true));
    assert!(s.requests().is_empty());
    let again: Value = serde_json::from_str(&s.call("sdk", "llm.chat", &chat).unwrap()).unwrap();
    assert!(again["text"].is_string(), "{again}");
}

#[test]
fn a_tile_may_ask_the_dock_to_open_an_app_the_policy_allows_once_and_nothing_else() {
    use iyakki_sec::policy::{App, Policy, PolicyOverride, Role};
    let admin = Policy::resolve(Role::Admin, &PolicyOverride::default());
    let guest = Policy::resolve(Role::Guest, &PolicyOverride::default());
    let dir = std::env::temp_dir().join(format!("iyakki-wasmsvc-{}-dock", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let mk = |policy: Option<Policy>| {
        let out = Arc::new(Mutex::new(Outbox::default()));
        let sdk = Arc::new(TileSdk::fake("t", out.clone(), &dir.join("t.json")));
        let info = AppInfo { id: "t".into(), name: "T".into(), version: "1.0.0".into(), publisher: "acme".into(), trust: "Community".into(), dev: false };
        let host = TileHost::new(info, Caps::all_v2(), TileStorage::memory(), out).with_sdk(sdk);
        match policy {
            Some(p) => WasmServices::with_dock(host, p),
            None => WasmServices::new(host),
        }
    };
    // a loader without the dock answers an error the tile reads as unavailable
    assert!(mk(None).call("dock", "open", r#"{"app":"settings"}"#).unwrap_err().contains("no dock service"));
    let s = mk(Some(admin.clone()));
    assert_eq!(s.take_dock_open(), None);
    let app = App::ALL.into_iter().find(|a| admin.allows_app(*a)).expect("one allowed app").id();
    s.call("dock", "open", &json!({"app": app}).to_string()).unwrap();
    assert_eq!(s.take_dock_open().as_deref(), Some(app));
    assert_eq!(s.take_dock_open(), None, "taken once");
    // refusals keep nothing; the other dock methods are not served here
    assert!(s.call("dock", "open", r#"{"app":"no-such-app"}"#).unwrap_err().contains("unknown app"));
    assert!(s.call("dock", "open", "{}").is_err());
    assert!(s.call("dock", "open", "not json").is_err());
    assert!(s.call("dock", "pin", &json!({"app": app}).to_string()).is_err());
    assert_eq!(s.take_dock_open(), None);
    // an app the person's policy does not allow is refused in words
    let g = mk(Some(guest.clone()));
    let denied = App::ALL.into_iter().find(|a| admin.allows_app(*a) && !guest.allows_app(*a)).expect("an app a guest may not use");
    assert!(g.call("dock", "open", &json!({"app": denied.id()}).to_string()).unwrap_err().contains("not allowed"));
    assert_eq!(g.take_dock_open(), None);
}
