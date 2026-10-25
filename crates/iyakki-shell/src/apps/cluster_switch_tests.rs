// #[cfg(test)] -- test code, included by cluster_switch.rs under #[cfg(test)].
//! The cluster switcher: hidden with one cluster, drawn at every size with every hit region inside its area (and inside a pane),
//! rows with status dots, kind and GPU badge, "Manage clusters...", keys and clicks, role filtering.

use super::*;
use crate::gfx::Gfx;
use iyakki_agent::clusters::{Dot, Kind, Registry, Status};

fn registry() -> Registry {
    let mut r = Registry::default();
    r.add_remote("gpu-box", Kind::Remote, "uvault:gpu-kc", "", "").unwrap();
    r.add_local("Second").unwrap();
    r.add_remote("tunnelled", Kind::Tunnel, "/k.yaml", "", "").unwrap();
    r.set_visible_to_users("tunnelled", false).unwrap();
    r
}

fn status(e: &ClusterEntry) -> Status {
    let mut s = Status::unknown();
    s.dot = match e.id.as_str() {
        "local" => Dot::Running,
        "gpu-box" => Dot::Unreachable,
        "second" => Dot::Stopped,
        _ => Dot::Stale,
    };
    if e.id == "gpu-box" {
        s.gpu = Some("24 GB".into());
    }
    s
}

#[test]
fn one_cluster_means_no_switcher_and_nothing_clickable() {
    let r = Registry::default();
    let rows = rows_for(&r, Role::Admin, "w", &status);
    assert_eq!(rows.len(), 1);
    assert!(!ClusterSwitch::visible(&rows));
    let mut g = Gfx::new(800, 480);
    let mut sw = ClusterSwitch::new(9100);
    assert!(sw.draw(&mut g, 10.0, 10.0, 300.0, 1.0, &rows, [64, 196, 232, 255]).is_none());
    assert!(g.hits.is_empty(), "no clutter: not even a hit region");
    assert_eq!(ClusterSwitch::width(&mut g, 1.0, &rows), 0.0);
    assert!(sw.draw_popup(&mut g, R::new(0.0, 0.0, 800.0, 480.0), 1.0, &rows, [64, 196, 232, 255]).is_none());
    assert_eq!(sw.handle(&Event::Tab(9100), &rows), SwitchOut::None);
    assert!(!sw.open, "cannot be opened with nothing to choose");
}

#[test]
fn rows_follow_the_role_the_active_cluster_and_the_workspace_override() {
    let mut r = registry();
    assert_eq!(rows_for(&r, Role::Guest, "w", &status).len(), 0);
    let user = rows_for(&r, Role::User, "w", &status);
    assert_eq!(user.iter().map(|x| x.id.as_str()).collect::<Vec<_>>(), ["local", "gpu-box", "second"]);
    let admin = rows_for(&r, Role::Admin, "w", &status);
    assert_eq!(admin.len(), 4);
    assert_eq!(admin.iter().filter(|x| x.active).map(|x| x.id.as_str()).collect::<Vec<_>>(), ["local"]);
    r.set_active("gpu-box").unwrap();
    r.set_workspace("web", Some("second")).unwrap();
    let web = rows_for(&r, Role::Admin, "web", &status);
    assert_eq!(web.iter().find(|x| x.active).unwrap().id, "second");
    let api = rows_for(&r, Role::Admin, "api", &status);
    assert_eq!(api.iter().find(|x| x.active).unwrap().id, "gpu-box");
    assert_eq!(api[1].gpu.as_deref(), Some("24 GB"));
    assert_eq!(api[1].describe(), "gpu-box (remote, unreachable)");
    assert_eq!(ClusterSwitch::label(&api), "gpu-box");
}

fn assert_inside(g: &Gfx, area: R, what: &str) {
    assert!(!g.hits.is_empty(), "{what}: nothing is clickable");
    for h in &g.hits {
        let r = h.rect;
        assert!(r.x >= area.x - 0.5 && r.y >= area.y - 0.5 && r.x + r.w <= area.x + area.w + 0.5 && r.y + r.h <= area.y + area.h + 0.5, "{what}: `{}` {:?} is outside {:?}", h.label, r, area);
        assert!(r.w > 0.0 && r.h > 0.0, "{what}: `{}` is empty", h.label);
    }
}

#[test]
fn it_draws_at_every_size_closed_and_open_with_every_hit_region_inside_the_area() {
    let rows = rows_for(&registry(), Role::Admin, "w", &status);
    for (w, h, s) in [(360u32, 740u32, 1.0f32), (800, 480, 1.0), (1280, 720, 1.0), (1920, 1080, 1.5), (2560, 1600, 2.0)] {
        let area = R::new(0.0, 0.0, w as f32, h as f32);
        for (ax, ay) in [(8.0f32, 8.0f32), (w as f32 - 90.0, 8.0), (8.0, h as f32 - 60.0 * s)] {
            let mut g = Gfx::new(w, h);
            let mut sw = ClusterSwitch::new(9100);
            let r = sw.draw(&mut g, ax * s.min(1.0), ay, w as f32 - ax.min(w as f32 - 60.0) - 4.0, s, &rows, [64, 196, 232, 255]).expect("shown");
            assert!(r.w > 0.0 && r.h > 0.0);
            assert_inside(&g, area, &format!("{w}x{h} closed at {ax},{ay}"));
            sw.handle(&Event::Tab(9100), &rows);
            assert!(sw.open);
            let mut g = Gfx::new(w, h);
            sw.draw(&mut g, ax * s.min(1.0), ay, w as f32 - ax.min(w as f32 - 60.0) - 4.0, s, &rows, [64, 196, 232, 255]);
            let pop = sw.draw_popup(&mut g, area, s, &rows, [64, 196, 232, 255]).expect("open");
            assert!(pop.x >= -0.5 && pop.y >= -0.5 && pop.x + pop.w <= w as f32 + 0.5 && pop.y + pop.h <= h as f32 + 0.5, "{w}x{h}: popup {pop:?}");
            assert_inside(&g, area, &format!("{w}x{h} open at {ax},{ay}"));
            assert!(g.hits.iter().any(|x| x.label == "Manage clusters..."), "{w}x{h}: the manage row is reachable");
            assert!(g.hits.iter().any(|x| x.label.contains("gpu-box")));
            assert!(g.rgba().iter().any(|b| *b != 0));
        }
    }
}

#[test]
fn inside_a_pane_every_hit_stays_inside_the_pane() {
    let rows = rows_for(&registry(), Role::Admin, "w", &status);
    for pane in [R::new(400.0, 100.0, 380.0, 300.0), R::new(0.0, 0.0, 300.0, 200.0), R::new(500.0, 300.0, 280.0, 170.0)] {
        let mut g = Gfx::new(800, 480);
        let mut sw = ClusterSwitch::new(9100);
        sw.open = true;
        let r = sw.draw(&mut g, pane.x + 6.0, pane.y + 6.0, pane.w - 12.0, 1.0, &rows, [64, 196, 232, 255]).unwrap();
        assert!(r.x + r.w <= pane.x + pane.w + 0.5, "the box fits the pane");
        sw.draw_popup(&mut g, pane, 1.0, &rows, [64, 196, 232, 255]).unwrap();
        // the box itself is registered before the popup: only the popup's close region covers the pane
        for h in &g.hits {
            let b = h.rect;
            assert!(b.x >= pane.x - 0.5 && b.y >= pane.y - 0.5 && b.x + b.w <= pane.x + pane.w + 0.5 && b.y + b.h <= pane.y + pane.h + 0.5, "`{}` {:?} outside pane {:?}", h.label, b, pane);
        }
    }
}

#[test]
fn a_tiny_area_never_panics() {
    let rows = rows_for(&registry(), Role::Admin, "w", &status);
    for (w, h) in [(1u32, 1u32), (40, 30), (120, 60)] {
        let mut g = Gfx::new(w, h);
        let mut sw = ClusterSwitch::new(9100);
        sw.open = true;
        sw.draw(&mut g, 0.0, 0.0, w as f32, 1.0, &rows, [1, 2, 3, 255]);
        sw.draw_popup(&mut g, R::new(0.0, 0.0, w as f32, h as f32), 1.0, &rows, [1, 2, 3, 255]);
    }
}

#[test]
fn clicks_and_keys_choose_a_cluster_or_open_manage() {
    let rows = rows_for(&registry(), Role::Admin, "w", &status);
    let mut sw = ClusterSwitch::new(9100);
    sw.handle(&Event::Tab(9100), &rows);
    assert!(sw.open && sw.owns_keys());
    assert_eq!(sw.handle(&Event::Tab(9110 + 1), &rows), SwitchOut::Switch("gpu-box".into()));
    assert!(!sw.open);
    sw.handle(&Event::Tab(9100), &rows);
    assert_eq!(sw.handle(&Event::Tab(9102), &rows), SwitchOut::Manage);
    sw.handle(&Event::Tab(9100), &rows);
    sw.handle(&Event::Key(Key::Down), &rows);
    sw.handle(&Event::Key(Key::Down), &rows);
    assert_eq!(sw.handle(&Event::Key(Key::Enter), &rows), SwitchOut::Switch("second".into()));
    // Up from the first row wraps to "Manage clusters..."
    sw.handle(&Event::Tab(9100), &rows);
    sw.handle(&Event::Key(Key::Up), &rows);
    assert_eq!(sw.handle(&Event::Key(Key::Up), &rows), SwitchOut::None);
    sw.handle(&Event::Key(Key::Esc), &rows);
    sw.handle(&Event::Tab(9100), &rows);
    for _ in 0..rows.len() {
        sw.handle(&Event::Key(Key::Down), &rows);
    }
    assert_eq!(sw.handle(&Event::Key(Key::Enter), &rows), SwitchOut::Manage);
    // Esc and a click outside close it; keys do nothing while closed
    sw.handle(&Event::Tab(9100), &rows);
    sw.handle(&Event::Key(Key::Esc), &rows);
    assert!(!sw.open);
    assert_eq!(sw.handle(&Event::Key(Key::Enter), &rows), SwitchOut::None);
    sw.handle(&Event::Tab(9100), &rows);
    sw.handle(&Event::Tab(9101), &rows);
    assert!(!sw.open);
    // codes of another host are not ours
    assert_eq!(sw.handle(&Event::Tab(5), &rows), SwitchOut::None);
}

#[test]
fn dot_colours_differ_for_every_state() {
    let ds = [Dot::Running, Dot::Stopped, Dot::Unreachable, Dot::Stale, Dot::Unknown];
    for (i, a) in ds.iter().enumerate() {
        for b in ds.iter().skip(i + 1) {
            assert_ne!(dot_color(*a), dot_color(*b), "{a:?} and {b:?} look the same");
        }
    }
}
