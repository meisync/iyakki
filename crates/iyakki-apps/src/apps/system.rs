//! SYSTEM in the shell: the tile is the crate `iyakki-tile-system` (alpha.9 P6). This file gives it the shell's context: on every call it builds
//! the real port (`system_port::CxPort`: the core's host facts, the memory, hardware, audio and cluster facets, the runs, the programs started for
//! the person, the audit log) and forwards the `App` methods. The tile's own tests run in the crate against a fake port; the tests here run it
//! over the real services and a made-up `/proc`. `Deref` shows the tile to the tests. `text_report` (`iyakki top`) stays here: it reads the core's
//! host facts directly.

use std::ops::{Deref, DerefMut};
use std::time::Duration;

use iyakki_core_api::sysinfo::{self, SysRoots};
use iyakki_core_api::HostFacts;

use super::system_port::{mem_stats, CxPort};
use super::{App, AppCx, Flow as ShellFlow};
use crate::gfx::{Gfx, Icon};
use crate::ui::{Event, R};
#[cfg(test)]
use crate::gfx::rgb;
#[cfg(test)]
use crate::ui::{Key, ERR};

pub use iyakki_tile_system::app::{Confirm, Page, Sample, Sort};

pub struct SystemApp(iyakki_tile_system::SystemApp);

impl Deref for SystemApp {
    type Target = iyakki_tile_system::SystemApp;
    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl DerefMut for SystemApp {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.0
    }
}

impl SystemApp {
    pub fn new(_cx: &AppCx) -> SystemApp {
        SystemApp(iyakki_tile_system::SystemApp::with_roots(SysRoots::real(), "/var/lib/iyakki".into()))
    }

    pub fn with_roots(roots: SysRoots, data_dir: std::path::PathBuf) -> SystemApp {
        SystemApp(iyakki_tile_system::SystemApp::with_roots(roots, data_dir))
    }

    /// Read the kernel's figures now.
    pub fn refresh(&mut self, cx: &AppCx) {
        self.0.refresh_with(&CxPort(cx));
    }

    /// The page showing (an inherent method, so it wins over the shell's `App::page` text).
    pub fn page(&self) -> Page {
        self.0.page()
    }
}

/// The extra line of the MEMORY card: compressed memory (zram) and memory pressure.
pub fn memory_extra_line(m: &iyakki_core_api::memory::MemStats) -> String {
    iyakki_tile_system::memory_extra_line(&mem_stats(m))
}

impl App for SystemApp {
    fn title(&self) -> &'static str {
        self.0.title()
    }
    fn icon(&self) -> Icon {
        self.0.icon()
    }
    fn accent(&self) -> [u8; 4] {
        self.0.accent()
    }
    fn hint(&self) -> String {
        self.0.hint()
    }
    fn help(&self) -> Vec<String> {
        self.0.help()
    }
    fn handle(&mut self, ev: &Event, cx: &AppCx) -> ShellFlow {
        match self.0.handle_with(ev, &CxPort(cx)) {
            iyakki_tile_system::Flow::Stay => ShellFlow::Stay,
            iyakki_tile_system::Flow::Back => ShellFlow::Back,
        }
    }
    fn render(&mut self, g: &mut Gfx, area: R, s: f32, cx: &AppCx, _tick: u64) {
        self.0.render_with(g, area, s, &CxPort(cx));
    }
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
        let first = a.live.iter().position(|r| r.user == "root").unwrap();
        a.set_sel(first);
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

