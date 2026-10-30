//! Tests of the Touch Bar strip (`touchbar.rs`): layout at the reported width, hit regions, slider math, strip contents from fake
//! state, the per-app registry, recents, touches, drawing (pixel checks), pacing and the source scan. No device is involved.

use crate::fnkeys::FnKey;
use crate::gfx::Icon;
use crate::touchbar::*;
use crate::ui::Key;

fn rich() -> Ctx {
    Ctx {
        recents: ["chat", "ide", "radio", "browser", "settings", "media", "tales"].iter().map(|id| Recent { id: id.to_string(), label: id.to_uppercase(), icon: Icon::Chat }).collect(),
        kbd_light: Some(0.3),
        focused: Some("ide".into()),
        busy: true,
        ..Ctx::default()
    }
}

fn bar(w: u32, h: u32) -> Bar {
    Bar::new(w, h, &[])
}

fn every_strip_slots(w: u32, h: u32, c: &Ctx) -> Vec<(Strip, Vec<Slot>)> {
    let reg = Registry::standard();
    Strip::ALL.iter().map(|s| (*s, layout(&build(*s, &Strip::ALL, c, &reg), w, h))).collect()
}

#[test]
fn every_strip_fits_the_bar_without_overlaps_at_many_sizes() {
    for (w, h) in [(2170, 60), (1000, 60), (600, 60), (4096, 60), (2170, 30), (2170, 120), (200, 60), (50, 60), (2170, 8), (1, 1)] {
        for (strip, slots) in every_strip_slots(w, h, &rich()) {
            let mut right = 0;
            for s in &slots {
                let r = s.rect;
                assert!(r.w > 0 && r.h > 0, "{strip:?} {w}x{h}: empty rect");
                assert!(r.right() <= w && r.bottom() <= h, "{strip:?} {w}x{h}: {r:?} outside the bar");
                assert!(r.x >= right, "{strip:?} {w}x{h}: {r:?} overlaps the one before (ends at {right})");
                right = r.right();
            }
        }
    }
    // at the real size nothing is dropped, and Esc and the strip chip lead
    for (strip, slots) in every_strip_slots(2170, 60, &rich()) {
        assert_eq!(slots[0].item.id, "esc", "{strip:?}");
        assert_eq!(slots[1].item.id, "mode");
        assert!(slots[0].rect.x < 10, "Esc sits at the left edge");
    }
    let reg = Registry::standard();
    let items = build(Strip::System, &Strip::ALL, &rich(), &reg);
    assert_eq!(layout(&items, 2170, 60).len(), items.len());
    // a narrow bar drops items from the end
    let narrow = layout(&items, 700, 60);
    assert!(narrow.len() < items.len() && narrow.len() >= 2);
    assert_eq!(layout(&[], 2170, 60).len(), 0);
}

#[test]
fn the_sliders_take_the_spare_width_and_the_rest_keep_their_size() {
    let reg = Registry::standard();
    let slots = layout(&build(Strip::System, &Strip::ALL, &rich(), &reg), 2170, 60);
    let vol = slots.iter().find(|s| s.item.id == "volume").unwrap();
    let esc = slots.iter().find(|s| s.item.id == "esc").unwrap();
    assert!(vol.rect.w > 300 && esc.rect.w == 96);
    let last = slots.last().unwrap().rect;
    assert!(2170 - last.right() <= 12 + 3, "the strip fills the bar: {last:?}");
}

#[test]
fn slider_values_map_the_touch_with_clamps() {
    let r = Rc { x: 100, y: 6, w: 500, h: 48 };
    let (x0, x1) = slider_track(&r);
    assert!(x0 > 100.0 && x1 < 600.0 && x1 > x0);
    assert_eq!(slider_value(&r, x0 as i32 - 500), 0.0);
    assert_eq!(slider_value(&r, x1 as i32 + 500), 1.0);
    assert_eq!(slider_value(&r, i32::MIN), 0.0);
    assert_eq!(slider_value(&r, i32::MAX), 1.0);
    let mid = slider_value(&r, ((x0 + x1) / 2.0) as i32);
    assert!((mid - 0.5).abs() < 0.01, "{mid}");
    for x in (0..800).step_by(7) {
        let v = slider_value(&r, x);
        assert!((0.0..=1.0).contains(&v));
    }
    // a collapsed slider never panics
    let tiny = Rc { x: 0, y: 0, w: 1, h: 1 };
    assert!((0.0..=1.0).contains(&slider_value(&tiny, 0)));
    assert_eq!((percent(0.0), percent(0.5), percent(1.0), percent(2.0), percent(-1.0), percent(f32::NAN)), (0, 50, 100, 100, 0, 0));
}

#[test]
fn the_system_strip_shows_the_machine_state() {
    let reg = Registry::standard();
    let ids = |c: &Ctx| build(Strip::System, &Strip::ALL, c, &reg).iter().map(|i| i.id.clone()).collect::<Vec<_>>();
    assert_eq!(ids(&Ctx::default()), ["esc", "mode", "volume", "mute", "brightness", "playpause", "clock"]);
    assert!(ids(&rich()).contains(&"kbdlight".to_string()));
    let no_panel = Ctx { brightness: None, ..Ctx::default() };
    assert!(!ids(&no_panel).contains(&"brightness".to_string()));
    let items = build(Strip::System, &Strip::ALL, &Ctx { muted: true, playing: true, volume: 0.25, clock: "09:41".into(), ..Ctx::default() }, &reg);
    let get = |id: &str| items.iter().find(|i| i.id == id).unwrap().clone();
    assert_eq!((get("mute").label.as_str(), get("playpause").label.as_str(), get("clock").label.as_str()), ("Unmute", "Pause", "09:41"));
    assert!(get("volume").on && (get("volume").value - 0.25).abs() < 1e-6 && get("playpause").on);
    let silent = build(Strip::System, &Strip::ALL, &Ctx { has_sound: false, ..Ctx::default() }, &reg);
    assert!(!silent.iter().find(|i| i.id == "volume").unwrap().enabled && !silent.iter().find(|i| i.id == "mute").unwrap().enabled);
    assert_eq!(get("playpause").action, Action::Fn(FnKey::PlayPause));
}

#[test]
fn the_recent_strip_lists_the_last_six_apps_and_opens_them() {
    let reg = Registry::standard();
    let items = build(Strip::Recent, &Strip::ALL, &rich(), &reg);
    let tiles: Vec<&Item> = items.iter().filter(|i| i.kind == Kind::Tile).collect();
    assert_eq!(tiles.len(), MAX_RECENT);
    assert_eq!(tiles[0].action, Action::Open("chat".into()));
    assert_eq!(tiles[5].action, Action::Open("media".into()));
    let none = build(Strip::Recent, &Strip::ALL, &Ctx::default(), &reg);
    assert!(none.iter().any(|i| i.kind == Kind::Label && i.label.contains("Apps you open")));
}

#[test]
fn the_ai_strip_shows_the_agent_and_the_context_buttons() {
    let reg = Registry::standard();
    let get = |c: &Ctx| build(Strip::Ai, &Strip::ALL, c, &reg);
    let find = |v: &[Item], id: &str| v.iter().find(|i| i.id == id).cloned();
    for (st, text, tone) in [(AgentState::Ready, "ready", Tone::Good), (AgentState::Working, "working", Tone::Warn), (AgentState::NeedsLogin, "needs login", Tone::Bad), (AgentState::Missing, "not installed", Tone::Bad), (AgentState::Unknown, "unknown", Tone::Normal)] {
        let v = get(&Ctx { agent: st, ..Ctx::default() });
        let a = find(&v, "agent").unwrap();
        assert!(a.label == format!("Claude Code: {text}") && a.tone == tone, "{a:?}");
    }
    let plain = get(&Ctx::default());
    assert_eq!(find(&plain, "chat").unwrap().action, Action::Open("chat".into()));
    assert_eq!(find(&plain, "ask").unwrap().action, Action::Open("chat".into()));
    assert!(!find(&plain, "stop").unwrap().enabled, "nothing to stop");
    assert!(find(&plain, "approve").is_none() && find(&plain, "run").is_none());
    let busy = get(&Ctx { focused: Some("chat".into()), busy: true, ..Ctx::default() });
    assert!(find(&busy, "stop").unwrap().enabled);
    assert_eq!(find(&busy, "stop").unwrap().action, Action::Key(Key::Ctrl('k')));
    assert!(find(&busy, "chat").unwrap().on);
    let busy_elsewhere = get(&Ctx { focused: Some("ide".into()), busy: true, ..Ctx::default() });
    assert!(!find(&busy_elsewhere, "stop").unwrap().enabled, "Stop only acts in the chat that runs");
    assert!(find(&busy_elsewhere, "run").is_some() && find(&busy_elsewhere, "save").is_some(), "IDE shortcuts");
    let ask = get(&Ctx { approval: true, focused: Some("chat".into()), ..Ctx::default() });
    assert_eq!(find(&ask, "approve").unwrap().action, Action::Key(Key::Ctrl('y')));
    assert_eq!(find(&ask, "deny").unwrap().action, Action::Key(Key::Ctrl('n')));
}

#[test]
fn the_registry_keeps_six_items_per_app_and_the_app_strip_falls_back() {
    let mut r = Registry::default();
    let many: Vec<Item> = (0..9).map(|k| Item::button(&format!("i{k}"), "x", Glyph::None, Action::None, 80)).collect();
    r.register("demo", many);
    assert_eq!(r.get("demo").len(), MAX_APP_ITEMS);
    r.register("dup", vec![Item::button("a", "A", Glyph::None, Action::None, 80), Item::button("a", "B", Glyph::None, Action::None, 80)]);
    assert_eq!(r.get("dup").len(), 1);
    r.register("demo", vec![]);
    assert!(r.get("demo").is_empty() && r.get("nothing").is_empty());
    let std = Registry::standard();
    for app in ["chat", "ide", "tales", "radio", "media", "browser"] {
        let n = std.get(app).len();
        assert!((1..=MAX_APP_ITEMS).contains(&n), "{app}: {n}");
    }
    // chat: new chat is Ctrl+N, as its help says; radio: media keys
    assert_eq!(std.get("chat")[0].action, Action::Key(Key::Ctrl('n')));
    assert_eq!(std.get("radio")[0].action, Action::Fn(FnKey::Prev));
    // the app strip shows the focused app's items and falls back to the system strip
    let in_chat = Ctx { focused: Some("chat".into()), ..Ctx::default() };
    assert_eq!(effective_strip(Strip::App, &in_chat, &std), Strip::App);
    let ids: Vec<String> = build(Strip::App, &Strip::ALL, &in_chat, &std).iter().skip(2).map(|i| i.id.clone()).collect();
    assert_eq!(ids, ["new", "model", "agent", "approvals"]);
    assert_eq!(effective_strip(Strip::App, &Ctx::default(), &std), Strip::System, "launcher: no app items");
    assert_eq!(effective_strip(Strip::App, &Ctx { focused: Some("users".into()), ..Ctx::default() }, &std), Strip::System);
    assert_eq!(effective_strip(Strip::Recent, &Ctx { approval: true, ..Ctx::default() }, &std), Strip::Ai, "a question waiting brings the AI strip");
    // the media item shows what plays
    let radio = Ctx { focused: Some("radio".into()), playing: true, ..Ctx::default() };
    let it = build(Strip::App, &Strip::ALL, &radio, &std);
    assert!(it.iter().any(|i| i.id == "play" && i.label == "Pause" && i.on));
}

#[test]
fn recents_are_newest_first_without_repeats_and_remembered_per_account() {
    let mut c = Choice::default();
    assert!(c.touch_app("chat") && c.touch_app("ide") && c.touch_app("radio"));
    assert!(!c.touch_app("radio"), "the same app again changes nothing");
    assert!(c.touch_app("chat"));
    assert_eq!(c.recents, ["chat", "radio", "ide"]);
    for a in ["a", "b", "c", "d", "e", "f", "g"] {
        c.touch_app(a);
    }
    assert_eq!(c.recents.len(), MAX_RECENT);
    assert_eq!(c.recents[0], "g");
    let d = std::env::temp_dir().join(format!("tbar-choice-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    c.strip = "ai".into();
    c.save(&d, "alice");
    Choice { strip: "recent".into(), recents: vec![] }.save(&d, "bob/../x");
    assert_eq!(Choice::load(&d, "alice"), c);
    assert_eq!(Choice::load(&d, "bob/../x").strip, "recent");
    assert_eq!(Choice::load(&d, "nobody"), Choice::default());
    assert!(d.join("touchbar-bobx.json").is_file(), "the user name cannot leave the folder");
    std::fs::write(d.join("touchbar-bad.json"), "{not json").unwrap();
    assert_eq!(Choice::load(&d, "bad"), Choice::default());
    let _ = std::fs::remove_dir_all(&d);
}

fn bar_with_slots(c: &Ctx) -> Bar {
    let mut b = bar(2170, 60);
    b.update(c, 0);
    b
}

fn center(s: &Slot) -> i32 {
    (s.rect.x + s.rect.w / 2) as i32
}

#[test]
fn a_tap_acts_when_the_finger_lifts_over_the_button_it_went_down_on() {
    let mut b = bar_with_slots(&Ctx::default());
    let esc = b.slots()[0].clone();
    assert!(b.touch(TouchEv::Down(center(&esc))).is_empty(), "nothing on touch-down");
    assert_eq!(b.touch(TouchEv::Up(center(&esc))), [Action::Esc]);
    // slid off before lifting: cancelled
    b.touch(TouchEv::Down(center(&esc)));
    b.touch(TouchEv::Move(1500));
    assert!(b.touch(TouchEv::Up(1500)).is_empty());
    // a tap on empty space or a label does nothing
    assert!(b.touch(TouchEv::Down(2169)).is_empty() && b.touch(TouchEv::Up(2169)).is_empty());
    let clock = b.slots().iter().find(|s| s.item.id == "clock").unwrap().clone();
    b.touch(TouchEv::Down(center(&clock)));
    assert!(b.touch(TouchEv::Up(center(&clock))).is_empty());
    // a disabled button does nothing
    let mut b = bar_with_slots(&Ctx::default());
    let mute_slot = b.slots().iter().find(|s| s.item.id == "mute").unwrap().clone();
    let quiet = Ctx { has_sound: false, ..Ctx::default() };
    b.update(&quiet, 1000);
    let m2 = b.slots().iter().find(|s| s.item.id == "mute").unwrap().clone();
    assert_eq!(m2.rect, mute_slot.rect);
    b.touch(TouchEv::Down(center(&m2)));
    assert!(b.touch(TouchEv::Up(center(&m2))).is_empty());
}

#[test]
fn a_slider_follows_the_finger_and_reports_each_new_percent_once() {
    let mut b = bar_with_slots(&Ctx::default());
    let vol = b.slots().iter().find(|s| s.item.id == "volume").unwrap().clone();
    let (x0, x1) = slider_track(&vol.rect);
    let at = |f: f32| (x0 + (x1 - x0) * f) as i32;
    let a = b.touch(TouchEv::Down(at(0.2)));
    assert!(matches!(a.as_slice(), [Action::Slider(SliderKind::Volume, v)] if (v - 0.2).abs() < 0.02), "{a:?}");
    assert!(b.touch(TouchEv::Move(at(0.2))).is_empty(), "the same percent is not repeated");
    let m = b.touch(TouchEv::Move(at(0.9)));
    assert!(matches!(m.as_slice(), [Action::Slider(SliderKind::Volume, v)] if (v - 0.9).abs() < 0.02));
    let far = b.touch(TouchEv::Move(5000));
    assert!(matches!(far.as_slice(), [Action::Slider(SliderKind::Volume, v)] if *v == 1.0), "clamped at the end");
    assert!(b.touch(TouchEv::Up(5000)).is_empty());
    // brightness is its own slider
    let br = b.slots().iter().find(|s| s.item.id == "brightness").unwrap().clone();
    let a = b.touch(TouchEv::Down(center(&br)));
    assert!(matches!(a.as_slice(), [Action::Slider(SliderKind::Brightness, _)]));
}

#[test]
fn the_strip_chip_cycles_the_enabled_strips_and_is_not_passed_on() {
    let mut b = Bar::new(2170, 60, &["system", "ai"]);
    assert_eq!(b.strips, [Strip::System, Strip::Ai]);
    b.update(&Ctx::default(), 0);
    let chip = b.slots()[1].clone();
    b.touch(TouchEv::Down(center(&chip)));
    assert!(b.touch(TouchEv::Up(center(&chip))).is_empty(), "handled inside");
    assert_eq!(b.chosen(), Strip::Ai);
    b.touch(TouchEv::Down(center(&chip)));
    b.touch(TouchEv::Up(center(&chip)));
    assert_eq!(b.chosen(), Strip::System);
    b.set_strip(Strip::Recent);
    assert_eq!(b.chosen(), Strip::System, "a strip that is not enabled cannot be chosen");
    assert_eq!(Bar::new(2170, 60, &["bogus"]).strips, Strip::ALL, "nothing valid: all of them");
}

#[test]
fn touches_do_nothing_while_the_bar_is_dark_or_locked() {
    let mut b = bar(2170, 60);
    b.update(&Ctx { idle_ms: IDLE_DARK_MS, ..Ctx::default() }, 0);
    assert_eq!(b.power(), Power::Dark);
    assert!(b.touch(TouchEv::Down(40)).is_empty() && b.touch(TouchEv::Up(40)).is_empty());
    b.update(&Ctx { signed_in: false, ..Ctx::default() }, 1000);
    assert_eq!(b.power(), Power::Locked);
    assert!(b.touch(TouchEv::Down(40)).is_empty() && b.touch(TouchEv::Up(40)).is_empty());
    assert!(b.slots().is_empty());
}

#[test]
fn raw_touch_events_become_positions_along_the_bar() {
    let mut t = RawTouch::new(0, 1000, 2170, false, AXIS_X);
    let run = |t: &mut RawTouch, evs: &[(u16, u16, i32)]| evs.iter().flat_map(|(a, b, c)| t.feed(*a, *b, *c)).collect::<Vec<_>>();
    let out = run(&mut t, &[(3, 0x39, 5), (3, 0x35, 500), (0, 0, 0), (3, 0x35, 1000), (0, 0, 0), (3, 0x39, -1)]);
    assert_eq!(out, [TouchEv::Down(1085), TouchEv::Move(2169), TouchEv::Up(2169)]);
    // a second finger (slot 1) is ignored; the long axis can be Y; mirrored bars flip
    let mut t = RawTouch::new(0, 100, 101, true, AXIS_Y);
    let out = run(&mut t, &[(3, 0x2f, 1), (3, 0x39, 9), (3, 0x36, 50), (0, 0, 0), (3, 0x2f, 0), (3, 0x39, 4), (3, 0x35, 99), (3, 0x36, 10), (0, 0, 0), (3, 0x39, -1)]);
    assert_eq!(out, [TouchEv::Down(90), TouchEv::Up(90)]);
    // garbage never panics and an out-of-range position is clamped
    let mut t = RawTouch::new(10, 10, 0, false, AXIS_X);
    for k in 0..50 {
        t.feed((k % 5) as u16, (k * 7) as u16, k as i32 * 1000 - 20000);
    }
}

fn near(a: [u8; 3], b: [u8; 4]) -> bool {
    (0..3).all(|k| (a[k] as i32 - b[k] as i32).abs() <= 2)
}

#[test]
fn the_drawn_strip_has_the_expected_pixels() {
    let mut b = bar(2170, 60);
    let c = Ctx { volume: 0.5, ..Ctx::default() };
    let pic = b.update(&c, 0).expect("the first picture is due");
    assert_eq!((pic.w, pic.h, pic.rgba.len()), (2170, 60, 2170 * 60 * 4));
    assert!(pic.rgba.chunks_exact(4).all(|p| p[3] == 255), "opaque");
    let slots = b.slots().to_vec();
    // the bar's far right margin and the gap between Esc and the chip are black
    assert_eq!(pic.pixel(2169, 30), [0, 0, 0]);
    let gap = slots[0].rect.right() + 2;
    assert_eq!(pic.pixel(gap, 30), [0, 0, 0]);
    // Esc is a card
    let esc = slots[0].rect;
    assert!(near(pic.pixel(esc.x + 4, esc.y + esc.h / 2), crate::ui::CARD), "{:?}", pic.pixel(esc.x + 4, esc.y + esc.h / 2));
    // the volume track: accent up to the middle, the empty colour after it
    let vol = slots.iter().find(|s| s.item.id == "volume").unwrap().rect;
    let (x0, x1) = slider_track(&vol);
    let y = vol.y + vol.h / 2;
    assert!(near(pic.pixel((x0 + (x1 - x0) * 0.25) as u32, y), crate::gfx::rgb(79, 140, 255)), "filled part");
    assert!(near(pic.pixel((x0 + (x1 - x0) * 0.75) as u32, y), crate::gfx::rgb(70, 78, 108)), "empty part");
    // something is drawn in the Esc label (text pixels)
    let lit = (esc.x..esc.right()).any(|x| (esc.y..esc.bottom()).any(|y| pic.pixel(x, y)[0] > 150));
    assert!(lit, "the label has light pixels");
    // a pressed button is lighter
    b.touch(TouchEv::Down(center(&slots[0])));
    let pic2 = b.update(&c, 1000).expect("pressing redraws");
    assert!(near(pic2.pixel(esc.x + 4, esc.y + esc.h / 2), crate::ui::CARD_SEL));
    // muting changes the speaker and highlights the slider card
    let muted = Ctx { muted: true, ..c.clone() };
    let mut b2 = bar(2170, 60);
    let pm = b2.update(&muted, 0).unwrap();
    assert_ne!(pm.rgba, pic.rgba);
    assert!(near(pm.pixel(vol.x + 3, vol.y + vol.h / 2), crate::gfx::rgb(36, 56, 104)), "a muted volume card is highlighted");
}

#[test]
fn the_locked_bar_shows_only_the_name_and_the_dark_bar_nothing() {
    let mut b = bar(2170, 60);
    let locked = b.update(&Ctx { signed_in: false, recents: rich().recents, clock: "13:37".into(), ..Ctx::default() }, 0).unwrap();
    let cards = locked.rgba.chunks_exact(4).filter(|p| p[..3] == crate::ui::CARD[..3]).count();
    assert_eq!(cards, 0, "no buttons, no state");
    assert!(locked.rgba.chunks_exact(4).any(|p| p[0] > 100), "the name is drawn");
    // the name is centred, nothing near the edges
    assert!((0..300).all(|x| locked.pixel(x, 30) == [0, 0, 0]) && (1870..2170).all(|x| locked.pixel(x, 30) == [0, 0, 0]));
    let dark = bar(2170, 60).update(&Ctx { idle_ms: IDLE_DARK_MS + 1, ..Ctx::default() }, 0).unwrap();
    assert!(dark.rgba.chunks_exact(4).all(|p| p[..3] == [0, 0, 0]));
    assert_eq!(power(&Ctx { screen_off: true, ..Ctx::default() }), Power::Dark);
    assert_eq!(power(&Ctx { idle_ms: IDLE_DARK_MS - 1, ..Ctx::default() }), Power::On);
    // the locked fingerprint does not contain the clock or the apps: the same picture whatever they say
    let a = bar(2170, 60).update(&Ctx { signed_in: false, clock: "01:00".into(), ..Ctx::default() }, 0).unwrap();
    assert_eq!(a, locked);
}

#[test]
fn every_strip_draws_at_every_size_without_panicking() {
    for (w, h) in [(2170, 60), (600, 60), (100, 60), (2170, 20), (64, 8), (4096, 120)] {
        for s in Strip::ALL {
            let mut b = Bar::new(w, h, &[]);
            b.set_strip(s);
            let pic = b.update(&rich(), 0).unwrap();
            assert_eq!(pic.rgba.len(), pic.w as usize * pic.h as usize * 4);
        }
    }
}

#[test]
fn redraws_happen_only_on_change_and_at_most_ten_times_a_second() {
    let mut p = Pacer::default();
    assert!(p.due(0, 1));
    assert!(!p.due(1_000, 1), "nothing changed");
    assert!(!p.due(50, 2), "changed, but too soon");
    assert!(p.due(100, 2));
    assert!(!p.due(150, 3) && p.due(200, 3));
    p.reset();
    assert!(!p.due(210, 3), "a reset does not beat the rate limit");
    assert!(p.due(300, 3), "after a reset the same content is drawn again");
    // the bar: an idle second makes no picture; a changing clock makes at most ten
    let mut b = bar(2170, 60);
    let mut c = Ctx::default();
    assert!(b.update(&c, 0).is_some());
    for t in (100..2000).step_by(100) {
        assert!(b.update(&c, t).is_none(), "an unchanged bar sends nothing");
    }
    let mut n = 0;
    for ms in 2000..3000u64 {
        c.clock = format!("{ms}");
        n += b.update(&c, ms).is_some() as u32;
    }
    assert!((9..=10).contains(&n), "{n} pictures in one second");
}

#[test]
fn the_changed_part_of_a_picture_is_found() {
    let a = Picture::black(100, 10);
    assert_eq!(a.damage(None), Some(iyakki_drivers::touchbar::Rect { x: 0, y: 0, w: 100, h: 10 }));
    assert_eq!(a.damage(Some(&a.clone())), None);
    let mut b = a.clone();
    for (x, y) in [(10u32, 2u32), (40, 7)] {
        let at = (y as usize * 100 + x as usize) * 4;
        b.rgba[at] = 200;
    }
    assert_eq!(b.damage(Some(&a)), Some(iyakki_drivers::touchbar::Rect { x: 10, y: 2, w: 31, h: 6 }));
    assert_eq!(b.damage(Some(&Picture::black(50, 10))), Some(iyakki_drivers::touchbar::Rect { x: 0, y: 0, w: 100, h: 10 }), "another size: all of it");
}

#[test]
fn only_touch_bar_names_are_the_bars() {
    assert!(is_touchbar_name("Apple Inc. Touch Bar Display Touchpad"));
    assert!(is_touchbar_name("appletb_backlight"));
    assert!(!is_touchbar_name("Apple Internal Keyboard / Trackpad"));
    assert!(!is_touchbar_name("USB Optical Mouse"));
    assert!(!is_touchbar_name(""));
}

#[test]
fn the_source_has_no_panicking_geometry() {
    let root = env!("CARGO_MANIFEST_DIR");
    for rel in ["src/touchbar.rs", "src/touchbar_host.rs", "../iyakki-drivers/src/touchbar.rs", "../iyakki-drivers/src/usbfs.rs"] {
        let s = std::fs::read_to_string(format!("{root}/{rel}")).unwrap();
        let code = s.find("\n#[cfg(test)]").map_or(&s[..], |k| &s[..k]);
        // the fake device module of the drivers crate is test support inside the file: scan only what comes before it
        let code = code.find("\n/// A scripted Touch Bar").map_or(code, |k| &code[..k]);
        for pat in [".clamp(", ".unwrap()", ".expect(", "panic!(", "unreachable!("] {
            assert!(!code.contains(pat), "{rel} uses {pat}");
        }
    }
}
