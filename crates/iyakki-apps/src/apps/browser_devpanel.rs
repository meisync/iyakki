//! The Developer tools panel of the BROWSER tile: what a browser's developer tools show, for the person using the browser (the
//! chat agent has its own tools over the same data, `iyakki_agent::devtools`).
//!
//! * **Opening** (F12, Ctrl+Shift+I, the "Dev tools" button) starts collecting for this person for as long as the panel is open
//!   (`DevTools::set_panel`, memory only; the saved switch "Developer tools for the chat agent" is not touched). Closing it
//!   stops collecting and forgets what was collected, unless that switch is on (then the buffers are the chat agent's too).
//! * **Data.** Console, Network and Timings are drawn from a copy of the shared buffers (`DevTools::snapshot`), refreshed only when
//!   `DevTools::seq` moved. Nothing is parsed or scrubbed here: the lines were scrubbed when they were stored (no query strings,
//!   tokens, bodies, headers or cookies). The built-in reader has no page scripts; its Network tab lists the requests the reader
//!   made itself (addresses scrubbed the same way).
//! * **The inspector (F-BROW-41).** For pages the built-in reader shows, the Console tab has a prompt (Enter or `e`) that evaluates an
//!   expression in the reader's own script engine, and the Elements tab (`4`) shows the page's tree with the chosen node's attributes
//!   and text (`browser_inspect.rs`, the engine is `browser_inspecthost.rs`). The full web engine's protocol has no safe way to run a
//!   script and bring a result back, so for pages it shows there is no prompt (the panel says so) and the Elements tab explains it.
//!   What an expression returns stays on this screen: it is never stored in the buffers and never copied by "Copy as text".
//! * **Layout.** The panel takes about 35 % of the page area, at the bottom or (wide windows) at the right, with a divider the mouse
//!   can drag (`+` and `-` do the same from the keyboard); the page is resized to what is left, so nothing is drawn over anything.

use iyakki_clock as clock;
use iyakki_clock::{Zone};
use iyakki_shared::devtools::{DevEvent, Level, Snapshot, Tagged};
use iyakki_shared::devtools::DevTools;

use super::browser_devtools::NetLine;
use super::browser_inspect::{Inspector, Kind as InKind};
use crate::safeclamp::SafeClamp;
use crate::gfx::{rgb, Face, Gfx};
use crate::ui::{Event, Key, DIM, ERR, MUTED, OK, R, TEXT};

const ACCENT: [u8; 4] = rgb(88, 190, 235);
const WARN: [u8; 4] = rgb(255, 190, 90);
/// The developer panel's ground, themed (see `fx::panel_alpha`): a const cannot read the live theme.
fn panel_bg() -> [u8; 4] {
    crate::fx::panel_alpha(250)
}

/// Buttons (`Event::Tab`). The browser routes this whole range here before its own numbers.
pub const TAB_TOGGLE: usize = 7000;
pub const TAB_CONSOLE: usize = 7001;
pub const TAB_NETWORK: usize = 7002;
pub const TAB_TIMINGS: usize = 7003;
pub const TAB_CLEAR: usize = 7004;
pub const TAB_COPY: usize = 7005;
pub const TAB_CLOSE: usize = 7006;
pub const TAB_LEVEL: usize = 7007;
pub const TAB_FAILED: usize = 7008;
pub const TAB_SORT: usize = 7009;
pub const TAB_FILTER: usize = 7010;
pub const TAB_DOCK: usize = 7011;
pub const TAB_ELEMENTS: usize = 7012;
pub const TAB_PROMPT: usize = 7013;
/// The Report site button: the same as the `!` key (a local report about the page, nothing sent).
pub const TAB_REPORT: usize = 7014;
/// Rows of the Elements page: `TAB_ROW + n` is the nth visible row.
pub const TAB_ROW: usize = 7100;
/// Areas (`Event::AreaAt`): the list, and the divider the mouse drags.
pub const AREA_BODY: usize = 905;
pub const AREA_SPLIT: usize = 906;

pub fn is_tab(n: usize) -> bool {
    (TAB_TOGGLE..=TAB_REPORT).contains(&n) || (TAB_ROW..TAB_ROW + 400).contains(&n)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tab {
    Console,
    Network,
    Timings,
    /// The page's tree and the chosen node (the reader's pages).
    Elements,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Dock {
    Bottom,
    Right,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LevelFilter {
    All,
    Errors,
    Warnings,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Sort {
    Time,
    Duration,
}

/// What the browser has to do after a key or click that the panel took.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Act {
    /// Nothing for the browser: the panel changed (or did not take the key).
    Done,
    Close,
    Copy,
    /// The Report site button.
    Report,
    /// The prompt sent this expression to the reader's engine.
    Eval(String),
    /// The Elements page was opened or asked to refresh: the browser fills in the tree.
    Inspect,
}

/// What the page area shows, as far as the panel cares.
#[derive(Clone, Copy, Debug)]
pub struct Ctx {
    /// The full web engine shows the page (only it reports console messages and timings).
    pub full: bool,
    /// The full web engine can be started at all in this build.
    pub offered: bool,
}

/// Where the page and the panel go inside the browser's page area.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Split {
    pub page: R,
    pub panel: R,
}

pub const DEFAULT_FRAC: f32 = 0.35;

/// Panel on the right when the window is wide (in interface units), else at the bottom.
pub fn auto_dock(view: R, s: f32) -> Dock {
    if view.w / s.max(0.1) >= 1500.0 { Dock::Right } else { Dock::Bottom }
}

pub fn split(view: R, s: f32, dock: Dock, frac: f32) -> Split {
    let frac = frac.clamp(0.2, 0.7);
    match dock {
        Dock::Bottom => {
            let min = (150.0 * s).min(view.h * 0.6);
            let h = (view.h * frac).clamp_safe(min, (view.h * 0.7).max(min)).floor();
            Split { page: R::new(view.x, view.y, view.w, view.h - h), panel: R::new(view.x, view.y + view.h - h, view.w, h) }
        }
        Dock::Right => {
            let min = (320.0 * s).min(view.w * 0.6);
            let w = (view.w * frac).clamp_safe(min, (view.w * 0.7).max(min)).floor();
            Split { page: R::new(view.x, view.y, view.w - w, view.h), panel: R::new(view.x + view.w - w, view.y, w, view.h) }
        }
    }
}

/// One line of a list, ready to draw or to copy.
struct Line {
    /// Arrival time, `HH:MM:SS` (empty when unknown).
    time: String,
    /// The coloured word before the text (`error`, `warn`, `FAIL`, `slow`, `ok`).
    tag: String,
    color: [u8; 4],
    text: String,
    ms: u32,
    seq: u64,
}

impl Line {
    fn plain(&self) -> String {
        let mut o = String::new();
        if !self.time.is_empty() {
            o.push_str(&self.time);
            o.push(' ');
        }
        o.push_str(&format!("{:<5} {}", self.tag, self.text));
        o
    }
}

pub struct Panel {
    pub open: bool,
    /// The panel has the keyboard (opening gives it; a click on the page takes it back).
    pub focus: bool,
    pub tab: Tab,
    pub filter: String,
    pub editing: bool,
    pub level: LevelFilter,
    pub failed_only: bool,
    pub sort: Sort,
    pub frac: f32,
    pub dock: Option<Dock>,
    /// Rows scrolled off the top, per tab.
    scroll: [usize; 4],
    /// The console prompt and the Elements page (`browser_inspect.rs`).
    pub insp: Inspector,
    /// Entries up to this `seq` were cleared by the person (Console, Network): they are hidden here, not removed from the buffers.
    floor: [u64; 2],
    snap: Snapshot,
    seen: u64,
    /// The reader's request count last drawn (it has no sequence number).
    reader_seen: usize,
    zone: Zone,
    /// Where the panel was drawn last, and the page area it shares.
    pub rect: R,
    pub dock_now: Dock,
    total: f32,
    grab: Option<f32>,
}

impl Default for Panel {
    fn default() -> Panel {
        Panel {
            open: false,
            focus: false,
            tab: Tab::Console,
            filter: String::new(),
            editing: false,
            level: LevelFilter::All,
            failed_only: false,
            sort: Sort::Time,
            frac: DEFAULT_FRAC,
            dock: None,
            scroll: [0; 4],
            insp: Inspector::default(),
            floor: [0; 2],
            snap: Snapshot::default(),
            seen: 0,
            reader_seen: 0,
            zone: Zone::default(),
            rect: R::default(),
            dock_now: Dock::Bottom,
            total: 0.0,
            grab: None,
        }
    }
}

fn idx(t: Tab) -> usize {
    match t {
        Tab::Console => 0,
        Tab::Network => 1,
        Tab::Timings => 2,
        Tab::Elements => 3,
    }
}

fn clock_text(z: &Zone, at: i64) -> String {
    if at <= 0 {
        return String::new();
    }
    clock::bar_time(z, at, false, true)
}

fn human(n: u64) -> String {
    if n >= 1 << 20 {
        format!("{:.1} MB", n as f64 / (1u64 << 20) as f64)
    } else if n >= 1024 {
        format!("{:.1} KB", n as f64 / 1024.0)
    } else {
        format!("{n} B")
    }
}

impl Panel {
    /// Opens the panel: collecting starts for this person (memory only) and the panel takes the keyboard.
    pub fn open(&mut self, dt: &DevTools, user: &str) {
        self.open = true;
        self.focus = true;
        self.editing = false;
        self.zone = Zone::system();
        self.seen = u64::MAX;
        dt.set_panel(user, true);
    }

    /// Closes the panel: collecting stops (unless the chat agent's switch is on) and what it collected is forgotten, here too.
    pub fn close(&mut self, dt: &DevTools, user: &str) {
        if self.open {
            dt.set_panel(user, false);
        }
        self.open = false;
        self.focus = false;
        self.editing = false;
        self.snap = Snapshot::default();
        self.seen = 0;
        self.reader_seen = 0;
        self.scroll = [0; 4];
        self.insp = Inspector::default();
        self.floor = [0; 2];
        self.grab = None;
    }

    /// Copies the shared buffers again when they changed since the last call. True when the panel has to be drawn again.
    pub fn refresh(&mut self, dt: &DevTools, user: &str, reader_len: usize) -> bool {
        if !self.open {
            return false;
        }
        let mut changed = false;
        let seq = dt.seq(user);
        if seq != self.seen {
            self.snap = dt.snapshot(user);
            self.seen = seq;
            changed = true;
        }
        if reader_len != self.reader_seen {
            self.reader_seen = reader_len;
            changed = true;
        }
        changed
    }

    pub fn dock_side(&self, view: R, s: f32) -> Dock {
        self.dock.unwrap_or_else(|| auto_dock(view, s))
    }

    // ------------------------------------------------------------------------------------------ rows

    fn matches(&self, hay: &str) -> bool {
        let f = self.filter.trim();
        f.is_empty() || hay.to_ascii_lowercase().contains(&f.to_ascii_lowercase())
    }

    fn console_lines(&self) -> Vec<Line> {
        let mut v = Vec::new();
        for t in self.snap.console.iter().filter(|t| t.seq > self.floor[0]) {
            let DevEvent::Console { level, line, col, source, text } = &t.ev else { continue };
            match (self.level, level) {
                (LevelFilter::Errors, Level::Warn) | (LevelFilter::Warnings, Level::Error) => continue,
                _ => {}
            }
            let at = if source.is_empty() { String::new() } else if *line > 0 { format!("  ({source}:{line}:{col})") } else { format!("  ({source})") };
            let text = format!("{text}{at}");
            if !self.matches(&text) {
                continue;
            }
            let (tag, color) = if *level == Level::Error { ("error", ERR) } else { ("warn", WARN) };
            v.push(Line { time: clock_text(&self.zone, t.at), tag: tag.into(), color, text, ms: 0, seq: t.seq });
        }
        v
    }

    fn net_lines(&self, ctx: Ctx, reader: &[NetLine]) -> Vec<Line> {
        let mut v: Vec<Line> = Vec::new();
        if ctx.full {
            for t in self.snap.failed.iter().chain(self.snap.slow.iter()).filter(|t| t.seq > self.floor[1]) {
                v.push(net_line(&self.zone, t));
            }
        } else {
            for (i, n) in reader.iter().enumerate() {
                let bad = n.status.is_none_or(|s| s >= 400);
                let url = crate::agent().devtools_scrub_url(&n.url);
                let what = match n.status {
                    Some(s) => format!("{s} {} {} ms  {} {}", n.method, n.ms, human(n.bytes as u64), url),
                    None => format!("--- {} {} ms  {}  ({})", n.method, n.ms, url, crate::agent().devtools_scrub_text(&n.error)),
                };
                v.push(Line { time: String::new(), tag: if bad { "FAIL".into() } else { "ok".into() }, color: if bad { ERR } else { MUTED }, text: what, ms: n.ms.min(u32::MAX as u128) as u32, seq: i as u64 + 1 });
            }
        }
        v.retain(|l| (!self.failed_only || l.tag == "FAIL") && self.matches(&l.text));
        match self.sort {
            Sort::Time => v.sort_by_key(|l| std::cmp::Reverse(l.seq)),
            Sort::Duration => v.sort_by(|a, b| b.ms.cmp(&a.ms).then(b.seq.cmp(&a.seq))),
        }
        v
    }

    /// The counts on the tab chips: (console entries, network entries) before filters.
    fn counts(&self, ctx: Ctx, reader: &[NetLine]) -> (usize, usize) {
        let c = self.snap.console.iter().filter(|t| t.seq > self.floor[0]).count();
        let n = if ctx.full { self.snap.failed.iter().chain(self.snap.slow.iter()).filter(|t| t.seq > self.floor[1]).count() } else { reader.len() };
        (c, n)
    }

    fn slowest(&self) -> Vec<(u32, String, String)> {
        let mut v: Vec<(u32, String, String)> = Vec::new();
        for t in &self.snap.slow {
            if let DevEvent::Slow { kind, bytes, ms, url } = &t.ev {
                v.push((*ms, url.clone(), format!("{}{}", if kind.is_empty() { "resource" } else { kind }, if *bytes > 0 { format!(", {}", human(*bytes)) } else { String::new() })));
            }
        }
        for t in &self.snap.failed {
            if let DevEvent::Failed { ms, url, .. } = &t.ev {
                if *ms >= 300 {
                    v.push((*ms, url.clone(), "failed".into()));
                }
            }
        }
        v.sort_by(|a, b| b.0.cmp(&a.0));
        v.dedup_by(|a, b| a.1 == b.1 && a.2 == b.2);
        v.retain(|(_, u, k)| self.matches(&format!("{u} {k}")));
        v
    }

    fn newest_page(&self) -> Option<(u32, u32, u32, String)> {
        self.snap.pages.iter().rev().find_map(|t| if let DevEvent::Page { response_ms, dcl_ms, load_ms, url } = &t.ev { Some((*response_ms, *dcl_ms, *load_ms, url.clone())) } else { None })
    }

    // ------------------------------------------------------------------------------------------ the text report

    /// The scrubbed text of what the current tab shows with its filters (the lines of the list, not only the visible ones).
    pub fn report(&self, ctx: Ctx, reader: &[NetLine]) -> String {
        let mut o = String::new();
        let page = if self.snap.page.is_empty() { "(no page seen yet)".to_string() } else { self.snap.page.clone() };
        let tab = match self.tab {
            Tab::Console => "Console",
            Tab::Network => "Network",
            Tab::Timings => "Timings",
            Tab::Elements => "Elements",
        };
        o.push_str(&format!("IYAKKI browser developer tools - {tab}\nPage: {page}\n"));
        let mut filters = Vec::new();
        if !self.filter.trim().is_empty() {
            filters.push(format!("text \"{}\"", crate::agent().devtools_scrub_text(self.filter.trim())));
        }
        match (self.tab, self.level, self.failed_only) {
            (Tab::Console, LevelFilter::Errors, _) => filters.push("errors only".into()),
            (Tab::Console, LevelFilter::Warnings, _) => filters.push("warnings only".into()),
            (Tab::Network, _, true) => filters.push("failed only".into()),
            _ => {}
        }
        if !filters.is_empty() {
            o.push_str(&format!("Filter: {}\n", filters.join(", ")));
        }
        o.push_str("(addresses have no query strings or tokens; no page content, headers or cookies)\n\n");
        match self.tab {
            Tab::Console => {
                let l = self.console_lines();
                if l.is_empty() {
                    o.push_str("No console errors or warnings.\n");
                }
                for x in l {
                    o.push_str(&x.plain());
                    o.push('\n');
                }
            }
            Tab::Network => {
                let l = self.net_lines(ctx, reader);
                if l.is_empty() {
                    o.push_str("No requests to show.\n");
                }
                for x in l {
                    o.push_str(&x.plain());
                    o.push('\n');
                }
            }
            Tab::Elements => {
                // the tree as the panel shows it; what expressions returned is not copied
                let t = self.insp.tree_text();
                o.push_str(if t.is_empty() { "No page tree is shown.\n" } else { &t });
                o.push('\n');
            }
            Tab::Timings => {
                match self.newest_page() {
                    Some((r, d, l, u)) => o.push_str(&format!("{u}\nfirst response {r} ms, DOMContentLoaded {d} ms, load {l} ms (from navigation start)\n")),
                    None => o.push_str("No page load timing was reported yet.\n"),
                }
                let slow = self.slowest();
                if slow.is_empty() {
                    o.push_str("No resource took 300 ms or more.\n");
                } else {
                    o.push_str("Slowest resources:\n");
                    for (ms, u, k) in slow.iter().take(20) {
                        o.push_str(&format!("{ms} ms  {u}  ({k})\n"));
                    }
                }
            }
        }
        o
    }

    // ------------------------------------------------------------------------------------------ keys and clicks

    /// A key while the panel has the keyboard. `Act::Done` for keys it used or ignored; the browser asks `Act::Close` and `Act::Copy`.
    pub fn key(&mut self, k: &Key) -> Act {
        if self.insp.editing {
            return match self.insp.prompt_key(k) {
                Some(e) => {
                    self.scroll[idx(Tab::Console)] = usize::MAX / 2;
                    self.insp.push(InKind::In, format!("> {e}"));
                    Act::Eval(e)
                }
                None => Act::Done,
            };
        }
        if self.tab == Tab::Elements && matches!(k, Key::Up | Key::Down | Key::PageUp | Key::PageDown | Key::Home | Key::End | Key::Left | Key::Right | Key::Enter) {
            self.insp.tree_key(k);
            return Act::Done;
        }
        if self.tab == Tab::Console && !self.editing && matches!(k, Key::Enter | Key::Char('e')) {
            self.insp.editing = true;
            return Act::Done;
        }
        if self.editing {
            match k {
                Key::Esc | Key::Enter => self.editing = false,
                Key::Backspace => {
                    self.filter.pop();
                    self.reset_scroll();
                }
                Key::Char(c) if !c.is_control() && self.filter.chars().count() < 60 => {
                    self.filter.push(*c);
                    self.reset_scroll();
                }
                _ => {}
            }
            return Act::Done;
        }
        let t = idx(self.tab);
        match k {
            Key::Esc => return Act::Close,
            Key::Tab | Key::Right => return self.tab_to(next(self.tab, true)),
            Key::BackTab | Key::Left => return self.tab_to(next(self.tab, false)),
            Key::Char('1') => self.set_tab(Tab::Console),
            Key::Char('2') => self.set_tab(Tab::Network),
            Key::Char('3') => self.set_tab(Tab::Timings),
            Key::Char('4') => {
                self.set_tab(Tab::Elements);
                return Act::Inspect;
            }
            Key::Char('r') if self.tab == Tab::Elements => return Act::Inspect,
            Key::Char('/') => self.editing = true,
            Key::Char('l') => self.cycle_level(),
            Key::Char('f') => self.toggle_failed(),
            Key::Char('s') => self.toggle_sort(),
            Key::Char('x') | Key::Delete => self.clear_tab(),
            Key::Char('y') => return Act::Copy,
            Key::Char('d') => self.toggle_dock(),
            Key::Char('+' | '=') => self.frac = (self.frac + 0.05).min(0.7),
            Key::Char('-') => self.frac = (self.frac - 0.05).max(0.2),
            Key::Up => self.scroll[t] = self.scroll[t].saturating_sub(1),
            Key::Down => self.scroll[t] += 1,
            Key::PageUp => self.scroll[t] = self.scroll[t].saturating_sub(8),
            Key::PageDown => self.scroll[t] += 8,
            Key::Home => self.scroll[t] = 0,
            Key::End => self.scroll[t] = usize::MAX / 2,
            _ => {}
        }
        Act::Done
    }

    pub fn paste(&mut self, t: &str) -> bool {
        if self.insp.editing {
            self.insp.paste(t);
            return true;
        }
        if !self.editing {
            return false;
        }
        for c in t.chars().filter(|c| !c.is_control()) {
            if self.filter.chars().count() < 60 {
                self.filter.push(c);
            }
        }
        self.reset_scroll();
        true
    }

    /// A button of the panel (`TAB_*`). `TAB_TOGGLE` is the browser's.
    pub fn click(&mut self, n: usize) -> Act {
        self.focus = true;
        match n {
            TAB_CONSOLE => self.set_tab(Tab::Console),
            TAB_NETWORK => self.set_tab(Tab::Network),
            TAB_TIMINGS => self.set_tab(Tab::Timings),
            TAB_ELEMENTS => {
                self.set_tab(Tab::Elements);
                return Act::Inspect;
            }
            TAB_PROMPT => {
                self.set_tab(Tab::Console);
                self.insp.editing = true;
            }
            n if (TAB_ROW..TAB_ROW + 400).contains(&n) => self.insp.pick(self.insp.scroll + (n - TAB_ROW)),
            TAB_CLEAR => self.clear_tab(),
            TAB_COPY => return Act::Copy,
            TAB_REPORT => return Act::Report,
            TAB_CLOSE => return Act::Close,
            TAB_LEVEL => self.cycle_level(),
            TAB_FAILED => self.toggle_failed(),
            TAB_SORT => self.toggle_sort(),
            TAB_FILTER => self.editing = true,
            TAB_DOCK => self.toggle_dock(),
            _ => {}
        }
        Act::Done
    }

    pub fn scroll_by(&mut self, rows: i32) {
        let t = idx(self.tab);
        self.scroll[t] = if rows < 0 { self.scroll[t].saturating_sub(rows.unsigned_abs() as usize) } else { self.scroll[t].saturating_add(rows as usize) };
    }

    fn set_tab(&mut self, t: Tab) {
        self.tab = t;
        self.editing = false;
        self.insp.editing = false;
    }

    /// Tab and Shift+Tab: the next tab; the Elements page also asks the browser for its tree.
    fn tab_to(&mut self, t: Tab) -> Act {
        self.set_tab(t);
        if t == Tab::Elements { Act::Inspect } else { Act::Done }
    }

    fn reset_scroll(&mut self) {
        self.scroll[idx(self.tab)] = 0;
    }

    fn cycle_level(&mut self) {
        if self.tab == Tab::Console {
            self.level = match self.level {
                LevelFilter::All => LevelFilter::Errors,
                LevelFilter::Errors => LevelFilter::Warnings,
                LevelFilter::Warnings => LevelFilter::All,
            };
            self.reset_scroll();
        }
    }

    fn toggle_failed(&mut self) {
        if self.tab == Tab::Network {
            self.failed_only = !self.failed_only;
            self.reset_scroll();
        }
    }

    fn toggle_sort(&mut self) {
        if self.tab == Tab::Network {
            self.sort = if self.sort == Sort::Time { Sort::Duration } else { Sort::Time };
            self.reset_scroll();
        }
    }

    fn toggle_dock(&mut self) {
        self.dock = Some(if self.dock_now == Dock::Bottom { Dock::Right } else { Dock::Bottom });
        self.frac = DEFAULT_FRAC;
    }

    /// Hides what the tab shows now (the shared buffers, and so the chat agent's view, are not touched).
    fn clear_tab(&mut self) {
        let newest = |v: &[Tagged]| v.iter().map(|t| t.seq).max().unwrap_or(0);
        match self.tab {
            Tab::Console => self.floor[0] = self.floor[0].max(newest(&self.snap.console)),
            Tab::Network => {
                let n = newest(&self.snap.failed).max(newest(&self.snap.slow));
                self.floor[1] = self.floor[1].max(n);
            }
            Tab::Timings | Tab::Elements => {}
        }
        if self.tab == Tab::Console {
            self.insp.lines.clear();
        }
        self.reset_scroll();
    }

    /// The mouse on the divider: `x`, `y` relative to it. The first press only remembers where it was grabbed.
    pub fn drag(&mut self, x: f32, y: f32, dragging: bool) {
        let along = if self.dock_now == Dock::Bottom { y } else { x };
        let Some(g) = self.grab.filter(|_| dragging) else {
            self.grab = Some(along);
            self.focus = true;
            return;
        };
        if self.total <= 1.0 {
            return;
        }
        let size = if self.dock_now == Dock::Bottom { self.rect.h } else { self.rect.w };
        self.frac = ((size - (along - g)) / self.total).clamp(0.2, 0.7);
    }

    pub fn drag_end(&mut self) {
        self.grab = None;
    }

    // ------------------------------------------------------------------------------------------ drawing

    /// Draws the panel into `rect` (from [`split`]) and registers its buttons.
    pub fn render(&mut self, g: &mut Gfx, rect: R, total: f32, dock: Dock, s: f32, ctx: Ctx, reader: &[NetLine]) {
        self.rect = rect;
        self.total = total;
        self.dock_now = dock;
        g.rect(rect.x, rect.y, rect.w, rect.h, panel_bg());
        // the divider: a strip at the edge next to the page
        let div = match dock {
            Dock::Bottom => R::new(rect.x, rect.y, rect.w, 5.0 * s),
            Dock::Right => R::new(rect.x, rect.y, 5.0 * s, rect.h),
        };
        g.rect(div.x, div.y, div.w, div.h, if g.hovered(div) || self.grab.is_some() { ACCENT } else { [ACCENT[0], ACCENT[1], ACCENT[2], 120] });
        g.hit(div, Event::Area(AREA_SPLIT), "dev divider");
        let inner = match dock {
            Dock::Bottom => R::new(rect.x, rect.y + 5.0 * s, rect.w, rect.h - 5.0 * s),
            Dock::Right => R::new(rect.x + 5.0 * s, rect.y, rect.w - 5.0 * s, rect.h),
        };
        let pad = 8.0 * s;
        let fs = 12.0 * s;
        let (console_n, net_n) = self.counts(ctx, reader);
        let errors = self.snap.console.iter().filter(|t| t.seq > self.floor[0] && matches!(t.ev, DevEvent::Console { level: Level::Error, .. })).count();

        // row 1: the tabs and Close
        let r1 = R::new(inner.x, inner.y + 2.0 * s, inner.w, 28.0 * s);
        let close_w = chip_width(g, fs, "Close Esc");
        let mut x = inner.x + pad;
        let tabs: [(Tab, usize, String, [u8; 4]); 4] = [
            (Tab::Console, TAB_CONSOLE, if console_n > 0 { format!("Console {console_n}") } else { "Console".into() }, if errors > 0 { ERR } else { ACCENT }),
            (Tab::Network, TAB_NETWORK, if net_n > 0 { format!("Network {net_n}") } else { "Network".into() }, ACCENT),
            (Tab::Timings, TAB_TIMINGS, "Timings".into(), ACCENT),
            (Tab::Elements, TAB_ELEMENTS, "Elements".into(), ACCENT),
        ];
        for (t, id, label, col) in tabs {
            let w = chip_width(g, fs, &label);
            if x + w + 6.0 * s + close_w > r1.x + r1.w - pad {
                break;
            }
            tab_chip(g, R::new(x, r1.y, w, r1.h - 4.0 * s), fs, &label, col, self.tab == t, Event::Tab(id), match t {
                Tab::Console => "Console",
                Tab::Network => "Network",
                Tab::Timings => "Timings",
                Tab::Elements => "Elements",
            });
            x += w + 6.0 * s;
        }
        let cx = r1.x + r1.w - pad - close_w;
        tab_chip(g, R::new(cx, r1.y, close_w, r1.h - 4.0 * s), fs, "Close Esc", MUTED, false, Event::Tab(TAB_CLOSE), "Close dev tools");

        // row 2: toggles, copy, clear, dock, and the filter box with what is left
        let r2 = R::new(inner.x, r1.y + r1.h, inner.w, 26.0 * s);
        let mut x = inner.x + pad;
        let right = inner.x + inner.w - pad;
        let min_filter = 90.0 * s;
        let place = |g: &mut Gfx, x: &mut f32, label: &str, col: [u8; 4], on: bool, id: usize, name: &str| {
            let w = chip_width(g, fs, label);
            if *x + w + min_filter > right {
                return;
            }
            tab_chip(g, R::new(*x, r2.y, w, r2.h - 4.0 * s), fs, label, col, on, Event::Tab(id), name);
            *x += w + 6.0 * s;
        };
        match self.tab {
            Tab::Console => {
                let l = match self.level {
                    LevelFilter::All => "Level: all",
                    LevelFilter::Errors => "Level: errors",
                    LevelFilter::Warnings => "Level: warnings",
                };
                place(g, &mut x, l, ACCENT, self.level != LevelFilter::All, TAB_LEVEL, "Level");
            }
            Tab::Network => {
                place(g, &mut x, "Failed only", ERR, self.failed_only, TAB_FAILED, "Failed only");
                place(g, &mut x, if self.sort == Sort::Time { "Sort: newest" } else { "Sort: slowest" }, ACCENT, self.sort == Sort::Duration, TAB_SORT, "Sort");
            }
            Tab::Timings => {}
            Tab::Elements => place(g, &mut x, "Refresh (r)", ACCENT, false, TAB_ELEMENTS, "Refresh"),
        }
        place(g, &mut x, "Copy as text", OK, false, TAB_COPY, "Copy as text");
        place(g, &mut x, "Report site (!)", OK, false, TAB_REPORT, "Report site");
        if !matches!(self.tab, Tab::Timings | Tab::Elements) {
            place(g, &mut x, "Clear", MUTED, false, TAB_CLEAR, "Clear");
        }
        place(g, &mut x, if dock == Dock::Bottom { "Dock right" } else { "Dock bottom" }, MUTED, false, TAB_DOCK, "Dock");
        if right - x >= min_filter && self.tab != Tab::Elements {
            let fr = R::new(x, r2.y, right - x, r2.h - 4.0 * s);
            g.rrect(fr.x, fr.y, fr.w, fr.h, 6.0 * s, rgb(16, 20, 36));
            g.rrect_outline(fr.x, fr.y, fr.w, fr.h, 6.0 * s, 1.0 * s, if self.editing { ACCENT } else { rgb(60, 70, 110) });
            let shown = if self.filter.is_empty() && !self.editing { "/ filter".to_string() } else { format!("{}{}", self.filter, if self.editing { "|" } else { "" }) };
            let shown = g.fit(fs, &shown, Face::Mono, fr.w - 12.0 * s);
            g.text(fr.x + 6.0 * s, fr.y + (fr.h - g.line_height(fs, Face::Mono)) / 2.0, fs, if self.filter.is_empty() && !self.editing { DIM } else { TEXT }, &shown, Face::Mono);
            g.hit(fr, Event::Tab(TAB_FILTER), "Filter");
        }

        // the note, one line at the bottom
        let note_h = 16.0 * s;
        let note = if g.text_width(fs, NOTE_LONG, Face::Sans) <= inner.w - 2.0 * pad { NOTE_LONG } else { NOTE_SHORT };
        let note = g.fit(fs, note, Face::Sans, inner.w - 2.0 * pad);
        g.text(inner.x + pad, inner.y + inner.h - note_h, fs, DIM, &note, Face::Sans);

        // the body
        let body = R::new(inner.x, r2.y + r2.h + 2.0 * s, inner.w, (inner.y + inner.h - note_h - 2.0 * s) - (r2.y + r2.h + 2.0 * s));
        if body.h < 10.0 * s {
            return;
        }
        g.hit(body, Event::Area(AREA_BODY), "dev panel");
        g.clipped(body, |g| match self.tab {
            Tab::Console => {
                let mut l = self.console_lines();
                l.extend(self.transcript_lines());
                let prompt_h = g.line_height(fs, Face::Mono) + 12.0 * s;
                let list = R::new(body.x, body.y, body.w, (body.h - prompt_h).max(0.0));
                let empty = self.empty_text(ctx, Tab::Console, l.is_empty());
                self.draw_list(g, list, s, &l, &empty, false);
                self.draw_prompt(g, R::new(body.x, body.y + list.h, body.w, body.h - list.h), s, ctx);
            }
            Tab::Elements => self.draw_elements(g, body, s, ctx),
            Tab::Network => {
                let l = self.net_lines(ctx, reader);
                let empty = self.empty_text(ctx, Tab::Network, l.is_empty());
                self.draw_list(g, body, s, &l, &empty, true);
            }
            Tab::Timings => self.draw_timings(g, body, s, ctx),
        });
    }

    fn empty_text(&self, ctx: Ctx, tab: Tab, empty: bool) -> String {
        if !empty {
            return String::new();
        }
        if !ctx.full {
            return match tab {
                Tab::Network => "No requests yet. This is the built-in reader's own network list.".to_string(),
                Tab::Console if self.insp.lines.is_empty() => "No script errors or warnings on this page. Press e (or Enter) to type an expression for the page's script engine. Timings and messages of the page's own network need the full web engine (F).".to_string(),
                Tab::Console => String::new(),
                _ if ctx.offered => "The built-in reader reports no load timings. Press F to show this page in the full web engine; then timings appear here.".to_string(),
                _ => "Load timings need the full web engine, which this build does not have. Use the full web build (docs/FULL-BROWSER.md).".to_string(),
            };
        }
        if !self.snap.probe_seen {
            return "Waiting for the page probe. Reload the page (r) to see what happened while loading. If nothing ever appears, this web engine build has no page probe: the developer tools need the full web build (docs/FULL-BROWSER.md).".to_string();
        }
        let f = !self.filter.trim().is_empty() || self.failed_only || self.level != LevelFilter::All;
        if f {
            "Nothing matches the filter.".to_string()
        } else {
            match tab {
                Tab::Console => "No console errors or warnings on this page since the panel was opened.".to_string(),
                _ => "No failed or slow requests on this page since the panel was opened.".to_string(),
            }
        }
    }

    /// The transcript of the prompt as list lines.
    fn transcript_lines(&self) -> Vec<Line> {
        self.insp
            .lines
            .iter()
            .enumerate()
            .map(|(i, o)| {
                let (tag, color) = match o.kind {
                    InKind::In => ("in", ACCENT),
                    InKind::Out => ("=", OK),
                    InKind::Err => ("err", ERR),
                    InKind::Log => ("log", MUTED),
                    InKind::Note => ("--", DIM),
                };
                Line { time: String::new(), tag: tag.into(), color, text: o.text.clone(), ms: 0, seq: u64::MAX / 2 + i as u64 }
            })
            .collect()
    }

    /// The input line under the console list.
    fn draw_prompt(&mut self, g: &mut Gfx, r: R, s: f32, _ctx: Ctx) {
        let fs = 12.0 * s;
        let fr = R::new(r.x + 8.0 * s, r.y + 2.0 * s, (r.w - 16.0 * s).max(10.0), (r.h - 4.0 * s).max(8.0));
        let on = self.insp.editing;
        g.rrect(fr.x, fr.y, fr.w, fr.h, 6.0 * s, rgb(16, 20, 36));
        g.rrect_outline(fr.x, fr.y, fr.w, fr.h, 6.0 * s, 1.0 * s, if on { ACCENT } else { rgb(60, 70, 110) });
        let (text, col) = if self.insp.waiting {
            ("Starting the page's script engine...".to_string(), MUTED)
        } else if on {
            (format!("> {}|", self.insp.input), TEXT)
        } else {
            ("> press Enter or e to evaluate an expression in this page".to_string(), DIM)
        };
        let shown = g.fit(fs, &text, Face::Mono, fr.w - 12.0 * s);
        g.text(fr.x + 6.0 * s, fr.y + (fr.h - g.line_height(fs, Face::Mono)) / 2.0, fs, col, &shown, Face::Mono);
        g.hit(fr, Event::Tab(TAB_PROMPT), "Expression");
    }

    /// The Elements page: the tree on top, the chosen node under it.
    fn draw_elements(&mut self, g: &mut Gfx, body: R, s: f32, ctx: Ctx) {
        let fs = 12.0 * s;
        let lh = g.line_height(fs, Face::Mono) + 3.0 * s;
        let pad = 8.0 * s;
        if ctx.full || !self.insp.has_tree() {
            let msg = if ctx.full {
                "The Elements page shows pages the built-in reader shows. The full web engine keeps its own page tree and offers no safe way to read it here. Press Esc in the page for the built-in reader.".to_string()
            } else {
                "Press r (or Refresh) to read this page's tree.".to_string()
            };
            let mut y = body.y + 4.0 * s;
            for w in wrap(g, fs, &msg, body.w - 2.0 * pad) {
                if y + lh > body.y + body.h {
                    break;
                }
                g.text(body.x + pad, y, fs, MUTED, &w, Face::Sans);
                y += lh;
            }
            return;
        }
        let detail = self.insp.detail();
        let want = (detail.len() as f32 + 1.0) * lh;
        let dh = want.min(body.h * 0.45).max(lh * 2.0).min(body.h);
        let list = R::new(body.x, body.y, body.w, body.h - dh);
        let rows = self.insp.rows();
        let visible = ((list.h - lh) / lh).max(1.0) as usize;
        // keep the chosen row in view
        let sel = self.insp.sel.min(rows.len().saturating_sub(1));
        if sel < self.insp.scroll {
            self.insp.scroll = sel;
        }
        if sel >= self.insp.scroll + visible {
            self.insp.scroll = sel + 1 - visible;
        }
        self.insp.scroll = self.insp.scroll.min(rows.len().saturating_sub(visible));
        let head = if self.insp.live { "The page as its scripts left it" } else { "The page as the server sent it (evaluate an expression to start its scripts)" };
        let head = g.fit(fs, head, Face::Sans, body.w - 2.0 * pad);
        g.text(body.x + pad, list.y + 1.0 * s, fs, DIM, &head, Face::Sans);
        for (i, r) in rows.iter().skip(self.insp.scroll).take(visible).enumerate() {
            let y = list.y + lh + i as f32 * lh;
            let rr = R::new(body.x + 2.0 * s, y, body.w - 4.0 * s, lh);
            if self.insp.scroll + i == sel {
                g.rect(rr.x, rr.y, rr.w, rr.h, [ACCENT[0], ACCENT[1], ACCENT[2], 60]);
            }
            let mark = if !r.has_kids { "  " } else if r.open { "v " } else { "> " };
            let text = g.fit(fs, &format!("{}{}{}", "  ".repeat(r.depth.min(30)), mark, r.label), Face::Mono, body.w - 2.0 * pad);
            g.text(body.x + pad, y, fs, if self.insp.scroll + i == sel { TEXT } else { MUTED }, &text, Face::Mono);
            g.hit(rr, Event::Tab(TAB_ROW + i), "element row");
        }
        // the chosen node
        let dr = R::new(body.x, list.y + list.h, body.w, dh);
        g.rect(dr.x, dr.y, dr.w, dr.h, rgb(14, 18, 34));
        let mut y = dr.y + 2.0 * s;
        for l in detail.iter() {
            if y + lh > dr.y + dr.h {
                break;
            }
            let t = g.fit(fs, l, Face::Mono, dr.w - 2.0 * pad);
            g.text(dr.x + pad, y, fs, if l.starts_with('<') { ACCENT } else { TEXT }, &t, Face::Mono);
            y += lh;
        }
    }

    fn draw_list(&mut self, g: &mut Gfx, body: R, s: f32, lines: &[Line], empty: &str, show_bar: bool) {
        let fs = 12.0 * s;
        let lh = g.line_height(fs, Face::Mono) + 3.0 * s;
        let pad = 8.0 * s;
        if lines.is_empty() {
            let mut y = body.y + 4.0 * s;
            for w in wrap(g, fs, empty, body.w - 2.0 * pad) {
                if y + lh > body.y + body.h {
                    break;
                }
                g.text(body.x + pad, y, fs, MUTED, &w, Face::Sans);
                y += lh;
            }
            return;
        }
        let visible = ((body.h / lh) as usize).max(1);
        let t = idx(self.tab);
        let max_start = lines.len().saturating_sub(visible);
        self.scroll[t] = self.scroll[t].min(max_start);
        let maxms = lines.iter().map(|l| l.ms).max().unwrap_or(0).max(1);
        for (i, l) in lines.iter().skip(self.scroll[t]).take(visible).enumerate() {
            let y = body.y + i as f32 * lh + 1.0 * s;
            g.rect(body.x + pad - 5.0 * s, y + 2.0 * s, 3.0 * s, lh - 5.0 * s, l.color);
            let mut x = body.x + pad;
            let head = if l.time.is_empty() { l.tag.clone() } else { format!("{} {}", l.time, l.tag) };
            let hw = g.text(x, y, fs, l.color, &head, Face::Mono);
            x += hw + 8.0 * s;
            let room = body.x + body.w - pad - x;
            if show_bar && l.ms > 0 && room > 200.0 * s {
                let bw = (60.0 * s * l.ms as f32 / maxms as f32).max(2.0 * s);
                g.rect(body.x + body.w - pad - 60.0 * s, y + 3.0 * s, bw, lh - 8.0 * s, [l.color[0], l.color[1], l.color[2], 150]);
            }
            let room = if show_bar && l.ms > 0 && room > 200.0 * s { room - 68.0 * s } else { room };
            let text = g.fit(fs, &l.text, Face::Mono, room.max(10.0 * s));
            g.text(x, y, fs, TEXT, &text, Face::Mono);
        }
        // where the visible part is
        if lines.len() > visible {
            let th = body.h * visible as f32 / lines.len() as f32;
            let ty = body.y + (body.h - th) * self.scroll[t] as f32 / max_start.max(1) as f32;
            g.rect(body.x + body.w - 3.0 * s, ty, 3.0 * s, th.max(6.0 * s), [ACCENT[0], ACCENT[1], ACCENT[2], 160]);
        }
    }

    fn draw_timings(&mut self, g: &mut Gfx, body: R, s: f32, ctx: Ctx) {
        let fs = 12.0 * s;
        let lh = g.line_height(fs, Face::Mono) + 3.0 * s;
        let pad = 8.0 * s;
        let mut y = body.y + 4.0 * s;
        let bottom = body.y + body.h;
        let Some((resp, dcl, load, url)) = self.newest_page().filter(|_| ctx.full) else {
            let msg = self.empty_text(ctx, Tab::Timings, true);
            let msg = if ctx.full && self.snap.probe_seen { "No page load timing yet: it arrives a moment after a page finishes loading.".to_string() } else { msg };
            for w in wrap(g, fs, &msg, body.w - 2.0 * pad) {
                if y + lh > bottom {
                    break;
                }
                g.text(body.x + pad, y, fs, MUTED, &w, Face::Sans);
                y += lh;
            }
            return;
        };
        let u = g.fit(fs, &url, Face::Mono, body.w - 2.0 * pad);
        g.text(body.x + pad, y, fs, TEXT, &u, Face::Mono);
        y += lh;
        // the phases of the load as one bar
        let load = load.max(dcl).max(resp).max(1);
        let dcl = dcl.max(resp).min(load);
        let phases = [("waiting", resp, rgb(120, 130, 190)), ("download + parse", dcl - resp.min(dcl), ACCENT), ("sub-resources", load - dcl, OK)];
        let bw = body.w - 2.0 * pad;
        if y + lh * 2.0 <= bottom {
            let mut bx = body.x + pad;
            for (_, ms, c) in phases {
                let w = (bw * ms as f32 / load as f32).floor();
                if w >= 1.0 {
                    g.rect(bx, y, w, 12.0 * s, c);
                    bx += w;
                }
            }
            y += 16.0 * s;
        }
        let facts = [format!("first response {resp} ms"), format!("DOMContentLoaded {dcl} ms"), format!("load {load} ms")];
        let mut x = body.x + pad;
        for (i, f) in facts.iter().enumerate() {
            let col = phases[i].2;
            let w = g.text_width(fs, f, Face::Mono) + 18.0 * s;
            if x + w > body.x + body.w - pad && x > body.x + pad {
                x = body.x + pad;
                y += lh;
            }
            if y + lh > bottom {
                return;
            }
            g.rect(x, y + 3.0 * s, 8.0 * s, 8.0 * s, col);
            g.text(x + 12.0 * s, y, fs, MUTED, f, Face::Mono);
            x += w + 6.0 * s;
        }
        y += lh + 4.0 * s;
        if y + lh > bottom {
            return;
        }
        let slow = self.slowest();
        let head = if slow.is_empty() { "No resource took 300 ms or more.".to_string() } else { format!("Slowest resources ({} took 300 ms or more)", slow.len()) };
        g.text(body.x + pad, y, fs, ACCENT, &head, Face::Bold);
        y += lh;
        let top = slow.first().map_or(1, |s| s.0).max(1);
        let t = idx(Tab::Timings);
        let visible = (((bottom - y) / lh) as usize).max(1);
        self.scroll[t] = self.scroll[t].min(slow.len().saturating_sub(visible));
        for (ms, u, k) in slow.iter().skip(self.scroll[t]).take(visible) {
            if y + lh > bottom + 1.0 {
                break;
            }
            let bwid = (50.0 * s * *ms as f32 / top as f32).max(2.0 * s);
            g.rect(body.x + pad, y + 3.0 * s, bwid, lh - 8.0 * s, [WARN[0], WARN[1], WARN[2], 170]);
            let x = body.x + pad + 56.0 * s;
            let label = format!("{ms:>5} ms ");
            let lw = g.text(x, y, fs, WARN, &label, Face::Mono);
            let text = g.fit(fs, &format!("{u} ({k})"), Face::Mono, body.x + body.w - pad - x - lw);
            g.text(x + lw, y, fs, TEXT, &text, Face::Mono);
            y += lh;
        }
    }
}

const NOTE_LONG: &str = "Kept in memory only, while this panel is open: addresses without query strings, no page content, headers or cookies.";
const NOTE_SHORT: &str = "In memory only, while open; no queries, content, headers or cookies.";

fn net_line(z: &Zone, t: &Tagged) -> Line {
    match &t.ev {
        DevEvent::Failed { method, status, ms, url, error } => {
            let what = match (status, error.is_empty()) {
                (0, true) => "no answer".to_string(),
                (0, false) => error.clone(),
                (s, true) => format!("status {s}"),
                (s, false) => format!("status {s} {error}"),
            };
            let m = if method.is_empty() { "GET" } else { method };
            Line { time: clock_text(z, t.at), tag: "FAIL".into(), color: ERR, text: format!("{m} {url} - {what}, {ms} ms"), ms: *ms, seq: t.seq }
        }
        DevEvent::Slow { kind, bytes, ms, url } => {
            let k = if kind.is_empty() { "resource" } else { kind };
            Line { time: clock_text(z, t.at), tag: "slow".into(), color: WARN, text: format!("{k} {url} - {ms} ms{}", if *bytes > 0 { format!(", {}", human(*bytes)) } else { String::new() }), ms: *ms, seq: t.seq }
        }
        _ => Line { time: String::new(), tag: String::new(), color: MUTED, text: String::new(), ms: 0, seq: t.seq },
    }
}

fn next(t: Tab, fwd: bool) -> Tab {
    let all = [Tab::Console, Tab::Network, Tab::Timings, Tab::Elements];
    let i = idx(t);
    all[if fwd { (i + 1) % 4 } else { (i + 3) % 4 }]
}

fn chip_width(g: &mut Gfx, fs: f32, label: &str) -> f32 {
    g.text_width(fs, label, Face::Bold) + fs * 1.4
}

fn tab_chip(g: &mut Gfx, r: R, fs: f32, label: &str, col: [u8; 4], on: bool, ev: Event, name: &str) {
    let hot = g.hovered(r);
    g.rrect(r.x, r.y, r.w, r.h, 7.0 * (fs / 12.0), if on { rgb(34, 48, 84) } else if hot { rgb(26, 34, 62) } else { rgb(18, 23, 42) });
    g.rrect_outline(r.x, r.y, r.w, r.h, 7.0 * (fs / 12.0), 1.0, if on { col } else { [col[0], col[1], col[2], 90] });
    g.text(r.x + fs * 0.7, r.y + (r.h - g.line_height(fs, Face::Bold)) / 2.0, fs, if on { TEXT } else { col }, label, Face::Bold);
    g.hit(r, ev, name);
}

/// Words of `text` in lines no wider than `w`.
fn wrap(g: &mut Gfx, fs: f32, text: &str, w: f32) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    for word in text.split_whitespace() {
        let t = if cur.is_empty() { word.to_string() } else { format!("{cur} {word}") };
        if g.text_width(fs, &t, Face::Sans) > w && !cur.is_empty() {
            out.push(std::mem::take(&mut cur));
            cur = word.to_string();
        } else {
            cur = t;
        }
    }
    if !cur.is_empty() {
        out.push(cur);
    }
    out
}

/// The text of the toolbar button: the key is part of the label so that it is always visible; counters appear while collecting.
pub fn button_label(open: bool, collecting: bool, failed: usize, errors: usize) -> String {
    let mut l = if open { "Dev tools Esc".to_string() } else { "Dev tools F12".to_string() };
    if collecting && (failed > 0 || errors > 0) {
        l.push_str(&format!(" {failed}req {errors}err"));
    }
    l
}
