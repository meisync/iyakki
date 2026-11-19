//! The SDK tile adapter against fakes: the mapping onto the tile ABI, the events, the core-backed ports with a scripted core.
use iyakki_app_sample::SampleApp;
use iyakki_sdk_tile::convert::{to_tile, ConvertError, MAX_BANDS};
use iyakki_sdk_tile::ports::{CoreDock, CoreStorage, Shared};
use iyakki_sdk_tile::SdkTile;
use iyakki_tile_abi::{limits, Color, Event, Op};
use iyakki_tile_kit::{App, Source};
use iyakki_ui_sdk::canvas::{Hit, Role, TextStyle};
use iyakki_ui_sdk::desktop::{DeskError, DockPort, MemDock};
use iyakki_ui_sdk::geom::Rect;
use iyakki_ui_sdk::list::{DisplayList, Op as S};
use iyakki_ui_sdk::ports::{MemStorage, PortError, StoragePort};
use std::cell::RefCell;
use std::collections::BTreeMap;
use std::rc::Rc;

fn list(ops: Vec<S>) -> DisplayList {
    DisplayList { width: 100.0, height: 100.0, ops }
}

#[test]
fn every_sdk_operation_maps_onto_the_tile_abi() {
    let hit = Hit { action: 6, name: "Go".into(), role: Role::Button };
    let t = to_tile(&list(vec![
        S::Rect(Rect::new(1.4, 2.6, 10.0, 5.0), [1, 2, 3, 255]),
        S::RoundRect(Rect::new(0.0, 0.0, 20.0, 20.0), 4.0, [9, 9, 9, 255]),
        S::Outline(Rect::new(0.0, 0.0, 20.0, 20.0), 6.0, 2.0, [5, 5, 5, 255]),
        S::Line([0.0, 0.0, 10.0, 10.0], 0.2, [7, 7, 7, 255]),
        S::Text { x: 3.0, y: 4.0, size: 13.6, color: [0, 0, 0, 255], style: TextStyle::Bold, text: "hi".into() },
        S::Text { x: 3.0, y: 4.0, size: 13.0, color: [0, 0, 0, 255], style: TextStyle::Mono, text: "".into() },
        S::PushClip(Rect::new(0.0, 0.0, 5.0, 5.0)),
        S::PopClip,
        S::Hit(Rect::new(0.0, 0.0, 30.0, 10.0), hit),
    ]))
    .unwrap();
    assert!(matches!(t.ops[0], Op::Rect { r, color: Color(1, 2, 3, 255) } if (r.x, r.y, r.w, r.h) == (1, 3, 10, 5)), "{:?}", t.ops[0]);
    assert!(matches!(t.ops[1], Op::RoundRect { radius: 4, .. }));
    assert!(matches!(t.ops[2], Op::Outline { thickness: 2, .. }), "the radius is dropped, the width stays");
    assert!(matches!(t.ops[3], Op::Line { thickness: 1, .. }), "a hair line is one pixel");
    assert!(matches!(&t.ops[4], Op::Text { face: 2, size: 14, text, .. } if text == "hi"), "{:?}", t.ops[4]);
    assert!(matches!(t.ops[5], Op::PushClip { .. }) && matches!(t.ops[6], Op::PopClip), "an empty text draws nothing");
    assert!(matches!(t.ops[7], Op::Hit { id: 7, .. }), "the id is the action plus one");
    // the same hit is also a named access node (the list is then ABI 2)
    assert!(matches!(&t.ops[8], Op::Access { id: 7, role: iyakki_tile_abi::Role::Button, name, .. } if !name.is_empty()), "{:?}", t.ops[8]);
    assert_eq!(t.ops.len(), 9);
    assert!(t.unnamed_hits().is_empty(), "no hit is left unnamed");
}

#[test]
fn a_gradient_is_bands_that_cover_the_rectangle_and_a_shadow_is_three_fading_rounded_rectangles() {
    let t = to_tile(&list(vec![S::Gradient(Rect::new(10.0, 20.0, 50.0, 41.0), [0, 0, 0, 255], [200, 100, 50, 255])])).unwrap();
    let bands: Vec<_> = t.ops.iter().map(|o| if let Op::Rect { r, color } = o { (*r, *color) } else { panic!("{o:?}") }).collect();
    assert_eq!(bands.len(), 20);
    assert_eq!(bands[0].0.y, 20);
    assert_eq!(bands.last().map(|b| b.0.y + b.0.h), Some(61), "no gap at the bottom");
    assert!(bands.windows(2).all(|w| w[0].0.y + w[0].0.h == w[1].0.y), "no gap between bands");
    assert!(bands.iter().all(|b| b.0.x == 10 && b.0.w == 50));
    assert!(bands[0].1 .0 < 20 && bands[19].1 .0 > 180 && bands[10].1 .0 > bands[9].1 .0, "dark at the top, light at the bottom, rising");
    let tall = to_tile(&list(vec![S::Gradient(Rect::new(0.0, 0.0, 10.0, 4000.0), [0, 0, 0, 255], [255, 255, 255, 255])])).unwrap();
    assert_eq!(tall.ops.len() as i32, MAX_BANDS);
    let s = to_tile(&list(vec![S::Shadow(Rect::new(20.0, 20.0, 40.0, 40.0), 8.0, 9.0, [0, 0, 0, 200])])).unwrap();
    let rr: Vec<_> = s.ops.iter().map(|o| if let Op::RoundRect { r, radius, color } = o { (r.w, *radius, color.3) } else { panic!() }).collect();
    assert_eq!(rr.len(), 3);
    assert!(rr[0].0 > rr[1].0 && rr[1].0 > rr[2].0 && rr[0].1 > rr[2].1, "growing outwards");
    assert!(rr[0].2 < rr[1].2 && rr[1].2 < rr[2].2, "fainter outwards: {rr:?}");
}

#[test]
fn a_list_the_abi_cannot_hold_is_refused_whole_and_text_is_cut_on_a_character() {
    let many = list((0..limits::MAX_OPS + 1).map(|_| S::PopClip).collect());
    assert_eq!(to_tile(&many), Err(ConvertError::TooManyOps));
    let deep = list((0..17).map(|_| S::PushClip(Rect::new(0.0, 0.0, 5.0, 5.0))).collect());
    assert_eq!(to_tile(&deep), Err(ConvertError::ClipsTooDeep));
    let long = "é".repeat(700);
    let t = to_tile(&list(vec![S::Text { x: 0.0, y: 0.0, size: 9999.0, color: [0, 0, 0, 255], style: TextStyle::Regular, text: long }])).unwrap();
    let Op::Text { size, text, .. } = &t.ops[0] else { panic!() };
    assert_eq!(*size, 512);
    assert!(text.len() <= limits::MAX_TEXT && text.chars().all(|c| c == 'é'));
    assert!(t.encode().is_ok(), "what comes out is something the ABI can encode");
}

#[derive(Default)]
struct Inner {
    store: BTreeMap<String, String>,
    pinned: Vec<String>,
    calls: Vec<(String, String, String)>,
    quota: Option<usize>,
    deny: Vec<&'static str>,
}

/// A scripted core: the `storage` namespace as the bridge has it, and a `dock` namespace as the port asks.
#[derive(Clone, Default)]
struct FakeCore(Rc<RefCell<Inner>>);

impl Source for FakeCore {
    fn call(&mut self, d: &str, m: &str, j: &str) -> Result<String, String> {
        let mut s = self.0.borrow_mut();
        s.calls.push((d.into(), m.into(), j.into()));
        if s.deny.contains(&d) {
            return Err("this tile is not allowed to do that".into());
        }
        let a: serde_json::Value = serde_json::from_str(j).unwrap();
        let k = a["k"].as_str().unwrap_or("").to_string();
        match (d, m) {
            ("storage", "get") => Ok(s.store.get(&k).map_or("null".into(), |v| serde_json::to_string(v).unwrap())),
            ("storage", "set") => {
                let v = a["v"].as_str().unwrap().to_string();
                if s.quota.is_some_and(|q| v.len() > q) {
                    return Err("QuotaExceeded: a value is at most 1 KB".into());
                }
                s.store.insert(k, v);
                Ok("null".into())
            }
            ("storage", "remove") => {
                s.store.remove(&k);
                Ok("null".into())
            }
            ("storage", "list") => Ok(serde_json::to_string(&s.store.keys().collect::<Vec<_>>()).unwrap()),
            ("dock", "pinned") => Ok(serde_json::to_string(&s.pinned).unwrap()),
            ("dock", "pin") => {
                let app = a["app"].as_str().unwrap().to_string();
                s.pinned.push(app);
                Ok("{}".into())
            }
            ("dock", "unpin") => {
                let app = a["app"].as_str().unwrap().to_string();
                s.pinned.retain(|p| *p != app);
                Ok("{}".into())
            }
            ("dock", "status") => Ok("{}".into()),
            _ => Err("no such call".into()),
        }
    }
}

fn tile(core: &FakeCore) -> SdkTile<SampleApp> {
    let door = Shared::new(core.clone());
    let mut t = SdkTile::new(SampleApp::with_storage(Box::new(CoreStorage::new(door.clone()))), Box::new(CoreDock::new(door)));
    t.event(&Event::Resize { w: 360, h: 740, scale_pct: 100 });
    t.event(&Event::Theme { dark: false, accent: Color(0, 0, 0, 255), background: Color(255, 255, 255, 255), foreground: Color(0, 0, 0, 255) });
    t.frame();
    t
}

/// What the tile host does for a tap: pointer down, up, then `Hit` for the topmost hit region of the last frame.
fn tap(t: &mut SdkTile<SampleApp>, name: &str) {
    let l = t.sdk_frame();
    let n = l.a11y().into_iter().find(|n| n.name == name).unwrap_or_else(|| panic!("no control named {name}"));
    let (x, y) = ((n.rect.x + n.rect.w / 2.0) as i32, (n.rect.y + n.rect.h / 2.0) as i32);
    let hit = to_tile(&t.sdk_frame()).unwrap().hit_test(x, y);
    t.event(&Event::PointerDown { x, y, button: 0 });
    t.event(&Event::PointerUp { x, y, button: 0 });
    if let Some(id) = hit {
        t.event(&Event::Hit { id });
    }
    t.frame();
}

fn texts(t: &SdkTile<SampleApp>) -> Vec<String> {
    t.frame().ops.iter().filter_map(|o| if let Op::Text { text, .. } = o { Some(text.clone()) } else { None }).collect()
}

/// Some text is drawn (a long one is cut with an ellipsis by the app's own `fit`, so a cut text counts when what is left is its start).
fn has(t: &SdkTile<SampleApp>, s: &str) -> bool {
    texts(t).iter().any(|x| x == s || (x.ends_with('…') && s.starts_with(x.trim_end_matches('…'))))
}

#[test]
fn a_tap_is_one_action_not_two_and_typing_is_one_character_not_two_and_the_keyboard_hook_follows_the_field() {
    let core = FakeCore::default();
    let mut t = tile(&core);
    assert!(has(&t, "[ ] Try the wallpaper picker"));
    assert!(!t.wants_keyboard());
    tap(&mut t, "[ ] Try the wallpaper picker");
    assert!(has(&t, "[x] Try the wallpaper picker"), "toggled once: a second action would have undone it");
    assert!(!t.wants_keyboard());
    tap(&mut t, "New note");
    assert!(t.wants_keyboard(), "the field has focus: the on-screen keyboard is wanted");
    for c in "ab".chars() {
        t.event(&Event::Key { code: c as u32, mods: 0, down: true });
        t.event(&Event::TextInput { text: c.to_string() });
    }
    t.event(&Event::Key { code: 9, mods: 0, down: true });
    assert!(!t.wants_keyboard(), "Tab leaves the field and the keyboard goes away");
    t.event(&Event::Key { code: 9, mods: 0, down: true });
    t.event(&Event::TextInput { text: "pasted words".into() });
    t.event(&Event::TextInput { text: "!".into() });
    t.event(&Event::Key { code: 13, mods: 0, down: true });
    assert!(has(&t, "[ ] abpasted words!"));
    // a hit with no pointer pair before it (a keyboard walk or switch access) is an action
    let before = texts(&t);
    let l = t.sdk_frame();
    let id = l.a11y().iter().zip(l.ops.iter().filter_map(|o| if let S::Hit(_, h) = o { Some(h.action) } else { None })).find(|(n, _)| n.name == "[ ] abpasted words!").map(|(_, a)| a).unwrap();
    t.event(&Event::Hit { id: id + 1 });
    assert_ne!(texts(&t), before);
    assert!(has(&t, "[x] abpasted words!"));
    // ctrl with a letter is the host's shortcut, not the app's typing
    assert!(!t.event(&Event::Key { code: 'a' as u32, mods: 2, down: true }));
    assert!(!t.event(&Event::Key { code: 13, mods: 0, down: false }), "a key release is nothing");
}

#[test]
fn the_notes_are_kept_through_the_core_and_come_back_in_a_new_tile_and_a_refusal_is_said() {
    let core = FakeCore::default();
    let mut t = tile(&core);
    tap(&mut t, "New note");
    t.event(&Event::TextInput { text: "buy milk".into() });
    t.event(&Event::Key { code: 13, mods: 0, down: true });
    tap(&mut t, "[ ] buy milk");
    let saved = core.0.borrow().store.get("notes").cloned().unwrap();
    assert!(saved.contains("1 buy milk") && saved.contains("0 Try the wallpaper picker"), "{saved:?}");
    let calls: Vec<_> = core.0.borrow().calls.iter().map(|c| (c.0.clone(), c.1.clone())).collect();
    assert!(calls.iter().all(|c| c.0 == "storage" || c.0 == "dock"), "only the two namespaces: {calls:?}");
    // a new tile (a restart) reads them back
    let t2 = tile(&core);
    assert!(has(&t2, "[x] buy milk"), "{:?}", texts(&t2));
    // the core refuses a big value: the app says so instead of pretending
    core.0.borrow_mut().quota = Some(20);
    let mut t3 = tile(&core);
    tap(&mut t3, "Clear done (1)");
    tap(&mut t3, "New note");
    t3.event(&Event::TextInput { text: "a very long note that does not fit".into() });
    t3.event(&Event::Key { code: 13, mods: 0, down: true });
    t3.event(&Event::Key { code: 27, mods: 0, down: true });
    assert!(t3.with_app(|a| a.note.clone()).contains("could not be saved"));
    // the whole door shut: no storage, no crash, the notes still work in memory
    let shut = FakeCore::default();
    shut.0.borrow_mut().deny = vec!["storage", "dock"];
    let mut t4 = tile(&shut);
    assert!(has(&t4, "[ ] Try the wallpaper picker"));
    tap(&mut t4, "[ ] Try the wallpaper picker");
    assert!(has(&t4, "[x] Try the wallpaper picker"));
}

#[test]
fn the_dock_port_asks_the_core_and_tells_a_refusal_from_a_missing_part() {
    let core = FakeCore::default();
    let mut t = tile(&core);
    tap(&mut t, "Pin to dock");
    assert_eq!(core.0.borrow().pinned, vec!["sample".to_string()]);
    assert!(has(&t, "Unpin from dock"), "the button follows what the core says");
    tap(&mut t, "Unpin from dock");
    assert!(core.0.borrow().pinned.is_empty());
    let calls: Vec<String> = core.0.borrow().calls.iter().filter(|c| c.0 == "dock").map(|c| c.1.clone()).collect();
    assert!(calls.contains(&"pin".to_string()) && calls.contains(&"unpin".to_string()) && calls.contains(&"status".to_string()), "{calls:?}");
    // ports directly
    let door = Shared::new(core.clone());
    let mut dock = CoreDock::new(door.clone());
    assert_eq!(dock.pin("sample"), Ok(()));
    core.0.borrow_mut().deny = vec!["dock"];
    assert!(matches!(dock.pin("sample"), Err(DeskError::Refused(_))), "a refusal is a refusal");
    let mut other = FakeCore::default();
    assert_eq!(other.call("dock", "nothing", "{}"), Err("no such call".into()));
    let mut st = CoreStorage::new(door);
    assert_eq!(st.set("", "x"), Err(PortError::BadKey));
    assert_eq!(st.set("k", &"x".repeat(70_000)), Err(PortError::BadKey));
    assert_eq!(st.get("has\nnewline"), None);
    let _ = MemDock::default().pinned();
}

#[test]
fn the_in_memory_storage_keeps_the_same_limits_as_the_core() {
    let mut m = MemStorage::default();
    assert_eq!(m.set("a", "1"), Ok(()));
    assert_eq!(m.get("a"), Some("1".into()));
    assert_eq!(m.keys(), vec!["a".to_string()]);
    for i in 0..4 {
        let _ = m.set(&format!("k{i}"), &"x".repeat(60_000));
    }
    assert_eq!(m.set("big", &"x".repeat(60_000)), Err(PortError::TooBig), "256 KB for one app, keys and values together");
    assert_eq!(m.remove("a"), Ok(()));
    assert_eq!(m.get("a"), None);
}

#[test]
fn version_3_paths_layers_and_cluster_runs_map_onto_the_tile_abi() {
    use iyakki_ui_sdk::path::Seg;
    let tri = vec![Seg::Move(0.0, 0.0), Seg::Line(10.0, 0.0), Seg::Line(0.0, 10.0), Seg::Close];
    let t = to_tile(&list(vec![
        S::PushLayer(0.5),
        S::Path { fill: Some([1, 2, 3, 200]), stroke: Some(([4, 5, 6, 255], 2.0)), segs: tri.clone() },
        S::PopLayer,
        S::Rect(Rect::new(0.0, 0.0, 2.0, 2.0), [9, 9, 9, 255]),
        S::TextC { x: 1.0, y: 2.0, size: 12.0, color: [0, 0, 0, 255], style: TextStyle::Regular, text: "e\u{0301}".into(), opts: Default::default(), advances: vec![7.0] },
    ]))
    .unwrap();
    let bands: Vec<_> = t.ops.iter().filter(|o| matches!(o, Op::Rect { r, .. } if r.h == 1)).collect();
    assert_eq!(bands.len(), 10, "one band per row of the triangle");
    assert!(bands.iter().all(|o| matches!(o, Op::Rect { color, .. } if color.3 == 100)), "the layer halves the fill's alpha");
    let lines: Vec<_> = t.ops.iter().filter(|o| matches!(o, Op::Line { .. })).collect();
    assert_eq!(lines.len(), 3);
    assert!(lines.iter().all(|o| matches!(o, Op::Line { color, thickness: 2, .. } if color.3 == 128)));
    assert!(t.ops.iter().any(|o| matches!(o, Op::Rect { color, .. } if *color == Color(9, 9, 9, 255))), "outside the layer nothing fades");
    assert!(t.ops.iter().any(|o| matches!(o, Op::Text { text, .. } if text == "e\u{0301}")));
}
