//! SYSTEM: what the machine is doing. Four pages: the **overview** (processor, memory, load, disks, network, heat, how
//! long it has been up), the **processes** (administrators only: who uses what, stop or kill one) and the **agents** (every
//! run going on now: whose, what it was asked, what it waits for, stop one or all) and the **hardware**
//! (administrators only: what the kernel found, PCI devices with their drivers, loaded drivers). Figures are read from the kernel once a
//! second; nothing is kept.

use std::collections::HashMap;
use std::time::{Duration, Instant};

use iyakki_sysinfo_model::sysprocs::Row;
use iyakki_sysinfo_model::sysprocs::{self, PodCache};
use iyakki_sysinfo_model::{self as sysinfo, CpuTimes, DiskCounters, NetCounters, Proc, SysRoots};
use iyakki_tile_api::system::{LiveRun, MemStats, WasmTask};

use crate::port::{OwnedProc, SystemPort};
use iyakki_uikit::gfx::{rgb, Face, Gfx, Icon};
use iyakki_uikit::ui::{Event, Key, DIM, ERR, MUTED, OK, R, TEXT};

/// What the shell does with the keys the tile did not use.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Flow {
    Stay,
    Back,
}

const ACCENT: [u8; 4] = rgb(176, 226, 92);
const AMBER: [u8; 4] = rgb(255, 184, 77);
const HISTORY: usize = 90;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Page {
    Overview,
    Processes,
    Agents,
    Hardware,
    /// The programs started for the signed-in person (everyone's for an administrator), with their network use.
    Mine,
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
pub enum Confirm {
    Signal { pid: u32, name: String, hard: bool, mine: bool },
    StopAll,
}

pub struct Sample {
    pub at: Instant,
    pub cpu: CpuTimes,
    pub procs: Vec<Proc>,
    pub net: Vec<NetCounters>,
    pub disk: Vec<DiskCounters>,
}

pub struct SystemApp {
    pub page: Page,
    pub roots: SysRoots,
    pub data_dir: std::path::PathBuf,
    pub prev: Option<Sample>,
    pub cur: Option<Sample>,
    /// Processor use over the last samples, 0 to 100.
    pub history: Vec<f32>,
    pub proc_cpu: HashMap<u32, f32>,
    /// Bytes per second received and sent, summed over the interfaces.
    pub net_rate: (f64, f64),
    pub disk_rate: (f64, f64),
    /// "MAC: aa:bb:cc:dd:ee:ff (private)" for the adapter in use, read with the other figures.
    pub mac_line: Option<String>,
    /// The compressed-memory line, read through the port at each refresh.
    pub zram_line: String,
    /// Where sound plays and why (Settings -> Sound), read with the other figures.
    pub sound_line: String,
    /// One line per graphics card (name, driver, memory), read with the other figures.
    pub gpu_lines: Vec<String>,
    /// The machine's name and the one line about it, read with the other figures.
    pub host_name: String,
    pub about_line: String,
    pub sort: Sort,
    pub sel: [usize; 5],
    /// The hardware page, read when it is opened or refreshed.
    pub hardware: Vec<String>,
    pub confirm: Option<Confirm>,
    pub status: Option<(String, bool)>,
    pub live: Vec<LiveRun>,
    /// WebAssembly tasks of the supervisor running now (F-SYS-01).
    pub wasm: Vec<WasmTask>,
    pub last_refresh: Option<Instant>,
    /// The dev cluster's service log (View logs), selectable text.
    pub cluster_log: Option<iyakki_uikit::scrolllog::ScrollLog>,
    pub health_busy: std::sync::Arc<std::sync::atomic::AtomicBool>,
    /// Pod uid to service table of the dev cluster, kept fresh in the background (empty when the cluster is off).
    pub pods: PodCache,
    /// The processes with their service label and near-limit marker, by pid.
    pub rows: HashMap<u32, Row>,
    /// Processes tab: one line per service instead of one per process.
    pub grouped: bool,
    /// The Mine page: the programs started for the person (F-SYS-01), read while the page is open.
    pub mine: Vec<OwnedProc>,
    pub mine_read: bool,
    /// Network use of the first rows of the Processes page, read while the page is open.
    pub nets: HashMap<u32, sysinfo::NetUse>,
    /// Title of the log window (the cluster's own log, or what a pod is busy with).
    pub log_title: String,
}

impl SystemApp {
    pub fn with_roots(roots: SysRoots, data_dir: std::path::PathBuf) -> SystemApp {
        SystemApp { page: Page::Overview, roots, data_dir, prev: None, cur: None, history: Vec::new(), proc_cpu: HashMap::new(), net_rate: (0.0, 0.0), disk_rate: (0.0, 0.0), mac_line: None, zram_line: String::new(), sound_line: String::new(), gpu_lines: Vec::new(), host_name: String::new(), about_line: String::new(), sort: Sort::Cpu, sel: [0; 5], hardware: Vec::new(), confirm: None, status: None, live: Vec::new(), wasm: Vec::new(), last_refresh: None, cluster_log: None, health_busy: Default::default(), pods: PodCache::new(), rows: HashMap::new(), grouped: false, mine: Vec::new(), mine_read: false, nets: HashMap::new(), log_title: String::new() }
    }

    pub fn page(&self) -> Page {
        self.page
    }

    fn pages(port: &dyn SystemPort) -> Vec<Page> {
        if port.is_admin() { vec![Page::Overview, Page::Processes, Page::Agents, Page::Hardware, Page::Mine] } else { vec![Page::Overview, Page::Agents, Page::Mine] }
    }

    pub fn refresh_with(&mut self, port: &dyn SystemPort) {
        let now = Instant::now();
        let sample = Sample { at: now, cpu: port.host().cpu_times(&self.roots), procs: if port.is_admin() { port.host().processes(&self.roots) } else { Vec::new() }, net: port.host().net_counters(&self.roots), disk: port.host().disk_counters(&self.roots) };
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
            let dsum = |v: &[DiskCounters]| v.iter().fold((0u64, 0u64), |(r, w), d| (r + d.read, w + d.written));
            let ((r0, w0), (r1, w1)) = (dsum(&cur.disk), dsum(&sample.disk));
            self.disk_rate = (r1.saturating_sub(r0) as f64 / dt.max(0.001) as f64, w1.saturating_sub(w0) as f64 / dt.max(0.001) as f64);
            self.prev = Some(cur);
        }
        self.cur = Some(sample);
        if port.is_admin() {
            self.pods.poll(port.cluster_kubectl(), port.cluster_running(), iyakki_clock::now_unix().max(0) as u64);
            let table = self.pods.table();
            self.rows = self.cur.as_ref().map(|c| sysprocs::rows(&c.procs, &self.proc_cpu, &table).into_iter().map(|r| (r.pid, r)).collect()).unwrap_or_default();
            if let Some((pod, text)) = self.pods.take_answer() {
                let mut l = iyakki_uikit::scrolllog::ScrollLog::default();
                for line in text.lines() {
                    l.push(line, [0, 0, 0, 0]);
                }
                self.log_title = format!("What {pod} is busy with (select with the mouse, Ctrl+C copies)");
                self.cluster_log = Some(l);
            }
        }
        self.zram_line = memory_extra_line(&port.memory_stats());
        self.mac_line = port.mac_summary();
        if self.page == Page::Hardware && self.hardware.is_empty() {
            self.hardware = port.hardware_report();
        }
        self.sound_line = port.audio_summary(&self.roots.proc);
        self.gpu_lines = port.gpu_lines(self.roots.sys.parent().unwrap_or(std::path::Path::new("/")));
        self.host_name = port.hostname();
        self.about_line = port.machine_about();
        self.live = port.live_runs().into_iter().filter(|r| port.is_admin() || r.user == port.user()).collect();
        self.wasm = port.wasm_tasks().into_iter().filter(|t| port.is_admin() || t.user == port.user()).collect();
        self.last_refresh = Some(now);
        if self.page == Page::Mine {
            self.refresh_mine(port);
        }
        if self.page == Page::Processes && port.is_admin() && !self.grouped {
            let pids: Vec<u32> = self.sorted().iter().take(40).map(|p| p.pid).collect();
            self.nets = port.host().net_use(&self.roots, &pids);
        }
        // the dev cluster's health is read in the background every 15 s while it runs (a few kubectl calls)
        let h = port.cluster_health();
        if h.installed && !matches!(h.state.as_str(), "stopped" | "not installed") && h.checked_at < iyakki_clock::now_unix() - 15 && !self.health_busy.swap(true, std::sync::atomic::Ordering::SeqCst) {
            let (refresh, busy) = (port.cluster_refresh_health(), self.health_busy.clone());
            std::thread::spawn(move || {
                refresh();
                busy.store(false, std::sync::atomic::Ordering::SeqCst);
            });
        }
    }

    /// The programs started for this person (everyone's for an administrator), read now.
    fn refresh_mine(&mut self, port: &dyn SystemPort) {
        self.mine = port.programs_mine(&self.roots);
        self.mine_read = true;
    }

    /// The processes in the order chosen.
    pub fn sorted(&self) -> Vec<&Proc> {
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
            Page::Processes if self.grouped => self.sorted_groups().len(),
            Page::Processes => self.cur.as_ref().map_or(0, |c| c.procs.len()),
            Page::Agents => self.live.len(),
            Page::Hardware => self.hardware.len(),
            Page::Mine => self.mine.len(),
        }
    }

    fn sel_idx(&self) -> usize {
        match self.page {
            Page::Overview => 0,
            Page::Processes => self.sel[1],
            Page::Agents => self.sel[2],
            Page::Hardware => self.sel[3],
            Page::Mine => self.sel[4],
        }
    }

    pub fn set_sel(&mut self, i: usize) {
        let n = self.list_len();
        let i = i.min(n.saturating_sub(1));
        match self.page {
            Page::Overview => {}
            Page::Processes => self.sel[1] = i,
            Page::Agents => self.sel[2] = i,
            Page::Hardware => self.sel[3] = i,
            Page::Mine => self.sel[4] = i,
        }
    }

    fn say(&mut self, text: impl Into<String>, ok: bool) {
        self.status = Some((text.into(), ok));
    }

    fn selected_proc(&self) -> Option<Proc> {
        self.sorted().get(self.sel[1]).map(|p| (*p).clone())
    }

    fn ask_signal(&mut self, hard: bool) {
        if self.page == Page::Mine {
            let Some(o) = self.mine.get(self.sel[4]) else { return };
            if let Some(why) = sysinfo::protected(o.proc.pid) {
                self.say(format!("Process {} cannot be stopped: {why}.", o.proc.pid), false);
                return;
            }
            self.confirm = Some(Confirm::Signal { pid: o.proc.pid, name: o.proc.name.clone(), hard, mine: true });
            return;
        }
        if self.grouped {
            self.say("Switch grouping off (g) to stop one process.", false);
            return;
        }
        let Some(p) = self.selected_proc() else { return };
        if let Some(why) = sysinfo::protected(p.pid) {
            self.say(format!("Process {} cannot be stopped: {why}.", p.pid), false);
            return;
        }
        self.confirm = Some(Confirm::Signal { pid: p.pid, name: p.name, hard, mine: false });
    }

    fn run_confirmed(&mut self, port: &dyn SystemPort) {
        match self.confirm.take() {
            Some(Confirm::Signal { pid, name, hard, mine }) => {
                if !mine && !port.is_admin() {
                    return;
                }
                port.audit(if hard { "system.kill" } else { "system.stop" }, &format!("{pid} {name}"));
                // from My processes the ownership is checked again against the machine as it is now
                let r = if mine { port.program_stop(&self.roots, pid, hard).map(|_| ()) } else { port.host().signal(pid, hard) };
                match r {
                    Ok(()) => self.say(format!("{} {name} ({pid}).", if hard { "Killed" } else { "Asked to stop" }), true),
                    Err(e) => self.say(format!("Could not stop {name} ({pid}): {e}"), false),
                }
                self.refresh_with(port);
            }
            Some(Confirm::StopAll) => {
                let user = port.user();
                let only = if port.is_admin() { None } else { Some(user.as_str()) };
                let n = port.stop_all_live(only);
                port.audit("system.agents.stop_all", &n.to_string());
                self.say(format!("Told {n} run{} to stop.", if n == 1 { "" } else { "s" }), true);
                self.refresh_with(port);
            }
            None => {}
        }
    }

    /// The service log of the dev cluster (the end of it), as selectable text.
    fn open_cluster_log(&mut self, port: &dyn SystemPort) {
        let t = port.cluster_log_tail(400);
        let mut l = iyakki_uikit::scrolllog::ScrollLog::default();
        if t.trim().is_empty() {
            l.push("The log is empty.", DIM);
        }
        for line in t.lines() {
            l.push(line, if line.contains("level=error") || line.contains("Failed") || line.contains("error") { ERR } else { [0, 0, 0, 0] });
        }
        port.audit("devcluster.logs", "");
        self.log_title = "The dev cluster's log (select with the mouse, Ctrl+C copies)".into();
        self.cluster_log = Some(l);
    }

    /// The groups of the grouped view, in the order chosen.
    pub fn sorted_groups(&self) -> Vec<sysprocs::Group> {
        let mut rows: Vec<Row> = self.rows.values().cloned().collect();
        rows.sort_by_key(|r| r.pid);
        let mut g = sysprocs::groups(&rows);
        match self.sort {
            Sort::Cpu => g.sort_by(|a, b| b.cpu.partial_cmp(&a.cpu).unwrap_or(std::cmp::Ordering::Equal).then(b.rss.cmp(&a.rss))),
            Sort::Memory => {}
            Sort::Pid | Sort::Name => g.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase())),
        }
        g
    }

    /// The pod behind the chosen line (process or group), if it runs in the cluster.
    pub fn selected_pod(&self) -> Option<(String, String)> {
        if self.grouped {
            self.sorted_groups().get(self.sel[1]).and_then(|g| g.pod.clone())
        } else {
            let p = self.sorted().get(self.sel[1]).map(|p| p.pid)?;
            self.rows.get(&p)?.label.as_ref().map(|l| (l.ns.clone(), l.pod.clone()))
        }
    }

    /// "What is this busy with": the pod's recent log and usage numbers, read in the background.
    fn ask_busy(&mut self, port: &dyn SystemPort) {
        if !port.is_admin() {
            return;
        }
        let Some((ns, pod)) = self.selected_pod() else {
            self.say("This line is not a pod of the dev cluster.", false);
            return;
        };
        port.audit("system.pod_busy", &format!("{ns}/{pod}"));
        self.pods.ask_busy(port.cluster_kubectl(), ns, pod);
        self.say("Reading the pod's log and usage...", true);
    }

    fn stop_selected_agent(&mut self, port: &dyn SystemPort) {
        let Some(r) = self.live.get(self.sel[2]).cloned() else { return };
        if !port.is_admin() && r.user != port.user() {
            self.say("That run belongs to someone else.", false);
            return;
        }
        if port.stop_live(&r.id) {
            port.audit("system.agent.stop", &r.user);
            self.say(format!("Stopping {}'s run.", r.user), true);
        } else {
            self.say("That run has already ended.", false);
        }
        self.refresh_with(port);
    }
}

fn bar(g: &mut Gfx, r: R, frac: f32, color: [u8; 4], s: f32) {
    iyakki_uikit::fx::progress_bar(g, r, frac, color, s);
}

fn heat(pct: f32) -> [u8; 4] {
    if pct >= 90.0 { ERR } else if pct >= 70.0 { AMBER } else { ACCENT }
}

impl SystemApp {
    pub fn title(&self) -> &'static str {
        "System"
    }
    pub fn icon(&self) -> Icon {
        Icon::System
    }
    pub fn accent(&self) -> [u8; 4] {
        ACCENT
    }
    pub fn hint(&self) -> String {
        match self.page {
            Page::Overview => "Tab or 1-3 pages · figures update every second · l the dev cluster's log · Esc back".into(),
            Page::Processes => "↑↓ choose · c m p n sort · g by service · b what is it busy with · k stop · K kill · Tab next page · Esc back".into(),
            Page::Agents => "↑↓ choose · x stop this run · X stop all · Tab next page · Esc back".into(),
            Page::Hardware => "↑↓ PgUp PgDn scroll · r read again · Tab next page · Esc back".into(),
            Page::Mine => "↑↓ choose · k stop · K kill · Tab next page · Esc back".into(),
        }
    }
    pub fn help(&self) -> Vec<String> {
        [
            "Tab / Shift+Tab · next or earlier page (Overview, Processes, Agents, Hardware)",
            "Overview · processor, memory, load, disks, network and heat, updated every second",
            "l · on the Overview, the dev cluster's service log (administrators, when IYAKKI installed a cluster here)",
            "Processes (administrators) · Up / Down choose, c m p n sort by processor, memory, number or name",
            "k · ask the chosen process to stop (SIGTERM)    K · kill it (SIGKILL)    Enter confirms, Esc cancels",
            "Agents · every assistant run going on now, with what it waits for",
            "x · stop the chosen run     X · stop every run (yours, or everyone's for administrators)",
            "Hardware (administrators) · what the kernel found, PCI devices with their drivers, loaded drivers; r reads it again",
            "g · group the processes by service (the pod and Quick install service of a cluster process); b · show what the chosen pod is busy with (log tail, kubectl top)",
            "Mine · the programs started for you (by the assistant or in a terminal) and what they started, with their network use; k stops, K kills one of them",
            "Process 1 (the system) and this shell are never touched.",
        ]
        .iter()
        .map(|s| s.to_string())
        .collect()
    }
}

impl SystemApp {
    pub fn handle_with(&mut self, ev: &Event, port: &dyn SystemPort) -> Flow {
        if matches!(ev, Event::Tick) {
            if self.last_refresh.is_none_or(|t| t.elapsed() >= Duration::from_millis(1000)) {
                self.refresh_with(port);
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
                Event::Key(Key::Enter) | Event::Key(Key::Char('y')) | Event::Tab(90) => self.run_confirmed(port),
                Event::Key(Key::Esc) | Event::Key(Key::Char('n')) | Event::Tab(91) => {
                    self.confirm = None;
                }
                _ => {}
            }
            return Flow::Stay;
        }
        let pages = Self::pages(port);
        let step = |app: &mut SystemApp, d: i32| {
            let i = pages.iter().position(|p| *p == app.page).unwrap_or(0) as i32;
            app.page = pages[(i + d).rem_euclid(pages.len() as i32) as usize];
            app.status = None;
        };
        match ev {
            Event::Key(Key::Esc) => return Flow::Back,
            Event::Key(Key::Tab) => step(self, 1),
            Event::Key(Key::BackTab) => step(self, -1),
            Event::Key(Key::Char(c @ '1'..='5')) => {
                if let Some(p) = pages.get(*c as usize - '1' as usize) {
                    self.page = *p;
                    self.status = None;
                }
            }
            Event::Tab(n) if *n < 5 => {
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
            Event::Key(Key::Char('g')) | Event::Tab(24) if self.page == Page::Processes => {
                self.grouped = !self.grouped;
                self.sel[1] = 0;
            }
            Event::Key(Key::Char('b')) | Event::Tab(25) if self.page == Page::Processes => self.ask_busy(port),
            Event::Key(Key::Char('k')) if (self.page == Page::Processes && port.is_admin()) || self.page == Page::Mine => self.ask_signal(false),
            Event::Key(Key::Char('K')) if (self.page == Page::Processes && port.is_admin()) || self.page == Page::Mine => self.ask_signal(true),
            Event::Key(Key::Char('r')) if self.page == Page::Hardware => self.hardware = port.hardware_report(),
            Event::Key(Key::Char('l')) | Event::Tab(93) if self.page == Page::Overview && port.is_admin() && port.cluster_installed() => self.open_cluster_log(port),
            Event::Key(Key::Char('x')) if self.page == Page::Agents => self.stop_selected_agent(port),
            Event::Key(Key::Char('X')) if self.page == Page::Agents => {
                if self.live.is_empty() {
                    self.say("No run is going on.", false);
                } else {
                    self.confirm = Some(Confirm::StopAll);
                }
            }
            Event::Tab(20) => self.ask_signal(false),
            Event::Tab(21) => self.ask_signal(true),
            Event::Tab(22) => self.stop_selected_agent(port),
            Event::Tab(23) if !self.live.is_empty() => self.confirm = Some(Confirm::StopAll),
            _ => {}
        }
        Flow::Stay
    }

    pub fn render_with(&mut self, g: &mut Gfx, area: R, s: f32, port: &dyn SystemPort) {
        if self.cur.is_none() {
            self.refresh_with(port);
        }
        let (x, w) = (area.x + 24.0 * s, area.w - 48.0 * s);
        let mut y = area.y + 10.0 * s;
        // the page tabs
        let items: Vec<(String, Event)> = Self::pages(port)
            .iter()
            .enumerate()
            .map(|(i, p)| {
                let label = match p {
                    Page::Overview => "Overview",
                    Page::Processes => "Processes",
                    Page::Agents => "Agents",
                    Page::Hardware => "Hardware",
                    Page::Mine => "Mine",
                };
                (label.to_string(), Event::Tab(i))
            })
            .collect();
        let cur = Self::pages(port).iter().position(|p| *p == self.page).unwrap_or(0);
        iyakki_uikit::fx::tab_strip(g, x, y, s, w, &items, cur, ACCENT);
        y += 52.0 * s;
        let body = R::new(x, y, w, area.y + area.h - y - 34.0 * s);
        match self.page {
            Page::Overview => self.render_overview(g, body, s, port),
            Page::Processes => self.render_processes(g, body, s),
            Page::Agents => self.render_agents(g, body, s),
            Page::Hardware => self.render_hardware(g, body, s),
            Page::Mine => self.render_mine(g, body, s, port),
        }
        // the line at the bottom: the answer to the last action
        if let Some((t, ok)) = &self.status {
            iyakki_uikit::fx::status_line(g, x, area.y + area.h - 26.0 * s, w, s, t, *ok);
        }
        if let Some(l) = self.cluster_log.as_mut() {
            let p = iyakki_uikit::fx::modal(g, area, s, 900.0 * s, area.h - 24.0 * s, ACCENT, false);
            let y0 = iyakki_uikit::fx::modal_title(g, p, s, &self.log_title.clone(), ACCENT);
            let bh = 13.0 * s * 2.1;
            l.render(g, R::new(p.x + 24.0 * s, y0 + 4.0 * s, p.w - 48.0 * s, (p.y + p.h - y0 - bh - 40.0 * s).max(40.0 * s)), 13.0 * s, Face::Mono);
            g.button(p.x + 24.0 * s, p.y + p.h - bh - 16.0 * s, 13.0 * s, "Close", MUTED, Event::Tab(92));
        }
        if let Some(c) = &self.confirm {
            let text = match c {
                Confirm::Signal { pid, name, hard, .. } => format!("{} {name} (process {pid})?", if *hard { "Kill" } else { "Ask to stop" }),
                Confirm::StopAll => format!("Stop all {} run{}?", self.live.len(), if self.live.len() == 1 { "" } else { "s" }),
            };
            let p = iyakki_uikit::fx::modal(g, area, s, 460.0 * s, 150.0 * s, ACCENT, true);
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
    fn render_overview(&mut self, g: &mut Gfx, a: R, s: f32, port: &dyn SystemPort) {
        let m = port.host().memory_of(&self.roots);
        let cpu_now = self.history.last().copied().unwrap_or(0.0);
        let (l1, l5, l15) = port.host().load_average(&self.roots);
        let cpus = port.host().cpu_count(&self.roots);
        let card_w = (a.w - 2.0 * 12.0 * s) / 3.0;
        let ch = 112.0 * s;
        // processor with its history
        let c = R::new(a.x, a.y, card_w, ch);
        iyakki_uikit::fx::glass(g, c.x, c.y, c.w, c.h, 12.0 * s, ACCENT);
        g.text(c.x + 14.0 * s, c.y + 9.0 * s, 11.0 * s, DIM, "PROCESSOR", Face::Bold);
        g.text(c.x + 14.0 * s, c.y + 26.0 * s, 24.0 * s, heat(cpu_now), &format!("{cpu_now:.0}%"), Face::Bold);
        let cores = g.fit(13.0 * s, &format!("{cpus} core{}", if cpus == 1 { "" } else { "s" }), Face::Sans, (c.w - 28.0 * s - 78.0 * s).max(30.0 * s));
        g.text(c.x + 14.0 * s + 78.0 * s, c.y + 34.0 * s, 13.0 * s, MUTED, &cores, Face::Sans);
        let spark = R::new(c.x + 14.0 * s, c.y + 62.0 * s, c.w - 28.0 * s, 38.0 * s);
        iyakki_uikit::fx::well(g, spark, s);
        let bw = spark.w / HISTORY as f32;
        let offset = HISTORY - self.history.len();
        for (i, v) in self.history.iter().enumerate() {
            let h = (spark.h - 4.0 * s) * v / 100.0;
            g.rect(spark.x + (offset + i) as f32 * bw, spark.y + spark.h - 2.0 * s - h, (bw - 1.0).max(1.0), h.max(1.0), heat(*v));
        }
        // memory
        let c = R::new(a.x + card_w + 12.0 * s, a.y, card_w, ch);
        iyakki_uikit::fx::glass(g, c.x, c.y, c.w, c.h, 12.0 * s, ACCENT);
        g.text(c.x + 14.0 * s, c.y + 9.0 * s, 11.0 * s, DIM, "MEMORY", Face::Bold);
        let frac = if m.total == 0 { 0.0 } else { m.used() as f32 / m.total as f32 };
        g.text(c.x + 14.0 * s, c.y + 26.0 * s, 24.0 * s, heat(frac * 100.0), &format!("{:.0}%", frac * 100.0), Face::Bold);
        let used_of = g.fit(13.0 * s, &format!("{} of {}", sysinfo::human_bytes(m.used()), sysinfo::human_bytes(m.total)), Face::Sans, (c.w - 28.0 * s - 78.0 * s).max(30.0 * s));
        g.text(c.x + 14.0 * s + 78.0 * s, c.y + 34.0 * s, 13.0 * s, MUTED, &used_of, Face::Sans);
        bar(g, R::new(c.x + 14.0 * s, c.y + 62.0 * s, c.w - 28.0 * s, 10.0 * s), frac, heat(frac * 100.0), s);
        if m.swap_total > 0 {
            let used = m.swap_total.saturating_sub(m.swap_free);
            let swap = g.fit(12.0 * s, &format!("swap {} of {}", sysinfo::human_bytes(used), sysinfo::human_bytes(m.swap_total)), Face::Sans, c.w - 28.0 * s);
            g.text(c.x + 14.0 * s, c.y + 76.0 * s, 12.0 * s, DIM, &swap, Face::Sans);
        } else {
            g.text(c.x + 14.0 * s, c.y + 76.0 * s, 12.0 * s, DIM, "no swap", Face::Sans);
        }
        let zram = g.fit(12.0 * s, &self.zram_line, Face::Sans, c.w - 28.0 * s);
        g.text(c.x + 14.0 * s, c.y + 92.0 * s, 12.0 * s, DIM, &zram, Face::Sans);
        // load, uptime, heat
        let c = R::new(a.x + 2.0 * (card_w + 12.0 * s), a.y, card_w, ch);
        iyakki_uikit::fx::glass(g, c.x, c.y, c.w, c.h, 12.0 * s, ACCENT);
        let t = g.fit(11.0 * s, "LOAD AND UPTIME", Face::Bold, c.w - 28.0 * s);
        g.text(c.x + 14.0 * s, c.y + 9.0 * s, 11.0 * s, DIM, &t, Face::Bold);
        if port.scheduler_running() {
            let t = "flows scheduler running";
            let w = g.text_width(10.5 * s, t, Face::Sans);
            if w + 150.0 * s < c.w {
                g.text(c.x + c.w - w - 12.0 * s, c.y + 10.0 * s, 10.5 * s, OK, t, Face::Sans);
            }
        }
        g.text(c.x + 14.0 * s, c.y + 26.0 * s, 24.0 * s, TEXT, &format!("{l1:.2}"), Face::Bold);
        // beside the big number when the card is wide enough for the words, left out when it is not (the number says it)
        if c.w - 92.0 * s - 8.0 * s >= 60.0 * s {
            let t = g.fit(13.0 * s, &format!("{l5:.2} · {l15:.2} (5 and 15 min)"), Face::Sans, c.w - 92.0 * s - 8.0 * s);
            g.text(c.x + 14.0 * s + 78.0 * s, c.y + 34.0 * s, 13.0 * s, MUTED, &t, Face::Sans);
        }
        let t = g.fit(14.0 * s, &format!("up {}", sysinfo::human_duration(port.host().uptime_of(&self.roots))), Face::Sans, c.w - 28.0 * s);
        g.text(c.x + 14.0 * s, c.y + 66.0 * s, 14.0 * s, TEXT, &t, Face::Sans);
        match port.host().hottest_temp(&self.roots) {
            Some(t) => {
                let line = g.fit(13.0 * s, &format!("hottest sensor {t:.0} °C"), Face::Sans, c.w - 28.0 * s);
                g.text(c.x + 14.0 * s, c.y + 86.0 * s, 13.0 * s, if t >= 85.0 { ERR } else if t >= 70.0 { AMBER } else { MUTED }, &line, Face::Sans)
            }
            None => {
                let line = g.fit(13.0 * s, "no temperature sensor", Face::Sans, c.w - 28.0 * s);
                g.text(c.x + 14.0 * s, c.y + 86.0 * s, 13.0 * s, DIM, &line, Face::Sans)
            }
        };
        // disks, network, agents
        let mut y = a.y + ch + 14.0 * s;
        let rows: Vec<(String, String, f32)> = {
            let mut v = Vec::new();
            for (label, path) in [("Data", self.data_dir.as_path()), ("System", std::path::Path::new("/"))] {
                if let Ok((free, total)) = port.disk_space(&path.to_string_lossy()) {
                    let used = total.saturating_sub(free);
                    v.push((format!("{label} disk"), format!("{} free of {}", sysinfo::human_bytes(free), sysinfo::human_bytes(total)), if total == 0 { 0.0 } else { used as f32 / total as f32 }));
                }
            }
            v
        };
        for (title, detail, frac) in rows {
            g.text(a.x, y, 14.0 * s, TEXT, &title, Face::Bold);
            // the bar gives way to the words when the pane is narrow
            let want = g.text_width(13.0 * s, &detail, Face::Sans);
            let bar_w = iyakki_uikit::safeclamp::SafeClamp::clamp_safe(a.w - 130.0 * s - 14.0 * s - want, 40.0 * s, a.w * 0.4);
            bar(g, R::new(a.x + 130.0 * s, y + 4.0 * s, bar_w, 10.0 * s), frac, heat(frac * 100.0), s);
            let room = (a.w - 130.0 * s - bar_w - 14.0 * s).max(20.0 * s);
            let detail = g.fit(13.0 * s, &detail, Face::Sans, room);
            g.text(a.x + 130.0 * s + bar_w + 14.0 * s, y, 13.0 * s, MUTED, &detail, Face::Sans);
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
        let (dr, dw) = self.disk_rate;
        g.text(a.x, y, 14.0 * s, TEXT, "Disk activity", Face::Bold);
        let t = g.fit(14.0 * s, &format!("read {}/s    write {}/s", sysinfo::human_bytes(dr as u64), sysinfo::human_bytes(dw as u64)), Face::Sans, a.w - 130.0 * s);
        g.text(a.x + 130.0 * s, y, 14.0 * s, MUTED, &t, Face::Sans);
        y += 28.0 * s;
        g.text(a.x, y, 14.0 * s, TEXT, "Host name", Face::Bold);
        let host = g.fit(14.0 * s, &self.host_name, Face::Sans, (a.w - 130.0 * s).max(40.0 * s));
        g.text(a.x + 130.0 * s, y, 14.0 * s, MUTED, &host, Face::Sans);
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
        let about = g.fit(13.0 * s, &self.about_line, Face::Sans, a.w - 130.0 * s);
        g.text(a.x + 130.0 * s, y, 13.0 * s, MUTED, &about, Face::Sans);
        // the dev cluster IYAKKI manages on this machine
        let h = port.cluster_health();
        if h.installed {
            y += 28.0 * s;
            g.text(a.x, y, 14.0 * s, TEXT, "Cluster", Face::Bold);
            let col = match h.state.as_str() {
                "running" => ACCENT,
                "failed" => ERR,
                "stopped" => DIM,
                _ => AMBER,
            };
            let line = g.fit(14.0 * s, &h.line, Face::Sans, a.w - 130.0 * s - 120.0 * s);
            g.text(a.x + 130.0 * s, y, 14.0 * s, col, &line, Face::Sans);
            if port.is_admin() {
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
        if self.grouped {
            return self.render_groups(g, a, s);
        }
        let sorted: Vec<&Proc> = self.sorted();
        let header_y = a.y;
        let cols = [("PID", 14.0), ("USER", 84.0), ("CPU", 184.0), ("MEMORY", 244.0), ("SERVICE / NETWORK", 330.0), ("PROCESS", 590.0)];
        iyakki_uikit::fx::table_header(g, a.x, header_y, a.w, s, &cols, ACCENT);
        let sorted_by = format!("sorted by {}", self.sort.label());
        let sbw = g.text_width(12.0 * s, &sorted_by, Face::Sans);
        g.text(a.x + a.w - sbw - 6.0 * s, header_y - 1.0 * s, 12.0 * s, MUTED, &sorted_by, Face::Sans);
        let row_h = 30.0 * s;
        let top = header_y + 20.0 * s;
        let rows = (((a.y + a.h - top - 66.0 * s) / row_h) as usize).max(1);
        let sel = self.sel[1].min(sorted.len().saturating_sub(1));
        let first = sel.saturating_sub(rows - 1).min(sorted.len().saturating_sub(rows));
        for (n, p) in sorted.iter().enumerate().skip(first).take(rows) {
            let r = R::new(a.x, top + (n - first) as f32 * row_h, a.w, row_h - 2.0 * s);
            let on = n == sel;
            let hot = g.hovered(r);
            if on || hot {
                iyakki_uikit::fx::row_at(g, r.x, r.y, r.w, r.h, 8.0 * s, on, hot, ACCENT);
            }
            g.hit(r, Event::Row(n), "process");
            let cpu = self.proc_cpu.get(&p.pid).copied().unwrap_or(0.0);
            let ty = r.y + 6.0 * s;
            g.text(a.x + 14.0 * s, ty, 14.0 * s, MUTED, &p.pid.to_string(), Face::Mono);
            let user = g.fit(14.0 * s, &p.user, Face::Sans, 90.0 * s);
            g.text(a.x + 84.0 * s, ty, 14.0 * s, TEXT, &user, Face::Sans);
            g.text(a.x + 184.0 * s, ty, 14.0 * s, if cpu >= 50.0 { AMBER } else { TEXT }, &format!("{cpu:.0}%"), Face::Sans);
            g.text(a.x + 244.0 * s, ty, 14.0 * s, TEXT, &sysinfo::human_bytes(p.rss), Face::Sans);
            // the service of a cluster process, and the plain marker when it is near a limit its pod has
            if let Some(row) = self.rows.get(&p.pid) {
                if let Some(l) = &row.label {
                    let room = if row.near { 168.0 } else { 250.0 };
                    let t = g.fit(14.0 * s, &l.text(), Face::Sans, room * s);
                    g.text(a.x + 330.0 * s, ty, 14.0 * s, ACCENT, &t, Face::Sans);
                    if row.near {
                        g.text(a.x + 502.0 * s, ty, 12.0 * s, AMBER, "near limit", Face::Bold);
                    }
                } else if let Some(n) = self.nets.get(&p.pid).filter(|n| !n.is_quiet()) {
                    let t = g.fit(12.0 * s, &n.text(), Face::Sans, 250.0 * s);
                    g.text(a.x + 330.0 * s, ty + 1.0 * s, 12.0 * s, MUTED, &t, Face::Sans);
                }
            } else if let Some(n) = self.nets.get(&p.pid).filter(|n| !n.is_quiet()) {
                let t = g.fit(12.0 * s, &n.text(), Face::Sans, 250.0 * s);
                g.text(a.x + 330.0 * s, ty + 1.0 * s, 12.0 * s, MUTED, &t, Face::Sans);
            }
            // command lines can run to kilobytes: only the start can be seen, and fitting a long string is slow
            let label = if p.cmd.is_empty() { format!("[{}]", p.name) } else { p.cmd.chars().take(300).collect::<String>() };
            let label = g.fit(14.0 * s, &label, Face::Sans, (a.w - 590.0 * s - 20.0 * s).max(40.0 * s));
            g.text(a.x + 590.0 * s, ty, 14.0 * s, TEXT, &label, Face::Sans);
        }
        if sorted.is_empty() {
            iyakki_uikit::fx::empty_state(g, R::new(a.x, top, a.w, 160.0 * s), s, Icon::System, "No processes could be read", "The kernel's process list is not available here.", ACCENT);
        }
        // what the chosen one's pod is allowed
        let info = sorted.get(sel).and_then(|p| self.rows.get(&p.pid)).and_then(|r| r.label.as_ref().map(|l| format!("pod {} · {}", l.pod, sysprocs::res_text(&l.res)))).unwrap_or_default();
        let info = g.fit(12.0 * s, &info, Face::Sans, a.w);
        g.text(a.x, a.y + a.h - 62.0 * s, 12.0 * s, MUTED, &info, Face::Sans);
        self.process_buttons(g, a, s);
    }

    /// The programs started for the person, with their network use.
    fn render_mine(&mut self, g: &mut Gfx, a: R, s: f32, port: &dyn SystemPort) {
        if !self.mine_read {
            self.refresh_mine(port);
        }
        let cols = [("PID", 14.0), ("OWNER", 84.0), ("MEMORY", 184.0), ("NETWORK", 270.0), ("PROCESS", 590.0)];
        iyakki_uikit::fx::table_header(g, a.x, a.y, a.w, s, &cols, ACCENT);
        let row_h = 30.0 * s;
        let top = a.y + 20.0 * s;
        let rows = (((a.y + a.h - top - 66.0 * s) / row_h) as usize).max(1);
        let sel = self.sel[4].min(self.mine.len().saturating_sub(1));
        let first = sel.saturating_sub(rows - 1).min(self.mine.len().saturating_sub(rows));
        for (n, o) in self.mine.iter().enumerate().skip(first).take(rows) {
            let r = R::new(a.x, top + (n - first) as f32 * row_h, a.w, row_h - 2.0 * s);
            let (on, hot) = (n == sel, g.hovered(r));
            if on || hot {
                iyakki_uikit::fx::row_at(g, r.x, r.y, r.w, r.h, 8.0 * s, on, hot, ACCENT);
            }
            g.hit(r, Event::Row(n), "my process");
            let ty = r.y + 6.0 * s;
            g.text(a.x + 14.0 * s, ty, 14.0 * s, MUTED, &o.proc.pid.to_string(), Face::Mono);
            let owner = g.fit(14.0 * s, &o.owner, Face::Sans, 90.0 * s);
            g.text(a.x + 84.0 * s, ty, 14.0 * s, TEXT, &owner, Face::Sans);
            g.text(a.x + 184.0 * s, ty, 14.0 * s, TEXT, &sysinfo::human_bytes(o.proc.rss), Face::Sans);
            let net = g.fit(13.0 * s, &o.net.text(), Face::Sans, 310.0 * s);
            g.text(a.x + 270.0 * s, ty, 13.0 * s, MUTED, &net, Face::Sans);
            let label = if o.proc.cmd.is_empty() { format!("[{}]", o.proc.name) } else { port.redact(&o.proc.cmd.chars().take(300).collect::<String>()) };
            let label = g.fit(14.0 * s, &label, Face::Sans, (a.w - 590.0 * s - 20.0 * s).max(40.0 * s));
            g.text(a.x + 590.0 * s, ty, 14.0 * s, TEXT, &label, Face::Sans);
        }
        if self.mine.is_empty() {
            iyakki_uikit::fx::empty_state(g, R::new(a.x, top, a.w, 160.0 * s), s, Icon::System, "Nothing is running for you", "Programs the assistant or a terminal starts for you are listed here, with what they started.", ACCENT);
        }
        let mut x = a.x;
        let by = a.y + a.h - 38.0 * s;
        x += g.button(x, by, 14.0 * s, "Stop (k)", AMBER, Event::Tab(20)) + 8.0 * s;
        g.button(x, by, 14.0 * s, "Kill (K)", ERR, Event::Tab(21));
    }

    fn process_buttons(&self, g: &mut Gfx, a: R, s: f32) {
        let by = a.y + a.h - 38.0 * s;
        let mut x = a.x;
        let mut put = |g: &mut Gfx, label: &str, color: [u8; 4], ev: Event| {
            x += g.button(x, by, 14.0 * s, label, color, ev) + 8.0 * s;
        };
        if !self.grouped {
            put(g, "Stop (k)", AMBER, Event::Tab(20));
            put(g, "Kill (K)", ERR, Event::Tab(21));
        }
        put(g, if self.grouped { "Single processes (g)" } else { "By service (g)" }, ACCENT, Event::Tab(24));
        put(g, "What is it busy with (b)", ACCENT, Event::Tab(25));
    }

    /// The grouped view: one line per service (and one for the machine's own processes).
    fn render_groups(&mut self, g: &mut Gfx, a: R, s: f32) {
        let groups = self.sorted_groups();
        let cols = [("SERVICE", 14.0), ("PROCESSES", 330.0), ("CPU", 430.0), ("MEMORY", 500.0), ("REQUEST / LIMIT", 600.0)];
        iyakki_uikit::fx::table_header(g, a.x, a.y, a.w, s, &cols, ACCENT);
        let sorted_by = format!("sorted by {}", self.sort.label());
        let sbw = g.text_width(12.0 * s, &sorted_by, Face::Sans);
        g.text(a.x + a.w - sbw - 6.0 * s, a.y - 1.0 * s, 12.0 * s, MUTED, &sorted_by, Face::Sans);
        let row_h = 30.0 * s;
        let top = a.y + 20.0 * s;
        let rows = (((a.y + a.h - top - 66.0 * s) / row_h) as usize).max(1);
        let sel = self.sel[1].min(groups.len().saturating_sub(1));
        let first = sel.saturating_sub(rows - 1).min(groups.len().saturating_sub(rows));
        for (n, gr) in groups.iter().enumerate().skip(first).take(rows) {
            let r = R::new(a.x, top + (n - first) as f32 * row_h, a.w, row_h - 2.0 * s);
            let (on, hot) = (n == sel, g.hovered(r));
            if on || hot {
                iyakki_uikit::fx::row_at(g, r.x, r.y, r.w, r.h, 8.0 * s, on, hot, ACCENT);
            }
            g.hit(r, Event::Row(n), "service");
            let ty = r.y + 6.0 * s;
            let name_room = if gr.near { 230.0 } else { 306.0 };
            let name = g.fit(14.0 * s, &gr.name, Face::Sans, name_room * s);
            g.text(a.x + 14.0 * s, ty, 14.0 * s, if gr.pod.is_some() { ACCENT } else { TEXT }, &name, Face::Sans);
            if gr.near {
                g.text(a.x + 250.0 * s, ty, 12.0 * s, AMBER, "near limit", Face::Bold);
            }
            g.text(a.x + 330.0 * s, ty, 14.0 * s, TEXT, &gr.count.to_string(), Face::Sans);
            g.text(a.x + 430.0 * s, ty, 14.0 * s, if gr.cpu >= 50.0 { AMBER } else { TEXT }, &format!("{:.0}%", gr.cpu), Face::Sans);
            g.text(a.x + 500.0 * s, ty, 14.0 * s, TEXT, &sysinfo::human_bytes(gr.rss), Face::Sans);
            let lim = gr.res.map(|r| sysprocs::res_text(&r)).unwrap_or_default();
            let lim = g.fit(12.0 * s, &lim, Face::Sans, (a.w - 600.0 * s - 12.0 * s).max(40.0 * s));
            g.text(a.x + 600.0 * s, ty + 1.0 * s, 12.0 * s, MUTED, &lim, Face::Sans);
        }
        if groups.is_empty() {
            iyakki_uikit::fx::empty_state(g, R::new(a.x, top, a.w, 160.0 * s), s, Icon::System, "No processes could be read", "The kernel's process list is not available here.", ACCENT);
        }
        self.process_buttons(g, a, s);
    }

    fn render_hardware(&mut self, g: &mut Gfx, a: R, s: f32) {
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

    /// The supervisor's WebAssembly tasks, read-only: they run in a sandbox with fuel and time limits and end by themselves, so there is no stop button.
    fn render_wasm(&self, g: &mut Gfx, a: R, s: f32) {
        let now = iyakki_clock::now_unix();
        g.text(a.x, a.y, 13.0 * s, MUTED, "WebAssembly tasks (they end at their own fuel and time limits)", Face::Bold);
        let row_h = 22.0 * s;
        for (n, t) in self.wasm.iter().enumerate() {
            let y = a.y + 20.0 * s + n as f32 * row_h;
            if y + row_h > a.y + a.h {
                break;
            }
            let line = format!("{} · {} · {} KB · {}", t.user, t.entry, t.bytes.div_ceil(1024), sysinfo::human_duration((now - t.started).max(0) as u64));
            let line = g.fit(13.0 * s, &line, Face::Mono, a.w - 8.0 * s);
            g.text(a.x + 4.0 * s, y, 13.0 * s, TEXT, &line, Face::Mono);
        }
    }

    fn render_agents(&mut self, g: &mut Gfx, a: R, s: f32) {
        if self.live.is_empty() && self.wasm.is_empty() {
            iyakki_uikit::fx::empty_state(g, a, s, Icon::Runs, "No agent run is going on", "Runs started in CHAT show up here while they work.", ACCENT);
            return;
        }
        if self.live.is_empty() {
            self.render_wasm(g, R::new(a.x, a.y, a.w, a.h), s);
            return;
        }
        let row_h = 62.0 * s;
        let now = iyakki_clock::now_unix();
        let sel = self.sel[2].min(self.live.len() - 1);
        for (n, r) in self.live.iter().enumerate() {
            let rr = R::new(a.x, a.y + n as f32 * (row_h + 4.0 * s), a.w, row_h);
            if rr.y + rr.h > a.y + a.h - 44.0 * s {
                break;
            }
            let hot = g.hovered(rr);
            iyakki_uikit::fx::row_at(g, rr.x, rr.y, rr.w, rr.h, 10.0 * s, n == sel, hot, ACCENT);
            g.hit(rr, Event::Row(n), "agent run");
            let title = g.fit(15.0 * s, &format!("{} · {}", r.user, r.prompt), Face::Bold, rr.w - 24.0 * s);
            g.text(rr.x + 14.0 * s, rr.y + 8.0 * s, 15.0 * s, TEXT, &title, Face::Bold);
            let mut sub = format!("{} · {} steps", sysinfo::human_duration((now - r.started).max(0) as u64), r.steps);
            if r.tokens > 0 {
                sub.push_str(&format!(" · {} tokens", iyakki_uikit::tilekit::fmt_tokens(r.tokens)));
            }
            sub.push_str(&format!(" · {}", r.last));
            let sub = g.fit(13.0 * s, &sub, Face::Sans, rr.w - 24.0 * s - if r.pending > 0 { 150.0 * s } else { 0.0 });
            g.text(rr.x + 14.0 * s, rr.y + 34.0 * s, 13.0 * s, MUTED, &sub, Face::Sans);
            if r.pending > 0 {
                let t = format!("{} waiting", r.pending);
                let tw = g.text_width(12.0 * s * 1.0, &t, Face::Bold) + 17.0 * s;
                iyakki_uikit::fx::chip(g, rr.x + rr.w - tw - 12.0 * s, rr.y + 32.0 * s, s, &t, AMBER, true);
            }
        }
        if !self.wasm.is_empty() {
            let used = self.live.len() as f32 * (row_h + 4.0 * s);
            let rest = R::new(a.x, a.y + used + 6.0 * s, a.w, (a.h - used - 50.0 * s).max(0.0));
            if rest.h > 40.0 * s {
                self.render_wasm(g, rest, s);
            }
        }
        let by = a.y + a.h - 38.0 * s;
        let w1 = g.button(a.x, by, 14.0 * s, "Stop this run (x)", AMBER, Event::Tab(22));
        g.button(a.x + w1 + 8.0 * s, by, 14.0 * s, "Stop all (X)", ERR, Event::Tab(23));
    }
}


/// The extra line of the MEMORY card: compressed memory (zram) and memory pressure.
pub fn memory_extra_line(m: &MemStats) -> String {
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
