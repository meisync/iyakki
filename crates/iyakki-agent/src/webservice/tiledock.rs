//! The `dock` domain of a browser tile (F-SDK-24): what the UI SDK's `DockPort` asks (`dock.pinned`, `dock.pin`, `dock.unpin`, with
//! `{"app": ".."}`) turned into the person's own chat tools `display_dock` and `display_dock_set`. The tile gets no power of its own:
//! the tools are the person's (their tool set, their refusals), pinning and unpinning ask first on an approval card in their browser, and
//! every call is one audit line without arguments.
//!
//! `dock.open` and `dock.status` are not served here: opening an app is the framebuffer shell's act on a screen this service does not
//! drive, and the status of an app is the shell's. They answer "not a call this tile may make", which the port reads as unavailable.

use serde_json::{json, Value};

/// The tool and its arguments for one dock call, or why the call is refused.
pub fn plan(method: &str, args_json: &str) -> Result<(&'static str, Value), String> {
    let args: Value = serde_json::from_str(if args_json.trim().is_empty() { "{}" } else { args_json }).map_err(|_| "the arguments are not JSON".to_string())?;
    if !args.is_object() {
        return Err("the arguments are a JSON object".into());
    }
    match method {
        "pinned" => Ok(("display_dock", json!({}))),
        "pin" | "unpin" => {
            let app = args["app"].as_str().unwrap_or("");
            if app.is_empty() || app.len() > 32 || !app.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-')) {
                return Err("that is not an app id".into());
            }
            Ok(("display_dock_set", json!({"op": method, "app": app})))
        }
        _ => Err("that is not a call this tile may make".into()),
    }
}

/// What the tile reads back: the pinned list as a JSON array for `pinned`, `{"ok":true}` for the others. `text` is what the tool answered.
pub fn answer(method: &str, text: &str) -> Result<String, String> {
    if method != "pinned" {
        return Ok(json!({"ok": true}).to_string());
    }
    let v: Value = serde_json::from_str(text).map_err(|_| "the dock list was not readable".to_string())?;
    let list: Vec<&str> = v["pinned"].as_array().map(|a| a.iter().filter_map(Value::as_str).collect()).unwrap_or_default();
    Ok(json!(list).to_string())
}

/// `dock.status {app, badge, line}`: a tile says what its own icon shows. `badge` is a whole number or null, `line` text of at most 40 characters.
pub fn parse_status(args_json: &str) -> Result<(String, Option<u32>, String), String> {
    let v: Value = serde_json::from_str(args_json).map_err(|_| "the arguments are not JSON".to_string())?;
    let app = v["app"].as_str().unwrap_or("");
    if app.is_empty() || app.len() > 32 || !app.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-')) {
        return Err("that is not an app id".into());
    }
    let badge = match &v["badge"] {
        Value::Null => None,
        Value::Number(n) => Some(n.as_u64().filter(|b| *b <= 100_000).ok_or("the badge is a whole number")? as u32),
        _ => return Err("the badge is a whole number or null".into()),
    };
    let line = match &v["line"] {
        Value::Null => "",
        Value::String(l) if l.chars().count() <= 200 => l.as_str(),
        _ => return Err("the line is text".into()),
    };
    Ok((app.to_string(), badge, line.chars().filter(|c| !c.is_control()).take(40).collect()))
}

/// The `wallpaper` domain: `get` is the person's `display_wallpaper`, `set {wallpaper}` their `display_wallpaper_set` (which asks first).
pub fn plan_wallpaper(method: &str, args_json: &str) -> Result<(&'static str, Value), String> {
    let args: Value = serde_json::from_str(if args_json.trim().is_empty() { "{}" } else { args_json }).map_err(|_| "the arguments are not JSON".to_string())?;
    if !args.is_object() {
        return Err("the arguments are a JSON object".into());
    }
    match method {
        "get" => Ok(("display_wallpaper", json!({}))),
        "set" => {
            let w = args["wallpaper"].as_str().unwrap_or("");
            if w.is_empty() || w.len() > 16 || !w.chars().all(|c| c.is_ascii_lowercase()) {
                return Err("that is not a wallpaper key".into());
            }
            Ok(("display_wallpaper_set", json!({"wallpaper": w})))
        }
        _ => Err("that is not a call this tile may make".into()),
    }
}

/// `get` reads back the chosen key from the tool's first line (`Chosen: dusk`) as a JSON string; `set` answers `{"ok":true}`.
pub fn answer_wallpaper(method: &str, text: &str) -> Result<String, String> {
    if method != "get" {
        return Ok(json!({"ok": true}).to_string());
    }
    let key = text.lines().find_map(|l| l.strip_prefix("Chosen: ")).map(str::trim).ok_or("the wallpaper answer was not readable")?;
    Ok(json!(key).to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_calls_map_to_the_two_chat_tools_and_nothing_else() {
        assert_eq!(plan("pinned", "{}").unwrap(), ("display_dock", json!({})));
        assert_eq!(plan("pinned", "").unwrap().0, "display_dock");
        assert_eq!(plan("pin", r#"{"app":"radio"}"#).unwrap(), ("display_dock_set", json!({"op": "pin", "app": "radio"})));
        assert_eq!(plan("unpin", r#"{"app":"chat"}"#).unwrap(), ("display_dock_set", json!({"op": "unpin", "app": "chat"})));
        for m in ["open", "status", "move", "", "Pin", "pin ", "tools"] {
            assert!(plan(m, r#"{"app":"chat"}"#).unwrap_err().contains("not a call"), "{m}");
        }
    }

    #[test]
    fn hostile_arguments_are_refused_before_any_tool_is_named() {
        for bad in ["not json", "[1]", "42", r#"{"app":""}"#, r#"{"app":7}"#, r#"{"app":"a b"}"#, r#"{"app":"../x"}"#, r#"{"app":"a;rm"}"#, "{}"] {
            assert!(plan("pin", bad).is_err(), "{bad}");
        }
        let long = format!(r#"{{"app":"{}"}}"#, "a".repeat(33));
        assert!(plan("unpin", &long).is_err());
        // extra fields are ignored, not passed on
        assert_eq!(plan("pin", r#"{"app":"chat","op":"move","position":3}"#).unwrap().1, json!({"op": "pin", "app": "chat"}));
    }

    #[test]
    fn a_status_is_an_app_id_a_whole_badge_or_null_and_a_short_line() {
        assert_eq!(parse_status(r#"{"app":"chat","badge":3,"line":"2 waiting"}"#).unwrap(), ("chat".into(), Some(3), "2 waiting".into()));
        assert_eq!(parse_status(r#"{"app":"x-1","badge":null,"line":null}"#).unwrap(), ("x-1".into(), None, String::new()));
        let long = format!(r#"{{"app":"a","line":"{}"}}"#, "é".repeat(100));
        assert_eq!(parse_status(&long).unwrap().2.chars().count(), 40);
        assert_eq!(parse_status("{\"app\":\"a\",\"line\":\"a\\u0007b\"}").unwrap().2, "ab", "control characters are dropped");
        for bad in ["nope", "{}", r#"{"app":"a b"}"#, r#"{"app":"a","badge":-1}"#, r#"{"app":"a","badge":1.5}"#, r#"{"app":"a","badge":"3"}"#, r#"{"app":"a","badge":99999999}"#, r#"{"app":"a","line":5}"#] {
            assert!(parse_status(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn the_wallpaper_calls_map_to_the_two_wallpaper_tools() {
        assert_eq!(plan_wallpaper("get", "").unwrap(), ("display_wallpaper", json!({})));
        assert_eq!(plan_wallpaper("set", r#"{"wallpaper":"dusk","x":1}"#).unwrap(), ("display_wallpaper_set", json!({"wallpaper": "dusk"})));
        for bad in ["{}", r#"{"wallpaper":"../x"}"#, r#"{"wallpaper":3}"#, r#"{"wallpaper":"Dusk"}"#, "[]", "nope"] {
            assert!(plan_wallpaper("set", bad).is_err(), "{bad}");
        }
        assert!(plan_wallpaper("list", "{}").is_err());
        assert_eq!(answer_wallpaper("get", "Chosen: dusk\nWallpaper theme: Theme").unwrap(), "\"dusk\"");
        assert!(answer_wallpaper("get", "nothing").is_err());
        assert_eq!(answer_wallpaper("set", "ok").unwrap(), r#"{"ok":true}"#);
    }

    #[test]
    fn the_answers_are_what_the_port_reads() {
        assert_eq!(answer("pinned", r#"{"pinned":["chat","radio"],"available":["chat","radio","scan"]}"#).unwrap(), r#"["chat","radio"]"#);
        assert_eq!(answer("pinned", r#"{"available":[]}"#).unwrap(), "[]");
        assert!(answer("pinned", "plain words").is_err());
        assert_eq!(answer("pin", "Pinned radio.").unwrap(), r#"{"ok":true}"#);
    }
}
