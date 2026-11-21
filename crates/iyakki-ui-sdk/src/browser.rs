//! `BrowserView`: the IYAKKI Browser as a component an SDK app places in its layout, like a web view widget (F-SDK-60, UI-SDK.md
//! "Embedding the browser"). The app gives it a rectangle and an address (or page content); the view shows the page, takes the wheel, keys
//! and link taps, and reports the title, the address and the loading state as events.
//!
//! The view does no network and no drawing of pages itself (the core rule). It is handed a [`BrowserPort`], which is the core's: the core
//! judges every address (known-bad sites, the person's policy), loads the page, and answers with a *display list* that this view replays
//! inside its rectangle, so the shell, a client and a WASM tile show the same page. A download or a permission question the page raises
//! comes back as a prompt the person answers (a bar under the page), never the page itself.

use std::collections::VecDeque;

use crate::canvas::{fit, Canvas, Hit, Role, TextStyle};
use crate::cx::UiCx;
use crate::event::{UiEvent, UiKey};
use crate::geom::Rect;
use crate::list::DisplayList;
use crate::size::TypeRole;
use crate::tokens::{Radius, Space};
use crate::widgets;

/// The room of action numbers a view uses from its base: page links first, the view's own buttons from [`OWN_ACTIONS`].
pub const ACTION_SPAN: u32 = 10_000;
const OWN_ACTIONS: u32 = 9_000;
const A_ALLOW: u32 = OWN_ACTIONS + 1;
const A_DENY: u32 = OWN_ACTIONS + 2;
const A_RETRY: u32 = OWN_ACTIONS + 3;

/// Which engine draws the page (the core decides what is installed and allowed).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Engine {
    #[default]
    InHouse,
    Full,
    Auto,
}

/// How the view takes space from the rectangle the app gives it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum SizePolicy {
    /// All of it.
    Fill,
    /// This size (cut to the rectangle).
    Fixed(f32, f32),
    /// The full width and this height.
    Height(f32),
    /// The full width and a height of width / ratio (16:9 is 1.777).
    Aspect(f32),
}

impl SizePolicy {
    pub fn used(self, area: Rect) -> Rect {
        let (w, h) = match self {
            SizePolicy::Fill => (area.w, area.h),
            SizePolicy::Fixed(w, h) => (w.min(area.w), h.min(area.h)),
            SizePolicy::Height(h) => (area.w, h.min(area.h)),
            SizePolicy::Aspect(r) => (area.w, (area.w / r.max(0.1)).min(area.h)),
        };
        Rect::new(area.x, area.y, w.max(0.0), h.max(0.0))
    }
}

/// The colours the core draws the page chrome in (taken from the Look at open).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PagePalette {
    pub bg: [u8; 4],
    pub text: [u8; 4],
    pub muted: [u8; 4],
    pub link: [u8; 4],
    pub rule: [u8; 4],
    pub code_bg: [u8; 4],
    pub mark: [u8; 4],
}

/// The scheme the page is asked to follow.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum PageScheme {
    #[default]
    Dark,
    Light,
    HighContrast,
}

/// The page's default font family.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum PageFamily {
    #[default]
    Sans,
    Mono,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ViewSpec {
    pub scheme: PageScheme,
    /// The default text size (before `text_scale`) and family handed to the engine as the page's defaults.
    pub font_size: f32,
    pub family: PageFamily,
    pub reduced_motion: bool,
    pub width: f32,
    pub height: f32,
    pub text_scale: f32,
    pub engine: Engine,
    pub palette: PagePalette,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Nav {
    Url(String),
    Html { base: String, html: String },
    Back,
    Forward,
    Reload,
    Stop,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PageKey {
    Up,
    Down,
    PageUp,
    PageDown,
    Home,
    End,
    Enter,
    Esc,
}

#[derive(Clone, Debug, PartialEq)]
pub enum PageInput {
    Activate(u32),
    Wheel(i32),
    Key(PageKey),
    Text(String),
    Resize { width: f32, height: f32 },
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct FindInfo {
    pub query: String,
    pub matches: usize,
    pub current: usize,
}

#[derive(Clone, Debug, PartialEq)]
pub struct PageInfo {
    pub url: String,
    pub title: String,
    pub loading: bool,
    pub can_back: bool,
    pub can_forward: bool,
    pub error: Option<String>,
    pub engine: Engine,
    pub find: FindInfo,
    pub generation: u64,
    pub scroll_permille: u16,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PromptKind {
    Download,
    Permission,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PagePrompt {
    pub id: u64,
    pub kind: PromptKind,
    pub origin: String,
    pub text: String,
}

/// What the view asks of the core. Errors are plain sentences the view may show. The shell's implementation is over the core's browser
/// sessions (`iyakki-apps`, `sdk_browser.rs`); [`MemBrowser`] is the in-memory one for tests and the browser preview.
pub trait BrowserPort {
    fn open(&mut self, spec: &ViewSpec) -> Result<u64, String>;
    fn close(&mut self, id: u64);
    fn navigate(&mut self, id: u64, nav: &Nav) -> Result<(), String>;
    fn input(&mut self, id: u64, input: &PageInput) -> Result<(), String>;
    fn find(&mut self, id: u64, query: &str, forward: bool) -> Result<FindInfo, String>;
    fn state(&mut self, id: u64) -> Result<PageInfo, String>;
    /// The frame (display list text) when its generation is not `since`.
    fn frame(&mut self, id: u64, since: u64) -> Result<Option<(u64, String)>, String>;
    fn prompts(&mut self, id: u64) -> Vec<PagePrompt>;
    fn answer(&mut self, id: u64, prompt: u64, allow: bool) -> Result<(), String>;
}

/// What the app hears from the view (take them with [`BrowserView::take_events`]).
#[derive(Clone, Debug, PartialEq)]
pub enum BrowserEvent {
    Opened,
    Title(String),
    /// The address changed (a link, a redirect, back or forward).
    Url(String),
    Loading(bool),
    /// A load or an address was refused or failed (the plain reason); `None` when it is cleared by the next good load.
    Failed(Option<String>),
    Find(FindInfo),
    /// A download or permission question is waiting for the person.
    Prompt(PagePrompt),
    /// The core ended the session or would not open one.
    Closed(String),
}

pub struct BrowserView {
    port: Box<dyn BrowserPort>,
    id: Option<u64>,
    engine: Engine,
    family: PageFamily,
    policy: SizePolicy,
    base: u32,
    /// The address to load once the session is open (the app may ask before the first draw).
    queued: Option<Nav>,
    unavailable: Option<String>,
    focused: bool,
    state: Option<PageInfo>,
    frame: Option<DisplayList>,
    frame_gen: u64,
    prompts: Vec<PagePrompt>,
    events: VecDeque<BrowserEvent>,
    size: (f32, f32),
    palette: Option<PagePalette>,
    text_scale: f32,
    ticks: u32,
    dirty: bool,
}

const MAX_EVENTS: usize = 64;

impl BrowserView {
    /// A view over the core's port. `base` is the first of [`ACTION_SPAN`] action numbers the view may use in the app's layout.
    pub fn new(port: Box<dyn BrowserPort>, base: u32) -> BrowserView {
        BrowserView {
            port,
            id: None,
            engine: Engine::InHouse,
            family: PageFamily::Sans,
            policy: SizePolicy::Fill,
            base,
            queued: None,
            unavailable: None,
            focused: false,
            state: None,
            frame: None,
            frame_gen: 0,
            prompts: Vec::new(),
            events: VecDeque::new(),
            size: (0.0, 0.0),
            palette: None,
            text_scale: 1.0,
            ticks: 0,
            dirty: false,
        }
    }

    /// The engine to ask for (before the first draw; later it applies at the next open).
    pub fn with_engine(mut self, e: Engine) -> BrowserView {
        self.engine = e;
        self
    }

    /// The page's default font family (sans or mono).
    pub fn with_family(mut self, f: PageFamily) -> BrowserView {
        self.family = f;
        self
    }

    pub fn with_size_policy(mut self, p: SizePolicy) -> BrowserView {
        self.policy = p;
        self
    }

    pub fn set_size_policy(&mut self, p: SizePolicy) {
        self.policy = p;
    }

    /// Whether the wheel and the keys go to the page. The app sets it from its own focus; a tap on a link of the page sets it too.
    pub fn set_focused(&mut self, on: bool) {
        self.focused = on;
    }

    pub fn is_focused(&self) -> bool {
        self.focused
    }

    pub fn owns_action(&self, a: u32) -> bool {
        a >= self.base && a < self.base + ACTION_SPAN
    }

    // ------------------------------------------------------------------------------------------------ what the app asks

    /// Open this address (the core judges it; a refusal comes as `Failed`). A bare host name is fine.
    pub fn load_url(&mut self, url: &str) {
        self.go(Nav::Url(url.to_string()));
    }

    /// Show content the app brings (a help text, a mail body): never fetched, shown under `base` as its address.
    pub fn load_html(&mut self, base: &str, html: &str) {
        self.go(Nav::Html { base: base.to_string(), html: html.to_string() });
    }

    pub fn back(&mut self) {
        self.go(Nav::Back);
    }

    pub fn forward(&mut self) {
        self.go(Nav::Forward);
    }

    pub fn reload(&mut self) {
        self.go(Nav::Reload);
    }

    pub fn stop(&mut self) {
        self.go(Nav::Stop);
    }

    fn go(&mut self, nav: Nav) {
        match self.id {
            Some(id) => {
                let r = self.port.navigate(id, &nav);
                self.after_call(r);
                self.refresh();
            }
            None => self.queued = Some(nav),
        }
    }

    pub fn url(&self) -> &str {
        self.state.as_ref().map_or("", |s| s.url.as_str())
    }

    pub fn title(&self) -> &str {
        self.state.as_ref().map_or("", |s| s.title.as_str())
    }

    pub fn is_loading(&self) -> bool {
        self.state.as_ref().is_some_and(|s| s.loading)
    }

    pub fn can_back(&self) -> bool {
        self.state.as_ref().is_some_and(|s| s.can_back)
    }

    pub fn can_forward(&self) -> bool {
        self.state.as_ref().is_some_and(|s| s.can_forward)
    }

    pub fn error(&self) -> Option<&str> {
        self.unavailable.as_deref().or_else(|| self.state.as_ref().and_then(|s| s.error.as_deref()))
    }

    /// The engine that really draws the page (known once the session is open).
    pub fn engine_used(&self) -> Option<Engine> {
        self.state.as_ref().map(|s| s.engine)
    }

    pub fn find_state(&self) -> FindInfo {
        self.state.as_ref().map(|s| s.find.clone()).unwrap_or_default()
    }

    /// Find in page: a new query goes to the first match, the same query again to the next or previous one; empty clears the marks.
    pub fn find(&mut self, query: &str, forward: bool) -> FindInfo {
        let Some(id) = self.id else { return FindInfo::default() };
        match self.port.find(id, query, forward) {
            Ok(f) => {
                self.refresh();
                f
            }
            Err(e) => {
                self.push(BrowserEvent::Failed(Some(e)));
                FindInfo::default()
            }
        }
    }

    pub fn prompts(&self) -> &[PagePrompt] {
        &self.prompts
    }

    /// Answer the oldest question (what the bar's Allow and Deny buttons do; a chat approval can answer through the core too).
    pub fn answer(&mut self, allow: bool) {
        let (Some(id), Some(p)) = (self.id, self.prompts.first().cloned()) else { return };
        if let Err(e) = self.port.answer(id, p.id, allow) {
            self.push(BrowserEvent::Failed(Some(e)));
        }
        self.refresh();
    }

    pub fn take_events(&mut self) -> Vec<BrowserEvent> {
        self.events.drain(..).collect()
    }

    /// End the session (the view opens a new one at the next draw unless it is dropped).
    pub fn close(&mut self) {
        if let Some(id) = self.id.take() {
            self.port.close(id);
        }
        self.state = None;
        self.frame = None;
        self.prompts.clear();
    }

    // ------------------------------------------------------------------------------------------------ input

    /// Feed one event; true when the view used it (the app then does not).
    pub fn event(&mut self, ev: &UiEvent) -> bool {
        match ev {
            UiEvent::Action(a) if self.owns_action(*a) => {
                let n = *a - self.base;
                match n {
                    A_ALLOW => self.answer(true),
                    A_DENY => self.answer(false),
                    A_RETRY => {
                        self.unavailable = None;
                        self.dirty = true;
                    }
                    n if n < OWN_ACTIONS => {
                        self.focused = true;
                        self.send(PageInput::Activate(n));
                    }
                    _ => {}
                }
                true
            }
            UiEvent::Wheel(d) if self.focused && self.id.is_some() => {
                self.send(PageInput::Wheel(*d));
                true
            }
            UiEvent::Key(k) if self.focused && self.id.is_some() => {
                let pk = match k {
                    UiKey::Up => PageKey::Up,
                    UiKey::Down => PageKey::Down,
                    UiKey::Char(' ') => PageKey::PageDown,
                    _ => return false,
                };
                self.send(PageInput::Key(pk));
                true
            }
            UiEvent::Text(t) if self.focused && self.id.is_some() => {
                self.send(PageInput::Text(t.clone()));
                true
            }
            UiEvent::Tick => {
                self.ticks = self.ticks.wrapping_add(1);
                if self.id.is_some() && (self.is_loading() || self.ticks % 5 == 0) {
                    self.refresh();
                }
                false
            }
            _ => false,
        }
    }

    fn send(&mut self, input: PageInput) {
        if let Some(id) = self.id {
            let r = self.port.input(id, &input);
            self.after_call(r);
            self.refresh();
        }
    }

    fn after_call(&mut self, r: Result<(), String>) {
        if let Err(e) = r {
            self.push(BrowserEvent::Failed(Some(e)));
        }
    }

    fn push(&mut self, e: BrowserEvent) {
        if self.events.len() >= MAX_EVENTS {
            self.events.pop_front();
        }
        self.events.push_back(e);
    }

    // ------------------------------------------------------------------------------------------------ state

    /// Ask the core for the state and a new frame, and say what changed.
    pub fn refresh(&mut self) {
        let Some(id) = self.id else { return };
        match self.port.state(id) {
            Ok(st) => {
                let old = self.state.take();
                let o = old.as_ref();
                if o.is_none_or(|o| o.title != st.title) && !st.title.is_empty() {
                    self.push(BrowserEvent::Title(st.title.clone()));
                }
                if o.is_none_or(|o| o.url != st.url) && !st.url.is_empty() {
                    self.push(BrowserEvent::Url(st.url.clone()));
                }
                if o.is_some_and(|o| o.loading != st.loading) || (o.is_none() && st.loading) {
                    self.push(BrowserEvent::Loading(st.loading));
                }
                if o.map(|o| &o.error) != Some(&st.error) && (o.is_some() || st.error.is_some()) {
                    self.push(BrowserEvent::Failed(st.error.clone()));
                }
                if o.is_some_and(|o| o.find != st.find) {
                    self.push(BrowserEvent::Find(st.find.clone()));
                }
                let changed = o.is_none_or(|o| o.generation != st.generation);
                self.state = Some(st);
                if changed || self.frame.is_none() {
                    match self.port.frame(id, self.frame_gen) {
                        Ok(Some((g, text))) => match DisplayList::parse(&text) {
                            Ok(l) => {
                                self.frame = Some(l);
                                self.frame_gen = g;
                            }
                            Err(e) => self.push(BrowserEvent::Failed(Some(format!("the page could not be drawn: {e}")))),
                        },
                        Ok(None) => {}
                        Err(e) => self.push(BrowserEvent::Failed(Some(e))),
                    }
                }
                let now = self.port.prompts(id);
                for p in &now {
                    if !self.prompts.iter().any(|q| q.id == p.id) {
                        self.push(BrowserEvent::Prompt(p.clone()));
                    }
                }
                self.prompts = now;
            }
            Err(e) => {
                self.id = None;
                self.state = None;
                self.unavailable = Some(e.clone());
                self.push(BrowserEvent::Closed(e));
            }
        }
    }

    fn spec_for(&self, cx: &UiCx, w: f32, h: f32) -> ViewSpec {
        use iyakki_uikit::look::Motion;
        use iyakki_uikit::theme::ThemeId;
        let scheme = match cx.colors().id {
            ThemeId::Dark => PageScheme::Dark,
            ThemeId::Light => PageScheme::Light,
            ThemeId::HighContrast => PageScheme::HighContrast,
        };
        ViewSpec { scheme, font_size: TypeRole::Label.base(), family: self.family, reduced_motion: cx.look().motion == Motion::Reduced, width: w, height: h, text_scale: cx.metrics().text_scale, engine: self.engine, palette: palette_of(cx) }
    }

    fn ensure_open(&mut self, cx: &UiCx, used: Rect) {
        let pal = palette_of(cx);
        let scale = cx.metrics().text_scale;
        if self.id.is_some() && (self.palette != Some(pal) || (self.text_scale - scale).abs() > f32::EPSILON) {
            // the Look or the text size changed: the core drew the page in the old ones, so open again and go back to the address
            let url = self.url().to_string();
            self.close();
            if !url.is_empty() {
                self.queued = Some(Nav::Url(url));
            }
        }
        if self.id.is_none() && self.unavailable.is_none() {
            let spec = self.spec_for(cx, used.w, used.h);
            match self.port.open(&spec) {
                Ok(id) => {
                    self.id = Some(id);
                    self.palette = Some(pal);
                    self.text_scale = scale;
                    self.size = (used.w, used.h);
                    self.frame_gen = 0;
                    self.push(BrowserEvent::Opened);
                    if let Some(n) = self.queued.take() {
                        let r = self.port.navigate(id, &n);
                        self.after_call(r);
                    }
                    self.refresh();
                }
                Err(e) => {
                    self.unavailable = Some(e.clone());
                    self.queued = None;
                    self.push(BrowserEvent::Closed(e));
                }
            }
        } else if let Some(id) = self.id {
            if (self.size.0 - used.w).abs() >= 1.0 || (self.size.1 - used.h).abs() >= 1.0 {
                self.size = (used.w, used.h);
                let r = self.port.input(id, &PageInput::Resize { width: used.w, height: used.h });
                self.after_call(r);
                self.refresh();
            }
        }
        if self.dirty {
            self.dirty = false;
        }
    }

    // ------------------------------------------------------------------------------------------------ drawing

    /// Draw the view inside `area` and return the rectangle it took (by its size policy). The page is the core's frame replayed here; the
    /// bar for a waiting question is drawn under it; when the core would not open a session the view says why, with a Retry button.
    pub fn draw(&mut self, c: &mut dyn Canvas, cx: &UiCx, area: Rect) -> Rect {
        let th = cx.colors();
        let m = cx.metrics();
        let used = self.policy.used(area);
        let bar_h = if self.prompts.is_empty() { 0.0 } else { (m.min_target() + m.gap(Space::Lg)).min(used.h / 2.0) };
        let page = Rect::new(used.x, used.y, used.w, (used.h - bar_h).max(0.0));
        self.ensure_open(cx, page);
        if let Some(why) = self.unavailable.clone() {
            widgets::surface(c, th, &m, used, th.panel, Radius::Md, false);
            let size = m.size(TypeRole::Body);
            let t = fit(c, size, &format!("The browser is not available: {why}"), TextStyle::Regular, (used.w - 2.0 * m.gap(Space::Lg)).max(0.0));
            c.text(used.x + m.gap(Space::Lg), used.y + m.gap(Space::Lg), size, th.muted, &t, TextStyle::Regular);
            widgets::button(c, th, &m, Rect::new(used.x + m.gap(Space::Lg), used.y + m.gap(Space::Lg) + size * 2.0, m.em(7.0).min(used.w - 2.0 * m.gap(Space::Lg)).max(0.0), m.min_target()), "Try again", self.base + A_RETRY, false);
            return used;
        }
        if let Some(l) = &self.frame {
            let moved = l.translated(page.x, page.y, self.base, OWN_ACTIONS);
            c.clipped(page, &mut |cc| moved.replay(cc));
        } else {
            widgets::surface(c, th, &m, page, th.panel, Radius::Md, false);
        }
        if let Some(p) = self.prompts.first() {
            let bar = Rect::new(used.x, page.bottom(), used.w, bar_h);
            widgets::surface(c, th, &m, bar, th.card, Radius::Md, false);
            let (xs, pad) = (m.gap(Space::Xs), m.gap(Space::Md) + m.gap(Space::Xxs));
            let size = m.size(TypeRole::Small);
            let btn_w = m.em(5.5);
            let text_w = (bar.w - 2.0 * btn_w - 2.0 * m.gap(Space::Xl) - 2.0 * m.gap(Space::Md)).max(0.0);
            let label = format!("{}: {}", p.origin, p.text);
            let t = fit(c, size, &label, TextStyle::Regular, text_w);
            c.text(bar.x + pad, bar.y + ((bar.h - size) / 2.0).max(0.0), size, th.text, &t, TextStyle::Regular);
            c.hit(Rect::new(bar.x, bar.y, text_w + pad, bar.h), Hit { action: self.base + OWN_ACTIONS + 9, name: label, role: Role::Text, ..Default::default() });
            widgets::button(c, th, &m, Rect::new(bar.right() - 2.0 * btn_w - m.gap(Space::Xl) + m.gap(Space::Xxs), bar.y + xs, btn_w, bar.h - 2.0 * xs), "Allow", self.base + A_ALLOW, true);
            widgets::button(c, th, &m, Rect::new(bar.right() - btn_w - m.gap(Space::Sm), bar.y + xs, btn_w, bar.h - 2.0 * xs), "Deny", self.base + A_DENY, false);
        }
        if self.focused {
            c.outline(page, m.tokens.radius_for(Radius::Md, page.h), m.tokens.focus_width(), th.accent);
        }
        used
    }
}

fn palette_of(cx: &UiCx) -> PagePalette {
    let th = cx.colors();
    PagePalette { bg: th.panel, text: th.text, muted: th.muted, link: th.accent, rule: th.border, code_bg: th.well, mark: th.amber }
}

// -------------------------------------------------------------------------------------------------------- the in-memory port

/// A page of [`MemBrowser`].
#[derive(Clone, Debug, Default)]
pub struct MemPage {
    pub title: String,
    pub lines: Vec<String>,
    pub links: Vec<(String, String)>,
}

/// An in-memory [`BrowserPort`] for tests and the browser preview: scripted pages, one session at a time per `open`, links that work. It
/// has no known-bad list and no policy (the core's job); it is not a substitute for the core in a test of those.
#[derive(Default)]
pub struct MemBrowser {
    pub pages: std::collections::BTreeMap<String, MemPage>,
    sessions: std::collections::BTreeMap<u64, MemSession>,
    next: u64,
    pub log: Vec<String>,
}

struct MemSession {
    spec: ViewSpec,
    history: Vec<String>,
    pos: usize,
    generation: u64,
    find: FindInfo,
}

impl MemBrowser {
    pub fn with_page(mut self, url: &str, title: &str, lines: &[&str], links: &[(&str, &str)]) -> MemBrowser {
        self.pages.insert(url.into(), MemPage { title: title.into(), lines: lines.iter().map(|s| s.to_string()).collect(), links: links.iter().map(|(a, b)| (a.to_string(), b.to_string())).collect() });
        self
    }

    fn page(&self, s: &MemSession) -> Option<&MemPage> {
        s.history.get(s.pos).and_then(|u| self.pages.get(u))
    }
}

impl BrowserPort for MemBrowser {
    fn open(&mut self, spec: &ViewSpec) -> Result<u64, String> {
        self.next += 1;
        self.log.push("open".into());
        self.sessions.insert(self.next, MemSession { spec: spec.clone(), history: Vec::new(), pos: 0, generation: 1, find: FindInfo::default() });
        Ok(self.next)
    }
    fn close(&mut self, id: u64) {
        self.log.push("close".into());
        self.sessions.remove(&id);
    }
    fn navigate(&mut self, id: u64, nav: &Nav) -> Result<(), String> {
        self.log.push(format!("{nav:?}"));
        let s = self.sessions.get_mut(&id).ok_or("closed")?;
        match nav {
            Nav::Url(u) if u.starts_with("javascript:") => return Err("only http and https are opened".into()),
            Nav::Url(u) => {
                let mut u = if u.contains("://") { u.clone() } else { format!("https://{u}") };
                if u.split_once("://").is_some_and(|(_, r)| !r.contains('/')) {
                    u.push('/');
                }
                s.history.truncate(if s.history.is_empty() { 0 } else { s.pos + 1 });
                s.history.push(u);
                s.pos = s.history.len() - 1;
            }
            Nav::Back if s.pos > 0 => s.pos -= 1,
            Nav::Forward if s.pos + 1 < s.history.len() => s.pos += 1,
            _ => {}
        }
        s.generation += 1;
        s.find = FindInfo::default();
        Ok(())
    }
    fn input(&mut self, id: u64, input: &PageInput) -> Result<(), String> {
        self.log.push(format!("{input:?}"));
        let target = {
            let s = self.sessions.get(&id).ok_or("closed")?;
            match input {
                PageInput::Activate(n) => self.page(s).and_then(|p| p.links.get((*n as usize).wrapping_sub(1))).map(|(_, t)| t.clone()),
                _ => None,
            }
        };
        let s = self.sessions.get_mut(&id).ok_or("closed")?;
        if let PageInput::Resize { width, height } = input {
            s.spec.width = *width;
            s.spec.height = *height;
        }
        match target {
            Some(t) => {
                s.history.truncate(s.pos + 1);
                s.history.push(t);
                s.pos = s.history.len() - 1;
            }
            None => {}
        }
        s.generation += 1;
        Ok(())
    }
    fn find(&mut self, id: u64, query: &str, _forward: bool) -> Result<FindInfo, String> {
        let n = {
            let s = self.sessions.get(&id).ok_or("closed")?;
            self.page(s).map_or(0, |p| if query.is_empty() { 0 } else { p.lines.iter().filter(|l| l.contains(query)).count() })
        };
        let s = self.sessions.get_mut(&id).ok_or("closed")?;
        s.find = FindInfo { query: query.into(), matches: n, current: usize::from(n > 0) };
        s.generation += 1;
        Ok(s.find.clone())
    }
    fn state(&mut self, id: u64) -> Result<PageInfo, String> {
        let s = self.sessions.get(&id).ok_or("the browser view is closed")?;
        let p = self.page(s);
        Ok(PageInfo {
            url: s.history.get(s.pos).cloned().unwrap_or_default(),
            title: p.map(|p| p.title.clone()).unwrap_or_default(),
            loading: false,
            can_back: s.pos > 0,
            can_forward: s.pos + 1 < s.history.len(),
            error: if !s.history.is_empty() && p.is_none() { Some("not found".into()) } else { None },
            engine: s.spec.engine,
            find: s.find.clone(),
            generation: s.generation,
            scroll_permille: 0,
        })
    }
    fn frame(&mut self, id: u64, since: u64) -> Result<Option<(u64, String)>, String> {
        let s = self.sessions.get(&id).ok_or("closed")?;
        if s.generation == since {
            return Ok(None);
        }
        let pal = s.spec.palette;
        let mut c = crate::list::ListCanvas::new(s.spec.width, s.spec.height);
        c.rect(Rect::new(0.0, 0.0, s.spec.width, s.spec.height), pal.bg);
        let mut y = 8.0;
        if let Some(p) = self.page(s) {
            c.text(8.0, y, 18.0, pal.text, &p.title, TextStyle::Bold);
            y += 28.0;
            for l in &p.lines {
                c.text(8.0, y, 14.0, pal.text, l, TextStyle::Regular);
                y += 20.0;
            }
            for (i, (label, _)) in p.links.iter().enumerate() {
                let w = c.text(8.0, y, 14.0, pal.link, label, TextStyle::Regular);
                c.hit(Rect::new(8.0, y, w.max(8.0), 18.0), Hit { action: i as u32 + 1, name: label.clone(), role: Role::Link, ..Default::default() });
                y += 22.0;
            }
        }
        Ok(Some((s.generation, c.finish().to_text())))
    }
    fn prompts(&mut self, _id: u64) -> Vec<PagePrompt> {
        Vec::new()
    }
    fn answer(&mut self, _id: u64, _prompt: u64, _allow: bool) -> Result<(), String> {
        Err("no such question".into())
    }
}
