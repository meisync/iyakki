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
pub const CAPS: [&str; 4] = ["settings.get", "settings.set", "settings.page", "settings.form"];

const REFRESH_MS: u32 = 10_000;
const TOP: i32 = 40;
/// The room the tab row takes when the account has read-only pages.
const TABS_H: i32 = 28;
const MAX_PAGES: usize = 6;
const MAX_FORMS: usize = 3;
const MAX_FIELDS: usize = 6;
const MAX_VALUE: usize = 80;
/// A form: what the service lists (`settings.get` `forms`). The tile only fills in text; the service decides what a form may change.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Form {
    pub id: String,
    pub title: String,
    pub fields: Vec<(String, String)>,
}

/// The forms the service lists: at most three of at most six fields.
pub fn forms_of(v: &Value) -> Vec<Form> {
    v["forms"].as_array().map(|a| a.iter().take(MAX_FORMS).filter_map(|f| {
        let fields: Vec<(String, String)> = f["fields"].as_array()?.iter().take(MAX_FIELDS).filter_map(|x| Some((x["name"].as_str()?.chars().take(24).collect(), x["label"].as_str()?.chars().take(60).collect()))).collect();
        if fields.is_empty() { return None; }
        Some(Form { id: f["id"].as_str()?.chars().take(24).collect(), title: f["title"].as_str()?.chars().take(12).collect(), fields })
    }).collect()).unwrap_or_default()
}

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
    /// The read-only pages the service offers (id, title) and the one shown (None: the overview).
    pages: Vec<(String, String)>,
    page: Option<usize>,
    page_lines: Vec<String>,
    /// The forms the service offers, the one shown, what was typed in each (kept while the tile runs), the field with the caret, and a
    /// submission that waits for the person's answer.
    forms: Vec<Form>,
    form: Option<usize>,
    values: Vec<Vec<String>>,
    focus: usize,
    form_pending: bool,
}

/// The pages the service lists: (id, title), at most six.
pub fn pages_of(v: &Value) -> Vec<(String, String)> {
    v["pages"].as_array().map(|a| a.iter().take(MAX_PAGES).filter_map(|t| Some((t["id"].as_str()?.chars().take(24).collect(), t["title"].as_str()?.chars().take(12).collect()))).collect()).unwrap_or_default()
}

/// The lines of one page's answer (`settings.page`): at most 30 lines of 160 characters.
pub fn page_lines_of(v: &Value) -> Vec<String> {
    v["lines"].as_array().map(|a| a.iter().take(30).filter_map(|l| l.as_str().map(|t| t.chars().take(160).collect())).collect()).unwrap_or_default()
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
        let mut t = SettingsTile { src, size: (360, 480), dark: false, lines: vec![], error: String::new(), scroll: 0, since: 0, toggles: vec![], pending: None, note: String::new(), pages: vec![], page: None, page_lines: vec![], forms: vec![], form: None, values: vec![], focus: 0, form_pending: false };
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

    /// The tabs as (hit id, label, rect, shown), laid out in rows that wrap at the tile's width.
    fn tabs(&self) -> Vec<(u32, String, Rect, bool)> {
        if self.pages.is_empty() && self.forms.is_empty() {
            return vec![];
        }
        let mut all: Vec<(u32, String, bool)> = vec![(30, "Overview".into(), self.page.is_none() && self.form.is_none())];
        all.extend(self.pages.iter().enumerate().map(|(i, (_, t))| (31 + i as u32, t.clone(), self.page == Some(i))));
        all.extend(self.forms.iter().enumerate().map(|(i, f)| (40 + i as u32, f.title.clone(), self.form == Some(i))));
        let (mut x, mut row) = (8, 0);
        let mut o = vec![];
        for (id, label, on) in all {
            let w = 20 + label.chars().count() as i32 * 7;
            if x + w > self.size.0 - 4 && x > 8 {
                x = 8;
                row += 1;
            }
            o.push((id, label, Rect::new(x, 34 + row * TABS_H, w, 22), on));
            x += w + 4;
        }
        o
    }

    fn top(&self) -> i32 {
        let rows = self.tabs().iter().map(|t| (t.2.y - 34) / TABS_H + 1).max().unwrap_or(0);
        TOP + rows * TABS_H
    }

    fn per_page(&self) -> usize {
        (((self.size.1 - self.top()) / 18).max(1)) as usize
    }

    fn total(&self) -> usize {
        if self.form.is_some() { 0 } else if self.page.is_some() { self.page_lines.len() } else { self.lines.len() }
    }

    /// Show one read-only page (or the overview with `None`): the text of that page's status tool, read, never changed here.
    fn show(&mut self, which: Option<usize>) {
        self.scroll = 0;
        self.since = 0;
        self.form = None;
        let Some(i) = which else {
            self.page = None;
            return;
        };
        let Some((id, _)) = self.pages.get(i).cloned() else { return };
        match call_json(&mut self.src, "settings", "page", &serde_json::json!({"id": id}).to_string()) {
            Ok(v) => {
                self.error.clear();
                self.page_lines = page_lines_of(&v);
                self.page = Some(i);
            }
            Err(e) => self.error = clip(&e, 300, 12),
        }
    }

    fn open_form(&mut self, i: usize) {
        self.scroll = 0;
        self.since = 0;
        self.focus = 0;
        self.page = None;
        self.form_pending = false;
        self.note.clear();
        self.form = Some(i);
    }

    /// Send the typed values to the service, which makes the change through the person's own tool and asks first. Nothing typed is kept
    /// after a change; a refusal keeps it so nothing has to be typed again.
    fn submit(&mut self) {
        let Some(i) = self.form else { return };
        let (Some(f), Some(vals)) = (self.forms.get(i).cloned(), self.values.get(i).cloned()) else { return };
        self.since = 0;
        let mut values = serde_json::Map::new();
        for ((name, _), v) in f.fields.iter().zip(vals) {
            values.insert(name.clone(), Value::String(v));
        }
        match call_json(&mut self.src, "settings", "form", &serde_json::json!({"id": f.id, "values": values}).to_string()) {
            Ok(v) if v["ok"].as_bool() == Some(true) => {
                self.form_pending = false;
                self.note = "Done.".into();
                self.values[i] = vec![String::new(); f.fields.len()];
                self.focus = 0;
            }
            Ok(v) if v["asked"].as_bool() == Some(true) => {
                self.form_pending = true;
                self.note = "Waiting for your answer in the approvals list.".into();
            }
            Ok(v) => {
                self.form_pending = false;
                self.note = v["error"].as_str().unwrap_or("that could not be changed").chars().take(200).collect();
            }
            Err(e) => {
                self.form_pending = false;
                self.note = e.chars().take(200).collect();
            }
        }
    }

    fn type_into(&mut self, text: &str) -> bool {
        let (Some(i), f) = (self.form, self.focus) else { return false };
        let Some(v) = self.values.get_mut(i).and_then(|v| v.get_mut(f)) else { return false };
        let room = MAX_VALUE.saturating_sub(v.chars().count());
        v.extend(text.chars().filter(|c| !c.is_control()).take(room));
        self.form_pending = false;
        true
    }

    fn refresh(&mut self) {
        self.since = 0;
        match call_json(&mut self.src, "settings", "get", "{}") {
            Ok(v) => {
                self.error.clear();
                self.lines = lines_of(&v);
                self.toggles = toggles_of(&v);
                self.pages = pages_of(&v);
                self.forms = forms_of(&v);
                if self.values.len() != self.forms.len() {
                    self.values = self.forms.iter().map(|f| vec![String::new(); f.fields.len()]).collect();
                }
                if self.form.is_some_and(|i| i >= self.forms.len()) {
                    self.form = None;
                }
                if self.page.is_some_and(|i| i >= self.pages.len()) {
                    self.page = None;
                }
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
                if self.form_pending {
                    if self.since >= 2000 {
                        self.submit();
                        return true;
                    }
                    return false;
                }
                if self.form.is_some() {
                    return false;
                }
                if let Some((id, on)) = self.pending.clone() {
                    if self.since >= 2000 {
                        self.set(&id, on);
                        return true;
                    }
                    return false;
                }
                if self.since >= REFRESH_MS {
                    if let Some(i) = self.page {
                        self.show(Some(i));
                        return true;
                    }
                    self.refresh();
                    return true;
                }
                false
            }
            Event::TextInput { text } if self.form.is_some() => self.type_into(text),
            Event::Key { code: 8, down: true, .. } if self.form.is_some() => {
                let (i, f) = (self.form.unwrap_or(0), self.focus);
                if let Some(v) = self.values.get_mut(i).and_then(|v| v.get_mut(f)) {
                    v.pop();
                }
                self.form_pending = false;
                true
            }
            Event::Key { code: 9, down: true, .. } if self.form.is_some() => {
                let n = self.form.and_then(|i| self.forms.get(i)).map_or(1, |f| f.fields.len().max(1));
                self.focus = (self.focus + 1) % n;
                true
            }
            Event::Key { code: 13, down: true, .. } if self.form.is_some() => {
                self.submit();
                true
            }
            Event::Hit { id } if (40..40 + MAX_FORMS as u32).contains(id) && ((*id - 40) as usize) < self.forms.len() => {
                self.open_form((*id - 40) as usize);
                true
            }
            Event::Hit { id } if (50..50 + MAX_FIELDS as u32).contains(id) && self.form.and_then(|i| self.forms.get(i)).is_some_and(|f| ((*id - 50) as usize) < f.fields.len()) => {
                self.focus = (*id - 50) as usize;
                true
            }
            Event::Hit { id: 60 } if self.form.is_some() => {
                self.submit();
                true
            }
            Event::Hit { id: 61 } if self.form.is_some() => {
                let (i, n) = (self.form.unwrap_or(0), self.forms.get(self.form.unwrap_or(0)).map_or(0, |f| f.fields.len()));
                if let Some(v) = self.values.get_mut(i) {
                    *v = vec![String::new(); n];
                }
                self.focus = 0;
                self.form_pending = false;
                self.note.clear();
                true
            }
            Event::Hit { id: 2 } if self.form.is_some() => {
                self.refresh();
                true
            }
            Event::Hit { id: 2 } => {
                match self.page {
                    Some(i) => self.show(Some(i)),
                    None => self.refresh(),
                }
                true
            }
            Event::Hit { id: 30 } => {
                self.show(None);
                true
            }
            Event::Hit { id } if (31..31 + MAX_PAGES as u32).contains(id) && ((*id - 31) as usize) < self.pages.len() => {
                self.show(Some((*id - 31) as usize));
                true
            }
            Event::Hit { id: 3 } => {
                self.scroll = self.scroll.saturating_sub(self.per_page());
                true
            }
            Event::Hit { id: 4 } => {
                if self.scroll + self.per_page() < self.total() {
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
        if self.total() > self.per_page() {
            button(&mut l, 3, Rect::new(w - 128, 4, 38, 24), "Up", &p);
            button(&mut l, 4, Rect::new(w - 172, 4, 38, 24), "Down", &p);
        }
        for (id, label, r, on) in self.tabs() {
            if on {
                l.push(Op::RoundRect { r, radius: 6, color: p.accent });
                l.push(text(r.x + 10, r.y + 4, 13, Color::rgb(255, 255, 255), &label));
                iyakki_tile_kit::named_hit(&mut l, id, iyakki_tile_kit::abi::Role::Button, r, &label, "shown", 0);
            } else {
                l.push(Op::RoundRect { r, radius: 6, color: p.bar });
                l.push(text(r.x + 10, r.y + 4, 13, p.fg, &label));
                iyakki_tile_kit::named_hit(&mut l, id, iyakki_tile_kit::abi::Role::Button, r, &label, "", 0);
            }
        }
        let mut y = self.top();
        if !self.error.is_empty() {
            l.push(text(8, y, 12, p.bad, &clip(&self.error, w - 16, 12)));
            y += 20;
        }
        if let Some(f) = self.form.and_then(|i| self.forms.get(i).map(|f| (i, f))) {
            let (i, f) = f;
            l.push(text(8, y, 11, p.mute, "Passwords are never typed here: give the name of a vault entry."));
            y += 18;
            for (k, (_, label)) in f.fields.iter().enumerate() {
                l.push(text(12, y, 11, p.mute, &clip(label, w - 24, 11)));
                y += 15;
                let r = Rect::new(8, y, w - 16, 24);
                l.push(Op::RoundRect { r, radius: 4, color: if self.focus == k { p.accent } else { p.bar } });
                l.push(Op::RoundRect { r: Rect::new(r.x + 1, r.y + 1, r.w - 2, r.h - 2), radius: 4, color: p.bg });
                let v = self.values.get(i).and_then(|v| v.get(k)).cloned().unwrap_or_default();
                let shown = if self.focus == k { format!("{v}|") } else { v.clone() };
                l.push(text(14, y + 5, 13, p.fg, &clip(&shown, w - 28, 13)));
                iyakki_tile_kit::named_hit(&mut l, 50 + k as u32, iyakki_tile_kit::abi::Role::Field, r, label, &v, 0);
                y += 30;
            }
            button(&mut l, 60, Rect::new(8, y, 90, 26), "Apply", &p);
            button(&mut l, 61, Rect::new(104, y, 70, 26), "Clear", &p);
            if !self.note.is_empty() {
                let c = if self.note.starts_with("Waiting") { Color(200, 130, 20, 255) } else if self.note == "Done." { p.mute } else { p.bad };
                l.push(text(8, h - 16, 11, c, &clip(&self.note, w - 16, 11)));
            }
            return l;
        }
        if self.page.is_some() {
            for t in self.page_lines.iter().skip(self.scroll).take(self.per_page()) {
                l.push(text(12, y, 13, p.fg, &clip(t, w - 20, 13)));
                y += line_height(13) + 3;
            }
            if self.page_lines.is_empty() {
                l.push(text(12, y, 13, p.mute, "nothing to show"));
            }
            l.push(text(8, h - 16, 11, p.mute, "Shown only. To change it, use the Settings page or the chat."));
            return l;
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
        assert!(ids.iter().all(|i| (2..=4).contains(i) || (20..28).contains(i) || (30..40).contains(i)), "{ids:?}");
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

    fn with_pages() -> String {
        json!({"user": "ann", "role": "user", "admin": false, "models": [], "tools": 3, "pages": [{"id": "display", "title": "Display"}, {"id": "sound", "title": "Sound"}]}).to_string()
    }

    #[test]
    fn pages_are_tabs_each_read_by_settings_page_and_never_changed_from_here() {
        let page = || Ok(json!({"title": "Display", "lines": ["Screen 1: 2560x1440", "Screen 2: off"]}).to_string());
        let mut t = tile(vec![Ok(with_pages()), page(), Err("gone".into())]);
        let x = texts(&t.frame());
        assert!(x.contains(&"Overview".to_string()) && x.contains(&"Display".to_string()) && x.contains(&"Sound".to_string()), "{x:?}");
        assert!(t.event(&Event::Hit { id: 31 }));
        let x = texts(&t.frame());
        assert!(x.contains(&"Screen 1: 2560x1440".to_string()) && !x.contains(&"Account".to_string()), "{x:?}");
        assert_eq!(t.source().calls.last().unwrap(), &("settings".to_string(), "page".to_string()));
        // a failed page is one red line and the old page stays; a tab that does not exist does nothing
        assert!(t.event(&Event::Hit { id: 32 }));
        assert_eq!(t.error(), "gone");
        let n = t.source().calls.len();
        assert!(!t.event(&Event::Hit { id: 35 }));
        assert_eq!(t.source().calls.len(), n);
        assert!(t.event(&Event::Hit { id: 30 }));
        assert!(texts(&t.frame()).contains(&"Account".to_string()));
        assert!(t.source().calls.iter().all(|c| c.0 == "settings" && ["get", "page"].contains(&c.1.as_str())), "{:?}", t.source().calls);
    }

    #[test]
    fn hostile_page_lists_and_lines_do_not_panic_and_are_capped() {
        for bad in [json!({"pages": 5}), json!({"pages": [1, null, {"id": 3}]}), json!({"pages": vec![json!({"id": "a", "title": "b"}); 50]})] {
            let t = tile(vec![Ok(bad.to_string())]);
            assert!(t.frame().encode().is_ok());
        }
        assert_eq!(pages_of(&json!({"pages": vec![json!({"id": "a", "title": "b"}); 50]})).len(), MAX_PAGES);
        assert_eq!(page_lines_of(&json!({"lines": vec!["x".repeat(500); 99]})).len(), 30);
        assert_eq!(page_lines_of(&json!({"lines": ["x".repeat(500)]}))[0].len(), 160);
        assert!(page_lines_of(&json!({"lines": 5})).is_empty());
    }

    fn with_forms() -> String {
        json!({"user": "ann", "role": "admin", "admin": true, "models": [], "tools": 3, "pages": [{"id": "display", "title": "Display"}], "forms": [
            {"id": "wifi", "title": "Join Wi-Fi", "fields": [{"name": "ssid", "label": "Network name"}, {"name": "secret_entry", "label": "Vault entry"}]},
            {"id": "model", "title": "Model", "fields": [{"name": "provider", "label": "Provider"}]}]})
        .to_string()
    }

    fn type_in(t: &mut SettingsTile<Fake>, s: &str) {
        t.event(&Event::TextInput { text: s.into() });
    }

    #[test]
    fn a_form_is_typed_in_sent_by_settings_form_waits_for_the_answer_and_is_cleared_after_a_yes() {
        let mut t = tile(vec![Ok(with_forms()), Ok(json!({"ok": false, "asked": true}).to_string()), Ok(json!({"ok": true}).to_string())]);
        let x = texts(&t.frame());
        assert!(x.contains(&"Join Wi-Fi".to_string()) && x.contains(&"Model".to_string()), "{x:?}");
        assert!(t.event(&Event::Hit { id: 40 }));
        let x = texts(&t.frame());
        assert!(x.contains(&"Network name".to_string()) && x.contains(&"Apply".to_string()), "{x:?}");
        type_in(&mut t, "home\u{7}-net");
        assert!(t.event(&Event::Key { code: 9, mods: 0, down: true }));
        type_in(&mut t, "wifi-pass");
        assert!(texts(&t.frame()).contains(&"wifi-pass|".to_string()));
        assert!(t.event(&Event::Hit { id: 60 }));
        let (d, m) = t.source().calls.last().cloned().unwrap();
        assert_eq!((d.as_str(), m.as_str()), ("settings", "form"));
        assert!(texts(&t.frame()).iter().any(|x| x.starts_with("Waiting for your answer")));
        // nothing is sent again before two seconds; then it asks again with the same values and a yes clears the form
        let n = t.source().calls.len();
        assert!(!t.event(&Event::Tick { dt_ms: 1000 }));
        assert_eq!(t.source().calls.len(), n);
        assert!(t.event(&Event::Tick { dt_ms: 1000 }));
        assert!(texts(&t.frame()).contains(&"Done.".to_string()));
        assert!(!texts(&t.frame()).iter().any(|x| x.contains("home-net") || x.contains("wifi-pass")));
    }

    #[test]
    fn a_refusal_keeps_what_was_typed_backspace_and_clear_work_and_limits_hold() {
        let mut t = tile(vec![Ok(with_forms()), Ok(json!({"ok": false, "error": "type the network name"}).to_string())]);
        t.event(&Event::Hit { id: 40 });
        type_in(&mut t, "abc");
        t.event(&Event::Key { code: 8, mods: 0, down: true });
        assert!(texts(&t.frame()).contains(&"ab|".to_string()));
        t.event(&Event::Key { code: 13, mods: 0, down: true });
        let x = texts(&t.frame());
        assert!(x.contains(&"type the network name".to_string()) && x.contains(&"ab|".to_string()), "{x:?}");
        type_in(&mut t, &"z".repeat(500));
        assert!(t.values[0][0].chars().count() == MAX_VALUE);
        t.event(&Event::Hit { id: 61 });
        assert!(t.values[0][0].is_empty());
        // a field that does not exist is not focused; leaving the form for the overview works
        assert!(!t.event(&Event::Hit { id: 59 }));
        assert!(t.event(&Event::Hit { id: 30 }));
        assert!(texts(&t.frame()).contains(&"Account".to_string()));
        assert!(t.source().calls.iter().all(|c| c.0 == "settings" && ["get", "form"].contains(&c.1.as_str())));
    }

    #[test]
    fn many_tabs_wrap_to_more_rows_and_hostile_form_lists_do_not_panic() {
        let mut t = tile(vec![Ok(with_forms())]);
        t.event(&Event::Resize { w: 200, h: 400, scale_pct: 100 });
        let rows: std::collections::BTreeSet<i32> = t.tabs().iter().map(|x| x.2.y).collect();
        assert!(rows.len() >= 2 && t.tabs().iter().all(|x| x.2.x + x.2.w <= 200));
        for bad in [json!({"forms": 4}), json!({"forms": [1, {"id": "a"}, {"id": "a", "title": "b", "fields": []}, {"id": "a", "title": "b", "fields": 3}]}), json!({"forms": vec![json!({"id": "a", "title": "b", "fields": [{"name": "n", "label": "l"}]}); 20]})] {
            let t = tile(vec![Ok(bad.to_string())]);
            assert!(t.frame().encode().is_ok());
        }
        assert_eq!(forms_of(&json!({"forms": vec![json!({"id": "a", "title": "b", "fields": vec![json!({"name": "n", "label": "l"}); 20]}); 20]})).len(), MAX_FORMS);
    }
}
