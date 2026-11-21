//! Graphics acceleration of the full web engine (F-BROX-24, F-BROX-25): which of the four parts (compositing, rasterisation, hardware
//! video decoding, WebGL and canvas) are switched on for this person and this site on this machine, and a plain reason for every one that
//! is off. The default is on where a render node and a driver are usable, with the processor as the fallback that stays when they are not.
//!
//! Three switches put together, the strictest wins: the administrator's (device-wide, `admin_off`), the person's (`people[user].on`) and
//! the person's list of sites (`sites_off`, a host and its subdomains). The result of [`Policy::verdict`] and the machine's facts go to
//! [`plan`], a pure function whose environment lines are added to the engine's start. Nothing here opens a device.

use std::collections::BTreeMap;
use std::path::Path;

use serde::{Deserialize, Serialize};

pub const FILE: &str = "web-accel.json";
pub const MAX_SITES: usize = 64;

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Person {
    /// `None` is the default: on.
    #[serde(default)]
    pub off: bool,
    #[serde(default)]
    pub sites_off: Vec<String>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Policy {
    #[serde(default)]
    pub admin_off: bool,
    #[serde(default)]
    pub people: BTreeMap<String, Person>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Verdict {
    pub allowed: bool,
    /// Why it is off, in a sentence; empty when allowed.
    pub why: String,
}

/// A site is a plain host name: letters, digits, dots and dashes, at most 253 characters, lower-cased.
pub fn clean_site(s: &str) -> Option<String> {
    let s = s.trim().trim_start_matches("*.").to_ascii_lowercase();
    let ok = !s.is_empty() && s.len() <= 253 && s.contains('.') && s.bytes().all(|c| c.is_ascii_alphanumeric() || c == b'.' || c == b'-') && !s.starts_with('.') && !s.ends_with('.') && !s.contains("..");
    ok.then_some(s)
}

fn host_matches(site: &str, host: &str) -> bool {
    let host = host.trim().to_ascii_lowercase();
    host == site || host.ends_with(&format!(".{site}"))
}

impl Policy {
    pub fn verdict(&self, user: &str, host: &str) -> Verdict {
        if self.admin_off {
            return Verdict { allowed: false, why: "an administrator turned graphics acceleration off for this computer".into() };
        }
        if let Some(p) = self.people.get(user) {
            if p.off {
                return Verdict { allowed: false, why: "you turned graphics acceleration off".into() };
            }
            if p.sites_off.iter().any(|s| host_matches(s, host)) {
                return Verdict { allowed: false, why: format!("you turned graphics acceleration off for {host}") };
            }
        }
        Verdict { allowed: true, why: String::new() }
    }
    pub fn set_admin_off(&mut self, off: bool) {
        self.admin_off = off;
    }
    pub fn set_person_off(&mut self, user: &str, off: bool) {
        self.people.entry(user.to_string()).or_default().off = off;
        self.tidy(user);
    }
    /// Adds or removes a site from the person's list. Errors say why.
    pub fn set_site_off(&mut self, user: &str, site: &str, off: bool) -> Result<(), String> {
        let site = clean_site(site).ok_or_else(|| "a site is a host name such as video.example.org".to_string())?;
        let p = self.people.entry(user.to_string()).or_default();
        if off {
            if !p.sites_off.contains(&site) {
                if p.sites_off.len() >= MAX_SITES {
                    return Err(format!("at most {MAX_SITES} sites"));
                }
                p.sites_off.push(site);
            }
        } else {
            p.sites_off.retain(|s| *s != site);
        }
        self.tidy(user);
        Ok(())
    }
    fn tidy(&mut self, user: &str) {
        if self.people.get(user).is_some_and(|p| *p == Person::default()) {
            self.people.remove(user);
        }
    }
}

/// Reads the file; a missing or damaged one is the default (everything on), and the second value says it was damaged.
pub fn load(dir: &Path) -> (Policy, bool) {
    match std::fs::read_to_string(dir.join(FILE)) {
        Ok(s) => match serde_json::from_str::<Policy>(&s) {
            Ok(mut p) => {
                for q in p.people.values_mut() {
                    q.sites_off = q.sites_off.iter().filter_map(|s| clean_site(s)).take(MAX_SITES).collect();
                }
                (p, false)
            }
            Err(_) => (Policy::default(), true),
        },
        Err(_) => (Policy::default(), false),
    }
}

pub fn save(dir: &Path, p: &Policy) -> Result<(), String> {
    std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    let tmp = dir.join(format!("{FILE}.tmp"));
    std::fs::write(&tmp, serde_json::to_string_pretty(p).map_err(|e| e.to_string())?).map_err(|e| e.to_string())?;
    std::fs::rename(&tmp, dir.join(FILE)).map_err(|e| e.to_string())
}

/// What the machine offers, put together by the caller from the probe (`iyakki_uikit::accel::probe`, `iyakki_shared::gpu`) and the files.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Machine {
    pub node_usable: bool,
    /// Why there is no usable node or driver (empty when there is).
    pub node_reason: String,
    /// A VA-API driver and the GStreamer VA plug-in are installed.
    pub va_driver: bool,
    /// The browser's graphics mode (Settings, Browser, Graphics) has decided on software drawing, by choice or after a freeze.
    pub software_mode: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Item {
    pub name: &'static str,
    pub on: bool,
    pub reason: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Plan {
    pub items: Vec<Item>,
    pub env: Vec<(String, String)>,
}

impl Plan {
    pub fn item(&self, name: &str) -> Option<&Item> {
        self.items.iter().find(|i| i.name == name)
    }
    /// For Settings and the chat tool: one line a part.
    pub fn lines(&self) -> Vec<String> {
        self.items.iter().map(|i| if i.on { format!("{}: on", i.name) } else { format!("{}: off, {}", i.name, i.reason) }).collect()
    }
}

pub const COMPOSITING: &str = "compositing";
pub const RASTER: &str = "rasterisation";
pub const VIDEO: &str = "video decoding";
pub const WEBGL: &str = "WebGL and canvas";

pub fn plan(m: &Machine, v: &Verdict) -> Plan {
    let gpu_off: Option<String> = if !v.allowed {
        Some(v.why.clone())
    } else if m.software_mode {
        Some("the browser is set to draw in software (Settings, Browser, Graphics)".into())
    } else if !m.node_usable {
        Some(if m.node_reason.is_empty() { "no usable graphics driver".into() } else { m.node_reason.clone() })
    } else {
        None
    };
    let mut items = Vec::new();
    let mut env: Vec<(String, String)> = Vec::new();
    match &gpu_off {
        Some(why) => {
            for n in [COMPOSITING, RASTER, VIDEO, WEBGL] {
                items.push(Item { name: n, on: false, reason: why.clone() });
            }
            // only the switches of the person, the site and the administrator add lines: the graphics mode and the missing driver already
            // put the engine into software drawing by their own paths
            if !v.allowed {
                env.push(("IYAKKI_WEBVIEW_GPU".into(), "0".into()));
                env.push(("LIBGL_ALWAYS_SOFTWARE".into(), "1".into()));
                env.push(("WEBKIT_SKIA_ENABLE_CPU_RENDERING".into(), "1".into()));
            }
        }
        None => {
            items.push(Item { name: COMPOSITING, on: true, reason: String::new() });
            items.push(Item { name: RASTER, on: true, reason: String::new() });
            items.push(Item { name: WEBGL, on: true, reason: String::new() });
            if m.va_driver {
                items.push(Item { name: VIDEO, on: true, reason: String::new() });
                env.push(("IYAKKI_WEBVIEW_VAAPI".into(), "1".into()));
            } else {
                items.push(Item { name: VIDEO, on: false, reason: "no VA-API video driver is installed; the processor decodes".into() });
            }
        }
    }
    Plan { items, env }
}

/// Adds `extra` to `env`, replacing a variable that is already there.
pub fn merge_env(env: &mut Vec<(String, String)>, extra: &[(String, String)]) {
    for (k, val) in extra {
        env.retain(|(n, _)| n != k);
        env.push((k.clone(), val.clone()));
    }
}

/// For a site the person turned off: the engine's environment with the card switched off (no video decoder, no card pictures).
pub fn apply_site_off(env: &mut Vec<(String, String)>, v: &Verdict) {
    if v.allowed {
        return;
    }
    env.retain(|(k, _)| k != "IYAKKI_WEBVIEW_VAAPI");
    merge_env(env, &plan(&Machine::default(), v).env);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn card() -> Machine {
        Machine { node_usable: true, va_driver: true, ..Default::default() }
    }

    #[test]
    fn everything_is_on_by_default_where_a_card_and_a_video_driver_are_there() {
        let p = plan(&card(), &Policy::default().verdict("ann", "video.example.org"));
        assert!(p.items.iter().all(|i| i.on), "{:?}", p.lines());
        assert_eq!(p.env, vec![("IYAKKI_WEBVIEW_VAAPI".to_string(), "1".to_string())]);
        assert_eq!(p.lines().len(), 4);
    }

    #[test]
    fn a_missing_driver_or_library_turns_the_parts_off_with_the_reason_and_adds_no_variables() {
        let m = Machine { node_usable: false, node_reason: "no render node (no graphics driver is loaded)".into(), ..Default::default() };
        let p = plan(&m, &Policy::default().verdict("ann", "a.org"));
        assert!(p.items.iter().all(|i| !i.on && i.reason.contains("no render node")));
        assert!(p.env.is_empty());
    }

    #[test]
    fn no_video_driver_leaves_only_video_off() {
        let p = plan(&Machine { va_driver: false, ..card() }, &Policy::default().verdict("ann", "a.org"));
        assert!(p.item(COMPOSITING).unwrap().on && !p.item(VIDEO).unwrap().on);
        assert!(p.item(VIDEO).unwrap().reason.contains("VA-API"));
        assert!(p.env.is_empty());
    }

    #[test]
    fn the_graphics_mode_set_to_software_wins_without_extra_variables() {
        let p = plan(&Machine { software_mode: true, ..card() }, &Policy::default().verdict("ann", "a.org"));
        assert!(p.items.iter().all(|i| !i.on && i.reason.contains("Graphics")));
        assert!(p.env.is_empty());
    }

    #[test]
    fn the_person_the_site_and_the_administrator_each_switch_it_off_and_the_strictest_wins() {
        let mut pol = Policy::default();
        assert!(pol.verdict("ann", "x.example.org").allowed);
        pol.set_site_off("ann", "Example.org", true).unwrap();
        assert!(!pol.verdict("ann", "x.example.org").allowed);
        assert!(!pol.verdict("ann", "example.org").allowed);
        assert!(pol.verdict("ann", "notexample.org").allowed, "a name that only ends in the same letters is another site");
        assert!(pol.verdict("bob", "x.example.org").allowed, "per person");
        pol.set_person_off("bob", true);
        assert!(pol.verdict("bob", "a.org").why.contains("you turned"));
        pol.set_admin_off(true);
        let v = pol.verdict("carol", "a.org");
        assert!(!v.allowed && v.why.contains("administrator"));
        let p = plan(&card(), &v);
        assert!(p.items.iter().all(|i| !i.on));
        assert!(p.env.iter().any(|(k, v)| k == "LIBGL_ALWAYS_SOFTWARE" && v == "1"));
        assert!(p.env.iter().any(|(k, v)| k == "IYAKKI_WEBVIEW_GPU" && v == "0"));
        assert!(!p.env.iter().any(|(k, _)| k == "IYAKKI_WEBVIEW_VAAPI"));
    }

    #[test]
    fn sites_are_checked_and_an_empty_person_record_is_dropped() {
        let mut pol = Policy::default();
        for bad in ["", "localhost", "a b.org", "x/y.org", ".org", "a..org"] {
            assert!(pol.set_site_off("ann", bad, true).is_err(), "{bad}");
        }
        pol.set_site_off("ann", "*.Example.org", true).unwrap();
        assert_eq!(pol.people["ann"].sites_off, vec!["example.org".to_string()]);
        pol.set_site_off("ann", "example.org", false).unwrap();
        assert!(pol.people.is_empty());
        for i in 0..MAX_SITES {
            pol.set_site_off("ann", &format!("s{i}.example.org"), true).unwrap();
        }
        assert!(pol.set_site_off("ann", "more.example.org", true).is_err());
    }

    #[test]
    fn the_file_round_trips_and_a_damaged_or_hand_edited_file_is_safe() {
        let d = std::env::temp_dir().join(format!("iyakki-webaccel-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        assert_eq!(load(&d), (Policy::default(), false));
        let mut pol = Policy::default();
        pol.set_admin_off(true);
        pol.set_site_off("ann", "a.example.org", true).unwrap();
        save(&d, &pol).unwrap();
        assert_eq!(load(&d), (pol, false));
        std::fs::write(d.join(FILE), "{ not json").unwrap();
        assert_eq!(load(&d), (Policy::default(), true));
        std::fs::write(d.join(FILE), r#"{"people":{"ann":{"sites_off":["ok.example.org","bad host","../x"]}}}"#).unwrap();
        let (p, damaged) = load(&d);
        assert!(!damaged);
        assert_eq!(p.people["ann"].sites_off, vec!["ok.example.org".to_string()]);
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn merging_replaces_a_variable_and_keeps_the_others() {
        let mut e = vec![("A".to_string(), "1".to_string()), ("B".to_string(), "1".to_string())];
        merge_env(&mut e, &[("A".to_string(), "2".to_string())]);
        assert_eq!(e, vec![("B".to_string(), "1".to_string()), ("A".to_string(), "2".to_string())]);
    }

    #[test]
    fn a_site_turned_off_drops_the_video_decoder_and_the_card_pictures_from_the_start_environment() {
        let mut env = vec![("IYAKKI_WEBVIEW_VAAPI".to_string(), "1".to_string()), ("LP_NUM_THREADS".to_string(), "4".to_string())];
        apply_site_off(&mut env, &Verdict { allowed: true, why: String::new() });
        assert_eq!(env.len(), 2, "allowed changes nothing");
        apply_site_off(&mut env, &Verdict { allowed: false, why: "x".into() });
        assert!(!env.iter().any(|(k, _)| k == "IYAKKI_WEBVIEW_VAAPI"));
        assert!(env.iter().any(|(k, v)| k == "IYAKKI_WEBVIEW_GPU" && v == "0") && env.iter().any(|(k, _)| k == "LP_NUM_THREADS"));
    }
}
