//! The real sessions over the in-house engine, with a loopback web server (127.0.0.1, a thread of the test) as the page source.

use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use iyakki_core_api::browsersession::{BrowserError, BrowserSessions, EnginePref, Input, Navigate, OpenSpec, PageKey, PromptKind, Refusal, SessionId};
use iyakki_ui_sdk::list::{DisplayList, Op};
use iyakki_web::fetch::{Fetch, Response};
use iyakki_web::url::Url;

use crate::{BrowserHost, LocalBrowserSessions};

/// A tiny HTTP/1.0 client over a plain socket: the transport of the tests (the device's is the agent's TLS one).
struct Plain;

impl Fetch for Plain {
    fn get(&self, url: &Url) -> Result<Response, String> {
        let mut s = TcpStream::connect((url.host.as_str(), url.port.unwrap_or(80))).map_err(|e| e.to_string())?;
        s.set_read_timeout(Some(Duration::from_secs(5))).ok();
        write!(s, "GET {} HTTP/1.0\r\nHost: {}\r\nConnection: close\r\n\r\n", url.request_target(), url.host).map_err(|e| e.to_string())?;
        let mut raw = Vec::new();
        s.read_to_end(&mut raw).map_err(|e| e.to_string())?;
        let at = raw.windows(4).position(|w| w == b"\r\n\r\n").ok_or("no header")?;
        let head = String::from_utf8_lossy(&raw[..at]).to_string();
        let mut lines = head.lines();
        let status: u16 = lines.next().and_then(|l| l.split(' ').nth(1)).and_then(|c| c.parse().ok()).ok_or("bad status")?;
        let mut r = Response { status, body: raw[at + 4..].to_vec(), ..Response::default() };
        for l in lines {
            if let Some((k, v)) = l.split_once(':') {
                match k.to_ascii_lowercase().as_str() {
                    "content-type" => r.content_type = v.trim().into(),
                    "location" => r.location = Some(v.trim().into()),
                    _ => {}
                }
            }
        }
        Ok(r)
    }
}

/// Serve `routes` (path to content type and body) on a free loopback port until the test ends; the requests seen are kept.
struct Server {
    port: u16,
    seen: Arc<Mutex<Vec<String>>>,
}

fn serve(routes: Vec<(&'static str, &'static str, Vec<u8>)>) -> Server {
    let l = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = l.local_addr().unwrap().port();
    let seen = Arc::new(Mutex::new(Vec::new()));
    let seen2 = seen.clone();
    std::thread::spawn(move || {
        for s in l.incoming() {
            let Ok(mut s) = s else { break };
            let mut got = Vec::new();
            let mut buf = [0u8; 1024];
            while !got.windows(4).any(|w| w == b"\r\n\r\n") {
                match s.read(&mut buf) {
                    Ok(0) | Err(_) => break,
                    Ok(n) => got.extend_from_slice(&buf[..n]),
                }
            }
            let req = String::from_utf8_lossy(&got).to_string();
            let path = req.split(' ').nth(1).unwrap_or("/").to_string();
            seen2.lock().unwrap().push(path.clone());
            let hit = routes.iter().find(|(p, _, _)| *p == path);
            let (status, ct, body) = match hit {
                Some((_, ct, b)) => ("200 OK", *ct, b.clone()),
                None => ("404 Not Found", "text/plain", b"not found".to_vec()),
            };
            let _ = write!(s, "HTTP/1.0 {status}\r\nContent-Type: {ct}\r\nContent-Length: {}\r\n\r\n", body.len());
            let _ = s.write_all(&body);
        }
    });
    Server { port, seen }
}

#[derive(Default)]
struct Host {
    bad: Vec<String>,
    deny: Vec<String>,
    off: bool,
    audit: Mutex<Vec<String>>,
    saved: Mutex<Vec<String>>,
    can_save: bool,
}

impl BrowserHost for Host {
    fn fetch(&self, _user: &str) -> Arc<dyn Fetch> {
        Arc::new(Plain)
    }
    fn known_bad(&self, host: &str) -> Option<(String, String)> {
        self.bad.iter().find(|b| host == b.as_str()).map(|b| (b.clone(), "phishing".into()))
    }
    fn allows_host(&self, _user: &str, host: &str) -> bool {
        !self.deny.iter().any(|d| d == host)
    }
    fn enabled(&self, _user: &str) -> bool {
        !self.off
    }
    fn audit(&self, user: &str, app: &str, event: &str, detail: &str) {
        self.audit.lock().unwrap().push(format!("{user} {app} {event} {detail}"));
    }
    fn save_download(&self, _user: &str, _app: &str, url: &str) -> Result<String, String> {
        if self.can_save {
            self.saved.lock().unwrap().push(url.to_string());
            Ok("Saved to your downloads.".into())
        } else {
            Err("downloads are not switched on".into())
        }
    }
}

const HOME: &str = "<html><head><title>Loopback home</title></head><body><h1>Welcome</h1><p>This page comes from <a href=\"/two\">the second page</a> and says hello twice: hello.</p><p>A <a href=\"/f.zip\">download</a> and <a href=\"javascript:alert(1)\">a script link</a>.</p></body></html>";
const TWO: &str = "<html><head><title>Second</title></head><body><h2>Page two</h2><p>second page text</p></body></html>";

fn site() -> Server {
    let long: String = (0..200).map(|i| format!("<p>line number {i} of a long page</p>")).collect();
    serve(vec![
        ("/", "text/html", HOME.as_bytes().to_vec()),
        ("/two", "text/html", TWO.as_bytes().to_vec()),
        ("/long", "text/html", format!("<html><title>Long</title><body>{long}</body></html>").into_bytes()),
        ("/f.zip", "application/zip", vec![0x50, 0x4b, 3, 4, 0, 0, 0, 0]),
    ])
}

fn sessions(h: Host) -> (LocalBrowserSessions, Arc<Host>) {
    let h = Arc::new(h);
    (LocalBrowserSessions::new(h.clone()), h)
}

fn url(s: &Server, p: &str) -> String {
    format!("http://127.0.0.1:{}{p}", s.port)
}

/// Wait until the session is not loading (the load runs on a worker thread).
fn settle(b: &LocalBrowserSessions, id: SessionId) -> iyakki_core_api::browsersession::PageState {
    let t = Instant::now();
    loop {
        let st = b.state("alice", "demo", id).unwrap();
        if !st.loading {
            return st;
        }
        assert!(t.elapsed() < Duration::from_secs(10), "the page never finished loading");
        std::thread::sleep(Duration::from_millis(10));
    }
}

fn list(b: &LocalBrowserSessions, id: SessionId) -> DisplayList {
    DisplayList::parse(&b.frame("alice", "demo", id, 0).unwrap().unwrap().list).unwrap()
}

fn action_of(l: &DisplayList, name: &str) -> u32 {
    l.ops.iter().find_map(|o| if let Op::Hit(_, h) = o { (h.name == name).then_some(h.action) } else { None }).unwrap_or_else(|| panic!("no link {name}: {:?}", l.a11y().iter().map(|n| n.name.clone()).collect::<Vec<_>>()))
}

fn open(b: &LocalBrowserSessions) -> SessionId {
    b.open("alice", "demo", &OpenSpec::new(360.0, 300.0)).unwrap()
}

#[test]
fn a_loopback_page_loads_through_the_core_draws_as_a_display_list_and_its_link_is_followed() {
    let srv = site();
    let (b, h) = sessions(Host::default());
    let id = open(&b);
    b.navigate("alice", "demo", id, &Navigate::Url(url(&srv, "/"))).unwrap();
    let st = settle(&b, id);
    assert_eq!(st.error, None);
    assert_eq!((st.title.as_str(), st.url.as_str(), st.can_back, st.engine), ("Loopback home", url(&srv, "/").as_str(), false, EnginePref::InHouse));
    let l = list(&b, id);
    assert!(l.texts().contains(&"Welcome"), "{:?}", l.texts());
    assert!(l.texts().iter().any(|t| *t == "hello" || *t == "hello."), "{:?}", l.texts());
    assert!(l.a11y().iter().filter(|n| n.role == iyakki_ui_sdk::canvas::Role::Link).count() >= 3);
    // the frame is drawn for the view's size and inside it
    assert_eq!((l.width, l.height), (360.0, 300.0));
    for n in l.a11y() {
        assert!(n.rect.x >= 0.0 && n.rect.right() <= 360.0 + 0.5, "{} sticks out: {:?}", n.name, n.rect);
    }
    // a tap on "the second page" follows the link; Back and Forward walk the history
    assert!(action_of(&l, "second") >= 1);
    b.input("alice", "demo", id, &Input::Activate(1)).unwrap();
    let st = settle(&b, id);
    assert_eq!(st.title, "Second");
    assert!(st.can_back && !st.can_forward);
    assert!(list(&b, id).texts().contains(&"second"));
    b.navigate("alice", "demo", id, &Navigate::Back).unwrap();
    assert_eq!(settle(&b, id).title, "Loopback home");
    b.navigate("alice", "demo", id, &Navigate::Forward).unwrap();
    assert_eq!(settle(&b, id).title, "Second");
    assert!(srv.seen.lock().unwrap().iter().any(|p| p == "/two"));
    // the audit names the person, the app and the events, never the page
    let a = h.audit.lock().unwrap().join("\n");
    assert!(a.contains("alice demo browser.embed.open") && a.contains("browser.embed.navigate 127.0.0.1"), "{a}");
    assert!(!a.contains("hello"));
}

#[test]
fn the_core_refuses_known_bad_sites_policy_hosts_and_odd_addresses_before_anything_is_sent() {
    let srv = site();
    let (b, h) = sessions(Host { bad: vec!["bad.example".into()], deny: vec!["work.example".into()], ..Host::default() });
    let id = open(&b);
    for (u, want) in [("https://bad.example/x", "known-bad"), ("https://work.example/", "may not open")] {
        let e = b.navigate("alice", "demo", id, &Navigate::Url(u.into())).unwrap_err();
        assert!(e.to_string().contains(want), "{u}: {e}");
    }
    for bad in ["javascript:alert(1)", "file:///etc/passwd", "ftp://x.org/", "data:text/html,hi", "https://user:pw@x.org/"] {
        assert!(matches!(b.navigate("alice", "demo", id, &Navigate::Url(bad.into())), Err(BrowserError::Refused(Refusal::BadAddress(_)))), "{bad}");
    }
    assert!(srv.seen.lock().unwrap().is_empty());
    assert_eq!(b.state("alice", "demo", id).unwrap().url, "", "nothing was opened");
    let a = h.audit.lock().unwrap().join("\n");
    assert!(a.contains("browser.embed.refused known-bad bad.example") && a.contains("browser.embed.refused policy work.example"), "{a}");
    // a link on a page that leads to a refused host is refused too, and the view is told
    let s2 = serve(vec![("/", "text/html", b"<title>Links</title><a href=\"https://bad.example/\">trap</a>".to_vec())]);
    b.navigate("alice", "demo", id, &Navigate::Url(url(&s2, "/"))).unwrap();
    settle(&b, id);
    assert!(matches!(b.input("alice", "demo", id, &Input::Activate(1)), Err(BrowserError::Refused(Refusal::KnownBad { .. }))));
    assert!(b.state("alice", "demo", id).unwrap().error.unwrap().contains("known-bad"));
    // switched off: no session at all
    let (b2, _) = sessions(Host { off: true, ..Host::default() });
    assert_eq!(b2.open("alice", "demo", &OpenSpec::new(100.0, 100.0)), Err(BrowserError::Refused(Refusal::Off)));
}

#[test]
fn sessions_belong_to_a_person_and_an_app_and_are_limited() {
    let (b, _) = sessions(Host::default());
    let id = open(&b);
    assert_eq!(b.state("bob", "demo", id), Err(BrowserError::NoSuchSession));
    assert_eq!(b.state("alice", "other", id), Err(BrowserError::NoSuchSession));
    assert_eq!(b.state("alice", "demo", SessionId(9999)), Err(BrowserError::NoSuchSession), "a stranger cannot tell a closed view from one that is not his");
    b.close("bob", "demo", id);
    assert_eq!(b.open_sessions(), 1);
    for _ in 1..iyakki_core_api::browsersession::MAX_SESSIONS_PER_APP {
        open(&b);
    }
    assert_eq!(b.open("alice", "demo", &OpenSpec::new(100.0, 100.0)), Err(BrowserError::TooMany));
    assert!(b.open("alice", "second-app", &OpenSpec::new(100.0, 100.0)).is_ok(), "the limit is per app");
    assert_eq!(b.end_sessions_of("alice"), iyakki_core_api::browsersession::MAX_SESSIONS_PER_APP + 1);
    assert_eq!(b.open_sessions(), 0, "sign-out ends them all");
    // a size nobody can draw in, and the full engine, which cannot be embedded yet
    assert!(matches!(b.open("alice", "demo", &OpenSpec::new(f32::NAN, 100.0)), Err(BrowserError::Invalid(_))));
    let mut full = OpenSpec::new(100.0, 100.0);
    full.engine = EnginePref::Full;
    assert!(matches!(b.open("alice", "demo", &full), Err(BrowserError::Unavailable(_))));
    full.engine = EnginePref::Auto;
    let id = b.open("alice", "demo", &full).unwrap();
    assert_eq!(b.state("alice", "demo", id).unwrap().engine, EnginePref::InHouse);
}

#[test]
fn the_wheel_keys_find_and_resize_work_on_a_long_page() {
    let srv = site();
    let (b, _) = sessions(Host::default());
    let id = open(&b);
    b.navigate("alice", "demo", id, &Navigate::Url(url(&srv, "/long"))).unwrap();
    let st = settle(&b, id);
    assert_eq!((st.scroll_permille, st.title.as_str()), (0, "Long"));
    let before = list(&b, id);
    assert!(before.texts().contains(&"line") && !before.texts().iter().any(|t| *t == "199"));
    b.input("alice", "demo", id, &Input::Wheel(-5)).unwrap();
    let st = b.state("alice", "demo", id).unwrap();
    assert!(st.scroll_permille > 0, "the wheel moved the page down");
    b.input("alice", "demo", id, &Input::Key(PageKey::End)).unwrap();
    assert_eq!(b.state("alice", "demo", id).unwrap().scroll_permille, 1000);
    assert!(list(&b, id).texts().iter().any(|t| *t == "199"), "the end of the page shows");
    b.input("alice", "demo", id, &Input::Key(PageKey::Home)).unwrap();
    // find: 200 lines say "long page"; the match is marked and the view scrolls to it
    let f = b.find("alice", "demo", id, "number 150", true).unwrap();
    assert_eq!((f.matches, f.current), (1, 1));
    assert!(b.state("alice", "demo", id).unwrap().scroll_permille > 500);
    let all = b.find("alice", "demo", id, "long page", true).unwrap();
    assert_eq!((all.matches, all.current), (200, 1));
    assert_eq!(b.find("alice", "demo", id, "long page", true).unwrap().current, 2);
    assert_eq!(b.find("alice", "demo", id, "long page", false).unwrap().current, 1);
    assert_eq!(b.find("alice", "demo", id, "long page", false).unwrap().current, 200, "backwards wraps");
    assert_eq!(b.find("alice", "demo", id, "", true).unwrap().matches, 0);
    // narrower: the page is laid out again for the new width and still fits it
    b.input("alice", "demo", id, &Input::Resize { width: 120.0, height: 200.0 }).unwrap();
    let l = list(&b, id);
    assert_eq!((l.width, l.height), (120.0, 200.0));
    assert!(b.input("alice", "demo", id, &Input::Resize { width: 0.0, height: 0.0 }).is_err());
    // an unchanged frame is not sent again
    let g = b.state("alice", "demo", id).unwrap().generation;
    assert!(b.frame("alice", "demo", id, g).unwrap().is_none());
}

#[test]
fn a_file_is_a_download_question_that_only_the_answer_can_open() {
    let srv = site();
    let (b, h) = sessions(Host { can_save: true, ..Host::default() });
    let id = open(&b);
    b.navigate("alice", "demo", id, &Navigate::Url(url(&srv, "/"))).unwrap();
    settle(&b, id);
    // the "download" link is the second link of the page
    let dl = action_of(&list(&b, id), "download");
    b.input("alice", "demo", id, &Input::Activate(dl)).unwrap();
    settle(&b, id);
    let p = b.prompts("alice", "demo", id);
    assert_eq!(p.len(), 1);
    assert_eq!(p[0].kind, PromptKind::Download);
    assert!(p[0].text.contains("f.zip") && p[0].text.contains("bytes"), "{}", p[0].text);
    assert_eq!(p[0].origin, "127.0.0.1");
    assert!(h.saved.lock().unwrap().is_empty(), "nothing is saved before the person answers");
    b.answer_prompt("alice", "demo", id, p[0].id, true).unwrap();
    assert_eq!(h.saved.lock().unwrap().len(), 1);
    assert!(h.saved.lock().unwrap()[0].ends_with("/f.zip"));
    assert!(list(&b, id).texts().iter().any(|t| t.starts_with("Saved")), "the view says it was saved");
    assert!(b.prompts("alice", "demo", id).is_empty());
    assert!(b.answer_prompt("alice", "demo", id, p[0].id, true).is_err(), "a question is answered once");
    // declined, and the core that cannot save says so
    let id2 = open(&b);
    b.navigate("alice", "demo", id2, &Navigate::Url(url(&srv, "/f.zip"))).unwrap();
    settle(&b, id2);
    let q = b.prompts("alice", "demo", id2)[0].id;
    b.answer_prompt("alice", "demo", id2, q, false).unwrap();
    assert_eq!(h.saved.lock().unwrap().len(), 1);
    let a = h.audit.lock().unwrap().join("\n");
    assert!(a.contains("browser.embed.download 127.0.0.1 saved") && a.contains("browser.embed.download 127.0.0.1 declined"), "{a}");
}

#[test]
fn page_content_the_app_brings_is_shown_without_a_request_and_its_links_are_still_judged() {
    let srv = site();
    let (b, _) = sessions(Host { bad: vec!["bad.example".into()], ..Host::default() });
    let id = open(&b);
    b.navigate("alice", "demo", id, &Navigate::Html { base: "about:help".into(), html: "<title>Help</title><h1>Help text</h1><a href=\"https://bad.example/\">bad</a>".into() }).unwrap();
    let st = b.state("alice", "demo", id).unwrap();
    assert_eq!((st.title.as_str(), st.url.as_str(), st.loading), ("Help", "about:help", false));
    assert!(list(&b, id).texts().contains(&"Help"));
    assert!(srv.seen.lock().unwrap().is_empty());
    // the base is not an address a link can be resolved against, so the link is refused by name, never fetched
    assert!(b.input("alice", "demo", id, &Input::Activate(1)).is_err());
    let big = "x".repeat(iyakki_core_api::browsersession::MAX_HTML + 1);
    assert!(matches!(b.navigate("alice", "demo", id, &Navigate::Html { base: "about:x".into(), html: big }), Err(BrowserError::Invalid(_))));
    // Stop while nothing loads is harmless; Reload shows the page again
    b.navigate("alice", "demo", id, &Navigate::Stop).unwrap();
    b.navigate("alice", "demo", id, &Navigate::Reload).unwrap();
    assert_eq!(b.state("alice", "demo", id).unwrap().title, "Help");
}

#[test]
fn the_page_follows_the_apps_defaults_scheme_size_family_and_text_scale() {
    use iyakki_core_api::browsersession::{PageDefaults, PageFont, Scheme};
    let (b, _) = sessions(Host::default());
    let page = "<title>T</title><h1>Heading</h1><p>some words that need to wrap on a narrow view <a href=\"https://a.org/\">link</a></p><hr>";
    let frame_of = |d: PageDefaults, ts: f32, w: f32| {
        let mut spec = OpenSpec::new(w, 2000.0);
        spec.defaults = d;
        spec.text_scale = ts;
        let id = b.open("alice", "demo", &spec).unwrap();
        b.navigate("alice", "demo", id, &Navigate::Html { base: "about:t".into(), html: page.into() }).unwrap();
        list(&b, id)
    };
    let base = frame_of(PageDefaults::default(), 1.0, 360.0);
    let size_of = |l: &DisplayList, t: &str| l.ops.iter().find_map(|o| if let Op::Text { text, size, style, .. } = o { (text == t).then_some((*size, *style)) } else { None }).unwrap();
    let (s0, st0) = size_of(&base, "words");
    assert_eq!(st0, iyakki_ui_sdk::canvas::TextStyle::Regular);
    // a bigger default size and the mono family reach the page
    let big = frame_of(PageDefaults { font_size: 20.0, font: PageFont::Mono, ..PageDefaults::default() }, 1.0, 360.0);
    let (s1, st1) = size_of(&big, "words");
    assert!(s1 > s0 * 1.3 && st1 == iyakki_ui_sdk::canvas::TextStyle::Mono, "{s0} {s1} {st1:?}");
    // 3x text at a phone width: still inside the view, still wrapped, links still named controls
    for w in [360.0f32, 800.0, 1280.0] {
        let l = frame_of(PageDefaults::default(), 3.0, w);
        assert!(size_of(&l, "words").0 > s0 * 2.9);
        for n in l.a11y() {
            assert!(n.rect.right() <= w + 0.5 && n.rect.x >= 0.0, "{} sticks out at {w}: {:?}", n.name, n.rect);
        }
        assert!(l.a11y().iter().any(|n| n.name == "link"));
    }
    // high contrast: thicker underlines and rules in the text colour
    let widths = |l: &DisplayList| l.ops.iter().filter_map(|o| if let Op::Line(_, w, _) = o { Some(*w) } else { None }).fold(0.0f32, f32::max);
    let hc = frame_of(PageDefaults { scheme: Scheme::HighContrast, ..PageDefaults::default() }, 1.0, 360.0);
    assert!(widths(&hc) > widths(&base));
}
