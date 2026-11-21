//! The producers behind `bulk.get` (F-SPLIT-07): the display list of an SDK app the core runs and the newest picture of a camera view the person
//! agreed to, handed over by name; the scopes decide; and over the real unix socket of the real core as a memory file beside the answer.

use std::sync::Arc;

use iyakki_app_sample::SampleApp;
use iyakki_compose::frames::{FrameBulk, HostThread, DISPLAY_VIEW};
use iyakki_core_api::cameras::{CamFrame, Cameras, FrameFormat};
use iyakki_core_api::contract::{CallContext, ErrorCode, Transport};
use iyakki_ui_client::server::BulkSource;
use iyakki_ui_sdk::desktop::{Arrangement, MemDock, MemWallpaper};
use iyakki_ui_sdk::host::Host;
use iyakki_ui_sdk::list::DisplayList;
use iyakki_uikit::look::Look;

fn sample(w: u32, h: u32) -> HostThread {
    HostThread::spawn(move || Host::new(SampleApp::new(), Look::default(), Arrangement::single(w, h), Box::new(MemDock::default()), Box::new(MemWallpaper::default())))
}

fn ctx(user: &str, scopes: &[&str]) -> CallContext {
    CallContext::remote(user, Transport::LocalSocket, scopes)
}

/// A core whose one view, `v1` of `ann`, has one camera with one picture; every other view, person or camera has none.
struct OneView;

impl Cameras for OneView {
    fn frame(&self, user: &str, view: &str, camera: &str, after: u64, format: FrameFormat) -> Result<Option<CamFrame>, String> {
        if (user, view, camera, after, format) == ("ann", "v1", "front", 0, FrameFormat::Jpeg) {
            return Ok(Some(CamFrame { seq: 7, width: 2, height: 2, format, data: Arc::new(vec![0xff, 0xd8, 1, 2, 3, 0xff, 0xd9]) }));
        }
        Ok(None)
    }
}

#[test]
fn an_app_the_core_runs_hands_over_its_display_list_as_text_a_client_can_draw_and_only_to_a_caller_with_the_scope() {
    let frames = FrameBulk::new();
    frames.add_app("sample", sample(400, 300)).unwrap();
    assert_eq!(frames.names(), vec!["display.sample".to_string()]);
    let ok = ctx("ui", &[DISPLAY_VIEW]);
    let a = frames.get(&ok, "display.sample").unwrap();
    let list = DisplayList::parse(std::str::from_utf8(&a).unwrap()).expect("the block is the display list text a client parses");
    assert_eq!((list.width, list.height), (400.0, 300.0), "drawn at the pane of the host");
    assert!(list.ops.len() > 5, "the app drew something: {} ops", list.ops.len());
    assert_eq!(frames.get(&ok, "display.sample").unwrap(), a, "the same app, the same moment: the same frame");
    // the pane changes size: the next frame is drawn at it
    let thread = sample(400, 300);
    thread.resize(320.0, 240.0).unwrap();
    let l2 = DisplayList::parse(&thread.frame().unwrap()).unwrap();
    assert_eq!((l2.width, l2.height), (320.0, 240.0));
    // a caller without the scope is refused before the name is looked at
    assert_eq!(frames.get(&ctx("ui", &["settings:read"]), "display.sample").unwrap_err().code, ErrorCode::Denied);
    assert_eq!(frames.get(&ctx("ui", &["settings:read"]), "display.no-such").unwrap_err().code, ErrorCode::Denied, "no hint whether the app exists");
    assert_eq!(frames.get(&ok, "display.no-such").unwrap_err().code, ErrorCode::NotFound);
    assert_eq!(frames.get(&ok, "settings.all").unwrap_err().code, ErrorCode::NotFound, "not a block of this source");
    // names that cannot be served
    assert!(frames.add_app("", sample(10, 10)).is_err());
    assert!(frames.add_app("a/b", sample(10, 10)).is_err());
    assert!(frames.add_app(&"x".repeat(33), sample(10, 10)).is_err());
}

#[test]
fn a_camera_picture_is_handed_over_only_for_a_view_the_caller_holds_and_every_miss_looks_the_same() {
    let frames = FrameBulk::new().with_cameras(Arc::new(OneView));
    let ann = ctx("ann", &[iyakki_ui_client::facets::CAMERAS_VIEW]);
    assert_eq!(frames.get(&ann, "camera.v1.front").unwrap(), vec![0xff, 0xd8, 1, 2, 3, 0xff, 0xd9], "the JPEG of the newest picture, as it is");
    // another person, another view, another camera, a name with no camera in it: one answer for all, nothing about why
    for (who, name) in [("bob", "camera.v1.front"), ("ann", "camera.v2.front"), ("ann", "camera.v1.back"), ("ann", "camera.v1")] {
        let c = ctx(who, &[iyakki_ui_client::facets::CAMERAS_VIEW]);
        assert_eq!(frames.get(&c, name).unwrap_err().code, ErrorCode::NotFound, "{who} {name}");
    }
    // the scope of the cameras, not of the settings
    assert_eq!(frames.get(&ctx("ann", &["settings:read"]), "camera.v1.front").unwrap_err().code, ErrorCode::Denied);
    // a core with no camera views serves no such block
    assert_eq!(FrameBulk::new().get(&ann, "camera.v1.front").unwrap_err().code, ErrorCode::NotFound);
}

#[cfg(target_os = "linux")]
#[test]
fn the_real_core_serves_the_frames_beside_its_own_blocks_over_the_unix_socket() {
    use iyakki_agent::contract_local::uiserve::{start_with, Facets, UiSocketConfig};
    use iyakki_ui_client::client::Connection;
    struct Dead;
    impl iyakki_proxy::Upstream for Dead {
        fn request(&self, _: &str, _: &str, _: &[(String, String)], _: &[u8]) -> Result<iyakki_proxy::UpstreamResponse, String> {
            Err("offline".into())
        }
    }
    let d = iyakki_agent::testdir::short("iyakki-frames", "sock");
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    let svc = iyakki_agent::services::Services::open(&d.join("svc"), Arc::new(Dead), Arc::new(Dead)).unwrap();
    let core = iyakki_agent::contract_local::core_for(&svc);
    let frames = Arc::new(FrameBulk::new().with_cameras(Arc::new(OneView)));
    frames.add_app("sample", sample(200, 100)).unwrap();
    let facets = Facets { bulk: Some(frames), ..Facets::default() };
    let cfg = UiSocketConfig { enabled: true, scopes: vec!["settings:read".into(), DISPLAY_VIEW.into(), iyakki_ui_client::facets::CAMERAS_VIEW.into()], account: "ann".into(), admin: false, rpc: false };
    let on = start_with(&core, &cfg, "iyakki-test", &d.join("f.sock"), &facets).unwrap();
    let mut c = Connection::unix(on.path(), std::time::Duration::from_secs(5)).unwrap();
    let text = c.bulk_get("display.sample").unwrap();
    let list = DisplayList::parse(std::str::from_utf8(&text).unwrap()).unwrap();
    assert_eq!((list.width, list.height), (200.0, 100.0));
    assert_eq!(c.bulk_get("camera.v1.front").unwrap(), vec![0xff, 0xd8, 1, 2, 3, 0xff, 0xd9]);
    assert!(c.bulk_get("settings.all").is_ok(), "the core's own block still answers first");
    assert_eq!(c.bulk_get("display.no-such").unwrap_err().code, ErrorCode::NotFound);
    assert!(c.call("settings.schema", serde_json::json!({})).is_ok(), "ordinary calls go on after blocks");
    drop(c);
    drop(on);
    let _ = std::fs::remove_dir_all(&d);
}
