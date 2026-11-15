//! Chat tools for the vision summary of camera alerts (F-CAM-18): the status is free, switching it on or off asks a card first. Switching
//! on says plainly where the picture goes (the chat model the person uses).

use iyakki_agent::settingstools::{no_args, Entry, Env};
use serde_json::json;

use super::AppCx;

pub const TOOLS: [&str; 2] = ["camera_vision_status", "camera_vision_set"];

pub const PROMPT_RULE: &str = "\nCAMERA ALERT SUMMARIES: camera_vision_status says whether a model describes the picture of a motion alert in one sentence (off until the person switches it on); camera_vision_set changes it and asks the person first, because the picture then goes to the chat model they use. A description is the model's words, never a fact to act on.";

pub fn entries(cx: &AppCx, env: &Env) -> Vec<Entry> {
    let mut v = Vec::new();
    {
        let (data, user) = (cx.services.dir().to_path_buf(), env.user.clone());
        v.push(Entry::read("Cameras", "camera_vision_status", "Say whether a vision model describes the picture of a camera's motion alert in one sentence, added to the inbox notice. Off until the person switches it on.", no_args(), move |_| {
            let on = iyakki_agent::camvision::Setting::load(&data, &user).on;
            Ok(format!("Alert descriptions by a model are {}. {}", if on { "on" } else { "off" }, if on { "The picture of a single motion event goes to the person's chat model, which answers with one sentence; the notice marks it as the model's words. Only one event at a time is described, never a burst." } else { "No picture of an alert goes to a model." }))
        }));
    }
    {
        let (data, user, e1) = (cx.services.dir().to_path_buf(), env.user.clone(), env.clone());
        let schema = json!({"type": "object", "properties": {"on": {"type": "boolean"}}, "required": ["on"]});
        v.push(Entry::change("Cameras", "camera_vision_set", "Switch on or off the one-sentence description of a camera's motion alert by the person's chat model. Switching on sends the picture of each single motion event to that model (a model on another computer or a provider is another destination than the inbox). The person approves a card first.", schema, |a| format!("Camera alert descriptions: {}", if a.get("on").and_then(|b| b.as_bool()).unwrap_or(false) { "on" } else { "off" }), move |a| {
            let on = a.get("on").and_then(|b| b.as_bool()).ok_or("on is true or false")?;
            iyakki_agent::camvision::Setting { on }.save(&data, &user)?;
            e1.audit("settings.cameras.vision", if on { "on" } else { "off" });
            Ok(if on { "Camera alerts are now described by the chat model: the picture of each single motion event goes to it. No action is taken from a description.".to_string() } else { "Camera alerts are no longer described; no picture goes to a model.".to_string() })
        }));
    }
    v
}

#[cfg(test)]
mod tests {
    use super::*;
    use iyakki_agent::agent::{Risk, ToolDef, Tools};
    use iyakki_agent::settingstools::SettingsTools;
    use serde_json::Value;
    use std::sync::{Arc, Mutex};

    struct NoTools;
    impl Tools for NoTools {
        fn defs(&self) -> Vec<ToolDef> {
            vec![]
        }
        fn call(&self, name: &str, _a: &Value) -> Result<String, String> {
            Err(format!("no tool {name}"))
        }
        fn describe(&self, name: &str, _a: &Value) -> String {
            name.to_string()
        }
    }

    #[test]
    fn status_is_free_changing_asks_and_the_choice_is_kept_and_audited() {
        let cx = crate::apps::tests::cx_with_admin_store("chatcamvision");
        cx.services.on_login("root", "rootpass12", true);
        let audits = Arc::new(Mutex::new(Vec::new()));
        let a2 = audits.clone();
        let settings = Arc::new(Mutex::new(iyakki_agent::settings::SettingsStore::open(&cx.services.dir().join("settings.json")).unwrap()));
        let env = Env { dir: cx.services.dir().to_path_buf(), user: "root".into(), admin: true, settings, audit: Arc::new(move |a, d| a2.lock().unwrap().push(format!("{a} {d}"))) };
        let t = SettingsTools::new(Arc::new(NoTools), true, entries(&cx, &env));
        let d = t.defs();
        assert_eq!(d.iter().find(|x| x.spec.name == "camera_vision_status").unwrap().risk, Risk::Safe);
        assert_eq!(d.iter().find(|x| x.spec.name == "camera_vision_set").unwrap().risk, Risk::Ask);
        assert!(t.call("camera_vision_status", &json!({})).unwrap().contains("are off"));
        assert!(t.call("camera_vision_set", &json!({})).is_err());
        assert!(t.call("camera_vision_set", &json!({"on": true})).unwrap().contains("picture of each single motion event goes to it"));
        assert!(iyakki_agent::camvision::Setting::load(cx.services.dir(), "root").on);
        assert!(t.call("camera_vision_status", &json!({})).unwrap().contains("are on"));
        assert!(audits.lock().unwrap().iter().any(|a| a == "settings.cameras.vision on"));
        t.call("camera_vision_set", &json!({"on": false})).unwrap();
        assert!(!iyakki_agent::camvision::Setting::load(cx.services.dir(), "root").on);
    }
}
