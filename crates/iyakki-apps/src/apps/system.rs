//! SYSTEM: what the machine is doing. Four pages: the **overview** (processor, memory, load, disks, network, heat, how
//! long it has been up), the **processes** (administrators only: who uses what, stop or kill one) and the **agents** (every
//! run going on now: whose, what it was asked, what it waits for, stop one or all) and the **hardware**
//! (administrators only: what the kernel found, PCI devices with their drivers, loaded drivers). Figures are read from the kernel once a
//! second; nothing is kept.

use std::collections::HashMap;
use std::time::{Duration, Instant};

use iyakki_shared::runs::LiveRun;
use iyakki_core_api::sysinfo::{self, CpuTimes, DiskCounters, NetCounters, Proc, SysRoots};
use iyakki_core_api::HostFacts;
use iyakki_shared::sysprocs::{Row};
use iyakki_shared::sysprocs::{self, PodCache};

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
enum Confirm {
    Signal { pid: u32, name: String, hard: bool, mine: bool },
    StopAll,
}

struct Sample {
    at: Instant,
    cpu: CpuTimes,
    procs: Vec<Proc>,
    net: Vec<NetCounters>,
    disk: Vec<DiskCounters>,
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
    disk_rate: (f64, f64),
    /// "MAC: aa:bb:cc:dd:ee:ff (private)" for the adapter in use, read with the other figures.
    mac_line: Option<String>,
    /// The compressed-memory line, read through `cx.os` at each refresh.
    zram_line: String,
    /// Where sound plays and why (Settings -> Sound), read with the other figures.
    sound_line: String,
    /// One line per graphics card (name, driver, memory), read with the other figures.
    gpu_lines: Vec<String>,
    sort: Sort,
    sel: [usize; 5],
    /// The hardware page, read when it is opened or refreshed.
    hardware: Vec<String>,
    confirm: Option<Confirm>,
    status: Option<(String, bool)>,
    live: Vec<LiveRun>,
    /// WebAssembly tasks of the supervisor running now (F-SYS-01).
    wasm: Vec<iyakki_core_api::WasmTask>,
    last_refresh: Option<Instant>,
    /// The dev cluster's service log (View logs), selectable text.
    cluster_log: Option<crate::apps::ScrollLog>,
    health_busy: std::sync::Arc<std::sync::atomic::AtomicBool>,
    /// Pod uid to service table of the dev cluster, kept fresh in the background (empty when the cluster is off).
    pods: PodCache,
    /// The processes with their service label and near-limit marker, by pid.
    rows: HashMap<u32, Row>,
    /// Processes tab: one line per service instead of one per process.
    grouped: bool,
    /// The Mine page: the programs started for the person (F-SYS-01), read while the page is open.
    mine: Vec<iyakki_agent_api::OwnedProc>,
    mine_read: bool,
    /// Network use of the first rows of the Processes page, read while the page is open.
    nets: HashMap<u32, sysinfo::NetUse>,
    /// Title of the log window (the cluster's own log, or what a pod is busy with).
    log_title: String,
}

impl SystemApp {
    /// The graphics cards for the overview, e.g. "AMD Radeon Graphics (Raphael) · driver amdgpu" and "NVIDIA GeForce RTX 4070 Laptop GPU · driver nvidia · 8 GB".
    /// Virtual display devices are left out; a card with no driver says so (the NVIDIA driver is a download).
    fn gpu_summary(gpus: &[iyakki_shared::gpu::Gpu]) -> Vec<String> {
        gpus.iter()
            .filter(|g| !matches!(g.vendor, iyakki_shared::gpu::Vendor::Other(_)))
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
        SystemApp { page: Page::Overview, roots, data_dir, prev: None, cur: None, history: Vec::new(), proc_cpu: HashMap::new(), net_rate: (0.0, 0.0), disk_rate: (0.0, 0.0), mac_line: None, zram_line: String::new(), sound_line: String::new(), gpu_lines: Vec::new(), sort: Sort::Cpu, sel: [0; 5], hardware: Vec::new(), confirm: None, status: None, live: Vec::new(), wasm: Vec::new(), last_refresh: None, cluster_log: None, health_busy: Default::default(), pods: PodCache::new(), rows: HashMap::new(), grouped: false, mine: Vec::new(), mine_read: false, nets: HashMap::new(), log_title: String::new() }
    }

    pub fn page(&self) -> Page {
        self.page
    }

    fn is_admin(cx: &AppCx) -> bool {
        cx.session.policy.manage_settings
    }

    fn pages(cx: &AppCx) -> Vec<Page> {
        if Self::is_admin(cx) { vec![Page::Overview, Page::Processes, Page::Agents, Page::Hardware, Page::Mine] } else { vec![Page::Overview, Page::Agents, Page::Mine] }
    }

    /// Read the kernel's figures now.
    pub fn refresh(&mut self, cx: &AppCx) {
        let now = Instant::now();
        let sample = Sample { at: now, cpu: cx.core.host().cpu_times(&self.roots), procs: if Self::is_admin(cx) { cx.core.host().processes(&self.roots) } else { Vec::new() }, net: cx.core.host().net_counters(&self.roots), disk: cx.core.host().disk_counters(&self.roots) };
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
        if Self::is_admin(cx) {
            let k = &cx.k3sdev();
            self.pods.poll(k.clone().kubectl_reader(), k.state() == iyakki_shared::k3sdev::Svc::Running, iyakki_clock::now_unix().max(0) as u64);
            let table = self.pods.table();
            self.rows = self.cur.as_ref().map(|c| sysprocs::rows(&c.procs, &self.proc_cpu, &table).into_iter().map(|r| (r.pid, r)).collect()).unwrap_or_default();
            if let Some((pod, text)) = self.pods.take_answer() {
                let mut l = crate::apps::ScrollLog::default();
                for line in text.lines() {
                    l.push(line, [0, 0, 0, 0]);
                }
                self.log_title = format!("What {pod} is busy with (select with the mouse, Ctrl+C copies)");
                self.cluster_log = Some(l);
            }
        }
        self.zram_line = memory_extra_line(&cx.os.memory().stats());
        self.mac_line = cx.os.hardware().mac_summary();
        if self.page == Page::Hardware && self.hardware.is_empty() {
            self.hardware = cx.os.hardware().hardware_report();
        }
        self.sound_line = cx.agent.audioout_summary_at(&self.roots.proc);
        self.gpu_lines = Self::gpu_summary(&iyakki_shared::gpu::detect(self.roots.sys.parent().unwrap_or(std::path::Path::new("/"))));
        self.live = cx.runs().live().into_iter().filter(|r| Self::is_admin(cx) || r.user == cx.session.user).collect();
        self.wasm = cx.host.wasm_tasks().into_iter().filter(|t| Self::is_admin(cx) || t.user == cx.session.user).collect();
        self.last_refresh = Some(now);
        if self.page == Page::Mine {
            self.refresh_mine(cx);
        }
        if self.page == Page::Processes && Self::is_admin(cx) && !self.grouped {
            let pids: Vec<u32> = self.sorted().iter().take(40).map(|p| p.pid).collect();
            self.nets = cx.core.host().net_use(&self.roots, &pids);
        }
        // the dev cluster's health is read in the background every 15 s while it runs (a few kubectl calls)
        let k = &cx.k3sdev();
        let h = k.health();
        if h.installed && !matches!(h.state.as_str(), "stopped" | "not installed") && h.checked_at < iyakki_clock::now_unix() - 15 && !self.health_busy.swap(true, std::sync::atomic::Ordering::SeqCst) {
            let (k, busy) = (k.clone(), self.health_busy.clone());
            std::thread::spawn(move || {
                k.refresh_health();
                busy.store(false, std::sync::atomic::Ordering::SeqCst);
            });
        }
    }

    /// The programs started for this person (everyone's for an administrator), read now.
    fn refresh_mine(&mut self, cx: &AppCx) {
        self.mine = cx.agent.proc_owner().mine(&self.roots, &cx.session.user, Self::is_admin(cx));
        self.mine_read = true;
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

    fn set_sel(&mut self, i: usize) {
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

    fn run_confirmed(&mut self, cx: &AppCx) {
        match self.confirm.take() {
            Some(Confirm::Signal { pid, name, hard, mine }) => {
                if !mine && !Self::is_admin(cx) {
                    return;
                }
                cx.audit.record(&cx.session.user, if hard { "system.kill" } else { "system.stop" }, &format!("{pid} {name}"));
                // from My processes the ownership is checked again against the machine as it is now
                let r = if mine { cx.agent.proc_owner().stop(&self.roots, &cx.session.user, Self::is_admin(cx), pid, hard).map(|_| ()) } else { cx.core.host().signal(pid, hard) };
                match r {
                    Ok(()) => self.say(format!("{} {name} ({pid}).", if hard { "Killed" } else { "Asked to stop" }), true),
                    Err(e) => self.say(format!("Could not stop {name} ({pid}): {e}"), false),
                }
                self.refresh(cx);
            }
            Some(Confirm::StopAll) => {
                let only = if Self::is_admin(cx) { None } else { Some(cx.session.user.as_str()) };
                let n = cx.runs().stop_all_live(only);
                cx.audit.record(&cx.session.user, "system.agents.stop_all", &n.to_string());
                self.say(format!("Told {n} run{} to stop.", if n == 1 { "" } else { "s" }), true);
                self.refresh(cx);
            }
            None => {}
        }
    }

    /// The service log of the dev cluster (the end of it), as selectable text.
    fn open_cluster_log(&mut self, cx: &AppCx) {
        let t = cx.k3sdev().log_tail(400);
        let mut l = crate::apps::ScrollLog::default();
        if t.trim().is_empty() {
            l.push("The log is empty.", DIM);
        }
        for line in t.lines() {
            l.push(line, if line.contains("level=error") || line.contains("Failed") || line.contains("error") { ERR } else { [0, 0, 0, 0] });
        }
        cx.audit.record(&cx.session.user, "devcluster.logs", "");
        self.log_title = "The dev cluster's log (select with the mouse, Ctrl+C copies)".into();
        self.cluster_log = Some(l);
    }

    /// The groups of the grouped view, in the order chosen.
    fn sorted_groups(&self) -> Vec<sysprocs::Group> {
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
    fn selected_pod(&self) -> Option<(String, String)> {
        if self.grouped {
            self.sorted_groups().get(self.sel[1]).and_then(|g| g.pod.clone())
        } else {
            let p = self.sorted().get(self.sel[1]).map(|p| p.pid)?;
            self.rows.get(&p)?.label.as_ref().map(|l| (l.ns.clone(), l.pod.clone()))
        }
    }

    /// "What is this busy with": the pod's recent log and usage numbers, read in the background.
    fn ask_busy(&mut self, cx: &AppCx) {
        if !Self::is_admin(cx) {
            return;
        }
        let Some((ns, pod)) = self.selected_pod() else {
            self.say("This line is not a pod of the dev cluster.", false);
            return;
        };
        cx.audit.record(&cx.session.user, "system.pod_busy", &format!("{ns}/{pod}"));
        self.pods.ask_busy(cx.k3sdev().kubectl_reader(), ns, pod);
        self.say("Reading the pod's log and usage...", true);
    }

    fn stop_selected_agent(&mut self, cx: &AppCx) {
        let Some(r) = self.live.get(self.sel[2]).cloned() else { return };
        if !Self::is_admin(cx) && r.user != cx.session.user {
            self.say("That run belongs to someone else.", false);
            return;
        }
        if cx.runs().stop_live(&r.id) {
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
            Page::Processes => "↑↓ choose · c m p n sort · g by service · b what is it busy with · k stop · K kill · Tab next page · Esc back".into(),
            Page::Agents => "↑↓ choose · x stop this run · X stop all · Tab next page · Esc back".into(),
            Page::Hardware => "↑↓ PgUp PgDn scroll · r read again · Tab next page · Esc back".into(),
            Page::Mine => "↑↓ choose · k stop · K kill · Tab next page · Esc back".into(),
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
            "g · group the processes by service (the pod and Quick install service of a cluster process); b · show what the chosen pod is busy with (log tail, kubectl top)",
            "Mine · the programs started for you (by the assistant or in a terminal) and what they started, with their network use; k stops, K kills one of them",
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
            Event::Key(Key::Char('b')) | Event::Tab(25) if self.page == Page::Processes => self.ask_busy(cx),
            Event::Key(Key::Char('k')) if (self.page == Page::Processes && Self::is_admin(cx)) || self.page == Page::Mine => self.ask_signal(false),
            Event::Key(Key::Char('K')) if (self.page == Page::Processes && Self::is_admin(cx)) || self.page == Page::Mine => self.ask_signal(true),
            Event::Key(Key::Char('r')) if self.page == Page::Hardware => self.hardware = cx.os.hardware().hardware_report(),
            Event::Key(Key::Char('l')) | Event::Tab(93) if self.page == Page::Overview && Self::is_admin(cx) && cx.k3sdev().installed().is_some() => self.open_cluster_log(cx),
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
                    Page::Mine => "Mine",
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
            Page::Mine => self.render_mine(g, body, s, cx),
        }
        // the line at the bottom: the answer to the last action
        if let Some((t, ok)) = &self.status {
            crate::fx::status_line(g, x, area.y + area.h - 26.0 * s, w, s, t, *ok);
        }
        if let Some(l) = self.cluster_log.as_mut() {
            let p = crate::fx::modal(g, area, s, 900.0 * s, area.h - 24.0 * s, ACCENT, false);
            let y0 = crate::fx::modal_title(g, p, s, &self.log_title.clone(), ACCENT);
            let bh = 13.0 * s * 2.1;
            l.render(g, R::new(p.x + 24.0 * s, y0 + 4.0 * s, p.w - 48.0 * s, (p.y + p.h - y0 - bh - 40.0 * s).max(40.0 * s)), 13.0 * s, Face::Mono);
            g.button(p.x + 24.0 * s, p.y + p.h - bh - 16.0 * s, 13.0 * s, "Close", MUTED, Event::Tab(92));
        }
        if let Some(c) = &self.confirm {
            let text = match c {
                Confirm::Signal { pid, name, hard, .. } => format!("{} {name} (process {pid})?", if *hard { "Kill" } else { "Ask to stop" }),
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
        let m = cx.core.host().memory_of(&self.roots);
        let cpu_now = self.history.last().copied().unwrap_or(0.0);
        let (l1, l5, l15) = cx.core.host().load_average(&self.roots);
        let cpus = cx.core.host().cpu_count(&self.roots);
        let card_w = (a.w - 2.0 * 12.0 * s) / 3.0;
        let ch = 112.0 * s;
        // processor with its history
        let c = R::new(a.x, a.y, card_w, ch);
        crate::fx::glass(g, c.x, c.y, c.w, c.h, 12.0 * s, ACCENT);
        g.text(c.x + 14.0 * s, c.y + 9.0 * s, 11.0 * s, DIM, "PROCESSOR", Face::Bold);
        g.text(c.x + 14.0 * s, c.y + 26.0 * s, 24.0 * s, heat(cpu_now), &format!("{cpu_now:.0}%"), Face::Bold);
        let cores = g.fit(13.0 * s, &format!("{cpus} core{}", if cpus == 1 { "" } else { "s" }), Face::Sans, (c.w - 28.0 * s - 78.0 * s).max(30.0 * s));
        g.text(c.x + 14.0 * s + 78.0 * s, c.y + 34.0 * s, 13.0 * s, MUTED, &cores, Face::Sans);
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
        crate::fx::glass(g, c.x, c.y, c.w, c.h, 12.0 * s, ACCENT);
        let t = g.fit(11.0 * s, "LOAD AND UPTIME", Face::Bold, c.w - 28.0 * s);
        g.text(c.x + 14.0 * s, c.y + 9.0 * s, 11.0 * s, DIM, &t, Face::Bold);
        if cx.core.flows_view().scheduler_running(&cx.session.user) {
            let t = "flows scheduler running";
            let w = g.text_width(10.5 * s, t, Face::Sans);
            if w + 150.0 * s < c.w {
                g.text(c.x + c.w - w - 12.0 * s, c.y + 10.0 * s, 10.5 * s, crate::ui::OK, t, Face::Sans);
            }
        }
        g.text(c.x + 14.0 * s, c.y + 26.0 * s, 24.0 * s, TEXT, &format!("{l1:.2}"), Face::Bold);
        // beside the big number when the card is wide enough for the words, left out when it is not (the number says it)
        if c.w - 92.0 * s - 8.0 * s >= 60.0 * s {
            let t = g.fit(13.0 * s, &format!("{l5:.2} · {l15:.2} (5 and 15 min)"), Face::Sans, c.w - 92.0 * s - 8.0 * s);
            g.text(c.x + 14.0 * s + 78.0 * s, c.y + 34.0 * s, 13.0 * s, MUTED, &t, Face::Sans);
        }
        let t = g.fit(14.0 * s, &format!("up {}", sysinfo::human_duration(cx.core.host().uptime_of(&self.roots))), Face::Sans, c.w - 28.0 * s);
        g.text(c.x + 14.0 * s, c.y + 66.0 * s, 14.0 * s, TEXT, &t, Face::Sans);
        match cx.core.host().hottest_temp(&self.roots) {
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
                if let Ok((free, total)) = cx.os.disks().space(&path.to_string_lossy()) {
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
            let bar_w = crate::safeclamp::SafeClamp::clamp_safe(a.w - 130.0 * s - 14.0 * s - want, 40.0 * s, a.w * 0.4);
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
        let host = g.fit(14.0 * s, &crate::hostname::current("iyakki"), Face::Sans, (a.w - 130.0 * s).max(40.0 * s));
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
        let about = g.fit(13.0 * s, iyakki_shared::relinfo::machine_about(), Face::Sans, a.w - 130.0 * s);
        g.text(a.x + 130.0 * s, y, 13.0 * s, MUTED, &about, Face::Sans);
        // the dev cluster IYAKKI manages on this machine
        let h = cx.k3sdev().health();
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
        if self.grouped {
            return self.render_groups(g, a, s);
        }
        let sorted: Vec<&Proc> = self.sorted();
        let header_y = a.y;
        let cols = [("PID", 14.0), ("USER", 84.0), ("CPU", 184.0), ("MEMORY", 244.0), ("SERVICE / NETWORK", 330.0), ("PROCESS", 590.0)];
        crate::fx::table_header(g, a.x, header_y, a.w, s, &cols, ACCENT);
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
                crate::fx::row_at(g, r.x, r.y, r.w, r.h, 8.0 * s, on, hot, ACCENT);
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
            crate::fx::empty_state(g, R::new(a.x, top, a.w, 160.0 * s), s, Icon::System, "No processes could be read", "The kernel's process list is not available here.", ACCENT);
        }
        // what the chosen one's pod is allowed
        let info = sorted.get(sel).and_then(|p| self.rows.get(&p.pid)).and_then(|r| r.label.as_ref().map(|l| format!("pod {} · {}", l.pod, sysprocs::res_text(&l.res)))).unwrap_or_default();
        let info = g.fit(12.0 * s, &info, Face::Sans, a.w);
        g.text(a.x, a.y + a.h - 62.0 * s, 12.0 * s, MUTED, &info, Face::Sans);
        self.process_buttons(g, a, s);
    }

    /// The programs started for the person, with their network use.
    fn render_mine(&mut self, g: &mut Gfx, a: R, s: f32, cx: &AppCx) {
        if !self.mine_read {
            self.refresh_mine(cx);
        }
        let cols = [("PID", 14.0), ("OWNER", 84.0), ("MEMORY", 184.0), ("NETWORK", 270.0), ("PROCESS", 590.0)];
        crate::fx::table_header(g, a.x, a.y, a.w, s, &cols, ACCENT);
        let row_h = 30.0 * s;
        let top = a.y + 20.0 * s;
        let rows = (((a.y + a.h - top - 66.0 * s) / row_h) as usize).max(1);
        let sel = self.sel[4].min(self.mine.len().saturating_sub(1));
        let first = sel.saturating_sub(rows - 1).min(self.mine.len().saturating_sub(rows));
        for (n, o) in self.mine.iter().enumerate().skip(first).take(rows) {
            let r = R::new(a.x, top + (n - first) as f32 * row_h, a.w, row_h - 2.0 * s);
            let (on, hot) = (n == sel, g.hovered(r));
            if on || hot {
                crate::fx::row_at(g, r.x, r.y, r.w, r.h, 8.0 * s, on, hot, ACCENT);
            }
            g.hit(r, Event::Row(n), "my process");
            let ty = r.y + 6.0 * s;
            g.text(a.x + 14.0 * s, ty, 14.0 * s, MUTED, &o.proc.pid.to_string(), Face::Mono);
            let owner = g.fit(14.0 * s, &o.owner, Face::Sans, 90.0 * s);
            g.text(a.x + 84.0 * s, ty, 14.0 * s, TEXT, &owner, Face::Sans);
            g.text(a.x + 184.0 * s, ty, 14.0 * s, TEXT, &sysinfo::human_bytes(o.proc.rss), Face::Sans);
            let net = g.fit(13.0 * s, &o.net.text(), Face::Sans, 310.0 * s);
            g.text(a.x + 270.0 * s, ty, 13.0 * s, MUTED, &net, Face::Sans);
            let label = if o.proc.cmd.is_empty() { format!("[{}]", o.proc.name) } else { cx.agent.sysprocs_redact(&o.proc.cmd.chars().take(300).collect::<String>()) };
            let label = g.fit(14.0 * s, &label, Face::Sans, (a.w - 590.0 * s - 20.0 * s).max(40.0 * s));
            g.text(a.x + 590.0 * s, ty, 14.0 * s, TEXT, &label, Face::Sans);
        }
        if self.mine.is_empty() {
            crate::fx::empty_state(g, R::new(a.x, top, a.w, 160.0 * s), s, Icon::System, "Nothing is running for you", "Programs the assistant or a terminal starts for you are listed here, with what they started.", ACCENT);
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
        crate::fx::table_header(g, a.x, a.y, a.w, s, &cols, ACCENT);
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
                crate::fx::row_at(g, r.x, r.y, r.w, r.h, 8.0 * s, on, hot, ACCENT);
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
            crate::fx::empty_state(g, R::new(a.x, top, a.w, 160.0 * s), s, Icon::System, "No processes could be read", "The kernel's process list is not available here.", ACCENT);
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
            crate::fx::empty_state(g, a, s, Icon::Runs, "No agent run is going on", "Runs started in CHAT show up here while they work.", ACCENT);
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

/// Everything the hardware page shows, as lines: the boot hardware report, the PCI devices with the driver bound to
/// each, and the loaded drivers (kernel modules).
/// The extra line of the MEMORY card: compressed memory (zram) and memory pressure.
pub fn memory_extra_line(m: &iyakki_core_api::memory::MemStats) -> String {
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

/// `iyakki top`: the same figures as text, once, for a terminal or a script.
pub fn text_report(host: &dyn HostFacts, roots: &SysRoots, top: usize) -> String {
    let m = host.memory_of(roots);
    let (l1, l5, l15) = host.load_average(roots);
    let first = (host.cpu_times(roots), host.processes(roots));
    std::thread::sleep(Duration::from_millis(500));
    let second = (host.cpu_times(roots), host.processes(roots));
    let cpu = sysinfo::cpu_percent(first.0, second.0);
    let per = sysinfo::proc_cpu(&first.1, &second.1, 0.5);
    let mut out = format!("up {} · load {l1:.2} {l5:.2} {l15:.2} · processor {cpu:.0}% of {} · memory {} of {} used\n", sysinfo::human_duration(host.uptime_of(roots)), host.cpu_count(roots), sysinfo::human_bytes(m.used()), sysinfo::human_bytes(m.total));
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
    use iyakki_agent::facets_h::RealServices as _;
    use super::*;
    use crate::apps::tests::cx_with_admin_store;
    use iyakki_shared::agent::{Risk, ToolDef, Verdict};
    use iyakki_shared::agent::{Tools};
    use iyakki_shared::chat::{ChatBackend, Message, ModelTurn, ToolCall, ToolSpec};
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
    fn the_disk_rows_of_the_overview_come_from_cx_os_so_a_fake_machine_decides_them() {
        use iyakki_core_fake::{Canned, FakeOs};
        let roots = machine("os-disks", None);
        let mut cx = cx_with_admin_store("sys-os");
        let mut a = SystemApp::with_roots(roots, std::env::temp_dir());
        cx.os = Arc::new(FakeOs::with(Canned { space: Some((30_000_000_000, 120_000_000_000)), ..Canned::default() }));
        a.refresh(&cx);
        let g = frame(&mut a, &cx);
        let texts: Vec<String> = g.text_log.iter().map(|(_, _, t)| t.clone()).collect();
        assert!(texts.iter().any(|t| t == "Data disk") && texts.iter().any(|t| t == "System disk"), "{texts:?}");
        let (free, total) = (sysinfo::human_bytes(30_000_000_000), sysinfo::human_bytes(120_000_000_000));
        assert!(texts.iter().any(|t| *t == format!("{free} free of {total}")), "the figures are the fake's, not this computer's: {texts:?}");
        // a machine that cannot say shows no disk rows at all
        cx.os = Arc::new(FakeOs::with(Canned { space: None, ..Canned::default() }));
        let g = frame(&mut a, &cx);
        assert!(!g.text_log.iter().any(|(_, _, t)| t == "Data disk"));
    }

    #[test]
    fn the_hardware_page_and_the_private_address_line_come_from_cx_os_so_a_fake_machine_decides_them() {
        use iyakki_core_fake::FakeOs;
        let roots = machine("os-hw", None);
        let mut cx = cx_with_admin_store("sys-os-hw");
        let mut a = SystemApp::with_roots(roots, std::env::temp_dir());
        let os = Arc::new(FakeOs::new());
        os.hardware_fake().set(|s| {
            s.mac_summary = Some("MAC: fake-private".into());
            s.report = vec!["MACHINE".into(), "  fake board".into()];
        });
        cx.os = os.clone();
        a.refresh(&cx);
        assert_eq!(a.mac_line.as_deref(), Some("MAC: fake-private"));
        a.page = Page::Hardware;
        a.refresh(&cx);
        assert_eq!(a.hardware, vec!["MACHINE".to_string(), "  fake board".to_string()]);
        os.hardware_fake().set(|s| s.report = vec!["AFTER".into()]);
        key(&mut a, &cx, Key::Char('r'));
        assert_eq!(a.hardware, vec!["AFTER".to_string()], "r reads the page again through the core");
    }

    #[test]
    fn the_tile_runs_in_the_harness_on_a_fake_machine_and_fits_the_four_standard_sizes() {
        use super::super::tile_harness::TileHarness;
        use iyakki_core_fake::{Canned, FakeOs};
        let roots = machine("os-harness", None);
        let cx = cx_with_admin_store("sys-harness");
        let a = SystemApp::with_roots(roots, std::env::temp_dir());
        let mut t = TileHarness::new(a, cx).with_os(Arc::new(FakeOs::with(Canned { space: Some((12_000_000_000, 64_000_000_000)), ..Canned::default() })));
        t.app.refresh(&t.cx);
        let (free, total) = (sysinfo::human_bytes(12_000_000_000), sysinfo::human_bytes(64_000_000_000));
        assert!(t.shows(&format!("{free} free of {total}")), "{:?}", t.texts());
        t.assert_no_overlap_at_standard_sizes();
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
        // the tab key walks the pages (administrators have five)
        key(&mut a, &cx, Key::Tab);
        assert_eq!(a.page(), Page::Processes);
        key(&mut a, &cx, Key::Tab);
        assert_eq!(a.page(), Page::Agents);
        key(&mut a, &cx, Key::Tab);
        assert_eq!(a.page(), Page::Hardware);
        key(&mut a, &cx, Key::Tab);
        assert_eq!(a.page(), Page::Mine);
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
        key(&mut b, &cx2, Key::Tab);
        assert_eq!(b.page(), Page::Mine, "their own programs are the third page");
        key(&mut b, &cx2, Key::Char('4'));
        assert_eq!(b.page(), Page::Mine, "there are only three pages");
        key(&mut b, &cx2, Key::Esc);
    }

    #[test]
    fn mine_lists_what_was_started_for_the_person_with_its_network_use_and_stops_only_her_own() {
        let mut child = std::process::Command::new("sleep").arg("60").spawn().expect("sleep");
        let mut other = std::process::Command::new("sleep").arg("60").spawn().expect("sleep");
        let (cpid, opid) = (child.id(), other.id());
        let roots = machine("mine", Some((cpid, "sleeper")));
        // a second program in the same made-up machine, claimed for someone else
        let p = roots.proc.join(opid.to_string());
        std::fs::create_dir_all(&p).unwrap();
        std::fs::write(p.join("stat"), format!("{opid} (other) S 1 0 0 0 0 0 0 0 0 0 5 0 0 0 20 0 1 0 5 1 3000 0\n")).unwrap();
        std::fs::write(p.join("status"), "Uid:\t0\t0\t0\t0\n").unwrap();
        std::fs::write(p.join("cmdline"), "/bin/other\0").unwrap();
        // the sleeper has a listener and a connection
        std::fs::write(roots.proc.join("net/tcp"), "h\n 0: a b 0A c d e 0 0 7001 x\n 1: a b 01 c d e 0 0 7002 x\n").unwrap();
        std::fs::create_dir_all(roots.proc.join(cpid.to_string()).join("fd")).unwrap();
        std::os::unix::fs::symlink("socket:[7001]", roots.proc.join(cpid.to_string()).join("fd/3")).unwrap();
        std::os::unix::fs::symlink("socket:[7002]", roots.proc.join(cpid.to_string()).join("fd/4")).unwrap();
        let ann = format!("mine-ann-{}", std::process::id());
        let bob = format!("mine-bob-{}", std::process::id());
        let _c1 = iyakki_agent::procowner::claim(cpid, &ann, "sleep");
        let _c2 = iyakki_agent::procowner::claim(opid, &bob, "sleep");
        // an ordinary person: only hers, with the network text, and the process list of the machine stays unread
        let mut cx = cx_with_admin_store("sysmine");
        cx.session.user = ann.clone();
        cx.session.role = Role::User;
        cx.session.policy = Role::User.template();
        let mut a = SystemApp::with_roots(roots.clone(), std::env::temp_dir());
        a.refresh(&cx);
        assert!(a.cur.as_ref().unwrap().procs.is_empty());
        key(&mut a, &cx, Key::Char('3'));
        assert_eq!(a.page(), Page::Mine);
        let g = frame(&mut a, &cx);
        assert_eq!(a.mine.iter().map(|o| o.proc.pid).collect::<Vec<_>>(), vec![cpid]);
        assert_eq!(a.mine[0].net.text(), "1 conn · 1 listening");
        assert!(g.hits.iter().any(|h| h.label == "my process"), "a row is drawn");
        // she cannot reach the other person's program, even by number: a stop on it is refused and nothing happens
        a.sel[4] = 0;
        key(&mut a, &cx, Key::Char('k'));
        assert!(matches!(a.confirm, Some(Confirm::Signal { mine: true, hard: false, .. })));
        key(&mut a, &cx, Key::Esc);
        assert!(a.confirm.is_none() && child.try_wait().unwrap().is_none());
        a.confirm = Some(Confirm::Signal { pid: opid, name: "other".into(), hard: true, mine: true });
        key(&mut a, &cx, Key::Enter);
        assert!(a.status.as_ref().is_some_and(|(t, ok)| !ok && t.contains("not one of yours")), "{:?}", a.status);
        assert!(other.try_wait().unwrap().is_none(), "the other person's program is untouched");
        // hers: asked, confirmed, audited, gone
        key(&mut a, &cx, Key::Char('K'));
        key(&mut a, &cx, Key::Enter);
        use std::os::unix::process::ExitStatusExt;
        assert_eq!(child.wait().unwrap().signal(), Some(9));
        assert!(cx.audit.recent(10).iter().any(|l| l.contains("system.kill") && l.contains("sleeper")));
        // an administrator sees both owners
        let mut cxa = cx_with_admin_store("sysmine2");
        cxa.session.user = "root-admin".into();
        let mut b = SystemApp::with_roots(roots.clone(), std::env::temp_dir());
        b.page = Page::Mine;
        let _ = frame(&mut b, &cxa);
        assert!(b.mine.iter().any(|o| o.owner == bob) && b.mine.iter().all(|o| o.owner == ann || o.owner == bob), "{:?}", b.mine.iter().map(|o| &o.owner).collect::<Vec<_>>());
        // and the Processes page shows the network use of a host process
        b.page = Page::Processes;
        b.refresh(&cxa);
        assert!(b.nets.get(&cpid).is_some_and(|n| n.connections == 1));
        let _ = other.kill();
        let _ = other.wait();
    }

    #[test]
    fn the_overview_shows_disk_activity_from_two_samples() {
        let roots = machine("diskact", None);
        std::fs::create_dir_all(roots.sys.join("block/sda")).unwrap();
        std::fs::write(roots.proc.join("diskstats"), "   8       0 sda 1 0 1000 0 1 0 1000 0 0 0 0\n").unwrap();
        let cx = cx_with_admin_store("sysdisk");
        let mut a = SystemApp::with_roots(roots.clone(), std::env::temp_dir());
        a.refresh(&cx);
        std::fs::write(roots.proc.join("diskstats"), "   8       0 sda 2 0 5000 0 2 0 3000 0 0 0 0\n").unwrap();
        std::thread::sleep(Duration::from_millis(30));
        a.refresh(&cx);
        let (rd, wr) = a.disk_rate;
        // 4000 sectors read and 2000 written in about 30 ms: a positive rate, reads twice the writes
        assert!(rd > 0.0 && (rd / wr - 2.0).abs() < 0.01, "{rd} {wr}");
        let g = frame(&mut a, &cx);
        let _ = g;
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
        let __wall = std::time::Instant::now();
        while __wall.elapsed() < std::time::Duration::from_secs(30) {
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
        let store = cx.services.real().runs.clone();
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

    struct WasmHost(Vec<iyakki_core_api::WasmTask>);
    impl iyakki_core_api::HostApi for WasmHost {
        fn detect_env(&self) -> String {
            String::new()
        }
        fn net_status(&self) -> String {
            String::new()
        }
        fn run_wasm(&self, _: &str, _: &iyakki_sec::policy::Policy, _: &[u8], _: &str) -> Result<String, String> {
            Err("unused".into())
        }
        fn power_off(&self) {}
        fn reboot(&self) {}
        fn wasm_tasks(&self) -> Vec<iyakki_core_api::WasmTask> {
            self.0.clone()
        }
    }

    #[test]
    fn the_supervisors_webassembly_tasks_are_rows_of_the_agents_page_for_their_owner_and_administrators() {
        let tasks = vec![
            iyakki_core_api::WasmTask { id: 1, user: "root".into(), entry: "_start".into(), bytes: 2048, started: iyakki_clock::now_unix() - 5 },
            iyakki_core_api::WasmTask { id: 2, user: "someone".into(), entry: "main".into(), bytes: 100, started: iyakki_clock::now_unix() },
        ];
        let mut cx = cx_with_admin_store("sys-wasm");
        cx.host = Arc::new(WasmHost(tasks));
        let mut a = SystemApp::with_roots(machine("wasm", None), std::env::temp_dir());
        a.refresh(&cx);
        assert_eq!(a.wasm.len(), 2, "an administrator sees every task");
        key(&mut a, &cx, Key::Char('3'));
        let g = frame(&mut a, &cx);
        assert!(g.layout_log.texts.iter().any(|t| t.3.contains("WebAssembly tasks")) && g.layout_log.texts.iter().any(|t| t.3.contains("_start") && t.3.contains("2 KB")), "{:?}", g.layout_log.texts);
        assert!(g.hits.iter().all(|h| h.label != "agent run"), "no agent run, so the page does not pretend there is one");
        let mut cx2 = cx.clone();
        cx2.session.user = "someone".into();
        cx2.session.role = Role::User;
        cx2.session.policy = Role::User.template();
        let mut b = SystemApp::with_roots(machine("wasm2", None), std::env::temp_dir());
        b.refresh(&cx2);
        assert_eq!(b.wasm.iter().map(|t| t.user.as_str()).collect::<Vec<_>>(), vec!["someone"]);
    }

    #[test]
    fn the_memory_card_shows_zram_ratio_and_pressure() {
        use iyakki_core_api::device_types::zram::{MemStats, ZramStats};
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
        let t = text_report(&iyakki_agent::corehandle::LocalHost, &roots, 5);
        assert!(t.contains("up 1d 2h") && t.contains("memory 2.0 GB of 8.2 GB used"), "{t}");
        assert!(t.contains("/bin/busy") && t.contains("/bin/iyakki"), "{t}");
    }
}


#[cfg(test)]
#[path = "system_cluster_tests.rs"]
mod cluster_tests;
#[cfg(test)]
#[path = "system_procs_tests.rs"]
mod procs_tests;
