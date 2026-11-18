//! Chat tools for the embedded media player (F-SDK-148): `embedded_media_status` (free) says whether apps may play sound and video in their
//! own windows for this person and how many players are open; `embedded_media_set` (asks every time) switches it. Both go through the core
//! (`cx.core.media()`): the same call a Settings page would make, never the agent's data folder. Switching off stops every open player at once.

use super::chat_settings::{arg_bool, no_args, Entry, Env};
use serde_json::json;

use super::AppCx;

pub const TOOLS: [&str; 2] = ["embedded_media_status", "embedded_media_set"];

pub const PROMPT_RULE: &str = "\nEMBEDDED MEDIA PLAYER (Settings -> Sound): embedded_media_status says whether apps may play sound inside their own windows (a sound player in an app, with its own play, pause and seek) and how many players are open now (free). embedded_media_set switches that on or off (asks first); off stops every open player at once. An app plays the person's own sound files; an internet address or a radio station asks the person first. The sound goes through the same mixer as everything else, so the person's volume applies.";

pub fn entries(cx: &AppCx, env: &Env) -> Vec<Entry> {
    let mut v = Vec::new();
    {
        let (core, user) = (cx.core.clone(), cx.session.user.clone());
        v.push(Entry::read("Sound", "embedded_media_status", "Say whether apps may play sound in their own windows for this person, and how many embedded players are open now. Read only.", no_args(), move |_| {
            let m = core.media();
            Ok(format!("Embedded sound players are {} for you. {} open now.", if m.enabled(&user) { "on" } else { "off (or this system has none)" }, match m.open_sessions(&user) {
                0 => "None are".to_string(),
                1 => "One player is".to_string(),
                n => format!("{n} players are"),
            }))
        }));
    }
    {
        let (core, user, e) = (cx.core.clone(), cx.session.user.clone(), env.clone());
        let schema = json!({"type": "object", "properties": {"on": {"type": "boolean"}}, "required": ["on"]});
        v.push(Entry::change("Sound", "embedded_media_set", "Switch embedded sound players in apps on or off for this person. Off stops every open player. The person approves a card first.", schema, |a| format!("Turn embedded sound players {}", if a["on"].as_bool().unwrap_or(true) { "on" } else { "off" }), move |a| {
            let on = arg_bool(a, "on")?;
            core.media().set_enabled(&user, on)?;
            e.audit("settings.sound.embedded", &format!("Embedded sound players: {}.", if on { "on" } else { "off" }));
            Ok(format!("Embedded sound players are {}.", if on { "on" } else { "off, and the open ones are stopped" }))
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
    use iyakki_core_api::media::{MediaSource, MediaSpec};
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
    fn the_status_is_free_the_switch_asks_and_off_stops_the_players_through_the_core() {
        let mut cx = crate::apps::tests::cx_with_admin_store("embed-media-chat");
        let core = Arc::new(FakeCore::new());
        cx.core = core.clone();
        let user = cx.session.user.clone();
        core.media_fake().add_file(&user, "a.mp3", 5000, false);
        let env = Env { dir: cx.data_dir().to_path_buf(), user: user.clone(), admin: true, settings: cx.settings().clone(), audit: Arc::new(|_, _| {}) };
        let t = SettingsTools::new(Arc::new(NoTools), true, entries(&cx, &env));
        let defs = t.defs();
        let risk = |n: &str| defs.iter().find(|d| d.spec.name == n).unwrap_or_else(|| panic!("{n}")).risk;
        assert_eq!(risk("embedded_media_status"), Risk::Safe);
        assert_eq!(risk("embedded_media_set"), Risk::Ask);
        core.media().open(&user, "sample", &MediaSpec::new(MediaSource::Home { name: "a.mp3".into() })).unwrap();
        let s = t.call("embedded_media_status", &json!({})).unwrap();
        assert!(s.contains("are on for you") && s.contains("One player is open"), "{s}");
        let r = t.call("embedded_media_set", &json!({"on": false})).unwrap();
        assert!(r.contains("off"), "{r}");
        assert!(!core.media().enabled(&user));
        assert_eq!(core.media().open_sessions(&user), 0, "the open player was stopped");
        assert!(t.call("embedded_media_status", &json!({})).unwrap().contains("off"));
        assert!(t.call("embedded_media_set", &json!({"on": "yes"})).is_err());
    }
}
