//! Chat tools for the Alexa provider behind the voice seam (F-VOICE-43): `alexa_status` (free) and `alexa_set` (asks every time,
//! administrators: on or off for this computer). FAIL CLOSED: with no owner client id nothing can be switched on. The person's yes to the
//! card that sends the voice to Amazon is given on the Settings page only: there is no tool and no argument for it, and none for an id.

use serde_json::json;

use crate::settingstools::{no_args, Entry, Env};
use iyakki_shared::alexa;

const TAB: &str = "Voice";
pub const TOOLS: [&str; 2] = ["alexa_status", "alexa_set"];

pub use iyakki_shared::settingstools_alexa::PROMPT_RULE;

pub fn entries(env: &Env) -> Vec<Entry> {
    let mut v = Vec::new();
    {
        let e = env.clone();
        v.push(Entry::read(TAB, "alexa_status", "Say whether the Alexa provider (Amazon's assistant, optional) is set up with the owner's product, switched on, and whether this person said yes to sending their voice to Amazon.", no_args(), move |_| Ok(alexa::status_text(&e.dir, &e.user))));
    }
    {
        let e = env.clone();
        let schema = json!({"type": "object", "properties": {"enabled": {"type": "boolean", "description": "true makes Alexa available on this computer (nothing is sent until the person says yes on the Settings page and starts a turn); false turns it off."}}, "required": ["enabled"]});
        v.push(
            Entry::change(TAB, "alexa_set", "Switch the Alexa provider on or off for this computer. Refused when the owner has not set up an Alexa product. It cannot give the person's yes to the card; they answer it in Settings -> Alexa. The person approves a card first.", schema, |a| if a["enabled"] == false { "Turn Alexa off".into() } else { "Make Alexa (Amazon's assistant) available on this computer (nothing is sent until you say yes in Settings and start a turn)".into() }, move |a| {
                let on = a.get("enabled").and_then(|x| x.as_bool()).ok_or("`enabled` must be true or false")?;
                alexa::set_enabled(&e.dir, on)?;
                e.audit("settings.alexa", if on { "switched on" } else { "switched off" });
                Ok(if on { "Alexa is available. Nothing is sent to Amazon until the person says yes in Settings -> Alexa and starts a turn.".to_string() } else { "Alexa is off.".to_string() })
            })
            .admin(),
        );
    }
    v
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::settingstools_tests::{assert_policy, rig, tools};
    use crate::agent::Tools;

    #[test]
    fn the_tools_fail_closed_without_a_client_id_and_never_give_the_persons_yes() {
        let r = rig("alexa", true);
        let t = tools(&r, entries(&r.env));
        assert_policy(&t, &["alexa_status"], &["alexa_set"]);
        assert!(t.call("alexa_status", &json!({})).unwrap().contains("no client id"));
        assert!(t.call("alexa_set", &json!({"enabled": true})).unwrap_err().contains("no client id"));
        assert!(t.call("alexa_set", &json!({"enabled": false})).is_ok());
        std::fs::write(r.dir.join(alexa::CONFIG_FILE), r#"{"client_id":"id1","product_id":"p1"}"#).unwrap();
        assert!(t.call("alexa_set", &json!({"enabled": true})).unwrap().contains("Nothing is sent"));
        assert!(t.call("alexa_status", &json!({})).unwrap().contains("not said yes"));
        // no argument gives the yes or an id
        assert!(t.call("alexa_set", &json!({"enabled": true, "consent": true, "client_id": "x"})).is_ok());
        assert!(alexa::allowed(&r.dir, "ada").is_err(), "the person's yes is still missing");
        assert!(r.audits.lock().unwrap().iter().any(|l| l.contains("settings.alexa switched on")));
        let _ = std::fs::remove_dir_all(&r.dir);
    }

    #[test]
    fn the_shared_client_id_rule_is_the_transports_rule() {
        for (text, ok) in [(r#"{"client_id":"a","product_id":"b"}"#, true), (r#"{"client_id":"","product_id":"b"}"#, false), (r#"{"client_id":"a b","product_id":"b"}"#, false), (r#"{"product_id":"b"}"#, false), ("nope", false)] {
            assert_eq!(crate::avs_h2::AvsConfig::parse(text).is_ok(), ok, "{text}");
            let d = std::env::temp_dir().join(format!("iyakki-alexa-rule-{}-{}", std::process::id(), ok as u8 + text.len() as u8));
            let _ = std::fs::remove_dir_all(&d);
            std::fs::create_dir_all(&d).unwrap();
            std::fs::write(d.join(alexa::CONFIG_FILE), text).unwrap();
            assert_eq!(alexa::configured(&d), ok, "{text}");
            let _ = std::fs::remove_dir_all(&d);
        }
        assert_eq!(alexa::CONFIG_FILE, crate::avs_h2::CONFIG_FILE);
        assert_eq!(alexa::NO_CLIENT_ID, crate::avs_h2::NO_CLIENT_ID);
    }
}
