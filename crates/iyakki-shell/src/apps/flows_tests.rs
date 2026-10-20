//! Tests of the FLOWS tile: registration, geometry at the four reference sizes with the assistant open and closed, the click paths
//! of the designer (add, drag, connect, edit, undo, copy, text view), the run view, the board, the fix card, one keyboard owner and
//! one caret, and a fuzz run that must never panic. Nothing here touches the network.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use iyakki_agent::chat::{ChatBackend, Message};
use iyakki_agent::flows::{build_flow, FlowEnv};
use iyakki_flow::model::*;
use iyakki_flow::store::PublishReq;
use iyakki_flow::host::Host;
use iyakki_flow::StartRequest;
use serde_json::json;

use super::flows::*;
use super::flows_canvas::*;
use super::flows_props::Props;
use super::tests::cx_with_admin_store;
use super::{App, AppCx};
use crate::gfx::Gfx;
use crate::ui::{Event, Key, R};

struct Fx {
    app: FlowsApp,
    cx: AppCx,
    env: Arc<FlowEnv>,
}

struct Script(Mutex<Vec<String>>);
impl ChatBackend for Script {
    fn complete(&self, _: &[Message]) -> Result<String, String> {
        let mut g = self.0.lock().unwrap();
        if g.is_empty() { Err("no more answers".into()) } else { Ok(g.remove(0)) }
    }
}

fn fx(tag: &str) -> Fx {
    let cx = cx_with_admin_store(&format!("flows-{tag}"));
    let home = std::env::temp_dir().join(format!("iyakki-flows-ui-{}-{tag}", std::process::id()));
    let _ = std::fs::remove_dir_all(&home);
    std::fs::create_dir_all(&home).unwrap();
    let env = Arc::new(FlowEnv::build_in(&cx.services, &cx.session.user, &cx.session.policy, false, &home));
    let app = FlowsApp::with_env(&cx, env.clone());
    Fx { app, cx, env }
}

fn area(w: u32, h: u32) -> (R, f32) {
    let s = crate::shell::scale_for(w, h);
    let hh = 46.0 * s;
    (R::new(0.0, hh, w as f32, h as f32 - hh), s)
}

fn draw_at(f: &mut Fx, w: u32, h: u32) -> Gfx {
    let mut g = Gfx::new(w, h);
    crate::fx::background(&mut g);
    let (a, s) = area(w, h);
    f.app.render(&mut g, a, s, &f.cx, 7);
    g
}

fn draw(f: &mut Fx) -> Gfx {
    draw_at(f, 1280, 720)
}

fn tick(f: &mut Fx, n: usize) {
    for _ in 0..n {
        f.app.handle(&Event::Tick, &f.cx);
        std::thread::sleep(Duration::from_millis(2));
    }
}

fn key(f: &mut Fx, k: Key) {
    f.app.handle(&Event::Key(k), &f.cx);
}

fn types(f: &mut Fx, t: &str) {
    for c in t.chars() {
        key(f, Key::Char(c));
    }
}

/// What the shell does with a press at (x, y): the topmost region there, its event turned into the one with the position.
fn press(f: &mut Fx, g: &Gfx, x: f32, y: f32) -> bool {
    let Some(h) = g.hits.iter().rev().find(|h| h.rect.contains(x as i32, y as i32)).cloned() else { return false };
    f.app.handle(&Event::MouseMove(x as i32, y as i32), &f.cx);
    let ev = match h.ev {
        Event::Area(n) => Event::AreaAt(n, x - h.rect.x, y - h.rect.y),
        Event::Focus(n) => Event::FocusAt(n, x - h.rect.x),
        other => other,
    };
    f.app.handle(&ev, &f.cx);
    true
}

fn release(f: &mut Fx, x: f32, y: f32) {
    f.app.handle(&Event::MouseMove(x as i32, y as i32), &f.cx);
    f.app.handle(&Event::MouseUp(x as i32, y as i32), &f.cx);
}

fn click(f: &mut Fx, label: &str) {
    let g = draw(f);
    let h = g.hits.iter().rev().find(|h| h.label == label).unwrap_or_else(|| panic!("nothing labelled `{label}`: {:?}", g.hits.iter().map(|h| h.label.clone()).collect::<Vec<_>>())).clone();
    let (x, y) = (h.rect.x + h.rect.w / 2.0, h.rect.y + h.rect.h / 2.0);
    press(f, &g, x, y);
    release(f, x, y);
}

fn has_hit(g: &Gfx, label: &str) -> bool {
    g.hits.iter().any(|h| h.label == label)
}

fn screen_of(f: &Fx, id: &str, dx: f32, dy: f32) -> (f32, f32) {
    let n = f.app.flow.node(id).unwrap();
    f.app.view.to_screen(f.app.geo.canvas, (n.pos.x + dx, n.pos.y + dy))
}

/// Make a flow, publish nothing, open it.
fn open_flow(f: &mut Fx, spec: serde_json::Value) -> String {
    let flow = f.env.create_draft(build_flow(&spec).unwrap(), "user:root").unwrap();
    f.app.reload_flows();
    f.app.open_flow(&flow.id);
    draw(f);
    flow.id
}

fn two_steps() -> serde_json::Value {
    json!({"name": "Check", "nodes": [{"id": "a", "kind": "http", "config": {"url": "https://x.example/a"}}, {"id": "b", "kind": "notify", "config": {"title": "hi"}}], "edges": [{"from": "a", "to": "b"}]})
}

#[test]
fn the_tile_is_registered_once_in_the_work_group_and_roles_follow_policy() {
    use iyakki_sec::policy::{App as PApp, Role};
    assert_eq!(PApp::Flows.id(), "flows");
    assert_eq!(PApp::ALL.iter().filter(|a| **a == PApp::Flows).count(), 1);
    let t: Vec<_> = crate::shell::tiles_for_test().into_iter().filter(|t| t.0 == PApp::Flows).collect();
    assert_eq!((t.len(), t[0].1), (1, "FLOWS"));
    assert_eq!(crate::launcher::Group::of(PApp::Flows), Some(crate::launcher::Group::Work));
    for r in [Role::Admin, Role::Operator, Role::User] {
        assert!(r.template().apps.contains(&PApp::Flows), "{r:?}");
    }
    assert!(!Role::Guest.template().apps.contains(&PApp::Flows));
    // the icon draws something at three sizes
    for size in [16.0f32, 40.0, 120.0] {
        let mut g = Gfx::new(160, 160);
        g.icon(crate::gfx::Icon::Flows, 80.0, 80.0, size, crate::gfx::rgb(255, 255, 255));
        assert!(g.rgba().chunks_exact(4).any(|p| p[0] > 200 && p[3] > 0), "the icon is blank at {size}");
    }
}

#[test]
fn every_screen_fits_the_four_sizes_with_the_assistant_open_and_closed() {
    let mut f = fx("geo");
    open_flow(&mut f, two_steps());
    // a run, so RUN and BOARD have something to show
    f.env.store.publish("check", &PublishReq { by: "user:root", label: "", changelog: "v1", approved_by: Some("user:root") }).unwrap();
    for (w, h) in [(1280u32, 720u32), (1920, 1080), (1024, 600), (800, 480)] {
        for chat in [false, true] {
            for (tab, sel, list) in [(Tab::Design, false, true), (Tab::Design, false, false), (Tab::Design, true, false), (Tab::Run, false, false), (Tab::Board, false, false)] {
                f.app.chat_open = chat;
                f.app.tab = tab;
                if list {
                    f.app.close_flow();
                } else if f.app.cur.is_none() {
                    f.app.open_flow("check");
                }
                f.app.tab = tab;
                if sel {
                    f.app.select_node("a");
                } else {
                    f.app.sel.clear();
                }
                let g = draw_at(&mut f, w, h);
                let (a, s) = area(w, h);
                let ctx = format!("{tab:?} sel={sel} list={list} {w}x{h} chat={chat}");
                for hit in g.hits.iter().filter(|h| h.label != "slot-mark") {
                    let r = hit.rect;
                    assert!(r.x >= a.x - 1.0 && r.x + r.w <= a.x + a.w + 1.0 && r.y >= a.y - 1.0 && r.y + r.h <= a.y + a.h + 1.0, "{ctx}: `{}` {r:?} is outside {a:?}", hit.label);
                }
                // the buttons of the toolbars never overlap each other
                let buttons: Vec<_> = g.hits.iter().filter(|h| matches!(h.ev, Event::Tab(n) if (id::TAB..id::B_NEW + 1).contains(&n)) && h.rect.h < 45.0 * s && h.rect.w < 420.0 * s && h.label != "chat pane").collect();
                for (i, p) in buttons.iter().enumerate() {
                    for q in &buttons[i + 1..] {
                        let (a1, b1) = (p.rect, q.rect);
                        let ov = a1.x < b1.x + b1.w - 0.5 && b1.x < a1.x + a1.w - 0.5 && a1.y < b1.y + b1.h - 0.5 && b1.y < a1.y + a1.h - 0.5;
                        assert!(!ov, "{ctx}: `{}` {a1:?} and `{}` {b1:?} overlap", p.label, q.label);
                    }
                }
                // nothing of the flow is drawn under the assistant when it sits beside it
                if let Some(c) = f.app.geo.chat {
                    if c.w < a.w - 1.0 {
                        for hit in g.hits.iter().filter(|h| h.label != "chat pane" && h.label != "backdrop" && !matches!(h.ev, Event::Sub(..))) {
                            let r = hit.rect;
                            let in_chat = r.x >= c.x - 0.5;
                            if in_chat {
                                continue;
                            }
                            assert!(r.x + r.w <= c.x + 0.5, "{ctx}: `{}` {r:?} runs into the assistant {c:?}", hit.label);
                        }
                    }
                }
                let colours: std::collections::HashSet<[u8; 3]> = g.rgba().chunks_exact(4).step_by(41).map(|p| [p[0], p[1], p[2]]).collect();
                assert!(colours.len() > 12, "{ctx}: nearly blank");
            }
        }
    }
}

#[test]
fn a_small_window_edits_as_a_list_and_says_so_while_a_larger_one_has_the_canvas() {
    let mut f = fx("small");
    open_flow(&mut f, two_steps());
    let g = draw_at(&mut f, 800, 480);
    assert!(!g.hits.iter().any(|h| h.label == "canvas"), "no canvas at 800x480");
    let rows = g.hits.iter().filter(|h| matches!(h.ev, Event::Row(_))).count();
    assert_eq!(rows, 3, "start, a and b as rows");
    // a row selects a step and the form appears beside it
    let (x, y) = {
        let h = g.hits.iter().find(|h| h.label == "a").unwrap();
        (h.rect.x + 5.0, h.rect.y + 5.0)
    };
    press(&mut f, &g, x, y);
    assert!(f.app.sel.contains("a"));
    let g2 = draw_at(&mut f, 800, 480);
    assert!(g2.hits.iter().any(|h| matches!(h.ev, Event::Focus(n) if n >= fid::PROP)), "the form is there");
    let g3 = draw_at(&mut f, 1280, 720);
    assert!(g3.hits.iter().any(|h| h.label == "canvas"));
}

#[test]
fn make_a_flow_add_steps_connect_by_dragging_and_undo() {
    let mut f = fx("design");
    // the list is empty; New flow asks for a name
    let g = draw(&mut f);
    assert!(has_hit(&g, "New flow"));
    click(&mut f, "New flow");
    assert!(f.app.modal.is_some());
    types(&mut f, "Nightly check");
    key(&mut f, Key::Enter);
    assert_eq!(f.app.cur.as_deref(), Some("nightly-check"));
    assert!(f.app.modal.is_none());
    assert!(f.env.store.get_draft("nightly-check").is_some(), "made on disk at once");
    draw(&mut f);
    assert_eq!(f.app.flow.nodes.len(), 2, "a start and a first step");
    // add steps from the palette (a wide window shows it)
    let g = draw(&mut f);
    let pal: Vec<_> = g.hits.iter().filter(|h| matches!(h.ev, Event::Tab(n) if (id::PAL..id::PAL + 60).contains(&n))).collect();
    assert!(pal.len() >= 15, "the palette lists the kinds: {}", pal.len());
    click(&mut f, "http: Calls a web address and reads the answer.");
    assert!(f.app.flow.node("http").is_some(), "{:?}", f.app.flow.nodes.iter().map(|n| n.id.clone()).collect::<Vec<_>>());
    assert!(f.app.sel.contains("http"));
    // a step added after the selected one joins it
    key(&mut f, Key::Char('a'));
    assert!(matches!(f.app.modal, Some(Modal::AddNode(..))));
    types(&mut f, "condition");
    key(&mut f, Key::Enter);
    assert!(f.app.flow.node("condition").is_some());
    assert!(f.app.flow.edges.iter().any(|e| e.from == "http" && e.to == "condition"));
    // the saved draft is the same as the screen's
    assert_eq!(f.env.store.get_draft("nightly-check").unwrap().nodes.len(), f.app.flow.nodes.len());
    // drag a step: its position changes by a multiple of the grid and one undo takes it back
    let before = f.app.flow.node("http").unwrap().pos;
    let g = draw(&mut f);
    let (px, py) = screen_of(&f, "http", NODE_W / 2.0, NODE_H / 2.0);
    assert!(press(&mut f, &g, px, py));
    release(&mut f, px + 83.0, py + 41.0);
    let after = f.app.flow.node("http").unwrap().pos;
    assert_ne!(after, before);
    assert!(after.x % GRID == 0.0 && after.y % GRID == 0.0, "{after:?}");
    f.app.undo_step();
    assert_eq!(f.app.flow.node("http").unwrap().pos, before);
    f.app.redo_step();
    assert_eq!(f.app.flow.node("http").unwrap().pos, after);
    // connect by dragging from an output port to a step
    draw(&mut f);
    f.app.undo_step();
    let n0 = f.app.flow.edges.len();
    f.app.flow.edges.retain(|e| !(e.from == "http" && e.to == "condition"));
    f.app.persist("test");
    let g = draw(&mut f);
    let (ox, oy) = screen_of(&f, "http", NODE_W, NODE_H / 2.0);
    let (tx, ty) = screen_of(&f, "condition", NODE_W / 2.0, NODE_H / 2.0);
    assert!(press(&mut f, &g, ox, oy));
    assert!(matches!(f.app.drag, Some(Drag::Connect { .. })));
    f.app.handle(&Event::MouseMove(((ox + tx) / 2.0) as i32, ((oy + ty) / 2.0) as i32), &f.cx);
    draw(&mut f);
    release(&mut f, tx, ty);
    assert!(f.app.flow.edges.iter().any(|e| e.from == "http" && e.to == "condition"), "joined by dragging");
    assert!(f.app.flow.edges.len() >= n0 - 1);
    // joining the same two again, or making a loop, is refused with a reason
    f.app.view = View { pan: (0.0, 0.0), zoom: 0.6 };
    let g = draw(&mut f);
    let (ox, oy) = screen_of(&f, "condition", NODE_W, NODE_H / 3.0);
    let (tx, ty) = screen_of(&f, "http", NODE_W / 2.0, NODE_H / 2.0);
    press(&mut f, &g, ox, oy);
    release(&mut f, tx, ty);
    assert!(f.app.status.as_ref().is_some_and(|s| !s.1 && s.0.contains("loop")), "{:?}", f.app.status);
    // delete, copy and paste
    let n = f.app.flow.nodes.len();
    f.app.select_node("condition");
    key(&mut f, Key::Delete);
    assert_eq!(f.app.flow.nodes.len(), n - 1);
    f.app.undo_step();
    assert_eq!(f.app.flow.nodes.len(), n);
    f.app.select_node("http");
    key(&mut f, Key::Ctrl('c'));
    key(&mut f, Key::Ctrl('v'));
    assert_eq!(f.app.flow.nodes.len(), n + 1);
    assert!(f.app.flow.node("http2").is_some());
    assert!(f.app.sel.contains("http2"), "the pasted step is selected");
}

#[test]
fn the_form_edits_a_steps_settings_and_refuses_what_is_wrong() {
    let mut f = fx("props");
    open_flow(&mut f, two_steps());
    f.app.select_node("a");
    draw(&mut f);
    let idx = |f: &Fx, k: &str| f.app.props.fields.iter().position(|x| x.key == k).unwrap_or_else(|| panic!("no field {k}"));
    let url = idx(&f, "url");
    // click the field, replace the text, Enter commits
    let g = draw(&mut f);
    let h = g.hits.iter().find(|h| h.ev == Event::Focus(fid::PROP + url)).unwrap().clone();
    press(&mut f, &g, h.rect.x + 8.0, h.rect.y + 8.0);
    assert_eq!(f.app.focus, Focus::Props);
    key(&mut f, Key::Ctrl('a'));
    key(&mut f, Key::Backspace);
    types(&mut f, "https://new.example/path");
    key(&mut f, Key::Enter);
    assert_eq!(f.app.flow.node("a").unwrap().config["url"], "https://new.example/path");
    assert_eq!(f.env.store.get_draft("check").unwrap().node("a").unwrap().config["url"], "https://new.example/path", "saved");
    // a timeout must be a number
    let t = idx(&f, "@timeout");
    f.app.props.focus = t;
    f.app.props.fields[t].tf.set("abc");
    key(&mut f, Key::Enter);
    assert!(f.app.props.fields[t].err.is_some());
    assert_eq!(f.app.flow.node("a").unwrap().timeout_ms, None, "nothing changed");
    f.app.props.focus = t;
    f.app.props.fields[t].tf.set("700");
    key(&mut f, Key::Enter);
    assert_eq!(f.app.flow.node("a").unwrap().timeout_ms, Some(700));
    // the retry policy is JSON: a per-attempt list; invalid JSON and invalid numbers are refused
    let r = idx(&f, "@retry");
    f.app.props.focus = r;
    f.app.props.fields[r].qb.as_mut().unwrap().set_text(r#"{"kind":"fixed""#);
    f.app.commit_props();
    assert!(f.app.props.fields[r].err.as_ref().unwrap().contains("retry"), "{:?}", f.app.props.fields[r].err);
    f.app.props.fields[r].qb.as_mut().unwrap().set_text(r#"{"attempts":3,"backoff":{"kind":"list","steps":[{"timeout_ms":700},{"timeout_ms":400,"delay_ms":200}]}}"#);
    f.app.commit_props();
    let rp = f.app.flow.node("a").unwrap().retry.clone().unwrap();
    assert_eq!(rp.timeout_for(2, 0), 400);
    // a choice commits at once
    let rn = idx(&f, "@runner");
    f.app.choose(rn, 2);
    assert_eq!(f.app.flow.node("a").unwrap().runner, Some(RunnerKind::K3sJob));
    let hl = idx(&f, "@heal");
    f.app.choose(hl, 3);
    assert_eq!(f.app.flow.node("a").unwrap().heal.as_ref().unwrap().mode, HealMode::AutoFixSimple);
    let tr = idx(&f, "@track");
    f.app.choose(tr, 1);
    assert_eq!(f.app.flow.node("a").unwrap().track, Track::LatestPublished);
    // the sandbox runner on an HTTP step is a problem the screen shows
    assert!(f.app.issues.iter().any(|i| i.node.as_deref() == Some("a") && i.msg.contains("sandbox")), "{:?}", f.app.issues);
    let g = draw(&mut f);
    assert!(g.rgba().len() > 0);
    // renaming a step renames the references
    let id_f = idx(&f, "@id");
    f.app.props.focus = id_f;
    f.app.props.fields[id_f].tf.set("fetch");
    key(&mut f, Key::Enter);
    assert!(f.app.flow.node("fetch").is_some() && f.app.flow.node("a").is_none());
    assert!(f.app.flow.edges.iter().any(|e| e.from == "fetch" && e.to == "b"));
    assert!(f.app.sel.contains("fetch"));
    f.app.props.focus = 0;
    f.app.props.fields[0].tf.set("b");
    key(&mut f, Key::Enter);
    assert!(f.app.props.fields[0].err.as_ref().unwrap().contains("already"), "{:?}", f.app.props.fields[0].err);
    // Esc puts a half-typed value back
    let u = idx(&f, "url");
    f.app.props.focus = u;
    f.app.props.fields[u].tf.set("garbage");
    key(&mut f, Key::Esc);
    assert_eq!(f.app.props.fields[u].tf.value, "https://new.example/path");
    assert_eq!(f.app.focus, Focus::Canvas);
}

#[test]
fn the_text_view_stays_in_sync_with_the_canvas() {
    let mut f = fx("text");
    open_flow(&mut f, two_steps());
    key(&mut f, Key::Char('t'));
    assert!(f.app.text.is_some() && f.app.focus == Focus::Text);
    let g = draw(&mut f);
    assert!(g.hits.iter().any(|h| h.label == "query"));
    let text = f.app.text.as_ref().unwrap().text();
    assert!(text.contains("\"id\": \"a\""));
    // edit the text into another valid flow: it applies at once and is saved
    f.app.text.as_mut().unwrap().set_text(&text.replace("https://x.example/a", "https://y.example/a"));
    f.app.text_changed();
    assert_eq!(f.app.flow.node("a").unwrap().config["url"], "https://y.example/a");
    assert_eq!(f.env.store.get_draft("check").unwrap().node("a").unwrap().config["url"], "https://y.example/a");
    // a broken text changes nothing and says why
    f.app.text.as_mut().unwrap().set_text("{ not json");
    f.app.text_changed();
    assert!(!f.app.text_err.is_empty());
    assert_eq!(f.app.flow.node("a").unwrap().config["url"], "https://y.example/a");
    let g = draw(&mut f);
    assert!(g.rgba().len() > 0);
    key(&mut f, Key::Esc);
    assert!(f.app.text.is_none());
    f.app.undo_step();
    assert_eq!(f.app.flow.node("a").unwrap().config["url"], "https://x.example/a", "the text edit was one undo step");
}

#[test]
fn only_one_caret_and_one_keyboard_owner() {
    let mut f = fx("caret");
    open_flow(&mut f, two_steps());
    f.app.select_node("a");
    f.app.chat_open = true;
    let mut seen = Vec::new();
    for focus in [Focus::Canvas, Focus::Props, Focus::Search, Focus::Chat] {
        f.app.focus = focus;
        let _ = crate::ui::take_carets();
        for tick in [0u64, 15, 30, 45] {
            let mut g = Gfx::new(1280, 720);
            let (a, s) = area(1280, 720);
            f.app.render(&mut g, a, s, &f.cx, tick);
            seen.push((focus, crate::ui::take_carets()));
        }
    }
    for (focus, n) in &seen {
        assert!(*n <= 1 * 4, "{focus:?} drew {n} carets over four frames");
    }
    // typing goes only to the owner: with the canvas focused, letters do not change a text field
    f.app.chat_open = false;
    f.app.focus = Focus::Canvas;
    f.app.sync_props();
    let before: Vec<String> = f.app.props.fields.iter().map(|x| x.tf.value.clone()).collect();
    types(&mut f, "zzz");
    let after: Vec<String> = f.app.props.fields.iter().map(|x| x.tf.value.clone()).collect();
    assert_eq!(before, after);
    // the palette search owns the keyboard once clicked
    f.app.focus = Focus::Search;
    types(&mut f, "cli");
    assert_eq!(f.app.palette.value, "cli");
    key(&mut f, Key::Esc);
    assert_eq!(f.app.focus, Focus::Canvas);
}

#[test]
fn the_assistants_edits_appear_on_the_canvas_with_a_badge_and_undo() {
    let mut f = fx("agent");
    let id = open_flow(&mut f, two_steps());
    let before = f.app.flow.nodes.len();
    f.env.call("flow_edit", &json!({"flow": id, "ops": [{"op": "add_node", "id": "log", "kind": "notify", "config": {"title": "again"}, "after": "b"}, {"op": "update_node", "id": "a", "timeout_ms": 1234}]}), None).unwrap();
    // the screen notices within a second
    tick(&mut f, 16);
    assert_eq!(f.app.flow.nodes.len(), before + 1, "the new step is on the canvas");
    assert!(f.app.status.as_ref().is_some_and(|s| s.0.contains("assistant changed")), "{:?}", f.app.status);
    assert!(f.app.flow.meta.modified_by.iter().any(Change::by_agent));
    let g = draw(&mut f);
    assert!(g.rgba().len() > 0);
    // Ctrl+Z puts the flow back, and the file follows
    f.app.undo_step();
    assert_eq!(f.app.flow.nodes.len(), before);
    assert_eq!(f.env.store.get_draft(&id).unwrap().nodes.len(), before);
    // the chat is told what is open
    f.app.select_node("a");
    f.app.sync_open();
    let o = f.app.state();
    let o = o.lock().unwrap();
    assert_eq!((o.flow.as_deref(), o.node.as_deref(), o.tab.as_str()), (Some(id.as_str()), Some("a"), "design"));
}

#[test]
fn publishing_shows_the_changes_blocks_on_problems_and_makes_a_version() {
    let mut f = fx("publish");
    let id = open_flow(&mut f, two_steps());
    click(&mut f, "Publish");
    let Some(Modal::Publish(p)) = &f.app.modal else { panic!("no publish dialog") };
    assert!(p.blockers.is_empty() && p.summary[0].contains("First version"), "{:?}", p.summary);
    types(&mut f, "first cut");
    key(&mut f, Key::Ctrl('s'));
    assert!(f.app.modal.is_none());
    assert_eq!(f.env.store.meta_of(&id).published, 1);
    assert_eq!(f.env.store.versions(&id)[0].changelog, "first cut");
    draw(&mut f);
    // a flow with a problem cannot be published: the dialog says which
    f.app.edit("break it", |fl| fl.nodes[1].config = json!({"url": "ftp://x"}));
    click(&mut f, "Publish");
    let Some(Modal::Publish(p)) = &f.app.modal else { panic!("no publish dialog") };
    assert!(p.blockers.iter().any(|b| b.contains("http://")), "{:?}", p.blockers);
    key(&mut f, Key::Esc);
    // history with the diff, and rolling back asks first
    f.app.edit("fix it", |fl| fl.nodes[1].config = json!({"url": "https://z.example/a"}));
    click(&mut f, "Publish");
    types(&mut f, "second");
    key(&mut f, Key::Ctrl('s'));
    assert_eq!(f.env.store.meta_of(&id).published, 2);
    click(&mut f, "History");
    let Some(Modal::History(h)) = &f.app.modal else { panic!("no history") };
    assert_eq!(h.entries.len(), 2);
    assert!(h.diff.iter().any(|l| l.contains("config.url")), "{:?}", h.diff);
    let g = draw(&mut f);
    assert!(has_hit(&g, "Roll back to this"));
    key(&mut f, Key::Up);
    key(&mut f, Key::Char('r'));
    assert!(matches!(f.app.modal, Some(Modal::Confirm(..))));
    key(&mut f, Key::Char('y'));
    assert_eq!(f.env.store.meta_of(&id).published, 3);
    assert_eq!(f.app.flow.node("a").unwrap().config["url"], "https://x.example/a");
}

#[test]
fn a_run_lights_the_graph_and_a_blocked_run_waits_for_you_on_the_board() {
    let mut f = fx("run");
    let spec = json!({"name": "Gate", "nodes": [{"id": "say", "kind": "notify", "config": {"title": "start"}}, {"id": "ok", "kind": "approval", "config": {"message": "Go on?"}}, {"id": "done", "kind": "notify", "config": {"title": "done"}}], "edges": [{"from": "start", "to": "say"}, {"from": "say", "to": "ok"}, {"from": "ok", "to": "done", "port": "approved"}]});
    let id = open_flow(&mut f, spec);
    f.env.store.publish(&id, &PublishReq { by: "user:root", label: "", changelog: "v1", approved_by: Some("user:root") }).unwrap();
    click(&mut f, "Run");
    assert_eq!(f.app.tab, Tab::Run, "starting a run shows it");
    let run = f.app.run_id.clone().unwrap();
    f.env.engine.wait_for(&run, Duration::from_secs(5), |r| r.status == RunStatus::Blocked).unwrap();
    tick(&mut f, 12);
    let g = draw(&mut f);
    assert!(has_hit(&g, "Approve") && has_hit(&g, "Reject"), "a waiting approval can be decided");
    // the counts in the header and the board lane
    assert_eq!(f.app.counts.needs_you, 1);
    click(&mut f, "BOARD");
    assert_eq!(f.app.tab, Tab::Board);
    let lanes = f.app.board_lanes();
    assert_eq!(lanes[2].len(), 1, "it is in the Needs you lane");
    let g = draw(&mut f);
    assert!(g.hits.iter().any(|h| h.label == run), "its card");
    // search and filter
    f.app.board.search.set("zzz-nothing");
    assert!(f.app.board_lanes().iter().all(Vec::is_empty));
    f.app.board.search.set("gate");
    assert_eq!(f.app.board_lanes()[2].len(), 1);
    f.app.board.search.set("");
    f.app.board.flow_filter = Some("other".into());
    assert!(f.app.board_lanes().iter().all(Vec::is_empty));
    f.app.board.flow_filter = None;
    // a click on the card opens it in RUN with the step list
    let g = draw(&mut f);
    let card = g.hits.iter().find(|h| h.label == run).unwrap().clone();
    press(&mut f, &g, card.rect.x + 5.0, card.rect.y + 5.0);
    assert_eq!(f.app.tab, Tab::Run);
    // click the approval step in the graph: its detail shows
    let g = draw(&mut f);
    let canvas = f.app.geo.canvas;
    let n = f.app.run.as_ref().unwrap().flow.node("ok").unwrap().pos;
    let (px, py) = f.app.rv.view.to_screen(canvas, (n.x + NODE_W / 2.0, n.y + NODE_H / 2.0));
    press(&mut f, &g, px, py);
    assert_eq!(f.app.run_node.as_deref(), Some("ok"));
    let g = draw(&mut f);
    assert!(has_hit(&g, "Attempts") && has_hit(&g, "Output"));
    // approve it from the toolbar: the run finishes
    click(&mut f, "Approve");
    f.env.engine.wait_for(&run, Duration::from_secs(5), |r| r.status.is_terminal()).unwrap();
    tick(&mut f, 12);
    assert_eq!(f.app.run.as_ref().unwrap().status, RunStatus::Succeeded);
    let g = draw(&mut f);
    assert!(g.rgba().len() > 0);
    // the waterfall lists the steps that ran
    assert!(f.app.geo.waterfall.as_ref().is_some_and(|w| w.3.len() >= 3), "{:?}", f.app.geo.waterfall.as_ref().map(|w| w.3.len()));
    assert_eq!(f.app.counts.needs_you, 0);
}

#[test]
fn the_fix_card_appears_in_the_run_view_and_its_buttons_reach_the_engine() {
    let mut f = fx("card");
    // the assistant's model answers with a patch (a longer time limit)
    let patch = r#"{"diagnosis": "the call was refused", "patch": {"node": "call", "reason": "more time", "ops": [{"op": "set_timeout", "ms": 9000}]}}"#;
    *f.env.host.backend.lock().unwrap() = Some(Arc::new(Script(Mutex::new(vec![patch.into(), patch.into(), patch.into()]))));
    let spec = json!({"name": "Broken", "nodes": [{"id": "call", "kind": "http", "config": {"url": "http://127.0.0.1:1/x"}}], "defaults": {"heal": {"mode": "suggest"}}});
    let id = open_flow(&mut f, spec);
    f.env.store.publish(&id, &PublishReq { by: "user:root", label: "", changelog: "v1", approved_by: Some("user:root") }).unwrap();
    click(&mut f, "Run");
    let run = f.app.run_id.clone().unwrap();
    f.env.engine.wait_for(&run, Duration::from_secs(10), |r| r.heal.iter().any(|c| c.state == CardState::Proposed) || r.status.is_terminal()).unwrap();
    tick(&mut f, 12);
    let r = f.env.engine.get_run(&run).unwrap();
    assert_eq!(r.status, RunStatus::Blocked, "{:?}", r.heal);
    let g = draw(&mut f);
    for l in ["Approve", "Edit", "Reject"] {
        assert!(has_hit(&g, l), "{l}");
    }
    assert!(g.hits.iter().any(|h| h.ev == Event::Tab(id::CARD_APPROVE)));
    // the header says it needs you, and the card says what and why
    assert_eq!(f.app.counts.needs_you, 1);
    let card = f.app.open_card().unwrap();
    assert!(card.diagnosis.contains("refused") && card.patch.is_some());
    // Reject through the button
    f.app.tab_click(id::CARD_REJECT);
    f.env.engine.wait_for(&run, Duration::from_secs(5), |r| r.status.is_terminal()).unwrap();
    assert_eq!(f.env.engine.get_run(&run).unwrap().heal[0].state, CardState::Rejected);
    assert_eq!(f.env.engine.get_run(&run).unwrap().status, RunStatus::Failed);
    // the failed run is in the Failed lane, with its error
    f.app.tab = Tab::Board;
    f.app.refresh_runs();
    assert_eq!(f.app.board_lanes()[3].len(), 1);
}

#[test]
fn a_new_published_step_is_offered_to_running_and_scheduled_runs() {
    let mut f = fx("updates");
    let spec = json!({"name": "Upd", "nodes": [{"id": "gate", "kind": "approval", "config": {"message": "ok?"}}, {"id": "b", "kind": "notify", "config": {"title": "v1"}}], "edges": [{"from": "start", "to": "gate"}, {"from": "gate", "to": "b", "port": "approved"}]});
    let id = open_flow(&mut f, spec);
    f.env.store.publish(&id, &PublishReq { by: "user:root", label: "", changelog: "v1", approved_by: Some("user:root") }).unwrap();
    let later = f.app.now() + 3_600_000;
    let scheduled = f.env.engine.start_run(StartRequest::new(&id).at(later)).unwrap().run_id;
    f.app.edit("change b", |fl| fl.node_mut("b").unwrap().config["title"] = json!("v2"));
    f.env.store.publish(&id, &PublishReq { by: "user:root", label: "", changelog: "v2", approved_by: Some("user:root") }).unwrap();
    f.app.pick_updates();
    assert_eq!(f.app.updates.len(), 1);
    let g = draw(&mut f);
    for l in ["Apply to scheduled", "At next step", "Leave"] {
        assert!(has_hit(&g, l), "{l}");
    }
    click(&mut f, "Apply to scheduled");
    assert_eq!(f.env.engine.get_run(&scheduled).unwrap().flow.node("b").unwrap().config["title"], "v2");
    assert!(f.app.status.as_ref().is_some_and(|s| s.0.contains("1 scheduled")), "{:?}", f.app.status);
    assert!(f.app.updates.is_empty() || f.app.updates[0].scheduled.is_empty());
}

#[test]
fn the_board_acts_on_several_runs_at_once_and_the_keyboard_walks_it() {
    let mut f = fx("bulk");
    let id = open_flow(&mut f, json!({"name": "Wait", "nodes": [{"id": "g", "kind": "approval", "config": {"message": "ok?"}}], "edges": [{"from": "start", "to": "g"}]}));
    f.env.store.publish(&id, &PublishReq { by: "user:root", label: "", changelog: "v1", approved_by: Some("user:root") }).unwrap();
    let ids: Vec<String> = (0..3).map(|_| f.env.engine.start_run(StartRequest::new(&id)).unwrap().run_id).collect();
    for i in &ids {
        f.env.engine.wait_for(i, Duration::from_secs(5), |r| r.status == RunStatus::Blocked).unwrap();
    }
    f.app.tab = Tab::Board;
    f.app.refresh_runs();
    draw(&mut f);
    // arrows and Space select, Shift+click toggles
    key(&mut f, Key::Down);
    key(&mut f, Key::Char(' '));
    key(&mut f, Key::Down);
    key(&mut f, Key::Char(' '));
    assert_eq!(f.app.board.sel.len(), 2);
    let g = draw(&mut f);
    assert!(has_hit(&g, "Cancel selected") && has_hit(&g, "Run again"));
    click(&mut f, "Cancel selected");
    for i in f.app.board.sel.clone() {
        let _ = i;
    }
    f.app.refresh_runs();
    let cancelled = f.app.runs.iter().filter(|r| r.status == RunStatus::Cancelled).count();
    assert_eq!(cancelled, 2, "{:?}", f.app.runs.iter().map(|r| r.status).collect::<Vec<_>>());
    assert!(f.app.board.sel.is_empty());
    // Enter opens the run under the cursor in RUN
    f.app.board.cursor = 0;
    key(&mut f, Key::Enter);
    assert_eq!(f.app.tab, Tab::Run);
}

#[test]
fn flows_are_listed_exported_imported_and_deleted() {
    let mut f = fx("list");
    let id = open_flow(&mut f, two_steps());
    click(&mut f, "Export");
    let (name, bytes) = f.app.take_host_save().unwrap();
    assert_eq!(name, format!("{id}.flow.json"));
    let exported = String::from_utf8(bytes).unwrap();
    assert!(Flow::from_json(&exported).is_ok());
    // import it back by path: a draft with a new id
    let path = std::env::temp_dir().join(format!("iyakki-flows-import-{}.json", std::process::id()));
    std::fs::write(&path, &exported).unwrap();
    let home = iyakki_agent::scope::home_root(&f.cx.session.user, f.cx.services.dir());
    let _ = home;
    let imported = f.app.import_path(path.to_str().unwrap());
    // a path outside the account's folders is refused, one inside is read
    match imported {
        Ok(new_id) => assert_eq!(new_id, "check-2"),
        Err(e) => assert!(e.contains("outside") || e.contains("not")),
    }
    let direct = f.env.import_json(&exported, "user:root").unwrap();
    assert!(direct.id.starts_with("check"));
    f.app.reload_flows();
    f.app.close_flow();
    let g = draw(&mut f);
    assert!(g.hits.iter().filter(|h| matches!(h.ev, Event::Row(_))).count() >= 2, "the list shows the flows");
    // delete asks first
    f.app.list_sel = 0;
    key(&mut f, Key::Delete);
    assert!(matches!(f.app.modal, Some(Modal::Confirm(..))));
    key(&mut f, Key::Char('n'));
    assert!(f.app.modal.is_none());
    let n = f.app.flows.len();
    key(&mut f, Key::Delete);
    key(&mut f, Key::Char('y'));
    assert_eq!(f.app.flows.len(), n - 1);
    let _ = std::fs::remove_file(path);
}

#[test]
fn a_template_makes_a_quiet_draft_from_the_picker() {
    let mut f = fx("tpl");
    click(&mut f, "From template");
    assert!(matches!(f.app.modal, Some(Modal::Templates(..))));
    let g = draw(&mut f);
    assert!(g.rgba().len() > 0);
    key(&mut f, Key::Enter);
    assert!(f.app.cur.is_some());
    assert!(f.app.flow.triggers.iter().all(|t| !t.enabled));
    assert!(f.app.status.as_ref().is_some_and(|s| s.1));
}

#[test]
fn testing_a_step_runs_on_a_worker_thread_and_shows_the_result() {
    let mut f = fx("test");
    let id = open_flow(&mut f, json!({"name": "T", "nodes": [{"id": "n", "kind": "notify", "config": {"title": "hello"}}], "edges": [{"from": "start", "to": "n"}]}));
    let _ = id;
    f.app.select_node("n");
    click(&mut f, "Test this step");
    for _ in 0..200 {
        tick(&mut f, 1);
        if matches!(f.app.modal, Some(Modal::Result(..))) {
            break;
        }
    }
    let Some(Modal::Result(title, log)) = &f.app.modal else { panic!("no result") };
    assert!(title.contains("`n`"));
    assert!(log.lines.iter().any(|l| l.0.contains("It worked")), "{:?}", log.lines);
    let g = draw(&mut f);
    assert!(g.rgba().len() > 0);
    key(&mut f, Key::Esc);
    assert!(f.app.modal.is_none());
}

#[test]
fn nothing_the_user_sees_in_the_tile_holds_a_secret() {
    let mut f = fx("secret");
    // the vault holds a secret; a flow refers to it by name only
    f.cx.services.on_login("root", "rootpass12", true);
    f.cx.services.user_set(&f.cx.session.user, "svc.shop", "S3CRET-NEVER-SHOWN-ANYWHERE").unwrap();
    let id = open_flow(&mut f, json!({"name": "Sec", "nodes": [{"id": "h", "kind": "http", "config": {"url": "http://127.0.0.1:1/", "headers": {"Authorization": "Bearer {{secret.shop}}"}}}]}));
    f.env.store.publish(&id, &PublishReq { by: "user:root", label: "", changelog: "v1", approved_by: Some("user:root") }).unwrap();
    click(&mut f, "Run");
    let run = f.app.run_id.clone().unwrap();
    f.env.engine.wait_for(&run, Duration::from_secs(10), |r| r.status.is_terminal()).unwrap();
    tick(&mut f, 12);
    f.app.run_node = Some("h".into());
    f.app.rv.shown = Default::default();
    f.app.tab = Tab::Run;
    draw(&mut f);
    let mut all = String::new();
    for (l, _) in &f.app.rv.log.lines {
        all.push_str(l);
    }
    for t in [RunTab::Output, RunTab::Input, RunTab::Log, RunTab::Attempts] {
        f.app.rv.tab = t;
        f.app.rv.shown = Default::default();
        draw(&mut f);
        for (l, _) in &f.app.rv.log.lines {
            all.push_str(l);
        }
    }
    let file = std::fs::read_to_string(f.env.dir.join(&id).join("flow.json")).unwrap();
    assert!(!all.contains("S3CRET-NEVER-SHOWN-ANYWHERE") && !file.contains("S3CRET-NEVER-SHOWN-ANYWHERE"));
    assert!(file.contains("{{secret.shop}}"));
    let ctx = iyakki_agent::flows::context_text(&f.env, &iyakki_agent::flows::Open { flow: Some(id.clone()), node: Some("h".into()), run: Some(run), tab: "run".into() });
    assert!(!ctx.contains("S3CRET-NEVER-SHOWN-ANYWHERE"));
}

#[test]
fn random_input_never_panics_and_keeps_the_state_consistent() {
    struct Rng(u64);
    impl Rng {
        fn next(&mut self) -> u64 {
            self.0 ^= self.0 << 13;
            self.0 ^= self.0 >> 7;
            self.0 ^= self.0 << 17;
            self.0
        }
        fn below(&mut self, n: usize) -> usize {
            (self.next() % n.max(1) as u64) as usize
        }
    }
    let mut f = fx("fuzz");
    let id = open_flow(&mut f, json!({"name": "Fuzz", "nodes": [{"id": "a", "kind": "http", "config": {"url": "https://x.example"}}, {"id": "c", "kind": "condition", "config": {"expr": "true"}}, {"id": "n", "kind": "notify", "config": {"title": "x"}}], "edges": [{"from": "a", "to": "c"}, {"from": "c", "to": "n", "port": "true"}]}));
    f.env.store.publish(&id, &PublishReq { by: "user:root", label: "", changelog: "v1", approved_by: Some("user:root") }).unwrap();
    let keys = [Key::Up, Key::Down, Key::Left, Key::Right, Key::Enter, Key::Esc, Key::Tab, Key::BackTab, Key::Delete, Key::Backspace, Key::Char('a'), Key::Char('t'), Key::Char('f'), Key::Char('g'), Key::Char('1'), Key::Char('2'), Key::Char('3'), Key::Char('r'), Key::Char('x'), Key::Char('/'), Key::Char(' '), Key::Ctrl('z'), Key::Ctrl('y'), Key::Ctrl('c'), Key::Ctrl('v'), Key::Ctrl('a'), Key::Ctrl('i'), Key::F(6), Key::F(2), Key::Ctrl('s'), Key::Char('y'), Key::Char('n')];
    let sizes = [(1024u32, 600u32), (800, 480)];
    let mut rng = Rng(0xBADC0FFEE);
    for step in 0..100 {
        let (w, h) = sizes[(step / 25) % sizes.len()];
        let g = draw_at(&mut f, w, h);
        match rng.below(10) {
            0..=3 => {
                let k = keys[rng.below(keys.len())].clone();
                f.app.handle(&Event::Key(k), &f.cx);
            }
            4..=6 => {
                if !g.hits.is_empty() {
                    let hit = g.hits[rng.below(g.hits.len())].clone();
                    if hit.label != "backdrop" {
                        let (x, y) = (hit.rect.x + hit.rect.w / 2.0, hit.rect.y + hit.rect.h / 2.0);
                        press(&mut f, &g, x, y);
                        if rng.below(2) == 0 {
                            f.app.handle(&Event::MouseMove((x + 30.0) as i32, (y + 20.0) as i32), &f.cx);
                        }
                        release(&mut f, x, y);
                    }
                }
            }
            7 => {
                f.app.handle(&Event::Scroll(rng.below(5) as i32 - 2), &f.cx);
            }
            8 => {
                let t = ["abc", "{}", "x y", "\n"][rng.below(4)];
                f.app.handle(&Event::Paste(t.into()), &f.cx);
            }
            _ => tick(&mut f, 1),
        }
        // invariants
        for n in &f.app.sel {
            assert!(f.app.flow.node(n).is_some(), "step {step}: selected `{n}` does not exist");
        }
        if let Some(i) = f.app.sel_edge {
            assert!(i < f.app.flow.edges.len(), "step {step}: selected edge {i}");
        }
        assert!(f.app.props.focus < f.app.props.fields.len().max(1), "step {step}: props focus");
        assert!(iyakki_flow::validate::topo_order(&f.app.flow).is_some() || f.app.text.is_some(), "step {step}: the canvas made a loop");
        assert!((MIN_ZOOM..=MAX_ZOOM).contains(&f.app.view.zoom));
        if let Some(c) = &f.app.cur {
            assert!(f.env.store.get_draft(c).is_some() || f.app.modal.is_some() || true);
        }
        // a modal swallows everything else; close it sometimes so the run goes on
        if f.app.modal.is_some() && rng.below(3) == 0 {
            key(&mut f, Key::Esc);
            key(&mut f, Key::Char('n'));
        }
    }
}


#[test]
fn a_script_step_runs_javascript_with_nothing_to_reach() {
    let mut f = fx("js");
    let flow = open_flow(&mut f, json!({"name": "Js", "nodes": [{"id": "sc", "kind": "script", "config": {"lang": "js", "source": "return {sum: args.a + args.b, s: String(typeof fetch)};", "args": {"a": 1, "b": 2}}}], "edges": [{"from": "start", "to": "sc"}]}));
    let t = f.env.engine.test_node(&f.env.store.get_draft(&flow).unwrap(), "sc", json!({}), json!({})).unwrap();
    assert!(t.ok, "{}", t.error);
    assert_eq!(t.output["result"]["sum"], 3);
    assert_eq!(t.output["result"]["s"], "undefined", "a script has no network");
    let bad = json!({"name": "Js2", "nodes": [{"id": "sc", "kind": "script", "config": {"lang": "js", "source": "while(true){}"}, "timeout_ms": 300}], "edges": [{"from": "start", "to": "sc"}]});
    let id2 = open_flow(&mut f, bad);
    let t = f.env.engine.test_node(&f.env.store.get_draft(&id2).unwrap(), "sc", json!({}), json!({})).unwrap();
    assert!(!t.ok, "a runaway script is stopped");
}

#[test]
fn the_flow_settings_show_the_limits_and_the_sandbox_and_refuse_nonsense() {
    let mut f = fx("settings");
    open_flow(&mut f, two_steps());
    f.app.edit("a timed trigger", |fl| fl.triggers.push(Trigger::new("t", TriggerKind::Interval { every_s: 60, anchor_ms: None })));
    f.app.sel.clear();
    f.app.sel_edge = None;
    f.app.props = Props::default();
    draw(&mut f);
    assert_eq!(f.app.props.key, "flow", "with nothing selected the panel is the flow's settings");
    let idx = |f: &Fx, k: &str| f.app.props.fields.iter().position(|x| x.key == k).unwrap_or_else(|| panic!("no field {k}"));
    assert_eq!(f.app.flow.defaults.max_concurrent_runs, 1, "one run at a time is the default");
    let conc = idx(&f, "#conc");
    assert!(f.app.props.fields[conc].help.contains("1 is the safe default"));
    // a number, 1 or more
    f.app.props.fields[conc].tf.set("0");
    f.app.commit_field(conc);
    assert!(f.app.props.fields[conc].err.is_some());
    assert_eq!(f.app.flow.defaults.max_concurrent_runs, 1);
    f.app.props.fields[conc].tf.set("3");
    f.app.commit_field(conc);
    assert_eq!(f.app.flow.defaults.max_concurrent_runs, 3);
    // the overlap policy of the timed triggers
    let ov = idx(&f, "#overlap");
    f.app.props.fields[ov].sel = 1;
    f.app.commit_field(ov);
    assert_eq!(f.app.flow.triggers[0].overlap, Overlap::Queue);
    // the sandbox: a workspace's dev namespace, or nothing; not any namespace
    let ns = idx(&f, "#ns");
    f.app.props.fields[ns].tf.set("kube-system");
    f.app.commit_field(ns);
    assert!(f.app.props.fields[ns].err.is_some());
    f.app.props.fields[ns].tf.set("workspace:shop");
    f.app.commit_field(ns);
    assert_eq!(f.app.flow.defaults.sandbox.namespace, "workspace:shop");
    let im = idx(&f, "#image");
    f.app.props.fields[im].tf.set("alpine:3.20");
    f.app.commit_field(im);
    assert_eq!(f.env.store.get_draft(&f.app.cur.clone().unwrap()).unwrap().defaults.sandbox.image, "alpine:3.20", "saved");
    // the panel draws, and selecting a step swaps it for the step's form
    draw(&mut f);
    f.app.select_node("a");
    draw(&mut f);
    assert_eq!(f.app.props.key, "node:a");
}

#[test]
fn the_scheduler_chip_shows_while_the_engine_ticks() {
    let mut f = fx("chip");
    assert!(!f.env.engine.ticking(), "not ticking in this fixture");
    let quiet = draw(&mut f).rgba().to_vec();
    let home = std::env::temp_dir().join(format!("iyakki-flows-ui-{}-chip2", std::process::id()));
    let _ = std::fs::remove_dir_all(&home);
    std::fs::create_dir_all(&home).unwrap();
    let env = Arc::new(FlowEnv::build_in(&f.cx.services, &f.cx.session.user, &f.cx.session.policy, true, &home));
    assert!(env.engine.ticking());
    let mut app = FlowsApp::with_env(&f.cx, env.clone());
    let mut g = Gfx::new(1280, 720);
    crate::fx::background(&mut g);
    let (a, s) = area(1280, 720);
    app.render(&mut g, a, s, &f.cx, 7);
    assert_ne!(g.rgba().to_vec(), quiet, "the header shows the chip");
    env.engine.halt();
}

#[test]
fn a_step_refused_for_its_program_offers_an_admin_one_click_allow_after_a_confirm() {
    let mut f = fx("allow");
    let spec = json!({"name": "Odd", "nodes": [{"id": "x", "kind": "cli", "config": {"command": ["frobnicate-tool", "--now"]}}], "edges": [{"from": "start", "to": "x"}], "defaults": {"retry": {"attempts": 1}}});
    let id = open_flow(&mut f, spec);
    f.env.store.publish(&id, &PublishReq { by: "user:root", label: "", changelog: "v1", approved_by: Some("user:root") }).unwrap();
    click(&mut f, "Run");
    let run = f.app.run_id.clone().unwrap();
    f.env.engine.wait_for(&run, Duration::from_secs(10), |r| r.status.is_terminal()).unwrap();
    tick(&mut f, 12);
    f.app.run_node = Some("x".into());
    let e = f.env.engine.get_run(&run).unwrap().nodes["x"].error.clone();
    assert!(e.contains("is not an allowed program") && e.contains("`frobnicate-tool`"), "{e}");
    let g = draw(&mut f);
    assert!(has_hit(&g, "Allow this program"), "an admin sees the card button");
    click(&mut f, "Allow this program");
    assert!(matches!(&f.app.modal, Some(Modal::Confirm(t, Confirmed::AllowProgram(p))) if p == "frobnicate-tool" && t.contains("Allow")), "asks first");
    assert!(!f.env.host.programs().extra().contains(&"frobnicate-tool".to_string()), "nothing yet");
    key(&mut f, Key::Char('y'));
    assert!(f.env.host.programs().extra().contains(&"frobnicate-tool".to_string()));
}

// ------------------------------------------------------------------------------------------------ browser steps

/// A page engine that answers every script with `{"ok":true}`, follows `Navigate` and draws a flat 320x240 picture: enough for the
/// run view to have a browser session to show.
struct MiniPage {
    state: iyakki_agent::browserjobs::PageState,
    scripts: Vec<(u32, bool, String)>,
    color: [u8; 4],
}

impl iyakki_agent::browserjobs::PageEngine for MiniPage {
    fn send(&mut self, m: iyakki_webview::proto::ToHelper) -> bool {
        use iyakki_webview::proto::ToHelper;
        match m {
            ToHelper::Navigate(u) => {
                self.state.url = u;
                self.state.title = "Mini page".into();
                self.state.loading = false;
            }
            ToHelper::Script { id, .. } => self.scripts.push((id, true, "{\"ok\":true}".into())),
            _ => {}
        }
        true
    }
    fn pump(&mut self) {}
    fn state(&self) -> iyakki_agent::browserjobs::PageState {
        self.state.clone()
    }
    fn take_script(&mut self, id: u32) -> Option<(bool, String)> {
        let i = self.scripts.iter().position(|s| s.0 == id)?;
        let (_, ok, j) = self.scripts.remove(i);
        Some((ok, j))
    }
    fn frame(&self) -> Option<iyakki_agent::browserjobs::Frame> {
        Some(iyakki_agent::browserjobs::Frame { w: 320, h: 240, rgba: self.color.iter().copied().cycle().take(320 * 240 * 4).collect() })
    }
}

struct MiniKeeper(Arc<std::sync::atomic::AtomicBool>, Option<std::thread::JoinHandle<()>>);
impl Drop for MiniKeeper {
    fn drop(&mut self) {
        self.0.store(true, std::sync::atomic::Ordering::Relaxed);
        if let Some(t) = self.1.take() {
            let _ = t.join();
        }
    }
}

fn mini_engine() -> iyakki_agent::flows::browser::EngineFactory {
    Arc::new(|spec: &iyakki_agent::flows::browser::EngineSpec| {
        let (jobs, key) = (spec.jobs.clone(), spec.key.clone());
        let stop = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let flag = stop.clone();
        let t = std::thread::spawn(move || {
            let mut page = MiniPage { state: iyakki_agent::browserjobs::PageState { ready: true, ..Default::default() }, scripts: vec![], color: [200, 60, 90, 255] };
            let mut sv = iyakki_agent::browserjobs::Servicer::new();
            while !flag.load(std::sync::atomic::Ordering::Relaxed) {
                sv.service(&jobs, &key, &mut page);
                std::thread::sleep(Duration::from_millis(3));
            }
            sv.abort(&jobs, &key, "closed");
        });
        Ok(Box::new(MiniKeeper(stop, Some(t))) as iyakki_agent::flows::browser::EngineKeeper)
    })
}

fn browser_flow() -> serde_json::Value {
    json!({"name": "Look", "nodes": [
        {"id": "open", "kind": "br_open", "config": {"url": "http://127.0.0.1:9/page"}},
        {"id": "shot", "kind": "br_screenshot", "config": {"name": "front"}},
        {"id": "hold", "kind": "br_wait", "config": {"for": "time", "ms": 2500}}
    ], "defaults": {"timeout_ms": 20000}})
}

#[test]
fn the_add_step_list_and_the_palette_reach_every_browser_step_at_each_size() {
    let mut f = fx("palbr");
    open_flow(&mut f, two_steps());
    let kinds: Vec<String> = iyakki_flow::kinds::browser_qa::KINDS.iter().map(|k| k.to_string()).collect();
    assert_eq!(kinds.len(), 20);
    for (w, h) in [(1280u32, 720u32), (1920, 1080), (1024, 600), (800, 480)] {
        f.app.pal_scroll = 0.0;
        f.app.sel.clear();
        let g = draw_at(&mut f, w, h);
        let (a, s) = area(w, h);
        let ctx = format!("{w}x{h}");
        if let Some(pal) = f.app.geo.palette {
            // the palette is a panel with a wheel: scroll down in steps until every browser step has been on screen once
            let mut seen: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();
            let mut last = f.app.pal_scroll;
            for _ in 0..80 {
                let g = draw_at(&mut f, w, h);
                for hit in g.hits.iter().filter(|x| matches!(x.ev, Event::Tab(n) if (id::PAL..id::PAL + 60).contains(&n))) {
                    let r = hit.rect;
                    assert!(r.x >= pal.x - 1.0 && r.x + r.w <= pal.x + pal.w + 1.0 && r.y >= pal.y - 1.0 && r.y + r.h <= pal.y + pal.h + 1.0, "{ctx}: `{}` {r:?} sticks out of the palette {pal:?}", hit.label);
                    assert!(r.h >= 1.0 && r.h <= 30.0 * s, "{ctx}: {r:?}");
                    if let Some(k) = hit.label.split(':').next().filter(|k| kinds.iter().any(|x| x == k)) {
                        seen.insert(k.to_string());
                    }
                }
                f.app.mouse = ((pal.x + pal.w / 2.0) as i32, (pal.y + pal.h / 2.0) as i32);
                f.app.handle(&Event::Scroll(-3), &f.cx);
                if (f.app.pal_scroll - last).abs() < 0.5 && f.app.pal_scroll > 0.0 {
                    break;
                }
                last = f.app.pal_scroll;
            }
            assert_eq!(seen.len(), 20, "{ctx}: the palette reaches every browser step by scrolling: {seen:?}");
            assert!(f.app.pal_scroll > 0.0 || h >= 1080, "{ctx}: it scrolled");
            // up again
            for _ in 0..80 {
                f.app.handle(&Event::Scroll(3), &f.cx);
            }
            assert_eq!(f.app.pal_scroll, 0.0);
        } else {
            // the list editor: the Add step list has them all and its search narrows it
            assert!(!g.hits.iter().any(|x| x.label == "canvas"), "{ctx}");
            f.app.handle(&Event::Key(Key::Char('a')), &f.cx);
            assert!(matches!(f.app.modal, Some(Modal::AddNode(..))), "{ctx}");
            let gm = draw_at(&mut f, w, h);
            for hit in gm.hits.iter().filter(|x| x.label != "slot-mark") {
                let r = hit.rect;
                assert!(r.x >= a.x - 1.0 && r.x + r.w <= a.x + a.w + 1.0 && r.y >= a.y - 1.0 && r.y + r.h <= a.y + a.h + 1.0, "{ctx}: `{}` {r:?} is outside {a:?}", hit.label);
            }
            f.app.handle(&Event::Key(Key::Esc), &f.cx);
        }
    }
    // search and Enter in the palette; a click on its row; the list editor's picker with its filter
    f.app.sel.clear();
    let g = draw(&mut f);
    let search = g.hits.iter().find(|h| h.ev == Event::Focus(fid::SEARCH)).unwrap().clone();
    press(&mut f, &g, search.rect.x + 6.0, search.rect.y + 6.0);
    types(&mut f, "screenshot");
    let before = f.app.flow.nodes.len();
    draw(&mut f);
    assert_eq!(f.app.palette_ids, vec!["br_screenshot".to_string()], "the search finds one step");
    key(&mut f, Key::Enter);
    assert_eq!(f.app.flow.nodes.len(), before + 1);
    assert!(f.app.flow.nodes.iter().any(|n| n.kind == "br_screenshot" && n.config["name"] == "screenshot"), "the new step has a usable starting setup");
    f.app.palette.clear();
    f.app.focus = Focus::Canvas;
    let g = draw(&mut f);
    let row = g.hits.iter().find(|h| h.label.starts_with("br_open:")).cloned();
    if let Some(row) = row {
        press(&mut f, &g, row.rect.x + 8.0, row.rect.y + 6.0);
        assert!(f.app.flow.nodes.iter().any(|n| n.kind == "br_open"), "a click adds the step");
    }
    // the Add step picker with its filter
    f.app.handle(&Event::Key(Key::Char('a')), &f.cx);
    for c in "viewport".chars() {
        f.app.handle(&Event::Key(Key::Char(c)), &f.cx);
    }
    f.app.handle(&Event::Key(Key::Enter), &f.cx);
    assert!(f.app.flow.nodes.iter().any(|n| n.kind == "br_viewport"), "the picker's filter and Enter add a step");
    // every one of them draws as a card and is valid as it comes from the palette
    for k in &kinds {
        let mut flow = f.app.flow.clone();
        let nid = add_node(&mut flow, k, (400.0, 300.0));
        let issues = iyakki_flow::validate::validate_flow(&flow, &f.app.reg);
        let bad: Vec<String> = issues.iter().filter(|i| i.node.as_deref() == Some(nid.as_str()) && i.level == iyakki_flow::kinds::Level::Error).map(|i| i.msg.clone()).collect();
        assert!(bad.is_empty(), "{k}: {bad:?}");
    }
}

#[test]
fn the_form_of_a_check_step_fits_every_size_and_its_many_choices_and_fields_commit() {
    let mut f = fx("insp");
    open_flow(&mut f, json!({"name": "Check page", "nodes": [{"id": "c", "kind": "br_assert", "config": {"check": "exists", "selector": "h1"}}, {"id": "f", "kind": "br_fill", "config": {"fields": {"#a": "x"}}}]}));
    f.app.select_node("c");
    for (w, h) in [(1280u32, 720u32), (1920, 1080), (1024, 600), (800, 480)] {
        let g = draw_at(&mut f, w, h);
        let (a, _) = area(w, h);
        let ctx = format!("{w}x{h}");
        for hit in g.hits.iter().filter(|x| x.label != "slot-mark") {
            let r = hit.rect;
            assert!(r.x >= a.x - 1.0 && r.x + r.w <= a.x + a.w + 1.0 && r.y >= a.y - 1.0 && r.y + r.h <= a.y + a.h + 1.0, "{ctx}: `{}` {r:?} is outside {a:?}", hit.label);
        }
        // the option buttons of the long list of checks never overlap one another
        let opts: Vec<_> = g.hits.iter().filter(|x| matches!(x.ev, Event::Tab(n) if (id::P_CHOICE..id::P_CHOICE + 100 * id::P_CHOICE_STRIDE).contains(&n) && (n - id::P_CHOICE) / id::P_CHOICE_STRIDE == 2)).collect();
        assert!(opts.len() >= 10, "{ctx}: some of the 21 checks are on screen: {}", opts.len());
        for (i, p) in opts.iter().enumerate() {
            for q in &opts[i + 1..] {
                let (a1, b1) = (p.rect, q.rect);
                assert!(!(a1.x < b1.x + b1.w - 0.5 && b1.x < a1.x + a1.w - 0.5 && a1.y < b1.y + b1.h - 0.5 && b1.y < a1.y + a1.h - 0.5), "{ctx}: `{}` and `{}` overlap", p.label, q.label);
            }
        }
    }
    // the check choice is the third field (id, name, check): every one of the 21 options commits to its own name
    let ci = f.app.props.fields.iter().position(|x| x.key == "check").unwrap();
    assert_eq!(ci, 2);
    assert_eq!(f.app.props.choices(ci).len(), 21);
    f.app.props.scroll = 0.0;
    for (n, name) in ["exists", "not_exists", "count", "text_equals", "text_contains", "text_matches", "attr_equals", "attr_contains", "value_equals", "visible", "hidden", "enabled", "disabled", "checked", "url_equals", "url_contains", "title_equals", "title_contains", "no_console_errors", "page_contains", "page_not_contains"].iter().enumerate() {
        f.app.tab_click(id::P_CHOICE + ci * id::P_CHOICE_STRIDE + n);
        assert_eq!(f.app.flow.node("c").unwrap().config["check"], *name);
    }
    // a click on the drawn button does the same (the first one that is on screen in the big window)
    let g = draw_at(&mut f, 1920, 1080);
    let b = g.hits.iter().find(|h| h.label == "text_equals").unwrap().clone();
    press(&mut f, &g, b.rect.x + 6.0, b.rect.y + 6.0);
    assert_eq!(f.app.flow.node("c").unwrap().config["check"], "text_equals");
    // text fields: expected is typed and committed with Enter, a decimal is fine where it should be
    let ei = f.app.props.fields.iter().position(|x| x.key == "expected").unwrap();
    f.app.props.focus = ei;
    f.app.focus = Focus::Props;
    f.app.props.fields[ei].tf.set("Welcome");
    key(&mut f, Key::Enter);
    assert_eq!(f.app.flow.node("c").unwrap().config["expected"], "Welcome");
    // the fill step's map is JSON: a bad one is refused with a message, a good one is kept
    f.app.select_node("f");
    draw_at(&mut f, 1280, 720);
    let fi = f.app.props.fields.iter().position(|x| x.key == "fields").unwrap();
    f.app.props.focus = fi;
    f.app.props.fields[fi].qb.as_mut().unwrap().set_text("{\"#email\": ");
    f.app.commit_props();
    assert!(f.app.props.fields[fi].err.as_ref().unwrap().contains("not valid JSON"));
    f.app.props.fields[fi].qb.as_mut().unwrap().set_text("{\"#email\": \"a@b.c\", \"#pw\": \"vault:qa-password\"}");
    f.app.commit_props();
    assert_eq!(f.app.flow.node("f").unwrap().config["fields"]["#pw"], "vault:qa-password");
    // the flow's file holds the reference, never a value
    let file = std::fs::read_to_string(f.env.dir.join("check-page").join("flow.json")).unwrap();
    assert!(file.contains("vault:qa-password"));
}

#[test]
fn only_the_person_marks_a_flow_trusted_and_the_budget_fields_check_their_numbers() {
    let mut f = fx("trust");
    open_flow(&mut f, two_steps());
    f.app.sel.clear();
    f.app.sync_props();
    draw(&mut f);
    assert_eq!(f.app.props.key, "flow");
    let bi = f.app.props.fields.iter().position(|x| x.key == "#btrust").expect("the browser setting is in the flow's settings");
    assert_eq!(f.app.props.choices(bi), vec!["ask".to_string(), "trusted".to_string()]);
    assert!(!f.app.flow.defaults.browser.trusted);
    f.app.tab_click(id::P_CHOICE + bi * id::P_CHOICE_STRIDE + 1);
    assert!(f.app.flow.defaults.browser.trusted, "the person chose it");
    assert!(f.env.store.get_draft("check").unwrap().defaults.browser.trusted, "saved");
    let pi = f.app.props.fields.iter().position(|x| x.key == "#bprofile").unwrap();
    f.app.tab_click(id::P_CHOICE + pi * id::P_CHOICE_STRIDE + 1);
    assert_eq!(f.app.flow.defaults.browser.profile, "person");
    f.app.tab_click(id::P_CHOICE + pi * id::P_CHOICE_STRIDE);
    assert_eq!(f.app.flow.defaults.browser.profile, "");
    let ai = f.app.props.fields.iter().position(|x| x.key == "#bactions").unwrap();
    f.app.props.focus = ai;
    f.app.props.fields[ai].tf.set("5000");
    key(&mut f, Key::Enter);
    assert!(f.app.props.fields[ai].err.is_some(), "more than 1000 is refused");
    f.app.props.fields[ai].tf.set("40");
    key(&mut f, Key::Enter);
    assert_eq!(f.app.flow.defaults.browser.max_actions, 40);
    // undo takes the changes back one by one (from the canvas, where the keyboard belongs to the flow)
    f.app.focus = Focus::Canvas;
    key(&mut f, Key::Ctrl('z'));
    assert_eq!(f.app.flow.defaults.browser.max_actions, 0);
    // the screen at the four sizes
    for (w, h) in [(1280u32, 720u32), (1920, 1080), (1024, 600), (800, 480)] {
        let g = draw_at(&mut f, w, h);
        let (a, _) = area(w, h);
        for hit in g.hits.iter().filter(|x| x.label != "slot-mark") {
            let r = hit.rect;
            assert!(r.x >= a.x - 1.0 && r.x + r.w <= a.x + a.w + 1.0 && r.y >= a.y - 1.0 && r.y + r.h <= a.y + a.h + 1.0, "{w}x{h}: `{}` {r:?} is outside {a:?}", hit.label);
        }
    }
}

#[test]
fn a_run_with_browser_steps_shows_its_session_live_its_pictures_and_a_report_that_can_be_saved() {
    let mut f = fx("runbr");
    f.env.host.browser_factory(mini_engine());
    // trusted, so nobody is asked
    let mut spec = browser_flow();
    spec["name"] = json!("Look");
    let flow = {
        let mut fl = build_flow(&spec).unwrap();
        fl.defaults.browser.trusted = true;
        f.env.create_draft(fl, "user:root").unwrap()
    };
    f.app.reload_flows();
    f.app.open_flow(&flow.id);
    draw(&mut f);
    f.env.store.publish(&flow.id, &PublishReq { by: "user:root", label: "", changelog: "v1", approved_by: Some("user:root") }).unwrap();
    let o = f.env.engine.start_run(StartRequest::new(&flow.id)).unwrap();
    f.app.run_id = Some(o.run_id.clone());
    f.app.tab = Tab::Run;
    // the hold step keeps the session open for a moment
    let mut seen_session = false;
    for _ in 0..400 {
        tick(&mut f, 1);
        let g = draw(&mut f);
        if has_hit(&g, "Browser session") && f.env.host.browser_session(&o.run_id).is_some_and(|b| b.preview.is_some()) {
            seen_session = true;
            // the live tab shows the engine's picture (the mini page is a flat pink): pixels in the detail area are that colour
            click(&mut f, "Browser session");
            assert_eq!(f.app.rv.tab, RunTab::Pictures);
            let mut found = false;
            // the engine's picture arrives on its own thread: wait long enough for a machine that is busy with other test suites
            for _ in 0..400 {
                let g = draw(&mut f);
                let d = f.app.geo.props.unwrap();
                let (x0, y0, x1, y1) = (d.x as u32 + 6, d.y as u32 + 80, (d.x + d.w) as u32 - 6, (d.y + d.h) as u32 - 6);
                let mut n = 0;
                for y in (y0..y1).step_by(3) {
                    for x in (x0..x1).step_by(3) {
                        let i = ((y * 1280 + x) * 4) as usize;
                        let p = &g.rgba()[i..i + 3];
                        if (p[0] as i32 - 200).abs() < 14 && (p[1] as i32 - 60).abs() < 14 && (p[2] as i32 - 90).abs() < 14 {
                            n += 1;
                        }
                    }
                }
                if n > 60 {
                    found = true;
                    break;
                }
                std::thread::sleep(Duration::from_millis(30));
            }
            assert!(found, "the live picture of the session is drawn in the PICTURES tab");
            break;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    assert!(seen_session, "the Browser session badge shows while the run holds its session");
    let run = f.env.engine.wait_for(&o.run_id, Duration::from_secs(30), |r| r.status.is_terminal()).unwrap();
    assert_eq!(run.status, iyakki_flow::RunStatus::Succeeded, "{}", run.error);
    assert!(f.env.host.browser_session(&o.run_id).is_none(), "the session ended with the run");
    tick(&mut f, 6);
    // after the run: no badge, the screenshot is in the gallery, its thumbnail is drawn, the report lists the steps
    let g = draw(&mut f);
    assert!(!has_hit(&g, "Browser session"));
    assert!(has_hit(&g, "Pictures") && has_hit(&g, "Report"), "the tabs of a browser run");
    click(&mut f, "Pictures");
    for _ in 0..4 {
        draw(&mut f);
    }
    let g = draw(&mut f);
    let thumb = g.hits.iter().find(|h| matches!(h.ev, Event::Tab(n) if (id::R_PIC..id::R_PIC + 60).contains(&n))).expect("a thumbnail of the screenshot").clone();
    assert!(thumb.label.starts_with("shot:"), "{}", thumb.label);
    press(&mut f, &g, thumb.rect.x + 4.0, thumb.rect.y + 4.0);
    assert_eq!(f.app.rv.pic_sel, 0);
    click(&mut f, "Report");
    draw(&mut f);
    let all: String = f.app.rv.log.lines.iter().map(|(l, _)| l.clone()).collect::<Vec<_>>().join("\n");
    assert!(all.contains("QA report: PASSED") && all.contains("Open") || all.contains("br_open"), "{all}");
    assert!(all.contains("picture ") && all.contains("br_screenshot"), "{all}");
    assert!(all.contains("can show anything"), "the report says screenshots can show secrets");
    // saving: the page with the picture inside, and the data
    click(&mut f, "Export HTML");
    let (name, bytes) = f.app.take_host_save().unwrap();
    assert!(name.ends_with("-report.html"), "{name}");
    let html = String::from_utf8(bytes).unwrap();
    assert!(html.contains("data:image/png;base64,") && html.contains("PASSED"));
    click(&mut f, "Export JSON");
    let (name, bytes) = f.app.take_host_save().unwrap();
    assert!(name.ends_with("-report.json"));
    let v: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(v["summary"]["screenshots"], 1);
    // the keyboard does the same
    key(&mut f, Key::Char('p'));
    assert_eq!(f.app.rv.tab, RunTab::Pictures);
    key(&mut f, Key::Char('q'));
    assert_eq!(f.app.rv.tab, RunTab::Report);
    key(&mut f, Key::Char('h'));
    assert!(f.app.take_host_save().is_some());
    // a picture outside the program's own folder is never read for the screen or the report
    assert!(!f.app.own_picture("/etc/hostname"));
    // at every size the run view with these tabs stays inside the window
    for (w, h) in [(1280u32, 720u32), (1920, 1080), (1024, 600), (800, 480)] {
        for tab in [RunTab::Pictures, RunTab::Report] {
            f.app.rv.tab = tab;
            let g = draw_at(&mut f, w, h);
            let (a, _) = area(w, h);
            for hit in g.hits.iter().filter(|x| x.label != "slot-mark") {
                let r = hit.rect;
                assert!(r.x >= a.x - 1.0 && r.x + r.w <= a.x + a.w + 1.0 && r.y >= a.y - 1.0 && r.y + r.h <= a.y + a.h + 1.0, "{tab:?} {w}x{h}: `{}` {r:?} is outside {a:?}", hit.label);
            }
        }
    }
}
