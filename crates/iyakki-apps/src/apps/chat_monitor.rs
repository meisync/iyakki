//! Chat tools for the brightness of external monitors over DDC/CI (F-KBD-08). Listing is free and only reads. Choosing the one screen
//! that may be written to, and setting its brightness, each ask the person on a card first. Everything goes through `cx.core`.

use serde_json::json;

use super::chat_themes::{no_args, Entry, Env};
use super::chat_port::ChatPort;

pub const TOOLS: [&str; 3] = ["monitor_brightness", "monitor_choose", "monitor_brightness_set"];

pub const PROMPT_RULE: &str = "\nEXTERNAL MONITORS: monitor_brightness lists the screens that can be set over DDC/CI (name, id, brightness in percent, or why one cannot be read; many monitors do not answer) and which one is chosen; it only reads. monitor_choose chooses the one screen that may be written to by its id (or none) and asks the person first. monitor_brightness_set sets the brightness (1 to 100, never fully dark) of the chosen screen and asks the person first; the value is read back. A machine whose kernel lacks the I2C driver says so.";

pub fn entries(cx: &dyn ChatPort, env: &Env) -> Vec<Entry> {
    let mut v = Vec::new();
    {
        let core = cx.core();
        v.push(Entry::read("Display", "monitor_brightness", "List the external monitors that can be set over DDC/CI: name, id, brightness in percent (or why it cannot be read) and which one is chosen. Reads only.", no_args(), move |_| {
            let list = core.monitors().list();
            if list.is_empty() {
                return Ok("No screen with a DDC/CI channel was found (the kernel may lack the I2C driver, or the screen is built in).".into());
            }
            let mut out = Vec::new();
            for m in list {
                let b = match &m.percent {
                    Ok(p) => format!("{p}%"),
                    Err(e) => format!("cannot be read: {e}"),
                };
                out.push(format!("{} (id {}, {}): {b}{}", m.label, m.id.as_deref().unwrap_or("none"), m.connector, if m.chosen { ", CHOSEN" } else { "" }));
            }
            Ok(out.join("\n"))
        }));
    }
    {
        let (core, e1) = (cx.core(), env.clone());
        let schema = json!({"type": "object", "properties": {"id": {"type": "string", "description": "The id of the screen from monitor_brightness; leave out or say none to choose no screen."}}});
        v.push(Entry::change("Display", "monitor_choose", "Choose the one external monitor whose brightness may be set over DDC/CI, by the id monitor_brightness shows (nothing else is ever written to). The person approves a card first.", schema, |a| format!("Allow setting the brightness of monitor {}", a["id"].as_str().unwrap_or("none")), move |a| {
            let id = a.get("id").and_then(|x| x.as_str()).map(str::trim).filter(|s| !s.is_empty() && !s.eq_ignore_ascii_case("none"));
            core.monitors().choose(id)?;
            e1.audit("settings.display.monitor_choose", id.unwrap_or("none"));
            Ok(match id {
                Some(i) => format!("The screen {i} is chosen; monitor_brightness_set changes only this one."),
                None => "No screen is chosen now.".into(),
            })
        }));
    }
    {
        let (core, e1) = (cx.core(), env.clone());
        let schema = json!({"type": "object", "properties": {"percent": {"type": "integer", "description": "Brightness from 1 to 100 (never fully dark)."}}, "required": ["percent"]});
        v.push(Entry::change("Display", "monitor_brightness_set", "Set the brightness of the chosen external monitor over DDC/CI (1 to 100). Fails when no screen is chosen. The value is read back. The person approves a card first.", schema, |a| format!("Set the monitor brightness to {}%", a["percent"].as_i64().unwrap_or(0)), move |a| {
            let p = a.get("percent").and_then(|x| x.as_i64()).ok_or("percent is a number from 1 to 100")?;
            if !(1..=100).contains(&p) {
                return Err("percent must be from 1 to 100".into());
            }
            let got = core.monitors().set_brightness(p as u8)?;
            e1.audit("settings.display.monitor_brightness", &format!("{got}%"));
            Ok(format!("The monitor now reads back {got}%."))
        }));
    }
    v
}

#[cfg(test)]
#[path = "chat_monitor_tests.rs"]
mod tests;
