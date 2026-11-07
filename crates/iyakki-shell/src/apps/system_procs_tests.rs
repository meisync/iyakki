//! SYSTEM Processes tab: the service of a cluster process, the grouping, the near-limit marker, the busy aid. Fake cluster answers only.

use super::*;
use crate::apps::tests::{cx_with_admin_store, temp_dir};
use iyakki_agent::dash::cluster::Kubectl;
use iyakki_agent::sysprocs::PodTable;
use std::sync::Arc;

const UID: &str = "7c1e2f3a-aaaa-4bbb-8ccc-0123456789ab";
const CID: &str = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";

struct FakeKube;
impl Kubectl for FakeKube {
    fn kubectl(&self, args: &[&str]) -> Result<String, String> {
        if args.contains(&"top") {
            return Ok("clickhouse-0  clickhouse  1020m  775Mi\n".into());
        }
        if args.contains(&"logs") {
            return Ok("merging parts\napi_key=sk-abcdefghijklmnopqrstuvwxyz0123456789\n".into());
        }
        Ok(format!(
            r#"{{"items":[{{"metadata":{{"uid":"{UID}","namespace":"iyakki-data","name":"clickhouse-0","labels":{{"app":"clickhouse"}}}},"spec":{{"containers":[{{"name":"clickhouse","resources":{{"limits":{{"cpu":"1","memory":"800Mi"}}}}}}]}},"status":{{"containerStatuses":[{{"name":"clickhouse","containerID":"containerd://{CID}"}}]}}}}]}}"#
        ))
    }
}

fn machine(tag: &str) -> SysRoots {
    let d = temp_dir(&format!("{tag}-sys"));
    let proc = d.join("proc");
    std::fs::create_dir_all(proc.join("net")).unwrap();
    std::fs::write(proc.join("stat"), "cpu  100 0 0 800 0 0 0 0 0 0\ncpu0 100 0 0 800 0 0 0 0 0 0\n").unwrap();
    std::fs::write(proc.join("meminfo"), "MemTotal: 8000000 kB\nMemAvailable: 6000000 kB\nSwapTotal: 0 kB\nSwapFree: 0 kB\n").unwrap();
    std::fs::write(proc.join("loadavg"), "0.5 0.4 0.3 1/200 9\n").unwrap();
    std::fs::write(proc.join("uptime"), "100.0 1.0\n").unwrap();
    std::fs::write(proc.join("net/dev"), "h\nh\n  eth0: 1 1 0 0 0 0 0 0 1 1 0 0 0 0 0 0\n").unwrap();
    std::fs::write(d.join("passwd"), "root:x:0:0::/root:/bin/sh\n").unwrap();
    let cg = format!("0::/kubepods.slice/kubepods-burstable.slice/kubepods-burstable-pod{}.slice/cri-containerd-{CID}.scope\n", UID.replace('-', "_"));
    for (pid, name, rss, cgroup) in [(1u32, "iyakki", 1000u64, ""), (7, "clickhouse-serv", 220_000, cg.as_str()), (8, "clickhouse-serv", 1000, cg.as_str())] {
        let p = proc.join(pid.to_string());
        std::fs::create_dir_all(&p).unwrap();
        std::fs::write(p.join("stat"), format!("{pid} ({name}) S 1 0 0 0 0 0 0 0 0 0 10 0 0 0 20 0 1 0 5 1 {rss} 0\n")).unwrap();
        std::fs::write(p.join("status"), "Uid:\t101\t101\t101\t101\n").unwrap();
        std::fs::write(p.join("cmdline"), format!("/usr/bin/{name}\0")).unwrap();
        std::fs::write(p.join("cgroup"), cgroup).unwrap();
    }
    SysRoots { proc, sys: d.join("sys"), passwd: d.join("passwd") }
}

fn app(tag: &str, cluster: bool) -> (SystemApp, AppCx) {
    let cx = cx_with_admin_store(tag);
    let mut a = SystemApp::with_roots(machine(tag), temp_dir(&format!("{tag}-data")));
    a.refresh(&cx);
    a.page = Page::Processes;
    if cluster {
        a.pods.refresh_now(&FakeKube, 0);
    }
    let table = a.pods.table();
    a.rows = sysprocs::rows(&a.cur.as_ref().unwrap().procs, &a.proc_cpu, &table).into_iter().map(|r| (r.pid, r)).collect();
    (a, cx)
}

fn draw(a: &mut SystemApp, cx: &AppCx, w: u32, h: u32, s: f32) -> Gfx {
    let mut g = Gfx::new(w, h);
    g.clear(rgb(9, 11, 20));
    a.render(&mut g, R::new(0.0, 72.0 * s, w as f32, h as f32 - 72.0 * s), s, cx, 0);
    g
}

#[test]
fn a_cluster_process_carries_its_service_and_no_cluster_means_no_labels() {
    let (a, _) = app("sysp1", true);
    let l = a.rows[&7].label.as_ref().unwrap();
    assert_eq!((l.service.as_str(), l.ns.as_str()), ("clickhouse", "iyakki-data"));
    assert!(a.rows[&7].near, "220000 pages of 4 KiB is 859 MB against an 800 MB limit");
    assert!(a.rows[&1].label.is_none() && !a.rows[&8].near);
    let (b, _) = app("sysp2", false);
    assert!(b.rows.values().all(|r| r.label.is_none() && !r.near));
    assert_eq!(b.selected_pod(), None);
}

#[test]
fn grouping_by_service_toggles_with_key_and_button_and_keeps_sorting() {
    let (mut a, cx) = app("sysp3", true);
    assert!(!a.grouped);
    a.handle(&Event::Key(Key::Char('g')), &cx);
    assert!(a.grouped);
    let g = a.sorted_groups();
    assert_eq!(g.len(), 2);
    assert_eq!((g[0].name.as_str(), g[0].count), ("clickhouse (iyakki-data)", 2));
    a.handle(&Event::Key(Key::Char('n')), &cx);
    assert_eq!(a.sorted_groups()[0].name, "clickhouse (iyakki-data)");
    a.handle(&Event::Key(Key::Char('K')), &cx);
    assert!(a.confirm.is_none(), "grouped lines are not stopped");
    a.handle(&Event::Tab(24), &cx);
    assert!(!a.grouped);
}

#[test]
fn what_is_it_busy_with_opens_a_redacted_log_for_a_pod_line_only() {
    let (mut a, cx) = app("sysp4", true);
    // process 7 is the first by memory
    a.handle(&Event::Key(Key::Char('m')), &cx);
    assert_eq!(a.selected_pod(), Some(("iyakki-data".into(), "clickhouse-0".into())));
    a.pods.ask_busy(Arc::new(FakeKube), "iyakki-data".into(), "clickhouse-0".into());
    let mut got = None;
    for _ in 0..500 {
        if let Some(x) = a.pods.take_answer() {
            got = Some(x);
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    let (_, text) = got.expect("an answer");
    assert!(text.contains("1020m") && text.contains("merging parts") && !text.contains("sk-abcdef"), "{text}");
    // a host line is not a pod
    a.sel[1] = a.sorted().iter().position(|p| p.pid == 1).unwrap();
    a.handle(&Event::Key(Key::Char('b')), &cx);
    assert!(a.status.as_ref().is_some_and(|s| !s.1));
}

#[test]
fn the_processes_tab_draws_inside_the_screen_without_overlap_at_the_four_sizes() {
    for (w, h, s) in [(1280u32, 720u32, 1.0f32), (1920, 1080, 1.5), (1024, 600, 0.8), (800, 480, 0.62)] {
        for grouped in [false, true] {
            let (mut a, cx) = app(&format!("sysp5-{w}-{grouped}"), true);
            a.grouped = grouped;
            let g = draw(&mut a, &cx, w, h, s);
            let area = R::new(0.0, 72.0 * s, w as f32, h as f32 - 72.0 * s);
            let buttons: Vec<_> = g.hits.iter().filter(|x| matches!(x.ev, Event::Tab(20..=25))).collect();
            assert!(buttons.iter().any(|b| b.ev == Event::Tab(25)) && buttons.iter().any(|b| b.ev == Event::Tab(24)), "{w}x{h}");
            for b in &buttons {
                assert!(b.rect.x >= area.x && b.rect.y >= area.y && b.rect.x + b.rect.w <= area.x + area.w + 0.5 && b.rect.y + b.rect.h <= area.y + area.h + 0.5, "{w}x{h} grouped {grouped}: {:?} outside {area:?}", b.rect);
                for o in buttons.iter().filter(|o| o.ev != b.ev) {
                    let (x, y) = (b.rect.x.max(o.rect.x), b.rect.y.max(o.rect.y));
                    assert!(!(x < (b.rect.x + b.rect.w).min(o.rect.x + o.rect.w) - 0.5 && y < (b.rect.y + b.rect.h).min(o.rect.y + o.rect.h) - 0.5), "{w}x{h}: {:?} covers {:?}", b.ev, o.ev);
                }
            }
            // the rows are touchable and stay on the screen
            let rows: Vec<_> = g.hits.iter().filter(|x| matches!(x.ev, Event::Row(_))).collect();
            assert!(!rows.is_empty());
            for r in rows {
                assert!(r.rect.x >= 0.0 && r.rect.x + r.rect.w <= w as f32 + 0.5 && r.rect.y + r.rect.h <= area.y + area.h - 30.0 * s, "{w}x{h}: row {:?}", r.rect);
            }
        }
    }
}

#[test]
fn an_ordinary_account_gets_no_labels_and_no_cluster_read() {
    let mut cx = cx_with_admin_store("sysp6");
    cx.session.role = iyakki_sec::policy::Role::User;
    cx.session.policy = iyakki_sec::policy::Role::User.template();
    let mut a = SystemApp::with_roots(machine("sysp6"), temp_dir("sysp6-data"));
    a.refresh(&cx);
    assert!(a.rows.is_empty() && a.pods.table().is_empty());
}

#[allow(dead_code)]
fn _table_is_public(_: PodTable) {}
