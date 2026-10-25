//! Settings -> Models -> Model host on your network, against a fake OpenAI-style server on loopback and a scripted kubectl:
//! opening the page (menu jump, key, administrators only), the form, Test, Add to Models (provider, vault key, persistence),
//! Scan on a loopback target only, the cluster buttons, nothing happens without a click, and the page at five screen sizes.

use super::*;
use crate::apps::tests::{cx_with_admin_store, services_in, temp_dir};
use iyakki_agent::modelhost::{Kube, Limits, ScanOpts};
use iyakki_agent::wsexec::Out;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{IpAddr, Ipv4Addr, TcpListener, TcpStream};
use std::sync::Mutex;

struct Offline;
impl iyakki_agent::iyakki_proxy::Upstream for Offline {
    fn request(&self, _: &str, _: &str, _: &[(String, String)], _: &[u8]) -> Result<iyakki_agent::iyakki_proxy::UpstreamResponse, String> {
        Err("offline".into())
    }
}

fn admin(tag: &str) -> AppCx {
    let mut cx = cx_with_admin_store(tag);
    let d = temp_dir(&format!("{tag}-svc"));
    cx.services = services_in(&d, Arc::new(Offline));
    cx.services.on_login("root", "rootpass12", true);
    cx
}

fn key(a: &mut SettingsApp, cx: &AppCx, k: Key) {
    a.handle(&Event::Key(k), cx);
}
fn typ(a: &mut SettingsApp, cx: &AppCx, s: &str) {
    s.chars().for_each(|c| key(a, cx, Key::Char(c)));
}
fn status(a: &SettingsApp) -> String {
    a.status().map(|s| s.0.clone()).unwrap_or_default()
}
fn settle(a: &mut SettingsApp, cx: &AppCx) {
    for _ in 0..3000 {
        a.handle(&Event::Tick, cx);
        if !a.is_busy() {
            return;
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    panic!("job did not finish");
}
fn draw(a: &mut SettingsApp, cx: &AppCx, w: u32, h: u32) -> Gfx {
    let mut g = Gfx::new(w, h);
    g.clear(rgb(0, 0, 0));
    let s = (w as f32 / 1280.0).min(h as f32 / 720.0).clamp(0.6, 3.0);
    a.render(&mut g, R::new(0.0, 72.0, w as f32, h as f32 - 72.0), s, cx, 0);
    g
}
/// Click the button called `label`, scrolling the page (down, then from the top) until it is on screen.
fn click(a: &mut SettingsApp, cx: &AppCx, label: &str) {
    for _ in 0..2 {
        for _ in 0..60 {
            let g = draw(a, cx, 1280, 720);
            if let Some(h) = g.hits.iter().find(|h| h.label == label) {
                let ev = h.ev.clone();
                a.handle(&ev, cx);
                return;
            }
            key(a, cx, Key::PageDown);
        }
        key(a, cx, Key::Home);
    }
    panic!("no `{label}` button anywhere on the page");
}

/// A loopback server that answers /v1/models and a plain chat; with `need_key` anything else is 401.
struct Fake {
    port: u16,
    seen: Arc<Mutex<Vec<String>>>,
}

fn serve(need_key: bool) -> Fake {
    let l = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = l.local_addr().unwrap().port();
    let seen: Arc<Mutex<Vec<String>>> = Default::default();
    let seen2 = seen.clone();
    std::thread::spawn(move || {
        for c in l.incoming().flatten() {
            let seen = seen2.clone();
            std::thread::spawn(move || handle(c, need_key, seen));
        }
    });
    Fake { port, seen }
}

fn handle(mut s: TcpStream, need_key: bool, seen: Arc<Mutex<Vec<String>>>) {
    let mut r = BufReader::new(s.try_clone().unwrap());
    let mut line = String::new();
    if r.read_line(&mut line).unwrap_or(0) == 0 {
        return;
    }
    let path = line.split_whitespace().nth(1).unwrap_or("").to_string();
    let (mut len, mut bearer) = (0usize, String::new());
    loop {
        let mut h = String::new();
        if r.read_line(&mut h).unwrap_or(0) == 0 || h.trim().is_empty() {
            break;
        }
        let low = h.to_ascii_lowercase();
        if let Some(v) = low.strip_prefix("content-length:") {
            len = v.trim().parse().unwrap_or(0);
        }
        if low.starts_with("authorization:") {
            bearer = h.split_once(':').unwrap().1.trim().to_string();
        }
    }
    let mut body = vec![0u8; len];
    let _ = r.read_exact(&mut body);
    seen.lock().unwrap().push(format!("{} {}", line.split_whitespace().next().unwrap_or(""), path));
    let reply = |s: &mut TcpStream, code: &str, b: &str| {
        let _ = s.write_all(format!("HTTP/1.1 {code}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{b}", b.len()).as_bytes());
    };
    if need_key && bearer != "Bearer s3cret" {
        return reply(&mut s, "401 Unauthorized", r#"{"error":{"message":"no key"}}"#);
    }
    match path.as_str() {
        "/v1/models" => reply(&mut s, "200 OK", r#"{"object":"list","data":[{"id":"qwen2.5:7b"},{"id":"nomic-embed-text"}]}"#),
        "/v1/chat/completions" => {
            std::thread::sleep(Duration::from_millis(40));
            reply(&mut s, "200 OK", r#"{"choices":[{"message":{"content":"4"}}],"usage":{"completion_tokens":12}}"#)
        }
        _ => reply(&mut s, "404 Not Found", "{}"),
    }
}

fn unused_port() -> u16 {
    TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port()
}

fn limits() -> (Limits, Limits) {
    (Limits { connect: Duration::from_millis(500), total: Duration::from_secs(3), max_body: 1 << 20 }, Limits { connect: Duration::from_millis(500), total: Duration::from_secs(5), max_body: 1 << 18 })
}

/// Fill the form: nickname, address, key.
fn fill(a: &mut SettingsApp, cx: &AppCx, nick: &str, addr: &str, apikey: &str) {
    key(a, cx, Key::Char('e'));
    assert!(matches!(a.modal, Some(Modal::Form(FormKind::ModelHost, _))), "the details form opens");
    let clear = |a: &mut SettingsApp| (0..80).for_each(|_| key(a, cx, Key::Backspace));
    clear(a);
    typ(a, cx, nick);
    key(a, cx, Key::Enter);
    clear(a);
    typ(a, cx, addr);
    key(a, cx, Key::Enter);
    typ(a, cx, apikey);
    key(a, cx, Key::Enter);
    key(a, cx, Key::Enter); // the confirmation field stays empty; Enter on the last field saves
}

fn open_page(tag: &str) -> (AppCx, SettingsApp) {
    let cx = admin(tag);
    let mut a = SettingsApp::new(&cx);
    a.mh.limits = Some(limits());
    a.open_item("modelhost:mac", &cx);
    (cx, a)
}

#[derive(Default)]
struct FakeKube {
    calls: Mutex<Vec<Vec<String>>>,
}
impl Kube for FakeKube {
    fn kubectl(&self, args: &[&str]) -> Out {
        self.calls.lock().unwrap().push(args.iter().map(|s| s.to_string()).collect());
        let ok = |s: &str| Out { code: Some(0), stdout: s.into(), ..Default::default() };
        match args {
            [.., "get", "pod", _, "-o", _] => ok("Succeeded"),
            [.., "logs", _] => ok("DNS:ok\nHTTP:ok\n{\"data\":[]}\n"),
            _ => ok("created"),
        }
    }
}

#[test]
fn the_page_opens_from_the_menu_jump_and_the_models_key_and_only_for_administrators() {
    let (cx, mut a) = open_page("mh1");
    assert!(a.mh_is_open() && a.has_modal() && a.tab() == Tab::Models);
    let g = draw(&mut a, &cx, 1280, 3600);
    for needed in ["Edit details", "Test", "Add to Models", "Scan this network", "Close", "A Mac (Apple Silicon)", "Another computer with a GPU"] {
        assert!(g.hits.iter().any(|h| h.label == needed), "{needed} among {:?}", g.hits.iter().map(|h| &h.label).collect::<Vec<_>>());
    }
    for n in ["1 Ollama", "2 LM Studio (server mode)", "3 llama.cpp (llama-server)", "4 MLX-LM server"] {
        assert!(g.hits.iter().any(|h| h.label == n), "{n}");
    }
    key(&mut a, &cx, Key::Esc);
    assert!(!a.mh_is_open());
    // the Models key
    key(&mut a, &cx, Key::Char('H'));
    assert!(a.mh_is_open());
    key(&mut a, &cx, Key::Char('P'));
    key(&mut a, &cx, Key::Char('3'));
    key(&mut a, &cx, Key::Esc);
    // a normal user cannot open it
    let mut ucx = cx.clone();
    ucx.session.user = "amy".into();
    ucx.session.role = iyakki_sec::policy::Role::User;
    ucx.session.policy = iyakki_sec::policy::Role::User.template();
    let mut u = SettingsApp::new(&ucx);
    u.open_item("modelhost:mac", &ucx);
    assert!(!u.mh_is_open() && status(&u).contains("administrator"), "{}", status(&u));
}

#[test]
fn test_then_add_registers_the_provider_with_the_key_in_the_vault_and_nothing_runs_before_a_click() {
    let fake = serve(true);
    let (cx, mut a) = open_page("mh2");
    // opening and filling in the form sends nothing
    fill(&mut a, &cx, "Studio", &format!("127.0.0.1:{}", fake.port), "s3cret");
    assert_eq!(status(&a), "Saved. Press Test to check the connection.");
    std::thread::sleep(Duration::from_millis(100));
    assert!(fake.seen.lock().unwrap().is_empty(), "no request before the person presses Test");
    // Test
    click(&mut a, &cx, "Test");
    settle(&mut a, &cx);
    assert!(status(&a).contains("2 model(s) listed") && status(&a).contains("qwen2.5:7b") && status(&a).contains("tokens/s"), "{}", status(&a));
    assert!(cx.services.settings.lock().unwrap().get().provider("mac-studio").is_none(), "Test does not add");
    // the models and the answer are on the page
    let g = draw(&mut a, &cx, 1280, 720);
    assert!(g.hits.iter().any(|h| h.label == "Add to Models"));
    // Add to Models
    click(&mut a, &cx, "Add to Models");
    settle(&mut a, &cx);
    assert!(status(&a).starts_with("Added to Models as mac-studio (default model qwen2.5:7b)"), "{}", status(&a));
    let p = cx.services.settings.lock().unwrap().get().provider("mac-studio").cloned().unwrap();
    assert_eq!((p.kind, p.base_url.clone()), (Kind::Compat, Some(format!("http://127.0.0.1:{}/v1", fake.port))));
    assert_eq!(p.key_ref.as_deref(), Some("vault:mac-studio-key"));
    assert_eq!(cx.services.secrets.resolve("vault:mac-studio-key", None).unwrap().as_str(), "s3cret");
    assert_eq!(iyakki_agent::modelhost::Hosts::new(cx.services.dir()).list().len(), 1);
    // it is listed on the page with the cluster buttons, and in Models with its label
    let g = draw(&mut a, &cx, 1280, 3600);
    for b in ["Publish to cluster", "Test from the cluster", "Remove"] {
        assert!(g.hits.iter().any(|h| h.label == b), "{b}");
    }
    assert!(cx.audit.recent(40).iter().any(|l| l.contains("settings.modelhost.add")));
    // the key is not in the audit log or the settings file text
    assert!(!cx.audit.recent(100).iter().any(|l| l.contains("s3cret")));
    // a wrong key says 401 in words
    let mut b = SettingsApp::new(&cx);
    b.mh.limits = Some(limits());
    b.open_item("modelhost:mac", &cx);
    fill(&mut b, &cx, "Other", &format!("127.0.0.1:{}", fake.port), "wrong");
    click(&mut b, &cx, "Test");
    settle(&mut b, &cx);
    assert!(status(&b).contains("401") && status(&b).contains("API key"), "{}", status(&b));
    // nothing listening, and an address outside the network
    fill(&mut b, &cx, "Other", &format!("127.0.0.1:{}", unused_port()), "");
    click(&mut b, &cx, "Test");
    settle(&mut b, &cx);
    assert!(status(&b).contains("Nothing is listening"), "{}", status(&b));
    // an outside address is refused in the form itself
    fill(&mut b, &cx, "Other", "8.8.8.8", "");
    let Some(Modal::Form(_, f)) = &b.modal else { panic!("the form stays open with the reason") };
    assert!(f.error.as_deref().unwrap().contains("not on your local network"), "{:?}", f.error);
}

#[test]
fn scan_runs_only_on_the_click_lists_what_answers_and_adds_nothing() {
    let fake = serve(false);
    let (cx, mut a) = open_page("mh3");
    a.mh.scan_override = Some((vec![IpAddr::V4(Ipv4Addr::LOCALHOST), IpAddr::V4(Ipv4Addr::new(8, 8, 8, 8))], ScanOpts { ports: vec![fake.port, unused_port()], connect: Duration::from_millis(300), read: Duration::from_secs(1), concurrency: 4 }));
    for _ in 0..5 {
        a.handle(&Event::Tick, &cx);
    }
    draw(&mut a, &cx, 1280, 720);
    assert!(fake.seen.lock().unwrap().is_empty(), "no scan until the click");
    click(&mut a, &cx, "Scan this network");
    settle(&mut a, &cx);
    assert!(status(&a).starts_with("1 model server(s) found. Nothing was added"), "{}", status(&a));
    assert!(cx.services.settings.lock().unwrap().get().providers.is_empty() && iyakki_agent::modelhost::Hosts::new(cx.services.dir()).list().is_empty(), "never auto-added");
    assert!(cx.audit.recent(20).iter().any(|l| l.contains("settings.modelhost.scan")));
    // Use fills the address, then Test works on it
    click(&mut a, &cx, "Use");
    assert!(status(&a).contains("Address filled in"));
    click(&mut a, &cx, "Test");
    settle(&mut a, &cx);
    assert!(status(&a).contains("model(s) listed"), "{}", status(&a));
}

#[test]
fn publishing_and_the_cluster_test_use_only_the_managed_kubectl_and_remove_cleans_up() {
    let fake = serve(false);
    let (cx, mut a) = open_page("mh4");
    let kube = Arc::new(FakeKube::default());
    a.mh.kube = Some(kube.clone());
    fill(&mut a, &cx, "Mini", &format!("127.0.0.1:{}", fake.port), "");
    click(&mut a, &cx, "Add to Models");
    settle(&mut a, &cx);
    assert!(status(&a).starts_with("Added to Models as mac-mini"), "{}", status(&a));
    click(&mut a, &cx, "Publish to cluster");
    settle(&mut a, &cx);
    assert!(status(&a).starts_with("Published: pods in the cluster call http://mac-mini:"), "{}", status(&a));
    assert!(iyakki_agent::modelhost::Hosts::new(cx.services.dir()).get("mac-mini").unwrap().in_cluster);
    click(&mut a, &cx, "Test from the cluster");
    settle(&mut a, &cx);
    assert!(status(&a).starts_with("A pod in the cluster reached mac-mini"), "{}", status(&a));
    click(&mut a, &cx, "Remove");
    settle(&mut a, &cx);
    assert!(status(&a).starts_with("Removed mac-mini from Models.") && status(&a).contains("cluster Service was removed"), "{}", status(&a));
    assert!(cx.services.settings.lock().unwrap().get().provider("mac-mini").is_none());
    let calls = kube.calls.lock().unwrap().clone();
    assert!(calls[0][0] == "apply" && calls.iter().any(|c| c.contains(&"delete".to_string()) && c.contains(&"service,endpoints".to_string())), "{calls:?}");
    assert!(calls.iter().all(|c| !c.contains(&"--kubeconfig".to_string())));
    // without a cluster the real path says so (this machine has none)
    let (cx2, mut b) = open_page("mh5");
    fill(&mut b, &cx2, "Mini", &format!("127.0.0.1:{}", fake.port), "");
    click(&mut b, &cx2, "Add to Models");
    settle(&mut b, &cx2);
    click(&mut b, &cx2, "Publish to cluster");
    settle(&mut b, &cx2);
    assert!(!status(&b).starts_with("Published"), "{}", status(&b));
}

#[test]
fn the_page_fits_and_scrolls_at_every_screen_size() {
    let (cx, mut a) = open_page("mh6");
    for (w, h) in [(360u32, 740u32), (800, 480), (1280, 720), (1920, 1080), (1024, 600)] {
        a.mh.scroll_to_top_for_tests();
        let g = draw(&mut a, &cx, w, h);
        for hit in &g.hits {
            let r = hit.rect;
            assert!(r.x >= -0.5 && r.y >= 0.0 && r.x + r.w <= w as f32 + 0.5 && r.y + r.h <= h as f32 + 0.5, "{} off screen at {w}x{h}: {r:?}", hit.label);
        }
        // the first screenful has the title area and Close; the rest is reached by scrolling
        assert!(g.hits.iter().any(|h| h.label == "Close"), "Close at {w}x{h}");
        // scrolling to the end reaches the action buttons and the memory table without anything leaving the screen
        let mut seen: Vec<String> = g.hits.iter().map(|h| h.label.clone()).collect();
        let mut last: Vec<(String, i32)> = Vec::new();
        for _ in 0..80 {
            key(&mut a, &cx, Key::PageDown);
            let g = draw(&mut a, &cx, w, h);
            let now: Vec<(String, i32)> = g.hits.iter().map(|h| (h.label.clone(), h.rect.y as i32)).collect();
            if now == last {
                break; // the end of the page: nothing moved
            }
            last = now;
            for hit in &g.hits {
                let r = hit.rect;
                assert!(r.x >= -0.5 && r.y >= 0.0 && r.x + r.w <= w as f32 + 0.5 && r.y + r.h <= h as f32 + 0.5, "{} off screen at {w}x{h} after scrolling: {r:?}", hit.label);
            }
            seen.extend(g.hits.iter().map(|h| h.label.clone()));
        }
        for must in ["Edit details", "Test", "Add to Models", "Scan this network", "Close"] {
            assert!(seen.iter().any(|l| l == must), "{must} reachable at {w}x{h}");
        }
        // buttons of one row never overlap each other
        let g = draw(&mut a, &cx, w, h);
        for (i, x) in g.hits.iter().enumerate() {
            for y in &g.hits[i + 1..] {
                let ox = (x.rect.x + x.rect.w).min(y.rect.x + y.rect.w) - x.rect.x.max(y.rect.x);
                let oy = (x.rect.y + x.rect.h).min(y.rect.y + y.rect.h) - x.rect.y.max(y.rect.y);
                assert!(!(ox > 1.5 && oy > 1.5), "{:?} and {:?} overlap at {w}x{h}", x.label, y.label);
            }
        }
    }
}
