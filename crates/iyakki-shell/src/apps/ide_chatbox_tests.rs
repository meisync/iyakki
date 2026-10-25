//! The AI IDE's chat input and its footer controls at the screen sizes of real devices: every one lies fully inside the IDE's
//! rectangle and above its status bar, none covers another, and a click at the centre of each reaches it (focus, typing, Send,
//! Enter). A real MacBook (2560x1600) reported "AI IDE chat box not clickable": the pointer reached the chat and the IDE
//! panicked in a layout clamp, so the click never landed; the layout itself is checked here.
use super::*;

const SIZES: [(u32, u32); 5] = [(2560, 1600), (1920, 1080), (1366, 768), (1280, 720), (800, 480)];

fn area_for(w: u32, h: u32) -> (R, f32) {
    let s = crate::shell::scale_for(w, h);
    (R::new(0.0, 46.0 * s, w as f32, h as f32 - 46.0 * s), s)
}

fn ready(tag: &str, w: u32, h: u32) -> (World, IdeApp, Gfx, R, f32) {
    let wd = world(tag);
    let mut app = IdeApp::new(&wd.cx);
    let d = project(&wd, "demo", &[]);
    open(&mut app, &wd.cx, "demo", &d);
    // narrow windows show one pane at a time: the chat is the one shown
    app.focus_pane(Pane::Chat);
    let (area, s) = area_for(w, h);
    let mut g = Gfx::new(w, h);
    app.render(&mut g, area, s, &wd.cx, 1);
    (wd, app, g, area, s)
}

fn inside(r: R, a: R) -> bool {
    r.x >= a.x - 0.5 && r.y >= a.y - 0.5 && r.x + r.w <= a.x + a.w + 0.5 && r.y + r.h <= a.y + a.h + 0.5
}

fn overlap(a: R, b: R) -> bool {
    a.x < b.x + b.w - 0.5 && b.x < a.x + a.w - 0.5 && a.y < b.y + b.h - 0.5 && b.y < a.y + a.h - 0.5
}

fn find(g: &Gfx, label: &str) -> Option<R> {
    g.hits.iter().rev().find(|h| h.label == label || h.label.starts_with(&format!("{label}:"))).map(|h| h.rect)
}

/// A click the way the shell delivers it: the topmost region at the point, a field region becoming a click with the offset.
fn click(app: &mut IdeApp, cx: &AppCx, g: &Gfx, x: i32, y: i32) -> Option<String> {
    let h = g.hits.iter().rev().find(|h| h.rect.contains(x, y))?;
    let ev = match h.ev {
        Event::Focus(i) => Event::FocusAt(i, x as f32 - h.rect.x),
        Event::Area(i) => Event::AreaAt(i, x as f32 - h.rect.x, y as f32 - h.rect.y),
        ref e => e.clone(),
    };
    app.handle(&ev, cx);
    Some(h.label.clone())
}

fn centre(r: R) -> (i32, i32) {
    ((r.x + r.w / 2.0) as i32, (r.y + r.h / 2.0) as i32)
}

#[test]
fn the_chat_input_send_and_footer_controls_are_inside_the_ide_and_clear_of_each_other_at_every_size() {
    for (w, h) in SIZES {
        let (_wd, app, g, area, s) = ready(&format!("cb-in-{w}"), w, h);
        let (field, send) = (find(&g, "message field").expect("the message field"), find(&g, "Send").expect("Send"));
        let ctl: Vec<(&str, R)> = ["Model ▾", "Agent: on", "Approvals"].iter().map(|l| (*l, find(&g, l).unwrap_or_else(|| panic!("{l} at {w}x{h}")))).collect();
        let _ = &app;
        let status_h = super::super::ide_chrome::cur().status_h * s;
        let usable = R::new(area.x, area.y, area.w, area.h - status_h);
        for (n, r) in [("message field", field), ("Send", send)].into_iter().chain(ctl.iter().copied()) {
            assert!(inside(r, usable), "{n} {r:?} is inside the IDE above its status bar {usable:?} at {w}x{h}");
            assert!(r.w > 4.0 && r.h > 4.0, "{n} has a size at {w}x{h}");
        }
        assert!(!overlap(field, send), "Send does not cover the message field at {w}x{h}: {field:?} {send:?}");
        for (n, r) in &ctl {
            assert!(!overlap(*r, field) && !overlap(*r, send), "{n} {r:?} is clear of the input at {w}x{h}");
        }
        for (i, (na, a)) in ctl.iter().enumerate() {
            for (nb, b) in &ctl[i + 1..] {
                assert!(!overlap(*a, *b), "{na} and {nb} overlap at {w}x{h}");
            }
        }
        // nothing drawn later sits on top of any of them: the topmost region at each centre is the control itself
        for (n, r) in [("message field", field), ("Send", send)].into_iter().chain(ctl.iter().copied()) {
            let (x, y) = centre(r);
            let top = g.hits.iter().rev().find(|h| h.rect.contains(x, y)).expect("a region at the centre");
            assert!(top.label.starts_with(n) || top.label == n || top.rect == r, "the topmost region at the centre of {n} is {:?} at {w}x{h}", top.label);
        }
    }
}

#[test]
fn a_click_on_the_chat_input_focuses_it_typing_appears_and_send_and_enter_send_it_at_every_size() {
    for (w, h) in SIZES {
        let (wd, mut app, g, _area, _s) = ready(&format!("cb-click-{w}"), w, h);
        let (x, y) = centre(find(&g, "message field").unwrap());
        assert!(click(&mut app, &wd.cx, &g, x, y).is_some(), "a region at the centre of the input at {w}x{h}");
        typ(&mut app, &wd.cx, "hello there");
        assert_eq!(cur(&app).chat.input_text(), "hello there", "typing appears in the input at {w}x{h}");
        // Send by clicking its centre
        let mut g2 = Gfx::new(w, h);
        let (area, s) = area_for(w, h);
        app.render(&mut g2, area, s, &wd.cx, 2);
        let (sx, sy) = centre(find(&g2, "Send").unwrap());
        click(&mut app, &wd.cx, &g2, sx, sy).expect("Send has a region");
        until(&mut app, &wd.cx, "the question to be sent", |a| a.ws[0].chat.messages().iter().any(|m| m.content.contains("hello there")));
        assert_eq!(cur(&app).chat.input_text(), "", "the input is emptied by Send at {w}x{h}");
        until(&mut app, &wd.cx, "the answer", |a| !a.ws[0].chat.is_running() && a.ws[0].chat.messages().iter().any(|m| m.content.contains("plain answer") || m.content.contains("ok")));
        // and Enter
        let mut g3 = Gfx::new(w, h);
        app.render(&mut g3, area, s, &wd.cx, 3);
        let (x, y) = centre(find(&g3, "message field").unwrap());
        click(&mut app, &wd.cx, &g3, x, y).unwrap();
        typ(&mut app, &wd.cx, "second one");
        key(&mut app, &wd.cx, Key::Enter);
        until(&mut app, &wd.cx, "the second question", |a| a.ws[0].chat.messages().iter().any(|m| m.content.contains("second one")));
    }
}

#[test]
fn hovering_and_dragging_around_the_chat_at_the_edges_never_panics() {
    // the pointer sweeps the whole chat pane, the right and bottom edges, and the dividers, with a button held and without
    for (w, h) in SIZES {
        let (wd, mut app, mut g, area, s) = ready(&format!("cb-sweep-{w}"), w, h);
        let mut n = 0u32;
        for fy in 0..=12 {
            for fx in 0..=12 {
                let (x, y) = (((w as f32 + 6.0) * fx as f32 / 12.0) as i32 - 3, ((h as f32 + 6.0) * fy as f32 / 12.0) as i32 - 3);
                app.handle(&Event::MouseMove(x, y), &wd.cx);
                g.mouse = (x, y);
                n += 1;
                if n % 29 == 0 {
                    g.hits.clear();
                    app.render(&mut g, area, s, &wd.cx, n as u64);
                }
            }
        }
        for (x, y) in [(w as i32 - 1, h as i32 - 1), (0, 0), (w as i32 - 1, 60)] {
            g.mouse = (x, y);
            app.handle(&Event::MouseDown(x, y), &wd.cx);
            app.handle(&Event::MouseMove(x - 30, y - 30), &wd.cx);
            app.handle(&Event::MouseUp(x, y), &wd.cx);
            app.render(&mut g, area, s, &wd.cx, 99);
        }
    }
}
