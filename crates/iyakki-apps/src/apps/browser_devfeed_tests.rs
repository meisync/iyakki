// The reader's own network list and script notes reach the Developer tools buffers (and so the chat agent's tools and the panel) while
// something collects, scrubbed, and not otherwise. Included (include!) into the browser's test module.

fn tick_a_few(a: &mut BrowserApp, cx: &AppCx) {
    for _ in 0..5 {
        a.handle(&Event::Tick, cx);
    }
}

#[test]
fn the_readers_failed_requests_and_load_time_reach_the_agents_buffers_only_while_collecting_and_scrubbed() {
    let web = site(vec![("https://a.org/", html("<title>T</title><link rel=stylesheet href=\"/gone.css?token=abc123\"><p>hi</p>"))]);
    let cx = cx_with("brfeed1", web);
    let (dt, u) = (&cx.services.devtools(), user(&cx));
    let mut a = BrowserApp::new(&cx);
    a.open_url("a.org", &cx);
    settle(&mut a, &cx);
    tick_a_few(&mut a, &cx);
    assert!(a.net_log.lock().unwrap().iter().any(|n| n.url.contains("gone.css") && n.status.is_none()), "the reader's own list has the failure");
    assert_eq!(a.dev_snapshot_for_test(&cx), (0, 0, 0));
    assert_eq!(dt.summary(&u).failed, 0, "nothing is kept while nothing collects");
    // the person turns the switch on and loads the page again
    dt.set_enabled(&u, true);
    key(&mut a, &cx, Key::Char('r'));
    settle(&mut a, &cx);
    tick_a_few(&mut a, &cx);
    let failed = dt.failed_report(&u, &iyakki_agent::devtools::Scope::All, 10);
    assert!(failed.contains("https://a.org/gone.css") && !failed.contains("token") && !failed.contains("abc123"), "{failed}");
    let timings = dt.timings_report(&u, &iyakki_agent::devtools::Scope::All, 10);
    assert!(timings.contains("a.org"), "{timings}");
    let once = dt.summary(&u).failed;
    tick_a_few(&mut a, &cx);
    assert_eq!(dt.summary(&u).failed, once, "a line is handed over once");
    // turning it off forgets
    dt.set_enabled(&u, false);
    assert_eq!(dt.summary(&u).failed, 0);
    tick_a_few(&mut a, &cx);
    assert_eq!(a.dev_snapshot_for_test(&cx), (0, 0, 0), "and nothing comes back while off");
}

#[test]
fn script_errors_and_console_messages_of_a_reader_page_become_console_entries_for_the_panel_and_the_agent() {
    let body = "<title>S</title><p>x</p><script>console.error('boom happened'); console.warn('careful'); console.log('plainlogline'); nosuchfunction();</script>";
    let web = site(vec![("https://a.org/", html(body))]);
    let cx = cx_with("brfeed2", web);
    let (dt, u) = (&cx.services.devtools(), user(&cx));
    let mut a = BrowserApp::new(&cx);
    key(&mut a, &cx, Key::F(12));
    assert!(a.dev.open);
    a.open_url("a.org", &cx);
    settle(&mut a, &cx);
    tick_a_few(&mut a, &cx);
    let out = dt.console_report(&u, &iyakki_agent::devtools::Scope::All, "all", 20);
    assert!(out.contains("boom happened") && out.contains("careful"), "{out}");
    assert!(!out.contains("plainlogline"), "a plain log line is not a problem: {out}");
    assert!(out.to_lowercase().contains("nosuchfunction"), "the script's error is there: {out}");
    // the panel shows them, with the script's name as the source, and its button counts the errors
    key(&mut a, &cx, Key::Char('1'));
    let g = frame(&mut a, &cx, 1280, 720);
    let t = shown(&g);
    assert!(t.contains("boom happened"), "{t}");
    assert!(a.dev_counts.1 >= 2, "{:?}", a.dev_counts);
}

#[test]
fn script_notes_are_sorted_into_errors_warnings_and_things_that_are_not_messages() {
    use devfeed::note_to_console;
    use iyakki_shared::devtools::Level;
    assert_eq!(note_to_console("error in app.js: TypeError: x is undefined  [f (app.js:3) | g]"), Some((Level::Error, "app.js".into(), "TypeError: x is undefined  [f (app.js:3) | g]".into())));
    assert_eq!(note_to_console("could not load https://a.org/x.js: refused"), Some((Level::Error, "https://a.org/x.js".into(), "could not load: refused".into())));
    assert_eq!(note_to_console("warn: careful").map(|n| n.0), Some(Level::Warn));
    assert_eq!(note_to_console("warning: careful").map(|n| n.0), Some(Level::Warn));
    assert_eq!(note_to_console("error: boom").map(|n| n.0), Some(Level::Error));
    assert_eq!(note_to_console("scripts stopped: too many scripts").map(|n| n.0), Some(Level::Warn));
    assert!(note_to_console("log: hello").is_none());
    assert!(note_to_console("scripts: 3 ran, 0 failed, 0 skipped").is_none());
    assert!(note_to_console("Google's results page needs a full browser").is_none());
}

// ---------------------------------------------------------------------------------------------- the inspector (F-BROW-41)

fn insp_has(a: &BrowserApp, kind: browser_inspect::Kind, needle: &str) -> bool {
    a.dev.insp.lines.iter().any(|l| l.kind == kind && l.text.contains(needle))
}

fn eval_now(a: &mut BrowserApp, cx: &AppCx, expr: &str, wait_for: &str) {
    if !a.dev.insp.editing {
        key(a, cx, Key::Char('e'));
    }
    typ(a, cx, expr);
    key(a, cx, Key::Enter);
    for _ in 0..4000 {
        a.handle(&Event::Tick, cx);
        if a.dev.insp.lines.iter().any(|l| l.text.contains(wait_for) && l.kind != browser_inspect::Kind::In) {
            return;
        }
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    panic!("timed out waiting for {wait_for:?}; transcript {:?}", a.dev.insp.lines);
}

#[test]
fn the_prompt_evaluates_expressions_in_the_readers_engine_and_shows_values_errors_and_printed_lines() {
    use browser_inspect::Kind;
    let web = site(vec![("https://a.org/", html("<title>Insp</title><h1 id=top class=big>Hello</h1><p>one <b>two</b></p><script>var counter = 40;</script>"))]);
    let cx = cx_with("brinsp1", web);
    let mut a = BrowserApp::new(&cx);
    a.open_url("a.org", &cx);
    settle(&mut a, &cx);
    key(&mut a, &cx, Key::F(12));
    assert!(a.dev.open && a.dev.focus);
    assert!(!a.dev.insp.editing);
    // Enter or e starts the prompt; the first expression starts the engine and waits for it
    eval_now(&mut a, &cx, "counter + 2", "42");
    assert!(insp_has(&a, Kind::In, "> counter + 2"));
    assert!(insp_has(&a, Kind::Out, "42"), "the page's own variable is there");
    assert!(a.dev.insp.editing, "the prompt stays open for the next line");
    // text values are quoted, the page is the page, state stays between lines
    key(&mut a, &cx, Key::Enter);
    typ(&mut a, &cx, "document.title");
    key(&mut a, &cx, Key::Enter);
    assert!(insp_has(&a, Kind::Out, "\"Insp\""), "{:?}", a.dev.insp.lines);
    typ(&mut a, &cx, "var later = counter * 2; later");
    key(&mut a, &cx, Key::Enter);
    assert!(insp_has(&a, Kind::Out, "80"));
    typ(&mut a, &cx, "document.getElementById('top').className");
    key(&mut a, &cx, Key::Enter);
    assert!(insp_has(&a, Kind::Out, "\"big\""));
    typ(&mut a, &cx, "({a: 1, b: [1, 2]})");
    key(&mut a, &cx, Key::Enter);
    assert!(insp_has(&a, Kind::Out, "{\"a\":1,\"b\":[1,2]}"), "{:?}", a.dev.insp.lines);
    // something the expression prints, and an error with the place the engine names
    typ(&mut a, &cx, "console.log('printed here'); 7");
    key(&mut a, &cx, Key::Enter);
    assert!(insp_has(&a, Kind::Log, "printed here") && insp_has(&a, Kind::Out, "7"));
    typ(&mut a, &cx, "nosuchthing.go()");
    key(&mut a, &cx, Key::Enter);
    assert!(insp_has(&a, Kind::Err, "ReferenceError"), "{:?}", a.dev.insp.lines);
    // the arrow keys recall what was typed
    key(&mut a, &cx, Key::Up);
    assert_eq!(a.dev.insp.input, "nosuchthing.go()");
    key(&mut a, &cx, Key::Esc);
    assert!(a.dev.open && !a.dev.insp.editing, "Esc leaves the prompt, not the panel");
    // it is drawn: the transcript and the prompt row
    let g = frame(&mut a, &cx, 1280, 720);
    let t = shown(&g);
    assert!(t.contains("ReferenceError"), "{t}");
    assert!(g.hits.iter().any(|h| h.label == "Expression"));
    // the audit line names the host and nothing typed or returned
    let log = cx.audit.recent(60);
    assert!(log.iter().any(|l| l.contains("browser.devtools.eval") && l.contains("a.org")), "{log:?}");
    assert!(!log.iter().any(|l| l.contains("counter") || l.contains("80") || l.contains("Insp")), "no expression or value in the audit log");
    // nothing typed or returned reached the buffers the chat agent reads, and the text report does not carry it
    let u = user(&cx);
    let rep = cx.services.devtools().console_report(&u, &iyakki_agent::devtools::Scope::All, "all", 50);
    assert!(!rep.contains("counter") && !rep.contains("80"), "{rep}");
    a.handle(&Event::Tab(browser_devpanel::TAB_COPY), &cx);
    assert!(!crate::ui::clipboard::latest().unwrap_or_default().contains("counter"));
}

#[test]
fn the_elements_page_shows_the_tree_attributes_and_text_and_follows_the_scripts_once_the_engine_runs() {
    let web = site(vec![("https://a.org/", html("<title>Tree</title><div id=box class=\"x y\"><p>Para <b>bold</b></p><a href=\"/go\" title=Go>link</a></div>"))]);
    let cx = cx_with("brinsp2", web);
    let mut a = BrowserApp::new(&cx);
    a.open_url("a.org", &cx);
    settle(&mut a, &cx);
    key(&mut a, &cx, Key::F(12));
    key(&mut a, &cx, Key::Char('4'));
    assert_eq!(a.dev.tab, DevTab::Elements);
    assert!(a.dev.insp.has_tree() && !a.dev.insp.live, "the page as the server sent it");
    let rows = a.dev.insp.rows();
    let div = rows.iter().position(|r| r.label.contains("id=\"box\"")).expect("the div");
    // Down walks the rows; the detail is the chosen node
    for _ in 0..div {
        key(&mut a, &cx, Key::Down);
    }
    let d = a.dev.insp.detail().join("\n");
    assert!(d.contains("id=\"box\"") && d.contains("class=\"x y\"") && d.contains("text: Para bold link"), "{d}");
    key(&mut a, &cx, Key::Right);
    key(&mut a, &cx, Key::Right);
    let a_row = a.dev.insp.rows().iter().position(|r| r.label == "<a>").unwrap();
    while a.dev.insp.sel < a_row {
        key(&mut a, &cx, Key::Down);
    }
    let d = a.dev.insp.detail().join("\n");
    assert!(d.contains("href=\"/go\"") && d.contains("title=\"Go\"") && d.contains("text: link"), "{d}");
    // it is drawn, rows can be clicked, and the report lists the tree
    let g = frame(&mut a, &cx, 1280, 720);
    let t = shown(&g);
    assert!(t.contains("<a>") && t.contains("href=\"/go\""), "{t}");
    let row_hit = g.hits.iter().find(|h| h.label == "element row").expect("rows are clickable");
    let Event::Tab(n) = row_hit.ev else { panic!() };
    a.handle(&Event::Tab(n), &cx);
    assert_eq!(a.dev.insp.sel, n - browser_devpanel::TAB_ROW + a.dev.insp.scroll);
    a.handle(&Event::Tab(browser_devpanel::TAB_COPY), &cx);
    assert!(crate::ui::clipboard::latest().unwrap_or_default().contains("<div id=\"box\""));
    // after an expression changes the page, Refresh shows the live tree
    key(&mut a, &cx, Key::Char('1'));
    eval_now(&mut a, &cx, "document.getElementById('box').setAttribute('data-n', '5'); 99", "99");
    key(&mut a, &cx, Key::Esc);
    key(&mut a, &cx, Key::Char('4'));
    assert!(a.dev.insp.live);
    let live = a.dev.insp.rows();
    assert!(live.iter().any(|r| r.label.contains("id=\"box\"")));
    // Tab moves between the tabs and comes back
    key(&mut a, &cx, Key::Tab);
    assert_eq!(a.dev.tab, DevTab::Console);
    // closing the panel forgets the tree and the engine
    key(&mut a, &cx, Key::F(12));
    assert!(!a.dev.open && !a.dev.insp.has_tree() && a.inspect.live.is_none());
}

#[test]
fn the_prompt_says_why_when_scripts_are_off_or_the_full_engine_shows_the_page_and_a_new_page_forgets_the_engine() {
    use browser_inspect::Kind;
    let web = site(vec![("https://a.org/", html("<title>A</title><p>a</p>")), ("https://b.org/", html("<title>B</title><p>b</p>"))]);
    let cx = cx_with("brinsp3", web);
    let mut a = BrowserApp::new(&cx);
    a.open_url("a.org", &cx);
    settle(&mut a, &cx);
    key(&mut a, &cx, Key::F(12));
    // scripts off for the site: refused with the way out
    let mut c = browser_config::load(&cx);
    c.js_all = false;
    browser_config::save(&cx, &c);
    a.cfg = browser_config::load(&cx);
    key(&mut a, &cx, Key::Char('e'));
    typ(&mut a, &cx, "1+1");
    key(&mut a, &cx, Key::Enter);
    assert!(insp_has(&a, Kind::Note, "Scripts are off for a.org") && a.inspect.boot.is_none() && a.inspect.live.is_none());
    // on again: it works, and another page does not inherit the engine
    c.js_all = true;
    browser_config::save(&cx, &c);
    a.cfg = browser_config::load(&cx);
    eval_now(&mut a, &cx, "document.title", "\"A\"");
    key(&mut a, &cx, Key::Esc);
    a.open_url("b.org", &cx);
    settle(&mut a, &cx);
    for _ in 0..3 {
        a.handle(&Event::Tick, &cx);
    }
    assert!(a.inspect.live.is_none() && a.inspect.page.is_none(), "the new page has no engine yet");
    assert!(insp_has(&a, Kind::Note, "a new page"));
    eval_now(&mut a, &cx, "document.title", "\"B\"");
}

#[test]
fn the_full_engine_console_prompt_sends_the_expression_to_the_page_and_the_elements_tab_says_it_is_the_readers_only() {
    use browser_inspect::Kind;
    let (_fake, cx, mut a, _g) = in_full("dev_insp_full", "--static");
    key(&mut a, &cx, Key::F(12));
    key(&mut a, &cx, Key::Char('e'));
    typ(&mut a, &cx, "1+1");
    key(&mut a, &cx, Key::Enter);
    // since the console went through the engine's Script call the prompt works in the full engine: the line is kept and the
    // expression waits for the page's answer (the answers are in browser_devpanel_tests); the reader's own script engine is not started
    assert!(insp_has(&a, Kind::In, "1+1"), "{:?}", a.dev.insp.lines);
    assert_eq!(a.inspect.full_pending.len(), 1);
    assert!(a.inspect.boot.is_none());
    key(&mut a, &cx, Key::Esc);
    key(&mut a, &cx, Key::Char('4'));
    let g = frame(&mut a, &cx, 1280, 720);
    assert!(shown(&g).contains("Elements page shows pages the built-in reader shows"));
}
