//! Chat tools for Settings -> Sound, the output (F-SND-02): `sound_output_status` (free) says which output IYAKKI plays on and why, whether
//! the stored choice is set aside, and lists every output with its monitor; `sound_output_set` (asks every time, administrators) chooses
//! an output or goes back to Automatic. Same path as the page: `update_sound_in` (settings file, remembered volume, selector).

use serde_json::{json, Value};

use crate::audioout::{self, Kind, Status};
use crate::settingstools::{no_args, Entry, Env};
pub use iyakki_shared::settingstools_sound::{PROMPT_RULE};

const TAB: &str = "Sound";

pub const TOOLS: [&str; 2] = ["sound_output_status", "sound_output_set"];


fn kind_word(k: Kind) -> &'static str {
    match k {
        Kind::Hdmi => "HDMI/DP",
        Kind::Usb => "USB",
        Kind::Bluetooth => "Bluetooth",
        Kind::Analog => "analog",
        Kind::Digital => "digital",
    }
}

/// The status as text: the output in use and why, the stored choice (set aside or not), and the list of outputs.
pub(crate) fn describe(st: &Status) -> String {
    let stored = st.prefs.output.as_str();
    let mut s = match &st.choice.output {
        Some(o) => format!("Playing on {} [{}] ({}).", o.label(), o.key(), st.choice.reason),
        None => format!("No output in use ({}).", st.choice.reason),
    };
    if stored.is_empty() {
        s += " Setting: Automatic.";
    } else if st.choice.chosen {
        s += &format!(" Setting: {stored}, chosen by hand and in use.");
    } else {
        let why = st.outputs.iter().find(|o| o.key() == stored).map_or("it is not connected now".to_string(), |o| o.state());
        s += &format!(" Setting: {stored}, chosen by hand but SET ASIDE ({why}); the automatic choice plays until it is back.");
    }
    if st.sound_server {
        s += " A sound server runs: in Automatic it decides the output.";
    }
    if st.outputs.is_empty() {
        s += "\nNo sound outputs were found.";
    }
    for o in &st.outputs {
        let monitor = match (&o.monitor, o.kind) {
            (Some(m), Kind::Hdmi) => format!(", monitor {m}"),
            _ => String::new(),
        };
        let present = if o.kind == Kind::Hdmi { format!("monitor present: {}", match o.connected { Some(true) => "yes", Some(false) => "no", None => "unknown" }) } else { o.state() };
        let in_use = if st.choice.output.as_ref().is_some_and(|c| c.key() == o.key()) { " · IN USE" } else { "" };
        s += &format!("\n- {} [{}] {}: {present}{monitor}{in_use}", o.label(), o.key(), kind_word(o.kind));
    }
    s
}

fn wanted(a: &Value) -> Result<String, String> {
    let v = a.get("output").and_then(Value::as_str).map(str::trim).ok_or("say `output`: `automatic` or an output key from sound_output_status such as PCH:3")?;
    Ok(if v.eq_ignore_ascii_case("automatic") || v.is_empty() { String::new() } else { v.to_string() })
}

pub fn entries(env: &Env) -> Vec<Entry> {
    let mut v = Vec::new();
    {
        v.push(Entry::read(TAB, "sound_output_status", "Show which sound output IYAKKI plays on and why, whether an output chosen by hand is set aside because its pin has no monitor, and every output with its key and whether a monitor is connected.", no_args(), move |_| Ok(describe(&audioout::global().status()))));
    }
    {
        let (e, e2) = (env.clone(), env.clone());
        let schema = json!({"type": "object", "required": ["output"], "properties": {"output": {"type": "string", "description": "`automatic` to follow what is connected, or an output key from sound_output_status such as PCH:3."}}});
        v.push(
            Entry::change(TAB, "sound_output_set", "Choose the sound output (or `automatic`). The person approves a card first. An HDMI/DP output without a monitor is set aside until a monitor is there.", schema, |a| format!("Set the sound output to {}", wanted(a).map(|k| if k.is_empty() { "Automatic".to_string() } else { k }).unwrap_or_default()), move |a| {
                let key = wanted(a)?;
                let mut check = audioout::SoundSettings::default();
                check.output = key.clone();
                check.validate()?;
                crate::services::update_sound_in(&e.settings, |s| s.output = key.clone())?;
                e.audit("settings.sound_output", if key.is_empty() { "automatic" } else { &key });
                Ok(describe(&audioout::global().status()))
            })
            .admin()
            .with_preview(move |a| {
                let now = e2.settings.lock().map(|s| s.get().sound.output.clone()).unwrap_or_default();
                let after = wanted(a).unwrap_or_default();
                let name = |k: &str| if k.is_empty() { "Automatic".to_string() } else { k.to_string() };
                format!("Now: {}\nAfter: {}", name(&now), name(&after))
            })
            .with_refusal(|a| wanted(a).err()),
        );
    }
    v
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::Tools;
    use crate::settingstools_tests::{assert_policy, rig, tools};
    use std::path::Path;

    fn put(root: &Path, rel: &str, content: &str) {
        let p = root.join(rel);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, content).unwrap();
    }

    /// A Mac mini: HDMI devices 3 and 7 (pins 0 and 1), `monitor_on` is the pin with a monitor, plus a USB card.
    fn status_with(tag: &str, monitor_on: Option<usize>, stored: &str) -> (Status, std::path::PathBuf) {
        let r = std::env::temp_dir().join(format!("iyakki-sndtool-{}-{tag}", std::process::id()));
        let _ = std::fs::remove_dir_all(&r);
        put(&r, "asound/cards", " 0 [PCH            ]: HDA-Intel - HDA Intel PCH\n 1 [Device         ]: USB-Audio - USB Audio Device\n");
        put(&r, "asound/card1/usbid", "0d8c:0014\n");
        for (card, dev, id) in [(0, 3, "HDMI 0"), (0, 7, "HDMI 1"), (1, 0, "USB Audio")] {
            put(&r, &format!("asound/card{card}/pcm{dev}p/info"), &format!("card: {card}\ndevice: {dev}\nsubdevice: 0\nstream: PLAYBACK\nid: {id}\nname: {id}\n"));
        }
        for p in 0..2 {
            let t = if monitor_on == Some(p) { "monitor_present\t\t1\neld_valid\t\t1\nmonitor_name\t\tDELL U2415\nsad_count\t\t1\n" } else { "monitor_present\t\t0\neld_valid\t\t0\n" };
            put(&r, &format!("asound/card0/eld#2.{p}"), t);
        }
        let sel = audioout::Selector::new(r.clone(), |_| None, || false);
        sel.set_prefs(audioout::SoundSettings { output: stored.into(), ..Default::default() });
        (sel.refresh(), r)
    }

    #[test]
    fn the_status_says_what_plays_why_and_when_the_chosen_output_is_set_aside() {
        let (st, d) = status_with("auto", Some(1), "");
        let t = describe(&st);
        assert!(t.contains("Playing on") && t.contains("[PCH:7]") && t.contains("DELL U2415") && t.contains("Setting: Automatic"), "{t}");
        assert!(t.contains("monitor present: yes") && t.contains("monitor present: no") && t.contains("IN USE"), "{t}");
        // chosen by hand, monitor there: in use
        let (st, d2) = status_with("chosen", Some(1), "PCH:7");
        assert!(describe(&st).contains("chosen by hand and in use"), "{}", describe(&st));
        // chosen by hand, its pin lost the monitor: set aside, USB plays
        let (st, d3) = status_with("aside", None, "PCH:7");
        let t = describe(&st);
        assert!(t.contains("SET ASIDE") && t.contains("no monitor") && t.contains("Device:0") && t.contains("USB audio device is plugged in"), "{t}");
        for x in [d, d2, d3] {
            let _ = std::fs::remove_dir_all(x);
        }
    }

    #[test]
    fn reading_is_free_choosing_asks_administrators_only_and_audits() {
        let r = rig("soundout", true);
        let t = tools(&r, entries(&r.env));
        assert_policy(&t, &["sound_output_status"], &["sound_output_set"]);
        assert!(t.refusal("sound_output_set", &json!({})).is_some(), "needs output");
        assert!(t.call("sound_output_set", &json!({"output": "bad key!"})).is_err(), "a malformed key is refused and nothing is kept");
        assert_eq!(r.env.settings.lock().unwrap().get().sound.output, "");
        assert!(t.preview("sound_output_set", &json!({"output": "PCH:3"})).unwrap().contains("After: PCH:3"));
        t.call("sound_output_set", &json!({"output": "PCH:3"})).unwrap();
        assert_eq!(r.env.settings.lock().unwrap().get().sound.output, "PCH:3");
        assert!(r.audits.lock().unwrap().iter().any(|l| l.contains("settings.sound_output PCH:3")));
        t.call("sound_output_set", &json!({"output": "automatic"})).unwrap();
        assert_eq!(r.env.settings.lock().unwrap().get().sound.output, "");
        let user = rig("soundout-user", false);
        assert!(!tools(&user, entries(&user.env)).defs().iter().any(|d| d.spec.name == "sound_output_set"));
        assert!(tools(&user, entries(&user.env)).defs().iter().any(|d| d.spec.name == "sound_output_status"), "everyone reads");
        let _ = std::fs::remove_dir_all(&r.dir);
        let _ = std::fs::remove_dir_all(&user.dir);
    }
}
