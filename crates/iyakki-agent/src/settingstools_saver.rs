//! Chat tools for Settings -> Screen saver: `screensaver_status` (free), `screensaver_set` (asks) and `screensaver_preview` (asks, it
//! takes over the screen for five seconds). Same `screensaver::SaverConfig` file as the page; the shell reads it again every half minute.

use serde_json::{json, Value};

use crate::screensaver::{self, First, SaverConfig, Style, AFTER_CHOICES, DIM_CHOICES, THEN_CHOICES};
use crate::settingstools::{no_args, Entry, Env};
pub use iyakki_shared::settingstools_saver::{PROMPT_RULE};

const TAB: &str = "ScreenSaver";

pub const TOOLS: [&str; 3] = ["screensaver_status", "screensaver_set", "screensaver_preview"];


fn describe(c: &SaverConfig) -> String {
    format!(
        "After {} minutes idle: {}. Then the screen turns off after {} more minutes (0 = never). Style: {}. Dimming: {}% of full brightness (0 = off). Shorter on battery: {}.",
        c.after_min,
        c.first.label(),
        c.then_off_min,
        c.style.label(),
        c.dim_pct,
        if c.battery_shorter { "yes" } else { "no" }
    )
}

fn pick<T: Copy>(a: &Value, key: &str, all: &[(&str, T)]) -> Result<Option<T>, String> {
    match a.get(key).and_then(Value::as_str) {
        None => Ok(None),
        Some(s) => all.iter().find(|(n, _)| *n == s).map(|(_, v)| Some(*v)).ok_or_else(|| format!("{key} is one of: {}", all.iter().map(|x| x.0).collect::<Vec<_>>().join(", "))),
    }
}

fn choice(a: &Value, key: &str, choices: &[u32]) -> Result<Option<u32>, String> {
    match a.get(key) {
        None => Ok(None),
        Some(v) => v.as_u64().and_then(|n| u32::try_from(n).ok()).filter(|n| choices.contains(n)).map(Some).ok_or_else(|| format!("{key} is one of: {}", choices.iter().map(|c| c.to_string()).collect::<Vec<_>>().join(", "))),
    }
}

fn apply(mut c: SaverConfig, a: &Value) -> Result<SaverConfig, String> {
    let firsts = [("nothing", First::Nothing), ("saver", First::Saver), ("off", First::Off)];
    let styles = [("logo", Style::Logo), ("logo_clock", Style::LogoClock), ("small", Style::Small), ("breathing", Style::Breathing), ("starfield", Style::Starfield), ("slideshow", Style::Slideshow)];
    if let Some(f) = pick(a, "first", &firsts)? {
        c.first = f;
    }
    if let Some(s) = pick(a, "style", &styles)? {
        c.style = s;
    }
    if let Some(n) = choice(a, "after_min", &AFTER_CHOICES)? {
        c.after_min = n;
    }
    if let Some(n) = choice(a, "then_off_min", &THEN_CHOICES)? {
        c.then_off_min = n;
    }
    if let Some(n) = choice(a, "dim_pct", &DIM_CHOICES)? {
        c.dim_pct = n;
    }
    if let Some(b) = a.get("battery_shorter").and_then(Value::as_bool) {
        c.battery_shorter = b;
    }
    Ok(c)
}

pub fn entries(env: &Env) -> Vec<Entry> {
    let mut v = Vec::new();
    {
        let e = env.clone();
        v.push(Entry::read(TAB, "screensaver_status", "Show the person's screen saver and screen-off choices.", no_args(), move |_| Ok(describe(&SaverConfig::load(&e.dir, &e.user)))));
    }
    {
        let (e, e2, e3) = (env.clone(), env.clone(), env.clone());
        let schema = json!({"type": "object", "properties": {
            "first": {"type": "string", "enum": ["nothing", "saver", "off"]},
            "after_min": {"type": "integer", "enum": AFTER_CHOICES, "description": "Idle minutes before (0 = never)."},
            "then_off_min": {"type": "integer", "enum": THEN_CHOICES},
            "style": {"type": "string", "enum": ["logo", "logo_clock", "small", "breathing", "starfield", "slideshow"]},
            "dim_pct": {"type": "integer", "enum": DIM_CHOICES},
            "battery_shorter": {"type": "boolean"}}});
        v.push(
            Entry::change(TAB, "screensaver_set", "Change the person's screen saver choices. Only the fields you give change. The person approves a card first.", schema, |_| "Change the screen saver choices".into(), move |a| {
                let now = SaverConfig::load(&e.dir, &e.user);
                let next = apply(now, a)?;
                next.save(&e.dir, &e.user)?;
                e.audit("settings.screensaver", &describe(&next));
                Ok(format!("Saved. {}", describe(&next)))
            })
            .with_preview(move |a| match apply(SaverConfig::load(&e2.dir, &e2.user), a) {
                Ok(n) => format!("Now: {}\nAfter: {}", describe(&SaverConfig::load(&e2.dir, &e2.user)), describe(&n)),
                Err(m) => format!("Cannot change it: {m}"),
            })
            .with_refusal(move |a| apply(SaverConfig::load(&e3.dir, &e3.user), a).err()),
        );
    }
    {
        let e = env.clone();
        v.push(Entry::change(TAB, "screensaver_preview", "Show the screen saver for five seconds (or until a key is pressed). The person approves a card first.", no_args(), |_| "Preview the screen saver".into(), move |_| {
            screensaver::request(&e.dir, &e.user, screensaver::Request::Preview);
            e.audit("settings.screensaver", "preview");
            Ok("The screen saver shows for five seconds.".into())
        }));
    }
    v
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::Tools;
    use crate::settingstools_tests::{assert_policy, rig, tools};

    #[test]
    fn choices_are_checked_saved_in_the_pages_file_and_preview_asks_the_shell() {
        let r = rig("saver", false);
        let t = tools(&r, entries(&r.env));
        assert_policy(&t, &["screensaver_status"], &["screensaver_set", "screensaver_preview"]);
        assert!(t.call("screensaver_status", &json!({})).unwrap().contains("After 10 minutes"));
        assert!(t.refusal("screensaver_set", &json!({"after_min": 7})).is_some());
        assert!(t.refusal("screensaver_set", &json!({"style": "disco"})).is_some());
        // the schema offers the new styles too (a model only sends what the schema lists)
        let defs = t.defs();
        let schema = &defs.iter().find(|d| d.spec.name == "screensaver_set").unwrap().spec.schema;
        let styles: Vec<&str> = schema["properties"]["style"]["enum"].as_array().unwrap().iter().filter_map(Value::as_str).collect();
        assert!(styles.contains(&"starfield") && styles.contains(&"slideshow"), "{styles:?}");
        assert!(t.refusal("screensaver_set", &json!({"style": "starfield"})).is_none());
        t.call("screensaver_set", &json!({"after_min": 5, "style": "small", "first": "off"})).unwrap();
        let c = SaverConfig::load(&r.dir, "ada");
        assert_eq!((c.after_min, c.style, c.first), (5, Style::Small, First::Off));
        assert_eq!(c.then_off_min, 10, "untouched fields stay");
        assert!(t.preview("screensaver_set", &json!({"dim_pct": 25})).unwrap().contains("25%"));
        t.call("screensaver_preview", &json!({})).unwrap();
        assert_eq!(screensaver::take_request(&r.dir, "ada"), Some(screensaver::Request::Preview));
        let _ = std::fs::remove_dir_all(&r.dir);
    }
}
