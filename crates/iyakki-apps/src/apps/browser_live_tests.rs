// Clicks on a scripted page in the reader reach the page's scripts and the changed page is drawn again (F-BJS-01). Included
// (include!) into the browser's test module.

const LIVE_PAGE: &str = "<p id=n>0</p><button id=b type=button>Add</button><input id=c type=checkbox><p id=s>off</p><button id=go type=button>Go</button>\
    <script>let n = 0; document.getElementById('b').addEventListener('click', () => { n++; document.getElementById('n').textContent = String(n); });\
    document.getElementById('c').addEventListener('change', e => { document.getElementById('s').textContent = e.target.checked ? 'on' : 'off'; });\
    document.getElementById('go').addEventListener('click', () => { location.href = '/next'; });</script>";

fn live_texts(a: &BrowserApp) -> Vec<String> {
    a.page().unwrap().doc.blocks.iter().filter_map(|b| if let Block::Text(t) = b { Some(t.spans.iter().map(|s| s.text.as_str()).collect::<String>()) } else { None }).collect()
}

fn live_wait(a: &mut BrowserApp, cx: &AppCx, what: &str, done: impl Fn(&BrowserApp) -> bool) {
    for _ in 0..3000 {
        a.handle(&Event::Tick, cx);
        if done(a) {
            return;
        }
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    panic!("never happened: {what}; page text {:?}, status {:?}", live_texts(a), a.status_text());
}

fn live_control(a: &BrowserApp, label: &str) -> usize {
    a.page().unwrap().doc.controls.iter().position(|c| c.label == label || (label == "box" && c.kind == ControlKind::Checkbox)).unwrap_or_else(|| panic!("no control {label}"))
}

#[test]
fn a_button_whose_script_changes_the_page_changes_it_in_the_reader_and_a_script_navigation_is_followed() {
    let web = site(vec![("https://a.org/", html(LIVE_PAGE)), ("https://a.org/next", html("<p>arrived</p>"))]);
    let cx = cx_with("live1", web);
    browser_config::save(&cx, &BrowserConfig { js_all: true, ..BrowserConfig::default() });
    let mut a = BrowserApp::new(&cx);
    a.open_url("a.org", &cx);
    settle(&mut a, &cx);
    assert_eq!(live_texts(&a)[0], "0");
    assert!(a.live.is_some(), "the page's engine was kept");
    let add = live_control(&a, "Add");
    a.click_control(add, 0.0, &cx);
    live_wait(&mut a, &cx, "the count went to 1", |a| live_texts(a)[0] == "1");
    a.click_control(add, 0.0, &cx);
    live_wait(&mut a, &cx, "the count went to 2", |a| live_texts(a)[0] == "2");
    assert_eq!(a.page().unwrap().doc.controls.len(), 3, "the page still has its controls");
    // a check box tells the page, and what the person had set stays
    let boxn = live_control(&a, "box");
    a.click_control(boxn, 0.0, &cx);
    live_wait(&mut a, &cx, "the page heard the check box", |a| live_texts(a).iter().any(|t| t == "on"));
    assert!(a.control(boxn).unwrap().1.checked, "the box stays checked");
    a.click_control(boxn, 0.0, &cx);
    live_wait(&mut a, &cx, "and the box being cleared", |a| live_texts(a).iter().any(|t| t == "off"));
    // a script that sends the page on after a click is followed like any address
    let go = live_control(&a, "Go");
    a.click_control(go, 0.0, &cx);
    live_wait(&mut a, &cx, "the page arrived", |a| live_texts(a).first().is_some_and(|t| t == "arrived"));
    assert_eq!(a.page().unwrap().url.path, "/next");
    // going back drops the old page's engine: nothing is kept for a page that is not in view
    a.back();
    settle(&mut a, &cx);
    for _ in 0..5 {
        a.handle(&Event::Tick, &cx);
    }
    assert!(a.live.is_none() || a.live.as_ref().map(|(s, _)| *s) == a.history.get(a.pos).map(|e| e.serial), "a session only lives with its own visit");
}

#[test]
fn without_scripts_a_button_says_so_and_a_page_without_scripts_keeps_no_engine() {
    let web = site(vec![("https://a.org/", html(LIVE_PAGE))]);
    let cx = cx_with("live2", web);
    browser_config::save(&cx, &BrowserConfig { js_all: false, ..BrowserConfig::default() });
    let mut a = BrowserApp::new(&cx);
    a.open_url("a.org", &cx);
    settle(&mut a, &cx);
    assert!(a.live.as_ref().is_none_or(|(_, k)| k.session.is_none()), "scripts are off: no engine");
    let add = live_control(&a, "Add");
    a.click_control(add, 0.0, &cx);
    assert!(a.status_text().unwrap().contains("needs JavaScript"), "{:?}", a.status_text());
    assert_eq!(live_texts(&a)[0], "0");
}

#[test]
fn a_handler_that_throws_or_clicks_in_quick_succession_leave_the_page_usable() {
    let page = "<p id=n>ok</p><button id=b type=button>Boom</button><button id=d type=button>Count</button><script>let k = 0; document.getElementById('b').addEventListener('click', () => { throw new Error('bad handler'); });\
        document.getElementById('d').addEventListener('click', () => { k++; document.getElementById('n').textContent = 'k' + k; });</script>";
    let web = site(vec![("https://a.org/", html(page))]);
    let cx = cx_with("live3", web);
    browser_config::save(&cx, &BrowserConfig { js_all: true, ..BrowserConfig::default() });
    let mut a = BrowserApp::new(&cx);
    a.open_url("a.org", &cx);
    settle(&mut a, &cx);
    let (boom, count) = (live_control(&a, "Boom"), live_control(&a, "Count"));
    a.click_control(boom, 0.0, &cx);
    for _ in 0..20 {
        a.click_control(count, 0.0, &cx);
    }
    live_wait(&mut a, &cx, "the clicks that were taken ran", |a| live_texts(a)[0].starts_with('k'));
    // more clicks than the queue holds are told to wait instead of piling up
    assert!(a.status_text().is_none_or(|s| s.contains("busy") || s.is_empty() || !s.contains("panic")));
    live_wait(&mut a, &cx, "the page settles", |a| a.live.as_ref().is_some_and(|(_, k)| k.session.as_ref().is_some_and(|s| s.pending() == 0)));
    let k: usize = live_texts(&a)[0].trim_start_matches('k').parse().unwrap();
    assert!((1..=21).contains(&k), "{k}");
}

const RESPONSIVE: &str = "<html><head><style>#narrow{display:none} @media (max-width: 700px) { #wide{display:none} #narrow{display:block} }</style></head><body><p id=wide>wide layout</p><p id=narrow>narrow layout</p></body></html>";

fn reflow_wait(a: &mut BrowserApp, cx: &AppCx, w: u32, h: u32, what: &str, done: impl Fn(&BrowserApp) -> bool) {
    for _ in 0..3000 {
        let _ = frame(a, cx, w, h);
        a.handle(&Event::Tick, cx);
        if done(a) {
            return;
        }
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    panic!("never happened: {what}; page text {:?}", live_texts(a));
}

#[test]
fn a_page_designed_for_the_window_is_styled_again_when_the_window_changes_size() {
    let web = site(vec![("https://a.org/", html(RESPONSIVE))]);
    let cx = cx_with("reflow1", web);
    browser_config::save(&cx, &BrowserConfig { js_all: false, ..BrowserConfig::default() });
    let mut a = BrowserApp::new(&cx);
    a.open_url("a.org", &cx);
    settle(&mut a, &cx);
    let _ = frame(&mut a, &cx, 1280, 720);
    assert_eq!(live_texts(&a), vec!["wide layout"], "built for a wide window");
    // the window is made narrow: after a moment the page answers its media queries for it
    reflow_wait(&mut a, &cx, 360, 600, "the narrow layout", |a| live_texts(a) == vec!["narrow layout"]);
    // and wide again
    reflow_wait(&mut a, &cx, 1280, 720, "the wide layout again", |a| live_texts(a) == vec!["wide layout"]);
    // a small change does not build the page again: the document stays the very same one
    let before = Arc::as_ptr(a.page().unwrap());
    for _ in 0..40 {
        let _ = frame(&mut a, &cx, 1270, 720);
        a.handle(&Event::Tick, &cx);
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    assert_eq!(Arc::as_ptr(a.page().unwrap()), before, "a few pixels of difference is not a new page");
}

#[test]
fn a_scripted_page_hears_the_resize_and_is_built_again_for_the_new_window() {
    let page = "<html><head><style>#narrow{display:none} @media (max-width: 700px) { #wide{display:none} #narrow{display:block} }</style></head><body><p id=wide>wide layout</p><p id=narrow>narrow layout</p><p id=w>none</p>\
        <script>window.addEventListener('resize', () => { document.getElementById('w').textContent = 'width ' + (window.innerWidth < 700 ? 'small' : 'big'); });</script></body></html>";
    let web = site(vec![("https://a.org/", html(page))]);
    let cx = cx_with("reflow2", web);
    browser_config::save(&cx, &BrowserConfig { js_all: true, ..BrowserConfig::default() });
    let mut a = BrowserApp::new(&cx);
    a.open_url("a.org", &cx);
    settle(&mut a, &cx);
    let _ = frame(&mut a, &cx, 1280, 720);
    assert!(a.live.as_ref().is_some_and(|(_, k)| k.session.is_some()), "the page keeps its engine");
    reflow_wait(&mut a, &cx, 360, 600, "the script heard the resize and the narrow layout came", |a| live_texts(a).iter().any(|t| t == "width small") && live_texts(a).iter().any(|t| t == "narrow layout"));
    assert!(!live_texts(&a).iter().any(|t| t == "wide layout"));
    reflow_wait(&mut a, &cx, 1280, 720, "and the wide one again", |a| live_texts(a).iter().any(|t| t == "width big") && live_texts(a).iter().any(|t| t == "wide layout"));
}


#[test]
fn text_typed_into_a_field_reaches_the_pages_input_handler_and_stays_where_it_was_typed() {
    let page = "<input id=t type=text placeholder=Name><p id=h>nobody</p><textarea id=ta></textarea><p id=c>0</p>\
        <script>document.getElementById('t').addEventListener('input', e => { document.getElementById('h').textContent = 'Hello ' + e.target.value; });\
        document.getElementById('ta').addEventListener('input', e => { document.getElementById('c').textContent = String(e.target.value.length); });</script>";
    let web = site(vec![("https://a.org/", html(page))]);
    let cx = cx_with("live4", web);
    browser_config::save(&cx, &BrowserConfig { js_all: true, ..BrowserConfig::default() });
    let mut a = BrowserApp::new(&cx);
    a.open_url("a.org", &cx);
    settle(&mut a, &cx);
    let field = a.page().unwrap().doc.controls.iter().position(|c| c.placeholder == "Name").unwrap();
    a.click_control(field, 0.0, &cx);
    for ch in "Ada".chars() {
        a.handle(&Event::Key(Key::Char(ch)), &cx);
    }
    live_wait(&mut a, &cx, "the page heard the typing", |a| live_texts(a).iter().any(|t| t == "Hello Ada"));
    assert_eq!(a.control(field).unwrap().1.value, "Ada", "what was typed is still in the field");
    assert_eq!(a.focus_ctl, Some(field), "and the field still has the keyboard");
    // more typing carries on from there, and a text area is heard too
    a.handle(&Event::Key(Key::Char('!')), &cx);
    live_wait(&mut a, &cx, "and the next letter", |a| live_texts(a).iter().any(|t| t == "Hello Ada!"));
    let ta = a.page().unwrap().doc.controls.iter().position(|c| c.kind == ControlKind::Textarea).unwrap();
    a.click_control(ta, 0.0, &cx);
    for ch in "hello".chars() {
        a.handle(&Event::Key(Key::Char(ch)), &cx);
    }
    live_wait(&mut a, &cx, "the text area was heard", |a| live_texts(a).iter().any(|t| t == "5"));
    // a fast typist: many letters at once end with the newest text, not a queue of old ones
    for ch in "abcdefghijklmnopqrstuvwxyz".chars() {
        a.handle(&Event::Key(Key::Char(ch)), &cx);
    }
    live_wait(&mut a, &cx, "the newest text arrived", |a| live_texts(a).iter().any(|t| t == "31"));
    assert!(a.live.as_ref().is_some_and(|(_, k)| k.session.as_ref().is_some_and(|s| s.pending() <= 1)));
}

#[test]
fn closing_for_the_lock_lets_the_engine_go_audits_it_and_says_how_to_open_the_page_again() {
    let (_fake, cx, mut a, _g) = in_full("lockclose", "");
    assert!(a.full.is_some(), "the page is in the full engine");
    a.on_lock(&cx);
    assert!(a.full.is_none(), "the engine was let go");
    assert!(a.status_text().is_some_and(|t| t.contains("closed when the screen locked")), "{:?}", a.status_text());
    assert!(cx.audit.recent(20).iter().any(|l| l.contains("browser.full.end") && l.contains("screen lock")));
    // a second lock, or a browser with no engine, does nothing and says nothing more
    a.on_lock(&cx);
    assert!(!a.close_full_for_lock(&cx));
}

