//! The Settings page as a tile, first slice (F-WEB-56): an OVERVIEW that only reads. It shows who is signed in and the role, the model
//! the machine chats with and how many others there are, whether the machine serves the web UI (administrators), and how many tools the
//! chat is offered. Two SAFE TOGGLES can be changed (flow notices reach the bell or only the inbox; a shorter screen saver time on battery)
//! through `settings.set`, which runs the person's own `notification_filter_set` and `screensaver_set` tools: a change is a tool that asks
//! first, so it shows an approval card in the approvals list and nothing changes until the person's own click (the tile asks again every two
//! seconds until answered). Every other change (a model, a password, web access, the vault) stays on the Settings page and in the chat tools.
use iyakki_tile_kit::abi::{line_height, Color, DisplayList, Event, Op, Rect};
use iyakki_tile_kit::{button, call_json, clip, palette, text, App, Source};
use serde_json::Value;

#[cfg(target_arch = "wasm32")]
mod guest;

#[cfg(not(target_arch = "wasm32"))]
pub const TILE_WASM: &[u8] = include_bytes!("../tile.wasm");
pub const CAPS: [&str; 2] = ["settings.get", "settings.set"];

const REFRESH_MS: u32 = 10_000;
const TOP: i32 = 40;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Kind {
    Head,
    Line,
    Note,
    /// A toggle: the index into the list of toggles.
    Toggle(usize),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Toggle {
    pub id: String,
    pub label: String,
    pub on: bool,
}

pub struct SettingsTile<S: Source> {
    src: S,
    size: (i32, i32),
    dark: bool,
    lines: Vec<(Kind, String)>,
    error: String,
    scroll: usize,
    since: u32,
    toggles: Vec<Toggle>,
    /// A change that waits for the person's answer: (toggle id, wanted state).
    pending: Option<(String, bool)>,
    note: String,
}

/// The overview as lines, from the service's answer (`settings.get`).
pub fn toggles_of(v: &Value) -> Vec<Toggle> {
    v["toggles"].as_array().map(|a| a.iter().take(8).filter_map(|t| Some(Toggle { id: t["id"].as_str()?.to_string(), label: t["label"].as_str()?.chars().take(60).collect(), on: t["on"].as_bool()? })).collect()).unwrap_or_default()
}

pub fn lines_of(v: &Value) -> Vec<(Kind, String)> {
    let s = |p: &str| v.pointer(p).and_then(Value::as_str).unwrap_or("").to_string();
    let mut o = vec![];
    o.push((Kind::Head, "Account".to_string()));
    let admin = v["admin"].as_bool().unwrap_or(false);
    o.push((Kind::Line, format!("{} ({})", s("/user"), if s("/role").is_empty() { if admin { "administrator".to_string() } else { "person".to_string() } } else { s("/role") })));
    o.push((Kind::Head, "Chat model".to_string()));
    let models = v["models"].as_array().cloned().unwrap_or_default();
    match models.first() {
        Some(m) => {
            o.push((Kind::Line, format!("{} / {}", m["provider"].as_str().unwrap_or("?"), m["model"].as_str().unwrap_or("?"))));
            if models.len() > 1 {
                o.push((Kind::Line, format!("and {} more to choose from", models.len() - 1)));
            }
        }
        None => o.push((Kind::Line, "none is set up".to_string())),
    }
    o.push((Kind::Head, "Web access".to_string()));
    match v.get("web").filter(|w| w.is_object()) {
        Some(w) => {
            o.push((Kind::Line, if w["on"].as_bool().unwrap_or(false) { format!("on, port {}", w["port"].as_u64().unwrap_or(0)) } else { "off: nothing listens".to_string() }));
            if let Some(st) = w["status"].as_str().filter(|t| !t.is_empty()) {
                o.push((Kind::Note, st.to_string()));
            }
        }
        None => o.push((Kind::Line, "not shown to you here".to_string())),
    }
    if let Some(t) = v["toggles"].as_array().filter(|t| !t.is_empty()) {
        o.push((Kind::Head, "Switches".to_string()));
        for i in 0..t.len().min(8) {
            o.push((Kind::Toggle(i), String::new()));
        }
    }
    o.push((Kind::Head, "Tools".to_string()));
    o.push((Kind::Line, format!("{} offered to the chat", v["tools"].as_u64().unwrap_or(0))));
    o.push((Kind::Note, "To change any of this, use the Settings page. This tile only shows it.".to_string()));
    o
}

impl<S: Source> SettingsTile<S> {
    pub fn new(src: S) -> Self {
        let mut t = SettingsTile { src, size: (360, 480), dark: false, lines: vec![], error: String::new(), scroll: 0, since: 0, toggles: vec![], pending: None, note: String::new() };
        t.refresh();
        t
    }

    pub fn source(&mut self) -> &mut S {
        &mut self.src
    }

    pub fn error(&self) -> &str {
        &self.error
    }

    pub fn note(&self) -> &str {
        &self.note
    }

    /// Ask the service to put one switch in the wanted state.
    fn set(&mut self, id: &str, on: bool) {
        self.since = 0;
        match call_json(&mut self.src, "settings", "set", &serde_json::json!({"id": id, "on": on}).to_string()) {
            Ok(v) if v["ok"].as_bool() == Some(true) => {
                self.pending = None;
                self.note = "Changed.".into();
                self.refresh();
            }
            Ok(v) if v["asked"].as_bool() == Some(true) => {
                self.pending = Some((id.to_string(), on));
                self.note = "Waiting for your answer in the approvals list.".into();
            }
            Ok(v) => {
                self.pending = None;
                self.note = v["error"].as_str().unwrap_or("that could not be changed").chars().take(200).collect();
            }
            Err(e) => {
                self.pending = None;
                self.note = e.chars().take(200).collect();
            }
        }
    }

    fn per_page(&self) -> usize {
        (((self.size.1 - TOP) / 18).max(1)) as usize
    }

    fn refresh(&mut self) {
        self.since = 0;
        match call_json(&mut self.src, "settings", "get", "{}") {
            Ok(v) => {
                self.error.clear();
                self.lines = lines_of(&v);
                self.toggles = toggles_of(&v);
                if let Some((id, want)) = self.pending.clone() {
                    if self.toggles.iter().any(|t| t.id == id && t.on == want) {
                        self.pending = None;
                    }
                }
                self.scroll = self.scroll.min(self.lines.len().saturating_sub(1));
            }
            Err(e) => self.error = clip(&e, 300, 12),
        }
    }
}

impl<S: Source> App for SettingsTile<S> {
    fn event(&mut self, e: &Event) -> bool {
        match e {
            Event::Resize { w, h, .. } => {
                self.size = (*w, *h);
                true
            }
            Event::Theme { dark, .. } => {
                self.dark = *dark;
                true
            }
            Event::Tick { dt_ms } => {
                self.since = self.since.saturating_add(*dt_ms);
                if let Some((id, on)) = self.pending.clone() {
                    if self.since >= 2000 {
                        self.set(&id, on);
                        return true;
                    }
                    return false;
                }
                if self.since >= REFRESH_MS {
                    self.refresh();
                    return true;
                }
                false
            }
            Event::Hit { id: 2 } => {
                self.refresh();
                true
            }
            Event::Hit { id: 3 } => {
                self.scroll = self.scroll.saturating_sub(self.per_page());
                true
            }
            Event::Hit { id: 4 } => {
                if self.scroll + self.per_page() < self.lines.len() {
                    self.scroll += self.per_page();
                }
                true
            }
            Event::Hit { id } if (20..28).contains(id) => {
                let Some(t) = self.toggles.get((*id - 20) as usize).cloned() else { return false };
                self.set(&t.id, !t.on);
                true
            }
            _ => false,
        }
    }

    fn frame(&self) -> DisplayList {
        let (w, h) = self.size;
        let p = palette(self.dark);
        let mut l = DisplayList::new();
        l.push(Op::Rect { r: Rect::new(0, 0, w, h), color: p.bg });
        l.push(Op::Rect { r: Rect::new(0, 0, w, 32), color: p.bar });
        l.push(text(8, 8, 14, p.fg, "SETTINGS"));
        button(&mut l, 2, Rect::new(w - 84, 4, 76, 24), "Refresh", &p);
        if self.lines.len() > self.per_page() {
            button(&mut l, 3, Rect::new(w - 128, 4, 38, 24), "Up", &p);
            button(&mut l, 4, Rect::new(w - 172, 4, 38, 24), "Down", &p);
        }
        let mut y = TOP;
        if !self.error.is_empty() {
            l.push(text(8, y, 12, p.bad, &clip(&self.error, w - 16, 12)));
            y += 20;
        }
        for (k, t) in self.lines.iter().skip(self.scroll).take(self.per_page()) {
            let (size, c, x): (u32, Color, i32) = match k {
                Kind::Head => (14, p.accent, 8),
                Kind::Line | Kind::Toggle(_) => (13, p.fg, 20),
                Kind::Note => (11, p.mute, 20),
            };
            if let Kind::Toggle(i) = k {
                if let Some(tg) = self.toggles.get(*i) {
                    let waiting = self.pending.as_ref().is_some_and(|(id, _)| *id == tg.id);
                    let label = format!("[{}] {}", if waiting { "?" } else if tg.on { "x" } else { " " }, tg.label);
                    l.push(text(x, y, size, c, &clip(&label, w - x - 8, size)));
                    iyakki_tile_kit::named_hit(&mut l, 20 + *i as u32, iyakki_tile_kit::abi::Role::Toggle, Rect::new(x - 4, y - 2, w - x, line_height(size) + 4), &tg.label, if tg.on { "on" } else { "off" }, if tg.on { iyakki_tile_kit::abi::access_flags::CHECKED } else { 0 });
                }
            } else {
                l.push(text(x, y, size, c, &clip(t, w - x - 8, size)));
            }
            y += line_height(size) + 3;
        }
        if !self.note.is_empty() {
            let c = if self.note.starts_with("Waiting") { Color(200, 130, 20, 255) } else if self.note == "Changed." { p.mute } else { p.bad };
            l.push(text(8, h - 16, 11, c, &clip(&self.note, w - 16, 11)));
        }
        l
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::collections::VecDeque;

    #[derive(Default)]
    struct Fake {
        calls: Vec<(String, String)>,
        replies: VecDeque<Result<String, String>>,
    }
    impl Source for Fake {
        fn call(&mut self, d: &str, m: &str, _: &str) -> Result<String, String> {
            self.calls.push((d.into(), m.into()));
            self.replies.pop_front().unwrap_or_else(|| Ok("{}".into()))
        }
    }

    fn answer() -> String {
        json!({"user": "ann", "role": "admin", "admin": true, "models": [{"provider": "local", "model": "tiny"}, {"provider": "cloud", "model": "big"}], "web": {"on": true, "port": 8443, "status": "on, port 8443"}, "tools": 31}).to_string()
    }

    fn tile(r: Vec<Result<String, String>>) -> SettingsTile<Fake> {
        let mut t = SettingsTile::new(Fake { replies: r.into(), ..Default::default() });
        t.event(&Event::Resize { w: 360, h: 400, scale_pct: 100 });
        t
    }

    fn texts(l: &DisplayList) -> Vec<String> {
        l.ops.iter().filter_map(|o| if let Op::Text { text, .. } = o { Some(text.clone()) } else { None }).collect()
    }

    #[test]
    fn the_overview_names_the_account_the_model_web_access_and_the_tools() {
        let mut t = tile(vec![Ok(answer())]);
        let x = texts(&t.frame());
        for want in ["Account", "ann (admin)", "Chat model", "local / tiny", "and 1 more to choose from", "Web access", "on, port 8443", "Tools", "31 offered to the chat"] {
            assert!(x.contains(&want.to_string()), "{want}: {x:?}");
        }
        assert_eq!(t.source_calls(), vec![("settings".to_string(), "get".to_string())]);
    }

    impl SettingsTile<Fake> {
        fn source_calls(&mut self) -> Vec<(String, String)> {
            self.source().calls.clone()
        }
    }

    #[test]
    fn what_the_person_may_not_see_is_said_and_nothing_is_made_up() {
        let t = tile(vec![Ok(json!({"user": "bob", "role": "user", "admin": false, "models": [], "tools": 3}).to_string())]);
        let x = texts(&t.frame());
        assert!(x.contains(&"none is set up".to_string()) && x.contains(&"not shown to you here".to_string()) && x.contains(&"bob (user)".to_string()), "{x:?}");
        for bad in ["", "null", "[]", "{\"models\":5}", "{\"web\":5}", "{\"tools\":\"x\"}"] {
            let t = tile(vec![Ok(bad.into())]);
            assert!(t.frame().encode().is_ok(), "{bad}");
        }
    }

    #[test]
    fn it_reads_again_every_ten_seconds_and_on_refresh_and_a_failure_is_one_red_line() {
        let mut t = tile(vec![Ok(answer()), Err("the service did not answer".into())]);
        let n = t.source().calls.len();
        assert!(!t.event(&Event::Tick { dt_ms: 9000 }));
        assert!(t.event(&Event::Tick { dt_ms: 1000 }));
        assert_eq!(t.source().calls.len(), n + 1);
        assert_eq!(t.error(), "the service did not answer");
        assert!(texts(&t.frame()).contains(&"ann (admin)".to_string()), "the old overview stays");
        assert!(t.event(&Event::Hit { id: 2 }));
        assert_eq!(t.error(), "");
    }

    #[test]
    fn the_only_changes_it_can_ask_for_are_the_two_switches_and_only_by_settings_set() {
        let mut t = tile(vec![Ok(answer())]);
        let ids: Vec<u32> = t.frame().ops.iter().filter_map(|o| if let Op::Hit { id, .. } = o { Some(*id) } else { None }).collect();
        assert!(ids.iter().all(|i| (2..=4).contains(i) || (20..28).contains(i)), "{ids:?}");
        for id in 0..300 {
            t.event(&Event::Hit { id });
        }
        assert!(t.source().calls.iter().all(|c| c == &("settings".to_string(), "get".to_string()) || (c.0 == "settings" && c.1 == "set")), "{:?}", t.source().calls);
    }

    fn with_switches() -> String {
        json!({"user": "ann", "role": "user", "admin": false, "models": [], "tools": 3, "toggles": [{"id": "flows_notices", "label": "Flow notices in the bell", "on": true}, {"id": "saver_battery", "label": "Shorter saver on battery", "on": false}]}).to_string()
    }

    #[test]
    fn a_switch_is_shown_and_a_click_asks_the_service_and_waits_for_the_persons_answer() {
        let asked = || Ok(json!({"ok": false, "asked": true}).to_string());
        let after = json!({"user": "ann", "role": "user", "admin": false, "models": [], "tools": 3, "toggles": [{"id": "flows_notices", "label": "Flow notices in the bell", "on": true}, {"id": "saver_battery", "label": "Shorter saver on battery", "on": true}]}).to_string();
        let mut t = tile(vec![Ok(with_switches()), asked(), asked(), Ok(json!({"ok": true}).to_string()), Ok(after)]);
        let x = texts(&t.frame());
        assert!(x.contains(&"Switches".to_string()) && x.contains(&"[x] Flow notices in the bell".to_string()) && x.contains(&"[ ] Shorter saver on battery".to_string()), "{x:?}");
        assert!(t.event(&Event::Hit { id: 21 }));
        let set = t.source().calls.iter().find(|c| c.1 == "set").unwrap().clone();
        assert_eq!(set.0, "settings");
        assert!(t.note().starts_with("Waiting for your answer"));
        assert!(texts(&t.frame()).contains(&"[?] Shorter saver on battery".to_string()));
        assert!(!t.event(&Event::Tick { dt_ms: 1500 }), "not before two seconds");
        assert!(t.event(&Event::Tick { dt_ms: 600 }), "asks again");
        assert!(t.event(&Event::Tick { dt_ms: 2000 }), "and once more: now answered");
        assert_eq!(t.source().calls.iter().filter(|c| c.1 == "set").count(), 3);
        assert!(texts(&t.frame()).contains(&"[x] Shorter saver on battery".to_string()), "{:?}", texts(&t.frame()));
        assert!(!t.event(&Event::Tick { dt_ms: 3000 }) || t.source().calls.iter().filter(|c| c.1 == "set").count() == 3, "nothing is asked again once it is done");
    }

    #[test]
    fn a_refusal_is_said_a_missing_switch_does_nothing_and_hostile_switch_lists_do_not_panic() {
        let mut t = tile(vec![Ok(with_switches()), Ok(json!({"ok": false, "error": "you said no"}).to_string())]);
        t.event(&Event::Hit { id: 20 });
        assert_eq!(t.note(), "you said no");
        assert!(texts(&t.frame()).contains(&"you said no".to_string()));
        let n = t.source().calls.len();
        assert!(!t.event(&Event::Hit { id: 27 }), "no such switch");
        assert_eq!(t.source().calls.len(), n);
        for bad in [json!({"toggles": 5}), json!({"toggles": [1, null, {"id": 3}]}), json!({"toggles": [{"id": "a", "label": "b"}]}), json!({"toggles": vec![json!({"id": "a", "label": "b", "on": true}); 50]})] {
            let t = tile(vec![Ok(bad.to_string())]);
            assert!(t.frame().encode().is_ok());
        }
    }
}
