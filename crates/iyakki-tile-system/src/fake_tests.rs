//! The SYSTEM tile on a fake port (docs/development/APP-DEVELOPMENT-PROCEDURE.md section 10): no `AppCx`, no core, no services, no session.
//! The host's figures are canned (the processor counters move by hand between two refreshes, so the percentages are exact), the runs, the
//! programs, the cluster and the audit list are in-memory. The screen tests over the real port and a made-up `/proc` stay in the shell crate.

use std::cell::RefCell;
use std::sync::{Arc, Mutex};

use iyakki_sysinfo_model::kubectl::Kubectl;
use iyakki_sysinfo_model::{CpuTimes, DiskCounters, DiskUse, Memory, NetCounters, NetUse, Proc, SysRoots};
use iyakki_tile_api::system::{ClusterHealth, LiveRun, MemStats, WasmTask};
use iyakki_uikit::gfx::{rgb, Gfx};
use iyakki_uikit::ui::{Event, Key, R};

use super::*;

struct FakeHost {
    cpu: Mutex<CpuTimes>,
    procs: Vec<Proc>,
    signals: Mutex<Vec<(u32, bool)>>,
}

impl SysRead for FakeHost {
    fn cpu_times(&self, _r: &SysRoots) -> CpuTimes {
        *self.cpu.lock().unwrap()
    }
    fn cpu_count(&self, _r: &SysRoots) -> usize {
        4
    }
    fn memory_of(&self, _r: &SysRoots) -> Memory {
        Memory { total: 8 << 30, available: 6 << 30, swap_total: 0, swap_free: 0 }
    }
    fn load_average(&self, _r: &SysRoots) -> (f32, f32, f32) {
        (0.5, 0.4, 0.3)
    }
    fn uptime_of(&self, _r: &SysRoots) -> u64 {
        93_784
    }
    fn hottest_temp(&self, _r: &SysRoots) -> Option<f32> {
        Some(51.0)
    }
    fn net_counters(&self, _r: &SysRoots) -> Vec<NetCounters> {
        vec![]
    }
    fn disk_counters(&self, _r: &SysRoots) -> Vec<DiskCounters> {
        vec![]
    }
    fn net_use(&self, _r: &SysRoots, _pids: &[u32]) -> std::collections::HashMap<u32, NetUse> {
        Default::default()
    }
    fn processes(&self, _r: &SysRoots) -> Vec<Proc> {
        self.procs.clone()
    }
    fn signal(&self, pid: u32, hard: bool) -> Result<(), String> {
        self.signals.lock().unwrap().push((pid, hard));
        Ok(())
    }
}

struct Fake {
    admin: bool,
    host: FakeHost,
    live: RefCell<Vec<LiveRun>>,
    mine: Vec<OwnedProc>,
    stopped_programs: RefCell<Vec<(u32, bool)>>,
    health: ClusterHealth,
    log: RefCell<Vec<String>>,
}

const ME: &str = "ann";

fn proc(pid: u32, name: &str, user: &str, rss: u64, ticks: u64) -> Proc {
    Proc { pid, ppid: 1, uid: 1000, user: user.into(), name: name.into(), cmd: format!("/usr/bin/{name}"), state: 'S', threads: 1, ticks, rss, start_ticks: 0, cgroup: String::new() }
}

fn live(id: &str, user: &str) -> LiveRun {
    LiveRun { id: id.into(), user: user.into(), prompt: "tidy the folder".into(), started: iyakki_clock::now_unix() - 30, pending: 0, steps: 3, tokens: 0, last: String::new() }
}

impl Fake {
    fn new(admin: bool) -> Fake {
        Fake {
            admin,
            host: FakeHost { cpu: Mutex::new(CpuTimes { busy: 100, total: 1000 }), procs: vec![proc(10, "busy", "root", 9000, 100), proc(11, "idle", "ann", 100, 1)], signals: Mutex::new(vec![]) },
            live: RefCell::new(vec![live("r1", ME), live("r2", "bob")]),
            mine: vec![OwnedProc { proc: proc(11, "sleep", ME, 100, 1), owner: ME.into(), what: "sleep 60".into(), net: NetUse::default(), disk: DiskUse::default() }],
            stopped_programs: RefCell::new(vec![]),
            health: ClusterHealth::default(),
            log: RefCell::new(vec![]),
        }
    }
    fn logged(&self, w: &str) -> bool {
        self.log.borrow().iter().any(|l| l.contains(w))
    }
}

struct NoKube;
impl Kubectl for NoKube {
    fn kubectl(&self, _a: &[&str]) -> Result<String, String> {
        Err("no cluster".into())
    }
}

impl SystemPort for Fake {
    fn user(&self) -> String {
        ME.into()
    }
    fn is_admin(&self) -> bool {
        self.admin
    }
    fn audit(&self, action: &str, subject: &str) {
        self.log.borrow_mut().push(format!("{action} {subject}"));
    }
    fn host(&self) -> Box<dyn SysRead + '_> {
        Box::new(&self.host)
    }
    fn memory_stats(&self) -> MemStats {
        Default::default()
    }
    fn mac_summary(&self) -> Option<String> {
        Some("MAC: aa:bb:cc:dd:ee:ff (private)".into())
    }
    fn hardware_report(&self) -> Vec<String> {
        vec!["CPU: fake".into(), "Memory: 8 GB".into()]
    }
    fn disk_space(&self, _path: &str) -> Result<(u64, u64), String> {
        Ok((30_000_000_000, 120_000_000_000))
    }
    fn audio_summary(&self, _p: &std::path::Path) -> String {
        "Sound plays on the built-in speakers".into()
    }
    fn gpu_lines(&self, _root: &std::path::Path) -> Vec<String> {
        vec!["Fake Graphics · driver fake".into()]
    }
    fn hostname(&self) -> String {
        "fakehost".into()
    }
    fn machine_about(&self) -> String {
        "IYAKKI test machine".into()
    }
    fn cluster_installed(&self) -> bool {
        false
    }
    fn cluster_running(&self) -> bool {
        false
    }
    fn cluster_health(&self) -> ClusterHealth {
        self.health.clone()
    }
    fn cluster_refresh_health(&self) -> Arc<dyn Fn() + Send + Sync> {
        Arc::new(|| {})
    }
    fn cluster_kubectl(&self) -> Arc<dyn Kubectl> {
        Arc::new(NoKube)
    }
    fn cluster_log_tail(&self, _lines: usize) -> String {
        String::new()
    }
    fn live_runs(&self) -> Vec<LiveRun> {
        self.live.borrow().clone()
    }
    fn stop_live(&self, id: &str) -> bool {
        let mut v = self.live.borrow_mut();
        let n = v.len();
        v.retain(|r| r.id != id);
        v.len() != n
    }
    fn stop_all_live(&self, only: Option<&str>) -> usize {
        let mut v = self.live.borrow_mut();
        let n = v.len();
        v.retain(|r| only.is_some_and(|u| r.user != u));
        n - v.len()
    }
    fn wasm_tasks(&self) -> Vec<WasmTask> {
        vec![]
    }
    fn programs_mine(&self, _roots: &SysRoots) -> Vec<OwnedProc> {
        self.mine.clone()
    }
    fn program_stop(&self, _roots: &SysRoots, pid: u32, hard: bool) -> Result<String, String> {
        self.stopped_programs.borrow_mut().push((pid, hard));
        Ok("stopped".into())
    }
    fn redact(&self, line: &str) -> String {
        line.replace("hunter2", "***")
    }
    fn scheduler_running(&self) -> bool {
        false
    }
}

fn tile() -> SystemApp {
    SystemApp::with_roots(SysRoots { proc: "/nonexistent/proc".into(), sys: "/nonexistent/sys".into(), passwd: "/nonexistent/passwd".into() }, "/nonexistent/data".into())
}

fn frame(a: &mut SystemApp, p: &Fake, w: u32, h: u32) -> Gfx {
    let mut g = Gfx::new(w, h);
    g.clear(rgb(9, 11, 20));
    a.render_with(&mut g, R::new(0.0, 46.0, w as f32, h as f32 - 46.0), 1.0, p);
    g
}

#[test]
fn the_pages_follow_the_role_and_the_overview_shows_the_figures_the_port_gave() {
    let admin = Fake::new(true);
    let mut a = tile();
    a.refresh_with(&admin);
    let g = frame(&mut a, &admin, 1280, 720);
    for label in ["Processes", "Hardware"] {
        assert!(g.hits.iter().any(|h| h.label == label), "an administrator sees {label}");
    }
    let user = Fake::new(false);
    let mut b = tile();
    b.refresh_with(&user);
    let g = frame(&mut b, &user, 1280, 720);
    assert!(g.hits.iter().all(|h| h.label != "Processes" && h.label != "Hardware"), "a person without the setting sees only Overview, Agents and Mine");
    assert_eq!(b.page(), Page::Overview);
    assert!(b.mac_line.as_deref().unwrap().starts_with("MAC:") && b.sound_line.contains("speakers"));
}

#[test]
fn the_processor_percentage_comes_from_two_samples_of_the_ports_counters() {
    let p = Fake::new(true);
    let mut a = tile();
    a.refresh_with(&p);
    *p.host.cpu.lock().unwrap() = CpuTimes { busy: 600, total: 1800 };
    a.refresh_with(&p);
    assert_eq!(a.history.last().copied().map(|v| v.round() as i32), Some(63), "500 busy of 800 total");
    // the Processes page lists what the port's host gave, the busiest first
    a.handle_with(&Event::Key(Key::Tab), &p);
    assert_eq!(a.page(), Page::Processes);
    let g = frame(&mut a, &p, 1280, 720);
    assert!(g.hits.iter().any(|h| h.label == "Stop (k)"), "an administrator can stop a process");
}

#[test]
fn stopping_a_process_asks_first_signals_through_the_host_and_writes_the_audit_line() {
    let p = Fake::new(true);
    let mut a = tile();
    a.refresh_with(&p);
    a.handle_with(&Event::Key(Key::Tab), &p);
    frame(&mut a, &p, 1280, 720);
    a.handle_with(&Event::Key(Key::Char('k')), &p);
    assert!(a.confirm.is_some(), "it asks first");
    frame(&mut a, &p, 1280, 720);
    a.handle_with(&Event::Key(Key::Char('n')), &p);
    assert!(p.host.signals.lock().unwrap().is_empty());
    a.handle_with(&Event::Key(Key::Char('K')), &p);
    a.handle_with(&Event::Key(Key::Char('y')), &p);
    let sent = p.host.signals.lock().unwrap().clone();
    assert_eq!(sent.len(), 1);
    assert!(sent[0].1, "K is the hard kill");
    assert!(p.logged("system.kill"), "{:?}", p.log.borrow());
}

#[test]
fn a_person_sees_and_stops_only_their_own_runs_and_an_administrator_everyones() {
    let user = Fake::new(false);
    let mut a = tile();
    a.refresh_with(&user);
    assert_eq!(a.live.iter().map(|r| r.id.as_str()).collect::<Vec<_>>(), ["r1"], "bob's run is not shown");
    a.handle_with(&Event::Tab(2), &user);
    a.page = Page::Agents;
    a.handle_with(&Event::Key(Key::Char('x')), &user);
    assert!(user.logged("system.agent.stop"));
    assert!(user.live.borrow().iter().all(|r| r.id != "r1") && user.live.borrow().len() == 1);
    // the administrator stops all of them after asking
    let admin = Fake::new(true);
    let mut b = tile();
    b.refresh_with(&admin);
    assert_eq!(b.live.len(), 2);
    b.page = Page::Agents;
    frame(&mut b, &admin, 1280, 720);
    b.handle_with(&Event::Key(Key::Char('X')), &admin);
    assert!(b.confirm.is_some());
    b.handle_with(&Event::Key(Key::Char('y')), &admin);
    assert!(admin.live.borrow().is_empty() && admin.logged("system.agents.stop_all 2"), "{:?}", admin.log.borrow());
}

#[test]
fn the_mine_page_lists_the_programs_of_the_port_and_stops_one_through_it() {
    let p = Fake::new(false);
    let mut a = tile();
    a.page = Page::Mine;
    a.refresh_with(&p);
    assert_eq!(a.mine.len(), 1);
    let g = frame(&mut a, &p, 1280, 720);
    assert!(g.hits.iter().any(|h| h.label == "Stop (k)"));
    a.handle_with(&Event::Key(Key::Char('k')), &p);
    a.handle_with(&Event::Key(Key::Char('y')), &p);
    assert_eq!(p.stopped_programs.borrow().as_slice(), [(11, false)]);
    assert!(p.logged("system.stop 11"), "{:?}", p.log.borrow());
}

#[test]
fn every_page_draws_at_the_sizes() {
    let p = Fake::new(true);
    let mut a = tile();
    a.refresh_with(&p);
    for (w, h) in [(1280u32, 720u32), (800, 600), (480, 400)] {
        for page in [Page::Overview, Page::Processes, Page::Agents, Page::Hardware, Page::Mine] {
            a.page = page;
            a.refresh_with(&p);
            frame(&mut a, &p, w, h);
        }
    }
}

#[test]
fn the_memory_line_names_zram_its_ratio_and_the_stall() {
    use iyakki_tile_api::system::Zram;
    assert_eq!(memory_extra_line(&MemStats::default()), "no zram");
    let mut m = MemStats { zram: Some(Zram { algorithm: "zstd".into(), disksize: 4 << 30, orig: 3 << 30, compressed: 1 << 30 }), pressure: Some([2.4, 1.0, 0.5]) };
    assert_eq!(memory_extra_line(&m), "zram zstd 3.0:1 (3.2 GB in 1.1 GB) · stall 2%");
    m.zram.as_mut().unwrap().orig = 0;
    m.zram.as_mut().unwrap().compressed = 0;
    assert!(memory_extra_line(&m).contains("empty"));
    m.zram.as_mut().unwrap().disksize = 0;
    assert_eq!(memory_extra_line(&m), "zram off · stall 2%");
}

#[test]
fn the_overview_reads_the_name_the_about_line_and_the_cards_through_the_port() {
    let p = Fake::new(true);
    let mut a = tile();
    a.refresh_with(&p);
    assert_eq!((a.host_name.as_str(), a.about_line.as_str()), ("fakehost", "IYAKKI test machine"));
    assert_eq!(a.gpu_lines, vec!["Fake Graphics · driver fake".to_string()]);
    let g = frame(&mut a, &p, 1280, 720);
    let said = |needle: &str| g.text_log.iter().any(|t| t.2.contains(needle));
    assert!(said("fakehost") && said("IYAKKI test machine") && said("Fake Graphics"), "the Overview draws the name, the about line and the card");
}

#[test]
fn the_cluster_row_shows_only_when_it_is_installed_and_asks_for_a_fresh_reading_when_stale() {
    let mut p = Fake::new(true);
    let mut a = tile();
    a.refresh_with(&p);
    frame(&mut a, &p, 1280, 720);
    p.health = ClusterHealth { installed: true, state: "running".into(), line: "running · v1.30 · node Ready".into(), checked_at: 0, ..Default::default() };
    a.refresh_with(&p);
    let g = frame(&mut a, &p, 1280, 720);
    assert!(g.hits.iter().any(|h| h.label == "View logs"), "an administrator can open the cluster's log");
}
