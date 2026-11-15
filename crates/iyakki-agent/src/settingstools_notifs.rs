//! Chat tools for Settings -> Notifications (the bell and inbox filters): `notification_filters` (free) and `notification_filter_set`
//! (asks). Same `inbox::Inbox` levels as the page: Show (counted in the bell), Quiet (inbox only), Off (not kept).

use serde_json::json;

use crate::inbox::{Inbox, Level, SOURCES};
use crate::settingstools::{arg_str, no_args, Entry, Env};

const TAB: &str = "Notifications";

pub const TOOLS: [&str; 2] = ["notification_filters", "notification_filter_set"];

pub const PROMPT_RULE: &str = "\nNOTIFICATIONS (Settings -> Notifications): notification_filters lists every source with how much of it reaches the bell (show, quiet = inbox only, off = not kept); it is free. notification_filter_set changes one source and asks first. Information, new devices and finished jobs never add to the bell's number.";

fn inbox(e: &Env) -> Inbox {
    Inbox::new(&e.dir.join("inbox"))
}

fn level_of(s: &str) -> Result<Level, String> {
    match s.to_ascii_lowercase().as_str() {
        "show" => Ok(Level::Show),
        "quiet" => Ok(Level::Quiet),
        "off" => Ok(Level::Off),
        _ => Err("level is show, quiet or off".into()),
    }
}

pub fn entries(env: &Env) -> Vec<Entry> {
    let mut v = Vec::new();
    {
        let e = env.clone();
        v.push(Entry::read(TAB, "notification_filters", "List the notification sources and how much of each reaches the bell: show, quiet (inbox only) or off (not kept).", no_args(), move |_| {
            let ib = inbox(&e);
            let lv = ib.levels(&e.user);
            let mut out = String::new();
            for (src, label) in ib.sources(&e.user) {
                out.push_str(&format!("{src}: {} ({label})\n", lv.get(&src).copied().unwrap_or(Level::Show).name()));
            }
            Ok(out.trim_end().to_string())
        }));
    }
    {
        let e = env.clone();
        let schema = json!({"type": "object", "properties": {"source": {"type": "string", "enum": SOURCES.iter().map(|s| s.0).collect::<Vec<_>>(), "description": "The source id from notification_filters."}, "level": {"type": "string", "enum": ["show", "quiet", "off"]}}, "required": ["source", "level"]});
        let (e2, e3) = (env.clone(), env.clone());
        v.push(
            Entry::change(TAB, "notification_filter_set", "Change how much of one notification source reaches the bell: show, quiet (inbox only, no number, no sound) or off (not kept). The person approves a card first.", schema, |a| format!("Set notifications from {} to {}", a["source"].as_str().unwrap_or("a source"), a["level"].as_str().unwrap_or("?")), move |a| {
                let (src, level) = (arg_str(a, "source")?, level_of(&arg_str(a, "level")?)?);
                inbox(&e).set_level(&e.user, &src, level)?;
                e.audit("settings.notifications", &format!("{src} {}", level.name()));
                Ok(format!("Notifications from {src}: {}.", level.label()))
            })
            .with_preview(move |a| {
                let src = a["source"].as_str().unwrap_or("");
                format!("Now: {}.\nAfter: {}.", inbox(&e2).level(&e2.user, src).label(), level_of(a["level"].as_str().unwrap_or("")).map(|l| l.label()).unwrap_or("?"))
            })
            .with_refusal(move |a| {
                let src = a["source"].as_str().unwrap_or("");
                if !inbox(&e3).sources(&e3.user).iter().any(|(s, _)| s == src) {
                    return Some(format!("There is no notification source {src}; call notification_filters"));
                }
                level_of(a["level"].as_str().unwrap_or("")).err()
            }),
        );
    }
    v
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::Tools;
    use crate::settingstools_tests::{assert_policy, rig, tools};

    #[test]
    fn the_filters_change_the_pages_levels_and_ask_every_time() {
        let r = rig("notifs", false);
        let t = tools(&r, entries(&r.env));
        assert_policy(&t, &["notification_filters"], &["notification_filter_set"]);
        assert!(t.call("notification_filters", &json!({})).unwrap().contains("flows: show"));
        assert!(t.refusal("notification_filter_set", &json!({"source": "nope", "level": "off"})).is_some());
        assert!(t.refusal("notification_filter_set", &json!({"source": "flows", "level": "loud"})).is_some());
        assert!(t.preview("notification_filter_set", &json!({"source": "flows", "level": "quiet"})).unwrap().contains("After: Quiet"));
        t.call("notification_filter_set", &json!({"source": "flows", "level": "quiet"})).unwrap();
        assert_eq!(Inbox::new(&r.dir.join("inbox")).level("ada", "flows"), Level::Quiet);
        assert!(t.call("notification_filters", &json!({})).unwrap().contains("flows: quiet"));
        assert!(r.audits.lock().unwrap().iter().any(|a| a == "settings.notifications flows quiet"));
        let _ = std::fs::remove_dir_all(&r.dir);
    }
}
