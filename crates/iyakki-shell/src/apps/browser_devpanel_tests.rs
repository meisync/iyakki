// The browser's Developer tools panel (F12): keys and buttons, tabs and filters over the fake helper's `fake://dev` lines, what is
// collected and when, the counters, and the layout at four window sizes. Included (include!) into the browser's test module.

use browser_devpanel::{Dock, Tab as DevTab};

fn user(cx: &AppCx) -> String {
    cx.session.user.clone()
}

/// The fake page that reports a failed request, a console error and page timings (the lines of `iyakki-fake-webview`).
fn dev_page(a: &mut BrowserApp, cx: &AppCx) {
    let want = cx.services.devtools.seq(&user(cx));
    a.full.as_mut().unwrap().navigate("fake://dev");
    pump(a, cx, "the developer lines", |a| {
        let s = a.dev_snapshot_for_test(cx);
        s.0 >= 1 && s.1 >= 1 && s.2 >= 1 && cx.services.devtools.seq(&user(cx)) != want
    });
}

impl BrowserApp {
    /// (failed, console, page timings) in the shared buffers.
    fn dev_snapshot_for_test(&self, cx: &AppCx) -> (usize, usize, usize) {
        let s = cx.services.devtools.snapshot(&user(cx));
        (s.failed.len(), s.console.len(), s.pages.len())
    }
}

fn shown(g: &Gfx) -> String {
    g.text_log.iter().map(|t| t.2.as_str()).collect::<Vec<_>>().join("\n")
}

fn dev_hits(g: &Gfx) -> Vec<(usize, R)> {
    g.hits.iter().filter_map(|h| if let Event::Tab(n) = h.ev { browser_devpanel::is_tab(n).then_some((n, h.rect)) } else { None }).collect()
}

fn overlap(a: R, b: R) -> bool {
    a.x < b.x + b.w - 0.5 && b.x < a.x + a.w - 0.5 && a.y < b.y + b.h - 0.5 && b.y < a.y + a.h - 0.5
}

fn inside(inner: R, outer: R) -> bool {
    inner.x >= outer.x - 0.5 && inner.y >= outer.y - 0.5 && inner.x + inner.w <= outer.x + outer.w + 0.5 && inner.y + inner.h <= outer.y + outer.h + 0.5
}

#[test]
fn f12_ctrl_shift_i_and_the_button_open_and_close_the_panel_and_esc_closes_it() {
    let (_fake, cx, mut a, _g) = in_full("dev_open", "--static");
    let dt = &cx.services.devtools;
    let u = user(&cx);
    assert!(!a.dev.open && !dt.panel_open(&u));
    // the button is in the tool bar with its key in the label
    let g = frame(&mut a, &cx, 1280, 720);
    let hit = g.hits.iter().find(|h| h.ev == Event::Tab(browser_devpanel::TAB_TOGGLE)).expect("the Dev tools button");
    assert!(hit.label.contains("Dev tools") && hit.label.contains("F12"), "{}", hit.label);
    key(&mut a, &cx, Key::F(12));
    assert!(a.dev.open && a.dev.focus && dt.panel_open(&u), "F12 opens it and collecting starts");
    assert!(!dt.enabled(&u), "the saved switch for the chat agent is untouched");
    let g = frame(&mut a, &cx, 1280, 720);
    assert!(g.hits.iter().any(|h| h.ev == Event::Tab(browser_devpanel::TAB_CLOSE)) && shown(&g).contains("Kept in memory only"), "the note is there");
    key(&mut a, &cx, Key::Esc);
    assert!(!a.dev.open && !dt.panel_open(&u), "Esc closes it");
    assert!(a.full.is_some(), "and leaves the engine alone");
    key(&mut a, &cx, Key::Ctrl('I'));
    assert!(a.dev.open, "Ctrl+Shift+I opens it");
    key(&mut a, &cx, Key::Ctrl('I'));
    assert!(!a.dev.open, "and closes it");
    a.handle(&Event::Tab(browser_devpanel::TAB_TOGGLE), &cx);
    assert!(a.dev.open, "the button opens it");
    frame(&mut a, &cx, 1280, 720);
    a.handle(&Event::Tab(browser_devpanel::TAB_CLOSE), &cx);
    assert!(!a.dev.open, "its Close button closes it");
    a.handle(&Event::Tab(browser_devpanel::TAB_TOGGLE), &cx);
    a.handle(&Event::Tab(browser_devpanel::TAB_TOGGLE), &cx);
    assert!(!a.dev.open && !dt.panel_open(&u), "the toolbar button toggles");
    // opening and closing is in the audit log, with no page data
    let audit = std::fs::read_to_string(cx.services.dir().parent().unwrap().join("audit.log")).unwrap_or_default();
    assert!(audit.contains("browser.devtools.panel"), "{audit}");
}

#[test]
fn the_panel_has_the_keyboard_until_the_page_is_clicked_and_ctrl_shortcuts_still_work() {
    let (_fake, cx, mut a, _g) = in_full("dev_focus", "--static");
    key(&mut a, &cx, Key::F(12));
    // a letter is the panel's, not the browser's (r would reload, F would go back to the reader)
    key(&mut a, &cx, Key::Char('F'));
    assert!(a.full.is_some() && a.dev.open);
    key(&mut a, &cx, Key::Char('2'));
    assert_eq!(a.dev.tab, DevTab::Network);
    key(&mut a, &cx, Key::Tab);
    assert_eq!(a.dev.tab, DevTab::Timings);
    key(&mut a, &cx, Key::BackTab);
    key(&mut a, &cx, Key::Left);
    assert_eq!(a.dev.tab, DevTab::Console);
    // clicking the page gives it the keyboard back; the panel stays open
    frame(&mut a, &cx, 1280, 720);
    a.handle(&Event::AreaAt(AREA_FULL, 100.0, 100.0), &cx);
    assert!(a.dev.open && !a.dev.focus);
    key(&mut a, &cx, Key::Char('x'));
    pump(&mut a, &cx, "the key reaching the page", |a| a.full.as_ref().is_some_and(|f| f.log.iter().any(|l| l.contains("Key {"))));
    // F12 still closes it from the page
    key(&mut a, &cx, Key::F(12));
    assert!(!a.dev.open);
    // Ctrl shortcuts work while the panel has the keyboard
    key(&mut a, &cx, Key::F(12));
    let tabs = a.tab_count();
    key(&mut a, &cx, Key::Ctrl('t'));
    assert_eq!(a.tab_count(), tabs + 1, "Ctrl+T still opens a tab");
}

#[test]
fn console_network_and_timings_show_the_scrubbed_lines_and_the_filters_work() {
    let (_fake, cx, mut a, _g) = in_full("dev_tabs", "--static");
    key(&mut a, &cx, Key::F(12));
    dev_page(&mut a, &cx);
    let g = frame(&mut a, &cx, 1280, 720);
    let t = shown(&g);
    assert!(t.contains("TypeError: x is undefined") && t.contains("https://a.org/app.js:12:3"), "console row with source and line: {t}");
    assert!(t.contains("error"), "{t}");
    assert!(t.contains("Console 1"), "the tab has a count: {t}");
    // network: the failed request, without its query string
    key(&mut a, &cx, Key::Char('2'));
    let g = frame(&mut a, &cx, 1280, 720);
    let t = shown(&g);
    assert!(t.contains("FAIL") && t.contains("https://a.org/missing.js") && t.contains("status 404") && t.contains("120 ms"), "{t}");
    assert!(!t.contains("token=abc") && !t.contains("?token"), "no query string is shown: {t}");
    // timings: the three times and a bar
    key(&mut a, &cx, Key::Char('3'));
    let g = frame(&mut a, &cx, 1280, 720);
    let t = shown(&g);
    assert!(t.contains("first response 80 ms") && t.contains("DOMContentLoaded 300 ms") && t.contains("load 900 ms"), "{t}");
    let ctx = a.dev_ctx();
    // filters: text, level, failed only, sort
    key(&mut a, &cx, Key::Char('1'));
    key(&mut a, &cx, Key::Char('/'));
    assert!(a.dev.editing);
    typ(&mut a, &cx, "zzz");
    key(&mut a, &cx, Key::Enter);
    assert!(!a.dev.editing && a.dev.filter == "zzz");
    let rep = a.dev.report(ctx, &[]);
    assert!(rep.contains("No console errors or warnings.") && rep.contains("zzz"), "{rep}");
    key(&mut a, &cx, Key::Char('/'));
    for _ in 0..3 {
        key(&mut a, &cx, Key::Backspace);
    }
    typ(&mut a, &cx, "TYPEERR");
    key(&mut a, &cx, Key::Esc);
    assert!(a.dev.open, "Esc leaves the filter first");
    assert!(a.dev.report(ctx, &[]).contains("TypeError: x is undefined"), "case-insensitive text filter");
    key(&mut a, &cx, Key::Char('l'));
    assert_eq!(a.dev.level, browser_devpanel::LevelFilter::Errors);
    assert!(a.dev.report(ctx, &[]).contains("errors only"));
    key(&mut a, &cx, Key::Char('l'));
    assert!(a.dev.report(ctx, &[]).contains("No console errors or warnings."), "warnings only hides the error");
    key(&mut a, &cx, Key::Char('l'));
    // network toggles
    key(&mut a, &cx, Key::Char('2'));
    key(&mut a, &cx, Key::Char('/'));
    for _ in 0..7 {
        key(&mut a, &cx, Key::Backspace);
    }
    key(&mut a, &cx, Key::Enter);
    key(&mut a, &cx, Key::Char('f'));
    assert!(a.dev.failed_only && a.dev.report(ctx, &[]).contains("failed only"));
    key(&mut a, &cx, Key::Char('s'));
    assert_eq!(a.dev.sort, browser_devpanel::Sort::Duration);
    key(&mut a, &cx, Key::Char('x'));
    assert!(a.dev.report(ctx, &[]).contains("No requests to show."), "Clear hides the rows");
    // Clear hides them here only: the shared buffers keep them for whoever else reads them
    assert_eq!(a.dev_snapshot_for_test(&cx).0, 1);
    // the mouse does the same
    frame(&mut a, &cx, 1280, 720);
    a.handle(&Event::Tab(browser_devpanel::TAB_FAILED), &cx);
    assert!(!a.dev.failed_only);
    a.handle(&Event::Tab(browser_devpanel::TAB_CONSOLE), &cx);
    assert_eq!(a.dev.tab, DevTab::Console);
    a.handle(&Event::Tab(browser_devpanel::TAB_FILTER), &cx);
    assert!(a.dev.editing);
    a.handle(&Event::Paste("app.js".into()), &cx);
    assert_eq!(a.dev.filter, "app.js");
}

#[test]
fn what_is_copied_is_the_shown_rows_scrubbed() {
    let (_fake, cx, mut a, _g) = in_full("dev_copy", "--static");
    key(&mut a, &cx, Key::F(12));
    dev_page(&mut a, &cx);
    crate::ui::clipboard::clear();
    key(&mut a, &cx, Key::Char('2'));
    key(&mut a, &cx, Key::Char('y'));
    let text = crate::ui::clipboard::latest().expect("copied");
    assert!(text.contains("Network") && text.contains("https://a.org/missing.js") && text.contains("status 404"), "{text}");
    for secret in ["token=abc", "?token", "Cookie", "Authorization"] {
        assert!(!text.contains(secret), "{secret} in {text}");
    }
    key(&mut a, &cx, Key::Char('1'));
    a.handle(&Event::Tab(browser_devpanel::TAB_COPY), &cx);
    let text = crate::ui::clipboard::latest().unwrap();
    assert!(text.contains("Console") && text.contains("TypeError: x is undefined  (https://a.org/app.js:12:3)"), "{text}");
}

#[test]
fn nothing_is_collected_before_the_panel_opens_or_after_it_closes_and_closing_forgets() {
    let (_fake, cx, mut a, _g) = in_full("dev_collect", "--static");
    let dt = &cx.services.devtools;
    let u = user(&cx);
    // neither the panel nor the switch: the lines are dropped
    a.full.as_mut().unwrap().navigate("fake://dev");
    for _ in 0..80 {
        a.handle(&Event::Tick, &cx);
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    assert_eq!(dt.seq(&u), 0, "nothing kept");
    key(&mut a, &cx, Key::F(12));
    dev_page(&mut a, &cx);
    assert!(dt.summary(&u).failed >= 1 && dt.summary(&u).console_errors >= 1);
    key(&mut a, &cx, Key::Esc);
    assert_eq!(dt.seq(&u), 0, "closing forgets everything");
    assert!(dt.snapshot(&u).console.is_empty());
    a.full.as_mut().unwrap().navigate("fake://dev");
    for _ in 0..80 {
        a.handle(&Event::Tick, &cx);
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    assert_eq!(dt.seq(&u), 0, "and collecting has stopped");
    assert!(std::fs::read_dir(cx.services.dir()).unwrap().flatten().all(|e| !e.file_name().to_string_lossy().starts_with("devtools-")), "no page data and no choice written to disk");
}

#[test]
fn with_the_chat_agent_switch_on_the_panel_shows_the_same_buffers_and_closing_keeps_them() {
    let (_fake, cx, mut a, _g) = in_full("dev_shared", "--static");
    let dt = &cx.services.devtools;
    let u = user(&cx);
    dt.set_enabled(&u, true);
    key(&mut a, &cx, Key::F(12));
    dev_page(&mut a, &cx);
    let before = dt.summary(&u);
    key(&mut a, &cx, Key::Esc);
    assert!(dt.enabled(&u), "the switch is still on");
    assert_eq!(dt.summary(&u), before, "the agent's buffers survive the panel");
    // closed panel, switch on: the button still counts
    a.handle(&Event::Tick, &cx);
    let g = frame(&mut a, &cx, 1280, 720);
    let label = g.hits.iter().find(|h| h.ev == Event::Tab(browser_devpanel::TAB_TOGGLE)).map(|h| h.label.clone()).unwrap();
    assert!(label.contains("1req") && label.contains("1err"), "counters while collecting: {label}");
}

#[test]
fn the_button_counts_failed_requests_and_console_errors_only_while_collecting() {
    let (_fake, cx, mut a, _g) = in_full("dev_counts", "--static");
    let label = |a: &mut BrowserApp| frame(a, &cx, 1280, 720).hits.iter().find(|h| h.ev == Event::Tab(browser_devpanel::TAB_TOGGLE)).map(|h| h.label.clone()).unwrap();
    assert_eq!(label(&mut a), "Dev tools F12", "no counters when nothing is collected");
    key(&mut a, &cx, Key::F(12));
    assert_eq!(label(&mut a), "Dev tools Esc", "open: the key to close it");
    dev_page(&mut a, &cx);
    a.handle(&Event::Tick, &cx);
    assert!(label(&mut a).contains("1req 1err"), "{}", label(&mut a));
    key(&mut a, &cx, Key::F(12));
    assert_eq!(label(&mut a), "Dev tools F12", "closed again: no counters");
}

#[test]
fn the_panel_is_drawn_again_when_new_lines_arrive() {
    let (_fake, cx, mut a, _g) = in_full("dev_redraw", "--static");
    key(&mut a, &cx, Key::F(12));
    frame(&mut a, &cx, 1280, 720);
    while a.wants_redraw() {}
    a.full.as_mut().unwrap().navigate("fake://dev");
    let mut asked = false;
    for _ in 0..800 {
        a.handle(&Event::Tick, &cx);
        if a.redraw_kind() == Redraw::Full && a.dev_snapshot_for_test(&cx).1 >= 1 {
            asked = true;
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    assert!(asked, "new console lines ask for a whole redraw");
    // and an idle panel asks for nothing
    frame(&mut a, &cx, 1280, 720);
    for _ in 0..5 {
        a.handle(&Event::Tick, &cx);
    }
    while a.wants_redraw() {}
    a.handle(&Event::Tick, &cx);
    assert!(!matches!(a.redraw_kind(), Redraw::Full) || a.full.as_ref().unwrap().frames > 0);
}

#[test]
fn the_panel_says_plainly_when_the_page_probe_is_not_there() {
    let (_fake, cx, mut a, _g) = in_full("dev_noprobe", "--static");
    key(&mut a, &cx, Key::F(12));
    let t = shown(&frame(&mut a, &cx, 1280, 720));
    assert!(t.contains("no page probe") && t.contains("full web build"), "{t}");
    // a probe that reported something and found nothing wrong says that instead
    dev_page(&mut a, &cx);
    key(&mut a, &cx, Key::Char('/'));
    typ(&mut a, &cx, "nothing like this");
    key(&mut a, &cx, Key::Enter);
    let t = shown(&frame(&mut a, &cx, 1280, 720));
    assert!(t.contains("Nothing matches the filter"), "{t}");
}

#[test]
fn the_built_in_reader_lists_its_own_requests_scrubbed_and_points_to_the_full_engine() {
    let web = site(vec![("https://a.org/", html(GUIDE))]);
    let cx = cx_with("dev_reader", web);
    let mut a = BrowserApp::new(&cx);
    typ(&mut a, &cx, "a.org");
    key(&mut a, &cx, Key::Enter);
    settle(&mut a, &cx);
    assert!(a.full.is_none());
    a.net_log.lock().unwrap().push(browser_devtools::NetLine { method: "GET".into(), url: "https://cdn.example/app.js?token=ABCDEF0123456789ABCDEF".into(), status: Some(404), bytes: 12, ms: 90, error: String::new() });
    key(&mut a, &cx, Key::F(12));
    assert!(a.dev.open, "the panel opens in the reader too");
    key(&mut a, &cx, Key::Char('2'));
    a.handle(&Event::Tick, &cx);
    let t = shown(&frame(&mut a, &cx, 1280, 720));
    assert!(t.contains("https://cdn.example/app.js") && t.contains("404"), "{t}");
    assert!(!t.contains("ABCDEF0123456789") && !t.contains("token="), "{t}");
    key(&mut a, &cx, Key::Char('y'));
    let copied = crate::ui::clipboard::latest().unwrap();
    assert!(copied.contains("https://cdn.example/app.js") && !copied.contains("ABCDEF0123456789"), "{copied}");
    key(&mut a, &cx, Key::Char('1'));
    let t = shown(&frame(&mut a, &cx, 1280, 720));
    assert!(t.contains("built-in reader") || t.contains("does not have"), "{t}");
    key(&mut a, &cx, Key::Esc);
    assert!(!a.dev.open);
}

#[test]
fn the_panel_and_the_page_never_overlap_at_the_four_window_sizes_in_both_docks() {
    for (w, h) in [(1280u32, 720u32), (1024, 600), (800, 480), (480, 600)] {
        let (_fake, cx, mut a, _g) = in_full(&format!("dev_layout_{w}"), "--static");
        frame(&mut a, &cx, w, h);
        let closed = a.view_rect;
        key(&mut a, &cx, Key::F(12));
        for dock in [Dock::Bottom, Dock::Right] {
            a.dev.dock = Some(dock);
            let _g = frame(&mut a, &cx, w, h);
            let (page, panel) = (a.view_rect, a.dev.rect);
            let whole = R::new(closed.x, closed.y, closed.w, closed.h);
            let tag = format!("{w}x{h} {dock:?}: page {page:?} panel {panel:?}");
            assert!(!overlap(page, panel), "{tag}");
            assert!(inside(page, whole) && inside(panel, whole), "{tag}");
            match dock {
                Dock::Bottom => {
                    assert_eq!((panel.x, panel.w), (whole.x, whole.w), "{tag}");
                    assert!((page.y + page.h - panel.y).abs() < 0.5 && (panel.y + panel.h - (whole.y + whole.h)).abs() < 0.5, "{tag}");
                    let share = panel.h / whole.h;
                    assert!((0.3..=0.45).contains(&share), "about 35 %: {share} {tag}");
                }
                Dock::Right => {
                    assert_eq!((panel.y, panel.h), (whole.y, whole.h), "{tag}");
                    assert!((page.x + page.w - panel.x).abs() < 0.5 && (panel.x + panel.w - (whole.x + whole.w)).abs() < 0.5, "{tag}");
                    let share = panel.w / whole.w;
                    assert!((0.3..=0.65).contains(&share), "{share} {tag}");
                }
            }
            // the engine is told the size of what is left
            let want = (page.w as u32, page.h as u32);
            pump(&mut a, &cx, "the engine resized", |a| {
                frame(a, &cx, w, h);
                a.full.as_ref().is_some_and(|f| f.frame_size() == want)
            });
            // every button of the panel lies inside it, none covers another, and the tabs and Close are always there
            let g = frame(&mut a, &cx, w, h);
            let hits = dev_hits(&g);
            for (n, r) in &hits {
                if *n != browser_devpanel::TAB_TOGGLE {
                    assert!(inside(*r, panel), "button {n} {r:?} outside the panel: {tag}");
                }
            }
            let own: Vec<_> = hits.iter().filter(|(n, _)| *n != browser_devpanel::TAB_TOGGLE).collect();
            for (i, (n1, r1)) in own.iter().enumerate() {
                for (n2, r2) in own.iter().skip(i + 1) {
                    assert!(!overlap(*r1, *r2), "buttons {n1} {r1:?} and {n2} {r2:?} overlap: {tag}");
                }
            }
            for need in [browser_devpanel::TAB_CONSOLE, browser_devpanel::TAB_CLOSE] {
                assert!(own.iter().any(|(n, _)| *n == need), "button {need} missing: {tag}");
            }
            for h in g.hits.iter().filter(|h| matches!(h.ev, Event::Area(browser_devpanel::AREA_BODY) | Event::Area(browser_devpanel::AREA_SPLIT))) {
                assert!(inside(h.rect, panel), "{:?} {tag}", h.label);
            }
            // the page's own hit region stays out of the panel
            let area = g.hits.iter().find(|h| h.ev == Event::Area(AREA_FULL)).unwrap().rect;
            assert!(!overlap(area, panel) && area == page, "{tag}");
            // the tool bar: the Dev tools button does not cover any other button
            let bar: Vec<_> = g.hits.iter().filter(|h| matches!(h.ev, Event::Tab(n) if n <= 10 || n == browser_devpanel::TAB_TOGGLE || n == 5) && h.rect.y < closed.y).collect();
            for (i, x) in bar.iter().enumerate() {
                for y in bar.iter().skip(i + 1) {
                    assert!(!overlap(x.rect, y.rect), "{:?} and {:?} overlap in the tool bar: {tag}", x.label, y.label);
                }
            }
            // text stays inside the window
            for (x, y, _) in &g.text_log {
                assert!(*x < w as f32 && *y < h as f32, "{tag}");
            }
        }
        // every size shows the button with its key, or (a very narrow bar) leaves it to the keys
        key(&mut a, &cx, Key::F(12));
        let g = frame(&mut a, &cx, w, h);
        let has = g.hits.iter().any(|h| h.ev == Event::Tab(browser_devpanel::TAB_TOGGLE));
        assert!(has || w < 600, "{w}x{h}: the Dev tools button is in the tool bar");
        let after = a.view_rect;
        assert_eq!(after, closed, "{w}x{h}: closing gives the page its whole area back");
    }
}

#[test]
fn the_divider_resizes_the_panel_with_the_mouse_and_plus_minus_with_the_keyboard() {
    let (_fake, cx, mut a, _g) = in_full("dev_drag", "--static");
    key(&mut a, &cx, Key::F(12));
    let g = frame(&mut a, &cx, 1280, 720);
    assert!(g.hits.iter().any(|h| h.ev == Event::Area(browser_devpanel::AREA_SPLIT)), "the divider is a hit region");
    let h0 = a.dev.rect.h;
    crate::ui::set_dragging(false);
    a.handle(&Event::AreaAt(browser_devpanel::AREA_SPLIT, 100.0, 2.0), &cx);
    crate::ui::set_dragging(true);
    a.handle(&Event::AreaAt(browser_devpanel::AREA_SPLIT, 100.0, -60.0), &cx);
    crate::ui::set_dragging(false);
    a.handle(&Event::MouseUp(100, 100), &cx);
    frame(&mut a, &cx, 1280, 720);
    assert!(a.dev.rect.h > h0 + 40.0, "dragging up makes it taller: {} -> {}", h0, a.dev.rect.h);
    let h1 = a.dev.rect.h;
    key(&mut a, &cx, Key::Char('-'));
    frame(&mut a, &cx, 1280, 720);
    assert!(a.dev.rect.h < h1);
    for _ in 0..30 {
        key(&mut a, &cx, Key::Char('+'));
    }
    frame(&mut a, &cx, 1280, 720);
    assert!(a.dev.rect.h / (a.dev.rect.h + a.view_rect.h) <= 0.71, "never more than 70 %");
    // the dock key
    key(&mut a, &cx, Key::Char('d'));
    frame(&mut a, &cx, 1280, 720);
    assert_eq!(a.dev.dock_now, Dock::Right);
    assert!(a.dev.rect.x > a.view_rect.x);
}

#[test]
fn the_wheel_over_the_panel_scrolls_its_list_and_not_the_page() {
    let (_fake, cx, mut a, _g) = in_full("dev_wheel", "--static");
    key(&mut a, &cx, Key::F(12));
    let before = a.full.as_ref().unwrap().log.len();
    frame(&mut a, &cx, 1280, 720);
    let r = a.dev.rect;
    a.handle(&Event::MouseMove((r.x + 50.0) as i32, (r.y + r.h / 2.0) as i32), &cx);
    a.handle(&Event::Scroll(-1), &cx);
    let after: Vec<String> = a.full.as_ref().unwrap().log.iter().skip(before).cloned().collect();
    assert!(!after.iter().any(|l| l.contains("Wheel")), "{after:?}");
    // over the page it still goes to the page
    let v = a.view_rect;
    a.handle(&Event::MouseMove((v.x + 100.0) as i32, (v.y + 100.0) as i32), &cx);
    a.handle(&Event::Scroll(-1), &cx);
    pump(&mut a, &cx, "the wheel on the page", |a| a.full.as_ref().is_some_and(|f| f.log.iter().any(|l| l.contains("Wheel"))));
}
