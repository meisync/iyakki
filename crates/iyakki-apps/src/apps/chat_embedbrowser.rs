//! Chat tools for the embedded browser (F-SDK-65, Settings -> Browser): `embedded_browser_status` (free) says whether apps may place the
//! browser in their own windows for this person and how many views are open; `embedded_browser_set` (asks every time) switches it. Both go
//! through the core (`cx.core.browser()`): the same call a Settings page makes, never the agent's data folder. Switching off closes the
//! person's open views at once.

use super::chat_settings::{arg_bool, no_args, Entry, Env};
use serde_json::json;

use super::AppCx;

pub const TOOLS: [&str; 2] = ["embedded_browser_status", "embedded_browser_set"];

pub const PROMPT_RULE: &str = "\nEMBEDDED BROWSER (Settings -> Browser): embedded_browser_status says whether apps may show web pages inside their own windows (a browser view, like a web view) and how many views are open now (free). embedded_browser_set switches that on or off (asks first); off closes every open view at once. The pages are loaded by the core, which refuses sites on the known-bad list and sites the account may not reach; a download or a permission a page asks for is a question the person answers.";

pub fn entries(cx: &AppCx, env: &Env) -> Vec<Entry> {
    let mut v = Vec::new();
    {
        let (core, user) = (cx.core.clone(), cx.session.user.clone());
        v.push(Entry::read("Browser", "embedded_browser_status", "Say whether apps may embed the browser in their own windows for this person, and how many embedded views are open now. Read only.", no_args(), move |_| {
            let b = core.browser();
            Ok(format!("Embedded browser views are {} for you. {} open now.", if b.enabled(&user) { "on" } else { "off (or this system has none)" }, match b.open_views(&user) {
                0 => "None are".to_string(),
                1 => "One view is".to_string(),
                n => format!("{n} views are"),
            }))
        }));
    }
    {
        let (core, user, e) = (cx.core.clone(), cx.session.user.clone(), env.clone());
        let schema = json!({"type": "object", "properties": {"on": {"type": "boolean"}}, "required": ["on"]});
        v.push(Entry::change("Browser", "embedded_browser_set", "Switch embedded browser views on or off for this person. Off closes every open view. The person approves a card first.", schema, |a| format!("Turn embedded browser views {}", if a["on"].as_bool().unwrap_or(true) { "on" } else { "off" }), move |a| {
            let on = arg_bool(a, "on")?;
            core.browser().set_enabled(&user, on).map_err(|e| e.to_string())?;
            e.audit("settings.browser", &format!("Embedded browser views: {}.", if on { "on" } else { "off" }));
            Ok(format!("Embedded browser views are {}.", if on { "on" } else { "off, and the open ones are closed" }))
        }));
    }
    v
}

#[cfg(test)]
mod tests {
    use super::*;
    use iyakki_shared::agent::{Risk, ToolDef};
    use iyakki_shared::agent::{Tools};
    use iyakki_shared::settingstools::SettingsTools;
    use iyakki_core_api::browsersession::{BrowserSessions, OpenSpec};
    use iyakki_core_api::Core;
    use iyakki_core_fake::FakeCore;
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
        let mut cx = crate::apps::tests::cx_with_admin_store("embed-chat");
        let core = Arc::new(FakeCore::new());
        cx.core = core.clone();
        let user = cx.session.user.clone();
        let env = Env { dir: cx.data_dir().to_path_buf(), user: user.clone(), admin: true, settings: cx.settings().clone(), audit: Arc::new(|_, _| {}) };
        let t = SettingsTools::new(Arc::new(NoTools), true, entries(&cx, &env));
        let defs = t.defs();
        let risk = |n: &str| defs.iter().find(|d| d.spec.name == n).unwrap_or_else(|| panic!("{n}")).risk;
        assert_eq!(risk("embedded_browser_status"), Risk::Safe);
        assert_eq!(risk("embedded_browser_set"), Risk::Ask);
        core.browser().open(&user, "sample", &OpenSpec::new(100.0, 100.0)).unwrap();
        let s = t.call("embedded_browser_status", &json!({})).unwrap();
        assert!(s.contains("are on for you") && s.contains("One view is open"), "{s}");
        let r = t.call("embedded_browser_set", &json!({"on": false})).unwrap();
        assert!(r.contains("off"), "{r}");
        assert!(!core.browser().enabled(&user));
        assert_eq!(core.browser().open_views(&user), 0, "the open view was closed");
        assert!(t.call("embedded_browser_status", &json!({})).unwrap().contains("off"));
        assert!(t.call("embedded_browser_set", &json!({"on": "yes"})).is_err());
    }
}
