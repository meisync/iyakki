use std::sync::Arc;

use serde_json::json;

use super::*;
use crate::agent::Tools;
use crate::directlink::fake::FakeEnv;
use crate::settingstools_tests::{assert_policy, rig, tools};

fn fake(env: Arc<FakeEnv>) -> MakeEnv {
    Arc::new(move || env.clone() as Arc<dyn LinkEnv>)
}

#[test]
fn both_tools_only_read_are_for_administrators_and_are_written_to_the_log() {
    let r = rig("usblink", true);
    let env = Arc::new(FakeEnv::new());
    let t = tools(&r, entries(&r.env, fake(env)));
    assert_policy(&t, &TOOLS, &[]);
    let out = t.call("usb_link_check", &json!({})).unwrap();
    assert!(out.contains("Last result:") && out.contains("[FAIL]") || out.contains("[ .. ]"), "{out}");
    assert!(r.audits.lock().unwrap().iter().any(|a| a.starts_with("settings.usblink.check")));
    // not for a person who is not an administrator
    let r2 = rig("usblink-user", false);
    let t2 = tools(&r2, entries(&r2.env, fake(Arc::new(FakeEnv::new()))));
    assert!(t2.call("usb_link_check", &json!({})).is_err());
}

#[test]
fn the_replug_test_validates_the_seconds_and_says_what_the_port_noticed() {
    let r = rig("usblink-watch", true);
    let env = Arc::new(FakeEnv::new());
    let t = tools(&r, entries(&r.env, fake(env.clone())));
    for bad in [json!({"seconds": 1}), json!({"seconds": 500}), json!({"seconds": "ten"})] {
        assert!(t.call("usb_link_watch", &bad).unwrap_err().contains("5 to 120"), "{bad}");
    }
    let out = t.call("usb_link_watch", &json!({"seconds": 5})).unwrap();
    assert!(out.contains("Watching the Thunderbolt/USB4 ports for 5 seconds") && out.contains("5 seconds"), "{out}");
    assert!(r.audits.lock().unwrap().iter().any(|a| a == "settings.usblink.watch 5 s"));
    // nothing on the machine was changed: the fake saw no write
    assert!(env.calls().iter().all(|c| !c.starts_with("set_addr") && !c.starts_with("authorize") && !c.starts_with("pair")), "{:?}", env.calls());
}
