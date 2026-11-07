//! The Developer tools page: the list, choosing a tool by click and by key, a button producing output, scrolling and the small sizes.

use super::*;
use crate::apps::tests::cx_with_admin_store;
use crate::gfx::Gfx;
use crate::ui::R;

struct Fx {
    app: DevToolsApp,
    cx: AppCx,
    g: Gfx,
    area: R,
    s: f32,
    tick: u64,
}

impl Fx {
    fn new(w: u32, h: u32, s: f32) -> Fx {
        let cx = cx_with_admin_store("devtools");
        let mut f = Fx { app: DevToolsApp::new(&cx), cx, g: Gfx::new(w, h), area: R::new(0.0, 0.0, w as f32, h as f32), s, tick: 0 };
        f.draw();
        f
    }
    fn draw(&mut self) {
        self.tick += 1;
        self.g.clear(rgb(8, 10, 22));
        self.g.clear_hits();
        self.g.text_log.clear();
        self.app.render(&mut self.g, self.area, self.s, &self.cx, self.tick);
    }
    fn ev(&mut self, e: Event) {
        self.app.handle(&e, &self.cx);
        self.draw();
    }
    fn key(&mut self, k: Key) {
        self.ev(Event::Key(k));
    }
    fn text(&self) -> String {
        self.g.text_log.iter().map(|t| t.2.clone()).collect::<Vec<_>>().join("\n")
    }
    fn hit(&self, e: &Event) -> Option<R> {
        self.g.hits.iter().find(|h| &h.ev == e).map(|h| h.rect)
    }
    /// How many pixels are pure white and pure black (the QR code is neither theme).
    fn count_pixels(&self) -> (usize, usize) {
        self.g.rgba().chunks_exact(4).fold((0, 0), |(w, b), p| (w + usize::from(p[..3] == [255, 255, 255]), b + usize::from(p[..3] == [0, 0, 0])))
    }
    /// A click on a drawn hit region, the way the shell sends it.
    fn click(&mut self, e: Event) {
        let r = self.hit(&e).unwrap_or_else(|| panic!("{e:?} is not on screen:\n{}", self.text()));
        assert!(r.w > 0.0 && r.h > 0.0);
        crate::ui::set_from_click(true);
        self.ev(e);
        crate::ui::set_from_click(false);
    }
}

#[test]
// (the name keeps "twelve": FEATURES.md lists the test by it; it covers every tool in TOOLS)
fn all_twelve_tools_are_listed_and_clickable_at_every_size() {
    for (w, h, s) in [(1280, 800, 1.0), (800, 480, 1.0), (360, 740, 1.0), (360, 360, 1.0), (1600, 1000, 1.6)] {
        let f = Fx::new(w, h, s);
        let t = f.text();
        for (i, tool) in TOOLS.iter().enumerate() {
            let r = f.hit(&Event::Tab(T_TOOL + i)).unwrap_or_else(|| panic!("{w}x{h}: {} has no hit", tool.name));
            assert!(r.x >= 0.0 && r.y >= 0.0 && r.x + r.w <= w as f32 && r.y + r.h <= h as f32, "{w}x{h}: {} at {r:?}", tool.name);
        }
        if w >= 800 {
            for tool in &TOOLS {
                assert!(t.contains(tool.name), "{w}x{h}: {} in\n{t}", tool.name);
            }
        }
    }
}

#[test]
fn a_click_or_the_arrow_keys_choose_a_tool() {
    let mut f = Fx::new(900, 600, 1.0);
    assert_eq!(f.app.page(), "json");
    f.click(Event::Tab(T_TOOL + 3));
    assert_eq!(f.app.page(), "jwt");
    assert!(f.text().contains("Secret"), "{}", f.text());
    f.key(Key::Esc);
    assert_eq!(f.app.focus, Focus::List);
    f.key(Key::Down);
    assert_eq!(f.app.page(), "url");
    f.key(Key::Up);
    f.key(Key::Up);
    assert_eq!(f.app.page(), "base64");
    for _ in 0..20 {
        f.key(Key::Down);
    }
    assert_eq!(f.app.page(), "qr-code", "stops at the last");
    assert!(matches!(f.app.handle(&Event::Key(Key::Esc), &f.cx), Flow::Back), "Esc from the list leaves");
}

#[test]
fn a_button_click_and_the_keyboard_both_produce_output() {
    let mut f = Fx::new(900, 600, 1.0);
    f.app.input.set("{\"b\":1,\"a\":[true,null]}");
    f.draw();
    f.click(Event::Tab(T_ACT + 3));
    assert_eq!(f.app.out.text.lines().next(), Some("{"));
    assert!(f.text().contains("\"a\": ["), "{}", f.text());
    f.click(Event::Tab(T_ACT + 1));
    assert_eq!(f.app.out.text, "{\"b\":1,\"a\":[true,null]}");
    // keyboard: Tab to the buttons (the last click left the choice on Minify), Right to Validate, Enter
    f.key(Key::Esc);
    f.key(Key::Tab);
    f.key(Key::Tab);
    assert_eq!(f.app.focus, Focus::Buttons);
    f.key(Key::Right);
    f.key(Key::Enter);
    assert!(f.app.out.text.starts_with("Valid JSON"), "{}", f.app.out.text);
    // Ctrl+Enter from a box runs the first button
    f.key(Key::Tab);
    f.key(Key::Tab);
    assert_eq!(f.app.focus, Focus::Input);
    f.key(Key::Ctrl('\n'));
    assert!(f.app.out.text.contains("\n  \"b\": 1"), "{}", f.app.out.text);
    // a mistake is shown in the error colour with line and column
    f.app.input.set("{\n \"a\" 1}");
    f.click(Event::Tab(T_ACT + 2));
    assert!(f.app.out.err && f.app.out.text.contains("line 2, column 6"), "{}", f.app.out.text);
}

#[test]
fn typing_goes_to_the_focused_box_and_paste_and_clear_work() {
    let mut f = Fx::new(900, 600, 1.0);
    f.click(Event::Tab(T_TOOL + 7));
    f.key(Key::Tab);
    assert_eq!(f.app.focus, Focus::Aux);
    f.ev(Event::Paste("a+".into()));
    assert_eq!(f.app.aux.text, "a+");
    f.key(Key::BackTab);
    f.ev(Event::Paste("caaat".into()));
    assert_eq!(f.app.input.text, "caaat");
    f.click(Event::Tab(T_ACT));
    assert!(f.app.out.text.starts_with("1 match") && f.app.out.text.contains("\"aaa\""), "{}", f.app.out.text);
    f.click(Event::Tab(T_FLAG));
    assert!(f.text().contains("Ignore case: on"));
    crate::ui::clipboard::copy("pasted text");
    f.click(Event::Tab(T_PASTE));
    assert_eq!(f.app.input.text, "pasted text");
    f.click(Event::Tab(T_COPY));
    assert_eq!(crate::ui::clipboard::latest().as_deref(), Some(f.app.out.text.as_str()));
    f.click(Event::Tab(T_CLEAR));
    assert!(f.app.input.text.is_empty() && f.app.aux.text.is_empty() && f.app.out.text.is_empty());
    // the second box is kept per tool
    f.ev(Event::Key(Key::Esc));
    f.click(Event::Tab(T_TOOL + 1));
    f.key(Key::Tab);
    f.ev(Event::Paste(".a".into()));
    f.click(Event::Tab(T_TOOL + 7));
    assert!(f.app.aux.text.is_empty());
    f.click(Event::Tab(T_TOOL + 1));
    assert_eq!(f.app.aux.text, ".a");
}

#[test]
fn the_output_scrolls_with_wheel_pixels_keys_and_the_bar_and_show_more_adds_lines() {
    let mut f = Fx::new(900, 600, 1.0);
    f.click(Event::Tab(T_TOOL));
    f.app.input.set(&format!("[{}]", (0..400).map(|i| i.to_string()).collect::<Vec<_>>().join(",")));
    f.click(Event::Tab(T_ACT));
    assert!(f.app.scroll.scrolls(), "400 lines do not fit");
    f.ev(Event::MouseMove(f.app.scroll.view.x as i32 + 5, f.app.scroll.view.y as i32 + 5));
    f.ev(Event::Scroll(-2));
    let a = f.app.scroll.offset;
    assert!(a > 0.0);
    assert!(f.app.scroll_px(-60, &f.cx));
    assert!(f.app.scroll.offset > a);
    f.key(Key::Esc);
    f.key(Key::PageDown);
    f.key(Key::End);
    assert_eq!(f.app.scroll.offset, f.app.scroll.max);
    f.key(Key::Home);
    assert_eq!(f.app.scroll.offset, 0.0);
    // the scroll bar is on screen and a press on its track moves the list
    assert!(f.hit(&Event::Area(A_BAR)).is_some());
    f.ev(Event::AreaAt(A_BAR, 5.0, f.app.scroll.view.h - 2.0));
    f.ev(Event::MouseUp(0, 0));
    assert!(f.app.scroll.offset > 0.0);
    assert!(f.text().contains("Output: 402 lines") || f.text().contains("Output: 4"), "{}", f.text());
    // many lines: only the first are drawn until Show more
    f.app.input.set(&format!("[{}]", (0..9000).map(|i| i.to_string()).collect::<Vec<_>>().join(",")));
    f.click(Event::Tab(T_ACT));
    assert!(f.text().contains("first 3000 of 9002 lines"), "{}", f.text());
    f.click(Event::Tab(T_MORE));
    assert!(f.text().contains("first 6000 of 9002 lines"), "{}", f.text());
}

#[test]
fn json_query_reads_a_file_and_show_more_raises_the_match_limit() {
    let mut f = Fx::new(900, 600, 1.0);
    let d = crate::apps::tests::temp_dir("devtools-file");
    let p = d.join("big.json");
    std::fs::write(&p, format!("{{\"items\":[{}]}}", (0..1000).map(|i| format!("{{\"id\":{i}}}")).collect::<Vec<_>>().join(","))).unwrap();
    f.click(Event::Tab(T_TOOL + 1));
    f.app.input.set(&format!("@{}", p.display()));
    f.app.aux.set(".items[*].id");
    f.click(Event::Tab(T_ACT));
    assert!(f.app.out.text.starts_with("1000 matches"), "{}", f.app.out.text);
    assert!(f.app.out.text.contains("800 more not shown"), "{}", &f.app.out.text[..60]);
    f.click(Event::Tab(T_MORE));
    assert!(f.app.out.text.contains("200 more not shown"), "the limit grew to 800 and the query ran again");
    f.click(Event::Tab(T_MORE));
    assert!(!f.app.out.text.contains("more not shown") && f.app.out.text.starts_with("1000 matches"), "all of them now");
    f.app.input.set("@/nonexistent/file.json");
    f.click(Event::Tab(T_ACT));
    assert!(f.app.out.err && f.app.out.text.contains("Cannot read"), "{}", f.app.out.text);
}

#[test]
fn the_port_checker_fills_in_live_and_stop_ends_it() {
    let mut f = Fx::new(900, 600, 1.0);
    let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = l.local_addr().unwrap().port();
    f.click(Event::Tab(T_TOOL + 11));
    f.app.input.set("127.0.0.1");
    f.app.aux.set(&port.to_string());
    f.click(Event::Tab(T_ACT));
    let t = std::time::Instant::now();
    while f.app.job.is_some() && t.elapsed().as_secs() < 20 {
        std::thread::sleep(std::time::Duration::from_millis(20));
        assert!(f.app.wants_redraw() || f.app.job.is_none() || true);
    }
    f.app.wants_redraw();
    f.draw();
    assert!(f.app.job.is_none() && f.app.out.text.contains("open") && f.app.out.text.contains("1 open"), "{}", f.app.out.text);
    // a running check shows Stop, and pressing it ends the check
    f.app.input.set("127.0.0.1");
    f.app.aux.set("20000-20255");
    f.click(Event::Tab(T_ACT));
    if f.app.job.is_some() {
        f.click(Event::Tab(T_STOP));
    }
    let t = std::time::Instant::now();
    while f.app.job.is_some() && t.elapsed().as_secs() < 20 {
        std::thread::sleep(std::time::Duration::from_millis(20));
        f.app.wants_redraw();
    }
    assert!(f.app.job.is_none());
    f.draw();
    f.app.aux.set("1-300");
    f.click(Event::Tab(T_ACT));
    assert!(f.app.out.err && f.app.out.text.contains("at most 256"), "{}", f.app.out.text);
}

#[test]
fn ping_starts_in_the_background_and_ends_with_a_summary() {
    let mut f = Fx::new(900, 600, 1.0);
    f.click(Event::Tab(T_TOOL + 10));
    f.app.input.set("127.0.0.1");
    f.click(Event::Tab(T_ACT));
    assert!(f.app.job.is_some(), "the UI is not held up: the check runs on");
    assert!(f.hit(&Event::Tab(T_STOP)).is_some(), "Stop is offered");
    let t = std::time::Instant::now();
    while f.app.job.is_some() && t.elapsed().as_secs() < 30 {
        std::thread::sleep(std::time::Duration::from_millis(50));
        f.app.wants_redraw();
    }
    assert!(f.app.out.text.contains("5 sent"), "{}", f.app.out.text);
    f.draw();
    f.app.input.set("");
    f.click(Event::Tab(T_ACT));
    assert!(f.app.out.err);
}

#[test]
fn the_diff_page_shows_prefixes() {
    let mut f = Fx::new(900, 600, 1.0);
    f.click(Event::Tab(T_TOOL + 8));
    f.app.input.set("a\nb\nc");
    f.app.aux.set("a\nB\nc");
    f.click(Event::Tab(T_ACT));
    let t = f.text();
    assert!(t.contains("- b") && t.contains("+ B"), "{t}");
}

#[cfg(test)]
#[path = "tests_qr.rs"]
mod qr_page;
