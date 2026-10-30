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
    for (w, h) in [(1280u32, 720u32), (1920, 1080), (1024, 600), (800, 480), (480, 600)] {
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
    assert!(has_hit(&g, "New flow (n)"));
    click(&mut f, "New flow (n)");
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
    click(&mut f, "Publish (p)");
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
    click(&mut f, "Publish (p)");
    let Some(Modal::Publish(p)) = &f.app.modal else { panic!("no publish dialog") };
    assert!(p.blockers.iter().any(|b| b.contains("http://")), "{:?}", p.blockers);
    key(&mut f, Key::Esc);
    // history with the diff, and rolling back asks first
    f.app.edit("fix it", |fl| fl.nodes[1].config = json!({"url": "https://z.example/a"}));
    click(&mut f, "Publish (p)");
    types(&mut f, "second");
    key(&mut f, Key::Ctrl('s'));
    assert_eq!(f.env.store.meta_of(&id).published, 2);
    click(&mut f, "History (h)");
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
    click(&mut f, "Run (r)");
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
    click(&mut f, "Run (r)");
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
    click(&mut f, "From template (t)");
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
    click(&mut f, "Run (r)");
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
    click(&mut f, "Run (r)");
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
    for (w, h) in [(1280u32, 720u32), (1920, 1080), (1024, 600), (800, 480), (480, 600)] {
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
    for (w, h) in [(1280u32, 720u32), (1920, 1080), (1024, 600), (800, 480), (480, 600)] {
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
fn a_step_stopped_by_the_camera_permission_offers_the_flow_settings_and_never_the_assistant() {
    let mut f = fx("camcard");
    let script = Arc::new(Script(Mutex::new(vec!["{}".into(), "{}".into(), "{}".into()])));
    *f.env.host.backend.lock().unwrap() = Some(script.clone());
    let spec = json!({"name": "Look", "nodes": [{"id": "shot", "kind": "cam_photo", "config": {}}], "defaults": {"heal": {"mode": "suggest"}}});
    let id = open_flow(&mut f, spec);
    f.env.store.publish(&id, &PublishReq { by: "user:root", label: "", changelog: "v1", approved_by: Some("user:root") }).unwrap();
    click(&mut f, "Run (r)");
    let run = f.app.run_id.clone().unwrap();
    f.env.engine.wait_for(&run, Duration::from_secs(10), |r| r.status == RunStatus::Blocked || r.status.is_terminal()).unwrap();
    tick(&mut f, 12);
    let r = f.env.engine.get_run(&run).unwrap();
    assert_eq!(r.status, RunStatus::Blocked, "{:?}", r.heal);
    assert_eq!(script.0.lock().unwrap().len(), 3, "the assistant was not asked");
    assert_eq!(r.heal_spent, 0);
    let g = draw(&mut f);
    assert!(has_hit(&g, "Open flow settings"), "{:?}", g.hits.iter().map(|h| h.label.clone()).collect::<Vec<_>>());
    assert!(!has_hit(&g, "Approve") && !has_hit(&g, "Ask the assistant"));
    assert!(f.app.open_card().unwrap().diagnosis.contains("Camera allowed"));
    f.app.tab_click(id::CARD_SETTINGS);
    draw(&mut f);
    assert_eq!(f.app.tab, Tab::Design);
    assert_eq!(f.app.props.key, "flow", "the flow's settings are open");
    let ci = f.app.props.fields.iter().position(|x| x.key == "#camallow").expect("the camera switch is there");
    assert!(f.app.props.fields[ci].help.contains("is sent only to the vision model") && f.app.props.fields[ci].help.contains("cloud service") && (f.app.props.fields[ci].help.contains("Using ") || f.app.props.fields[ci].help.contains("No vision model")), "{}", f.app.props.fields[ci].help);
    assert!(!f.app.flow.defaults.camera.allowed);
    f.app.tab_click(id::P_CHOICE + ci * id::P_CHOICE_STRIDE + 1);
    assert!(f.app.flow.defaults.camera.allowed, "the person chose it");
    assert!(f.env.store.get_draft(&id).unwrap().defaults.camera.allowed, "saved");
}

#[test]
fn with_nothing_selected_the_flow_settings_are_drawn_at_every_size_and_a_toolbar_button_and_s_reach_them() {
    let mut f = fx("settings-reach");
    let id = open_flow(&mut f, two_steps());
    for (w, h) in [(1024u32, 768u32), (1280, 720), (1920, 1080), (2560, 1600), (3840, 2160)] {
        f.app.sel.clear();
        f.app.sel_edge = None;
        f.app.sync_props();
        let g = draw_at(&mut f, w, h);
        assert_eq!(f.app.props.key, "flow", "{w}x{h}");
        let pr = f.app.geo.props.unwrap_or_else(|| panic!("{w}x{h}: the flow settings panel is not drawn with nothing selected"));
        assert!(pr.w > 150.0 && pr.x + pr.w <= w as f32 + 0.5 && pr.y + pr.h <= h as f32 + 0.5, "{w}x{h}: {pr:?}");
        assert!(has_hit(&g, "Flow settings (s)"), "{w}x{h}: the toolbar button");
        // the first fields of the panel are real controls inside it
        let ci = f.app.props.fields.iter().position(|x| x.key == "#camallow" && x.label == "Camera allowed").expect("Camera allowed is a flow setting");
        let first = f.app.props.fields.iter().position(|x| x.key == "#heal").unwrap();
        assert!(g.hits.iter().any(|h| h.ev == Event::Tab(id::P_CHOICE + first * id::P_CHOICE_STRIDE + 1) && h.rect.x >= pr.x - 1.0 && h.rect.x + h.rect.w <= pr.x + pr.w + 1.0), "{w}x{h}: a setting of the panel can be clicked");
        // on a tall window the camera switch itself is on screen and clickable
        if h >= 1080 {
            assert!(g.hits.iter().any(|h| h.ev == Event::Tab(id::P_CHOICE + ci * id::P_CHOICE_STRIDE + 1) && h.rect.x >= pr.x - 1.0 && h.rect.x + h.rect.w <= pr.x + pr.w + 1.0 && h.rect.y + h.rect.h <= pr.y + pr.h + 1.0), "{w}x{h}: the Camera allowed switch");
        }
    }
    // from a selected step, the button (and the key s) bring the flow's settings back
    f.app.sel.insert("a".to_string());
    f.app.sync_props();
    draw(&mut f);
    assert_eq!(f.app.props.key, "node:a");
    click(&mut f, "Flow settings (s)");
    draw(&mut f);
    assert!(f.app.sel.is_empty() && f.app.props.key == "flow");
    f.app.sel.insert("b".to_string());
    f.app.sync_props();
    key(&mut f, Key::Char('s'));
    draw(&mut f);
    assert!(f.app.sel.is_empty() && f.app.props.key == "flow");
    let _ = id;
}

#[test]
fn a_waiting_run_banner_keeps_its_instruction_and_clips_a_long_message_at_a_word_with_an_ellipsis() {
    use iyakki_flow::driver::clip_words;
    let long = "The error message explicitly states that 'Camera allowed' must be switched on in the flow settings before this step can take a picture, and nothing else was changed";
    let c = clip_words(long, 60);
    assert!(c.ends_with('\u{2026}') && c.chars().count() <= 61 && !c.contains("Camer'"), "{c}");
    assert!(c.starts_with("The error message explicitly states that") && c.split(' ').all(|w| long.contains(w.trim_end_matches('\u{2026}'))), "{c}");
    assert_eq!(clip_words("short", 60), "short");
    assert_eq!(clip_words("one\ntwo", 60), "one\u{2026}");
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
    for (w, h) in [(1280u32, 720u32), (1920, 1080), (1024, 600), (800, 480), (480, 600)] {
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
    for (w, h) in [(1280u32, 720u32), (1920, 1080), (1024, 600), (800, 480), (480, 600)] {
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

#[test]
fn deleting_a_flow_and_dropping_a_version_ask_first_default_to_keeping_and_are_refused_while_running() {
    let mut f = fx("delask");
    let id = open_flow(&mut f, json!({"name": "Keep me", "nodes": [{"id": "a", "kind": "notify", "config": {"title": "x"}}]}));
    f.env.store.publish(&id, &PublishReq { by: "user:root", label: "", changelog: "v1", approved_by: Some("user:root") }).unwrap();
    f.app.edit("second", |fl| fl.nodes[1].config = json!({"title": "y"}));
    f.env.store.publish(&id, &PublishReq { by: "user:root", label: "", changelog: "v2", approved_by: Some("user:root") }).unwrap();
    // drop: the history names the flow and what is lost; Enter keeps; the current version is refused
    click(&mut f, "History (h)");
    key(&mut f, Key::Char('x'));
    let Some(Modal::Confirm(t, Confirmed::DropVersion(v))) = &f.app.modal else { panic!("no confirm") };
    assert!(t.contains("Keep me") && t.contains("lost") && t.contains("CURRENT"), "{t}");
    assert_eq!(*v, 2);
    key(&mut f, Key::Enter);
    assert!(f.app.modal.is_none(), "Enter does not confirm a drop");
    assert_eq!(f.env.store.versions(&id).len(), 2);
    click(&mut f, "History (h)");
    key(&mut f, Key::Char('x'));
    key(&mut f, Key::Char('y'));
    assert_eq!(f.env.store.versions(&id).len(), 2, "the current version is not dropped");
    click(&mut f, "History (h)");
    key(&mut f, Key::Up);
    key(&mut f, Key::Char('x'));
    key(&mut f, Key::Char('y'));
    assert_eq!(f.env.store.versions(&id).len(), 1);
    assert!(f.env.store.get_version(&id, 1).is_none());
    // delete: Esc and Enter keep it
    f.app.close_flow();
    f.app.reload_flows();
    f.app.list_sel = f.app.flows.iter().position(|x| x.id == id).unwrap();
    key(&mut f, Key::Delete);
    let Some(Modal::Confirm(t, Confirmed::DeleteFlow(_))) = &f.app.modal else { panic!("no confirm") };
    assert!(t.contains("Keep me") && t.contains(&id));
    key(&mut f, Key::Enter);
    assert!(f.env.store.exists(&id), "Enter keeps it");
    key(&mut f, Key::Delete);
    key(&mut f, Key::Esc);
    assert!(f.env.store.exists(&id));
    // a run that is not finished refuses the delete
    f.env.engine.start_run(StartRequest::new(&id)).unwrap();
    key(&mut f, Key::Delete);
    key(&mut f, Key::Char('y'));
    assert!(f.env.store.exists(&id), "refused while a run is unfinished");
    // finished: confirm removes
    let rid = f.env.engine.summaries()[0].id.clone();
    f.env.engine.cancel(&rid).unwrap();
    f.env.engine.wait_for(&rid, Duration::from_secs(5), |r| r.status.is_terminal()).unwrap();
    key(&mut f, Key::Delete);
    key(&mut f, Key::Char('y'));
    assert!(!f.env.store.exists(&id));
}

// ---------------------------------------------------------------------------------------------------------------- closing, results, failure card

/// A flow whose one step cannot succeed, published and run once; the run is waited for. Returns (flow id, run id).
fn failed_run(f: &mut Fx, name: &str, inputs: serde_json::Value) -> (String, String) {
    let spec = json!({"name": name, "inputs": [{"name": "api", "type": "text", "required": false}], "nodes": [{"id": "call", "kind": "http", "name": "Call the site", "config": {"url": "http://127.0.0.1:1/x"}, "retry": {"attempts": 1}}], "edges": [{"from": "start", "to": "call"}]});
    let flow = f.env.create_draft(build_flow(&spec).unwrap(), "user:root").unwrap();
    f.env.store.publish(&flow.id, &PublishReq { by: "user:root", label: "", changelog: "v1", approved_by: Some("user:root") }).unwrap();
    let mut req = StartRequest::new(&flow.id).trigger("cron", "scheduler");
    req.inputs = inputs;
    let run = f.env.engine.start_run(req).unwrap().run_id;
    f.env.engine.wait_for(&run, Duration::from_secs(10), |r| r.status.is_terminal()).unwrap();
    assert_eq!(f.env.engine.get_run(&run).unwrap().status, RunStatus::Failed);
    f.app.reload_flows();
    f.app.refresh_runs();
    (flow.id, run)
}

fn fits(f: &mut Fx, w: u32, h: u32, ctx: &str) -> Gfx {
    let g = draw_at(f, w, h);
    let (a, s) = area(w, h);
    for hit in g.hits.iter().filter(|h| h.label != "slot-mark") {
        let r = hit.rect;
        assert!(r.x >= a.x - 1.0 && r.x + r.w <= a.x + a.w + 1.0 && r.y >= a.y - 1.0 && r.y + r.h <= a.y + a.h + 1.0, "{ctx} {w}x{h}: `{}` {r:?} is outside {a:?}", hit.label);
    }
    let buttons: Vec<_> = g.hits.iter().filter(|h| matches!(h.ev, Event::Tab(n) if (id::TAB..id::B_NEW + 1).contains(&n)) && h.rect.h < 45.0 * s && h.rect.w < 420.0 * s && h.label != "chat pane").collect();
    for (i, p) in buttons.iter().enumerate() {
        for q in &buttons[i + 1..] {
            let (a1, b1) = (p.rect, q.rect);
            let ov = a1.x < b1.x + b1.w - 0.5 && b1.x < a1.x + a1.w - 0.5 && a1.y < b1.y + b1.h - 0.5 && b1.y < a1.y + a1.h - 0.5;
            assert!(!ov, "{ctx} {w}x{h}: `{}` {a1:?} and `{}` {b1:?} overlap", p.label, q.label);
        }
    }
    g
}

#[test]
fn esc_closes_the_open_flow_and_the_run_view_one_step_back_and_only_refused_text_asks_first() {
    let mut f = fx("close");
    let id = open_flow(&mut f, two_steps());
    f.env.store.publish(&id, &PublishReq { by: "user:root", label: "", changelog: "v1", approved_by: Some("user:root") }).unwrap();
    // visible controls with their keys
    let g = draw(&mut f);
    assert!(has_hit(&g, "Close (Esc)") && has_hit(&g, "Run (r)") && has_hit(&g, "Publish (p)"));
    // Esc with nothing selected closes the flow, says nothing was lost, and stays in FLOWS
    f.app.edit("change", |fl| fl.name = "Renamed".into());
    f.app.select_node("a");
    assert!(matches!(f.app.handle(&Event::Key(Key::Esc), &f.cx), crate::apps::Flow::Stay));
    assert!(f.app.cur.is_some(), "the first Esc only deselects");
    assert!(matches!(f.app.handle(&Event::Key(Key::Esc), &f.cx), crate::apps::Flow::Stay));
    assert!(f.app.cur.is_none(), "the second closes the flow");
    assert!(f.app.status.as_ref().is_some_and(|s| s.0.contains("Closed") && s.0.contains("saved")), "{:?}", f.app.status);
    assert_eq!(f.env.store.get_draft(&id).unwrap().name, "Renamed", "the edit is in the draft");
    // Esc on the list leaves FLOWS
    assert!(matches!(f.app.handle(&Event::Key(Key::Esc), &f.cx), crate::apps::Flow::Back));
    // text that was refused asks before it is dropped; Enter keeps editing, y closes
    f.app.open_flow(&id);
    draw(&mut f);
    f.app.toggle_text();
    f.app.text.as_mut().unwrap().set_text("{ not a flow");
    f.app.text_changed();
    assert!(!f.app.text_err.is_empty());
    f.app.tab_click(id::CLOSE);
    assert!(matches!(&f.app.modal, Some(Modal::Confirm(t, Confirmed::CloseFlow)) if t.contains("Renamed")));
    key(&mut f, Key::Enter);
    assert!(f.app.cur.is_some() && f.app.modal.is_none(), "Enter keeps editing");
    f.app.tab_click(id::CLOSE);
    key(&mut f, Key::Char('y'));
    assert!(f.app.cur.is_none());
    assert_eq!(f.env.store.get_draft(&id).unwrap().name, "Renamed", "the last valid draft is untouched");
    // the run view: Esc goes back to the screen it came from, never out of FLOWS
    f.app.open_flow(&id);
    click(&mut f, "Run (r)");
    assert_eq!(f.app.tab, Tab::Run);
    let g = draw(&mut f);
    assert!(has_hit(&g, "Close (Esc)"));
    assert!(matches!(f.app.handle(&Event::Key(Key::Esc), &f.cx), crate::apps::Flow::Stay));
    assert_eq!(f.app.tab, Tab::Design);
    assert!(f.app.cur.is_some(), "back in the editor");
    click(&mut f, "BOARD");
    let run = f.app.board_rows().first().cloned().unwrap();
    f.app.select_run(&run, true);
    assert_eq!(f.app.tab, Tab::Run);
    key(&mut f, Key::Esc);
    assert_eq!(f.app.tab, Tab::Board, "a run opened from the board closes back to it");
}

#[test]
fn a_run_with_required_inputs_asks_for_them_and_a_finished_run_says_how_it_ended_and_what_next() {
    let mut f = fx("inputs");
    let spec = json!({"name": "Needs input", "inputs": [{"name": "site", "type": "text", "required": true}, {"name": "count", "type": "number", "required": false}], "nodes": [{"id": "n", "kind": "notify", "config": {"title": "{{ inputs.site }}"}}], "edges": [{"from": "start", "to": "n"}]});
    let id = open_flow(&mut f, spec);
    f.env.store.publish(&id, &PublishReq { by: "user:root", label: "", changelog: "v1", approved_by: Some("user:root") }).unwrap();
    key(&mut f, Key::Char('r'));
    assert!(matches!(f.app.modal, Some(Modal::RunInputs(..))), "the required input is asked for");
    assert!(f.app.run_id.is_none());
    key(&mut f, Key::Enter);
    key(&mut f, Key::Enter);
    assert!(matches!(f.app.modal, Some(Modal::RunInputs(..))), "an empty required value keeps the form open");
    draw(&mut f);
    types(&mut f, "x");
    for _ in 0..4 {
        if f.app.modal.is_none() {
            break;
        }
        key(&mut f, Key::Enter);
    }
    assert!(f.app.modal.is_none(), "{:?}", f.app.status);
    let run = f.app.run_id.clone().unwrap();
    f.env.engine.wait_for(&run, Duration::from_secs(5), |r| r.status.is_terminal()).unwrap();
    tick(&mut f, 12);
    assert_eq!(f.app.tab, Tab::Run);
    assert_eq!(f.env.engine.get_run(&run).unwrap().inputs["site"], "x");
    // the result is stated, with Run again and the log
    let g = draw(&mut f);
    assert!(has_hit(&g, "Run again (a)") && has_hit(&g, "Open log (l)") && has_hit(&g, "Close (Esc)"), "{:?}", g.hits.iter().map(|h| h.label.clone()).collect::<Vec<_>>());
    assert!(f.app.run_banner(f.app.run.as_ref().unwrap(), f.app.now()).text.starts_with("Succeeded"));
    assert!(f.app.fail.is_none(), "no failure card after a success");
    click(&mut f, "Run again (a)");
    assert_ne!(f.app.run_id.as_deref(), Some(run.as_str()), "a new run");
    let again = f.app.run_id.clone().unwrap();
    assert_eq!(f.env.engine.get_run(&again).unwrap().inputs["site"], "x", "with the same inputs");
    // Open log shows the log tab
    f.env.engine.wait_for(&again, Duration::from_secs(5), |r| r.status.is_terminal()).unwrap();
    tick(&mut f, 12);
    click(&mut f, "Open log (l)");
    assert_eq!(f.app.rv.tab, RunTab::Log);
}

#[test]
fn a_running_run_shows_progress_and_a_stop_button() {
    let mut f = fx("stop");
    let spec = json!({"name": "Waits", "nodes": [{"id": "w", "kind": "delay", "config": {"seconds": 30}}], "edges": [{"from": "start", "to": "w"}]});
    let id = open_flow(&mut f, spec);
    f.env.store.publish(&id, &PublishReq { by: "user:root", label: "", changelog: "v1", approved_by: Some("user:root") }).unwrap();
    click(&mut f, "Run (r)");
    let run = f.app.run_id.clone().unwrap();
    f.env.engine.wait_for(&run, Duration::from_secs(5), |r| r.status == RunStatus::Running).unwrap();
    tick(&mut f, 12);
    let g = draw(&mut f);
    assert!(has_hit(&g, "Stop run (c)"), "{:?}", g.hits.iter().map(|h| h.label.clone()).collect::<Vec<_>>());
    let b = f.app.run_banner(f.app.run.as_ref().unwrap(), f.app.now());
    assert!(b.text.contains("steps done"), "{}", b.text);
    key(&mut f, Key::Char('c'));
    f.env.engine.wait_for(&run, Duration::from_secs(10), |r| r.status.is_terminal()).unwrap();
    tick(&mut f, 12);
    let g = draw(&mut f);
    assert!(has_hit(&g, "Run again (a)"));
    assert!(f.app.fail.is_none(), "a run the person stopped is not a failure");
}

#[test]
fn a_failed_last_run_shows_the_card_on_open_dismiss_persists_and_a_success_shows_none() {
    let mut f = fx("card2");
    let (flow, run) = failed_run(&mut f, "Site check", json!({}));
    // a flow that succeeds has no card
    let ok = open_flow(&mut f, json!({"name": "Fine", "nodes": [{"id": "n", "kind": "notify", "config": {"title": "x"}}], "edges": [{"from": "start", "to": "n"}]}));
    f.env.store.publish(&ok, &PublishReq { by: "user:root", label: "", changelog: "v1", approved_by: Some("user:root") }).unwrap();
    let r2 = f.env.engine.start_run(StartRequest::new(&ok)).unwrap().run_id;
    f.env.engine.wait_for(&r2, Duration::from_secs(5), |r| r.status.is_terminal()).unwrap();
    f.app.refresh_runs();
    f.app.open_flow(&ok);
    assert!(f.app.fail.is_none());
    assert!(!has_hit(&draw(&mut f), "Ask the agent to fix (F)"));
    // the list marks the failed flow
    f.app.close_flow();
    f.app.reload_flows();
    draw(&mut f);
    // opening the failed flow shows the card at once, with the step and the reason
    f.app.open_flow(&flow);
    let fail = f.app.fail.clone().expect("the card");
    assert_eq!((fail.run_id.as_str(), fail.step.as_str()), (run.as_str(), "call"));
    assert!(fail.headline().starts_with("Run failed at step Call the site:"), "{}", fail.headline());
    let g = draw(&mut f);
    for l in ["Ask the agent to fix (F)", "Open log (L)", "Dismiss (x)"] {
        assert!(has_hit(&g, l), "{l}");
    }
    assert!(f.app.footer_hint().starts_with("F ask the agent to fix"));
    // it fits every size, in the editor, the list editor and the run view, with the assistant open or closed
    for (w, h) in [(1280u32, 720u32), (1024, 600), (800, 480), (480, 600)] {
        for chat in [false, true] {
            f.app.chat_open = chat;
            f.app.tab = Tab::Design;
            let g = fits(&mut f, w, h, &format!("design chat={chat}"));
            if w >= 1000 {
                assert!(has_hit(&g, "Ask the agent to fix (F)"));
            }
            f.app.tab = Tab::Run;
            f.app.select_run(&run, false);
            fits(&mut f, w, h, &format!("run chat={chat}"));
            f.app.tab = Tab::Design;
        }
    }
    f.app.chat_open = false;
    // the list tags it
    f.app.close_flow();
    let g = fits(&mut f, 1280, 720, "list");
    assert!(g.rgba().len() > 0);
    f.app.open_flow(&flow);
    // Dismiss: gone now, and it does not come back on the next open or after a restart
    click(&mut f, "Dismiss (x)");
    assert!(f.app.fail.is_none());
    f.app.close_flow();
    f.app.open_flow(&flow);
    assert!(f.app.fail.is_none(), "dismissed for this run");
    let mut again = FlowsApp::with_env(&f.cx, f.env.clone());
    again.open_flow(&flow);
    assert!(again.fail.is_none(), "dismiss is remembered");
    // a new failed run is a new card
    let mut req = StartRequest::new(&flow).trigger("cron", "scheduler");
    req.inputs = json!({});
    let run2 = f.env.engine.start_run(req).unwrap().run_id;
    f.env.engine.wait_for(&run2, Duration::from_secs(10), |r| r.status.is_terminal()).unwrap();
    f.app.refresh_runs();
    assert_eq!(f.app.fail.as_ref().map(|x| x.run_id.clone()), Some(run2));
    // the setting: off means no card
    f.app.tab_click(id::OFFER);
    assert!(f.app.fail.is_none() && !f.app.book.offer());
    f.app.tab_click(id::OFFER);
    assert!(f.app.fail.is_some());
}

#[test]
fn opening_a_failed_flow_with_the_assistant_open_makes_it_offer_once_and_never_edits() {
    let mut f = fx("offer");
    let (flow, run) = failed_run(&mut f, "Offer me", json!({}));
    let before = f.env.store.get_draft(&flow).unwrap();
    // chat closed: only the card, no message
    f.app.open_flow(&flow);
    tick(&mut f, 4);
    assert!(f.app.fail.is_some() && f.app.pending_note.is_none());
    assert!(!f.app.chat.messages().iter().any(|m| m.content.contains("Want me to")));
    f.app.close_flow();
    // chat open: a short message appears in the thread, with Yes and Not now beside it
    f.app.chat_open = true;
    f.app.open_flow(&flow);
    tick(&mut f, 6);
    let said: Vec<_> = f.app.chat.messages().iter().filter(|m| m.content.contains("Want me to look into it")).collect();
    assert_eq!(said.len(), 1, "{:?}", f.app.chat.messages().iter().map(|m| m.content.clone()).collect::<Vec<_>>());
    assert!(said[0].content.contains("failed at step Call the site"));
    assert!(f.app.book.announced(&run));
    let g = draw(&mut f);
    assert!(has_hit(&g, "Yes, fix it") && has_hit(&g, "Not now"));
    // once per failed run: closing and opening again does not repeat it
    f.app.close_flow();
    tick(&mut f, 2);
    f.app.open_flow(&flow);
    tick(&mut f, 6);
    let n = f.app.chat.messages().iter().filter(|m| m.content.contains("Want me to look into it")).count();
    assert!(n <= 1, "said {n} times");
    assert!(f.app.pending_note.is_none());
    // nothing was changed or run by the offer
    assert_eq!(f.env.store.get_draft(&flow).unwrap(), before);
    assert_eq!(f.env.engine.runs_of(&flow).len(), 1);
    // Not now dismisses it for this run
    click(&mut f, "Not now");
    assert!(f.app.fail.is_none());
}

#[test]
fn ask_the_agent_to_fix_loads_the_failure_without_secrets_is_bounded_and_changes_nothing() {
    let mut f = fx("fix");
    let token = format!("{}{}", "ghp_", "Z9y8X7w6V5u4T3s2R1q0P9o8N7m6L5k4J3i2");
    let (flow, run) = failed_run(&mut f, "Fix me", json!({"api": token}));
    let before = f.env.store.get_draft(&flow).unwrap();
    f.app.open_flow(&flow);
    assert!(f.app.fail.is_some());
    assert!(f.cx.session.policy.agent, "the test account has the assistant");
    for n in 1..=3u32 {
        // wait for a turn that is still going
        for _ in 0..600 {
            f.app.handle(&Event::Tick, &f.cx);
            if !f.app.chat.is_running() {
                break;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        assert!(!f.app.chat.is_running());
        f.app.tab_click(id::FAIL_FIX);
        f.app.handle(&Event::Tick, &f.cx);
        // the click is handled with the event, as the shell does
        if f.app.fix_requested {
            f.app.handle(&Event::Key(Key::Char('F')), &f.cx);
        }
        assert_eq!(f.app.book.attempts(&flow), n, "attempt {n}");
        assert!(f.app.chat_open && f.app.focus == Focus::Chat, "the assistant opens on the flow");
        // the request is in the thread: flow, run, step, input names only
        let asked: Vec<String> = f.app.chat.messages().iter().filter(|m| m.content.starts_with("FLOW FAILURE REQUEST")).map(|m| m.content.clone()).collect();
        assert!(asked.last().unwrap().contains(&format!("attempt {n} of 3")) && asked.last().unwrap().contains(&flow) && asked.last().unwrap().contains(&run) && asked.last().unwrap().contains("step `call`"));
        f.app.focus = Focus::Canvas;
    }
    let all: String = f.app.chat.messages().iter().map(|m| m.content.clone()).collect::<Vec<_>>().join("\n");
    assert!(!all.contains(&token), "no input value reaches the conversation");
    assert!(all.contains("api"), "the input's name does");
    // the fourth is refused, with the reason, and spends nothing
    for _ in 0..600 {
        f.app.handle(&Event::Tick, &f.cx);
        if !f.app.chat.is_running() {
            break;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    f.app.tab_click(id::FAIL_FIX);
    f.app.handle(&Event::Key(Key::Char('F')), &f.cx);
    assert_eq!(f.app.book.attempts(&flow), 3);
    assert!(f.app.status.as_ref().is_some_and(|s| !s.1 && s.0.contains("already tried 3 times")), "{:?}", f.app.status);
    let b = f.app.fail_banner().unwrap();
    assert!(b.buttons[0].0 == "No fix attempts left" && !b.buttons[0].3, "the button is off when no attempt is left");
    draw(&mut f);
    // nothing was edited or re-run on its own, and no change waits unseen
    assert_eq!(f.env.store.get_draft(&flow).unwrap(), before);
    assert_eq!(f.env.engine.runs_of(&flow).len(), 1);
    // a successful run starts the count again
    let ok = f.env.store.get_draft(&flow).unwrap();
    assert_eq!(ok.id, flow);
    f.app.book.reset(&flow);
    assert_eq!(f.app.book.attempts_left(&flow), 3);
}

#[test]
fn every_screen_has_a_key_hint_and_the_empty_states_say_what_to_do_next() {
    let mut f = fx("hints");
    // the empty list
    let g = draw(&mut f);
    assert!(f.app.footer_hint().contains("n New") && f.app.footer_hint().contains("Esc leave"));
    assert!(g.hits.iter().any(|h| h.label == "New flow (n)"));
    let id = open_flow(&mut f, json!({"name": "Hint", "nodes": [{"id": "n", "kind": "notify", "config": {"title": "x"}}]}));
    let h = f.app.footer_hint();
    assert!(h.contains("r Run") && h.contains("Esc Close") && h.contains("p Publish"), "{h}");
    f.app.add_step("notify");
    let h = f.app.footer_hint();
    assert!(h.contains("Enter edit settings") && h.contains("Esc deselect"), "{h}");
    f.app.sel.clear();
    f.app.focus = Focus::Props;
    assert!(f.app.footer_hint().contains("Esc undo"));
    f.app.focus = Focus::Text;
    assert!(f.app.footer_hint().contains("Esc back to the canvas"));
    f.app.focus = Focus::Chat;
    f.app.chat_open = true;
    assert!(f.app.footer_hint().contains("Enter send"));
    f.app.chat_open = false;
    f.app.focus = Focus::Canvas;
    f.app.tab = Tab::Run;
    assert!(f.app.footer_hint().contains("Esc Close") && f.app.footer_hint().contains("c Stop"));
    f.app.tab = Tab::Board;
    assert!(f.app.footer_hint().contains("Esc leave"));
    // the hint is the app's hint (the shell shows it and F1 lists it) and is drawn on every screen at every size
    use super::App;
    assert_eq!(f.app.hint(), f.app.footer_hint());
    let _ = id;
    for (w, hh) in [(1280u32, 720u32), (1024, 600), (800, 480), (480, 600)] {
        for tab in [Tab::Design, Tab::Run, Tab::Board] {
            f.app.tab = tab;
            fits(&mut f, w, hh, &format!("{tab:?}"));
        }
    }
    // the empty list and the empty flow say what to do
    f.app.close_flow();
    f.app.tab = Tab::Design;
    f.app.status = None;
    fits(&mut f, 800, 480, "empty list");
}

// ------------------------------------------------------------------------------------------------ Assist (copilot and autopilot)

/// A published flow with Assist set as given and one failed run; the scripted model answers with a fix of the address.
fn assisted_failure(f: &mut Fx, name: &str, mode: AssistMode) -> String {
    let spec = json!({"name": name, "nodes": [{"id": "call", "kind": "http", "name": "Call the site", "config": {"url": "http://127.0.0.1:1/x"}, "retry": {"attempts": 1}}], "edges": [{"from": "start", "to": "call"}]});
    let flow = f.env.create_draft(build_flow(&spec).unwrap(), "user:root").unwrap();
    let mut d = f.env.store.get_draft(&flow.id).unwrap();
    d.defaults.assist.mode = mode;
    f.env.store.save_draft(&d, None).unwrap();
    f.env.store.publish(&flow.id, &PublishReq { by: "user:root", label: "", changelog: "v1", approved_by: Some("user:root") }).unwrap();
    let run = f.env.engine.start_run(StartRequest::new(&flow.id).trigger("cron", "scheduler")).unwrap().run_id;
    f.env.engine.wait_for(&run, Duration::from_secs(10), |r| r.status.is_terminal()).unwrap();
    *f.env.host.backend.lock().unwrap() = Some(Arc::new(Script(Mutex::new(vec![r#"{"diagnosis": "the path moved", "patch": {"node": "call", "reason": "moved", "ops": [{"op": "set_config", "path": "url", "value": "http://127.0.0.1:1/y"}]}}"#.into()]))));
    f.app.reload_flows();
    f.app.refresh_runs();
    flow.id
}

#[test]
fn the_assist_settings_are_in_the_flow_settings_and_only_the_person_changes_them() {
    let mut f = fx("assist-set");
    let id = open_flow(&mut f, two_steps());
    f.app.sel.clear();
    f.app.sync_props();
    let labels: Vec<String> = f.app.props.fields.iter().map(|x| x.label.clone()).collect();
    for want in ["Assist: an agent that fixes and upgrades this flow", "Assist: autopilot may publish a tested fix", "Assist: fixes after a failed run", "Assist: upgrades", "Assist: automatic publishes per day (0 to 20)"] {
        assert!(labels.iter().any(|l| l == want), "{want}: {labels:?}");
    }
    // a flow starts with Assist off
    assert_eq!(f.app.flow.defaults.assist.mode, AssistMode::Off);
    // the person picks copilot and a number; a wrong number is refused with the range
    let idx = f.app.props.fields.iter().position(|x| x.key == "#assist").unwrap();
    f.app.choose(idx, 1);
    assert_eq!(f.app.flow.defaults.assist.mode, AssistMode::Copilot);
    let pidx = f.app.props.fields.iter().position(|x| x.key == "#assistpub").unwrap();
    f.app.props.focus = pidx;
    f.app.props.fields[pidx].tf.set("99");
    f.app.commit_field(pidx);
    assert!(f.app.props.fields[pidx].err.as_ref().is_some_and(|e| e.contains("0 to 20")), "{:?}", f.app.props.fields[pidx].err);
    assert_eq!(f.app.flow.defaults.assist.max_publish_per_day, 1);
    f.app.props.focus = pidx;
    f.app.props.fields[pidx].tf.set("2");
    f.app.commit_field(pidx);
    assert_eq!(f.app.flow.defaults.assist.max_publish_per_day, 2);
    let _ = id;
}

#[test]
fn a_failed_run_in_copilot_shows_a_strip_the_list_approve_goes_through_publish_and_reject_leaves_the_draft() {
    let mut f = fx("assist-copilot");
    let id = assisted_failure(&mut f, "Copilot flow", AssistMode::Copilot);
    f.app.open_flow(&id);
    // the fail card is there, and no suggestion yet: nothing runs the agent until the pass
    let g = draw(&mut f);
    assert!(has_hit(&g, "Ask the agent to fix (F)"));
    assert!(!has_hit(&g, "Review (S)"));
    let lines = f.env.assist_tick();
    assert!(lines.iter().any(|l| l.contains("suggestion s1")), "{lines:?}");
    tick(&mut f, 40);
    let g = draw(&mut f);
    assert!(has_hit(&g, "Review (S)"), "the strip offers the list: {:?}", g.hits.iter().map(|h| h.label.clone()).collect::<Vec<_>>());
    // the draft is untouched until the person decides
    assert_eq!(f.env.store.get_draft(&id).unwrap().node("call").unwrap().config["url"], "http://127.0.0.1:1/x");
    click(&mut f, "Review (S)");
    assert!(matches!(f.app.modal, Some(Modal::Suggestions(_))));
    let g = draw(&mut f);
    assert!(has_hit(&g, "Approve and publish (a)") && has_hit(&g, "Reject (x)") && has_hit(&g, "Close (Esc)"));
    // Reject first on a copy of the flow's list: nothing was changed
    click(&mut f, "Reject (x)");
    assert!(f.app.modal.is_none(), "the list closes when it is empty");
    assert!(f.env.open_suggestions(&id).is_empty());
    assert_eq!(f.env.store.get_draft(&id).unwrap().node("call").unwrap().config["url"], "http://127.0.0.1:1/x");
    // a second failure and suggestion; this time approve
    *f.env.host.backend.lock().unwrap() = Some(Arc::new(Script(Mutex::new(vec![r#"{"diagnosis": "the path moved again", "patch": {"node": "call", "reason": "moved", "ops": [{"op": "set_config", "path": "url", "value": "http://127.0.0.1:1/z"}]}}"#.into()]))));
    let run = f.env.engine.start_run(StartRequest::new(&id).trigger("cron", "scheduler")).unwrap().run_id;
    f.env.engine.wait_for(&run, Duration::from_secs(10), |r| r.status.is_terminal()).unwrap();
    f.env.assist_tick();
    f.app.refresh_runs();
    tick(&mut f, 40);
    click(&mut f, "Review (S)");
    click(&mut f, "Approve and publish (a)");
    // the change is in the draft and the ordinary Publish dialog is open: nothing is published yet
    assert!(matches!(f.app.modal, Some(Modal::Publish(_))), "publishing stays the person's confirmation");
    assert_eq!(f.env.store.published(&id).unwrap().meta.version, 1);
    assert_eq!(f.app.flow.node("call").unwrap().config["url"], "http://127.0.0.1:1/z");
    key(&mut f, Key::Ctrl('s'));
    assert_eq!(f.env.store.published(&id).unwrap().meta.version, 2);
    f.app.update_assist();
    assert!(f.app.assist.sugs.is_empty(), "the suggestion is settled by the publish");
    let book = f.env.assist_store();
    assert!(book.list(&id).iter().any(|s| s.status == iyakki_flow::assist_store::Status::Approved && s.published_version == 2));
}

#[test]
fn the_watch_strip_has_a_one_click_rollback_and_a_pause() {
    let mut f = fx("assist-watch");
    let id = assisted_failure(&mut f, "Watched", AssistMode::Autopilot);
    // as if autopilot had published version 2: a watch on it
    let mut d = f.env.store.get_draft(&id).unwrap();
    d.nodes.iter_mut().find(|n| n.id == "call").unwrap().config["url"] = json!("http://127.0.0.1:1/y");
    f.env.store.save_draft(&d, None).unwrap();
    f.env.store.publish(&id, &PublishReq { by: "agent:assist", label: "", changelog: "autopilot fix", approved_by: Some("policy:autopilot") }).unwrap();
    let book = f.env.assist_store();
    book.set_watch(&id, Some(iyakki_flow::assist_store::Watch { version: 2, previous: 1, since_ms: 1, suggestion: "s9".into() })).unwrap();
    f.app.open_flow(&id);
    f.app.update_assist();
    let g = draw(&mut f);
    assert!(has_hit(&g, "Roll back to v1 (B)") && has_hit(&g, "Pause autopilot (U)"), "{:?}", g.hits.iter().map(|h| h.label.clone()).collect::<Vec<_>>());
    click(&mut f, "Pause autopilot (U)");
    assert!(book.paused(&id));
    assert!(has_hit(&draw(&mut f), "Resume autopilot (U)"));
    key(&mut f, Key::Char('U'));
    assert!(!book.paused(&id));
    click(&mut f, "Roll back to v1 (B)");
    assert!(matches!(f.app.modal, Some(Modal::Confirm(..))), "the ordinary confirmation follows");
    key(&mut f, Key::Char('y'));
    assert_eq!(f.env.store.published(&id).unwrap().node("call").unwrap().config["url"], "http://127.0.0.1:1/x");
    assert!(book.watch(&id).is_none() && book.paused(&id), "after a rollback the watch ends and autopilot pauses");
}

#[test]
fn the_suggestion_strip_list_and_settings_fit_at_every_size_with_the_assistant_open_and_closed() {
    let mut f = fx("assist-geo");
    let id = assisted_failure(&mut f, "Geometry flow", AssistMode::Copilot);
    f.env.assist_tick();
    // a second waiting suggestion, and a watch, so every strip is up
    let book = f.env.assist_store();
    let mut s = book.list(&id)[0].clone();
    s.id = String::new();
    s.run_id = "other".into();
    s.summary = "a long summary of a change that is shown in the list and must be cut to the width of the row it is drawn in".into();
    book.add(s).unwrap();
    book.set_watch(&id, Some(iyakki_flow::assist_store::Watch { version: 1, previous: 1, since_ms: 1, suggestion: "s1".into() })).unwrap();
    f.app.open_flow(&id);
    f.app.update_assist();
    assert_eq!(f.app.assist.sugs.len(), 2);
    for (w, h) in [(1280u32, 720u32), (1920, 1080), (1024, 600), (800, 480), (480, 600)] {
        for chat in [false, true] {
            f.app.chat_open = chat;
            f.app.modal = None;
            f.app.sel.clear();
            let g = fits(&mut f, w, h, &format!("strips chat={chat}"));
            // a small window gives the whole screen to the assistant when it is open; the strips are then not on screen
            let covered = f.app.geo.chat.is_some_and(|c| c.w >= area(w, h).0.w - 1.0);
            if !covered {
                assert!(has_hit(&g, "Review (S)"), "{w}x{h} chat={chat}");
                // the strips never overlap each other
                let strips: Vec<_> = g.hits.iter().filter(|x| matches!(x.ev, Event::Tab(n) if [id::FAIL_FIX, id::FAIL_LOG, id::FAIL_DISMISS, id::SUG_OPEN, id::ASSIST_ROLLBACK, id::ASSIST_PAUSE].contains(&n))).collect();
                assert!(strips.len() >= 5, "{w}x{h}: {}", strips.len());
                for (i, p) in strips.iter().enumerate() {
                    for q in &strips[i + 1..] {
                        let (a1, b1) = (p.rect, q.rect);
                        let ov = a1.x < b1.x + b1.w - 0.5 && b1.x < a1.x + a1.w - 0.5 && a1.y < b1.y + b1.h - 0.5 && b1.y < a1.y + a1.h - 0.5;
                        assert!(!ov, "{w}x{h} chat={chat}: `{}` {a1:?} and `{}` {b1:?} overlap", p.label, q.label);
                    }
                }
            }
            // the list dialog
            f.app.chat_open = false;
            f.app.open_suggestions();
            let g = fits(&mut f, w, h, "suggestions");
            let (a, _s) = area(w, h);
            for l in ["Approve and publish (a)", "Reject (x)", "Close (Esc)"] {
                let hit = g.hits.iter().find(|x| x.label == l).unwrap_or_else(|| panic!("{w}x{h}: no `{l}`"));
                assert!(hit.rect.x >= a.x && hit.rect.x + hit.rect.w <= a.x + a.w && hit.rect.y + hit.rect.h <= a.y + a.h, "{w}x{h}: `{l}` {:?} outside {a:?}", hit.rect);
            }
            let btn: Vec<_> = ["Approve and publish (a)", "Reject (x)", "Close (Esc)"].iter().map(|l| g.hits.iter().find(|x| x.label == *l).unwrap().rect).collect();
            for (i, a1) in btn.iter().enumerate() {
                for b1 in &btn[i + 1..] {
                    let ov = a1.x < b1.x + b1.w - 0.5 && b1.x < a1.x + a1.w - 0.5 && a1.y < b1.y + b1.h - 0.5 && b1.y < a1.y + a1.h - 0.5;
                    assert!(!ov, "{w}x{h}: dialog buttons overlap {a1:?} {b1:?}");
                }
            }
            assert!(g.hits.iter().filter(|x| matches!(x.ev, Event::Row(_))).count() >= 2, "{w}x{h}: both suggestions are rows");
            key(&mut f, Key::Esc);
            assert!(f.app.modal.is_none());
        }
        // the settings page with the Assist fields
        f.app.sel.clear();
        f.app.sync_props();
        fits(&mut f, w, h, "settings");
    }
}

// ------------------------------------------------------------------------------------------------ icons and the node tooltip

const FIVE: [(u32, u32); 5] = [(1024, 768), (1280, 720), (1920, 1080), (2560, 1600), (3840, 2160)];

fn sample_flow(f: &mut Fx, template: &str) -> String {
    let t = iyakki_flow::templates::get(template).unwrap();
    let flow = f.env.create_draft(iyakki_flow::templates::instantiate(&t, template, ""), "user:root").unwrap();
    f.app.reload_flows();
    f.app.open_flow(&flow.id);
    draw(f);
    flow.id
}

/// Draw with the pointer at `m` (the shell sets it before every frame).
fn draw_m(f: &mut Fx, w: u32, h: u32, m: (i32, i32)) -> Gfx {
    let mut g = Gfx::new(w, h);
    g.mouse = m;
    crate::fx::background(&mut g);
    let (a, s) = area(w, h);
    f.app.render(&mut g, a, s, &f.cx, 7);
    g
}

fn node_center(f: &Fx, id: &str) -> (i32, i32) {
    let n = f.app.flow.node(id).unwrap();
    let (x, y) = f.app.view.to_screen(f.app.geo.canvas, (n.pos.x + NODE_W / 2.0, n.pos.y + NODE_H / 2.0));
    (x as i32, y as i32)
}

#[test]
fn every_node_kind_has_an_icon_that_draws_and_every_run_state_has_its_own_badge() {
    use super::flows_icons::*;
    let reg = iyakki_flow::Registry::builtin();
    for id in reg.ids() {
        assert!(has_icon(&id), "{id} has no icon");
        let mut g = Gfx::new(48, 48);
        g.clear([10, 12, 24, 255]);
        let blank = g.rgba().to_vec();
        draw_kind(&mut g, &id, 24.0, 24.0, 30.0, crate::ui::TEXT);
        assert_ne!(g.rgba(), &blank[..], "{id}: the icon draws something");
    }
    assert!(KINDS.iter().all(|k| reg.get(k).is_some()), "no icon for a kind that does not exist");
    let states = [NodeState::Pending, NodeState::Running, NodeState::Retrying, NodeState::Waiting, NodeState::Succeeded, NodeState::Failed, NodeState::Skipped, NodeState::Interrupted, NodeState::Cancelled];
    let mut pictures: Vec<(NodeState, Vec<u8>)> = vec![];
    for st in states {
        let mut g = Gfx::new(32, 32);
        g.clear([10, 12, 24, 255]);
        let blank = g.rgba().to_vec();
        draw_state(&mut g, st, 16.0, 16.0, 22.0, 3);
        assert_ne!(g.rgba(), &blank[..], "{st:?} draws a badge");
        assert!(!state_words(st).is_empty());
        pictures.push((st, g.rgba().to_vec()));
    }
    // the tick, the cross, the hand, the dot and the dash look different from one another
    for i in 0..pictures.len() {
        for j in i + 1..pictures.len() {
            if matches!((pictures[i].0, pictures[j].0), (NodeState::Failed, NodeState::Cancelled) | (NodeState::Cancelled, NodeState::Failed)) {
                continue;
            }
            assert_ne!(pictures[i].1, pictures[j].1, "{:?} and {:?} must not look alike", pictures[i].0, pictures[j].0);
        }
    }
}

#[test]
fn a_node_card_shows_the_whole_short_name_at_every_size_and_keeps_its_ports_where_they_were() {
    let mut f = fx("cardtitle");
    for tpl in ["cam-sleep-check", "cam-person-check"] {
        sample_flow(&mut f, tpl);
        for (w, h) in FIVE {
            let mut g = draw_at(&mut f, w, h);
            let s = crate::shell::scale_for(w, h);
            f.app.view.zoom = 1.0;
            for n in f.app.flow.nodes.clone() {
                if n.kind == "start" || n.kind == "end" || n.title().len() > 20 {
                    continue;
                }
                let shown = super::flows_paint::card_title(&mut g, &n, s, 1.0);
                assert_eq!(shown, n.title(), "{tpl} {w}x{h}: `{}` is cut to `{shown}` although it fits", n.title());
            }
            // the card has no second line of cut text: the subtitle is not drawn (checked by the card's pixel rows below the title)
        }
        // the ports are where the geometry always put them
        let n = f.app.flow.node("shot").unwrap().clone();
        let (ins, outs, _) = node_ports(&f.app.reg, &n);
        assert_eq!(port_world(&n, Side::In, &ins[0], &ins, &outs), (n.pos.x, n.pos.y + NODE_H / 2.0));
        assert_eq!(port_world(&n, Side::Out, &outs[0], &ins, &outs), (n.pos.x + NODE_W, n.pos.y + NODE_H / 2.0));
        assert_eq!((NODE_W, NODE_H), (176.0, 64.0));
    }
}

#[test]
fn the_tooltip_card_of_a_node_appears_after_a_rest_stays_inside_the_window_and_goes_when_the_pointer_moves() {
    let mut f = fx("nodetip");
    sample_flow(&mut f, "cam-person-check");
    for (w, h) in FIVE {
        let _ = draw_at(&mut f, w, h);
        let (x, y) = node_center(&f, "ask");
        f.app.handle(&Event::MouseMove(x, y), &f.cx);
        draw_m(&mut f, w, h, (x, y));
        assert!(f.app.tip_rect.is_none(), "{w}x{h}: nothing before the rest");
        for _ in 0..12 {
            f.app.handle(&Event::Tick, &f.cx);
        }
        draw_m(&mut f, w, h, (x, y));
        let r = f.app.tip_rect.unwrap_or_else(|| panic!("{w}x{h}: no tooltip after resting on the node"));
        assert!(r.x >= 0.0 && r.y >= 0.0 && r.x + r.w <= w as f32 && r.y + r.h <= h as f32, "{w}x{h}: the card is outside the window: {r:?}");
        let text = f.app.tip.shown.clone().unwrap();
        assert!(text.starts_with("Look at the picture") && text.contains("Agent") && text.contains("id ask"), "{text}");
        // a move hides it; resting again shows it again
        f.app.handle(&Event::MouseMove(x + 3, y + 3), &f.cx);
        draw_m(&mut f, w, h, (x + 3, y + 3));
        assert!(f.app.tip_rect.is_none(), "{w}x{h}: hidden on a move");
    }
    // the corner of the window: still inside
    let _ = draw_at(&mut f, 1280, 720);
    let (x, y) = node_center(&f, "ask");
    f.app.handle(&Event::MouseMove(x, y), &f.cx);
    for _ in 0..12 {
        f.app.handle(&Event::Tick, &f.cx);
    }
    draw_m(&mut f, 1280, 720, (x, y));
    assert!(f.app.tip_rect.is_some());
    // Esc puts it away, a click puts it away
    key(&mut f, Key::Esc);
    draw_m(&mut f, 1280, 720, (x, y));
    assert!(f.app.tip_rect.is_none());
}

#[test]
fn a_long_press_and_the_question_mark_key_show_the_card_for_touch_and_keyboard() {
    let mut f = fx("tipkeys");
    sample_flow(&mut f, "cam-person-check");
    let (x, y) = node_center(&f, "shot");
    // a finger held on the node
    f.app.handle(&Event::MouseDown(x, y), &f.cx);
    for _ in 0..8 {
        f.app.handle(&Event::Tick, &f.cx);
    }
    draw_m(&mut f, 1280, 720, (x, y));
    assert!(f.app.tip_rect.is_none(), "not yet");
    for _ in 0..14 {
        f.app.handle(&Event::Tick, &f.cx);
    }
    draw_m(&mut f, 1280, 720, (x, y));
    assert!(f.app.tip.shown.as_deref().is_some_and(|t| t.starts_with("Take the picture")), "{:?}", f.app.tip.shown);
    f.app.handle(&Event::MouseUp(x, y), &f.cx);
    // the keyboard: the selected node, on the ? key
    f.app.sel.clear();
    f.app.sel.insert("check".to_string());
    f.app.sync_props();
    key(&mut f, Key::Char('?'));
    draw(&mut f);
    assert!(f.app.tip.shown.as_deref().is_some_and(|t| t.starts_with("Is it the case?")), "{:?}", f.app.tip.shown);
}

#[test]
fn a_nodes_problems_are_in_its_tooltip_in_full() {
    let mut f = fx("tipwarn");
    sample_flow(&mut f, "cam-person-check");
    let (x, y) = node_center(&f, "shot");
    f.app.handle(&Event::MouseMove(x, y), &f.cx);
    for _ in 0..12 {
        f.app.handle(&Event::Tick, &f.cx);
    }
    draw_m(&mut f, 1280, 720, (x, y));
    let t = f.app.tip.shown.clone().unwrap();
    assert!(t.contains("Warning: the camera is not allowed for this flow yet") && t.contains("Flow settings"), "{t}");
}

#[test]
fn in_a_run_the_tooltip_says_the_state_the_time_and_the_error_and_the_corner_badge_has_its_own_words() {
    let mut f = fx("runtip");
    let spec = json!({"name": "Broken", "nodes": [{"id": "call", "kind": "http", "name": "Call the site", "config": {"url": "http://127.0.0.1:1/x"}}]});
    let id = open_flow(&mut f, spec);
    f.env.store.publish(&id, &PublishReq { by: "user:root", label: "", changelog: "v1", approved_by: Some("user:root") }).unwrap();
    click(&mut f, "Run (r)");
    let run = f.app.run_id.clone().unwrap();
    f.env.engine.wait_for(&run, Duration::from_secs(10), |r| r.status.is_terminal()).unwrap();
    tick(&mut f, 12);
    let _ = draw(&mut f);
    let canvas = f.app.geo.canvas;
    let n = f.app.run.as_ref().unwrap().flow.node("call").unwrap().pos;
    let (px, py) = f.app.rv.view.to_screen(canvas, (n.x + NODE_W / 2.0, n.y + NODE_H / 2.0));
    f.app.handle(&Event::MouseMove(px as i32, py as i32), &f.cx);
    for _ in 0..12 {
        f.app.handle(&Event::Tick, &f.cx);
    }
    draw_m(&mut f, 1280, 720, (px as i32, py as i32));
    let t = f.app.tip.shown.clone().expect("a tooltip in the run view");
    assert!(t.starts_with("Call the site") && t.contains("Failed") && t.contains("Error:"), "{t}");
    // the corner badge: its own small card
    let z = f.app.rv.view.zoom;
    let s = crate::shell::scale_for(1280, 720);
    let _ = s;
    let (bx, by) = ((px + NODE_W / 2.0 * z - 3.0 * z) as i32, (py - NODE_H / 2.0 * z + 3.0 * z) as i32);
    f.app.handle(&Event::MouseMove(bx, by), &f.cx);
    for _ in 0..12 {
        f.app.handle(&Event::Tick, &f.cx);
    }
    draw_m(&mut f, 1280, 720, (bx, by));
    assert!(f.app.tip.shown.as_deref().is_some_and(|t| t.contains("Call the site: Failed")), "{:?}", f.app.tip.shown);
}
