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
