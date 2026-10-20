//! The AI IDE's Browser tab: a web page inside the IDE, for testing the app you just deployed (docs/AI-IDE.md, slice B1 of
//! docs/EMBEDDING-AND-BACKEND.md).
//!
//! It is an embedded [`Component`](super::component::Component): the IDE gives it a rectangle, it draws a compact toolbar (back,
//! forward, reload, address, Reader/Full, open in the BROWSER tile) and the page below it.
//!
//! * **Reader** is the built-in reader of the BROWSER tile ([`BrowserApp`], drawn without its own toolbar); no helper program.
//! * **Full** shows the web engine (WebKitGTK in a window session, WPE into a picture buffer otherwise: whichever the BROWSER tile
//!   uses) as the person's one engine of this window: the assistant's tab ([`web_agent`]). The assistant's `browser_*` tools and the
//!   person look at the same page, so the person watches the assistant click, the banner says "Assistant is controlling this tab"
//!   with Stop and Take over, and the person's own click or key on the page takes over. A second engine is never started; when a page
//!   needs one, the reader hands it to this shared one.
//! * The person's own navigation is not an assistant action (no question); the assistant's `ide_browser_open` and `browser_*` tools
//!   keep their consent rules ([`iyakki_agent::idebrowser`]).
//! * Plain `http://` is accepted for the workspace's own cluster addresses and localhost: a NodePort or a dev server has no
//!   certificate, and the person deployed it. The reader shows a "Not secure" note for it like anywhere else.
//!
//! Click ids are small numbers here; the IDE moves them into the range of [`route::BROWSER`] (see [`super::component::route`]).

use iyakki_agent::browserjobs::Op;
use serde_json::json;

use super::browser::BrowserApp;
use super::component::{route, Component, ConsentTier, EmbedMode, EmbedParams, HostEvent, Params, View};
use super::{web_agent, App, AppCx, Flow};
use crate::fx::{self, Glyph};
use crate::gfx::{Face, Gfx};
use crate::ui::{Event, Key, TextField, DIM, ERR, MUTED, OK, R, TEXT};

// ids inside the part (the IDE adds BROWSER * STRIDE)
pub const T_BACK: usize = 50_001;
pub const T_FORWARD: usize = 50_002;
pub const T_RELOAD: usize = 50_003;
pub const T_MODE: usize = 50_004;
pub const T_EXTERNAL: usize = 50_005;
pub const T_NOTES: usize = 50_006;
pub const T_CHIP: usize = 51_000;
pub const FOCUS_ADDR: usize = 49_000;
/// The reader's page area and the engine's (the same numbers the BROWSER tile uses).
const AREA_READER: usize = 901;

/// An address offered as a button: a deployed service, a dev server, the last page.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Chip {
    pub label: String,
    pub url: String,
}

/// Where everything goes inside the rectangle the IDE gave (pure, so the tests and the drawing agree).
#[derive(Clone, Debug, PartialEq)]
pub struct Parts {
    pub back: R,
    pub forward: R,
    pub reload: R,
    pub addr: R,
    pub mode: R,
    pub notes: Option<R>,
    pub external: R,
    pub toolbar: R,
    pub chips: Option<R>,
    pub body: R,
}

pub fn parts(rect: R, s: f32, mode_w: f32, notes_w: Option<f32>, chips: bool) -> Parts {
    let h = super::ide_chrome::tool(s);
    let pad = 4.0 * s;
    let gap = 3.0 * s;
    let toolbar = R::new(rect.x, rect.y, rect.w, h + 2.0 * pad);
    let y = rect.y + pad;
    let b = (h * 1.25).max(26.0 * s);
    let mut x = rect.x + pad;
    let back = R::new(x, y, b, h);
    x += b + gap;
    let forward = R::new(x, y, b, h);
    x += b + gap;
    let reload = R::new(x, y, b, h);
    x += b + gap;
    let mut right = rect.x + rect.w - pad;
    let external = R::new(right - b, y, b, h);
    right -= b + gap;
    // the address needs room: the notes button goes first, then the mode button shrinks to its picture
    let min_addr = 110.0 * s;
    let mut mode_w = mode_w;
    let mut notes_w = notes_w;
    if right - x - mode_w - notes_w.map_or(0.0, |w| w + gap) - gap < min_addr {
        notes_w = None;
    }
    if right - x - mode_w - gap < min_addr {
        mode_w = b;
    }
    let mode = R::new(right - mode_w, y, mode_w, h);
    right -= mode_w + gap;
    let notes = notes_w.map(|w| {
        let r = R::new(right - w, y, w, h);
        right -= w + gap;
        r
    });
    let addr = R::new(x, y, (right - x).max(20.0), h);
    let chips_r = chips.then(|| R::new(rect.x, toolbar.y + toolbar.h, rect.w, h + 4.0 * s));
    let top = chips_r.map_or(toolbar.y + toolbar.h, |c| c.y + c.h);
    let body = R::new(rect.x, top, rect.w, (rect.y + rect.h - top).max(0.0));
    Parts { back, forward, reload, addr, mode, notes, external, toolbar, chips: chips_r, body }
}

pub struct IdeBrowser {
    reader: BrowserApp,
    mode: EmbedMode,
    params: EmbedParams,
    addr: TextField,
    addr_focus: bool,
    page_focus: bool,
    events: Vec<HostEvent>,
    last: Option<(String, String, bool)>,
    user: String,
    ws: String,
    chips: Vec<Chip>,
    note: Option<(String, bool)>,
    focused: bool,
    closed: bool,
    /// What the engine was last asked to show (so Reload after a crash knows where to go).
    full_url: String,
    /// Where the page area was drawn last (tests and the host's pointer logic).
    pub page_rect: R,
}

impl IdeBrowser {
    pub fn new(params: EmbedParams, ws: &str, cx: &AppCx) -> IdeBrowser {
        let mut b = IdeBrowser {
            reader: BrowserApp::new_embedded(cx),
            mode: params.mode,
            addr: TextField::new("Address of the app to test, for example http://192.168.1.69:30929/", false),
            addr_focus: false,
            page_focus: false,
            events: Vec::new(),
            last: None,
            user: cx.session.user.clone(),
            ws: ws.to_string(),
            chips: Vec::new(),
            note: None,
            focused: false,
            closed: false,
            full_url: String::new(),
            page_rect: R::new(0.0, 0.0, 0.0, 0.0),
            params: params.clone(),
        };
        b.addr.max = 2048;
        if b.mode == EmbedMode::Full && !b.engine_ok() {
            b.mode = EmbedMode::Reader;
            b.note = Some(("The web engine is not installed here, so pages are shown by the built-in reader.".into(), false));
        }
        match params.url.as_deref().map(str::trim).filter(|u| !u.is_empty()) {
            Some(u) => b.navigate(u, cx),
            None => b.addr_focus = true,
        }
        b
    }

    fn engine_ok(&self) -> bool {
        web_agent::engine_available(&self.user)
    }

    pub fn mode(&self) -> EmbedMode {
        self.mode
    }

    pub fn readonly(&self) -> bool {
        self.params.readonly
    }

    pub fn tier(&self) -> ConsentTier {
        self.params.start_consent_tier
    }

    pub fn url(&self) -> String {
        match self.mode {
            EmbedMode::Reader => self.reader.current_url().map(|u| u.to_string()).unwrap_or_default(),
            EmbedMode::Full => web_agent::page_view(&self.user).map(|v| v.0).filter(|u| !u.is_empty() && !u.starts_with("about:")).unwrap_or_else(|| self.full_url.clone()),
        }
    }

    #[cfg(test)]
    pub fn focus_address_for_test(&mut self, on: bool) {
        self.addr_focus = on;
        self.page_focus = !on;
    }

    /// What the address field holds.
    pub fn address_text(&self) -> &str {
        &self.addr.value
    }

    pub fn address_focused(&self) -> bool {
        self.addr_focus
    }

    pub fn set_chips(&mut self, chips: Vec<Chip>) {
        self.chips = chips;
    }

    pub fn chips(&self) -> &[Chip] {
        &self.chips
    }

    pub fn note(&self) -> Option<&str> {
        self.note.as_ref().map(|n| n.0.as_str())
    }

    /// Show an address the person asked for (a chip, K8S "Open in browser", Ctrl+L): the person's own navigation, no question.
    pub fn navigate(&mut self, raw: &str, cx: &AppCx) {
        let url = match iyakki_agent::idebrowser::normalize(&self.user, &self.ws, raw) {
            Ok(u) => u,
            Err(e) => {
                self.note = Some((e, false));
                return;
            }
        };
        let host = iyakki_sec::url_host(&url).unwrap_or_default();
        if !cx.session.policy.allows_host(&host) {
            self.note = Some((format!("Your account may not open pages from {host}."), false));
            return;
        }
        self.note = None;
        self.addr.set(&url);
        self.addr_focus = false;
        match self.mode {
            EmbedMode::Reader => self.reader.open_url(&url, cx),
            EmbedMode::Full => self.engine_go(&url, cx),
        }
    }

    fn engine_go(&mut self, url: &str, cx: &AppCx) {
        // the person is acting on the page the assistant may be using: that is a take-over
        if cx.services.browser.control_view(&self.user).active {
            cx.services.browser.take_over(&self.user);
        }
        self.full_url = url.to_string();
        cx.audit.record(&self.user, "ide.browser.open", &iyakki_sec::url_host(url).unwrap_or_default());
        cx.services.browser.jobs.submit_detached(&self.user, Op::Navigate(url.to_string()));
    }

    /// The assistant opened `url` in the shared tab: show the engine, whatever the mode was.
    pub fn show_assistant_page(&mut self, url: &str) {
        if self.engine_ok() {
            self.mode = EmbedMode::Full;
            self.params.mode = EmbedMode::Full;
        }
        self.addr.set(url);
        self.addr_focus = false;
        self.full_url = url.to_string();
        self.note = None;
    }

    pub fn set_mode(&mut self, m: EmbedMode, cx: &AppCx) {
        if m == self.mode {
            return;
        }
        let here = self.url();
        if m == EmbedMode::Full && !self.engine_ok() {
            self.note = Some((format!("{}.", iyakki_agent::webengine::NOT_INSTALLED), false));
            return;
        }
        self.mode = m;
        self.params.mode = m;
        self.note = None;
        if !here.is_empty() {
            self.navigate(&here, cx);
        }
    }

    fn back(&mut self, cx: &AppCx) {
        match self.mode {
            EmbedMode::Reader => self.reader.embed_go_back(),
            EmbedMode::Full => self.engine_op(Op::Back, cx),
        }
    }

    fn forward(&mut self, cx: &AppCx) {
        match self.mode {
            EmbedMode::Reader => self.reader.embed_go_forward(),
            EmbedMode::Full => self.engine_op(Op::Forward, cx),
        }
    }

    pub fn reload(&mut self, cx: &AppCx) {
        match self.mode {
            EmbedMode::Reader => self.reader.embed_reload(cx),
            EmbedMode::Full => {
                if web_agent::page_view(&self.user).is_none() && !self.full_url.is_empty() {
                    // the engine is gone (crashed or never started): ask for a fresh one on the same page
                    let u = self.full_url.clone();
                    self.engine_go(&u, cx);
                } else {
                    self.engine_op(Op::Reload, cx);
                }
            }
        }
    }

    fn engine_op(&mut self, op: Op, cx: &AppCx) {
        if cx.services.browser.control_view(&self.user).active {
            cx.services.browser.take_over(&self.user);
        }
        cx.services.browser.jobs.submit_detached(&self.user, op);
    }

    fn can_back_forward(&self) -> (bool, bool) {
        match self.mode {
            EmbedMode::Reader => (self.reader.embed_can_back(), self.reader.embed_can_forward()),
            EmbedMode::Full => web_agent::page_view(&self.user).map_or((false, false), |v| (v.3, v.4)),
        }
    }

    /// The engine died or could not start, and the person is told so (None: fine).
    pub fn crashed(&self) -> Option<String> {
        if self.mode == EmbedMode::Full && web_agent::page_view(&self.user).is_none() {
            return web_agent::problem(&self.user);
        }
        None
    }

    /// Something new to draw (a frame of the engine, a banner that changes).
    pub fn wants_redraw(&mut self, cx: &AppCx) -> bool {
        match self.mode {
            EmbedMode::Reader => self.reader.embed_is_loading(),
            EmbedMode::Full => {
                let ctl = cx.services.browser.control_view(&self.user);
                web_agent::take_redraw(&self.user) | ctl.active | web_agent::page_view(&self.user).is_some_and(|v| v.2)
            }
        }
    }

    fn sync_events(&mut self) {
        let (url, title, loading) = match self.mode {
            EmbedMode::Reader => (self.reader.current_url().map(|u| u.to_string()).unwrap_or_default(), self.reader.embed_title(), self.reader.embed_is_loading()),
            EmbedMode::Full => match web_agent::page_view(&self.user) {
                Some((u, t, l, ..)) => (u, t, l),
                None => (self.full_url.clone(), String::new(), false),
            },
        };
        let now = Some((url.clone(), title.clone(), loading));
        if self.last == now {
            return;
        }
        let was = self.last.clone().unwrap_or_default();
        if was.0 != url || was.1 != title {
            if !url.is_empty() && !url.starts_with("about:") {
                if !self.addr_focus {
                    self.addr.set(&url);
                }
                self.events.push(HostEvent::Navigated { url, title });
            }
        }
        if was.2 != loading {
            self.events.push(HostEvent::Loading(loading));
        }
        self.last = now;
    }

    fn page_event_allowed(&self) -> bool {
        !self.params.readonly
    }

    /// The person acts on the page: it becomes the keyboard owner, the address field lets go, and the assistant pauses.
    fn take_page(&mut self, cx: &AppCx) {
        self.addr_focus = false;
        self.page_focus = true;
        if self.mode == EmbedMode::Full && cx.services.browser.control_view(&self.user).active {
            cx.services.browser.take_over(&self.user);
        }
    }

    fn key(&mut self, k: &Key, cx: &AppCx) -> bool {
        if matches!(k, Key::Ctrl('l')) {
            self.addr_focus = true;
            self.addr.select_all();
            return true;
        }
        if self.addr_focus {
            match k {
                Key::Enter => {
                    let t = self.addr.value.trim().to_string();
                    if !t.is_empty() {
                        self.navigate(&t, cx);
                    }
                }
                Key::Esc => self.addr_focus = false,
                other => {
                    self.addr.handle(other);
                }
            }
            return true;
        }
        if matches!(k, Key::Esc) {
            return false;
        }
        if !self.page_event_allowed() {
            return true;
        }
        self.take_page(cx);
        match self.mode {
            EmbedMode::Full => web_agent::handle(&Event::Key(k.clone()), &cx.services, &self.user),
            EmbedMode::Reader => {
                self.reader.embed_take_keyboard();
                !matches!(self.reader.handle(&Event::Key(k.clone()), cx), Flow::Back)
            }
        }
    }

    /// The person's mouse moved or let go over the page (these are not click regions, so the host passes them on).
    pub fn pointer(&mut self, ev: &Event, cx: &AppCx) {
        if self.mode == EmbedMode::Full && self.page_event_allowed() {
            web_agent::handle(ev, &cx.services, &self.user);
        }
    }

    pub fn paste(&mut self, text: &str, cx: &AppCx) -> bool {
        if self.addr_focus {
            self.addr.paste(text);
            return true;
        }
        if !self.page_event_allowed() {
            return true;
        }
        match self.mode {
            EmbedMode::Full => web_agent::handle(&Event::Paste(text.to_string()), &cx.services, &self.user),
            EmbedMode::Reader => {
                self.reader.embed_take_keyboard();
                self.reader.handle(&Event::Paste(text.to_string()), cx);
                true
            }
        }
    }

    pub fn scroll(&mut self, d: i32, cx: &AppCx) {
        match self.mode {
            EmbedMode::Full => {
                web_agent::handle(&Event::Scroll(d), &cx.services, &self.user);
            }
            EmbedMode::Reader => {
                self.reader.handle(&Event::Scroll(d), cx);
            }
        }
    }
}

impl Component for IdeBrowser {
    fn kind(&self) -> &'static str {
        "browser"
    }

    fn title(&self) -> String {
        let t = match self.mode {
            EmbedMode::Reader => self.reader.embed_title(),
            EmbedMode::Full => web_agent::page_view(&self.user).map(|v| v.1).unwrap_or_default(),
        };
        if t.is_empty() { "Browser".into() } else { t }
    }

    fn params(&self) -> Params {
        let mut p = self.params.clone();
        let u = self.url();
        p.url = if u.is_empty() { None } else { Some(u) };
        p.mode = self.mode;
        p.to_json()
    }

    fn set_params(&mut self, p: &Params, cx: &AppCx) -> Result<(), String> {
        let n = EmbedParams::from_json(p)?;
        self.params.readonly = n.readonly;
        self.params.start_consent_tier = n.start_consent_tier;
        self.set_mode(n.mode, cx);
        if let Some(u) = n.url.as_deref().map(str::trim).filter(|u| !u.is_empty()) {
            self.navigate(u, cx);
        }
        Ok(())
    }

    fn render(&mut self, g: &mut Gfx, rect: R, view: &View, cx: &AppCx) {
        let s = view.scale;
        let m = super::ide_chrome::cur();
        let bounds = R::new(0.0, 0.0, g.width() as f32, g.height() as f32);
        let sz = m.font_small * s + 1.0 * s;
        let mode_label = if self.mode == EmbedMode::Full { "Full" } else { "Reader" };
        let mode_w = g.text_width(sz, mode_label, Face::Bold) + 2.0 * 8.0 * s + 18.0 * s;
        let notes_n = if self.mode == EmbedMode::Reader { self.reader.embed_notes() } else { 0 };
        let notes_label = format!("Notes {notes_n}");
        let notes_w = (notes_n > 0).then(|| g.text_width(sz, &notes_label, Face::Bold) + 16.0 * s);
        let p = parts(rect, s, mode_w, notes_w, !self.chips.is_empty());
        self.page_rect = p.body;
        // toolbar
        g.rect(p.toolbar.x, p.toolbar.y, p.toolbar.w, p.toolbar.h, fx::PANEL);
        let (can_back, can_fwd) = self.can_back_forward();
        let btn = |g: &mut Gfx, r: R, glyph: &str, on: bool, ev: usize, hit: &str| {
            let col = if on { TEXT } else { DIM };
            super::ide_chrome::icon_btn(g, r.x, r.y, s, glyph, col, false, if on { Event::Tab(ev) } else { Event::Tick }, hit, bounds);
        };
        btn(g, p.back, "←", can_back, T_BACK, "Back");
        btn(g, p.forward, "→", can_fwd, T_FORWARD, "Forward");
        btn(g, p.reload, "⟳", true, T_RELOAD, "Reload");
        // the address field is the only text input of the part: it shows a caret only while it owns the keyboard
        let typing = view.focused && self.addr_focus;
        super::with_caret(typing, || self.addr.render(g, p.addr, self.addr_focus, s, (view.tick / 15) % 2 == 0, Face::Sans, FOCUS_ADDR));
        if let Some(r) = p.notes {
            super::ide_chrome::text_btn(g, r.x, r.y, s, &notes_label, MUTED, true, Event::Tab(T_NOTES), "Notes of the page", Some("What the reader skipped or could not run"), bounds);
        }
        {
            let hot = g.hovered(p.mode);
            if hot {
                g.rrect(p.mode.x, p.mode.y, p.mode.w, p.mode.h, 4.0 * s, crate::gfx::rgb(34, 44, 78));
            }
            let narrow = p.mode.w < mode_w - 1.0;
            let icon = super::ide_chrome::icon_size(s);
            fx::glyph(g, Glyph::Globe, p.mode.x + 8.0 * s + icon / 2.0, p.mode.y + p.mode.h / 2.0, icon, if self.mode == EmbedMode::Full { OK } else { MUTED });
            if !narrow {
                g.text(p.mode.x + 8.0 * s + icon + 5.0 * s, p.mode.y + (p.mode.h - g.line_height(sz, Face::Bold)) / 2.0, sz, TEXT, mode_label, Face::Bold);
            }
            g.hit(p.mode, Event::Tab(T_MODE), if self.mode == EmbedMode::Full { "Full engine (click for Reader)" } else { "Reader (click for the Full engine)" });
        }
        super::ide_chrome::icon_btn(g, p.external.x, p.external.y, s, "↗", MUTED, false, Event::Tab(T_EXTERNAL), "Open in the Browser tile", bounds);
        if self.reader.embed_is_loading() || web_agent::page_view(&self.user).is_some_and(|v| self.mode == EmbedMode::Full && v.2) {
            let w = p.toolbar.w * (0.25 + 0.5 * ((view.tick % 40) as f32 / 40.0));
            g.rect(p.toolbar.x, p.toolbar.y + p.toolbar.h - 2.0 * s, w, 2.0 * s, super::ide::ACCENT);
        }
        // the app addresses: deployed services, dev servers, the last page
        if let Some(c) = p.chips {
            let mut x = c.x + 6.0 * s;
            let cs = s * 0.85;
            let y = c.y + 2.0 * s;
            let lab = "Open app";
            g.text(x, y + (c.h - 4.0 * s - g.line_height(sz, Face::Sans)) / 2.0, sz, super::ide_chrome::dim(), lab, Face::Sans);
            x += g.text_width(sz, lab, Face::Sans) + 8.0 * s;
            for (i, ch) in self.chips.iter().enumerate() {
                let text = g.fit(11.0 * cs, &ch.label, Face::Bold, (c.w * 0.4).max(80.0 * s));
                let w = g.text_width(fx::size::CAPTION * cs, &text, Face::Bold) + fx::size::CAPTION * cs * 1.4;
                if x + w > c.x + c.w - 4.0 * s {
                    break;
                }
                let cur = !self.url().is_empty() && ch.url == self.url();
                let used = fx::chip(g, x, y + 1.0 * s, cs, &text, super::ide::ACCENT, cur);
                g.hit(R::new(x, y, used, c.h - 2.0 * s), Event::Tab(T_CHIP + i), &format!("Open {}", ch.url));
                x += used + 5.0 * s;
            }
        }
        // the page
        let body = p.body;
        if body.w < 1.0 || body.h < 1.0 {
            return;
        }
        match self.mode {
            EmbedMode::Reader => {
                g.rect(body.x, body.y, body.w, body.h, crate::gfx::rgb(18, 22, 40));
                let reader = &mut self.reader;
                g.clipped(body, |g| reader.render_body(g, body, s, view.tick));
                if let Some((n, ok)) = self.note.clone().or_else(|| self.reader.embed_status()) {
                    note_bar(g, body, s, &n, ok);
                }
            }
            EmbedMode::Full => {
                if let Some(why) = self.crashed() {
                    self.crash_notice(g, body, s, &why);
                } else {
                    web_agent::render_with(g, body, s, &cx.services, &self.user, None);
                    if let Some((n, ok)) = &self.note {
                        note_bar(g, body, s, n, *ok);
                    }
                }
            }
        }
    }

    fn handle(&mut self, ev: &Event, cx: &AppCx) -> bool {
        match ev {
            Event::Key(k) => return self.key(k, cx),
            Event::Paste(t) => return self.paste(t, cx),
            Event::Scroll(d) => {
                self.scroll(*d, cx);
                return true;
            }
            Event::MouseMove(..) | Event::MouseUp(..) => {
                self.pointer(ev, cx);
                return true;
            }
            Event::Tab(T_BACK) => self.back(cx),
            Event::Tab(T_FORWARD) => self.forward(cx),
            Event::Tab(T_RELOAD) => self.reload(cx),
            Event::Tab(T_MODE) => {
                let m = if self.mode == EmbedMode::Full { EmbedMode::Reader } else { EmbedMode::Full };
                self.set_mode(m, cx);
            }
            Event::Tab(T_EXTERNAL) => {
                let mut u = self.url();
                if u.is_empty() {
                    u = self.addr.value.trim().to_string();
                }
                if !u.is_empty() {
                    self.events.push(HostEvent::OpenInTab(u));
                }
            }
            Event::Tab(T_NOTES) => self.reader.embed_toggle_notes(),
            Event::Tab(n) if (T_CHIP..T_CHIP + 100).contains(n) => {
                if let Some(c) = self.chips.get(n - T_CHIP).cloned() {
                    self.navigate(&c.url, cx);
                }
            }
            Event::Focus(FOCUS_ADDR) | Event::FocusAt(FOCUS_ADDR, _) => {
                self.addr_focus = true;
                self.page_focus = false;
                if let Event::FocusAt(_, x) = ev {
                    self.addr.click(*x);
                }
            }
            Event::AreaAt(web_agent::AREA_AGENT, ..) => {
                if !self.page_event_allowed() {
                    return true;
                }
                self.take_page(cx);
                web_agent::handle(ev, &cx.services, &self.user);
            }
            // the engine's banner buttons: Stop, Take over, Resume
            Event::Tab(web_agent::TAB_STOP | web_agent::TAB_TAKE_OVER | web_agent::TAB_RESUME) => {
                web_agent::handle(ev, &cx.services, &self.user);
            }
            Event::AreaAt(AREA_READER, ..) | Event::Row(_) | Event::FocusAt(..) | Event::Tab(_) if self.mode == EmbedMode::Reader => {
                if !self.page_event_allowed() {
                    return true;
                }
                self.take_page(cx);
                self.reader.handle(ev, cx);
            }
            _ => return false,
        }
        true
    }

    fn tick(&mut self, cx: &AppCx) {
        if self.closed {
            return;
        }
        self.reader.tick_embedded(cx);
        // a page the reader cannot show goes to the shared engine (the auto-switch of the BROWSER tile, without a second engine)
        if let Some(t) = self.reader.embed_take_want_full() {
            if self.engine_ok() {
                self.mode = EmbedMode::Full;
                self.params.mode = EmbedMode::Full;
                self.note = Some(("This page needs the web engine: shown there.".into(), true));
                self.navigate(&t, cx);
            }
        }
        // the assistant started acting while the reader showed: show the page it is working on
        if self.mode == EmbedMode::Reader && cx.services.browser.control_view(&self.user).active && self.engine_ok() {
            self.mode = EmbedMode::Full;
            self.params.mode = EmbedMode::Full;
        }
        self.sync_events();
    }

    fn drain_events(&mut self) -> Vec<HostEvent> {
        std::mem::take(&mut self.events)
    }

    fn focus(&mut self, on: bool) {
        self.focused = on;
        if !on {
            self.page_focus = false;
            self.addr_focus = false;
        }
    }

    fn wants_text(&self) -> bool {
        self.addr_focus
    }

    fn close(&mut self, _cx: &AppCx) {
        if !self.closed {
            self.closed = true;
            self.events.push(HostEvent::Closed);
        }
    }
}

impl IdeBrowser {
    fn crash_notice(&mut self, g: &mut Gfx, body: R, s: f32, why: &str) {
        g.rect(body.x, body.y, body.w, body.h, crate::gfx::rgb(18, 22, 40));
        let sz = 16.0 * s;
        let head = "The page crashed";
        let hw = g.text_width(sz, head, Face::Bold);
        let y = body.y + body.h / 2.0 - 40.0 * s;
        g.text(body.x + (body.w - hw) / 2.0, y, sz, ERR, head, Face::Bold);
        let t = g.fit(13.0 * s, why, Face::Sans, body.w - 40.0 * s);
        let tw = g.text_width(13.0 * s, &t, Face::Sans);
        g.text(body.x + (body.w - tw) / 2.0, y + 26.0 * s, 13.0 * s, DIM, &t, Face::Sans);
        let bw = g.text_width(14.0 * s, "Reload", Face::Bold) + 14.0 * s * 1.7;
        g.button(body.x + (body.w - bw) / 2.0, y + 54.0 * s, 14.0 * s, "Reload", OK, Event::Tab(T_RELOAD));
    }
}

fn note_bar(g: &mut Gfx, body: R, s: f32, text: &str, ok: bool) {
    let h = 22.0 * s;
    let r = R::new(body.x, body.y + body.h - h, body.w, h);
    g.rect(r.x, r.y, r.w, r.h, crate::gfx::rgb(14, 17, 32));
    let t = g.fit(12.0 * s, text, Face::Sans, r.w - 16.0 * s);
    g.text(r.x + 8.0 * s, r.y + (r.h - g.line_height(12.0 * s, Face::Sans)) / 2.0, 12.0 * s, if ok { MUTED } else { ERR }, &t, Face::Sans);
}

/// The slot code of this part in the IDE.
pub const CODE: usize = route::BROWSER;

/// `{url}` for the assistant's `ide_browser_open` / restoring the last page.
pub fn params_for(url: &str) -> Params {
    json!({"url": url, "mode": "full"})
}

/// The address saved with the workspace: query and fragment are dropped (they may carry tokens), a trailing `?` too.
pub fn persistable(url: &str) -> Option<String> {
    let u = url.split(['?', '#']).next().unwrap_or("").trim();
    if !(u.starts_with("http://") || u.starts_with("https://")) || u.contains('@') || u.len() > 512 {
        return None;
    }
    Some(u.to_string())
}

// ------------------------------------------------------------------------------------------------ addresses to offer

/// Ports where dev servers usually listen.
pub const DEV_PORTS: [u16; 9] = [3000, 3001, 4000, 4200, 5000, 5173, 8000, 8080, 8888];

/// Looks for dev servers on this machine in the background (a short connect to each usual port, now and then, only while the
/// Browser tab is showing). Nothing is requested from them: a connection is opened and closed.
#[derive(Default)]
pub struct DevScan {
    found: std::sync::Arc<std::sync::Mutex<Vec<u16>>>,
    busy: std::sync::Arc<std::sync::atomic::AtomicBool>,
    last: Option<std::time::Instant>,
}

impl DevScan {
    pub fn poll(&mut self) {
        // unit tests never touch the real machine's ports
        if cfg!(test) || self.busy.load(std::sync::atomic::Ordering::SeqCst) || self.last.is_some_and(|t| t.elapsed() < std::time::Duration::from_secs(8)) {
            return;
        }
        self.last = Some(std::time::Instant::now());
        self.busy.store(true, std::sync::atomic::Ordering::SeqCst);
        let (found, busy) = (self.found.clone(), self.busy.clone());
        std::thread::spawn(move || {
            let open: Vec<u16> = DEV_PORTS
                .iter()
                .copied()
                .filter(|p| std::net::TcpStream::connect_timeout(&std::net::SocketAddr::from(([127, 0, 0, 1], *p)), std::time::Duration::from_millis(120)).is_ok())
                .collect();
            *found.lock().unwrap_or_else(|e| e.into_inner()) = open;
            busy.store(false, std::sync::atomic::Ordering::SeqCst);
        });
    }

    pub fn urls(&self) -> Vec<String> {
        self.found.lock().unwrap_or_else(|e| e.into_inner()).iter().map(|p| format!("http://localhost:{p}/")).collect()
    }

    #[cfg(test)]
    pub fn set_for_test(&mut self, ports: &[u16]) {
        *self.found.lock().unwrap() = ports.to_vec();
    }
}

/// The buttons of the "Open app" row: the cluster's services, dev servers on this machine, then the last page. At most eight,
/// each address once.
pub fn chips_for(last: Option<&str>, services: &[(String, String)], dev: &[String]) -> Vec<Chip> {
    let mut out: Vec<Chip> = Vec::new();
    let mut add = |label: String, url: String| {
        if out.len() < 8 && !out.iter().any(|c| c.url.trim_end_matches('/') == url.trim_end_matches('/')) {
            out.push(Chip { label, url });
        }
    };
    for (name, url) in services {
        let host = url.trim_start_matches("http://").trim_start_matches("https://").trim_end_matches('/');
        add(format!("{name} · {host}"), url.clone());
    }
    for u in dev {
        add(u.trim_start_matches("http://").trim_end_matches('/').to_string(), u.clone());
    }
    if let Some(l) = last {
        add(format!("last: {}", l.trim_start_matches("http://").trim_start_matches("https://").trim_end_matches('/')), l.to_string());
    }
    out
}

// ------------------------------------------------------------------------------------------------ the IDE's side

use super::ide::{IdeApp, MainTab, Ws};
use super::ide_layout::Pane;

/// The addresses of the workspace's own cluster: its nodes and the API server's host.
fn own_hosts(w: &Ws) -> Vec<String> {
    let mut v: Vec<String> = Vec::new();
    if let Some(snap) = w.k8s.as_ref().and_then(|k| k.snapshot()) {
        v.extend(snap.nodes.iter().filter(|n| !n.ip.is_empty()).map(|n| n.ip.clone()));
        v.extend(iyakki_agent::idebrowser::hosts_of(&iyakki_agent::idebrowser::reachable_apps(&iyakki_agent::k8s::reachable_lines(&snap)).into_iter().map(|a| a.1).collect::<Vec<_>>()));
    }
    if let Some(p) = w.chosen_profile() {
        if let Some(h) = iyakki_sec::url_host(&p.server) {
            v.push(h);
        }
    }
    v.sort();
    v.dedup();
    v
}

/// What the cluster's Services say can be opened from outside.
pub fn service_urls(w: &Ws) -> Vec<(String, String)> {
    w.k8s.as_ref().and_then(|k| k.snapshot()).map(|snap| iyakki_agent::idebrowser::reachable_apps(&iyakki_agent::k8s::reachable_lines(&snap))).unwrap_or_default()
}

impl IdeApp {
    fn browser_allowed(cx: &AppCx) -> bool {
        cx.session.policy.apps.contains(&iyakki_sec::policy::App::Browser)
    }

    /// Make the workspace's browser if it is not there yet (restoring the page of last time).
    pub(super) fn ensure_browser(w: &mut Ws, cx: &AppCx) {
        if w.browser.is_some() || !Self::browser_allowed(cx) {
            return;
        }
        let last = (!w.cfg.preview.url.is_empty()).then(|| w.cfg.preview.url.clone());
        // several tiles of the stand-alone browser may exist elsewhere; this one is the window's only embedded one
        let p = EmbedParams { url: last, mode: EmbedMode::Full, readonly: false, start_consent_tier: ConsentTier::ReadOnly };
        w.browser = Some(IdeBrowser::new(p, &w.meta.name, cx));
    }

    /// Show the Browser tab, on `url` when given. The keyboard goes to the page's address field only when it is empty.
    pub fn open_browser(&mut self, url: Option<&str>, cx: &AppCx) {
        let Some(i) = self.active else { return };
        if !Self::browser_allowed(cx) {
            self.notice = Some(("Your account may not use the browser.".into(), false));
            return;
        }
        Self::ensure_browser(&mut self.ws[i], cx);
        self.show_main(MainTab::Browser);
        if let (Some(u), Some(b)) = (url, self.ws[i].browser.as_mut()) {
            b.navigate(u, cx);
        }
        self.redraw = true;
    }

    /// An event of the Browser part (already moved back into its own ids).
    pub(super) fn browser_route(&mut self, inner: Event, cx: &AppCx) {
        let Some(i) = self.active else { return };
        let w = &mut self.ws[i];
        w.set_focus(Pane::Main);
        w.compact_pane = Pane::Main;
        w.main_tab = MainTab::Browser;
        if let Some(b) = w.browser.as_mut() {
            b.handle(&inner, cx);
        }
        self.redraw = true;
    }

    /// Once per heartbeat: the assistant's requests, the trusted addresses, the page's events, the address buttons.
    pub(super) fn browser_tick(&mut self, cx: &AppCx) {
        let user = cx.session.user.clone();
        let active = self.active;
        let mut open_tab: Option<String> = None;
        for (i, w) in self.ws.iter_mut().enumerate() {
            let name = w.meta.name.clone();
            iyakki_agent::idebrowser::set_trusted(&user, &name, own_hosts(w));
            for r in iyakki_agent::idebrowser::take_requests(&user, &name) {
                Self::ensure_browser(w, cx);
                if let Some(b) = w.browser.as_mut() {
                    b.show_assistant_page(&r.url);
                    // the page shows; the keyboard stays where the person is typing (the editor's text must not become the page's)
                    if w.main_tab != MainTab::Browser {
                        w.main_tab = MainTab::Browser;
                        if w.focus == Pane::Main {
                            w.set_focus(Pane::Chat);
                        }
                    }
                    self.redraw = true;
                }
            }
            let showing = active == Some(i) && w.main_tab == MainTab::Browser;
            if showing {
                w.dev_ports.poll();
            }
            let services = service_urls(w);
            let dev = w.dev_ports.urls();
            let last = (!w.cfg.preview.url.is_empty()).then(|| w.cfg.preview.url.clone());
            let ro = w.ro.is_some();
            let mut persist: Option<String> = None;
            if let Some(b) = w.browser.as_mut() {
                b.tick(cx);
                let chips = chips_for(last.as_deref(), &services, &dev);
                if b.chips() != chips.as_slice() {
                    b.set_chips(chips);
                    self.redraw = true;
                }
                for e in b.drain_events() {
                    match e {
                        HostEvent::Navigated { url, .. } => persist = persistable(&url),
                        HostEvent::OpenInTab(u) => open_tab = Some(u),
                        HostEvent::Loading(_) | HostEvent::Closed => {}
                    }
                    self.redraw = true;
                }
                if showing && b.wants_redraw(cx) {
                    self.redraw = true;
                }
            }
            if let Some(u) = persist.filter(|u| *u != w.cfg.preview.url) {
                w.cfg.preview.url = u;
                if !ro {
                    let _ = w.cfg.save(&w.root());
                }
            }
        }
        if let Some(u) = open_tab {
            super::browser::request_open(&u);
            self.request = Some(("browser".into(), u));
        }
    }
}
