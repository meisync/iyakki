//! Chat tools for the embedded camera views (F-SDK-165): `embedded_cameras_status` (free) says whether apps may show this person's cameras
//! in their own windows and how many views are open now; `embedded_cameras_set` (asks every time) switches it. Both go through the core
//! (`cx.core.cameras()`): the same call a Settings page would make, never the agent's data folder. Switching off closes the person's open
//! views at once, so nothing more is pulled from a camera. Which camera a person may watch is not a switch of this: it is the camera's own
//! list of people (the Cameras app, `cameras_access`).

use super::chat_settings::{arg_bool, no_args, Entry, Env};
use serde_json::json;

use super::AppCx;

pub const TOOLS: [&str; 2] = ["embedded_cameras_status", "embedded_cameras_set"];

pub const PROMPT_RULE: &str = "\nEMBEDDED CAMERA VIEWS: embedded_cameras_status says whether apps may show this person's cameras inside their own windows (a camera view, like a live picture panel) and how many views are open now (free). embedded_cameras_set switches that on or off (asks first); off closes every open view at once and no picture is pulled. Which cameras a person may watch is decided by each camera's own list of people (cameras_access), and every view asks the person on a card before it shows anything; a snapshot asks again every time.";

pub fn entries(cx: &AppCx, env: &Env) -> Vec<Entry> {
    let mut v = Vec::new();
    {
        let (core, user) = (cx.core.clone(), cx.session.user.clone());
        v.push(Entry::read("Cameras", "embedded_cameras_status", "Say whether apps may show this person's cameras in their own windows, and how many embedded camera views are open now. Read only.", no_args(), move |_| {
            let c = core.cameras();
            Ok(format!("Camera views in apps are {} for you. {} open now.", if c.enabled(&user) { "on" } else { "off (or this system has none)" }, match c.open_views(&user) {
                0 => "None are".to_string(),
                1 => "One view is".to_string(),
                n => format!("{n} views are"),
            }))
        }));
    }
    {
        let (core, user, e) = (cx.core.clone(), cx.session.user.clone(), env.clone());
        let schema = json!({"type": "object", "properties": {"on": {"type": "boolean"}}, "required": ["on"]});
        v.push(Entry::change("Cameras", "embedded_cameras_set", "Switch camera views in apps on or off for this person. Off closes every open view and nothing more is pulled from a camera. The person approves a card first.", schema, |a| format!("Turn camera views in apps {}", if a["on"].as_bool().unwrap_or(true) { "on" } else { "off" }), move |a| {
            let on = arg_bool(a, "on")?;
            core.cameras().set_enabled(&user, on).map_err(|e| e.to_string())?;
            e.audit("settings.cameras", &format!("Camera views in apps: {}.", if on { "on" } else { "off" }));
            Ok(format!("Camera views in apps are {}.", if on { "on" } else { "off, and the open ones are closed" }))
        }));
    }
    v
}

#[cfg(test)]
mod tests {
    use super::*;
    use iyakki_shared::agent::{Tools};
    use iyakki_agent::agent::{Risk, ToolDef};
    use iyakki_shared::settingstools::SettingsTools;
    use iyakki_core_api::cameras::{ViewSpec};
    use iyakki_core_api::Core;
    use iyakki_core_fake::{FakeCam, FakeCore};
    use std::sync::Arc;

    struct NoTools;
    impl Tools for NoTools {
        fn defs(&self) -> Vec<ToolDef> {
            vec![]
        }
        fn call(&self, name: &str, _a: &serde_json::Value) -> Result<String, String> {
            Err(format!("no tool {name}"))
        }
        fn describe(&self, name: &str, _a: &serde_json::Value) -> String {
            name.to_string()
        }
    }

    #[test]
    fn the_status_is_free_the_switch_asks_and_off_closes_the_views_through_the_core() {
        let mut cx = crate::apps::tests::cx_with_admin_store("embed-cams-chat");
        let core = Arc::new(FakeCore::new());
        cx.core = core.clone();
        let user = cx.session.user.clone();
        core.cameras_fake().add(FakeCam::new("front", "Front door", &user));
        let env = Env { dir: cx.data_dir().to_path_buf(), user: user.clone(), admin: true, settings: cx.settings(), audit: Arc::new(|_, _| {}) };
        let t = SettingsTools::new(Arc::new(NoTools), true, entries(&cx, &env));
        let defs = t.defs();
        let risk = |n: &str| defs.iter().find(|d| d.spec.name == n).unwrap_or_else(|| panic!("{n}")).risk;
        assert_eq!(risk("embedded_cameras_status"), Risk::Safe);
        assert_eq!(risk("embedded_cameras_set"), Risk::Ask);
        core.cameras().open(&user, "sample", &ViewSpec { cameras: vec!["front".into()], ..Default::default() }).unwrap();
        let s = t.call("embedded_cameras_status", &json!({})).unwrap();
        assert!(s.contains("are on for you") && s.contains("One view is open"), "{s}");
        let r = t.call("embedded_cameras_set", &json!({"on": false})).unwrap();
        assert!(r.contains("off"), "{r}");
        assert!(!core.cameras().enabled(&user));
        assert_eq!(core.cameras().open_views(&user), 0, "the open view was closed");
        assert!(core.cameras().open(&user, "sample", &ViewSpec { cameras: vec!["front".into()], ..Default::default() }).is_err());
        assert!(t.call("embedded_cameras_status", &json!({})).unwrap().contains("off"));
        assert!(t.call("embedded_cameras_set", &json!({"on": "yes"})).is_err());
    }
}
