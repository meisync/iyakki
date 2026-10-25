//! The Touch Bar strip: what the 2170 x 60 display shows and what a touch on it does. Pure model, layout, drawing and touch handling;
//! nothing here talks to the device (that is `touchbar_host.rs`, over `iyakki_drivers::touchbar`).
//!
//! Four strips, cycled with the chip next to Esc: **System** (volume with mute, brightness, keyboard light, play/pause, clock),
//! **Recent** (the last apps), **AI** (the agent's state, Chat, Ask, Stop, Approve / Deny) and **App** (up to six items the focused app
//! offers; the system strip when it offers none). Esc is always at the left. The picture is redrawn only when something it shows
//! changed, at most ten times a second; it goes dark after five idle minutes and shows only the name on the sign-in screen.
//!
//! The shell builds a [`Ctx`] from its own state, asks [`Bar::update`] for a new [`Picture`] when one is due and feeds touches to
//! [`Bar::touch`], which returns [`Action`]s for the shell to carry out (a key, a media key, an app to open, a volume to set).

use crate::fnkeys::FnKey;
use crate::gfx::{rgb, Face, Gfx, Icon};
use crate::safeclamp::SafeClamp;
use crate::ui::{Key, CARD, CARD_SEL, DIM, ERR, MUTED, OK, TEXT};

/// Bar height every layout is made for; other heights scale.
pub const BASE_H: u32 = 60;
/// Items an app may offer.
pub const MAX_APP_ITEMS: usize = 6;
/// Recent apps shown.
pub const MAX_RECENT: usize = 6;
/// The fastest redraw rate: one picture per this many milliseconds.
pub const MIN_GAP_MS: u64 = 100;
/// Idle time after which the bar goes dark.
pub const IDLE_DARK_MS: u64 = 5 * 60 * 1000;

const BG: [u8; 4] = rgb(0, 0, 0);
const ACCENT: [u8; 4] = rgb(79, 140, 255);

// ------------------------------------------------------------------------------------------------ strips and items

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Strip {
    System,
    Recent,
    Ai,
    App,
}

impl Strip {
    pub const ALL: [Strip; 4] = [Strip::System, Strip::Recent, Strip::Ai, Strip::App];
    pub fn id(self) -> &'static str {
        match self {
            Strip::System => "system",
            Strip::Recent => "recent",
            Strip::Ai => "ai",
            Strip::App => "app",
        }
    }
    pub fn label(self) -> &'static str {
        match self {
            Strip::System => "System",
            Strip::Recent => "Recent",
            Strip::Ai => "AI",
            Strip::App => "App",
        }
    }
    pub fn from_id(s: &str) -> Option<Strip> {
        Strip::ALL.into_iter().find(|x| x.id() == s)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Glyph {
    None,
    Speaker,
    Muted,
    Sun,
    Light,
    Play,
    Pause,
    Next,
    Prev,
    Stop,
    Check,
    Cross,
    Plus,
    Back,
    Forward,
    Reload,
    Run,
    Save,
    App(Icon),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SliderKind {
    Volume,
    Brightness,
    KeyboardLight,
}

/// What the shell does for a touch.
#[derive(Clone, Debug, PartialEq)]
pub enum Action {
    None,
    /// Esc, as a key press for the focused screen.
    Esc,
    /// Show the next strip.
    Cycle,
    /// A media, volume or brightness key, exactly as the keyboard's would act.
    Fn(FnKey),
    /// A key for the focused app.
    Key(Key),
    /// Open (or return to) an app by id.
    Open(String),
    /// A slider moved to this fraction (0 to 1).
    Slider(SliderKind, f32),
    /// Mute or unmute.
    ToggleMute,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Button,
    Slider,
    Tile,
    /// A status chip: drawn like a button, acts only when it has an action.
    Chip,
    Label,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Item {
    pub id: String,
    pub label: String,
    pub glyph: Glyph,
    pub kind: Kind,
    pub action: Action,
    pub enabled: bool,
    /// Drawn highlighted (the current strip, a muted speaker, something running).
    pub on: bool,
    /// Slider position or a progress, 0 to 1.
    pub value: f32,
    pub min_w: u32,
    /// Share of the free width this item takes (0 = none).
    pub grow: u32,
    /// Colour of the chip dot or label text (state colours).
    pub tone: Tone,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tone {
    Normal,
    Good,
    Warn,
    Bad,
}

impl Item {
    pub fn button(id: &str, label: &str, glyph: Glyph, action: Action, min_w: u32) -> Item {
        Item { id: id.into(), label: label.into(), glyph, kind: Kind::Button, action, enabled: true, on: false, value: 0.0, min_w, grow: 0, tone: Tone::Normal }
    }
    pub fn slider(id: &str, glyph: Glyph, kind: SliderKind, value: f32, on: bool) -> Item {
        Item { id: id.into(), label: String::new(), glyph, kind: Kind::Slider, action: Action::Slider(kind, value), enabled: true, on, value: value.clamp_safe(0.0, 1.0), min_w: 300, grow: 3, tone: Tone::Normal }
    }
    pub fn tile(id: &str, label: &str, icon: Icon, action: Action) -> Item {
        Item { id: id.into(), label: label.into(), glyph: Glyph::App(icon), kind: Kind::Tile, action, enabled: true, on: false, value: 0.0, min_w: 150, grow: 0, tone: Tone::Normal }
    }
    pub fn chip(id: &str, label: &str, tone: Tone, action: Action, min_w: u32) -> Item {
        Item { id: id.into(), label: label.into(), glyph: Glyph::None, kind: Kind::Chip, action, enabled: true, on: false, value: 0.0, min_w, grow: 0, tone }
    }
    pub fn label(id: &str, text: &str, min_w: u32) -> Item {
        Item { id: id.into(), label: text.into(), glyph: Glyph::None, kind: Kind::Label, action: Action::None, enabled: false, on: false, value: 0.0, min_w, grow: 0, tone: Tone::Normal }
    }
    fn off(mut self) -> Item {
        self.enabled = false;
        self
    }
    fn lit(mut self, on: bool) -> Item {
        self.on = on;
        self
    }
}

// ------------------------------------------------------------------------------------------------ what the shell tells the bar

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AgentState {
    Ready,
    Working,
    NeedsLogin,
    Missing,
    Unknown,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Recent {
    pub id: String,
    pub label: String,
    pub icon: Icon,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Ctx {
    pub signed_in: bool,
    /// Milliseconds since the last key, touch or click anywhere.
    pub idle_ms: u64,
    /// The screen is blanked (no such feature yet; kept for when there is).
    pub screen_off: bool,
    pub volume: f32,
    pub muted: bool,
    pub has_sound: bool,
    pub brightness: Option<f32>,
    pub kbd_light: Option<f32>,
    pub playing: bool,
    pub clock: String,
    pub recents: Vec<Recent>,
    /// The id of the focused app (`chat`, `ide`, ...), `None` on the launcher.
    pub focused: Option<String>,
    pub agent_name: String,
    pub agent: AgentState,
    /// The focused chat has a run going / is waiting for a decision.
    pub busy: bool,
    pub approval: bool,
}

impl Default for Ctx {
    fn default() -> Ctx {
        Ctx { signed_in: true, idle_ms: 0, screen_off: false, volume: 0.5, muted: false, has_sound: true, brightness: Some(0.5), kbd_light: None, playing: false, clock: "12:00".into(), recents: Vec::new(), focused: None, agent_name: "Claude Code".into(), agent: AgentState::Ready, busy: false, approval: false }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Power {
    /// Everything shown.
    On,
    /// Signed out: the name only, nothing about the person.
    Locked,
    /// Black: idle or the screen is off.
    Dark,
}

pub fn power(c: &Ctx) -> Power {
    if c.screen_off || c.idle_ms >= IDLE_DARK_MS {
        Power::Dark
    } else if !c.signed_in {
        Power::Locked
    } else {
        Power::On
    }
}

/// The strip that is really shown: an approval waiting for a decision brings the AI strip up; the App strip falls back to System
/// when the focused app offers nothing.
pub fn effective_strip(chosen: Strip, c: &Ctx, reg: &Registry) -> Strip {
    if c.approval {
        return Strip::Ai;
    }
    if chosen == Strip::App && c.focused.as_deref().is_none_or(|a| reg.get(a).is_empty()) {
        return Strip::System;
    }
    chosen
}

// ------------------------------------------------------------------------------------------------ the per-app registry

/// Up to [`MAX_APP_ITEMS`] items per app, offered while it is focused.
#[derive(Clone, Debug, Default)]
pub struct Registry {
    map: std::collections::BTreeMap<String, Vec<Item>>,
}

impl Registry {
    /// Offer `items` for the app `app` (the first six with distinct ids are kept). An empty list removes the offer.
    pub fn register(&mut self, app: &str, items: Vec<Item>) {
        let mut kept: Vec<Item> = Vec::new();
        for i in items {
            if kept.len() < MAX_APP_ITEMS && !kept.iter().any(|k| k.id == i.id) {
                kept.push(i);
            }
        }
        if kept.is_empty() {
            self.map.remove(app);
        } else {
            self.map.insert(app.to_string(), kept);
        }
    }

    pub fn get(&self, app: &str) -> &[Item] {
        self.map.get(app).map_or(&[], |v| v.as_slice())
    }

    /// What the apps that have shortcuts offer today. Every action is a key the app already handles (its own help lists it) or a media key.
    pub fn standard() -> Registry {
        let mut r = Registry::default();
        let k = |id: &str, label: &str, g: Glyph, key: Key| Item::button(id, label, g, Action::Key(key), 120);
        let m = |id: &str, label: &str, g: Glyph, key: FnKey| Item::button(id, label, g, Action::Fn(key), 110);
        r.register("chat", vec![k("new", "New chat", Glyph::Plus, Key::Ctrl('n')), k("model", "Model", Glyph::None, Key::Ctrl('p')), k("agent", "Agent", Glyph::None, Key::Ctrl('g')), k("approvals", "Approvals", Glyph::Check, Key::Ctrl('q'))]);
        r.register("ide", vec![k("run", "Run", Glyph::Run, Key::F(5)), k("save", "Save", Glyph::Save, Key::Ctrl('s')), k("open", "Open file", Glyph::None, Key::Ctrl('p')), k("tree", "Files", Glyph::None, Key::Ctrl('b')), k("panel", "Panel", Glyph::None, Key::Ctrl('j'))]);
        r.register("tales", vec![k("play", "Play", Glyph::Play, Key::Char(' ')), k("save", "Save", Glyph::Save, Key::Ctrl('s')), k("model", "Model", Glyph::None, Key::Ctrl('p'))]);
        r.register("radio", vec![m("prev", "Prev", Glyph::Prev, FnKey::Prev), m("play", "Play", Glyph::Play, FnKey::PlayPause), m("next", "Next", Glyph::Next, FnKey::Next), m("stop", "Stop", Glyph::Stop, FnKey::Stop)]);
        r.register("media", vec![m("prev", "Prev", Glyph::Prev, FnKey::Prev), m("play", "Play", Glyph::Play, FnKey::PlayPause), m("next", "Next", Glyph::Next, FnKey::Next)]);
        // the browser's own keys (its hint: b back, f forward, r reload, g address); they work in the built-in reader view
        r.register("browser", vec![k("back", "Back", Glyph::Back, Key::Char('b')), k("forward", "Forward", Glyph::Forward, Key::Char('f')), k("reload", "Reload", Glyph::Reload, Key::Char('r')), k("address", "Address", Glyph::None, Key::Ctrl('l'))]);
        r
    }
}

// ------------------------------------------------------------------------------------------------ building the strips

/// The first two slots of every strip: Esc and the strip chip.
fn head(shown: Strip, cfg_strips: &[Strip]) -> Vec<Item> {
    let n = cfg_strips.len().max(1);
    let at = cfg_strips.iter().position(|s| *s == shown).unwrap_or(0);
    let mut chip = Item::button("mode", shown.label(), Glyph::None, Action::Cycle, 118);
    chip.value = at as f32 / n as f32;
    chip.tone = if n > 1 { Tone::Normal } else { Tone::Warn };
    vec![Item::button("esc", "esc", Glyph::None, Action::Esc, 96), chip]
}

fn media_item(c: &Ctx) -> Item {
    Item::button("playpause", if c.playing { "Pause" } else { "Play" }, if c.playing { Glyph::Pause } else { Glyph::Play }, Action::Fn(FnKey::PlayPause), 110).lit(c.playing)
}

fn system_items(c: &Ctx) -> Vec<Item> {
    let mut v = Vec::new();
    let mut vol = Item::slider("volume", if c.muted { Glyph::Muted } else { Glyph::Speaker }, SliderKind::Volume, c.volume, c.muted);
    vol.enabled = c.has_sound;
    v.push(vol);
    v.push(Item::button("mute", if c.muted { "Unmute" } else { "Mute" }, Glyph::None, Action::ToggleMute, 100).off_if(!c.has_sound));
    if let Some(b) = c.brightness {
        v.push(Item::slider("brightness", Glyph::Sun, SliderKind::Brightness, b, false));
    }
    if let Some(k) = c.kbd_light {
        let mut s = Item::slider("kbdlight", Glyph::Light, SliderKind::KeyboardLight, k, false);
        s.min_w = 200;
        s.grow = 2;
        v.push(s);
    }
    v.push(media_item(c));
    v.push(Item::label("clock", &c.clock, 90));
    v
}

fn recent_items(c: &Ctx) -> Vec<Item> {
    let mut v: Vec<Item> = c.recents.iter().take(MAX_RECENT).map(|r| Item::tile(&format!("recent-{}", r.id), &r.label, r.icon, Action::Open(r.id.clone()))).collect();
    if v.is_empty() {
        v.push(Item::label("none", "Apps you open show up here", 320));
    }
    v
}

fn agent_chip(c: &Ctx) -> Item {
    let (text, tone) = match c.agent {
        AgentState::Ready => ("ready", Tone::Good),
        AgentState::Working => ("working", Tone::Warn),
        AgentState::NeedsLogin => ("needs login", Tone::Bad),
        AgentState::Missing => ("not installed", Tone::Bad),
        AgentState::Unknown => ("unknown", Tone::Normal),
    };
    Item::chip("agent", &format!("{}: {text}", c.agent_name), tone, Action::Open("chat".into()), 250)
}

fn ai_items(c: &Ctx) -> Vec<Item> {
    let in_chat = c.focused.as_deref() == Some("chat");
    let mut v = vec![agent_chip(c)];
    if c.approval {
        v.push(Item::button("approve", "Approve", Glyph::Check, Action::Key(Key::Ctrl('y')), 130));
        v.push(Item::button("deny", "Deny", Glyph::Cross, Action::Key(Key::Ctrl('n')), 110));
    }
    v.push(Item::button("chat", "Chat", Glyph::App(Icon::Chat), Action::Open("chat".into()), 110).lit(in_chat));
    v.push(Item::button("ask", "Ask", Glyph::None, Action::Open("chat".into()), 90));
    v.push(Item::button("stop", "Stop agent", Glyph::Stop, Action::Key(Key::Ctrl('k')), 140).off_if(!(in_chat && c.busy)));
    if c.focused.as_deref() == Some("ide") {
        v.push(Item::button("run", "Run", Glyph::Run, Action::Key(Key::F(5)), 100));
        v.push(Item::button("save", "Save", Glyph::Save, Action::Key(Key::Ctrl('s')), 100));
    }
    v
}

impl Item {
    fn off_if(self, cond: bool) -> Item {
        if cond {
            self.off()
        } else {
            self
        }
    }
}

/// The items of a strip, Esc and the strip chip first.
pub fn build(shown: Strip, cfg_strips: &[Strip], c: &Ctx, reg: &Registry) -> Vec<Item> {
    let mut v = head(shown, cfg_strips);
    match power(c) {
        Power::On => {}
        // signed out or dark: nothing but what draws the name (see `render`)
        _ => return Vec::new(),
    }
    match shown {
        Strip::System => v.extend(system_items(c)),
        Strip::Recent => v.extend(recent_items(c)),
        Strip::Ai => v.extend(ai_items(c)),
        Strip::App => {
            let mut items = c.focused.as_deref().map(|a| reg.get(a).to_vec()).unwrap_or_default();
            // the media items show what is playing
            for i in items.iter_mut() {
                if let Action::Fn(FnKey::PlayPause) = i.action {
                    i.on = c.playing;
                    i.label = if c.playing { "Pause".into() } else { "Play".into() };
                    i.glyph = if c.playing { Glyph::Pause } else { Glyph::Play };
                }
            }
            if items.is_empty() {
                v.extend(system_items(c));
            } else {
                v.extend(items);
            }
        }
    }
    v
}

// ------------------------------------------------------------------------------------------------ layout

/// A rectangle in bar pixels.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub struct Rc {
    pub x: u32,
    pub y: u32,
    pub w: u32,
    pub h: u32,
}

impl Rc {
    pub fn contains_x(&self, x: i32) -> bool {
        x >= self.x as i32 && x < (self.x + self.w) as i32
    }
    pub fn right(&self) -> u32 {
        self.x + self.w
    }
    pub fn bottom(&self) -> u32 {
        self.y + self.h
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Slot {
    pub rect: Rc,
    pub item: Item,
}

/// Place the items left to right in a `w` x `h` bar. Items that do not fit are dropped from the end (Esc and the chip come first);
/// spare width goes to the items with a `grow` share. Every rectangle lies inside the bar and none overlaps another.
pub fn layout(items: &[Item], w: u32, h: u32) -> Vec<Slot> {
    let s = (h as f32 / BASE_H as f32).max(0.2);
    let margin = (6.0 * s).round() as u32;
    let gap = (8.0 * s).round() as u32;
    let (top, ih) = (margin.min(h / 4), h.saturating_sub(2 * margin.min(h / 4)).max(1));
    let min_of = |i: &Item| ((i.min_w as f32 * s).round() as u32).max(8);
    let avail = w.saturating_sub(2 * margin);
    let mut count = 0usize;
    let mut used = 0u32;
    for (k, i) in items.iter().enumerate() {
        let need = min_of(i) + if k > 0 { gap } else { 0 };
        if used + need > avail {
            break;
        }
        used += need;
        count = k + 1;
    }
    let shown = &items[..count];
    let grow_total: u32 = shown.iter().map(|i| i.grow).sum();
    let spare = avail.saturating_sub(used);
    let mut out = Vec::with_capacity(count);
    let mut x = margin;
    let mut given = 0u32;
    let last_grower = shown.iter().rposition(|i| i.grow > 0);
    for (k, i) in shown.iter().enumerate() {
        let mut wi = min_of(i);
        if grow_total > 0 && i.grow > 0 {
            let extra = if Some(k) == last_grower { spare - given } else { spare * i.grow / grow_total };
            given += extra;
            wi += extra;
        }
        out.push(Slot { rect: Rc { x, y: top, w: wi, h: ih }, item: i.clone() });
        x += wi + gap;
    }
    out
}

// ------------------------------------------------------------------------------------------------ sliders

/// The track of a slider slot: from where the icon ends to a little before the right edge.
pub fn slider_track(r: &Rc) -> (f32, f32) {
    let pad = (r.h as f32 * 0.35).max(4.0);
    let icon = r.h as f32 * 1.05;
    let x0 = r.x as f32 + icon;
    let x1 = (r.x + r.w) as f32 - pad;
    (x0, x1.max(x0 + 1.0))
}

/// The slider value for a touch at bar x: 0 to the left of the track, 1 to the right of it.
pub fn slider_value(r: &Rc, x: i32) -> f32 {
    let (x0, x1) = slider_track(r);
    ((x as f32 - x0) / (x1 - x0)).clamp_safe(0.0, 1.0)
}

/// A slider value as a percentage the audio and backlight code takes.
pub fn percent(v: f32) -> u8 {
    (v.clamp_safe(0.0, 1.0) * 100.0).round() as u8
}

// ------------------------------------------------------------------------------------------------ touch

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TouchEv {
    Down(i32),
    Move(i32),
    Up(i32),
}

/// One finger on the bar. A button acts when the finger lifts over the button it went down on; a slider follows the finger.
#[derive(Clone, Debug, Default)]
pub struct Touch {
    slot: Option<usize>,
    inside: bool,
    last_value: Option<u8>,
}

impl Touch {
    /// The slot being pressed (drawn pressed), if any.
    pub fn pressed(&self) -> Option<usize> {
        self.slot.filter(|_| self.inside)
    }

    pub fn feed(&mut self, ev: TouchEv, slots: &[Slot]) -> Vec<Action> {
        let find = |x: i32| slots.iter().position(|s| s.rect.contains_x(x));
        let mut out = Vec::new();
        match ev {
            TouchEv::Down(x) => {
                self.slot = find(x).filter(|k| slots.get(*k).is_some_and(|s| s.item.enabled));
                self.inside = self.slot.is_some();
                self.last_value = None;
                if let Some(s) = self.slot.and_then(|k| slots.get(k)) {
                    if let (Kind::Slider, Action::Slider(kind, _)) = (s.item.kind, &s.item.action) {
                        let v = slider_value(&s.rect, x);
                        self.last_value = Some(percent(v));
                        out.push(Action::Slider(*kind, v));
                    }
                }
            }
            TouchEv::Move(x) => {
                if let Some(s) = self.slot.and_then(|k| slots.get(k)) {
                    if let (Kind::Slider, Action::Slider(kind, _)) = (s.item.kind, &s.item.action) {
                        let v = slider_value(&s.rect, x);
                        if self.last_value != Some(percent(v)) {
                            self.last_value = Some(percent(v));
                            out.push(Action::Slider(*kind, v));
                        }
                    } else {
                        self.inside = s.rect.contains_x(x);
                    }
                }
            }
            TouchEv::Up(x) => {
                if let Some(s) = self.slot.and_then(|k| slots.get(k)) {
                    if s.item.kind != Kind::Slider && s.rect.contains_x(x) && s.item.action != Action::None {
                        out.push(s.item.action.clone());
                    }
                }
                *self = Touch::default();
            }
        }
        out
    }
}

/// A touch screen's raw events (multi-touch protocol B) to [`TouchEv`]s in bar pixels. One finger counts; others are ignored.
#[derive(Clone, Debug)]
pub struct RawTouch {
    pub x_min: i32,
    pub x_max: i32,
    pub bar_w: u32,
    pub mirror: bool,
    /// The position axis along the bar (ABS_MT_POSITION_X, or _Y when the touch panel is turned like the display).
    pub axis: u16,
    slot: i32,
    finger: Option<i32>,
    x: Option<i32>,
    down: bool,
    sent_down: bool,
}

const EV_ABS: u16 = 3;
const EV_SYN: u16 = 0;
const ABS_MT_SLOT: u16 = 0x2f;
/// ABS_MT_POSITION_X and _Y.
pub const AXIS_X: u16 = 0x35;
pub const AXIS_Y: u16 = 0x36;
const ABS_MT_TRACKING_ID: u16 = 0x39;

impl RawTouch {
    pub fn new(x_min: i32, x_max: i32, bar_w: u32, mirror: bool, axis: u16) -> RawTouch {
        RawTouch { x_min, x_max, bar_w, mirror, axis, slot: 0, finger: None, x: None, down: false, sent_down: false }
    }

    fn bar_x(&self) -> i32 {
        let span = (self.x_max - self.x_min).max(1) as f32;
        let f = ((self.x.unwrap_or(self.x_min) - self.x_min) as f32 / span).clamp_safe(0.0, 1.0);
        let f = if self.mirror { 1.0 - f } else { f };
        (f * self.bar_w.saturating_sub(1) as f32).round() as i32
    }

    /// Feed one event (type, code, value).
    pub fn feed(&mut self, typ: u16, code: u16, value: i32) -> Vec<TouchEv> {
        let mut out = Vec::new();
        match (typ, code) {
            (EV_ABS, ABS_MT_SLOT) => self.slot = value,
            (EV_ABS, ABS_MT_TRACKING_ID) if self.slot == 0 => {
                if value < 0 {
                    if self.sent_down {
                        out.push(TouchEv::Up(self.bar_x()));
                    }
                    self.down = false;
                    self.sent_down = false;
                    self.finger = None;
                } else {
                    self.finger = Some(value);
                    self.down = true;
                }
            }
            (EV_ABS, c) if c == self.axis && self.slot == 0 => self.x = Some(value),
            (EV_SYN, 0) => {
                if self.down && self.x.is_some() {
                    out.push(if self.sent_down { TouchEv::Move(self.bar_x()) } else { TouchEv::Down(self.bar_x()) });
                    self.sent_down = true;
                }
            }
            _ => {}
        }
        out
    }
}

// ------------------------------------------------------------------------------------------------ drawing

/// A finished picture: `w` x `h`, 4 bytes a pixel (RGBA, opaque).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Picture {
    pub w: u32,
    pub h: u32,
    pub rgba: Vec<u8>,
}

impl Picture {
    pub fn black(w: u32, h: u32) -> Picture {
        let mut rgba = vec![0u8; w as usize * h as usize * 4];
        for p in rgba.chunks_exact_mut(4) {
            p[3] = 255;
        }
        Picture { w, h, rgba }
    }

    pub fn pixel(&self, x: u32, y: u32) -> [u8; 3] {
        let at = (y as usize * self.w as usize + x as usize) * 4;
        match self.rgba.get(at..at + 3) {
            Some(p) => [p[0], p[1], p[2]],
            None => [0, 0, 0],
        }
    }

    /// The smallest rectangle holding every pixel that differs from `old` (all of it when the sizes differ or there is no old one).
    pub fn damage(&self, old: Option<&Picture>) -> Option<iyakki_drivers::touchbar::Rect> {
        use iyakki_drivers::touchbar::Rect;
        let Some(o) = old.filter(|o| o.w == self.w && o.h == self.h) else {
            return Some(Rect { x: 0, y: 0, w: self.w, h: self.h });
        };
        let (mut x0, mut y0, mut x1, mut y1) = (u32::MAX, u32::MAX, 0u32, 0u32);
        for y in 0..self.h {
            let row = y as usize * self.w as usize * 4;
            let (a, b) = (self.rgba.get(row..row + self.w as usize * 4), o.rgba.get(row..row + self.w as usize * 4));
            let (Some(a), Some(b)) = (a, b) else { continue };
            if a == b {
                continue;
            }
            let first = a.chunks_exact(4).zip(b.chunks_exact(4)).position(|(p, q)| p != q).unwrap_or(0) as u32;
            let last = a.chunks_exact(4).zip(b.chunks_exact(4)).rposition(|(p, q)| p != q).unwrap_or(0) as u32;
            x0 = x0.min(first);
            x1 = x1.max(last + 1);
            y0 = y0.min(y);
            y1 = y1.max(y + 1);
        }
        (x1 > x0 && y1 > y0).then(|| Rect { x: x0, y: y0, w: x1 - x0, h: y1 - y0 })
    }
}

fn tone_color(t: Tone) -> [u8; 4] {
    match t {
        Tone::Normal => TEXT,
        Tone::Good => OK,
        Tone::Warn => rgb(255, 196, 94),
        Tone::Bad => ERR,
    }
}

fn draw_glyph(g: &mut Gfx, glyph: Glyph, cx: f32, cy: f32, d: f32, c: [u8; 4]) {
    let r = d / 2.0;
    match glyph {
        Glyph::None => {}
        Glyph::App(i) => g.icon(i, cx, cy, d, c),
        Glyph::Play => g.polygon(&[(cx - r * 0.6, cy - r * 0.8), (cx - r * 0.6, cy + r * 0.8), (cx + r * 0.8, cy)], c),
        Glyph::Pause => {
            g.rect(cx - r * 0.65, cy - r * 0.75, r * 0.45, r * 1.5, c);
            g.rect(cx + r * 0.2, cy - r * 0.75, r * 0.45, r * 1.5, c);
        }
        Glyph::Next => {
            g.polygon(&[(cx - r * 0.8, cy - r * 0.7), (cx - r * 0.8, cy + r * 0.7), (cx + r * 0.4, cy)], c);
            g.rect(cx + r * 0.5, cy - r * 0.7, r * 0.3, r * 1.4, c);
        }
        Glyph::Prev => {
            g.polygon(&[(cx + r * 0.8, cy - r * 0.7), (cx + r * 0.8, cy + r * 0.7), (cx - r * 0.4, cy)], c);
            g.rect(cx - r * 0.8, cy - r * 0.7, r * 0.3, r * 1.4, c);
        }
        Glyph::Stop => g.rect(cx - r * 0.65, cy - r * 0.65, r * 1.3, r * 1.3, c),
        Glyph::Speaker | Glyph::Muted => {
            g.polygon(&[(cx - r * 0.9, cy - r * 0.35), (cx - r * 0.4, cy - r * 0.35), (cx + r * 0.1, cy - r * 0.8), (cx + r * 0.1, cy + r * 0.8), (cx - r * 0.4, cy + r * 0.35), (cx - r * 0.9, cy + r * 0.35)], c);
            if glyph == Glyph::Muted {
                g.line(cx + r * 0.35, cy - r * 0.5, cx + r * 0.95, cy + r * 0.5, 2.0, ERR);
                g.line(cx + r * 0.35, cy + r * 0.5, cx + r * 0.95, cy - r * 0.5, 2.0, ERR);
            } else {
                g.ring(cx + r * 0.1, cy, r * 0.65, 2.0, c);
            }
        }
        Glyph::Sun | Glyph::Light => {
            g.circle(cx, cy, r * 0.38, c);
            for k in 0..8 {
                let a = k as f32 * std::f32::consts::FRAC_PI_4;
                let (s, co) = a.sin_cos();
                g.line(cx + co * r * 0.62, cy + s * r * 0.62, cx + co * r * 0.95, cy + s * r * 0.95, 2.0, c);
            }
        }
        Glyph::Check => {
            g.line(cx - r * 0.7, cy, cx - r * 0.2, cy + r * 0.55, 3.0, c);
            g.line(cx - r * 0.2, cy + r * 0.55, cx + r * 0.8, cy - r * 0.6, 3.0, c);
        }
        Glyph::Cross => {
            g.line(cx - r * 0.6, cy - r * 0.6, cx + r * 0.6, cy + r * 0.6, 3.0, c);
            g.line(cx - r * 0.6, cy + r * 0.6, cx + r * 0.6, cy - r * 0.6, 3.0, c);
        }
        Glyph::Plus => {
            g.line(cx - r * 0.7, cy, cx + r * 0.7, cy, 3.0, c);
            g.line(cx, cy - r * 0.7, cx, cy + r * 0.7, 3.0, c);
        }
        Glyph::Back => {
            g.line(cx + r * 0.4, cy - r * 0.7, cx - r * 0.4, cy, 3.0, c);
            g.line(cx - r * 0.4, cy, cx + r * 0.4, cy + r * 0.7, 3.0, c);
        }
        Glyph::Forward => {
            g.line(cx - r * 0.4, cy - r * 0.7, cx + r * 0.4, cy, 3.0, c);
            g.line(cx + r * 0.4, cy, cx - r * 0.4, cy + r * 0.7, 3.0, c);
        }
        Glyph::Reload => g.ring(cx, cy, r * 0.65, 2.5, c),
        Glyph::Run => g.polygon(&[(cx - r * 0.55, cy - r * 0.8), (cx - r * 0.55, cy + r * 0.8), (cx + r * 0.85, cy)], c),
        Glyph::Save => {
            g.rrect_outline(cx - r * 0.7, cy - r * 0.7, r * 1.4, r * 1.4, 2.0, 2.0, c);
            g.rect(cx - r * 0.35, cy - r * 0.7, r * 0.7, r * 0.45, c);
        }
    }
}

fn draw_slot(g: &mut Gfx, slot: &Slot, pressed: bool) {
    let r = slot.rect;
    let (x, y, w, h) = (r.x as f32, r.y as f32, r.w as f32, r.h as f32);
    let i = &slot.item;
    let radius = h * 0.22;
    let fill = match (i.kind, pressed, i.on) {
        (Kind::Label, _, _) => BG,
        (_, true, _) => CARD_SEL,
        (_, _, true) => rgb(36, 56, 104),
        _ => CARD,
    };
    if i.kind != Kind::Label {
        g.rrect(x, y, w, h, radius, fill);
    }
    let text_c = if i.enabled || i.kind == Kind::Label { TEXT } else { DIM };
    let size = h * 0.40;
    let cy = y + h / 2.0;
    match i.kind {
        Kind::Slider => {
            draw_glyph(g, i.glyph, x + h * 0.55, cy, h * 0.5, text_c);
            let (x0, x1) = slider_track(&r);
            let th = (h * 0.14).max(3.0);
            g.rrect(x0, cy - th / 2.0, x1 - x0, th, th / 2.0, rgb(70, 78, 108));
            let xf = x0 + (x1 - x0) * i.value;
            if xf > x0 {
                g.rrect(x0, cy - th / 2.0, xf - x0, th, th / 2.0, if i.enabled { ACCENT } else { DIM });
            }
            g.circle(xf, cy, h * 0.17, text_c);
        }
        Kind::Tile => {
            draw_glyph(g, i.glyph, x + h * 0.55, cy, h * 0.5, text_c);
            let label = g.fit(size, &i.label, Face::Bold, (w - h * 1.0 - 8.0).max(8.0));
            let ty = cy - g.line_height(size, Face::Bold) / 2.0;
            g.text(x + h * 0.95, ty, size, text_c, &label, Face::Bold);
        }
        Kind::Chip => {
            g.circle(x + h * 0.45, cy, h * 0.12, tone_color(i.tone));
            let label = g.fit(size, &i.label, Face::Bold, (w - h * 0.8).max(8.0));
            let ty = cy - g.line_height(size, Face::Bold) / 2.0;
            g.text(x + h * 0.75, ty, size, text_c, &label, Face::Bold);
        }
        Kind::Label => {
            let label = g.fit(size, &i.label, Face::Sans, w.max(8.0));
            let tw = g.text_width(size, &label, Face::Sans);
            let ty = cy - g.line_height(size, Face::Sans) / 2.0;
            g.text(x + (w - tw).max(0.0) / 2.0, ty, size, MUTED, &label, Face::Sans);
        }
        Kind::Button => {
            if i.id == "mode" {
                // the chip: the strip's name and a row of dots, the current one lit
                let label = g.fit(size * 0.9, &i.label, Face::Bold, (w - 8.0).max(8.0));
                let tw = g.text_width(size * 0.9, &label, Face::Bold);
                let ty = y + h * 0.14;
                g.text(x + (w - tw).max(0.0) / 2.0, ty, size * 0.9, text_c, &label, Face::Bold);
                let dots = 4;
                let step = h * 0.22;
                let x0 = x + w / 2.0 - step * (dots as f32 - 1.0) / 2.0;
                let lit = (i.value * 4.0).round() as i32;
                for d in 0..dots {
                    let c = if d == lit { ACCENT } else { rgb(90, 98, 128) };
                    g.circle(x0 + d as f32 * step, y + h * 0.80, h * 0.05, c);
                }
            } else {
                let has_glyph = i.glyph != Glyph::None;
                let label = g.fit(size, &i.label, Face::Bold, (w - if has_glyph { h * 0.9 } else { 12.0 }).max(8.0));
                let tw = g.text_width(size, &label, Face::Bold);
                let total = tw + if has_glyph { h * 0.55 } else { 0.0 };
                let mut px = x + (w - total).max(0.0) / 2.0;
                if has_glyph {
                    draw_glyph(g, i.glyph, px + h * 0.2, cy, h * 0.42, text_c);
                    px += h * 0.55;
                }
                let ty = cy - g.line_height(size, Face::Bold) / 2.0;
                g.text(px, ty, size, text_c, &label, Face::Bold);
            }
        }
    }
}

/// Draw the bar for the slots (and the sign-in state).
pub fn render(slots: &[Slot], pressed: Option<usize>, p: Power, w: u32, h: u32) -> Picture {
    let mut g = Gfx::new(w, h);
    g.clear(BG);
    match p {
        Power::Dark => return Picture::black(w, h),
        Power::Locked => {
            let size = h as f32 * 0.42;
            let tw = g.text_width(size, "IYAKKI", Face::Bold);
            let ty = (h as f32 - g.line_height(size, Face::Bold)) / 2.0;
            g.text(((w as f32 - tw) / 2.0).max(0.0), ty, size, MUTED, "IYAKKI", Face::Bold);
        }
        Power::On => {
            for (k, s) in slots.iter().enumerate() {
                draw_slot(&mut g, s, pressed == Some(k));
            }
        }
    }
    let rgba = g.rgba().to_vec();
    Picture { w, h, rgba }
}

// ------------------------------------------------------------------------------------------------ the controller

/// Redraw pacing: a picture only when what it shows changed, and never faster than [`MIN_GAP_MS`].
#[derive(Clone, Debug, Default)]
pub struct Pacer {
    last_ms: Option<u64>,
    shown: Option<u64>,
}

impl Pacer {
    /// True when a picture should be made now for the content fingerprint `fp`. A true answer counts as drawn.
    pub fn due(&mut self, now_ms: u64, fp: u64) -> bool {
        if self.shown == Some(fp) {
            return false;
        }
        if self.last_ms.is_some_and(|t| now_ms.saturating_sub(t) < MIN_GAP_MS) {
            return false;
        }
        self.last_ms = Some(now_ms);
        self.shown = Some(fp);
        true
    }
    /// Forget what was shown (the bar was woken or the device came back): the next call draws.
    pub fn reset(&mut self) {
        self.shown = None;
    }
}

fn hash<T: std::hash::Hash>(t: &T) -> u64 {
    use std::hash::Hasher;
    let mut h = std::collections::hash_map::DefaultHasher::new();
    t.hash(&mut h);
    h.finish()
}

pub struct Bar {
    pub w: u32,
    pub h: u32,
    pub strips: Vec<Strip>,
    chosen: Strip,
    pub registry: Registry,
    pacer: Pacer,
    touch: Touch,
    slots: Vec<Slot>,
    power: Power,
}

impl Bar {
    pub fn new(w: u32, h: u32, strips: &[&str]) -> Bar {
        let mut s: Vec<Strip> = strips.iter().filter_map(|x| Strip::from_id(x)).collect();
        if s.is_empty() {
            s = Strip::ALL.to_vec();
        }
        Bar { w: w.max(64), h: h.max(8), chosen: s.first().copied().unwrap_or(Strip::System), strips: s, registry: Registry::standard(), pacer: Pacer::default(), touch: Touch::default(), slots: Vec::new(), power: Power::On }
    }

    pub fn chosen(&self) -> Strip {
        self.chosen
    }

    /// Choose a strip (ignored when it is not one of the enabled ones).
    pub fn set_strip(&mut self, s: Strip) {
        if self.strips.contains(&s) {
            self.chosen = s;
            self.pacer.reset();
        }
    }

    /// The next enabled strip.
    pub fn cycle(&mut self) -> Strip {
        let at = self.strips.iter().position(|s| *s == self.chosen).unwrap_or(0);
        self.chosen = self.strips.get((at + 1) % self.strips.len().max(1)).copied().unwrap_or(Strip::System);
        self.pacer.reset();
        self.chosen
    }

    pub fn slots(&self) -> &[Slot] {
        &self.slots
    }

    pub fn power(&self) -> Power {
        self.power
    }

    /// The device came back or the bar woke: draw again whatever it shows.
    pub fn invalidate(&mut self) {
        self.pacer.reset();
    }

    /// Look at the shell's state; a new picture when one is due.
    pub fn update(&mut self, c: &Ctx, now_ms: u64) -> Option<Picture> {
        let shown = effective_strip(self.chosen, c, &self.registry);
        let items = build(shown, &self.strips, c, &self.registry);
        let slots = layout(&items, self.w, self.h);
        let p = power(c);
        let pressed = self.touch.pressed();
        let fp = hash(&(format!("{slots:?}"), p as u8, pressed));
        self.slots = slots;
        self.power = p;
        if !self.pacer.due(now_ms, fp) {
            return None;
        }
        Some(render(&self.slots, pressed, p, self.w, self.h))
    }

    /// A touch: the actions to carry out. A touch while the bar is dark or locked only wakes it (the shell resets the idle time).
    pub fn touch(&mut self, ev: TouchEv) -> Vec<Action> {
        if self.power != Power::On {
            return Vec::new();
        }
        let acts = self.touch.feed(ev, &self.slots);
        self.pacer.reset();
        let mut out = Vec::new();
        for a in acts {
            match a {
                Action::Cycle => {
                    self.cycle();
                }
                other => out.push(other),
            }
        }
        out
    }
}

// ------------------------------------------------------------------------------------------------ remembered per account

#[derive(Clone, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct Choice {
    /// The strip last shown.
    pub strip: String,
    /// App ids, newest first.
    pub recents: Vec<String>,
}

impl Choice {
    fn path(dir: &std::path::Path, user: &str) -> std::path::PathBuf {
        let safe: String = user.chars().filter(|c| c.is_ascii_alphanumeric() || *c == '-' || *c == '_').collect();
        dir.join(format!("touchbar-{safe}.json"))
    }

    pub fn load(dir: &std::path::Path, user: &str) -> Choice {
        std::fs::read_to_string(Self::path(dir, user)).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default()
    }

    pub fn save(&self, dir: &std::path::Path, user: &str) {
        if std::fs::create_dir_all(dir).is_ok() {
            if let Ok(t) = serde_json::to_string(self) {
                let _ = std::fs::write(Self::path(dir, user), t);
            }
        }
    }

    /// Put an app first (once; the list is [`MAX_RECENT`] long). True when the list changed.
    pub fn touch_app(&mut self, id: &str) -> bool {
        if self.recents.first().is_some_and(|f| f == id) {
            return false;
        }
        self.recents.retain(|r| r != id);
        self.recents.insert(0, id.to_string());
        self.recents.truncate(MAX_RECENT);
        true
    }
}

/// A Touch Bar input device by its name (the kernel's: "Apple Inc. Touch Bar Display ..."): its touches belong to the bar, never to the
/// pointer of the main screen.
pub fn is_touchbar_name(name: &str) -> bool {
    let n = name.to_ascii_lowercase();
    n.contains("touch bar") || n.contains("touchbar") || n.contains("appletb")
}

/// What the strip is doing, for Settings (the shell writes it, the page reads it).
static STATUS: std::sync::Mutex<String> = std::sync::Mutex::new(String::new());

pub fn set_status(s: &str) {
    if let Ok(mut g) = STATUS.lock() {
        *g = s.to_string();
    }
}

pub fn status_text() -> String {
    STATUS.lock().map(|g| g.clone()).unwrap_or_default()
}
