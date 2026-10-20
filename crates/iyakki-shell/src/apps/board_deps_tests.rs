//! Tests of the board's dependency and agent layers: the chain on cards, the Depends and Agent columns, the Graph view, the Agents strip,
//! the filter and the agent panel, at the four sizes people use. A child of the board's module (it uses its private state).

use iyakki_agent::assign::{self, AState, Choice, StepKind};
use iyakki_agent::tasks::AutoAssign;

use super::deps::{self, AgentFilter};
use super::*;

const DEPS: &str = "# T\n\n<!-- summary:start -->done 1 · partial 1 · planned 3 · blocked 1 · deprecated 0 · total 6<!-- summary:end -->\n\n| ID | Feature | Phase | Status | Pri | Progress | Tests | Depends | Notes |\n|---|---|---|---|---|---|---|---|---|\n| F-A-01 | Database schema | P1 | done | P0 | 100% | `c::t::schema` |  |  |\n| F-A-02 | Server skeleton | P1 | partial | P0 | 40% | `c::t::server` | F-A-01 | wiring routes |\n| F-A-03 | Client app | P2 | planned | P1 | 0% | — | F-A-02, F-A-01 |  |\n| F-A-04 | Readme | P2 | planned | P2 | 0% | — |  |  |\n| F-A-05 | Payment provider | P3 | blocked | P1 | 0% | — |  | no account yet |\n| F-A-06 | Checkout page | P3 | planned | P1 | 0% | — | F-A-05 |  |\n";

fn world_with_deps(tag: &str) -> (BoardView, PathBuf) {
    let (root, f) = super::tests::world(tag);
    std::fs::write(&f, DEPS).unwrap();
    let mut v = BoardView::new(&root, &f);
    v.autosave_after = Duration::ZERO;
    v.ext.poll_every = Duration::ZERO;
    (v, root)
}

fn agents(root: &Path) {
    assign::update(root, |d| {
        d.register("t1", "task", "Claude Code (sonnet)", &["F-A-02".to_string()], "t1", 1000);
        d.register("t2", "task", "built-in (qwen)", &["F-A-04".to_string()], "t2", 1000);
        d.set_state("t2", AState::WaitingApproval, "write_file x", 1010);
        d.step("t1", StepKind::Plan, "autopilot: step 1 · row 1 of 1 · F-A-02", 1001);
        d.step("t1", StepKind::Tool, "run_command cargo build -> ok", 1002);
        d.step("t1", StepKind::Test, "cargo test -> test result: ok. 4 passed", 1003);
        d.step("t1", StepKind::Board, "board_update F-A-02: updated", 1004);
        d.touch_file("t1", "src/server.rs", 12, 3, 1005);
        d.touch_file("t2", "src/server.rs", 1, 0, 1006);
        d.note_added("t1", "N-001", "Routes live in server.rs", 1007);
        d.set_usage("t1", 12_300, 24.0);
    });
}

fn draw_at(v: &mut BoardView, w: f32, h: f32, s: f32) -> Gfx {
    let mut g = Gfx::new(w as u32, h as u32);
    g.clear(rgb(8, 11, 26));
    v.draw(&mut g, R::new(0.0, 0.0, w, h), s, true, CYAN, 7);
    g
}

fn labels(g: &Gfx) -> Vec<String> {
    g.hits.iter().map(|h| h.label.clone()).collect()
}

fn click(v: &mut BoardView, g: &Gfx, label: &str) -> BoardAct {
    let h = g.hits.iter().rev().find(|h| h.label == label).unwrap_or_else(|| panic!("no control {label:?} in {:?}", labels(g)));
    v.handle(&h.ev.clone())
}

const SIZES: [(f32, f32, f32); 4] = [(1280.0, 720.0, 1.0), (1920.0, 1080.0, 1.5), (1024.0, 600.0, 0.8), (800.0, 480.0, 0.62)];

fn inside_and_apart(g: &Gfx, w: f32, h: f32, tag: &str, bad: &mut Vec<String>) {
    let hits: Vec<_> = g.hits.iter().filter(|x| x.rect.w > 0.0 && x.rect.h > 0.0).collect();
    for x in &hits {
        let r = x.rect;
        if r.x < -0.5 || r.y < -0.5 || r.x + r.w > w + 0.5 || r.y + r.h > h + 0.5 {
            bad.push(format!("{tag}: {:?} leaves the screen ({:?})", x.label, r));
        }
    }
    // small controls of the strip, the panel and the graph do not overlap each other
    let ctl: Vec<_> = hits.iter().filter(|x| matches!(x.ev, Event::Tab(n) if n >= TAB_BASE + deps::BASE + deps::AGENT_CHIP && n < TAB_BASE + deps::BASE + 6000) && x.rect.w * x.rect.h < w * h * 0.06).collect();
    for (i, a) in ctl.iter().enumerate() {
        for b in &ctl[i + 1..] {
            let (ox, oy) = ((a.rect.x + a.rect.w).min(b.rect.x + b.rect.w) - a.rect.x.max(b.rect.x), (a.rect.y + a.rect.h).min(b.rect.y + b.rect.h) - a.rect.y.max(b.rect.y));
            if ox > 1.5 && oy > 1.5 && !graph_pair(&a.label, &b.label) {
                bad.push(format!("{tag}: {:?} {:?} overlaps {:?} {:?}", a.label, a.rect, b.label, b.rect));
            }
        }
    }
}

/// Boxes of the graph may touch when it is zoomed far out; everything else may not overlap.
fn graph_pair(a: &str, b: &str) -> bool {
    a.starts_with("graph node") && b.starts_with("graph node")
}

#[test]
fn cards_say_what_they_wait_for_what_they_unblock_and_who_works_on_them() {
    let (mut v, root) = world_with_deps("cards");
    v.refresh_deps(true);
    let f = |v: &BoardView, id: &str| v.file.as_ref().unwrap().board.get(id).unwrap();
    assert_eq!(v.dep_line(&f(&v, "F-A-01")).unwrap().0, "unblocks 2");
    assert_eq!(v.dep_line(&f(&v, "F-A-02")).unwrap().0, "ready · unblocks 1");
    assert_eq!(v.dep_line(&f(&v, "F-A-03")).unwrap().0, "waits: F-A-02");
    assert!(v.dep_line(&f(&v, "F-A-04")).is_none(), "no dependencies, nothing waits for it: no line");
    assert_eq!(v.dep_line(&f(&v, "F-A-06")).unwrap().0, "blocked by: F-A-05");
    assert_eq!(v.dep_line(&f(&v, "F-A-06")).unwrap().1, ERR);
    let before = v.card_height_px(&mut Gfx::new(400, 400), &f(&v, "F-A-03"), 220.0, 1.0);
    agents(&root);
    v.refresh_deps(true);
    assert!(v.card_height_px(&mut Gfx::new(400, 400), &f(&v, "F-A-02"), 220.0, 1.0) > before - 40.0);
    assert_eq!(v.agent_cell(&f(&v, "F-A-02")), "Agent 1 · working");
    assert_eq!(v.agent_cell(&f(&v, "F-A-04")), "Agent 2 · waiting for approval");
    assert_eq!(v.agent_cell(&f(&v, "F-A-03")), "");
    // the lanes draw: the chain mark and the assignee take room in the card but nothing leaves the lane
    for (w, h, s) in SIZES {
        let g = draw_at(&mut v, w, h, s);
        assert!(g.hits.iter().any(|x| x.label == "F-A-02"), "{w}x{h}");
    }
}

#[test]
fn the_table_has_depends_and_agent_columns_only_when_they_mean_something() {
    let (root, f) = super::tests::world("tablecols");
    let mut plain = BoardView::new(&root, &f);
    plain.refresh_deps(true);
    let names: Vec<&str> = plain.table_cols().iter().map(|c| c.0).collect();
    assert!(!names.contains(&"Depends") && !names.contains(&"Agent"), "an old board is unchanged: {names:?}");
    let (mut v, root) = world_with_deps("tablecols2");
    v.refresh_deps(true);
    let names: Vec<&str> = v.table_cols().iter().map(|c| c.0).collect();
    assert!(names.contains(&"Depends") && !names.contains(&"Agent"), "{names:?}");
    agents(&root);
    v.refresh_deps(true);
    let names: Vec<&str> = v.table_cols().iter().map(|c| c.0).collect();
    assert!(names.contains(&"Depends") && names.contains(&"Agent"), "{names:?}");
    v.mode = Mode::Table;
    let i = names.iter().position(|n| *n == "Agent").unwrap();
    v.sort = (i, true);
    let order: Vec<String> = v.filtered().into_iter().map(|f| f.id).collect();
    assert_eq!(order.last().map(String::as_str), Some("F-A-04"), "sorted by agent: unassigned first, then Agent 1, then Agent 2: {order:?}");
    for (w, h, s) in SIZES {
        draw_at(&mut v, w, h, s);
    }
}

#[test]
fn the_graph_view_lays_rows_out_in_layers_with_arrows_the_critical_path_and_a_click_selects() {
    let (mut v, _root) = world_with_deps("graph");
    v.mode = Mode::Graph;
    assert_eq!(Mode::ALL.map(|m| m.label()), ["Board", "Table", "Phases", "Graph"]);
    let g = draw_at(&mut v, 1280.0, 720.0, 1.0);
    // the connected rows are drawn: 01, 02, 03 (and 05, 06); the free row 04 is not
    let nodes: Vec<String> = g.hits.iter().filter(|h| h.label.starts_with("graph node")).map(|h| h.label.clone()).collect();
    assert_eq!(nodes.len(), 5, "{nodes:?}");
    assert!(!nodes.contains(&"graph node F-A-04".to_string()));
    // layers run left to right: a row is right of what it waits for
    let x = |id: &str| g.hits.iter().find(|h| h.label == format!("graph node {id}")).unwrap().rect.x;
    assert!(x("F-A-01") < x("F-A-02") && x("F-A-02") < x("F-A-03") && x("F-A-05") < x("F-A-06"), "layered by depth");
    // a click on a node selects its row
    let a = click(&mut v, &g, "graph node F-A-03");
    assert_eq!(a, BoardAct::Used);
    assert_eq!(v.selected().unwrap().id, "F-A-03");
    // keys move along the edges: left to what it waits for, right to what waits for it
    v.handle(&Event::Key(Key::Left));
    assert_eq!(v.selected().unwrap().id, "F-A-02");
    v.handle(&Event::Key(Key::Left));
    assert_eq!(v.selected().unwrap().id, "F-A-01");
    v.handle(&Event::Key(Key::Right));
    assert_eq!(v.selected().unwrap().id, "F-A-02");
    // zoom, pan and fit by key and by wheel
    assert!(v.ext.fit);
    v.handle(&Event::Key(Key::Char('+')));
    let z = v.ext.zoom;
    assert!(!v.ext.fit && z > 0.0);
    v.handle(&Event::Key(Key::Char('+')));
    assert!(v.ext.zoom > z);
    v.handle(&Event::Key(Key::Char('-')));
    v.handle(&Event::Scroll(1));
    let zoomed = v.ext.zoom;
    v.handle(&Event::Scroll(-1));
    assert!(v.ext.zoom < zoomed, "the wheel zooms");
    v.handle(&Event::Key(Key::Nav { nav: Nav::Left, ctrl: true, shift: false }));
    assert!(v.ext.pan.0 > 0.0, "{:?}", v.ext.pan);
    v.handle(&Event::Key(Key::Nav { nav: Nav::Down, ctrl: true, shift: false }));
    assert!(v.ext.pan.1 < 0.0);
    draw_at(&mut v, 1280.0, 720.0, 1.0);
    v.handle(&Event::Key(Key::Char('0')));
    assert!(v.ext.fit && v.ext.pan == (0.0, 0.0));
    // the critical path is the longest unfinished chain; t turns its highlight off
    assert_eq!(v.ext.graph.critical_path(), ["F-A-02", "F-A-03"]);
    assert!(v.ext.critical);
    v.handle(&Event::Key(Key::Char('t')));
    assert!(!v.ext.critical);
    // Tab walks Board, Table, Phases, Graph and back
    v.mode = Mode::Phases;
    v.handle(&Event::Key(Key::Tab));
    assert_eq!(v.mode, Mode::Graph);
    v.handle(&Event::Key(Key::Tab));
    assert_eq!(v.mode, Mode::Kanban);
    // the view tab is clickable
    let g = draw_at(&mut v, 1280.0, 720.0, 1.0);
    click(&mut v, &g, "Graph");
    assert_eq!(v.mode, Mode::Graph);
}

#[test]
fn the_graph_fits_and_stays_inside_its_pane_at_the_four_sizes_and_says_so_when_there_are_no_dependencies() {
    let (mut v, root) = world_with_deps("graphfit");
    v.mode = Mode::Graph;
    agents(&root);
    let mut bad = Vec::new();
    for (w, h, s) in SIZES {
        v.ext.fit = true;
        let g = draw_at(&mut v, w, h, s);
        let nodes: Vec<_> = g.hits.iter().filter(|x| x.label.starts_with("graph node")).collect();
        if nodes.len() != 5 {
            bad.push(format!("{w}x{h}: {} nodes drawn", nodes.len()));
        }
        inside_and_apart(&g, w, h, &format!("graph {w}x{h}"), &mut bad);
        // a node is large enough to click
        if let Some(n) = nodes.iter().find(|n| n.rect.w < 24.0 * s.min(1.0) || n.rect.h < 10.0) {
            bad.push(format!("{w}x{h}: {} is too small to click ({:?})", n.label, n.rect));
        }
    }
    assert!(bad.is_empty(), "{}", bad.join("\n"));
    // a board without dependencies explains the view instead of drawing nothing
    let (root2, f2) = super::tests::world("graphnone");
    let mut plain = BoardView::new(&root2, &f2);
    plain.mode = Mode::Graph;
    let g = draw_at(&mut plain, 1000.0, 500.0, 1.0);
    assert!(!g.hits.iter().any(|x| x.label.starts_with("graph node")));
}

#[test]
fn the_agents_strip_shows_each_agent_with_its_row_and_state_and_opens_its_panel() {
    let (mut v, root) = world_with_deps("strip");
    // nothing running and no dependencies drawn into a strip: an old board has no strip at all
    let (r0, f0) = super::tests::world("strip-old");
    let mut old = BoardView::new(&r0, &f0);
    let g = draw_at(&mut old, 1000.0, 500.0, 1.0);
    assert!(!labels(&g).iter().any(|l| l.starts_with("agent ")) && !labels(&g).contains(&"auto-assign: ask".to_string()));
    // dependencies alone: the filter, the ready rows and auto-assign are there, no chips
    let g = draw_at(&mut v, 1280.0, 720.0, 1.0);
    assert!(labels(&g).iter().any(|l| l == "auto-assign: ask"), "{:?}", labels(&g));
    assert!(g.hits.iter().any(|h| h.label.starts_with("1 ready: F-A-04")), "F-A-04 is ready and has nobody: {:?}", labels(&g));
    // two agents
    agents(&root);
    let g = draw_at(&mut v, 1280.0, 720.0, 1.0);
    let l = labels(&g);
    assert!(l.contains(&"agent Agent 1".to_string()) && l.contains(&"agent Agent 2".to_string()), "{l:?}");
    assert_eq!(v.ext.agents_working(), 1, "Agent 2 waits for an approval, it is not 'working'");
    // the lane filter: Agent 1, Agent 2, unassigned, all
    click(&mut v, &g, "agent: all");
    assert_eq!(v.ext.agent_filter, AgentFilter::Agent("Agent 1".into()));
    assert_eq!(v.filtered().iter().map(|f| f.id.as_str()).collect::<Vec<_>>(), ["F-A-02"]);
    v.key(&Key::Char('G'));
    assert_eq!(v.filtered().iter().map(|f| f.id.as_str()).collect::<Vec<_>>(), ["F-A-04"]);
    v.key(&Key::Char('G'));
    assert_eq!(v.ext.agent_filter, AgentFilter::Unassigned);
    assert!(!v.filtered().iter().any(|f| f.id == "F-A-02" || f.id == "F-A-04"));
    v.key(&Key::Char('G'));
    assert_eq!(v.ext.agent_filter, AgentFilter::All);
    // a chip opens the panel (and selects the agent's row); clicking it again closes it
    let g = draw_at(&mut v, 1280.0, 720.0, 1.0);
    click(&mut v, &g, "agent Agent 1");
    assert_eq!(v.ext.drill.as_deref(), Some("t1"));
    assert_eq!(v.selected().unwrap().id, "F-A-02");
    let g = draw_at(&mut v, 1280.0, 720.0, 1.0);
    click(&mut v, &g, "agent Agent 1");
    assert!(v.ext.drill.is_none());
    // auto-assign cycles ask, on, off and the IDE is told
    let g = draw_at(&mut v, 1280.0, 720.0, 1.0);
    assert_eq!(click(&mut v, &g, "auto-assign: ask"), BoardAct::AutoAssign(AutoAssign::On));
    // the ready rows can be assigned with one click, or with A (F-A-04 is free again when its agent is done)
    assign::update(&root, |d| d.set_state("t2", AState::Done, "", iyakki_agent::clock::now_unix()));
    v.refresh_deps(true);
    let g = draw_at(&mut v, 1280.0, 720.0, 1.0);
    let a = click(&mut v, &g, "1 ready: F-A-04 · assign (A)");
    assert_eq!(a, BoardAct::AssignReady(vec!["F-A-04".to_string()]));
    assert_eq!(v.handle(&Event::Key(Key::Char('A'))), BoardAct::AssignReady(vec!["F-A-04".to_string()]));
}

#[test]
fn the_agent_panel_shows_the_timeline_files_notes_and_log_and_its_controls_act_through_the_ide() {
    let (mut v, root) = world_with_deps("panel");
    agents(&root);
    v.refresh_deps(true);
    v.ext.log = vec!["building".into(), "tests passed".into()];
    let g = draw_at(&mut v, 1280.0, 720.0, 1.0);
    click(&mut v, &g, "agent Agent 1");
    let g = draw_at(&mut v, 1280.0, 720.0, 1.0);
    let l = labels(&g);
    for want in ["agent Stop", "agent Pause", "agent Resume", "agent Message", "agent Open run", "close the agent panel", "row F-A-02", "note N-001"] {
        assert!(l.contains(&want.to_string()), "{want}: {l:?}");
    }
    let (steps, files) = v.ext.panel_shape;
    assert!(steps >= 5 && files == 1, "timeline {steps}, files {files}");
    // the controls turn into acts for the IDE, which owns the task manager
    assert_eq!(click(&mut v, &g, "agent Stop"), BoardAct::AgentCmd("t1".into(), 's'));
    assert_eq!(click(&mut v, &g, "agent Pause"), BoardAct::AgentCmd("t1".into(), 'p'));
    // Resume is for an agent that stopped: while it works the button is off and does nothing
    assert_eq!(v.handle(&g.hits.iter().rev().find(|h| h.label == "agent Resume").unwrap().ev.clone()), BoardAct::None);
    assert_eq!(click(&mut v, &g, "agent Message"), BoardAct::AgentCmd("t1".into(), 'm'));
    assert_eq!(click(&mut v, &g, "agent Open run"), BoardAct::AgentCmd("t1".into(), 'o'));
    assert_eq!(click(&mut v, &g, "note N-001"), BoardAct::OpenNotes(Some("N-001".into())));
    // keys do the same while the panel is open; Esc closes it
    assign::update(&root, |d| d.set_state("t1", AState::Paused, "", 1100));
    v.refresh_deps(true);
    let g2 = draw_at(&mut v, 1280.0, 720.0, 1.0);
    assert_eq!(click(&mut v, &g2, "agent Resume"), BoardAct::AgentCmd("t1".into(), 'r'));
    assert_eq!(v.handle(&Event::Key(Key::Char('s'))), BoardAct::AgentCmd("t1".into(), 's'));
    assert_eq!(v.handle(&Event::Key(Key::Char('m'))), BoardAct::AgentCmd("t1".into(), 'm'));
    v.handle(&Event::Key(Key::PageUp));
    assert_eq!(v.ext.drill_scroll, 5, "older steps");
    v.handle(&Event::Key(Key::PageDown));
    v.handle(&Event::Key(Key::Esc));
    assert!(v.ext.drill.is_none());
}

#[test]
fn a_file_conflict_shows_in_the_panel_and_the_person_chooses_to_queue_or_continue() {
    let (mut v, root) = world_with_deps("conflict");
    agents(&root);
    v.refresh_deps(true);
    assert_eq!(v.ext.doc.live_conflicts().len(), 1);
    let g = draw_at(&mut v, 1280.0, 720.0, 1.0);
    assert!(labels(&g).contains(&"1 file conflict".to_string()), "{:?}", labels(&g));
    click(&mut v, &g, "agent Agent 2");
    let g = draw_at(&mut v, 1280.0, 720.0, 1.0);
    click(&mut v, &g, "queue the second agent");
    assert_eq!(assign::load(&root).conflicts[0].choice, Choice::Queue);
    assert_eq!(assign::load(&root).must_wait("t2", "src/server.rs").as_deref(), Some("Agent 1"));
    assert!(v.msg.as_ref().is_some_and(|(m, ok)| *ok && m.contains("waits for the other agent")));
    let g = draw_at(&mut v, 1280.0, 720.0, 1.0);
    click(&mut v, &g, "let it continue");
    assert_eq!(assign::load(&root).conflicts[0].choice, Choice::Continue);
    assert!(assign::load(&root).must_wait("t2", "src/server.rs").is_none());
}

#[test]
fn the_panel_and_the_strip_stay_inside_the_screen_at_the_four_sizes_and_the_panel_covers_the_board_when_narrow() {
    let (mut v, root) = world_with_deps("panelgeom");
    agents(&root);
    v.refresh_deps(true);
    v.ext.log = (1..=6).map(|i| format!("log line {i}")).collect();
    // many steps and many agents, to press on the layout
    assign::update(&root, |d| {
        for i in 0..40 {
            d.step("t1", StepKind::Tool, &format!("tool call number {i} with an output that is rather long and has to be cut to fit"), 1100 + i);
        }
        for k in 3..7 {
            d.register(&format!("t{k}"), "task", "Claude Code (sonnet)", &[format!("F-A-0{}", k.min(6))], &format!("t{k}"), 1000);
        }
    });
    v.refresh_deps(true);
    let mut bad = Vec::new();
    let mut saw_cover = false;
    let mut sizes = SIZES.to_vec();
    sizes.push((560.0, 420.0, 1.0));
    for (w, h, s) in sizes {
        for mode in Mode::ALL {
            for open in [false, true] {
                v.mode = mode;
                v.ext.drill = open.then(|| "t1".to_string());
                let g = draw_at(&mut v, w, h, s);
                let tag = format!("{mode:?} {w}x{h} panel={open}");
                inside_and_apart(&g, w, h, &tag, &mut bad);
                let panel = labels(&g).contains(&"close the agent panel".to_string());
                if panel != open {
                    bad.push(format!("{tag}: panel drawn = {panel}"));
                }
                if open {
                    let cards = g.hits.iter().filter(|x| x.label.starts_with("F-A-")).count();
                    if (w - 20.0 * s) < 700.0 * s {
                        saw_cover |= cards == 0 && mode == Mode::Kanban;
                    }
                    for want in ["agent Stop", "agent Message"] {
                        if !labels(&g).contains(&want.to_string()) {
                            bad.push(format!("{tag}: no {want}"));
                        }
                    }
                    if v.ext.panel_shape.0 < 2 {
                        bad.push(format!("{tag}: the timeline shows only {} steps", v.ext.panel_shape.0));
                    }
                }
            }
        }
    }
    assert!(saw_cover, "a narrow board gives the whole body to the panel");
    assert!(bad.is_empty(), "{} problems:\n{}", bad.len(), bad.iter().take(30).cloned().collect::<Vec<_>>().join("\n"));
}

#[test]
fn the_drawer_lists_what_a_row_waits_for_and_unblocks_and_a_click_goes_to_that_row() {
    let (mut v, root) = world_with_deps("drawer");
    agents(&root);
    v.refresh_deps(true);
    v.select_id("F-A-02");
    v.drawer = true;
    let g = draw_at(&mut v, 1280.0, 720.0, 1.0);
    let l = labels(&g);
    assert!(l.contains(&"go to F-A-01".to_string()) && l.contains(&"go to F-A-03".to_string()), "waits for F-A-01, unblocks F-A-03: {l:?}");
    assert!(l.contains(&"open Agent 1".to_string()));
    click(&mut v, &g, "go to F-A-03");
    assert_eq!(v.selected().unwrap().id, "F-A-03");
    v.drawer = true;
    let g = draw_at(&mut v, 1280.0, 720.0, 1.0);
    click(&mut v, &g, "go to F-A-01");
    assert_eq!(v.selected().unwrap().id, "F-A-01");
    // the agent of a row opens from the drawer
    v.select_id("F-A-02");
    v.drawer = true;
    let g = draw_at(&mut v, 1280.0, 720.0, 1.0);
    click(&mut v, &g, "open Agent 1");
    assert_eq!(v.ext.drill.as_deref(), Some("t1"));
}

#[test]
fn restarting_keeps_the_strip_and_a_dead_run_is_shown_as_interrupted_not_working() {
    let (mut v, root) = world_with_deps("restart");
    agents(&root);
    assign::update(&root, |d| d.reconcile(&[], iyakki_agent::clock::now_unix()));
    v.refresh_deps(true);
    assert_eq!(v.ext.doc.get("t1").unwrap().state, AState::Interrupted);
    assert_eq!(v.ext.agents_working(), 0);
    assert!(v.ext.doc.assignee("F-A-02").is_none(), "an interrupted agent holds nothing");
    let g = draw_at(&mut v, 1280.0, 720.0, 1.0);
    assert!(labels(&g).contains(&"agent Agent 1".to_string()), "it can still be opened (recently active)");
    assert!(deps::state_color(AState::Interrupted) != deps::state_color(AState::Working));
}
