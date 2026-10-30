//! CLI: a terminal. All commands run through `iyakki_agent::cli::Cli`, which enforces the
//! signed-in user's policy.

use crate::safeclamp::SafeClamp;
use iyakki_agent::cli::{Action, Cli};

use iyakki_agent::clis::{self, CATALOG};

use super::widgets::render_confirm;
use super::{blink, App, AppCx, Flow, ScrollLog};
use crate::gfx::{rgb, Face, Gfx, Icon};
use crate::ui::{Event, Key, TextField, DIM, ERR, MUTED, OK, R, TEXT};

const ACCENT: [u8; 4] = rgb(255, 184, 77);

/// The confirmation for an install that runs a command (npm, pip): the whole command in plain words, wrapped, nothing cut.
fn render_plan(g: &mut Gfx, area: R, s: f32, name: &str, plan: &str) {
    let p = crate::fx::modal(g, area, s, 640.0 * s, 330.0 * s, ERR, true);
    g.text(p.x + 24.0 * s, p.y + 24.0 * s, 19.0 * s, TEXT, &format!("Install {name}?"), Face::Bold);
    let mut y = p.y + 58.0 * s;
    for l in g.wrap(14.0 * s, plan, Face::Sans, p.w - 48.0 * s).into_iter().take(9) {
        g.text(p.x + 24.0 * s, y, 14.0 * s, TEXT, &l, Face::Sans);
        y += 19.0 * s;
    }
    g.text(p.x + 24.0 * s, p.y + p.h - 92.0 * s, 13.0 * s, MUTED, "Press y to confirm, or Esc to cancel. Nothing runs before you confirm.", Face::Sans);
    g.button(p.x + 24.0 * s, p.y + p.h - 58.0 * s, 14.0 * s, "Install", ERR, Event::Key(Key::Char('y')));
    g.button(p.x + 24.0 * s + 150.0 * s, p.y + p.h - 58.0 * s, 14.0 * s, "Cancel", MUTED, Event::Key(Key::Esc));
}

#[path = "cli_conn.rs"]
mod conn;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Tab {
    Terminal,
    Clis,
    /// Saved SSH hosts (cli_conn.rs).
    Connections,
}

enum Mode {
    Browse,
    ConfirmInstall(&'static str),
    ConfirmRemove(&'static str),
    ConfirmStart,
    ConfirmStop,
    ConfirmStartOllama,
    ConfirmStopOllama,
}

pub struct CliApp {
    tab: Tab,
    sel: usize,
    /// The first tool shown in the list, and whether the pointer is over the list (the wheel scrolls it).
    list_top: usize,
    list_hover: bool,
    mode: Mode,
    /// What the install about to be confirmed will do (the exact command), worked out once when it is asked for.
    plan: Option<String>,
    status: Option<(String, bool)>,
    cli: Cli,
    log: ScrollLog,
    input: TextField,
    history: Vec<String>,
    hist_pos: Option<usize>,
    conns: conn::Conns,
    /// The check for a newer download list on its own has been asked for once in this window.
    dl_auto: bool,
}

impl CliApp {
    pub fn new(cx: &AppCx) -> CliApp {
        let cli = Cli::new(&cx.session.user, cx.session.policy.clone(), cx.host.clone()).with_skills(cx.services.skills.clone(), cx.services.packages.clone()).with_services(cx.services.clone());
        let mut log = ScrollLog::default();
        log.push(format!("IYAKKI terminal — {} ({}). Type `help`, or press F1 for the main commands.", cx.session.user, cx.session.role.name()), MUTED);
        CliApp { tab: Tab::Terminal, sel: 0, list_top: 0, list_hover: false, mode: Mode::Browse, plan: None, status: None, cli, log, input: TextField::new("", false), history: vec![], hist_pos: None, conns: conn::Conns::new(), dl_auto: false }
    }

    pub fn tab(&self) -> Tab {
        self.tab
    }

    pub fn status_text(&self) -> Option<&str> {
        self.status.as_ref().map(|s| s.0.as_str())
    }

    /// The external agent a catalogue entry is (`claude-code`, `copilot`), if it is one the IDE chat can use.
    fn agent_of(cli: &str) -> Option<&'static str> {
        match cli {
            "claude-code" => Some(iyakki_agent::extagent::CLAUDE_CODE),
            "github-copilot" => Some(iyakki_agent::extagent::COPILOT),
            _ => None,
        }
    }

    fn can_manage(cx: &AppCx) -> bool {
        cx.session.policy.manage_settings && cx.session.policy.cli == iyakki_sec::policy::CliLevel::Full
    }

    fn handle_clis(&mut self, ev: &Event, cx: &AppCx) -> Flow {
        match &self.mode {
            Mode::ConfirmInstall(id) => {
                let id = *id;
                if matches!(ev, Event::Key(Key::Char('y')) | Event::Key(Key::Enter)) {
                    self.mode = Mode::Browse;
                    match cx.services.clis.install(id, &cx.session.user) {
                        Ok(()) => {
                            cx.audit.record(&cx.session.user, "cli.install", id);
                            self.status = Some((format!("Installing {id}…"), true));
                        }
                        Err(e) => self.status = Some((e, false)),
                    }
                } else if matches!(ev, Event::Key(_)) {
                    self.mode = Mode::Browse;
                }
                return Flow::Stay;
            }
            Mode::ConfirmRemove(id) => {
                let id = *id;
                if matches!(ev, Event::Key(Key::Char('y')) | Event::Key(Key::Enter)) {
                    self.mode = Mode::Browse;
                    match cx.services.clis.remove(id) {
                        Ok(()) => {
                            cx.audit.record(&cx.session.user, "cli.remove", id);
                            self.status = Some((format!("Removed {id}."), true));
                        }
                        Err(e) => self.status = Some((e, false)),
                    }
                } else if matches!(ev, Event::Key(_)) {
                    self.mode = Mode::Browse;
                }
                return Flow::Stay;
            }
            Mode::ConfirmStart | Mode::ConfirmStop => {
                let start = matches!(self.mode, Mode::ConfirmStart);
                if matches!(ev, Event::Key(Key::Char('y')) | Event::Key(Key::Enter)) {
                    self.mode = Mode::Browse;
                    let r = if start { cx.services.cluster.start() } else { cx.services.cluster.stop() };
                    match r {
                        Ok(()) => {
                            cx.audit.record(&cx.session.user, if start { "cluster.start" } else { "cluster.stop" }, "");
                            self.status = Some((if start { "Starting the container cluster…".into() } else { "The container cluster is stopped.".into() }, true));
                        }
                        Err(e) => self.status = Some((e, false)),
                    }
                } else if matches!(ev, Event::Key(_)) {
                    self.mode = Mode::Browse;
                }
                return Flow::Stay;
            }
            Mode::ConfirmStartOllama | Mode::ConfirmStopOllama => {
                let start = matches!(self.mode, Mode::ConfirmStartOllama);
                if matches!(ev, Event::Key(Key::Char('y')) | Event::Key(Key::Enter)) {
                    self.mode = Mode::Browse;
                    let r = if start { cx.services.ollama.start() } else { cx.services.ollama.stop() };
                    match r {
                        Ok(()) => {
                            cx.audit.record(&cx.session.user, if start { "ollama.start" } else { "ollama.stop" }, "");
                            self.status = Some((if start { "Starting Ollama… (it appears in Models when it answers)".into() } else { "Ollama is stopped.".into() }, true));
                        }
                        Err(e) => self.status = Some((e, false)),
                    }
                } else if matches!(ev, Event::Key(_)) {
                    self.mode = Mode::Browse;
                }
                return Flow::Stay;
            }
            Mode::Browse => {}
        }
        let n = CATALOG.len();
        let ask = |this: &mut Self, install: bool| {
            if !Self::can_manage(cx) {
                this.status = Some(("Only administrators with a full terminal can install or remove tools.".into(), false));
                return;
            }
            let d = &CATALOG[this.sel.min(n - 1)];
            if matches!(d.source, clis::Source::System) {
                this.status = Some((format!("{} comes with the system and is not downloaded here. {}", d.name, cx.services.clis.system_status(d.id).map(|s| s.how_to).unwrap_or_default()), false));
                return;
            }
            if install {
                this.plan = cx.services.clis.install_plan(d.id);
                this.mode = Mode::ConfirmInstall(d.id);
            } else if cx.services.clis.is_installed(d.id).is_some() {
                this.mode = Mode::ConfirmRemove(d.id);
            } else {
                this.status = Some((format!("{} is not installed.", d.name), false));
            }
        };
        match ev {
            Event::Key(Key::Esc) => return Flow::Back,
            Event::Key(Key::Up) => self.sel = self.sel.saturating_sub(1),
            Event::Key(Key::Down) => self.sel = (self.sel + 1).min(n - 1),
            Event::Row(i) => self.sel = (*i).min(n - 1),
            Event::Scroll(d) if self.list_hover => self.list_top = (self.list_top as i32 - d).max(0) as usize,
            Event::Key(Key::Char('c')) if matches!(CATALOG[self.sel.min(n - 1)].source, clis::Source::System) => self.tab = Tab::Connections,
            Event::Tab(10) | Event::Key(Key::Char('i')) => ask(self, true),
            // update: the same download, checksum check and swap as an install, which keeps the old version for one rollback
            Event::Key(Key::Char('u')) => {
                let d = &CATALOG[self.sel.min(n - 1)];
                if matches!(d.source, clis::Source::System) {
                    self.status = Some((format!("{} comes with the system and is updated by its package manager.", d.name), false));
                } else if cx.services.clis.is_installed(d.id).is_none() {
                    self.status = Some((format!("{} is not installed; press i to install it.", d.name), false));
                } else {
                    ask(self, true);
                }
            }
            // the person asks for a newer download list now (a background check; the screen keeps working)
            Event::Key(Key::Char('r')) => {
                if !Self::can_manage(cx) {
                    self.status = Some(("Only administrators with a full terminal can check for updates.".into(), false));
                } else {
                    iyakki_agent::dlmanifest::fetch::start(&cx.services.clis.manifest_dir(), true);
                }
            }
            Event::Key(Key::Char('b')) => {
                let d = &CATALOG[self.sel.min(n - 1)];
                if !Self::can_manage(cx) {
                    self.status = Some(("Only administrators with a full terminal can change tools.".into(), false));
                } else {
                    match cx.services.clis.rollback(d.id) {
                        Ok(m) => {
                            cx.audit.record(&cx.session.user, "cli.rollback", d.id);
                            self.status = Some((m, true));
                        }
                        Err(e) => self.status = Some((e, false)),
                    }
                }
            }
            Event::Tab(11) | Event::Key(Key::Char('x')) | Event::Key(Key::Delete) => ask(self, false),
            Event::Tab(16) | Event::Key(Key::Char('g')) if Self::agent_of(CATALOG[self.sel.min(n - 1)].id).is_some() => {
                let d = &CATALOG[self.sel.min(n - 1)];
                if cx.services.clis.is_installed(d.id).is_none() {
                    self.status = Some((format!("Install {} first.", d.name), false));
                } else if let Some(agent) = crate::agent_login::agent_of_cli(d.id) {
                    // the shell opens the sign-in dialog (agent_login.rs): the vendor's login runs there, with its address, code and QR code
                    crate::agent_login::request(agent, false);
                }
            }
            Event::Tab(17) | Event::Key(Key::Char('c')) if Self::agent_of(CATALOG[self.sel.min(n - 1)].id).is_some() => {
                let d = &CATALOG[self.sel.min(n - 1)];
                let id = Self::agent_of(d.id).unwrap_or_default();
                if cx.services.clis.is_installed(d.id).is_none() {
                    self.status = Some((format!("Install {} first.", d.name), false));
                } else if cx.services.agents_cfg().agent(id).is_some_and(|a| a.auth_args().is_none()) {
                    self.status = Some((format!("{} has no documented sign-in status command. Sign in with the button, then try a message in the chat.", d.name), false));
                } else {
                    cx.services.ext_status.refresh(cx.services.agents_cfg());
                    self.status = Some((format!("Checking {}'s sign-in status (its own `auth status` command; no model is called)…", d.name), true));
                }
            }
            Event::Tab(12) | Event::Key(Key::Char('s')) if CATALOG[self.sel.min(n - 1)].id == "k3s" => {
                if !Self::can_manage(cx) {
                    self.status = Some(("Only administrators with a full terminal can start or stop the cluster.".into(), false));
                } else {
                    self.mode = Mode::ConfirmStart;
                }
            }
            Event::Tab(14) | Event::Key(Key::Char('s')) if CATALOG[self.sel.min(n - 1)].id == "ollama" => {
                if !Self::can_manage(cx) {
                    self.status = Some(("Only administrators with a full terminal can start or stop Ollama.".into(), false));
                } else {
                    self.mode = Mode::ConfirmStartOllama;
                }
            }
            Event::Tab(15) | Event::Key(Key::Char('t')) if CATALOG[self.sel.min(n - 1)].id == "ollama" => {
                if !Self::can_manage(cx) {
                    self.status = Some(("Only administrators with a full terminal can start or stop Ollama.".into(), false));
                } else {
                    self.mode = Mode::ConfirmStopOllama;
                }
            }
            Event::Tab(13) | Event::Key(Key::Char('t')) if CATALOG[self.sel.min(n - 1)].id == "k3s" => {
                if !Self::can_manage(cx) {
                    self.status = Some(("Only administrators with a full terminal can start or stop the cluster.".into(), false));
                } else {
                    self.mode = Mode::ConfirmStop;
                }
            }
            _ => {}
        }
        Flow::Stay
    }

    fn draw_tabs(&mut self, g: &mut Gfx, area: R, s: f32) -> f32 {
        let tabs = [("Terminal".to_string(), Event::Tab(0)), ("CLIs".to_string(), Event::Tab(1)), ("Connections".to_string(), Event::Tab(2))];
        crate::fx::tab_strip(g, area.x + 24.0 * s, area.y + 6.0 * s, s, area.w - 48.0 * s, &tabs, match self.tab { Tab::Terminal => 0, Tab::Clis => 1, Tab::Connections => 2 }, ACCENT);
        46.0 * s
    }

    /// The download list line: a notice after an applied update, and the "Check for updates" control with the last result in plain words.
    /// Returns the height used.
    fn draw_dl_check(&mut self, g: &mut Gfx, x: f32, y: f32, w: f32, s: f32, cx: &AppCx) -> f32 {
        use iyakki_agent::dlmanifest::fetch;
        let dir = cx.services.clis.manifest_dir();
        if !self.dl_auto {
            self.dl_auto = true;
            // never in tests: they must not reach the network
            #[cfg(not(test))]
            fetch::start(&dir, false);
        }
        for (action, detail) in fetch::take_audit() {
            cx.audit.record(&cx.session.user, &action, &detail);
        }
        let st = fetch::status(&dir);
        let mut used = 0.0;
        if let Some(n) = &st.notice {
            let n = g.fit(13.0 * s, n, Face::Sans, w);
            g.text(x, y, 13.0 * s, OK, &n, Face::Sans);
            used += 20.0 * s;
        }
        let bw = g.button(x, y + used, 12.0 * s, "Check for updates", ACCENT, Event::Key(Key::Char('r')));
        let line = fetch::status_line(&st, fetch::now_s());
        let tw = (w - bw - 12.0 * s).max(0.0);
        if tw > 60.0 * s {
            let line = g.fit(12.0 * s, &line, Face::Sans, tw);
            g.text(x + bw + 12.0 * s, y + used + 6.0 * s, 12.0 * s, if st.result.starts_with("refused") { ERR } else { MUTED }, &line, Face::Sans);
        }
        used + 12.0 * s * 2.1 + 8.0 * s
    }

    fn draw_clis(&mut self, g: &mut Gfx, area: R, s: f32, cx: &AppCx) {
        let x = area.x + 24.0 * s;
        let w = area.w - 48.0 * s;
        let mut y = area.y + 4.0 * s;
        if let Some((m, ok)) = &self.status {
            let m = g.fit(14.0 * s, m, Face::Sans, w);
            g.text(x, y, 14.0 * s, if *ok { OK } else { ERR }, &m, Face::Sans);
            y += 24.0 * s;
        }
        y += self.draw_dl_check(g, x, y, w, s, cx);
        let installed = cx.services.clis.installed();
        let list_w = (w * 0.34).clamp_safe(220.0 * s, 360.0 * s);
        let rh = 46.0 * s;
        let gap = 5.0 * s;
        let slots = (((area.y + area.h - y) / (rh + gap)).floor() as usize).max(1);
        self.list_hover = g.hovered(R::new(x, y, list_w, area.y + area.h - y));
        if self.sel < self.list_top {
            self.list_top = self.sel;
        } else if self.sel >= self.list_top + slots && !self.list_hover {
            self.list_top = self.sel + 1 - slots;
        }
        self.list_top = self.list_top.min(CATALOG.len().saturating_sub(slots));
        for (k, (i, d)) in CATALOG.iter().enumerate().skip(self.list_top).take(slots).enumerate() {
            let r = R::new(x, y + k as f32 * (rh + gap), list_w, rh);
            crate::fx::row_at(g, r.x, r.y, r.w, r.h, 12.0 * s, i == self.sel, false, ACCENT);
            g.hit(r, Event::Row(i), d.name);
            let inst = installed.iter().find(|x| x.id == d.id);
            let sys = cx.services.clis.system_status(d.id);
            let job = cx.services.clis.job(d.id).filter(|j| !j.finished);
            let here = inst.is_some() || sys.as_ref().is_some_and(|s| s.found);
            g.rect(r.x, r.y + 9.0 * s, 4.0 * s, r.h - 18.0 * s, if here { OK } else if job.is_some() { ACCENT } else { DIM });
            let nm = g.fit(15.0 * s, d.name, Face::Bold, r.w - 30.0 * s);
            g.text(r.x + 16.0 * s, r.y + 6.0 * s, 15.0 * s, TEXT, &nm, Face::Bold);
            let sub = match (&job, inst, &sys) {
                (Some(j), _, _) => j.phase.clone(),
                (None, Some(i), _) => format!("installed {}", i.version),
                (None, None, Some(st)) if st.found => format!("on this machine: {}", st.label),
                (None, None, Some(_)) => "not found on this machine".to_string(),
                _ => "not installed".to_string(),
            };
            let sub = g.fit(12.0 * s, &sub, Face::Sans, r.w - 30.0 * s);
            g.text(r.x + 16.0 * s, r.y + 26.0 * s, 12.0 * s, if here { OK } else { MUTED }, &sub, Face::Sans);
        }
        if CATALOG.len() > slots {
            g.text(x + 4.0 * s, area.y + area.h - 16.0 * s, 11.0 * s, DIM, &format!("{}–{} of {}", self.list_top + 1, (self.list_top + slots).min(CATALOG.len()), CATALOG.len()), Face::Sans);
        }
        // the chosen tool
        let d = &CATALOG[self.sel.min(CATALOG.len() - 1)];
        let (px, pw) = (x + list_w + 18.0 * s, w - list_w - 18.0 * s);
        if pw < 220.0 * s {
            return;
        }
        let ph = area.y + area.h - y;
        crate::fx::panel(g, R::new(px, y, pw, ph), s, ACCENT);
        let mut ty = y + 16.0 * s;
        g.text(px + 20.0 * s, ty, 22.0 * s, TEXT, d.name, Face::Bold);
        ty += 34.0 * s;
        for l in g.wrap(14.0 * s, d.about, Face::Sans, pw - 40.0 * s) {
            g.text(px + 20.0 * s, ty, 14.0 * s, MUTED, &l, Face::Sans);
            ty += 19.0 * s;
        }
        ty += 8.0 * s;
        let inst = installed.iter().find(|x| x.id == d.id);
        let cmds: Vec<&str> = d.bins.iter().map(|b| b.0).filter(|n| !n.ends_with("_completer")).collect();
        let line = |g: &mut Gfx, ty: &mut f32, k: &str, v: &str, col: [u8; 4]| {
            g.text(px + 20.0 * s, *ty, 12.0 * s, DIM, k, Face::Bold);
            for l in g.wrap(13.0 * s, v, Face::Sans, pw - 190.0 * s) {
                g.text(px + 170.0 * s, *ty, 13.0 * s, col, &l, Face::Sans);
                *ty += 18.0 * s;
            }
            *ty += 4.0 * s;
        };
        // a tool that comes with the system: found, never downloaded
        if let Some(st) = cx.services.clis.system_status(d.id) {
            line(g, &mut ty, "STATUS", &if st.found { format!("available: {} at {}", st.label, st.path) } else { "not found on this machine".to_string() }, if st.found { OK } else { rgb(255, 196, 94) });
            line(g, &mut ty, "COMMANDS", &if st.found { st.commands.join("   ") } else { cmds.join("   ") }, TEXT);
            line(g, &mut ty, "HOW TO GET IT", &st.how_to, TEXT);
            line(g, &mut ty, "FOR THE CHAT", "yes, with your approval for every call: run_command with ssh, scp or sftp, hosts you trusted, keys from your vault", TEXT);
            line(g, &mut ty, "HOME PAGE", d.home, MUTED);
            if !d.note.is_empty() {
                line(g, &mut ty, "NOTE", d.note, rgb(255, 196, 94));
            }
            g.button(px + 20.0 * s, y + ph - 54.0 * s, 15.0 * s, "Saved hosts (c)", ACCENT, Event::Tab(2));
            return;
        }
        match inst {
            Some(i) => {
                line(g, &mut ty, "STATUS", &format!("installed {}{}", i.version, if i.verified { ", download checked against the vendor's checksum" } else { ", no vendor checksum was available" }), OK);
                if let Some(p) = &i.previous {
                    line(g, &mut ty, "KEPT", &format!("{} (press B to go back to it; U updates, I installs the newest again)", p.version), TEXT);
                }
                if !i.reported.is_empty() {
                    line(g, &mut ty, "REPORTS", &i.reported, TEXT);
                }
            }
            None => line(g, &mut ty, "STATUS", "not installed", MUTED),
        }
        line(g, &mut ty, "COMMANDS", &cmds.join("   "), TEXT);
        let chat = if inst.is_some() { "yes: the assistant can run it with run_command, and you can type it in the Terminal tab".to_string() } else { "after it is installed, the assistant can run it and you can type it in the Terminal tab".to_string() };
        line(g, &mut ty, "FOR THE CHAT", &chat, TEXT);
        line(g, &mut ty, "DOWNLOADED FROM", &d.hosts.join(", "), TEXT);
        line(g, &mut ty, "HOME PAGE", d.home, MUTED);
        if !d.note.is_empty() {
            line(g, &mut ty, "NOTE", d.note, rgb(255, 196, 94));
        }
        // an external coding agent: what the AI IDE sees of it, and the vendor's sign-in
        if let Some(aid) = Self::agent_of(d.id) {
            let (text, col) = match (inst.is_some(), cx.services.ext_status.get(aid)) {
                (false, _) => ("install it first".to_string(), MUTED),
                (true, None) => ("checking…".to_string(), MUTED),
                (true, Some(st)) if st.ready() => (format!("{}{}", st.chip, match &st.auth { iyakki_agent::extagent::Auth::SignedIn(w) => format!(" ({w})"), iyakki_agent::extagent::Auth::Unknown => " (sign-in cannot be checked from here)".to_string(), _ => String::new() }), OK),
                (true, Some(st)) => (st.chip.clone(), ERR),
            };
            line(g, &mut ty, "IN THE AI IDE", &format!("{text} · listed in the chat's model list and Settings → Agents"), col);
            if inst.is_some() {
                let (t, col) = crate::agent_login::chip(&crate::agent_login::state(cx, aid, true));
                line(g, &mut ty, "SIGNED IN", &t, col);
                line(g, &mut ty, "SIGN IN", "the Login button runs the vendor's own sign-in and shows its address, code and a QR code to scan with a phone; your login stays in your home folder, IYAKKI never sees it", MUTED);
            }
            if d.id == "github-copilot" {
                line(g, &mut ty, "UNVERIFIED", "GitHub documents no command that reports whether Copilot CLI is signed in: this reads the program's own settings file, and its JSONL output format is unverified.", rgb(255, 196, 94));
            }
        }
        // the container cluster, on the k3s entry
        if d.id == "k3s" {
            let st = cx.services.cluster.state();
            let missing = cx.services.cluster.requirements();
            line(g, &mut ty, "CONTAINER CLUSTER", &st.label(), if st == iyakki_agent::cluster::ClusterState::Running { OK } else { TEXT });
            if !missing.is_empty() {
                line(g, &mut ty, "CANNOT START YET", &missing.join("; "), rgb(255, 196, 94));
            }
            line(g, &mut ty, "WHAT IT IS FOR", "the assistant runs code you want to try (Python, Node, shell) in a fresh limited container, and manages workloads with kubectl", MUTED);
        }
        // the local model server, on the Ollama entry
        if d.id == "ollama" {
            use iyakki_agent::ollama::OllamaState;
            let st = cx.services.ollama.state();
            let (label, col) = match &st {
                OllamaState::Stopped => ("stopped".to_string(), TEXT),
                OllamaState::Starting => ("starting…".to_string(), TEXT),
                OllamaState::Running { external, version } => (format!("running, version {version}{} at {}", if *external { " (started elsewhere)" } else { "" }, cx.services.ollama.url()), OK),
                OllamaState::Stopping => ("stopping…".to_string(), TEXT),
                OllamaState::Failed(e) => (e.clone(), ERR),
            };
            line(g, &mut ty, "LOCAL SERVER", &label, col);
            if inst.is_some() && matches!(st, OllamaState::Running { .. }) {
                line(g, &mut ty, "MODELS", &format!("kept in {} · pull more from the chat or Settings → Models", cx.services.ollama.models_dir().display()), MUTED);
            }
            line(g, &mut ty, "WHAT IT IS FOR", "answers the chat on this machine's own processor or graphics card; it is added to Models as ollama-local when it is ready", MUTED);
        }
        // progress
        if let Some(j) = cx.services.clis.job(d.id) {
            if !j.finished {
                let bar = R::new(px + 20.0 * s, ty + 6.0 * s, pw - 40.0 * s, 14.0 * s);
                let frac = if j.total > 0 { (j.done as f32 / j.total as f32).clamp(0.0, 1.0) } else { 0.0 };
                crate::fx::progress_bar(g, bar, if j.error.is_none() { frac } else { 0.0 }, ACCENT, s);
                let t = g.fit(13.0 * s, &j.describe(), Face::Sans, pw - 40.0 * s);
                g.text(px + 20.0 * s, ty + 28.0 * s, 13.0 * s, if j.error.is_some() { ERR } else { TEXT }, &t, Face::Sans);
            }
        }
        if let Some(e) = cx.services.clis.job(d.id).and_then(|j| j.error) {
            let t = g.fit(13.0 * s, &e, Face::Sans, pw - 40.0 * s);
            g.text(px + 20.0 * s, ty + 28.0 * s, 13.0 * s, ERR, &t, Face::Sans);
        }
        // buttons
        let by = y + ph - 54.0 * s;
        if Self::can_manage(cx) {
            let busy = cx.services.clis.running();
            let label = if inst.is_some() { "Update (i)" } else { "Install (i)" };
            let w1 = if busy { g.button_off(px + 20.0 * s, by, 15.0 * s, label) } else { g.button(px + 20.0 * s, by, 15.0 * s, label, ACCENT, Event::Tab(10)) };
            let mut bx = px + 20.0 * s + w1 + 10.0 * s;
            if inst.is_some() && !busy {
                bx += g.button(bx, by, 15.0 * s, "Remove (x)", ERR, Event::Tab(11)) + 10.0 * s;
            }
            if Self::agent_of(d.id).is_some() && inst.is_some() && !busy {
                let st = crate::agent_login::state(cx, Self::agent_of(d.id).unwrap_or_default(), true);
                bx += crate::agent_login::buttons(g, bx, by, 15.0 * s, Self::agent_of(d.id).unwrap_or_default(), &st);
                bx += g.button(bx, by, 15.0 * s, "Status (c)", TEXT, Event::Tab(17)) + 10.0 * s;
            }
            if d.id == "ollama" && inst.is_some() {
                use iyakki_agent::ollama::OllamaState;
                if matches!(cx.services.ollama.state(), OllamaState::Running { .. } | OllamaState::Starting) {
                    g.button(bx, by, 15.0 * s, "Stop Ollama (t)", ERR, Event::Tab(15));
                } else {
                    g.button(bx, by, 15.0 * s, "Start Ollama (s)", OK, Event::Tab(14));
                }
            }
            if d.id == "k3s" && inst.is_some() {
                let st = cx.services.cluster.state();
                if matches!(st, iyakki_agent::cluster::ClusterState::Running | iyakki_agent::cluster::ClusterState::Starting) {
                    g.button(bx, by, 15.0 * s, "Stop cluster (t)", ERR, Event::Tab(13));
                } else if cx.services.cluster.requirements().is_empty() {
                    g.button(bx, by, 15.0 * s, "Start cluster (s)", OK, Event::Tab(12));
                } else {
                    g.button_off(bx, by, 15.0 * s, "Start cluster (s)");
                }
            }
        } else {
            let note = "Only administrators with a full terminal can install or remove tools.";
            g.text(px + 20.0 * s, by + 8.0 * s, 13.0 * s, DIM, note, Face::Sans);
            // signing in is personal: anyone who may run commands can do it
            if let (Some(aid), true) = (Self::agent_of(d.id), inst.is_some() && cx.session.policy.cli == iyakki_sec::policy::CliLevel::Full) {
                let nw = g.text_width(13.0 * s, note, Face::Sans);
                let st = crate::agent_login::state(cx, aid, true);
                crate::agent_login::buttons(g, px + 40.0 * s + nw, by, 15.0 * s, aid, &st);
            }
        }
    }

    pub fn output_lines(&self) -> Vec<&str> {
        self.log.lines.iter().map(|(t, _)| t.as_str()).collect()
    }

    fn run_line(&mut self, cx: &AppCx) -> Flow {
        let line = self.input.take();
        self.hist_pos = None;
        self.log.push(format!("{}{}", self.cli.prompt(), line), OK);
        if line.trim().is_empty() {
            return Flow::Stay;
        }
        self.history.push(line.clone());
        let cmd = line.split_whitespace().next().unwrap_or("");
        // ssh, scp, sftp and sshtrust can take long: they run on a thread and stream into this log
        if let Some(call) = self.cli.ssh_call(&line) {
            cx.audit.record(&cx.session.user, "cli.exec", call.command());
            if !self.conns.start_call(call) {
                self.log.push("another ssh command is still running; wait for it to finish", ERR);
            }
            return Flow::Stay;
        }
        let o = self.cli.exec(&line);
        // log which command ran (never its arguments, which may hold secrets) and refusals
        let denied = o.text.contains("not permitted") || o.text.contains("permission denied") || o.text.contains("disabled");
        cx.audit.record(&cx.session.user, if denied { "cli.denied" } else { "cli.exec" }, cmd);
        match o.action {
            Action::Clear => self.log.clear(),
            Action::PowerOff => {
                self.log.push(o.text, MUTED);
                cx.audit.record(&cx.session.user, "power.off", "");
                cx.host.power_off();
            }
            Action::Reboot => {
                self.log.push(o.text, MUTED);
                cx.audit.record(&cx.session.user, "power.reboot", "");
                cx.host.reboot();
            }
            Action::None => {
                if !o.text.is_empty() {
                    self.log.push(o.text, if denied { ERR } else { TEXT });
                }
            }
        }
        Flow::Stay
    }
}

impl App for CliApp {
    fn title(&self) -> &'static str {
        "CLI"
    }
    /// The IYAKKI menu's Linux apps page: `sandbox:<name>` opens the terminal inside that sandbox (`linux use <name>`).
    fn open_item(&mut self, item: &str, cx: &AppCx) {
        if let Some(name) = item.strip_prefix("sandbox:") {
            self.tab = Tab::Terminal;
            self.input.set(&format!("linux use {name}"));
            let _ = self.run_line(cx);
        }
    }
    fn icon(&self) -> Icon {
        Icon::Cli
    }
    fn accent(&self) -> [u8; 4] {
        ACCENT
    }
    fn hint(&self) -> String {
        match self.tab {
            Tab::Terminal => "Enter run · ↑↓ history · Ctrl+V paste · PgUp/PgDn scroll · Tab CLIs · Esc back".into(),
            Tab::Connections => self.conns.hint().into(),
            Tab::Clis => "↑↓ choose · I install · U update to the newest · B go back a version · X remove · R check for updates · S start / T stop the container cluster (k3s) or Ollama · Tab terminal · Esc back".into(),
        }
    }

    fn help(&self) -> Vec<String> {
        // the main commands first (the same ones `help` prints), then the keys of the terminal itself
        let full = self.cli.policy.cli == iyakki_sec::policy::CliLevel::Full;
        let mut v: Vec<String> = [
            "help · every command you may use",
            "skill list · the skills the assistant follows (add <file>, show <name>, remove <name>)",
            "store install <file> · install a signed skill package after reading it (store show / list / remove)",
            "run <file.wasm> · run a module with your limits and permissions",
            "ls, cd, cat, pwd · look around the files you may read",
            "policy · what your account may do",
            "whoami, echo, clear",
        ]
        .iter()
        .map(|s| s.to_string())
        .collect();
        if full {
            v.extend(["mkdir, write · create folders and files you may write".to_string(), "detect, net · what machine this is and its network".to_string(), "ssh, scp, sftp · other machines, with host keys you trust first (sshtrust <host>, sshhosts, sshforget); see the Connections tab".to_string(), "poweroff, reboot · when your role allows".to_string()]);
        }
        v.push(String::new());
        v.push("Keys".to_string());
        v.extend(["Enter · run the line".to_string(), "Up / Down · earlier lines".to_string(), "Ctrl+V · paste".to_string(), "PgUp / PgDn · scroll the output".to_string()]);
        v
    }

    fn context_items(&mut self, x: i32, y: i32) -> Vec<crate::ctxmenu::Item> {
        if self.tab == Tab::Terminal {
            self.log.pane.context_items(x, y)
        } else {
            Vec::new()
        }
    }

    fn handle(&mut self, ev: &Event, cx: &AppCx) -> Flow {
        // the tabs (Tab key or a click) work from either side
        if matches!(self.mode, Mode::Browse) {
            match ev {
                Event::Tab(0) => {
                    self.tab = Tab::Terminal;
                    return Flow::Stay;
                }
                Event::Tab(1) => {
                    self.tab = Tab::Clis;
                    self.status = None;
                    return Flow::Stay;
                }
                Event::Tab(2) => {
                    self.tab = Tab::Connections;
                    return Flow::Stay;
                }
                // the Tab key goes round the tabs, except while a form of the Connections tab is open
                Event::Key(Key::Tab) if !(self.tab == Tab::Connections && self.conns.in_form()) => {
                    self.tab = match self.tab {
                        Tab::Terminal => Tab::Clis,
                        Tab::Clis => Tab::Connections,
                        Tab::Connections => Tab::Terminal,
                    };
                    self.status = None;
                    return Flow::Stay;
                }
                _ => {}
            }
        }
        // SSH calls started from either tab go on in the background: their output reaches the terminal log
        if matches!(ev, Event::Tick) {
            self.conns.tick(cx);
        }
        let flow = if self.tab == Tab::Connections { Some(self.conns.handle(ev, cx)) } else { None };
        for (t, c) in self.conns.out.drain(..) {
            self.log.push(t, c);
        }
        if let Some(f) = flow {
            return f;
        }
        if self.tab == Tab::Clis {
            return self.handle_clis(ev, cx);
        }
        // selecting and copying the output
        if self.log.handle(ev) {
            return Flow::Stay;
        }
        match ev {
            Event::Tick => self.log.on_tick(),
            Event::Key(Key::Ctrl('a')) if self.input.value.is_empty() => self.log.pane.select_all(),
            Event::Key(Key::Esc) => return Flow::Back,
            Event::Key(Key::Enter) => return self.run_line(cx),
            Event::Paste(t) => {
                self.input.paste(t);
            }
            Event::FocusAt(_, x) => {
                self.log.pane.clear();
                self.input.click(*x)
            }
            Event::Key(Key::Up) if !self.history.is_empty() => {
                let p = self.hist_pos.map_or(self.history.len() - 1, |p| p.saturating_sub(1));
                self.hist_pos = Some(p);
                self.input.set(&self.history[p].clone());
            }
            Event::Key(Key::Down) => match self.hist_pos {
                Some(p) if p + 1 < self.history.len() => {
                    self.hist_pos = Some(p + 1);
                    self.input.set(&self.history[p + 1].clone());
                }
                _ => {
                    self.hist_pos = None;
                    self.input.clear();
                }
            },
            Event::Key(Key::PageUp) => self.log.scroll_by(10),
            Event::Key(Key::PageDown) => self.log.scroll_by(-10),
            Event::Scroll(d) => self.log.scroll_by(*d * 3),
            Event::Key(k) => {
                self.input.handle(k);
            }
            _ => {}
        }
        Flow::Stay
    }

    fn render(&mut self, g: &mut Gfx, area: R, s: f32, cx: &AppCx, tick: u64) {
        let off = self.draw_tabs(g, area, s);
        let area = R::new(area.x, area.y + off, area.w, area.h - off);
        if self.tab == Tab::Connections {
            self.conns.render(g, area, s, cx, tick);
            return;
        }
        if self.tab == Tab::Clis {
            self.draw_clis(g, area, s, cx);
            match &self.mode {
                Mode::ConfirmInstall(id) if self.plan.is_some() => {
                    let d = clis::def(id).expect("catalogue entry");
                    render_plan(g, R::new(area.x, area.y - off, area.w, area.h + off), s, d.name, self.plan.as_deref().unwrap_or_default());
                }
                Mode::ConfirmInstall(id) => {
                    let d = clis::def(id).expect("catalogue entry");
                    render_confirm(g, R::new(area.x, area.y - off, area.w, area.h + off), s, &format!("Download and install {} from {}?{}", d.name, d.hosts.join(", "), if d.checksum.is_some() { " It is checked against the vendor's checksum." } else { " The vendor publishes no checksum for it." }), "Install");
                }
                Mode::ConfirmRemove(id) => {
                    let d = clis::def(id).expect("catalogue entry");
                    render_confirm(g, R::new(area.x, area.y - off, area.w, area.h + off), s, &format!("Remove {} and its files?", d.name), "Remove");
                }
                Mode::ConfirmStart => render_confirm(g, R::new(area.x, area.y - off, area.w, area.h + off), s, "Start the container cluster (k3s)? It uses about 1 GB of memory and fetches container images the first time code is run.", "Start"),
                Mode::ConfirmStop => render_confirm(g, R::new(area.x, area.y - off, area.w, area.h + off), s, "Stop the container cluster? Anything running in it stops.", "Stop"),
                Mode::ConfirmStartOllama => render_confirm(g, R::new(area.x, area.y - off, area.w, area.h + off), s, "Start Ollama on this machine? It uses memory while models are loaded (several gigabytes for a large one) and listens on this machine only.", "Start"),
                Mode::ConfirmStopOllama => render_confirm(g, R::new(area.x, area.y - off, area.w, area.h + off), s, "Stop Ollama? Models in memory are unloaded; the downloaded models stay.", "Stop"),
                Mode::Browse => {}
            }
            return;
        }
        let size = 16.0 * s;
        let lh = g.line_height(size, Face::Mono);
        crate::fx::panel(g, R::new(area.x + 16.0 * s, area.y + 4.0 * s, area.w - 32.0 * s, area.h - 20.0 * s), s, ACCENT);
        let log_area = R::new(area.x + 32.0 * s, area.y + 18.0 * s, area.w - 64.0 * s, area.h - 40.0 * s - lh - 6.0 * s);
        self.log.render(g, log_area, size, Face::Mono);
        let y = area.y + area.h - 20.0 * s - lh - 8.0 * s;
        let prompt = self.cli.prompt();
        let pw = g.text(log_area.x, y, size, OK, &prompt, Face::Mono);
        // the whole bottom line is the input: clicking anywhere on it places the caret
        self.input.render_bare(g, R::new(log_area.x + pw, y, (log_area.w - pw - 8.0 * s).max(40.0 * s), lh), true, s, blink(tick), Face::Mono, 0);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::apps::tests::{cx_with_backend, MockHost};

    fn type_line(app: &mut CliApp, cx: &AppCx, s: &str) {
        for c in s.chars() {
            app.handle(&Event::Key(Key::Char(c)), cx);
        }
        app.handle(&Event::Key(Key::Enter), cx);
    }

    #[test]
    fn runs_commands_records_audit_and_history() {
        let (cx, d) = cx_with_backend("cli1", iyakki_agent::chat::Unconfigured);
        let mut app = CliApp::new(&cx);
        type_line(&mut app, &cx, "whoami");
        assert_eq!(app.output_lines().last().copied(), Some("tester"));
        type_line(&mut app, &cx, "frobnicate secret-arg");
        assert!(app.output_lines().last().unwrap().contains("not found"));
        app.handle(&Event::Key(Key::Up), &cx);
        assert_eq!(app.input.value, "frobnicate secret-arg");
        app.handle(&Event::Key(Key::Down), &cx);
        assert_eq!(app.input.value, "");
        let audit = std::fs::read_to_string(d.join("audit.log")).unwrap();
        assert!(audit.contains("cli.exec") && !audit.contains("secret-arg"), "arguments must not be logged");
        type_line(&mut app, &cx, "clear");
        assert_eq!(app.output_lines().len(), 0);
        let mut g = Gfx::new(900, 600);
        g.clear(rgb(0, 0, 0));
        app.render(&mut g, R::new(0.0, 80.0, 900.0, 520.0), 1.0, &cx, 0);
    }

    struct Vendor(std::collections::HashMap<String, Vec<u8>>);
    impl iyakki_agent::iyakki_proxy::Streamer for Vendor {
        fn open(&self, url: &str, _h: &[(String, String)]) -> Result<iyakki_agent::iyakki_proxy::StreamHead, String> {
            let b = self.0.get(url).ok_or_else(|| format!("no route {url}"))?.clone();
            Ok(iyakki_agent::iyakki_proxy::StreamHead { status: 200, headers: vec![("content-length".into(), b.len().to_string())], body: Box::new(std::io::Cursor::new(b)) })
        }
    }

    fn vendor_with_kubectl() -> Vendor {
        let arch = if std::env::consts::ARCH == "aarch64" { "arm64" } else { "amd64" };
        let bin = b"#!/bin/sh\necho 'Client Version: v1.31.0'\n".to_vec();
        let sum = iyakki_agent::clis::sha256_hex(&bin);
        let mut m = std::collections::HashMap::new();
        m.insert("https://dl.k8s.io/release/stable.txt".to_string(), b"v1.31.0".to_vec());
        m.insert(format!("https://dl.k8s.io/release/v1.31.0/bin/linux/{arch}/kubectl"), bin);
        m.insert(format!("https://dl.k8s.io/release/v1.31.0/bin/linux/{arch}/kubectl.sha256"), sum.into_bytes());
        Vendor(m)
    }

    fn draw(app: &mut CliApp, cx: &AppCx, w: u32, h: u32) -> Gfx {
        let mut g = Gfx::new(w, h);
        g.clear(rgb(9, 11, 20));
        app.render(&mut g, R::new(0.0, 46.0, w as f32, h as f32 - 46.0), 1.0, cx, 3);
        g
    }

    #[test]
    fn the_cli_tile_has_a_terminal_tab_and_a_clis_tab_and_an_administrator_installs_and_removes_a_tool_after_confirming() {
        let (cx, _d) = cx_with_backend("cli3", iyakki_agent::chat::Unconfigured);
        cx.services.set_streamer(std::sync::Arc::new(vendor_with_kubectl()));
        let mut app = CliApp::new(&cx);
        assert_eq!((app.title(), app.tab()), ("CLI", Tab::Terminal));
        let g = draw(&mut app, &cx, 1100, 650);
        assert!(g.hits.iter().any(|h| h.ev == Event::Tab(0)) && g.hits.iter().any(|h| h.ev == Event::Tab(1)), "both tabs are buttons");
        // the terminal still works, then the tab switches by key and by click
        type_line(&mut app, &cx, "whoami");
        assert_eq!(app.output_lines().last().copied(), Some("tester"));
        app.handle(&Event::Key(Key::Tab), &cx);
        assert_eq!(app.tab(), Tab::Clis);
        app.handle(&Event::Tab(0), &cx);
        assert_eq!(app.tab(), Tab::Terminal);
        app.handle(&Event::Tab(1), &cx);
        let g = draw(&mut app, &cx, 1100, 650);
        for want in ["kubectl", "k3s", "Helm", "Terraform", "GitHub CLI", "AWS CLI", "Google Cloud CLI"] {
            assert!(g.hits.iter().any(|h| h.label == want), "{want} is listed");
        }
        // the rest of the list is reached by selecting further down (the list scrolls to follow)
        // (as many steps as the catalogue needs: it grows, a fixed number of key presses goes stale)
        let mut g = draw(&mut app, &cx, 1100, 650);
        for want in ["Arduino CLI", "esptool (ESP32/ESP8266)", "espflash (ESP32)", "Azure CLI"] {
            for _ in 0..CATALOG.len() {
                if g.hits.iter().any(|h| h.label == want) {
                    break;
                }
                app.handle(&Event::Key(Key::Down), &cx);
                g = draw(&mut app, &cx, 1100, 650);
            }
            assert!(g.hits.iter().any(|h| h.label == want), "{want} is listed after scrolling");
        }
        assert!(app.list_top > 0);
        for _ in 0..CATALOG.len() {
            app.handle(&Event::Key(Key::Up), &cx);
        }
        draw(&mut app, &cx, 1100, 650);
        // choose kubectl (the first), ask to install, then decline
        app.handle(&Event::Key(Key::Char('i')), &cx);
        assert!(matches!(app.mode, Mode::ConfirmInstall("kubectl")));
        draw(&mut app, &cx, 1100, 650);
        app.handle(&Event::Key(Key::Char('n')), &cx);
        assert!(cx.services.clis.installed().is_empty() && matches!(app.mode, Mode::Browse));
        app.handle(&Event::Key(Key::Char('i')), &cx);
        app.handle(&Event::Key(Key::Char('y')), &cx);
        for _ in 0..300 {
            if cx.services.clis.job("kubectl").is_some_and(|j| j.finished || j.error.is_some()) {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        assert_eq!(cx.services.clis.is_installed("kubectl").map(|i| i.version), Some("v1.31.0".to_string()), "{:?}", cx.services.clis.job("kubectl"));
        let g = draw(&mut app, &cx, 1100, 650);
        assert!(g.hits.iter().any(|h| h.ev == Event::Tab(11)), "a Remove button appears once it is installed");
        // it can now be typed at the terminal tab
        app.handle(&Event::Tab(0), &cx);
        type_line(&mut app, &cx, "kubectl version");
        assert!(app.output_lines().last().unwrap().contains("Client Version: v1.31.0"), "{:?}", app.output_lines());
        // removal asks first
        app.handle(&Event::Tab(1), &cx);
        app.handle(&Event::Key(Key::Char('x')), &cx);
        assert!(matches!(app.mode, Mode::ConfirmRemove("kubectl")));
        draw(&mut app, &cx, 1100, 650);
        app.handle(&Event::Key(Key::Char('y')), &cx);
        assert!(cx.services.clis.installed().is_empty());
        let log = std::fs::read_to_string(cx.audit.path()).unwrap();
        assert!(log.contains("cli.install") && log.contains("cli.remove"), "{log}");
        // other sizes draw, and the rest of the catalogue can be browsed
        for _ in 0..8 {
            app.handle(&Event::Key(Key::Down), &cx);
        }
        for (w, h) in [(1280u32, 720u32), (700, 500), (420, 400)] {
            draw(&mut app, &cx, w, h);
        }
    }

    #[test]
    fn the_k3s_entry_starts_and_stops_the_container_cluster_after_confirming_and_says_what_is_missing() {
        let (cx, d) = cx_with_backend("cli5", iyakki_agent::chat::Unconfigured);
        let mut app = CliApp::new(&cx);
        app.handle(&Event::Tab(1), &cx);
        app.handle(&Event::Key(Key::Down), &cx); // k3s is the second entry
        // not installed, and this machine is not the real thing: the panel says what is missing and starting is off
        let g = draw(&mut app, &cx, 1200, 720);
        assert!(g.hits.iter().any(|h| h.label == "k3s"));
        assert!(!g.hits.iter().any(|h| h.ev == Event::Tab(12)));
        app.handle(&Event::Key(Key::Char('s')), &cx);
        assert!(matches!(app.mode, Mode::ConfirmStart));
        draw(&mut app, &cx, 1200, 720);
        app.handle(&Event::Key(Key::Char('y')), &cx);
        assert!(app.status_text().unwrap().contains("cannot start here"), "{:?}", app.status_text());
        // with k3s installed (a stand-in) on a machine that can, it starts, shows running, and stops
        let root = cx.services.dir().join("cli");
        iyakki_agent::cluster::write_fake_k3s(&root, &d.join("k.log"), "{}");
        cx.services.cluster.assume_machine(true, true);
        let g = draw(&mut app, &cx, 1200, 720);
        assert!(g.hits.iter().any(|h| h.ev == Event::Tab(12)), "a Start button appears");
        app.handle(&Event::Tab(12), &cx);
        app.handle(&Event::Key(Key::Enter), &cx);
        for _ in 0..200 {
            if cx.services.cluster.state() == iyakki_agent::cluster::ClusterState::Running {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
        let g = draw(&mut app, &cx, 1200, 720);
        assert!(g.hits.iter().any(|h| h.ev == Event::Tab(13)), "Stop replaces Start while it runs");
        app.handle(&Event::Key(Key::Char('t')), &cx);
        assert!(matches!(app.mode, Mode::ConfirmStop));
        app.handle(&Event::Key(Key::Char('y')), &cx);
        assert_eq!(cx.services.cluster.state(), iyakki_agent::cluster::ClusterState::Stopped);
        let log = std::fs::read_to_string(cx.audit.path()).unwrap();
        assert!(log.contains("cluster.start") && log.contains("cluster.stop"), "{log}");
        // the s key does nothing on other tools
        app.handle(&Event::Key(Key::Down), &cx);
        app.handle(&Event::Key(Key::Char('s')), &cx);
        assert!(matches!(app.mode, Mode::Browse));
    }

    #[test]
    fn people_who_are_not_administrators_see_the_tools_but_cannot_install_or_remove_them() {
        let (mut cx, _d) = cx_with_backend("cli4", iyakki_agent::chat::Unconfigured);
        cx.services.set_streamer(std::sync::Arc::new(vendor_with_kubectl()));
        cx.session.policy = iyakki_sec::policy::Role::Operator.template();
        cx.session.policy.manage_settings = false;
        let mut app = CliApp::new(&cx);
        app.handle(&Event::Tab(1), &cx);
        let g = draw(&mut app, &cx, 1100, 650);
        assert!(!g.hits.iter().any(|h| h.ev == Event::Tab(10)), "no install button");
        app.handle(&Event::Key(Key::Char('i')), &cx);
        assert!(matches!(app.mode, Mode::Browse));
        assert!(app.status_text().unwrap().contains("Only administrators"));
        assert!(cx.services.clis.installed().is_empty());
    }

    /// `IYAKKI_SHOT=/path.png cargo test -p iyakki-shell --lib clis_picture -- --ignored`
    #[test]
    #[ignore]
    fn clis_picture() {
        let Ok(out) = std::env::var("IYAKKI_SHOT") else { return };
        let (cx, _d) = cx_with_backend("clipic", iyakki_agent::chat::Unconfigured);
        cx.services.set_streamer(std::sync::Arc::new(vendor_with_kubectl()));
        cx.services.clis.install("kubectl", "root").unwrap();
        for _ in 0..300 {
            if cx.services.clis.job("kubectl").is_some_and(|j| j.finished) {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        let mut app = CliApp::new(&cx);
        app.handle(&Event::Tab(1), &cx);
        app.handle(&Event::Key(Key::Down), &cx);
        app.handle(&Event::Key(Key::Down), &cx);
        app.handle(&Event::Key(Key::Down), &cx);
        app.handle(&Event::Key(Key::Down), &cx);
        app.handle(&Event::Key(Key::Down), &cx);
        let g = draw(&mut app, &cx, 1200, 700);
        g.save_png(std::path::Path::new(&out)).unwrap();
    }

    #[test]
    fn power_commands_obey_role() {
        let (mut cx, _d) = cx_with_backend("cli2", iyakki_agent::chat::Unconfigured);
        let host = std::sync::Arc::new(MockHost::default());
        cx.host = host.clone();
        cx.session.policy = iyakki_sec::policy::Role::User.template();
        let mut app = CliApp::new(&cx);
        type_line(&mut app, &cx, "poweroff");
        assert!(host.powered_off.lock().unwrap().is_none());
        cx.session.policy = iyakki_sec::policy::Role::Operator.template();
        let mut app = CliApp::new(&cx);
        type_line(&mut app, &cx, "poweroff");
        assert_eq!(*host.powered_off.lock().unwrap(), Some("off".to_string()));
        type_line(&mut app, &cx, "reboot");
        assert_eq!(*host.powered_off.lock().unwrap(), Some("reboot".to_string()));
    }

    #[test]
    fn the_ollama_entry_starts_and_stops_a_local_server_after_confirming_and_adds_it_to_models() {
        use std::io::{Read, Write};
        let (cx, _d) = cx_with_backend("cli-ollama", iyakki_agent::chat::Unconfigured);
        let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = l.local_addr().unwrap().port();
        drop(l);
        cx.services.ollama.set_port(port);
        let mut app = CliApp::new(&cx);
        app.handle(&Event::Tab(1), &cx);
        let idx = CATALOG.iter().position(|d| d.id == "ollama").unwrap();
        for _ in 0..idx {
            app.handle(&Event::Key(Key::Down), &cx);
        }
        // not installed: no start button, and pressing s says what is missing
        let g = draw(&mut app, &cx, 1200, 720);
        assert!(g.hits.iter().any(|h| h.label == "Ollama"));
        assert!(!g.hits.iter().any(|h| h.ev == Event::Tab(14)));
        app.handle(&Event::Key(Key::Char('s')), &cx);
        assert!(matches!(app.mode, Mode::ConfirmStartOllama));
        app.handle(&Event::Key(Key::Char('y')), &cx);
        assert!(app.status_text().unwrap().contains("not installed"), "{:?}", app.status_text());
        // installed (a stand-in): the button appears, the server starts once something answers, shows up in Models, and stops
        iyakki_agent::clis::write_fake_installed(&cx.services.clis, "ollama", "#!/bin/sh\nexec sleep 300\n");
        let g = draw(&mut app, &cx, 1200, 720);
        assert!(g.hits.iter().any(|h| h.ev == Event::Tab(14)), "a Start button appears");
        app.handle(&Event::Tab(14), &cx);
        app.handle(&Event::Key(Key::Enter), &cx);
        let stop = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let l = std::net::TcpListener::bind(("127.0.0.1", port)).unwrap();
        l.set_nonblocking(true).unwrap();
        let s2 = stop.clone();
        std::thread::spawn(move || {
            while !s2.load(std::sync::atomic::Ordering::Relaxed) {
                if let Ok((mut c, _)) = l.accept() {
                    let mut b = [0u8; 256];
                    let _ = c.set_read_timeout(Some(std::time::Duration::from_millis(200)));
                    let _ = c.read(&mut b);
                    let body = "{\"version\":\"0.40.3\"}";
                    let _ = c.write_all(format!("HTTP/1.0 200 OK\r\nContent-Length: {}\r\n\r\n{body}", body.len()).as_bytes());
                } else {
                    std::thread::sleep(std::time::Duration::from_millis(10));
                }
            }
        });
        for _ in 0..400 {
            if matches!(cx.services.ollama.state(), iyakki_agent::ollama::OllamaState::Running { .. }) {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(25));
        }
        let g = draw(&mut app, &cx, 1200, 720);
        assert!(g.hits.iter().any(|h| h.ev == Event::Tab(15)), "Stop replaces Start while it runs");
        let models = cx.services.settings.lock().unwrap().get().providers.iter().filter(|p| p.id == "ollama-local").count();
        assert_eq!(models, 1, "it is in Models now");
        app.handle(&Event::Key(Key::Char('t')), &cx);
        assert!(matches!(app.mode, Mode::ConfirmStopOllama));
        app.handle(&Event::Key(Key::Char('y')), &cx);
        assert_eq!(cx.services.ollama.state(), iyakki_agent::ollama::OllamaState::Stopped);
        let log = std::fs::read_to_string(cx.audit.path()).unwrap();
        assert!(log.contains("ollama.start") && log.contains("ollama.stop"), "{log}");
        stop.store(true, std::sync::atomic::Ordering::Relaxed);
    }
}

#[cfg(test)]
#[path = "cli_select_tests.rs"]
mod select_tests;

#[cfg(test)]
#[path = "cli_agents_tests.rs"]
mod agents_tests;

#[cfg(test)]
#[path = "cli_conn_tests.rs"]
mod conn_tests;

#[cfg(test)]
#[path = "cli_dlcheck_tests.rs"]
mod dlcheck_tests;
