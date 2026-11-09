//! FLOWS: the workflow IDE. Design flows on a canvas, run them and watch the graph light up, see what is scheduled, running, blocked
//! or failed, and let the assistant (beside it) build, explain and fix. See `docs/FLOWS.md`.
//!
//! The tile is thin: the engine (`iyakki-flow`), the real host and the assistant's tools (`iyakki_agent::flows`) do the work; this
//! file holds the state, the keys and the mouse. The drawing is in `flows_view`, `flows_run` and `flows_board`, the geometry of the
//! canvas in `flows_canvas`, the painter in `flows_paint`, the properties form in `flows_props`. Nothing here blocks: the engine runs
//! on its own threads and this reads snapshots; a node test runs on a worker thread.

use crate::safeclamp::SafeClamp;
use std::collections::{BTreeMap, BTreeSet};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::{Arc, Mutex};

use iyakki_agent::flows::failfix::{FailBook, Failure};
use iyakki_agent::flows::{FlowEnv, Open, SharedOpen};
use iyakki_flow::engine::{ApplyScope, Counts, NodeTest, NodeUpdateInfo};
use iyakki_flow::kinds::{Issue, Level, Registry};
use iyakki_flow::model::*;
use iyakki_flow::store::FlowSummary;
use iyakki_flow::StartRequest;

use super::chat::{ChatApp, FlowsLink};
use super::component::route;
use super::flows_canvas::*;
use super::flows_props::Props;
use super::widgets::{Form, Picker};
use super::{App, AppCx, Flow as AppFlow, ScrollLog};
use crate::gfx::rgb;
use crate::ui::{Event, TextField, R};

pub const ACCENT: [u8; 4] = rgb(138, 128, 255);
/// The chat pane's click-id slot (the IDE uses 1 to 5, DASH 6).
pub const CHAT_SLOT: usize = 7;

/// Click ids (`Event::Tab(n)` unless said otherwise).
pub mod id {
    pub const TAB: usize = 100; // + 0 design, 1 run, 2 board
    pub const CHAT: usize = 110;
    pub const CHAT_HIDE: usize = 111;
    pub const CHAT_PANE: usize = 112;
    pub const PUBLISH: usize = 113;
    pub const RUN: usize = 114;
    pub const UNDO: usize = 115;
    pub const REDO: usize = 116;
    pub const ADD: usize = 117;
    pub const COPY: usize = 118;
    pub const PASTE: usize = 119;
    pub const DELETE: usize = 120;
    pub const ARRANGE: usize = 121;
    pub const FIT: usize = 122;
    pub const TEXT: usize = 123;
    pub const TEST: usize = 124;
    pub const EXPORT: usize = 125;
    pub const IMPORT: usize = 126;
    pub const HISTORY: usize = 127;
    pub const HOOKS: usize = 193;
    pub const FLOWS: usize = 128;
    pub const ZOOM_IN: usize = 129;
    pub const ZOOM_OUT: usize = 130;
    pub const NEW: usize = 131;
    pub const TEMPLATE: usize = 132;
    pub const OPEN: usize = 133;
    pub const DELETE_FLOW: usize = 134;
    pub const RUN_DRAFT: usize = 135;
    pub const PAUSE: usize = 136;
    pub const APPLY_SCHEDULED: usize = 140;
    pub const APPLY_RUNNING: usize = 141;
    pub const LEAVE_UPDATE: usize = 142;
    pub const OFFER: usize = 138;
    pub const FAIL_LOG: usize = 139;
    pub const CLOSE: usize = 145;
    pub const R_BACK: usize = 146;
    pub const R_AGAIN: usize = 147;
    pub const FAIL_FIX: usize = 148;
    pub const FAIL_DISMISS: usize = 149;
    pub const SUG_OPEN: usize = 185;
    pub const ASSIST_ROLLBACK: usize = 186;
    pub const ASSIST_PAUSE: usize = 187;
    // run view
    pub const R_CANCEL: usize = 150;
    pub const R_RETRY: usize = 151;
    pub const R_SKIP: usize = 152;
    pub const R_RERUN: usize = 153;
    pub const R_APPROVE: usize = 154;
    pub const R_REJECT: usize = 155;
    pub const R_TAB: usize = 156; // + 0 output, 1 input, 2 log, 3 attempts
    pub const CARD_APPROVE: usize = 160;
    pub const CARD_REJECT: usize = 161;
    pub const CARD_EDIT: usize = 162;
    pub const CARD_ASK: usize = 163;
    pub const R_NEWER: usize = 164;
    pub const R_OLDER: usize = 165;
    pub const R_ALLOW: usize = 166;
    pub const R_TAB_PICS: usize = 167;
    pub const CARD_SETTINGS: usize = 169;
    pub const CARD_MODELS: usize = 191;
    pub const CARD_REFRESH: usize = 192;
    pub const R_TAB_REPORT: usize = 168;
    pub const R_EXPORT_HTML: usize = 143;
    pub const R_EXPORT_JSON: usize = 144;
    pub const R_PIC: usize = 500; // + picture index
    // board
    pub const B_LANE: usize = 170; // + 0..5
    pub const B_FLOW: usize = 176;
    pub const B_RUN_AGAIN: usize = 177;
    pub const B_CANCEL_SEL: usize = 178;
    pub const B_CLEAR: usize = 179;
    pub const B_NEW: usize = 180;
    // props
    pub const P_APPLY: usize = 190;
    pub const P_CHOICE: usize = 20_000; // + field * 64 + option
    pub const P_CHOICE_STRIDE: usize = 64;
    pub const PUB_OK: usize = 190_000;
    pub const NODE_CARD: usize = 300_000; // + run index (board cards) as Event::Row
    pub const LIST_ROW: usize = 0;
    pub const PAL: usize = 400; // + palette index
}

/// Focus ids for `Event::Focus` (text fields).
pub mod fid {
    pub const SEARCH: usize = 1;
    pub const PROP: usize = 10; // + field index
    pub const BOARD_SEARCH: usize = 2;
}

/// Press areas (`Event::Area`).
pub mod area {
    pub use super::super::flows_paint::{GRID_AREA as CANVAS, MINIMAP_AREA as MINIMAP};
    pub const TEXT: usize = 71_010;
    pub const PROP_QB: usize = 71_100; // + field index
    pub const WATERFALL: usize = 71_020;
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Tab {
    Design,
    Run,
    Board,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Focus {
    Canvas,
    Props,
    Search,
    Text,
    BoardSearch,
    Chat,
}

#[derive(Clone, Debug)]
pub(super) enum Drag {
    Pan { last: (i32, i32) },
    Move { ids: Vec<String>, last: (i32, i32), before: Box<iyakki_flow::model::Flow>, moved: bool },
    Connect { from: PortRef, pos: (f32, f32) },
    Band { from: (f32, f32), to: (f32, f32) },
    Mini,
}

pub(super) enum Modal {
    Name(Form, NameFor),
    Templates(Picker, Vec<String>),
    AddNode(Picker, Vec<String>),
    Confirm(String, Confirmed),
    Publish(PubForm),
    History(HistView),
    /// The flow's webhooks: on or off, the last 20 deliveries, a new address shown once.
    Webhook(super::flows_hook::HookPanel),
    /// The suggested changes of the agent (Assist): list, stored diff, Approve, Reject.
    Suggestions(super::flows_assist::SugView),
    Import(Form),
    Result(String, ScrollLog),
    EditPatch(String, super::dash_query::QueryBox),
    /// The values a run needs (the flow's declared inputs): the form asked before a run starts; true: run the draft.
    RunInputs(Form, bool, Vec<Param>),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum NameFor {
    NewFlow,
    Rename,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum Confirmed {
    DeleteFlow(String),
    Rollback(u32),
    /// Drop one published version of the open flow (asked for in the history, confirmed with y only).
    DropVersion(u32),
    AllowProgram(String),
    /// Close the open flow although something typed was not accepted yet.
    CloseFlow,
}

pub(super) struct PubForm {
    pub label: TextField,
    pub changelog: TextField,
    pub focus: usize,
    pub summary: Vec<String>,
    pub blockers: Vec<String>,
    pub agent: bool,
}

pub(super) struct HistView {
    pub entries: Vec<iyakki_flow::store::VersionEntry>,
    pub sel: usize,
    pub diff: Vec<String>,
}

/// What the screen waits for from a worker thread.
pub(super) enum Msg {
    NodeTest(String, Result<NodeTest, String>),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum RunTab {
    Output,
    Input,
    Log,
    Attempts,
    /// The browser steps' screenshots and the live picture of the run's browser session.
    Pictures,
    /// The QA report: per step status, checks, console errors.
    Report,
}

pub(super) struct RunView {
    pub tab: RunTab,
    pub log: ScrollLog,
    /// What the log shows (the run, the node, the tab and the run's revision), so it is rebuilt only when that changes.
    pub shown: (String, String, u8, u64),
    pub view: View,
    pub fitted_for: String,
    /// Decoded screenshots by file (None: could not be read), at most a couple of dozen.
    pub pics: std::collections::HashMap<String, Option<Arc<tiny_skia::Pixmap>>>,
    /// The picture shown large in the PICTURES tab (index into the run's pictures).
    pub pic_sel: usize,
    /// The live picture of the browser session: when it was read (ms), its file, and the picture.
    pub preview: Option<(i64, String, Option<Arc<tiny_skia::Pixmap>>)>,
}

pub(super) struct Board {
    pub lane: usize,
    pub flow_filter: Option<String>,
    pub search: TextField,
    pub sel: BTreeSet<String>,
    pub cursor: usize,
    pub next: Vec<(String, String, i64)>,
    pub hist_scroll: usize,
}

#[derive(Clone)]
pub(super) struct Geo {
    pub area: Option<R>,
    pub chat: Option<R>,
    pub main: R,
    pub canvas: R,
    pub minimap: Option<R>,
    pub props: Option<R>,
    pub palette: Option<R>,
    pub rows: Vec<(usize, R)>,
    pub waterfall: Option<(R, i64, i64, Vec<(String, i64, i64)>)>,
}

impl Default for Geo {
    fn default() -> Geo {
        Geo { area: None, chat: None, main: R::new(0.0, 0.0, 0.0, 0.0), canvas: R::new(0.0, 0.0, 0.0, 0.0), minimap: None, props: None, palette: None, rows: vec![], waterfall: None }
    }
}

pub struct FlowsApp {
    pub(super) env: Arc<FlowEnv>,
    pub(super) open: SharedOpen,
    pub(super) reg: Arc<Registry>,
    pub(super) tab: Tab,
    pub(super) tick: u64,
    pub(super) redraw: bool,
    pub(super) status: Option<(String, bool)>,
    pub(super) flows: Vec<FlowSummary>,
    /// (id, name) of every flow, as `flows.list` of the core contract answers it (the first FLOWS read on the contract).
    pub(super) flow_names: Vec<(String, String)>,
    pub(super) list_sel: usize,
    pub(super) cur: Option<String>,
    pub(super) flow: iyakki_flow::model::Flow,
    pub(super) stamp: (u64, u128),
    pub(super) undo: Vec<iyakki_flow::model::Flow>,
    pub(super) redo: Vec<iyakki_flow::model::Flow>,
    pub(super) issues: Vec<Issue>,
    /// The tooltip card of a node (pointer rest, selection, long press).
    pub(super) tip: crate::tooltip::Tooltip,
    /// Where the tooltip card was drawn last frame (None: none).
    pub(super) tip_rect: Option<R>,
    pub(super) issue_map: BTreeMap<String, (usize, usize)>,
    pub(super) view: View,
    pub(super) sel: BTreeSet<String>,
    pub(super) sel_edge: Option<usize>,
    pub(super) hover: Hit,
    pub(super) drag: Option<Drag>,
    pub(super) mouse: (i32, i32),
    pub(super) clip: Clip,
    pub(super) props: Props,
    pub(super) palette: TextField,
    pub(super) text: Option<super::dash_query::QueryBox>,
    pub(super) text_err: String,
    pub(super) focus: Focus,
    pub(super) modal: Option<Modal>,
    pub(super) runs: Vec<RunSummary>,
    pub(super) counts: Counts,
    pub(super) run_rev: u64,
    pub(super) run_id: Option<String>,
    pub(super) run: Option<Run>,
    pub(super) run_node: Option<String>,
    pub(super) rv: RunView,
    pub(super) updates: Vec<NodeUpdateInfo>,
    pub(super) updates_at: u64,
    pub(super) board: Board,
    pub(super) chat: ChatApp,
    pub(super) chat_open: bool,
    pub(super) tx: Sender<Msg>,
    pub(super) rx: Receiver<Msg>,
    pub(super) host_save: Option<(String, Vec<u8>)>,
    pub(super) geo: Geo,
    pub(super) fitted: bool,
    pub(super) testing: Option<String>,
    pub(super) palette_ids: Vec<String>,
    /// How far the palette list is scrolled (pixels).
    pub(super) pal_scroll: f32,
    pub(super) cur_changed: bool,
    /// What the person dismissed, what the assistant already said, the attempts spent (a small file).
    pub(super) book: FailBook,
    /// The failed last run of the open flow that the card talks about.
    pub(super) fail: Option<Failure>,
    /// The tab a run was opened from (Esc goes back to it).
    pub(super) ret: Tab,
    /// The assistant's offer to post in the thread: (run, text).
    pub(super) pending_note: Option<(String, String)>,
    /// "Ask the agent to fix" was pressed (it needs the session, so the event handler starts it).
    pub(super) fix_requested: bool,
    /// Another app the person asked for ("Open Settings -> Models"): (app, item), taken by the shell.
    pub(super) jump: Option<(String, String)>,
    /// What Assist shows for the open flow: waiting suggestions, the watched version, the pause.
    pub(super) assist: super::flows_assist::AssistUi,
}

pub(super) fn stamp_of(env: &FlowEnv, id: &str) -> (u64, u128) {
    let p = env.dir.join(id).join("flow.json");
    std::fs::metadata(p).map(|m| (m.len(), m.modified().ok().and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok()).map_or(0, |d| d.as_nanos()))).unwrap_or((0, 0))
}

impl FlowsApp {
    pub fn new(cx: &AppCx) -> FlowsApp {
        let env = FlowEnv::open(&cx.services, &cx.session.user, &cx.session.policy).unwrap_or_else(|_| Arc::new(FlowEnv::build(&cx.services, &cx.session.user, &cx.session.policy, false)));
        // what the engine does goes to the audit log by name and place, never by value
        let (audit, user) = (cx.audit.clone(), cx.session.user.clone());
        *env.host.audit_sink.lock().unwrap_or_else(|e| e.into_inner()) = Some(Arc::new(move |_u: &str, kind: &str, detail: &str| audit.record(&user, kind, detail)));
        FlowsApp::with_env(cx, env)
    }

    /// Tests give the tile an environment of their own.
    pub fn with_env(cx: &AppCx, env: Arc<FlowEnv>) -> FlowsApp {
        // script steps run in the in-house JavaScript engine: no network, no files, a time and memory limit
        *env.host.js.lock().unwrap_or_else(|e| e.into_inner()) = Some(js_runner());
        let open: SharedOpen = Arc::new(Mutex::new(Open { tab: "design".into(), ..Default::default() }));
        let chat = ChatApp::new().with_agent(cx.session.policy.agent).with_flows(FlowsLink { env: env.clone(), open: open.clone() }).resumed(cx);
        let (tx, rx) = channel();
        let env_data = env.data.clone();
        let mut a = FlowsApp {
            reg: env.reg.clone(),
            env,
            open,
            tab: Tab::Design,
            tick: 0,
            redraw: true,
            status: None,
            flows: vec![],
            flow_names: vec![],
            list_sel: 0,
            cur: None,
            flow: iyakki_flow::model::Flow::new("", ""),
            stamp: (0, 0),
            undo: vec![],
            redo: vec![],
            issues: vec![],
            issue_map: BTreeMap::new(),
            view: View::default(),
            sel: BTreeSet::new(),
            sel_edge: None,
            hover: Hit::Empty,
            drag: None,
            mouse: (0, 0),
            clip: Clip::default(),
            props: Props::default(),
            palette: TextField::new("search steps", false),
            text: None,
            text_err: String::new(),
            focus: Focus::Canvas,
            modal: None,
            runs: vec![],
            counts: Counts::default(),
            run_rev: 0,
            run_id: None,
            run: None,
            run_node: None,
            rv: RunView { tab: RunTab::Output, log: ScrollLog::default(), shown: Default::default(), view: View::default(), fitted_for: String::new(), pics: Default::default(), pic_sel: 0, preview: None },
            updates: vec![],
            updates_at: 0,
            board: Board { lane: 0, flow_filter: None, search: TextField::new("search runs", false), sel: BTreeSet::new(), cursor: 0, next: vec![], hist_scroll: 0 },
            chat,
            chat_open: false,
            tx,
            rx,
            host_save: None,
            geo: Geo::default(),
            fitted: false,
            testing: None,
            palette_ids: vec![],
            pal_scroll: 0.0,
            cur_changed: false,
            book: FailBook::open(&env_data),
            fail: None,
            ret: Tab::Design,
            pending_note: None,
            fix_requested: false,
            jump: None,
            tip_rect: None,
            tip: crate::tooltip::Tooltip::new().with_pointer_delay(super::flows_tip::NODE_TIP_DELAY),
            assist: Default::default(),
        };
        a.reload_flows();
        a.refresh_runs();
        a
    }

    pub fn state(&self) -> SharedOpen {
        self.open.clone()
    }

    /// The program a failed step was refused for, when an administrator could allow it (the error names it between backticks).
    pub(super) fn refused_program(&self) -> Option<String> {
        if !self.env.policy.manage_settings {
            return None;
        }
        let e = &self.run.as_ref()?.nodes.get(self.run_node.as_ref()?)?.error;
        if !e.contains(iyakki_agent::flows::programs::NOT_ALLOWED) {
            return None;
        }
        let a = e.find('`')? + 1;
        let b = a + e[a..].find('`')?;
        Some(e[a..b].to_string()).filter(|p| !p.is_empty())
    }

    pub(super) fn say(&mut self, t: impl Into<String>, ok: bool) {
        self.status = Some((t.into(), ok));
        self.redraw = true;
    }

    pub(super) fn now(&self) -> i64 {
        iyakki_agent::clock::now_unix() * 1000
    }

    pub(super) fn reload_flows(&mut self) {
        self.flows = self.env.summaries();
        self.flow_names = self.contract_flow_names();
        self.list_sel = self.list_sel.min(self.flows.len().saturating_sub(1));
    }

    /// The flows' ids and names through the core contract (`flows.list`); empty when the call is refused.
    fn contract_flow_names(&self) -> Vec<(String, String)> {
        use iyakki_agent::core_contract::{CallContext, Flows as _};
        let ctx = CallContext::local(&self.env.user, self.env.policy.manage_settings);
        iyakki_agent::contract_local::core_for(&self.env.svc).flows_for_env(self.env.clone()).list(&ctx).map(|l| l.into_iter().map(|i| (i.id, i.name)).collect()).unwrap_or_default()
    }

    pub(super) fn sync_open(&mut self) {
        // the CHAT tile keeps working on the flow last open here (id and name only; cleared at sign-out, on delete or by its chip)
        if let Some(id) = &self.cur {
            iyakki_agent::flows::set_last_flow(&self.env.svc, &self.env.user, id, &self.flow.name);
        }
        let mut o = self.open.lock().unwrap_or_else(|e| e.into_inner());
        o.flow = self.cur.clone();
        o.node = self.sel.iter().next().cloned();
        o.run = self.run_id.clone();
        o.tab = match self.tab {
            Tab::Design => "design",
            Tab::Run => "run",
            Tab::Board => "board",
        }
        .into();
    }

    // ------------------------------------------------------------------------------------------ the open flow

    pub(super) fn open_flow(&mut self, id: &str) {
        match self.env.store.get_draft(id) {
            Some(f) => {
                self.flow = f;
                self.cur = Some(id.to_string());
                self.stamp = stamp_of(&self.env, id);
                self.undo.clear();
                self.redo.clear();
                self.sel.clear();
                self.sel_edge = None;
                self.text = None;
                self.fitted = false;
                self.view = View::default();
                self.props = Props::default();
                self.revalidate();
                self.focus = Focus::Canvas;
                self.tab = Tab::Design;
                self.refresh_runs();
                self.sync_open();
            }
            None => self.say(format!("There is no flow `{id}`."), false),
        }
    }

    pub(super) fn close_flow(&mut self) {
        self.cur = None;
        self.fail = None;
        self.sel.clear();
        self.sel_edge = None;
        self.text = None;
        self.reload_flows();
        self.sync_open();
    }

    pub(super) fn revalidate(&mut self) {
        self.issues = if self.cur.is_some() { self.env.issues(&self.flow) } else { vec![] };
        let mut m: BTreeMap<String, (usize, usize)> = BTreeMap::new();
        for i in &self.issues {
            if let Some(n) = &i.node {
                let e = m.entry(n.clone()).or_default();
                if i.level == Level::Error {
                    e.0 += 1;
                } else {
                    e.1 += 1;
                }
            }
        }
        self.issue_map = m;
    }

    /// Remember the flow as it is, to come back to with Ctrl+Z.
    pub(super) fn checkpoint(&mut self) {
        self.undo.push(self.flow.clone());
        if self.undo.len() > 100 {
            self.undo.remove(0);
        }
        self.redo.clear();
    }

    /// Save the working copy as the draft (the person's own edit: no badge).
    pub(super) fn persist(&mut self, summary: &str) {
        let Some(id) = self.cur.clone() else { return };
        let node = self.sel.iter().next().cloned().unwrap_or_default();
        match self.env.save_edit(&self.flow, &self.env.who(), summary, &node) {
            Ok(()) => self.stamp = stamp_of(&self.env, &id),
            Err(e) => self.say(format!("Could not save: {e}"), false),
        }
        self.revalidate();
        self.redraw = true;
    }

    /// An edit: checkpoint, change, save.
    pub(super) fn edit(&mut self, summary: &str, f: impl FnOnce(&mut iyakki_flow::model::Flow)) {
        self.checkpoint();
        f(&mut self.flow);
        self.persist(summary);
    }

    pub(super) fn undo_step(&mut self) {
        if let Some(prev) = self.undo.pop() {
            self.redo.push(std::mem::replace(&mut self.flow, prev));
            self.sel.retain(|id| self.flow.node(id).is_some());
            self.persist("undo");
            self.props = Props::default();
        } else {
            self.say("Nothing to undo.", false);
        }
    }

    pub(super) fn redo_step(&mut self) {
        if let Some(next) = self.redo.pop() {
            self.undo.push(std::mem::replace(&mut self.flow, next));
            self.sel.retain(|id| self.flow.node(id).is_some());
            self.persist("redo");
            self.props = Props::default();
        } else {
            self.say("Nothing to redo.", false);
        }
    }

    // ------------------------------------------------------------------------------------------ snapshots from the engine

    pub(super) fn refresh_runs(&mut self) {
        self.runs = self.env.engine.summaries();
        self.counts = self.env.engine.counts();
        self.run_rev = self.env.engine.revision();
        if let Some(id) = self.run_id.clone() {
            self.run = self.env.engine.get_run(&id);
        }
        // a run that waits for you or failed: its step is the one to look at
        if self.run_node.is_none() {
            if let Some(r) = &self.run {
                self.run_node = r.flow.nodes.iter().find(|n| r.nodes.get(&n.id).is_some_and(|x| matches!(x.state, NodeState::Waiting | NodeState::Failed | NodeState::Interrupted))).map(|n| n.id.clone());
            }
        }
        if self.run_id.is_none() {
            // follow the newest run of the open flow (or the newest of all) until the person picks one
            let want = self.runs.iter().find(|r| self.cur.as_ref().is_none_or(|c| *c == r.flow_id)).map(|r| r.id.clone());
            if let Some(w) = want {
                self.select_run(&w, false);
            }
        }
        self.update_fail_card();
        self.update_assist();
    }

    pub(super) fn select_run(&mut self, id: &str, go: bool) {
        self.run_id = Some(id.to_string());
        self.run = self.env.engine.get_run(id);
        self.run_node = None;
        self.rv.shown = Default::default();
        self.rv.fitted_for.clear();
        if go {
            if self.tab != Tab::Run {
                self.ret = self.tab;
            }
            self.tab = Tab::Run;
        }
        self.sync_open();
        self.redraw = true;
    }

    pub(super) fn poll(&mut self) {
        while let Ok(m) = self.rx.try_recv() {
            match m {
                Msg::NodeTest(node, r) => {
                    self.testing = None;
                    let mut log = ScrollLog::default();
                    let title = format!("Test of `{node}`");
                    match r {
                        Ok(t) => {
                            log.push(if t.ok { "It worked." } else { "It failed." }, if t.ok { crate::ui::OK } else { crate::ui::ERR });
                            if !t.error.is_empty() {
                                log.push(t.error.clone(), crate::ui::ERR);
                            }
                            if t.ok {
                                log.push(format!("output port: {}", t.port), crate::ui::MUTED);
                                for l in serde_json::to_string_pretty(&t.output).unwrap_or_default().lines().take(120) {
                                    log.push(l.to_string(), crate::ui::TEXT);
                                }
                            }
                            for l in &t.logs {
                                log.push(format!("| {l}"), crate::ui::DIM);
                            }
                        }
                        Err(e) => log.push(e, crate::ui::ERR),
                    }
                    self.modal = Some(Modal::Result(title, log));
                    self.redraw = true;
                }
            }
        }
    }

    pub(super) fn pick_updates(&mut self) {
        self.updates = match &self.cur {
            Some(id) if self.env.store.published(id).is_some() => self.env.engine.node_updates(id),
            _ => vec![],
        };
        self.updates_at = self.tick;
    }

    // ------------------------------------------------------------------------------------------ commands

    pub(super) fn new_flow(&mut self, name: &str) {
        let name = name.trim();
        let spec = serde_json::json!({"name": name, "nodes": [{"id": "step1", "kind": "notify", "config": {"title": "Hello from FLOWS"}}]});
        match iyakki_agent::flows::build_flow(&spec).and_then(|f| self.env.create_draft(f, &self.env.who())) {
            Ok(f) => {
                self.reload_flows();
                self.open_flow(&f.id);
                self.say(format!("Made `{}`. It is a draft: publish it when it is ready.", f.id), true);
            }
            Err(e) => self.say(e, false),
        }
    }

    pub(super) fn from_template(&mut self, tid: &str) {
        match self.env.from_template(tid, "", &self.env.who()) {
            Ok(f) => {
                self.reload_flows();
                self.open_flow(&f.id);
                self.say(format!("Made `{}` from the template. Its schedule is off until you turn it on.", f.id), true);
            }
            Err(e) => self.say(e, false),
        }
    }

    /// Run the open flow. A flow with inputs it cannot do without asks for them first, in a form.
    pub(super) fn run_flow(&mut self, draft: bool) {
        if self.cur.is_none() {
            return;
        }
        if self.flow.inputs.iter().any(|p| p.required && p.default.is_none()) {
            let mut form = Form::new("Run this flow: its inputs");
            for p in &self.flow.inputs {
                let label = format!("{}{}", p.name, if p.required { " (required)" } else { "" });
                let hint = if p.description.is_empty() { format!("a {} value", if p.ty.is_empty() { "text" } else { &p.ty }) } else { p.description.clone() };
                if p.ty == "secret" {
                    form = form.secret(&label, &hint);
                } else {
                    let init = match &p.default {
                        Some(serde_json::Value::String(t)) => t.clone(),
                        Some(v) => v.to_string(),
                        None => String::new(),
                    };
                    form = form.text(&label, &init, &hint);
                }
            }
            self.modal = Some(Modal::RunInputs(form, draft, self.flow.inputs.clone()));
            return;
        }
        self.start_flow_run(draft, None);
    }

    pub(super) fn start_flow_run(&mut self, draft: bool, inputs: Option<serde_json::Value>) {
        let Some(id) = self.cur.clone() else { return };
        let mut req = StartRequest::new(&id).trigger("manual", &self.env.who());
        req.draft = draft || self.env.store.published(&id).is_none();
        if let Some(i) = inputs {
            req.inputs = i;
        }
        match self.env.engine.start_run(req) {
            Ok(o) => {
                self.refresh_runs();
                self.select_run(&o.run_id, true);
                self.say(format!("Started run {}{}.", o.run_id, if draft { " of the draft" } else { "" }), true);
            }
            Err(e) => self.say(e, false),
        }
    }

    pub(super) fn open_publish(&mut self) {
        let Some(id) = self.cur.clone() else { return };
        let blockers: Vec<String> = self.issues.iter().filter(|i| i.level == Level::Error).map(|i| format!("{}{}", i.node.as_ref().map(|n| format!("{n}: ")).unwrap_or_default(), i.msg)).collect();
        let summary = match self.env.store.published(&id) {
            Some(p) => iyakki_flow::store::diff(&p, &self.flow).text().lines().take(14).map(str::to_string).collect(),
            None => vec![format!("First version: {} steps, {} connections.", self.flow.nodes.len(), self.flow.edges.len())],
        };
        let agent = self.flow.meta.modified_by.iter().any(Change::by_agent);
        let mut changelog = TextField::new("what changed", false);
        changelog.max = 200;
        let mut label = TextField::new("label (optional, like 1.2.0)", false);
        label.max = 24;
        self.modal = Some(Modal::Publish(PubForm { label, changelog, focus: 1, summary, blockers, agent }));
    }

    pub(super) fn do_publish(&mut self, label: &str, changelog: &str) {
        let Some(id) = self.cur.clone() else { return };
        let who = self.env.who();
        match self.env.store.publish(&id, &iyakki_flow::store::PublishReq { by: &who, label, changelog, approved_by: Some(&who) }) {
            Ok(v) => {
                iyakki_flow::host::Host::audit(self.env.host.as_ref(), "flow.publish", &format!("{id} v{v} by {who}"));
                self.env.assist_after_publish(&id, v);
                if let Some(f) = self.env.store.get_draft(&id) {
                    self.flow = f;
                }
                self.stamp = stamp_of(&self.env, &id);
                self.reload_flows();
                self.pick_updates();
                self.revalidate();
                self.say(format!("Published version {v}."), true);
            }
            Err(e) => self.say(e, false),
        }
    }

    pub(super) fn open_history(&mut self) {
        let Some(id) = self.cur.clone() else { return };
        let entries = self.env.store.versions(&id);
        if entries.is_empty() {
            self.say("Nothing has been published yet.", false);
            return;
        }
        let sel = entries.len() - 1;
        let mut h = HistView { entries, sel, diff: vec![] };
        self.fill_diff(&mut h);
        self.modal = Some(Modal::History(h));
    }

    pub(super) fn fill_diff(&self, h: &mut HistView) {
        let Some(id) = &self.cur else { return };
        let v = h.entries[h.sel].version;
        let cur = self.env.store.get_version(id, v);
        let prev = if h.sel > 0 { self.env.store.get_version(id, h.entries[h.sel - 1].version) } else { None };
        h.diff = match (prev, cur) {
            (Some(p), Some(c)) => iyakki_flow::store::diff(&p, &c).text().lines().take(60).map(str::to_string).collect(),
            (None, Some(c)) => vec![format!("The first version: {} steps, {} connections.", c.nodes.len(), c.edges.len())],
            _ => vec!["(this version could not be read)".into()],
        };
        let e = &h.entries[h.sel];
        if !e.changes.is_empty() {
            h.diff.push(String::new());
            for c in &e.changes {
                h.diff.push(format!("{} changed {}: {}", c.who, if c.node.is_empty() { "the flow" } else { &c.node }, c.summary));
            }
        }
    }

    pub(super) fn test_selected(&mut self) {
        let Some(id) = self.sel.iter().next().cloned() else {
            self.say("Select a step to test.", false);
            return;
        };
        if self.testing.is_some() {
            self.say("A test is already running.", false);
            return;
        }
        let flow = self.flow.clone();
        let inputs: serde_json::Map<String, serde_json::Value> = flow.inputs.iter().filter_map(|p| p.default.clone().map(|d| (p.name.clone(), d))).collect();
        let (engine, tx, nid) = (self.env.engine.clone(), self.tx.clone(), id.clone());
        self.testing = Some(id.clone());
        self.say(format!("Testing `{id}`..."), true);
        std::thread::spawn(move || {
            let r = engine.test_node(&flow, &nid, serde_json::Value::Object(inputs), serde_json::json!({}));
            let _ = tx.send(Msg::NodeTest(nid, r));
        });
    }

    pub(super) fn export_flow(&mut self) {
        if let Some(id) = self.cur.clone() {
            match self.env.export_json(&id) {
                Ok(t) => {
                    self.host_save = Some((format!("{id}.flow.json"), t.into_bytes()));
                    self.say("Choose where to save the file.", true);
                }
                Err(e) => self.say(e, false),
            }
        }
    }

    pub(super) fn import_path(&mut self, path: &str) -> Result<String, String> {
        let roots = iyakki_agent::scope::FileRoots::new(&self.env.policy, &self.env.user, self.env.svc.dir());
        let home = iyakki_agent::scope::home_root(&self.env.user, self.env.svc.dir());
        let real = roots.resolve(&home, path)?;
        if !roots.read_ok(&self.env.policy, &self.env.user, &real) {
            return Err("that file is outside the folders you may read".into());
        }
        let meta = std::fs::metadata(&real).map_err(|e| e.to_string())?;
        if meta.len() > 2 << 20 {
            return Err("that file is over 2 MB".into());
        }
        let text = std::fs::read_to_string(&real).map_err(|e| e.to_string())?;
        let f = self.env.import_json(&text, &self.env.who())?;
        self.reload_flows();
        Ok(f.id)
    }

    pub(super) fn delete_flow(&mut self, id: &str) {
        // the environment refuses a flow with unfinished runs and writes the audit line (id and name only)
        match self.env.delete_flow(id) {
            Ok(_) => {
                if self.cur.as_deref() == Some(id) {
                    self.close_flow();
                }
                self.reload_flows();
                self.say(format!("Deleted `{id}` (its runs stay in the history)."), true);
            }
            Err(e) => self.say(e, false),
        }
    }

    pub(super) fn apply_update(&mut self, scope: ApplyScope) {
        let (Some(id), Some(u)) = (self.cur.clone(), self.updates.first().cloned()) else { return };
        match self.env.engine.apply_node_update(&id, &u.node, scope, &self.env.who(), false) {
            Ok(r) => {
                self.pick_updates();
                self.say(format!("Step {}: {} scheduled run(s) updated, {} will take it at the step's next boundary, {} already past it, {} left (incompatible).", u.node, r.scheduled_updated.len(), r.running_pending.len(), r.skipped_started.len(), r.skipped_incompatible.len()), true);
            }
            Err(e) => self.say(e, false),
        }
    }

    // ------------------------------------------------------------------------------------------ canvas editing

    pub(super) fn add_step(&mut self, kind: &str) {
        let rect = self.geo.canvas;
        let (cx, cy) = self.view.to_world(rect, (rect.x + rect.w / 2.0, rect.y + rect.h / 2.0));
        let after = self.sel.iter().next().cloned();
        let mut new_id = String::new();
        self.edit(&format!("added a {kind} step"), |f| {
            new_id = add_node(f, kind, (cx - NODE_W / 2.0, cy - NODE_H / 2.0));
        });
        if let Some(a) = after {
            // a new step goes after the selected one, to its right
            let reg = self.reg.clone();
            if let Some(pa) = self.flow.node(&a).map(|n| n.pos) {
                if let Some(n) = self.flow.node_mut(&new_id) {
                    n.pos = Pos { x: snap(pa.x + COL_STEP), y: snap(pa.y) };
                }
            }
            let (_, outs, _) = self.flow.node(&a).map(|n| node_ports(&reg, n)).unwrap_or_default();
            if let Some(port) = outs.first() {
                let _ = connect(&mut self.flow, &reg, &a, port, &new_id);
            }
            self.persist("connected the new step");
        }
        self.sel = [new_id.clone()].into_iter().collect();
        self.sel_edge = None;
        self.props = Props::default();
        self.say(format!("Added `{new_id}`."), true);
    }

    pub(super) fn delete_selection(&mut self) {
        if let Some(i) = self.sel_edge.take() {
            if i < self.flow.edges.len() {
                self.edit("removed a connection", |f| {
                    f.edges.remove(i);
                });
            }
            return;
        }
        if self.sel.is_empty() {
            return;
        }
        let ids: Vec<String> = self.sel.iter().cloned().collect();
        let n = ids.len();
        self.edit("removed steps", |f| delete_nodes(f, &ids));
        self.sel.clear();
        self.props = Props::default();
        self.say(format!("Removed {n} step{}.", if n == 1 { "" } else { "s" }), true);
    }

    pub(super) fn copy_selection(&mut self) {
        let ids: Vec<String> = self.sel.iter().cloned().collect();
        if ids.is_empty() {
            return;
        }
        self.clip = copy(&self.flow, &ids);
        let text = serde_json::to_string_pretty(&self.clip.nodes).unwrap_or_default();
        crate::ui::clipboard::copy(&text);
        self.say(format!("Copied {} step(s).", ids.len()), true);
    }

    pub(super) fn paste_clip(&mut self) {
        if self.clip.nodes.is_empty() {
            self.say("Nothing copied.", false);
            return;
        }
        let clip = self.clip.clone();
        let mut made = vec![];
        self.edit("pasted steps", |f| made = paste(f, &clip, (GRID * 2.0, GRID * 2.0)));
        self.sel = made.into_iter().collect();
        self.props = Props::default();
    }

    pub(super) fn arrange(&mut self) {
        self.edit("arranged the steps", iyakki_flow::layout::auto_layout);
        self.fitted = false;
    }

    pub(super) fn move_selection(&mut self, dx: f32, dy: f32) {
        if self.sel.is_empty() {
            self.view.pan_by(-dx * self.view.zoom, -dy * self.view.zoom);
            return;
        }
        let ids: Vec<String> = self.sel.iter().cloned().collect();
        self.edit("moved steps", |f| move_nodes(f, &ids, dx, dy, true));
    }

    pub(super) fn cycle_selection(&mut self, back: bool) {
        if self.flow.nodes.is_empty() {
            return;
        }
        let cur = self.sel.iter().next().and_then(|id| self.flow.nodes.iter().position(|n| n.id == *id));
        let n = self.flow.nodes.len();
        let next = match cur {
            None => 0,
            Some(i) => if back { (i + n - 1) % n } else { (i + 1) % n },
        };
        let id = self.flow.nodes[next].id.clone();
        self.select_node(&id);
        // keep it on screen
        let canvas = self.geo.canvas;
        if let Some(p) = self.flow.node(&id).map(|n| n.pos) {
            let s = self.view.to_screen(canvas, (p.x, p.y));
            if s.0 < canvas.x || s.1 < canvas.y || s.0 + NODE_W * self.view.zoom > canvas.x + canvas.w || s.1 + NODE_H * self.view.zoom > canvas.y + canvas.h {
                center_on(&mut self.view, canvas, (p.x + NODE_W / 2.0, p.y + NODE_H / 2.0));
            }
        }
    }

    pub(super) fn select_node(&mut self, id: &str) {
        self.sel = [id.to_string()].into_iter().collect();
        self.sel_edge = None;
        self.props = Props::default();
        self.sync_open();
    }

    pub(super) fn toggle_text(&mut self) {
        if self.text.take().is_some() {
            self.text_err.clear();
            self.focus = Focus::Canvas;
            return;
        }
        if self.cur.is_none() {
            return;
        }
        let mut qb = super::dash_query::QueryBox::new(area::TEXT);
        qb.set_text(&self.flow.to_json());
        self.text = Some(qb);
        self.text_err.clear();
        self.focus = Focus::Text;
    }

    /// The text view changed: when it is a flow, it becomes the flow (and is saved); when not, say why and keep the old one.
    pub(super) fn text_changed(&mut self) {
        let Some(qb) = &self.text else { return };
        match iyakki_flow::model::Flow::from_json(&qb.text()) {
            Ok(mut f) => {
                f.id = self.flow.id.clone();
                f.meta = self.flow.meta.clone();
                if f != self.flow {
                    self.checkpoint();
                    self.flow = f;
                    self.persist("edited the text");
                }
                self.text_err.clear();
            }
            Err(e) => self.text_err = e,
        }
    }

    pub(super) fn is_small(area: R, s: f32) -> bool {
        area.w < 900.0 * s.min(1.0).max(0.5) || area.h < 460.0
    }
}

/// How far to the right a step added after another goes.
pub(super) const COL_STEP: f32 = 240.0;

// ---------------------------------------------------------------------------------------------------------------- events

impl FlowsApp {
    fn chat_rect(&self) -> Option<R> {
        self.geo.chat
    }

    fn on_tick(&mut self, cx: &AppCx) -> AppFlow {
        self.tick += 1;
        self.poll();
        let _ = self.chat.handle(&Event::Tick, cx);
        self.deliver_note(cx);
        if self.chat_open && (self.chat.is_running() || self.chat.pending_approval().is_some()) && self.tick % 3 == 0 {
            self.redraw = true;
        }
        if self.tick % 6 == 0 {
            let rev = self.env.engine.revision();
            if rev != self.run_rev {
                self.refresh_runs();
                self.redraw = true;
            }
            let active = self.run.as_ref().is_some_and(|r| !r.status.is_terminal());
            if active && !crate::fx::reduced_motion() {
                self.redraw = true;
            }
        }
        if self.tick % 30 == 0 {
            self.update_assist();
        }
        // the assistant made a webhook address: it waits for this screen only (never in the chat), shown once in the panel
        if self.tick % 6 == 0 && self.modal.is_none() && self.cur.is_some() && iyakki_agent::flows::hookstore::reveal_waiting(&self.env.user) {
            self.open_hooks();
            self.redraw = true;
        }
        if self.tick % 15 == 0 {
            // the assistant (or another window) may have changed the draft
            if let Some(id) = self.cur.clone() {
                let st = stamp_of(&self.env, &id);
                if st != self.stamp && self.drag.is_none() && self.modal.is_none() && self.text.is_none() {
                    if let Some(f) = self.env.store.get_draft(&id) {
                        if f != self.flow {
                            self.undo.push(std::mem::replace(&mut self.flow, f));
                            self.redo.clear();
                            self.sel.retain(|n| self.flow.node(n).is_some());
                            self.props = Props::default();
                            self.say("The assistant changed this flow (Ctrl+Z puts it back).", true);
                        }
                        self.stamp = st;
                        self.revalidate();
                    }
                }
            }
            self.board.next = self.next_runs();
        }
        if self.tick - self.updates_at >= 60 {
            self.pick_updates();
            self.redraw = true;
        }
        AppFlow::Stay
    }

    pub(super) fn next_runs(&self) -> Vec<(String, String, i64)> {
        let mut v = Vec::new();
        for s in self.flows.iter().filter(|s| s.published > 0).take(40) {
            if let Some(f) = self.env.store.published(&s.id) {
                for (t, at) in self.env.engine.next_runs(&f, 3) {
                    v.push((s.id.clone(), t, at));
                }
            }
        }
        v.sort_by_key(|x| x.2);
        v.truncate(12);
        v
    }

    pub(super) fn handle_impl(&mut self, ev: &Event, cx: &AppCx) -> AppFlow {
        self.tip.event(ev);
        self.poll();
        match ev {
            Event::Tick => return self.on_tick(cx),
            Event::MouseMove(x, y) => {
                self.mouse = (*x, *y);
                self.on_move();
                return AppFlow::Stay;
            }
            Event::MouseUp(x, y) => {
                self.mouse = (*x, *y);
                self.on_release();
                return AppFlow::Stay;
            }
            _ => {}
        }
        self.redraw = true;
        if self.modal.is_some() {
            return self.modal_event(ev, cx);
        }
        if let Some(inner) = route::decode_slot(ev, CHAT_SLOT) {
            self.focus = Focus::Chat;
            let f = self.chat.handle(&inner, cx);
            if matches!(f, AppFlow::Back) {
                self.focus = Focus::Canvas;
            }
            return AppFlow::Stay;
        }
        let f = match ev {
            Event::Scroll(d) => {
                self.on_scroll(*d, cx);
                AppFlow::Stay
            }
            Event::Paste(t) => {
                match self.focus {
                    Focus::Chat => {
                        return self.to_chat(ev, cx);
                    }
                    Focus::Search => {
                        self.palette.paste(t);
                    }
                    Focus::BoardSearch => {
                        self.board.search.paste(t);
                    }
                    Focus::Text => {
                        if let Some(q) = self.text.as_mut() {
                            q.handle(ev);
                        }
                        self.text_changed();
                    }
                    Focus::Props => self.props_event(ev),
                    Focus::Canvas => {
                        // pasted steps (the copied JSON) land on the canvas
                        if let Ok(nodes) = serde_json::from_str::<Vec<Node>>(t) {
                            if !nodes.is_empty() && self.cur.is_some() {
                                self.clip = Clip { nodes, edges: vec![] };
                                self.paste_clip();
                            }
                        }
                    }
                }
                AppFlow::Stay
            }
            Event::Key(k) => self.key(k, ev, cx),
            Event::Tab(n) => self.tab_click(*n),
            Event::Row(n) => {
                self.row_click(*n);
                AppFlow::Stay
            }
            Event::FocusAt(n, x) => {
                self.focus_click(*n, *x);
                AppFlow::Stay
            }
            Event::AreaAt(n, x, y) => {
                self.area_click(*n, *x, *y);
                AppFlow::Stay
            }
            _ => AppFlow::Stay,
        };
        if std::mem::take(&mut self.fix_requested) {
            self.start_fix(cx);
        }
        self.sync_open();
        f
    }

    fn to_chat(&mut self, ev: &Event, cx: &AppCx) -> AppFlow {
        let f = self.chat.handle(ev, cx);
        if matches!(f, AppFlow::Back) {
            self.focus = Focus::Canvas;
        }
        AppFlow::Stay
    }

    fn on_scroll(&mut self, d: i32, cx: &AppCx) {
        let (mx, my) = self.mouse;
        if self.chat_rect().is_some_and(|r| r.contains(mx, my)) {
            let _ = self.to_chat(&Event::Scroll(d), cx);
            return;
        }
        match self.tab {
            Tab::Design => {
                if self.text.is_some() {
                    if let Some(q) = self.text.as_mut() {
                        q.handle(&Event::Scroll(d));
                    }
                } else if self.cur.is_some() && self.geo.canvas.contains(mx, my) {
                    self.view.zoom_at(self.geo.canvas, mx as f32, my as f32, 1.12f32.powi(d));
                } else if self.cur.is_none() {
                    self.list_sel = (self.list_sel as i32 - d).clamp_safe(0, self.flows.len().saturating_sub(1) as i32) as usize;
                } else if self.geo.props.is_some_and(|r| r.contains(mx, my)) {
                    self.props.scroll = (self.props.scroll - d as f32 * 40.0).max(0.0);
                } else if self.geo.palette.is_some_and(|r| r.contains(mx, my)) {
                    self.pal_scroll = (self.pal_scroll - d as f32 * 40.0).max(0.0);
                }
            }
            Tab::Run => {
                if self.geo.canvas.contains(mx, my) {
                    self.rv.view.zoom_at(self.geo.canvas, mx as f32, my as f32, 1.12f32.powi(d));
                } else {
                    self.rv.log.scroll_by(d * 3);
                }
            }
            Tab::Board => {
                self.board.hist_scroll = (self.board.hist_scroll as i32 - d).max(0) as usize;
            }
        }
    }

    fn on_move(&mut self) {
        let (mx, my) = (self.mouse.0 as f32, self.mouse.1 as f32);
        let canvas = self.geo.canvas;
        let tab = self.tab;
        let hover_view = if tab == Tab::Run { self.rv.view } else { self.view };
        if self.drag.is_none() && matches!(tab, Tab::Design | Tab::Run) && canvas.contains(self.mouse.0, self.mouse.1) && self.text.is_none() {
            let h = hit_test(&self.flow_for_view(), &self.reg, &hover_view, canvas, mx, my);
            if h != self.hover {
                self.hover = h;
                self.redraw = true;
            }
        }
        let Some(drag) = self.drag.take() else { return };
        self.redraw = true;
        let z = if tab == Tab::Run { self.rv.view.zoom } else { self.view.zoom };
        self.drag = Some(match drag {
            Drag::Pan { last } => {
                let (dx, dy) = ((self.mouse.0 - last.0) as f32, (self.mouse.1 - last.1) as f32);
                if tab == Tab::Run {
                    self.rv.view.pan_by(dx, dy);
                } else {
                    self.view.pan_by(dx, dy);
                }
                Drag::Pan { last: self.mouse }
            }
            Drag::Move { ids, last, before, mut moved } => {
                let (dx, dy) = ((self.mouse.0 - last.0) as f32 / z, (self.mouse.1 - last.1) as f32 / z);
                if dx != 0.0 || dy != 0.0 {
                    moved = true;
                    move_nodes(&mut self.flow, &ids, dx, dy, false);
                }
                Drag::Move { ids, last: self.mouse, before, moved }
            }
            Drag::Connect { from, .. } => {
                self.hover = hit_test(&self.flow, &self.reg, &self.view, canvas, mx, my);
                Drag::Connect { from, pos: (mx, my) }
            }
            Drag::Band { from, .. } => Drag::Band { from, to: (mx, my) },
            Drag::Mini => {
                self.minimap_press(mx, my);
                Drag::Mini
            }
        });
    }

    fn flow_for_view(&self) -> iyakki_flow::model::Flow {
        if self.tab == Tab::Run {
            self.run.as_ref().map(|r| r.flow.clone()).unwrap_or_else(|| self.flow.clone())
        } else {
            self.flow.clone()
        }
    }

    fn on_release(&mut self) {
        let Some(drag) = self.drag.take() else { return };
        self.redraw = true;
        let (mx, my) = (self.mouse.0 as f32, self.mouse.1 as f32);
        match drag {
            Drag::Move { ids, before, moved, .. } => {
                if moved {
                    // drop on the grid; one undo step for the whole drag
                    for n in self.flow.nodes.iter_mut().filter(|n| ids.contains(&n.id)) {
                        n.pos.x = snap(n.pos.x);
                        n.pos.y = snap(n.pos.y);
                    }
                    self.undo.push(*before);
                    self.redo.clear();
                    self.persist("moved steps");
                }
            }
            Drag::Connect { from, .. } => {
                let h = hit_test(&self.flow, &self.reg, &self.view, self.geo.canvas, mx, my);
                let target = match h {
                    Hit::Port(p) if p.side == Side::In => Some(p.node),
                    Hit::Node(n) => Some(n),
                    _ => None,
                };
                if let Some(t) = target {
                    let reg = self.reg.clone();
                    match can_connect(&self.flow, &reg, &from.node, &from.name, &t) {
                        Ok(()) => {
                            let (a, p) = (from.node.clone(), from.name.clone());
                            self.edit("joined two steps", |f| {
                                let _ = connect(f, &reg, &a, &p, &t);
                            });
                            self.say(format!("Joined `{a}` to `{t}`."), true);
                        }
                        Err(e) => self.say(e, false),
                    }
                }
            }
            Drag::Band { from, to } => {
                let band = R::new(from.0.min(to.0), from.1.min(to.1), (from.0 - to.0).abs(), (from.1 - to.1).abs());
                let ids = nodes_in(&self.flow, &self.view, self.geo.canvas, band);
                if crate::ui::shift_held() {
                    self.sel.extend(ids);
                } else {
                    self.sel = ids.into_iter().collect();
                }
                self.props = Props::default();
            }
            Drag::Pan { .. } | Drag::Mini => {}
        }
    }

    fn minimap_press(&mut self, px: f32, py: f32) {
        let Some(mini) = self.geo.minimap else { return };
        let (view, flow) = if self.tab == Tab::Run { (self.rv.view, self.run.as_ref().map(|r| r.flow.clone()).unwrap_or_default()) } else { (self.view, self.flow.clone()) };
        let m = minimap(&flow, &view, self.geo.canvas, mini);
        let w = minimap_to_world(&m, px, py);
        if self.tab == Tab::Run {
            center_on(&mut self.rv.view, self.geo.canvas, w);
        } else {
            center_on(&mut self.view, self.geo.canvas, w);
        }
    }

    fn area_click(&mut self, n: usize, x: f32, y: f32) {
        if self.drag.is_some() && n != area::MINIMAP {
            // the shell repeats a press while the button is held; the drag is handled by the moves
            return;
        }
        match n {
            area::CANVAS => {
                let canvas = self.geo.canvas;
                let (px, py) = (canvas.x + x, canvas.y + y);
                self.focus = Focus::Canvas;
                self.commit_props();
                if self.tab == Tab::Run {
                    let flow = self.flow_for_view();
                    match hit_test(&flow, &self.reg, &self.rv.view, canvas, px, py) {
                        Hit::Node(id) | Hit::Port(PortRef { node: id, .. }) => {
                            self.run_node = Some(id);
                            self.rv.shown = Default::default();
                        }
                        Hit::Edge(_) => {}
                        Hit::Empty => self.drag = Some(Drag::Pan { last: self.mouse }),
                    }
                    return;
                }
                if self.cur.is_none() {
                    return;
                }
                let hit = hit_test(&self.flow, &self.reg, &self.view, canvas, px, py);
                let shift = crate::ui::shift_held();
                match hit {
                    Hit::Port(p) if p.side == Side::Out => {
                        self.drag = Some(Drag::Connect { from: p, pos: (px, py) });
                    }
                    Hit::Port(p) => {
                        self.select_node(&p.node);
                    }
                    Hit::Node(id) => {
                        if shift {
                            if !self.sel.remove(&id) {
                                self.sel.insert(id.clone());
                            }
                        } else if !self.sel.contains(&id) {
                            self.select_node(&id);
                        }
                        self.sel_edge = None;
                        self.props = Props::default();
                        let ids: Vec<String> = self.sel.iter().cloned().collect();
                        if !ids.is_empty() {
                            self.drag = Some(Drag::Move { ids, last: self.mouse, before: Box::new(self.flow.clone()), moved: false });
                        }
                    }
                    Hit::Edge(i) => {
                        self.sel.clear();
                        self.sel_edge = Some(i);
                        self.props = Props::default();
                    }
                    Hit::Empty => {
                        if shift {
                            self.drag = Some(Drag::Band { from: (px, py), to: (px, py) });
                        } else {
                            self.sel.clear();
                            self.sel_edge = None;
                            self.props = Props::default();
                            self.drag = Some(Drag::Pan { last: self.mouse });
                        }
                    }
                }
            }
            area::MINIMAP => {
                if let Some(m) = self.geo.minimap {
                    self.minimap_press(m.x + x, m.y + y);
                    self.drag = Some(Drag::Mini);
                }
            }
            area::TEXT => {
                self.focus = Focus::Text;
                if let Some(q) = self.text.as_mut() {
                    q.click(x, y, crate::ui::shift_held());
                }
            }
            n if (area::PROP_QB..area::PROP_QB + 64).contains(&n) => self.props_area(n - area::PROP_QB, x, y),
            area::WATERFALL => {
                if let Some((r, t0, t1, rows)) = &self.geo.waterfall {
                    // a press in the waterfall picks the step in that row
                    let rowh = ((r.h - 4.0) / rows.len().max(1) as f32).max(1.0);
                    let i = (y / rowh) as usize;
                    if let Some((id, _, _)) = rows.get(i) {
                        let _ = (t0, t1);
                        self.run_node = Some(id.clone());
                        self.rv.shown = Default::default();
                    }
                }
            }
            _ => {}
        }
    }

    fn focus_click(&mut self, n: usize, x: f32) {
        match n {
            fid::SEARCH => {
                self.focus = Focus::Search;
                self.palette.click(x);
            }
            fid::BOARD_SEARCH => {
                self.focus = Focus::BoardSearch;
                self.board.search.click(x);
            }
            n if n >= fid::PROP => {
                self.props_focus(n - fid::PROP, x);
                self.focus = Focus::Props;
            }
            _ => {}
        }
    }

    fn row_click(&mut self, n: usize) {
        match self.tab {
            Tab::Design if self.cur.is_none() => {
                self.list_sel = n.min(self.flows.len().saturating_sub(1));
                if let Some(s) = self.flows.get(self.list_sel).cloned() {
                    self.open_flow(&s.id);
                }
            }
            Tab::Design => {
                // the list editor (small windows): a row is a step
                if let Some(id) = self.flow.nodes.get(n).map(|x| x.id.clone()) {
                    self.select_node(&id);
                }
            }
            Tab::Run => {
                if let Some(r) = self.runs.iter().filter(|r| self.cur.as_ref().is_none_or(|c| *c == r.flow_id)).nth(n).map(|r| r.id.clone()) {
                    self.select_run(&r, false);
                }
            }
            Tab::Board => {
                let rows = self.board_rows();
                if let Some(id) = rows.get(n).cloned() {
                    if crate::ui::shift_held() {
                        if !self.board.sel.remove(&id) {
                            self.board.sel.insert(id);
                        }
                    } else {
                        self.select_run(&id, true);
                    }
                }
            }
        }
    }
}


/// Runs a script step: `args` is the input, `return` the result. The engine has no web modules, so a script can reach nothing.
pub fn js_runner() -> iyakki_agent::flows::host::JsFn {
    Arc::new(|source: &str, args: &serde_json::Value, wall_ms: u64| {
        let mut limits = iyakki_js::Limits::default();
        limits.wall = std::time::Duration::from_millis(wall_ms.clamp(100, 60_000));
        let mut e = iyakki_js::JsEngine::new(limits, vec![]).map_err(|e| e.to_string())?;
        let code = format!("JSON.stringify((function(args){{ {source}\n}})({}))", serde_json::to_string(args).unwrap_or_else(|_| "null".into()));
        let out = e.eval("flow-script", &code).map_err(|e| e.to_string())?;
        Ok(serde_json::from_str(&out).unwrap_or(serde_json::Value::String(out)))
    })
}
