//! The AI IDE's bottom panel: a command runner in the workspace folder (TERMINAL), the project's test command with
//! clickable `file:line` messages (PROBLEMS), and a text search over the project (SEARCH).
//!
//! Commands are run without a shell (a program and its arguments, see `iyakki_agent::wsexec`), one at a time, with a
//! time limit and a cap on the output, in the workspace folder. `cd` stays inside the workspace.

use std::path::{Path, PathBuf};
use std::sync::mpsc::{channel, Receiver};
use std::time::{Duration, Instant};

use iyakki_agent::workspace::{self, Hit};
use iyakki_agent::wsexec::{self, Out};
use serde::{Deserialize, Serialize};

use super::{ScrollLog, LOG_AREA};
use crate::fx::{self, with_alpha, AMBER, CYAN};
use crate::gfx::{rgb, Face, Gfx};
use crate::textsel;
use crate::ui::{Event, Key, TextField, DIM, ERR, MUTED, OK, R, TEXT};

pub const TERM_AREA: usize = 62_001;
pub const FOCUS_TERM: usize = 9_500_001;
pub const FOCUS_SEARCH: usize = 9_500_002;
pub const PROBLEM_ROW: usize = 400_000;
pub const SEARCH_ROW: usize = 300_000;
/// `Event::Sub` code of the bottom panel's buttons and keys.
pub const SUB: u8 = 34;
pub const MAX_RUN: Duration = Duration::from_secs(300);
pub const MAX_OUTPUT: usize = 256 * 1024;
const _: usize = LOG_AREA;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Tab {
    Terminal,
    K8s,
    Problems,
    Search,
}

impl Tab {
    pub const ALL: [Tab; 4] = [Tab::Terminal, Tab::K8s, Tab::Problems, Tab::Search];
    pub fn label(self) -> &'static str {
        match self {
            Tab::Terminal => "TERMINAL",
            Tab::K8s => "K8S",
            Tab::Problems => "PROBLEMS",
            Tab::Search => "SEARCH",
        }
    }
}

/// A `file:line` found in a line of tool output, resolved to a file of the workspace.
pub fn parse_location(root: &Path, line: &str) -> Option<(String, usize)> {
    // Python: File "path", line N
    if let Some(i) = line.find("File \"") {
        let rest = &line[i + 6..];
        let (path, tail) = rest.split_once('"')?;
        let n = tail.split("line ").nth(1)?.split(|c: char| !c.is_ascii_digit()).next()?.parse().ok()?;
        return check(root, path, n);
    }
    // Rust and compilers: --> path:line:col, path:line:col: message, (path:line:col) in JavaScript stacks
    let cleaned: String = line.replace(['(', ')', '[', ']', '<', '>', '\'', '"'], " ");
    for tok in cleaned.split_whitespace() {
        let tok = tok.trim_end_matches([':', ',']);
        let mut parts = tok.rsplitn(3, ':').collect::<Vec<_>>();
        parts.reverse();
        // path:line or path:line:col
        let cand = match parts.len() {
            3 if parts[1].chars().all(|c| c.is_ascii_digit()) && parts[2].chars().all(|c| c.is_ascii_digit()) && !parts[1].is_empty() => Some((parts[0], parts[1])),
            2 if parts[1].chars().all(|c| c.is_ascii_digit()) && !parts[1].is_empty() => Some((parts[0], parts[1])),
            _ => None,
        };
        if let Some((p, n)) = cand {
            if p.contains('.') || p.contains('/') {
                if let Some(r) = check(root, p, n.parse().ok()?) {
                    return Some(r);
                }
            }
        }
    }
    None
}

fn check(root: &Path, path: &str, line: usize) -> Option<(String, usize)> {
    let p = path.trim_start_matches("./");
    let real = workspace::resolve_in(root, p).ok()?;
    if !real.is_file() {
        return None;
    }
    Some((workspace::rel_of(root, &real), line.max(1)))
}

fn line_color(l: &str) -> [u8; 4] {
    let t = l.to_lowercase();
    if t.starts_with("error") || t.contains("error[") || t.contains("failed") || t.contains("panicked") || t.contains("traceback") || t.contains("assertionerror") {
        ERR
    } else if t.starts_with("warning") || t.contains("warning:") {
        AMBER
    } else if t.contains("test result: ok") || t.starts_with("ok") || t.contains(" passed") || t.ends_with("... ok") {
        OK
    } else {
        [0, 0, 0, 0]
    }
}

// ----------------------------------------------------------------------------------------------------- the runner

static CLI_BIN: std::sync::Mutex<Option<PathBuf>> = std::sync::Mutex::new(None);

/// The CLI tile's folder of installed tools (kubectl, helm, claude, ...): the terminal and the run/test commands find them
/// on `PATH` too, ahead of the system's.
pub fn set_cli_bin(dir: PathBuf) {
    *CLI_BIN.lock().unwrap_or_else(|e| e.into_inner()) = Some(dir);
}

/// `PATH` with the CLI tile's folder in front, when it exists.
pub fn path_env() -> Vec<(String, String)> {
    let Some(bin) = CLI_BIN.lock().unwrap_or_else(|e| e.into_inner()).clone().filter(|b| b.is_dir()) else { return Vec::new() };
    let old = std::env::var("PATH").unwrap_or_default();
    if old.split(':').any(|d| Path::new(d) == bin) {
        return Vec::new();
    }
    vec![("PATH".to_string(), if old.is_empty() { bin.display().to_string() } else { format!("{}:{old}", bin.display()) })]
}

/// One command at a time in the workspace.
pub struct Runner {
    rx: Option<Receiver<Out>>,
    pub started: Option<Instant>,
    pub name: String,
}

impl Runner {
    pub fn new() -> Runner {
        Runner { rx: None, started: None, name: String::new() }
    }

    pub fn busy(&self) -> bool {
        self.rx.is_some()
    }

    pub fn start(&mut self, cwd: &Path, argv: Vec<String>, timeout: Duration) {
        let (tx, rx) = channel();
        let cwd = cwd.to_path_buf();
        self.name = argv.join(" ");
        self.started = Some(Instant::now());
        self.rx = Some(rx);
        // a command that would start, stop or install a cluster is not run while one is connected (or in a desktop window)
        if let Err(why) = iyakki_agent::devcluster::guard(argv.first().map(String::as_str).unwrap_or(""), argv.get(1..).unwrap_or(&[])) {
            let _ = tx.send(Out { code: Some(126), stderr: why, ..Default::default() });
            return;
        }
        let env = path_env();
        let _ = std::thread::Builder::new().name("iyakki-ide-run".into()).spawn(move || {
            let _ = tx.send(wsexec::run_argv(&cwd, &argv, &env, timeout, MAX_OUTPUT));
        });
    }

    pub fn poll(&mut self) -> Option<(Out, Duration)> {
        let rx = self.rx.as_ref()?;
        match rx.try_recv() {
            Ok(o) => {
                self.rx = None;
                Some((o, self.started.take().map(|s| s.elapsed()).unwrap_or_default()))
            }
            Err(std::sync::mpsc::TryRecvError::Empty) => None,
            Err(_) => {
                self.rx = None;
                Some((Out { stderr: "the command stopped without an answer".into(), ..Default::default() }, Duration::default()))
            }
        }
    }

    /// Give up waiting (the command keeps running until its time limit, but its output is dropped).
    pub fn abandon(&mut self) {
        self.rx = None;
        self.started = None;
    }
}

impl Default for Runner {
    fn default() -> Self {
        Self::new()
    }
}

// ------------------------------------------------------------------------------------------------------ terminal

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct TermSaved {
    pub cwd: String,
    pub history: Vec<String>,
}

pub struct Term {
    pub root: PathBuf,
    /// Working folder below the root (empty = the root).
    pub cwd: String,
    pub log: ScrollLog,
    pub input: TextField,
    pub history: Vec<String>,
    hist_pos: Option<usize>,
    pub runner: Runner,
    allowed: bool,
    /// The workspace's chosen cluster: (kubeconfig, context, server, workspace name). `kubectl` and `helm` typed here are pinned to it.
    pub kube: Option<(String, String, String, String)>,
}

impl Term {
    pub fn new(root: &Path, saved: &TermSaved, allowed: bool) -> Term {
        let mut log = ScrollLog::default();
        log.pane = textsel::Pane::new(TERM_AREA);
        let mut t = Term { root: root.into(), cwd: String::new(), log, input: TextField::new("type a command and press Enter", false), history: saved.history.clone(), hist_pos: None, runner: Runner::new(), allowed, kube: None };
        if !saved.cwd.is_empty() && workspace::resolve_in(root, &saved.cwd).is_ok_and(|p| p.is_dir()) {
            t.cwd = saved.cwd.clone();
        }
        t.log.push(format!("workspace terminal · {}", t.dir().display()), DIM);
        if !allowed {
            t.log.push("Running programs needs the full command-line permission for your account. The file tree, the editor and the assistant still work.", AMBER);
        }
        t
    }

    pub fn set_allowed(&mut self, on: bool) {
        self.allowed = on;
    }

    pub fn saved(&self) -> TermSaved {
        TermSaved { cwd: self.cwd.clone(), history: self.history.iter().rev().take(200).rev().cloned().collect() }
    }

    pub fn dir(&self) -> PathBuf {
        workspace::resolve_in(&self.root, &self.cwd).unwrap_or_else(|_| self.root.clone())
    }

    pub fn prompt(&self) -> String {
        format!("{}$", if self.cwd.is_empty() { "~".to_string() } else { format!("~/{}", self.cwd) })
    }

    pub fn run_line(&mut self, line: &str) {
        let line = line.trim();
        if line.is_empty() {
            return;
        }
        self.log.push(format!("{} {line}", self.prompt()), fx::CYAN);
        if self.history.last().map(String::as_str) != Some(line) {
            self.history.push(line.to_string());
        }
        self.hist_pos = None;
        let argv = wsexec::split_command(line);
        match argv.first().map(String::as_str) {
            Some("clear") => {
                self.log.clear();
                return;
            }
            Some("help") => {
                self.log.push("Runs one program with its arguments in the workspace folder (no pipes or shell syntax). Built in: cd <folder> · pwd · clear · help. Up/Down recall earlier commands.", DIM);
                return;
            }
            Some("pwd") => {
                let d = self.dir();
                self.log.push(d.display().to_string(), [0, 0, 0, 0]);
                return;
            }
            Some("cd") => {
                let target = argv.get(1).map(String::as_str).unwrap_or("");
                let rel = if target.is_empty() || target == "~" { String::new() } else if target.starts_with('/') { target.to_string() } else if self.cwd.is_empty() { target.to_string() } else { format!("{}/{}", self.cwd, target) };
                match workspace::resolve_in(&self.root, &rel) {
                    Ok(p) if p.is_dir() => self.cwd = workspace::rel_of(&self.root, &p).trim_matches('/').to_string(),
                    Ok(_) => self.log.push(format!("cd: {target}: not a folder"), ERR),
                    Err(e) => self.log.push(format!("cd: {e}"), ERR),
                }
                return;
            }
            _ => {}
        }
        if !self.allowed {
            self.log.push("This account may not run programs (the command-line permission is below Full).", ERR);
            return;
        }
        if self.runner.busy() {
            self.log.push("A command is still running; wait for it to end (Esc stops waiting).", AMBER);
            return;
        }
        let mut argv = argv;
        if let Some((kc, ctx, server, ws)) = self.kube.clone() {
            if let Some(pinned) = iyakki_agent::devcluster::pin_cluster_args(&argv[0], &argv[1..], &kc, &ctx) {
                self.log.push(format!("{} → {} (workspace {ws})", argv[0], if server.is_empty() { "cluster".to_string() } else { server }), fx::CYAN);
                let prog = argv[0].clone();
                argv = std::iter::once(prog).chain(pinned).collect();
            }
        }
        self.runner.start(&self.dir(), argv, MAX_RUN);
    }

    pub fn tick(&mut self) -> bool {
        self.log.on_tick();
        let Some((o, took)) = self.runner.poll() else { return false };
        self.print(&o, took);
        true
    }

    fn print(&mut self, o: &Out, took: Duration) {
        for l in o.stdout.lines().chain(o.stderr.lines()).take(2000) {
            self.log.push(l.chars().take(500).collect::<String>(), line_color(l));
        }
        if o.missing {
            self.log.push("command not found", ERR);
        } else if o.timed_out {
            self.log.push(format!("stopped after {} s (the time limit)", took.as_secs()), ERR);
        } else if o.truncated {
            self.log.push("(output cut at 256 KB)", AMBER);
        }
        if let Some(c) = o.code {
            self.log.push(format!("exit {c} · {:.1} s", took.as_secs_f32()), if c == 0 { OK } else { ERR });
        }
    }

    pub fn handle(&mut self, ev: &Event) -> bool {
        if self.log.handle(ev) {
            return true;
        }
        match ev {
            Event::Key(Key::Enter) => {
                let l = self.input.take();
                self.run_line(&l);
            }
            Event::Key(Key::Up) => {
                if self.history.is_empty() {
                    return true;
                }
                let p = self.hist_pos.map_or(self.history.len() - 1, |p| p.saturating_sub(1));
                self.hist_pos = Some(p);
                let h = self.history[p].clone();
                self.input.set(&h);
            }
            Event::Key(Key::Down) => {
                if let Some(p) = self.hist_pos {
                    if p + 1 < self.history.len() {
                        self.hist_pos = Some(p + 1);
                        let h = self.history[p + 1].clone();
                        self.input.set(&h);
                    } else {
                        self.hist_pos = None;
                        self.input.clear();
                    }
                }
            }
            Event::Key(Key::Esc) => {
                if self.runner.busy() {
                    self.runner.abandon();
                    self.log.push("stopped waiting", AMBER);
                } else if !self.input.value.is_empty() {
                    self.input.clear();
                } else {
                    return false;
                }
            }
            Event::Key(Key::PageUp) => self.log.scroll_by(8),
            Event::Key(Key::PageDown) => self.log.scroll_by(-8),
            Event::Key(Key::Ctrl('l')) => self.log.clear(),
            Event::Key(Key::Ctrl('c')) => {
                if let Some(t) = self.log.pane.selected_text() {
                    crate::ui::clipboard::copy(&t);
                } else if self.input.selection().is_some() {
                    self.input.handle(&Key::Ctrl('c'));
                } else if self.runner.busy() {
                    self.runner.abandon();
                    self.log.push("^C (stopped waiting)", AMBER);
                }
            }
            Event::Key(k) => {
                self.input.handle(k);
            }
            Event::Paste(t) => {
                self.input.paste(t);
            }
            Event::FocusAt(FOCUS_TERM, x) => self.input.click(*x),
            Event::Scroll(d) => self.log.scroll_by(*d * 3),
            _ => return false,
        }
        true
    }

    pub fn draw(&mut self, g: &mut Gfx, r: R, s: f32, focused: bool, accent: [u8; 4], blink: bool) {
        let ih = 30.0 * s;
        let log_r = R::new(r.x + 12.0 * s, r.y + 4.0 * s, r.w - 24.0 * s, (r.h - ih - 10.0 * s).max(10.0));
        self.log.render(g, log_r, 13.0 * s, Face::Mono);
        let ir = R::new(r.x + 8.0 * s, r.y + r.h - ih - 2.0 * s, r.w - 16.0 * s, ih);
        g.rrect(ir.x, ir.y, ir.w, ir.h, 8.0 * s, rgb(12, 16, 34));
        g.rrect_outline(ir.x, ir.y, ir.w, ir.h, 8.0 * s, if focused { 1.6 * s } else { 1.0 }, if focused { accent } else { rgb(50, 60, 92) });
        let prompt = self.prompt();
        let pw = g.text_width(13.0 * s, &prompt, Face::Mono) + 8.0 * s;
        g.text(ir.x + 10.0 * s, ir.y + 7.0 * s, 13.0 * s, CYAN, &prompt, Face::Mono);
        let tr = R::new(ir.x + 10.0 * s + pw, ir.y + 7.0 * s, ir.w - pw - 24.0 * s, ih - 10.0 * s);
        if self.runner.busy() {
            let t = format!("running {} … ({} s)  Esc stops waiting", self.runner.name, self.runner.started.map_or(0, |s| s.elapsed().as_secs()));
            let t = g.fit(13.0 * s, &t, Face::Mono, tr.w);
            g.text(tr.x, tr.y, 13.0 * s, AMBER, &t, Face::Mono);
        } else {
            self.input.render_bare(g, tr, focused, s * 0.82, blink, Face::Mono, FOCUS_TERM);
        }
    }
}

// ---------------------------------------------------------------------------------------------------------- problems

#[derive(Clone, Debug, PartialEq)]
pub struct Line {
    pub text: String,
    pub color: [u8; 4],
    pub loc: Option<(String, usize)>,
}

pub struct Problems {
    pub root: PathBuf,
    pub command: String,
    pub lines: Vec<Line>,
    pub sel: usize,
    scroll: usize,
    visible: usize,
    pub runner: Runner,
    pub summary: Option<(String, bool)>,
}

impl Problems {
    pub fn new(root: &Path, command: &str) -> Problems {
        Problems { root: root.into(), command: command.into(), lines: vec![], sel: 0, scroll: 0, visible: 8, runner: Runner::new(), summary: None }
    }

    pub fn run(&mut self, allowed: bool) -> Result<(), String> {
        if self.command.trim().is_empty() {
            return Err("No test command is set. Add \"commands\": {\"test\": \"cargo test\"} to .iyakki/project.json in this workspace.".into());
        }
        if !allowed {
            return Err("Running the tests needs the full command-line permission for your account.".into());
        }
        if self.runner.busy() {
            return Err("The tests are already running.".into());
        }
        self.lines.clear();
        self.summary = None;
        self.sel = 0;
        self.runner.start(&self.root, wsexec::split_command(&self.command), MAX_RUN);
        Ok(())
    }

    pub fn tick(&mut self) -> bool {
        let Some((o, took)) = self.runner.poll() else { return false };
        self.load_output(&o, took);
        true
    }

    pub fn load_output(&mut self, o: &Out, took: Duration) {
        self.lines.clear();
        for l in o.stdout.lines().chain(o.stderr.lines()).take(4000) {
            let text: String = l.chars().take(400).collect();
            let loc = parse_location(&self.root, &text);
            let color = if loc.is_some() && line_color(&text) == [0, 0, 0, 0] { CYAN } else { line_color(&text) };
            self.lines.push(Line { text, color, loc });
        }
        let errors = self.lines.iter().filter(|l| l.color == ERR).count();
        let msg = if o.missing {
            format!("`{}` was not found on this machine", self.command)
        } else if o.timed_out {
            format!("stopped after {} s (time limit)", took.as_secs())
        } else {
            format!("{} · {} lines · {} flagged · {:.1} s", if o.ok() { "passed" } else { "FAILED" }, self.lines.len(), errors, took.as_secs_f32())
        };
        self.summary = Some((msg, o.ok()));
        // the result of every test, for the board (stored per workspace)
        let _ = iyakki_agent::testresults::record_run(&self.root, &o.stdout, &o.stderr, iyakki_agent::clock::now_unix());
        // the first problem with a location is selected
        self.sel = self.lines.iter().position(|l| l.loc.is_some() && l.color == ERR).or_else(|| self.lines.iter().position(|l| l.loc.is_some())).unwrap_or(self.lines.len().saturating_sub(1));
        self.scroll = self.sel.saturating_sub(2);
    }

    /// The location of the selected line.
    pub fn selected_loc(&self) -> Option<(String, usize)> {
        self.lines.get(self.sel).and_then(|l| l.loc.clone())
    }

    /// Next line with a location (`dir` 1 or -1).
    pub fn step_problem(&mut self, dir: i32) {
        let n = self.lines.len() as i32;
        let mut i = self.sel as i32 + dir;
        while i >= 0 && i < n {
            if self.lines[i as usize].loc.is_some() {
                self.sel = i as usize;
                return;
            }
            i += dir;
        }
    }

    pub fn handle(&mut self, ev: &Event) -> Option<(String, usize)> {
        let n = self.lines.len();
        match ev {
            Event::Key(Key::Up) => self.sel = self.sel.saturating_sub(1),
            Event::Key(Key::Down) => self.sel = (self.sel + 1).min(n.saturating_sub(1)),
            Event::Key(Key::PageUp) => self.sel = self.sel.saturating_sub(self.visible),
            Event::Key(Key::PageDown) => self.sel = (self.sel + self.visible).min(n.saturating_sub(1)),
            Event::Key(Key::Char('n')) => self.step_problem(1),
            Event::Key(Key::Char('p')) => self.step_problem(-1),
            Event::Key(Key::Enter) => return self.selected_loc(),
            Event::Key(Key::Ctrl('c')) => {
                if let Some(l) = self.lines.get(self.sel) {
                    crate::ui::clipboard::copy(&l.text);
                }
            }
            Event::Row(r) if (PROBLEM_ROW..PROBLEM_ROW + 100_000).contains(r) => {
                self.sel = (r - PROBLEM_ROW).min(n.saturating_sub(1));
                return self.selected_loc();
            }
            Event::Scroll(d) => self.scroll = (self.scroll as i32 - d * 3).clamp(0, n.saturating_sub(self.visible) as i32) as usize,
            _ => {}
        }
        if self.sel < self.scroll {
            self.scroll = self.sel;
        } else if self.sel >= self.scroll + self.visible {
            self.scroll = self.sel + 1 - self.visible;
        }
        None
    }

    pub fn draw(&mut self, g: &mut Gfx, r: R, s: f32, focused: bool, accent: [u8; 4]) {
        // the header: the command, a Run button, the result
        let y = r.y + 4.0 * s;
        let cmd = if self.command.is_empty() { "no test command set".to_string() } else { self.command.clone() };
        let w = g.text_width(12.0 * s, &cmd, Face::Mono) + 20.0 * s;
        let cr = R::new(r.x + 12.0 * s, y, w.min(r.w * 0.4), 24.0 * s);
        g.rrect(cr.x, cr.y, cr.w, cr.h, 8.0 * s, rgb(12, 16, 34));
        let t = g.fit(12.0 * s, &cmd, Face::Mono, cr.w - 16.0 * s);
        g.text(cr.x + 8.0 * s, cr.y + 5.0 * s, 12.0 * s, MUTED, &t, Face::Mono);
        let label = if self.runner.busy() { "Running…" } else { "Run tests  F5" };
        let bw = g.text_width(12.0 * s, label, Face::Bold) + 20.0 * s;
        let br = R::new(cr.x + cr.w + 8.0 * s, y, bw, 24.0 * s);
        if self.runner.busy() {
            g.rrect(br.x, br.y, br.w, br.h, 12.0 * s, rgb(16, 20, 36));
            g.text(br.x + 10.0 * s, br.y + 5.0 * s, 12.0 * s, AMBER, label, Face::Bold);
        } else {
            let hot = g.hovered(br);
            g.rrect(br.x, br.y, br.w, br.h, 12.0 * s, if hot { rgb(40, 50, 86) } else { fx::WELL });
            g.rrect_outline(br.x, br.y, br.w, br.h, 12.0 * s, 1.2, if hot { accent } else { with_alpha(accent, 150) });
            g.text(br.x + 10.0 * s, br.y + 5.0 * s, 12.0 * s, TEXT, label, Face::Bold);
            g.hit(br, Event::Sub(SUB, Key::Char('T')), "Run tests");
        }
        if let Some((m, ok)) = &self.summary {
            let t = g.fit(12.0 * s, m, Face::Bold, (r.x + r.w - br.x - br.w - 24.0 * s).max(20.0));
            g.text(br.x + br.w + 14.0 * s, y + 5.0 * s, 12.0 * s, if *ok { OK } else { ERR }, &t, Face::Bold);
        }
        let list = R::new(r.x + 8.0 * s, y + 32.0 * s, r.w - 16.0 * s, (r.h - 40.0 * s).max(0.0));
        let rh = 19.0 * s;
        self.visible = ((list.h / rh) as usize).max(1);
        if self.lines.is_empty() {
            let msg = if self.runner.busy() { "running the tests…" } else { "Press F5 (or the button) to run the project's test command. Lines with file:line become links to the editor." };
            let t = g.fit(13.0 * s, msg, Face::Sans, list.w);
            g.text(list.x + 6.0 * s, list.y + 6.0 * s, 13.0 * s, DIM, &t, Face::Sans);
            return;
        }
        for (i, l) in self.lines.iter().enumerate().skip(self.scroll).take(self.visible) {
            let ry = list.y + (i - self.scroll) as f32 * rh;
            let rect = R::new(list.x, ry, list.w - 8.0 * s, rh);
            let selected = i == self.sel;
            if selected {
                g.rrect(rect.x, rect.y, rect.w, rect.h, 5.0 * s, fx::over(rgb(22, 30, 58), with_alpha(accent, if focused { 50 } else { 24 })));
            } else if l.loc.is_some() && g.hovered(rect) {
                g.rrect(rect.x, rect.y, rect.w, rect.h, 5.0 * s, rgb(20, 27, 50));
            }
            let color = if l.color == [0, 0, 0, 0] { rgb(200, 207, 228) } else { l.color };
            let t = g.fit(12.0 * s, &l.text, Face::Mono, rect.w - 40.0 * s);
            g.text(rect.x + 6.0 * s, ry + 2.0 * s, 12.0 * s, color, &t, Face::Mono);
            if l.loc.is_some() {
                g.text(rect.x + rect.w - 24.0 * s, ry + 1.0 * s, 12.0 * s, CYAN, "↗", Face::Bold);
                g.hit(rect, Event::Row(PROBLEM_ROW + i), &l.text);
            }
        }
        fx::scrollbar(g, R::new(r.x + r.w - 8.0 * s, list.y, 6.0 * s, list.h), s, self.scroll, self.visible, self.lines.len(), accent);
    }
}

// ------------------------------------------------------------------------------------------------------------ search

pub struct SearchPanel {
    pub root: PathBuf,
    pub query: TextField,
    pub hits: Vec<Hit>,
    pub sel: usize,
    scroll: usize,
    visible: usize,
    rx: Option<Receiver<(String, Vec<Hit>)>>,
    pub last_query: String,
    pub searched: bool,
}

impl SearchPanel {
    pub fn new(root: &Path) -> SearchPanel {
        SearchPanel { root: root.into(), query: TextField::new("search the project's text (Enter)", false), hits: vec![], sel: 0, scroll: 0, visible: 8, rx: None, last_query: String::new(), searched: false }
    }

    pub fn search(&mut self) {
        let q = self.query.value.trim().to_string();
        if q.is_empty() {
            return;
        }
        let (tx, rx) = channel();
        let root = self.root.clone();
        self.rx = Some(rx);
        self.last_query = q.clone();
        let _ = std::thread::Builder::new().name("iyakki-ide-search".into()).spawn(move || {
            let hits = workspace::grep(&root, &q, 300);
            let _ = tx.send((q, hits));
        });
    }

    pub fn busy(&self) -> bool {
        self.rx.is_some()
    }

    pub fn tick(&mut self) -> bool {
        let Some(rx) = &self.rx else { return false };
        match rx.try_recv() {
            Ok((q, hits)) if q == self.last_query => {
                self.hits = hits;
                self.sel = 0;
                self.scroll = 0;
                self.searched = true;
                self.rx = None;
                true
            }
            Ok(_) => false,
            Err(std::sync::mpsc::TryRecvError::Empty) => false,
            Err(_) => {
                self.rx = None;
                false
            }
        }
    }

    pub fn handle(&mut self, ev: &Event) -> Option<(String, usize)> {
        let n = self.hits.len();
        match ev {
            Event::Key(Key::Enter) => {
                if self.query.value.trim() != self.last_query || !self.searched {
                    self.search();
                } else if let Some(h) = self.hits.get(self.sel) {
                    return Some((h.file.clone(), h.line));
                }
            }
            Event::Key(Key::Up) => self.sel = self.sel.saturating_sub(1),
            Event::Key(Key::Down) => self.sel = (self.sel + 1).min(n.saturating_sub(1)),
            Event::Key(Key::PageUp) => self.sel = self.sel.saturating_sub(self.visible),
            Event::Key(Key::PageDown) => self.sel = (self.sel + self.visible).min(n.saturating_sub(1)),
            Event::Key(Key::Esc) => {
                if self.query.value.is_empty() {
                    return None;
                }
                self.query.clear();
            }
            Event::Key(k) => {
                self.query.handle(k);
            }
            Event::Paste(t) => {
                self.query.paste(t);
            }
            Event::FocusAt(FOCUS_SEARCH, x) => self.query.click(*x),
            Event::Row(r) if (SEARCH_ROW..SEARCH_ROW + 100_000).contains(r) => {
                self.sel = (r - SEARCH_ROW).min(n.saturating_sub(1));
                return self.hits.get(self.sel).map(|h| (h.file.clone(), h.line));
            }
            Event::Scroll(d) => self.scroll = (self.scroll as i32 - d * 3).clamp(0, n.saturating_sub(self.visible) as i32) as usize,
            _ => {}
        }
        if self.sel < self.scroll {
            self.scroll = self.sel;
        } else if self.sel >= self.scroll + self.visible {
            self.scroll = self.sel + 1 - self.visible;
        }
        None
    }

    pub fn draw(&mut self, g: &mut Gfx, r: R, s: f32, focused: bool, accent: [u8; 4], blink: bool) {
        let fr = R::new(r.x + 10.0 * s, r.y + 4.0 * s, (r.w * 0.5).min(460.0 * s), 28.0 * s);
        self.query.render(g, fr, focused, s * 0.8, blink, Face::Sans, FOCUS_SEARCH);
        let note = if self.busy() { "searching…".to_string() } else if self.searched { format!("{} match{} in the project{}", self.hits.len(), if self.hits.len() == 1 { "" } else { "es" }, if self.hits.len() >= 300 { " (first 300)" } else { "" }) } else { "Ctrl+Shift+F · target, node_modules and .git are skipped".into() };
        let t = g.fit(12.0 * s, &note, Face::Sans, (r.x + r.w - fr.x - fr.w - 24.0 * s).max(20.0));
        g.text(fr.x + fr.w + 12.0 * s, r.y + 11.0 * s, 12.0 * s, DIM, &t, Face::Sans);
        let list = R::new(r.x + 8.0 * s, r.y + 40.0 * s, r.w - 16.0 * s, (r.h - 46.0 * s).max(0.0));
        let rh = 21.0 * s;
        self.visible = ((list.h / rh) as usize).max(1);
        for (i, h) in self.hits.iter().enumerate().skip(self.scroll).take(self.visible) {
            let ry = list.y + (i - self.scroll) as f32 * rh;
            let rect = R::new(list.x, ry, list.w - 8.0 * s, rh - 1.0 * s);
            if i == self.sel {
                g.rrect(rect.x, rect.y, rect.w, rect.h, 5.0 * s, fx::over(rgb(22, 30, 58), with_alpha(accent, if focused { 50 } else { 24 })));
            } else if g.hovered(rect) {
                g.rrect(rect.x, rect.y, rect.w, rect.h, 5.0 * s, rgb(20, 27, 50));
            }
            let loc = format!("{}:{}", h.file, h.line);
            let lw = (rect.w * 0.34).min(g.text_width(12.0 * s, &loc, Face::Mono) + 10.0 * s);
            let lt = g.fit(12.0 * s, &loc, Face::Mono, lw);
            g.text(rect.x + 6.0 * s, ry + 3.0 * s, 12.0 * s, CYAN, &lt, Face::Mono);
            let tt = g.fit(12.0 * s, &h.text, Face::Mono, rect.w - lw - 24.0 * s);
            g.text(rect.x + lw + 14.0 * s, ry + 3.0 * s, 12.0 * s, rgb(200, 207, 228), &tt, Face::Mono);
            g.hit(rect, Event::Row(SEARCH_ROW + i), &loc);
        }
        fx::scrollbar(g, R::new(r.x + r.w - 8.0 * s, list.y, 6.0 * s, list.h), s, self.scroll, self.visible, self.hits.len(), accent);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn world(tag: &str) -> PathBuf {
        let d = std::fs::canonicalize(std::env::temp_dir()).unwrap().join(format!("iyakki-bottom-{}-{tag}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(d.join("src")).unwrap();
        std::fs::write(d.join("src/main.rs"), "fn main() {\n    let x = 1;\n}\n").unwrap();
        std::fs::write(d.join("app.py"), "print(1)\n").unwrap();
        std::fs::write(d.join("index.js"), "1\n").unwrap();
        d
    }

    fn wait<T>(mut f: impl FnMut() -> Option<T>) -> Option<T> {
        for _ in 0..400 {
            if let Some(v) = f() {
                return Some(v);
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        None
    }

    #[test]
    fn rust_python_and_js_locations_are_found_and_must_be_inside_the_workspace() {
        let d = world("loc");
        assert_eq!(parse_location(&d, "  --> src/main.rs:2:9"), Some(("src/main.rs".into(), 2)));
        assert_eq!(parse_location(&d, "src/main.rs:3:1: error: oops"), Some(("src/main.rs".into(), 3)));
        assert_eq!(parse_location(&d, "  File \"app.py\", line 7, in <module>"), Some(("app.py".into(), 7)));
        assert_eq!(parse_location(&d, "    at run (/x/y.js:1:2)"), None, "not a file of the workspace");
        assert_eq!(parse_location(&d, &format!("    at run ({}/index.js:12:5)", d.display())), Some(("index.js".into(), 12)));
        assert_eq!(parse_location(&d, "thread 'main' panicked at src/main.rs:2:5:"), Some(("src/main.rs".into(), 2)));
        assert_eq!(parse_location(&d, "--> ../outside.rs:1:1"), None);
        assert_eq!(parse_location(&d, "--> /etc/passwd:1:1"), None);
        assert_eq!(parse_location(&d, "src/gone.rs:3"), None, "a file that does not exist");
        assert_eq!(parse_location(&d, "nothing to see 12:30"), None);
    }

    #[test]
    fn the_terminal_runs_commands_in_the_workspace_keeps_history_and_cd_stays_inside() {
        let d = world("term");
        let mut t = Term::new(&d, &TermSaved::default(), true);
        t.run_line("pwd");
        t.run_line("cd src");
        assert_eq!(t.cwd, "src");
        t.run_line("cd ..");
        assert_eq!(t.cwd, "");
        t.run_line("cd ../..");
        assert_eq!(t.cwd, "", "cd out of the workspace is refused");
        assert!(t.log.lines.iter().any(|l| l.0.contains("outside the workspace")));
        t.run_line("cd /etc");
        assert_eq!(t.cwd, "");
        t.run_line("cd src");
        t.run_line("ls");
        assert!(wait(|| t.tick().then_some(())).is_some());
        assert!(t.log.lines.iter().any(|l| l.0 == "main.rs"), "ran in src: {:?}", t.log.lines);
        assert!(t.log.lines.iter().any(|l| l.0.starts_with("exit 0")));
        t.run_line("no-such-program-xyz");
        assert!(wait(|| t.tick().then_some(())).is_some());
        assert!(t.log.lines.iter().any(|l| l.0 == "command not found"));
        let saved = t.saved();
        assert_eq!(saved.cwd, "src");
        assert!(saved.history.contains(&"ls".to_string()));
        let t2 = Term::new(&d, &saved, true);
        assert_eq!(t2.cwd, "src", "the folder and the history come back");
        // up recalls
        let mut t3 = Term::new(&d, &saved, true);
        t3.handle(&Event::Key(Key::Up));
        assert_eq!(t3.input.value, *saved.history.last().unwrap());
        // no permission: nothing runs
        let mut t4 = Term::new(&d, &TermSaved::default(), false);
        t4.run_line("ls");
        assert!(!t4.runner.busy() && t4.log.lines.last().unwrap().0.contains("may not run"));
        // a bogus saved folder is ignored
        assert_eq!(Term::new(&d, &TermSaved { cwd: "../x".into(), history: vec![] }, true).cwd, "");
    }

    #[test]
    fn problems_run_the_test_command_and_link_lines_to_files() {
        let d = world("prob");
        let mut p = Problems::new(&d, "");
        assert!(p.run(true).unwrap_err().contains("project.json"));
        p.command = "sh -c \"echo 'error[E0425]: cannot find value'; echo '  --> src/main.rs:2:9'; echo 'test result: FAILED'; exit 1\"".into();
        assert!(p.run(false).unwrap_err().contains("permission"));
        p.run(true).unwrap();
        assert!(wait(|| p.tick().then_some(())).is_some());
        assert_eq!(p.summary.as_ref().map(|s| s.1), Some(false));
        assert!(p.summary.as_ref().unwrap().0.starts_with("FAILED"));
        assert_eq!(p.lines.len(), 3);
        assert_eq!(p.selected_loc(), Some(("src/main.rs".into(), 2)));
        assert_eq!(p.handle(&Event::Key(Key::Enter)), Some(("src/main.rs".into(), 2)));
        assert_eq!(p.handle(&Event::Row(PROBLEM_ROW + 1)), Some(("src/main.rs".into(), 2)));
        assert_eq!(p.lines[0].color, ERR);
        assert!(p.lines[0].loc.is_none());
        p.command = "no-such-program-xyz".into();
        p.run(true).unwrap();
        assert!(wait(|| p.tick().then_some(())).is_some());
        assert!(p.summary.as_ref().unwrap().0.contains("was not found"));
    }

    #[test]
    fn search_finds_text_off_the_ui_thread_and_opens_the_hit() {
        let d = world("search");
        std::fs::create_dir_all(d.join("target")).unwrap();
        std::fs::write(d.join("target/x.rs"), "let x = 1;\n").unwrap();
        let mut s = SearchPanel::new(&d);
        for c in "let x".chars() {
            s.handle(&Event::Key(Key::Char(c)));
        }
        assert!(s.handle(&Event::Key(Key::Enter)).is_none());
        assert!(wait(|| s.tick().then_some(())).is_some());
        assert_eq!(s.hits.len(), 1, "{:?}", s.hits);
        assert_eq!(s.handle(&Event::Key(Key::Enter)), Some(("src/main.rs".into(), 2)));
        assert_eq!(s.handle(&Event::Row(SEARCH_ROW)), Some(("src/main.rs".into(), 2)));
    }

    #[test]
    fn the_panels_draw() {
        let d = world("draw");
        let mut g = Gfx::new(900, 300);
        g.clear(rgb(8, 11, 26));
        let mut t = Term::new(&d, &TermSaved::default(), true);
        t.run_line("echo hi");
        wait(|| t.tick().then_some(()));
        t.draw(&mut g, R::new(0.0, 0.0, 900.0, 300.0), 1.0, true, CYAN, true);
        let mut p = Problems::new(&d, "echo a");
        p.draw(&mut g, R::new(0.0, 0.0, 900.0, 300.0), 1.0, true, CYAN);
        p.run(true).unwrap();
        wait(|| p.tick().then_some(()));
        p.draw(&mut g, R::new(0.0, 0.0, 900.0, 300.0), 1.0, true, CYAN);
        let mut s = SearchPanel::new(&d);
        s.draw(&mut g, R::new(0.0, 0.0, 900.0, 300.0), 1.0, true, CYAN, true);
        assert!(g.hits.iter().any(|h| h.label == "field"));
    }

    #[test]
    fn the_terminal_finds_the_tools_the_cli_tile_installed() {
        use std::os::unix::fs::PermissionsExt;
        let d = std::env::temp_dir().join(format!("iyakki-clibin-{}", std::process::id()));
        let bin = d.join("bin");
        std::fs::create_dir_all(&bin).unwrap();
        let tool = bin.join("iyakki-demo-tool");
        std::fs::write(&tool, "#!/bin/sh\necho from the cli bin\n").unwrap();
        std::fs::set_permissions(&tool, std::fs::Permissions::from_mode(0o755)).unwrap();
        set_cli_bin(bin.clone());
        let env = path_env();
        assert!(env[0].1.starts_with(&format!("{}:", bin.display())), "the CLI folder comes first: {env:?}");
        let mut r = Runner::new();
        r.start(&d, vec!["iyakki-demo-tool".to_string()], Duration::from_secs(10));
        let mut out = None;
        for _ in 0..500 {
            if let Some((o, _)) = r.poll() {
                out = Some(o);
                break;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        let o = out.expect("the command ended");
        assert!(o.stdout.contains("from the cli bin"), "{:?} {:?}", o.stdout, o.stderr);
        // a folder that is already on PATH is not added twice
        std::env::set_var("PATH", format!("{}:{}", bin.display(), std::env::var("PATH").unwrap_or_default()));
        assert!(path_env().is_empty());
    }
}
