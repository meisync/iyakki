//! SYSTEM: what the machine is doing. Four pages: the **overview** (processor, memory, load, disks, network, heat, how
//! long it has been up), the **processes** (administrators only: who uses what, stop or kill one) and the **agents** (every
//! run going on now: whose, what it was asked, what it waits for, stop one or all) and the **hardware**
//! (administrators only: what the kernel found, PCI devices with their drivers, loaded drivers). Figures are read from the kernel once a
//! second; nothing is kept.

use std::collections::HashMap;
use std::time::{Duration, Instant};

use iyakki_agent::runs::LiveRun;
use iyakki_agent::sysinfo::{self, CpuTimes, NetCounters, Proc, SysRoots};

use super::{App, AppCx, Flow};
use crate::gfx::{rgb, Face, Gfx, Icon};
use crate::ui::{Event, Key, DIM, ERR, MUTED, R, TEXT};

const ACCENT: [u8; 4] = rgb(176, 226, 92);
const AMBER: [u8; 4] = rgb(255, 184, 77);
const HISTORY: usize = 90;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Page {
    Overview,
    Processes,
    Agents,
    Hardware,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Sort {
    Cpu,
    Memory,
    Pid,
    Name,
}

impl Sort {
    fn label(self) -> &'static str {
        match self {
            Sort::Cpu => "processor",
            Sort::Memory => "memory",
            Sort::Pid => "number",
            Sort::Name => "name",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum Confirm {
    Signal { pid: u32, name: String, hard: bool },
    StopAll,
}

struct Sample {
    at: Instant,
    cpu: CpuTimes,
    procs: Vec<Proc>,
    net: Vec<NetCounters>,
}

pub struct SystemApp {
    page: Page,
    roots: SysRoots,
    data_dir: std::path::PathBuf,
    prev: Option<Sample>,
    cur: Option<Sample>,
    /// Processor use over the last samples, 0 to 100.
    history: Vec<f32>,
    proc_cpu: HashMap<u32, f32>,
    /// Bytes per second received and sent, summed over the interfaces.
    net_rate: (f64, f64),
    /// "MAC: aa:bb:cc:dd:ee:ff (private)" for the adapter in use, read with the other figures.
    mac_line: Option<String>,
    /// Where sound plays and why (Settings -> Sound), read with the other figures.
    sound_line: String,
    /// One line per graphics card (name, driver, memory), read with the other figures.
    gpu_lines: Vec<String>,
    sort: Sort,
    sel: [usize; 4],
    /// The hardware page, read when it is opened or refreshed.
    hardware: Vec<String>,
    confirm: Option<Confirm>,
    status: Option<(String, bool)>,
    live: Vec<LiveRun>,
    last_refresh: Option<Instant>,
    /// The dev cluster's service log (View logs), selectable text.
    cluster_log: Option<crate::apps::ScrollLog>,
    health_busy: std::sync::Arc<std::sync::atomic::AtomicBool>,
}

impl SystemApp {
    /// The graphics cards for the overview, e.g. "AMD Radeon Graphics (Raphael) · driver amdgpu" and "NVIDIA GeForce RTX 4070 Laptop GPU · driver nvidia · 8 GB".
    /// Virtual display devices are left out; a card with no driver says so (the NVIDIA driver is a download).
    fn gpu_summary(gpus: &[iyakki_agent::gpu::Gpu]) -> Vec<String> {
        gpus.iter()
            .filter(|g| !matches!(g.vendor, iyakki_agent::gpu::Vendor::Other(_)))
            .map(|g| {
                let name = if g.name.to_ascii_lowercase().contains(&g.vendor.name().to_ascii_lowercase()) { g.name.clone() } else { format!("{} {}", g.vendor.name(), g.name) };
                let driver = g.driver.clone().map(|d| format!("driver {d}")).unwrap_or_else(|| "no driver loaded".to_string());
                let mem = g.vram.filter(|v| *v > 0).map(|v| format!(" · {}", sysinfo::human_bytes(v))).unwrap_or_default();
                format!("{name} · {driver}{mem}")
            })
            .collect()
    }

    pub fn new(_cx: &AppCx) -> SystemApp {
        SystemApp::with_roots(SysRoots::real(), "/var/lib/iyakki".into())
    }

    pub fn with_roots(roots: SysRoots, data_dir: std::path::PathBuf) -> SystemApp {
        SystemApp { page: Page::Overview, roots, data_dir, prev: None, cur: None, history: Vec::new(), proc_cpu: HashMap::new(), net_rate: (0.0, 0.0), mac_line: None, sound_line: String::new(), gpu_lines: Vec::new(), sort: Sort::Cpu, sel: [0; 4], hardware: Vec::new(), confirm: None, status: None, live: Vec::new(), last_refresh: None, cluster_log: None, health_busy: Default::default() }
    }

    pub fn page(&self) -> Page {
        self.page
    }

    fn is_admin(cx: &AppCx) -> bool {
        cx.session.policy.manage_settings
    }

    fn pages(cx: &AppCx) -> Vec<Page> {
        if Self::is_admin(cx) { vec![Page::Overview, Page::Processes, Page::Agents, Page::Hardware] } else { vec![Page::Overview, Page::Agents] }
    }

    /// Read the kernel's figures now.
    pub fn refresh(&mut self, cx: &AppCx) {
        let now = Instant::now();
        let sample = Sample { at: now, cpu: sysinfo::cpu_times(&self.roots), procs: if Self::is_admin(cx) { sysinfo::processes(&self.roots) } else { Vec::new() }, net: sysinfo::net_counters(&self.roots) };
        if let Some(cur) = self.cur.take() {
            let dt = sample.at.duration_since(cur.at).as_secs_f32();
            self.history.push(sysinfo::cpu_percent(cur.cpu, sample.cpu));
            if self.history.len() > HISTORY {
                self.history.remove(0);
            }
            self.proc_cpu = sysinfo::proc_cpu(&cur.procs, &sample.procs, dt);
            let sum = |v: &[NetCounters]| v.iter().fold((0u64, 0u64), |(r, t), n| (r + n.rx, t + n.tx));
            let ((r0, t0), (r1, t1)) = (sum(&cur.net), sum(&sample.net));
            self.net_rate = (r1.saturating_sub(r0) as f64 / dt.max(0.001) as f64, t1.saturating_sub(t0) as f64 / dt.max(0.001) as f64);
            self.prev = Some(cur);
        }
        self.cur = Some(sample);
        self.mac_line = iyakki_devctl::mac::summary_line(&iyakki_devctl::Roots { sys: self.roots.sys.clone(), proc_: self.roots.proc.clone() });
        self.sound_line = iyakki_agent::audioout::summary_at(&self.roots.proc);
        self.gpu_lines = Self::gpu_summary(&iyakki_agent::gpu::detect(self.roots.sys.parent().unwrap_or(std::path::Path::new("/"))));
        self.live = cx.services.runs.live().into_iter().filter(|r| Self::is_admin(cx) || r.user == cx.session.user).collect();
        self.last_refresh = Some(now);
        // the dev cluster's health is read in the background every 15 s while it runs (a few kubectl calls)
        let k = &cx.services.k3sdev;
        let h = k.health();
        if h.installed && !matches!(h.state.as_str(), "stopped" | "not installed") && h.checked_at < iyakki_agent::clock::now_unix() - 15 && !self.health_busy.swap(true, std::sync::atomic::Ordering::SeqCst) {
            let (k, busy) = (k.clone(), self.health_busy.clone());
            std::thread::spawn(move || {
                k.refresh_health();
                busy.store(false, std::sync::atomic::Ordering::SeqCst);
            });
        }
    }

    /// The processes in the order chosen.
    fn sorted(&self) -> Vec<&Proc> {
        let mut v: Vec<&Proc> = self.cur.as_ref().map(|c| c.procs.iter().collect()).unwrap_or_default();
        let cpu = |p: &Proc| self.proc_cpu.get(&p.pid).copied().unwrap_or(0.0);
        match self.sort {
            Sort::Cpu => v.sort_by(|a, b| cpu(b).partial_cmp(&cpu(a)).unwrap_or(std::cmp::Ordering::Equal).then(b.rss.cmp(&a.rss))),
            Sort::Memory => v.sort_by(|a, b| b.rss.cmp(&a.rss)),
            Sort::Pid => v.sort_by_key(|p| p.pid),
            Sort::Name => v.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()).then(a.pid.cmp(&b.pid))),
        }
        v
    }

    fn list_len(&self) -> usize {
        match self.page {
            Page::Overview => 0,
            Page::Processes => self.cur.as_ref().map_or(0, |c| c.procs.len()),
            Page::Agents => self.live.len(),
            Page::Hardware => self.hardware.len(),
        }
    }

    fn sel_idx(&self) -> usize {
        match self.page {
            Page::Overview => 0,
            Page::Processes => self.sel[1],
            Page::Agents => self.sel[2],
            Page::Hardware => self.sel[3],
        }
    }

    fn set_sel(&mut self, i: usize) {
        let n = self.list_len();
        let i = i.min(n.saturating_sub(1));
        match self.page {
            Page::Overview => {}
            Page::Processes => self.sel[1] = i,
            Page::Agents => self.sel[2] = i,
            Page::Hardware => self.sel[3] = i,
        }
    }

    fn say(&mut self, text: impl Into<String>, ok: bool) {
        self.status = Some((text.into(), ok));
    }

    fn selected_proc(&self) -> Option<Proc> {
        self.sorted().get(self.sel[1]).map(|p| (*p).clone())
    }

    fn ask_signal(&mut self, hard: bool) {
        let Some(p) = self.selected_proc() else { return };
        if let Some(why) = sysinfo::protected(p.pid) {
            self.say(format!("Process {} cannot be stopped: {why}.", p.pid), false);
            return;
        }
        self.confirm = Some(Confirm::Signal { pid: p.pid, name: p.name, hard });
    }

    fn run_confirmed(&mut self, cx: &AppCx) {
        match self.confirm.take() {
            Some(Confirm::Signal { pid, name, hard }) => {
                if !Self::is_admin(cx) {
                    return;
                }
                cx.audit.record(&cx.session.user, if hard { "system.kill" } else { "system.stop" }, &format!("{pid} {name}"));
                match sysinfo::signal(pid, hard) {
                    Ok(()) => self.say(format!("{} {name} ({pid}).", if hard { "Killed" } else { "Asked to stop" }), true),
                    Err(e) => self.say(format!("Could not stop {name} ({pid}): {e}"), false),
                }
                self.refresh(cx);
            }
            Some(Confirm::StopAll) => {
                let only = if Self::is_admin(cx) { None } else { Some(cx.session.user.as_str()) };
                let n = cx.services.runs.stop_all_live(only);
                cx.audit.record(&cx.session.user, "system.agents.stop_all", &n.to_string());
                self.say(format!("Told {n} run{} to stop.", if n == 1 { "" } else { "s" }), true);
                self.refresh(cx);
            }
            None => {}
        }
    }

    /// The service log of the dev cluster (the end of it), as selectable text.
    fn open_cluster_log(&mut self, cx: &AppCx) {
        let t = cx.services.k3sdev.log_tail(400);
        let mut l = crate::apps::ScrollLog::default();
        if t.trim().is_empty() {
            l.push("The log is empty.", DIM);
        }
        for line in t.lines() {
            l.push(line, if line.contains("level=error") || line.contains("Failed") || line.contains("error") { ERR } else { [0, 0, 0, 0] });
        }
        cx.audit.record(&cx.session.user, "devcluster.logs", "");
        self.cluster_log = Some(l);
    }

    fn stop_selected_agent(&mut self, cx: &AppCx) {
        let Some(r) = self.live.get(self.sel[2]).cloned() else { return };
        if !Self::is_admin(cx) && r.user != cx.session.user {
            self.say("That run belongs to someone else.", false);
            return;
        }
        if cx.services.runs.stop_live(&r.id) {
            cx.audit.record(&cx.session.user, "system.agent.stop", &r.user);
            self.say(format!("Stopping {}'s run.", r.user), true);
        } else {
            self.say("That run has already ended.", false);
        }
        self.refresh(cx);
    }
}

fn bar(g: &mut Gfx, r: R, frac: f32, color: [u8; 4], s: f32) {
    crate::fx::progress_bar(g, r, frac, color, s);
}

fn heat(pct: f32) -> [u8; 4] {
    if pct >= 90.0 { ERR } else if pct >= 70.0 { AMBER } else { ACCENT }
}

impl App for SystemApp {
    fn title(&self) -> &'static str {
        "System"
    }
    fn icon(&self) -> Icon {
        Icon::System
    }
    fn accent(&self) -> [u8; 4] {
        ACCENT
    }
    fn hint(&self) -> String {
        match self.page {
            Page::Overview => "Tab or 1-3 pages · figures update every second · l the dev cluster's log · Esc back".into(),
            Page::Processes => "↑↓ choose · c m p n sort · k stop it · K kill it · Tab next page · Esc back".into(),
            Page::Agents => "↑↓ choose · x stop this run · X stop all · Tab next page · Esc back".into(),
            Page::Hardware => "↑↓ PgUp PgDn scroll · r read again · Tab next page · Esc back".into(),
        }
    }
    fn help(&self) -> Vec<String> {
        [
            "Tab / Shift+Tab · next or earlier page (Overview, Processes, Agents, Hardware)",
            "Overview · processor, memory, load, disks, network and heat, updated every second",
            "l · on the Overview, the dev cluster's service log (administrators, when IYAKKI installed a cluster here)",
            "Processes (administrators) · Up / Down choose, c m p n sort by processor, memory, number or name",
            "k · ask the chosen process to stop (SIGTERM)    K · kill it (SIGKILL)    Enter confirms, Esc cancels",
            "Agents · every assistant run going on now, with what it waits for",
            "x · stop the chosen run     X · stop every run (yours, or everyone's for administrators)",
            "Hardware (administrators) · what the kernel found, PCI devices with their drivers, loaded drivers; r reads it again",
            "Process 1 (the system) and this shell are never touched.",
        ]
        .iter()
        .map(|s| s.to_string())
        .collect()
    }

    fn handle(&mut self, ev: &Event, cx: &AppCx) -> Flow {
        if matches!(ev, Event::Tick) {
            if self.last_refresh.is_none_or(|t| t.elapsed() >= Duration::from_millis(1000)) {
                self.refresh(cx);
            }
            return Flow::Stay;
        }
        // the cluster's log: select with the mouse, scroll, Esc closes
        if let Some(l) = self.cluster_log.as_mut() {
            match ev {
                Event::Key(Key::Esc) | Event::Key(Key::Enter) | Event::Tab(92) => self.cluster_log = None,
                Event::Key(Key::Up) => l.scroll_by(1),
                Event::Key(Key::Down) => l.scroll_by(-1),
                Event::Key(Key::PageUp) => l.scroll_by(10),
                Event::Key(Key::PageDown) => l.scroll_by(-10),
                Event::Scroll(d) => l.scroll_by(*d * 3),
                other => {
                    l.handle(other);
                }
            }
            return Flow::Stay;
        }
        // a question to answer first
        if self.confirm.is_some() {
            match ev {
                Event::Key(Key::Enter) | Event::Key(Key::Char('y')) | Event::Tab(90) => self.run_confirmed(cx),
                Event::Key(Key::Esc) | Event::Key(Key::Char('n')) | Event::Tab(91) => {
                    self.confirm = None;
                }
                _ => {}
            }
            return Flow::Stay;
        }
        let pages = Self::pages(cx);
        let step = |app: &mut SystemApp, d: i32| {
            let i = pages.iter().position(|p| *p == app.page).unwrap_or(0) as i32;
            app.page = pages[(i + d).rem_euclid(pages.len() as i32) as usize];
            app.status = None;
        };
        match ev {
            Event::Key(Key::Esc) => return Flow::Back,
            Event::Key(Key::Tab) => step(self, 1),
            Event::Key(Key::BackTab) => step(self, -1),
            Event::Key(Key::Char(c @ '1'..='4')) => {
                if let Some(p) = pages.get(*c as usize - '1' as usize) {
                    self.page = *p;
                    self.status = None;
                }
            }
            Event::Tab(n) if *n < 4 => {
                if let Some(p) = pages.get(*n) {
                    self.page = *p;
                    self.status = None;
                }
            }
            Event::Key(Key::Up) => self.set_sel(self.sel_idx().saturating_sub(1)),
            Event::Key(Key::Down) => self.set_sel(self.sel_idx() + 1),
            Event::Key(Key::PageUp) => self.set_sel(self.sel_idx().saturating_sub(10)),
            Event::Key(Key::PageDown) => self.set_sel(self.sel_idx() + 10),
            Event::Key(Key::Home) => self.set_sel(0),
            Event::Key(Key::End) => self.set_sel(usize::MAX),
            Event::Row(i) => self.set_sel(*i),
            Event::Key(Key::Char('c')) if self.page == Page::Processes => self.sort = Sort::Cpu,
            Event::Key(Key::Char('m')) if self.page == Page::Processes => self.sort = Sort::Memory,
            Event::Key(Key::Char('p')) if self.page == Page::Processes => self.sort = Sort::Pid,
            Event::Key(Key::Char('n')) if self.page == Page::Processes => self.sort = Sort::Name,
            Event::Key(Key::Char('k')) if self.page == Page::Processes && Self::is_admin(cx) => self.ask_signal(false),
            Event::Key(Key::Char('K')) if self.page == Page::Processes && Self::is_admin(cx) => self.ask_signal(true),
            Event::Key(Key::Char('r')) if self.page == Page::Hardware => self.hardware = hardware_lines(&self.roots),
            Event::Key(Key::Char('l')) | Event::Tab(93) if self.page == Page::Overview && Self::is_admin(cx) && cx.services.k3sdev.installed().is_some() => self.open_cluster_log(cx),
            Event::Key(Key::Char('x')) if self.page == Page::Agents => self.stop_selected_agent(cx),
            Event::Key(Key::Char('X')) if self.page == Page::Agents => {
                if self.live.is_empty() {
                    self.say("No run is going on.", false);
                } else {
                    self.confirm = Some(Confirm::StopAll);
                }
            }
            Event::Tab(20) => self.ask_signal(false),
            Event::Tab(21) => self.ask_signal(true),
            Event::Tab(22) => self.stop_selected_agent(cx),
            Event::Tab(23) if !self.live.is_empty() => self.confirm = Some(Confirm::StopAll),
            _ => {}
        }
        Flow::Stay
    }

    fn render(&mut self, g: &mut Gfx, area: R, s: f32, cx: &AppCx, _tick: u64) {
        if self.cur.is_none() {
            self.refresh(cx);
        }
        let (x, w) = (area.x + 24.0 * s, area.w - 48.0 * s);
        let mut y = area.y + 10.0 * s;
        // the page tabs
        let items: Vec<(String, Event)> = Self::pages(cx)
            .iter()
            .enumerate()
            .map(|(i, p)| {
                let label = match p {
                    Page::Overview => "Overview",
                    Page::Processes => "Processes",
                    Page::Agents => "Agents",
                    Page::Hardware => "Hardware",
                };
                (label.to_string(), Event::Tab(i))
            })
            .collect();
        let cur = Self::pages(cx).iter().position(|p| *p == self.page).unwrap_or(0);
        crate::fx::tab_strip(g, x, y, s, w, &items, cur, ACCENT);
        y += 52.0 * s;
        let body = R::new(x, y, w, area.y + area.h - y - 34.0 * s);
        match self.page {
            Page::Overview => self.render_overview(g, body, s, cx),
            Page::Processes => self.render_processes(g, body, s),
            Page::Agents => self.render_agents(g, body, s),
            Page::Hardware => self.render_hardware(g, body, s),
        }
        // the line at the bottom: the answer to the last action
        if let Some((t, ok)) = &self.status {
            crate::fx::status_line(g, x, area.y + area.h - 26.0 * s, w, s, t, *ok);
        }
        if let Some(l) = self.cluster_log.as_mut() {
            let p = crate::fx::modal(g, area, s, 900.0 * s, area.h - 24.0 * s, ACCENT, false);
            let y0 = crate::fx::modal_title(g, p, s, "The dev cluster's log (select with the mouse, Ctrl+C copies)", ACCENT);
            let bh = 13.0 * s * 2.1;
            l.render(g, R::new(p.x + 24.0 * s, y0 + 4.0 * s, p.w - 48.0 * s, (p.y + p.h - y0 - bh - 40.0 * s).max(40.0 * s)), 13.0 * s, Face::Mono);
            g.button(p.x + 24.0 * s, p.y + p.h - bh - 16.0 * s, 13.0 * s, "Close", MUTED, Event::Tab(92));
        }
        if let Some(c) = &self.confirm {
            let text = match c {
                Confirm::Signal { pid, name, hard } => format!("{} {name} (process {pid})?", if *hard { "Kill" } else { "Ask to stop" }),
                Confirm::StopAll => format!("Stop all {} run{}?", self.live.len(), if self.live.len() == 1 { "" } else { "s" }),
            };
            let p = crate::fx::modal(g, area, s, 460.0 * s, 150.0 * s, ACCENT, true);
            let (bx, by, bw, bh) = (p.x, p.y, p.w, p.h);
            let t = g.fit(18.0 * s, &text, Face::Bold, bw - 40.0 * s);
            g.text(bx + 20.0 * s, by + 22.0 * s, 18.0 * s, TEXT, &t, Face::Bold);
            g.text(bx + 20.0 * s, by + 52.0 * s, 13.0 * s, MUTED, "Enter confirms, Esc cancels.", Face::Sans);
            let w1 = g.button(bx + 20.0 * s, by + bh - 52.0 * s, 15.0 * s, "Yes", ERR, Event::Tab(90));
            g.button(bx + 30.0 * s + w1, by + bh - 52.0 * s, 15.0 * s, "No", MUTED, Event::Tab(91));
        }
    }
}

impl SystemApp {
    fn render_overview(&mut self, g: &mut Gfx, a: R, s: f32, cx: &AppCx) {
        let m = sysinfo::memory(&self.roots);
        let cpu_now = self.history.last().copied().unwrap_or(0.0);
        let (l1, l5, l15) = sysinfo::load_average(&self.roots);
        let cpus = sysinfo::cpu_count(&self.roots);
        let card_w = (a.w - 2.0 * 12.0 * s) / 3.0;
        let ch = 112.0 * s;
        // processor with its history
        let c = R::new(a.x, a.y, card_w, ch);
        crate::fx::glass(g, c.x, c.y, c.w, c.h, 12.0 * s, ACCENT);
        g.text(c.x + 14.0 * s, c.y + 9.0 * s, 11.0 * s, DIM, "PROCESSOR", Face::Bold);
        g.text(c.x + 14.0 * s, c.y + 26.0 * s, 24.0 * s, heat(cpu_now), &format!("{cpu_now:.0}%"), Face::Bold);
        g.text(c.x + 14.0 * s + 78.0 * s, c.y + 34.0 * s, 13.0 * s, MUTED, &format!("{cpus} core{}", if cpus == 1 { "" } else { "s" }), Face::Sans);
        let spark = R::new(c.x + 14.0 * s, c.y + 62.0 * s, c.w - 28.0 * s, 38.0 * s);
        crate::fx::well(g, spark, s);
        let bw = spark.w / HISTORY as f32;
        let offset = HISTORY - self.history.len();
        for (i, v) in self.history.iter().enumerate() {
            let h = (spark.h - 4.0 * s) * v / 100.0;
            g.rect(spark.x + (offset + i) as f32 * bw, spark.y + spark.h - 2.0 * s - h, (bw - 1.0).max(1.0), h.max(1.0), heat(*v));
        }
        // memory
        let c = R::new(a.x + card_w + 12.0 * s, a.y, card_w, ch);
        crate::fx::glass(g, c.x, c.y, c.w, c.h, 12.0 * s, ACCENT);
        g.text(c.x + 14.0 * s, c.y + 9.0 * s, 11.0 * s, DIM, "MEMORY", Face::Bold);
        let frac = if m.total == 0 { 0.0 } else { m.used() as f32 / m.total as f32 };
        g.text(c.x + 14.0 * s, c.y + 26.0 * s, 24.0 * s, heat(frac * 100.0), &format!("{:.0}%", frac * 100.0), Face::Bold);
        g.text(c.x + 14.0 * s + 78.0 * s, c.y + 34.0 * s, 13.0 * s, MUTED, &format!("{} of {}", sysinfo::human_bytes(m.used()), sysinfo::human_bytes(m.total)), Face::Sans);
        bar(g, R::new(c.x + 14.0 * s, c.y + 62.0 * s, c.w - 28.0 * s, 10.0 * s), frac, heat(frac * 100.0), s);
        if m.swap_total > 0 {
            let used = m.swap_total.saturating_sub(m.swap_free);
            g.text(c.x + 14.0 * s, c.y + 76.0 * s, 12.0 * s, DIM, &format!("swap {} of {}", sysinfo::human_bytes(used), sysinfo::human_bytes(m.swap_total)), Face::Sans);
        } else {
            g.text(c.x + 14.0 * s, c.y + 76.0 * s, 12.0 * s, DIM, "no swap", Face::Sans);
        }
        let zram = memory_extra_line(&iyakki_zram::MemStats::read(&iyakki_zram::Env { proc: self.roots.proc.clone(), sys: self.roots.sys.clone(), dev: "/dev".into() }));
        let zram = g.fit(12.0 * s, &zram, Face::Sans, c.w - 28.0 * s);
        g.text(c.x + 14.0 * s, c.y + 92.0 * s, 12.0 * s, DIM, &zram, Face::Sans);
        // load, uptime, heat
        let c = R::new(a.x + 2.0 * (card_w + 12.0 * s), a.y, card_w, ch);
        crate::fx::glass(g, c.x, c.y, c.w, c.h, 12.0 * s, ACCENT);
        g.text(c.x + 14.0 * s, c.y + 9.0 * s, 11.0 * s, DIM, "LOAD AND UPTIME", Face::Bold);
        if iyakki_agent::flows::FlowEnv::scheduler_running(&cx.services, &cx.session.user) {
            let t = "flows scheduler running";
            let w = g.text_width(10.5 * s, t, Face::Sans);
            if w + 150.0 * s < c.w {
                g.text(c.x + c.w - w - 12.0 * s, c.y + 10.0 * s, 10.5 * s, crate::ui::OK, t, Face::Sans);
            }
        }
        g.text(c.x + 14.0 * s, c.y + 26.0 * s, 24.0 * s, TEXT, &format!("{l1:.2}"), Face::Bold);
        g.text(c.x + 14.0 * s + 78.0 * s, c.y + 34.0 * s, 13.0 * s, MUTED, &format!("{l5:.2} · {l15:.2} (5 and 15 min)"), Face::Sans);
        g.text(c.x + 14.0 * s, c.y + 66.0 * s, 14.0 * s, TEXT, &format!("up {}", sysinfo::human_duration(sysinfo::uptime_secs(&self.roots))), Face::Sans);
        match sysinfo::hottest_temp(&self.roots) {
            Some(t) => g.text(c.x + 14.0 * s, c.y + 86.0 * s, 13.0 * s, if t >= 85.0 { ERR } else if t >= 70.0 { AMBER } else { MUTED }, &format!("hottest sensor {t:.0} °C"), Face::Sans),
            None => g.text(c.x + 14.0 * s, c.y + 86.0 * s, 13.0 * s, DIM, "no temperature sensor", Face::Sans),
        };
        // disks, network, agents
        let mut y = a.y + ch + 14.0 * s;
        let rows: Vec<(String, String, f32)> = {
            let mut v = Vec::new();
            for (label, path) in [("Data", self.data_dir.as_path()), ("System", std::path::Path::new("/"))] {
                if let Some((free, total)) = iyakki_storage::space(path) {
                    let used = total.saturating_sub(free);
                    v.push((format!("{label} disk"), format!("{} free of {}", sysinfo::human_bytes(free), sysinfo::human_bytes(total)), if total == 0 { 0.0 } else { used as f32 / total as f32 }));
                }
            }
            v
        };
        for (title, detail, frac) in rows {
            g.text(a.x, y, 14.0 * s, TEXT, &title, Face::Bold);
            bar(g, R::new(a.x + 130.0 * s, y + 4.0 * s, a.w * 0.4, 10.0 * s), frac, heat(frac * 100.0), s);
            g.text(a.x + 130.0 * s + a.w * 0.4 + 14.0 * s, y, 13.0 * s, MUTED, &detail, Face::Sans);
            y += 28.0 * s;
        }
        let (rx, tx) = self.net_rate;
        g.text(a.x, y, 14.0 * s, TEXT, "Network", Face::Bold);
        let rates = format!("↓ {}/s    ↑ {}/s", sysinfo::human_bytes(rx as u64), sysinfo::human_bytes(tx as u64));
        g.text(a.x + 130.0 * s, y, 14.0 * s, MUTED, &rates, Face::Sans);
        if let Some(m) = &self.mac_line {
            // after the rates, when it fits
            let x = a.x + 130.0 * s + g.text_width(14.0 * s, &rates, Face::Sans) + 24.0 * s;
            let room = a.x + a.w - x;
            if room > 120.0 * s {
                let t = g.fit(13.0 * s, m, Face::Sans, room);
                g.text(x, y, 13.0 * s, DIM, &t, Face::Sans);
            }
        }
        y += 28.0 * s;
        g.text(a.x, y, 14.0 * s, TEXT, "Host name", Face::Bold);
        g.text(a.x + 130.0 * s, y, 14.0 * s, MUTED, &crate::hostname::current("iyakki"), Face::Sans);
        y += 28.0 * s;
        for (i, line) in self.gpu_lines.iter().enumerate() {
            g.text(a.x, y, 14.0 * s, TEXT, if i == 0 { "Graphics" } else { "" }, Face::Bold);
            let t = g.fit(13.0 * s, line, Face::Sans, a.w - 130.0 * s);
            g.text(a.x + 130.0 * s, y, 13.0 * s, MUTED, &t, Face::Sans);
            y += 28.0 * s;
        }
        if !self.sound_line.is_empty() {
            g.text(a.x, y, 14.0 * s, TEXT, "Sound", Face::Bold);
            let t = g.fit(13.0 * s, &self.sound_line, Face::Sans, a.w - 130.0 * s);
            g.text(a.x + 130.0 * s, y, 13.0 * s, if self.sound_line.starts_with("no sound output") { AMBER } else { MUTED }, &t, Face::Sans);
            y += 28.0 * s;
        }
        let working = self.live.len();
        let waiting: usize = self.live.iter().map(|r| r.pending).sum();
        g.text(a.x, y, 14.0 * s, TEXT, "Agents", Face::Bold);
        let txt = if working == 0 { "none running".to_string() } else { format!("{working} running{}", if waiting > 0 { format!(", {waiting} waiting for an answer") } else { String::new() }) };
        g.text(a.x + 130.0 * s, y, 14.0 * s, if waiting > 0 { AMBER } else { MUTED }, &txt, Face::Sans);
        // About: the version and, on an installed image, what /etc/iyakki-release says
        y += 28.0 * s;
        g.text(a.x, y, 14.0 * s, TEXT, "About", Face::Bold);
        let about = g.fit(13.0 * s, iyakki_agent::relinfo::machine_about(), Face::Sans, a.w - 130.0 * s);
        g.text(a.x + 130.0 * s, y, 13.0 * s, MUTED, &about, Face::Sans);
        // the dev cluster IYAKKI manages on this machine
        let h = cx.services.k3sdev.health();
        if h.installed {
            y += 28.0 * s;
            g.text(a.x, y, 14.0 * s, TEXT, "Cluster", Face::Bold);
            let col = match h.state.as_str() {
                "running" => ACCENT,
                "failed" => ERR,
                "stopped" => DIM,
                _ => AMBER,
            };
            let line = g.fit(14.0 * s, &h.line(), Face::Sans, a.w - 130.0 * s - 120.0 * s);
            g.text(a.x + 130.0 * s, y, 14.0 * s, col, &line, Face::Sans);
            if Self::is_admin(cx) {
                g.button(a.x + a.w - 110.0 * s, y - 6.0 * s, 13.0 * s, "View logs", ACCENT, Event::Tab(93));
            }
            y += 24.0 * s;
            let mut d: Vec<String> = Vec::new();
            if let Some(b) = h.data_bytes {
                d.push(format!("data {}", sysinfo::human_bytes(b)));
            }
            if let Some(p) = h.disk_pct {
                d.push(format!("disk {p}% full"));
            }
            if !h.failed_reason.is_empty() {
                d.push(h.failed_reason.clone());
            } else if h.restarts > 0 {
                d.push(format!("restarted {}x, last: {}", h.restarts, h.last_restart_reason));
            }
            if !d.is_empty() {
                let t = g.fit(13.0 * s, &d.join(" · "), Face::Sans, a.w - 130.0 * s);
                g.text(a.x + 130.0 * s, y, 13.0 * s, if h.disk_pct.is_some_and(|p| p >= 85) || !h.failed_reason.is_empty() { ERR } else { MUTED }, &t, Face::Sans);
            }
        }
    }

    fn render_processes(&mut self, g: &mut Gfx, a: R, s: f32) {
        let sorted: Vec<&Proc> = self.sorted();
        let header_y = a.y;
        let cols = [("PID", 14.0), ("USER", 84.0), ("CPU", 204.0), ("MEMORY", 274.0), ("PROCESS", 374.0)];
        crate::fx::table_header(g, a.x, header_y, a.w, s, &cols, ACCENT);
        let sorted_by = format!("sorted by {}", self.sort.label());
        let sbw = g.text_width(12.0 * s, &sorted_by, Face::Sans);
        g.text(a.x + a.w - sbw - 6.0 * s, header_y - 1.0 * s, 12.0 * s, MUTED, &sorted_by, Face::Sans);
        let row_h = 30.0 * s;
        let top = header_y + 20.0 * s;
        let rows = (((a.y + a.h - top - 46.0 * s) / row_h) as usize).max(1);
        let sel = self.sel[1].min(sorted.len().saturating_sub(1));
        let first = sel.saturating_sub(rows - 1).min(sorted.len().saturating_sub(rows));
        for (n, p) in sorted.iter().enumerate().skip(first).take(rows) {
            let r = R::new(a.x, top + (n - first) as f32 * row_h, a.w, row_h - 2.0 * s);
            let on = n == sel;
            let hot = g.hovered(r);
            if on || hot {
                crate::fx::row_at(g, r.x, r.y, r.w, r.h, 8.0 * s, on, hot, ACCENT);
            }
            g.hit(r, Event::Row(n), "process");
            let cpu = self.proc_cpu.get(&p.pid).copied().unwrap_or(0.0);
            let ty = r.y + 6.0 * s;
            g.text(a.x + 14.0 * s, ty, 14.0 * s, MUTED, &p.pid.to_string(), Face::Mono);
            let user = g.fit(14.0 * s, &p.user, Face::Sans, 100.0 * s);
            g.text(a.x + 84.0 * s, ty, 14.0 * s, TEXT, &user, Face::Sans);
            g.text(a.x + 204.0 * s, ty, 14.0 * s, if cpu >= 50.0 { AMBER } else { TEXT }, &format!("{cpu:.0}%"), Face::Sans);
            g.text(a.x + 274.0 * s, ty, 14.0 * s, TEXT, &sysinfo::human_bytes(p.rss), Face::Sans);
            // command lines can run to kilobytes: only the start can be seen, and fitting a long string is slow
            let label = if p.cmd.is_empty() { format!("[{}]", p.name) } else { p.cmd.chars().take(300).collect::<String>() };
            let label = g.fit(14.0 * s, &label, Face::Sans, a.w - 374.0 * s - 20.0 * s);
            g.text(a.x + 374.0 * s, ty, 14.0 * s, TEXT, &label, Face::Sans);
        }
        if sorted.is_empty() {
            crate::fx::empty_state(g, R::new(a.x, top, a.w, 160.0 * s), s, Icon::System, "No processes could be read", "The kernel's process list is not available here.", ACCENT);
        }
        // the buttons for the chosen one
        let by = a.y + a.h - 38.0 * s;
        let w1 = g.button(a.x, by, 14.0 * s, "Stop (k)", AMBER, Event::Tab(20));
        g.button(a.x + w1 + 8.0 * s, by, 14.0 * s, "Kill (K)", ERR, Event::Tab(21));
    }

    fn render_hardware(&mut self, g: &mut Gfx, a: R, s: f32) {
        if self.hardware.is_empty() {
            self.hardware = hardware_lines(&self.roots);
        }
        let row_h = 18.0 * s;
        let rows = ((a.h / row_h) as usize).max(1);
        let sel = self.sel[3].min(self.hardware.len().saturating_sub(1));
        let first = sel.saturating_sub(rows - 1).min(self.hardware.len().saturating_sub(rows));
        for (n, line) in self.hardware.iter().enumerate().skip(first).take(rows) {
            let y = a.y + (n - first) as f32 * row_h;
            let heading = !line.is_empty() && !line.starts_with(' ');
            let face = if heading { Face::Bold } else { Face::Mono };
            let t = g.fit(13.0 * s, line, face, a.w - 8.0 * s);
            let color = if heading { ACCENT } else if line.trim_start().starts_with('!') { AMBER } else { TEXT };
            g.text(a.x + 4.0 * s, y, 13.0 * s, color, &t, face);
        }
    }

    fn render_agents(&mut self, g: &mut Gfx, a: R, s: f32) {
        if self.live.is_empty() {
            crate::fx::empty_state(g, a, s, Icon::Runs, "No agent run is going on", "Runs started in CHAT show up here while they work.", ACCENT);
            return;
        }
        let row_h = 62.0 * s;
        let now = iyakki_agent::clock::now_unix();
        let sel = self.sel[2].min(self.live.len() - 1);
        for (n, r) in self.live.iter().enumerate() {
            let rr = R::new(a.x, a.y + n as f32 * (row_h + 4.0 * s), a.w, row_h);
            if rr.y + rr.h > a.y + a.h - 44.0 * s {
                break;
            }
            let hot = g.hovered(rr);
            crate::fx::row_at(g, rr.x, rr.y, rr.w, rr.h, 10.0 * s, n == sel, hot, ACCENT);
            g.hit(rr, Event::Row(n), "agent run");
            let title = g.fit(15.0 * s, &format!("{} · {}", r.user, r.prompt), Face::Bold, rr.w - 24.0 * s);
            g.text(rr.x + 14.0 * s, rr.y + 8.0 * s, 15.0 * s, TEXT, &title, Face::Bold);
            let mut sub = format!("{} · {} steps", sysinfo::human_duration((now - r.started).max(0) as u64), r.steps);
            if r.tokens > 0 {
                sub.push_str(&format!(" · {} tokens", crate::apps::usage::fmt_tokens(r.tokens)));
            }
            sub.push_str(&format!(" · {}", r.last));
            let sub = g.fit(13.0 * s, &sub, Face::Sans, rr.w - 24.0 * s - if r.pending > 0 { 150.0 * s } else { 0.0 });
            g.text(rr.x + 14.0 * s, rr.y + 34.0 * s, 13.0 * s, MUTED, &sub, Face::Sans);
            if r.pending > 0 {
                let t = format!("{} waiting", r.pending);
                let tw = g.text_width(12.0 * s * 1.0, &t, Face::Bold) + 17.0 * s;
                crate::fx::chip(g, rr.x + rr.w - tw - 12.0 * s, rr.y + 32.0 * s, s, &t, AMBER, true);
            }
        }
        let by = a.y + a.h - 38.0 * s;
        let w1 = g.button(a.x, by, 14.0 * s, "Stop this run (x)", AMBER, Event::Tab(22));
        g.button(a.x + w1 + 8.0 * s, by, 14.0 * s, "Stop all (X)", ERR, Event::Tab(23));
    }
}

/// Everything the hardware page shows, as lines: the boot hardware report, the PCI devices with the driver bound to
/// each, and the loaded drivers (kernel modules).
/// The extra line of the MEMORY card: compressed memory (zram) and memory pressure.
pub fn memory_extra_line(m: &iyakki_zram::MemStats) -> String {
    let mut parts: Vec<String> = Vec::new();
    match &m.zram {
        Some(z) if z.disksize > 0 => parts.push(match z.ratio() {
            Some(r) => format!("zram {} {r:.1}:1 ({} in {})", z.algorithm, sysinfo::human_bytes(z.orig), sysinfo::human_bytes(z.compressed)),
            None => format!("zram {} {} (empty)", z.algorithm, sysinfo::human_bytes(z.disksize)),
        }),
        Some(_) => parts.push("zram off".into()),
        None => parts.push("no zram".into()),
    }
    if let Some([p10, _, _]) = m.pressure {
        parts.push(format!("stall {p10:.0}%"));
    }
    parts.join(" · ")
}

pub fn hardware_lines(roots: &SysRoots) -> Vec<String> {
    let report = iyakki_hwinfo::collect(&iyakki_hwinfo::Roots { sys: roots.sys.clone(), proc: roots.proc.clone() });
    let mut out: Vec<String> = report.to_text().lines().map(str::to_string).collect();
    let rd = |p: std::path::PathBuf| std::fs::read_to_string(p).unwrap_or_default().trim().to_string();
    let mut devices = Vec::new();
    if let Ok(dir) = std::fs::read_dir(roots.sys.join("bus/pci/devices")) {
        for e in dir.flatten() {
            let (vendor, device, class) = (rd(e.path().join("vendor")), rd(e.path().join("device")), rd(e.path().join("class")));
            let driver = std::fs::read_link(e.path().join("driver")).ok().and_then(|p| p.file_name().map(|n| n.to_string_lossy().into_owned())).unwrap_or_else(|| "(no driver)".into());
            devices.push(format!("  {}  {} {}  class {}  {}", e.file_name().to_string_lossy(), vendor.trim_start_matches("0x"), device.trim_start_matches("0x"), class.trim_start_matches("0x"), driver));
        }
    }
    devices.sort();
    out.push("PCI DEVICES".into());
    if devices.is_empty() {
        out.push("  none (this machine has no PCI bus, or it is not readable)".into());
    }
    out.extend(devices);
    out.push(String::new());
    let mut modules: Vec<(String, u64, String)> = std::fs::read_to_string(roots.proc.join("modules"))
        .unwrap_or_default()
        .lines()
        .filter_map(|l| {
            let mut f = l.split_whitespace();
            let (name, size) = (f.next()?, f.next()?.parse::<u64>().ok()?);
            let _count = f.next()?;
            let by = f.next().unwrap_or("-").trim_end_matches(',').to_string();
            Some((name.to_string(), size, if by == "-" { String::new() } else { by }))
        })
        .collect();
    modules.sort();
    out.push("LOADED DRIVERS".into());
    if modules.is_empty() {
        out.push("  none loaded as modules (the drivers are built into the kernel)".into());
    }
    for (name, size, by) in modules {
        out.push(format!("  {name:<28} {:>9}  {by}", sysinfo::human_bytes(size)));
    }
    out
}

/// `iyakki top`: the same figures as text, once, for a terminal or a script.
pub fn text_report(roots: &SysRoots, top: usize) -> String {
    let m = sysinfo::memory(roots);
    let (l1, l5, l15) = sysinfo::load_average(roots);
    let first = (sysinfo::cpu_times(roots), sysinfo::processes(roots));
    std::thread::sleep(Duration::from_millis(500));
    let second = (sysinfo::cpu_times(roots), sysinfo::processes(roots));
    let cpu = sysinfo::cpu_percent(first.0, second.0);
    let per = sysinfo::proc_cpu(&first.1, &second.1, 0.5);
    let mut out = format!("up {} · load {l1:.2} {l5:.2} {l15:.2} · processor {cpu:.0}% of {} · memory {} of {} used\n", sysinfo::human_duration(sysinfo::uptime_secs(roots)), sysinfo::cpu_count(roots), sysinfo::human_bytes(m.used()), sysinfo::human_bytes(m.total));
    out.push_str(&format!("{:>7} {:<10} {:>5} {:>9}  {}\n", "PID", "USER", "CPU%", "MEMORY", "PROCESS"));
    let mut procs = second.1;
    procs.sort_by(|a, b| per.get(&b.pid).copied().unwrap_or(0.0).partial_cmp(&per.get(&a.pid).copied().unwrap_or(0.0)).unwrap_or(std::cmp::Ordering::Equal).then(b.rss.cmp(&a.rss)));
    for p in procs.iter().take(top) {
        let name = if p.cmd.is_empty() { format!("[{}]", p.name) } else { p.cmd.chars().take(80).collect() };
        out.push_str(&format!("{:>7} {:<10} {:>5.0} {:>9}  {}\n", p.pid, p.user.chars().take(10).collect::<String>(), per.get(&p.pid).copied().unwrap_or(0.0), sysinfo::human_bytes(p.rss), name));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::apps::tests::cx_with_admin_store;
    use iyakki_agent::agent::{Risk, ToolDef, Tools, Verdict};
    use iyakki_agent::chat::{ChatBackend, Message, ModelTurn, ToolCall, ToolSpec};
    use iyakki_agent::runs::RunSession;
    use iyakki_sec::policy::Role;
    use serde_json::{json, Value};
    use std::sync::Arc;

    /// A made-up machine; `extra` adds a process entry for a real pid.
    fn machine(tag: &str, extra: Option<(u32, &str)>) -> SysRoots {
        let d = std::env::temp_dir().join(format!("iyakki-ui-sysinfo-{}-{tag}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        let (proc, sys) = (d.join("proc"), d.join("sys"));
        std::fs::create_dir_all(proc.join("net")).unwrap();
        std::fs::create_dir_all(&sys).unwrap();
        write_stat(&proc, 100, 800);
        std::fs::write(proc.join("meminfo"), "MemTotal: 8000000 kB\nMemAvailable: 6000000 kB\nSwapTotal: 0 kB\nSwapFree: 0 kB\n").unwrap();
        std::fs::write(proc.join("loadavg"), "0.52 0.40 0.30 1/200 999\n").unwrap();
        std::fs::write(proc.join("uptime"), "93784.55 1.0\n").unwrap();
        std::fs::write(proc.join("net/dev"), "h\nh\n  eth0: 1000 1 0 0 0 0 0 0 500 1 0 0 0 0 0 0\n").unwrap();
        std::fs::write(d.join("passwd"), "root:x:0:0::/root:/bin/sh\n").unwrap();
        let mut procs = vec![(1u32, "iyakki", 500u64, 5000u64), (7, "busy", 100, 9000)];
        if let Some((pid, name)) = extra {
            procs.push((pid, name, 10, 2000));
        }
        for (pid, name, ticks, rss) in procs {
            let p = proc.join(pid.to_string());
            std::fs::create_dir_all(&p).unwrap();
            std::fs::write(p.join("stat"), format!("{pid} ({name}) S 1 0 0 0 0 0 0 0 0 0 {ticks} 0 0 0 20 0 1 0 5 1 {rss} 0\n")).unwrap();
            std::fs::write(p.join("status"), "Uid:\t0\t0\t0\t0\n").unwrap();
            std::fs::write(p.join("cmdline"), format!("/bin/{name}\0")).unwrap();
        }
        SysRoots { proc, sys, passwd: d.join("passwd") }
    }

    fn write_stat(proc: &std::path::Path, busy: u64, idle: u64) {
        std::fs::write(proc.join("stat"), format!("cpu  {busy} 0 0 {idle} 0 0 0 0 0 0\ncpu0 {busy} 0 0 {idle} 0 0 0 0 0 0\n")).unwrap();
    }

    fn bump_ticks(roots: &SysRoots, pid: u32, name: &str, ticks: u64) {
        let p = roots.proc.join(pid.to_string());
        std::fs::write(p.join("stat"), format!("{pid} ({name}) S 1 0 0 0 0 0 0 0 0 0 {ticks} 0 0 0 20 0 1 0 5 1 9000 0\n")).unwrap();
    }

    fn frame(a: &mut SystemApp, cx: &AppCx) -> Gfx {
        let mut g = Gfx::new(1280, 720);
        g.clear(rgb(9, 11, 20));
        a.render(&mut g, R::new(0.0, 72.0, 1280.0, 648.0), 1.0, cx, 0);
        g
    }

    fn key(a: &mut SystemApp, cx: &AppCx, k: Key) {
        a.handle(&Event::Key(k), cx);
    }

    #[test]
    fn the_overview_shows_processor_history_memory_and_the_pages_follow_the_role() {
        let roots = machine("overview", None);
        let cx = cx_with_admin_store("sys1");
        let mut a = SystemApp::with_roots(roots.clone(), std::env::temp_dir());
        a.refresh(&cx);
        write_stat(&roots.proc, 300, 900); // 200 busy of 300 more
        std::thread::sleep(Duration::from_millis(20));
        a.refresh(&cx);
        assert_eq!(a.history.len(), 1);
        assert!((a.history[0] - 66.7).abs() < 0.2, "{:?}", a.history);
        let g = frame(&mut a, &cx);
        assert!(g.hits.iter().filter(|h| h.label.starts_with("Overview") || h.label == "Processes" || h.label == "Agents").count() >= 3, "{:?}", g.hits.iter().map(|h| h.label.clone()).collect::<Vec<_>>());
        // the tab key walks the pages (administrators have four)
        key(&mut a, &cx, Key::Tab);
        assert_eq!(a.page(), Page::Processes);
        key(&mut a, &cx, Key::Tab);
        assert_eq!(a.page(), Page::Agents);
        key(&mut a, &cx, Key::Tab);
        assert_eq!(a.page(), Page::Hardware);
        key(&mut a, &cx, Key::Tab);
        assert_eq!(a.page(), Page::Overview);
        key(&mut a, &cx, Key::Char('3'));
        assert_eq!(a.page(), Page::Agents);
        // an ordinary person has the overview and their own agents, not the process list
        let mut cx2 = cx_with_admin_store("sys2");
        cx2.session.role = Role::User;
        cx2.session.policy = Role::User.template();
        let mut b = SystemApp::with_roots(roots, std::env::temp_dir());
        b.refresh(&cx2);
        assert!(b.cur.as_ref().unwrap().procs.is_empty(), "the process list is never even read for them");
        key(&mut b, &cx2, Key::Tab);
        assert_eq!(b.page(), Page::Agents);
        key(&mut b, &cx2, Key::Char('2'));
        assert_eq!(b.page(), Page::Agents, "there are only two pages");
        key(&mut b, &cx2, Key::Esc);
    }

    #[test]
    fn processes_are_sorted_by_processor_use_and_the_system_cannot_be_stopped_but_a_real_child_can() {
        let mut child = std::process::Command::new("sleep").arg("60").spawn().expect("sleep");
        let roots = machine("procs", Some((child.id(), "sleeper")));
        let cx = cx_with_admin_store("sys3");
        let mut a = SystemApp::with_roots(roots.clone(), std::env::temp_dir());
        a.refresh(&cx);
        bump_ticks(&roots, 7, "busy", 100 + 80);
        std::thread::sleep(Duration::from_millis(50));
        a.refresh(&cx);
        key(&mut a, &cx, Key::Char('2'));
        let order: Vec<u32> = a.sorted().iter().map(|p| p.pid).collect();
        assert_eq!(order[0], 7, "the busy one is first when sorted by processor: {order:?}");
        key(&mut a, &cx, Key::Char('p'));
        assert_eq!(a.sorted().iter().map(|p| p.pid).next(), Some(1));
        key(&mut a, &cx, Key::Char('n'));
        assert_eq!(a.sorted()[0].name, "busy");
        key(&mut a, &cx, Key::Char('m'));
        assert_eq!(a.sorted()[0].pid, 7, "the biggest in memory first");
        let g = frame(&mut a, &cx);
        assert!(g.hits.iter().filter(|h| h.label == "process").count() >= 3);
        // process 1 is refused with a reason
        key(&mut a, &cx, Key::Char('p'));
        a.set_sel(0);
        key(&mut a, &cx, Key::Char('k'));
        assert!(a.confirm.is_none() && a.status.as_ref().unwrap().0.contains("system itself"), "{:?}", a.status);
        // the real child: ask, answer no, ask again, confirm
        let i = a.sorted().iter().position(|p| p.pid == child.id()).unwrap();
        a.set_sel(i);
        key(&mut a, &cx, Key::Char('k'));
        assert!(matches!(a.confirm, Some(Confirm::Signal { hard: false, .. })));
        let _ = frame(&mut a, &cx);
        key(&mut a, &cx, Key::Esc);
        assert!(a.confirm.is_none());
        assert!(child.try_wait().unwrap().is_none(), "nothing happened yet");
        key(&mut a, &cx, Key::Char('K'));
        key(&mut a, &cx, Key::Enter);
        let status = child.wait().unwrap();
        use std::os::unix::process::ExitStatusExt;
        assert_eq!(status.signal(), Some(9), "killed");
        assert!(cx.audit.recent(10).iter().any(|l| l.contains("system.kill") && l.contains("sleeper")));
    }

    struct Stuck;
    impl ChatBackend for Stuck {
        fn complete(&self, _: &[Message]) -> Result<String, String> {
            Err("unused".into())
        }
        fn complete_tools(&self, _: &[Message], _: &[ToolSpec]) -> Result<ModelTurn, String> {
            Ok(ModelTurn { text: String::new(), calls: vec![ToolCall { id: "c1".into(), name: "poke".into(), args: json!({}) }] })
        }
    }
    struct PokeTools;
    impl Tools for PokeTools {
        fn defs(&self) -> Vec<ToolDef> {
            vec![ToolDef { spec: ToolSpec { name: "poke".into(), description: "poke".into(), schema: json!({"type": "object"}) }, risk: Risk::Ask, untrusted: false }]
        }
        fn call(&self, _: &str, _: &Value) -> Result<String, String> {
            Ok("ok".into())
        }
        fn describe(&self, name: &str, _: &Value) -> String {
            name.into()
        }
    }

    fn wait_for(mut f: impl FnMut() -> bool) {
        for _ in 0..500 {
            if f() {
                return;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        panic!("it never happened");
    }

    #[test]
    fn agent_runs_are_listed_with_what_they_wait_for_and_stopped_one_or_all_with_confirmation() {
        let cx = cx_with_admin_store("sys4");
        let store = cx.services.runs.clone();
        let mut mine = RunSession::start(Some(store.clone()), "root", "sort out my invoices", Arc::new(Stuck), Arc::new(PokeTools), vec![], Default::default());
        let mut other = RunSession::start(Some(store.clone()), "someone", "other work", Arc::new(Stuck), Arc::new(PokeTools), vec![], Default::default());
        wait_for(|| {
            mine.poll();
            other.poll();
            store.live().iter().all(|r| r.pending == 1)
        });
        let mut a = SystemApp::with_roots(machine("agents", None), std::env::temp_dir());
        a.refresh(&cx);
        key(&mut a, &cx, Key::Char('3'));
        let g = frame(&mut a, &cx);
        assert_eq!(g.hits.iter().filter(|h| h.label == "agent run").count(), 2, "an administrator sees everyone's runs");
        // a plain user sees only their own
        let mut cx2 = cx_with_admin_store("sys5");
        cx2.services = cx.services.clone();
        cx2.session.user = "someone".into();
        cx2.session.role = Role::User;
        cx2.session.policy = Role::User.template();
        let mut b = SystemApp::with_roots(machine("agents2", None), std::env::temp_dir());
        b.refresh(&cx2);
        assert_eq!(b.live.iter().map(|r| r.user.as_str()).collect::<Vec<_>>(), vec!["someone"]);
        // stop the first (root's) run
        a.set_sel(a.live.iter().position(|r| r.user == "root").unwrap());
        key(&mut a, &cx, Key::Char('x'));
        wait_for(|| {
            mine.poll();
            mine.is_done()
        });
        assert_eq!(mine.log().status, "stopped");
        assert!(cx.audit.recent(10).iter().any(|l| l.contains("system.agent.stop")));
        // stop everything needs a yes
        a.refresh(&cx);
        key(&mut a, &cx, Key::Char('X'));
        assert_eq!(a.confirm, Some(Confirm::StopAll));
        key(&mut a, &cx, Key::Esc);
        assert!(!other.is_done());
        key(&mut a, &cx, Key::Char('X'));
        key(&mut a, &cx, Key::Enter);
        wait_for(|| {
            other.poll();
            other.is_done()
        });
        assert!(a.status.as_ref().unwrap().0.contains("Told 1 run"), "{:?}", a.status);
        a.refresh(&cx);
        let g = frame(&mut a, &cx);
        assert!(g.hits.iter().all(|h| h.label != "agent run"));
        let _ = Verdict::Deny;
    }

    #[test]
    fn the_memory_card_shows_zram_ratio_and_pressure() {
        use iyakki_zram::{MemStats, ZramStats};
        let mut m = MemStats::default();
        assert_eq!(memory_extra_line(&m), "no zram");
        m.zram = Some(ZramStats { device: "zram0".into(), algorithm: "zstd".into(), disksize: 4 << 30, orig: 3 << 30, compressed: 1 << 30, mem_used: 1 << 30, ..Default::default() });
        m.pressure = Some([2.4, 1.0, 0.5]);
        assert_eq!(memory_extra_line(&m), "zram zstd 3.0:1 (3.2 GB in 1.1 GB) · stall 2%");
        m.zram.as_mut().unwrap().orig = 0;
        m.zram.as_mut().unwrap().compressed = 0;
        assert!(memory_extra_line(&m).contains("empty"));
        // the card draws at the sizes the shell supports, with the real files of a made-up machine
        let roots = machine("zcard", None);
        let z = roots.sys.join("block/zram0");
        std::fs::create_dir_all(&z).unwrap();
        std::fs::write(z.join("comp_algorithm"), "lzo [zstd]\n").unwrap();
        std::fs::write(z.join("disksize"), "1073741824\n").unwrap();
        std::fs::write(z.join("mm_stat"), "4096000 1024000 1200000 0 0 0 0 0 0\n").unwrap();
        let (cx, _) = (crate::apps::tests::cx_with_admin_store("zcard-cx"), ());
        let mut a = SystemApp::with_roots(roots, std::env::temp_dir());
        for (w, h) in [(1280u32, 720u32), (1024, 600), (800, 480)] {
            let mut g = Gfx::new(w, h);
            a.refresh(&cx);
            a.render(&mut g, R::new(0.0, 72.0, w as f32, h as f32 - 72.0), (w as f32 / 1280.0).min(h as f32 / 720.0).clamp(0.6, 3.0), &cx, 0);
        }
    }

    #[test]
    fn the_text_report_lists_the_busiest_processes() {
        let roots = machine("report", None);
        let t = text_report(&roots, 5);
        assert!(t.contains("up 1d 2h") && t.contains("memory 2.0 GB of 8.2 GB used"), "{t}");
        assert!(t.contains("/bin/busy") && t.contains("/bin/iyakki"), "{t}");
    }
}

#[cfg(test)]
mod hardware_tests {
    use super::*;

    #[test]
    fn hardware_lines_list_pci_devices_with_drivers_and_modules() {
        let d = std::env::temp_dir().join(format!("iyakki-hw-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        let dev = d.join("sys/bus/pci/devices/0000:00:02.0");
        std::fs::create_dir_all(&dev).unwrap();
        std::fs::write(dev.join("vendor"), "0x8086\n").unwrap();
        std::fs::write(dev.join("device"), "0x1234\n").unwrap();
        std::fs::write(dev.join("class"), "0x030000\n").unwrap();
        std::fs::create_dir_all(d.join("sys/bus/pci/drivers/i915")).unwrap();
        std::os::unix::fs::symlink(d.join("sys/bus/pci/drivers/i915"), dev.join("driver")).unwrap();
        std::fs::create_dir_all(d.join("proc")).unwrap();
        std::fs::write(d.join("proc/modules"), "snd_hda 49152 2 snd_core,snd_pcm, Live 0x0\nvfat 20480 1 - Live 0x0\n").unwrap();
        let roots = SysRoots { proc: d.join("proc"), sys: d.join("sys"), passwd: d.join("passwd") };
        let lines = hardware_lines(&roots);
        assert!(lines.iter().any(|l| l.contains("0000:00:02.0") && l.contains("8086 1234") && l.ends_with("i915")), "{lines:?}");
        assert!(lines.iter().any(|l| l.starts_with("  snd_hda") && l.contains("snd_core,snd_pcm")), "{lines:?}");
        assert!(lines.iter().any(|l| l.starts_with("  vfat")));
        let _ = std::fs::remove_dir_all(&d);
    }
}

#[cfg(test)]
#[path = "system_cluster_tests.rs"]
mod cluster_tests;
