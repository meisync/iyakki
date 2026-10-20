//! STORE: find, install, update and remove the things that extend IYAKKI (skills, tools, MCP servers, command-line
//! tools, themes), with a trust badge on everything and a plain-words review before anything is installed. This file only
//! draws and handles keys; the catalogs, the trust model, the licences and the installing are `iyakki_agent::store`
//! (docs/STORE.md). Nothing here touches the network on the UI thread: a look at the network and the download of a
//! package run on a background thread and report back through a slot the drawing polls.

use std::collections::HashSet;
use std::sync::{Arc, Mutex};

use iyakki_agent::store::catalog::{Fetch, HttpFetch, Loaded, Origin, SourceKind};
use iyakki_agent::store::install::{Actor, Confirm, Review};
use iyakki_agent::store::installed::{self as inst, Entry as IEntry, Sort, Status, Type as IType};
use iyakki_agent::store::lifecycle::{Ctx, Life, Report, UpdatePlan};
use iyakki_agent::store::organize::Org;
use iyakki_agent::store::trust::Level;
use iyakki_agent::store::{self, Filter, Item, PriceFilter, Refresh, State, Store};

use super::chatpic::PicCache;
use super::store_drivers::{self as drv, Out as DOut};
use super::widgets::{render_confirm, Form, FormEvent};
use super::{blink, App, AppCx, Flow, ScrollLog};
use crate::fx;
use crate::gfx::{rgb, Face, Gfx, Icon};
use crate::ui::{clipboard, Event, Key, TextField, DIM, ERR, MUTED, OK, R, TEXT};

#[path = "store_manage.rs"]
mod manage;
use manage::{Drag, EditState, MenuState, Press, RemoveUi, TagsState, View, Zone};

pub const ACCENT: [u8; 4] = rgb(214, 236, 80);
const AMBER: [u8; 4] = fx::AMBER;
const SEARCH_ID: usize = 1;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Section {
    Discover,
    Installed,
    Publishers,
    Settings,
}

const SECTIONS: [(Section, &str); 4] = [(Section::Discover, "Discover"), (Section::Installed, "Installed"), (Section::Publishers, "Publishers"), (Section::Settings, "Settings")];

enum Row {
    Head(String),
    /// A heading of a grouped list (the person's groups, or Category, Kind, Source).
    Group(manage::Head),
    Item(usize),
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Purpose {
    Install,
    Update,
}

enum FormKind {
    SourceFolder,
    TrustKey,
    FromFile,
    License,
    /// Install the driver pack with this id from a folder or USB stick.
    DriverFolder(String),
    /// Extract the camera firmware (this id) from the Apple driver file the person names.
    DriverFile(String),
    /// Look at a private store's key before pinning it (the address form).
    PrivateStore,
}

enum Ask {
    /// Update a command-line tool (no package to review).
    UpdateCli(Box<UpdatePlan>),
    UpdateAll(Vec<(String, String, bool)>),
    Rollback { key: String, title: String, from: String, to: String },
    RemovePlan(Box<RemoveUi>),
    DeleteGroup(String, String),
    DeleteTag(String, usize),
    Remove { kind: String, name: String, scope: iyakki_agent::skills::Scope, title: String },
    RemoveSource(String),
    RemoveInstalled(Box<IEntry>),
    /// Pin this private store's key: the person compares the fingerprint first.
    PinStore(Box<store::Probe>, String),
}

enum Mode {
    Browse,
    Detail,
    Review(Box<ReviewView>),
    Form(Form, FormKind),
    Confirm(Ask),
    Edit(Box<EditState>),
    Menu(Box<MenuState>),
    Tags(Box<TagsState>),
    Report(Box<Report>),
}

struct ReviewView {
    review: Review,
    scroll: usize,
    purpose: Purpose,
    /// An update planned by the lifecycle layer: applied with its check and automatic rollback.
    plan: Option<UpdatePlan>,
}

enum Done {
    Refreshed(Refresh),
    Reviewed(Box<Review>, Purpose),
    Probed(Box<store::Probe>, String),
    Failed(String),
    /// An update was downloaded and reviewed: show what it would change.
    Planned(Box<UpdatePlan>),
    /// An update, rollback or removal finished.
    Applied(Result<String, String>),
    AllDone(Report),
}

struct Job {
    label: String,
    slot: Arc<Mutex<Option<Done>>>,
    /// What a long job is doing now (shown beside its label).
    progress: Option<Arc<Mutex<String>>>,
}

pub struct StoreApp {
    section: Section,
    mode: Mode,
    store: Store,
    loaded: Vec<Loaded>,
    items: Vec<Item>,
    /// Every app of every catalog (`items` holds only the chosen store's apps when one is chosen).
    all_items: Vec<Item>,
    /// The store shown in Discover: a catalog's source id, or empty for all of them. Remembered per user.
    store_pick: String,
    filter: Filter,
    query: TextField,
    searching: bool,
    sel: [usize; 4],
    first: [usize; 4],
    detail: Option<(String, String)>,
    text: ScrollLog,
    text_for: Option<(String, String, String)>,
    status: Option<(String, bool)>,
    job: Option<Job>,
    spin: u32,
    pics: PicCache,
    tip: Option<(f32, f32, String)>,
    shared: bool,
    request: Option<(String, String)>,
    /// How the network is reached; tests replace it.
    pub fetch: Arc<dyn Fetch>,
    redraw: bool,
    /// What is installed (read from the real stores), the chip, the search, the order and the open page.
    inst: Vec<IEntry>,
    ifilter: Option<IType>,
    iquery: TextField,
    isearching: bool,
    isort: Sort,
    idetail: Option<String>,
    /// Whether Discover and Publishers are shown (off until switched on or a catalog is added).
    catalog: bool,
    /// The Drivers chip: drivers for this machine (docs/MAC-HARDWARE.md). `drivers_for` replaces what is read (tests).
    drivers: drv::Pane,
    pub drivers_for: Option<iyakki_agent::store::drivers::Drivers>,
    /// The lifecycle of everything installed (update state, pins, kept versions) and what the last look at the network found.
    lives: Vec<Life>,
    ctx: Ctx,
    /// The Updates chip: only what has an update.
    iupdates: bool,
    /// The person's own arrangement (tags, groups, titles) and how the list is grouped.
    org: Org,
    org_rev: u32,
    iview: View,
    itags: Vec<String>,
    /// The cursor is on a heading (My groups): its group id, None for Ungrouped.
    on_head: Option<Option<String>>,
    collapsed_ungrouped: bool,
    collapsed_heads: HashSet<String>,
    pointer: Option<(i32, i32)>,
    drag: Option<Drag>,
    zones: Vec<Zone>,
    host_save: Option<(String, Vec<u8>)>,
}

/// The dialog that shows a private store's key fingerprint: the person compares it with the one the store's administrator gave.
fn render_pin_confirm(g: &mut Gfx, area: R, s: f32, p: &store::Probe, name: &str) {
    let pn = crate::fx::modal(g, area, s, 640.0 * s, 310.0 * s, ERR, true);
    let (bx, by, bw, bh) = (pn.x, pn.y, pn.w, pn.h);
    let title = g.fit(19.0 * s, &format!("Trust the key of “{}”?", if name.trim().is_empty() { &p.name } else { name }), Face::Bold, bw - 48.0 * s);
    g.text(bx + 24.0 * s, by + 24.0 * s, 19.0 * s, TEXT, &title, Face::Bold);
    let addr = g.fit(13.0 * s, &format!("{} · {} app{} · catalog number {}", p.url, p.entries, if p.entries == 1 { "" } else { "s" }, p.serial), Face::Sans, bw - 48.0 * s);
    g.text(bx + 24.0 * s, by + 60.0 * s, 13.0 * s, MUTED, &addr, Face::Sans);
    g.text(bx + 24.0 * s, by + 90.0 * s, 13.0 * s, MUTED, "Key fingerprint (compare it with the one the store printed):", Face::Sans);
    let fp = g.fit(22.0 * s, &p.fingerprint, Face::Mono, bw - 48.0 * s);
    g.text(bx + 24.0 * s, by + 116.0 * s, 22.0 * s, TEXT, &fp, Face::Mono);
    let note = g.fit(12.0 * s, "Only if they match: this store may then describe its apps. Each package is still checked and shown with its own trust level.", Face::Sans, bw - 48.0 * s);
    g.text(bx + 24.0 * s, by + 180.0 * s, 12.0 * s, DIM, &note, Face::Sans);
    let trust = g.fit(13.0 * s, &p.trust, Face::Bold, bw - 48.0 * s);
    g.text(bx + 24.0 * s, by + 154.0 * s, 13.0 * s, if p.trust.starts_with("Official") || p.trust.starts_with("Verified") { OK } else { AMBER }, &trust, Face::Bold);
    g.text(bx + 24.0 * s, by + 208.0 * s, 14.0 * s, MUTED, "Press y if they match, or Esc to cancel.", Face::Sans);
    g.button(bx + 24.0 * s, by + bh - 58.0 * s, 14.0 * s, "They match", ERR, Event::Key(Key::Char('y')));
    g.button(bx + 24.0 * s + 170.0 * s, by + bh - 58.0 * s, 14.0 * s, "Cancel", MUTED, Event::Key(Key::Esc));
}

fn level_color(l: Level) -> [u8; 4] {
    match l {
        Level::Official => fx::CYAN,
        Level::Verified => rgb(86, 224, 140),
        Level::Trusted => rgb(122, 172, 255),
        Level::Community => AMBER,
        Level::Unsigned => rgb(255, 130, 120),
        // developer mode: the amber of the DEV ribbon on the launcher tile
        Level::Developer => rgb(255, 196, 94),
        Level::Revoked => rgb(255, 72, 96),
    }
}

fn kind_color(kind: &str) -> [u8; 4] {
    match kind {
        "skill" => rgb(255, 170, 90),
        "tool" => rgb(100, 210, 255),
        "mcp" => rgb(167, 116, 255),
        "cli" => rgb(120, 220, 140),
        "theme" | "template" => rgb(240, 120, 220),
        _ => rgb(150, 160, 200),
    }
}

fn kind_label(kind: &str) -> &'static str {
    match kind {
        "skill" => "Skill",
        "tool" => "Tool",
        "mcp" => "MCP server",
        "cli" => "Command-line tool",
        "agent" => "Agent preset",
        "template" => "Template",
        "theme" => "Theme",
        "app" => "App",
        _ => "Other",
    }
}

fn permissive(license: &str) -> bool {
    let l = license.to_ascii_lowercase();
    ["mit", "apache", "bsd", "isc", "unlicense", "cc0", "zlib"].iter().any(|p| l.contains(p))
}

fn size_text(n: u64) -> String {
    if n >= 1 << 20 { format!("{:.1} MB", n as f64 / (1 << 20) as f64) } else if n >= 1024 { format!("{} KB", n.div_ceil(1024)) } else if n > 0 { format!("{n} B") } else { String::new() }
}

/// A shield in the colour of the trust level; filled with a tick for the vouched-for ones, an outline for unsigned, struck
/// through for revoked.
fn shield(g: &mut Gfx, x: f32, y: f32, h: f32, level: Level) {
    let c = level_color(level);
    let w = h * 0.82;
    let p = |ux: f32, uy: f32| (x + ux * w, y + uy * h);
    let pts = [p(0.5, 0.0), p(1.0, 0.16), p(1.0, 0.56), p(0.5, 1.0), p(0.0, 0.56), p(0.0, 0.16)];
    if level == Level::Unsigned {
        for i in 0..pts.len() {
            let (a, b) = (pts[i], pts[(i + 1) % pts.len()]);
            g.line(a.0, a.1, b.0, b.1, (h * 0.1).max(1.4), c);
        }
        return;
    }
    g.polygon(&pts, c);
    let dark = rgb(10, 14, 28);
    match level {
        Level::Official | Level::Verified | Level::Trusted => {
            let (a, b, d) = (p(0.26, 0.5), p(0.44, 0.68), p(0.76, 0.3));
            g.line(a.0, a.1, b.0, b.1, (h * 0.12).max(1.4), dark);
            g.line(b.0, b.1, d.0, d.1, (h * 0.12).max(1.4), dark);
        }
        Level::Community | Level::Developer => {
            let (a, b) = (p(0.5, 0.25), p(0.5, 0.6));
            g.line(a.0, a.1, b.0, b.1, (h * 0.13).max(1.4), dark);
            let d = p(0.5, 0.78);
            g.circle(d.0, d.1, (h * 0.07).max(1.0), dark);
        }
        Level::Revoked => {
            let (a, b) = (p(0.2, 0.2), p(0.8, 0.8));
            g.line(a.0, a.1, b.0, b.1, (h * 0.14).max(1.6), dark);
            let (a, b) = (p(0.8, 0.2), p(0.2, 0.8));
            g.line(a.0, a.1, b.0, b.1, (h * 0.14).max(1.6), dark);
        }
        Level::Unsigned => {}
    }
}

/// The trust badge: shield and name in a pill. Hovering it asks for the plain-words tooltip. Returns the width.
fn badge(g: &mut Gfx, x: f32, y: f32, s: f32, level: Level, tip: &mut Option<(f32, f32, String)>) -> f32 {
    let sz = 12.0 * s;
    let h = sz * 1.9;
    let label = level.label();
    let tw = g.text_width(sz, label, Face::Bold);
    let w = h * 0.55 + tw + sz * 1.3;
    let c = level_color(level);
    let r = R::new(x, y, w, h);
    let hot = g.hovered(r);
    let fill = fx::over(fx::WELL, fx::with_alpha(c, if hot { 80 } else { 40 }));
    g.rrect(x, y, w, h, h / 2.0, fill);
    g.rrect_outline(x, y, w, h, h / 2.0, 1.0, fx::with_alpha(c, 200));
    shield(g, x + h * 0.28, y + h * 0.2, h * 0.6, level);
    g.text(x + h * 0.28 + h * 0.6 * 0.82 + sz * 0.4, y + (h - g.line_height(sz, Face::Bold)) / 2.0, sz, fx::readable(c, fill, 4.5), label, Face::Bold);
    g.hit(r, Event::Tick, &format!("{}: {}", label, level.plain()));
    if hot {
        *tip = Some((x, y + h + 4.0 * s, format!("{}: {}", label, level.plain())));
    }
    w
}

fn tick_mark(g: &mut Gfx, cx: f32, cy: f32, size: f32, ok: bool) {
    let c = if ok { OK } else { ERR };
    g.circle(cx, cy, size / 2.0, fx::with_alpha(c, 50));
    let k = size * 0.22;
    if ok {
        g.line(cx - k * 1.1, cy, cx - k * 0.2, cy + k * 0.9, (size * 0.13).max(1.4), c);
        g.line(cx - k * 0.2, cy + k * 0.9, cx + k * 1.2, cy - k * 0.9, (size * 0.13).max(1.4), c);
    } else {
        g.line(cx - k, cy - k, cx + k, cy + k, (size * 0.13).max(1.4), c);
        g.line(cx + k, cy - k, cx - k, cy + k, (size * 0.13).max(1.4), c);
    }
}

fn price_color(it: &Item) -> [u8; 4] {
    if it.entry.meta.pricing.model.needs_licence() { AMBER } else { OK }
}

fn store_pref_path(cx: &AppCx) -> std::path::PathBuf {
    cx.services.dir().join(format!("store-{}.json", super::browser_config::who(cx)))
}

/// The store this person last chose in Discover (empty: all of them).
fn load_store_pick(cx: &AppCx) -> String {
    std::fs::read_to_string(store_pref_path(cx)).ok().and_then(|t| serde_json::from_str::<serde_json::Value>(&t).ok()).and_then(|v| v["store"].as_str().map(str::to_string)).filter(|s| s.len() < 40 && s.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')).unwrap_or_default()
}

fn save_store_pick(cx: &AppCx, pick: &str) {
    let p = store_pref_path(cx);
    let tmp = p.with_extension("tmp");
    if std::fs::write(&tmp, serde_json::json!({ "store": pick }).to_string()).is_ok() {
        let _ = std::fs::rename(&tmp, &p);
    }
}

impl StoreApp {
    pub fn new(cx: &AppCx) -> StoreApp {
        let mut a = StoreApp {
            section: Section::Installed,
            mode: Mode::Browse,
            store: Store::for_services(&cx.services),
            loaded: vec![],
            items: vec![],
            all_items: vec![],
            store_pick: load_store_pick(cx),
            filter: Filter::default(),
            query: TextField::new("/ search skills, tools, servers…", false),
            searching: false,
            sel: [0; 4],
            first: [0; 4],
            detail: None,
            text: ScrollLog::default(),
            text_for: None,
            status: None,
            job: None,
            spin: 0,
            pics: PicCache::default(),
            tip: None,
            shared: false,
            request: None,
            fetch: Arc::new(HttpFetch(cx.services.quick_upstream())),
            redraw: false,
            inst: vec![],
            ifilter: None,
            iquery: TextField::new("/ search what is installed…", false),
            isearching: false,
            isort: Sort::Name,
            idetail: None,
            catalog: false,
            drivers: drv::Pane::default(),
            drivers_for: None,
            lives: vec![],
            ctx: Ctx::default(),
            iupdates: false,
            org: Org::default(),
            org_rev: 0,
            iview: View::Flat,
            itags: vec![],
            on_head: None,
            collapsed_ungrouped: false,
            collapsed_heads: HashSet::new(),
            pointer: None,
            drag: None,
            zones: vec![],
            host_save: None,
        };
        a.text.nowrap = false;
        let off = a.store.enforce_revocations(&cx.services);
        // an update that was cut off is put right before anything is shown
        let mended = a.store.recover(&cx.services);
        a.reload(cx);
        if let Some(m) = mended.first() {
            a.status = Some((m.clone(), true));
        }
        if let Some(n) = off.first() {
            a.status = Some((n.clone(), false));
        }
        if a.store.auto_check_due() && cx.session.policy.apps.contains(&iyakki_sec::policy::App::Store) {
            a.start_refresh(cx);
        }
        a
    }

    pub fn status_text(&self) -> Option<&str> {
        self.status.as_ref().map(|s| s.0.as_str())
    }

    fn actor(&self, cx: &AppCx) -> Actor {
        Actor::from_policy(&cx.session.user, &cx.session.policy)
    }

    fn reload(&mut self, cx: &AppCx) {
        self.loaded = self.store.local_catalogs(&cx.services);
        let actor = self.actor(cx);
        self.all_items = self.store.items(&cx.services, &actor, &self.loaded);
        if !self.store_pick.is_empty() && !self.loaded.iter().any(|l| l.source == self.store_pick) {
            // the store was removed or switched off: show all of them again
            self.store_pick.clear();
        }
        self.items = if self.store_pick.is_empty() {
            self.all_items.clone()
        } else {
            let only: Vec<Loaded> = self.loaded.iter().filter(|l| l.source == self.store_pick).cloned().collect();
            let mut v = self.store.items(&cx.services, &actor, &only);
            v.retain(|i| i.source_name != "installed");
            v
        };
        self.catalog = self.store.catalog_visible();
        self.lives = self.store.lifecycle(&cx.services, &cx.session.policy, &cx.session.user, &self.loaded, &self.ctx);
        self.inst = self.lives.iter().map(|l| l.entry.clone()).collect();
        self.reload_org(cx);
        self.refresh_drivers(cx);
        if !self.catalog && matches!(self.section, Section::Discover | Section::Publishers) {
            self.section = Section::Installed;
            self.detail = None;
            self.mode = Mode::Browse;
        }
    }

    /// The stores that can be chosen: `(source id, name)`; empty when there is only one catalog to look at.
    fn store_choices(&self) -> Vec<(String, String)> {
        if self.loaded.len() < 2 {
            return vec![];
        }
        self.loaded.iter().map(|l| (l.source.clone(), l.name.clone())).collect()
    }

    fn store_label(&self) -> String {
        if self.store_pick.is_empty() {
            return "All".into();
        }
        self.loaded.iter().find(|l| l.source == self.store_pick).map_or("All".to_string(), |l| l.name.chars().take(18).collect())
    }

    /// Show the next store (All, then each catalog in turn); remembered for this person.
    fn cycle_store(&mut self, cx: &AppCx) {
        let choices = self.store_choices();
        if choices.is_empty() {
            self.say("There is only one store: add another in Settings (a).", false);
            return;
        }
        let at = choices.iter().position(|c| c.0 == self.store_pick).map_or(0, |i| i + 1);
        self.set_store(cx, choices.get(at).map(|c| c.0.clone()).unwrap_or_default());
    }

    fn set_store(&mut self, cx: &AppCx, id: String) {
        self.store_pick = id;
        save_store_pick(cx, &self.store_pick);
        self.reload(cx);
        self.sel[0] = 0;
        self.first[0] = 0;
        let l = self.store_label();
        self.say(if self.store_pick.is_empty() { "Showing the apps of all stores.".to_string() } else { format!("Showing only the apps of {l}.") }, true);
    }

    /// The other catalogs that list this app: `(source id, name, version)`.
    fn also_in(&self, it: &Item) -> Vec<(String, String, String)> {
        self.loaded.iter().filter(|l| l.source != it.source).filter_map(|l| l.catalog.entries.iter().find(|e| e.kind == it.entry.kind && e.name == it.entry.name).map(|e| (l.source.clone(), l.name.clone(), e.version.clone()))).collect()
    }

    /// On an app that more than one store lists: open the page of the same app as that store describes it.
    fn next_store_for_app(&mut self, cx: &AppCx) {
        let Some(it) = self.current().cloned() else { return };
        let others = self.also_in(&it);
        if others.is_empty() {
            self.say("Only one store lists this app.", false);
            return;
        }
        let mut order: Vec<String> = vec![it.source.clone()];
        order.extend(others.iter().map(|o| o.0.clone()));
        // the next store after the one shown, in the order the catalogs are listed
        let ids: Vec<&String> = self.loaded.iter().map(|l| &l.source).filter(|id| order.contains(id)).collect();
        let at = ids.iter().position(|id| **id == it.source).unwrap_or(0);
        let next = ids[(at + 1) % ids.len()].clone();
        self.set_store(cx, next);
        self.detail = Some((it.entry.kind.clone(), it.entry.name.clone()));
        self.mode = Mode::Detail;
        self.text_for = None;
    }

    fn say(&mut self, m: impl Into<String>, ok: bool) {
        self.status = Some((m.into(), ok));
    }

    // ------------------------------------------------------------------------------------------ what is listed

    fn filtered(&self) -> Vec<usize> {
        let mut base: Vec<usize> = store::filter(&self.items, &self.filter).iter().filter_map(|i| self.items.iter().position(|x| std::ptr::eq(x, *i))).collect();
        // what the person called things, tagged them and grouped them also finds them
        let words: Vec<String> = self.filter.query.to_lowercase().split_whitespace().map(str::to_string).collect();
        if !words.is_empty() {
            let rest = Filter { query: String::new(), ..self.filter.clone() };
            for i in store::filter(&self.items, &rest).iter().filter_map(|i| self.items.iter().position(|x| std::ptr::eq(x, *i))).collect::<Vec<_>>() {
                if base.contains(&i) {
                    continue;
                }
                let it = &self.items[i];
                let hay = format!("{} {} {} {}", it.title(), it.entry.name, it.entry.meta.summary, self.org.hay(&iyakki_agent::store::organize::item_key(&it.entry.kind, &it.entry.name))).to_lowercase();
                if words.iter().all(|w| hay.contains(w)) {
                    base.push(i);
                }
            }
        }
        if !self.itags.is_empty() {
            base.retain(|i| {
                let k = iyakki_agent::store::organize::item_key(&self.items[*i].entry.kind, &self.items[*i].entry.name);
                self.itags.iter().all(|t| self.org.tags_of(&k).contains(t))
            });
        }
        base
    }

    fn filtering(&self) -> bool {
        !self.itags.is_empty() || !self.filter.query.is_empty() || self.filter.price != PriceFilter::All || self.filter.level.is_some() || self.filter.kind.is_some() || self.filter.category.is_some() || self.filter.publisher.is_some()
    }

    fn discover_rows(&self) -> Vec<Row> {
        let pos = |i: &Item| self.items.iter().position(|x| std::ptr::eq(x, i));
        if self.iview != View::Flat {
            let mut v = Vec::new();
            for r in self.discover_display() {
                match r {
                    manage::DRow::Head(h) => v.push(Row::Group(h)),
                    manage::DRow::Item(k) => {
                        if let Some(i) = k.split_once(':').and_then(|(kind, name)| self.items.iter().position(|x| x.entry.kind == kind && x.entry.name == name)) {
                            v.push(Row::Item(i));
                        }
                    }
                }
            }
            return v;
        }
        if self.filtering() {
            let ids = self.filtered();
            let mut v = vec![Row::Head(format!("{} result{}", ids.len(), if ids.len() == 1 { "" } else { "s" }))];
            v.extend(ids.into_iter().map(Row::Item));
            return v;
        }
        let mut v = Vec::new();
        let f: Vec<usize> = store::featured(&self.items).into_iter().filter_map(pos).take(3).collect();
        if !f.is_empty() {
            v.push(Row::Head("Featured".into()));
            v.extend(f.into_iter().map(Row::Item));
        }
        let n: Vec<usize> = store::newest(&self.items, 4).into_iter().filter_map(pos).collect();
        if !n.is_empty() {
            v.push(Row::Head("New".into()));
            v.extend(n.into_iter().map(Row::Item));
        }
        v.push(Row::Head("All apps".into()));
        v.extend(self.filtered().into_iter().map(Row::Item));
        v
    }

    fn rows(&self) -> Vec<Row> {
        match self.section {
            Section::Discover => self.discover_rows(),
            _ => vec![],
        }
    }

    fn item_at(&self, ordinal: usize) -> Option<usize> {
        self.rows().into_iter().filter_map(|r| if let Row::Item(i) = r { Some(i) } else { None }).nth(ordinal)
    }

    fn current(&self) -> Option<&Item> {
        if let Some((k, n)) = &self.detail {
            return self.items.iter().find(|i| i.entry.kind == *k && i.entry.name == *n);
        }
        self.item_at(self.sel[self.section as usize]).map(|i| &self.items[i])
    }

    fn count(&self) -> usize {
        match self.section {
            Section::Discover => self.rows().iter().filter(|r| matches!(r, Row::Item(_))).count(),
            Section::Installed => self.ifiltered().len(),
            Section::Publishers => store::publishers(&self.items).len(),
            Section::Settings => self.settings_rows(None).len(),
        }
    }

    fn move_sel(&mut self, delta: i32) {
        let n = self.count();
        let i = self.section as usize;
        self.sel[i] = (self.sel[i] as i32 + delta).clamp(0, n.saturating_sub(1) as i32) as usize;
    }

    // ------------------------------------------------------------------------------------------ actions

    fn target_scope(&self, cx: &AppCx, kind: &str) -> iyakki_agent::skills::Scope {
        use iyakki_agent::skills::Scope;
        let a = self.actor(cx);
        if kind == "mcp" || (self.shared && a.can_shared) || !a.can_own {
            Scope::Shared
        } else {
            Scope::User(a.user)
        }
    }

    fn scope_of_installed(&self, cx: &AppCx, kind: &str, name: &str) -> iyakki_agent::skills::Scope {
        use iyakki_agent::skills::Scope;
        let me = Scope::User(cx.session.user.clone());
        if self.store.installed_version(&cx.services, kind, name, &me).is_some() || self.store.is_disabled(kind, name, &me) { me } else { Scope::Shared }
    }

    fn is_remote(&self, source: &str) -> bool {
        self.loaded.iter().any(|l| l.source == source && matches!(l.origin, Origin::Remote { .. }))
    }

    fn start_review(&mut self, cx: &AppCx, purpose: Purpose) {
        let Some(item) = self.current().cloned() else { return };
        self.start_review_item(cx, item, purpose);
    }

    fn start_review_item(&mut self, cx: &AppCx, item: Item, purpose: Purpose) {
        if self.job.is_some() {
            self.say("Wait for the current job to finish.", false);
            return;
        }
        if let State::Link(tile) = &item.state {
            if tile == "drivers" {
                self.section = Section::Installed;
                self.mode = Mode::Browse;
                self.idetail = None;
                self.ifilter = Some(IType::Driver);
                self.refresh_drivers(cx);
                return;
            }
            self.request = Some((tile.clone(), item.entry.action.as_ref().map(|a| a.id.clone()).unwrap_or_default()));
            return;
        }
        if item.entry.demo {
            self.say("This is a made-up example: it cannot be installed, and nothing is sold here.", false);
            return;
        }
        if !item.installable {
            self.say("The catalog this came from has expired. Check for updates to load a newer one.", false);
            return;
        }
        let scope = if purpose == Purpose::Update { self.scope_of_installed(cx, &item.entry.kind, &item.entry.name) } else if item.entry.meta.ext_kind == "driver" { iyakki_agent::skills::Scope::Shared } else { self.target_scope(cx, &item.entry.kind) };
        let (st, svc, actor, loaded, known) = (self.store.clone(), cx.services.clone(), self.actor(cx), self.loaded.clone(), Store::known_names(&self.all_items));
        if self.is_remote(&item.source) {
            let slot = Arc::new(Mutex::new(None));
            let (out, fetch) = (slot.clone(), self.fetch.clone());
            let label = format!("Downloading and checking {}…", item.title());
            std::thread::spawn(move || {
                let done = match st.review_item(&svc, &actor, &item, &loaded, fetch.as_ref(), scope, &known) {
                    Ok(r) => Done::Reviewed(Box::new(r), purpose),
                    Err(e) => Done::Failed(e),
                };
                *out.lock().unwrap_or_else(|e| e.into_inner()) = Some(done);
            });
            self.job = Some(Job { label, slot, progress: None });
        } else {
            match st.review_item(&svc, &actor, &item, &loaded, self.fetch.as_ref(), scope, &known) {
                Ok(r) => self.show_review(r, purpose),
                Err(e) => self.say(e, false),
            }
        }
    }

    fn show_review(&mut self, r: Review, purpose: Purpose) {
        self.searching = false;
        self.mode = Mode::Review(Box::new(ReviewView { review: r, scroll: 0, purpose, plan: None }));
    }

    fn start_refresh(&mut self, cx: &AppCx) {
        if self.job.is_some() {
            return;
        }
        let slot = Arc::new(Mutex::new(None));
        let (out, st, svc, fetch) = (slot.clone(), self.store.clone(), cx.services.clone(), self.fetch.clone());
        std::thread::spawn(move || {
            let r = st.refresh(&svc, fetch.as_ref());
            // and the vendors of the command-line tools (their newest version numbers)
            st.check_cli_latest(&svc);
            *out.lock().unwrap_or_else(|e| e.into_inner()) = Some(Done::Refreshed(r));
        });
        self.job = Some(Job { label: "Looking for news in your catalogs…".into(), slot, progress: None });
    }

    /// Take the result of a finished background job (called from drawing and from the heartbeat).
    fn apply_job(&mut self, cx: &AppCx) {
        let done = match &self.job {
            Some(j) => j.slot.lock().unwrap_or_else(|e| e.into_inner()).take(),
            None => None,
        };
        let Some(done) = done else { return };
        self.job = None;
        self.redraw = true;
        match done {
            Done::Refreshed(r) => {
                self.ctx.refresh_errors = r.errors.clone();
                self.reload(cx);
                let updates = self.lives.iter().filter(|l| l.has_update()).count();
                let mut m = if r.errors.is_empty() { "Catalogs are up to date.".to_string() } else { format!("Could not reach {}: {}", r.errors[0].0, r.errors[0].1) };
                if updates > 0 {
                    m.push_str(&format!(" {updates} update{} available.", if updates == 1 { "" } else { "s" }));
                }
                for n in r.notes.iter().chain(r.disabled.iter()) {
                    m.push(' ');
                    m.push_str(n);
                }
                cx.audit.record(&cx.session.user, "store.check", &format!("{} catalogs", r.loaded.len()));
                self.say(m, r.errors.is_empty());
            }
            Done::Reviewed(r, p) => self.show_review(*r, p),
            Done::Probed(p, name) => self.mode = Mode::Confirm(Ask::PinStore(p, name)),
            Done::Failed(e) => self.say(e, false),
            Done::Planned(plan) => {
                let plan = *plan;
                match manage::review_for(plan.clone()) {
                    Some(m) => self.mode = m,
                    None => self.mode = Mode::Confirm(Ask::UpdateCli(Box::new(plan))),
                }
            }
            Done::Applied(r) => {
                self.reload(cx);
                match r {
                    Ok(m) => self.say(m, true),
                    Err(e) => self.say(e, false),
                }
                self.mode = self.back_mode();
            }
            Done::AllDone(rep) => {
                self.reload(cx);
                let one = rep.summary().lines().next().unwrap_or("").to_string();
                self.say(one, true);
                self.mode = Mode::Report(Box::new(rep));
            }
        }
    }

    fn do_install(&mut self, cx: &AppCx, rv: &ReviewView) {
        let a = self.actor(cx);
        match self.store.install(&cx.services, &a, &rv.review, Confirm::all()) {
            Ok(rec) => {
                cx.audit.record(&cx.session.user, "store.install", &format!("{} {} {} ({}, {})", rec.kind, rec.name, rec.version, rec.level, rec.scope));
                self.say(format!("Installed {} {}.", rec.title, rec.version), true);
                self.mode = self.back_mode();
                self.reload(cx);
            }
            Err(e) => {
                self.mode = self.back_mode();
                self.say(format!("Not installed: {e}"), false);
                self.reload(cx);
            }
        }
    }

    fn do_remove(&mut self, cx: &AppCx, kind: &str, name: &str, scope: &iyakki_agent::skills::Scope, title: &str) {
        let a = self.actor(cx);
        match self.store.remove(&cx.services, &a, kind, name, scope) {
            Ok(()) => {
                cx.audit.record(&cx.session.user, "store.remove", &format!("{kind} {name}"));
                self.say(format!("Removed {title}."), true);
            }
            Err(e) => self.say(e, false),
        }
        self.reload(cx);
        let n = self.count();
        let i = self.section as usize;
        self.sel[i] = self.sel[i].min(n.saturating_sub(1));
    }

    fn toggle_off(&mut self, cx: &AppCx) {
        let Some(it) = self.current().cloned() else { return };
        let a = self.actor(cx);
        let scope = self.scope_of_installed(cx, &it.entry.kind, &it.entry.name);
        let r = if it.state == State::Disabled {
            self.store.enable(&cx.services, &a, &it.entry.kind, &it.entry.name, &scope).map(|_| format!("{} is on again.", it.title())).map_err(|e| e.to_string())
        } else if it.state.is_installed() {
            self.store.disable(&cx.services, &a, &it.entry.kind, &it.entry.name, &scope).map(|()| format!("{} is turned off. Turn it on again here.", it.title()))
        } else {
            return;
        };
        match r {
            Ok(m) => {
                cx.audit.record(&cx.session.user, "store.toggle", &it.entry.name);
                self.say(m, true);
            }
            Err(e) => self.say(e, false),
        }
        self.reload(cx);
    }

    fn open_detail(&mut self) {
        if let Some(it) = self.current() {
            self.detail = Some((it.entry.kind.clone(), it.entry.name.clone()));
            self.mode = Mode::Detail;
            self.text_for = None;
            self.searching = false;
        }
    }

    fn can_change_settings(&self, cx: &AppCx) -> bool {
        self.actor(cx).admin
    }

    fn read_file(&self, cx: &AppCx, path: &str, max: u64) -> Result<Vec<u8>, String> {
        let p = std::path::PathBuf::from(path.trim());
        iyakki_agent::scope::check_read(&cx.session.policy, &cx.session.user, &p)?;
        match std::fs::metadata(&p) {
            Ok(m) if m.is_file() && m.len() <= max => std::fs::read(&p).map_err(|e| e.to_string()),
            Ok(_) => Err(format!("that is not a file of up to {} KiB", max / 1024)),
            Err(e) => Err(e.to_string()),
        }
    }

    fn settings_rows(&self, cx: Option<&AppCx>) -> Vec<(String, String, Option<Key>, [u8; 4])> {
        let cfg = self.store.config();
        let mut v: Vec<(String, String, Option<Key>, [u8; 4])> = Vec::new();
        v.push(("Built-in catalog".into(), "always there, works offline".into(), None, OK));
        for s in &cfg.sources {
            v.push((format!("{} {}", s.name, if s.enabled { "" } else { "(off)" }).trim().to_string(), format!("{} · {}", if s.kind == SourceKind::Url { "address" } else { "folder or stick" }, s.location), Some(Key::Enter), if s.enabled { TEXT } else { DIM }));
        }
        v.push(("Add a store".into(), "paste a portal link: you see its name, signer and trust before it is enabled (a)".into(), Some(Key::Char('a')), ACCENT));
        v.push(("Add the official IYAKKI store".into(), if iyakki_agent::store::catalog::OFFICIAL_STORE_URL.is_empty() { "coming soon: the project has not published its store address yet (w)".to_string() } else { "one step; you confirm its key before it is enabled (w)".to_string() }, Some(Key::Char('w')), if iyakki_agent::store::catalog::OFFICIAL_STORE_URL.is_empty() { DIM } else { ACCENT }));
        v.push(("Add a folder or USB stick".into(), "a folder with catalog.json and packages (f)".into(), Some(Key::Char('f')), ACCENT));
        v.push(("Find USB sticks".into(), "look for stores on mounted sticks (u)".into(), Some(Key::Char('u')), ACCENT));
        let pinfo = cx.map(|c| iyakki_agent::store::privatestore::status_info(c.services.dir(), &c.session.user, &self.store));
        v.push(("Private store".into(), pinfo.as_ref().map_or_else(|| "your own app store in a cluster".to_string(), iyakki_agent::store::privatestore::status_text), None, if pinfo.as_ref().is_some_and(|i| i.added.is_some()) { OK } else { DIM }));
        v.push(("Deploy a private store to k3s…".into(), "pick a cluster, review the plan, approve (p)".into(), Some(Key::Char('p')), ACCENT));
        v.push(("Repair, reset or remove the private store…".into(), "an install that stopped half-way, a lost token, or start over (m)".into(), Some(Key::Char('m')), ACCENT));
        v.push(("Add a private store (check its key)".into(), "its address, then compare the key fingerprint (v)".into(), Some(Key::Char('v')), ACCENT));
        v.push(("Publish here".into(), "make the selected private catalog the default target for publishing (h)".into(), Some(Key::Char('h')), ACCENT));
        v.push(("Show the catalog (coming soon)".into(), if cfg.catalog_enabled { "on: Discover and Publishers are shown, with the built-in catalog (g)".into() } else if !cfg.sources.is_empty() { "on because a catalog is added (g)".into() } else { "off: the store shows only what is installed (g)".into() }, Some(Key::Char('g')), if cfg.catalog_visible() { OK } else { DIM }));
        v.push(("Daily check for news".into(), if cfg.auto_check { "on: a quick look when you open the store (o)".into() } else { "off: nothing is fetched unless you press c (o)".into() }, Some(Key::Char('o')), if cfg.auto_check { OK } else { DIM }));
        v.push(("Install from a file".into(), "a package (.iyp) from a stick or download (i)".into(), Some(Key::Char('i')), ACCENT));
        v.push(("Add a licence".into(), "for a paid app; a file or what you copied (l)".into(), Some(Key::Char('l')), ACCENT));
        v.push(("Trust a publisher key".into(), "packages signed by it show as Trusted (t)".into(), Some(Key::Char('t')), ACCENT));
        let svc_keys: Vec<(String, String)> = cx.map(|c| c.services.packages.trusted_keys()).unwrap_or_default();
        for (k, l) in svc_keys {
            v.push((format!("Trusted key: {l}"), format!("{}…", &k[..16.min(k.len())]), None, level_color(Level::Trusted)));
        }
        if self.store.roots().is_empty() {
            v.push(("IYAKKI release keys".into(), "none pinned in this build yet: only built-in items are Official".into(), None, AMBER));
        }
        for r in self.store.roots().list() {
            v.push((format!("IYAKKI release key {}", r.id), format!("{}…", &r.key[..16.min(r.key.len())]), None, level_color(Level::Official)));
        }
        let revs = self.store.revocations();
        v.push(("Revocation list".into(), match revs.freshness(self.store.now()) {
            iyakki_agent::store::revocation::Freshness::Missing => "none loaded yet".to_string(),
            iyakki_agent::store::revocation::Freshness::Fresh => format!("number {}, recent", revs.serial()),
            iyakki_agent::store::revocation::Freshness::Old(d) => format!("number {}, {d} days old", revs.serial()),
        }, None, DIM));
        v
    }

    fn settings_key(&mut self, cx: &AppCx, k: Key) {
        let admin = self.can_change_settings(cx);
        let need = |s: &mut StoreApp| s.say("Only administrators can change the store's settings.", false);
        match k {
            Key::Char('a') => {
                if admin {
                    self.mode = Mode::Form(Form::new("Add a store").text("Name", "", "optional: shown in the store switcher").text("Portal link", "https://", "https://portal.example/store/  or  …/catalog.json"), FormKind::PrivateStore);
                } else {
                    need(self);
                }
            }
            Key::Char('w') => {
                if !admin {
                    need(self);
                } else if let Some(url) = Store::official_store_url() {
                    self.probe_store(url.to_string(), String::new());
                } else {
                    self.say("The official store is coming soon: its address has not been published yet. Until then add a store with its portal link (a).", false);
                }
            }
            Key::Char('f') => {
                if admin {
                    self.mode = Mode::Form(Form::new("Add a folder or USB stick").text("Name", "", "for example My stick").text("Folder", "/mnt/", "/mnt/usb/iyakki-store"), FormKind::SourceFolder);
                } else {
                    need(self);
                }
            }
            Key::Char('u') => {
                if !admin {
                    need(self);
                    return;
                }
                let found = iyakki_agent::store::catalog::discover_sticks(&iyakki_agent::store::catalog::stick_mounts().iter().map(|p| p.as_path()).collect::<Vec<_>>());
                let mut added = 0;
                for d in &found {
                    if self.store.add_source(&self.actor(cx), "USB stick", SourceKind::Folder, &d.display().to_string()).is_ok() {
                        added += 1;
                    }
                }
                self.say(if found.is_empty() { "No store found on mounted sticks. Plug one in and try again.".to_string() } else { format!("Found {} store(s) on sticks, {added} added.", found.len()) }, !found.is_empty());
                self.reload(cx);
            }
            Key::Char('p') => {
                if admin {
                    self.request = Some(("settings".into(), "platform:private-store".into()));
                } else {
                    need(self);
                }
            }
            Key::Char('m') => {
                if admin {
                    self.request = Some(("settings".into(), "platform:private-store:manage".into()));
                } else {
                    need(self);
                }
            }
            Key::Char('v') => {
                if admin {
                    let url = iyakki_agent::store::privatestore::status_info(cx.services.dir(), &cx.session.user, &self.store).instance.map(|i| i.1).unwrap_or_default();
                    let url = if url.is_empty() { "https://".to_string() } else { url.trim_end_matches("/catalog.json").to_string() };
                    self.mode = Mode::Form(Form::new("Add a private store").text("Name", "", "for example Team store").text("Address", &url, "https://store.example.lan"), FormKind::PrivateStore);
                } else {
                    need(self);
                }
            }
            Key::Char('h') => {
                if !admin {
                    need(self);
                    return;
                }
                let cfg = self.store.config();
                let sel = self.sel[Section::Settings as usize];
                let pinned: Vec<&iyakki_agent::store::catalog::SourceCfg> = cfg.sources.iter().filter(|s| !s.pin.is_empty()).collect();
                let pick = if sel >= 1 && sel <= cfg.sources.len() && !cfg.sources[sel - 1].pin.is_empty() { Some(&cfg.sources[sel - 1]) } else if pinned.len() == 1 { Some(pinned[0]) } else { None };
                match pick {
                    Some(s) => match self.store.set_publish_target(&self.actor(cx), &s.id) {
                        Ok(()) => self.say(format!("Publishing goes to {} by default.", s.name), true),
                        Err(e) => self.say(e, false),
                    },
                    None => self.say("Select a private catalog (one added with its key checked) in the list first.", false),
                }
            }
            Key::Char('g') => {
                if admin {
                    let on = !self.store.config().catalog_enabled;
                    let _ = self.store.set_catalog_enabled(&self.actor(cx), on);
                    self.reload(cx);
                    self.say(if on { "The catalog is shown: Discover and Publishers are back." } else if self.catalog { "The catalog switch is off, but a catalog is added, so it stays visible." } else { "The catalog is hidden: the store shows what is installed." }, true);
                } else {
                    need(self);
                }
            }
            Key::Char('o') => {
                if admin {
                    let on = !self.store.config().auto_check;
                    let _ = self.store.set_auto_check(&self.actor(cx), on);
                    self.say(if on { "The store will look for news once a day when you open it." } else { "No automatic checks. Nothing is fetched unless you ask." }, true);
                } else {
                    need(self);
                }
            }
            Key::Char('i') => self.mode = Mode::Form(Form::new("Install from a file").text("File path", "", "/mnt/usb/notes.iyp").choice("For", &["Me only", "Everyone"], usize::from(self.shared && self.actor(cx).can_shared)), FormKind::FromFile),
            Key::Char('l') => self.mode = Mode::Form(Form::new("Add a licence").text("File path", "", "a licence file, or leave empty to use what you copied"), FormKind::License),
            Key::Char('t') => {
                if admin {
                    self.mode = Mode::Form(Form::new("Trust a publisher key").text("Public key (64 hex characters)", "", "from the publisher").text("Name", "", "who it is"), FormKind::TrustKey);
                } else {
                    need(self);
                }
            }
            Key::Enter => {
                // Enter on a catalog row switches it on or off; Delete removes it
                let rows = self.settings_rows(Some(cx));
                let sel = self.sel[Section::Settings as usize];
                let cfg = self.store.config();
                if sel >= 1 && sel <= cfg.sources.len() && rows.get(sel).is_some() {
                    if admin {
                        let id = cfg.sources[sel - 1].id.clone();
                        let on = self.store.toggle_source(&self.actor(cx), &id).unwrap_or(false);
                        self.say(if on { "Catalog switched on." } else { "Catalog switched off." }, true);
                        self.reload(cx);
                    } else {
                        need(self);
                    }
                } else if let Some((_, _, Some(key), _)) = rows.get(sel) {
                    let key = key.clone();
                    self.settings_key(cx, key);
                }
            }
            Key::Delete | Key::Backspace => {
                let sel = self.sel[Section::Settings as usize];
                let cfg = self.store.config();
                if sel >= 1 && sel <= cfg.sources.len() {
                    if admin {
                        self.mode = Mode::Confirm(Ask::RemoveSource(cfg.sources[sel - 1].id.clone()));
                    } else {
                        need(self);
                    }
                }
            }
            _ => {}
        }
    }

    /// Read a store's catalog in the background and then ask the person to confirm its key (nothing is saved before).
    fn probe_store(&mut self, addr: String, name: String) {
        if self.job.is_some() {
            self.say("Wait for the current job to finish.", false);
            return;
        }
        let slot = Arc::new(Mutex::new(None));
        let (out, st, fetch) = (slot.clone(), self.store.clone(), self.fetch.clone());
        std::thread::spawn(move || {
            let r = match st.probe_private(fetch.as_ref(), &addr) {
                Ok(p) => Done::Probed(Box::new(p), name),
                Err(e) => Done::Failed(format!("Could not read the store's catalog: {e}")),
            };
            *out.lock().unwrap_or_else(|e| e.into_inner()) = Some(r);
        });
        self.job = Some(Job { label: "Reading the store's key…".into(), slot, progress: None });
    }

    fn submit_form(&mut self, cx: &AppCx, form: &mut Form, kind: &FormKind) -> bool {
        let a = self.actor(cx);
        let r: Result<String, String> = match kind {
            FormKind::SourceFolder => self.store.add_source(&a, &form.value(0), SourceKind::Folder, &form.value(1)).map(|_| "Folder added.".to_string()),
            FormKind::PrivateStore => {
                if !a.admin {
                    Err("Only administrators can change the store's settings.".to_string())
                } else {
                    self.probe_store(form.value(1), form.value(0));
                    self.mode = Mode::Browse;
                    return true;
                }
            }
            FormKind::TrustKey => self.store.trust_key(&cx.services, &a, &form.value(0), &form.value(1)).map(|()| "Publisher key trusted. Packages signed by it show as Trusted.".to_string()),
            FormKind::FromFile => {
                let scope = if form.choice_idx(1) == 1 { iyakki_agent::skills::Scope::Shared } else { iyakki_agent::skills::Scope::User(cx.session.user.clone()) };
                match self.read_file(cx, &form.value(0), iyakki_agent::pkg::MAX_EXTENSION_PACKAGE_BYTES as u64) {
                    Ok(bytes) => {
                        let r = self.store.review_sideload(&cx.services, &a, bytes, scope, &Store::known_names(&self.all_items));
                        self.show_review(r, Purpose::Install);
                        return true;
                    }
                    Err(e) => Err(e),
                }
            }
            FormKind::DriverFolder(id) => {
                let d = self.drivers_handle(cx);
                let dir = std::path::PathBuf::from(form.value(0).trim());
                match iyakki_agent::scope::check_read(&cx.session.policy, &cx.session.user, &dir).and_then(|()| d.folder_package(id, &dir, self.store.now())) {
                    Ok((text, taken)) => {
                        self.say(format!("Took {} file(s) from the folder: {}", taken.len(), taken.join("; ")), true);
                        let r = self.store.review_sideload(&cx.services, &a, text.into_bytes(), iyakki_agent::skills::Scope::Shared, &Store::known_names(&self.all_items));
                        self.show_review(r, Purpose::Install);
                        return true;
                    }
                    Err(e) => Err(e),
                }
            }
            FormKind::DriverFile(id) if id != "mac-facetimehd" => Err(format!("{id} does not take an Apple driver file")),
            FormKind::DriverFile(_) => {
                let d = self.drivers_handle(cx);
                let path = form.value(0);
                let accepted = form.choice_idx(1) == 1;
                match self.read_file(cx, &path, iyakki_agent::store::drivers::MAX_DRIVER_FILE).and_then(|b| d.facetime_package(&b, accepted, self.store.now())) {
                    Ok((text, what)) => {
                        self.say(format!("Extracted {what}."), true);
                        let r = self.store.review_sideload(&cx.services, &a, text.into_bytes(), iyakki_agent::skills::Scope::Shared, &Store::known_names(&self.all_items));
                        self.show_review(r, Purpose::Install);
                        return true;
                    }
                    Err(e) => Err(e),
                }
            }
            FormKind::License => {
                let path = form.value(0);
                let text = if path.trim().is_empty() { clipboard::latest().ok_or_else(|| "The clipboard has no text. Copy the licence first, or give a file.".to_string()) } else { self.read_file(cx, &path, 16 * 1024).and_then(|b| String::from_utf8(b).map_err(|_| "that file is not text".to_string())) };
                text.and_then(|t| {
                    let items = self.items.clone();
                    self.store.add_license(&a, &t, &|app| items.iter().filter(|i| i.entry.name == app && !i.entry.signer.is_empty()).map(|i| i.entry.signer.clone()).collect()).map(|tok| {
                        cx.audit.record(&cx.session.user, "store.license", &tok.app);
                        format!("Licence added for {} ({}).", tok.app, tok.describe())
                    })
                })
            }
        };
        match r {
            Ok(m) => {
                self.mode = Mode::Browse;
                self.say(m, true);
                self.reload(cx);
                true
            }
            Err(e) => {
                form.error = Some(e);
                false
            }
        }
    }

    // ------------------------------------------------------------------------------------------ keys

    fn key_browse(&mut self, k: &Key, cx: &AppCx) -> Flow {
        let sec = self.section;
        match k {
            Key::Esc => {
                if self.filtering() {
                    self.filter = Filter::default();
                    self.query.clear();
                    self.itags.clear();
                    self.sel[0] = 0;
                } else if self.section != Section::Discover {
                    self.section = Section::Discover;
                } else {
                    return Flow::Back;
                }
            }
            Key::Tab | Key::BackTab => {
                let v = self.sections();
                let at = v.iter().position(|x| *x == self.section).unwrap_or(0);
                let next = if *k == Key::Tab { (at + 1) % v.len() } else { (at + v.len() - 1) % v.len() };
                self.switch(v[next]);
            }
            Key::Up => self.move_sel(-1),
            Key::Down => self.move_sel(1),
            Key::PageUp => self.move_sel(-6),
            Key::PageDown => self.move_sel(6),
            Key::Home => self.move_sel(-1000),
            Key::End => self.move_sel(1000),
            Key::Char('c') => self.start_refresh(cx),
            Key::Char('/') if sec == Section::Discover => {
                self.searching = true;
            }
            Key::Char('f') if sec == Section::Discover => {
                self.filter.price = match self.filter.price { PriceFilter::All => PriceFilter::Free, PriceFilter::Free => PriceFilter::Paid, PriceFilter::Paid => PriceFilter::All };
                self.sel[0] = 0;
            }
            Key::Char('s') if sec == Section::Discover => self.cycle_store(cx),
            Key::Char('t') if sec == Section::Discover => {
                let order = [None, Some(Level::Official), Some(Level::Verified), Some(Level::Trusted), Some(Level::Community), Some(Level::Unsigned)];
                let at = order.iter().position(|l| *l == self.filter.level).unwrap_or(0);
                self.filter.level = order[(at + 1) % order.len()];
                self.sel[0] = 0;
            }
            Key::Char('k') if sec == Section::Discover => {
                let kinds = ["skill", "tool", "mcp", "cli", "theme"];
                self.filter.kind = match &self.filter.kind { None => Some(kinds[0].into()), Some(k) => kinds.iter().position(|x| x == k).and_then(|i| kinds.get(i + 1)).map(|s| s.to_string()) };
                self.sel[0] = 0;
            }
            Key::Char('g') if sec == Section::Discover => {
                let cats: Vec<String> = store::categories(&self.items).into_iter().map(|c| c.0).take(8).collect();
                self.filter.category = match &self.filter.category { None => cats.first().cloned(), Some(c) => cats.iter().position(|x| x == c).and_then(|i| cats.get(i + 1)).cloned() };
                self.sel[0] = 0;
            }
            Key::Enter => match sec {
                Section::Discover | Section::Installed => self.open_detail(),
                Section::Publishers => {
                    if let Some(p) = store::publishers(&self.items).get(self.sel[2]) {
                        self.filter = Filter { publisher: Some(p.id.clone()), ..Default::default() };
                        self.section = Section::Discover;
                        self.sel[0] = 0;
                    }
                }
                Section::Settings => self.settings_key(cx, Key::Enter),
            },
            Key::Char('i') | Key::Char('u') if matches!(sec, Section::Discover | Section::Installed) => {
                let purpose = if matches!(self.current().map(|i| &i.state), Some(State::Update(_))) { Purpose::Update } else { Purpose::Install };
                if *k == Key::Char('u') && purpose != Purpose::Update {
                    self.say("There is no update for this one.", false);
                } else {
                    self.start_review(cx, purpose);
                }
            }
            Key::Char('r') if matches!(sec, Section::Discover | Section::Installed) => self.ask_remove(cx),
            Key::Char('d') if matches!(sec, Section::Discover | Section::Installed) => self.toggle_off(cx),
            Key::Char('w') if matches!(sec, Section::Discover | Section::Installed) => {
                if self.actor(cx).can_shared {
                    self.shared = !self.shared;
                    self.say(if self.shared { "New installs are for everyone." } else { "New installs are only for you." }, true);
                } else {
                    self.say("Your role installs only for you.", false);
                }
            }
            k if sec == Section::Settings => self.settings_key(cx, k.clone()),
            k if sec == Section::Discover => {
                self.org_key(k, cx);
            }
            _ => {}
        }
        Flow::Stay
    }

    fn ask_remove(&mut self, cx: &AppCx) {
        let Some(it) = self.current().cloned() else { return };
        if !it.state.is_installed() {
            self.say("It is not installed.", false);
            return;
        }
        if matches!(it.entry.kind.as_str(), "cli") {
            self.say("Command-line tools are removed in the CLI tile.", false);
            return;
        }
        let scope = self.scope_of_installed(cx, &it.entry.kind, &it.entry.name);
        self.mode = Mode::Confirm(Ask::Remove { kind: it.entry.kind.clone(), name: it.entry.name.clone(), scope, title: it.title().to_string() });
    }

    fn sections(&self) -> Vec<Section> {
        if self.catalog { vec![Section::Installed, Section::Discover, Section::Publishers, Section::Settings] } else { vec![Section::Installed, Section::Settings] }
    }

    fn back_mode(&self) -> Mode {
        if self.detail.is_some() || self.idetail.is_some() { Mode::Detail } else { Mode::Browse }
    }

    fn switch(&mut self, s: Section) {
        let s = if self.sections().contains(&s) { s } else { Section::Installed };
        self.idetail = None;
        self.isearching = false;
        self.section = s;
        self.searching = false;
        self.detail = None;
        self.mode = Mode::Browse;
    }

    fn key_detail(&mut self, k: &Key, cx: &AppCx) -> Flow {
        match k {
            Key::Esc | Key::Left => {
                self.detail = None;
                self.mode = Mode::Browse;
            }
            Key::Up => self.text.scroll_by(1),
            Key::Down => self.text.scroll_by(-1),
            Key::PageUp => self.text.scroll_by(8),
            Key::PageDown => self.text.scroll_by(-8),
            Key::Char('i') | Key::Char('u') => {
                let purpose = if matches!(self.current().map(|i| &i.state), Some(State::Update(_))) { Purpose::Update } else { Purpose::Install };
                // an update goes through the lifecycle layer: checked after installing, the old version put back if it fails
                let life = self.current().and_then(|it| self.lives.iter().find(|l| l.entry.kind == it.entry.kind && l.entry.name == it.entry.name && l.has_update()).map(|l| l.entry.key.clone()));
                match (purpose, life) {
                    (Purpose::Update, Some(key)) => self.start_update(cx, &key),
                    _ => self.start_review(cx, purpose),
                }
            }
            Key::Char('r') => self.ask_remove(cx),
            Key::Char('d') => self.toggle_off(cx),
            Key::Char('s') => self.next_store_for_app(cx),
            Key::Char('b') => self.say("Buying is not available yet: payments are coming soon.", false),
            Key::Char('l') => self.settings_key(cx, Key::Char('l')),
            Key::Char('c') => self.start_refresh(cx),
            k => {
                self.org_key(k, cx);
            }
        }
        Flow::Stay
    }

    fn key_review(&mut self, k: &Key, cx: &AppCx) {
        let Mode::Review(rv) = &mut self.mode else { return };
        match k {
            Key::Esc | Key::Char('n') => {
                let updating = rv.plan.is_some();
                self.mode = self.back_mode();
                self.say(if updating { "Nothing was changed." } else { "Nothing was installed." }, true);
            }
            Key::Up => rv.scroll = rv.scroll.saturating_sub(1),
            Key::Down => rv.scroll += 1,
            Key::PageUp => rv.scroll = rv.scroll.saturating_sub(8),
            Key::PageDown => rv.scroll += 8,
            Key::Char('y') | Key::Enter => {
                if rv.review.can_install() {
                    let Mode::Review(rv) = std::mem::replace(&mut self.mode, Mode::Browse) else { return };
                    match rv.plan.clone() {
                        Some(plan) => {
                            self.mode = self.back_mode();
                            self.apply_plan(cx, plan);
                        }
                        None => self.do_install(cx, &rv),
                    }
                } else if rv.review.locked {
                    self.mode = Mode::Form(Form::new("Add a licence").text("File path", "", "a licence file, or leave empty to use what you copied"), FormKind::License);
                }
            }
            Key::Char('l') if rv.review.locked => {
                self.mode = Mode::Form(Form::new("Add a licence").text("File path", "", "a licence file, or leave empty to use what you copied"), FormKind::License);
            }
            _ => {}
        }
    }
}

// ---------------------------------------------------------------------------------------------- the App

impl App for StoreApp {
    fn title(&self) -> &'static str {
        "Store"
    }
    fn icon(&self) -> Icon {
        Icon::Store
    }
    fn accent(&self) -> [u8; 4] {
        ACCENT
    }
    fn hint(&self) -> String {
        match (&self.mode, self.section) {
            (Mode::Review(_), _) => "y install · Esc cancel · ↑↓ scroll".into(),
            (Mode::Detail, Section::Installed) => "o open · u update · b go back a version · p pin · d turn off · r remove · e title · t tags · m group · Esc back".into(),
            (Mode::Browse, Section::Installed) => "↑↓ choose · Enter details · u update · U update all · b go back · p pin · r remove · m group · t tags · e title · v group by · g organise · / search".into(),
            (Mode::Detail, _) => "i install · u update · r remove · d turn off · s other store · Esc back".into(),
            (_, Section::Settings) => "↑↓ choose · Enter change · a add · c check · Tab section".into(),
            _ => "↑↓ choose · Enter open · / search · s store · f t k g filter · i install · c check · Tab section".into(),
        }
    }

    fn help(&self) -> Vec<String> {
        [
            "Installed (the page the store opens on): everything on this system that extends it, read from the real stores. Left / Right · type (All, MCP, CLI, Skills, Apps, Extensions)      / · search      s · order (name, recently installed, size, trust)",
            "Enter · details      o · open it in the tile where it lives      d · turn off or on (store packages)      r · remove (shows what goes and what stays; its data is kept unless you choose to delete it)      u · update, when a catalog knows one (you see the new permissions before anything changes)",
            "b · go back to the previous version (one is kept)      p · pin: do not update this one      U · update all (a review first; anything that needs a decision is skipped)      c · check for updates, including the vendors of command-line tools",
            "Arranging (yours alone, never sent anywhere): m · move to a group      t · tags (Tab completes)      e · your own title (the real name always stays visible)      n · new group      v · group by None / My groups / Category / Kind / Source      g · organise menu: groups, tags, export and import      T · tags: filter, rename, merge, delete",
            "Drag an item or a group heading in My groups to move it; Shift+Up / Shift+Down reorder with the keyboard; Enter or Space on a heading folds it; on a heading e renames, k changes the colour, x deletes the group (its items stay)",
            "Tab / Shift+Tab · the pages (Discover and Publishers appear when the catalog is switched on in Settings, key g)      Up / Down / PgUp / PgDn · choose",
            "Enter · open an app's page      Esc · back, or clear the search and filters",
            "/ · search by words (name, summary, tags, publisher)      f · free or paid      t · trust level      k · type      g · category",
            "s · switch store: all stores, then each one in turn (remembered for you); on an app's page, the same app as another store lists it      a (Settings) · add a store from its portal link",
            "i · install (you see who made it, how far it is trusted and what it asks for, and confirm)      u · update      r · remove      d · turn off or on",
            "w · install only for you, or for everyone (if your role allows)      c · look for news in your catalogs (nothing is fetched otherwise)",
            "Discover can be arranged too: m move to a group · e your own title · # tags · v group by · O organise menu · T tags (Installed uses t and g for tags and the menu)",
            "On an app's page: b shows that buying is not available yet; l adds a licence you were given",
            "Settings (administrators): a add a catalog address, f add a folder or USB stick, u find sticks, o the daily check, i install from a file, t trust a publisher key",
            "Badges: Official = made by IYAKKI; Verified = identity checked by IYAKKI; Trusted = a key your administrator chose; Community = signed by someone unknown; Unsigned = anyone could have written it; Revoked = withdrawn",
            "Select text with the pointer and Ctrl+C to copy it.",
        ]
        .iter()
        .map(|s| s.to_string())
        .collect()
    }

    fn take_request(&mut self) -> Option<(String, String)> {
        self.request.take()
    }

    fn take_host_save(&mut self) -> Option<(String, Vec<u8>)> {
        self.host_save.take()
    }

    /// Another tile (MCP, CLI, SKILLS) asked for one installed item: `installed:<kind>:<name>`.
    fn open_item(&mut self, item: &str, cx: &AppCx) {
        // the Devices popover's "Driver help": the Drivers chip
        if item == "drivers" {
            self.handle(&Event::Tab(226), cx);
            return;
        }
        let Some(id) = item.strip_prefix("installed:") else { return };
        self.reload(cx);
        let keys: Vec<String> = self.lives.iter().filter(|l| l.id == id || l.entry.name == id).map(|l| l.entry.key.clone()).collect();
        if let Some(k) = keys.first() {
            self.section = Section::Installed;
            self.ifilter = None;
            self.iquery.clear();
            self.iupdates = false;
            self.itags.clear();
            self.idetail = Some(k.clone());
            self.mode = Mode::Detail;
            self.text_for = None;
        } else {
            self.say(format!("{id} is not installed any more."), false);
        }
    }

    fn wants_redraw(&mut self) -> bool {
        if std::mem::take(&mut self.redraw) {
            return true;
        }
        if self.job.is_some() {
            self.spin += 1;
            if self.job.as_ref().is_some_and(|j| j.slot.lock().unwrap_or_else(|e| e.into_inner()).is_some()) {
                return true;
            }
            return self.spin % 6 == 0;
        }
        false
    }

    fn handle(&mut self, ev: &Event, cx: &AppCx) -> Flow {
        if matches!(ev, Event::Tick) {
            self.apply_job(cx);
            return Flow::Stay;
        }
        // a key ends any drag in progress
        if matches!(ev, Event::Key(_)) {
            self.drag = None;
        }
        // the pointer: remembered always; in the list it also decides click or drag
        if let Event::MouseMove(x, y) = ev {
            if matches!(self.mode, Mode::Browse) && matches!(self.section, Section::Installed | Section::Discover) {
                self.pointer_move(*x, *y);
                return Flow::Stay;
            }
            self.pointer = Some((*x, *y));
        }
        if let Event::MouseUp(x, y) = ev {
            if matches!(self.mode, Mode::Browse) && matches!(self.section, Section::Installed | Section::Discover) {
                self.pointer_up(*x, *y, cx);
                return Flow::Stay;
            }
            self.drag = None;
        }
        // the small dialogs of the management layer
        match (&self.mode, ev) {
            (Mode::Edit(_), Event::Key(k)) => {
                let k = k.clone();
                self.edit_key(&k, cx);
                return Flow::Stay;
            }
            (Mode::Edit(_), Event::Paste(t)) => {
                if let Mode::Edit(st) = &mut self.mode {
                    st.field.paste(t);
                }
                return Flow::Stay;
            }
            (Mode::Menu(_), Event::Key(k)) => {
                let k = k.clone();
                self.menu_key(&k, cx);
                return Flow::Stay;
            }
            (Mode::Tags(_), Event::Key(k)) => {
                let k = k.clone();
                self.tags_key(&k);
                return Flow::Stay;
            }
            (Mode::Edit(_) | Mode::Menu(_) | Mode::Tags(_), Event::Tab(n)) => {
                self.dialog_click(*n, cx);
                return Flow::Stay;
            }
            (Mode::Edit(_) | Mode::Menu(_) | Mode::Tags(_), _) => return Flow::Stay,
            (Mode::Report(_), Event::Key(_)) => {
                self.mode = self.back_mode();
                return Flow::Stay;
            }
            (Mode::Report(_), _) => return Flow::Stay,
            (Mode::Confirm(Ask::RemovePlan(_)), Event::Key(k)) => {
                let k = k.clone();
                self.remove_key(&k, cx);
                return Flow::Stay;
            }
            (Mode::Confirm(Ask::RemovePlan(_)), Event::Paste(t)) => {
                if let Mode::Confirm(Ask::RemovePlan(ui)) = &mut self.mode {
                    if let Some(f) = &mut ui.typing {
                        f.paste(t);
                    }
                }
                return Flow::Stay;
            }
            _ => {}
        }
        // dialogs own the keyboard
        if let Mode::Form(form, kind) = &mut self.mode {
            match form.handle_event(ev) {
                FormEvent::Cancel => self.mode = Mode::Browse,
                FormEvent::Submit => {
                    let mut f = std::mem::replace(form, Form::new(""));
                    let k = std::mem::replace(kind, FormKind::TrustKey);
                    let keep_going = !self.submit_form(cx, &mut f, &k);
                    if keep_going {
                        self.mode = Mode::Form(f, k);
                    }
                }
                _ => {}
            }
            return Flow::Stay;
        }
        if let Mode::Confirm(ask) = &self.mode {
            if let Event::Key(k) = ev {
                if *k == Key::Char('y') {
                    match ask {
                        Ask::UpdateCli(plan) => {
                            let plan = (**plan).clone();
                            self.mode = self.back_mode();
                            self.apply_plan(cx, plan);
                        }
                        Ask::UpdateAll(_) => {
                            self.mode = self.back_mode();
                            self.run_update_all(cx);
                        }
                        Ask::Rollback { key, .. } => {
                            let key = key.clone();
                            self.mode = self.back_mode();
                            self.do_rollback(cx, &key);
                        }
                        Ask::RemovePlan(_) => {}
                        Ask::DeleteGroup(id, name) => {
                            let (id, name) = (id.clone(), name.clone());
                            self.mode = Mode::Browse;
                            match self.org.delete_group(&id) {
                                Ok(n) => {
                                    self.org_save();
                                    self.say(format!("Group “{name}” deleted; its {n} item(s) are now Ungrouped."), true);
                                }
                                Err(e) => self.say(e, false),
                            }
                            self.on_head = None;
                        }
                        Ask::DeleteTag(t, _) => {
                            let t = t.clone();
                            let n = self.org.delete_tag(&t);
                            self.itags.retain(|x| *x != t);
                            self.org_save();
                            self.say(format!("Tag “{t}” deleted from {n} item(s)."), true);
                            self.mode = Mode::Tags(Box::new(TagsState { sel: 0 }));
                        }
                        Ask::Remove { kind, name, scope, title } => {
                            let (kind, name, scope, title) = (kind.clone(), name.clone(), scope.clone(), title.clone());
                            self.mode = self.back_mode();
                            self.do_remove(cx, &kind, &name, &scope, &title);
                            if self.current().is_none_or(|i| !i.state.is_installed()) {
                                self.detail = None;
                                self.mode = Mode::Browse;
                            }
                        }
                        Ask::RemoveInstalled(e) => {
                            let e = (**e).clone();
                            self.mode = self.back_mode();
                            match self.store.remove_installed(&cx.services, &cx.session.policy, &cx.session.user, &e) {
                                Ok(m) => {
                                    cx.audit.record(&cx.session.user, "store.remove", &format!("{} {}", e.kind, e.name));
                                    self.say(m, true);
                                    self.idetail = None;
                                    self.mode = Mode::Browse;
                                }
                                Err(m) => self.say(m, false),
                            }
                            self.reload(cx);
                            let n = self.count();
                            self.sel[1] = self.sel[1].min(n.saturating_sub(1));
                        }
                        Ask::PinStore(p, name) => {
                            let (p, name) = ((**p).clone(), name.clone());
                            self.mode = Mode::Browse;
                            match self.store.pin_probed(&self.actor(cx), &name, &p, &p.fingerprint) {
                                Ok(_) => {
                                    cx.audit.record(&cx.session.user, "store.pin", &format!("{} {}", p.url, p.fingerprint));
                                    self.say("Store added with its key pinned. Press c to fetch it, then switch stores with s in Discover.", true);
                                }
                                Err(e) => self.say(e, false),
                            }
                            self.reload(cx);
                        }
                        Ask::RemoveSource(id) => {
                            let id = id.clone();
                            let r = self.store.remove_source(&self.actor(cx), &id);
                            self.mode = Mode::Browse;
                            match r {
                                Ok(()) => self.say("Catalog removed.", true),
                                Err(e) => self.say(e, false),
                            }
                            self.reload(cx);
                        }
                    }
                } else {
                    self.mode = self.back_mode();
                }
            }
            return Flow::Stay;
        }
        if matches!(self.mode, Mode::Review(_)) {
            if let Event::Key(k) = ev {
                let k = k.clone();
                self.key_review(&k, cx);
            }
            return Flow::Stay;
        }
        if self.section == Section::Installed && matches!(self.mode, Mode::Browse | Mode::Detail) {
            if let Some(f) = self.handle_installed(ev, cx) {
                return f;
            }
        }
        // pointer: the detail text can be selected and copied
        if matches!(self.mode, Mode::Detail) && self.text.handle(ev) {
            return Flow::Stay;
        }
        match ev {
            Event::Tab(n) if *n < 4 => {
                self.switch(SECTIONS[*n].0);
                return Flow::Stay;
            }
            Event::Tab(n) if (306..307).contains(n) => {
                self.start_refresh(cx);
                return Flow::Stay;
            }
            Event::Tab(manage::TAB_VIEW) if self.section == Section::Discover => {
                let v = self.iview.next();
                self.set_view(v);
                return Flow::Stay;
            }
            Event::Tab(manage::TAB_TAGS) if self.section == Section::Discover => {
                self.mode = Mode::Tags(Box::new(TagsState { sel: 0 }));
                return Flow::Stay;
            }
            Event::Tab(n) if (manage::TAB_TAG_CHIP..manage::TAB_TAG_CHIP + 200).contains(n) && self.section == Section::Discover => {
                if let Some((t, _)) = self.org.all_tags().get(*n - manage::TAB_TAG_CHIP).cloned() {
                    self.toggle_tag_filter(&t);
                }
                return Flow::Stay;
            }
            Event::Tab(n) if (manage::TAB_HEAD..manage::TAB_HEAD + 200).contains(n) && self.section == Section::Discover && matches!(self.mode, Mode::Browse) => {
                self.press(Press::Head(*n - manage::TAB_HEAD), cx);
                return Flow::Stay;
            }
            Event::Tab(n) if (200..210).contains(n) => {
                match n - 200 {
                    0 => self.filter.price = match self.filter.price { PriceFilter::All => PriceFilter::Free, PriceFilter::Free => PriceFilter::Paid, PriceFilter::Paid => PriceFilter::All },
                    1 => return self.key_browse(&Key::Char('t'), cx),
                    5 => {
                        if matches!(self.mode, Mode::Detail) { self.next_store_for_app(cx) } else { self.cycle_store(cx) }
                        return Flow::Stay;
                    }
                    2 => return self.key_browse(&Key::Char('k'), cx),
                    3 => return self.key_browse(&Key::Char('g'), cx),
                    _ => {
                        self.filter = Filter::default();
                        self.query.clear();
                    }
                }
                self.sel[0] = 0;
                return Flow::Stay;
            }
            Event::Tab(n) if (300..312).contains(n) => {
                let key = match n - 300 {
                    0 => Key::Char('i'),
                    1 => Key::Char('u'),
                    2 => Key::Char('r'),
                    3 => Key::Char('d'),
                    4 => Key::Char('b'),
                    5 => Key::Char('l'),
                    6 => Key::Char('c'),
                    _ => Key::Esc,
                };
                return if matches!(self.mode, Mode::Detail) { self.key_detail(&key, cx) } else { self.key_browse(&key, cx) };
            }
            Event::Row(i) => {
                match self.section {
                    Section::Discover if matches!(self.mode, Mode::Browse) => self.press(Press::Item(*i), cx),
                    Section::Discover | Section::Installed => {
                        self.sel[self.section as usize] = *i;
                        self.open_detail();
                    }
                    _ => self.sel[self.section as usize] = *i,
                }
                return Flow::Stay;
            }
            Event::Scroll(d) => {
                if matches!(self.mode, Mode::Detail) {
                    self.text.scroll_by(*d * 3);
                } else {
                    self.move_sel(-*d);
                }
                return Flow::Stay;
            }
            Event::Focus(_) | Event::FocusAt(..) if self.section == Section::Discover && matches!(self.mode, Mode::Browse) => {
                self.searching = true;
                if let Event::FocusAt(_, x) = ev {
                    self.query.click(*x);
                }
                return Flow::Stay;
            }
            Event::Paste(t) if self.searching => {
                self.query.paste(t);
                self.filter.query = self.query.value.clone();
                self.sel[0] = 0;
                return Flow::Stay;
            }
            _ => {}
        }
        let Event::Key(k) = ev else { return Flow::Stay };
        if self.searching {
            match k {
                Key::Enter => self.searching = false,
                Key::Esc => {
                    self.query.clear();
                    self.filter.query.clear();
                    self.searching = false;
                }
                Key::Up => self.move_sel(-1),
                Key::Down => self.move_sel(1),
                other => {
                    self.query.handle(other);
                    self.filter.query = self.query.value.clone();
                    self.sel[0] = 0;
                }
            }
            return Flow::Stay;
        }
        if matches!(self.mode, Mode::Detail) {
            return self.key_detail(k, cx);
        }
        self.key_browse(k, cx)
    }

    fn render(&mut self, g: &mut Gfx, area: R, s: f32, cx: &AppCx, tick: u64) {
        self.apply_job(cx);
        self.tip = None;
        let x = area.x + 24.0 * s;
        let w = area.w - 48.0 * s;
        let mut y = area.y + 10.0 * s;
        // sections
        let tabs: Vec<(String, Event)> = self
            .sections()
            .iter()
            .map(|sec| {
                let i = SECTIONS.iter().position(|x| x.0 == *sec).unwrap_or(0);
                let name = SECTIONS[i].1;
                let n = if *sec == Section::Installed {
                    let u = self.inst.iter().filter(|e| matches!(e.status, Status::Update(_))).count();
                    if u > 0 { format!("{name} ({u} update{})", if u == 1 { "" } else { "s" }) } else { name.to_string() }
                } else {
                    name.to_string()
                };
                (n, Event::Tab(i))
            })
            .collect();
        let selected = self.sections().iter().position(|x| *x == self.section).unwrap_or(0);
        let sw = fx::tab_strip(g, x, y, s, w, &tabs, selected, ACCENT);
        let check = "Check (c)";
        let cw = g.text_width(13.0 * s, check, Face::Bold) + 13.0 * 1.7 * s;
        let mut right = x + w;
        if x + w - cw > x + sw + 12.0 * s {
            g.button(x + w - cw, y, 13.0 * s, check, ACCENT, Event::Tab(306));
            right = x + w - cw - 8.0 * s;
        }
        let n_up = self.lives.iter().filter(|l| l.has_update()).count();
        if self.section == Section::Installed && n_up > 0 && matches!(self.mode, Mode::Browse) && self.idetail.is_none() {
            let label = format!("Update all {n_up} (U)");
            let uw = g.text_width(13.0 * s, &label, Face::Bold) + 13.0 * 1.7 * s;
            if right - uw > x + sw + 12.0 * s {
                g.button(right - uw, y, 13.0 * s, &label, OK, Event::Tab(manage::TAB_UPDATE_ALL));
            }
        }
        y += 40.0 * s;
        if let Some(j) = &self.job {
            let dots = ".".repeat(1 + (tick / 10 % 3) as usize);
            let now = j.progress.as_ref().map(|p| p.lock().unwrap_or_else(|e| e.into_inner()).clone()).filter(|p| !p.is_empty() && *p != j.label);
            let line = format!("{}{}{}", j.label.trim_end_matches('…'), dots, now.map(|n| format!(" {n}")).unwrap_or_default());
            fx::status_line(g, x, y, w, s, &line, true);
            y += 24.0 * s;
        } else if let Some((m, ok)) = &self.status {
            let m = g.fit(13.0 * s, m, Face::Sans, w - 30.0 * s);
            fx::status_line(g, x, y, w, s, &m, *ok);
            y += 24.0 * s;
        }
        let body = R::new(x, y, w, (area.y + area.h - y - 6.0 * s).max(40.0 * s));
        match (&self.mode, self.section) {
            (Mode::Detail, Section::Installed) => self.render_inst_detail(g, body, s, cx),
            (Mode::Detail, _) => self.render_detail(g, body, s, cx, tick),
            (_, Section::Discover) => self.render_discover(g, body, s, tick),
            (_, Section::Installed) => self.render_installed(g, body, s, tick),
            (_, Section::Publishers) => self.render_publishers(g, body, s),
            (_, Section::Settings) => self.render_settings(g, body, s, cx),
        }
        match &self.mode {
            Mode::Form(f, _) => f.render(g, area, s, ACCENT, blink(tick)),
            Mode::Confirm(Ask::Remove { title, .. }) => render_confirm(g, area, s, &format!("Remove “{title}”?"), "Remove"),
            Mode::Confirm(Ask::PinStore(p, name)) => render_pin_confirm(g, area, s, p, name),
            Mode::Confirm(Ask::RemoveSource(_)) => render_confirm(g, area, s, "Remove this catalog?", "Remove"),
            Mode::Confirm(Ask::RemoveInstalled(e)) => render_confirm(g, area, s, &format!("Remove “{}”?", e.title), "Remove"),
            Mode::Confirm(Ask::RemovePlan(ui)) => manage::render_remove(g, area, s, ui, tick),
            Mode::Confirm(Ask::UpdateAll(rows)) => manage::render_update_all(g, area, s, rows),
            Mode::Confirm(Ask::UpdateCli(plan)) => manage::render_update_cli(g, area, s, plan),
            Mode::Confirm(Ask::Rollback { title, from, to, .. }) => render_confirm(g, area, s, &format!("Go back from {from} to {to} for “{title}”? It will be pinned at {to}."), "Go back"),
            Mode::Confirm(Ask::DeleteGroup(_, name)) => render_confirm(g, area, s, &format!("Delete the group “{name}”? Its items become Ungrouped."), "Delete group"),
            Mode::Confirm(Ask::DeleteTag(t, n)) => render_confirm(g, area, s, &format!("Delete the tag “{t}” from {n} item(s)?"), "Delete tag"),
            Mode::Edit(st) => self.render_edit(g, area, s, st, tick),
            Mode::Menu(m) => self.render_menu(g, area, s, m),
            Mode::Tags(t) => self.render_tags(g, area, s, t),
            Mode::Report(rep) => manage::render_report(g, area, s, rep),
            Mode::Review(rv) => {
                let (review, scroll) = (rv.review.clone(), rv.scroll);
                let top = rv.purpose;
                let plan = rv.plan.clone();
                let sc = self.render_review(g, area, s, &review, scroll, top, plan.as_ref());
                if let Mode::Review(rv) = &mut self.mode {
                    rv.scroll = sc;
                }
            }
            _ => {}
        }
        if let Some((tx, ty, text)) = self.tip.clone() {
            let lines = g.wrap(12.0 * s, &text, Face::Sans, 280.0 * s);
            let tw = lines.iter().map(|l| g.text_width(12.0 * s, l, Face::Sans)).fold(0.0, f32::max) + 20.0 * s;
            let th = lines.len() as f32 * 16.0 * s + 14.0 * s;
            let tx = tx.min(area.x + area.w - tw - 8.0 * s).max(area.x + 4.0 * s);
            let ty = ty.min(area.y + area.h - th - 4.0 * s);
            g.rrect(tx, ty, tw, th, 8.0 * s, fx::PANEL);
            g.rrect_outline(tx, ty, tw, th, 8.0 * s, 1.0, ACCENT);
            for (i, l) in lines.iter().enumerate() {
                g.text(tx + 10.0 * s, ty + 7.0 * s + i as f32 * 16.0 * s, 12.0 * s, TEXT, l, Face::Sans);
            }
        }
    }
}

// ---------------------------------------------------------------------------------------------- drawing

impl StoreApp {
    fn icon_tile(&mut self, g: &mut Gfx, x: f32, y: f32, d: f32, s: f32, it: &Item) {
        let c = kind_color(&it.entry.kind);
        if let Some(bytes) = it.entry.meta.icon.as_ref().and_then(|i| i.bytes()) {
            let id = it.entry.sha256.bytes().take(8).fold(0u64, |a, b| a * 31 + b as u64) ^ bytes.len() as u64;
            if let Ok(r) = self.pics.draw_bytes(g, id, &bytes, d, s) {
                let k = d / r.pm.width().max(1) as f32 * r.k.max(0.01) / r.k.max(0.01);
                let paint = tiny_skia::PixmapPaint { quality: tiny_skia::FilterQuality::Bilinear, ..tiny_skia::PixmapPaint::default() };
                g.rrect(x, y, d, d, d * 0.22, fx::WELL);
                g.pm.draw_pixmap(0, 0, r.pm.as_ref().as_ref(), &paint, tiny_skia::Transform::from_translate(x, y).pre_scale(k, k), None);
                return;
            }
        }
        g.rrect(x, y, d, d, d * 0.22, fx::over(fx::WELL, fx::with_alpha(c, 70)));
        g.rrect_outline(x, y, d, d, d * 0.22, 1.2, fx::with_alpha(c, 200));
        let letter: String = it.title().chars().next().map(|c| c.to_uppercase().collect()).unwrap_or_default();
        let sz = d * 0.5;
        let lw = g.text_width(sz, &letter, Face::Bold);
        g.text(x + (d - lw) / 2.0, y + (d - g.line_height(sz, Face::Bold)) / 2.0, sz, fx::readable(c, fx::WELL, 4.5), &letter, Face::Bold);
    }

    fn item_row(&mut self, g: &mut Gfx, r: R, s: f32, idx: usize, ordinal: usize, selected: bool) {
        let it = self.items[idx].clone();
        let hot = g.hovered(r);
        fx::row_at(g, r.x, r.y, r.w, r.h, 12.0 * s, selected, hot, ACCENT);
        let okey = iyakki_agent::store::organize::item_key(&it.entry.kind, &it.entry.name);
        let shown = self.org.display_title(&okey, it.title()).to_string();
        let aliased = shown != it.title();
        g.hit(r, Event::Row(ordinal), &format!("{} {}", shown, it.state.label()));
        let d = r.h - 16.0 * s;
        self.icon_tile(g, r.x + 10.0 * s, r.y + 8.0 * s, d, s, &it);
        let tx = r.x + 10.0 * s + d + 12.0 * s;
        // the right side: price and state
        let mut rx = r.x + r.w - 10.0 * s;
        let state = match &it.state {
            State::Available => None,
            other => Some(other.label()),
        };
        if let Some(label) = state {
            let col = match &it.state { State::Installed => OK, State::Update(_) => ACCENT, State::Locked => AMBER, State::Demo => rgb(255, 120, 200), State::Revoked => ERR, _ => MUTED };
            let tw = g.text_width(12.0 * s, &label, Face::Bold) + 17.0 * s;
            rx -= tw;
            fx::chip(g, rx, r.y + (r.h - 22.8 * s) / 2.0, s, &label, col, matches!(it.state, State::Update(_)));
            rx -= 8.0 * s;
        }
        let price = it.entry.meta.pricing.label();
        let pw = g.text_width(12.0 * s, &price, Face::Bold) + 17.0 * s;
        if rx - pw > tx + 160.0 * s {
            rx -= pw;
            fx::chip(g, rx, r.y + (r.h - 22.8 * s) / 2.0, s, &price, price_color(&it), false);
            rx -= 8.0 * s;
        }
        let room = (rx - tx - 6.0 * s).max(40.0 * s);
        // line 1: the title and the badge
        let mut tip = self.tip.take();
        let bw = 74.0 * s + 8.0 * s;
        let title = g.fit(16.0 * s, &shown, Face::Bold, (room - bw).max(60.0 * s));
        let tw = g.text_width(16.0 * s, &title, Face::Bold);
        g.text(tx, r.y + 7.0 * s, 16.0 * s, TEXT, &title, Face::Bold);
        let mut bx = tx + tw + 10.0 * s;
        if tx + tw + 8.0 * s + 60.0 * s < tx + room {
            bx += badge(g, bx, r.y + 6.0 * s, s, it.verdict.level, &mut tip) + 6.0 * s;
        }
        for t in self.org.tags_of(&okey).iter().take(3) {
            let lbl = format!("#{t}");
            let w = g.text_width(12.0 * s, &lbl, Face::Bold) + 17.0 * s;
            if bx + w >= tx + room {
                break;
            }
            bx += fx::chip(g, bx, r.y + 6.0 * s, s, &lbl, rgb(150, 200, 255), self.itags.contains(t)) + 6.0 * s;
        }
        self.tip = tip;
        // line 2: who and what (and the real name when a title of the person's own is shown)
        let line = g.fit(12.0 * s, &self.item_sub(&it, aliased), Face::Sans, room);
        g.text(tx, r.y + r.h - 8.0 * s - g.line_height(12.0 * s, Face::Sans), 12.0 * s, MUTED, &line, Face::Sans);
    }

    fn list_rows(&mut self, g: &mut Gfx, body: R, s: f32, rows: &[Row], sec: usize) {
        let (ih, hh) = (58.0 * s, 28.0 * s);
        if sec == 0 {
            self.zones.clear();
        }
        let heights: Vec<f32> = rows.iter().map(|r| if matches!(r, Row::Head(_)) { hh } else if matches!(r, Row::Group(_)) { 34.0 * s } else { ih + 4.0 * s }).collect();
        let sel_ord = self.sel[sec];
        let sel_row = rows.iter().enumerate().filter(|(_, r)| matches!(r, Row::Item(_))).nth(sel_ord).map(|(i, _)| i).unwrap_or(0);
        let mut first = self.first[sec].min(rows.len().saturating_sub(1));
        if sel_row < first {
            first = sel_row;
            // show the heading above the first item if there is one
            if first > 0 && matches!(rows[first - 1], Row::Head(_) | Row::Group(_)) {
                first -= 1;
            }
        }
        while first < sel_row && heights[first..=sel_row].iter().sum::<f32>() > body.h {
            first += 1;
        }
        self.first[sec] = first;
        let mut y = body.y;
        let mut ordinal = rows.iter().take(first).filter(|r| matches!(r, Row::Item(_))).count();
        let mut head_idx = rows.iter().take(first).filter(|r| matches!(r, Row::Group(_))).count();
        let mut group_now: Option<Option<String>> = rows.iter().take(first).rev().find_map(|r| if let Row::Group(h) = r { Some(if h.kind == manage::HeadKind::Group { h.id.clone() } else { None }) } else { None });
        let grouped = self.iview == View::Groups && sec == 0;
        let dragging = self.drag.as_ref().is_some_and(|d| d.active);
        let drop_at = if dragging { self.pointer } else { None };
        let mut shown = 0;
        for (i, row) in rows.iter().enumerate().skip(first) {
            if y + heights[i] > body.y + body.h + 0.5 {
                break;
            }
            match row {
                Row::Head(t) => {
                    fx::section_title(g, body.x, y + 4.0 * s, body.w, s, t, ACCENT);
                }
                Row::Group(h) => {
                    let rect = R::new(body.x, y, body.w, 30.0 * s);
                    let dropping = grouped && h.kind != manage::HeadKind::Plain && drop_at.is_some_and(|(px, py)| rect.contains(px, py));
                    manage::render_head(g, rect, s, h, head_idx, false, dropping, dragging);
                    let grp = if h.kind == manage::HeadKind::Group { h.id.clone() } else { None };
                    group_now = Some(grp.clone());
                    if grouped {
                        self.zones.push(Zone { rect, what: manage::ZoneKind::Head { group: grp, kind: h.kind } });
                    }
                    head_idx += 1;
                }
                Row::Item(idx) => {
                    let inset = if grouped { 16.0 * s } else { 0.0 };
                    let rr = R::new(body.x + inset, y, body.w - inset, ih);
                    self.item_row(g, rr, s, *idx, ordinal, ordinal == sel_ord);
                    if grouped {
                        let k = iyakki_agent::store::organize::item_key(&self.items[*idx].entry.kind, &self.items[*idx].entry.name);
                        self.zones.push(Zone { rect: rr, what: manage::ZoneKind::Item { key: k, group: group_now.clone().unwrap_or(None) } });
                        if let Some((_, py)) = drop_at.filter(|(px, py)| rr.contains(*px, *py)) {
                            let ly = if (py as f32) < rr.y + rr.h / 2.0 { rr.y - 2.0 * s } else { rr.y + rr.h + 1.0 * s };
                            g.rect(rr.x, ly, rr.w, 3.0 * s, ACCENT);
                        }
                    }
                    ordinal += 1;
                }
            }
            y += heights[i];
            shown += 1;
        }
        if shown < rows.len() {
            let total = rows.iter().filter(|r| matches!(r, Row::Item(_))).count();
            fx::scrollbar(g, R::new(body.x + body.w - 6.0 * s, body.y, 4.0 * s, body.h), s, first, shown, rows.len().max(total), ACCENT);
        }
        if let (Some(d), Some(at), true) = (&self.drag, self.pointer, sec == 0) {
            if d.active {
                manage::render_ghost(g, s, at, &d.label);
            }
        }
    }

    fn filter_chips(&mut self, g: &mut Gfx, x: f32, y: f32, w: f32, s: f32) -> f32 {
        let f = &self.filter;
        let mut chips: Vec<(String, bool, usize)> = Vec::new();
        if !self.store_choices().is_empty() {
            chips.push((format!("Store: {} (s)", self.store_label()), !self.store_pick.is_empty(), 205));
        }
        chips.extend(vec![
            (format!("Price: {}", match f.price { PriceFilter::All => "any", PriceFilter::Free => "free", PriceFilter::Paid => "paid" }), f.price != PriceFilter::All, 200),
            (format!("Trust: {}", f.level.map_or("any", |l| l.label())), f.level.is_some(), 201),
            (format!("Type: {}", f.kind.as_deref().map_or("any", |k| kind_label(k))), f.kind.is_some(), 202),
            (format!("Category: {}", f.category.as_deref().unwrap_or("any")), f.category.is_some(), 203),
            (format!("Group by: {} (v)", self.iview.label()), self.iview != View::Flat, manage::TAB_VIEW),
            (if self.itags.is_empty() { "Tags (T)".to_string() } else { format!("Tags: {} (T)", self.itags.join(", ")) }, !self.itags.is_empty(), manage::TAB_TAGS),
        ]);
        let mut cx_ = x;
        let mut cy = y;
        let lh = 12.0 * s * 1.9;
        for (label, on, ev) in chips {
            let cw = g.text_width(12.0 * s, &label, Face::Bold) + 17.0 * s;
            if cx_ + cw > x + w {
                cx_ = x;
                cy += lh + 6.0 * s;
            }
            let r = R::new(cx_, cy, cw, lh);
            fx::chip(g, cx_, cy, s, &label, ACCENT, on);
            g.hit(r, Event::Tab(ev), &label);
            cx_ += cw + 8.0 * s;
        }
        if self.filtering() {
            let label = "Clear (Esc)";
            let cw = g.text_width(12.0 * s, label, Face::Bold) + 17.0 * s;
            if cx_ + cw > x + w {
                cx_ = x;
                cy += lh + 6.0 * s;
            }
            fx::chip(g, cx_, cy, s, label, ERR, false);
            g.hit(R::new(cx_, cy, cw, lh), Event::Tab(204), label);
        }
        cy + lh - y
    }

    fn render_discover(&mut self, g: &mut Gfx, body: R, s: f32, tick: u64) {
        let (x, mut y, w) = (body.x, body.y, body.w);
        let sr = R::new(x, y, (w * 0.42).clamp(200.0 * s, 420.0 * s), 36.0 * s);
        self.query.render(g, sr, self.searching && matches!(self.mode, Mode::Browse), s, blink(tick), Face::Sans, SEARCH_ID);
        let chips_x = sr.x + sr.w + 14.0 * s;
        let ch = self.filter_chips(g, chips_x, y + 2.0 * s, (x + w - chips_x).max(100.0 * s), s);
        y += (36.0 * s).max(ch + 4.0 * s) + 8.0 * s;
        let tags = self.org.all_tags();
        if !tags.is_empty() {
            let lh = 12.0 * s * 1.9;
            let mut tx = x;
            for (i, (t, n)) in tags.iter().take(12).enumerate() {
                let label = format!("#{t} {n}");
                let cw = g.text_width(12.0 * s, &label, Face::Bold) + 17.0 * s;
                if tx + cw > x + w {
                    break;
                }
                fx::chip(g, tx, y, s, &label, rgb(150, 200, 255), self.itags.contains(t));
                g.hit(R::new(tx, y, cw, lh), Event::Tab(manage::TAB_TAG_CHIP + i), &format!("tag {t}"));
                tx += cw + 6.0 * s;
            }
            y += lh + 8.0 * s;
        }
        let rows = self.discover_rows();
        let items_n = rows.iter().filter(|r| matches!(r, Row::Item(_))).count();
        if items_n == 0 {
            fx::empty_state(g, R::new(x, y, w, (body.y + body.h - y).max(120.0 * s)), s, Icon::Store, "Nothing matches", "Clear the search or the filters with Esc. Only the catalogs on this device are searched; press c to look for news in the ones you added.", ACCENT);
            return;
        }
        self.sel[0] = self.sel[0].min(items_n - 1);
        self.list_rows(g, R::new(x, y, w, (body.y + body.h - y).max(60.0 * s)), s, &rows, 0);
    }

    fn render_publishers(&mut self, g: &mut Gfx, body: R, s: f32) {
        let ps = store::publishers(&self.items);
        if ps.is_empty() {
            fx::empty_state(g, body, s, Icon::Users, "No publishers yet", "Publishers appear when a catalog lists their apps.", ACCENT);
            return;
        }
        self.sel[2] = self.sel[2].min(ps.len() - 1);
        let rh = 54.0 * s;
        let fit = ((body.h / (rh + 6.0 * s)).floor() as usize).max(1);
        let first = self.sel[2].saturating_sub(fit - 1);
        let mut y = body.y;
        for (i, p) in ps.iter().enumerate().skip(first).take(fit) {
            let r = R::new(body.x, y, body.w, rh);
            fx::row_at(g, r.x, r.y, r.w, r.h, 12.0 * s, i == self.sel[2], g.hovered(r), ACCENT);
            g.hit(r, Event::Row(i), &p.name);
            let mut tip = self.tip.take();
            let name = g.fit(16.0 * s, &p.name, Face::Bold, r.w * 0.4);
            let nw = g.text_width(16.0 * s, &name, Face::Bold);
            g.text(r.x + 16.0 * s, r.y + 8.0 * s, 16.0 * s, TEXT, &name, Face::Bold);
            badge(g, r.x + 26.0 * s + nw, r.y + 7.0 * s, s, p.level, &mut tip);
            self.tip = tip;
            let sub = g.fit(12.0 * s, &format!("{} app{} · {}", p.apps, if p.apps == 1 { "" } else { "s" }, if p.url.is_empty() { p.id.clone() } else { p.url.clone() }), Face::Sans, r.w - 40.0 * s);
            g.text(r.x + 16.0 * s, r.y + rh - 8.0 * s - g.line_height(12.0 * s, Face::Sans), 12.0 * s, MUTED, &sub, Face::Sans);
            y += rh + 6.0 * s;
        }
    }

    fn render_settings(&mut self, g: &mut Gfx, body: R, s: f32, cx: &AppCx) {
        let rows = self.settings_rows(Some(cx));
        self.sel[3] = self.sel[3].min(rows.len().saturating_sub(1));
        let rh = 46.0 * s;
        let fit = ((body.h / (rh + 4.0 * s)).floor() as usize).max(1);
        let first = self.sel[3].saturating_sub(fit - 1);
        let mut y = body.y;
        for (i, (label, value, key, col)) in rows.iter().enumerate().skip(first).take(fit) {
            let r = R::new(body.x, y, body.w, rh);
            fx::row_at(g, r.x, r.y, r.w, r.h, 12.0 * s, i == self.sel[3], g.hovered(r), ACCENT);
            g.hit(r, Event::Row(i), label);
            let l = g.fit(15.0 * s, label, Face::Bold, r.w * 0.5);
            g.text(r.x + 16.0 * s, r.y + 6.0 * s, 15.0 * s, *col, &l, Face::Bold);
            let v = g.fit(12.0 * s, value, Face::Sans, r.w - 36.0 * s);
            g.text(r.x + 16.0 * s, r.y + rh - 8.0 * s - g.line_height(12.0 * s, Face::Sans), 12.0 * s, MUTED, &v, Face::Sans);
            let _ = key;
            y += rh + 4.0 * s;
        }
        if !self.can_change_settings(cx) {
            let note = g.fit(12.0 * s, "You can look, but only administrators change these settings.", Face::Sans, body.w);
            g.text(body.x, body.y + body.h - 16.0 * s, 12.0 * s, DIM, &note, Face::Sans);
        }
    }

    fn rebuild_text(&mut self, it: &Item, cx: &AppCx) {
        let others = self.also_in(it);
        let key = (it.entry.kind.clone(), it.entry.name.clone(), format!("{}|{}|{}", it.entry.version, it.source, others.iter().map(|o| format!("{}={}", o.0, o.2)).collect::<Vec<_>>().join(",")));
        if self.text_for.as_ref() == Some(&key) {
            return;
        }
        self.text_for = Some(key);
        self.text.clear();
        let m = &it.entry.meta;
        let user = &cx.session.user;
        let push = |t: &mut ScrollLog, s: &str, c: [u8; 4]| t.push(s.to_string(), c);
        let para = |t: &mut ScrollLog, text: &str, c: [u8; 4]| {
            for l in text.lines() {
                t.push(l.trim_end().to_string(), c);
            }
        };
        if !m.summary.is_empty() {
            push(&mut self.text, &m.summary, TEXT);
        }
        if !m.description.is_empty() {
            push(&mut self.text, "", TEXT);
            para(&mut self.text, &m.description, MUTED);
        }
        push(&mut self.text, "", TEXT);
        push(&mut self.text, "WHAT IT CAN DO", ACCENT);
        let perms: Vec<String> = m.permissions.iter().filter_map(|p| iyakki_agent::store::manifest::Permission::parse(p).ok()).map(|p| p.words(user)).collect();
        if perms.is_empty() {
            push(&mut self.text, if it.entry.kind == "skill" { "Nothing: a skill is text the assistant reads. It runs nothing and asks for no access." } else { "No extra access is listed." }, MUTED);
        }
        for p in perms {
            push(&mut self.text, &format!("• {p}"), AMBER);
        }
        push(&mut self.text, "(This is what the catalog says. The install review reads the package itself and shows what it really asks for.)", DIM);
        if !m.changelog.is_empty() {
            push(&mut self.text, "", TEXT);
            push(&mut self.text, "WHAT'S NEW", ACCENT);
            para(&mut self.text, &m.changelog, MUTED);
        }
        push(&mut self.text, "", TEXT);
        push(&mut self.text, "DETAILS", ACCENT);
        let e = &it.entry;
        for (k, v) in [
            ("Type", kind_label(&e.kind).to_string()),
            ("Licence", if e.license.is_empty() { "see the maker".into() } else { format!("{}{}", e.license, if permissive(&e.license) { " (permissive)" } else { "" }) }),
            ("Size", size_text(e.size)),
            ("Catalog", it.source_name.clone()),
            ("Also in", others.iter().map(|o| format!("{} {}", o.1, o.2)).collect::<Vec<_>>().join(", ")),
            ("Support", m.support_url.clone()),
            ("Source", m.source_url.clone()),
            ("Needs IYAKKI", m.min_version.clone()),
            ("Depends on", m.dependencies.iter().map(|d| d.name.clone()).collect::<Vec<_>>().join(", ")),
            ("Hash", e.sha256.chars().take(24).collect()),
        ] {
            if !v.is_empty() {
                push(&mut self.text, &format!("{k}: {v}"), MUTED);
            }
        }
        // start at the top
        self.text.scroll = usize::MAX / 4;
    }

    fn render_detail(&mut self, g: &mut Gfx, body: R, s: f32, cx: &AppCx, tick: u64) {
        let _ = tick;
        let Some(it) = self.current().cloned() else {
            self.detail = None;
            self.mode = Mode::Browse;
            return;
        };
        self.rebuild_text(&it, cx);
        let (x, w) = (body.x, body.w);
        let mut y = body.y;
        g.button(x, y, 13.0 * s, "Back (Esc)", MUTED, Event::Tab(307));
        y += 40.0 * s;
        let d = 64.0 * s;
        self.icon_tile(g, x, y, d, s, &it);
        let tx = x + d + 16.0 * s;
        let title = g.fit(24.0 * s, it.title(), Face::Bold, w - d - 16.0 * s);
        g.text(tx, y, 24.0 * s, TEXT, &title, Face::Bold);
        let e = &it.entry;
        let facts = [
            if e.meta.publisher.name.is_empty() { String::new() } else { format!("by {}", e.meta.publisher.name) },
            format!("version {}", e.version),
            size_text(e.size),
            e.license.clone(),
        ]
        .into_iter()
        .filter(|t| !t.is_empty())
        .collect::<Vec<_>>()
        .join("  ·  ");
        let facts = g.fit(13.0 * s, &facts, Face::Sans, w - d - 16.0 * s);
        g.text(tx, y + 32.0 * s, 13.0 * s, MUTED, &facts, Face::Sans);
        let mut tip = self.tip.take();
        let mut bx = tx;
        bx += badge(g, bx, y + 54.0 * s, s, it.verdict.level, &mut tip) + 8.0 * s;
        let price = e.meta.pricing.label();
        bx += fx::chip(g, bx, y + 54.0 * s, s, &price, price_color(&it), false) + 8.0 * s;
        if permissive(&e.license) {
            bx += fx::chip(g, bx, y + 54.0 * s, s, "Permissive licence", OK, false) + 8.0 * s;
        }
        if e.demo {
            fx::chip(g, bx, y + 54.0 * s, s, "DEMO", rgb(255, 120, 200), true);
        } else if it.preinstalled {
            fx::chip(g, bx, y + 54.0 * s, s, "Preinstalled", MUTED, false);
        }
        self.tip = tip;
        y += d + 16.0 * s;
        let plain = g.fit(12.0 * s, &format!("{}: {}", it.verdict.level.label(), it.verdict.level.plain()), Face::Sans, w);
        g.text(x, y, 12.0 * s, level_color(it.verdict.level), &plain, Face::Sans);
        y += 22.0 * s;
        // the actions
        let mut bx = x;
        let a = self.actor(cx);
        let mut add = |g: &mut Gfx, label: &str, col: [u8; 4], ev: usize, on: bool| {
            let bw = g.text_width(14.0 * s, label, Face::Bold) + 14.0 * 1.7 * s;
            if bx + bw > x + w {
                return;
            }
            if on {
                g.button(bx, y, 14.0 * s, label, col, Event::Tab(ev));
            } else {
                g.button_off(bx, y, 14.0 * s, label);
            }
            bx += bw + 8.0 * s;
        };
        let can = a.allowed && it.installable;
        match &it.state {
            State::Available => add(g, "Install (i)", ACCENT, 300, can),
            State::Update(v) => {
                add(g, &format!("Update to {v} (u)"), ACCENT, 301, can);
                add(g, "Remove (r)", ERR, 302, true);
                add(g, "Turn off (d)", MUTED, 303, true);
            }
            State::Installed => {
                add(g, "Remove (r)", ERR, 302, e.kind != "cli");
                add(g, "Turn off (d)", MUTED, 303, e.kind != "cli");
            }
            State::Disabled => {
                add(g, "Turn on (d)", OK, 303, true);
                add(g, "Remove (r)", ERR, 302, true);
            }
            State::Locked => {
                add(g, "Buy (coming soon)", AMBER, 304, false);
                add(g, "Add licence (l)", ACCENT, 305, true);
            }
            State::Demo => {
                add(g, "Buy (coming soon)", AMBER, 304, false);
            }
            State::Link(t) => add(g, &format!("Open {} (i)", t.to_uppercase()), ACCENT, 300, true),
            State::Revoked => {
                add(g, "Revoked: cannot be installed", ERR, 300, false);
            }
        }
        if !self.also_in(&it).is_empty() {
            add(g, &format!("Store: {} (s)", it.source_name.chars().take(18).collect::<String>()), ACCENT, 205, true);
        }
        if matches!(it.state, State::Demo | State::Locked) {
            let n = g.fit(12.0 * s, "Payments are coming soon.", Face::Sans, (x + w - bx).max(40.0 * s));
            g.text(bx + 4.0 * s, y + 8.0 * s, 12.0 * s, DIM, &n, Face::Sans);
        }
        y += 14.0 * 2.1 * s + 10.0 * s;
        let r = R::new(x, y, w, (body.y + body.h - y).max(40.0 * s));
        fx::panel(g, r, s, ACCENT);
        let inner = R::new(r.x + 16.0 * s, r.y + 10.0 * s, r.w - 32.0 * s, (r.h - 20.0 * s).max(16.0 * s));
        self.text.render(g, inner, 13.0 * s, Face::Sans);
    }

    #[allow(clippy::too_many_arguments)]
    #[allow(clippy::too_many_arguments)]
    fn render_review(&mut self, g: &mut Gfx, area: R, s: f32, r: &Review, scroll: usize, purpose: Purpose, plan: Option<&UpdatePlan>) -> usize {
        let p = fx::modal(g, area, s, 840.0 * s, (area.h - 24.0 * s).min(660.0 * s), ACCENT, false);
        let (x, y, w, h) = (p.x + 26.0 * s, p.y + 18.0 * s, p.w - 52.0 * s, p.h);
        let verb = match purpose { Purpose::Install => "Install", Purpose::Update => "Update" };
        let title = g.fit(20.0 * s, &format!("{verb} “{}”?", if r.title.is_empty() { &r.name } else { &r.title }), Face::Bold, w);
        g.text(x, y, 20.0 * s, TEXT, &title, Face::Bold);
        let mut tip = self.tip.take();
        let bw = badge(g, x, y + 32.0 * s, s, r.verdict.level, &mut tip);
        self.tip = tip;
        let sub = g.fit(12.0 * s, &manage::review_sub(r), Face::Sans, w - bw - 12.0 * s);
        g.text(x + bw + 12.0 * s, y + 36.0 * s, 12.0 * s, MUTED, &sub, Face::Sans);
        let mut top = y + 66.0 * s;
        // the verdict on top, if it cannot go on
        if let Some(why) = &r.refusal {
            let lines = g.wrap(13.0 * s, why, Face::Bold, w - 24.0 * s);
            let bh = lines.len().min(3) as f32 * 18.0 * s + 14.0 * s;
            g.rrect(x, top, w, bh, 8.0 * s, fx::over(fx::WELL, fx::with_alpha(ERR, 50)));
            g.rrect_outline(x, top, w, bh, 8.0 * s, 1.2, ERR);
            for (i, l) in lines.iter().take(3).enumerate() {
                g.text(x + 12.0 * s, top + 7.0 * s + i as f32 * 18.0 * s, 13.0 * s, ERR, l, Face::Bold);
            }
            top += bh + 8.0 * s;
        }
        // the lines: checks, chain, permissions, warnings, what is inside
        #[derive(Clone, Copy, PartialEq)]
        enum K {
            Head,
            Text,
            Ok,
            No,
            Perm,
            Warn,
            Dim,
        }
        let mut lines: Vec<(K, String)> = Vec::new();
        let wrap = |g: &mut Gfx, t: &str| g.wrap(13.0 * s, t, Face::Sans, w - 40.0 * s);
        lines.push((K::Head, "Who vouches for it".into()));
        for l in wrap(g, &r.verdict.label) {
            lines.push((K::Text, l));
        }
        for c in &r.verdict.chain {
            for l in wrap(g, c) {
                lines.push((K::Dim, l));
            }
        }
        lines.push((K::Head, "Checks".into()));
        for st in &r.steps {
            let t = if st.note.is_empty() { st.label.clone() } else { format!("{}: {}", st.label, st.note) };
            for (i, l) in wrap(g, &t).into_iter().enumerate() {
                lines.push((if i == 0 { if st.ok { K::Ok } else { K::No } } else { K::Dim }, l));
            }
        }
        if let Some(pl) = plan {
            lines.push((K::Head, format!("Update {} to {}", pl.from, pl.to)));
            if !pl.changelog.is_empty() {
                for l in wrap(g, &format!("What is new: {}", pl.changelog)) {
                    lines.push((K::Text, l));
                }
            }
            if pl.added.is_empty() {
                lines.push((K::Ok, "No new permissions: it asks for nothing the installed version did not have.".into()));
            } else {
                lines.push((K::Head, "NEW PERMISSIONS: the installed version did not have these".into()));
                for pm in &pl.added {
                    lines.push((K::Warn, format!("NEW  {}", pm.words)));
                }
            }
            for rm in &pl.removed {
                lines.push((K::Dim, format!("no longer asks for: {rm}")));
            }
            lines.push((K::Dim, "The old version is kept for one rollback; if the new one fails its check, the old one is put back.".into()));
        }
        lines.push((K::Head, if plan.is_some() { "It asks for (all of it)".into() } else { "It asks for".into() }));
        if r.permissions.is_empty() {
            lines.push((K::Text, "Nothing extra: no files, no network, no programs.".into()));
        }
        for pm in &r.permissions {
            let new = plan.is_some_and(|pl| pl.added.iter().any(|a| a.canonical == pm.canonical));
            lines.push((K::Perm, format!("{}{}{}", if new { "NEW  " } else { "" }, pm.words, if pm.sensitive { "  (every use still asks you)" } else { "" })));
        }
        if !r.warnings.is_empty() {
            lines.push((K::Head, "Read before you go on".into()));
            for wn in &r.warnings {
                for l in wrap(g, wn) {
                    lines.push((K::Warn, l));
                }
            }
        }
        if !r.preview.is_empty() {
            lines.push((K::Head, "What is inside".into()));
            for pv in r.preview.iter().take(8) {
                for l in wrap(g, pv).into_iter().take(2) {
                    lines.push((K::Dim, l));
                }
            }
        }
        let lh = 18.0 * s;
        let bottom = p.y + h - 64.0 * s;
        let fit = (((bottom - top) / lh).floor() as usize).max(1);
        let first = scroll.min(lines.len().saturating_sub(fit));
        let mut ly = top;
        for (k, t) in lines.iter().skip(first).take(fit) {
            match k {
                K::Head => {
                    g.text(x, ly + 2.0 * s, 11.0 * s, ACCENT, &t.to_uppercase(), Face::Bold);
                }
                K::Text => {
                    g.text(x + 8.0 * s, ly, 13.0 * s, TEXT, t, Face::Sans);
                }
                K::Ok | K::No => {
                    tick_mark(g, x + 12.0 * s, ly + 8.0 * s, 15.0 * s, *k == K::Ok);
                    g.text(x + 30.0 * s, ly, 13.0 * s, if *k == K::Ok { TEXT } else { ERR }, t, Face::Sans);
                }
                K::Perm => {
                    g.circle(x + 12.0 * s, ly + 8.0 * s, 3.0 * s, AMBER);
                    g.text(x + 24.0 * s, ly, 13.0 * s, AMBER, t, Face::Bold);
                }
                K::Warn => {
                    g.text(x + 8.0 * s, ly, 13.0 * s, AMBER, t, Face::Sans);
                }
                K::Dim => {
                    g.text(x + 30.0 * s, ly, 12.0 * s, MUTED, t, Face::Sans);
                }
            }
            ly += lh;
        }
        if lines.len() > fit {
            fx::scrollbar(g, R::new(p.x + p.w - 14.0 * s, top, 4.0 * s, bottom - top), s, first, fit, lines.len(), ACCENT);
        }
        // the buttons
        let by = p.y + h - 48.0 * s;
        if r.can_install() {
            let label = match (plan, r.needs_warning_ack()) {
                (Some(pl), _) if !pl.added.is_empty() => "Update and allow the new permissions (y)",
                (Some(_), true) => "Update anyway (y)",
                (Some(_), false) => "Update (y)",
                (None, true) => "Install anyway (y)",
                (None, false) if !r.permissions.is_empty() => "Install and allow these (y)",
                (None, false) => "Install (y)",
            };
            let col = if r.needs_warning_ack() { AMBER } else { OK };
            let bw = g.button(x, by, 14.0 * s, label, col, Event::Key(Key::Char('y')));
            g.button(x + bw + 12.0 * s, by, 14.0 * s, "Cancel (Esc)", MUTED, Event::Key(Key::Esc));
        } else {
            let mut bx = x;
            if r.locked {
                bx += g.button(bx, by, 14.0 * s, "Add licence (l)", ACCENT, Event::Key(Key::Char('l'))) + 12.0 * s;
                bx += g.button_off(bx, by, 14.0 * s, "Buy (coming soon)") + 12.0 * s;
            }
            g.button(bx, by, 14.0 * s, "Close (Esc)", MUTED, Event::Key(Key::Esc));
        }
        first
    }
}

// ---------------------------------------------------------------------------------------------- the installed list

fn type_color(t: IType) -> [u8; 4] {
    match t {
        IType::Mcp => rgb(167, 116, 255),
        IType::Cli => rgb(120, 220, 140),
        IType::Skill => rgb(255, 170, 90),
        IType::App => rgb(120, 170, 255),
        IType::Extension => rgb(240, 120, 220),
        IType::Driver => rgb(110, 200, 255),
    }
}

fn status_color(s: &Status) -> [u8; 4] {
    match s {
        Status::On => OK,
        Status::Off => MUTED,
        Status::Update(_) => ACCENT,
        Status::Revoked => ERR,
    }
}

fn for_chip(target: &str) -> String {
    if target == "any" { "for any tile".into() } else { format!("for {}", target.to_uppercase()) }
}

const ACT_OPEN: usize = 0;
const ACT_TOGGLE: usize = 1;
const ACT_REMOVE: usize = 2;
const ACT_UPDATE: usize = 3;

impl StoreApp {
    fn iselected(&self) -> Option<IEntry> {
        if let Some(k) = &self.idetail {
            return self.inst.iter().find(|e| e.key == *k).cloned();
        }
        if self.on_head.is_some() {
            return None;
        }
        self.ifiltered().get(self.sel[1]).map(|e| (*e).clone())
    }

    fn drivers_handle(&self, cx: &AppCx) -> iyakki_agent::store::drivers::Drivers {
        self.drivers_for.clone().unwrap_or_else(|| iyakki_agent::store::drivers::Drivers::for_services(&cx.services))
    }

    /// Read the machine again for the Drivers chip (only while it is showing).
    fn refresh_drivers(&mut self, cx: &AppCx) {
        if self.ifilter == Some(IType::Driver) {
            let d = self.drivers_handle(cx);
            self.drivers.refresh(&d);
        }
    }

    /// The Drivers chip's keys and clicks; what it asks for goes through the same forms, reviews and removals as the rest.
    fn handle_drivers(&mut self, ev: &Event, cx: &AppCx) -> Option<Flow> {
        let mine = match ev {
            Event::Tab(n) => (drv::BASE..=drv::WHY_REMOVE).contains(n),
            Event::Row(_) | Event::Scroll(_) => true,
            Event::Key(k) => match k {
                Key::Esc | Key::Left => self.drivers.wants_escape(),
                Key::Up | Key::Down | Key::Enter | Key::Char('w' | 'i' | 'r') => true,
                _ => false,
            },
            _ => false,
        };
        if !mine {
            return None;
        }
        match self.drivers.handle(ev) {
            None => {}
            Some(DOut::Say(m, ok)) => self.say(m, ok),
            Some(DOut::InstallBuiltin(id)) => match self.items.iter().find(|i| i.entry.id == id).cloned() {
                Some(item) => self.start_review_item(cx, item, Purpose::Install),
                None => self.say("That driver pack is not in the built-in catalog.", false),
            },
            Some(DOut::InstallFolder(id)) => {
                let title = iyakki_drivers::known::by_id(&id).map_or(id.clone(), |k| k.title.to_string());
                self.mode = Mode::Form(Form::new(&format!("{title}: install from a folder or USB stick")).text("Folder", "/mnt/", "/mnt/usb/firmware"), FormKind::DriverFolder(id));
            }
            Some(DOut::InstallExtract(id)) => {
                self.mode = Mode::Form(
                    Form::new("Extract the camera firmware from Apple's driver file").text("File (AppleCameraInterface)", "/mnt/", "/mnt/usb/AppleCameraInterface").choice("Apple's licence (read it under Why)", &["I do not accept", "I accept"], 0),
                    FormKind::DriverFile(id),
                );
            }
            Some(DOut::Remove { id, title }) => self.mode = Mode::Confirm(Ask::Remove { kind: "extension".into(), name: id, scope: iyakki_agent::skills::Scope::Shared, title }),
        }
        Some(Flow::Stay)
    }

    fn open_idetail(&mut self) {
        if let Some(e) = self.iselected() {
            self.idetail = Some(e.key);
            self.mode = Mode::Detail;
            self.text_for = None;
            self.isearching = false;
        }
    }

    fn inst_action(&mut self, act: usize, cx: &AppCx) {
        let Some(e) = self.iselected() else {
            if self.on_head.is_some() {
                self.say("Choose an item first (Down).", false);
            }
            return;
        };
        match act {
            ACT_OPEN => match &e.open {
                Some(o) => self.request = Some((o.tile.clone(), o.item.clone())),
                None => self.say(format!("There is nowhere to open {} from your account.", e.title), false),
            },
            ACT_TOGGLE => match self.store.toggle_installed(&cx.services, &cx.session.policy, &cx.session.user, &e) {
                Ok(m) => {
                    cx.audit.record(&cx.session.user, "store.toggle", &e.name);
                    self.say(m, true);
                    self.reload(cx);
                }
                Err(m) => self.say(m, false),
            },
            ACT_REMOVE => {
                if e.removable {
                    self.ask_remove_plan(cx, e);
                } else {
                    let r = self.store.remove_installed(&cx.services, &cx.session.policy, &cx.session.user, &e);
                    self.say(r.err().unwrap_or_default(), false);
                }
            }
            ACT_UPDATE => self.start_update(cx, &e.key),
            manage::ACT_ROLLBACK => self.ask_rollback(),
            manage::ACT_PIN => self.toggle_pin(cx),
            manage::ACT_RENAME => self.edit_alias(),
            manage::ACT_TAGS => self.edit_tags(),
            _ => self.open_move_menu(),
        }
    }

    /// Keys and clicks of the Installed page and of an installed item's page. `None` leaves the event to the shared code.
    fn handle_installed(&mut self, ev: &Event, cx: &AppCx) -> Option<Flow> {
        let detail = self.idetail.is_some();
        if detail && self.text.handle(ev) {
            return Some(Flow::Stay);
        }
        if self.ifilter == Some(IType::Driver) && !detail {
            if let Some(f) = self.handle_drivers(ev, cx) {
                return Some(f);
            }
        }
        match ev {
            Event::Row(i) if !detail => self.press(Press::Item(*i), cx),
            Event::Tab(n) if (manage::TAB_HEAD..manage::TAB_HEAD + 200).contains(n) && !detail => self.press(Press::Head(*n - manage::TAB_HEAD), cx),
            Event::Scroll(d) => {
                if detail { self.text.scroll_by(*d * 3) } else { self.nav_move(-*d) }
            }
            Event::Tab(n) if (220..220 + 1 + IType::ALL.len()).contains(n) && !detail => {
                self.ifilter = if *n == 220 { None } else { IType::ALL.get(*n - 221).copied() };
                self.sel[1] = 0;
                self.on_head = None;
                self.refresh_drivers(cx);
            }
            Event::Tab(manage::TAB_UPDATES) if !detail => {
                self.iupdates = !self.iupdates;
                self.sel[1] = 0;
                self.on_head = None;
            }
            Event::Tab(230) if !detail => self.isort = self.isort.next(),
            Event::Tab(manage::TAB_VIEW) if !detail => {
                let v = self.iview.next();
                self.set_view(v);
            }
            Event::Tab(manage::TAB_TAGS) if !detail => self.mode = Mode::Tags(Box::new(TagsState { sel: 0 })),
            Event::Tab(manage::TAB_UPDATE_ALL) if !detail => self.ask_update_all(),
            Event::Tab(n) if (manage::TAB_TAG_CHIP..manage::TAB_TAG_CHIP + 200).contains(n) && !detail => {
                if let Some((t, _)) = self.org.all_tags().get(*n - manage::TAB_TAG_CHIP).cloned() {
                    self.toggle_tag_filter(&t);
                }
            }
            Event::Tab(n) if (1000..2000).contains(n) => {
                let (ord, act) = ((*n - 1000) / 4, (*n - 1000) % 4);
                self.sel[1] = ord;
                self.on_head = None;
                self.inst_action(act, cx);
            }
            Event::Tab(n) if (240..249).contains(n) && detail => self.inst_action(*n - 240, cx),
            Event::Tab(307) if detail => {
                self.idetail = None;
                self.mode = Mode::Browse;
            }
            Event::Focus(_) | Event::FocusAt(..) if !detail => {
                self.isearching = true;
                if let Event::FocusAt(_, x) = ev {
                    self.iquery.click(*x);
                }
            }
            Event::Paste(t) if self.isearching => {
                self.iquery.paste(t);
                self.sel[1] = 0;
            }
            Event::Key(k) if self.isearching => match k {
                Key::Enter => self.isearching = false,
                Key::Esc => {
                    self.iquery.clear();
                    self.isearching = false;
                }
                Key::Up => self.nav_move(-1),
                Key::Down => self.nav_move(1),
                other => {
                    self.iquery.handle(other);
                    self.sel[1] = 0;
                }
            },
            Event::Key(k) if detail => match k {
                Key::Esc | Key::Left => {
                    self.idetail = None;
                    self.mode = Mode::Browse;
                }
                Key::Up => self.text.scroll_by(1),
                Key::Down => self.text.scroll_by(-1),
                Key::PageUp => self.text.scroll_by(8),
                Key::PageDown => self.text.scroll_by(-8),
                Key::Char('o') => self.inst_action(ACT_OPEN, cx),
                Key::Char('d') => self.inst_action(ACT_TOGGLE, cx),
                Key::Char('r') => self.inst_action(ACT_REMOVE, cx),
                Key::Char('u') => self.inst_action(ACT_UPDATE, cx),
                Key::Char('b') => self.inst_action(manage::ACT_ROLLBACK, cx),
                Key::Char('p') => self.inst_action(manage::ACT_PIN, cx),
                Key::Char('e') => self.inst_action(manage::ACT_RENAME, cx),
                Key::Char('t') => self.inst_action(manage::ACT_TAGS, cx),
                Key::Char('m') => self.inst_action(manage::ACT_MOVE, cx),
                Key::Char('c') => self.start_refresh(cx),
                _ => {}
            },
            Event::Key(k) => match k {
                Key::Esc => {
                    if self.ifilter.is_some() || !self.iquery.value.is_empty() || self.iupdates || !self.itags.is_empty() {
                        self.ifilter = None;
                        self.iquery.clear();
                        self.iupdates = false;
                        self.itags.clear();
                    } else {
                        return Some(Flow::Back);
                    }
                }
                Key::Up => self.nav_move(-1),
                Key::Down => self.nav_move(1),
                Key::PageUp => self.nav_move(-6),
                Key::PageDown => self.nav_move(6),
                Key::Home => self.nav_move(-1000),
                Key::End => self.nav_move(1000),
                Key::Left | Key::Right => {
                    let order: Vec<Option<IType>> = std::iter::once(None).chain(IType::ALL.into_iter().map(Some)).collect();
                    let at = order.iter().position(|t| *t == self.ifilter).unwrap_or(0);
                    self.ifilter = order[if *k == Key::Right { (at + 1) % order.len() } else { (at + order.len() - 1) % order.len() }];
                    self.sel[1] = 0;
                    self.on_head = None;
                    self.refresh_drivers(cx);
                }
                Key::Enter if self.on_head.is_none() => self.open_idetail(),
                Key::Char('/') => self.isearching = true,
                Key::Char('s') => self.isort = self.isort.next(),
                Key::Char('o') => self.inst_action(ACT_OPEN, cx),
                Key::Char('d') => self.inst_action(ACT_TOGGLE, cx),
                Key::Char('r') => self.inst_action(ACT_REMOVE, cx),
                Key::Char('u') => self.inst_action(ACT_UPDATE, cx),
                Key::Char('b') => self.inst_action(manage::ACT_ROLLBACK, cx),
                Key::Char('p') => self.inst_action(manage::ACT_PIN, cx),
                Key::Char('U') => self.ask_update_all(),
                Key::Char('c') => self.start_refresh(cx),
                other => {
                    if !self.org_key(other, cx) {
                        return None;
                    }
                }
            },
            _ => return None,
        }
        Some(Flow::Stay)
    }

    fn inst_row(&mut self, g: &mut Gfx, r: R, s: f32, e: &IEntry, ordinal: usize, selected: bool) {
        let hot = g.hovered(r);
        fx::row_at(g, r.x, r.y, r.w, r.h, 12.0 * s, selected, hot, ACCENT);
        let shown = self.shown_title(e).to_string();
        g.hit(r, Event::Row(ordinal), &format!("{} {}", shown, e.status.label()));
        let life = self.life_of(&e.key).cloned();
        let c = type_color(e.ty);
        let d = r.h - 16.0 * s;
        g.rrect(r.x + 10.0 * s, r.y + 8.0 * s, d, d, d * 0.22, fx::over(fx::WELL, fx::with_alpha(c, 70)));
        g.rrect_outline(r.x + 10.0 * s, r.y + 8.0 * s, d, d, d * 0.22, 1.2, fx::with_alpha(c, 200));
        let letter: String = shown.chars().next().map(|ch| ch.to_uppercase().collect()).unwrap_or_default();
        let lw = g.text_width(d * 0.5, &letter, Face::Bold);
        g.text(r.x + 10.0 * s + (d - lw) / 2.0, r.y + 8.0 * s + (d - g.line_height(d * 0.5, Face::Bold)) / 2.0, d * 0.5, fx::readable(c, fx::WELL, 4.5), &letter, Face::Bold);
        let tx = r.x + 10.0 * s + d + 12.0 * s;
        // the right side: Remove, Turn off/on, Open, Update, then the status and the size
        let mut rx = r.x + r.w - 8.0 * s;
        let bh = 12.0 * 2.1 * s;
        let by = r.y + (r.h - bh) / 2.0;
        let put = |g: &mut Gfx, rx: &mut f32, label: &str, col: [u8; 4], act: usize| {
            let bw = g.text_width(12.0 * s, label, Face::Bold) + 12.0 * 1.7 * s;
            *rx -= bw;
            g.button(*rx, by, 12.0 * s, label, col, Event::Tab(1000 + ordinal * 4 + act));
            *rx -= 6.0 * s;
        };
        if e.removable {
            put(g, &mut rx, "Remove", ERR, ACT_REMOVE);
        }
        if e.can_toggle {
            put(g, &mut rx, if e.status == Status::Off { "Turn on" } else { "Turn off" }, MUTED, ACT_TOGGLE);
        }
        if e.open.is_some() {
            put(g, &mut rx, "Open", ACCENT, ACT_OPEN);
        }
        if life.as_ref().is_some_and(|l| l.can_update()) {
            put(g, &mut rx, "Update", OK, ACT_UPDATE);
        }
        let (st, st_col) = match (&e.status, &life) {
            (Status::Update(v), Some(l)) if l.pinned.is_some() => (format!("Pinned (update {v})"), MUTED),
            (Status::On, Some(l)) => match &l.check {
                iyakki_agent::store::lifecycle::Check::SourceGone(_) => ("Source gone".to_string(), AMBER),
                iyakki_agent::store::lifecycle::Check::Offline(_) => ("Can't check offline".to_string(), MUTED),
                _ => (e.status.label(), status_color(&e.status)),
            },
            _ => (e.status.label(), status_color(&e.status)),
        };
        let sw = g.text_width(12.0 * s, &st, Face::Bold) + 17.0 * s;
        if rx - sw > tx + 220.0 * s {
            rx -= sw;
            fx::chip(g, rx, r.y + (r.h - 22.8 * s) / 2.0, s, &st, st_col, matches!(e.status, Status::Update(_)));
            rx -= 8.0 * s;
        }
        let size = if e.size > 0 { size_text(e.size) } else { String::new() };
        let zw = g.text_width(12.0 * s, &size, Face::Sans);
        if !size.is_empty() && rx - zw > tx + 240.0 * s {
            rx -= zw;
            g.text(rx, r.y + (r.h - g.line_height(12.0 * s, Face::Sans)) / 2.0, 12.0 * s, DIM, &size, Face::Sans);
            rx -= 10.0 * s;
        }
        let room = (rx - tx - 4.0 * s).max(60.0 * s);
        // line 1: name, the badge, the type, the tags
        let mut tip = self.tip.take();
        let title = g.fit(16.0 * s, &shown, Face::Bold, (room - 170.0 * s).max(60.0 * s));
        let tw = g.text_width(16.0 * s, &title, Face::Bold);
        g.text(tx, r.y + 7.0 * s, 16.0 * s, TEXT, &title, Face::Bold);
        let mut bx = tx + tw + 10.0 * s;
        if bx + 150.0 * s < tx + room {
            bx += badge(g, bx, r.y + 6.0 * s, s, e.level, &mut tip) + 6.0 * s;
        }
        let tl = e.ty.one();
        if bx + g.text_width(12.0 * s, tl, Face::Bold) + 17.0 * s < tx + room {
            bx += fx::chip(g, bx, r.y + 6.0 * s, s, tl, c, false) + 6.0 * s;
        }
        if let Some(t) = &e.target {
            let lbl = for_chip(t);
            if bx + g.text_width(12.0 * s, &lbl, Face::Bold) + 17.0 * s < tx + room {
                bx += fx::chip(g, bx, r.y + 6.0 * s, s, &lbl, rgb(240, 120, 220), false) + 6.0 * s;
            }
        }
        for t in self.org.tags_of(&Self::okey(e)).iter().take(3) {
            let lbl = format!("#{t}");
            let w = g.text_width(12.0 * s, &lbl, Face::Bold) + 17.0 * s;
            if bx + w >= tx + room {
                break;
            }
            bx += fx::chip(g, bx, r.y + 6.0 * s, s, &lbl, rgb(150, 200, 255), self.itags.contains(t)) + 6.0 * s;
        }
        self.tip = tip;
        // line 2: the real name when a title of the person's own is shown, version, scope and what it is
        let sub = self.row_sub(e);
        let line = g.fit(12.0 * s, &sub, Face::Sans, room);
        g.text(tx, r.y + r.h - 8.0 * s - g.line_height(12.0 * s, Face::Sans), 12.0 * s, MUTED, &line, Face::Sans);
    }

    fn render_installed(&mut self, g: &mut Gfx, body: R, s: f32, tick: u64) {
        let (x, mut y, w) = (body.x, body.y, body.w);
        self.zones.clear();
        // the type chips with their counts, then Updates
        let counts = inst::counts(&self.inst);
        let lh = 12.0 * s * 1.9;
        let mut cx_ = x;
        let mut labels: Vec<(String, usize, Event, bool)> = std::iter::once(("All".to_string(), 0)).chain(IType::ALL.iter().enumerate().map(|(i, t)| (t.chip().to_string(), i + 1))).enumerate().map(|(i, (name, ci))| {
            let on = if i == 0 { self.ifilter.is_none() } else { self.ifilter == Some(IType::ALL[i - 1]) };
            (format!("{name} {}", counts[ci]), ci, Event::Tab(220 + i), on)
        }).collect();
        let n_up = self.inst.iter().filter(|e| matches!(e.status, Status::Update(_))).count();
        labels.push((format!("Updates {n_up}"), 99, Event::Tab(manage::TAB_UPDATES), self.iupdates));
        for (label, _, ev, on) in &labels {
            let cw = g.text_width(12.0 * s, label, Face::Bold) + 17.0 * s;
            if cx_ + cw > x + w {
                cx_ = x;
                y += lh + 6.0 * s;
            }
            let col = if matches!(ev, Event::Tab(manage::TAB_UPDATES)) && n_up > 0 { OK } else { ACCENT };
            fx::chip(g, cx_, y, s, label, col, *on);
            g.hit(R::new(cx_, y, cw, lh), ev.clone(), label);
            cx_ += cw + 8.0 * s;
        }
        y += lh + 8.0 * s;
        if self.ifilter == Some(IType::Driver) {
            self.drivers.render(g, R::new(x, y, w, (body.y + body.h - y).max(60.0 * s)), s);
            return;
        }
        // the search box, the order, the grouping and the tags
        let sr = R::new(x, y, (w * 0.34).clamp(180.0 * s, 380.0 * s), 36.0 * s);
        self.iquery.render(g, sr, self.isearching && matches!(self.mode, Mode::Browse), s, blink(tick), Face::Sans, SEARCH_ID);
        let mut px = sr.x + sr.w + 12.0 * s;
        let mut py = y + 5.0 * s;
        let controls = [
            (format!("Sort: {} (s)", self.isort.label()), Event::Tab(230)),
            (format!("Group by: {} (v)", self.iview.label()), Event::Tab(manage::TAB_VIEW)),
            ("Tags (T)".to_string(), Event::Tab(manage::TAB_TAGS)),
        ];
        for (label, ev) in controls {
            let cw = g.text_width(12.0 * s, &label, Face::Bold) + 17.0 * s;
            if px + cw > x + w {
                px = x;
                py += lh + 6.0 * s;
                y += lh + 6.0 * s;
            }
            fx::chip(g, px, py, s, &label, ACCENT, false);
            g.hit(R::new(px, py, cw, lh), ev, &label);
            px += cw + 8.0 * s;
        }
        y += 36.0 * s + 8.0 * s;
        // the person's tags as filters (several narrow the list down)
        let tags = self.org.all_tags();
        if !tags.is_empty() {
            let mut tx = x;
            for (i, (t, n)) in tags.iter().take(12).enumerate() {
                let label = format!("#{t} {n}");
                let cw = g.text_width(12.0 * s, &label, Face::Bold) + 17.0 * s;
                if tx + cw > x + w {
                    break;
                }
                fx::chip(g, tx, y, s, &label, rgb(150, 200, 255), self.itags.contains(t));
                g.hit(R::new(tx, y, cw, lh), Event::Tab(manage::TAB_TAG_CHIP + i), &format!("tag {t}"));
                tx += cw + 6.0 * s;
            }
            y += lh + 8.0 * s;
        }
        let rows = self.display();
        let nitems = rows.iter().filter(|r| matches!(r, manage::DRow::Item(_))).count();
        if nitems == 0 && !rows.iter().any(|r| matches!(r, manage::DRow::Head(_))) {
            fx::empty_state(g, R::new(x, y, w, (body.y + body.h - y).max(120.0 * s)), s, Icon::Store, "Nothing installed matches", "Clear the search with Esc, or choose All. Skills, servers, tools and extensions you add show up here.", ACCENT);
            return;
        }
        self.sel[1] = self.sel[1].min(nitems.saturating_sub(1));
        let (rh, hh, gap) = (58.0 * s, 32.0 * s, 4.0 * s);
        let heights: Vec<f32> = rows.iter().map(|r| if matches!(r, manage::DRow::Head(_)) { hh } else { rh }).collect();
        // the row under the cursor
        let mut ord = 0;
        let mut cur = 0;
        for (i, r) in rows.iter().enumerate() {
            match r {
                manage::DRow::Head(h) if self.on_head.as_ref() == Some(&h.id) => cur = i,
                manage::DRow::Item(_) => {
                    if self.on_head.is_none() && ord == self.sel[1] {
                        cur = i;
                    }
                    ord += 1;
                }
                _ => {}
            }
        }
        let avail = (body.y + body.h - y).max(rh);
        let mut first = self.first[1].min(rows.len().saturating_sub(1));
        if cur < first {
            first = cur;
        }
        let span = |a: usize, b: usize| -> f32 { (a..=b).map(|i| heights[i] + gap).sum::<f32>() - gap };
        while first < cur && span(first, cur) > avail {
            first += 1;
        }
        self.first[1] = first;
        let dragging = self.drag.as_ref().is_some_and(|d| d.active);
        let drop_at = if dragging { self.pointer } else { None };
        let mut ord = rows.iter().take(first).filter(|r| matches!(r, manage::DRow::Item(_))).count();
        let mut head_idx = rows.iter().take(first).filter(|r| matches!(r, manage::DRow::Head(_))).count();
        let mut group_of_row: Option<Option<String>> = None;
        for r in rows.iter().take(first) {
            if let manage::DRow::Head(h) = r {
                group_of_row = Some(if h.kind == manage::HeadKind::Group { h.id.clone() } else { None });
            }
        }
        let mut shown = 0;
        for (i, row) in rows.iter().enumerate().skip(first) {
            let h = heights[i];
            if y + h > body.y + body.h + 0.5 && shown > 0 {
                break;
            }
            let rect = R::new(x, y, w, h);
            match row {
                manage::DRow::Head(hd) => {
                    let cursor = self.on_head.as_ref() == Some(&hd.id) && self.iview == View::Groups;
                    let dropping = drop_at.is_some_and(|(px, py)| rect.contains(px, py)) && self.iview == View::Groups && hd.kind != manage::HeadKind::Plain;
                    manage::render_head(g, rect, s, hd, head_idx, cursor, dropping, dragging);
                    let grp = if hd.kind == manage::HeadKind::Group { hd.id.clone() } else { None };
                    group_of_row = Some(grp.clone());
                    if self.iview == View::Groups {
                        self.zones.push(Zone { rect, what: manage::ZoneKind::Head { group: grp, kind: hd.kind } });
                    }
                    head_idx += 1;
                }
                manage::DRow::Item(k) => {
                    if let Some(e) = self.inst.iter().find(|e| e.key == *k).cloned() {
                        let inset = if self.iview == View::Groups { 16.0 * s } else { 0.0 };
                        let rr = R::new(x + inset, y, w - inset, h);
                        self.inst_row(g, rr, s, &e, ord, ord == self.sel[1] && self.on_head.is_none());
                        if self.iview == View::Groups {
                            self.zones.push(Zone { rect: rr, what: manage::ZoneKind::Item { key: k.clone(), group: group_of_row.clone().unwrap_or(None) } });
                            if drop_at.is_some_and(|(px, py)| rr.contains(px, py)) {
                                let above = (drop_at.map_or(0, |p| p.1) as f32) < rr.y + rr.h / 2.0;
                                let ly = if above { rr.y - 2.0 * s } else { rr.y + rr.h + 1.0 * s };
                                g.rect(rr.x, ly, rr.w, 3.0 * s, ACCENT);
                            }
                        }
                    }
                    ord += 1;
                }
            }
            y += h + gap;
            shown += 1;
        }
        if rows.len() > shown {
            fx::scrollbar(g, R::new(x + w - 6.0 * s, body.y, 4.0 * s, body.h), s, first, shown, rows.len(), ACCENT);
        }
        if let (Some(d), Some(at)) = (&self.drag, self.pointer) {
            if d.active {
                manage::render_ghost(g, s, at, &d.label);
            }
        }
    }

    fn rebuild_itext(&mut self, e: &IEntry, cx: &AppCx) {
        let sig = self.life_of(&e.key).map(|l| format!("{:?}|{:?}|{:?}|{}|{}", l.check, l.pinned, l.previous, l.history.len(), l.latest.clone().unwrap_or_default())).unwrap_or_default();
        let key = (e.kind.clone(), e.key.clone(), format!("{}|{}|{}", e.version, sig, self.org_rev));
        if self.text_for.as_ref() == Some(&key) {
            return;
        }
        self.text_for = Some(key);
        self.text.clear();
        let push = |t: &mut ScrollLog, s: &str, c: [u8; 4]| t.push(s.to_string(), c);
        if !e.summary.is_empty() {
            push(&mut self.text, &e.summary, TEXT);
            push(&mut self.text, "", TEXT);
        }
        // what it is, whatever it is called here
        push(&mut self.text, "WHAT IT IS", ACCENT);
        push(&mut self.text, &format!("Real name: {} · id {}:{}", e.title, e.kind, e.name), TEXT);
        if !e.publisher.is_empty() {
            push(&mut self.text, &format!("Publisher: {}", e.publisher), TEXT);
        }
        let ok = Self::okey(e);
        let mine = [
            ("Your title", self.org.alias_of(&ok).unwrap_or("").to_string()),
            ("Your tags", self.org.tags_of(&ok).join(", ")),
            ("Your group", self.org.group_of(&ok).map(|g| g.name.clone()).unwrap_or_default()),
        ];
        if mine.iter().any(|m| !m.1.is_empty()) {
            for (k, v) in mine.iter().filter(|m| !m.1.is_empty()) {
                push(&mut self.text, &format!("{k}: {v}"), MUTED);
            }
        }
        push(&mut self.text, "", TEXT);
        if let Some(l) = self.life_of(&e.key).cloned() {
            push(&mut self.text, "VERSION AND UPDATES", ACCENT);
            push(&mut self.text, &format!("Installed: {}", if e.version == "-" || e.version.is_empty() { "no version number".to_string() } else { e.version.clone() }), TEXT);
            push(&mut self.text, &format!("From: {}", if l.source_name.is_empty() { e.source.clone() } else { l.source_name.clone() }), TEXT);
            match &l.check {
                iyakki_agent::store::lifecycle::Check::Update => {
                    push(&mut self.text, &format!("Latest: {}  (update available)", l.latest.clone().unwrap_or_default()), OK);
                    if !l.changelog.is_empty() {
                        push(&mut self.text, &format!("What is new: {}", l.changelog), TEXT);
                    }
                    if !l.added_permissions.is_empty() {
                        push(&mut self.text, "The newer version lists permissions this one does not have (you will see them before anything changes):", AMBER);
                        for p in &l.added_permissions {
                            push(&mut self.text, &format!("• {p}"), AMBER);
                        }
                    }
                    if let Some(o) = &l.from_other {
                        push(&mut self.text, &format!("It comes from {o}, not the store that installed it (same publisher key): you will be asked to confirm."), AMBER);
                    }
                }
                c => {
                    if let Some(v) = &l.latest {
                        push(&mut self.text, &format!("Latest: {v}"), TEXT);
                    }
                    push(&mut self.text, &format!("Status: {} {}", c.label(), c.note()), if matches!(c, iyakki_agent::store::lifecycle::Check::UpToDate) { OK } else { MUTED });
                }
            }
            if let Some(p) = &l.pinned {
                push(&mut self.text, &format!("Pinned at {} ({}): updates skip it. Press p to unpin.", p.version, p.why), AMBER);
            }
            match &l.previous {
                Some(v) => push(&mut self.text, &format!("Previous version kept: {v}. Press b to go back to it."), TEXT),
                None => push(&mut self.text, "No earlier version is kept to go back to.", MUTED),
            }
            if !l.history.is_empty() {
                push(&mut self.text, "", TEXT);
                push(&mut self.text, "VERSION HISTORY", ACCENT);
                for h in l.history.iter().rev() {
                    push(&mut self.text, &format!("{} · {}", h.version, h.action), MUTED);
                }
            }
            let plan = self.store.plan_remove(&cx.services, &cx.session.user, &self.lives, &l);
            push(&mut self.text, "", TEXT);
            push(&mut self.text, "DATA AND USE", ACCENT);
            if plan.data.is_empty() && plan.vault.is_empty() {
                push(&mut self.text, "It keeps no data of its own.", MUTED);
            }
            for (n, b) in &plan.data {
                push(&mut self.text, &format!("Data folder: {n} ({})", size_text(*b)), TEXT);
            }
            for v in &plan.vault {
                push(&mut self.text, &format!("Vault entry named after it: {v}"), TEXT);
            }
            if plan.cached > 0 {
                push(&mut self.text, &format!("Kept package copies: {}", size_text(plan.cached)), MUTED);
            }
            if plan.dependents.is_empty() {
                push(&mut self.text, "Nothing installed depends on it.", MUTED);
            } else {
                push(&mut self.text, &format!("Used by: {}", plan.dependents.join(", ")), AMBER);
            }
            push(&mut self.text, "", TEXT);
        }
        push(&mut self.text, "WHAT IT CAN DO", ACCENT);
        if e.permissions.is_empty() {
            push(&mut self.text, if e.ty == IType::App { "It is part of the system, under your account's permissions." } else { "No extra access is listed." }, MUTED);
        }
        for p in &e.permissions {
            push(&mut self.text, &format!("• {p}"), AMBER);
        }
        push(&mut self.text, "", TEXT);
        push(&mut self.text, "DETAILS", ACCENT);
        let scope = if e.ty == IType::App { "the whole system".to_string() } else if e.scope == iyakki_agent::skills::Scope::Shared { "everyone".into() } else { "only you".into() };
        for (k, v) in [
            ("Type", e.ty.one().to_string()),
            ("Licence", if e.license.is_empty() { String::new() } else { format!("{}{}", e.license, if permissive(&e.license) { " (permissive)" } else { "" }) }),
            ("Trust", format!("{}: {}", e.level.label(), e.trust_label)),
            ("For", scope),
            ("Extends", e.target.as_deref().map(|t| format!("{} ({})", if t == "any" { "any tile".to_string() } else { t.to_uppercase() }, e.ext_kind.clone().unwrap_or_default())).unwrap_or_default()),
            ("Source", e.source.clone()),
            ("Categories", e.categories.join(", ")),
            ("Size", size_text(e.size)),
            ("Status", e.status.label()),
        ] {
            if !v.is_empty() {
                push(&mut self.text, &format!("{k}: {v}"), MUTED);
            }
        }
        if !e.files.is_empty() {
            push(&mut self.text, "", TEXT);
            push(&mut self.text, "FILES", ACCENT);
            for f in &e.files {
                push(&mut self.text, f, MUTED);
            }
        }
        self.text.scroll = usize::MAX / 4;
    }

    fn render_inst_detail(&mut self, g: &mut Gfx, body: R, s: f32, cx: &AppCx) {
        let Some(e) = self.iselected() else {
            self.idetail = None;
            self.mode = Mode::Browse;
            return;
        };
        self.rebuild_itext(&e, cx);
        let life = self.life_of(&e.key).cloned();
        let shown = self.shown_title(&e).to_string();
        let (x, w) = (body.x, body.w);
        let mut y = body.y;
        g.button(x, y, 13.0 * s, "Back (Esc)", MUTED, Event::Tab(307));
        y += 40.0 * s;
        let d = 64.0 * s;
        let c = type_color(e.ty);
        g.rrect(x, y, d, d, d * 0.22, fx::over(fx::WELL, fx::with_alpha(c, 70)));
        g.rrect_outline(x, y, d, d, d * 0.22, 1.2, fx::with_alpha(c, 200));
        let letter: String = shown.chars().next().map(|ch| ch.to_uppercase().collect()).unwrap_or_default();
        let lw = g.text_width(d * 0.5, &letter, Face::Bold);
        g.text(x + (d - lw) / 2.0, y + (d - g.line_height(d * 0.5, Face::Bold)) / 2.0, d * 0.5, fx::readable(c, fx::WELL, 4.5), &letter, Face::Bold);
        let tx = x + d + 16.0 * s;
        let title = g.fit(24.0 * s, &shown, Face::Bold, w - d - 16.0 * s);
        g.text(tx, y, 24.0 * s, TEXT, &title, Face::Bold);
        let real = if shown != e.title { format!("“{}” · ", e.title) } else { String::new() };
        let facts = [format!("{real}{}", if e.publisher.is_empty() { String::new() } else { format!("by {}", e.publisher) }), if e.version == "-" { String::new() } else { format!("version {}", e.version) }, size_text(e.size), e.license.clone()].into_iter().filter(|t| !t.is_empty() && t != "“” · ").collect::<Vec<_>>().join("  ·  ");
        let facts = g.fit(13.0 * s, &facts, Face::Sans, w - d - 16.0 * s);
        g.text(tx, y + 32.0 * s, 13.0 * s, MUTED, &facts, Face::Sans);
        let mut tip = self.tip.take();
        let mut bx = tx;
        bx += badge(g, bx, y + 54.0 * s, s, e.level, &mut tip) + 8.0 * s;
        self.tip = tip;
        bx += fx::chip(g, bx, y + 54.0 * s, s, e.ty.one(), c, false) + 8.0 * s;
        if let Some(t) = &e.target {
            bx += fx::chip(g, bx, y + 54.0 * s, s, &for_chip(t), rgb(240, 120, 220), false) + 8.0 * s;
        }
        let chip_text = match &life {
            Some(l) if matches!(e.status, Status::On) => l.check.label(),
            _ => e.status.label(),
        };
        fx::chip(g, bx, y + 54.0 * s, s, &chip_text, status_color(&e.status), matches!(e.status, Status::Update(_)));
        y += d + 16.0 * s;
        let plain = g.fit(12.0 * s, &format!("{}: {}", e.level.label(), e.level.plain()), Face::Sans, w);
        g.text(x, y, 12.0 * s, level_color(e.level), &plain, Face::Sans);
        y += 22.0 * s;
        // the buttons wrap onto more lines when the window is narrow
        let bh = 14.0 * 2.1 * s;
        let mut bx = x;
        let mut add = |g: &mut Gfx, y: &mut f32, label: &str, col: [u8; 4], ev: usize| {
            let bw = g.text_width(14.0 * s, label, Face::Bold) + 14.0 * 1.7 * s;
            if bx + bw > x + w && bx > x {
                bx = x;
                *y += bh + 6.0 * s;
            }
            g.button(bx, *y, 14.0 * s, label, col, Event::Tab(ev));
            bx += bw + 8.0 * s;
        };
        if e.open.is_some() {
            add(g, &mut y, "Open (o)", ACCENT, 240);
        }
        if let Status::Update(v) = &e.status {
            if life.as_ref().is_some_and(|l| l.pinned.is_none()) || life.is_none() {
                add(g, &mut y, &format!("Update to {v} (u)"), OK, 243);
            }
        } else if e.ty == IType::Cli && life.as_ref().is_some_and(|l| l.entry.ty == IType::Cli) {
            add(g, &mut y, "Update to the newest (u)", ACCENT, 243);
        }
        if life.as_ref().is_some_and(|l| l.can_rollback()) {
            add(g, &mut y, &format!("Go back to {} (b)", life.as_ref().and_then(|l| l.previous.clone()).unwrap_or_default()), AMBER, 244);
        }
        if let Some(l) = &life {
            if l.pinned.is_some() {
                add(g, &mut y, "Unpin (p)", MUTED, 245);
            } else if l.can_pin() {
                add(g, &mut y, "Pin this version (p)", MUTED, 245);
            }
        }
        if e.can_toggle {
            add(g, &mut y, if e.status == Status::Off { "Turn on (d)" } else { "Turn off (d)" }, MUTED, 241);
        }
        add(g, &mut y, "Your title (e)", MUTED, 246);
        add(g, &mut y, "Tags (t)", MUTED, 247);
        add(g, &mut y, "Move to a group (m)", MUTED, 248);
        if e.removable {
            add(g, &mut y, "Remove (r)", ERR, 242);
        }
        y += bh + 10.0 * s;
        let r = R::new(x, y, w, (body.y + body.h - y).max(40.0 * s));
        fx::panel(g, r, s, ACCENT);
        let inner = R::new(r.x + 16.0 * s, r.y + 10.0 * s, r.w - 32.0 * s, (r.h - 20.0 * s).max(16.0 * s));
        self.text.render(g, inner, 13.0 * s, Face::Sans);
    }
}

#[cfg(test)]
mod tests {
    mod manage_tests;

    use super::*;
    use crate::apps::tests::{cx_with_admin_store, services_in, temp_dir, Offline};
    use iyakki_agent::pkg::{new_signing_key, public_key_hex};
    use iyakki_agent::store::catalog::{sha256_hex, Catalog, Entry};
    use iyakki_agent::store::manifest::{Meta, PriceModel, Pricing, Publisher};
    use iyakki_sec::policy::Role;
    use std::collections::HashMap;

    pub(super) fn cx(tag: &str, role: Role) -> AppCx {
        let mut cx = cx_with_admin_store(tag);
        let d = temp_dir(&format!("{tag}-svc"));
        cx.services = services_in(&d, Arc::new(Offline));
        cx.session.role = role;
        cx.session.policy = role.template();
        if role != Role::Admin {
            cx.session.user = "alice".into();
        }
        cx
    }

    /// An address book that answers from a table and remembers which thread asked.
    #[derive(Default)]
    struct Table {
        files: Mutex<HashMap<String, Vec<u8>>>,
        threads: Mutex<Vec<std::thread::ThreadId>>,
    }

    impl Fetch for Table {
        fn get(&self, url: &str, _max: usize) -> Result<Vec<u8>, String> {
            self.threads.lock().unwrap().push(std::thread::current().id());
            self.files.lock().unwrap().get(url).cloned().ok_or_else(|| format!("could not reach {url}: offline"))
        }
    }

    fn key(a: &mut StoreApp, cx: &AppCx, k: Key) {
        a.handle(&Event::Key(k), cx);
    }

    fn typed(a: &mut StoreApp, cx: &AppCx, s: &str) {
        for c in s.chars() {
            key(a, cx, Key::Char(c));
        }
    }

    fn draw(a: &mut StoreApp, cx: &AppCx, w: u32, h: u32) -> Gfx {
        let s = crate::shell::scale_for(w, h);
        let mut g = Gfx::new(w, h);
        g.clear(rgb(9, 11, 20));
        let hh = 46.0 * s;
        a.render(&mut g, R::new(0.0, hh, w as f32, h as f32 - hh), s, cx, 3);
        g
    }

    const SIZES: [(u32, u32); 4] = [(1280, 720), (1920, 1080), (1024, 600), (800, 480)];

    fn wait_job(a: &mut StoreApp, cx: &AppCx) {
        for _ in 0..300 {
            a.apply_job(cx);
            if a.job.is_none() {
                return;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        panic!("the job did not finish");
    }

    /// Open the page of the app with this id (from the full list).
    fn open_page(a: &mut StoreApp, cx: &AppCx, id: &str) {
        a.switch(Section::Discover);
        a.filter = Filter::default();
        a.query.clear();
        let ordinal = a.discover_rows().iter().filter_map(|r| if let Row::Item(i) = r { Some(*i) } else { None }).collect::<Vec<_>>().iter().rposition(|i| a.items[*i].entry.id == id).unwrap_or_else(|| panic!("{id} is listed"));
        a.sel[0] = ordinal;
        key(a, cx, Key::Enter);
        assert!(matches!(a.mode, Mode::Detail) && a.current().unwrap().entry.id == id, "{id} opens its page");
    }

    /// Show the catalog (Discover and Publishers): off by default.
    fn cat_on(a: &mut StoreApp, cx: &AppCx) {
        a.store.set_catalog_enabled(&Actor::system(), true).unwrap();
        a.reload(cx);
        a.switch(Section::Discover);
    }

    fn all_inside(g: &Gfx, w: u32, h: u32, what: &str) {
        for hit in &g.hits {
            let r = hit.rect;
            assert!(r.x >= -1.0 && r.y >= -1.0 && r.x + r.w <= w as f32 + 1.0 && r.y + r.h <= h as f32 + 1.0, "{what} at {w}x{h}: {} sticks out {r:?}", hit.label);
        }
    }

    fn no_overlap(g: &Gfx, w: u32, h: u32, pred: impl Fn(&Event) -> bool, what: &str) {
        let rs: Vec<R> = g.hits.iter().filter(|h| pred(&h.ev)).map(|h| h.rect).collect();
        for (i, a) in rs.iter().enumerate() {
            for b in &rs[i + 1..] {
                let apart = a.y + a.h <= b.y + 0.5 || b.y + b.h <= a.y + 0.5 || a.x + a.w <= b.x + 0.5 || b.x + b.w <= a.x + 0.5;
                assert!(apart, "{what} at {w}x{h}: {a:?} overlaps {b:?}");
            }
        }
    }

    fn signed_skill(name: &str, version: &str, k: &iyakki_agent::pkg::SigningKey, meta: &Meta) -> String {
        let md = format!("---\nname: {name}\ndescription: {name} helper\ntriggers: {name}\n---\nDo the {name} thing.\n");
        let t = iyakki_agent::pkg::build_skill(&md, version, "MIT", Some(k)).unwrap();
        iyakki_agent::pkg::resign(&t, Some(&meta.to_json()), None, k).unwrap()
    }

    fn pub_meta(title: &str, pricing: Pricing) -> Meta {
        Meta { title: title.into(), summary: format!("{title} for the tests"), publisher: Publisher { id: "acme".into(), name: "Acme Ltd".into(), url: "https://acme.example".into() }, pricing, categories: vec!["work".into()], ..Default::default() }
    }

    /// A folder catalog (like a USB stick) holding the given packages.
    fn stick(tag: &str, pkgs: &[(&str, &str, &Meta, &iyakki_agent::pkg::SigningKey)]) -> std::path::PathBuf {
        let d = temp_dir(tag).join("iyakki-store");
        std::fs::create_dir_all(d.join("packages")).unwrap();
        let mut entries = vec![];
        for (name, version, meta, k) in pkgs {
            let text = signed_skill(name, version, k, meta);
            std::fs::write(d.join(format!("packages/{name}.iyp")), &text).unwrap();
            entries.push(Entry { id: name.to_string(), kind: "skill".into(), name: name.to_string(), version: version.to_string(), license: "MIT".into(), size: text.len() as u64, sha256: sha256_hex(text.as_bytes()), url: format!("packages/{name}.iyp"), signer: public_key_hex(k), added: 1_800_000_000, meta: (*meta).clone(), ..Default::default() });
        }
        let c = Catalog { v: 1, id: "usb-stick".into(), name: "Test stick".into(), serial: 1, entries, ..Default::default() };
        std::fs::write(d.join("catalog.json"), c.to_json()).unwrap();
        d
    }

    fn add_folder(a: &mut StoreApp, cx: &AppCx, dir: &std::path::Path) {
        a.switch(Section::Settings);
        key(a, cx, Key::Char('f'));
        assert!(matches!(a.mode, Mode::Form(..)));
        typed(a, cx, "Stick");
        key(a, cx, Key::Tab);
        for _ in 0..8 {
            key(a, cx, Key::Backspace);
        }
        typed(a, cx, &dir.display().to_string());
        key(a, cx, Key::Enter);
        assert!(matches!(a.mode, Mode::Browse), "{:?}", a.status_text());
    }

    #[test]
    fn rows_and_filter_chips_never_overlap_each_other() {
        let cx = cx("st-rows", Role::Admin);
        let mut a = StoreApp::new(&cx);
        cat_on(&mut a, &cx);
        for (w, h) in SIZES {
            let g = draw(&mut a, &cx, w, h);
            assert!(g.hits.iter().filter(|h| matches!(h.ev, Event::Row(_))).count() >= 2, "{w}x{h}");
            no_overlap(&g, w, h, |e| matches!(e, Event::Row(_)), "rows");
            no_overlap(&g, w, h, |e| matches!(e, Event::Row(_) | Event::Tab(200..=204) | Event::Focus(_)), "rows, chips and the search box");
            a.filter.category = Some("languages".into());
            a.filter.level = Some(Level::Official);
            let g = draw(&mut a, &cx, w, h);
            no_overlap(&g, w, h, |e| matches!(e, Event::Row(_) | Event::Tab(200..=204) | Event::Focus(_)), "rows, chips and the search box when filtered");
            a.filter = Filter::default();
        }
    }

    #[test]
    fn search_filters_and_keys_narrow_the_list_and_escape_clears_them() {
        let cx = cx("st-search", Role::Admin);
        let mut a = StoreApp::new(&cx);
        cat_on(&mut a, &cx);
        let all = a.filtered().len();
        key(&mut a, &cx, Key::Char('/'));
        assert!(a.searching);
        typed(&mut a, &cx, "rust");
        assert_eq!(a.items[a.filtered()[0]].entry.id, "lang-rust", "the best match comes first");
        assert!(a.filtered().len() < all / 4);
        draw(&mut a, &cx, 1280, 720);
        key(&mut a, &cx, Key::Enter);
        assert!(!a.searching && a.filtering());
        key(&mut a, &cx, Key::Esc);
        assert!(!a.filtering() && a.filtered().len() == all, "Esc clears the search");
        key(&mut a, &cx, Key::Char('f'));
        assert_eq!(a.filter.price, PriceFilter::Free);
        key(&mut a, &cx, Key::Char('f'));
        assert_eq!(a.filter.price, PriceFilter::Paid);
        assert_eq!(a.filtered().len(), 3, "the three made-up paid apps");
        a.handle(&Event::Tab(200), &cx);
        assert_eq!(a.filter.price, PriceFilter::All);
        key(&mut a, &cx, Key::Char('t'));
        assert_eq!(a.filter.level, Some(Level::Official));
        assert!(a.filtered().iter().all(|i| a.items[*i].verdict.level == Level::Official));
        for _ in 0..3 {
            key(&mut a, &cx, Key::Char('t'));
        }
        assert_eq!(a.filter.level, Some(Level::Community));
        assert_eq!(a.filtered().len(), 3);
        draw(&mut a, &cx, 800, 480);
        key(&mut a, &cx, Key::Esc);
        key(&mut a, &cx, Key::Char('k'));
        assert_eq!(a.filter.kind.as_deref(), Some("skill"));
        assert!(a.filtered().iter().all(|i| a.items[*i].entry.kind == "skill"));
        key(&mut a, &cx, Key::Char('g'));
        assert!(a.filter.category.is_some());
        a.handle(&Event::Tab(204), &cx);
        assert!(!a.filtering());
        key(&mut a, &cx, Key::Char('/'));
        typed(&mut a, &cx, "qqqqq");
        let g = draw(&mut a, &cx, 1280, 720);
        assert!(!g.hits.iter().any(|h| matches!(h.ev, Event::Row(_))), "an empty result shows the empty state");
        assert!(a.help().len() > 5 && a.hint().contains("search"));
    }

    #[test]
    fn the_caret_belongs_to_one_field_at_a_time_and_typing_goes_to_the_owner() {
        let cx = cx("st-caret", Role::Admin);
        let mut a = StoreApp::new(&cx);
        cat_on(&mut a, &cx);
        key(&mut a, &cx, Key::Char('/'));
        assert!(a.searching);
        a.switch(Section::Settings);
        assert!(!a.searching, "leaving the page gives the keyboard back");
        key(&mut a, &cx, Key::Char('a'));
        assert!(matches!(a.mode, Mode::Form(..)) && !a.searching);
        typed(&mut a, &cx, "xx");
        assert!(a.query.value.is_empty(), "typing goes to the dialog, not the search box");
        key(&mut a, &cx, Key::Esc);
        assert!(matches!(a.mode, Mode::Browse));
        // a click on the box claims the keyboard, the review dialog releases it
        a.switch(Section::Discover);
        a.handle(&Event::FocusAt(SEARCH_ID, 4.0), &cx);
        assert!(a.searching);
        open_page(&mut a, &cx, "lang-rust");
        assert!(!a.searching, "an app's page has no search box");
    }

    #[test]
    fn an_app_page_shows_the_badge_publisher_price_permissions_and_the_right_buttons() {
        let cx = cx("st-detail", Role::Admin);
        let mut a = StoreApp::new(&cx);
        cat_on(&mut a, &cx);
        open_page(&mut a, &cx, "lang-rust");
        for (w, h) in SIZES {
            let g = draw(&mut a, &cx, w, h);
            let labels: Vec<&str> = g.hits.iter().map(|h| h.label.as_str()).collect();
            assert!(labels.contains(&"Install (i)") && labels.contains(&"Back (Esc)"), "{w}x{h}: {labels:?}");
            assert!(labels.iter().any(|l| l.starts_with("Official: Made by IYAKKI")));
            all_inside(&g, w, h, "an app page");
            no_overlap(&g, w, h, |e| matches!(e, Event::Tab(300..=307)), "the buttons");
        }
        assert!(a.text.lines.iter().any(|(l, _)| l.contains("WHAT IT CAN DO")) && a.text.lines.iter().any(|(l, _)| l.contains("Licence: Apache-2.0 (permissive)")));
        // hovering the badge shows what it means in plain words
        let s = crate::shell::scale_for(1280, 720);
        let hit = draw(&mut a, &cx, 1280, 720).hits.iter().find(|h| h.label.starts_with("Official")).unwrap().rect;
        let mut g = Gfx::new(1280, 720);
        g.mouse = ((hit.x + hit.w / 2.0) as i32, (hit.y + hit.h / 2.0) as i32);
        g.clear(rgb(9, 11, 20));
        a.render(&mut g, R::new(0.0, 46.0 * s, 1280.0, 720.0 - 46.0 * s), s, &cx, 3);
        assert!(a.tip.as_ref().is_some_and(|t| t.2.contains("signed with its release key")), "{:?}", a.tip);
        key(&mut a, &cx, Key::Esc);
        assert!(matches!(a.mode, Mode::Browse) && a.detail.is_none());
        // a made-up paid app: the price is shown, Buy is a button that does nothing and says so
        open_page(&mut a, &cx, "demo-meeting-suite");
        let g = draw(&mut a, &cx, 1280, 720);
        assert!(!g.hits.iter().any(|h| h.label == "Buy (coming soon)"), "no click region for a button that does nothing");
        assert!(a.text.lines.iter().any(|(l, _)| l.contains("not a real product")));
        key(&mut a, &cx, Key::Char('b'));
        assert!(a.status_text().unwrap().contains("not available yet"));
        key(&mut a, &cx, Key::Char('i'));
        assert!(a.status_text().unwrap().contains("made-up example"));
        assert!(matches!(a.mode, Mode::Detail), "no review for a demo");
        assert_eq!(a.current().unwrap().entry.meta.pricing.label(), "USD 3.00 / month");
    }

    #[test]
    fn installing_shows_the_review_then_installs_on_confirmation_and_the_page_follows() {
        let cx = cx("st-install", Role::Admin);
        let mut a = StoreApp::new(&cx);
        cat_on(&mut a, &cx);
        open_page(&mut a, &cx, "lang-go");
        key(&mut a, &cx, Key::Char('i'));
        let Mode::Review(rv) = &a.mode else { panic!("the review opens: {:?}", a.status_text()) };
        assert!(rv.review.can_install() && rv.review.verdict.level == Level::Official);
        for (w, h) in SIZES {
            let g = draw(&mut a, &cx, w, h);
            let labels: Vec<&str> = g.hits.iter().map(|h| h.label.as_str()).collect();
            assert!(labels.contains(&"Install (y)") && labels.contains(&"Cancel (Esc)"), "{w}x{h}: {labels:?}");
            all_inside(&g, w, h, "the review");
        }
        key(&mut a, &cx, Key::Esc);
        assert!(matches!(a.mode, Mode::Detail));
        assert!(cx.services.skills.get(&iyakki_agent::skills::Scope::User("root".into()), "lang-go").is_none(), "Cancel installs nothing");
        key(&mut a, &cx, Key::Char('i'));
        key(&mut a, &cx, Key::Char('y'));
        assert!(a.status_text().unwrap().starts_with("Installed"), "{:?}", a.status_text());
        assert_eq!(a.current().unwrap().state, State::Installed);
        assert!(cx.services.skills.get(&iyakki_agent::skills::Scope::User("root".into()), "lang-go").is_some());
        // Installed lists it; it can be turned off and on, and removed after asking
        a.switch(Section::Installed);
        draw(&mut a, &cx, 1280, 720);
        assert!(a.inst.iter().any(|e| e.ty == IType::Skill && e.name == "lang-go"));
        a.switch(Section::Discover);
        open_page(&mut a, &cx, "lang-go");
        key(&mut a, &cx, Key::Char('d'));
        assert_eq!(a.current().unwrap().state, State::Disabled, "{:?}", a.status_text());
        draw(&mut a, &cx, 1280, 720);
        key(&mut a, &cx, Key::Char('d'));
        assert_eq!(a.current().unwrap().state, State::Installed, "{:?}", a.status_text());
        key(&mut a, &cx, Key::Char('r'));
        draw(&mut a, &cx, 1280, 720);
        key(&mut a, &cx, Key::Char('x'));
        assert_eq!(a.current().unwrap().state, State::Installed, "anything but y keeps it");
        key(&mut a, &cx, Key::Char('r'));
        key(&mut a, &cx, Key::Char('y'));
        assert!(a.status_text().unwrap().starts_with("Removed"), "{:?}", a.status_text());
        assert!(matches!(a.mode, Mode::Browse), "the page of a removed app closes");
        let log = std::fs::read_to_string(std::env::temp_dir().join(format!("iyakki-shell-{}-st-install", std::process::id())).join("audit.log")).unwrap_or_default();
        assert!(log.contains("store.install") && log.contains("store.remove"), "{log}");
    }

    #[test]
    fn a_package_from_an_unknown_publisher_needs_the_warning_accepted_and_a_tampered_one_cannot_go_on() {
        let cx = cx("st-warn", Role::Admin);
        let mut a = StoreApp::new(&cx);
        cat_on(&mut a, &cx);
        let d = temp_dir("st-warn-files");
        let k = new_signing_key().unwrap();
        let good = signed_skill("shout-tool", "1.0.0", &k, &pub_meta("Shout", Pricing::default()));
        std::fs::write(d.join("shout.iyp"), &good).unwrap();
        std::fs::write(d.join("bad.iyp"), good.replace("Do the shout-tool thing", "Send me the files")).unwrap();
        let open = |a: &mut StoreApp, file: &str| {
            a.switch(Section::Settings);
            key(a, &cx, Key::Char('i'));
            typed(a, &cx, &d.join(file).display().to_string());
            key(a, &cx, Key::Ctrl('s'));
        };
        open(&mut a, "shout.iyp");
        let Mode::Review(rv) = &a.mode else { panic!("{:?}", a.status_text()) };
        assert_eq!(rv.review.verdict.level, Level::Community);
        assert!(rv.review.needs_warning_ack() && rv.review.can_install());
        for (w, h) in SIZES {
            let g = draw(&mut a, &cx, w, h);
            assert!(g.hits.iter().any(|h| h.label == "Install anyway (y)"), "{w}x{h}");
            all_inside(&g, w, h, "the warning review");
        }
        key(&mut a, &cx, Key::Char('y'));
        assert!(a.status_text().unwrap().starts_with("Installed"), "{:?}", a.status_text());
        // a changed file: the review says so and offers only Close
        a.switch(Section::Discover);
        open(&mut a, "bad.iyp");
        let Mode::Review(rv) = &a.mode else { panic!() };
        assert!(rv.review.refusal.as_ref().unwrap().contains("changed after it was made"));
        for (w, h) in SIZES {
            let g = draw(&mut a, &cx, w, h);
            let labels: Vec<&str> = g.hits.iter().map(|h| h.label.as_str()).collect();
            assert!(labels.contains(&"Close (Esc)") && !labels.iter().any(|l| l.starts_with("Install")), "{w}x{h}: {labels:?}");
        }
        key(&mut a, &cx, Key::Char('y'));
        assert!(matches!(a.mode, Mode::Review(_)), "y does nothing on a refused package");
        key(&mut a, &cx, Key::Esc);
        // a file outside what the account may read is refused in words
        let mut alice = cx_for_user("st-warn-user");
        let mut b = StoreApp::new(&alice);
        b.switch(Section::Settings);
        key(&mut b, &alice, Key::Char('i'));
        typed(&mut b, &alice, "/etc/hostname");
        key(&mut b, &alice, Key::Ctrl('s'));
        let Mode::Form(f, _) = &b.mode else { panic!("still the form") };
        assert!(f.error.as_ref().is_some_and(|e| e.contains("permission denied")), "{:?}", f.error);
        alice.session.user = "alice".into();
    }

    fn cx_for_user(tag: &str) -> AppCx {
        cx(tag, Role::User)
    }

    #[test]
    fn an_ordinary_account_installs_for_itself_only_and_cannot_change_settings_or_add_servers() {
        let cx = cx("st-user", Role::User);
        let mut a = StoreApp::new(&cx);
        cat_on(&mut a, &cx);
        key(&mut a, &cx, Key::Char('w'));
        assert!(a.status_text().unwrap().contains("only for you"));
        open_page(&mut a, &cx, "lang-c");
        key(&mut a, &cx, Key::Char('i'));
        let Mode::Review(rv) = &a.mode else { panic!("{:?}", a.status_text()) };
        assert_eq!(rv.review.scope, iyakki_agent::skills::Scope::User("alice".into()));
        key(&mut a, &cx, Key::Char('y'));
        assert!(cx.services.skills.get(&iyakki_agent::skills::Scope::User("alice".into()), "lang-c").is_some());
        // the sample server is for administrators
        open_page(&mut a, &cx, "iyakki-demo");
        key(&mut a, &cx, Key::Char('i'));
        let Mode::Review(rv) = &a.mode else { panic!() };
        assert!(rv.review.refusal.as_ref().unwrap().contains("administrators") || rv.review.refusal.as_ref().unwrap().contains("for everyone"), "{:?}", rv.review.refusal);
        key(&mut a, &cx, Key::Esc);
        // settings are read-only
        a.switch(Section::Settings);
        for k in ['a', 'f', 'o', 't'] {
            key(&mut a, &cx, Key::Char(k));
            assert!(a.status_text().unwrap().contains("administrators"), "{k}: {:?}", a.status_text());
            assert!(matches!(a.mode, Mode::Browse));
        }
        let g = draw(&mut a, &cx, 800, 480);
        all_inside(&g, 800, 480, "settings");
    }

    #[test]
    fn a_folder_or_usb_catalog_is_added_from_settings_and_its_app_installs_with_no_network() {
        let cx = cx("st-usb", Role::Admin);
        let mut a = StoreApp::new(&cx);
        let k = new_signing_key().unwrap();
        let d = stick("st-usb-files", &[("weekly-report", "1.0.0", &pub_meta("Weekly report", Pricing::default()), &k)]);
        add_folder(&mut a, &cx, &d);
        let it = a.items.iter().find(|i| i.entry.id == "weekly-report").expect("the stick's app is listed");
        assert_eq!((it.verdict.level, it.source_name.as_str()), (Level::Community, "Stick"));
        open_page(&mut a, &cx, "weekly-report");
        key(&mut a, &cx, Key::Char('i'));
        let Mode::Review(rv) = &a.mode else { panic!("{:?}", a.status_text()) };
        assert!(rv.review.needs_warning_ack());
        key(&mut a, &cx, Key::Char('y'));
        assert!(a.status_text().unwrap().starts_with("Installed"), "{:?}", a.status_text());
        // the catalog list in Settings can switch it off and remove it
        a.switch(Section::Settings);
        a.sel[3] = 1;
        key(&mut a, &cx, Key::Enter);
        assert!(a.status_text().unwrap().contains("switched off"));
        assert_eq!(a.items.iter().find(|i| i.entry.id == "weekly-report").unwrap().source_name, "installed", "what was installed stays listed, but the catalog is gone");
        key(&mut a, &cx, Key::Delete);
        assert!(matches!(a.mode, Mode::Confirm(Ask::RemoveSource(_))));
        draw(&mut a, &cx, 1280, 720);
        key(&mut a, &cx, Key::Char('y'));
        assert!(a.status_text().unwrap().contains("removed"));
        assert!(Store::for_services(&cx.services).config().sources.is_empty());
    }

    #[test]
    fn checking_for_news_and_downloading_happen_on_a_background_thread_and_report_back() {
        let cx = cx("st-net", Role::Admin);
        let mut a = StoreApp::new(&cx);
        let k = new_signing_key().unwrap();
        let text = signed_skill("remote-helper", "1.0.0", &k, &pub_meta("Remote helper", Pricing::default()));
        let mut c = Catalog { v: 1, id: "web-store".into(), name: "Web store".into(), serial: 1, entries: vec![Entry { id: "remote-helper".into(), kind: "skill".into(), name: "remote-helper".into(), version: "1.0.0".into(), license: "MIT".into(), size: text.len() as u64, sha256: sha256_hex(text.as_bytes()), url: "packages/rh.iyp".into(), signer: public_key_hex(&k), added: 1_800_000_000, meta: pub_meta("Remote helper", Pricing::default()), ..Default::default() }], ..Default::default() };
        c.sign(&k);
        let table = Arc::new(Table::default());
        table.files.lock().unwrap().insert("https://cat.example/s/catalog.json".into(), c.to_json().into_bytes());
        table.files.lock().unwrap().insert("https://cat.example/s/packages/rh.iyp".into(), text.into_bytes());
        a.fetch = table.clone();
        a.switch(Section::Settings);
        key(&mut a, &cx, Key::Char('a'));
        typed(&mut a, &cx, "Web");
        key(&mut a, &cx, Key::Tab);
        for _ in 0..8 {
            key(&mut a, &cx, Key::Backspace);
        }
        typed(&mut a, &cx, "https://cat.example/s/catalog.json");
        key(&mut a, &cx, Key::Enter);
        // the store's key is read in the background and confirmed before it is added
        wait_job(&mut a, &cx);
        assert!(matches!(a.mode, Mode::Confirm(Ask::PinStore(..))), "{:?}", a.status_text());
        key(&mut a, &cx, Key::Char('y'));
        assert!(matches!(a.mode, Mode::Browse), "{:?}", a.status_text());
        let me = std::thread::current().id();
        key(&mut a, &cx, Key::Char('c'));
        assert!(a.job.is_some(), "the check runs in the background");
        let g = draw(&mut a, &cx, 800, 480);
        all_inside(&g, 800, 480, "while checking");
        let _ = a.wants_redraw();
        wait_job(&mut a, &cx);
        assert!(table.threads.lock().unwrap().iter().all(|t| *t != me), "no network on the UI thread");
        assert!(a.status_text().unwrap().contains("up to date"), "{:?}", a.status_text());
        assert!(a.items.iter().any(|i| i.entry.id == "remote-helper"));
        // the download and the review also happen off the UI thread
        open_page(&mut a, &cx, "remote-helper");
        key(&mut a, &cx, Key::Char('i'));
        assert!(a.job.is_some() && matches!(a.mode, Mode::Detail));
        key(&mut a, &cx, Key::Char('i'));
        assert!(a.status_text().unwrap().contains("Wait"), "one job at a time");
        wait_job(&mut a, &cx);
        assert!(table.threads.lock().unwrap().iter().all(|t| *t != me));
        let Mode::Review(rv) = &a.mode else { panic!("{:?}", a.status_text()) };
        assert_eq!(rv.review.verdict.level, Level::Community);
        key(&mut a, &cx, Key::Char('y'));
        assert!(a.status_text().unwrap().starts_with("Installed"), "{:?}", a.status_text());
        // an unreachable catalog is reported in words
        table.files.lock().unwrap().clear();
        let mut offline = StoreApp::new(&cx);
        offline.fetch = table.clone();
        key(&mut offline, &cx, Key::Char('c'));
        wait_job(&mut offline, &cx);
        assert!(offline.status_text().unwrap().contains("Catalogs are up to date") || offline.status_text().unwrap().contains("Could not"), "{:?}", offline.status_text());
    }

    #[test]
    fn a_paid_app_is_locked_until_a_licence_is_added_from_a_file_and_buying_is_disabled() {
        let cx = cx("st-paid", Role::Admin);
        let mut a = StoreApp::new(&cx);
        let k = new_signing_key().unwrap();
        let paid = Pricing { model: PriceModel::Paid, price: 999, currency: "USD".into(), ..Default::default() };
        let d = stick("st-paid-files", &[("pro-helper", "1.0.0", &pub_meta("Pro helper", paid), &k)]);
        add_folder(&mut a, &cx, &d);
        let it = a.items.iter().find(|i| i.entry.id == "pro-helper").unwrap();
        assert_eq!(it.state, State::Locked);
        open_page(&mut a, &cx, "pro-helper");
        let g = draw(&mut a, &cx, 1024, 600);
        let labels: Vec<&str> = g.hits.iter().map(|h| h.label.as_str()).collect();
        assert!(labels.contains(&"Add licence (l)") && !labels.contains(&"Install (i)") && !labels.contains(&"Buy (coming soon)"), "{labels:?}");
        key(&mut a, &cx, Key::Char('i'));
        let Mode::Review(rv) = &a.mode else { panic!("{:?}", a.status_text()) };
        assert!(rv.review.locked && !rv.review.can_install());
        let g = draw(&mut a, &cx, 1280, 720);
        assert!(g.hits.iter().any(|h| h.label == "Add licence (l)") && !g.hits.iter().any(|h| h.label.starts_with("Install")));
        // a token for another device is refused in words; the right one unlocks the app
        let device = a.store.licenses().device_id();
        let lic_dir = temp_dir("st-paid-lic");
        let wrong = iyakki_agent::store::license::Token::issue(&k, "pro-helper", "acme", "some-other-device", "", "L-1", "", 1, 0);
        std::fs::write(lic_dir.join("wrong.json"), wrong.to_json()).unwrap();
        let right = iyakki_agent::store::license::Token::issue(&k, "pro-helper", "acme", &device, "", "L-2", "standard", 1, 0);
        std::fs::write(lic_dir.join("right.json"), right.to_json()).unwrap();
        key(&mut a, &cx, Key::Char('l'));
        assert!(matches!(a.mode, Mode::Form(..)));
        typed(&mut a, &cx, &lic_dir.join("wrong.json").display().to_string());
        key(&mut a, &cx, Key::Enter);
        let Mode::Form(f, _) = &a.mode else { panic!("{:?}", a.status_text()) };
        assert!(f.error.as_ref().unwrap().contains("different device"), "{:?}", f.error);
        key(&mut a, &cx, Key::Esc);
        key(&mut a, &cx, Key::Esc);
        open_page(&mut a, &cx, "pro-helper");
        key(&mut a, &cx, Key::Char('l'));
        typed(&mut a, &cx, &lic_dir.join("right.json").display().to_string());
        key(&mut a, &cx, Key::Enter);
        assert!(a.status_text().unwrap().contains("Licence added"), "{:?}", a.status_text());
        assert_eq!(a.items.iter().find(|i| i.entry.id == "pro-helper").unwrap().state, State::Available);
        key(&mut a, &cx, Key::Char('i'));
        let Mode::Review(rv) = &a.mode else { panic!("{:?}", a.status_text()) };
        assert!(rv.review.can_install() && rv.review.steps.iter().any(|s| s.label == "Licence" && s.ok));
    }

    #[test]
    fn clicking_works_at_every_size_tabs_rows_back_and_the_install_buttons() {
        let cx = cx("st-click", Role::Admin);
        let mut a = StoreApp::new(&cx);
        cat_on(&mut a, &cx);
        let click = |a: &mut StoreApp, w: u32, h: u32, label: &str| {
            let g = draw(a, &cx, w, h);
            let hit = g.hits.iter().rev().find(|h| h.label == label || h.label.starts_with(label)).unwrap_or_else(|| panic!("{w}x{h}: no `{label}` in {:?}", g.hits.iter().map(|h| h.label.clone()).collect::<Vec<_>>()));
            let ev = hit.ev.clone();
            a.handle(&ev, &cx);
        };
        for (w, h) in SIZES {
            a.switch(Section::Discover);
            click(&mut a, w, h, "Installed");
            assert_eq!(a.section, Section::Installed, "{w}x{h}");
            click(&mut a, w, h, "Publishers");
            assert_eq!(a.section, Section::Publishers);
            click(&mut a, w, h, "Settings");
            assert_eq!(a.section, Section::Settings);
            click(&mut a, w, h, "Discover");
            assert_eq!(a.section, Section::Discover);
            // a row opens its page; Back returns; Install opens the review; Cancel closes it
            let g = draw(&mut a, &cx, w, h);
            let row = g.hits.iter().find(|h| matches!(h.ev, Event::Row(_))).unwrap().ev.clone();
            a.handle(&row, &cx);
            assert!(matches!(a.mode, Mode::Detail), "{w}x{h}");
            let installable = a.current().map_or(false, |i| matches!(i.state, State::Available));
            if installable {
                click(&mut a, w, h, "Install (i)");
                assert!(matches!(a.mode, Mode::Review(_)) || a.job.is_some(), "{w}x{h}: {:?}", a.status_text());
                click(&mut a, w, h, "Cancel");
                assert!(matches!(a.mode, Mode::Detail));
            }
            click(&mut a, w, h, "Back");
            assert!(matches!(a.mode, Mode::Browse) && a.detail.is_none(), "{w}x{h}");
            // the filter chips are clickable
            click(&mut a, w, h, "Price:");
            assert_eq!(a.filter.price, PriceFilter::Free);
            click(&mut a, w, h, "Clear");
            assert!(!a.filtering());
        }
    }

    #[test]
    fn publishers_filter_the_list_and_the_catalog_link_for_a_cli_tool_opens_the_cli_tile() {
        let cx = cx("st-pub", Role::Admin);
        let mut a = StoreApp::new(&cx);
        cat_on(&mut a, &cx);
        a.switch(Section::Publishers);
        let g = draw(&mut a, &cx, 1280, 720);
        assert!(g.hits.iter().any(|h| h.label == "IYAKKI") && g.hits.iter().any(|h| h.label.starts_with("Official")));
        let ps = store::publishers(&a.items);
        a.sel[2] = ps.iter().position(|p| p.id == "example-studio").unwrap();
        key(&mut a, &cx, Key::Enter);
        assert_eq!(a.section, Section::Discover);
        assert_eq!(a.filtered().len(), 3, "the publisher's apps");
        a.filter = Filter::default();
        open_page(&mut a, &cx, "cli-kubectl");
        let g = draw(&mut a, &cx, 1280, 720);
        assert!(g.hits.iter().any(|h| h.label == "Open CLI (i)"), "{:?}", g.hits.iter().map(|h| &h.label).collect::<Vec<_>>());
        key(&mut a, &cx, Key::Char('i'));
        assert_eq!(a.take_request(), Some(("cli".to_string(), "kubectl".to_string())));
        assert!(a.take_request().is_none());
    }

    #[test]
    fn settings_show_the_catalogs_keys_and_the_daily_check_which_is_off_until_switched_on() {
        let cx = cx("st-set", Role::Admin);
        let mut a = StoreApp::new(&cx);
        cat_on(&mut a, &cx);
        a.switch(Section::Settings);
        let rows = a.settings_rows(Some(&cx));
        assert!(rows.iter().any(|r| r.0 == "Built-in catalog") && rows.iter().any(|r| r.1.contains("off: nothing is fetched")));
        assert!(rows.iter().any(|r| r.0 == "IYAKKI release keys" && r.1.contains("none pinned")), "the real key has not been generated yet");
        key(&mut a, &cx, Key::Char('o'));
        assert!(Store::for_services(&cx.services).config().auto_check);
        key(&mut a, &cx, Key::Char('o'));
        assert!(!Store::for_services(&cx.services).config().auto_check);
        // trusting a key shows it in the list and makes its packages Trusted
        let k = new_signing_key().unwrap();
        key(&mut a, &cx, Key::Char('t'));
        typed(&mut a, &cx, &public_key_hex(&k));
        key(&mut a, &cx, Key::Tab);
        typed(&mut a, &cx, "Acme IT");
        key(&mut a, &cx, Key::Enter);
        assert!(a.status_text().unwrap().contains("Trusted"), "{:?}", a.status_text());
        assert!(a.settings_rows(Some(&cx)).iter().any(|r| r.0 == "Trusted key: Acme IT"));
        for (w, h) in SIZES {
            let g = draw(&mut a, &cx, w, h);
            all_inside(&g, w, h, "settings");
        }
    }

    #[test]
    fn the_shipped_skill_texts_match_the_ones_the_shell_seeds() {
        assert_eq!(crate::samples::BUILTIN_SKILLS.len(), iyakki_agent::store::builtin::SKILLS.len());
        for (a, b) in crate::samples::BUILTIN_SKILLS.iter().zip(iyakki_agent::store::builtin::SKILLS.iter()) {
            assert_eq!(a, b, "the store lists exactly what the first sign-in puts in place");
        }
        assert_eq!(crate::samples::SKILLS.len(), iyakki_agent::store::builtin::SAMPLE_SKILLS.len());
        for (a, b) in crate::samples::SKILLS.iter().zip(iyakki_agent::store::builtin::SAMPLE_SKILLS.iter()) {
            assert_eq!(a, b);
        }
    }

    #[test]
    fn the_store_is_a_system_tile_for_administrators_operators_and_users_but_not_guests() {
        use iyakki_sec::policy::App as PApp;
        assert_eq!(crate::launcher::Group::of(PApp::Store), Some(crate::launcher::Group::System));
        assert_eq!(PApp::Store.id(), "store");
        assert_eq!(PApp::ALL.iter().filter(|a| **a == PApp::Store).count(), 1);
        for r in [Role::Admin, Role::Operator, Role::User] {
            assert!(r.template().apps.contains(&PApp::Store), "{r:?}");
        }
        assert!(!Role::Guest.template().apps.contains(&PApp::Store));
    }

    // ------------------------------------------------------------------------------------ the installed view

    /// A system with one of everything installed, from the real stores: a shipped skill, one written by hand, the sample
    /// server, a server added by hand, a command-line tool, a tool package and a theme installed through the store.
    fn seed(cx: &AppCx) {
        use iyakki_agent::skills::Scope;
        let svc = &cx.services;
        svc.skills.add(Scope::Shared, iyakki_agent::store::builtin::SKILLS[1].1, true, true).unwrap();
        svc.skills.add(Scope::Shared, "---\nname: hand-made\ndescription: written by hand\ntriggers: x\n---\nDo it.\n", true, true).unwrap();
        svc.mcp.add(iyakki_agent::mcp::StoredServer { name: "iyakki-demo".into(), command: "/x/iyakki".into(), args: vec!["--mcp-demo".into()], sample: true }).unwrap();
        svc.mcp.add(iyakki_agent::mcp::StoredServer { name: "files".into(), command: "/usr/bin/files".into(), args: vec![], sample: false }).unwrap();
        iyakki_agent::clis::write_fake_installed(&svc.clis, "kubectl", "#!/bin/sh\necho v1\n");
        let k = new_signing_key().unwrap();
        let manifest = serde_json::json!({"name": "shout", "version": "1.0.0", "license": "MIT", "description": "test tool", "schema": {"type": "object", "properties": {"text": {"type": "string"}}, "required": ["text"]}, "capabilities": ["net:example.com"]}).to_string();
        let tool = iyakki_agent::pkg::build_tool(&manifest, include_str!("../../../../tests/fixtures/tool_echo.wat").as_bytes(), Some(&k)).unwrap();
        let r = svc.packages.review_tool(&tool, &Scope::Shared).unwrap();
        svc.packages.install_tool(&r, true, true, true).unwrap();
        let st = Store::for_services(svc);
        let a = Actor::from_policy(&cx.session.user, &cx.session.policy);
        let meta = Meta { target: "tales".into(), ..pub_meta("Sunset", Pricing::default()) };
        let theme = iyakki_agent::pkg::build_generic("theme", "sunset", "1.0.0", "MIT", "{}", Some(&meta.to_json()), None, Some(&k)).unwrap().into_bytes();
        let rv = st.review_sideload(svc, &a, theme, Scope::User(cx.session.user.clone()), &[]);
        st.install(svc, &a, &rv, Confirm::all()).unwrap();
    }

    fn ordinal(a: &StoreApp, name: &str) -> usize {
        a.ifiltered().iter().position(|e| e.name == name).unwrap_or_else(|| panic!("{name} is listed: {:?}", a.ifiltered().iter().map(|e| &e.name).collect::<Vec<_>>()))
    }

    fn click_ev(a: &mut StoreApp, cx: &AppCx, w: u32, h: u32, ev: Event) {
        // a row far down the list is scrolled into view by selecting it, as the keyboard would
        if let Event::Tab(n) = ev {
            if n >= 1000 {
                a.sel[1] = (n - 1000) / 4;
            }
        }
        let g = draw(a, cx, w, h);
        let hit = g.hits.iter().rev().find(|x| x.ev == ev).unwrap_or_else(|| panic!("{w}x{h}: no region for {ev:?}"));
        let e = hit.ev.clone();
        a.handle(&e, cx);
    }

    #[test]
    fn the_tile_opens_on_what_is_installed_with_no_storefront_and_no_demo_apps() {
        let cx = cx("st-inst-first", Role::Admin);
        seed(&cx);
        let mut a = StoreApp::new(&cx);
        assert_eq!(a.section, Section::Installed);
        assert!(!a.catalog && a.sections() == vec![Section::Installed, Section::Settings]);
        for (w, h) in SIZES {
            let g = draw(&mut a, &cx, w, h);
            let labels: Vec<&str> = g.hits.iter().map(|h| h.label.as_str()).collect();
            assert!(labels.contains(&"Installed") && labels.contains(&"Settings"), "{labels:?}");
            assert!(!labels.contains(&"Discover") && !labels.contains(&"Publishers"), "{w}x{h}: no storefront by default");
            assert!(!labels.iter().any(|l| l.contains("DEMO") || l.contains("Meeting suite")), "{labels:?}");
            all_inside(&g, w, h, "the installed page");
            no_overlap(&g, w, h, |e| matches!(e, Event::Row(_)), "rows");
            no_overlap(&g, w, h, |e| matches!(e, Event::Row(_) | Event::Tab(220..=230) | Event::Focus(_)), "rows, chips and the search box");
            assert!(g.hits.iter().filter(|h| matches!(h.ev, Event::Row(_))).count() >= 3, "{w}x{h}");
        }
        // the preinstalled items are there, from the real state
        let names: Vec<String> = a.inst.iter().map(|e| e.name.clone()).collect();
        for n in ["lang-python", "hand-made", "iyakki-demo", "files", "kubectl", "shout", "sunset", "chat", "cli"] {
            assert!(names.contains(&n.to_string()), "{n}: {names:?}");
        }
        assert!(!a.inst.iter().any(|e| e.name.starts_with("demo-")), "no demo apps in the installed list");
        let py = a.inst.iter().find(|e| e.name == "lang-python").unwrap();
        assert_eq!((py.level, py.publisher.as_str()), (Level::Official, "IYAKKI"));
        // the Discover page and the demo apps do not exist until the catalog is switched on
        a.switch(Section::Discover);
        assert_eq!(a.section, Section::Installed, "Discover is not reachable while the catalog is off");
        assert!(!a.items.iter().any(|i| i.entry.demo));
        let _ = a.take_request();
    }

    #[test]
    fn the_chips_count_the_real_installed_state_by_type_and_filter_search_and_sort_the_list() {
        let cx = cx("st-inst-chips", Role::Admin);
        seed(&cx);
        let mut a = StoreApp::new(&cx);
        let g = draw(&mut a, &cx, 1280, 720);
        let counts = iyakki_agent::store::installed::counts(&a.inst);
        for (i, label) in ["All", "MCP", "CLI", "Skills", "Apps", "Extensions"].iter().enumerate() {
            assert!(g.hits.iter().any(|h| h.label == format!("{label} {}", counts[i])), "{label} {} in {:?}", counts[i], g.hits.iter().map(|h| &h.label).collect::<Vec<_>>());
        }
        assert_eq!((counts[1], counts[2], counts[3], counts[5]), (2, 1, 2, 2), "MCP servers, CLI tools, skills, extensions (the tool package and the theme)");
        assert!(counts[4] >= 20, "the tiles are System apps");
        // every type chip filters
        for (n, ty) in [(221, IType::Mcp), (222, IType::Cli), (223, IType::Skill), (224, IType::App), (225, IType::Extension)] {
            click_ev(&mut a, &cx, 1280, 720, Event::Tab(n));
            assert_eq!(a.ifilter, Some(ty));
            assert!(a.ifiltered().iter().all(|e| e.ty == ty) && !a.ifiltered().is_empty());
        }
        click_ev(&mut a, &cx, 1024, 600, Event::Tab(220));
        assert!(a.ifilter.is_none());
        // the keyboard moves between chips, and Esc clears
        key(&mut a, &cx, Key::Right);
        assert_eq!(a.ifilter, Some(IType::Mcp));
        key(&mut a, &cx, Key::Left);
        key(&mut a, &cx, Key::Left);
        assert_eq!(a.ifilter, Some(IType::Driver), "Left from All wraps to the last chip");
        key(&mut a, &cx, Key::Esc);
        assert!(a.ifilter.is_none());
        // search
        key(&mut a, &cx, Key::Char('/'));
        assert!(a.isearching);
        typed(&mut a, &cx, "hand");
        assert_eq!(a.ifiltered().iter().map(|e| e.name.clone()).collect::<Vec<_>>(), vec!["hand-made"]);
        key(&mut a, &cx, Key::Esc);
        assert!(a.iquery.value.is_empty() && !a.isearching);
        key(&mut a, &cx, Key::Char('/'));
        typed(&mut a, &cx, "zzzz");
        let g = draw(&mut a, &cx, 800, 480);
        assert!(!g.hits.iter().any(|h| matches!(h.ev, Event::Row(_))), "the empty state shows");
        key(&mut a, &cx, Key::Esc);
        // sort: name, recently installed (the store-installed theme first), size, trust (Official first)
        click_ev(&mut a, &cx, 1280, 720, Event::Tab(230));
        assert_eq!(a.isort, Sort::Recent);
        assert_eq!(a.ifiltered()[0].name, "sunset");
        key(&mut a, &cx, Key::Char('s'));
        assert_eq!(a.isort, Sort::Size);
        assert!(a.ifiltered()[0].size >= a.ifiltered()[1].size);
        key(&mut a, &cx, Key::Char('s'));
        assert_eq!(a.isort, Sort::Trust);
        a.ifilter = Some(IType::Skill);
        assert_eq!(a.ifiltered()[0].level, Level::Official);
        assert_eq!(a.ifiltered().last().unwrap().level, Level::Unsigned);
        a.ifilter = None;
        assert!(a.ifiltered().iter().rev().take(5).all(|e| e.ty == IType::App), "the tiles of the system come last");
        key(&mut a, &cx, Key::Char('s'));
        assert_eq!(a.isort, Sort::Updates);
        key(&mut a, &cx, Key::Char('s'));
        assert_eq!(a.isort, Sort::Name);
    }

    #[test]
    fn each_row_shows_icon_name_type_version_badge_status_and_size_and_its_buttons() {
        let cx = cx("st-inst-rows", Role::Admin);
        seed(&cx);
        let mut a = StoreApp::new(&cx);
        a.ifilter = Some(IType::Extension);
        let g = draw(&mut a, &cx, 1280, 720);
        let labels: Vec<&str> = g.hits.iter().map(|h| h.label.as_str()).collect();
        assert!(labels.iter().any(|l| l.starts_with("Official: ") || l.starts_with("Community: ") || l.starts_with("Trusted: ") || l.starts_with("Unsigned: ")), "a trust badge on every row: {labels:?}");
        for l in ["Remove", "Turn off", "Open"] {
            assert!(labels.contains(&l), "{l}: {labels:?}");
        }
        // the theme says what it is for
        let th = a.ifiltered().into_iter().find(|e| e.name == "sunset").unwrap().clone();
        assert_eq!((th.target.as_deref(), th.ext_kind.as_deref()), (Some("tales"), Some("theme")));
        a.ifilter = None;
        for (w, h) in SIZES {
            let g = draw(&mut a, &cx, w, h);
            no_overlap(&g, w, h, |e| matches!(e, Event::Row(_)), "rows");
            // a row's buttons do not overlap each other
            for ord in 0..3 {
                let btns: Vec<R> = g.hits.iter().filter(|x| matches!(x.ev, Event::Tab(n) if (1000 + ord * 4..1000 + ord * 4 + 4).contains(&n))).map(|x| x.rect).collect();
                for (i, p) in btns.iter().enumerate() {
                    for q in &btns[i + 1..] {
                        assert!(p.x + p.w <= q.x + 0.5 || q.x + q.w <= p.x + 0.5, "{w}x{h}: row {ord} buttons overlap");
                    }
                }
            }
        }
    }

    #[test]
    fn system_apps_refuse_removal_with_a_plain_reason_and_cannot_be_turned_off() {
        let cx = cx("st-inst-sys", Role::Admin);
        let mut a = StoreApp::new(&cx);
        a.ifilter = Some(IType::App);
        let ord = ordinal(&a, "cli");
        let g = draw(&mut a, &cx, 1280, 720);
        assert!(!g.hits.iter().any(|h| h.ev == Event::Tab(1000 + ord * 4 + 2) || h.ev == Event::Tab(1000 + ord * 4 + 1)), "a system app row has no Remove or Turn off button");
        // asked by key all the same: refused in words, no dialog
        a.sel[1] = ord;
        key(&mut a, &cx, Key::Char('r'));
        assert!(matches!(a.mode, Mode::Browse));
        assert!(a.status_text().unwrap().contains("part of IYAKKI and cannot be removed"), "{:?}", a.status_text());
        key(&mut a, &cx, Key::Char('d'));
        assert!(a.status_text().unwrap().contains("cannot be turned off"));
        assert!(a.inst.iter().any(|e| e.name == "cli"), "still there");
    }

    #[test]
    fn open_jumps_to_the_tile_where_the_item_lives() {
        let cx = cx("st-inst-open", Role::Admin);
        seed(&cx);
        let mut a = StoreApp::new(&cx);
        for (name, tile, item) in [("kubectl", "cli", "kubectl"), ("files", "mcp", "files"), ("hand-made", "skills", "hand-made"), ("lang-python", "skills", "lang-python"), ("sunset", "tales", "sunset"), ("shout", "skills", "shout"), ("chat", "chat", "")] {
            let ord = ordinal(&a, name);
            click_ev(&mut a, &cx, 1280, 720, Event::Tab(1000 + ord * 4));
            assert_eq!(a.take_request(), Some((tile.to_string(), item.to_string())), "{name}");
        }
        // from the page, with the key
        a.sel[1] = ordinal(&a, "kubectl");
        key(&mut a, &cx, Key::Enter);
        key(&mut a, &cx, Key::Char('o'));
        assert_eq!(a.take_request(), Some(("cli".to_string(), "kubectl".to_string())));
        // an account without the tile gets no Open
        let mut user = self::cx("st-inst-open2", Role::User);
        seed_user(&user);
        user.session.policy.apps.remove(&iyakki_sec::policy::App::Skills);
        let mut b = StoreApp::new(&user);
        let g = draw(&mut b, &user, 1280, 720);
        let ord = ordinal(&b, "hand-made");
        assert!(!g.hits.iter().any(|h| h.ev == Event::Tab(1000 + ord * 4)), "no Open into SKILLS for this account");
    }

    fn seed_user(cx: &AppCx) {
        cx.services.skills.add(iyakki_agent::skills::Scope::Shared, "---\nname: hand-made\ndescription: x\ntriggers: x\n---\nDo it.\n", true, true).unwrap();
    }

    #[test]
    fn turn_off_on_and_remove_with_confirmation_act_on_the_real_stores() {
        let cx = cx("st-inst-act", Role::Admin);
        seed(&cx);
        let mut a = StoreApp::new(&cx);
        let user_scope = iyakki_agent::skills::Scope::User("root".into());
        // the theme was installed through the store: it can be turned off and on
        let ord = ordinal(&a, "sunset");
        click_ev(&mut a, &cx, 1280, 720, Event::Tab(1000 + ord * 4 + 1));
        assert!(a.status_text().unwrap().contains("turned off"), "{:?}", a.status_text());
        assert_eq!(a.inst.iter().find(|e| e.name == "sunset").unwrap().status, Status::Off);
        assert!(Store::for_services(&cx.services).content("theme", "root").is_empty());
        let ord = ordinal(&a, "sunset");
        let g = draw(&mut a, &cx, 1280, 720);
        assert!(g.hits.iter().any(|h| h.label == "Turn on"));
        click_ev(&mut a, &cx, 1280, 720, Event::Tab(1000 + ord * 4 + 1));
        assert_eq!(a.inst.iter().find(|e| e.name == "sunset").unwrap().status, Status::On);
        // one written by hand cannot be turned off, only removed
        let ord = ordinal(&a, "hand-made");
        a.sel[1] = ord;
        key(&mut a, &cx, Key::Char('d'));
        assert!(a.status_text().unwrap().contains("can only be removed"));
        // remove asks first; anything but y keeps it
        click_ev(&mut a, &cx, 1280, 720, Event::Tab(1000 + ord * 4 + 2));
        assert!(matches!(a.mode, Mode::Confirm(Ask::RemovePlan(_))));
        for (w, h) in SIZES {
            let g = draw(&mut a, &cx, w, h);
            assert!(g.hits.iter().any(|h| h.label == "Remove (y)") && g.hits.iter().any(|h| h.label == "Cancel (Esc)"), "{w}x{h}");
        }
        key(&mut a, &cx, Key::Char('x'));
        assert!(cx.services.skills.get(&iyakki_agent::skills::Scope::Shared, "hand-made").is_some());
        click_ev(&mut a, &cx, 1280, 720, Event::Tab(1000 + ord * 4 + 2));
        key(&mut a, &cx, Key::Char('y'));
        assert!(a.status_text().unwrap().starts_with("Removed"), "{:?}", a.status_text());
        assert!(cx.services.skills.get(&iyakki_agent::skills::Scope::Shared, "hand-made").is_none());
        assert!(!a.inst.iter().any(|e| e.name == "hand-made"));
        // the command-line tool and the server go through their own stores
        for name in ["kubectl", "files"] {
            let ord = ordinal(&a, name);
            click_ev(&mut a, &cx, 1280, 720, Event::Tab(1000 + ord * 4 + 2));
            key(&mut a, &cx, Key::Char('y'));
            assert!(a.status_text().unwrap().starts_with("Removed"), "{name}: {:?}", a.status_text());
        }
        assert!(cx.services.clis.installed().is_empty() && !cx.services.mcp.list().iter().any(|s| s.name == "files"));
        let _ = user_scope;
        let log = std::fs::read_to_string(std::env::temp_dir().join(format!("iyakki-shell-{}-st-inst-act", std::process::id())).join("audit.log")).unwrap_or_default();
        assert!(log.contains("store.remove") && log.contains("store.toggle"), "{log}");
    }

    #[test]
    fn an_installed_items_page_shows_permissions_publisher_licence_files_and_source_at_every_size() {
        let cx = cx("st-inst-page", Role::Admin);
        seed(&cx);
        let mut a = StoreApp::new(&cx);
        a.sel[1] = ordinal(&a, "shout");
        key(&mut a, &cx, Key::Enter);
        assert!(matches!(a.mode, Mode::Detail) && a.idetail.is_some());
        for (w, h) in SIZES {
            let g = draw(&mut a, &cx, w, h);
            let labels: Vec<&str> = g.hits.iter().map(|h| h.label.as_str()).collect();
            assert!(labels.contains(&"Back (Esc)") && labels.contains(&"Remove (r)") && labels.contains(&"Open (o)"), "{w}x{h}: {labels:?}");
            assert!(labels.iter().any(|l| l.starts_with("Community: ")), "{labels:?}");
            all_inside(&g, w, h, "an installed item's page");
        }
        let text: Vec<String> = a.text.lines.iter().map(|l| l.0.clone()).collect();
        assert!(text.iter().any(|l| l.contains("connect to example.com")), "{text:?}");
        assert!(text.iter().any(|l| l.starts_with("Source:")) && text.iter().any(|l| l.starts_with("Type: Extension")) && text.iter().any(|l| l.starts_with("Extends: CHAT (tool)")), "{text:?}");
        assert!(text.iter().any(|l| l.contains("shout.iyp")), "the files are listed");
        key(&mut a, &cx, Key::Esc);
        assert!(a.idetail.is_none() && matches!(a.mode, Mode::Browse));
        // a store-installed theme shows its publisher and licence and the tile it is for
        a.sel[1] = ordinal(&a, "sunset");
        a.handle(&Event::Row(ordinal(&a, "sunset")), &cx);
        draw(&mut a, &cx, 1280, 720);
        let text: Vec<String> = a.text.lines.iter().map(|l| l.0.clone()).collect();
        assert!(text.iter().any(|l| l == "Publisher: Acme Ltd") && text.iter().any(|l| l.starts_with("Licence: MIT (permissive)")) && text.iter().any(|l| l.starts_with("Extends: TALES (theme)")), "{text:?}");
        let g = draw(&mut a, &cx, 1280, 720);
        assert!(g.hits.iter().any(|h| h.label == "Turn off (d)"));
        key(&mut a, &cx, Key::Char('r'));
        assert!(matches!(a.mode, Mode::Confirm(_)));
        key(&mut a, &cx, Key::Esc);
        assert!(matches!(a.mode, Mode::Detail), "cancelling goes back to the page");
        // a system app's page has neither Remove nor Turn off
        key(&mut a, &cx, Key::Esc);
        a.ifilter = Some(IType::App);
        a.sel[1] = 0;
        key(&mut a, &cx, Key::Enter);
        let g = draw(&mut a, &cx, 1280, 720);
        assert!(!g.hits.iter().any(|h| h.label.starts_with("Remove") || h.label.starts_with("Turn")), "{:?}", g.hits.iter().map(|h| &h.label).collect::<Vec<_>>());
    }

    #[test]
    fn the_catalog_toggle_in_settings_brings_discover_and_publishers_and_the_demo_apps_back() {
        let cx = cx("st-inst-toggle", Role::Admin);
        let mut a = StoreApp::new(&cx);
        a.switch(Section::Settings);
        let rows = a.settings_rows(Some(&cx));
        assert!(rows.iter().any(|r| r.0 == "Show the catalog (coming soon)" && r.1.starts_with("off")));
        assert!(!a.items.iter().any(|i| i.entry.demo));
        key(&mut a, &cx, Key::Char('g'));
        assert!(a.catalog && a.sections().len() == 4, "{:?}", a.status_text());
        assert!(a.items.iter().any(|i| i.entry.demo), "the demo apps exist only while the catalog is on");
        let g = draw(&mut a, &cx, 800, 480);
        let labels: Vec<&str> = g.hits.iter().map(|h| h.label.as_str()).collect();
        assert!(labels.contains(&"Discover") && labels.contains(&"Publishers"));
        a.switch(Section::Discover);
        assert_eq!(a.section, Section::Discover);
        // Tab cycles through the four pages
        key(&mut a, &cx, Key::Tab);
        assert_eq!(a.section, Section::Publishers);
        key(&mut a, &cx, Key::Tab);
        assert_eq!(a.section, Section::Settings);
        key(&mut a, &cx, Key::Tab);
        assert_eq!(a.section, Section::Installed);
        // off again: back to the installed page
        a.switch(Section::Settings);
        key(&mut a, &cx, Key::Char('g'));
        assert!(!a.catalog && a.sections().len() == 2);
        assert!(!a.items.iter().any(|i| i.entry.demo));
        // adding a catalog (a folder) switches it on by itself, and removing it switches it off again
        let k = new_signing_key().unwrap();
        let d = stick("st-inst-toggle-files", &[("weekly-report", "1.0.0", &pub_meta("Weekly report", Pricing::default()), &k)]);
        add_folder(&mut a, &cx, &d);
        assert!(a.catalog, "a configured catalog shows the catalog");
        assert!(a.items.iter().any(|i| i.entry.id == "weekly-report"));
        // an ordinary account cannot flip it
        let u = self::cx("st-inst-toggle-user", Role::User);
        let mut b = StoreApp::new(&u);
        b.switch(Section::Settings);
        key(&mut b, &u, Key::Char('g'));
        assert!(b.status_text().unwrap().contains("administrators") && !b.catalog);
    }

    // ------------------------------------------------------------------------------------------ the Drivers chip

    /// A Mac mini 2014 as /sys and /proc would show it (Wi-Fi BCM4360, Bluetooth 05ac:8289 bound to btusb).
    fn fake_mini(cx: &AppCx, tag: &str) -> iyakki_agent::store::drivers::Drivers {
        let root = temp_dir(tag);
        for (rel, c) in [("sys/class/dmi/id/sys_vendor", "Apple Inc.\n"), ("sys/class/dmi/id/product_name", "Macmini7,1\n"), ("proc/sys/kernel/osrelease", "6.12.112\n"), ("proc/modules", "btusb 1 0 - Live 0x0\ni915 1 0 - Live 0x0\n"), ("sys/bus/pci/devices/0000:00:02.0/vendor", "0x8086\n"), ("sys/bus/pci/devices/0000:00:02.0/device", "0x0a26\n"), ("sys/bus/pci/devices/0000:03:00.0/vendor", "0x14e4\n"), ("sys/bus/pci/devices/0000:03:00.0/device", "0x43a0\n"), ("sys/bus/usb/devices/1-4/idVendor", "05ac\n"), ("sys/bus/usb/devices/1-4/idProduct", "8289\n"), ("sys/bus/usb/devices/1-4/product", "Bluetooth USB Host Controller\n")] {
            let p = root.join(rel);
            std::fs::create_dir_all(p.parent().unwrap()).unwrap();
            std::fs::write(p, c).unwrap();
        }
        iyakki_agent::store::drivers::Drivers::for_services(&cx.services).with_roots(iyakki_drivers::Roots { sys: root.join("sys"), proc: root.join("proc") })
    }

    fn pane_row(a: &StoreApp, id: &str) -> usize {
        a.drivers.rows.iter().position(|r| r.id == id).unwrap_or_else(|| panic!("{id} is listed: {:?}", a.drivers.rows.iter().map(|r| &r.id).collect::<Vec<_>>()))
    }

    #[test]
    fn the_drivers_chip_lists_this_machine_with_the_right_buttons_at_every_size() {
        let cx = cx("st-drivers-chip", Role::Admin);
        let mut a = StoreApp::new(&cx);
        a.drivers_for = Some(fake_mini(&cx, "st-drivers-chip-mini"));
        draw(&mut a, &cx, 1280, 720);
        let labels: Vec<String> = a.ifiltered().iter().map(|e| e.title.clone()).collect();
        assert!(!labels.iter().any(|l| l.contains("Mac")), "the Drivers chip is not the All list");
        // the chip is there, last, and clicking it shows the machine
        let g = draw(&mut a, &cx, 1280, 720);
        assert!(g.hits.iter().any(|h| matches!(h.ev, Event::Tab(226)) && h.label.starts_with("Drivers")), "the chip is drawn: {:?}", g.hits.iter().map(|h| &h.label).collect::<Vec<_>>());
        a.handle(&Event::Tab(226), &cx);
        assert_eq!(a.ifilter, Some(IType::Driver));
        let ids: Vec<&str> = a.drivers.rows.iter().map(|r| r.id.as_str()).collect();
        assert_eq!(ids, ["mac-gpu-intel", "mac-wifi-bcm4360", "mac-bt-broadcom", "mac-fan-sensors", "mac-audio-cirrus", "mac-keyboard-options"], "{ids:?}");
        for (w, h) in SIZES {
            let g = draw(&mut a, &cx, w, h);
            all_inside(&g, w, h, "the Drivers view");
            no_overlap(&g, w, h, |e| matches!(e, Event::Row(_) | Event::Tab(220..=230)), "rows and chips");
            no_overlap(&g, w, h, |e| matches!(e, Event::Tab(220..=230) | Event::Tab(2000..=2902)), "chips and buttons");
            let labels: Vec<&str> = g.hits.iter().map(|h| h.label.as_str()).collect();
            assert!(labels.contains(&"From USB…") || w < 900, "{w}x{h}: {labels:?}");
            assert!(labels.contains(&"Why"), "{w}x{h}");
        }
        // Esc with nothing open goes back to All; Left and Right still move between the chips
        key(&mut a, &cx, Key::Left);
        assert_eq!(a.ifilter, Some(IType::Extension));
        key(&mut a, &cx, Key::Right);
        assert_eq!(a.ifilter, Some(IType::Driver));
        key(&mut a, &cx, Key::Esc);
        assert!(a.ifilter.is_none());
    }

    #[test]
    fn a_built_in_pack_installs_from_the_drivers_view_through_the_review_and_removes_after_asking() {
        let cx = cx("st-drivers-pack", Role::Admin);
        let mut a = StoreApp::new(&cx);
        a.drivers_for = Some(fake_mini(&cx, "st-drivers-pack-mini"));
        a.handle(&Event::Tab(226), &cx);
        let kb = pane_row(&a, "mac-keyboard-options");
        a.handle(&Event::Tab(drv::BASE + kb * 4), &cx);
        let Mode::Review(rv) = &a.mode else { panic!("the review opens: {:?}", a.status_text()) };
        assert!(rv.review.can_install() && rv.review.verdict.level == Level::Official, "{}", rv.review.summary());
        assert!(rv.review.preview.iter().any(|l| l.contains("fnmode=2")));
        key(&mut a, &cx, Key::Char('y'));
        assert!(a.status_text().unwrap().starts_with("Installed"), "{:?}", a.status_text());
        let kb = pane_row(&a, "mac-keyboard-options");
        assert!(a.drivers.rows[kb].installed, "the row says installed after the install");
        assert!(a.inst.iter().any(|e| e.ty == IType::Driver && e.name == "mac-keyboard-options"), "and the chip count has it");
        // Remove asks first
        a.handle(&Event::Tab(drv::BASE + kb * 4 + 1), &cx);
        assert!(matches!(a.mode, Mode::Confirm(_)));
        key(&mut a, &cx, Key::Esc);
        assert!(a.drivers.rows[pane_row(&a, "mac-keyboard-options")].installed);
        a.handle(&Event::Tab(drv::BASE + kb * 4 + 1), &cx);
        key(&mut a, &cx, Key::Char('y'));
        assert!(!a.drivers.rows[pane_row(&a, "mac-keyboard-options")].installed, "{:?}", a.status_text());
    }

    #[test]
    fn firmware_from_a_usb_folder_is_asked_for_reviewed_as_unsigned_and_installed_for_the_machine() {
        let cx = cx("st-drivers-usb", Role::Admin);
        let mut a = StoreApp::new(&cx);
        a.drivers_for = Some(fake_mini(&cx, "st-drivers-usb-mini"));
        let stick = temp_dir("st-drivers-usb-stick");
        std::fs::write(stick.join("BCM20702A1-05ac-8289.hcd"), b"HCD").unwrap();
        a.handle(&Event::Tab(226), &cx);
        let bt = pane_row(&a, "mac-bt-broadcom");
        assert_eq!(a.drivers.rows[bt].status, Some(iyakki_drivers::Status::FirmwareMissing));
        a.handle(&Event::Tab(drv::BASE + bt * 4), &cx);
        assert!(matches!(a.mode, Mode::Form(_, FormKind::DriverFolder(_))), "it asks for the folder");
        for (w, h) in SIZES {
            all_inside(&draw(&mut a, &cx, w, h), w, h, "the folder form");
        }
        // type the folder: the form's first field holds "/mnt/"; replace it
        if let Mode::Form(f, _) = &mut a.mode {
            f.set_value(0, &stick.display().to_string());
        }
        key(&mut a, &cx, Key::Enter);
        let Mode::Review(rv) = &a.mode else { panic!("the review opens: {:?}", a.status_text()) };
        assert_eq!(rv.review.verdict.level, Level::Unsigned);
        assert!(rv.review.needs_warning_ack() && rv.review.preview.iter().any(|l| l.contains("BCM20702A1-05ac-8289.hcd")));
        key(&mut a, &cx, Key::Char('y'));
        assert!(a.status_text().unwrap().starts_with("Installed"), "{:?}", a.status_text());
        let bt = pane_row(&a, "mac-bt-broadcom");
        assert!(a.drivers.rows[bt].installed);
        // the camera asks for the file and the licence answer, and refuses without it
        let _ = bt;
        // a person without the right gets a plain refusal in the review, not a silent install
        let alice = cx_for_user("st-drivers-alice");
        let mut b = StoreApp::new(&alice);
        b.drivers_for = Some(fake_mini(&alice, "st-drivers-alice-mini"));
        b.handle(&Event::Tab(226), &alice);
        let bt = pane_row(&b, "mac-bt-broadcom");
        b.handle(&Event::Tab(drv::BASE + bt * 4), &alice);
        if let Mode::Form(f, _) = &mut b.mode {
            f.set_value(0, &stick.display().to_string());
        }
        key(&mut b, &alice, Key::Enter);
        match &b.mode {
            Mode::Review(rv) => assert!(rv.review.refusal.is_some(), "{}", rv.review.summary()),
            _ => assert!(b.status_text().is_some() || matches!(b.mode, Mode::Form(..)), "refused with a message"),
        }
    }

    #[test]
    fn the_camera_entry_asks_for_apples_file_and_the_licence_answer() {
        let cx = cx("st-drivers-cam", Role::Admin);
        let mut a = StoreApp::new(&cx);
        let root = temp_dir("st-drivers-cam-mbp");
        for (rel, c) in [("sys/class/dmi/id/sys_vendor", "Apple Inc.\n"), ("sys/class/dmi/id/product_name", "MacBookPro11,1\n"), ("sys/bus/pci/devices/0000:02:00.0/vendor", "0x14e4\n"), ("sys/bus/pci/devices/0000:02:00.0/device", "0x1570\n"), ("proc/sys/kernel/osrelease", "6.12.112\n")] {
            let p = root.join(rel);
            std::fs::create_dir_all(p.parent().unwrap()).unwrap();
            std::fs::write(p, c).unwrap();
        }
        a.drivers_for = Some(iyakki_agent::store::drivers::Drivers::for_services(&cx.services).with_roots(iyakki_drivers::Roots { sys: root.join("sys"), proc: root.join("proc") }));
        a.handle(&Event::Tab(226), &cx);
        let cam = pane_row(&a, "mac-facetimehd");
        a.handle(&Event::Tab(drv::BASE + cam * 4), &cx);
        assert!(matches!(a.mode, Mode::Form(_, FormKind::DriverFile(_))));
        let file = root.join("AppleCameraInterface");
        std::fs::write(&file, b"not the real driver").unwrap();
        if let Mode::Form(f, _) = &mut a.mode {
            f.set_value(0, &file.display().to_string());
        }
        key(&mut a, &cx, Key::Enter);
        key(&mut a, &cx, Key::Enter);
        // the licence was not accepted: the form stays open and says why
        let Mode::Form(f, _) = &a.mode else { panic!("still asking") };
        assert!(f.error.as_deref().unwrap_or("").contains("accept"), "{:?}", f.error);
    }


    fn private_store_catalog(tag: &str) -> (String, String, std::path::PathBuf) {
        use iyakki_agent::store::server::{Config, Req, StoreServer};
        let d = std::env::temp_dir().join(format!("iyakki-ui-priv-{}-{tag}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        let srv = StoreServer::open(Config::new(&d), &iyakki_agent::pkg::signing_key_hex(&new_signing_key().unwrap())).unwrap();
        let body = srv.handle(&Req { method: "GET".into(), path: "/catalog.json".into(), query: Default::default(), headers: Default::default(), body: vec![] }, "1.1.1.1").body;
        (String::from_utf8(body).unwrap(), iyakki_agent::store::server::fingerprint(&srv.catalog_key()), d)
    }

    #[test]
    fn the_private_store_card_adds_a_store_only_after_the_fingerprint_is_confirmed_and_everything_fits_at_four_sizes() {
        let cx = cx("st-private", Role::Admin);
        let mut a = StoreApp::new(&cx);
        let (catalog, fingerprint, dir) = private_store_catalog("card");
        let table = Arc::new(Table::default());
        table.files.lock().unwrap().insert("https://store.example.lan/catalog.json".into(), catalog.into_bytes());
        a.fetch = table.clone();
        a.switch(Section::Settings);
        let rows = a.settings_rows(Some(&cx));
        for l in ["Private store", "Deploy a private store to k3s…", "Repair, reset or remove the private store…", "Add a private store (check its key)", "Publish here"] {
            assert!(rows.iter().any(|r| r.0 == l), "{l}");
        }
        assert!(rows.iter().find(|r| r.0 == "Private store").unwrap().1.contains("No private store"));
        for (w, h) in SIZES {
            let g = draw(&mut a, &cx, w, h);
            all_inside(&g, w, h, "private store card");
        }
        // Deploy asks the shell to open the platform wizard of Settings with this service
        key(&mut a, &cx, Key::Char('p'));
        assert_eq!(a.take_request(), Some(("settings".to_string(), "platform:private-store".to_string())));
        // Repair, reset or remove asks the shell to open the same panel on the store's state
        key(&mut a, &cx, Key::Char('m'));
        assert_eq!(a.take_request(), Some(("settings".to_string(), "platform:private-store:manage".to_string())));
        // the address form reads the catalog in the background, then shows the fingerprint; nothing is saved yet
        key(&mut a, &cx, Key::Char('v'));
        assert!(matches!(a.mode, Mode::Form(_, FormKind::PrivateStore)));
        typed(&mut a, &cx, "Team store");
        key(&mut a, &cx, Key::Tab);
        for _ in 0..10 {
            key(&mut a, &cx, Key::Backspace);
        }
        typed(&mut a, &cx, "https://store.example.lan");
        key(&mut a, &cx, Key::Enter);
        wait_job(&mut a, &cx);
        let Mode::Confirm(Ask::PinStore(p, _)) = &a.mode else { panic!("{:?}", a.status_text()) };
        assert_eq!(p.fingerprint, fingerprint);
        assert!(Store::for_services(&cx.services).config().sources.is_empty());
        for (w, h) in SIZES {
            let g = draw(&mut a, &cx, w, h);
            all_inside(&g, w, h, "fingerprint dialog");
            assert!(g.hits.iter().any(|h| h.label == "They match") && g.hits.iter().any(|h| h.label == "Cancel"), "{w}x{h}");
        }
        // Esc cancels: still nothing; y pins the key
        key(&mut a, &cx, Key::Esc);
        assert!(Store::for_services(&cx.services).config().sources.is_empty());
        key(&mut a, &cx, Key::Char('v'));
        key(&mut a, &cx, Key::Tab);
        for _ in 0..10 {
            key(&mut a, &cx, Key::Backspace);
        }
        typed(&mut a, &cx, "https://store.example.lan");
        key(&mut a, &cx, Key::Enter);
        wait_job(&mut a, &cx);
        key(&mut a, &cx, Key::Char('y'));
        let cfg = Store::for_services(&cx.services).config();
        assert_eq!(cfg.sources.len(), 1, "{:?}", a.status_text());
        assert_eq!(cfg.sources[0].pin.len(), 64);
        assert!(a.status_text().unwrap().contains("key pinned"));
        // Publish here makes it the default target
        a.sel[Section::Settings as usize] = 1;
        key(&mut a, &cx, Key::Char('h'));
        assert_eq!(Store::for_services(&cx.services).config().publish_target, cfg.sources[0].id);
        assert!(a.status_text().unwrap().contains("by default"));
        for (w, h) in SIZES {
            all_inside(&draw(&mut a, &cx, w, h), w, h, "card after adding");
        }
        // an address that does not answer is a plain message, not a dialog
        key(&mut a, &cx, Key::Char('v'));
        for _ in 0..30 {
            key(&mut a, &cx, Key::Backspace);
        }
        key(&mut a, &cx, Key::Tab);
        for _ in 0..40 {
            key(&mut a, &cx, Key::Backspace);
        }
        typed(&mut a, &cx, "https://nowhere.example.lan");
        key(&mut a, &cx, Key::Enter);
        wait_job(&mut a, &cx);
        assert!(a.status_text().unwrap().contains("Could not read"), "{:?}", a.status_text());
        // a non-administrator cannot
        let _ = std::fs::remove_dir_all(&dir);
    }


    fn two_stores(a: &mut StoreApp, cx: &AppCx) {
        let k = new_signing_key().unwrap();
        let da = stick("st-two-a", &[("weekly-report", "1.0.0", &pub_meta("Weekly report", Pricing::default()), &k), ("only-in-a", "1.0.0", &pub_meta("Only in A", Pricing::default()), &k)]);
        let db = stick("st-two-b", &[("weekly-report", "2.0.0", &pub_meta("Weekly report", Pricing::default()), &k), ("only-in-b", "1.0.0", &pub_meta("Only in B", Pricing::default()), &k)]);
        a.store.add_source(&Actor::system(), "Store A", SourceKind::Folder, &da.display().to_string()).unwrap();
        a.store.add_source(&Actor::system(), "Store B", SourceKind::Folder, &db.display().to_string()).unwrap();
        a.reload(cx);
        a.switch(Section::Discover);
    }

    fn pick(a: &StoreApp) -> &str {
        &a.store_pick
    }

    #[test]
    fn the_store_switcher_filters_discover_to_one_catalog_by_key_and_click_and_is_remembered_per_user() {
        let cx = cx("st-switch", Role::Admin);
        let mut a = StoreApp::new(&cx);
        // one catalog only: nothing to switch, no chip
        cat_on(&mut a, &cx);
        assert!(!draw(&mut a, &cx, 1280, 720).hits.iter().any(|h| h.ev == Event::Tab(205)));
        key(&mut a, &cx, Key::Char('s'));
        assert!(a.status_text().unwrap().contains("only one store"), "{:?}", a.status_text());
        two_stores(&mut a, &cx);
        let ids = |a: &StoreApp| a.items.iter().map(|i| i.entry.id.clone()).collect::<Vec<_>>();
        assert!(ids(&a).contains(&"only-in-a".to_string()) && ids(&a).contains(&"only-in-b".to_string()));
        assert_eq!(a.items.iter().find(|i| i.entry.id == "weekly-report").unwrap().entry.version, "2.0.0", "All shows the newest");
        for (w, h) in SIZES {
            let g = draw(&mut a, &cx, w, h);
            let chip = g.hits.iter().find(|x| x.ev == Event::Tab(205)).unwrap_or_else(|| panic!("{w}x{h}: no store chip"));
            assert!(chip.label.starts_with("Store: All"), "{}", chip.label);
            all_inside(&g, w, h, "store chip");
            no_overlap(&g, w, h, |e| matches!(e, Event::Row(_) | Event::Tab(200..=205) | Event::Focus(_)), "rows, chips and the search box");
        }
        // the key walks: built-in, Store A, Store B, back to All
        let mut seen = vec![];
        for _ in 0..4 {
            key(&mut a, &cx, Key::Char('s'));
            seen.push(pick(&a).to_string());
        }
        assert_eq!(seen, ["builtin", "src1", "src2", ""]);
        // a click does the same, at every size
        for (w, h) in SIZES {
            a.set_store(&cx, String::new());
            click_ev(&mut a, &cx, w, h, Event::Tab(205));
            click_ev(&mut a, &cx, w, h, Event::Tab(205));
            assert_eq!(pick(&a), "src1", "{w}x{h}");
            let g = draw(&mut a, &cx, w, h);
            assert!(g.hits.iter().any(|x| x.ev == Event::Tab(205) && x.label == "Store: Store A (s)"), "{w}x{h}");
            all_inside(&g, w, h, "chip when filtered");
        }
        // only that catalog's apps are shown
        assert_eq!(a.items.iter().filter(|i| i.entry.id != "weekly-report" && i.source != "builtin").map(|i| i.entry.id.as_str()).collect::<Vec<_>>(), ["only-in-a"]);
        assert_eq!(a.items.iter().find(|i| i.entry.id == "weekly-report").unwrap().entry.version, "1.0.0");
        // remembered for this user, not for another
        let b = StoreApp::new(&cx);
        assert_eq!(pick(&b), "src1");
        let mut other = cx.clone();
        other.session.user = "someone-else".into();
        assert_eq!(pick(&StoreApp::new(&other)), "");
        // removing the picked store goes back to All
        a.store.remove_source(&Actor::system(), "src1").unwrap();
        a.reload(&cx);
        assert_eq!(pick(&a), "");
    }

    #[test]
    fn installing_from_a_chosen_store_uses_that_catalog_and_an_app_in_two_stores_shows_where_it_comes_from_and_lets_the_person_pick() {
        let cx = cx("st-twoinst", Role::Admin);
        let mut a = StoreApp::new(&cx);
        cat_on(&mut a, &cx);
        two_stores(&mut a, &cx);
        // All: the newest wins, the page says so and names the other store
        open_page(&mut a, &cx, "weekly-report");
        for (w, h) in SIZES {
            let g = draw(&mut a, &cx, w, h);
            assert!(g.hits.iter().any(|x| x.ev == Event::Tab(205) && x.label.starts_with("Store: Store B")), "{w}x{h}: {:?}", g.hits.iter().map(|x| &x.label).collect::<Vec<_>>());
            all_inside(&g, w, h, "app page with two stores");
            no_overlap(&g, w, h, |e| matches!(e, Event::Tab(300..=307) | Event::Tab(205)), "the buttons");
        }
        let lines = a.text.lines.iter().map(|l| l.0.clone()).collect::<Vec<_>>();
        assert!(lines.iter().any(|l| l == "Catalog: Store B") && lines.iter().any(|l| l == "Also in: Store A 1.0.0"), "{lines:?}");
        // the person picks the other store: the page is that store's version
        click_ev(&mut a, &cx, 1280, 720, Event::Tab(205));
        assert_eq!((pick(&a), a.current().unwrap().entry.version.as_str(), a.current().unwrap().source.as_str()), ("src1", "1.0.0", "src1"));
        assert!(matches!(a.mode, Mode::Detail));
        a.text_for = None;
        draw(&mut a, &cx, 1280, 720);
        let lines = a.text.lines.iter().map(|l| l.0.clone()).collect::<Vec<_>>();
        assert!(lines.iter().any(|l| l == "Catalog: Store A") && lines.iter().any(|l| l == "Also in: Store B 2.0.0"), "{lines:?}");
        // installing takes Store A's package, not the newer one in Store B
        key(&mut a, &cx, Key::Char('i'));
        let Mode::Review(rv) = &a.mode else { panic!("{:?}", a.status_text()) };
        assert_eq!(rv.review.version, "1.0.0");
        key(&mut a, &cx, Key::Char('y'));
        assert!(a.status_text().unwrap().starts_with("Installed"), "{:?}", a.status_text());
        let rec = Store::for_services(&cx.services).records().into_values().find(|r| r.name == "weekly-report").unwrap();
        assert_eq!((rec.version.as_str(), rec.source.as_str()), ("1.0.0", "src1"));
        // an app only one store lists has no switch
        a.detail = None;
        a.mode = Mode::Browse;
        open_page(&mut a, &cx, "only-in-a");
        assert!(!draw(&mut a, &cx, 1280, 720).hits.iter().any(|x| x.ev == Event::Tab(205)));
        key(&mut a, &cx, Key::Char('s'));
        assert!(a.status_text().unwrap().contains("Only one store lists"));
    }

    #[test]
    fn a_store_is_added_from_a_portal_link_only_after_its_name_signer_and_trust_are_shown_and_confirmed() {
        let cx = cx("st-portal", Role::Admin);
        let mut a = StoreApp::new(&cx);
        let (catalog, fingerprint, dir) = private_store_catalog("portal");
        let table = Arc::new(Table::default());
        table.files.lock().unwrap().insert("https://portal.example.org/store/catalog.json".into(), catalog.into_bytes());
        a.fetch = table.clone();
        a.switch(Section::Settings);
        let rows = a.settings_rows(Some(&cx));
        assert!(rows.iter().any(|r| r.0 == "Add a store") && rows.iter().any(|r| r.0 == "Add the official IYAKKI store"));
        // a page link ending in / resolves to <link>/catalog.json
        assert_eq!(Store::catalog_url("https://portal.example.org/store/"), "https://portal.example.org/store/catalog.json");
        assert_eq!(Store::catalog_url("https://portal.example.org/store/catalog.json"), "https://portal.example.org/store/catalog.json");
        key(&mut a, &cx, Key::Char('a'));
        assert!(matches!(a.mode, Mode::Form(_, FormKind::PrivateStore)));
        for (w, h) in SIZES {
            all_inside(&draw(&mut a, &cx, w, h), w, h, "add a store form");
        }
        key(&mut a, &cx, Key::Tab);
        for _ in 0..10 {
            key(&mut a, &cx, Key::Backspace);
        }
        typed(&mut a, &cx, "https://portal.example.org/store/");
        key(&mut a, &cx, Key::Enter);
        wait_job(&mut a, &cx);
        let Mode::Confirm(Ask::PinStore(p, _)) = &a.mode else { panic!("{:?}", a.status_text()) };
        assert_eq!((p.fingerprint.as_str(), p.url.as_str()), (fingerprint.as_str(), "https://portal.example.org/store/catalog.json"));
        assert!(p.trust.starts_with("Not vouched for by IYAKKI"), "{}", p.trust);
        assert!(Store::for_services(&cx.services).config().sources.is_empty(), "nothing is enabled before the confirmation");
        for (w, h) in SIZES {
            let g = draw(&mut a, &cx, w, h);
            all_inside(&g, w, h, "store confirmation");
            assert!(g.hits.iter().any(|x| x.label == "They match") && g.hits.iter().any(|x| x.label == "Cancel"), "{w}x{h}");
        }
        key(&mut a, &cx, Key::Esc);
        assert!(Store::for_services(&cx.services).config().sources.is_empty());
        // again, and confirm
        key(&mut a, &cx, Key::Char('a'));
        key(&mut a, &cx, Key::Tab);
        for _ in 0..10 {
            key(&mut a, &cx, Key::Backspace);
        }
        typed(&mut a, &cx, "https://portal.example.org/store/catalog.json");
        key(&mut a, &cx, Key::Enter);
        wait_job(&mut a, &cx);
        key(&mut a, &cx, Key::Char('y'));
        let cfg = Store::for_services(&cx.services).config();
        assert_eq!((cfg.sources.len(), cfg.sources[0].pin.len(), cfg.sources[0].enabled), (1, 64, true), "{:?}", a.status_text());
        // an address that is not a web link is refused before anything is read
        key(&mut a, &cx, Key::Char('a'));
        key(&mut a, &cx, Key::Tab);
        for _ in 0..10 {
            key(&mut a, &cx, Key::Backspace);
        }
        typed(&mut a, &cx, "ftp://x.example/");
        key(&mut a, &cx, Key::Enter);
        wait_job(&mut a, &cx);
        assert!(a.status_text().unwrap().contains("starts with https://"), "{:?}", a.status_text());
        // the official store: one step, but its address is not published yet
        assert!(iyakki_agent::store::catalog::OFFICIAL_STORE_URL.is_empty() && Store::official_store_url().is_none());
        key(&mut a, &cx, Key::Char('w'));
        assert!(a.job.is_none() && a.status_text().unwrap().contains("coming soon"), "{:?}", a.status_text());
        assert_eq!(Store::for_services(&cx.services).config().sources.len(), 1);
        // an ordinary account cannot add stores
        let user = self::cx("st-portal-user", Role::User);
        let mut b = StoreApp::new(&user);
        b.switch(Section::Settings);
        key(&mut b, &user, Key::Char('a'));
        assert!(matches!(b.mode, Mode::Browse) && b.status_text().unwrap().contains("administrators"));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
