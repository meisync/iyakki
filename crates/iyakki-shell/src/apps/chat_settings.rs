//! The assistant's tools for Settings pages whose code lives in the shell: Keyboard (`keyboard_status`, `keyboard_set`), Time (`time_status`,
//! `time_set`, `clock_face_set`), Browser custom headers (`browser_headers_list`, `browser_headers_enable`, `browser_headers_delete`) and
//! Home layout groups (`home_layout_list`, `home_layout_group`). Each tool ends in the function the page calls (`apply_keyboard`, the
//! settings store, `browserheaders::Store`, `layout::Layout`), so a change by chat and a change on the page cannot differ. Reads are free,
//! every change asks. A header's value is a secret: it is typed on the page into the vault and never passes through the chat.
//! `wrap` also puts the agent crate's groups (views, notifications, screen saver, Apps access, Connect a client) in front of the other tools.

use std::sync::Arc;

use iyakki_agent::agent::Tools;
use iyakki_agent::browserheaders::{self as bh, Store, Values};
use iyakki_agent::keyboard::{KeyboardSettings, FN_MODES, TOUCHBAR_CHOICES};
use iyakki_agent::layout::{Layout, BUILTIN};
use iyakki_agent::settingstools::{arg_bool, arg_str, no_args, Entry, Env, SettingsTools};
use serde_json::{json, Value};

use super::AppCx;

pub const PROMPT_RULE: &str = "\nDISPLAY (Settings -> Display): displays_status says how each connected display looks to the computer and whether its list of sizes matches what it can do (free). displays_look_again reads the displays again when one shows a smaller size than it can do, for example a 4K monitor plugged in by USB-C that only offers 1920x1080 (free; it also happens by itself on a plug-in). KEYBOARD (Settings -> Keyboard, administrators): keyboard_status shows the input profile, function keys, key repeat, trackpad and scrolling choices (free); keyboard_set changes the ones you name and applies them at once (asks first). TIME (Settings -> Time): time_status shows the zone, network time and server (free); time_set changes them (administrators; asks first); clock_face_set changes the person's own 12-hour clock and seconds (asks first). POINTER SPEED (Settings -> Keyboard, administrators): pointer_speed_status shows how fast the mouse and the touchpad move, in percent (free); pointer_speed_set changes the mouse speed (mouse_pct) and/or the touchpad speed (touchpad_pct), 25 to 300 with 100 the default, applied at once (asks first). BROWSER CUSTOM HEADERS: browser_headers_list shows the rules without their values (free); browser_headers_enable and browser_headers_delete ask first. You cannot add a header or read its value: the person types the value on the Settings page, it goes to their vault. BROWSER ADS AND TRACKERS: browser_block_status shows the switch, the downloaded filter lists (rules, age, licence, source) and the allowed sites (free); browser_block_set switches blocking, browser_block_download fetches EasyList, EasyPrivacy, uBlock filters, all three (standard means EasyList and EasyPrivacy) or a list from an address the person gives, browser_block_update fetches the lists again, browser_block_remove removes every list and browser_block_allow lets one site through or stops that (each asks first; a download comes from the list's own server and is never shipped; the work runs in the background, ask browser_block_status for the result). KNOWN-BAD SITES: browser_badsites_status shows the list of dangerous sites the browser refuses to open (free); browser_badsites_add and browser_badsites_remove change it, browser_badsites_set switches it and browser_badsites_download adds a list such as urlhaus from its own server or an https address the person gives (each asks first; nothing is shipped). browser_certificate_view shows the certificate a site presents, its dates, names and whether it is trusted, with warnings (free; it only does the handshake). HOME LAYOUT: home_layout_list shows the groups (free); home_layout_group makes, renames, deletes a group of the person's own or resets the layout (asks first); an administrator can also use it to set or clear the default layout for new accounts.";

pub const TOOLS: [&str; 26] = ["browser_badsites_status", "browser_badsites_set", "browser_badsites_add", "browser_badsites_remove", "browser_badsites_download", "browser_certificate_view", "displays_status", "displays_look_again", "pointer_speed_status", "pointer_speed_set", "browser_block_status", "browser_block_set", "browser_block_download", "browser_block_update", "browser_block_remove", "browser_block_allow", "keyboard_status", "keyboard_set", "time_status", "time_set", "clock_face_set", "browser_headers_list", "browser_headers_enable", "browser_headers_delete", "home_layout_list", "home_layout_group"];

fn kb_describe(k: &KeyboardSettings) -> String {
    let on = |o: Option<bool>, d: bool| if o.unwrap_or(d) { "on" } else { "off" };
    format!(
        "Input profile: {}. Function keys: {}. Key repeat: delay {} ms, rate {} per second (0 = the kernel's own). Trackpad tap: {}, natural scrolling: {}, three-finger drag: {}. Scroll speed: {}%, wheel lines: {}, wheel natural: {}, smooth: {}, momentum: {}. Touch Bar: {}. ISO fix: {}, swap Option/Command: {}.",
        iyakki_agent::inputprofile::label(&k.profile),
        if k.fn_mode.is_empty() { "as the kernel has it" } else { &k.fn_mode },
        k.repeat_delay_ms,
        k.repeat_rate,
        on(k.trackpad_tap, true),
        on(k.trackpad_natural, true),
        on(k.three_finger_drag, false),
        k.scroll().speed_pct,
        k.scroll().wheel_lines,
        on(k.wheel_natural, false),
        on(k.scroll_smooth, true),
        on(k.scroll_momentum, true),
        if k.touchbar.is_empty() { "default" } else { &k.touchbar },
        on(k.iso_layout, false),
        on(k.swap_opt_cmd, false)
    )
}

fn pointer_describe(k: &KeyboardSettings) -> String {
    let (m, t) = k.pointer_speeds();
    format!("Mouse speed: {m}%. Touchpad speed: {t}% (100 is the default; the range is 25 to 300).")
}

/// The keyboard settings with the pointer speeds from the arguments; 0 and values outside 25..=300 are refused.
fn pointer_apply_args(mut k: KeyboardSettings, a: &Value) -> Result<KeyboardSettings, String> {
    let num = |key: &str| a.get(key).map(|v| v.as_u64().filter(|n| *n >= 25 && *n <= 300).ok_or_else(|| format!("{key} must be a whole number from 25 to 300")));
    if num("mouse_pct").is_none() && num("touchpad_pct").is_none() {
        return Err("say `mouse_pct` and/or `touchpad_pct` (25 to 300)".into());
    }
    if let Some(n) = num("mouse_pct") {
        k.pointer_speed_pct = n? as u32;
    }
    if let Some(n) = num("touchpad_pct") {
        k.touchpad_speed_pct = n? as u32;
    }
    k.validate()?;
    Ok(k)
}

fn pointer_entries(env: &Env) -> Vec<Entry> {
    let mut v = Vec::new();
    {
        let e = env.clone();
        v.push(Entry::read("Keyboard", "pointer_speed_status", "Show how fast the mouse and the touchpad move, in percent.", no_args(), move |_| Ok(pointer_describe(&e.settings.lock().map_err(|_| "settings are busy".to_string())?.get().keyboard))).admin());
    }
    {
        let (e, e2, e3) = (env.clone(), env.clone(), env.clone());
        let schema = json!({"type": "object", "properties": {
            "mouse_pct": {"type": "integer", "description": "mouse pointer speed in percent, 25 to 300 (100 is the default)"},
            "touchpad_pct": {"type": "integer", "description": "touchpad pointer speed in percent, 25 to 300 (100 is the default)"}}});
        v.push(
            Entry::change("Keyboard", "pointer_speed_set", "Change how fast the mouse and/or the touchpad pointer moves (25 to 300 percent); saved for this computer and applied at once. The person approves a card first.", schema, |a| {
                let names: Vec<&str> = ["mouse_pct", "touchpad_pct"].into_iter().filter(|k| a.get(*k).is_some()).collect();
                format!("Change the pointer speed ({})", names.join(", "))
            }, move |a| {
                let mut store = e.settings.lock().map_err(|_| "settings are busy".to_string())?;
                let next = pointer_apply_args(store.get().keyboard.clone(), a)?;
                let n2 = next.clone();
                store.update(|s| s.keyboard = n2).map_err(|x| x.to_string())?;
                drop(store);
                let (m, t) = next.pointer_speeds();
                e.audit("settings.pointer_speed", &format!("chat: mouse={m} touchpad={t}"));
                let (mouse, pad) = next.pointer_speeds();
                crate::input::request_pointer_speed(mouse, pad);
                Ok(format!("Saved and applied. {}", pointer_describe(&next)))
            })
            .with_preview(move |a| {
                let now = e2.settings.lock().map(|s| s.get().keyboard.clone()).unwrap_or_default();
                match pointer_apply_args(now.clone(), a) {
                    Ok(n) => format!("Now: {}\nAfter: {}", pointer_describe(&now), pointer_describe(&n)),
                    Err(x) => format!("Cannot change it: {x}"),
                }
            })
            .with_refusal(move |a| {
                let now = e3.settings.lock().map(|s| s.get().keyboard.clone()).unwrap_or_default();
                pointer_apply_args(now, a).err()
            })
            .admin(),
        );
    }
    v
}

fn kb_apply_args(mut k: KeyboardSettings, a: &Value) -> Result<KeyboardSettings, String> {
    let text = |key: &str| a.get(key).and_then(Value::as_str);
    let num = |key: &str| a.get(key).and_then(Value::as_u64).map(|n| n as u32);
    let flag = |key: &str| a.get(key).and_then(Value::as_bool);
    if let Some(p) = text("profile") {
        k.profile = if p == "automatic" { String::new() } else { p.to_string() };
    }
    if let Some(m) = text("fn_mode") {
        k.fn_mode = if m == "kernel" { String::new() } else { m.to_string() };
    }
    if let Some(n) = num("repeat_delay_ms") {
        k.repeat_delay_ms = n;
    }
    if let Some(n) = num("repeat_rate") {
        k.repeat_rate = n;
    }
    if let Some(n) = num("scroll_speed_pct") {
        k.scroll_speed_pct = n;
    }
    if let Some(n) = num("wheel_lines") {
        k.wheel_lines = n;
    }
    if let Some(b) = flag("trackpad_tap") {
        k.trackpad_tap = Some(b);
    }
    if let Some(b) = flag("trackpad_natural") {
        k.trackpad_natural = Some(b);
    }
    if let Some(b) = flag("three_finger_drag") {
        k.three_finger_drag = Some(b);
    }
    if let Some(b) = flag("wheel_natural") {
        k.wheel_natural = Some(b);
    }
    if let Some(b) = flag("scroll_smooth") {
        k.scroll_smooth = Some(b);
    }
    if let Some(b) = flag("scroll_momentum") {
        k.scroll_momentum = Some(b);
    }
    if let Some(t) = text("touchbar") {
        k.touchbar = if t == "default" { String::new() } else { t.to_string() };
    }
    k.validate()?;
    Ok(k)
}

const KB_KEYS: [&str; 13] = ["profile", "fn_mode", "repeat_delay_ms", "repeat_rate", "scroll_speed_pct", "wheel_lines", "trackpad_tap", "trackpad_natural", "three_finger_drag", "wheel_natural", "scroll_smooth", "scroll_momentum", "touchbar"];

fn keyboard_entries(env: &Env, sys: std::path::PathBuf) -> Vec<Entry> {
    let mut v = Vec::new();
    {
        let e = env.clone();
        v.push(Entry::read("Keyboard", "keyboard_status", "Show the keyboard, key repeat, trackpad and scrolling choices.", no_args(), move |_| Ok(kb_describe(&e.settings.lock().map_err(|_| "settings are busy".to_string())?.get().keyboard))).admin());
    }
    {
        let (e, e2, e3) = (env.clone(), env.clone(), env.clone());
        let schema = json!({"type": "object", "properties": {
            "profile": {"type": "string", "enum": ["automatic", "mac", "pc"]},
            "fn_mode": {"type": "string", "enum": ["kernel", "media", "fkeys", "auto"], "description": FN_MODES.join(",")},
            "repeat_delay_ms": {"type": "integer", "description": "100 to 2000, 0 = the kernel's own"},
            "repeat_rate": {"type": "integer", "description": "2 to 60 per second, 0 = the kernel's own"},
            "scroll_speed_pct": {"type": "integer", "description": "50 to 300"},
            "wheel_lines": {"type": "integer", "description": "1 to 10"},
            "trackpad_tap": {"type": "boolean"}, "trackpad_natural": {"type": "boolean"}, "three_finger_drag": {"type": "boolean"}, "wheel_natural": {"type": "boolean"}, "scroll_smooth": {"type": "boolean"}, "scroll_momentum": {"type": "boolean"},
            "touchbar": {"type": "string", "enum": ["default", "off", "keys", "strip"], "description": TOUCHBAR_CHOICES.join(",")}}});
        v.push(
            Entry::change("Keyboard", "keyboard_set", "Change the keyboard, key repeat, trackpad and scrolling choices you name; they are saved and applied at once. The person approves a card first.", schema, |a| {
                let names: Vec<&str> = KB_KEYS.iter().copied().filter(|k| a.get(*k).is_some()).collect();
                format!("Change the keyboard choices ({})", names.join(", "))
            }, move |a| {
                if !KB_KEYS.iter().any(|k| a.get(*k).is_some()) {
                    return Err(format!("say at least one of: {}", KB_KEYS.join(", ")));
                }
                let mut store = e.settings.lock().map_err(|_| "settings are busy".to_string())?;
                let next = kb_apply_args(store.get().keyboard.clone(), a)?;
                let n2 = next.clone();
                store.update(|s| s.keyboard = n2).map_err(|x| x.to_string())?;
                drop(store);
                e.audit("settings.keyboard", &format!("chat: {}", KB_KEYS.iter().filter(|k| a.get(**k).is_some()).copied().collect::<Vec<_>>().join(",")));
                match super::settings::apply_keyboard(&next, &sys) {
                    Ok(()) => Ok(format!("Saved and applied. {}", kb_describe(&next))),
                    Err(x) => Ok(format!("Saved, but not applied now: {x}. {}", kb_describe(&next))),
                }
            })
            .with_preview(move |a| {
                let now = e2.settings.lock().map(|s| s.get().keyboard.clone()).unwrap_or_default();
                match kb_apply_args(now.clone(), a) {
                    Ok(n) => format!("Now: {}\nAfter: {}", kb_describe(&now), kb_describe(&n)),
                    Err(x) => format!("Cannot change it: {x}"),
                }
            })
            .with_refusal(move |a| {
                let now = e3.settings.lock().map(|s| s.get().keyboard.clone()).unwrap_or_default();
                kb_apply_args(now, a).err()
            })
            .admin(),
        );
    }
    v
}

fn time_describe(t: &iyakki_agent::settings::TimeSettings, face: (bool, bool)) -> String {
    format!(
        "Time zone: {}{}. Clock set from the network: {}. Time server: {}. This person's clock: {}{}.",
        if t.zone.is_empty() { "what the machine has" } else { &t.zone },
        if t.zone_auto { " (detected automatically)" } else { "" },
        if t.no_ntp { "no" } else { "yes" },
        if t.ntp_server.is_empty() { "the built-in ones" } else { &t.ntp_server },
        if face.0 { "12-hour" } else { "24-hour" },
        if face.1 { " with seconds" } else { "" }
    )
}

fn time_entries(env: &Env) -> Vec<Entry> {
    let mut v = Vec::new();
    {
        let e = env.clone();
        v.push(Entry::read("Time", "time_status", "Show the time zone, whether the clock is set from the network, the time server, and this person's own clock face.", no_args(), move |_| {
            let t = e.settings.lock().map_err(|_| "settings are busy".to_string())?.get().time.clone();
            Ok(time_describe(&t, iyakki_uikit::uisize::load_clock(&e.dir, &e.user)))
        }));
    }
    {
        let (e, e2) = (env.clone(), env.clone());
        let schema = json!({"type": "object", "properties": {
            "zone": {"type": "string", "description": "A zone name such as Europe/Paris, or empty for what the machine says. Choosing one switches automatic detection off."},
            "zone_auto": {"type": "boolean", "description": "Detect the zone from the public IP address (asks an internet service)."},
            "set_from_network": {"type": "boolean"},
            "ntp_server": {"type": "string", "description": "A host name, or empty for the built-in ones."}}});
        let check = |a: &Value| -> Result<(), String> {
            if let Some(z) = a.get("zone").and_then(Value::as_str) {
                if !z.is_empty() && !iyakki_agent::clock::zone_names().iter().any(|n| n == z) {
                    return Err(format!("{z} is not a time zone name (like Europe/Paris)"));
                }
            }
            if !["zone", "zone_auto", "set_from_network", "ntp_server"].iter().any(|k| a.get(*k).is_some()) {
                return Err("say `zone`, `zone_auto`, `set_from_network` or `ntp_server`".into());
            }
            Ok(())
        };
        v.push(
            Entry::change("Time", "time_set", "Change the machine's time zone, automatic zone detection, network time or time server. The person approves a card first.", schema, |a| format!("Change the time settings ({})", ["zone", "zone_auto", "set_from_network", "ntp_server"].iter().filter(|k| a.get(**k).is_some()).copied().collect::<Vec<_>>().join(", ")), move |a| {
                check(a)?;
                let mut store = e.settings.lock().map_err(|_| "settings are busy".to_string())?;
                store
                    .update(|s| {
                        if let Some(z) = a.get("zone").and_then(Value::as_str) {
                            s.time.zone = z.to_string();
                            s.time.zone_auto = false;
                        }
                        if let Some(b) = a.get("zone_auto").and_then(Value::as_bool) {
                            s.time.zone_auto = b;
                        }
                        if let Some(b) = a.get("set_from_network").and_then(Value::as_bool) {
                            s.time.no_ntp = !b;
                        }
                        if let Some(h) = a.get("ntp_server").and_then(Value::as_str) {
                            s.time.ntp_server = h.trim().to_string();
                        }
                    })
                    .map_err(|x| x.to_string())?;
                let t = store.get().time.clone();
                drop(store);
                iyakki_agent::clock::set_chosen_zone(&t.zone);
                e.audit("settings.time", &format!("chat: zone={} auto={} no_ntp={}", t.zone, t.zone_auto, t.no_ntp));
                Ok(time_describe(&t, iyakki_uikit::uisize::load_clock(&e.dir, &e.user)))
            })
            .with_refusal(move |a| check(a).err())
            .admin(),
        );
        let _ = e2;
    }
    {
        let e = env.clone();
        let schema = json!({"type": "object", "properties": {"hour12": {"type": "boolean"}, "seconds": {"type": "boolean"}}});
        v.push(Entry::change("Time", "clock_face_set", "Change this person's own clock: 12-hour with AM/PM, and seconds in the top bar. The person approves a card first.", schema, |_| "Change the clock face".into(), move |a| {
            if a.get("hour12").is_none() && a.get("seconds").is_none() {
                return Err("say `hour12` or `seconds`".into());
            }
            let (h, s) = iyakki_uikit::uisize::load_clock(&e.dir, &e.user);
            let (h, s) = (a.get("hour12").and_then(Value::as_bool).unwrap_or(h), a.get("seconds").and_then(Value::as_bool).unwrap_or(s));
            iyakki_uikit::uisize::save_clock(&e.dir, &e.user, h, s);
            e.audit("settings.time.clock", &format!("hour12={h} seconds={s}"));
            Ok(format!("The clock is {}{}.", if h { "12-hour" } else { "24-hour" }, if s { " with seconds" } else { "" }))
        }));
    }
    v
}

fn header_entries(env: &Env, values: Arc<dyn Values>) -> Vec<Entry> {
    let path = super::browser_headers::rules_path_for(&env.dir, &env.user);
    let store = Arc::new(Store::new(&path, values));
    let find = |s: &Store, which: &str| -> Result<bh::HeaderRule, String> {
        let rules = s.list();
        rules.iter().find(|r| r.id == which).or_else(|| rules.iter().find(|r| r.name.eq_ignore_ascii_case(which))).cloned().ok_or_else(|| format!("There is no header rule {which}; call browser_headers_list"))
    };
    let mut v = Vec::new();
    {
        let s = store.clone();
        v.push(Entry::read("Browser", "browser_headers_list", "List the browser's custom request headers: name, header, sites, on or off. Values are never shown.", no_args(), move |_| {
            let rules = s.list();
            if rules.is_empty() {
                return Ok("No custom header rules.".into());
            }
            Ok(rules.iter().map(|r| format!("{} [{}]: {}: {} -> {}{}{} ({})", r.name, r.id, r.header, bh::MASK, if r.domains.is_empty() { "-" } else { &r.domains }, if r.prefix.is_empty() { String::new() } else { format!(" below {}", r.prefix) }, if r.path_regex.is_empty() { String::new() } else { format!(" path {}", r.path_regex) }, if r.enabled { "on" } else { "off" })).collect::<Vec<_>>().join("\n"))
        }));
    }
    let sel = json!({"type": "object", "properties": {"rule": {"type": "string", "description": "The rule's id or name from browser_headers_list."}}, "required": ["rule"]});
    {
        let (s, e) = (store.clone(), env.clone());
        let schema = json!({"type": "object", "properties": {"rule": {"type": "string"}, "on": {"type": "boolean"}}, "required": ["rule", "on"]});
        v.push(Entry::change("Browser", "browser_headers_enable", "Turn one custom request header rule on or off. The person approves a card first.", schema, |a| format!("Turn the header rule {} {}", a["rule"].as_str().unwrap_or("?"), if a["on"].as_bool() == Some(false) { "off" } else { "on" }), move |a| {
            let r = find(&s, &arg_str(a, "rule")?)?;
            let on = arg_bool(a, "on")?;
            s.set_enabled(&r.id, on)?;
            e.audit("settings.browser.headers", &format!("Custom header rule \u{201C}{}\u{201D} {}", r.name, if on { "turned on" } else { "turned off" }));
            Ok(format!("{} is {}.", r.name, if on { "on" } else { "off" }))
        }));
    }
    {
        let (s, e) = (store, env.clone());
        v.push(Entry::change("Browser", "browser_headers_delete", "Delete one custom request header rule and its value in the vault. The person approves a card first.", sel, |a| format!("Delete the header rule {}", a["rule"].as_str().unwrap_or("?")), move |a| {
            let r = find(&s, &arg_str(a, "rule")?)?;
            s.delete(&r.id)?;
            e.audit("settings.browser.headers", &format!("Custom header rule \u{201C}{}\u{201D} deleted", r.name));
            Ok(format!("Deleted {}.", r.name))
        }));
    }
    v
}

/// Settings -> Display: how the connected displays look to the kernel (free) and "Look again" (free: it only asks the kernel to read the
/// displays again, a few times in the background; nothing is switched off or changed).
fn display_entries(env: &Env, sys_drm: std::path::PathBuf) -> Vec<Entry> {
    let _ = env;
    let (a, b) = (sys_drm.clone(), sys_drm);
    vec![
        Entry::read("Display", "displays_status", "Show the connected displays and whether the list of sizes each one offers matches what the display says it can do.", no_args(), move |_| Ok(crate::display::status_text(&a))),
        Entry::read("Display", "displays_look_again", "Ask the computer to read the connected displays again (use it when a display shows a smaller size than it can do). It retries a few times in the background and draws at the best size by itself. Changes nothing else.", no_args(), move |_| {
            let line = crate::display::look_again(&b);
            Ok(format!("Looking at the displays again: {line}. A display that now offers a bigger size is used within about fifteen seconds."))
        }),
    ]
}

fn layout_entries(env: &Env) -> Vec<Entry> {
    let mut v = Vec::new();
    {
        let e = env.clone();
        v.push(Entry::read("HomeLayout", "home_layout_list", "List the Home screen groups: the built-in ones and the person's own, with how many tiles were moved into each.", no_args(), move |_| {
            let l = Layout::load(&e.dir, &e.user);
            let mut out: Vec<String> = BUILTIN.iter().map(|(id, label)| format!("{label} [{id}] (built in, {} moved here)", l.count_in(id))).collect();
            out.extend(l.groups.iter().map(|g| format!("{} [{}] ({} tiles)", g.label, g.id, l.count_in(&g.id))));
            Ok(out.join("\n"))
        }));
    }
    {
        let e = env.clone();
        let schema = json!({"type": "object", "properties": {"action": {"type": "string", "enum": ["create", "rename", "delete", "reset", "set_default_for_new_accounts", "clear_default_for_new_accounts"], "description": "reset puts every tile back in its own group. The last two are for administrators: make the current layout the default for accounts that have no layout yet, or remove that default."}, "name": {"type": "string", "description": "The group's name (create), or its new name (rename). Up to 16 characters."}, "group": {"type": "string", "description": "The group's id or name (rename, delete)."}}, "required": ["action"]});
        v.push(Entry::change("HomeLayout", "home_layout_group", "Make, rename or delete a Home screen group of the person's own, or reset the whole layout. Built-in groups stay. The person approves a card first.", schema, |a| format!("Home layout: {} {}", a["action"].as_str().unwrap_or("change"), a.get("name").or(a.get("group")).and_then(Value::as_str).unwrap_or("")), move |a| {
            let mut l = Layout::load(&e.dir, &e.user);
            let msg = match arg_str(a, "action")?.as_str() {
                "create" => {
                    let name = arg_str(a, "name")?;
                    l.create_group(&name)?;
                    format!("Made the group {name}.")
                }
                "rename" => {
                    let id = l.find(&arg_str(a, "group")?).ok_or("There is no such group")?;
                    let name = arg_str(a, "name")?;
                    l.rename_group(&id, &name)?;
                    format!("Renamed to {name}.")
                }
                "delete" => {
                    let id = l.find(&arg_str(a, "group")?).ok_or("There is no such group")?;
                    let n = l.delete_group(&id)?;
                    e.audit("layout.group.delete", &id);
                    format!("Group deleted. {n} tiles went back to their own group.")
                }
                "reset" => {
                    l.reset();
                    "Every tile is back in its own group.".to_string()
                }
                "set_default_for_new_accounts" => {
                    if !e.admin {
                        return Err("Only an administrator sets the default layout for new accounts.".into());
                    }
                    if !l.save_default(&e.dir) {
                        return Err("The default layout could not be saved.".into());
                    }
                    e.audit("layout.default.set", "");
                    return Ok("This layout is now the default for new accounts.".into());
                }
                "clear_default_for_new_accounts" => {
                    if !e.admin {
                        return Err("Only an administrator clears the default layout for new accounts.".into());
                    }
                    Layout::clear_default(&e.dir);
                    e.audit("layout.default.clear", "");
                    return Ok("New accounts start with the plain layout again.".into());
                }
                other => return Err(format!("action is create, rename, delete, reset, set_default_for_new_accounts or clear_default_for_new_accounts, not {other}")),
            };
            l.save(&e.dir, &e.user);
            e.audit("layout.edit", &msg);
            Ok(msg)
        }));
    }
    v
}

/// The certificate chain probe the tool uses; tests put their own here so nothing touches the network.
#[cfg(test)]
pub(crate) static CERT_FAKE: std::sync::Mutex<Option<iyakki_agent::iyakki_proxy::certview::View>> = std::sync::Mutex::new(None);

pub(crate) fn cert_probe(host: &str, port: u16) -> Result<iyakki_agent::iyakki_proxy::certview::View, String> {
    #[cfg(test)]
    {
        if let Some(v) = CERT_FAKE.lock().ok().and_then(|g| g.clone()) {
            return Ok(v);
        }
        return Err(format!("{host}:{port}: tests do not use the network"));
    }
    #[allow(unreachable_code)]
    iyakki_agent::iyakki_proxy::certview::probe(host, port, iyakki_agent::clock::now_unix() as i64)
}

/// The known-bad site list and the certificate details view (F-BROW-98).
fn safe_browsing_entries(cx: &AppCx, env: &Env) -> Vec<Entry> {
    use super::browser_badsites as bs;
    let mut v = Vec::new();
    {
        let c = cx.clone();
        v.push(Entry::read("Browser", "browser_badsites_status", "Show the known-bad site list: whether it is on, how many sites it holds, the first entries with their reasons, the suggested lists and what a running download says. Read only.", no_args(), move |_| {
            let list = bs::load_list(&c);
            let mut out = vec![format!("The known-bad site list is {} and holds {} site{}.", if bs::enabled(&c) { "on" } else { "off" }, list.len(), if list.len() == 1 { "" } else { "s" })];
            for (h, r) in list.entries(20) {
                out.push(if r.is_empty() { format!("- {h}") } else { format!("- {h}: {r}") });
            }
            if list.len() > 20 {
                out.push(format!("... and {} more", list.len() - 20));
            }
            for s in iyakki_web::badsites::SUGGESTED {
                out.push(format!("Suggested list {} [{}]: {}; licence {}. browser_badsites_download fetches it (the person approves first).", s.name, s.id, s.what, s.licence));
            }
            let job = bs::job_status();
            if !job.is_empty() {
                out.push(format!("Last download job: {job}"));
            }
            Ok(out.join("\n"))
        }));
    }
    {
        let (c, e) = (cx.clone(), env.clone());
        let schema = json!({"type": "object", "properties": {"on": {"type": "boolean"}}, "required": ["on"]});
        v.push(Entry::change("Browser", "browser_badsites_set", "Switch the known-bad site list on or off. While it is on, a listed site (and every site under it) is not opened. The person approves a card first.", schema, |a| format!("Turn the known-bad site list {}", if a["on"].as_bool().unwrap_or(true) { "on" } else { "off" }), move |a| {
            let on = arg_bool(a, "on")?;
            bs::set_enabled(&c, on);
            e.audit("settings.browser", &format!("Known-bad site list: {}.", if on { "on" } else { "off" }));
            Ok(format!("The known-bad site list is {}.", if on { "on" } else { "off" }))
        }));
    }
    {
        let (c, e) = (cx.clone(), env.clone());
        let schema = json!({"type": "object", "properties": {"site": {"type": "string", "description": "A site name such as example.org (without http:// or a path); every site under it is covered too"}, "reason": {"type": "string", "description": "Why it is listed (shown on the page that explains the block)"}}, "required": ["site"]});
        v.push(Entry::change("Browser", "browser_badsites_add", "Put one site on the known-bad site list: it and every site under it will not be opened and the person is told why. The person approves a card first.", schema, |a| format!("Never open {} (known-bad site list)", a["site"].as_str().unwrap_or("?")), move |a| {
            let site = arg_str(a, "site")?;
            let reason = a["reason"].as_str().unwrap_or("").trim().to_string();
            let h = bs::add(&c, &site, &reason)?;
            e.audit("settings.browser", &format!("Known-bad site list: added {h}"));
            Ok(format!("{h} is on the known-bad site list; it and the sites under it will not open."))
        }));
    }
    {
        let (c, e) = (cx.clone(), env.clone());
        let schema = json!({"type": "object", "properties": {"site": {"type": "string", "description": "The site name as listed, or 'all' to empty the list"}}, "required": ["site"]});
        v.push(Entry::change("Browser", "browser_badsites_remove", "Take one site off the known-bad site list (or empty the list with site all). The person approves a card first.", schema, |a| format!("Take {} off the known-bad site list", a["site"].as_str().unwrap_or("?")), move |a| {
            let site = arg_str(a, "site")?;
            if site.trim().eq_ignore_ascii_case("all") {
                let n = bs::clear(&c);
                e.audit("settings.browser", "Known-bad site list: emptied");
                return Ok(format!("The known-bad site list is empty ({n} removed)."));
            }
            if bs::remove(&c, &site) {
                e.audit("settings.browser", &format!("Known-bad site list: removed {site}"));
                Ok(format!("{} is off the known-bad site list.", site.trim()))
            } else {
                Err(format!("{} is not on the list (a site covered only through a parent site is removed by naming the parent)", site.trim()))
            }
        }));
    }
    {
        let (c, e) = (cx.clone(), env.clone());
        let schema = json!({"type": "object", "properties": {"list": {"type": "string", "description": "urlhaus (suggested), or leave out and give url"}, "url": {"type": "string", "description": "Instead of list: an https address of a text list (one site per line, or a hosts file)"}}});
        let describe = |a: &Value| match (a["url"].as_str(), a["list"].as_str()) {
            (Some(u), _) if !u.is_empty() => format!("Download the known-bad site list at {u}"),
            (_, Some(l)) => format!("Download the known-bad site list {l} from its own server"),
            _ => "Download a known-bad site list".to_string(),
        };
        v.push(Entry::change("Browser", "browser_badsites_download", "Download a list of dangerous sites and add them to the known-bad site list: urlhaus (the suggested one) or an https address; at most 16 MB, it runs in the background. Lines that name one file (a full address) are skipped. The person approves a card first.", schema, describe, move |a| {
            let url = a["url"].as_str().map(str::trim).filter(|u| !u.is_empty());
            let (addr, what) = match (url, a["list"].as_str().map(str::trim).filter(|l| !l.is_empty())) {
                (Some(u), _) => {
                    if !u.starts_with("https://") {
                        return Err("The address must start with https://".into());
                    }
                    (u.to_string(), format!("the list at {u}"))
                }
                (None, Some(id)) => {
                    let s = iyakki_web::badsites::SUGGESTED.iter().find(|s| s.id == id || s.name.eq_ignore_ascii_case(id)).ok_or_else(|| "The list is urlhaus, or give an https address as url".to_string())?;
                    (s.url.to_string(), s.name.to_string())
                }
                (None, None) => return Err("Say which list (urlhaus) or give an https address as url".into()),
            };
            e.audit("settings.browser", &format!("Known-bad site list: downloading {what} (by chat)"));
            if bs::start_download(bs::list_path(&c), bs::source(&c), addr) {
                Ok(format!("Downloading {what} in the background; browser_badsites_status shows the result."))
            } else {
                Err("A list download is already running.".into())
            }
        }));
    }
    {
        let c = cx.clone();
        let schema = json!({"type": "object", "properties": {"host": {"type": "string", "description": "The site name, for example www.example.org"}, "port": {"type": "integer", "description": "Default 443"}}, "required": ["host"]});
        v.push(Entry::read("Browser", "browser_certificate_view", "Show the certificate a site presents: who it was issued to and by, its dates, fingerprint, whether its names fit the address and whether it leads to a trusted authority, with warnings. It connects to the site and does only the TLS handshake (nothing is requested or sent). Subject to the account's list of allowed sites.", schema, move |a| {
            let host = arg_str(a, "host")?.trim().trim_end_matches('.').to_ascii_lowercase();
            if host.is_empty() || host.len() > 253 || !host.chars().all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '-' | '.' | '_')) {
                return Err("That is not a site name (write it like www.example.org)".into());
            }
            let port = a["port"].as_u64().unwrap_or(443);
            if port == 0 || port > 65535 {
                return Err("The port must be 1 to 65535".into());
            }
            if !c.session.policy.allows_host(&host) {
                return Err(format!("Your account may not open pages from {host}."));
            }
            Ok(cert_probe(&host, port as u16)?.text())
        }));
    }
    v
}

fn block_entries(cx: &AppCx, env: &Env) -> Vec<Entry> {
    use super::browser_block as bb;
    let mut v = Vec::new();
    {
        let c = cx.clone();
        v.push(Entry::read("Browser", "browser_block_status", "Show the browser's ads and trackers blocking: the switch, each downloaded filter list (rules, age, licence, source), the sites allowed, and what a running download says. Read only.", no_args(), move |_| {
            let (cfg, lists, now) = (bb::load(&c), bb::store(&c).list(), bb::now_secs());
            let mut out = vec![format!("Blocking ads and trackers is {}.", if cfg.enabled { "on" } else { "off" })];
            if lists.is_empty() {
                out.push("No filter list is downloaded, so nothing is blocked. browser_block_download fetches one (the person approves first).".into());
            }
            for m in &lists {
                let from = if m.url.is_empty() { "a file the person chose".to_string() } else { iyakki_sec::url_host(&m.url).unwrap_or_else(|| "an address".into()) };
                out.push(format!("{} [{}]{}: {} rules, {}, licence {}, from {from}", m.name, m.id, if m.enabled { "" } else { " (off)" }, m.rules, bb::ago(m.updated, now), m.licence));
            }
            if !cfg.allow.is_empty() {
                out.push(format!("Allowed sites: {}.", cfg.allow.iter().cloned().collect::<Vec<_>>().join(", ")));
            }
            let job = bb::job_status();
            if !job.is_empty() {
                out.push(format!("Last download job: {job}"));
            }
            Ok(out.join("\n"))
        }));
    }
    {
        let (c, e) = (cx.clone(), env.clone());
        let schema = json!({"type": "object", "properties": {"on": {"type": "boolean"}}, "required": ["on"]});
        v.push(Entry::change("Browser", "browser_block_set", "Switch blocking of ads and trackers on or off for the person. The person approves a card first.", schema, |a| format!("Turn ads and trackers blocking {}", if a["on"].as_bool().unwrap_or(true) { "on" } else { "off" }), move |a| {
            let on = arg_bool(a, "on")?;
            let mut cfg = bb::load(&c);
            cfg.enabled = on;
            bb::save(&c, &cfg);
            bb::apply(&c);
            e.audit("settings.browser", &format!("Block ads and trackers: {}.", if on { "on" } else { "off" }));
            Ok(format!("Blocking ads and trackers is {}.", if on { "on" } else { "off" }))
        }));
    }
    {
        let (c, e) = (cx.clone(), env.clone());
        let schema = json!({"type": "object", "properties": {"list": {"type": "string", "description": "easylist, easyprivacy, ublock-filters or standard (EasyList and EasyPrivacy)"}, "url": {"type": "string", "description": "Instead of list: an https:// address of a filter list"}}});
        let describe = |a: &Value| match (a["url"].as_str(), a["list"].as_str()) {
            (Some(u), _) if !u.is_empty() => format!("Download the filter list at {u}"),
            (_, Some(l)) => format!("Download the filter list {l} from its own server"),
            _ => "Download a filter list".to_string(),
        };
        v.push(Entry::change("Browser", "browser_block_download", "Download a filter list for the ads and trackers blocker: easylist, easyprivacy, ublock-filters, standard (EasyList and EasyPrivacy) or an https address. It comes from the list's own server, at most 24 MB, 12 lists; it runs in the background. The person approves a card first.", schema, describe, move |a| {
            let url = a["url"].as_str().map(str::trim).filter(|u| !u.is_empty());
            let (job, what) = match (url, a["list"].as_str().map(str::trim).filter(|l| !l.is_empty())) {
                (Some(u), _) => {
                    if !u.starts_with("https://") {
                        return Err("The address must start with https://".into());
                    }
                    (bb::Job::AddUrl(u.to_string()), format!("the list at {u}"))
                }
                (None, Some("standard")) => (bb::Job::DownloadStandard, "EasyList and EasyPrivacy".to_string()),
                (None, Some(id)) => {
                    let k = iyakki_web::blocker_lists::KNOWN.iter().find(|k| k.id == id || k.name.eq_ignore_ascii_case(id)).ok_or_else(|| "The list is easylist, easyprivacy, ublock-filters or standard, or give an https address as url".to_string())?;
                    (bb::Job::AddKnown(k.id.to_string()), k.name.to_string())
                }
                (None, None) => return Err("Say which list (easylist, easyprivacy, ublock-filters, standard) or give an https address as url".into()),
            };
            e.audit("settings.browser", &format!("Filter lists: downloading {what} (by chat)"));
            if bb::start_job(bb::lists_dir(&c), bb::source(&c), job) {
                Ok(format!("Downloading {what} in the background; browser_block_status shows the result."))
            } else {
                Err("A list job is already running.".into())
            }
        }));
    }
    {
        let (c, e) = (cx.clone(), env.clone());
        v.push(Entry::change("Browser", "browser_block_update", "Download every filter list that has an address again. The person approves a card first.", no_args(), |_| "Update the filter lists".to_string(), move |_| {
            e.audit("settings.browser", "Filter lists: updating (by chat)");
            if bb::start_job(bb::lists_dir(&c), bb::source(&c), bb::Job::UpdateAll) {
                Ok("Updating the lists in the background; browser_block_status shows the result.".into())
            } else {
                Err("A list job is already running.".into())
            }
        }));
    }
    {
        let (c, e) = (cx.clone(), env.clone());
        v.push(Entry::change("Browser", "browser_block_remove", "Remove every downloaded filter list (nothing is blocked afterwards until a list is downloaded again). The person approves a card first.", no_args(), |_| "Remove the filter lists".to_string(), move |_| {
            let store = bb::store(&c);
            let n = store.list().len();
            for m in store.list() {
                store.remove(&m.id);
            }
            bb::forget_loaded();
            bb::apply(&c);
            e.audit("settings.browser", "Removed the filter lists");
            Ok(if n == 0 { "There were no lists.".to_string() } else { format!("Removed {n} list{}.", if n == 1 { "" } else { "s" }) })
        }));
    }
    {
        let (c, e) = (cx.clone(), env.clone());
        let schema = json!({"type": "object", "properties": {"site": {"type": "string", "description": "A host name such as example.org"}, "allow": {"type": "boolean", "description": "true lets the site through the blocker, false blocks it again"}}, "required": ["site", "allow"]});
        v.push(Entry::change("Browser", "browser_block_allow", "Let one site through the ads and trackers blocker, or stop doing so. The person approves a card first.", schema, |a| format!("{} {} for ads and trackers blocking", if a["allow"].as_bool().unwrap_or(true) { "Allow" } else { "Block again" }, a["site"].as_str().unwrap_or("?")), move |a| {
            let site = arg_str(a, "site")?.trim().to_ascii_lowercase();
            if site.is_empty() || site.len() > 253 || !site.chars().all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '-' | '.' | '_')) {
                return Err("That is not a host name".into());
            }
            let allow = arg_bool(a, "allow")?;
            let mut cfg = bb::load(&c);
            if allow {
                if cfg.allow.len() >= 500 {
                    return Err("500 sites are already allowed".into());
                }
                cfg.allow.insert(site.clone());
            } else {
                cfg.allow.remove(&site);
            }
            bb::save(&c, &cfg);
            bb::apply(&c);
            e.audit("settings.browser", &format!("Ads and trackers: {site} {}", if allow { "allowed" } else { "blocked again" }));
            Ok(format!("{site} is {}.", if allow { "allowed through the blocker" } else { "blocked again" }))
        }));
    }
    v
}

/// What the WiFi chat tools act on (nothing real in unit tests, like the page's own environment).
fn wifi_ctl() -> iyakki_agent::settingstools_wifi::WifiCtl {
    #[cfg(test)]
    {
        return iyakki_agent::settingstools_wifi::WifiCtl::offline();
    }
    #[allow(unreachable_code)]
    iyakki_agent::settingstools_wifi::WifiCtl::real()
}

/// Every Settings tool of this module and of the agent crate, in front of `inner`.
pub fn wrap(cx: &AppCx, inner: Arc<dyn Tools>) -> Arc<dyn Tools> {
    let audit = cx.audit.clone();
    let user = cx.session.user.clone();
    let admin = cx.session.policy.manage_settings;
    let env = Env { dir: cx.services.dir().to_path_buf(), user: user.clone(), admin, settings: cx.services.settings.clone(), audit: Arc::new(move |a, d| audit.record(&user, a, d)) };
    let mut entries = Vec::new();
    entries.extend(iyakki_agent::settingstools_views::entries(&env));
    entries.extend(iyakki_agent::settingstools_backup::entries(&env));
    entries.extend(iyakki_agent::settingstools_notifs::entries(&env));
    entries.extend(iyakki_agent::settingstools_saver::entries(&env));
    entries.extend(iyakki_agent::settingstools_wasmtiles::entries(&env));
    entries.extend(iyakki_agent::settingstools_startup::entries(&env));
    entries.extend({
        let svc = cx.services.clone();
        iyakki_agent::settingstools_gemini::entries(&env, Arc::new(move || svc.secrets.resolve(iyakki_agent::gemini::KEY_REF, None::<&iyakki_agent::vault::LocalVault>).is_ok()))
    });
    entries.extend(iyakki_agent::settingstools_sound::entries(&env));
    entries.extend(super::chat_settings_assistant::entries(cx, &env));
    entries.extend(iyakki_agent::settingstools_memory::entries(&env));
    entries.extend(iyakki_agent::settingstools_linux::entries(&env, iyakki_agent::settingstools_linux::Access { allowed: cx.session.policy.cli == iyakki_sec::policy::CliLevel::Full, net_hosts: cx.session.policy.net_hosts.clone() }));
    entries.extend(iyakki_agent::settingstools_access::entries(&env));
    entries.extend(iyakki_agent::settingstools_machine::entries(&env));
    entries.extend(iyakki_agent::settingstools_podtls::entries(&env));
    entries.extend(iyakki_agent::settingstools_net::entries(&env, iyakki_fw::service::Handle::Global));
    entries.extend({
        let svc = cx.services.clone();
        let user = cx.session.user.clone();
        let read: iyakki_agent::settingstools_wifi::SecretReader = Arc::new(move |e: &str| svc.user_get(&user, e).map(|z| z.to_string()).map_err(|_| format!("there is no usable vault entry \"{e}\" (the vault may be locked). Ask the person to store it with vault_request, then pass its name as secret_entry.")));
        iyakki_agent::settingstools_wifi::entries(&env, wifi_ctl(), read)
    });
    entries.extend(keyboard_entries(&env, iyakki_agent::sysinfo::sys_root().into()));
    entries.extend(pointer_entries(&env));
    entries.extend(time_entries(&env));
    entries.extend(header_entries(&env, super::browser_headers::vault_values(&cx.services, &cx.session.user)));
    entries.extend(layout_entries(&env));
    entries.extend(display_entries(&env, crate::display::drm_dir()));
    entries.extend(block_entries(cx, &env));
    entries.extend(safe_browsing_entries(cx, &env));
    entries.extend(super::chat_browser_data::entries(cx, &env));
    entries.extend(super::chat_camvision::entries(cx, &env));
    entries.extend(super::chat_themes::entries(cx, &env));
    entries.extend(super::chat_hostname::entries(&env));
    Arc::new(SettingsTools::new(inner, admin, entries))
}

/// The paragraphs of the system prompt about these tools.
pub fn prompt_rules(admin: bool) -> String {
    let mut s = String::new();
    s.push_str(iyakki_agent::settingstools_views::PROMPT_RULE);
    s.push_str(iyakki_agent::settingstools_backup::PROMPT_RULE);
    s.push_str(iyakki_agent::settingstools_notifs::PROMPT_RULE);
    s.push_str(iyakki_agent::settingstools_saver::PROMPT_RULE);
    s.push_str(iyakki_agent::settingstools_wasmtiles::PROMPT_RULE);
    s.push_str(iyakki_agent::settingstools_startup::PROMPT_RULE);
    s.push_str(iyakki_agent::settingstools_gemini::PROMPT_RULE);
    s.push_str(iyakki_agent::settingstools_sound::PROMPT_RULE);
    s.push_str(super::chat_settings_assistant::PROMPT_RULE);
    s.push_str(iyakki_agent::settingstools_memory::PROMPT_RULE);
    s.push_str(iyakki_agent::settingstools_linux::PROMPT_RULE);
    if admin {
        s.push_str(iyakki_agent::settingstools_access::PROMPT_RULE);
    }
    s.push_str(iyakki_agent::settingstools_machine::PROMPT_RULE);
    if admin {
        s.push_str(iyakki_agent::settingstools_podtls::PROMPT_RULE);
    }
    s.push_str(iyakki_agent::settingstools_net::MEDIA_RULE);
    if admin {
        s.push_str(iyakki_agent::settingstools_net::PROMPT_RULE);
        s.push_str(iyakki_agent::settingstools_wifi::PROMPT_RULE);
    }
    s.push_str(PROMPT_RULE);
    s.push_str(super::chat_browser_data::PROMPT_RULE);
    s.push_str(super::chat_camvision::PROMPT_RULE);
    s.push_str(super::chat_themes::PROMPT_RULE);
    s.push_str(super::chat_hostname::PROMPT_RULE);
    s
}

#[cfg(test)]
#[path = "chat_settings_tests.rs"]
mod tests;
#[cfg(test)]
#[path = "chat_reachability_tests.rs"]
mod reach;
