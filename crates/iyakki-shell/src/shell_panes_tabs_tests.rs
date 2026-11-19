//! Tabs, clicks, drags and the wheel inside panes: every app with a tab strip, in each two-pane template, at the usual sizes
//! (`docs/MULTI-DISPLAY.md`). A click in a pane reaches that pane's app at the pane's local coordinates.
use super::tests::*;
use super::*;
use crate::panes::LayoutKey;
use std::sync::{Arc, Mutex};

const SIZES: [(u32, u32); 4] = [(1280, 720), (1920, 1080), (2560, 1600), (2880, 1800)];
const TEMPLATE_KEYS: [(u8, &str); 3] = [(2, "cols-50-50"), (4, "cols-30-70"), (5, "cols-70-30")];

fn apps() -> Vec<PApp> {
    match std::env::var("PANE_TABS_APP") {
        Ok(n) => PApp::ALL.iter().copied().filter(|a| a.id() == n).collect(),
        // the full list (all apps, `PANE_TABS_ALL=1`) takes a quarter of an hour; the quick one covers the tab styles
        Err(_) if std::env::var("PANE_TABS_ALL").is_ok() => PApp::ALL.iter().copied().filter(|a| !matches!(a, PApp::Camera | PApp::Browser | PApp::Ide | PApp::Voice)).collect(),
        Err(_) => vec![PApp::Settings, PApp::System, PApp::Store, PApp::Calendar, PApp::Usage],
    }
}

fn pixel(g: &Gfx, x: i32, y: i32) -> [u8; 4] {
    let i = ((y as u32 * g.width() + x as u32) * 4) as usize;
    let d = g.pm.data();
    [d[i], d[i + 1], d[i + 2], d[i + 3]]
}

fn hash(g: &Gfx, r: R) -> u64 {
    let d = g.pm.data();
    let (gw, gh) = (g.width() as i32, g.height() as i32);
    let mut h = 0xcbf29ce484222325u64;
    for y in (r.y as i32).max(0)..((r.y + r.h) as i32).min(gh) {
        for x in (r.x as i32).max(0)..((r.x + r.w) as i32).min(gw) {
            let i = ((y * gw + x) * 4) as usize;
            for b in &d[i..i + 4] {
                h = (h ^ *b as u64).wrapping_mul(0x100000001b3);
            }
        }
    }
    h
}

/// The tab regions of pane `p` (not the strip's own buttons), as the shell sees them.
fn tab_hits(sh: &Shell, p: usize) -> Vec<crate::ui::Hit> {
    let r = sh.panes_rects_for_test()[p];
    let strip = 30.0 * scale_for(r.w as u32, r.h as u32);
    let mut v: Vec<crate::ui::Hit> = Vec::new();
    for h in sh.hits() {
        if let Event::Tab(n) = h.ev {
            if n < 0x5C00 && h.rect.x >= r.x - 0.6 && h.rect.x + h.rect.w <= r.x + r.w + 0.6 && h.rect.y >= r.y + strip - 0.6 && h.rect.y + h.rect.h <= r.y + r.h + 0.6 && !v.iter().any(|o| o.ev == h.ev) {
                v.push(h.clone());
            }
        }
    }
    v
}

fn session(tag: &str, w: u32, h: u32) -> (Shell, Gfx) {
    let (mut sh, _) = shell_with_users(tag);
    let mut g = Gfx::new(w, h);
    sh.render(&mut g);
    login(&mut sh, "root", "rootpass12");
    sh.set_mem_for_test(Some(crate::memgate::MemFacts { total: 64 << 30, available: 48 << 30 }));
    sh.render(&mut g);
    (sh, g)
}

/// A fresh two-pane template with the real app in pane `p` and the focus on the other pane (so a click has to move focus).
fn place(sh: &mut Shell, g: &mut Gfx, key: u8, app: PApp, p: usize) {
    sh.handle(&Event::Layout(LayoutKey::Apply(1)));
    sh.handle(&Event::Layout(LayoutKey::Apply(key)));
    sh.render(g);
    sh.ws_focus(p);
    sh.open_for_test(app);
    sh.ws_focus(1 - p);
    sh.render(g);
    sh.render(g);
}

fn click(sh: &mut Shell, g: &mut Gfx, x: i32, y: i32) {
    sh.handle(&Event::MouseMove(x, y));
    sh.handle(&Event::MouseDown(x, y));
    sh.handle(&Event::MouseUp(x, y));
    sh.render(g);
}

/// Every real app: clicking a tab-like region in the pane that is not focused focuses that pane and the app reacts, the same
/// way it does in the single screen. Set `PANE_TABS_APP=<id>` to run one app, `PANE_TABS_ALL=1` for all four sizes.
#[test]
fn every_apps_tabs_work_in_each_template_when_clicked_in_an_unfocused_or_focused_pane() {
    let sizes: Vec<(u32, u32)> = if std::env::var("PANE_TABS_ALL").is_ok() { SIZES.to_vec() } else { vec![(1920, 1080)] };
    let mut report = Vec::new();
    for (w, h) in sizes {
        let (mut sh, mut g) = session("pt1", w, h);
        for app in apps() {
            for (key, id) in TEMPLATE_KEYS {
                for p in 0..2usize {
                    place(&mut sh, &mut g, key, app, p);
                    assert_eq!(sh.panes_layout_for_test(), id);
                    let rect = sh.panes_rects_for_test()[p];
                    let tabs: Vec<_> = tab_hits(&sh, p).into_iter().take(4).collect();
                    if tabs.len() < 2 {
                        continue;
                    }
                    let mut changed = 0;
                    for (k, t) in tabs.iter().enumerate() {
                        // alternate: the other pane focused (the click must move focus), then the pane itself focused
                        if k % 2 == 0 {
                            sh.ws_focus(1 - p);
                        } else {
                            sh.ws_focus(p);
                        }
                        sh.render(&mut g);
                        let before = hash(&g, rect);
                        let Some(t2) = tab_hits(&sh, p).into_iter().find(|x| x.ev == t.ev) else { continue };
                        let (x, y) = ((t2.rect.x + t2.rect.w / 2.0) as i32, (t2.rect.y + t2.rect.h / 2.0) as i32);
                        click(&mut sh, &mut g, x, y);
                        if sh.panes_focus_for_test() != Some(p) {
                            report.push(format!("{app:?} {w}x{h} {id} pane {p} tab {k}: focus not moved"));
                        }
                        if hash(&g, rect) != before {
                            changed += 1;
                        }
                    }
                    if changed == 0 {
                        report.push(format!("{app:?} {w}x{h} {id} pane {p}: {} tab regions, no click changed anything", tabs.len()));
                    }
                }
            }
        }
    }
    assert!(report.is_empty(), "{}", report.join("\n"));
}

/// A stand-in app with a tab strip of three tabs at the top of its area. It records every event it gets and the size it was given.
struct Tabbed {
    tab: usize,
    log: Arc<Mutex<Vec<String>>>,
}

impl App for Tabbed {
    fn title(&self) -> &'static str {
        "TABBED"
    }
    fn icon(&self) -> Icon {
        Icon::Chat
    }
    fn accent(&self) -> [u8; 4] {
        [40, 160, 200, 255]
    }
    fn hint(&self) -> String {
        "tabbed".into()
    }
    fn handle(&mut self, ev: &Event, _cx: &AppCx) -> Flow {
        match ev {
            Event::Tick | Event::MouseMove(..) => {}
            Event::Tab(n) if *n < 3 => self.tab = *n,
            Event::Key(Key::Tab) => self.tab = (self.tab + 1) % 3,
            Event::Key(Key::BackTab) => self.tab = (self.tab + 2) % 3,
            Event::Key(Key::Char(c @ '1'..='3')) => self.tab = *c as usize - '1' as usize,
            _ => {}
        }
        self.log.lock().unwrap().push(format!("{ev:?}"));
        Flow::Stay
    }
    fn render(&mut self, g: &mut Gfx, area: R, s: f32, _cx: &AppCx, _tick: u64) {
        g.rect(area.x, area.y, area.w, area.h, [20, 20, 40, 255]);
        let tw = (area.w / 3.0).min(160.0 * s);
        for i in 0..3 {
            let r = R::new(area.x + i as f32 * tw, area.y, tw - 2.0, 30.0 * s);
            g.rect(r.x, r.y, r.w, r.h, if i == self.tab { [200, 200, 40, 255] } else { [60, 60, 90, 255] });
            g.hit(r, Event::Tab(i), &format!("tab {i}"));
        }
    }
}

fn tab_of(sh: &Shell) -> String {
    sh.panes_titles_for_test().join(",")
}

fn open_tabbed(sh: &mut Shell, pane: usize) -> Arc<Mutex<Vec<String>>> {
    let log = Arc::new(Mutex::new(Vec::new()));
    let app: Box<dyn App> = Box::new(Tabbed { tab: 0, log: log.clone() });
    sh.panes_put_for_test(pane, app, PApp::Settings);
    log
}

#[test]
fn tabs_clicks_keys_drags_and_the_wheel_reach_the_pane_under_them_with_local_coordinates() {
    for (w, h) in SIZES {
        let (mut sh, mut g) = session("pt2", w, h);
        for (key, id) in TEMPLATE_KEYS {
            sh.handle(&Event::Layout(LayoutKey::Apply(1)));
            sh.handle(&Event::Layout(LayoutKey::Apply(key)));
            sh.render(&mut g);
            assert_eq!(sh.panes_layout_for_test(), id);
            let logs = [open_tabbed(&mut sh, 0), open_tabbed(&mut sh, 1)];
            sh.render(&mut g);
            sh.render(&mut g);
            let rects = sh.panes_rects_for_test();
            for p in 0..2usize {
                let what = format!("{w}x{h} {id} pane {p} ({})", tab_of(&sh));
                for want in [2usize, 1, 0] {
                    // focus the other pane first: the click has to move focus and be delivered
                    let other = 1 - p;
                    sh.ws_focus(other);
                    sh.render(&mut g);
                    let hit = sh.hits().iter().rev().find(|x| x.ev == Event::Tab(want) && rects[p].contains((x.rect.x + 1.0) as i32, (x.rect.y + 1.0) as i32)).cloned().unwrap_or_else(|| panic!("tab {want} of {what} has a region: {:?}", sh.hits().iter().map(|x| (x.label.clone(), x.rect)).collect::<Vec<_>>()));
                    let (x, y) = ((hit.rect.x + hit.rect.w / 2.0) as i32, (hit.rect.y + hit.rect.h / 2.0) as i32);
                    logs[p].lock().unwrap().clear();
                    sh.handle(&Event::MouseMove(x, y));
                    sh.handle(&Event::MouseDown(x, y));
                    sh.handle(&Event::MouseUp(x, y));
                    sh.render(&mut g);
                    assert_eq!(sh.panes_focus_for_test(), Some(p), "click focuses {what}");
                    assert!(logs[p].lock().unwrap().iter().any(|l| l == &format!("{:?}", Event::Tab(want))), "tab {want} reached the app: {what}: {:?}", logs[p].lock().unwrap());
                    // the app now draws that tab as the lit one
                    let lit = sh.hits().iter().rev().find(|x| x.ev == Event::Tab(want) && rects[p].contains((x.rect.x + 1.0) as i32, (x.rect.y + 1.0) as i32)).unwrap().rect;
                    assert_eq!(pixel(&g, (lit.x + 2.0) as i32, (lit.y + 2.0) as i32), [200, 200, 40, 255], "tab {want} is lit: {what}");
                }
                // keyboard in the focused pane
                logs[0].lock().unwrap().clear();
                logs[1].lock().unwrap().clear();
                sh.handle(&Event::Key(Key::Tab));
                sh.handle(&Event::Key(Key::Char('2')));
                assert_eq!(logs[p].lock().unwrap().len(), 2, "keys reach the focused pane {what}");
                assert!(logs[1 - p].lock().unwrap().iter().all(|l| !l.contains("Key(")), "no key leaks to the other pane {what}");
                // a press that hits no region, a drag and the wheel carry pane-local coordinates
                let r = rects[p];
                let (px, py) = ((r.x + r.w - 20.0) as i32, (r.y + r.h - 20.0) as i32);
                sh.ws_focus(1 - p);
                sh.render(&mut g);
                logs[0].lock().unwrap().clear();
                logs[1].lock().unwrap().clear();
                sh.handle(&Event::MouseMove(px, py));
                sh.handle(&Event::MouseDown(px, py));
                sh.handle(&Event::MouseMove(px - 5, py - 5));
                sh.handle(&Event::Scroll(-3));
                sh.handle(&Event::MouseUp(px - 5, py - 5));
                let log = logs[p].lock().unwrap().clone();
                let local = |dx: i32| ((px - dx) as f32 - r.x, (py - dx) as f32 - r.y);
                let (lx, ly) = local(0);
                let near = |l: &String, name: &str, ex: f32, ey: f32| {
                    let nums: Vec<f32> = l.trim_start_matches(name).trim_matches(|c| c == '(' || c == ')').split(',').filter_map(|v| v.trim().parse().ok()).collect();
                    nums.len() == 2 && (nums[0] - ex).abs() <= 1.5 && (nums[1] - ey).abs() <= 1.5
                };
                assert!(log.iter().any(|l| l.starts_with("MouseDown") && near(l, "MouseDown", lx, ly)), "press at local {lx},{ly}: {what}: {log:?}");
                let (dx, dy) = local(5);
                assert!(log.iter().any(|l| l.starts_with("MouseUp") && near(l, "MouseUp", dx, dy)), "release local: {what}: {log:?}");
                assert!(log.iter().any(|l| l.starts_with("Scroll")), "wheel reaches the pane under the pointer: {what}: {log:?}");
                assert!(logs[1 - p].lock().unwrap().iter().all(|l| !l.starts_with("Scroll") && !l.starts_with("MouseDown")), "nothing leaks to the other pane: {what}");
            }
        }
    }
}

/// The second display takes the same route: clicks on a pane of either display reach the tab under the pointer, whichever
/// display was working before.
#[test]
fn tabs_on_the_second_display_are_clicked_and_switched_like_on_the_first() {
    use crate::display::outputs::{DispSpec, DisplayConfig};
    for ((w0, h0), (w1, h1)) in [((1280u32, 720u32), (1024u32, 600u32)), ((2560, 1600), (1920, 1080)), ((2880, 1800), (1280, 720))] {
        let specs = vec![
            DispSpec { name: "eDP-1".into(), w: w0, h: h0, x: 0, y: 0 },
            DispSpec { name: "HDMI-A-1".into(), w: w1, h: h1, x: w0 as i32, y: 0 },
        ];
        for (key, id) in TEMPLATE_KEYS {
            let (mut sh, mut g0) = session("pt3", w0, h0);
            let mut g1 = Gfx::new(w1, h1);
            sh.set_displays(&specs, &DisplayConfig::default());
            let frame = |sh: &mut Shell, g0: &mut Gfx, g1: &mut Gfx| {
                if sh.displays_active_for_test() == 0 {
                    sh.render_display(1, g1);
                    sh.render_display(0, g0);
                } else {
                    sh.render_display(0, g0);
                    sh.render_display(1, g1);
                }
            };
            frame(&mut sh, &mut g0, &mut g1);
            for d in 0..2usize {
                sh.set_pointer_display(d);
                let r = sh.display_rects_for_test(d)[0];
                let (x, y) = ((r.x + r.w / 2.0) as i32, (r.y + r.h - 8.0) as i32);
                sh.handle(&Event::MouseMove(x, y));
                sh.handle(&Event::MouseDown(x, y));
                sh.handle(&Event::MouseUp(x, y));
                assert_eq!(sh.displays_active_for_test(), d);
                sh.handle(&Event::Layout(LayoutKey::Apply(key)));
                frame(&mut sh, &mut g0, &mut g1);
                assert_eq!(sh.display_layouts_for_test()[d], id);
            }
            let mut logs: Vec<Vec<Arc<Mutex<Vec<String>>>>> = Vec::new();
            for d in 0..2usize {
                let mut per = Vec::new();
                for p in 0..2usize {
                    let log = Arc::new(Mutex::new(Vec::new()));
                    sh.display_put_for_test(d, p, Box::new(Tabbed { tab: 0, log: log.clone() }), PApp::Settings);
                    per.push(log);
                }
                logs.push(per);
            }
            frame(&mut sh, &mut g0, &mut g1);
            frame(&mut sh, &mut g0, &mut g1);
            for d in [1usize, 0, 1] {
                for p in 0..2usize {
                    for want in [2usize, 0, 1] {
                        let what = format!("{w0}x{h0}+{w1}x{h1} {id} display {d} pane {p} tab {want}");
                        frame(&mut sh, &mut g0, &mut g1);
                        let rect = sh.display_rects_for_test(d)[p];
                        // a pane smaller than the app's minimum shows a note instead of the app (see `a_pane_smaller_than_its_apps_minimum...`)
                        if rect.w < 400.0 {
                            continue;
                        }
                        let hit = sh.display_hits_for_test(d).into_iter().rev().find(|x| x.ev == Event::Tab(want) && rect.contains((x.rect.x + 1.0) as i32, (x.rect.y + 1.0) as i32)).unwrap_or_else(|| panic!("a region for {what}: rect {rect:?} hits {:?} active {}", sh.display_hits_for_test(d).iter().map(|x| (x.label.clone(), x.rect)).collect::<Vec<_>>(), sh.displays_active_for_test()));
                        let (x, y) = ((hit.rect.x + hit.rect.w / 2.0) as i32, (hit.rect.y + hit.rect.h / 2.0) as i32);
                        logs[d][p].lock().unwrap().clear();
                        sh.set_pointer_display(d);
                        sh.handle(&Event::MouseMove(x, y));
                        sh.handle(&Event::MouseDown(x, y));
                        sh.handle(&Event::MouseUp(x, y));
                        frame(&mut sh, &mut g0, &mut g1);
                        assert_eq!(sh.displays_active_for_test(), d, "{what}");
                        let all: Vec<Vec<Vec<String>>> = logs.iter().map(|v| v.iter().map(|l| l.lock().unwrap().clone()).collect()).collect();
                        assert!(all[d][p].iter().any(|l| l == &format!("{:?}", Event::Tab(want))), "{what}: all logs {all:?} focus {:?} hit {:?} click {x},{y}", sh.panes_focus_for_test(), hit.rect);
                        let lit = sh.display_hits_for_test(d).into_iter().rev().find(|x| x.ev == Event::Tab(want) && rect.contains((x.rect.x + 1.0) as i32, (x.rect.y + 1.0) as i32)).unwrap().rect;
                        let g = if d == 0 { &g0 } else { &g1 };
                        assert_eq!(pixel(g, (lit.x + 2.0) as i32, (lit.y + 2.0) as i32), [200, 200, 40, 255], "{what} is lit");
                    }
                    if sh.display_rects_for_test(d)[p].w < 400.0 {
                        continue;
                    }
                    // the keys go to the focused pane of the working display
                    logs[d][p].lock().unwrap().clear();
                    sh.handle(&Event::Key(Key::Tab));
                    assert_eq!(logs[d][p].lock().unwrap().len(), 1, "key reaches display {d} pane {p} ({id})");
                }
            }
        }
    }
}
