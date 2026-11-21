use super::*;
use crate::apps::tests::{cx_with_admin_store, temp_dir};
use iyakki_web::fetch::{Fetch, Response};
use std::sync::Mutex;

/// A small web: addresses to pages, with what each request carried recorded.
struct Site {
    pages: Vec<(&'static str, Response)>,
    seen: Mutex<Vec<(String, String)>>,
}

fn page(body: &str) -> Response {
    Response { status: 200, content_type: "text/html; charset=utf-8".into(), body: body.as_bytes().to_vec(), ..Default::default() }
}

impl Fetch for Site {
    fn get(&self, url: &Url) -> Result<Response, String> {
        self.send(&Request::get(url))
    }
    fn send(&self, req: &Request) -> Result<Response, String> {
        self.seen.lock().unwrap().push((req.url.to_string(), req.cookie.clone()));
        let u = req.url.to_string();
        self.pages.iter().find(|(a, _)| *a == u).map(|(_, r)| r.clone()).ok_or_else(|| format!("no such page {u}"))
    }
}

fn site(pages: Vec<(&'static str, Response)>) -> Arc<Site> {
    Arc::new(Site { pages, seen: Mutex::new(Vec::new()) })
}

fn cx_with(tag: &str, web: Arc<Site>) -> AppCx {
    let mut cx = cx_with_admin_store(tag);
    cx.web = web;
    let _ = temp_dir(tag);
    cx
}

fn wait(p: &mut PageEmbed) {
    // the fetch thread answers on a channel: block on it (no timing bound, no sleep)
    let State::Loading(rx) = &p.state else { return };
    let got = rx.recv();
    p.state = State::Loading(match got {
        Ok(v) => {
            let (tx, rx2) = channel();
            tx.send(v).unwrap();
            rx2
        }
        Err(_) => channel().1,
    });
    assert!(p.poll());
}

fn draw(p: &mut PageEmbed) -> Gfx {
    let mut g = Gfx::new(800, 600);
    p.paint(&mut g, R::new(10.0, 10.0, 600.0, 300.0), 1.0);
    g
}

fn hosts(h: &[&str]) -> Vec<String> {
    h.iter().map(|x| x.to_string()).collect()
}

#[test]
fn a_page_of_an_allowed_host_is_read_laid_out_and_drawn_inside_its_rectangle() {
    let web = site(vec![("https://docs.example/", page("<html><body><h1>Handbook</h1><p>Read this first.</p><a href=\"/next\">Next page</a></body></html>"))]);
    let cx = cx_with("embpage-draw", web);
    let mut p = PageEmbed::new(hosts(&["docs.example"]), "embed:amy:notes");
    p.open("https://docs.example/", &cx);
    assert!(p.loading());
    wait(&mut p);
    assert_eq!(p.url().as_deref(), Some("https://docs.example/"));
    let g = draw(&mut p);
    let said = g.text_log.iter().map(|(_, _, t)| t.as_str()).collect::<Vec<_>>().join(" ");
    assert!(said.contains("Handbook") && said.contains("Read") && said.contains("first"), "{said:?}");
    assert!(g.text_log.iter().all(|(x, y, _)| *x >= 10.0 && *y >= 10.0), "nothing is drawn left of or above the rectangle: {:?}", g.text_log);
    assert!(!p.links.is_empty() && p.links.iter().all(|(_, i)| *i == 0), "the link was registered for clicks: {:?}", p.links);
}

#[test]
fn a_host_the_app_did_not_declare_is_refused_before_anything_is_requested_and_a_link_to_it_is_not_followed() {
    let web = site(vec![("https://docs.example/", page("<html><body><a href=\"https://elsewhere.example/x\">Away</a></body></html>"))]);
    let cx = cx_with("embpage-hosts", web.clone());
    let mut p = PageEmbed::new(hosts(&["docs.example"]), "embed:amy:notes");
    p.open("https://elsewhere.example/", &cx);
    assert!(web.seen.lock().unwrap().is_empty(), "nothing was requested");
    assert!(p.failure().is_some_and(|f| f.contains("may not show elsewhere.example") && f.contains("docs.example")), "{:?}", p.failure());
    // a page of the allowed host that links away: the click is the viewer's, the host is named, the page stays
    let mut p = PageEmbed::new(hosts(&["docs.example"]), "embed:amy:notes");
    p.open("https://docs.example/", &cx);
    wait(&mut p);
    let _ = draw(&mut p);
    let (r, _) = p.links[0];
    assert!(p.click(r.x + 1.0, r.y + 1.0, &cx));
    assert_eq!(p.refused(), Some("elsewhere.example"));
    assert_eq!(p.url().as_deref(), Some("https://docs.example/"), "still on the allowed page");
    assert_eq!(web.seen.lock().unwrap().len(), 1, "the other host was never asked");
    let g = draw(&mut p);
    assert!(g.text_log.iter().any(|(_, _, t)| t.contains("Left the allowed sites") && t.contains("elsewhere.example")));
}

#[test]
fn a_redirect_to_another_host_is_stopped_by_the_same_rule() {
    let mut r = page("");
    r.status = 302;
    r.location = Some("https://other.example/landing".into());
    let web = site(vec![("https://docs.example/go", r)]);
    let cx = cx_with("embpage-redirect", web.clone());
    let mut p = PageEmbed::new(hosts(&["docs.example"]), "embed:amy:notes");
    p.open("https://docs.example/go", &cx);
    wait(&mut p);
    assert!(p.failure().is_some_and(|f| f.contains("other.example")), "{:?}", p.failure());
    assert_eq!(web.seen.lock().unwrap().len(), 1);
}

#[test]
fn a_link_inside_the_allowed_host_is_followed_and_back_returns() {
    let web = site(vec![
        ("https://docs.example/", page("<html><body><a href=\"/two\">Second</a></body></html>")),
        ("https://docs.example/two", page("<html><body><p>The second page.</p></body></html>")),
    ]);
    let cx = cx_with("embpage-follow", web);
    let mut p = PageEmbed::new(hosts(&["docs.example"]), "embed:amy:notes");
    p.open("https://docs.example/", &cx);
    wait(&mut p);
    let _ = draw(&mut p);
    let (r, _) = p.links[0];
    assert!(p.click(r.x + 1.0, r.y + 1.0, &cx));
    wait(&mut p);
    assert_eq!(p.url().as_deref(), Some("https://docs.example/two"));
    assert!(draw(&mut p).text_log.iter().any(|(_, _, t)| t.contains("second")));
    assert!(p.back(&cx));
    wait(&mut p);
    assert_eq!(p.url().as_deref(), Some("https://docs.example/"));
    assert!(!p.back(&cx), "nothing before the first page");
}

#[test]
fn the_cookies_are_the_apps_own_kept_in_memory_and_shared_with_no_other_app() {
    let mut first = page("<html><body><p>hello</p></body></html>");
    first.set_cookies = vec!["sid=abc123; Path=/".into()];
    let web = site(vec![("https://docs.example/", first)]);
    let cx = cx_with("embpage-jar", web.clone());
    let profile_a = "embed:amy:app-a-jar-test";
    let mut a = PageEmbed::new(hosts(&["docs.example"]), profile_a);
    a.open("https://docs.example/", &cx);
    wait(&mut a);
    // the same app asking again sends the cookie it was given
    let mut a2 = PageEmbed::new(hosts(&["docs.example"]), profile_a);
    a2.open("https://docs.example/", &cx);
    wait(&mut a2);
    // another app (another profile) starts with an empty jar
    let mut b = PageEmbed::new(hosts(&["docs.example"]), "embed:amy:app-b-jar-test");
    b.open("https://docs.example/", &cx);
    wait(&mut b);
    let seen = web.seen.lock().unwrap().clone();
    assert_eq!(seen[0].1, "", "the first request of an app carries nothing");
    assert!(seen[1].1.contains("sid=abc123"), "the app's own cookie comes back: {seen:?}");
    assert_eq!(seen[2].1, "", "another app never gets it");
    assert_eq!(super::jar_of("embed:amy:never-used-jar-test").lock().unwrap().header_for(&Url::parse("https://docs.example/").unwrap(), 0), "");
}

#[test]
fn a_file_is_not_a_page_and_a_failed_load_says_why() {
    let png = Response { status: 200, content_type: "image/png".into(), body: vec![0x89, b'P', b'N', b'G', 1, 2, 3, 4], ..Default::default() };
    let web = site(vec![("https://docs.example/pic.png", png)]);
    let cx = cx_with("embpage-file", web);
    let mut p = PageEmbed::new(hosts(&["docs.example"]), "embed:amy:file-test");
    p.open("https://docs.example/pic.png", &cx);
    wait(&mut p);
    assert!(p.failure().is_some_and(|f| f.contains("keeps no downloads")), "{:?}", p.failure());
    p.open("https://docs.example/missing", &cx);
    wait(&mut p);
    assert!(p.failure().is_some_and(|f| f.contains("no such page")));
    // a bad address is said in words
    p.open("javascript:alert(1)", &cx);
    assert!(p.failure().is_some());
    // the failure is drawn, inside the rectangle
    assert!(draw(&mut p).text_log.iter().all(|(x, _, _)| *x >= 10.0));
}

#[test]
fn the_wheel_scrolls_inside_the_page_and_stops_at_its_ends() {
    let long: String = (0..80).map(|i| format!("<p>Line number {i} of a long page.</p>")).collect();
    let web = site(vec![("https://docs.example/", page(&format!("<html><body>{long}</body></html>")))]);
    let cx = cx_with("embpage-scroll", web);
    let mut p = PageEmbed::new(hosts(&["docs.example"]), "embed:amy:scroll-test");
    p.open("https://docs.example/", &cx);
    wait(&mut p);
    let _ = draw(&mut p);
    p.scroll_by(1);
    assert_eq!(p.scroll, 0.0, "up from the top stays at the top");
    for _ in 0..500 {
        p.scroll_by(-1);
    }
    let _ = draw(&mut p);
    assert!(p.scroll > 100.0 && p.scroll <= p.content_h, "scrolled to the end and no further");
    let end = p.scroll;
    p.scroll_by(-5);
    let _ = draw(&mut p);
    assert_eq!(p.scroll, end);
}

#[test]
fn the_apps_embed_calls_back_forward_reload_and_stop_move_the_page_viewer() {
    let web = site(vec![
        ("https://docs.example/", page("<html><body><a href=\"/two\">Second</a></body></html>")),
        ("https://docs.example/two", page("<html><body><p>The second page.</p></body></html>")),
    ]);
    let cx = cx_with("embpage-calls", web.clone());
    let mut p = PageEmbed::new(hosts(&["docs.example"]), "embed:amy:calls-test");
    assert!(!p.apply_call("reload", &cx), "nothing was asked for yet");
    p.open("https://docs.example/", &cx);
    wait(&mut p);
    let _ = draw(&mut p);
    let (r, _) = p.links[0];
    assert!(p.click(r.x + 1.0, r.y + 1.0, &cx));
    wait(&mut p);
    assert!(!p.apply_call("forward", &cx), "nothing to go forward to yet");
    // back, then forward again to the page back left
    assert!(p.apply_call("back", &cx));
    wait(&mut p);
    assert_eq!(p.url().as_deref(), Some("https://docs.example/"));
    assert!(p.apply_call("forward", &cx));
    wait(&mut p);
    assert_eq!(p.url().as_deref(), Some("https://docs.example/two"));
    assert!(p.apply_call("back", &cx), "the history survived going forward");
    wait(&mut p);
    // reload asks for the same address again and adds no history
    let before = web.seen.lock().unwrap().len();
    assert!(p.apply_call("reload", &cx));
    wait(&mut p);
    assert_eq!((p.url().as_deref(), web.seen.lock().unwrap().len()), (Some("https://docs.example/"), before + 1));
    assert!(!p.apply_call("back", &cx), "a reload is not a page of its own");
    // a new address clears what was ahead; stop ends a load under way; focus and blur do nothing here
    p.open("https://docs.example/two", &cx);
    assert!(p.apply_call("stop", &cx) && p.failure() == Some("Stopped."));
    assert!(!p.apply_call("stop", &cx) && !p.apply_call("focus", &cx) && !p.apply_call("blur", &cx) && !p.apply_call("navigate", &cx));
}
