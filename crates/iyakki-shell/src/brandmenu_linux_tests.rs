//! The Linux apps page of the IYAKKI menu: keys, buttons and outcomes, the typed confirmation of a delete, the package-name check, the
//! layout at four screen sizes in every view, and that the page is in the menu and Quick install. Nothing is started here: the page only
//! returns the actions the shell would do (`shell_linux.rs`, tested through the shell).

use super::*;

const SIZES: [(u32, u32); 4] = [(1280, 720), (1920, 1080), (1024, 600), (800, 480)];

fn facts() -> LinuxFacts {
    LinuxFacts {
        boxes: vec![
            BoxRow { name: "dev".into(), title: "Debian 12 (bookworm)".into(), network: true, mounts: vec![("/home/admin/project".into(), "/mnt/project".into(), true), ("/home/admin/docs".into(), "/mnt/docs".into(), false)], mb: 412 },
            BoxRow { name: "web-with-a-long-name".into(), title: "Ubuntu 24.04 LTS (noble)".into(), network: false, mounts: vec![], mb: 120 },
        ],
        images: vec![
            ImageRow { id: "busybox-httpd".into(), name: "BusyBox web server (example)".into(), description: "EXAMPLE. A tiny static web server that serves one page. It shows what an image app is; it is not a recommendation.".into(), licence: "GPL-2.0-only (BusyBox)".into(), example: true, pinned: false, image: "docker.io/library/busybox@sha256:00".into(), status: String::new() },
            ImageRow { id: "pinned".into(), name: "A pinned one".into(), description: "d".into(), licence: "MIT".into(), example: false, pinned: true, image: "x@sha256:11".into(), status: "running, open http://<this machine>:31456".into() },
        ],
        problems: vec!["bad.json: no digest".into()],
        kernel_missing: vec![],
        optional_missing: vec!["the memory cgroup controller".into()],
        runtime: "Sandboxes run in Linux namespaces".into(),
        jobs_running: 0,
        allowed: true,
        any_host: true,
        cluster: true,
    }
}

fn open_page(f: LinuxFacts) -> BrandMenu {
    let mut m = BrandMenu::new();
    m.toggle();
    assert_eq!(m.open_page(Page::Linux), Outcome::Opened(Page::Linux));
    m.set_linux(f);
    m
}

#[test]
fn the_menu_has_a_linux_apps_entry_and_the_engine_still_lists_the_item() {
    let m = BrandMenu::new();
    assert!(m.shown().iter().any(|e| e.0 == Page::Linux && e.1 == "Linux apps" && e.3 == 'l'));
    let mut m = BrandMenu::new();
    m.toggle();
    assert_eq!(m.handle_key(&Key::Char('l')), Outcome::Opened(Page::Linux));
    assert_eq!(m.page, Page::Linux);
    assert_eq!(m.handle_key(&Key::Esc), Outcome::Nothing, "back to the first page");
    assert_eq!(m.page, Page::Menu);
    // Quick install lists the entry under its own heading
    use iyakki_agent::quickinstall::{self, Group, Item};
    assert!(quickinstall::items_of(Group::Linux).contains(&Item::LinuxApps));
    // (the Linux apps card in Store -> Setup is tested with the Setup tab)
}

#[test]
fn a_package_name_that_is_not_one_stops_before_a_job_and_valid_ones_become_the_action() {
    let mut m = open_page(facts());
    // nothing typed: the field is focused and the page explains
    assert_eq!(m.handle_hit(&Event::Tab(L_INSTALL)), Outcome::Nothing);
    assert!(m.note.as_ref().unwrap().0.contains("Type the packages"));
    m.linux.pkgs.set("curl --allow-unauthenticated");
    assert_eq!(m.handle_hit(&Event::Tab(L_INSTALL)), Outcome::Nothing);
    assert!(m.note.as_ref().unwrap().0.contains("not a package name"), "{:?}", m.note);
    m.linux.pkgs.set("curl, git  python3");
    assert_eq!(m.handle_hit(&Event::Tab(L_INSTALL)), Outcome::Linux(LinuxAction::Install { name: "dev".into(), packages: vec!["curl".into(), "git".into(), "python3".into()] }));
    assert_eq!(m.handle_hit(&Event::Tab(L_REMOVE)), Outcome::Linux(LinuxAction::Remove { name: "dev".into(), packages: vec!["curl".into(), "git".into(), "python3".into()] }));
    // the second sandbox is the one chosen with the arrow keys (Esc lets go of the field first)
    m.handle_key(&Key::Esc);
    m.handle_key(&Key::Down);
    assert_eq!(m.handle_hit(&Event::Tab(L_INSTALL)), Outcome::Linux(LinuxAction::Install { name: "web-with-a-long-name".into(), packages: vec!["curl".into(), "git".into(), "python3".into()] }));
}

#[test]
fn network_switch_terminal_upgrade_and_mounts_return_their_actions() {
    let mut m = open_page(facts());
    assert_eq!(m.handle_key(&Key::Char('t')), Outcome::Linux(LinuxAction::Terminal { name: "dev".into() }));
    assert_eq!(m.handle_key(&Key::Enter), Outcome::Linux(LinuxAction::Terminal { name: "dev".into() }));
    assert_eq!(m.handle_key(&Key::Char('n')), Outcome::Linux(LinuxAction::SetNetwork { name: "dev".into(), on: false }), "the network was on: the switch turns it off");
    m.handle_key(&Key::Down);
    assert_eq!(m.handle_hit(&Event::Tab(L_NET)), Outcome::Linux(LinuxAction::SetNetwork { name: "web-with-a-long-name".into(), on: true }));
    assert_eq!(m.handle_key(&Key::Char('u')), Outcome::Linux(LinuxAction::Upgrade { name: "web-with-a-long-name".into() }));
    // a folder is asked for first; read-only and read-write are different buttons
    assert_eq!(m.handle_hit(&Event::Tab(L_MOUNT_RO)), Outcome::Nothing);
    assert!(m.note.as_ref().unwrap().0.contains("full path"));
    m.linux.folder.set("/home/admin/work");
    assert_eq!(m.handle_hit(&Event::Tab(L_MOUNT_RO)), Outcome::Linux(LinuxAction::Mount { name: "web-with-a-long-name".into(), folder: "/home/admin/work".into(), rw: false }));
    assert_eq!(m.handle_hit(&Event::Tab(L_MOUNT_RW)), Outcome::Linux(LinuxAction::Mount { name: "web-with-a-long-name".into(), folder: "/home/admin/work".into(), rw: true }));
    assert_eq!(m.handle_hit(&Event::Tab(L_UNMOUNT)), Outcome::Linux(LinuxAction::Unmount { name: "web-with-a-long-name".into(), folder: "/home/admin/work".into() }));
    m.handle_key(&Key::Esc);
    assert_eq!(m.handle_key(&Key::Char('r')), Outcome::Linux(LinuxAction::Refresh));
    // an account that may not reach every host cannot turn a network on
    let mut f = facts();
    f.any_host = false;
    let mut m = open_page(f);
    m.handle_key(&Key::Down);
    assert_eq!(m.handle_key(&Key::Char('n')), Outcome::Nothing);
    assert!(m.note.as_ref().unwrap().0.contains("every host"));
    // and one without the full command line cannot do anything
    let mut f = facts();
    f.allowed = false;
    let mut m = open_page(f);
    assert_eq!(m.handle_key(&Key::Char('t')), Outcome::Nothing);
    assert!(m.note.as_ref().unwrap().0.contains("full command-line permission"));
    assert_eq!(m.handle_hit(&Event::Tab(L_INSTALL)), Outcome::Nothing);
}

#[test]
fn deleting_asks_for_the_name_and_only_the_exact_name_deletes() {
    let mut m = open_page(facts());
    assert_eq!(m.handle_key(&Key::Char('d')), Outcome::Nothing);
    assert!(m.note.as_ref().unwrap().0.contains("Type its name"));
    // the confirm field has the keyboard: letters go into it, not to the page's shortcuts
    for c in "de".chars() {
        assert_eq!(m.handle_key(&Key::Char(c)), Outcome::Nothing);
    }
    assert_eq!(m.linux.confirm.value, "de");
    assert_eq!(m.handle_key(&Key::Enter), Outcome::Nothing, "a part of the name is not enough");
    assert!(m.note.as_ref().unwrap().0.contains("exactly"));
    m.handle_key(&Key::Char('x'));
    assert_eq!(m.handle_hit(&Event::Tab(L_DELETE_YES)), Outcome::Nothing, "dex is not dev");
    m.linux.confirm.set("dev");
    assert_eq!(m.handle_key(&Key::Enter), Outcome::Linux(LinuxAction::Delete { name: "dev".into(), typed: "dev".into() }));
    // Cancel forgets it; choosing another sandbox does too
    m.handle_key(&Key::Char('d'));
    assert_eq!(m.handle_hit(&Event::Tab(L_DELETE_NO)), Outcome::Nothing);
    assert!(m.linux.confirm_for.is_none());
    m.handle_key(&Key::Char('d'));
    m.handle_hit(&Event::Tab(L_ROW + 1));
    assert!(m.linux.confirm_for.is_none() && m.sel() == 1, "choosing another sandbox drops the question");
    let mut g = Gfx::new(1280, 720);
    m.render(&mut g, 1280, 720, 1.0, 0);
    assert!(!g.hits.iter().any(|h| h.ev == Event::Tab(L_DELETE_YES)), "no delete button without a question");
}

#[test]
fn creating_a_sandbox_needs_a_good_name_and_a_network_decision() {
    let mut m = open_page(facts());
    m.handle_key(&Key::Char('2'));
    assert_eq!(m.linux.view, LView::New);
    assert_eq!(m.handle_hit(&Event::Tab(L_CREATE)), Outcome::Nothing);
    assert!(m.note.as_ref().unwrap().0.contains("name"), "{:?}", m.note);
    m.linux.name.set("Bad Name");
    assert_eq!(m.handle_key(&Key::Enter), Outcome::Nothing);
    m.linux.name.set("work");
    m.handle_hit(&Event::Tab(L_REL + 3));
    assert_eq!(m.handle_hit(&Event::Tab(L_CREATE)), Outcome::Linux(LinuxAction::Create { name: "work".into(), release: "ubuntu-24.04".into(), network: true, folder: None }));
    m.handle_hit(&Event::Tab(L_NEWNET));
    m.linux.folder.set("/home/admin/code");
    m.handle_hit(&Event::Tab(L_NEWRW));
    m.handle_key(&Key::Esc);
    assert_eq!(m.handle_key(&Key::Char('c')), Outcome::Linux(LinuxAction::Create { name: "work".into(), release: "ubuntu-24.04".into(), network: false, folder: Some(("/home/admin/code".into(), true)) }));
    // an account that cannot reach every host cannot make a sandbox with its network on
    let mut f = facts();
    f.any_host = false;
    let mut m = open_page(f);
    m.handle_key(&Key::Char('2'));
    m.linux.name.set("work");
    assert_eq!(m.handle_hit(&Event::Tab(L_CREATE)), Outcome::Nothing);
    assert!(m.note.as_ref().unwrap().0.contains("every host"));
    m.handle_hit(&Event::Tab(L_NEWNET));
    assert!(matches!(m.handle_hit(&Event::Tab(L_CREATE)), Outcome::Linux(LinuxAction::Create { network: false, .. })));
}

#[test]
fn image_apps_run_only_when_pinned_and_stop_any_time() {
    let mut m = open_page(facts());
    m.handle_key(&Key::Char('3'));
    assert_eq!(m.linux.view, LView::Images);
    assert_eq!(m.handle_hit(&Event::Tab(L_RUN)), Outcome::Nothing);
    assert!(m.note.as_ref().unwrap().0.contains("placeholder"), "{:?}", m.note);
    assert_eq!(m.handle_hit(&Event::Tab(L_STOP)), Outcome::Linux(LinuxAction::StopImage { id: "busybox-httpd".into() }));
    m.handle_key(&Key::Down);
    assert_eq!(m.handle_key(&Key::Char('a')), Outcome::Linux(LinuxAction::RunImage { id: "pinned".into() }));
    assert_eq!(m.handle_key(&Key::Enter), Outcome::Linux(LinuxAction::RunImage { id: "pinned".into() }));
    assert_eq!(m.handle_key(&Key::Char('x')), Outcome::Linux(LinuxAction::StopImage { id: "pinned".into() }));
}

#[test]
fn every_view_fits_the_panel_at_four_sizes_with_buttons_inside_and_big_enough() {
    for (w, h) in SIZES {
        let s = crate::shell::scale_for(w, h);
        for view in ['1', '2', '3'] {
            for (case, f) in [("full", facts()), ("empty", LinuxFacts { allowed: true, any_host: true, runtime: "Sandboxes run in Linux namespaces".into(), ..Default::default() }), ("no kernel support", LinuxFacts { kernel_missing: vec!["CONFIG_USER_NS".into(), "CONFIG_PID_NS".into(), "CONFIG_NET_NS".into()], allowed: true, ..facts() }), ("not allowed", LinuxFacts { allowed: false, ..facts() })] {
                let mut m = BrandMenu::new();
                m.toggle();
                m.anchor = (40.0 * s, 76.0 * s);
                m.open_page(Page::Linux);
                m.set_linux(f);
                m.handle_key(&Key::Char(view));
                // a delete asked, to see the other button set too
                if view == '1' && case == "full" {
                    m.handle_key(&Key::Char('d'));
                }
                let mut g = Gfx::new(w, h);
                m.render(&mut g, w, h, s, 0);
                let p = m.panel(w, h, s);
                assert!(p.x >= 0.0 && p.y >= 0.0 && p.x + p.w <= w as f32 + 0.5 && p.y + p.h <= h as f32 + 0.5, "{w}x{h}: {p:?}");
                for hit in &g.hits {
                    let r = hit.rect;
                    assert!(r.x >= p.x - 0.5 && r.y >= p.y - 0.5 && r.x + r.w <= p.x + p.w + 0.5 && r.y + r.h <= p.y + p.h + 0.5, "{w}x{h} view {view} {case}: {} {r:?} outside {p:?}", hit.label);
                }
                // the three view buttons and Refresh are always there
                for i in 0..3 {
                    assert!(g.hits.iter().any(|x| x.ev == Event::Tab(L_TAB + i) && x.rect.h >= 24.0 * s), "{w}x{h} {case}: view button {i}");
                }
                if case == "full" {
                    let want: &[usize] = match view {
                        '1' => &[L_DELETE_YES, L_DELETE_NO, L_CONFIRM],
                        '2' => &[L_REL, L_CREATE, L_NEWNET, L_NAME, L_FOLDER],
                        _ => &[L_RUN, L_STOP, L_IMG],
                    };
                    for id in want {
                        assert!(g.hits.iter().any(|x| x.ev == Event::Tab(*id) || x.ev == Event::Focus(*id)), "{w}x{h} view {view}: control {id}");
                    }
                }
                // nothing overlaps the foot line of the panel
                let foot_y = p.y + p.h - 28.0 * s;
                assert!(g.hits.iter().all(|x| x.rect.y + x.rect.h <= foot_y + 2.0 * s || x.ev == Event::Tab(CLOSE) || x.ev == Event::Tab(BACK)), "{w}x{h} view {view} {case}: a control reaches into the foot");
            }
        }
    }
}

#[test]
fn many_sandboxes_scroll_with_the_selection() {
    let mut f = facts();
    f.boxes = (0..25).map(|i| BoxRow { name: format!("box{i}"), title: "Debian 12".into(), network: false, mounts: vec![], mb: 100 }).collect();
    let mut m = open_page(f);
    let (w, h) = (800, 480);
    let s = crate::shell::scale_for(w, h);
    for _ in 0..20 {
        m.handle_key(&Key::Down);
    }
    assert_eq!(m.sel(), 20);
    let mut g = Gfx::new(w, h);
    m.render(&mut g, w, h, s, 0);
    assert!(g.hits.iter().any(|x| x.ev == Event::Tab(L_ROW + 20)), "the chosen row is on screen");
    assert!(!g.hits.iter().any(|x| x.ev == Event::Tab(L_ROW)), "the first rows scrolled off");
    m.handle_key(&Key::End);
    assert_eq!(m.sel(), 24);
}

#[test]
fn the_wheel_and_the_trackpad_move_through_the_sandbox_list_like_the_arrows_but_not_while_typing() {
    let mut m = open_page(facts());
    assert_eq!(m.sel(), 0);
    // a notch down (the wheel and the trackpad's two fingers both arrive as Event::Scroll) is a row down
    assert_eq!(m.handle_hit(&Event::Scroll(-1)), Outcome::Nothing);
    assert_eq!(m.sel(), 1);
    m.handle_hit(&Event::Scroll(1));
    assert_eq!(m.sel(), 0, "and a notch up goes back");
    // while a field has the keyboard the wheel does not touch the list
    m.handle_hit(&Event::Scroll(-1));
    assert_eq!(m.handle_hit(&Event::Tab(L_INSTALL)), Outcome::Nothing, "nothing typed: the packages field takes the keyboard");
    let before = m.sel();
    m.handle_hit(&Event::Scroll(-1));
    assert_eq!(m.sel(), before);
}
