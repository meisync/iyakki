//! DATA: look at a table file (CSV, TSV, JSON lines) and ask questions of it. The table is virtual-scrolled (only the rows
//! in view are fetched from the data service, so a file of a million rows opens at once), the header stays in view and
//! sorts on a click, a filter box takes expressions like `price > 10 and contains(name, 'tea')`, a summary pane gives the
//! statistics of every column, cells, rows and selections copy to the clipboard, and the view exports as CSV.
//!
//! [`DataView`] is the component: self-contained (`draw(rect)` and `handle(event)`), so any app or tile can embed it with
//! [`DataViewParams`] (a file and a first filter). [`DataApp`] is the tile that hosts one and adds the app chrome.
//! Everything slow (opening, paging, sorting, statistics, exporting) runs on a worker thread; the screen never waits.

use crate::safeclamp::SafeClamp;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::Arc;

use iyakki_data::{table, DType, DataError, DataService, DatasetInfo, Dir, ExportInfo, ExportRequest, Op, OpenRequest, Page, PageRequest, Pipeline, SortKey, Summary};
use serde_json::Value;

use super::widgets::{Form, FormEvent, PickEvent, Picker};
use super::{App, AppCx, Flow};
use crate::ctxmenu::Item;
use crate::gfx::{rgb, Face, Gfx, Icon};
use crate::ui::{Event, Key, Nav, TextField, DIM, ERR, MUTED, R, TEXT};

pub const ACCENT: [u8; 4] = rgb(238, 226, 112);

const TABLE_AREA: usize = 62_300;
const SCROLL_AREA: usize = 62_301;
const FILTER_FIELD: usize = 62_310;
const GRIP_BASE: usize = 62_400;
/// The most rows copied at once (a selection of the whole table is cut here, and the status line says so).
pub const MAX_COPY_ROWS: u64 = 10_000;
/// How many recently opened files the open dialog lists.
const RECENT_KEPT: usize = 8;
const MIN_COL: f32 = 56.0;
const MAX_COL: f32 = 520.0;

/// What a view may touch: the service, and the account's file scope.
#[derive(Clone)]
pub struct DataEnv {
    pub svc: Arc<dyn DataService>,
    pub can_read: Arc<dyn Fn(&str) -> bool + Send + Sync>,
    pub can_write: Arc<dyn Fn(&str) -> bool + Send + Sync>,
    /// Where the open dialog starts (`roots.home`) and the only folders it, a typed path and the assistant may open files
    /// from: the home folder, the account's workspaces and any narrow folder it may read.
    pub roots: Arc<dyn Fn() -> iyakki_agent::scope::FileRoots + Send + Sync>,
    /// Where the recently opened files are kept (none: not remembered).
    pub recent: Option<PathBuf>,
    /// Host files (window mode): the dialog's account side. None: no such buttons.
    pub host: Option<super::hostfiles::HostEnv>,
}

impl DataEnv {
    /// The folders in force right now (read again each time, so a host folder that stopped being shared is gone at once).
    pub fn roots(&self) -> iyakki_agent::scope::FileRoots {
        (self.roots)()
    }

    /// The folder the open dialog starts in and stays inside.
    pub fn picker_root(&self) -> PathBuf {
        self.roots().home
    }
}

impl DataEnv {
    pub fn from_cx(cx: &AppCx) -> DataEnv {
        let (p, u, d) = (cx.session.policy.clone(), cx.session.user.clone(), cx.services.dir().to_path_buf());
        let (p2, u2, d2, p3, u3, d3) = (p.clone(), u.clone(), d.clone(), p.clone(), u.clone(), d.clone());
        DataEnv {
            svc: iyakki_data::service(),
            // the account's scopes plus any shared host folder, as they are at this moment
            can_read: Arc::new(move |path| iyakki_agent::scope::FileRoots::new(&p, &u, &d).read_ok(&p, &u, Path::new(path))),
            can_write: Arc::new(move |path| iyakki_agent::scope::FileRoots::new(&p2, &u2, &d2).write_ok(&p2, &u2, Path::new(path))),
            roots: Arc::new(move || iyakki_agent::scope::FileRoots::new(&p3, &u3, &d3)),
            recent: Some(cx.services.dir().join(format!("data-recent-{}.json", cx.session.user))),
            host: Some(super::hostfiles::HostEnv::for_cx(cx)),
        }
    }
}

/// What to show first.
#[derive(Clone, Debug, Default)]
pub struct DataViewParams {
    pub path: Option<String>,
    pub filter: Option<String>,
}

/// What a view asks of its host after an event.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Out {
    Stay,
    /// Esc with nothing to dismiss: the host should close the view.
    Close,
}

enum Done {
    Opened(Result<DatasetInfo, DataError>),
    Page { ver: u64, start: u64, res: Result<Page, DataError> },
    Summary { ver: u64, res: Result<Summary, DataError> },
    Exported(Result<ExportInfo, DataError>),
}

struct Window {
    start: u64,
    rows: Vec<Vec<Value>>,
}

enum Modal {
    Host(Box<super::hostfiles::HostDialog>),
    Open { picker: Picker, dir: PathBuf },
    Path(Form, PathFor),
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum PathFor {
    Open,
    Export,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Focus {
    Table,
    Filter,
}

/// Where things were drawn last (clicks are mapped back through it).
#[derive(Clone, Default)]
struct Geo {
    body: Option<R>,
    row_h: f32,
    rows: u64,
    /// Left edge (relative to the body) and width of each column, before the horizontal scroll.
    cols: Vec<(f32, f32)>,
    scale: f32,
}

pub struct DataView {
    env: DataEnv,
    tx: Sender<Done>,
    rx: Receiver<Done>,
    jobs: usize,
    info: Option<DatasetInfo>,
    path: String,
    filter: TextField,
    /// The filter the pipeline uses now, and the last one the engine accepted.
    filter_applied: String,
    filter_good: String,
    filter_err: Option<String>,
    sort: Vec<SortKey>,
    ver: u64,
    total: u64,
    window: Option<Window>,
    asked: Option<(u64, u64)>,
    top: u64,
    hx: f32,
    widths: Vec<f32>,
    cursor: (u64, usize),
    anchor: Option<(u64, usize)>,
    focus: Focus,
    summary: Option<(u64, Summary)>,
    show_summary: bool,
    summary_asked: Option<u64>,
    modal: Option<Modal>,
    status: Option<(String, bool)>,
    geo: Geo,
    dirty: bool,
    pending_filter: Option<String>,
    show_hidden: bool,
    /// After this file has been written, offer it to the host's save dialog.
    host_export_after: Option<PathBuf>,
    ticks: u64,
    /// Size and change time of the file when it was opened, to notice that it was changed elsewhere (the assistant appended).
    stamp: Option<(u64, std::time::SystemTime)>,
}

fn thousands(v: u64) -> String {
    let s = v.to_string();
    let mut out = String::new();
    for (i, c) in s.chars().enumerate() {
        if i > 0 && (s.len() - i) % 3 == 0 {
            out.push(',');
        }
        out.push(c);
    }
    out
}

impl DataView {
    pub fn new(env: DataEnv, params: DataViewParams) -> DataView {
        let (tx, rx) = channel();
        let mut v = DataView {
            env,
            tx,
            rx,
            jobs: 0,
            info: None,
            path: String::new(),
            filter: {
                let mut f = TextField::new("Filter: price > 10 and contains(name, 'tea')   (Enter to apply)", false);
                f.max = 400;
                f
            },
            filter_applied: String::new(),
            filter_good: String::new(),
            filter_err: None,
            sort: Vec::new(),
            ver: 0,
            total: 0,
            window: None,
            asked: None,
            top: 0,
            hx: 0.0,
            widths: Vec::new(),
            cursor: (0, 0),
            anchor: None,
            focus: Focus::Table,
            summary: None,
            show_summary: false,
            summary_asked: None,
            modal: None,
            status: None,
            geo: Geo::default(),
            dirty: true,
            pending_filter: None,
            show_hidden: false,
            host_export_after: None,
            ticks: 0,
            stamp: None,
        };
        if let Some(p) = params.path {
            v.pending_filter = params.filter;
            v.open_path(&p);
        }
        v
    }

    // ------------------------------------------------------------------ state a host or a test may read

    pub fn path(&self) -> Option<&str> {
        self.info.as_ref().map(|_| self.path.as_str())
    }
    pub fn info(&self) -> Option<&DatasetInfo> {
        self.info.as_ref()
    }
    /// Rows in the result (after the filter).
    pub fn total(&self) -> u64 {
        self.total
    }
    pub fn status(&self) -> Option<&(String, bool)> {
        self.status.as_ref()
    }
    pub fn filter_error(&self) -> Option<&str> {
        self.filter_err.as_deref()
    }
    pub fn filter_text(&self) -> &str {
        &self.filter.value
    }
    pub fn sort_keys(&self) -> &[SortKey] {
        &self.sort
    }
    pub fn top(&self) -> u64 {
        self.top
    }
    pub fn cursor(&self) -> (u64, usize) {
        self.cursor
    }
    pub fn column_names(&self) -> Vec<String> {
        self.info.as_ref().map(|i| i.columns.iter().map(|c| c.name.clone()).collect()).unwrap_or_default()
    }
    /// The rows held in memory (the window), as text; for tests and hosts that want to read what is shown.
    pub fn loaded_rows(&self) -> Vec<Vec<String>> {
        self.window.as_ref().map(|w| w.rows.iter().map(|r| r.iter().map(table::cell_text).collect()).collect()).unwrap_or_default()
    }
    pub fn busy(&self) -> bool {
        self.jobs > 0
    }
    /// Wait for the worker (tests, and hosts that need the data now).
    pub fn settle(&mut self) {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(60);
        loop {
            self.poll();
            self.ensure_window();
            self.ensure_summary();
            if self.jobs == 0 && !self.needs_window() {
                break;
            }
            if std::time::Instant::now() > deadline {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(2));
        }
    }
    /// Something changed since the last draw.
    pub fn wants_redraw(&mut self) -> bool {
        std::mem::take(&mut self.dirty)
    }

    // ------------------------------------------------------------------ jobs

    fn say(&mut self, text: impl Into<String>, ok: bool) {
        self.status = Some((text.into(), ok));
        self.dirty = true;
    }

    fn spawn(&mut self, f: impl FnOnce(Arc<dyn DataService>) -> Done + Send + 'static) {
        let svc = self.env.svc.clone();
        let tx = self.tx.clone();
        self.jobs += 1;
        std::thread::spawn(move || {
            let _ = tx.send(f(svc));
        });
        self.dirty = true;
    }

    fn pipeline(&self) -> Option<Pipeline> {
        let info = self.info.as_ref()?;
        let mut p = Pipeline::new(&info.handle);
        if !self.filter_applied.trim().is_empty() {
            p.ops.push(Op::Filter { expr: self.filter_applied.clone() });
        }
        if !self.sort.is_empty() {
            p.ops.push(Op::Sort { keys: self.sort.clone() });
        }
        Some(p)
    }

    /// Open a file (checked against the account's read scope first).
    pub fn open_path(&mut self, path: &str) {
        let real_s = match self.resolve_open(path) {
            Ok(p) => p,
            Err(e) => return self.say(e, false),
        };
        if let Some(old) = self.info.take() {
            self.env.svc.close(&old.handle);
        }
        self.path = real_s.clone();
        self.reset_view();
        self.say(format!("Opening {real_s}…"), true);
        self.spawn(move |svc| Done::Opened(svc.open(&OpenRequest::new(real_s))));
    }

    /// A path the person typed or picked, as a real path inside the open dialog's roots (`~` is the home folder).
    fn resolve_open(&self, path: &str) -> Result<String, String> {
        let roots = self.env.roots();
        let real = roots.resolve(roots.home.as_path(), path)?;
        let real_s = real.to_string_lossy().into_owned();
        if !(self.env.can_read)(&real_s) {
            return Err(format!("You may not read {real_s}."));
        }
        Ok(real_s)
    }

    fn recent_list(&self) -> Vec<String> {
        let Some(p) = &self.env.recent else { return Vec::new() };
        let all: Vec<String> = std::fs::read(p).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default();
        let roots = self.env.roots();
        all.into_iter().filter(|f| Path::new(f).is_file() && std::fs::canonicalize(f).is_ok_and(|r| roots.contains(&r))).take(RECENT_KEPT).collect()
    }

    fn remember_recent(&self, path: &str) {
        let Some(p) = &self.env.recent else { return };
        let mut v = vec![path.to_string()];
        v.extend(self.recent_list().into_iter().filter(|f| f != path));
        v.truncate(RECENT_KEPT);
        if let Ok(t) = serde_json::to_vec(&v) {
            let _ = std::fs::write(p, t);
        }
    }

    fn reset_view(&mut self) {
        self.sort.clear();
        self.filter_applied.clear();
        self.filter_good.clear();
        self.filter_err = None;
        self.window = None;
        self.asked = None;
        self.summary = None;
        self.summary_asked = None;
        self.top = 0;
        self.hx = 0.0;
        self.widths.clear();
        self.cursor = (0, 0);
        self.anchor = None;
        self.total = 0;
        self.ver += 1;
    }

    fn rows_in_view(&self) -> u64 {
        self.geo.rows.max(30)
    }

    fn needs_window(&self) -> bool {
        if self.info.is_none() || self.total == 0 && self.window.is_some() {
            return false;
        }
        let n = self.rows_in_view();
        match (&self.window, self.asked) {
            (_, Some((v, _))) if v == self.ver => false,
            (Some(w), _) => {
                let end = w.start + w.rows.len() as u64;
                let last = (self.top + n).min(self.total.max(1));
                !(self.top >= w.start && last <= end) && self.total > 0
            }
            (None, _) => true,
        }
    }

    fn ensure_window(&mut self) {
        if !self.needs_window() {
            return;
        }
        let Some(pipeline) = self.pipeline() else { return };
        let n = self.rows_in_view();
        let start = self.top.saturating_sub(n);
        let limit = (n * 3).clamp(90, 400);
        let ver = self.ver;
        self.asked = Some((ver, start));
        self.spawn(move |svc| Done::Page { ver, start, res: svc.page(&PageRequest { pipeline, offset: start, limit }) });
    }

    fn ensure_summary(&mut self) {
        if !self.show_summary || self.summary.as_ref().is_some_and(|s| s.0 == self.ver) || self.summary_asked == Some(self.ver) {
            return;
        }
        let Some(pipeline) = self.pipeline() else { return };
        let ver = self.ver;
        self.summary_asked = Some(ver);
        self.spawn(move |svc| Done::Summary { ver, res: svc.describe(&pipeline) });
    }

    /// Take what the worker has finished.
    fn poll(&mut self) {
        while let Ok(done) = self.rx.try_recv() {
            self.jobs = self.jobs.saturating_sub(1);
            self.dirty = true;
            match done {
                Done::Opened(Ok(info)) => {
                    self.remember_recent(&self.path.clone());
                    self.total = info.rows;
                    self.say(format!("{} · {} rows × {} columns · {} engine", info.source, thousands(info.rows), info.columns.len(), info.engine), true);
                    self.info = Some(info);
                    self.stamp = self.file_stamp();
                    self.ver += 1;
                    if let Some(f) = self.pending_filter.take().filter(|f| !f.trim().is_empty()) {
                        self.filter.set(&f);
                        self.filter_applied = f;
                    }
                }
                Done::Opened(Err(e)) => {
                    self.pending_filter = None;
                    self.say(e.to_string(), false);
                }
                Done::Page { ver, start, res } => {
                    if ver != self.ver {
                        continue;
                    }
                    match res {
                        Ok(p) => {
                            self.total = p.total;
                            self.filter_good = self.filter_applied.clone();
                            if self.filter_err.is_some() && self.filter_applied == self.filter.value.trim() {
                                self.filter_err = None;
                            }
                            self.window = Some(Window { start, rows: p.rows });
                            self.asked = None;
                            self.top = self.top.min(self.total.saturating_sub(1));
                        }
                        Err(e) => {
                            // the filter (or a sort) was refused: say why and go back to the last one that worked
                            self.filter_err = Some(e.to_string());
                            self.filter_applied = self.filter_good.clone();
                            self.asked = None;
                            self.summary_asked = None;
                            self.ver += 1;
                        }
                    }
                }
                Done::Summary { ver, res } => {
                    if let (true, Ok(s)) = (ver == self.ver, res) {
                        self.summary = Some((ver, s));
                    }
                }
                Done::Exported(Ok(r)) if self.host_export_after.as_deref().is_some_and(|p| p == Path::new(&r.path)) => {
                    self.host_export_after = None;
                    if let Some(h) = self.env.host.clone() {
                        self.modal = Some(Modal::Host(Box::new(super::hostfiles::HostDialog::export_file(h, ACCENT, PathBuf::from(&r.path)))));
                    }
                }
                Done::Exported(Ok(r)) => self.say(format!("Exported {} rows to {} ({} KB).", thousands(r.rows), r.path, r.bytes.div_ceil(1024)), true),
                Done::Exported(Err(e)) => self.say(format!("Export failed: {e}"), false),
            }
        }
    }

    // ------------------------------------------------------------------ actions

    fn changed_pipeline(&mut self) {
        self.ver += 1;
        self.window = None;
        self.asked = None;
        self.summary_asked = None;
        self.top = 0;
        self.cursor.0 = 0;
        self.anchor = None;
    }

    pub fn apply_filter(&mut self) {
        let text = self.filter.value.trim().to_string();
        if text == self.filter_applied {
            return;
        }
        self.filter_err = None;
        self.filter_applied = text;
        self.changed_pipeline();
    }

    /// Click on a column's header: ascending, then descending, then unsorted (Shift adds it as a further key).
    pub fn sort_by(&mut self, col: usize, add: bool) {
        let Some(name) = self.info.as_ref().and_then(|i| i.columns.get(col)).map(|c| c.name.clone()) else { return };
        let cur = self.sort.iter().position(|k| k.column == name);
        let next = match cur.map(|i| self.sort[i].dir) {
            None => Some(Dir::Asc),
            Some(Dir::Asc) => Some(Dir::Desc),
            Some(Dir::Desc) => None,
        };
        if !add {
            self.sort.retain(|k| k.column == name);
        }
        match (cur, next) {
            (Some(i), Some(d)) => self.sort[i].dir = d,
            (Some(i), None) => {
                self.sort.remove(i);
            }
            (None, Some(d)) => self.sort.push(SortKey { column: name, dir: d }),
            (None, None) => {}
        }
        self.changed_pipeline();
    }

    fn set_sort(&mut self, col: usize, dir: Dir) {
        if let Some(name) = self.info.as_ref().and_then(|i| i.columns.get(col)).map(|c| c.name.clone()) {
            self.sort = vec![SortKey { column: name, dir }];
            self.changed_pipeline();
        }
    }

    pub fn toggle_summary(&mut self) {
        self.show_summary = !self.show_summary;
        self.dirty = true;
    }

    fn cell(&self, row: u64, col: usize) -> Option<&Value> {
        let w = self.window.as_ref()?;
        w.rows.get(row.checked_sub(w.start)? as usize)?.get(col)
    }

    fn selection(&self) -> ((u64, usize), (u64, usize)) {
        let a = self.anchor.unwrap_or(self.cursor);
        let c = self.cursor;
        ((a.0.min(c.0), a.1.min(c.1)), (a.0.max(c.0), a.1.max(c.1)))
    }

    /// The text of rows `r0..=r1` and columns `c0..=c1`, tab-separated (what a spreadsheet pastes).
    fn text_of(&self, r0: u64, r1: u64, c0: usize, c1: usize) -> Result<String, String> {
        let r1 = r1.min(r0 + MAX_COPY_ROWS - 1).min(self.total.saturating_sub(1));
        let rows: Vec<Vec<Value>> = match self.window.as_ref().filter(|w| r0 >= w.start && r1 < w.start + w.rows.len() as u64) {
            Some(w) => w.rows[(r0 - w.start) as usize..=(r1 - w.start) as usize].to_vec(),
            None => {
                let p = self.pipeline().ok_or("no table is open")?;
                self.env.svc.page(&PageRequest { pipeline: p, offset: r0, limit: r1 - r0 + 1 }).map_err(|e| e.to_string())?.rows
            }
        };
        let flat = |v: &Value| table::cell_text(v).replace(['\t', '\n', '\r'], " ");
        Ok(rows.iter().map(|r| r.iter().enumerate().filter(|(i, _)| *i >= c0 && *i <= c1).map(|(_, v)| flat(v)).collect::<Vec<_>>().join("\t")).collect::<Vec<_>>().join("\n"))
    }

    pub fn copy_cell(&mut self) {
        match self.cell(self.cursor.0, self.cursor.1).map(table::cell_text) {
            Some(t) => {
                crate::ui::clipboard::copy(&t);
                self.say("Copied the cell.", true);
            }
            None => self.say("Nothing to copy: scroll a row into view first.", false),
        }
    }

    pub fn copy_row(&mut self) {
        let ncols = self.info.as_ref().map_or(0, |i| i.columns.len());
        match self.text_of(self.cursor.0, self.cursor.0, 0, ncols.saturating_sub(1)) {
            Ok(t) => {
                crate::ui::clipboard::copy(&t);
                self.say("Copied the row.", true);
            }
            Err(e) => self.say(e, false),
        }
    }

    pub fn copy_selection(&mut self) {
        let ((r0, c0), (r1, c1)) = self.selection();
        match self.text_of(r0, r1, c0, c1) {
            Ok(t) if t.chars().count() > crate::ui::clipboard::MAX_BYTES => {
                crate::ui::clipboard::copy(&t);
                self.say(format!("Copied the first {} KB of the selection (the clipboard keeps that much). Export the view to get all of it.", crate::ui::clipboard::MAX_BYTES / 1024), true);
            }
            Ok(t) => {
                crate::ui::clipboard::copy(&t);
                let n = r1.min(r0 + MAX_COPY_ROWS - 1) - r0 + 1;
                self.say(if r1 - r0 + 1 > MAX_COPY_ROWS { format!("Copied the first {} rows of the selection.", thousands(n)) } else { format!("Copied {} row{} × {} column{}.", thousands(n), if n == 1 { "" } else { "s" }, c1 - c0 + 1, if c1 == c0 { "" } else { "s" }) }, true);
            }
            Err(e) => self.say(e, false),
        }
    }

    fn select_all(&mut self) {
        let ncols = self.info.as_ref().map_or(1, |i| i.columns.len());
        self.anchor = Some((0, 0));
        self.cursor = (self.total.saturating_sub(1), ncols.saturating_sub(1));
        self.dirty = true;
    }

    fn filter_to_value(&mut self) {
        let (row, col) = self.cursor;
        let (Some(c), Some(v)) = (self.info.as_ref().and_then(|i| i.columns.get(col)).cloned(), self.cell(row, col).cloned()) else { return };
        let name = if c.name.chars().all(|ch| ch.is_alphanumeric() || ch == '_') { c.name.clone() } else { format!("`{}`", c.name) };
        let expr = match v {
            Value::Null => format!("{name} is null"),
            Value::String(s) => format!("{name} = '{}'", s.replace('\'', "''")),
            other => format!("{name} = {other}"),
        };
        self.filter.set(&expr);
        self.apply_filter();
    }

    fn start_export(&mut self) {
        if self.info.is_none() {
            return self.say("Open a table first.", false);
        }
        let stem = Path::new(&self.path).file_stem().map_or("table".to_string(), |s| s.to_string_lossy().into_owned());
        let dir = Path::new(&self.path).parent().map(Path::to_path_buf).filter(|d| (self.env.can_write)(&d.to_string_lossy())).unwrap_or_else(|| self.env.roots().home);
        let f = Form::new("Export as CSV").path("File path", &format!("{}/{stem}-view.csv", dir.display()), "the rows now shown (filter and sort applied), as CSV", crate::filepicker::PathKind::Save, &[]);
        self.modal = Some(Modal::Path(f, PathFor::Export));
        self.dirty = true;
    }

    fn do_export(&mut self, path: &str) -> Result<(), String> {
        let real = iyakki_agent::scope::resolve_for_write(Path::new("/"), path.trim())?;
        if !(self.env.can_write)(&real.to_string_lossy()) {
            return Err(format!("You may not write {}.", real.display()));
        }
        if real.is_dir() {
            return Err(format!("{} is a folder; name a file.", real.display()));
        }
        iyakki_agent::scope::ensure_parent(&real)?;
        let Some(pipeline) = self.pipeline() else { return Err("Open a table first.".into()) };
        let req = ExportRequest { pipeline, path: real.to_string_lossy().into_owned() };
        self.say(format!("Exporting to {}…", real.display()), true);
        self.spawn(move |svc| Done::Exported(svc.export_csv(&req)));
        Ok(())
    }

    /// The quick links of the open dialog: (label, folder), those that exist inside the home folder.
    fn quick_links(&self) -> Vec<(String, PathBuf)> {
        let roots = self.env.roots();
        let home = roots.home.clone();
        let mut v = vec![("Home".to_string(), home.clone())];
        for n in ["Documents", "Downloads", "Workspaces"] {
            let p = home.join(n);
            if p.is_dir() && std::fs::canonicalize(&p).is_ok_and(|r| roots.contains(&r)) {
                v.push((n.to_string(), p));
            }
        }
        // folders shared from the host computer (window mode), each its own link
        for m in &roots.mounts {
            v.push((format!("host:{}", m.name), PathBuf::from(&m.path)));
        }
        v
    }

    /// Show the dialog at `dir`, which must be a real folder inside the roots (anything else falls back to home): the
    /// quick links, the recent files, then the folders and table files in it; `..` stops at the top of the root.
    fn open_dialog(&mut self, dir: PathBuf) {
        let roots = self.env.roots();
        let dir = match std::fs::canonicalize(&dir) {
            Ok(r) if r.is_dir() && roots.contains(&r) => r,
            _ => roots.home.clone(),
        };
        let mut dirs = Vec::new();
        let mut files = Vec::new();
        if let Ok(rd) = std::fs::read_dir(&dir) {
            for e in rd.flatten().take(5000) {
                let name = e.file_name().to_string_lossy().into_owned();
                if name.starts_with('.') && !self.show_hidden {
                    continue;
                }
                // a link that leads out of the roots is not shown
                let Ok(real) = std::fs::canonicalize(e.path()) else { continue };
                if !roots.contains(&real) || !(self.env.can_read)(&real.to_string_lossy()) {
                    continue;
                }
                match std::fs::metadata(&real) {
                    Ok(m) if m.is_dir() => dirs.push(format!("{name}/")),
                    Ok(m) if m.is_file() && iyakki_data::Format::from_path(&name).is_some() => files.push(name),
                    _ => {}
                }
            }
        }
        dirs.sort();
        files.sort();
        let mut items = vec!["Type a path…".to_string()];
        items.extend(self.quick_links().into_iter().map(|(l, _)| format!("★ {l}")));
        items.push(if self.show_hidden { "☑ Hidden folders shown (choose to hide)".to_string() } else { "☐ Hidden folders hidden (choose to show)".to_string() });
        items.extend(self.recent_list().into_iter().map(|r| format!("↺ {r}")));
        // `..` goes up only while the parent is still inside the same root
        if dir.parent().is_some_and(|p| roots.contains(p)) {
            items.push("../".into());
        }
        items.extend(dirs);
        items.extend(files);
        let shown = dir.strip_prefix(&roots.home).map_or_else(|_| dir.display().to_string(), |r| if r.as_os_str().is_empty() { "~".to_string() } else { format!("~/{}", r.display()) });
        self.modal = Some(Modal::Open { picker: Picker::new(&format!("Open a table — {shown}"), items), dir });
        self.dirty = true;
    }

    /// Open the file dialog (also what the Open button does): in the folder of the file open now, else in the home folder.
    pub fn show_open_dialog(&mut self) {
        let start = Path::new(&self.path).parent().map(Path::to_path_buf).filter(|d| d.is_dir()).unwrap_or_else(|| self.env.roots().home);
        self.open_dialog(start);
    }

    fn modal_event(&mut self, ev: &Event) {
        let Some(m) = self.modal.take() else { return };
        self.dirty = true;
        match m {
            Modal::Host(d) => self.modal = Some(Modal::Host(d)),
            Modal::Open { mut picker, dir } => match picker.handle_event(ev) {
                PickEvent::Cancel => {}
                PickEvent::None => self.modal = Some(Modal::Open { picker, dir }),
                PickEvent::Chosen(item) => {
                    if item == "Type a path…" {
                        let f = Form::new("Open a table").path("File path", &format!("{}/", dir.display()), "a CSV, TSV or JSON-lines file inside your home folder (~ is your home folder)", crate::filepicker::PathKind::File, &["csv", "tsv", "jsonl", "json"]);
                        self.modal = Some(Modal::Path(f, PathFor::Open));
                    } else if let Some(link) = item.strip_prefix("★ ") {
                        let to = self.quick_links().into_iter().find(|(l, _)| l == link).map(|l| l.1);
                        self.open_dialog(to.unwrap_or_else(|| self.env.roots().home));
                    } else if item.starts_with('☐') || item.starts_with('☑') {
                        self.show_hidden = !self.show_hidden;
                        self.open_dialog(dir);
                    } else if let Some(r) = item.strip_prefix("↺ ") {
                        self.open_path(r);
                    } else if item == "../" {
                        self.open_dialog(dir.parent().map_or(dir.clone(), Path::to_path_buf));
                    } else if let Some(d) = item.strip_suffix('/') {
                        self.open_dialog(dir.join(d));
                    } else {
                        self.open_path(&dir.join(&item).to_string_lossy());
                    }
                }
            },
            Modal::Path(mut f, what) => match f.handle_event(ev) {
                FormEvent::Cancel => {}
                FormEvent::Submit => {
                    let path = f.value(0);
                    match what {
                        PathFor::Open => match self.resolve_open(&path) {
                            Ok(_) => self.open_path(&path),
                            Err(e) => {
                                f.error = Some(e);
                                self.modal = Some(Modal::Path(f, what));
                            }
                        },
                        PathFor::Export => {
                            if let Err(e) = self.do_export(&path) {
                                f.error = Some(e);
                                self.modal = Some(Modal::Path(f, what));
                            }
                        }
                    }
                }
                _ => self.modal = Some(Modal::Path(f, what)),
            },
        }
    }

    // ------------------------------------------------------------------ host files

    /// Is host access available (window mode) and wired to this view?
    pub fn host_ready(&self) -> bool {
        self.env.host.is_some()
    }

    /// "Host access: 2 folders" while folders are shared (for a chip or the window title).
    pub fn host_chip(&self) -> Option<String> {
        let n = self.env.roots().mounts.len();
        (n > 0).then(|| format!("Host access: {n} folder{}", if n == 1 { "" } else { "s" }))
    }

    pub fn show_host_dialog(&mut self) {
        if let Some(h) = self.env.host.clone() {
            self.modal = Some(Modal::Host(Box::new(super::hostfiles::HostDialog::open(h, ACCENT))));
            self.dirty = true;
        }
    }

    /// Files or folders dropped on the window: import them (opens the table straight away when it is one).
    pub fn drop_files(&mut self, paths: Vec<PathBuf>) -> bool {
        let Some(h) = self.env.host.clone() else { return false };
        self.modal = Some(Modal::Host(Box::new(super::hostfiles::HostDialog::import_paths(h, ACCENT, paths))));
        self.dirty = true;
        true
    }

    /// Save the rows now shown on the host computer: write them inside the account first, then ask where to put them.
    pub fn export_to_host(&mut self) {
        let Some(h) = self.env.host.clone() else { return };
        let Some(pipeline) = self.pipeline() else { return self.say("Open a table first.", false) };
        let stem = Path::new(&self.path).file_stem().map_or("table".to_string(), |s| s.to_string_lossy().into_owned());
        let tmp = h.data_dir.join("host-out").join(&h.user);
        let _ = std::fs::create_dir_all(&tmp);
        let file = tmp.join(format!("{stem}-view.csv"));
        self.host_export_after = Some(file.clone());
        self.say("Preparing the file…", true);
        let req = ExportRequest { pipeline, path: file.to_string_lossy().into_owned() };
        self.spawn(move |svc| Done::Exported(svc.export_csv(&req)));
    }

    fn host_out(&mut self, out: super::hostfiles::Out) {
        use super::hostfiles::Out as H;
        match out {
            H::Stay => {}
            H::Close => self.modal = None,
            H::Imported(files) => {
                let table = files.iter().find(|f| f.extension().is_some_and(|e| matches!(e.to_string_lossy().to_ascii_lowercase().as_str(), "csv" | "tsv")));
                self.say(format!("Imported {} file{}.", files.len(), if files.len() == 1 { "" } else { "s" }), true);
                if let Some(t) = table.cloned() {
                    self.modal = None;
                    self.open_path(&t.to_string_lossy());
                }
            }
            H::Linked(name) => self.say(format!("host:{name} is shared read-only."), true),
            H::Unlinked(name) => {
                self.say(format!("host:{name} is no longer shared."), true);
                self.recheck_access();
            }
        }
    }

    fn file_stamp(&self) -> Option<(u64, std::time::SystemTime)> {
        std::fs::metadata(&self.path).ok().and_then(|m| Some((m.len(), m.modified().ok()?)))
    }

    /// The open file was replaced or grown by someone else (the assistant added rows): open it again, keeping the filter.
    /// Returns true when a reload started.
    pub fn reload_if_changed(&mut self) -> bool {
        if self.info.is_none() || self.jobs > 0 || self.stamp.is_none() || self.file_stamp().is_none_or(|now| Some(now) == self.stamp) {
            return false;
        }
        let (path, filter) = (self.path.clone(), self.filter_applied.clone());
        self.open_path(&path);
        self.pending_filter = Some(filter);
        self.say(format!("{path} changed, so it was reloaded."), true);
        true
    }

    /// The open file must still be somewhere the person may read; when a shared folder is withdrawn the table is closed.
    fn recheck_access(&mut self) {
        if self.info.is_none() {
            return;
        }
        let ok = std::fs::canonicalize(&self.path).is_ok_and(|real| self.env.roots().contains(&real) && (self.env.can_read)(&real.to_string_lossy()));
        if !ok {
            if let Some(old) = self.info.take() {
                self.env.svc.close(&old.handle);
            }
            let p = std::mem::take(&mut self.path);
            self.reset_view();
            self.say(format!("Access to {p} was removed (its folder is no longer shared or no longer there), so the table was closed."), false);
        }
    }

    // ------------------------------------------------------------------ events

    fn col_at(&self, x: f32) -> Option<usize> {
        let s = self.geo.scale.max(0.1);
        let x = x + self.hx * s;
        self.geo.cols.iter().position(|(cx, w)| x >= *cx && x < cx + w)
    }

    fn move_cursor(&mut self, dr: i64, dc: i64, extend: bool) {
        let ncols = self.info.as_ref().map_or(1, |i| i.columns.len()).max(1);
        let r = (self.cursor.0 as i64 + dr).clamp_safe(0, self.total.saturating_sub(1) as i64) as u64;
        let c = (self.cursor.1 as i64 + dc).clamp_safe(0, ncols as i64 - 1) as usize;
        if extend {
            self.anchor.get_or_insert(self.cursor);
        } else {
            self.anchor = None;
        }
        self.cursor = (r, c);
        self.scroll_to_cursor();
    }

    fn scroll_to_cursor(&mut self) {
        let n = self.rows_in_view();
        if self.cursor.0 < self.top {
            self.top = self.cursor.0;
        } else if self.cursor.0 >= self.top + n {
            self.top = self.cursor.0 + 1 - n;
        }
        // keep the column in view
        if let (Some(body), Some((x, w))) = (self.geo.body, self.geo.cols.get(self.cursor.1).copied()) {
            let s = self.geo.scale.max(0.1);
            let left = x / s;
            let right = (x + w) / s;
            let view = body.w / s;
            if left < self.hx {
                self.hx = left;
            } else if right > self.hx + view {
                self.hx = right - view;
            }
        }
        self.dirty = true;
    }

    fn scroll_rows(&mut self, d: i64) {
        let max = self.total.saturating_sub(self.rows_in_view().min(self.total));
        self.top = (self.top as i64 + d).clamp_safe(0, max as i64) as u64;
        self.dirty = true;
    }

    fn widen(&mut self, by: f32) {
        let c = self.cursor.1;
        if let Some(w) = self.widths.get_mut(c) {
            *w = (*w + by).clamp(MIN_COL, MAX_COL);
            self.dirty = true;
        }
    }

    pub fn handle(&mut self, ev: &Event) -> Out {
        if matches!(ev, Event::Tick) {
            self.poll();
            self.ticks += 1;
            if self.ticks % 30 == 0 {
                self.recheck_access();
                self.reload_if_changed();
            }
            if let Some(Modal::Host(d)) = self.modal.as_mut() {
                let out = d.handle(ev);
                self.host_out(out);
            }
            self.ensure_window();
            self.ensure_summary();
            return Out::Stay;
        }
        if let Some(Modal::Host(d)) = self.modal.as_mut() {
            let out = d.handle(ev);
            self.host_out(out);
            self.dirty = true;
            return Out::Stay;
        }
        if self.modal.is_some() {
            self.modal_event(ev);
            return Out::Stay;
        }
        self.dirty = true;
        match ev {
            Event::FocusAt(FILTER_FIELD, x) => {
                self.focus = Focus::Filter;
                self.filter.click(*x);
            }
            Event::AreaAt(TABLE_AREA, x, y) => {
                self.focus = Focus::Table;
                let (rh, s) = (self.geo.row_h.max(1.0), self.geo.scale);
                let Some(col) = self.col_at(*x) else { return Out::Stay };
                let row = (self.top + (*y / rh).floor().max(0.0) as u64).min(self.total.saturating_sub(1));
                let _ = s;
                if crate::ui::dragging() {
                    self.anchor.get_or_insert(self.cursor);
                } else if crate::ui::shift_held() {
                    self.anchor.get_or_insert(self.cursor);
                } else {
                    self.anchor = None;
                }
                self.cursor = (row, col);
                if crate::ui::click_count() >= 3 && !crate::ui::dragging() {
                    self.select_all();
                }
            }
            Event::AreaAt(SCROLL_AREA, _, y) => {
                if let Some(body) = self.geo.body {
                    // the thumb's middle follows the pointer, so the bottom of the track is the last row
                    let vis = self.rows_in_view().min(self.total).max(1) as f32;
                    let thumb = (body.h * vis / self.total.max(1) as f32).max(18.0 * self.geo.scale);
                    let frac = ((*y - thumb / 2.0) / (body.h - thumb).max(1.0)).clamp(0.0, 1.0);
                    let max = self.total.saturating_sub(self.rows_in_view().min(self.total));
                    self.top = (frac * max as f32).round() as u64;
                }
            }
            Event::AreaAt(id, x, _) if *id >= GRIP_BASE && *id < GRIP_BASE + 4096 => {
                // dragging the right edge of a header: the pointer is at `x` from the grip's left edge
                let col = id - GRIP_BASE;
                let s = self.geo.scale.max(0.1);
                if let Some(w) = self.widths.get_mut(col) {
                    *w = (*w + (*x - 3.0 * s) / s).clamp(MIN_COL, MAX_COL);
                }
            }
            Event::Tab(c) => {
                if *c < 4096 {
                    self.sort_by(*c, crate::ui::shift_held());
                    self.cursor.1 = *c;
                } else if *c == 5000 {
                    self.show_open_dialog();
                } else if *c == 5001 {
                    self.toggle_summary();
                } else if *c == 5002 {
                    self.start_export();
                } else if *c == 5003 {
                    self.copy_selection();
                } else if *c == 5004 {
                    self.show_host_dialog();
                } else if *c == 5005 {
                    self.export_to_host();
                }
            }
            Event::Scroll(d) => self.scroll_rows(-(*d as i64) * 3),
            Event::Paste(t) if self.focus == Focus::Filter => {
                self.filter.paste(t);
            }
            Event::Key(k) => return self.key(k),
            _ => {}
        }
        Out::Stay
    }

    fn key(&mut self, k: &Key) -> Out {
        if self.focus == Focus::Filter {
            match k {
                Key::Enter => {
                    self.apply_filter();
                    self.focus = Focus::Table;
                }
                Key::Esc | Key::Down | Key::Tab => self.focus = Focus::Table,
                other => {
                    self.filter.handle(other);
                }
            }
            return Out::Stay;
        }
        let ctrl = |c: char| matches!(k, Key::Ctrl(x) if x.eq_ignore_ascii_case(&c));
        match k {
            Key::Esc => {
                if self.anchor.is_some() {
                    self.anchor = None;
                } else {
                    return Out::Close;
                }
            }
            Key::Up | Key::Nav { nav: Nav::Up, .. } => self.move_cursor(-1, 0, matches!(k, Key::Nav { shift: true, .. })),
            Key::Down | Key::Nav { nav: Nav::Down, .. } => self.move_cursor(1, 0, matches!(k, Key::Nav { shift: true, .. })),
            Key::Left | Key::Nav { nav: Nav::Left, .. } => self.move_cursor(0, -1, matches!(k, Key::Nav { shift: true, .. })),
            Key::Right | Key::Nav { nav: Nav::Right, .. } => self.move_cursor(0, 1, matches!(k, Key::Nav { shift: true, .. })),
            Key::PageUp | Key::Nav { nav: Nav::PageUp, .. } => self.move_cursor(-(self.rows_in_view() as i64 - 1), 0, matches!(k, Key::Nav { shift: true, .. })),
            Key::PageDown | Key::Nav { nav: Nav::PageDown, .. } => self.move_cursor(self.rows_in_view() as i64 - 1, 0, matches!(k, Key::Nav { shift: true, .. })),
            Key::Home | Key::Nav { nav: Nav::Home, .. } => {
                let shift = matches!(k, Key::Nav { shift: true, .. });
                if matches!(k, Key::Nav { ctrl: true, .. }) {
                    self.move_cursor(i64::MIN / 2, i64::MIN / 2, shift)
                } else {
                    self.move_cursor(0, i64::MIN / 2, shift)
                }
            }
            Key::End | Key::Nav { nav: Nav::End, .. } => {
                let shift = matches!(k, Key::Nav { shift: true, .. });
                if matches!(k, Key::Nav { ctrl: true, .. }) {
                    self.move_cursor(i64::MAX / 2, i64::MAX / 2, shift)
                } else {
                    self.move_cursor(0, i64::MAX / 2, shift)
                }
            }
            Key::Enter => self.sort_by(self.cursor.1, false),
            _ if ctrl('c') => self.copy_selection(),
            _ if ctrl('a') => self.select_all(),
            _ if ctrl('f') || ctrl('l') => self.focus = Focus::Filter,
            _ if ctrl('o') => self.show_open_dialog(),
            _ if ctrl('e') => self.start_export(),
            Key::Char('/') | Key::Char('f') => self.focus = Focus::Filter,
            Key::Char('o') => self.show_open_dialog(),
            Key::Char('e') => self.start_export(),
            Key::Char('s') => self.toggle_summary(),
            Key::Char('c') => self.copy_cell(),
            Key::Char('r') => self.copy_row(),
            Key::Char('v') => self.filter_to_value(),
            Key::Char('>') => self.widen(24.0),
            Key::Char('<') => self.widen(-24.0),
            Key::Char('=') => {
                if let Some(w) = self.widths.get_mut(self.cursor.1) {
                    *w = 0.0; // measured again on the next draw
                }
            }
            Key::Char('x') => {
                self.filter_err = None;
                self.filter.set("");
                self.apply_filter();
                self.sort.clear();
                self.changed_pipeline();
            }
            _ => {}
        }
        Out::Stay
    }

    pub fn hint(&self) -> String {
        if self.modal.is_some() {
            return "Enter choose · Esc cancel".into();
        }
        "o open · click a header to sort · / filter · s summary · c cell · r row · Ctrl+C selection · e export · < > width".into()
    }

    pub fn help(&self) -> Vec<String> {
        vec![
            "o · open a table file".into(),
            "click a column header · sort ascending, descending, off (Shift adds a second key)".into(),
            "/ or Ctrl+F · the filter box (Enter applies; x clears filter and sort)".into(),
            "arrows, PgUp/PgDn, Ctrl+Home/End · move through the rows".into(),
            "Shift+arrows or Shift+click · select a block".into(),
            "c · copy the cell, r · copy the row, Ctrl+C · copy the selection, Ctrl+A · select all".into(),
            "v · filter to the value in this cell".into(),
            "< and > · narrow or widen the column, = · fit it, or drag the edge of its header".into(),
            "s · summary of every column, e · export as CSV".into(),
        ]
    }

    pub fn context_items(&mut self, x: i32, y: i32) -> Vec<Item> {
        let on_table = self.geo.body.is_some_and(|b| b.contains(x, y));
        if !on_table || self.info.is_none() {
            return Vec::new();
        }
        if let Some(b) = self.geo.body {
            let (rh, _) = (self.geo.row_h.max(1.0), 0);
            if let Some(col) = self.col_at(x as f32 - b.x) {
                let row = (self.top + ((y as f32 - b.y) / rh).floor().max(0.0) as u64).min(self.total.saturating_sub(1));
                let inside = {
                    let ((r0, c0), (r1, c1)) = self.selection();
                    self.anchor.is_some() && row >= r0 && row <= r1 && col >= c0 && col <= c1
                };
                if !inside {
                    self.cursor = (row, col);
                    self.anchor = None;
                }
            }
        }
        vec![
            Item::new("data.cell", "Copy cell", "C"),
            Item::new("data.row", "Copy row", "R"),
            Item::new("data.sel", "Copy selection", "Ctrl+C"),
            Item::separator(),
            Item::new("data.filter", "Filter to this value", "V"),
            Item::new("data.asc", "Sort by this column, ascending", ""),
            Item::new("data.desc", "Sort by this column, descending", ""),
        ]
    }

    pub fn context_pick(&mut self, id: &str) -> bool {
        match id {
            "data.cell" => self.copy_cell(),
            "data.row" => self.copy_row(),
            "data.sel" => self.copy_selection(),
            "data.filter" => self.filter_to_value(),
            "data.asc" => self.set_sort(self.cursor.1, Dir::Asc),
            "data.desc" => self.set_sort(self.cursor.1, Dir::Desc),
            _ => return false,
        }
        true
    }

    // ------------------------------------------------------------------ drawing

    fn measure_widths(&mut self, g: &mut Gfx, s: f32) {
        let Some(info) = &self.info else { return };
        let n = info.columns.len();
        if self.widths.len() != n {
            self.widths = vec![0.0; n];
        }
        for (i, c) in info.columns.iter().enumerate() {
            if self.widths[i] > 0.0 {
                continue;
            }
            let mut w = g.text_width(13.0 * s, &c.name, Face::Bold) / s + 44.0;
            if let Some(win) = &self.window {
                for r in win.rows.iter().take(60) {
                    if let Some(v) = r.get(i) {
                        w = w.max(g.text_width(13.0 * s, &table::cell_text(v), Face::Sans) / s + 20.0);
                    }
                }
            }
            self.widths[i] = w.clamp_safe(MIN_COL + 24.0, 300.0);
        }
    }

    pub fn draw(&mut self, g: &mut Gfx, area: R, s: f32, tick: u64) {
        self.poll();
        let pad = 16.0 * s;
        let x = area.x + pad;
        let w = area.w - 2.0 * pad;
        let mut y = area.y + 6.0 * s;
        let busy = self.jobs > 0;

        // the toolbar
        let mut bx = x;
        bx += g.button(bx, y, 14.0 * s, "Open…", ACCENT, Event::Tab(5000)) + 8.0 * s;
        if self.info.is_some() {
            bx += g.button(bx, y, 14.0 * s, if self.show_summary { "Hide summary" } else { "Summary" }, ACCENT, Event::Tab(5001)) + 8.0 * s;
            bx += g.button(bx, y, 14.0 * s, "Export CSV", ACCENT, Event::Tab(5002)) + 8.0 * s;
            bx += g.button(bx, y, 14.0 * s, "Copy selection", ACCENT, Event::Tab(5003)) + 8.0 * s;
            if self.env.host.is_some() {
                bx += g.button(bx, y, 14.0 * s, "Save to host…", ACCENT, Event::Tab(5005)) + 8.0 * s;
            }
            bx += 6.0 * s;
        }
        if self.env.host.is_some() {
            bx += g.button(bx, y, 14.0 * s, "Host files…", ACCENT, Event::Tab(5004)) + 8.0 * s;
            if let Some(chip) = self.host_chip() {
                bx += crate::fx::chip(g, bx, y + 6.0 * s, s, &chip, ACCENT, true) + 8.0 * s;
            }
        }
        let title = match &self.info {
            Some(i) => {
                let name = Path::new(&self.path).file_name().map_or(self.path.clone(), |n| n.to_string_lossy().into_owned());
                let shown = if self.total != i.rows { format!("{} of {} rows", thousands(self.total), thousands(i.rows)) } else { format!("{} rows", thousands(i.rows)) };
                format!("{name}  ·  {shown} × {} columns{}", i.columns.len(), if busy { "  ·  working…" } else { "" })
            }
            None if busy => "Opening…".to_string(),
            None => String::new(),
        };
        let title = g.fit(14.0 * s, &title, Face::Bold, (x + w - bx).max(40.0 * s));
        g.text(bx, y + 8.0 * s, 14.0 * s, TEXT, &title, Face::Bold);
        y += 44.0 * s;

        if self.info.is_none() {
            self.geo = Geo { scale: s, ..Geo::default() };
            let msg = match &self.status {
                Some((m, false)) => m.clone(),
                _ => "Open a CSV, TSV or JSON-lines file. Millions of rows are fine.".to_string(),
            };
            crate::fx::empty_state(g, R::new(area.x, y, area.w, area.h - (y - area.y) - 40.0 * s), s, Icon::Data, "No table open", &msg, ACCENT);
            self.draw_modal(g, area, s, tick);
            return;
        }

        // the filter
        let fh = 38.0 * s;
        let fr = R::new(x, y, w, fh);
        self.filter.render(g, fr, self.focus == Focus::Filter && self.modal.is_none(), s, super::blink(tick), Face::Mono, FILTER_FIELD);
        y += fh + 4.0 * s;
        if let Some(e) = self.filter_err.clone() {
            let t = g.fit(13.0 * s, &format!("Filter not applied: {e}"), Face::Sans, w);
            g.text(x, y, 13.0 * s, ERR, &t, Face::Sans);
            y += 20.0 * s;
        } else {
            y += 6.0 * s;
        }

        // the summary pane takes the right side when open
        let sum_w = if self.show_summary { (w * 0.36).clamp_safe(260.0 * s, 460.0 * s).min(w * 0.6) } else { 0.0 };
        let table_w = w - if self.show_summary { sum_w + 12.0 * s } else { 0.0 };
        let status_h = 28.0 * s;
        let table_h = (area.y + area.h - y - status_h).max(60.0 * s);
        let table = R::new(x, y, table_w, table_h);
        self.draw_table(g, table, s);
        if self.show_summary {
            self.draw_summary(g, R::new(x + table_w + 12.0 * s, y, sum_w, table_h), s);
        }

        // the status line
        let sy = area.y + area.h - status_h + 4.0 * s;
        let line = match &self.status {
            Some((m, ok)) if !*ok || self.status_is_recent() => (m.clone(), *ok),
            _ => (self.position_line(), true),
        };
        crate::fx::status_line(g, x, sy, w, s, &line.0, line.1);
        self.draw_modal(g, area, s, tick);
    }

    fn status_is_recent(&self) -> bool {
        // an action's message stays until the next one; the plain position line returns after a scroll
        self.status.as_ref().is_some_and(|(m, _)| m.starts_with("Copied") || m.starts_with("Exported") || m.starts_with("Opening"))
    }

    fn position_line(&self) -> String {
        let Some(i) = &self.info else { return String::new() };
        let (r, c) = self.cursor;
        let col = i.columns.get(c).map_or(String::new(), |c| format!("{} ({})", c.name, c.dtype.name()));
        let last = (self.top + self.rows_in_view()).min(self.total);
        format!("rows {}–{} of {} · row {} · column {} · {} engine", thousands(self.top.min(self.total.saturating_sub(1)) + 1), thousands(last), thousands(self.total), thousands(r + 1), col, i.engine)
    }

    fn draw_table(&mut self, g: &mut Gfx, area: R, s: f32) {
        let Some(info) = self.info.clone() else { return };
        self.measure_widths(g, s);
        crate::fx::panel(g, area, s, ACCENT);
        let row_h = 25.0 * s;
        let head_h = 34.0 * s;
        let bar = 10.0 * s;
        let inner = area.inset(2.0 * s);
        let body = R::new(inner.x, inner.y + head_h, inner.w - bar, (inner.h - head_h).max(row_h));
        let vis = (body.h / row_h).floor().max(1.0) as u64;
        // column positions before the horizontal scroll
        let mut cols = Vec::with_capacity(info.columns.len());
        let mut cx = 0.0;
        for wd in &self.widths {
            cols.push((cx, wd * s));
            cx += wd * s;
        }
        let content_w = cx;
        let max_hx = ((content_w - body.w) / s).max(0.0);
        self.hx = self.hx.clamp_safe(0.0, max_hx);
        self.geo = Geo { body: Some(body), row_h, rows: vis, cols: cols.clone(), scale: s };
        self.top = self.top.min(self.total.saturating_sub(vis.min(self.total)));
        self.ensure_window();

        let hx = self.hx * s;
        let sel = self.selection();
        let has_sel = self.anchor.is_some();
        let win_start = self.window.as_ref().map_or(0, |w| w.start);

        // the rows (clipped to the body)
        let rows_here = vis.min(self.total.saturating_sub(self.top));
        g.clipped(body, |g| {
            for k in 0..rows_here {
                let r = self.top + k;
                let ry = body.y + k as f32 * row_h;
                if k % 2 == 1 {
                    g.rect(body.x, ry, body.w, row_h, rgb(24, 30, 52));
                }
                let row = self.window.as_ref().and_then(|w| w.rows.get((r.wrapping_sub(win_start)) as usize).filter(|_| r >= win_start));
                for (c, (cl, cw)) in cols.iter().enumerate() {
                    let px = body.x + cl - hx;
                    if px + cw < body.x || px > body.x + body.w {
                        continue;
                    }
                    let in_sel = if has_sel { r >= sel.0 .0 && r <= sel.1 .0 && c >= sel.0 .1 && c <= sel.1 .1 } else { false };
                    if in_sel {
                        g.rect(px, ry, *cw, row_h, crate::fx::with_alpha(ACCENT, 70));
                    }
                    if (r, c) == self.cursor {
                        g.rrect_outline(px + 0.5, ry + 0.5, cw - 1.0, row_h - 1.0, 3.0 * s, 1.6 * s, ACCENT);
                    }
                    match row.and_then(|row| row.get(c)) {
                        Some(Value::Null) => {
                            g.text(px + 8.0 * s, ry + 4.0 * s, 12.5 * s, DIM, "null", Face::Sans);
                        }
                        Some(v) => {
                            let text = table::cell_text(v).replace(['\n', '\r', '\t'], " ");
                            let numeric = matches!(v, Value::Number(_));
                            let t = g.fit(13.0 * s, &text, Face::Sans, cw - 14.0 * s);
                            let tx = if numeric { px + cw - 8.0 * s - g.text_width(13.0 * s, &t, Face::Sans) } else { px + 8.0 * s };
                            g.text(tx, ry + 4.0 * s, 13.0 * s, if matches!(v, Value::Bool(_)) { MUTED } else { TEXT }, &t, Face::Sans);
                        }
                        None => g.rect(px + 8.0 * s, ry + row_h / 2.0 - 2.0 * s, (cw - 16.0 * s).min(40.0 * s), 4.0 * s, rgb(40, 48, 78)),
                    }
                }
            }
        });
        g.hit(body, Event::Area(TABLE_AREA), "table");

        // the header, always in view
        let head = R::new(inner.x, inner.y, inner.w - bar, head_h);
        g.rect(head.x, head.y, head.w, head.h, rgb(30, 38, 66));
        g.rect(head.x, head.y + head.h - 1.0, head.w, 1.0, crate::fx::with_alpha(ACCENT, 140));
        g.clipped(head, |g| {
            for (c, col) in info.columns.iter().enumerate() {
                let (cl, cw) = cols[c];
                let px = head.x + cl - hx;
                if px + cw < head.x || px > head.x + head.w {
                    continue;
                }
                let cell = R::new(px, head.y, cw, head.h);
                if g.hovered(cell) {
                    g.rect(px, head.y, cw, head.h, rgb(40, 50, 84));
                }
                let key = self.sort.iter().position(|k| k.column == col.name);
                let arrow = key.map(|i| {
                    let a = if self.sort[i].dir == Dir::Asc { "▲" } else { "▼" };
                    if self.sort.len() > 1 { format!("{a}{}", i + 1) } else { a.to_string() }
                });
                let reserve = 26.0 * s + arrow.as_ref().map_or(0.0, |_| 22.0 * s);
                let name = g.fit(13.0 * s, &col.name, Face::Bold, (cw - reserve - 10.0 * s).max(10.0 * s));
                g.text(px + 8.0 * s, head.y + 3.0 * s, 13.0 * s, TEXT, &name, Face::Bold);
                g.text(px + 8.0 * s, head.y + 19.0 * s, 10.5 * s, type_color(col.dtype), col.dtype.name(), Face::Sans);
                if let Some(a) = arrow {
                    g.text(px + cw - 22.0 * s - 6.0 * s, head.y + 9.0 * s, 12.0 * s, ACCENT, &a, Face::Bold);
                }
                g.rect(px + cw - 1.0, head.y + 6.0 * s, 1.0, head.h - 12.0 * s, rgb(60, 72, 110));
                g.hit(cell, Event::Tab(c), &format!("sort {}", col.name));
            }
        });
        // the width grips sit on top of the header's own click regions
        for (c, (cl, cw)) in cols.iter().enumerate() {
            let gx = head.x + cl + cw - hx - 3.0 * s;
            if gx > head.x && gx < head.x + head.w {
                g.hit(R::new(gx, head.y, 6.0 * s, head.h), Event::Area(GRIP_BASE + c), "column width");
            }
        }

        // the scroll bar
        let track = R::new(inner.x + inner.w - bar + 3.0 * s, body.y, bar, body.h);
        crate::fx::scrollbar(g, track, s, self.top as usize, vis.min(self.total) as usize, self.total as usize, ACCENT);
        g.hit(R::new(track.x - 3.0 * s, track.y, bar, track.h), Event::Area(SCROLL_AREA), "scroll bar");
        if self.total == 0 && self.window.is_some() {
            g.text(body.x + 14.0 * s, body.y + 12.0 * s, 14.0 * s, MUTED, "No rows match.", Face::Sans);
        }
    }

    fn draw_summary(&mut self, g: &mut Gfx, area: R, s: f32) {
        crate::fx::panel(g, area, s, ACCENT);
        let x = area.x + 14.0 * s;
        g.text(x, area.y + 10.0 * s, 15.0 * s, TEXT, "Summary", Face::Bold);
        let Some((_, sum)) = self.summary.as_ref() else {
            g.text(x, area.y + 40.0 * s, 13.0 * s, MUTED, "Working it out…", Face::Sans);
            return;
        };
        g.text(x, area.y + 31.0 * s, 12.0 * s, DIM, &format!("{} rows", thousands(sum.rows)), Face::Sans);
        let fmt = |f: f64| {
            let r = (f * 1e4).round() / 1e4;
            iyakki_data::expr::fmt_float(r)
        };
        let mut y = area.y + 56.0 * s;
        let bottom = area.y + area.h - 10.0 * s;
        let w = area.w - 28.0 * s;
        g.clipped(area, |g| {
            for c in &sum.columns {
                if y > bottom {
                    break;
                }
                let head = g.fit(13.5 * s, &format!("{}  ({})", c.name, c.dtype.name()), Face::Bold, w);
                g.text(x, y, 13.5 * s, ACCENT, &head, Face::Bold);
                y += 19.0 * s;
                let mut parts = vec![format!("{} values", thousands(c.count))];
                if c.nulls > 0 {
                    parts.push(format!("{} empty", thousands(c.nulls)));
                }
                if let Some(u) = c.unique {
                    parts.push(format!("{} different", thousands(u)));
                }
                for line in [parts.join(" · "), match (&c.min, &c.max) {
                    (Some(a), Some(b)) => format!("min {} · max {}", table::cell_text(a), table::cell_text(b)),
                    _ => String::new(),
                }, match (c.mean, c.median, c.std) {
                    (Some(m), Some(md), sd) => format!("mean {} · median {}{}", fmt(m), fmt(md), sd.map_or(String::new(), |d| format!(" · std {}", fmt(d)))),
                    _ => c.top.iter().take(3).map(|(v, n)| format!("{v} ({})", thousands(*n))).collect::<Vec<_>>().join(" · "),
                }] {
                    if line.is_empty() {
                        continue;
                    }
                    let t = g.fit(12.0 * s, &line, Face::Sans, w);
                    g.text(x, y, 12.0 * s, MUTED, &t, Face::Sans);
                    y += 17.0 * s;
                }
                y += 9.0 * s;
            }
        });
    }

    fn draw_modal(&mut self, g: &mut Gfx, area: R, s: f32, tick: u64) {
        if let Some(Modal::Host(d)) = self.modal.as_mut() {
            d.render(g, area, s, tick);
            return;
        }
        match &self.modal {
            Some(Modal::Open { picker, .. }) => picker.render(g, area, s, ACCENT),
            Some(Modal::Path(f, _)) => f.render(g, area, s, ACCENT, super::blink(tick)),
            Some(Modal::Host(_)) => {}
            None => {}
        }
    }
}

fn type_color(t: DType) -> [u8; 4] {
    match t {
        DType::Int | DType::Float => rgb(120, 190, 255),
        DType::Bool => rgb(255, 170, 120),
        DType::Str => rgb(150, 215, 160),
    }
}

// ------------------------------------------------------------------------------------------------ the tile

thread_local! {
    static OPEN_NEXT: std::cell::RefCell<Option<DataViewParams>> = const { std::cell::RefCell::new(None) };
}

/// Ask for the DATA app to open on a file the next time it starts (the IDE does this for a .csv, then asks the shell for
/// the app).
pub fn request_open(path: &str, filter: Option<&str>) {
    OPEN_NEXT.with(|o| *o.borrow_mut() = Some(DataViewParams { path: Some(path.to_string()), filter: filter.map(str::to_string) }));
}

pub struct DataApp {
    view: DataView,
    user: String,
    /// The table last told to the assistant (file, column names).
    told: Option<(String, Vec<String>)>,
}

impl DataApp {
    pub fn new(cx: &AppCx) -> DataApp {
        let params = OPEN_NEXT.with(|o| o.borrow_mut().take()).unwrap_or_default();
        DataApp { view: DataView::new(DataEnv::from_cx(cx), params), user: cx.session.user.clone(), told: None }
    }

    /// Tell the chat assistant which table is shown (file and column names only), so "this table" means it.
    fn tell_assistant(&mut self) {
        let (Some(path), Some(_)) = (self.view.path(), self.view.info()) else { return };
        let cols = self.view.column_names();
        if self.told.as_ref().is_some_and(|t| t.0 == path && t.1 == cols) {
            return;
        }
        iyakki_agent::datatools::set_tab_table(&self.user, path, &cols);
        self.told = Some((path.to_string(), cols));
    }

    pub fn view(&mut self) -> &mut DataView {
        &mut self.view
    }
}

impl App for DataApp {
    fn title(&self) -> &'static str {
        "Data"
    }
    fn icon(&self) -> Icon {
        Icon::Data
    }
    fn accent(&self) -> [u8; 4] {
        ACCENT
    }
    fn hint(&self) -> String {
        self.view.hint()
    }
    fn help(&self) -> Vec<String> {
        self.view.help()
    }
    fn wants_redraw(&mut self) -> bool {
        self.view.wants_redraw()
    }
    fn on_drop(&mut self, paths: &[std::path::PathBuf], _cx: &AppCx) -> super::DropAction {
        if self.view.drop_files(paths.to_vec()) { super::DropAction::Taken } else { super::DropAction::Uploads }
    }
    fn context_items(&mut self, x: i32, y: i32) -> Vec<Item> {
        self.view.context_items(x, y)
    }
    fn context_pick(&mut self, id: &str, _cx: &AppCx) -> Flow {
        self.view.context_pick(id);
        Flow::Stay
    }
    fn handle(&mut self, ev: &Event, _cx: &AppCx) -> Flow {
        let out = self.view.handle(ev);
        if matches!(ev, Event::Tick) {
            self.tell_assistant();
        }
        match out {
            Out::Close => Flow::Back,
            Out::Stay => Flow::Stay,
        }
    }
    fn render(&mut self, g: &mut Gfx, area: R, s: f32, _cx: &AppCx, tick: u64) {
        self.view.draw(g, area, s, tick);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::apps::tests::{cx_with_admin_store, temp_dir};

    fn env_for(dir: &Path) -> DataEnv {
        let d = dir.to_string_lossy().into_owned();
        let (d1, d2) = (d.clone(), d);
        DataEnv { svc: iyakki_data::builtin_service(), can_read: Arc::new(move |p| p.starts_with(&d1)), can_write: Arc::new(move |p| p.starts_with(&d2)), roots: { let r = iyakki_agent::scope::FileRoots { home: dir.to_path_buf(), roots: vec![dir.to_path_buf()], mounts: vec![] }; Arc::new(move || r.clone()) }, recent: Some(dir.join(".recent.json")), host: None }
    }

    fn sample(dir: &Path, n: usize) -> String {
        let mut t = String::from("id,name,group,score,paid\n");
        for i in 0..n {
            t.push_str(&format!("{i},\"user {i}\",g{},{},{}\n", i % 5, (i * 37 % 1000) as f64 / 10.0, i % 2 == 0));
        }
        let p = dir.join("sample.csv");
        std::fs::write(&p, t).unwrap();
        p.to_string_lossy().into_owned()
    }

    fn view(tag: &str, n: usize) -> (DataView, PathBuf, String) {
        let d = std::fs::canonicalize(temp_dir(tag)).unwrap();
        let f = sample(&d, n);
        let mut v = DataView::new(env_for(&d), DataViewParams { path: Some(f.clone()), filter: None });
        v.settle();
        (v, d, f)
    }

    fn frame(v: &mut DataView, w: u32, h: u32) -> Gfx {
        let mut g = Gfx::new(w, h);
        g.clear(rgb(9, 11, 20));
        v.draw(&mut g, R::new(0.0, 60.0, w as f32, h as f32 - 60.0), 1.0, 0);
        v.settle();
        g.clear(rgb(9, 11, 20));
        g.hits.clear();
        v.draw(&mut g, R::new(0.0, 60.0, w as f32, h as f32 - 60.0), 1.0, 0);
        g
    }

    fn click(v: &mut DataView, g: &Gfx, label: &str) {
        let h = g.hits.iter().rev().find(|h| h.label == label).unwrap_or_else(|| panic!("no region {label}: {:?}", g.hits.iter().map(|h| &h.label).collect::<Vec<_>>()));
        v.handle(&h.ev);
        v.settle();
    }

    fn at(v: &mut DataView, g: &Gfx, row_in_view: u64, col: usize) {
        let body = v.geo.body.unwrap();
        let (cx, cw) = v.geo.cols[col];
        let _ = g;
        v.handle(&Event::AreaAt(TABLE_AREA, cx + cw / 2.0 - v.hx, row_in_view as f32 * v.geo.row_h + 4.0));
        let _ = body;
    }

    #[test]
    fn opens_a_file_shows_types_and_counts() {
        let (mut v, _d, _) = view("dv-open", 1000);
        assert_eq!(v.total(), 1000);
        assert_eq!(v.column_names(), vec!["id", "name", "group", "score", "paid"]);
        let g = frame(&mut v, 1280, 700);
        for want in ["Open…", "Summary", "Export CSV", "Copy selection", "table", "scroll bar", "field"] {
            assert!(g.hits.iter().any(|h| h.label == want), "{want}");
        }
        assert_eq!(g.hits.iter().filter(|h| h.label.starts_with("sort ")).count(), 5, "a header per column");
        assert_eq!(v.loaded_rows()[0][0], "0");
        assert!(v.loaded_rows().len() >= 90, "a window of rows, not the file: {}", v.loaded_rows().len());
        assert!(v.loaded_rows().len() <= 400);
    }

    #[test]
    fn clicking_a_header_sorts_ascending_then_descending_then_off() {
        let (mut v, _d, _) = view("dv-sort", 500);
        let g = frame(&mut v, 1280, 700);
        click(&mut v, &g, "sort score");
        assert_eq!(v.sort_keys()[0].dir, Dir::Asc);
        let rows = v.loaded_rows();
        let s: Vec<f64> = rows.iter().take(50).map(|r| r[3].parse().unwrap()).collect();
        assert!(s.windows(2).all(|w| w[0] <= w[1]), "{s:?}");
        let g = frame(&mut v, 1280, 700);
        click(&mut v, &g, "sort score");
        assert_eq!(v.sort_keys()[0].dir, Dir::Desc);
        let s: Vec<f64> = v.loaded_rows().iter().take(50).map(|r| r[3].parse().unwrap()).collect();
        assert!(s.windows(2).all(|w| w[0] >= w[1]), "{s:?}");
        let g = frame(&mut v, 1280, 700);
        click(&mut v, &g, "sort score");
        assert!(v.sort_keys().is_empty());
        assert_eq!(v.loaded_rows()[0][0], "0", "back in file order");
        // a second column joins with Shift
        let g = frame(&mut v, 1280, 700);
        click(&mut v, &g, "sort group");
        crate::ui::set_shift_held(true);
        let g = frame(&mut v, 1280, 700);
        click(&mut v, &g, "sort score");
        crate::ui::set_shift_held(false);
        assert_eq!(v.sort_keys().len(), 2);
    }

    #[test]
    fn filter_applies_on_enter_and_a_bad_one_is_explained_without_losing_the_table() {
        let (mut v, _d, _) = view("dv-filter", 1000);
        let g = frame(&mut v, 1280, 700);
        let f = g.hits.iter().find(|h| h.label == "field").unwrap().ev.clone();
        // the shell turns the click into FocusAt with the position
        assert!(matches!(f, Event::Focus(_)));
        v.handle(&Event::FocusAt(FILTER_FIELD, 20.0));
        for c in "group = 'g3' and score > 50".chars() {
            v.handle(&Event::Key(Key::Char(c)));
        }
        v.handle(&Event::Key(Key::Enter));
        v.settle();
        let expect = (0..1000usize).filter(|i| i % 5 == 3 && (i * 37 % 1000) as f64 / 10.0 > 50.0).count() as u64;
        assert_eq!(v.total(), expect);
        assert!(v.loaded_rows().iter().all(|r| r[2] == "g3"));
        // a mistake
        v.handle(&Event::FocusAt(FILTER_FIELD, 20.0));
        v.handle(&Event::Key(Key::Ctrl('a')));
        for c in "scor > 5".chars() {
            v.handle(&Event::Key(Key::Char(c)));
        }
        v.handle(&Event::Key(Key::Enter));
        v.settle();
        let e = v.filter_error().expect("an error is shown");
        assert!(e.contains("no column \"scor\"") && e.contains("score"), "{e}");
        assert_eq!(v.total(), expect, "the last good filter still applies");
        let g = frame(&mut v, 1280, 700);
        assert!(!v.loaded_rows().is_empty());
        let _ = g;
        // clearing it shows everything again
        v.handle(&Event::Key(Key::Char('x')));
        v.settle();
        assert_eq!(v.total(), 1000);
    }

    #[test]
    fn a_million_rows_scroll_without_loading_them() {
        let d = std::fs::canonicalize(temp_dir("dv-big")).unwrap();
        let p = d.join("big.csv");
        {
            use std::io::Write;
            let mut w = std::io::BufWriter::new(std::fs::File::create(&p).unwrap());
            writeln!(w, "id,v").unwrap();
            for i in 0..1_000_000u32 {
                writeln!(w, "{i},{}", i % 977).unwrap();
            }
        }
        let t = std::time::Instant::now();
        let mut v = DataView::new(env_for(&d), DataViewParams { path: Some(p.to_string_lossy().into_owned()), filter: None });
        v.settle();
        assert_eq!(v.total(), 1_000_000);
        let mut g = frame(&mut v, 1280, 700);
        assert!(v.loaded_rows().len() <= 400, "only a window is held: {}", v.loaded_rows().len());
        // jump to the end with the scroll bar and with the keyboard
        let sb = g.hits.iter().find(|h| h.label == "scroll bar").unwrap().rect;
        v.handle(&Event::AreaAt(SCROLL_AREA, 3.0, sb.h - 1.0));
        v.settle();
        g = frame(&mut v, 1280, 700);
        assert!(v.top() > 999_900, "{}", v.top());
        assert_eq!(v.loaded_rows().last().unwrap()[0], "999999");
        v.handle(&Event::Key(Key::Nav { nav: Nav::Home, shift: false, ctrl: true }));
        v.settle();
        let _ = frame(&mut v, 1280, 700);
        assert_eq!(v.top(), 0);
        assert_eq!(v.loaded_rows()[0][0], "0");
        v.handle(&Event::Scroll(-10));
        assert_eq!(v.top(), 30);
        eprintln!("1M rows opened and scrolled in {:?}", t.elapsed());
        let _ = g;
    }

    #[test]
    fn cells_rows_and_selections_copy_to_the_clipboard() {
        let (mut v, _d, _) = view("dv-copy", 200);
        let g = frame(&mut v, 1280, 700);
        crate::ui::clipboard::clear();
        at(&mut v, &g, 2, 1);
        assert_eq!(v.cursor(), (2, 1));
        v.handle(&Event::Key(Key::Char('c')));
        assert_eq!(crate::ui::clipboard::latest().as_deref(), Some("user 2"));
        v.handle(&Event::Key(Key::Char('r')));
        assert_eq!(crate::ui::clipboard::latest().as_deref(), Some("2\tuser 2\tg2\t7.4\ttrue"));
        // a block with Shift+arrows, copied as tab-separated lines
        v.handle(&Event::Key(Key::Nav { nav: Nav::Down, shift: true, ctrl: false }));
        v.handle(&Event::Key(Key::Nav { nav: Nav::Right, shift: true, ctrl: false }));
        v.handle(&Event::Key(Key::Ctrl('c')));
        assert_eq!(crate::ui::clipboard::latest().as_deref(), Some("user 2\tg2\nuser 3\tg3"));
        // select all is capped
        v.handle(&Event::Key(Key::Ctrl('a')));
        v.handle(&Event::Key(Key::Ctrl('c')));
        let all = crate::ui::clipboard::latest().unwrap();
        assert_eq!(all.lines().count(), 200);
        assert!(v.status().unwrap().0.starts_with("Copied 200 rows × 5 columns"), "{:?}", v.status());
        // the context menu offers the same
        v.handle(&Event::Key(Key::Esc));
        let items = v.context_items(300, 200);
        let ids: Vec<&str> = items.iter().map(|i| i.id.as_str()).collect();
        assert!(ids.contains(&"data.cell") && ids.contains(&"data.sel") && ids.contains(&"data.filter"), "{ids:?}");
        assert!(v.context_pick("data.filter"));
        v.settle();
        assert!(v.filter_text().contains('='), "{}", v.filter_text());
        assert!(v.total() < 200);
    }

    #[test]
    fn selection_beyond_the_loaded_window_is_fetched_for_the_copy() {
        let (mut v, _d, _) = view("dv-copy2", 1200);
        let _ = frame(&mut v, 1280, 700);
        v.cursor = (0, 0);
        v.anchor = None;
        v.handle(&Event::Key(Key::Nav { nav: Nav::End, shift: true, ctrl: true }));
        v.handle(&Event::Key(Key::Ctrl('c')));
        let t = crate::ui::clipboard::latest().unwrap();
        assert_eq!(t.lines().count(), 1200, "the whole table, fetched for the copy");
        assert!(t.lines().last().unwrap().starts_with("1199\t"));
        // more than the clipboard keeps: said plainly
        let (mut big, _d2, _) = view("dv-copy3", 5000);
        let _ = frame(&mut big, 1280, 700);
        big.handle(&Event::Key(Key::Nav { nav: Nav::End, shift: true, ctrl: true }));
        big.handle(&Event::Key(Key::Ctrl('c')));
        assert!(big.status().unwrap().0.contains("clipboard keeps that much"), "{:?}", big.status());
    }

    #[test]
    fn export_asks_for_a_path_inside_the_write_scope_and_writes_the_view() {
        let (mut v, d, _) = view("dv-export", 300);
        v.handle(&Event::FocusAt(FILTER_FIELD, 20.0));
        for c in "group = 'g1'".chars() {
            v.handle(&Event::Key(Key::Char(c)));
        }
        v.handle(&Event::Key(Key::Enter));
        v.settle();
        assert_eq!(v.total(), 60);
        v.handle(&Event::Key(Key::Char('e')));
        let g = frame(&mut v, 1280, 700);
        assert!(g.hits.iter().any(|h| h.label.to_lowercase().contains("file path") || h.label == "field") || v.modal.is_some());
        // outside the scope: refused, the dialog stays with the reason
        if let Some(Modal::Path(f, _)) = &mut v.modal {
            f.set_value(0, "/etc/not-allowed.csv");
        }
        v.handle(&Event::Key(Key::Enter));
        match &v.modal {
            Some(Modal::Path(f, _)) => assert!(f.error.as_deref().unwrap_or("").contains("may not write"), "{:?}", f.error),
            _ => panic!("the dialog should stay open"),
        }
        let out = d.join("exports/g1.csv");
        if let Some(Modal::Path(f, _)) = &mut v.modal {
            f.set_value(0, &out.to_string_lossy());
        }
        v.handle(&Event::Key(Key::Enter));
        v.settle();
        assert!(v.modal.is_none());
        let text = std::fs::read_to_string(&out).unwrap();
        assert_eq!(text.lines().count(), 61, "header and the 60 filtered rows");
        assert!(text.lines().skip(1).all(|l| l.contains(",g1,")));
        assert!(v.status().unwrap().0.starts_with("Exported 60 rows"), "{:?}", v.status());
    }

    #[test]
    fn the_summary_pane_shows_statistics_and_follows_the_filter() {
        let (mut v, _d, _) = view("dv-summary", 400);
        v.handle(&Event::Key(Key::Char('s')));
        v.settle();
        let g = frame(&mut v, 1280, 700);
        let _ = g;
        let (_, sum) = v.summary.as_ref().expect("summary computed");
        assert_eq!(sum.rows, 400);
        assert_eq!(sum.columns.iter().find(|c| c.name == "id").unwrap().max, Some(serde_json::json!(399)));
        v.handle(&Event::FocusAt(FILTER_FIELD, 20.0));
        for c in "id < 10".chars() {
            v.handle(&Event::Key(Key::Char(c)));
        }
        v.handle(&Event::Key(Key::Enter));
        v.settle();
        let _ = frame(&mut v, 1280, 700);
        v.settle();
        assert_eq!(v.summary.as_ref().unwrap().1.rows, 10);
    }

    #[test]
    fn errors_are_shown_plainly_and_the_scope_is_enforced() {
        let d = std::fs::canonicalize(temp_dir("dv-err")).unwrap();
        let mut v = DataView::new(env_for(&d), DataViewParams::default());
        let g = frame(&mut v, 1280, 700);
        assert!(g.hits.iter().any(|h| h.label == "Open…"), "an empty viewer still has its Open button");
        v.open_path("/etc/passwd");
        assert!(v.status().unwrap().0.contains("outside your home folder"), "{:?}", v.status());
        v.open_path(&d.join("missing.csv").to_string_lossy());
        v.settle();
        assert!(v.status().unwrap().0.contains("no such file"), "{:?}", v.status());
        assert!(v.info().is_none());
        std::fs::write(d.join("empty.csv"), "").unwrap();
        v.open_path(&d.join("empty.csv").to_string_lossy());
        v.settle();
        assert!(v.status().unwrap().0.contains("empty"), "{:?}", v.status());
        let g = frame(&mut v, 900, 500);
        assert!(!g.hits.is_empty());
    }

    #[test]
    fn embedding_with_a_path_and_a_first_filter() {
        let d = std::fs::canonicalize(temp_dir("dv-embed")).unwrap();
        let f = sample(&d, 100);
        let mut v = DataView::new(env_for(&d), DataViewParams { path: Some(f), filter: Some("id >= 90".into()) });
        v.settle();
        assert_eq!(v.total(), 10);
        assert_eq!(v.filter_text(), "id >= 90");
        // it draws inside any rectangle, also a small tile
        for (w, h) in [(420u32, 260u32), (640, 360), (1920, 1080)] {
            let g = frame(&mut v, w, h);
            assert!(!g.hits.is_empty());
        }
        assert_eq!(v.handle(&Event::Key(Key::Esc)), Out::Close, "Esc with nothing to dismiss asks the host to close it");
    }

    fn writer(d: &Path) -> iyakki_sec::policy::Policy {
        let mut p = iyakki_sec::policy::Role::User.template();
        p.fs_read = vec![d.display().to_string()];
        p.fs_write = vec![d.display().to_string()];
        p
    }

    #[test]
    fn a_table_the_assistant_creates_is_listed_without_a_restart_and_an_append_reloads_the_open_table() {
        let d = std::fs::canonicalize(temp_dir("dv-agent-writes")).unwrap();
        let f = sample(&d, 5);
        let mut v = DataView::new(env_for(&d), DataViewParams { path: Some(f), filter: None });
        v.settle();
        v.show_open_dialog();
        let Some(Modal::Open { picker, .. }) = &v.modal else { panic!() };
        assert!(!picker.visible().into_iter().any(|i| i == "made.csv"));
        v.handle(&Event::Key(Key::Esc));
        let (p, u) = (writer(&d), "tester-dv-agent");
        let args = serde_json::json!({"name": "made.csv", "folder": d.display().to_string(), "columns": ["a", "b"], "rows": [[1, "x"], [2, "y"]]});
        iyakki_agent::datatools::call("data_create", &args, &p, u, &d, &d).unwrap();
        v.show_open_dialog();
        let Some(Modal::Open { picker, .. }) = &v.modal else { panic!() };
        assert!(picker.visible().into_iter().any(|i| i == "made.csv"), "the dialog reads the folder each time it opens");
        v.handle(&Event::Key(Key::Esc));
        // rows added to the open file by the assistant: the open table reloads by itself
        assert_eq!(v.total(), 5);
        assert!(!v.reload_if_changed(), "nothing changed yet");
        let row = serde_json::json!({"path": d.join("sample.csv").display().to_string(), "rows": [[5, "user 5", "g0", 1.5, true]]});
        iyakki_agent::datatools::call("data_append", &row, &p, u, &d, &d).unwrap();
        assert!(v.reload_if_changed());
        v.settle();
        assert_eq!(v.total(), 6);
        iyakki_agent::datatools::forget(u);
    }

    #[test]
    fn the_assistant_is_told_the_open_table_by_file_and_column_names_never_cells() {
        let (mut v, _d, f) = view("dv-tell", 5);
        v.settle();
        let mut app = DataApp { view: v, user: "tester-dv-tell".into(), told: None };
        assert!(iyakki_agent::datatools::tab_context("tester-dv-tell").is_none());
        app.tell_assistant();
        let c = iyakki_agent::datatools::tab_context("tester-dv-tell").unwrap();
        assert!(c.contains(&f) && c.contains("id, name, group, score, paid"), "{c}");
        assert!(!c.contains("user 0") && !c.contains("g1"), "no cell values: {c}");
        iyakki_agent::datatools::forget("tester-dv-tell");
    }

    #[test]
    fn the_open_dialog_lists_tables_and_folders_only() {
        let d = std::fs::canonicalize(temp_dir("dv-dialog")).unwrap();
        sample(&d, 5);
        std::fs::write(d.join("notes.txt.bak"), "x").unwrap();
        std::fs::write(d.join("pic.png"), "x").unwrap();
        std::fs::create_dir_all(d.join("sub")).unwrap();
        let mut v = DataView::new(env_for(&d), DataViewParams::default());
        v.open_dialog(d.clone());
        let Some(Modal::Open { picker, .. }) = &v.modal else { panic!() };
        let items: Vec<String> = picker.visible().into_iter().cloned().collect();
        assert!(items.contains(&"sample.csv".to_string()) && items.contains(&"sub/".to_string()) && items.contains(&"Type a path…".to_string()), "{items:?}");
        assert!(!items.iter().any(|i| i.contains("pic.png") || i.contains(".bak")), "{items:?}");
        v.handle(&Event::Key(Key::Down));
        v.handle(&Event::Key(Key::Esc));
        assert!(v.modal.is_none());
    }

    #[test]
    fn the_open_dialog_starts_in_home_stays_inside_hides_dot_folders_and_has_quick_links_and_recents() {
        let base = std::fs::canonicalize(temp_dir("dv-home")).unwrap();
        let home = base.join("home/amy");
        let outside = base.join("elsewhere");
        for d in ["Documents", "Downloads", ".config", "work/deep", "empty"] {
            std::fs::create_dir_all(home.join(d)).unwrap();
        }
        std::fs::create_dir_all(&outside).unwrap();
        std::fs::write(outside.join("secret.csv"), "a\n1\n").unwrap();
        std::fs::write(home.join("Documents/q.csv"), "a,b\n1,2\n").unwrap();
        std::fs::write(home.join("top.csv"), "a\n1\n").unwrap();
        std::os::unix::fs::symlink(&outside, home.join("leak")).unwrap();
        std::os::unix::fs::symlink(outside.join("secret.csv"), home.join("leak.csv")).unwrap();
        let (h, o) = (home.to_string_lossy().into_owned(), outside.to_string_lossy().into_owned());
        let env = DataEnv { svc: iyakki_data::builtin_service(), can_read: Arc::new(|_| true), can_write: Arc::new(|_| true), roots: { let r = iyakki_agent::scope::FileRoots { home: home.clone(), roots: vec![home.clone()], mounts: vec![] }; Arc::new(move || r.clone()) }, recent: Some(base.join("recent.json")), host: None };
        let mut v = DataView::new(env, DataViewParams::default());
        let items = |v: &DataView| -> Vec<String> {
            let Some(Modal::Open { picker, .. }) = &v.modal else { panic!("no dialog") };
            picker.visible().into_iter().cloned().collect()
        };
        v.show_open_dialog();
        let it = items(&v);
        assert!(it.contains(&"top.csv".to_string()) && it.contains(&"work/".to_string()) && it.contains(&"Documents/".to_string()), "{it:?}");
        assert!(!it.contains(&"../".to_string()), "no way up from the top of home: {it:?}");
        assert!(!it.iter().any(|i| i.contains(".config") || i.contains("leak")), "dot folders and links out of home are hidden: {it:?}");
        assert!(it.contains(&"★ Home".to_string()) && it.contains(&"★ Documents".to_string()) && it.contains(&"★ Downloads".to_string()) && !it.contains(&"★ Workspaces".to_string()), "{it:?}");
        // show hidden folders
        let toggle = it.iter().find(|i| i.starts_with('☐')).unwrap().clone();
        let idx = v.modal.as_ref().map(|m| if let Modal::Open { picker, .. } = m { picker.visible().iter().position(|i| **i == toggle).unwrap() } else { 0 }).unwrap();
        v.handle(&Event::Row(idx));
        assert!(items(&v).contains(&".config/".to_string()));
        // a quick link, then up and down; `..` never leaves home
        let idx = items(&v).iter().position(|i| i == "★ Documents").unwrap();
        v.handle(&Event::Row(idx));
        let it = items(&v);
        assert!(it.contains(&"q.csv".to_string()) && it.contains(&"../".to_string()), "{it:?}");
        let up = it.iter().position(|i| i == "../").unwrap();
        v.handle(&Event::Row(up));
        assert!(!items(&v).contains(&"../".to_string()), "back at the top of home");
        // an open dialog asked to show a folder outside falls back to home
        v.open_dialog(outside.clone());
        assert!(items(&v).contains(&"top.csv".to_string()));
        // typed paths: inside is fine (also with ~), outside gets a plain sentence, links do not escape
        v.modal = None;
        for bad in [format!("{o}/secret.csv"), "/etc/passwd".to_string(), "../../elsewhere/secret.csv".to_string(), "leak.csv".to_string(), "leak/secret.csv".to_string(), "~/../../elsewhere/secret.csv".to_string()] {
            v.open_path(&bad);
            assert!(v.status().unwrap().0.contains("outside your home folder"), "{bad}: {:?}", v.status());
            assert!(v.info().is_none() && !v.busy(), "{bad}");
        }
        v.open_dialog(home.clone());
        let idx = items(&v).iter().position(|i| i == "Type a path…").unwrap();
        v.handle(&Event::Row(idx));
        let Some(Modal::Path(f, _)) = &mut v.modal else { panic!() };
        f.set_value(0, "/etc/passwd");
        v.handle(&Event::Key(Key::Enter));
        match &v.modal {
            Some(Modal::Path(f, _)) => assert!(f.error.as_deref().unwrap_or("").contains("outside your home folder"), "{:?}", f.error),
            _ => panic!("the form stays with the reason"),
        }
        if let Some(Modal::Path(f, _)) = &mut v.modal {
            f.set_value(0, "~/Documents/q.csv");
        }
        v.handle(&Event::Key(Key::Enter));
        v.settle();
        assert_eq!(v.total(), 1, "{:?}", v.status());
        assert!(v.path().unwrap().ends_with("Documents/q.csv"));
        // recents
        v.open_path("top.csv");
        v.settle();
        v.show_open_dialog();
        let it = items(&v);
        let rec: Vec<&String> = it.iter().filter(|i| i.starts_with('↺')).collect();
        assert_eq!(rec.len(), 2, "{it:?}");
        assert!(rec[0].ends_with("top.csv") && rec[1].ends_with("Documents/q.csv"), "newest first: {rec:?}");
        let _ = (h, &o);
    }

    fn host_env(base: &Path, tag: &str) -> (DataEnv, PathBuf, PathBuf, Arc<std::sync::Mutex<Vec<String>>>) {
        let (host, data, acct) = (base.join("host"), base.join("data"), base.join("acct"));
        for d in [&host, &data, &acct] {
            std::fs::create_dir_all(d).unwrap();
        }
        let host = std::fs::canonicalize(host).unwrap();
        let acct = std::fs::canonicalize(acct).unwrap();
        let _ = tag;
        iyakki_agent::hostfiles::set_enabled(true);
        iyakki_agent::hostfiles::set_host_home(Some(host.clone()));
        std::fs::write(data.join("host-consent-amy"), "agreed\n").unwrap();
        let log = Arc::new(std::sync::Mutex::new(Vec::new()));
        let l2 = log.clone();
        let mut policy = iyakki_sec::policy::Role::User.template();
        policy.fs_read = vec![acct.display().to_string()];
        policy.fs_write = vec![acct.display().to_string()];
        let (p1, p2, p3) = (policy.clone(), policy.clone(), policy.clone());
        let (d1, d2, d3, a1) = (data.clone(), data.clone(), data.clone(), acct.clone());
        let env = DataEnv {
            svc: iyakki_data::builtin_service(),
            can_read: Arc::new(move |path| iyakki_agent::scope::FileRoots::with_home(&p1, "amy", &d1, a1.clone()).read_ok(&p1, "amy", Path::new(path))),
            can_write: { let a = acct.clone(); Arc::new(move |path| iyakki_agent::scope::FileRoots::with_home(&p2, "amy", &d2, a.clone()).write_ok(&p2, "amy", Path::new(path))) },
            roots: { let a = acct.clone(); Arc::new(move || iyakki_agent::scope::FileRoots::with_home(&p3, "amy", &d3, a.clone())) },
            recent: None,
            host: Some(crate::apps::hostfiles::HostEnv { data_dir: data.clone(), user: "amy".into(), dest: acct.join("Uploads"), dest_label: "your Uploads folder".into(), audit: Arc::new(move |a, d| l2.lock().unwrap().push(format!("{a} {d}"))) }),
        };
        (env, host, acct, log)
    }

    #[test]
    fn a_table_dropped_on_the_window_is_copied_into_the_account_and_opened() {
        let _g = iyakki_agent::hostfiles::test_guard();
        let base = std::fs::canonicalize(temp_dir("dv-drop")).unwrap();
        let (env, host, acct, log) = host_env(&base, "drop");
        std::fs::create_dir_all(host.join("Downloads")).unwrap();
        std::fs::write(host.join("Downloads/prices.csv"), "item,price\ntea,2.5\ncoffee,3\n").unwrap();
        let mut v = DataView::new(env, DataViewParams::default());
        assert!(v.host_ready());
        assert!(v.drop_files(vec![host.join("Downloads/prices.csv")]));
        for _ in 0..200 {
            v.handle(&Event::Tick);
            if v.info().is_some() || v.total() > 0 {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        v.settle();
        assert!(acct.join("Uploads/prices.csv").exists(), "copied into the account");
        assert_eq!(v.total(), 2, "{:?}", v.status());
        assert!(v.path().unwrap().ends_with("Uploads/prices.csv"), "the copy is what is open, not the host file");
        assert_eq!(std::fs::read_to_string(host.join("Downloads/prices.csv")).unwrap(), "item,price\ntea,2.5\ncoffee,3\n", "the host file is untouched");
        assert!(log.lock().unwrap().iter().any(|l| l.starts_with("host.import")), "{:?}", log.lock().unwrap());
        // the dialog and the toolbar draw
        let g = frame(&mut v, 1280, 700);
        for want in ["Host files…", "Save to host…"] {
            assert!(g.hits.iter().any(|h| h.label == want), "{want}");
        }
        iyakki_agent::hostfiles::set_host_home(None);
    }

    #[test]
    fn a_table_in_a_shared_host_folder_opens_in_place_and_closes_when_the_share_is_removed() {
        let _g = iyakki_agent::hostfiles::test_guard();
        let base = std::fs::canonicalize(temp_dir("dv-mount")).unwrap();
        let (env, host, _acct, _log) = host_env(&base, "mount");
        std::fs::create_dir_all(host.join("Documents/sheets")).unwrap();
        std::fs::write(host.join("Documents/sheets/t.csv"), "a,b\n1,x\n2,y\n3,z\n").unwrap();
        let data = base.join("data");
        let mut v = DataView::new(env, DataViewParams::default());
        // before the grant the host file is out of reach
        v.open_path(&host.join("Documents/sheets/t.csv").to_string_lossy());
        assert!(v.status().unwrap().0.contains("outside"), "{:?}", v.status());
        let mut m = iyakki_agent::hostfiles::HostMounts::load(&data, "amy");
        m.add(&host, "~/Documents/sheets", None, false).unwrap();
        v.open_path("host:sheets/t.csv");
        v.settle();
        assert_eq!(v.total(), 3, "{:?}", v.status());
        assert!(v.path().unwrap().ends_with("Documents/sheets/t.csv"), "read in place, no copy");
        assert_eq!(v.host_chip().as_deref(), Some("Host access: 1 folder"));
        // the quick links list it
        v.show_open_dialog();
        let Some(Modal::Open { picker, .. }) = &v.modal else { panic!() };
        assert!(picker.visible().iter().any(|i| i.as_str() == "★ host:sheets"), "{:?}", picker.visible());
        v.modal = None;
        // removing the share closes the table at the next check
        m.remove("sheets").unwrap();
        for _ in 0..40 {
            v.handle(&Event::Tick);
        }
        assert!(v.info().is_none(), "the table was closed");
        assert!(v.status().unwrap().0.contains("Access to") && v.status().unwrap().0.contains("was removed"), "{:?}", v.status());
        assert_eq!(v.host_chip(), None);
        iyakki_agent::hostfiles::set_host_home(None);
    }


    #[test]
    fn the_app_is_a_tile_that_hosts_the_view() {
        let cx = cx_with_admin_store("dv-app");
        let mut app = DataApp::new(&cx);
        assert_eq!((app.title(), app.icon()), ("Data", Icon::Data));
        let mut g = Gfx::new(1280, 700);
        g.clear(rgb(0, 0, 0));
        app.render(&mut g, R::new(0.0, 72.0, 1280.0, 628.0), 1.0, &cx, 0);
        assert!(g.hits.iter().any(|h| h.label == "Open…"));
        assert!(app.hint().contains("click a header to sort"));
        assert!(matches!(app.handle(&Event::Key(Key::Esc), &cx), Flow::Back));
        // the IDE hands a file over this way
        let d = std::fs::canonicalize(temp_dir("dv-app2")).unwrap();
        let f = sample(&d, 20);
        request_open(&f, Some("id < 5"));
        let mut cx2 = cx.clone();
        cx2.session.policy.fs_read = vec![d.to_string_lossy().into_owned()];
        let mut app = DataApp::new(&cx2);
        app.view().settle();
        assert_eq!(app.view().total(), 5);
        assert!(OPEN_NEXT.with(|o| o.borrow().is_none()), "the request is used once");
    }
}
