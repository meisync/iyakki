//! DASH: dashboards with graphs and tables over pluggable data sources (Prometheus, Elasticsearch, ClickHouse, DuckDB, and
//! whatever the connector seam gets next; see `docs/DASHBOARD.md`), with the assistant's chat beside them.
//!
//! The app is thin: the model, the connectors and the assistant's tools live in `iyakki_agent::dash`; the chart drawing is
//! `dash_charts`; the query box is `dash_query`. This file holds the screens (board, dashboards, sources, panel editor, table
//! view), the workers that ask the sources (never on the UI thread), the refresh timer, the mouse work (hover, zoom by dragging,
//! move and resize) and the chat pane. Credentials never pass through here: a source names a vault entry and the connector
//! reads it when it asks.

use crate::safeclamp::SafeClamp;
use std::collections::{HashMap, VecDeque};
use std::hash::{Hash, Hasher};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::Arc;

use iyakki_agent::dash::connector::{Cancel, DashError, FieldKind as CField, KindInfo, Node, NodeKind, SourceConfig, TestInfo};
use iyakki_agent::dash::engine::{self, RunCtx};
use iyakki_agent::dash::multi::Outcome;
use iyakki_agent::dash::frame::{format_time, Frame};
use iyakki_agent::dash::model::*;
use iyakki_agent::dash::session::{shared, Shared};
use iyakki_agent::dash::store::{Dashboards, Sources};
use iyakki_agent::dash::templates;
use iyakki_agent::dash::tools::Env;
use iyakki_agent::dash::registry;

use super::chat::{ChatApp, DashLink};
use super::component::route;
use super::dash_charts as ch;
use super::dash_query::QueryBox;
use super::widgets::{Form, FormEvent, PickEvent, Picker};
use super::{App, AppCx, Flow};
use crate::ctxmenu::Item;
use crate::fx::{self, with_alpha, MAGENTA};
use crate::gfx::{rgb, Face, Gfx, Icon};
use crate::ui::{Event, Key, TextField, DIM, ERR, MUTED, OK, R, TEXT};

pub const ACCENT: [u8; 4] = rgb(86, 200, 255);
/// The chat pane's click-id slot (the IDE uses 1 to 5).
pub const CHAT_SLOT: usize = 6;
const MAX_JOBS: usize = 4;
const GAP: f32 = 8.0;

/// Click ids. Everything is below one million except the chat's slot.
pub mod id {
    pub const RANGE: usize = 100; // + index in RANGE_CHOICES
    pub const REFRESH: usize = 120;
    pub const NOW: usize = 121;
    pub const ADD: usize = 122;
    pub const SOURCES: usize = 123;
    pub const UNDO: usize = 124;
    pub const REDO: usize = 125;
    pub const SAVE: usize = 126;
    pub const AI: usize = 127;
    pub const LIST: usize = 128;
    pub const ZOOM_OUT: usize = 129;
    pub const EXPORT: usize = 130;
    pub const RENAME: usize = 131;
    pub const BACK: usize = 132;
    pub const COPY: usize = 133;
    pub const EXPORT_CSV: usize = 134;
    pub const AI_HIDE: usize = 135;
    pub const CHAT_PANE: usize = 136;
    pub const VAR: usize = 140; // + variable index
    pub const NEW: usize = 160;
    pub const IMPORT: usize = 161;
    pub const OPEN: usize = 162;
    pub const DELETE: usize = 163;
    pub const S_ADD: usize = 170;
    pub const S_EDIT: usize = 171;
    pub const S_ADV: usize = 172;
    pub const S_TEST: usize = 173;
    pub const S_DEL: usize = 174;
    pub const E_RUN: usize = 180;
    pub const E_APPLY: usize = 181;
    pub const E_CANCEL: usize = 182;
    pub const E_UNIT: usize = 183;
    pub const E_STARTER: usize = 184;
    pub const E_PAGE: usize = 185; // + 0..3
    pub const E_UP: usize = 189;
    pub const E_TOP: usize = 190;
    pub const E_OVER: usize = 191;
    pub const E_RUN2: usize = 192;
    pub const E_SRC: usize = 200; // + source index
    pub const E_VIS: usize = 260; // + visual index
    pub const SEL: usize = 1000; // + panel index
    pub const EDIT: usize = 1100;
    pub const VIEW: usize = 1200;
    pub const ASK: usize = 1300;
    pub const ROW_LIST: usize = 0; // Row(n)
    pub const ROW_SRC: usize = 5000;
    pub const ROW_NODE: usize = 6000;
    pub const MOVE: usize = 600_000;
    pub const SIZE: usize = 601_000;
    pub const QUERY: usize = 602_000;
    pub const EXPRS: usize = 602_001;
    pub const E_Q: usize = 300; // + query index
    pub const E_QADD: usize = 310;
    pub const E_QDEL: usize = 311;
    pub const E_EXPR: usize = 312;
    pub const E_AXIS: usize = 313;
    pub const E_HIDE: usize = 314;
    pub const E_RUN_ONE: usize = 315;
    pub const S_DEFAULT: usize = 175;
    pub const F_TITLE: usize = 1; // Focus ids
    pub const F_TARGET: usize = 2;
    pub const F_LEGEND: usize = 3;
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum Screen {
    Board,
    List,
    Sources,
    Editor,
    Zoom(String),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Focus {
    Board,
    Chat,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum EdFocus {
    Title,
    Target,
    Legend,
    Query,
    Nodes,
}

/// What a panel last got from its source.
#[derive(Default)]
struct PData {
    frame: Option<Arc<Frame>>,
    model: Option<ch::Model>,
    err: Option<String>,
    /// Parts of the panel that failed while the rest has data (one line each).
    qerrors: Vec<String>,
    loading: bool,
    key: u64,
    gen: u64,
    at_ms: i64,
    range: (i64, i64),
    hidden: Vec<String>,
    table: ch::TableView,
}

enum Msg {
    Panel { id: String, gen: u64, range: (i64, i64), out: Outcome },
    Preview { gen: u64, range: (i64, i64), out: Outcome },
    Nodes { gen: u64, path: Vec<String>, res: Result<Vec<Node>, DashError> },
    Test { id: String, res: Result<TestInfo, DashError> },
    Var { name: String, gen: u64, res: Result<Vec<String>, DashError> },
}

enum Drag {
    Move { id: String, from: (i32, i32), orig: Grid },
    Size { id: String, from: (i32, i32), orig: Grid },
    Zoom { idx: usize, x0: f32, x1: f32 },
}

enum Modal {
    Kind(Picker),
    Source { form: Form, kind: String, editing: Option<String>, advanced: bool, secret_keys: Vec<(usize, String)> },
    Name(Form, NameFor),
    /// A new vault entry typed here goes straight to the vault; the source form waits behind it and gets only the reference.
    NewSecret { form: Form, src: Form, kind: String, editing: Option<String>, advanced: bool, keys: Vec<(usize, String)>, key: String, field: usize },
    Confirm(String, Confirmed),
    Template(Picker, Vec<String>, String),
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum NameFor {
    Rename,
    Import,
    New,
}

#[derive(Clone, PartialEq, Eq)]
enum Confirmed {
    DeleteSource(String),
    DeleteDashboard(String),
    Leave,
    DiscardEditor,
}

/// The panel editor: a working copy of the panel, its fields, the schema list and the preview.
struct Ed {
    panel: Panel,
    is_new: bool,
    title: TextField,
    target: TextField,
    legend: TextField,
    query: QueryBox,
    focus: EdFocus,
    page: usize,
    path: Vec<String>,
    nodes: Vec<Node>,
    nodes_err: Option<String>,
    nodes_loading: bool,
    node_sel: usize,
    node_gen: u64,
    pd: PData,
    changed: bool,
    /// Which query the fields show (0 is A, 1 is B, ...).
    cur: usize,
    /// The expressions (one per line) and whether they are shown instead of the query.
    exprs: QueryBox,
    show_expr: bool,
    /// The preview ran the current query alone.
    only_cur: bool,
}

impl Ed {
    /// The source and the query settings of query `i` of the panel.
    fn slot(panel: &mut Panel, i: usize) -> (&mut String, &mut PanelQuery) {
        if i == 0 || i > panel.queries.len() {
            (&mut panel.source, &mut panel.query)
        } else {
            let q = &mut panel.queries[i - 1];
            (&mut q.source, &mut q.query)
        }
    }

    fn cur_source(&self) -> String {
        if self.cur == 0 || self.cur > self.panel.queries.len() { self.panel.source.clone() } else { self.panel.queries[self.cur - 1].source.clone() }
    }

    fn cur_query(&self) -> &PanelQuery {
        if self.cur == 0 || self.cur > self.panel.queries.len() { &self.panel.query } else { &self.panel.queries[self.cur - 1].query }
    }

    /// Put the fields into the panel (the current query's slot) and the expressions.
    fn store(&mut self) {
        self.panel.title = self.title.value.trim().to_string();
        let (target, alias, text) = (Some(self.target.value.trim().to_string()).filter(|t| !t.is_empty()), self.legend.value.clone(), self.query.text());
        let cur = self.cur;
        let (_, q) = Ed::slot(&mut self.panel, cur);
        q.target = target;
        q.alias = alias.clone();
        q.text = text;
        if cur == 0 {
            self.panel.options.legend_format = alias;
        }
        self.panel.expressions = self.exprs.text().lines().map(|l| l.trim().to_string()).filter(|l| !l.is_empty()).collect();
    }

    /// Show query `i` in the fields.
    fn load(&mut self, i: usize) {
        self.store();
        self.cur = i.min(self.panel.queries.len());
        let q = self.cur_query().clone();
        self.target.set(q.target.as_deref().unwrap_or(""));
        self.legend.set(&if q.alias.is_empty() && self.cur == 0 { self.panel.options.legend_format.clone() } else { q.alias.clone() });
        self.query.set_text(&q.text);
    }

    /// The panel as the preview ran it: all of it, or only the current query without expressions.
    fn preview_panel(&self) -> Panel {
        let mut p = self.panel.clone();
        p.title = self.title.value.clone();
        if self.only_cur {
            let (src, q) = (self.cur_source(), self.cur_query().clone());
            p.source = src;
            p.query = PanelQuery { hidden: false, ..q };
            p.queries.clear();
            p.expressions.clear();
        }
        p
    }
}

/// Rectangles of the last frame, so events can be mapped to what is under the pointer.
#[derive(Default)]
struct Geo {
    area: Option<R>,
    board: Option<R>,
    chat: Option<R>,
    panels: Vec<(usize, R)>,
    plots: HashMap<usize, ch::Plot>,
    col_w: f32,
    row_h: f32,
}

pub struct DashApp {
    env: Arc<Env>,
    state: Shared,
    d: Dashboard,
    seen_rev: u64,
    screen: Screen,
    sources: Vec<SourceConfig>,
    src_sel: usize,
    src_status: HashMap<String, (bool, String)>,
    list: Vec<iyakki_agent::dash::store::Entry>,
    list_sel: usize,
    data: HashMap<String, PData>,
    var_opts: HashMap<String, Vec<String>>,
    tx: Sender<Msg>,
    rx: Receiver<Msg>,
    cancel: Cancel,
    queue: VecDeque<String>,
    inflight: usize,
    tick: u64,
    last_run_tick: u64,
    mouse: (i32, i32),
    drag: Option<Drag>,
    modal: Option<Modal>,
    chat: ChatApp,
    chat_open: bool,
    focus: Focus,
    ed: Option<Box<Ed>>,
    status: Option<(String, bool)>,
    scroll: f32,
    flash: Option<(String, u64)>,
    redraw: bool,
    last_label_s: i64,
    geo: Geo,
    ver: u64,
    host_save: Option<(String, Vec<u8>)>,
    cur_ctx: Option<RunCtx>,
    import_notes: Vec<String>,
}

fn hash_of<T: Hash>(t: &T) -> u64 {
    let mut h = std::collections::hash_map::DefaultHasher::new();
    t.hash(&mut h);
    h.finish()
}

impl Drop for DashApp {
    fn drop(&mut self) {
        // leaving the tile stops what is still being asked
        self.cancel.cancel();
    }
}

impl DashApp {
    pub fn new(cx: &AppCx) -> DashApp {
        let mut env = Env::from_services(&cx.services, &cx.session.user, &cx.session.policy);
        // names and ids only reach the audit log, never a value
        let (audit, user) = (cx.audit.clone(), cx.session.user.clone());
        env.audit = Arc::new(move |kind, detail| audit.record(&user, kind, detail));
        DashApp::with_env(cx, Arc::new(env))
    }

    /// Tests give the app an environment with fake connectors.
    pub fn with_env(cx: &AppCx, env: Arc<Env>) -> DashApp {
        let first = Dashboard::new("", "Untitled dashboard");
        let state = shared(first.clone());
        let chat = ChatApp::new().with_agent(cx.session.policy.agent).with_dash(DashLink { env: env.clone(), state: state.clone() }).resumed(cx);
        let (tx, rx) = channel();
        let mut a = DashApp {
            d: first,
            seen_rev: 0,
            env,
            state,
            screen: Screen::List,
            sources: vec![],
            src_sel: 0,
            src_status: HashMap::new(),
            list: vec![],
            list_sel: 0,
            data: HashMap::new(),
            var_opts: HashMap::new(),
            tx,
            rx,
            cancel: Cancel::new(),
            queue: VecDeque::new(),
            inflight: 0,
            tick: 0,
            last_run_tick: 0,
            mouse: (0, 0),
            drag: None,
            modal: None,
            chat,
            chat_open: false,
            focus: Focus::Board,
            ed: None,
            status: None,
            scroll: 0.0,
            flash: None,
            redraw: true,
            last_label_s: 0,
            geo: Geo::default(),
            ver: 0,
            host_save: None,
            cur_ctx: None,
            import_notes: vec![],
        };
        a.reload_lists();
        a
    }

    pub fn state(&self) -> Shared {
        self.state.clone()
    }

    fn say(&mut self, t: impl Into<String>, ok: bool) {
        self.status = Some((t.into(), ok));
        self.redraw = true;
    }

    fn now(&self) -> i64 {
        self.env.now()
    }

    fn reload_lists(&mut self) {
        self.sources = self.env.sources().list();
        self.src_sel = self.src_sel.min(self.sources.len().saturating_sub(1));
        self.list = self.env.dashboards().list();
        self.list_sel = self.list_sel.min((self.list.len() + templates::all().len()).saturating_sub(1));
    }

    fn kind_info(&self, source: &str) -> Option<KindInfo> {
        self.sources.iter().find(|s| s.id == source).and_then(|s| registry::kind(&s.kind))
    }

    // ------------------------------------------------------------------ the shared document

    /// Pull the dashboard from the shared state when it changed (an edit by the person or by the assistant).
    fn sync(&mut self) {
        let (rev, hl) = {
            let mut st = self.state.lock().unwrap_or_else(|e| e.into_inner());
            if st.rev != self.seen_rev {
                self.d = st.dashboard.clone();
            }
            let hl = st.highlight.take();
            (st.rev, hl)
        };
        if let Some(id) = hl {
            self.flash = Some((id.clone(), self.tick + 75));
            self.scroll_to(&id);
            self.redraw = true;
        }
        if rev != self.seen_rev {
            self.seen_rev = rev;
            self.redraw = true;
            self.data.retain(|k, _| self.d.panel(k).is_some());
            if self.screen == Screen::Board || matches!(self.screen, Screen::Zoom(_)) {
                self.schedule(false);
            }
        }
    }

    fn edit<T>(&mut self, f: impl FnOnce(&mut Dashboard) -> Result<T, String>) -> Result<T, String> {
        let r = self.state.lock().unwrap_or_else(|e| e.into_inner()).edit(f);
        self.sync();
        r
    }

    fn selected(&self) -> Option<usize> {
        let id = self.state.lock().unwrap_or_else(|e| e.into_inner()).selected.clone()?;
        self.d.panels.iter().position(|p| p.id == id)
    }

    fn select(&mut self, idx: Option<usize>) {
        let id = idx.and_then(|i| self.d.panels.get(i)).map(|p| p.id.clone());
        self.state.lock().unwrap_or_else(|e| e.into_inner()).selected = id;
        self.redraw = true;
    }

    fn scroll_to(&mut self, id: &str) {
        if let (Some(p), Some(b)) = (self.d.panel(id), self.geo.board) {
            let row_h = self.geo.row_h.max(1.0);
            let (top, bottom) = (p.grid.y as f32 * row_h, (p.grid.y + p.grid.h) as f32 * row_h);
            if top < self.scroll {
                self.scroll = top;
            } else if bottom > self.scroll + b.h {
                self.scroll = bottom - b.h;
            }
        }
    }

    // ------------------------------------------------------------------ asking the sources

    fn make_ctx(&self, width_px: usize) -> RunCtx {
        let mut c = RunCtx::new(resolve_range(&self.d.time, self.now()));
        c.width_px = width_px.max(100);
        c.timeout_s = Some(30);
        for v in &self.d.variables {
            let opts = self.var_opts.get(&v.name).cloned().unwrap_or_else(|| match &v.kind {
                VarKind::Custom { options } | VarKind::Interval { options } => options.clone(),
                VarKind::Query { .. } => v.current.clone(),
            });
            let vals = engine::variable_values(v, &opts);
            if !vals.is_empty() {
                c.vars.insert(v.name.clone(), vals);
            }
            if !v.mapping.is_empty() {
                c.var_maps.insert(v.name.clone(), v.mapping.clone());
            }
        }
        c
    }

    fn panel_width(&self, idx: usize) -> usize {
        self.geo.panels.iter().find(|p| p.0 == idx).map_or(600, |p| p.1.w as usize)
    }

    fn key_of(&self, p: &Panel, ctx: &RunCtx) -> u64 {
        hash_of(&format!("{:?}|{:?}|{:?}|{}|{}|{}|{:?}|{:?}|{}|{}", (&p.source, &p.query, &p.queries, &p.expressions), (p.visual.is_timeseries(), matches!(p.visual, Visual::Table), &p.options.legend_format), (&self.d.time.from, &self.d.time.to), self.d.default_source, "", "", ctx.vars, ctx.var_maps, "", ""))
    }

    fn runnable(&self, p: &Panel) -> bool {
        p.visual.needs_query() && p.query_defs(&self.d.default_source).iter().any(|(_, q)| !q.query.text.trim().is_empty())
    }

    /// Queue the panels whose question changed (or all of them with `force`, which also forgets the shared answers).
    fn schedule(&mut self, force: bool) {
        if force {
            self.env.cache.clear();
        }
        let ctx = self.make_ctx(600);
        let panels: Vec<Panel> = self.d.panels.clone();
        for p in panels {
            if !self.runnable(&p) {
                self.data.remove(&p.id);
                continue;
            }
            let key = self.key_of(&p, &ctx);
            let pd = self.data.entry(p.id.clone()).or_default();
            let fresh = pd.key == key && (pd.frame.is_some() || pd.err.is_some() || pd.loading);
            if force || !fresh {
                pd.key = key;
                pd.gen += 1;
                pd.loading = true;
                if !self.queue.contains(&p.id) {
                    self.queue.push_back(p.id.clone());
                }
            }
        }
        self.cur_ctx = Some(ctx);
        self.last_run_tick = self.tick;
        self.pump();
        self.redraw = true;
    }

    fn pump(&mut self) {
        while self.inflight < MAX_JOBS {
            let Some(id) = self.queue.pop_front() else { break };
            let Some(p) = self.d.panel(&id).cloned() else { continue };
            let idx = self.d.panels.iter().position(|x| x.id == id).unwrap_or(0);
            // one range for every panel of a refresh, so identical questions of different panels are one fetch
            let mut ctx = self.cur_ctx.clone().unwrap_or_else(|| self.make_ctx(600));
            ctx.width_px = self.panel_width(idx).max(100);
            ctx.timeout_s = Some(30);
            let gen = self.data.get(&id).map_or(0, |d| d.gen);
            let (env, tx, cancel, dsrc) = (self.env.clone(), self.tx.clone(), self.cancel.clone(), self.d.default_source.clone());
            let range = (ctx.range.from_ms, ctx.range.to_ms);
            self.inflight += 1;
            std::thread::spawn(move || {
                let out = env.run_panel(&p, &dsrc, &ctx, &cancel);
                let _ = tx.send(Msg::Panel { id, gen, range, out });
            });
        }
    }

    fn build_model(&self, p: &Panel, pd: &mut PData) {
        let Some(f) = &pd.frame else { return };
        let mut o = p.options.clone();
        o.hidden = pd.hidden.clone();
        pd.model = Some(ch::Model::build(f, p.visual, &o, pd.range));
    }

    /// Take what the workers have brought.
    fn poll(&mut self) {
        while let Ok(m) = self.rx.try_recv() {
            self.redraw = true;
            match m {
                Msg::Panel { id, gen, range, out } => {
                    self.inflight = self.inflight.saturating_sub(1);
                    let now = self.now();
                    let Some(p) = self.d.panel(&id).cloned() else { continue };
                    let Some(pd) = self.data.get_mut(&id) else { continue };
                    if pd.gen != gen {
                        continue;
                    }
                    pd.loading = false;
                    pd.at_ms = now;
                    pd.range = range;
                    let problems = out.problems();
                    if out.failed() {
                        let text = problems.join("; ");
                        pd.err = Some(text.clone());
                        pd.qerrors.clear();
                        self.state.lock().unwrap_or_else(|e| e.into_inner()).note_error(&id, &p.title, &text, now);
                    } else {
                        pd.err = None;
                        pd.qerrors = problems.clone();
                        pd.frame = Some(Arc::new(out.frame));
                        let mut tmp = std::mem::take(pd);
                        self.build_model(&p, &mut tmp);
                        let summary = tmp.frame.clone();
                        self.data.insert(id.clone(), tmp);
                        let mut st = self.state.lock().unwrap_or_else(|e| e.into_inner());
                        if let Some(f) = summary {
                            st.note_result(&id, &f, now);
                        }
                        for pr in &problems {
                            st.errors.push_back(format!("{}: {}", p.title, iyakki_agent::dash::frame::clip(pr, 140)));
                            while st.errors.len() > 5 {
                                st.errors.pop_front();
                            }
                        }
                    }
                    self.pump();
                }
                Msg::Preview { gen, range, out } => {
                    self.inflight = self.inflight.saturating_sub(1);
                    let Some(ed) = self.ed.as_mut() else { continue };
                    if ed.pd.gen != gen {
                        continue;
                    }
                    ed.pd.loading = false;
                    ed.pd.range = range;
                    if out.failed() {
                        ed.pd.err = Some(out.problems().join("; "));
                        ed.pd.qerrors.clear();
                    } else {
                        ed.pd.err = None;
                        ed.pd.qerrors = out.problems();
                        ed.pd.frame = Some(Arc::new(out.frame));
                        let p = ed.preview_panel();
                        let mut tmp = std::mem::take(&mut ed.pd);
                        self.build_model(&p, &mut tmp);
                        if let Some(ed) = self.ed.as_mut() {
                            ed.pd = tmp;
                        }
                    }
                }
                Msg::Nodes { gen, path, res } => {
                    self.inflight = self.inflight.saturating_sub(1);
                    let Some(ed) = self.ed.as_mut() else { continue };
                    if ed.node_gen != gen {
                        continue;
                    }
                    ed.nodes_loading = false;
                    ed.path = path;
                    match res {
                        Ok(n) => {
                            ed.nodes = n;
                            ed.nodes_err = None;
                            ed.node_sel = 0;
                        }
                        Err(e) => {
                            ed.nodes.clear();
                            ed.nodes_err = Some(e.to_string());
                        }
                    }
                }
                Msg::Test { id, res } => {
                    self.inflight = self.inflight.saturating_sub(1);
                    self.src_status.insert(id, match res {
                        Ok(t) => (true, t.detail),
                        Err(e) => (false, e.to_string()),
                    });
                }
                Msg::Var { name, gen, res } => {
                    self.inflight = self.inflight.saturating_sub(1);
                    let _ = gen;
                    if let Ok(o) = res {
                        self.var_opts.insert(name, o);
                        self.schedule(true);
                    }
                }
            }
        }
    }

    /// Resolve the options of query variables (once per dashboard opened).
    fn load_variables(&mut self) {
        let ctx = self.make_ctx(600);
        for v in self.d.variables.clone() {
            if let VarKind::Query { source, .. } = &v.kind {
                let (env, tx, cancel, ctx, source) = (self.env.clone(), self.tx.clone(), self.cancel.clone(), ctx.clone(), source.clone());
                self.inflight += 1;
                std::thread::spawn(move || {
                    let res = env.source(&source).and_then(|(_, src)| engine::variable_options(&v, Some(src.as_ref()), &ctx, &cancel));
                    let _ = tx.send(Msg::Var { name: v.name.clone(), gen: 0, res });
                });
            }
        }
    }

    fn open_dashboard(&mut self, d: Dashboard) {
        self.cancel.cancel();
        self.cancel = Cancel::new();
        self.queue.clear();
        self.inflight = 0;
        self.data.clear();
        self.var_opts.clear();
        self.scroll = 0.0;
        self.state.lock().unwrap_or_else(|e| e.into_inner()).load(d);
        self.sync();
        self.screen = Screen::Board;
        self.load_variables();
        self.schedule(true);
    }

    fn leave_board(&mut self) -> bool {
        let dirty = self.state.lock().unwrap_or_else(|e| e.into_inner()).dirty() && !self.d.panels.is_empty();
        if dirty {
            self.modal = Some(Modal::Confirm("This dashboard has unsaved changes. Leave and lose them?".into(), Confirmed::Leave));
            return false;
        }
        true
    }

    fn save(&mut self) {
        let dashes = self.env.dashboards();
        let r = {
            let mut st = self.state.lock().unwrap_or_else(|e| e.into_inner());
            if st.dashboard.id.is_empty() {
                let id = dashes.new_id(&st.dashboard.title);
                st.dashboard.id = id;
            }
            let r = dashes.save(&st.dashboard);
            if r.is_ok() {
                st.mark_saved();
            }
            r
        };
        self.sync();
        match r {
            Ok(()) => {
                self.reload_lists();
                self.say(format!("Saved “{}” in your Dashboards folder.", self.d.title), true);
            }
            Err(e) => self.say(e, false),
        }
    }

    // ------------------------------------------------------------------ actions

    fn set_range(&mut self, from: &str, to: &str) {
        let (f, t) = (from.to_string(), to.to_string());
        let _ = self.edit(|d| {
            d.time = TimeSpec { from: f, to: t };
            Ok(())
        });
        self.schedule(true);
    }

    fn zoom_out(&mut self) {
        let now = self.now();
        let r = resolve_range(&self.d.time, now);
        let len = r.len_ms();
        let (from, to) = (r.from_ms - len / 2, r.to_ms + len / 2);
        if self.d.time.to == "now" && to > now {
            let span = to - from;
            self.set_range(&format!("now-{}", duration_text(span.min(366 * 86_400_000))), "now");
        } else {
            self.set_range(&from.to_string(), &to.to_string());
        }
    }

    fn cycle_refresh(&mut self) {
        let cur = REFRESH_CHOICES.iter().position(|r| *r == self.d.refresh_s).unwrap_or(0);
        let next = REFRESH_CHOICES[(cur + 1) % REFRESH_CHOICES.len()];
        let _ = self.edit(|d| {
            d.refresh_s = next;
            Ok(())
        });
    }

    fn add_panel_form(&mut self, cx: &AppCx) {
        let src = self.sources.first().map(|s| s.id.clone()).unwrap_or_default();
        let p = Panel::new("", "", &src, Visual::Line, "", Grid::new(0, 0, 12, 8));
        self.open_editor(p, true, cx);
    }

    fn remove_selected(&mut self) {
        if let Some(i) = self.selected() {
            let id = self.d.panels[i].id.clone();
            let _ = self.edit(|d| Ok(d.remove_panel(&id)));
            self.select(None);
            self.say("Removed the panel (Ctrl+Z brings it back).", true);
        }
    }

    fn duplicate_selected(&mut self) {
        if let Some(i) = self.selected() {
            let mut p = self.d.panels[i].clone();
            p.id = String::new();
            p.title = format!("{} (copy)", p.title);
            let r = self.edit(|d| d.add_panel(p));
            if let Ok(id) = r {
                self.state.lock().unwrap_or_else(|e| e.into_inner()).selected = Some(id);
            }
        }
    }

    fn copy_panel_data(&mut self) {
        let Some(i) = self.selected() else { return self.say("Select a panel first.", false) };
        let id = self.d.panels[i].id.clone();
        match self.data.get(&id).and_then(|d| d.frame.clone()) {
            Some(f) if !f.is_empty() => {
                let rows = if f.columns.is_empty() { f.series.iter().map(|s| s.points.len()).max().unwrap_or(0) } else { f.rows.len() };
                crate::ui::clipboard::copy(&f.csv());
                self.say(format!("Copied {rows} rows as CSV."), true);
            }
            _ => self.say("This panel has no data to copy yet.", false),
        }
    }

    /// Write the panel's data as CSV in the account (and offer to save it on the host computer).
    fn export_csv(&mut self, idx: usize, _cx: &AppCx) {
        let Some(p) = self.d.panels.get(idx).cloned() else { return };
        let Some(f) = self.data.get(&p.id).and_then(|d| d.frame.clone()) else { return self.say("No data to export yet.", false) };
        let dir = self.env.home.join("Dashboards").join("exports");
        let name = format!("{}-{}.csv", iyakki_agent::dash::store::slug(&p.title), format_time(self.now()).replace([' ', ':'], "-"));
        match std::fs::create_dir_all(&dir).and_then(|_| std::fs::write(dir.join(&name), f.csv())) {
            Ok(()) => {
                self.say(format!("Exported {name} to Dashboards/exports."), true);
                if iyakki_agent::hostfiles::enabled() {
                    self.host_save = Some((name.clone(), f.csv().into_bytes()));
                }
            }
            Err(e) => self.say(format!("Could not write the file: {e}"), false),
        }
    }

    fn export_dashboard(&mut self, _cx: &AppCx) {
        let text = Dashboards::export_bundle(&self.d, &self.sources);
        let name = format!("{}.json", iyakki_agent::dash::store::slug(&self.d.title));
        let dir = self.env.home.join("Dashboards").join("exports");
        match std::fs::create_dir_all(&dir).and_then(|_| std::fs::write(dir.join(&name), &text)) {
            Ok(()) => {
                self.say(format!("Exported {name} to Dashboards/exports."), true);
                if iyakki_agent::hostfiles::enabled() {
                    self.host_save = Some((name.clone(), text.clone().into_bytes()));
                }
            }
            Err(e) => self.say(format!("Could not write the file: {e}"), false),
        }
    }

    /// Open the chat with a question about a panel (and the moment the pointer is at, when it is on a plot).
    fn ask_ai(&mut self, idx: usize, window: Option<(i64, i64)>, cx: &AppCx) {
        let Some(p) = self.d.panels.get(idx).cloned() else { return };
        self.select(Some(idx));
        self.state.lock().unwrap_or_else(|e| e.into_inner()).focus_window = window;
        let when = window.map_or(String::new(), |(a, b)| format!(" around {} (UTC){}", format_time((a + b) / 2), if b - a > 1000 { format!(", window {} to {}", format_time(a), format_time(b)) } else { String::new() }));
        self.chat_open = true;
        self.focus = Focus::Chat;
        self.chat.prefill(&format!("Explain panel {} “{}”{when}: what happened, with numbers, and what could explain it?", p.id, p.title));
        let _ = cx;
        self.redraw = true;
    }

    fn window_at_mouse(&self, idx: usize) -> Option<(i64, i64)> {
        let plot = self.geo.plots.get(&idx)?;
        if !plot.r.contains(self.mouse.0, self.mouse.1) {
            return None;
        }
        let t = plot.t_at(self.mouse.0 as f32);
        let half = (plot.t1 - plot.t0) / 40;
        Some((t - half, t + half))
    }

    fn open_editor(&mut self, panel: Panel, is_new: bool, cx: &AppCx) {
        let _ = cx;
        let mut title = TextField::new("Panel title", false);
        title.set(&panel.title);
        let mut target = TextField::new("index, table or file", false);
        target.set(panel.query.target.as_deref().unwrap_or(""));
        let mut legend = TextField::new("{{label}} text for the legend", false);
        legend.set(&panel.options.legend_format);
        let mut query = QueryBox::new(id::QUERY);
        query.set_text(&panel.query.text);
        let mut exprs = QueryBox::new(id::EXPRS);
        exprs.set_text(&panel.expressions.join("\n"));
        let mut legend2 = legend;
        if panel.query.alias.is_empty() {
            legend2.set(&panel.options.legend_format);
        } else {
            legend2.set(&panel.query.alias);
        }
        let mut ed = Box::new(Ed { panel, is_new, title, target, legend: legend2, query, focus: EdFocus::Title, page: 0, path: vec![], nodes: vec![], nodes_err: None, nodes_loading: false, node_sel: 0, node_gen: 0, pd: PData::default(), changed: false, cur: 0, exprs, show_expr: false, only_cur: false });
        ed.pd.gen = 1;
        self.ed = Some(ed);
        self.screen = Screen::Editor;
        self.browse(vec![]);
    }

    /// List what is inside the source at `path` (the schema list of the editor).
    fn browse(&mut self, path: Vec<String>) {
        let dsrc = self.d.default_source.clone();
        let Some(ed) = self.ed.as_mut() else { return };
        let cur = ed.cur_source();
        let cur = if cur.is_empty() { dsrc } else { cur };
        if cur.is_empty() {
            ed.nodes.clear();
            ed.nodes_err = Some("Choose a source first.".into());
            return;
        }
        ed.node_gen += 1;
        ed.nodes_loading = true;
        let (gen, source) = (ed.node_gen, cur);
        let (env, tx, cancel) = (self.env.clone(), self.tx.clone(), self.cancel.clone());
        self.inflight += 1;
        std::thread::spawn(move || {
            let res = env.source(&source).and_then(|(_, s)| s.browse(&path, &cancel)).map(|mut n| {
                n.truncate(2000);
                n
            });
            let _ = tx.send(Msg::Nodes { gen, path, res });
        });
    }

    fn run_preview(&mut self) {
        self.run_preview_of(false);
    }

    fn run_preview_of(&mut self, only_cur: bool) {
        self.sync_editor_fields();
        let dsrc = self.d.default_source.clone();
        let Some(ed) = self.ed.as_mut() else { return };
        ed.only_cur = only_cur;
        let panel = ed.preview_panel();
        if panel.visual.needs_query() && panel.query_defs(&dsrc).iter().all(|(_, q)| q.query.text.trim().is_empty()) {
            ed.pd.err = Some("Write a query first (or pick something in the list on the left).".into());
            return;
        }
        let bad = iyakki_agent::dash::expr::check(&panel.expressions.iter().filter(|l| !l.trim_start().to_ascii_lowercase().starts_with("align ")).cloned().collect::<Vec<_>>());
        ed.pd.gen += 1;
        ed.pd.loading = true;
        ed.pd.err = None;
        ed.pd.qerrors = bad.iter().map(|b| format!("expression {b}")).collect();
        let gen = ed.pd.gen;
        let mut ctx = self.make_ctx(700);
        ctx.timeout_s = Some(20);
        let (env, tx, cancel) = (self.env.clone(), self.tx.clone(), self.cancel.clone());
        let range = (ctx.range.from_ms, ctx.range.to_ms);
        self.inflight += 1;
        std::thread::spawn(move || {
            let out = env.run_panel(&panel, &dsrc, &ctx, &cancel);
            let _ = tx.send(Msg::Preview { gen, range, out });
        });
    }

    fn sync_editor_fields(&mut self) {
        if let Some(ed) = self.ed.as_mut() {
            ed.store();
        }
    }

    fn apply_editor(&mut self) {
        self.sync_editor_fields();
        let Some(ed) = self.ed.take() else { return };
        let p = ed.panel.clone();
        if p.title.is_empty() {
            self.ed = Some(ed);
            return self.say("Give the panel a title.", false);
        }
        let r = if ed.is_new {
            self.edit(|d| d.add_panel(p))
        } else {
            let id = p.id.clone();
            self.edit(|d| {
                *d.panel_mut(&id).ok_or("the panel is gone")? = p.clone();
                Ok(id.clone())
            })
        };
        match r {
            Ok(id) => {
                {
                    let mut st = self.state.lock().unwrap_or_else(|e| e.into_inner());
                    st.selected = Some(id.clone());
                    st.highlight = Some(id.clone());
                }
                self.data.remove(&id);
                self.screen = Screen::Board;
                self.sync();
                self.schedule(true);
            }
            Err(e) => {
                self.ed = Some(ed);
                self.say(e, false);
            }
        }
    }

    fn close_editor(&mut self, ask: bool) {
        self.sync_editor_fields();
        if ask && self.ed.as_ref().is_some_and(|e| e.changed) {
            self.modal = Some(Modal::Confirm("Discard the changes to this panel?".into(), Confirmed::DiscardEditor));
            return;
        }
        self.ed = None;
        self.screen = Screen::Board;
        self.redraw = true;
    }

    fn test_source(&mut self, id: &str) {
        let (env, tx, cancel, sid) = (self.env.clone(), self.tx.clone(), self.cancel.clone(), id.to_string());
        self.src_status.insert(id.to_string(), (true, "testing…".into()));
        self.inflight += 1;
        std::thread::spawn(move || {
            let res = env.source(&sid).and_then(|(_, s)| s.test(&cancel));
            let _ = tx.send(Msg::Test { id: sid, res });
        });
    }

    // ------------------------------------------------------------------ source forms (generated from the kind's fields)

    fn vault_names(&self) -> Vec<String> {
        self.env.vault_names()
    }

    fn source_form(&self, kind: &str, existing: Option<&SourceConfig>, advanced: bool) -> Option<(Form, Vec<(usize, String)>)> {
        let info = registry::kind(kind)?;
        const ADV: [&str; 6] = ["ca_file", "client_cert_file", "client_key", "insecure", "proxy", "timeout_s"];
        let mut form = Form::new(&format!("{} {}", if existing.is_some() { "Edit" } else { "Add" }, if advanced { format!("{} connection options", info.label) } else { info.label.clone() }));
        let mut keys = vec![];
        let val = |k: &str, d: &str| existing.and_then(|e| e.values.get(k).cloned()).unwrap_or_else(|| d.to_string());
        if !advanced {
            form = form.text("Name", &existing.map_or(info.label.clone(), |e| e.name.clone()), "a name you will recognise");
            keys.push((0, "name".to_string()));
        }
        let vault = self.vault_names();
        let auth = val("auth", "none");
        for f in info.fields.iter().filter(|f| ADV.contains(&f.key.as_str()) == advanced) {
            // only the secret that the chosen sign-in uses is asked for
            if !advanced && f.kind == CField::Secret && ((f.key == "password" && auth != "basic") || (f.key == "token" && !matches!(auth.as_str(), "bearer" | "api_key"))) {
                continue;
            }
            if !advanced && f.key == "username" && auth != "basic" {
                continue;
            }
            let i = form.fields.len();
            match &f.kind {
                CField::Secret => {
                    let mut opts = vec!["(none)".to_string()];
                    // a reference to an entry this vault does not have (an imported source) stays visible, so the person sees what is missing
                    if let Some(r) = existing.and_then(|e| e.secrets.get(&f.key)) {
                        if !vault.iter().any(|n| format!("uvault:{n}") == *r) {
                            opts.push(r.clone());
                        }
                    }
                    opts.extend(vault.iter().map(|n| format!("uvault:{n}")));
                    opts.push(CREATE_OPT.to_string());
                    let cur = existing.and_then(|e| e.secrets.get(&f.key)).and_then(|r| opts.iter().position(|o| o == r)).unwrap_or(0);
                    let o: Vec<&str> = opts.iter().map(String::as_str).collect();
                    form = form.choice(&format!("{} (from your vault)", f.label), &o, cur);
                }
                CField::Bool => form = form.choice(&f.label, &["no", "yes"], usize::from(matches!(val(&f.key, &f.default).as_str(), "true" | "yes" | "1"))),
                CField::Choice(opts) => {
                    let o: Vec<&str> = opts.iter().map(String::as_str).collect();
                    let cur = opts.iter().position(|x| *x == val(&f.key, &f.default)).unwrap_or(0);
                    form = form.choice(&f.label, &o, cur);
                }
                _ => form = form.text(&f.label, &val(&f.key, &f.default), &f.help),
            }
            keys.push((i, f.key.clone()));
        }
        Some((form, keys))
    }

    /// Read a submitted source form back into a config (what the form did not ask for stays as it was).
    fn config_from_form(&self, form: &Form, kind: &str, editing: Option<&str>, advanced: bool, keys: &[(usize, String)]) -> Result<SourceConfig, String> {
        let info = registry::kind(kind).ok_or("unknown kind")?;
        let mut cfg = editing.and_then(|id| self.sources.iter().find(|s| s.id == id).cloned()).unwrap_or_else(|| SourceConfig { id: Sources::open(&self.env.data_dir, &self.env.user).new_id(kind), kind: kind.into(), ..Default::default() });
        for (i, key) in keys {
            let v = fval(form, *i);
            if key == "name" {
                cfg.name = v.trim().to_string();
                continue;
            }
            let Some(f) = info.fields.iter().find(|f| &f.key == key) else { continue };
            match &f.kind {
                CField::Secret => {
                    if v.starts_with("uvault:") {
                        cfg.secrets.insert(key.clone(), v);
                    } else {
                        cfg.secrets.remove(key);
                    }
                }
                CField::Bool => {
                    cfg.values.insert(key.clone(), (v == "yes").to_string());
                }
                _ => {
                    if v.trim().is_empty() {
                        cfg.values.remove(key);
                    } else {
                        cfg.values.insert(key.clone(), v.trim().to_string());
                    }
                }
            }
        }
        let _ = advanced;
        Ok(cfg)
    }

    fn submit_source(&mut self) {
        let Some(Modal::Source { form, kind, editing, advanced, secret_keys }) = self.modal.take() else { return };
        let mut form = form;
        match self.config_from_form(&form, &kind, editing.as_deref(), advanced, &secret_keys).and_then(|cfg| {
            let id = cfg.id.clone();
            self.env.sources().save(cfg).map(|_| id)
        }) {
            Ok(id) => {
                (self.env.audit)("dash.source_saved", &id);
                self.reload_lists();
                self.src_sel = self.sources.iter().position(|s| s.id == id).unwrap_or(0);
                self.say("Source saved.", true);
                self.test_source(&id);
            }
            Err(e) => {
                form.error = Some(e);
                self.modal = Some(Modal::Source { form, kind, editing, advanced, secret_keys });
            }
        }
    }
}

// ---------------------------------------------------------------------------------------------------------- events

impl DashApp {
    fn chat_rect(&self) -> Option<R> {
        self.geo.chat
    }

    fn on_tick(&mut self, cx: &AppCx) -> Flow {
        self.tick += 1;
        self.poll();
        self.sync();
        let _ = self.chat.handle(&Event::Tick, cx);
        if self.chat_open && (self.chat.is_running() || self.chat.pending_approval().is_some()) && self.tick % 3 == 0 {
            self.redraw = true;
        }
        let loading = self.data.values().any(|d| d.loading) || self.ed.as_ref().is_some_and(|e| e.pd.loading || e.nodes_loading);
        if loading && self.tick % 3 == 0 {
            self.redraw = true;
        }
        if self.flash.as_ref().is_some_and(|f| f.1 > self.tick) && self.tick % 3 == 0 {
            self.redraw = true;
        }
        if self.flash.as_ref().is_some_and(|f| f.1 <= self.tick) {
            self.flash = None;
            self.redraw = true;
        }
        if matches!(self.screen, Screen::Board | Screen::Zoom(_)) && self.d.refresh_s > 0 && self.modal.is_none() && self.drag.is_none() && self.tick - self.last_run_tick >= self.d.refresh_s as u64 * 30 {
            self.schedule(true);
        }
        let ago = self.data.values().map(|d| d.at_ms).max().map_or(0, |t| (self.now() - t) / 1000);
        if ago != self.last_label_s {
            self.last_label_s = ago;
            self.redraw = true;
        }
        Flow::Stay
    }

    fn on_move(&mut self) {
        self.redraw = true;
        let (mx, my) = self.mouse;
        match &mut self.drag {
            Some(Drag::Zoom { x1, .. }) => *x1 = mx as f32,
            Some(Drag::Move { .. }) | Some(Drag::Size { .. }) => {}
            None => {}
        }
        let _ = my;
    }

    /// Grid cell under a pixel offset from where a drag began.
    fn drag_cells(&self, from: (i32, i32)) -> (i32, i32) {
        let (dx, dy) = ((self.mouse.0 - from.0) as f32, (self.mouse.1 - from.1) as f32);
        ((dx / self.geo.col_w.max(1.0)).round() as i32, (dy / self.geo.row_h.max(1.0)).round() as i32)
    }

    fn finish_drag(&mut self) {
        let Some(d) = self.drag.take() else { return };
        self.redraw = true;
        match d {
            Drag::Move { id, from, orig, .. } => {
                let (dx, dy) = self.drag_cells(from);
                if (dx, dy) != (0, 0) {
                    let g = Grid::new(orig.x + dx, (orig.y + dy).max(0), orig.w, orig.h);
                    let _ = self.edit(|d| Ok(d.place(&id, g)));
                }
            }
            Drag::Size { id, from, orig, .. } => {
                let (dx, dy) = self.drag_cells(from);
                if (dx, dy) != (0, 0) {
                    let g = Grid::new(orig.x, orig.y, (orig.w + dx).max(2), (orig.h + dy).max(2));
                    let _ = self.edit(|d| Ok(d.place(&id, g)));
                    self.data.values_mut().for_each(|d| d.key = 0);
                    self.schedule(false);
                }
            }
            Drag::Zoom { idx, x0, x1 } => {
                let Some(plot) = self.geo.plots.get(&idx).copied() else { return };
                if (x1 - x0).abs() >= 8.0 {
                    let (a, b) = (plot.t_at(x0.min(x1)), plot.t_at(x0.max(x1)));
                    if b - a >= 1000 {
                        self.set_range(&a.to_string(), &b.to_string());
                        self.say(format!("Zoomed to {} - {}. Zoom out (or pick a range) to go back.", format_time(a), format_time(b)), true);
                    }
                }
            }
        }
    }

    fn handle_impl(&mut self, ev: &Event, cx: &AppCx) -> Flow {
        self.poll();
        self.sync();
        match ev {
            Event::Tick => return self.on_tick(cx),
            Event::MouseMove(x, y) => {
                self.mouse = (*x, *y);
                self.on_move();
                return Flow::Stay;
            }
            Event::MouseUp(..) => {
                self.finish_drag();
                return Flow::Stay;
            }
            _ => {}
        }
        self.redraw = true;
        if self.modal.is_some() {
            return self.modal_event(ev, cx);
        }
        // clicks that belong to the chat pane
        if let Some(inner) = route::decode_slot(ev, CHAT_SLOT) {
            self.focus = Focus::Chat;
            return self.to_chat(&inner, cx);
        }
        match ev {
            Event::Scroll(d) => {
                if self.chat_rect().is_some_and(|r| r.contains(self.mouse.0, self.mouse.1)) {
                    return self.to_chat(ev, cx);
                }
                match self.screen {
                    Screen::Board => self.scroll = (self.scroll - *d as f32 * 48.0).max(0.0),
                    Screen::List | Screen::Sources => {}
                    _ => {}
                }
                Flow::Stay
            }
            Event::Paste(t) => {
                if self.focus == Focus::Chat && self.chat_open {
                    return self.to_chat(ev, cx);
                }
                if let Some(ed) = self.ed.as_mut() {
                    match ed.focus {
                        EdFocus::Title => {
                            ed.title.paste(t);
                        }
                        EdFocus::Target => {
                            ed.target.paste(t);
                        }
                        EdFocus::Legend => {
                            ed.legend.paste(t);
                        }
                        EdFocus::Query => {
                            if ed.show_expr {
                                ed.exprs.handle(ev);
                            } else {
                                ed.query.handle(ev);
                            }
                        }
                        EdFocus::Nodes => {}
                    }
                    ed.changed = true;
                }
                Flow::Stay
            }
            Event::Key(k) => self.key(k, ev, cx),
            Event::Tab(n) => self.tab(*n, cx),
            Event::Row(n) => {
                self.row(*n, cx);
                Flow::Stay
            }
            Event::FocusAt(n, x) => {
                if let Some(ed) = self.ed.as_mut() {
                    self.focus = Focus::Board;
                    match *n {
                        id::F_TITLE => {
                            ed.focus = EdFocus::Title;
                            ed.title.click(*x);
                        }
                        id::F_TARGET => {
                            ed.focus = EdFocus::Target;
                            ed.target.click(*x);
                        }
                        id::F_LEGEND => {
                            ed.focus = EdFocus::Legend;
                            ed.legend.click(*x);
                        }
                        _ => {}
                    }
                }
                Flow::Stay
            }
            Event::AreaAt(n, x, y) => {
                self.area(*n, *x, *y);
                Flow::Stay
            }
            _ => Flow::Stay,
        }
    }

    fn to_chat(&mut self, ev: &Event, cx: &AppCx) -> Flow {
        // the chat sees a panel's question when it is sent
        let f = self.chat.handle(ev, cx);
        if matches!(f, Flow::Back) {
            self.focus = Focus::Board;
        }
        Flow::Stay
    }

    fn area(&mut self, n: usize, x: f32, y: f32) {
        self.focus = Focus::Board;
        match n {
            _ if (id::MOVE..id::MOVE + 1000).contains(&n) => {
                if self.drag.is_none() {
                    let idx = n - id::MOVE;
                    if let Some(p) = self.d.panels.get(idx) {
                        let (id, orig) = (p.id.clone(), p.grid);
                        self.select(Some(idx));
                        self.drag = Some(Drag::Move { id, from: self.mouse, orig });
                    }
                }
            }
            _ if (id::SIZE..id::SIZE + 1000).contains(&n) => {
                if self.drag.is_none() {
                    let idx = n - id::SIZE;
                    if let Some(p) = self.d.panels.get(idx) {
                        let (id, orig) = (p.id.clone(), p.grid);
                        self.select(Some(idx));
                        self.drag = Some(Drag::Size { id, from: self.mouse, orig });
                    }
                }
            }
            _ if (ch::PLOT_AREA..ch::PLOT_AREA + 1000).contains(&n) => {
                let idx = n - ch::PLOT_AREA;
                self.select(Some(idx).filter(|i| *i < self.d.panels.len()));
                if self.drag.is_none() {
                    if let Some(plot) = self.geo.plots.get(&idx) {
                        let x0 = plot.r.x + x;
                        self.drag = Some(Drag::Zoom { idx, x0, x1: x0 });
                    }
                }
            }
            id::QUERY | id::EXPRS => {
                if let Some(ed) = self.ed.as_mut() {
                    ed.focus = EdFocus::Query;
                    if n == id::EXPRS {
                        ed.exprs.click(x, y, self.drag.is_some());
                    } else {
                        ed.query.click(x, y, self.drag.is_some());
                    }
                    self.drag = None;
                }
            }
            _ => {}
        }
    }

    fn row(&mut self, n: usize, cx: &AppCx) {
        match self.screen {
            Screen::List => {
                self.list_sel = n;
                // a second click on the chosen row opens it
                self.open_list_item(cx);
            }
            Screen::Sources if n >= id::ROW_SRC => self.src_sel = n - id::ROW_SRC,
            Screen::Editor if n >= id::ROW_NODE => {
                if let Some(ed) = self.ed.as_mut() {
                    ed.node_sel = n - id::ROW_NODE;
                    ed.focus = EdFocus::Nodes;
                }
                self.node_enter();
            }
            _ => {}
        }
    }

    fn open_list_item(&mut self, cx: &AppCx) {
        let n = self.list.len();
        if self.list_sel < n {
            let id = self.list[self.list_sel].id.clone();
            match self.env.dashboards().load(&id) {
                Ok(d) => self.open_dashboard(d),
                Err(e) => self.say(e, false),
            }
        } else if let Some(t) = templates::all().get(self.list_sel - n) {
            let kind = t.kind;
            let ids: Vec<String> = self.sources.iter().filter(|s| s.kind == kind).map(|s| s.id.clone()).collect();
            let names: Vec<String> = self.sources.iter().filter(|s| s.kind == kind).map(|s| s.name.clone()).collect();
            match ids.len() {
                0 => {
                    self.say(format!("This template needs a {} source: add one first.", registry::kind(kind).map_or(kind.to_string(), |k| k.label)), false);
                    self.screen = Screen::Sources;
                }
                1 => self.instantiate_template(t.id, &ids[0]),
                _ => self.modal = Some(Modal::Template(Picker::new("Which source should the panels use?", names), ids, t.id.to_string())),
            }
        }
        let _ = cx;
    }

    fn instantiate_template(&mut self, tid: &str, source: &str) {
        let Some(t) = templates::find(tid) else { return };
        let mut d = t.instantiate(source, "");
        // a DuckDB template starts on the first data file of the source
        if let Some(files) = self.sources.iter().find(|s| s.id == source).map(|s| s.value("files").to_string()) {
            if let Some(first) = files.split(',').map(str::trim).find(|f| !f.is_empty()) {
                for v in &mut d.variables {
                    if v.name == "file" {
                        v.kind = VarKind::Custom { options: vec![first.to_string()] };
                    }
                }
            }
        }
        self.open_dashboard(d);
        self.say(format!("Opened the template “{}”. Save it to keep your changes.", t.title), true);
    }

    fn key(&mut self, k: &Key, ev: &Event, cx: &AppCx) -> Flow {
        // the chat owns the keyboard while it is focused
        if self.focus == Focus::Chat && self.chat_open {
            if matches!(k, Key::F(6)) {
                self.focus = Focus::Board;
                return Flow::Stay;
            }
            return self.to_chat(ev, cx);
        }
        match k {
            Key::F(6) if self.chat_open => {
                self.focus = Focus::Chat;
                return Flow::Stay;
            }
            Key::Ctrl('i') => {
                self.toggle_chat();
                return Flow::Stay;
            }
            _ => {}
        }
        match self.screen.clone() {
            Screen::Board => self.board_key(k, cx),
            Screen::List => self.list_key(k, cx),
            Screen::Sources => self.sources_key(k),
            Screen::Editor => self.editor_key(k, ev, cx),
            Screen::Zoom(id) => self.zoom_key(k, &id),
        }
    }

    fn toggle_chat(&mut self) {
        self.chat_open = !self.chat_open;
        self.focus = if self.chat_open { Focus::Chat } else { Focus::Board };
        self.redraw = true;
    }

    fn move_selection(&mut self, dx: i32, dy: i32) {
        let cur = self.selected();
        let centre = |p: &Panel| (p.grid.x as f32 + p.grid.w as f32 / 2.0, p.grid.y as f32 + p.grid.h as f32 / 2.0);
        let Some(i) = cur else {
            if !self.d.panels.is_empty() {
                self.select(Some(0));
            }
            return;
        };
        let (cx0, cy0) = centre(&self.d.panels[i]);
        let best = self
            .d
            .panels
            .iter()
            .enumerate()
            .filter(|(j, _)| *j != i)
            .filter_map(|(j, p)| {
                let (x, y) = centre(p);
                let (ddx, ddy) = (x - cx0, y - cy0);
                let along = ddx * dx as f32 + ddy * dy as f32;
                if along <= 0.0 {
                    return None;
                }
                let across = (ddx * dy as f32).abs() + (ddy * dx as f32).abs();
                Some((j, along + across * 2.0))
            })
            .min_by(|a, b| a.1.total_cmp(&b.1));
        if let Some((j, _)) = best {
            self.select(Some(j));
            let id = self.d.panels[j].id.clone();
            self.scroll_to(&id);
        }
    }

    fn board_key(&mut self, k: &Key, cx: &AppCx) -> Flow {
        use crate::ui::Nav;
        let sel = self.selected();
        match k {
            Key::Esc => {
                if sel.is_some() {
                    self.select(None);
                } else if self.leave_board() {
                    self.screen = Screen::List;
                    self.reload_lists();
                }
            }
            Key::Left => self.move_selection(-1, 0),
            Key::Right => self.move_selection(1, 0),
            Key::Up => self.move_selection(0, -1),
            Key::Down => self.move_selection(0, 1),
            Key::Nav { nav, shift: true, ctrl: false } => {
                if let Some(i) = sel {
                    let p = &self.d.panels[i];
                    let (id, g) = (p.id.clone(), p.grid);
                    let ng = match nav {
                        Nav::Left => Grid::new(g.x - 1, g.y, g.w, g.h),
                        Nav::Right => Grid::new(g.x + 1, g.y, g.w, g.h),
                        Nav::Up => Grid::new(g.x, (g.y - 1).max(0), g.w, g.h),
                        Nav::Down => Grid::new(g.x, g.y + 1, g.w, g.h),
                        _ => g,
                    };
                    let _ = self.edit(|d| Ok(d.place(&id, ng)));
                }
            }
            Key::Nav { nav, ctrl: true, .. } => {
                if let Some(i) = sel {
                    let p = &self.d.panels[i];
                    let (id, g) = (p.id.clone(), p.grid);
                    let ng = match nav {
                        Nav::Left => Grid::new(g.x, g.y, (g.w - 1).max(2), g.h),
                        Nav::Right => Grid::new(g.x, g.y, g.w + 1, g.h),
                        Nav::Up => Grid::new(g.x, g.y, g.w, (g.h - 1).max(2)),
                        Nav::Down => Grid::new(g.x, g.y, g.w, g.h + 1),
                        _ => g,
                    };
                    let _ = self.edit(|d| Ok(d.place(&id, ng)));
                    self.schedule(false);
                }
            }
            Key::PageDown => self.scroll += self.geo.board.map_or(200.0, |b| b.h * 0.8),
            Key::PageUp => self.scroll = (self.scroll - self.geo.board.map_or(200.0, |b| b.h * 0.8)).max(0.0),
            Key::Home => self.scroll = 0.0,
            Key::Enter | Key::Char('e') => {
                if let Some(i) = sel {
                    let p = self.d.panels[i].clone();
                    self.open_editor(p, false, cx);
                }
            }
            Key::Char('a') | Key::Char('+') => self.add_panel_form(cx),
            Key::Delete | Key::Char('x') => self.remove_selected(),
            Key::Char('d') => self.duplicate_selected(),
            Key::Char('v') => {
                if let Some(i) = sel {
                    self.screen = Screen::Zoom(self.d.panels[i].id.clone());
                }
            }
            Key::Char('r') | Key::F(5) => self.schedule(true),
            Key::Char('z') => self.zoom_out(),
            Key::Char('c') | Key::Ctrl('c') => self.copy_panel_data(),
            Key::Char('s') => {
                self.screen = Screen::Sources;
                self.reload_lists();
            }
            Key::Char('l') => {
                if self.leave_board() {
                    self.screen = Screen::List;
                    self.reload_lists();
                }
            }
            Key::Char('i') => {
                if let Some(i) = sel {
                    let w = self.window_at_mouse(i);
                    self.ask_ai(i, w, cx);
                } else {
                    self.toggle_chat();
                }
            }
            Key::Char('n') => {
                let t = self.d.title.clone();
                let mut f = Form::new("Dashboard title").text("Title", &t, "");
                f.focus = 0;
                self.modal = Some(Modal::Name(f, NameFor::Rename));
            }
            Key::Char('t') => {
                // t cycles the auto refresh
                self.cycle_refresh();
            }
            Key::Char(c) if ('1'..='8').contains(c) => {
                let i = *c as usize - '1' as usize;
                if let Some(r) = RANGE_CHOICES.get(i) {
                    self.set_range(&format!("now-{r}"), "now");
                }
            }
            Key::Ctrl('z') => {
                let ok = self.state.lock().unwrap_or_else(|e| e.into_inner()).undo();
                self.sync();
                self.say(if ok { "Undone." } else { "Nothing to undo." }, ok);
            }
            Key::Ctrl('y') | Key::Ctrl('Z') => {
                let ok = self.state.lock().unwrap_or_else(|e| e.into_inner()).redo();
                self.sync();
                self.say(if ok { "Redone." } else { "Nothing to redo." }, ok);
            }
            Key::Ctrl('s') => self.save(),
            _ => {}
        }
        Flow::Stay
    }

    fn list_key(&mut self, k: &Key, cx: &AppCx) -> Flow {
        let total = self.list.len() + templates::all().len();
        match k {
            Key::Esc => return Flow::Back,
            Key::Up => self.list_sel = self.list_sel.saturating_sub(1),
            Key::Down => self.list_sel = (self.list_sel + 1).min(total.saturating_sub(1)),
            Key::Enter => self.open_list_item(cx),
            Key::Char('n') => {
                let mut f = Form::new("New dashboard").text("Title", "New dashboard", "");
                f.focus = 0;
                self.modal = Some(Modal::Name(f, NameFor::New));
            }
            Key::Char('i') => {
                self.modal = Some(Modal::Name(Form::new("Import a dashboard").path("File", "", "path of a dashboard .json in your home folder", crate::filepicker::PathKind::File, &["json"]), NameFor::Import));
            }
            Key::Char('s') => self.screen = Screen::Sources,
            Key::Delete | Key::Char('x') => {
                if let Some(e) = self.list.get(self.list_sel) {
                    self.modal = Some(Modal::Confirm(format!("Delete the dashboard “{}”? The file is removed.", e.title), Confirmed::DeleteDashboard(e.id.clone())));
                }
            }
            _ => {}
        }
        Flow::Stay
    }

    fn sources_key(&mut self, k: &Key) -> Flow {
        match k {
            Key::Esc => {
                self.screen = if self.d.panels.is_empty() && !self.d.id.is_empty() || self.d.panels.is_empty() { Screen::List } else { Screen::Board };
            }
            Key::Up => self.src_sel = self.src_sel.saturating_sub(1),
            Key::Down => self.src_sel = (self.src_sel + 1).min(self.sources.len().saturating_sub(1)),
            Key::Char('a') => self.tab_source_add(),
            Key::Char('e') | Key::Enter => self.tab_source_edit(false),
            Key::Char('o') => self.tab_source_edit(true),
            Key::Char('t') => {
                if let Some(s) = self.sources.get(self.src_sel).map(|s| s.id.clone()) {
                    self.test_source(&s);
                }
            }
            Key::Delete | Key::Char('x') => {
                if let Some(s) = self.sources.get(self.src_sel) {
                    self.modal = Some(Modal::Confirm(format!("Remove the source “{}”? Panels that use it show an error until you pick another.", s.name), Confirmed::DeleteSource(s.id.clone())));
                }
            }
            _ => {}
        }
        Flow::Stay
    }

    fn tab_source_add(&mut self) {
        let items: Vec<String> = registry::kinds().iter().map(|k| format!("{}: {}", k.label, k.about)).collect();
        self.modal = Some(Modal::Kind(Picker::new("What kind of data source?", items)));
    }

    fn tab_source_edit(&mut self, advanced: bool) {
        let Some(s) = self.sources.get(self.src_sel).cloned() else { return };
        if let Some((form, keys)) = self.source_form(&s.kind, Some(&s), advanced) {
            self.modal = Some(Modal::Source { form, kind: s.kind.clone(), editing: Some(s.id.clone()), advanced, secret_keys: keys });
        }
    }

    fn zoom_key(&mut self, k: &Key, pid: &str) -> Flow {
        let Some(idx) = self.d.panels.iter().position(|p| p.id == pid) else {
            self.screen = Screen::Board;
            return Flow::Stay;
        };
        let rows = self.data.get(pid).and_then(|d| d.frame.as_ref()).map_or(0, |f| f.rows.len());
        let cols = self.data.get(pid).and_then(|d| d.frame.as_ref()).map_or(0, |f| f.columns.len());
        let Some(pd) = self.data.get_mut(pid) else { return Flow::Stay };
        match k {
            Key::Esc => self.screen = Screen::Board,
            Key::PageDown => pd.table.page += 1,
            Key::PageUp => pd.table.page = pd.table.page.saturating_sub(1),
            Key::Down | Key::Up | Key::Left | Key::Right => {
                let (r, c) = pd.table.sel.unwrap_or((0, 0));
                let (r, c) = match k {
                    Key::Down => ((r + 1).min(rows.saturating_sub(1)), c),
                    Key::Up => (r.saturating_sub(1), c),
                    Key::Left => (r, c.saturating_sub(1)),
                    _ => (r, (c + 1).min(cols.saturating_sub(1))),
                };
                pd.table.sel = Some((r, c));
            }
            Key::Char('c') | Key::Ctrl('c') => {
                if let (Some((r, c)), Some(f)) = (pd.table.sel, pd.frame.clone()) {
                    let t = if f.columns.is_empty() { f.series_table() } else { (*f).clone() };
                    if r < t.rows.len() && c < t.columns.len() {
                        crate::ui::clipboard::copy(&t.cell(r, c));
                        self.say("Copied the cell.", true);
                    }
                } else {
                    self.copy_panel_data();
                }
            }
            Key::Char('r') => {
                if let (Some((r, _)), Some(f)) = (pd.table.sel, pd.frame.clone()) {
                    let t = if f.columns.is_empty() { f.series_table() } else { (*f).clone() };
                    if r < t.rows.len() {
                        crate::ui::clipboard::copy(&(0..t.columns.len()).map(|c| t.cell(r, c)).collect::<Vec<_>>().join("\t"));
                        self.say("Copied the row.", true);
                    }
                }
            }
            _ => {}
        }
        let _ = idx;
        Flow::Stay
    }

    fn editor_key(&mut self, k: &Key, ev: &Event, cx: &AppCx) -> Flow {
        let Some(ed) = self.ed.as_mut() else {
            self.screen = Screen::Board;
            return Flow::Stay;
        };
        match k {
            Key::Esc => {
                self.close_editor(true);
                return Flow::Stay;
            }
            Key::Ctrl('s') => {
                self.apply_editor();
                return Flow::Stay;
            }
            Key::F(5) => {
                self.run_preview();
                return Flow::Stay;
            }
            Key::Ctrl('r') => {
                self.run_preview();
                return Flow::Stay;
            }
            Key::BackTab => {
                ed.focus = match ed.focus {
                    EdFocus::Title => EdFocus::Nodes,
                    EdFocus::Target => EdFocus::Title,
                    EdFocus::Legend => EdFocus::Target,
                    EdFocus::Query => EdFocus::Legend,
                    EdFocus::Nodes => EdFocus::Query,
                };
                return Flow::Stay;
            }
            _ => {}
        }
        let ctrl_enter = matches!(k, Key::Ctrl('\n') | Key::Ctrl('\r') | Key::Ctrl('m'));
        if ctrl_enter {
            self.run_preview();
            return Flow::Stay;
        }
        match ed.focus {
            EdFocus::Title | EdFocus::Target | EdFocus::Legend => {
                let tf = match ed.focus {
                    EdFocus::Title => &mut ed.title,
                    EdFocus::Target => &mut ed.target,
                    _ => &mut ed.legend,
                };
                if matches!(k, Key::Tab | Key::Enter) {
                    ed.focus = match ed.focus {
                        EdFocus::Title => EdFocus::Target,
                        EdFocus::Target => EdFocus::Legend,
                        _ => EdFocus::Query,
                    };
                } else if tf.handle(k) {
                    ed.changed = true;
                }
            }
            EdFocus::Query => {
                if if ed.show_expr { ed.exprs.handle(ev) } else { ed.query.handle(ev) } {
                    ed.changed = true;
                }
            }
            EdFocus::Nodes => match k {
                Key::Tab => ed.focus = EdFocus::Title,
                Key::Up => ed.node_sel = ed.node_sel.saturating_sub(1),
                Key::Down => ed.node_sel = (ed.node_sel + 1).min(ed.nodes.len().saturating_sub(1)),
                Key::Enter => self.node_enter(),
                Key::Backspace | Key::Left => self.node_up(),
                _ => {}
            },
        }
        let _ = cx;
        Flow::Stay
    }

    /// Enter on a schema entry: open what has more inside, use what is a target, or put a name into the query.
    fn node_enter(&mut self) {
        let dsrc = self.d.default_source.clone();
        let Some(ed) = self.ed.as_mut() else { return };
        let Some(n) = ed.nodes.get(ed.node_sel).cloned() else { return };
        match n.kind {
            NodeKind::Metric | NodeKind::Table | NodeKind::Index | NodeKind::File | NodeKind::Database => {
                let mut path = ed.path.clone();
                path.push(n.name.clone());
                let sid = if ed.cur_source().is_empty() { dsrc.clone() } else { ed.cur_source() };
                if matches!(n.kind, NodeKind::Index | NodeKind::File) {
                    ed.target.set(&n.name);
                }
                if ed.query.text().trim().is_empty() && !matches!(n.kind, NodeKind::Database) {
                    let target = if matches!(n.kind, NodeKind::Table) && !ed.path.is_empty() { format!("{}.{}", ed.path[0], n.name) } else { n.name.clone() };
                    let starter = self.env.source(&sid).map(|(_, s)| s.starter(&target)).unwrap_or_default();
                    if let Some(ed) = self.ed.as_mut() {
                        ed.query.set_text(&starter);
                        ed.changed = true;
                    }
                }
                if n.has_children {
                    self.browse(path);
                }
            }
            NodeKind::Column | NodeKind::Label | NodeKind::Field => {
                if ed.show_expr {
                    ed.exprs.insert(&n.name);
                } else {
                    ed.query.insert(&n.name);
                }
                ed.focus = EdFocus::Query;
                ed.changed = true;
            }
        }
    }

    fn node_up(&mut self) {
        let Some(ed) = self.ed.as_mut() else { return };
        if ed.path.is_empty() {
            return;
        }
        let mut p = ed.path.clone();
        p.pop();
        self.browse(p);
    }

    fn tab(&mut self, n: usize, cx: &AppCx) -> Flow {
        if n == id::CHAT_PANE {
            self.focus = Focus::Chat;
            return Flow::Stay;
        }
        self.focus = Focus::Board;
        // charts and tables register their own ids
        if (ch::LEGEND_TAB..ch::LEGEND_TAB + 6_100).contains(&n) {
            let (pi, si) = ((n - ch::LEGEND_TAB) / 100, (n - ch::LEGEND_TAB) % 100);
            let key = if self.screen == Screen::Editor { None } else { self.d.panels.get(pi).map(|p| p.id.clone()) };
            let name = match &key {
                Some(k) => self.data.get(k).and_then(|d| d.model.as_ref()).and_then(|m| m.lines.get(si)).map(|l| l.name.clone()),
                None => self.ed.as_ref().and_then(|e| e.pd.model.as_ref()).and_then(|m| m.lines.get(si)).map(|l| l.name.clone()),
            };
            if let Some(name) = name {
                if let Some(k) = key {
                    if let (Some(p), Some(mut pd)) = (self.d.panel(&k).cloned(), self.data.remove(&k)) {
                        toggle(&mut pd.hidden, &name);
                        self.build_model(&p, &mut pd);
                        self.data.insert(k, pd);
                    }
                } else if let Some(ed) = self.ed.as_mut() {
                    toggle(&mut ed.pd.hidden, &name);
                    let (p, mut pd) = (ed.panel.clone(), std::mem::take(&mut ed.pd));
                    self.build_model(&p, &mut pd);
                    if let Some(ed) = self.ed.as_mut() {
                        ed.pd = pd;
                    }
                }
            }
            return Flow::Stay;
        }
        if (ch::TABLE_HEAD..ch::TABLE_HEAD + 1000).contains(&n) {
            let c = n - ch::TABLE_HEAD;
            if let Some(t) = self.table_state_mut() {
                t.sort = match t.sort {
                    Some((sc, false)) if sc == c => Some((c, true)),
                    Some((sc, true)) if sc == c => None,
                    _ => Some((c, false)),
                };
                t.page = 0;
            }
            return Flow::Stay;
        }
        if (ch::TABLE_CELL..ch::TABLE_CELL + 9_000 * 64 + 64).contains(&n) {
            let (r, c) = ((n - ch::TABLE_CELL) / 64, (n - ch::TABLE_CELL) % 64);
            if let Some(t) = self.table_state_mut() {
                t.sel = Some((r, c));
            }
            return Flow::Stay;
        }
        if (ch::TABLE_PAGE..ch::TABLE_PAGE + 130).contains(&n) {
            let up = (n - ch::TABLE_PAGE) % 2 == 1;
            if let Some(t) = self.table_state_mut() {
                t.page = if up { t.page + 1 } else { t.page.saturating_sub(1) };
            }
            return Flow::Stay;
        }
        match n {
            _ if (id::RANGE..id::RANGE + 8).contains(&n) => {
                let r = RANGE_CHOICES[n - id::RANGE];
                self.set_range(&format!("now-{r}"), "now");
            }
            id::REFRESH => self.cycle_refresh(),
            id::NOW => self.schedule(true),
            id::ADD => self.add_panel_form(cx),
            id::SOURCES => {
                self.reload_lists();
                self.screen = Screen::Sources;
            }
            id::UNDO => {
                let ok = self.state.lock().unwrap_or_else(|e| e.into_inner()).undo();
                self.sync();
                self.say(if ok { "Undone." } else { "Nothing to undo." }, ok);
            }
            id::REDO => {
                let ok = self.state.lock().unwrap_or_else(|e| e.into_inner()).redo();
                self.sync();
                self.say(if ok { "Redone." } else { "Nothing to redo." }, ok);
            }
            id::SAVE => self.save(),
            id::AI | id::AI_HIDE => self.toggle_chat(),
            id::LIST => {
                if self.leave_board() {
                    self.screen = Screen::List;
                    self.reload_lists();
                }
            }
            id::ZOOM_OUT => self.zoom_out(),
            id::EXPORT => self.export_dashboard(cx),
            id::RENAME => {
                let t = self.d.title.clone();
                self.modal = Some(Modal::Name(Form::new("Dashboard title").text("Title", &t, ""), NameFor::Rename));
            }
            id::BACK => match self.screen.clone() {
                Screen::Zoom(_) => self.screen = Screen::Board,
                Screen::Sources => self.screen = if self.d.panels.is_empty() { Screen::List } else { Screen::Board },
                Screen::Editor => self.close_editor(true),
                _ => {}
            },
            id::COPY => self.copy_panel_data(),
            id::EXPORT_CSV => {
                if let Screen::Zoom(pid) = self.screen.clone() {
                    if let Some(i) = self.d.panels.iter().position(|p| p.id == pid) {
                        self.export_csv(i, cx);
                    }
                }
            }
            _ if (id::VAR..id::VAR + 20).contains(&n) => self.cycle_variable(n - id::VAR),
            id::NEW => {
                self.modal = Some(Modal::Name(Form::new("New dashboard").text("Title", "New dashboard", ""), NameFor::New));
            }
            id::IMPORT => {
                self.modal = Some(Modal::Name(Form::new("Import a dashboard").path("File", "", "path of a dashboard .json in your home folder", crate::filepicker::PathKind::File, &["json"]), NameFor::Import));
            }
            id::OPEN => self.open_list_item(cx),
            id::DELETE => {
                if let Some(e) = self.list.get(self.list_sel) {
                    self.modal = Some(Modal::Confirm(format!("Delete the dashboard “{}”? The file is removed.", e.title), Confirmed::DeleteDashboard(e.id.clone())));
                }
            }
            id::S_ADD => self.tab_source_add(),
            id::S_EDIT => self.tab_source_edit(false),
            id::S_ADV => self.tab_source_edit(true),
            id::S_DEFAULT => {
                if let Some(sid) = self.sources.get(self.src_sel).map(|s| s.id.clone()) {
                    let _ = self.edit(|d| {
                        d.default_source = sid.clone();
                        Ok(())
                    });
                    self.say("Queries without a source of their own now use this one.", true);
                    self.schedule(false);
                }
            }
            id::S_TEST => {
                if let Some(s) = self.sources.get(self.src_sel).map(|s| s.id.clone()) {
                    self.test_source(&s);
                }
            }
            id::S_DEL => {
                if let Some(s) = self.sources.get(self.src_sel) {
                    self.modal = Some(Modal::Confirm(format!("Remove the source “{}”?", s.name), Confirmed::DeleteSource(s.id.clone())));
                }
            }
            id::E_RUN | id::E_RUN2 => self.run_preview(),
            id::E_APPLY => self.apply_editor(),
            id::E_CANCEL => self.close_editor(true),
            id::E_UP => self.node_up(),
            id::E_UNIT => {
                if let Some(ed) = self.ed.as_mut() {
                    let cur = ed.cur;
                    let now = ed.cur_query().unit.unwrap_or(if cur == 0 { ed.panel.options.unit } else { Unit::None });
                    let at = Unit::ALL.iter().position(|u| *u == now).unwrap_or(0);
                    let next = Unit::ALL[(at + 1) % Unit::ALL.len()];
                    Ed::slot(&mut ed.panel, cur).1.unit = Some(next);
                    if cur == 0 {
                        ed.panel.options.unit = next;
                    }
                    ed.changed = true;
                    let (p, mut pd) = (ed.panel.clone(), std::mem::take(&mut ed.pd));
                    self.build_model(&p, &mut pd);
                    if let Some(ed) = self.ed.as_mut() {
                        ed.pd = pd;
                    }
                }
            }
            id::E_AXIS | id::E_HIDE => {
                if let Some(ed) = self.ed.as_mut() {
                    let cur = ed.cur;
                    let q = Ed::slot(&mut ed.panel, cur).1;
                    if n == id::E_AXIS {
                        q.axis = if q.axis == Axis::Right { Axis::Left } else { Axis::Right };
                    } else {
                        q.hidden = !q.hidden;
                    }
                    ed.changed = true;
                }
            }
            id::E_RUN_ONE => self.run_preview_of(true),
            id::E_EXPR => {
                if let Some(ed) = self.ed.as_mut() {
                    ed.store();
                    ed.show_expr = !ed.show_expr;
                    ed.focus = EdFocus::Query;
                }
            }
            id::E_QADD => {
                if let Some(ed) = self.ed.as_mut() {
                    if ed.panel.queries.len() + 1 < MAX_QUERIES {
                        ed.store();
                        let src = ed.cur_source();
                        ed.panel.queries.push(QueryDef { source: src, query: PanelQuery::default() });
                        let i = ed.panel.queries.len();
                        ed.show_expr = false;
                        ed.load(i);
                        ed.changed = true;
                        ed.focus = EdFocus::Query;
                    }
                }
                self.browse(vec![]);
            }
            id::E_QDEL => {
                if let Some(ed) = self.ed.as_mut() {
                    if ed.cur > 0 && ed.cur <= ed.panel.queries.len() {
                        let i = ed.cur;
                        ed.panel.queries.remove(i - 1);
                        ed.cur = 0;
                        ed.load(0);
                        ed.changed = true;
                    }
                }
                self.browse(vec![]);
            }
            _ if (id::E_Q..id::E_Q + MAX_QUERIES).contains(&n) => {
                if let Some(ed) = self.ed.as_mut() {
                    ed.show_expr = false;
                    ed.load(n - id::E_Q);
                    ed.focus = EdFocus::Query;
                }
                self.browse(vec![]);
            }
            id::E_STARTER => {
                if let Some(ed) = self.ed.as_mut() {
                    let target = ed.target.value.trim().to_string();
                    let sid = if ed.cur_source().is_empty() { self.d.default_source.clone() } else { ed.cur_source() };
                    let t = self.env.source(&sid).map(|(_, s)| s.starter(&target)).unwrap_or_default();
                    if let Some(ed) = self.ed.as_mut() {
                        ed.query.set_text(&t);
                        ed.changed = true;
                    }
                }
            }
            id::E_TOP | id::E_OVER => self.es_builder(n == id::E_TOP),
            _ if (id::E_PAGE..id::E_PAGE + 4).contains(&n) => {
                if let Some(ed) = self.ed.as_mut() {
                    ed.page = n - id::E_PAGE;
                }
            }
            _ if (id::E_SRC..id::E_SRC + 60).contains(&n) => {
                let sid = self.sources.get(n - id::E_SRC).map(|s| s.id.clone());
                if let (Some(sid), Some(ed)) = (sid, self.ed.as_mut()) {
                    let cur = ed.cur;
                    *Ed::slot(&mut ed.panel, cur).0 = sid;
                    ed.changed = true;
                    self.browse(vec![]);
                }
            }
            _ if (id::E_VIS..id::E_VIS + 11).contains(&n) => {
                if let Some(ed) = self.ed.as_mut() {
                    ed.panel.visual = Visual::ALL[n - id::E_VIS];
                    ed.changed = true;
                    let (p, mut pd) = (ed.panel.clone(), std::mem::take(&mut ed.pd));
                    self.build_model(&p, &mut pd);
                    if let Some(ed) = self.ed.as_mut() {
                        ed.pd = pd;
                    }
                }
            }
            _ if (id::SEL..id::SEL + 100).contains(&n) => {
                let i = n - id::SEL;
                if i < self.d.panels.len() {
                    self.select(Some(i));
                }
            }
            _ if (id::EDIT..id::EDIT + 100).contains(&n) => {
                if let Some(p) = self.d.panels.get(n - id::EDIT).cloned() {
                    self.open_editor(p, false, cx);
                }
            }
            _ if (id::VIEW..id::VIEW + 100).contains(&n) => {
                if let Some(p) = self.d.panels.get(n - id::VIEW) {
                    self.screen = Screen::Zoom(p.id.clone());
                }
            }
            _ if (id::ASK..id::ASK + 100).contains(&n) => {
                let i = n - id::ASK;
                self.ask_ai(i, None, cx);
            }
            _ => {}
        }
        Flow::Stay
    }

    fn table_state_mut(&mut self) -> Option<&mut ch::TableView> {
        match &self.screen {
            Screen::Zoom(id) => {
                let id = id.clone();
                self.data.get_mut(&id).map(|d| &mut d.table)
            }
            Screen::Editor => self.ed.as_mut().map(|e| &mut e.pd.table),
            _ => {
                // a table panel on the board: the selected one
                let id = self.selected().map(|i| self.d.panels[i].id.clone())?;
                self.data.get_mut(&id).map(|d| &mut d.table)
            }
        }
    }

    fn cycle_variable(&mut self, i: usize) {
        let Some(v) = self.d.variables.get(i).cloned() else { return };
        let opts = self.var_opts.get(&v.name).cloned().unwrap_or_else(|| match &v.kind {
            VarKind::Custom { options } | VarKind::Interval { options } => options.clone(),
            VarKind::Query { .. } => v.current.clone(),
        });
        if opts.is_empty() {
            return;
        }
        let cur = engine::variable_values(&v, &opts);
        let next: Vec<String> = if v.multi && v.include_all && cur.len() == opts.len() && opts.len() > 1 {
            vec![opts[0].clone()]
        } else {
            let pos = cur.first().and_then(|c| opts.iter().position(|o| o == c)).unwrap_or(0);
            if pos + 1 >= opts.len() && v.multi && v.include_all { opts.clone() } else { vec![opts[(pos + 1) % opts.len()].clone()] }
        };
        let name = v.name.clone();
        let _ = self.edit(|d| {
            if let Some(x) = d.variables.iter_mut().find(|x| x.name == name) {
                x.current = next;
            }
            Ok(())
        });
        self.schedule(true);
    }

    /// Elasticsearch helpers: "Top values" of the selected field, or "Over time" (documents per interval).
    fn es_builder(&mut self, top: bool) {
        let Some(ed) = self.ed.as_mut() else { return };
        let field = ed.nodes.get(ed.node_sel).filter(|n| n.kind == NodeKind::Field).map(|n| n.name.clone());
        let text = if top {
            let Some(f) = field else { return self.say("Pick a keyword field in the list first.", false) };
            iyakki_agent::dash::es::terms_query(&f, 10, "")
        } else {
            let f = ed.nodes.iter().find(|n| n.kind == NodeKind::Field && n.detail == "date").map_or("@timestamp".to_string(), |n| n.name.clone());
            iyakki_agent::dash::es::date_histogram_query(&f, "$__interval", "", None, None)
        };
        ed.query.set_text(&text);
        ed.panel.visual = if top { Visual::Bar } else { Visual::Area };
        ed.changed = true;
    }

    fn modal_event(&mut self, ev: &Event, cx: &AppCx) -> Flow {
        let modal = self.modal.take();
        match modal {
            None => {}
            Some(Modal::Kind(mut p)) => match p.handle_event(ev) {
                PickEvent::Chosen(label) => {
                    let kind = registry::kinds().into_iter().find(|k| label.starts_with(&format!("{}:", k.label))).map(|k| k.kind);
                    if let Some(kind) = kind {
                        if let Some((form, keys)) = self.source_form(&kind, None, false) {
                            self.modal = Some(Modal::Source { form, kind, editing: None, advanced: false, secret_keys: keys });
                        }
                    }
                }
                PickEvent::Cancel => {}
                _ => self.modal = Some(Modal::Kind(p)),
            },
            Some(Modal::Source { mut form, kind, editing, advanced, secret_keys }) => {
                let r = form.handle_event(ev);
                match r {
                    FormEvent::Submit => {
                        self.modal = Some(Modal::Source { form, kind, editing, advanced, secret_keys });
                        self.submit_source();
                    }
                    FormEvent::Cancel => {}
                    FormEvent::Changed(i) if fval(&form, i) == CREATE_OPT && secret_keys.iter().any(|(j, _)| *j == i) => {
                        let key = secret_keys.iter().find(|(j, _)| *j == i).map(|x| x.1.clone()).unwrap_or_default();
                        let nf = Form::new("Create a vault entry")
                            .text("Entry name", &format!("svc.{kind}"), "letters, digits . _ - ; saved as svc.<name>")
                            .secret("Secret value", "typed here, written to your vault, never shown again");
                        self.modal = Some(Modal::NewSecret { form: nf, src: form, kind, editing, advanced, keys: secret_keys, key, field: i });
                    }
                    FormEvent::Changed(i) if secret_keys.iter().any(|(j, k)| *j == i && k == "auth") => {
                        // the sign-in changed: ask for the secret that matches it
                        let tmp = self.config_from_form(&form, &kind, editing.as_deref(), advanced, &secret_keys).ok();
                        if let Some((f2, k2)) = self.source_form(&kind, tmp.as_ref(), advanced) {
                            self.modal = Some(Modal::Source { form: f2, kind, editing, advanced, secret_keys: k2 });
                        } else {
                            self.modal = Some(Modal::Source { form, kind, editing, advanced, secret_keys });
                        }
                    }
                    _ => self.modal = Some(Modal::Source { form, kind, editing, advanced, secret_keys }),
                }
            }
            Some(Modal::NewSecret { mut form, mut src, kind, editing, advanced, keys, key, field }) => match form.handle_event(ev) {
                FormEvent::Submit => {
                    let name = form.value(0);
                    let value = iyakki_agent::dash::connector::Zeroizing::new(form.raw(1));
                    match self.env.create_vault_entry(&name, &value) {
                        Ok(reference) => {
                            // the typed text is wiped from the field; only the reference goes on
                            form.set_value(1, "");
                            drop(value);
                            let mut cfg = self.config_from_form(&src, &kind, editing.as_deref(), advanced, &keys).ok();
                            if let Some(c) = cfg.as_mut() {
                                c.secrets.insert(key.clone(), reference.clone());
                            }
                            let title = src.title.clone();
                            if let Some((mut f2, k2)) = self.source_form(&kind, cfg.as_ref(), advanced) {
                                f2.title = title;
                                self.say(format!("Stored in your vault as {reference}."), true);
                                self.modal = Some(Modal::Source { form: f2, kind, editing, advanced, secret_keys: k2 });
                            }
                        }
                        Err(e) => {
                            form.set_value(1, "");
                            form.error = Some(e);
                            self.modal = Some(Modal::NewSecret { form, src, kind, editing, advanced, keys, key, field });
                        }
                    }
                }
                FormEvent::Cancel => {
                    src.set_choice(field, 0);
                    self.modal = Some(Modal::Source { form: src, kind, editing, advanced, secret_keys: keys });
                }
                _ => self.modal = Some(Modal::NewSecret { form, src, kind, editing, advanced, keys, key, field }),
            },
            Some(Modal::Name(mut form, what)) => match form.handle_event(ev) {
                FormEvent::Submit => {
                    let v = form.value(0).trim().to_string();
                    match what {
                        NameFor::Rename => {
                            if v.is_empty() {
                                form.error = Some("Give it a title.".into());
                                self.modal = Some(Modal::Name(form, what));
                            } else {
                                let _ = self.edit(|d| {
                                    d.title = v.chars().take(MAX_TITLE).collect();
                                    Ok(())
                                });
                            }
                        }
                        NameFor::New => {
                            let mut d = Dashboard::new("", &v);
                            d.title = v.chars().take(MAX_TITLE).collect();
                            self.open_dashboard(d);
                        }
                        NameFor::Import => match self.import_file(&v) {
                            Ok(d) => {
                                self.open_dashboard(d);
                                self.reload_lists();
                                let n = std::mem::take(&mut self.import_notes);
                                if !n.is_empty() {
                                    self.say(format!("Imported. {} thing(s) need your attention: {}", n.len(), ch::clip(&n.join(" · "), 220)), false);
                                }
                            }
                            Err(e) => {
                                form.error = Some(e);
                                self.modal = Some(Modal::Name(form, what));
                            }
                        },
                    }
                }
                FormEvent::Cancel => {}
                _ => self.modal = Some(Modal::Name(form, what)),
            },
            Some(Modal::Confirm(text, what)) => match ev {
                Event::Key(Key::Char('y' | 'Y') | Key::Enter) => self.confirmed(what, cx),
                Event::Key(Key::Char('n' | 'N') | Key::Esc) => {}
                _ => self.modal = Some(Modal::Confirm(text, what)),
            },
            Some(Modal::Template(mut p, ids, tid)) => match p.handle_event(ev) {
                PickEvent::Chosen(_) => {
                    if let Some(i) = p.selected_index() {
                        self.instantiate_template(&tid, &ids[i.min(ids.len() - 1)]);
                    }
                }
                PickEvent::Cancel => {}
                _ => self.modal = Some(Modal::Template(p, ids, tid)),
            },
        }
        Flow::Stay
    }

    fn confirmed(&mut self, what: Confirmed, _cx: &AppCx) {
        match what {
            Confirmed::DeleteSource(id) => {
                let _ = self.env.sources().remove(&id);
                self.reload_lists();
                self.say("Source removed.", true);
            }
            Confirmed::DeleteDashboard(id) => {
                let _ = self.env.dashboards().delete(&id);
                self.reload_lists();
                self.say("Dashboard deleted.", true);
            }
            Confirmed::Leave => {
                self.state.lock().unwrap_or_else(|e| e.into_inner()).mark_saved();
                self.screen = Screen::List;
                self.reload_lists();
            }
            Confirmed::DiscardEditor => {
                self.ed = None;
                self.screen = Screen::Board;
            }
        }
    }

    /// Read a dashboard file the person named (inside their folders only).
    fn import_file(&mut self, path: &str) -> Result<Dashboard, String> {
        let roots = iyakki_agent::scope::FileRoots::new(&self.env.policy, &self.env.user, &self.env.data_dir);
        let real = roots.resolve(&self.env.home, path)?;
        if !roots.read_ok(&self.env.policy, &self.env.user, &real) {
            return Err("you may not read that file".into());
        }
        let meta = std::fs::metadata(&real).map_err(|e| format!("{}: {e}", real.display()))?;
        if meta.len() > iyakki_agent::dash::store::MAX_FILE_BYTES {
            return Err("that file is too large to be a dashboard".into());
        }
        let text = std::fs::read_to_string(&real).map_err(|e| format!("{}: {e}", real.display()))?;
        let imp = self.env.dashboards().import_bundle(&text, &self.vault_names())?;
        let mut kept = 0;
        for cfg in imp.sources {
            // a source that is already here is left as it is; new ones are stored cleaned, without any secret
            if self.env.sources().get(&cfg.id).is_none() && self.env.sources().save(cfg).is_ok() {
                kept += 1;
            }
        }
        let mut notes = imp.warnings;
        if kept > 0 {
            notes.push(format!("{kept} source(s) were added; pick a vault entry for each one that needs a sign-in"));
        }
        self.import_notes = notes;
        Ok(imp.dashboard)
    }
}

/// The text of option `choice` for a choice field, the (trimmed) text of any other field.
fn fval(form: &Form, i: usize) -> String {
    match &form.fields[i].kind {
        super::widgets::FieldKind::Choice(o) => o.get(form.fields[i].choice).cloned().unwrap_or_default(),
        _ => form.value(i),
    }
}

pub const CREATE_OPT: &str = "(create a new vault entry…)";

fn toggle(v: &mut Vec<String>, name: &str) {
    if let Some(i) = v.iter().position(|x| x == name) {
        v.remove(i);
    } else {
        v.push(name.to_string());
    }
}

// ---------------------------------------------------------------------------------------------------------- drawing

/// One chip of a toolbar.
struct Tb {
    label: String,
    ev: Event,
    accent: [u8; 4],
    enabled: bool,
}

fn tb(label: &str, n: usize, accent: [u8; 4]) -> Tb {
    Tb { label: label.into(), ev: Event::Tab(n), accent, enabled: true }
}

/// Lay chips out left to right, wrapping to a new line; returns the height used.
fn toolbar(g: &mut Gfx, r: R, s: f32, items: &[Tb]) -> f32 {
    let size = 12.5 * s;
    let (h, gap) = (size * 2.1, 6.0 * s);
    let (mut x, mut y) = (r.x, r.y);
    for it in items {
        let w = g.text_width(size, &it.label, Face::Bold) + size * 1.7;
        if x + w > r.x + r.w && x > r.x {
            x = r.x;
            y += h + gap;
        }
        if it.enabled {
            g.button(x, y, size, &it.label, it.accent, it.ev.clone());
        } else {
            g.button_off(x, y, size, &it.label);
        }
        x += w + gap;
    }
    y + h - r.y
}

pub fn spinner(g: &mut Gfx, cx: f32, cy: f32, r: f32, tick: u64, c: [u8; 4]) {
    let a0 = tick as f32 * 0.35;
    let n = 10;
    let mut prev = (cx + a0.cos() * r, cy + a0.sin() * r);
    for k in 1..=n {
        let a = a0 + 1.7 * k as f32 / n as f32;
        let p = (cx + a.cos() * r, cy + a.sin() * r);
        g.line(prev.0, prev.1, p.0, p.1, 2.0, with_alpha(c, (60 + k * 19) as u8));
        prev = p;
    }
}

fn ago_text(ms: i64) -> String {
    let s = (ms / 1000).max(0);
    match s {
        0..=1 => "just now".into(),
        2..=59 => format!("{s}s ago"),
        60..=3599 => format!("{}m ago", s / 60),
        _ => format!("{}h ago", s / 3600),
    }
}

/// The text of a text panel: headings, bullets and plain lines (a light touch of Markdown).
fn draw_text_panel(g: &mut Gfx, body: R, s: f32, text: &str) {
    if text.trim().is_empty() {
        g.text(body.x + 6.0 * s, body.y + 6.0 * s, 13.0 * s, DIM, "Empty text panel. Edit it to add notes.", Face::Sans);
        return;
    }
    let mut y = body.y + 4.0 * s;
    for line in text.lines() {
        let (txt, size, face, col) = if let Some(h) = line.strip_prefix("### ").or_else(|| line.strip_prefix("## ")).or_else(|| line.strip_prefix("# ")) {
            (h.to_string(), 15.0 * s, Face::Bold, TEXT)
        } else if let Some(b) = line.strip_prefix("- ").or_else(|| line.strip_prefix("* ")) {
            (format!("•  {b}"), 13.0 * s, Face::Sans, MUTED)
        } else {
            (line.replace(['*', '`'], ""), 13.0 * s, Face::Sans, MUTED)
        };
        for l in g.wrap(size, &txt, face, body.w - 8.0 * s) {
            if y + size > body.y + body.h {
                return;
            }
            g.text(body.x + 4.0 * s, y, size, col, &l, face);
            y += size * 1.35;
        }
    }
}

/// A panel's body: the error, the spinner, or the chart/table/number for its visual.
#[allow(clippy::too_many_arguments)]
fn draw_body(g: &mut Gfx, body: R, s: f32, p: &Panel, pd: &mut PData, idx: usize, ptr: ch::Pointer, tick: u64) -> ch::ChartGeo {
    let mut geo = ch::ChartGeo::default();
    if p.visual == Visual::Text {
        draw_text_panel(g, body, s, &p.options.text);
        return geo;
    }
    if p.source.is_empty() || p.query.text.trim().is_empty() {
        g.text(body.x + 6.0 * s, body.y + 8.0 * s, 13.0 * s, DIM, "Choose a source and write a query: click Edit.", Face::Sans);
        return geo;
    }
    if let Some(e) = &pd.err {
        g.rrect(body.x, body.y, body.w, body.h.min(130.0 * s), 8.0 * s, with_alpha(ERR, 22));
        g.rrect_outline(body.x, body.y, body.w, body.h.min(130.0 * s), 8.0 * s, 1.0, with_alpha(ERR, 120));
        g.circle(body.x + 14.0 * s, body.y + 16.0 * s, 5.0 * s, ERR);
        g.text(body.x + 28.0 * s, body.y + 8.0 * s, 12.5 * s, ERR, "This panel could not be loaded", Face::Bold);
        let lines = g.wrap(12.0 * s, e, Face::Sans, body.w - 24.0 * s);
        let max = (((body.h.min(130.0 * s) - 34.0 * s) / (15.0 * s)) as usize).max(1);
        for (i, l) in lines.iter().take(max).enumerate() {
            let l = if i + 1 == max && lines.len() > max { format!("{l}…") } else { l.clone() };
            g.text(body.x + 12.0 * s, body.y + 30.0 * s + i as f32 * 15.0 * s, 12.0 * s, MUTED, &l, Face::Sans);
        }
        return geo;
    }
    let Some(f) = pd.frame.clone() else {
        if pd.loading {
            spinner(g, body.x + body.w / 2.0, body.y + body.h / 2.0, 10.0 * s, tick, ACCENT);
            let t = "asking the source…";
            let w = g.text_width(12.0 * s, t, Face::Sans);
            g.text(body.x + (body.w - w) / 2.0, body.y + body.h / 2.0 + 18.0 * s, 12.0 * s, DIM, t, Face::Sans);
        } else {
            g.text(body.x + 6.0 * s, body.y + 8.0 * s, 13.0 * s, DIM, "No data yet.", Face::Sans);
        }
        return geo;
    };
    // a part of the panel failed (one query, one expression): the rest stays, the failure is named at the bottom
    let mut body = body;
    if !pd.qerrors.is_empty() {
        let h = 17.0 * s;
        let strip = R::new(body.x, body.y + body.h - h, body.w, h);
        g.rrect(strip.x, strip.y, strip.w, strip.h, 6.0 * s, with_alpha(ERR, 26));
        g.circle(strip.x + 9.0 * s, strip.y + h / 2.0, 4.0 * s, ERR);
        let extra = if pd.qerrors.len() > 1 { format!("  (+{} more)", pd.qerrors.len() - 1) } else { String::new() };
        let t = g.fit(11.0 * s, &format!("{}{extra}", pd.qerrors[0]), Face::Sans, strip.w - 24.0 * s);
        g.text(strip.x + 18.0 * s, strip.y + 2.0 * s, 11.0 * s, ERR, &t, Face::Sans);
        body.h = (body.h - h - 2.0 * s).max(10.0);
    }
    if f.is_empty() {
        g.text(body.x + 6.0 * s, body.y + 8.0 * s, 13.0 * s, MUTED, "No data in this time range.", Face::Bold);
        for (i, n) in f.meta.notices.iter().take(3).enumerate() {
            let t = g.fit(11.5 * s, n, Face::Sans, body.w - 12.0 * s);
            g.text(body.x + 6.0 * s, body.y + 30.0 * s + i as f32 * 16.0 * s, 11.5 * s, DIM, &t, Face::Sans);
        }
        g.text(body.x + 6.0 * s, body.y + 34.0 * s + f.meta.notices.len().min(3) as f32 * 16.0 * s, 11.5 * s, DIM, "Check the names with the schema list in Edit, or widen the range.", Face::Sans);
        return geo;
    }
    match p.visual {
        Visual::Line | Visual::Area | Visual::Bar | Visual::StackedBar | Visual::Heatmap => {
            if let Some(m) = &pd.model {
                geo = ch::draw_xy(g, body, s, m, p.visual, &p.options, ptr, idx, tick);
            }
        }
        Visual::Stat => ch::draw_stat(g, body, s, &f, &p.options, templates::GREEN),
        Visual::Gauge => ch::draw_gauge(g, body, s, &f, &p.options, templates::GREEN),
        Visual::Pie | Visual::Donut => {
            if let Some(m) = &pd.model {
                ch::draw_pie(g, body, s, m, &p.options, p.visual == Visual::Donut);
            }
        }
        Visual::Table => {
            ch::draw_table(g, body, s, &f, &p.options, &mut pd.table, idx, ACCENT);
        }
        Visual::Text => {}
    }
    geo
}

impl DashApp {
    /// (main area, chat pane, whether the chat covers everything). The chat sits beside the board when the window is wide
    /// enough; in a small window (800x480) it is a toggle that takes the whole area.
    fn lay(&self, area: R, s: f32) -> (R, Option<R>, bool) {
        if !self.chat_open {
            return (area, None, false);
        }
        if area.w >= 1000.0 && area.w / s >= 980.0 {
            let cw = (area.w * 0.32).clamp_safe(340.0 * s, 520.0 * s);
            (R::new(area.x, area.y, area.w - cw, area.h), Some(R::new(area.x + area.w - cw, area.y, cw, area.h)), false)
        } else {
            (area, Some(area), true)
        }
    }

    fn pointer_for(&self, idx: usize) -> ch::Pointer {
        let over_chat = self.geo.chat.is_some_and(|c| c.contains(self.mouse.0, self.mouse.1));
        let hover = (!over_chat && self.modal.is_none() && self.drag.is_none() || matches!(self.drag, Some(Drag::Zoom { .. }))).then_some(self.mouse);
        let drag = match &self.drag {
            Some(Drag::Zoom { idx: i, x0, x1 }) if *i == idx => Some((*x0, *x1)),
            _ => None,
        };
        ch::Pointer { hover: if drag.is_some() { None } else { hover }, drag }
    }

    fn render_impl(&mut self, g: &mut Gfx, area: R, s: f32, cx: &AppCx, tick: u64) {
        self.poll();
        self.sync();
        self.geo.panels.clear();
        self.geo.plots.clear();
        self.geo.area = Some(area);
        let (main, chat, overlay) = self.lay(area, s);
        self.geo.chat = chat;
        let mut tip: Option<ch::Tooltip> = None;
        if !overlay {
            match self.screen.clone() {
                Screen::Board => tip = self.draw_board(g, main, s, tick),
                Screen::List => self.draw_list(g, main, s),
                Screen::Sources => self.draw_sources(g, main, s),
                Screen::Editor => tip = self.draw_editor(g, main, s, tick),
                Screen::Zoom(id) => self.draw_zoom(g, main, s, &id, tick),
            }
        }
        if let Some(t) = tip {
            ch::draw_tooltip(g, &t, s, main);
        }
        if let Some(c) = chat {
            self.draw_chat(g, c, s, cx, tick);
        }
        self.draw_modal(g, area, s, tick);
    }

    fn draw_chat(&mut self, g: &mut Gfx, r: R, s: f32, cx: &AppCx, tick: u64) {
        let focused = self.focus == Focus::Chat && self.modal.is_none();
        fx::panel(g, r, s, if focused { MAGENTA } else { ACCENT });
        let head = 28.0 * s;
        g.text(r.x + 12.0 * s, r.y + 7.0 * s, 13.0 * s, TEXT, "Assistant", Face::Bold);
        let sub = "works on this dashboard";
        let aw = g.text_width(13.0 * s, "Assistant ", Face::Bold);
        g.text(r.x + 12.0 * s + aw, r.y + 9.0 * s, 11.0 * s, DIM, sub, Face::Sans);
        let bw = g.button(r.x + r.w - 60.0 * s, r.y + 2.0 * s, 11.0 * s, "Hide", MUTED, Event::Tab(id::AI_HIDE));
        let _ = bw;
        fx::accent_line(g, r.x + 12.0 * s, r.y + head - 2.0, r.w - 24.0 * s, 1.5, if focused { MAGENTA } else { ACCENT }, tick, s);
        let content = R::new(r.x + 2.0 * s, r.y + head, r.w - 4.0 * s, (r.h - head - 2.0 * s).max(0.0));
        g.hit(R::new(r.x, r.y + head, r.w, r.h - head), Event::Tab(id::CHAT_PANE), "chat pane");
        let chat = &mut self.chat;
        super::with_caret(focused, || route::render_slot(g, CHAT_SLOT, |g| App::render(chat, g, content, s, cx, tick)));
    }

    fn draw_modal(&mut self, g: &mut Gfx, area: R, s: f32, tick: u64) {
        let blink = super::blink(tick);
        match &mut self.modal {
            None => {}
            Some(Modal::Kind(p)) => p.render(g, area, s, ACCENT),
            Some(Modal::Template(p, ..)) => p.render(g, area, s, ACCENT),
            Some(Modal::Source { form, .. }) => form.render(g, area, s, ACCENT, blink),
            Some(Modal::Name(form, _)) => form.render(g, area, s, ACCENT, blink),
            Some(Modal::NewSecret { form, .. }) => form.render(g, area, s, ACCENT, blink),
            Some(Modal::Confirm(text, _)) => super::widgets::render_confirm(g, area, s, text, "Yes"),
        }
    }

    // ------------------------------------------------------------------ the board

    fn draw_board(&mut self, g: &mut Gfx, main: R, s: f32, tick: u64) -> Option<ch::Tooltip> {
        let pad = 10.0 * s;
        let top = R::new(main.x + pad, main.y + 6.0 * s, main.w - 2.0 * pad, main.h);
        let range_now = |r: &str| self.d.time.to == "now" && self.d.time.from == format!("now-{r}");
        let mut items: Vec<Tb> = vec![tb("Dashboards", id::LIST, MUTED)];
        items.push(tb(&ch::clip(&self.d.title, 28), id::RENAME, ACCENT));
        for (i, r) in RANGE_CHOICES.iter().enumerate().take(7) {
            items.push(tb(r, id::RANGE + i, if range_now(r) { ACCENT } else { MUTED }));
        }
        items.push(tb("Zoom out", id::ZOOM_OUT, MUTED));
        items.push(tb(&if self.d.refresh_s == 0 { "Auto: off".to_string() } else { format!("Auto: {}s", self.d.refresh_s) }, id::REFRESH, if self.d.refresh_s > 0 { OK } else { MUTED }));
        items.push(tb("Refresh", id::NOW, MUTED));
        for (i, v) in self.d.variables.iter().enumerate().take(6) {
            let opts = self.var_opts.get(&v.name).cloned().unwrap_or_default();
            let cur = engine::variable_values(v, &if opts.is_empty() { v.current.clone() } else { opts });
            items.push(tb(&format!("${}: {}", v.name, if cur.is_empty() { "-".to_string() } else { ch::clip(&cur.join(","), 18) }), id::VAR + i, MAGENTA));
        }
        items.push(tb("+ Panel", id::ADD, ACCENT));
        items.push(tb("Sources", id::SOURCES, MUTED));
        let (can_undo, can_redo) = {
            let st = self.state.lock().unwrap_or_else(|e| e.into_inner());
            (st.can_undo(), st.can_redo())
        };
        items.push(Tb { enabled: can_undo, ..tb("Undo", id::UNDO, MUTED) });
        items.push(Tb { enabled: can_redo, ..tb("Redo", id::REDO, MUTED) });
        let dirty = self.state.lock().unwrap_or_else(|e| e.into_inner()).dirty();
        items.push(tb(if dirty { "Save*" } else { "Save" }, id::SAVE, if dirty { OK } else { MUTED }));
        items.push(tb("Export", id::EXPORT, MUTED));
        items.push(tb(if self.chat_open { "AI: on" } else { "AI" }, id::AI, MAGENTA));
        let tbh = toolbar(g, top, s, &items);
        let status_h = 22.0 * s;
        let board = R::new(main.x, main.y + 6.0 * s + tbh + 6.0 * s, main.w, (main.h - tbh - 12.0 * s - status_h).max(40.0));
        self.geo.board = Some(board);
        self.draw_status(g, R::new(main.x + pad, main.y + main.h - status_h, main.w - 2.0 * pad, status_h), s, tick);
        // the grid
        let cols = GRID_COLS as f32;
        let col_w = (board.w - 2.0 * pad) / cols;
        let row_h = 26.0 * s;
        self.geo.col_w = col_w;
        self.geo.row_h = row_h;
        let total = self.d.rows() as f32 * row_h + pad;
        self.scroll = self.scroll.clamp_safe(0.0, (total - board.h).max(0.0));
        if self.d.panels.is_empty() {
            let hint = if self.sources.is_empty() { "Add a data source (Prometheus, Elasticsearch, ClickHouse or DuckDB) with Sources, then add panels. Or ask the assistant." } else { "Add a panel with + Panel, start from a template in Dashboards, or ask the assistant to build one." };
            fx::empty_state(g, board, s, Icon::Dash, "This dashboard has no panels yet", hint, ACCENT);
            return None;
        }
        let mut tooltip = None;
        let scroll = self.scroll;
        let sel = self.selected();
        let ghost = self.ghost_rect(board, pad, col_w, row_h);
        g.clipped(board, |g| {
            for idx in 0..self.d.panels.len() {
                let gr = self.d.panels[idx].grid;
                let r = R::new(board.x + pad + gr.x as f32 * col_w + GAP * s / 2.0, board.y + gr.y as f32 * row_h - scroll + GAP * s / 2.0 + 2.0 * s, gr.w as f32 * col_w - GAP * s, gr.h as f32 * row_h - GAP * s);
                if r.y + r.h < board.y || r.y > board.y + board.h {
                    continue;
                }
                self.geo.panels.push((idx, r));
                if let Some(t) = self.draw_panel(g, idx, r, s, tick, sel == Some(idx)) {
                    tooltip = Some(t);
                }
            }
            if let Some(gh) = ghost {
                g.rrect(gh.x, gh.y, gh.w, gh.h, 12.0 * s, with_alpha(ACCENT, 30));
                g.rrect_outline(gh.x, gh.y, gh.w, gh.h, 12.0 * s, 1.6 * s, ACCENT);
            }
        });
        if total > board.h {
            fx::scrollbar(g, R::new(board.x + board.w - 8.0 * s, board.y, 5.0 * s, board.h), s, (self.scroll / row_h) as usize, (board.h / row_h) as usize, (total / row_h) as usize, ACCENT);
        }
        tooltip
    }

    fn ghost_rect(&self, board: R, pad: f32, col_w: f32, row_h: f32) -> Option<R> {
        let (dx, dy, orig, size) = match &self.drag {
            Some(Drag::Move { from, orig, .. }) => {
                let (dx, dy) = self.drag_cells(*from);
                (dx, dy, *orig, false)
            }
            Some(Drag::Size { from, orig, .. }) => {
                let (dx, dy) = self.drag_cells(*from);
                (dx, dy, *orig, true)
            }
            _ => return None,
        };
        let g = if size { Grid::new(orig.x, orig.y, (orig.w + dx).max(2), (orig.h + dy).max(2)) } else { Grid::new((orig.x + dx).clamp_safe(0, GRID_COLS - orig.w), (orig.y + dy).max(0), orig.w, orig.h) };
        Some(R::new(board.x + pad + g.x as f32 * col_w + 4.0, board.y + g.y as f32 * row_h - self.scroll + 6.0, g.w as f32 * col_w - 8.0, g.h as f32 * row_h - 8.0))
    }

    fn draw_status(&mut self, g: &mut Gfx, r: R, s: f32, tick: u64) {
        let size = 12.0 * s;
        let mut right = String::new();
        let loading = self.data.values().any(|d| d.loading);
        if loading {
            spinner(g, r.x + r.w - 10.0 * s, r.y + 10.0 * s, 6.0 * s, tick, ACCENT);
            right = "updating".into();
        } else if let Some(t) = self.data.values().map(|d| d.at_ms).max().filter(|t| *t > 0) {
            right = format!("updated {}", ago_text(self.now() - t));
        }
        let rw = g.text_width(size, &right, Face::Sans);
        let rx = r.x + r.w - rw - if loading { 22.0 * s } else { 0.0 };
        g.text(rx, r.y + 3.0 * s, size, DIM, &right, Face::Sans);
        match &self.status {
            Some((t, ok)) => fx::status_line(g, r.x, r.y + 2.0 * s, (rx - r.x - 10.0 * s).max(40.0), s, t, *ok),
            None => {
                let rg = resolve_range(&self.d.time, self.now());
                let (a, b) = (rg.from_ms, rg.to_ms);
                let t = format!("{} - {} UTC · {} panel(s)", format_time(a), format_time(b), self.d.panels.len());
                let t = g.fit(size, &t, Face::Sans, (rx - r.x - 10.0 * s).max(40.0));
                g.text(r.x + 2.0 * s, r.y + 3.0 * s, size, DIM, &t, Face::Sans);
            }
        }
    }

    /// One panel: frame, header, body. Returns the tooltip when the pointer is on its plot.
    fn draw_panel(&mut self, g: &mut Gfx, idx: usize, r: R, s: f32, tick: u64, selected: bool) -> Option<ch::Tooltip> {
        let p = self.d.panels[idx].clone();
        let hot = g.hovered(r);
        fx::panel(g, r, s, if selected { ACCENT } else { rgb(90, 110, 180) });
        if selected {
            g.rrect_outline(r.x, r.y, r.w, r.h, 16.0 * s, 1.8 * s, ACCENT);
        }
        if let Some((fid, until)) = &self.flash {
            if *fid == p.id {
                let pulse = 0.5 + 0.5 * ((tick as f32) * 0.35).sin();
                g.rrect_glow(r.x, r.y, r.w, r.h, 16.0 * s, 8.0 * s, with_alpha(MAGENTA, (60.0 + 120.0 * pulse) as u8));
                g.rrect_outline(r.x, r.y, r.w, r.h, 16.0 * s, 2.2 * s, MAGENTA);
                let _ = until;
            }
        }
        g.hit(r, Event::Tab(id::SEL + idx.min(99)), &format!("panel {}", p.title));
        let head_h = 26.0 * s;
        let head = R::new(r.x, r.y, r.w, head_h);
        g.hit(head, Event::Area(id::MOVE + idx.min(999)), &format!("move {}", p.title));
        // buttons on hover or selection
        let mut bx = r.x + r.w - 8.0 * s;
        let loading = self.data.get(&p.id).is_some_and(|d| d.loading);
        if loading {
            spinner(g, bx - 6.0 * s, r.y + head_h / 2.0, 5.0 * s, tick, ACCENT);
            bx -= 20.0 * s;
        }
        if self.data.get(&p.id).is_some_and(|d| !d.qerrors.is_empty()) {
            g.circle(bx - 7.0 * s, r.y + head_h / 2.0, 6.0 * s, ERR);
            let w = g.text_width(10.0 * s, "!", Face::Bold);
            g.text(bx - 7.0 * s - w / 2.0, r.y + head_h / 2.0 - 6.0 * s, 10.0 * s, rgb(20, 8, 12), "!", Face::Bold);
            bx -= 20.0 * s;
        }
        if (selected || hot) && r.w > 190.0 * s {
            for (label, base) in [("AI", id::ASK), ("View", id::VIEW), ("Edit", id::EDIT)] {
                let size = 11.0 * s;
                let w = g.text_width(size, label, Face::Bold) + 14.0 * s;
                bx -= w + 4.0 * s;
                let rect = R::new(bx, r.y + 4.0 * s, w, 18.0 * s);
                let hh = g.hovered(rect);
                g.rrect(rect.x, rect.y, rect.w, rect.h, 9.0 * s, if hh { with_alpha(ACCENT, 70) } else { rgb(24, 31, 58) });
                g.text(rect.x + 7.0 * s, rect.y + 2.5 * s, size, if hh { TEXT } else { MUTED }, label, Face::Bold);
                g.hit(rect, Event::Tab(base + idx.min(99)), label);
            }
        }
        let title = g.fit(12.5 * s, &p.title, Face::Bold, (bx - r.x - 16.0 * s).max(30.0));
        g.text(r.x + 10.0 * s, r.y + 6.0 * s, 12.5 * s, TEXT, &title, Face::Bold);
        let body = R::new(r.x + 8.0 * s, r.y + head_h, r.w - 16.0 * s, (r.h - head_h - 8.0 * s).max(10.0));
        let ptr = self.pointer_for(idx);
        let mut pd = self.data.remove(&p.id).unwrap_or_default();
        let geo = draw_body(g, body, s, &p, &mut pd, idx, ptr, tick);
        self.data.insert(p.id.clone(), pd);
        if let Some(pl) = geo.plot {
            if let Some(m) = self.data.get(&p.id).and_then(|d| d.model.as_ref()) {
                self.geo.plots.insert(idx, m.plot(pl));
            }
        }
        // the resize corner
        let corner = R::new(r.x + r.w - 16.0 * s, r.y + r.h - 16.0 * s, 16.0 * s, 16.0 * s);
        if selected || hot {
            g.line(corner.x + 6.0 * s, corner.y + 14.0 * s, corner.x + 14.0 * s, corner.y + 6.0 * s, 1.5, with_alpha(ACCENT, 160));
            g.line(corner.x + 10.0 * s, corner.y + 14.0 * s, corner.x + 14.0 * s, corner.y + 10.0 * s, 1.5, with_alpha(ACCENT, 160));
        }
        g.hit(corner, Event::Area(id::SIZE + idx.min(999)), "resize");
        geo.tooltip
    }

    // ------------------------------------------------------------------ the table view of one panel

    fn draw_zoom(&mut self, g: &mut Gfx, main: R, s: f32, pid: &str, tick: u64) {
        let pad = 10.0 * s;
        let Some(idx) = self.d.panels.iter().position(|p| p.id == pid) else {
            self.screen = Screen::Board;
            return;
        };
        let p = self.d.panels[idx].clone();
        let items = vec![tb("Back", id::BACK, MUTED), Tb { enabled: false, ..tb(&ch::clip(&p.title, 40), id::BACK, ACCENT) }, tb("Copy CSV", id::COPY, MUTED), tb("Export CSV", id::EXPORT_CSV, MUTED), tb("Refresh", id::NOW, MUTED)];
        let tbh = toolbar(g, R::new(main.x + pad, main.y + 6.0 * s, main.w - 2.0 * pad, main.h), s, &items);
        let body = R::new(main.x + pad, main.y + tbh + 14.0 * s, main.w - 2.0 * pad, (main.h - tbh - 40.0 * s).max(40.0));
        self.geo.board = Some(body);
        fx::panel(g, body, s, ACCENT);
        let inner = body.inset(8.0 * s);
        let mut pd = self.data.remove(pid).unwrap_or_default();
        if pd.frame.is_some() && pd.err.is_none() {
            let f = pd.frame.clone().unwrap();
            ch::draw_table(g, inner, s, &f, &p.options, &mut pd.table, idx, ACCENT);
        } else {
            let ptr = ch::Pointer::default();
            draw_body(g, inner, s, &p, &mut pd, idx, ptr, tick);
        }
        self.data.insert(pid.to_string(), pd);
        if let Some((t, ok)) = &self.status {
            fx::status_line(g, main.x + pad, main.y + main.h - 24.0 * s, main.w - 2.0 * pad, s, t, *ok);
        }
    }

    // ------------------------------------------------------------------ dashboards and templates

    fn draw_list(&mut self, g: &mut Gfx, main: R, s: f32) {
        let pad = 14.0 * s;
        let items = vec![tb("New dashboard", id::NEW, ACCENT), tb("Import", id::IMPORT, MUTED), tb("Open", id::OPEN, ACCENT), Tb { enabled: !self.list.is_empty() && self.list_sel < self.list.len(), ..tb("Delete", id::DELETE, ERR) }, tb("Sources", id::SOURCES, MUTED)];
        let tbh = toolbar(g, R::new(main.x + pad, main.y + 6.0 * s, main.w - 2.0 * pad, main.h), s, &items);
        let mut y = main.y + tbh + 18.0 * s;
        let bottom = main.y + main.h - 28.0 * s;
        let row_h = 46.0 * s;
        let x = main.x + pad;
        let w = main.w - 2.0 * pad;
        y += fx::section_title(g, x, y, w, s, "YOUR DASHBOARDS", ACCENT);
        let tpl = templates::all();
        let n = self.list.len();
        // a window of rows around the selection so a long list stays reachable
        let rows_fit = (((bottom - y) / row_h) as usize).max(3);
        let total = n + tpl.len() + 1;
        let first = self.list_sel.saturating_sub(rows_fit.saturating_sub(2)).min(total.saturating_sub(rows_fit));
        let mut shown_template_header = false;
        let mut row = 0usize;
        let mut i = first;
        // rows: dashboards, then (a heading drawn between) templates
        while i < n + tpl.len() && y + row_h <= bottom + 1.0 {
            if i >= n && !shown_template_header {
                y += fx::section_title(g, x, y + 4.0 * s, w, s, "START FROM A TEMPLATE", MAGENTA) + 4.0 * s;
                shown_template_header = true;
                if y + row_h > bottom + 1.0 {
                    break;
                }
            }
            let r = R::new(x, y, w, row_h - 6.0 * s);
            let sel = i == self.list_sel;
            fx::list_row(g, r, s, sel, g.hovered(r), if i < n { ACCENT } else { MAGENTA });
            if i < n {
                let e = &self.list[i];
                let t = g.fit(15.0 * s, &e.title, Face::Bold, w - 160.0 * s);
                g.text(r.x + 14.0 * s, r.y + 6.0 * s, 15.0 * s, TEXT, &t, Face::Bold);
                g.text(r.x + 14.0 * s, r.y + 24.0 * s, 11.5 * s, DIM, &format!("{} panel(s) · {}", e.panels, e.id), Face::Sans);
            } else {
                let t = &tpl[i - n];
                let title = g.fit(14.0 * s, t.title, Face::Bold, w - 160.0 * s);
                g.text(r.x + 14.0 * s, r.y + 5.0 * s, 14.0 * s, TEXT, &title, Face::Bold);
                let about = g.fit(11.5 * s, t.about, Face::Sans, w - 160.0 * s);
                g.text(r.x + 14.0 * s, r.y + 24.0 * s, 11.5 * s, DIM, &about, Face::Sans);
                let have = self.sources.iter().any(|sx| sx.kind == t.kind);
                let chip = if have { format!("{}", registry::kind(t.kind).map_or(t.kind.to_string(), |k| k.label)) } else { format!("needs {}", registry::kind(t.kind).map_or(t.kind.to_string(), |k| k.label)) };
                let cw = g.text_width(11.0 * s, &chip, Face::Bold) + 16.0 * s;
                g.rrect(r.x + r.w - cw - 10.0 * s, r.y + 12.0 * s, cw, 18.0 * s, 9.0 * s, with_alpha(if have { OK } else { ERR }, 30));
                g.text(r.x + r.w - cw - 2.0 * s, r.y + 14.0 * s, 11.0 * s, if have { OK } else { ERR }, &chip, Face::Bold);
            }
            g.hit(r, Event::Row(row_event(i)), "row");
            y += row_h;
            i += 1;
            row += 1;
        }
        let _ = row;
        if n == 0 && first == 0 {
            // nothing saved yet: the heading above already says so; keep the hint short
            g.text(x + 4.0 * s, main.y + main.h - 26.0 * s, 12.0 * s, DIM, "Nothing saved yet: pick a template below, or create a new dashboard.", Face::Sans);
        }
        if let Some((t, ok)) = &self.status {
            fx::status_line(g, x, main.y + main.h - 24.0 * s, w, s, t, *ok);
        }
    }

    // ------------------------------------------------------------------ sources

    fn draw_sources(&mut self, g: &mut Gfx, main: R, s: f32) {
        let pad = 14.0 * s;
        let have = !self.sources.is_empty();
        let items = vec![
            tb("Back", id::BACK, MUTED),
            tb("Add source", id::S_ADD, ACCENT),
            Tb { enabled: have, ..tb("Edit", id::S_EDIT, MUTED) },
            Tb { enabled: have, ..tb("Connection options", id::S_ADV, MUTED) },
            Tb { enabled: have, ..tb("Use as default", id::S_DEFAULT, MUTED) },
            Tb { enabled: have, ..tb("Test", id::S_TEST, OK) },
            Tb { enabled: have, ..tb("Remove", id::S_DEL, ERR) },
        ];
        let tbh = toolbar(g, R::new(main.x + pad, main.y + 6.0 * s, main.w - 2.0 * pad, main.h), s, &items);
        let x = main.x + pad;
        let w = main.w - 2.0 * pad;
        let mut y = main.y + tbh + 18.0 * s;
        y += fx::section_title(g, x, y, w, s, "DATA SOURCES", ACCENT);
        let bottom = main.y + main.h - 56.0 * s;
        if !have {
            fx::empty_state(g, R::new(x, y, w, bottom - y), s, Icon::Dash, "No data sources yet", "Add Prometheus, Elasticsearch, ClickHouse or DuckDB. Passwords and tokens are never typed here: pick them from your vault.", ACCENT);
        }
        let row_h = 56.0 * s;
        for (i, c) in self.sources.iter().enumerate() {
            if y + row_h > bottom {
                break;
            }
            let r = R::new(x, y, w, row_h - 6.0 * s);
            fx::list_row(g, r, s, i == self.src_sel, g.hovered(r), ACCENT);
            let info = registry::kind(&c.kind);
            let is_default = !self.d.default_source.is_empty() && self.d.default_source == c.id;
            let name = g.fit(15.0 * s, &format!("{}{}", c.name, if is_default { "  (default)" } else { "" }), Face::Bold, w * 0.4);
            g.text(r.x + 14.0 * s, r.y + 6.0 * s, 15.0 * s, TEXT, &name, Face::Bold);
            let sub = format!("{} · {} · {}", info.as_ref().map_or(c.kind.as_str(), |k| k.label.as_str()), info.as_ref().map_or("", |k| k.language.name()), if c.value("url").is_empty() { ch::clip(&format!("{}{}", c.value("database"), c.value("files")), 60) } else { c.value("url").to_string() });
            let sub = g.fit(11.5 * s, &sub, Face::Sans, w - 40.0 * s);
            g.text(r.x + 14.0 * s, r.y + 25.0 * s, 11.5 * s, DIM, &sub, Face::Sans);
            if let Some((ok, t)) = self.src_status.get(&c.id) {
                let t = g.fit(11.5 * s, t, Face::Sans, w * 0.5);
                let tw = g.text_width(11.5 * s, &t, Face::Sans);
                g.circle(r.x + r.w - tw - 22.0 * s, r.y + 14.0 * s, 4.0 * s, if *ok { OK } else { ERR });
                g.text(r.x + r.w - tw - 12.0 * s, r.y + 7.0 * s, 11.5 * s, if *ok { MUTED } else { ERR }, &t, Face::Sans);
            }
            g.hit(r, Event::Row(id::ROW_SRC + i), "source");
            y += row_h;
        }
        let note = "Use a read-only account on the server. Only reads are ever sent.";
        g.text(x, main.y + main.h - 46.0 * s, 12.0 * s, DIM, note, Face::Sans);
        if let Some((t, ok)) = &self.status {
            fx::status_line(g, x, main.y + main.h - 24.0 * s, w, s, t, *ok);
        }
    }
}

fn row_event(i: usize) -> usize {
    i
}

// ---------------------------------------------------------------------------------------------------------- the panel editor

/// Small pills that wrap onto several lines; returns the height used. Each is (label, click, on).
fn pills(g: &mut Gfx, r: R, s: f32, items: &[(String, Event, bool)], accent: [u8; 4]) -> f32 {
    let size = 11.5 * s;
    let (h, gap) = (22.0 * s, 5.0 * s);
    let (mut x, mut y) = (r.x, r.y);
    for (label, ev, on) in items {
        let label = g.fit(size, label, Face::Bold, r.w - 12.0 * s);
        let w = g.text_width(size, &label, Face::Bold) + 16.0 * s;
        if x + w > r.x + r.w && x > r.x {
            x = r.x;
            y += h + gap;
        }
        let rect = R::new(x, y, w, h);
        let hot = g.hovered(rect);
        g.rrect(rect.x, rect.y, rect.w, rect.h, h / 2.0, if *on { with_alpha(accent, 64) } else if hot { rgb(40, 50, 86) } else { rgb(22, 29, 54) });
        g.rrect_outline(rect.x, rect.y, rect.w, rect.h, h / 2.0, if *on { 1.4 * s } else { 1.0 }, if *on { accent } else { rgb(62, 74, 112) });
        g.text(rect.x + 8.0 * s, rect.y + (h - g.line_height(size, Face::Bold)) / 2.0, size, if *on { TEXT } else { MUTED }, &label, Face::Bold);
        g.hit(rect, ev.clone(), &label);
        x += w + gap;
    }
    y + h - r.y
}

fn label(g: &mut Gfx, x: f32, y: f32, s: f32, t: &str) -> f32 {
    g.text(x, y, 10.5 * s, DIM, &t.to_uppercase(), Face::Bold);
    15.0 * s
}

impl DashApp {
    fn draw_editor(&mut self, g: &mut Gfx, main: R, s: f32, tick: u64) -> Option<ch::Tooltip> {
        let Some(mut ed) = self.ed.take() else {
            self.screen = Screen::Board;
            return None;
        };
        let pad = 12.0 * s;
        let tip = self.draw_editor_inner(g, main, s, tick, &mut ed, pad);
        self.ed = Some(ed);
        tip
    }

    fn draw_editor_inner(&mut self, g: &mut Gfx, main: R, s: f32, tick: u64, ed: &mut Ed, pad: f32) -> Option<ch::Tooltip> {
        let lang = self.kind_info(&ed.cur_source()).or_else(|| self.kind_info(&self.d.default_source)).map(|k| k.language);
        let items = vec![
            tb("Cancel", id::E_CANCEL, MUTED),
            tb("Run", id::E_RUN, OK),
            tb("Apply", id::E_APPLY, ACCENT),
            Tb { label: if ed.is_new { "New panel".into() } else { "Edit panel".into() }, ev: Event::Tab(id::E_PAGE + 3), accent: DIM, enabled: false },
        ];
        let tbh = toolbar(g, R::new(main.x + pad, main.y + 6.0 * s, main.w - 2.0 * pad, main.h), s, &items);
        let content = R::new(main.x + pad, main.y + tbh + 14.0 * s, main.w - 2.0 * pad, (main.h - tbh - 40.0 * s).max(60.0));
        if let Some((t, ok)) = &self.status {
            fx::status_line(g, main.x + pad, main.y + main.h - 22.0 * s, main.w - 2.0 * pad, s, t, *ok);
        }
        let wide = main.w / s >= 1000.0;
        // the row of query tabs and the row of that query's settings sit above the query box
        let qt = 56.0 * s;
        let blink = super::blink(tick);
        let focus_ok = self.focus == Focus::Board && self.modal.is_none();
        // the three areas: setup (fields), query + schema, preview
        let (setup, qbox, schema, preview);
        if wide {
            let lw = (content.w * 0.30).clamp_safe(250.0 * s, 360.0 * s);
            let rest = R::new(content.x + lw + 10.0 * s, content.y, content.w - lw - 10.0 * s, content.h);
            let qh = (rest.h * 0.38).clamp_safe(110.0 * s, 260.0 * s);
            setup = Some(R::new(content.x, content.y, lw, content.h));
            qbox = Some(R::new(rest.x, rest.y + 20.0 * s + qt, rest.w, qh));
            schema = Some(R::new(content.x, content.y, lw, content.h)); // below the fields; placed after they are drawn
            preview = Some(R::new(rest.x, rest.y + qh + 62.0 * s + qt, rest.w, (rest.h - qh - 62.0 * s - qt).max(60.0)));
        } else {
            let tabs: Vec<(String, Event)> = ["Setup", "Query", "Preview"].iter().enumerate().map(|(i, t)| (t.to_string(), Event::Tab(id::E_PAGE + i))).collect();
            let used = fx::tab_strip(g, content.x, content.y, s, content.w, &tabs, ed.page.min(2), ACCENT);
            let _ = used;
            let page = R::new(content.x, content.y + 36.0 * s, content.w, (content.h - 36.0 * s).max(40.0));
            match ed.page.min(2) {
                0 => {
                    setup = Some(page);
                    qbox = None;
                    schema = None;
                    preview = None;
                }
                1 => {
                    setup = None;
                    let lw = (page.w * 0.34).clamp_safe(150.0 * s, 260.0 * s);
                    schema = Some(R::new(page.x, page.y, lw, page.h));
                    qbox = Some(R::new(page.x + lw + 8.0 * s, page.y + 20.0 * s + qt, page.w - lw - 8.0 * s, (page.h * 0.62 - qt).max(56.0)));
                    preview = None;
                }
                _ => {
                    setup = None;
                    qbox = None;
                    schema = None;
                    preview = Some(page);
                }
            }
        }
        let mut fields_bottom = content.y;
        if let Some(r) = setup {
            fields_bottom = self.draw_setup(g, r, s, ed, blink, focus_ok);
        }
        if let Some(r) = schema {
            let top = if wide { fields_bottom + 8.0 * s } else { r.y };
            self.draw_schema(g, R::new(r.x, top, r.w, (r.y + r.h - top).max(0.0)), s, ed, focus_ok);
        }
        if let Some(r) = qbox {
            self.draw_query_tabs(g, R::new(r.x, r.y - 16.0 * s - qt + 2.0 * s, r.w, qt - 4.0 * s), s, ed);
            if ed.show_expr {
                g.text(r.x, r.y - 16.0 * s, 10.5 * s, DIM, "EXPRESSIONS · ONE PER LINE · NAME = A / B * 100 · JOIN · TOP · FILTER · GROUP", Face::Bold);
                ed.exprs.draw(g, r, s, ed.focus == EdFocus::Query && focus_ok, tick, "C = A / B * 100      T = join A, B on key      align interpolate 30s");
                ed.store();
                let lines: Vec<String> = ed.panel.expressions.iter().filter(|l| !l.to_ascii_lowercase().starts_with("align ")).cloned().collect();
                if let Some(e) = iyakki_agent::dash::expr::check(&lines).first() {
                    let t = g.fit(11.5 * s, e, Face::Sans, r.w);
                    g.text(r.x, r.y + r.h + 6.0 * s, 11.5 * s, ERR, &t, Face::Sans);
                } else {
                    g.text(r.x, r.y + r.h + 6.0 * s, 11.5 * s, DIM, "A, B, C are the queries; each line makes a new series or a joined table. Run shows the result.", Face::Sans);
                }
            } else {
                let l = lang.map_or("Query", |l| l.name());
                g.text(r.x, r.y - 16.0 * s, 10.5 * s, DIM, &format!("{} · QUERY {} · Ctrl+R runs it, only reads are sent", l, query_letter(ed.cur)).to_uppercase(), Face::Bold);
                ed.query.draw(g, r, s, ed.focus == EdFocus::Query && focus_ok, tick, "type the query, or pick a metric / table / index on the left");
                let by = r.y + r.h + 6.0 * s;
                let mut bx = r.x;
                bx += g.button(bx, by, 11.5 * s, "Run", OK, Event::Tab(id::E_RUN2)) + 6.0 * s;
                bx += g.button(bx, by, 11.5 * s, "Starter query", MUTED, Event::Tab(id::E_STARTER)) + 6.0 * s;
                if lang == Some(iyakki_agent::dash::connector::Lang::EsDsl) {
                    bx += g.button(bx, by, 11.5 * s, "Top values", MUTED, Event::Tab(id::E_TOP)) + 6.0 * s;
                    g.button(bx, by, 11.5 * s, "Over time", MUTED, Event::Tab(id::E_OVER));
                }
            }
        }
        let mut tip = None;
        if let Some(r) = preview {
            g.text(r.x, r.y - 16.0 * s, 10.5 * s, DIM, "PREVIEW", Face::Bold);
            fx::panel(g, r, s, ACCENT);
            let body = r.inset(8.0 * s);
            let p = ed.preview_panel();
            let ptr = ch::Pointer { hover: Some(self.mouse).filter(|_| self.modal.is_none() && self.drag.is_none()), drag: None };
            let geo = draw_body(g, body, s, &p, &mut ed.pd, 59, ptr, tick);
            if ed.pd.loading {
                spinner(g, r.x + r.w - 16.0 * s, r.y + 14.0 * s, 6.0 * s, tick, ACCENT);
            }
            if let Some(pl) = geo.plot {
                if let Some(m) = &ed.pd.model {
                    self.geo.plots.insert(59, m.plot(pl));
                }
            }
            tip = geo.tooltip;
        }
        tip
    }

    /// The tabs of the panel's queries (A, B, ...), the Expressions tab, and the settings of the query that is open.
    fn draw_query_tabs(&mut self, g: &mut Gfx, r: R, s: f32, ed: &Ed) {
        let mut items: Vec<(String, Event, bool)> = vec![];
        let dsrc = &self.d.default_source;
        let defs = ed.panel.query_defs(dsrc);
        for (i, (l, d)) in defs.iter().enumerate() {
            let name = self.sources.iter().find(|x| x.id == d.source).map_or(String::new(), |x| ch::clip(&x.name, 10));
            let off = if d.query.hidden { " (off)" } else { "" };
            items.push((format!("{l} {name}{off}").trim().to_string(), Event::Tab(id::E_Q + i), i == ed.cur && !ed.show_expr));
        }
        if defs.len() < MAX_QUERIES {
            items.push(("+ query".into(), Event::Tab(id::E_QADD), false));
        }
        items.push((if ed.panel.expressions.is_empty() { "Expressions".into() } else { format!("Expressions ({})", ed.panel.expressions.len()) }, Event::Tab(id::E_EXPR), ed.show_expr));
        let h1 = pills(g, r, s, &items, ACCENT);
        // the settings of the open query
        let q = ed.cur_query();
        let unit = q.unit.unwrap_or(if ed.cur == 0 { ed.panel.options.unit } else { Unit::None });
        let mut set: Vec<(String, Event, bool)> = vec![
            (format!("Axis: {}", if q.axis == Axis::Right { "right" } else { "left" }), Event::Tab(id::E_AXIS), q.axis == Axis::Right),
            (if q.hidden { "Hidden".into() } else { "Shown".into() }, Event::Tab(id::E_HIDE), q.hidden),
            (format!("Unit: {}", unit.label()), Event::Tab(id::E_UNIT), false),
            ("Preview this query".into(), Event::Tab(id::E_RUN_ONE), false),
        ];
        if ed.cur > 0 && !ed.show_expr {
            set.push(("Remove query".into(), Event::Tab(id::E_QDEL), false));
        }
        if !ed.show_expr {
            pills(g, R::new(r.x, r.y + h1 + 4.0 * s, r.w, r.h), s, &set, MAGENTA);
        }
    }

    /// Source, title, visual, unit, target and legend. Returns the y below the last field.
    fn draw_setup(&mut self, g: &mut Gfx, r: R, s: f32, ed: &mut Ed, blink: bool, focus_ok: bool) -> f32 {
        let mut y = r.y;
        y += label(g, r.x, y, s, &format!("Source of query {}", query_letter(ed.cur)));
        let src: Vec<(String, Event, bool)> = self.sources.iter().enumerate().map(|(i, c)| (c.name.clone(), Event::Tab(id::E_SRC + i.min(59)), c.id == ed.cur_source() || (ed.cur_source().is_empty() && c.id == self.d.default_source))).collect();
        if src.is_empty() {
            g.text(r.x, y, 12.5 * s, ERR, "No sources yet: leave and add one under Sources.", Face::Sans);
            y += 20.0 * s;
        } else {
            y += pills(g, R::new(r.x, y, r.w, 80.0 * s), s, &src, ACCENT) + 8.0 * s;
        }
        y += label(g, r.x, y, s, "Title");
        let fr = R::new(r.x, y, r.w, 30.0 * s);
        ed.title.render(g, fr, ed.focus == EdFocus::Title && focus_ok, s, blink, Face::Sans, id::F_TITLE);
        y += 36.0 * s;
        y += label(g, r.x, y, s, "Show as");
        let vis: Vec<(String, Event, bool)> = Visual::ALL.iter().enumerate().map(|(i, v)| (v.label().to_string(), Event::Tab(id::E_VIS + i), *v == ed.panel.visual)).collect();
        y += pills(g, R::new(r.x, y, r.w, 90.0 * s), s, &vis, MAGENTA) + 8.0 * s;
        y += label(g, r.x, y, s, "Index / table / file");
        ed.target.render(g, R::new(r.x, y, r.w, 30.0 * s), ed.focus == EdFocus::Target && focus_ok, s, blink, Face::Sans, id::F_TARGET);
        y += 36.0 * s;
        y += label(g, r.x, y, s, if ed.cur == 0 { "Legend ({{label}})" } else { "Alias ({{label}})" });
        ed.legend.render(g, R::new(r.x, y, r.w, 30.0 * s), ed.focus == EdFocus::Legend && focus_ok, s, blink, Face::Sans, id::F_LEGEND);
        y + 36.0 * s
    }

    fn draw_schema(&mut self, g: &mut Gfx, r: R, s: f32, ed: &mut Ed, focus_ok: bool) {
        if r.h < 40.0 * s {
            return;
        }
        let head = if ed.path.is_empty() { "Schema".to_string() } else { format!("Schema / {}", ed.path.join(" / ")) };
        let head = g.fit(10.5 * s, &head.to_uppercase(), Face::Bold, r.w - 60.0 * s);
        g.text(r.x, r.y, 10.5 * s, if ed.focus == EdFocus::Nodes && focus_ok { ACCENT } else { DIM }, &head, Face::Bold);
        if !ed.path.is_empty() {
            g.button(r.x + r.w - 46.0 * s, r.y - 6.0 * s, 10.5 * s, "Up", MUTED, Event::Tab(id::E_UP));
        }
        let list = R::new(r.x, r.y + 18.0 * s, r.w, r.h - 18.0 * s);
        fx::panel(g, list, s, ACCENT);
        if ed.nodes_loading {
            spinner(g, list.x + list.w / 2.0, list.y + 20.0 * s, 7.0 * s, self.tick, ACCENT);
            return;
        }
        if let Some(e) = &ed.nodes_err {
            for (i, l) in g.wrap(11.5 * s, e, Face::Sans, list.w - 16.0 * s).iter().take(6).enumerate() {
                g.text(list.x + 8.0 * s, list.y + 8.0 * s + i as f32 * 15.0 * s, 11.5 * s, ERR, l, Face::Sans);
            }
            return;
        }
        let row_h = 22.0 * s;
        let fit = (((list.h - 8.0 * s) / row_h) as usize).max(1);
        let first = ed.node_sel.saturating_sub(fit.saturating_sub(1)).min(ed.nodes.len().saturating_sub(fit));
        for (k, n) in ed.nodes.iter().enumerate().skip(first).take(fit) {
            let rr = R::new(list.x + 4.0 * s, list.y + 4.0 * s + (k - first) as f32 * row_h, list.w - 8.0 * s, row_h - 2.0 * s);
            let sel = k == ed.node_sel;
            if sel {
                g.rrect(rr.x, rr.y, rr.w, rr.h, 6.0 * s, with_alpha(ACCENT, if ed.focus == EdFocus::Nodes { 70 } else { 36 }));
            } else if g.hovered(rr) {
                g.rrect(rr.x, rr.y, rr.w, rr.h, 6.0 * s, with_alpha(ACCENT, 20));
            }
            let tag = match n.kind {
                NodeKind::Database => "db",
                NodeKind::Index => "idx",
                NodeKind::Table => "tbl",
                NodeKind::Metric => "met",
                NodeKind::Column => "col",
                NodeKind::Label => "lbl",
                NodeKind::Field => "fld",
                NodeKind::File => "file",
            };
            g.text(rr.x + 4.0 * s, rr.y + 3.0 * s, 10.0 * s, DIM, tag, Face::Mono);
            let name = g.fit(12.0 * s, &n.name, Face::Sans, rr.w - 40.0 * s - if n.detail.is_empty() { 0.0 } else { (rr.w * 0.35).min(120.0 * s) });
            g.text(rr.x + 32.0 * s, rr.y + 2.5 * s, 12.0 * s, TEXT, &name, Face::Sans);
            if !n.detail.is_empty() {
                let d = g.fit(10.5 * s, &n.detail, Face::Sans, rr.w * 0.35);
                let dw = g.text_width(10.5 * s, &d, Face::Sans);
                g.text(rr.x + rr.w - dw - 6.0 * s, rr.y + 4.0 * s, 10.5 * s, DIM, &d, Face::Sans);
            }
            g.hit(rr, Event::Row(id::ROW_NODE + k), &format!("node {}", n.name));
        }
        if ed.nodes.is_empty() {
            g.text(list.x + 8.0 * s, list.y + 8.0 * s, 11.5 * s, DIM, "Nothing here.", Face::Sans);
        }
    }
}

// ---------------------------------------------------------------------------------------------------------- the App

impl App for DashApp {
    fn title(&self) -> &'static str {
        "Dashboards"
    }
    fn icon(&self) -> Icon {
        Icon::Dash
    }
    fn accent(&self) -> [u8; 4] {
        ACCENT
    }
    fn hint(&self) -> String {
        match self.screen {
            Screen::Board => "arrows choose a panel · Enter edit · a add · v table · i ask AI · drag a chart to zoom · Ctrl+Z undo · Ctrl+I assistant · F1 all keys".into(),
            Screen::List => "Enter open · n new · i import · s sources · Esc leave".into(),
            Screen::Sources => "a add · e edit · o connection options · t test · Delete remove · Esc back".into(),
            Screen::Editor => "Ctrl+R run · Ctrl+S apply · Tab next field · Esc cancel".into(),
            Screen::Zoom(_) => "arrows move · c copy cell · r copy row · PageUp/Down · Esc back".into(),
        }
    }
    fn help(&self) -> Vec<String> {
        [
            "Board: arrows choose a panel, Enter or e edits it, a adds one, d duplicates, Delete removes (Ctrl+Z brings it back)",
            "Board: Shift+arrows move a panel, Ctrl+arrows resize it; or drag its title bar / its corner",
            "Charts: hover for values, drag across a chart to zoom into that time, click a legend entry to hide a line",
            "1 to 7 pick the time range, z zooms out, t changes auto refresh, r refreshes now",
            "v opens the data as a table (sort by header, c copies a cell, r a row); c on the board copies the panel as CSV",
            "i asks the assistant about the selected panel, Ctrl+I shows or hides the assistant, F6 moves between board and assistant",
            "Ctrl+S saves the dashboard, Ctrl+Z / Ctrl+Y undo and redo (also for what the assistant changed)",
            "Editor: Ctrl+R runs the preview, Ctrl+S applies, Tab moves between fields, Esc cancels",
            "Sources: only reads are ever sent; passwords and tokens are picked from your vault, never typed here",
        ]
        .map(String::from)
        .to_vec()
    }
    fn take_host_save(&mut self) -> Option<(String, Vec<u8>)> {
        self.host_save.take()
    }
    fn wants_redraw(&mut self) -> bool {
        self.poll();
        self.sync();
        let chat = self.chat_open && self.chat.wants_redraw();
        std::mem::take(&mut self.redraw) || chat
    }
    fn pointer_hint(&self, x: i32, y: i32) -> Option<crate::ui::Pointer> {
        if self.geo.plots.values().any(|p| p.r.contains(x, y)) { Some(crate::ui::Pointer::Crosshair) } else { None }
    }
    fn context_items(&mut self, x: i32, y: i32) -> Vec<Item> {
        if self.chat_open && self.chat_rect().is_some_and(|c| c.contains(x, y)) {
            return App::context_items(&mut self.chat, x, y);
        }
        if self.chat_rect().is_some_and(|c| c.contains(x, y)) || self.screen != Screen::Board {
            return vec![];
        }
        let Some((idx, _)) = self.geo.panels.iter().find(|(_, r)| r.contains(x, y)).cloned() else { return vec![] };
        self.select(Some(idx));
        let on_plot = self.geo.plots.get(&idx).is_some_and(|p| p.r.contains(x, y));
        let mut v = vec![Item::new("dash.copy", "Copy data (CSV)", ""), Item::new("dash.edit", "Edit panel", "Enter"), Item::new("dash.view", "Show as table", "v")];
        if on_plot {
            v.push(Item::new("dash.ask_here", "Ask AI about this moment", ""));
        }
        v.push(Item::new("dash.ask", "Ask AI about this panel", "i"));
        v.push(Item::new("dash.export", "Export CSV…", ""));
        v.push(Item::separator());
        v.push(Item::new("dash.remove", "Remove panel", "Del"));
        v
    }
    fn take_request(&mut self) -> Option<(String, String)> {
        // a link clicked in the chat that opens the BROWSER tile (Back returns here)
        App::take_request(&mut self.chat)
    }
    fn context_pick(&mut self, id: &str, cx: &AppCx) -> Flow {
        if id.starts_with("link.") || id == "copymsg" || id == "copycode" {
            return App::context_pick(&mut self.chat, id, cx);
        }
        let Some(idx) = self.selected() else { return Flow::Stay };
        match id {
            "dash.copy" => self.copy_panel_data(),
            "dash.edit" => {
                let p = self.d.panels[idx].clone();
                self.open_editor(p, false, cx);
            }
            "dash.view" => self.screen = Screen::Zoom(self.d.panels[idx].id.clone()),
            "dash.ask" => self.ask_ai(idx, None, cx),
            "dash.ask_here" => {
                let w = self.window_at_mouse(idx);
                self.ask_ai(idx, w, cx)
            }
            "dash.export" => self.export_csv(idx, cx),
            "dash.remove" => self.remove_selected(),
            _ => {}
        }
        self.redraw = true;
        Flow::Stay
    }
    fn handle(&mut self, ev: &Event, cx: &AppCx) -> Flow {
        let f = self.handle_impl(ev, cx);
        self.ver += 1;
        f
    }
    fn render(&mut self, g: &mut Gfx, area: R, s: f32, cx: &AppCx, tick: u64) {
        self.render_impl(g, area, s, cx, tick);
    }
}

#[cfg(test)]
impl DashApp {
    pub fn open_dashboard_for_test(&mut self, d: Dashboard) {
        self.open_dashboard(d);
    }
    pub fn go_for_test(&mut self, screen: &str) {
        self.modal = None;
        match screen {
            "board" => {
                self.ed = None;
                self.screen = Screen::Board;
            }
            "list" => {
                self.reload_lists();
                self.screen = Screen::List;
            }
            "sources" => {
                self.reload_lists();
                self.screen = Screen::Sources;
            }
            "editor" => {
                let p = self.d.panels.first().cloned().unwrap_or_else(|| Panel::new("", "New panel", "prom", Visual::Line, "up", Grid::new(0, 0, 12, 8)));
                let cx = crate::apps::tests::cx_with_admin_store("dash-go");
                self.open_editor(p, false, &cx);
            }
            "editor-check" => assert!(self.ed.is_some() && self.screen == Screen::Editor, "the editor is open"),
            "zoom" => {
                let id = self.d.panels.iter().find(|p| p.visual == Visual::Table).or(self.d.panels.first()).map(|p| p.id.clone()).unwrap_or_default();
                self.ed = None;
                self.screen = Screen::Zoom(id);
            }
            other => panic!("unknown screen {other}"),
        }
    }
    pub fn set_chat_for_test(&mut self, open: bool) {
        self.chat_open = open;
        self.focus = if open { Focus::Chat } else { Focus::Board };
    }
    pub fn panel_rects_for_test(&self) -> Vec<(usize, R)> {
        self.geo.panels.clone()
    }
    pub fn chat_rect_for_test(&self) -> Option<R> {
        self.geo.chat
    }
    pub fn hidden_for_test(&self, pid: &str) -> Vec<String> {
        self.data.get(pid).map(|d| d.hidden.clone()).unwrap_or_default()
    }
    pub fn preview_ready_for_test(&self) -> bool {
        self.ed.as_ref().is_some_and(|e| e.pd.frame.is_some() && !e.pd.loading)
    }
    pub fn chat_focused_for_test(&self) -> bool {
        self.focus == Focus::Chat && self.chat_open
    }
    pub fn chat_input_for_test(&self) -> String {
        self.chat.input_text().to_string()
    }
    pub fn in_editor_for_test(&self) -> bool {
        self.screen == Screen::Editor && self.ed.is_some()
    }
    pub fn editor_title_for_test(&self) -> String {
        self.ed.as_ref().map(|e| e.title.value.clone()).unwrap_or_default()
    }
    pub fn chat_open_for_test(&self) -> bool {
        self.chat_open
    }
    pub fn flashing_for_test(&self, id: &str) -> bool {
        self.flash.as_ref().is_some_and(|f| f.0 == id)
    }
    pub fn modal_open_for_test(&self) -> bool {
        self.modal.is_some()
    }
    pub fn list_len_for_test(&self) -> usize {
        self.list.len()
    }
    pub fn select_list_for_test(&mut self, i: usize) {
        self.list_sel = i;
    }
    pub fn status_for_test(&self) -> String {
        self.status.as_ref().map(|s| s.0.clone()).unwrap_or_default()
    }
    pub fn on_screen_for_test(&self, s: &str) -> bool {
        matches!((&self.screen, s), (Screen::Board, "board") | (Screen::List, "list") | (Screen::Sources, "sources") | (Screen::Editor, "editor") | (Screen::Zoom(_), "zoom"))
    }
    pub fn source_form_labels_for_test(&self) -> Vec<String> {
        match &self.modal {
            Some(Modal::Source { form, .. }) => form.fields.iter().map(|f| f.label.clone()).collect(),
            _ => vec![],
        }
    }
    pub fn source_form_choices_for_test(&self) -> Vec<Vec<String>> {
        match &self.modal {
            Some(Modal::Source { form, .. }) => form.fields.iter().filter_map(|f| if let super::widgets::FieldKind::Choice(o) = &f.kind { Some(o.clone()) } else { None }).collect(),
            _ => vec![],
        }
    }
    pub fn fill_source_form_for_test(&mut self, vals: &[(&str, &str)]) {
        if let Some(Modal::Source { form, .. }) = self.modal.as_mut() {
            for (l, v) in vals {
                if let Some(i) = form.fields.iter().position(|f| f.label.starts_with(l)) {
                    form.set_value(i, v);
                }
            }
        }
    }
    pub fn select_source_for_test(&mut self, id: &str) {
        self.src_sel = self.sources.iter().position(|s| s.id == id).unwrap_or(0);
    }
    pub fn source_status_for_test(&self, id: &str) -> Option<String> {
        self.src_status.get(id).map(|s| s.1.clone())
    }
    pub fn cancel_token_for_test(&self) -> Cancel {
        self.cancel.clone()
    }
    pub fn reload_for_test(&mut self) {
        self.reload_lists();
    }
    pub fn qerrors_for_test(&self, pid: &str) -> Vec<String> {
        self.data.get(pid).map(|d| d.qerrors.clone()).unwrap_or_default()
    }
    pub fn has_frame_for_test(&self, pid: &str) -> bool {
        self.data.get(pid).is_some_and(|d| d.frame.as_ref().is_some_and(|f| !f.is_empty()))
    }
    pub fn set_editor_view_for_test(&mut self, page: usize, expr: bool, cur: usize) {
        if let Some(ed) = self.ed.as_mut() {
            ed.page = page;
            ed.show_expr = expr;
            ed.cur = cur.min(ed.panel.queries.len());
        }
    }
    pub fn pick_source_choice_for_test(&mut self, label: &str, option: &str) {
        if let Some(Modal::Source { form, secret_keys, .. }) = self.modal.as_mut() {
            if let Some(i) = form.fields.iter().position(|f| f.label.starts_with(label)) {
                if let super::widgets::FieldKind::Choice(o) = &form.fields[i].kind {
                    let n = o.iter().position(|x| x == option).expect("an option of that name");
                    form.set_choice(i, n);
                }
            }
            let _ = secret_keys;
        }
        // the form is regenerated for the sign-in chosen, as a click would do
        if let Some(Modal::Source { form, kind, editing, advanced, secret_keys }) = self.modal.take() {
            let cfg = self.config_from_form(&form, &kind, editing.as_deref(), advanced, &secret_keys).ok();
            let title = form.title.clone();
            if let Some((mut f2, k2)) = self.source_form(&kind, cfg.as_ref(), advanced) {
                f2.title = title;
                self.modal = Some(Modal::Source { form: f2, kind, editing, advanced, secret_keys: k2 });
            }
        }
    }
    pub fn focus_source_field_for_test(&mut self, label: &str) {
        if let Some(Modal::Source { form, .. }) = self.modal.as_mut() {
            form.focus = form.fields.iter().position(|f| f.label.starts_with(label)).expect("a field of that name");
        }
    }
    pub fn source_form_kinds_for_test(&self) -> Vec<String> {
        match &self.modal {
            Some(Modal::Source { form, .. }) => form.fields.iter().map(|f| match f.kind {
                super::widgets::FieldKind::Secret => "secret",
                super::widgets::FieldKind::Choice(_) => "choice",
                super::widgets::FieldKind::Path(..) => "path",
                super::widgets::FieldKind::Text => "text",
            }.to_string()).collect(),
            _ => vec![],
        }
    }
    pub fn new_secret_open_for_test(&self) -> bool {
        matches!(self.modal, Some(Modal::NewSecret { .. }))
    }
    pub fn new_secret_error_for_test(&self) -> String {
        match &self.modal {
            Some(Modal::NewSecret { form, .. }) => form.error.clone().unwrap_or_default(),
            _ => String::new(),
        }
    }
    /// Does any text field of an open dialog still hold this text?
    pub fn any_form_value_contains_for_test(&self, text: &str) -> bool {
        let has = |f: &Form| f.fields.iter().any(|x| x.tf.value.contains(text));
        match &self.modal {
            Some(Modal::NewSecret { form, src, .. }) => has(form) || has(src),
            Some(Modal::Source { form, .. }) | Some(Modal::Name(form, _)) => has(form),
            _ => false,
        }
    }
}
