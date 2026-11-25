//! Settings -> Memory and Install to disk on a fake port: who may change them (administrators; the refusal is audited), the read-only window,
//! the Reread key, the restart question and its power right, the install form's answers for others and without a boot stick, and the pages at
//! several sizes. The machine itself stays the core's fake. No sleeps.

use super::system_port::FakeSys;
use super::*;
use crate::apps::tests::cx_with_admin_store;
use std::sync::atomic::Ordering;

fn app(tab: Tab) -> SettingsApp {
    let cx = cx_with_admin_store("mem-fake");
    let mut a = SettingsApp::new(&cx);
    a.go_to(tab);
    a
}

#[test]
fn memory_reads_again_for_everyone_and_changes_only_for_an_administrator() {
    let p = FakeSys::new(false);
    let mut a = app(Tab::Memory);
    a.handle_memory(&Key::Char('r'), &p);
    assert!(a.status().is_some_and(|s| s.1 && s.0.contains("read again")));
    assert!(p.audited().is_empty());
    for k in ['z', 'w', 'f'] {
        a.handle_memory(&Key::Char(k), &p);
        assert!(a.modal.is_none(), "{k}");
    }
    assert_eq!(p.audited().len(), 3);
    assert!(p.audited().iter().all(|l| l.0 == "settings.denied"));
}

#[test]
fn memory_in_a_window_changes_nothing_and_says_so() {
    let p = FakeSys::new(true);
    let mut a = app(Tab::Memory);
    a.mem.writable = false;
    a.handle_memory(&Key::Char('z'), &p);
    assert!(a.status().is_some_and(|s| !s.1 && s.0.contains("changes nothing there")));
    assert!(p.audited().is_empty());
}

#[test]
fn the_memory_page_draws_at_several_sizes_from_any_port() {
    let p = FakeSys::new(true);
    let mut a = app(Tab::Memory);
    for (w, h) in [(360u32, 740u32), (800, 480), (1280, 720), (2560, 1600)] {
        let mut g = Gfx::new(w, h);
        g.clear(rgb(0, 0, 0));
        a.render_memory(&mut g, R::new(0.0, 72.0, w as f32, h as f32 - 72.0), 1.0, &p);
    }
}

#[test]
fn install_restart_asks_first_and_only_for_an_administrator_who_may_power_off() {
    let p = FakeSys::new(true);
    let mut a = app(Tab::Install);
    a.handle_install(&Key::Char('b'), &p);
    assert!(matches!(a.modal, Some(Modal::Confirm(ConfirmKind::Reboot, _))));
    a.modal = None;
    p.power_allowed.store(false, Ordering::SeqCst);
    a.handle_install(&Key::Char('b'), &p);
    assert!(a.modal.is_none());
    assert!(a.status().is_some_and(|s| !s.1 && s.0.contains("may not restart")));
    let q = FakeSys::new(false);
    a.handle_install(&Key::Char('b'), &q);
    a.handle_install(&Key::Char('i'), &q);
    assert!(a.modal.is_none());
    assert_eq!(q.audited().iter().filter(|l| l.0 == "settings.denied").count(), 2);
}

#[test]
fn the_partition_table_key_cycles_for_everyone() {
    let p = FakeSys::new(false);
    let mut a = app(Tab::Install);
    let first = a.inst.table;
    a.handle_install(&Key::Char('t'), &p);
    assert_ne!(a.inst.table, first);
    assert!(a.status().is_some_and(|s| s.1 && s.0.contains("Partition table")));
}

#[test]
fn an_install_form_is_refused_to_others_and_without_a_source_and_audits_nothing() {
    let form = Form::new("Install").text("zram", "auto", "").text("swap", "off", "").text("name", "", "").text("drive", "", "").text("size", "", "");
    let mut a = app(Tab::Install);
    let q = FakeSys::new(false);
    assert!(a.inst_submit(&q, "vdb", &form).is_err_and(|e| e.contains("Only administrators")));
    let p = FakeSys::new(true);
    a.inst.source = None;
    assert!(a.inst_submit(&p, "vdb", &form).is_err_and(|e| e.contains("nothing to copy from")));
    assert!(p.audited().iter().all(|l| l.0 != "install.disk"));
}

#[test]
fn the_install_page_draws_at_several_sizes() {
    let mut a = app(Tab::Install);
    for (w, h) in [(360u32, 740u32), (800, 480), (1280, 720), (2560, 1600)] {
        let mut g = Gfx::new(w, h);
        g.clear(rgb(0, 0, 0));
        a.render_install(&mut g, R::new(0.0, 72.0, w as f32, h as f32 - 72.0), 1.0);
    }
}
