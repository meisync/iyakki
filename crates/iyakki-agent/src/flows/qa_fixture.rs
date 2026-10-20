// #[cfg(test)] -- this whole file is test code (it is included under #[cfg(test)] from flows/mod.rs); the marker keeps the BOUNDARY check from counting it.
//! Test fixtures for the browser nodes: a local web server on 127.0.0.1 (login form, select and checkboxes, a delayed element,
//! console errors, a redirect, a 404, an iframe, a download) and a page-engine double built on the in-house JavaScript engine
//! (`iyakki-js`) that loads those pages over real HTTP from the fixture. The double draws a picture from the page's text, so a
//! different page gives a different picture, and it follows `Resize`.

use std::collections::VecDeque;
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::Arc;
use std::time::Duration;

use iyakki_js::{run_page_with, JsEngine, Limits, SiteData};
use iyakki_web::fetch::Response;
use iyakki_web::{Fetch, Url};
use iyakki_webview::proto::ToHelper;

use super::browser::{EngineFactory, EngineKeeper, EngineSpec};
use crate::browserjobs::{Frame, PageEngine, PageState, Servicer};

// ------------------------------------------------------------------------------------------------ the server

pub const LOGIN: &str = r#"<html><head><title>Sign in</title></head><body><h1>Sign in</h1>
<form id="login"><label for="email">Email</label><input id="email" name="email" type="email">
<label for="pw">Password</label><input id="pw" name="password" type="password">
<label><input id="remember" type="checkbox" name="remember"> Remember me</label>
<select id="role"><option value="">Choose</option><option value="qa">QA engineer</option><option value="dev">Developer</option></select>
<button id="go" type="submit">Sign in</button></form><p id="result"></p><p id="echo"></p>
<script>document.getElementById('login').addEventListener('submit',function(e){e.preventDefault();var em=document.getElementById('email').value;var pw=document.getElementById('pw').value;var r=document.getElementById('result');
if(pw==='s3cret-pass'&&em){r.textContent='Welcome, '+em+(document.getElementById('remember').checked?' (remembered)':'');r.className='ok';document.getElementById('echo').textContent='debug: you typed '+pw;}
else{r.textContent='Invalid login';r.className='bad';console.error('login failed for '+em);}});</script></body></html>"#;

const FORM: &str = r#"<html><head><title>Preferences</title></head><body><h1>Preferences</h1>
<select id="color"><option value="r">Red</option><option value="g">Green</option><option value="b">Blue</option></select>
<label><input id="news" type="checkbox"> Newsletter</label>
<label><input id="a" type="radio" name="plan" value="free"> Free</label><label><input id="b" type="radio" name="plan" value="pro"> Pro</label>
<textarea id="note"></textarea><input id="locked" disabled value="x"><button id="save">Save</button><p id="saved"></p>
<script>document.getElementById('save').addEventListener('click',function(){document.getElementById('saved').textContent='color='+(function(c){return c.options[c.selectedIndex].value;})(document.getElementById('color'))+' news='+document.getElementById('news').checked+' plan='+(document.getElementById('b').checked?'pro':document.getElementById('a').checked?'free':'none');});</script></body></html>"#;

const DELAYED: &str = r#"<html><head><title>Delayed</title></head><body><h1>Delayed</h1><div id="status">waiting</div>
<script>setTimeout(function(){var d=document.createElement('div');d.id='late';d.textContent='Loaded late';document.body.appendChild(d);document.getElementById('status').textContent='ready';},700);</script></body></html>"#;

const ERRORS: &str = r#"<html><head><title>Errors</title></head><body><h1>Errors</h1><img src="/missing.png" alt="gone"><a href="/ok">fine</a> <a href="/nope">broken</a> <a href="/ok2">fine two</a>
<button id="boom">Trigger</button>
<script>console.error('boom at load');document.getElementById('boom').addEventListener('click',function(){console.error('boom from click');console.warn('careful');setTimeout(function(){console.error('boom from timer');},80);setTimeout(function(){throw new Error('async failure');},90);});</script></body></html>"#;

const TABLE: &str = r#"<html><head><title>Prices</title></head><body><h1>Prices</h1><table id="t"><tr><th>Name</th><th>Qty</th></tr>
<tr><td>apple</td><td>3</td></tr><tr><td>pear</td><td>5</td></tr><tr><td>plum</td><td>8</td></tr></table>
<span class="price">$12.50</span><a class="buy" href="/buy?item=1">Buy</a><i class="tag">a</i><i class="tag">b</i>
<input type="hidden" name="csrf_token" value="TOKEN-ABC-123"><div data-token="DATA-TOKEN-999">x</div></body></html>"#;

/// Six blocks of 500 px in six colours: a whole-page picture must show them in order.
const LONG: &str = r#"<html><head><title>Long</title></head><body style="margin:0"><div style="height:500px;background:#ff0000"></div><div style="height:500px;background:#00ff00"></div><div style="height:500px;background:#0000ff"></div><div style="height:500px;background:#ffff00"></div><div style="height:500px;background:#00ffff"></div><div style="height:500px;background:#ff00ff"></div></body></html>"#;
const FRAME: &str = r#"<html><head><title>Frames</title></head><body><h1>Outer</h1><iframe id="inner" src="/inner" width="300" height="100"></iframe></body></html>"#;
const INNER: &str = r#"<html><head><title>Inner</title></head><body><h2 id="in">Inside the frame</h2><button id="inbtn">Inner button</button></body></html>"#;
const DOWNLOAD: &str = r#"<html><head><title>Files</title></head><body><a id="dl" href="/file.txt" download>Get the file</a></body></html>"#;

fn route(path: &str) -> (u16, Vec<(&'static str, String)>, Vec<u8>) {
    let html = |s: &str| (200, vec![("Content-Type", "text/html; charset=utf-8".to_string())], s.as_bytes().to_vec());
    match path.split('?').next().unwrap_or("") {
        "/" => html(r#"<html><head><title>Home</title></head><body><h1>Home</h1><a href="/login">Login</a></body></html>"#),
        "/login" => html(LOGIN),
        "/form" => html(FORM),
        "/delayed" => html(DELAYED),
        "/errors" => html(ERRORS),
        "/table" => html(TABLE),
        "/frame" => html(FRAME),
        "/long" => html(LONG),
        "/inner" => html(INNER),
        "/downloads" => html(DOWNLOAD),
        "/ok" | "/ok2" => html("<html><head><title>OK</title></head><body>fine</body></html>"),
        "/redirect" => (302, vec![("Location", "/login".to_string())], b"moved".to_vec()),
        "/file.txt" => (200, vec![("Content-Type", "text/plain".to_string()), ("Content-Disposition", "attachment; filename=\"report.txt\"".to_string())], b"quarterly report\n".to_vec()),
        _ => (404, vec![("Content-Type", "text/html; charset=utf-8".to_string())], b"<html><head><title>Not found</title></head><body><h1>Not here</h1></body></html>".to_vec()),
    }
}

pub struct Fixture {
    pub port: u16,
    stop: Arc<AtomicBool>,
    pub hits: Arc<std::sync::Mutex<Vec<String>>>,
}

impl Fixture {
    pub fn start() -> Fixture {
        let l = TcpListener::bind("127.0.0.1:0").unwrap();
        l.set_nonblocking(true).unwrap();
        let port = l.local_addr().unwrap().port();
        let stop = Arc::new(AtomicBool::new(false));
        let hits = Arc::new(std::sync::Mutex::new(Vec::new()));
        let (s2, h2) = (stop.clone(), hits.clone());
        std::thread::spawn(move || {
            while !s2.load(Ordering::SeqCst) {
                match l.accept() {
                    Ok((c, _)) => {
                        let h3 = h2.clone();
                        std::thread::spawn(move || serve_one(c, h3));
                    }
                    Err(_) => std::thread::sleep(Duration::from_millis(5)),
                }
            }
        });
        Fixture { port, stop, hits }
    }
    pub fn url(&self, path: &str) -> String {
        format!("http://127.0.0.1:{}{path}", self.port)
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
    }
}

fn serve_one(mut c: TcpStream, hits: Arc<std::sync::Mutex<Vec<String>>>) {
    let _ = c.set_nonblocking(false);
    let _ = c.set_read_timeout(Some(Duration::from_secs(5)));
    let mut buf = [0u8; 4096];
    let mut got = Vec::new();
    loop {
        let n = c.read(&mut buf).unwrap_or(0);
        got.extend_from_slice(&buf[..n]);
        if n == 0 || got.windows(4).any(|w| w == b"\r\n\r\n") {
            break;
        }
    }
    let text = String::from_utf8_lossy(&got).into_owned();
    let line = text.lines().next().unwrap_or("");
    let path = line.split_whitespace().nth(1).unwrap_or("/").to_string();
    hits.lock().unwrap().push(line.to_string());
    let (status, headers, body) = route(&path);
    let mut head = format!("HTTP/1.1 {status} X\r\nContent-Length: {}\r\nConnection: close\r\n", body.len());
    for (k, v) in headers {
        head.push_str(&format!("{k}: {v}\r\n"));
    }
    head.push_str("\r\n");
    let _ = c.write_all(head.as_bytes());
    let _ = c.write_all(&body);
}

// ------------------------------------------------------------------------------------------------ the engine double

struct NoWeb;
impl Fetch for NoWeb {
    fn get(&self, _: &Url) -> Result<Response, String> {
        Err("offline".into())
    }
}

/// A blocking GET over plain HTTP to the fixture, following redirects: (final url, status, body).
fn get(url: &str) -> (String, u16, String) {
    let mut url = url.to_string();
    for _ in 0..5 {
        let rest = url.strip_prefix("http://").unwrap_or("");
        let (hostport, path) = rest.split_once('/').map_or((rest, String::from("/")), |(h, p)| (h, format!("/{p}")));
        let Ok(mut s) = TcpStream::connect(hostport) else { return (url, 0, String::new()) };
        let _ = s.set_read_timeout(Some(Duration::from_secs(5)));
        let _ = s.write_all(format!("GET {path} HTTP/1.1\r\nHost: {hostport}\r\nConnection: close\r\n\r\n").as_bytes());
        let mut raw = Vec::new();
        let _ = s.read_to_end(&mut raw);
        let text = String::from_utf8_lossy(&raw).into_owned();
        let (head, body) = text.split_once("\r\n\r\n").unwrap_or((&text, ""));
        let status: u16 = head.split_whitespace().nth(1).and_then(|x| x.parse().ok()).unwrap_or(0);
        if matches!(status, 301 | 302 | 303 | 307) {
            if let Some(loc) = head.lines().find_map(|l| l.strip_prefix("Location: ")) {
                url = if loc.contains("://") { loc.to_string() } else { format!("http://{hostport}{loc}") };
                continue;
            }
        }
        return (url, status, body.to_string());
    }
    (url, 0, String::new())
}

enum Cmd {
    Script { id: u32, js: String },
    Navigate(String),
    Text(String),
    Key { keysym: u32 },
    Quit,
}

enum Out {
    Script { id: u32, ok: bool, json: String },
    Page { url: String, title: String, text: String },
}

fn key_name(sym: u32) -> &'static str {
    match sym {
        0xff0d => "Enter",
        0xff1b => "Escape",
        0xff09 => "Tab",
        0xff08 => "Backspace",
        0xff54 => "ArrowDown",
        0xff52 => "ArrowUp",
        _ => "Enter",
    }
}

pub struct QaPage {
    tx: Sender<Cmd>,
    rx: Receiver<Out>,
    state: PageState,
    scripts: Vec<(u32, bool, String)>,
    size: (u32, u32),
    text: String,
    pub downloads: VecDeque<(String, String)>,
}

impl QaPage {
    pub fn start(size: (u32, u32)) -> QaPage {
        let (tx, crx) = channel::<Cmd>();
        let (ctx, rx) = channel::<Out>();
        std::thread::spawn(move || {
            let mut next: Option<String> = None;
            let mut pending: VecDeque<Cmd> = VecDeque::new();
            'pages: loop {
                let url = loop {
                    if let Some(u) = next.take() {
                        break u;
                    }
                    match crx.recv() {
                        Ok(Cmd::Navigate(u)) => break u,
                        Ok(Cmd::Quit) | Err(_) => return,
                        Ok(Cmd::Script { id, .. }) => {
                            let _ = ctx.send(Out::Script { id, ok: false, json: "no page".into() });
                        }
                        Ok(_) => {}
                    }
                };
                let (final_url, _status, html) = if url.starts_with("about:") { (url.clone(), 200, "<html><body></body></html>".to_string()) } else { get(&url) };
                let parsed = Url::parse(&final_url).unwrap_or_else(|_| Url::parse("http://127.0.0.1/").unwrap());
                let mut go: Option<String> = None;
                let mut quit = false;
                let cx = ctx.clone();
                run_page_with(&html, &parsed, Arc::new(NoWeb), |_| true, &SiteData::default(), Limits::default(), |e: &mut JsEngine| {
                    let title = e.eval("t", "document.title").unwrap_or_default();
                    let text = e.eval("x", "document.body ? document.body.textContent : ''").unwrap_or_default();
                    let _ = cx.send(Out::Page { url: if final_url.starts_with("about:") { final_url.clone() } else { final_url.clone() }, title: title.trim_matches('"').to_string(), text });
                    loop {
                        let cmd = match pending.pop_front() {
                            Some(c) => c,
                            None => match crx.recv() {
                                Ok(c) => c,
                                Err(_) => {
                                    quit = true;
                                    return;
                                }
                            },
                        };
                        match cmd {
                            Cmd::Script { id, js } => {
                                let r = e.eval("script", &js);
                                let _ = e.run_jobs();
                                let _ = iyakki_js::modules::run_timers(e, 3_000, 100);
                                let _ = cx.send(match r {
                                    Ok(j) => Out::Script { id, ok: true, json: j },
                                    Err(er) => Out::Script { id, ok: false, json: format!("{er:?}") },
                                });
                            }
                            Cmd::Navigate(u) => {
                                go = Some(u);
                                return;
                            }
                            Cmd::Text(t) => {
                                let lit = serde_json::to_string(&t).unwrap();
                                let _ = e.eval("text", &format!("(function(t){{var el=document.activeElement;if(el&&'value' in el){{el.value=String(el.value||'')+t;el.dispatchEvent(new Event('input',{{bubbles:true}}));}}}})({lit})"));
                                let _ = e.run_jobs();
                            }
                            Cmd::Key { keysym } => {
                                let _ = e.eval("key", &format!("window.__iyakkiQA&&window.__iyakkiQA.run('keyjs', JSON.stringify({{key:'{}'}}))", key_name(keysym)));
                                let _ = e.run_jobs();
                                let _ = iyakki_js::modules::run_timers(e, 3_000, 100);
                            }
                            Cmd::Quit => {
                                quit = true;
                                return;
                            }
                        }
                    }
                });
                if quit {
                    return;
                }
                if let Some(u) = go {
                    next = Some(u);
                    continue 'pages;
                }
            }
        });
        QaPage { tx, rx, state: PageState { ready: true, ..PageState::default() }, scripts: Vec::new(), size, text: String::new(), downloads: VecDeque::new() }
    }
}

impl PageEngine for QaPage {
    fn send(&mut self, m: ToHelper) -> bool {
        match m {
            ToHelper::Script { id, js, .. } => self.tx.send(Cmd::Script { id, js }).is_ok(),
            ToHelper::Navigate(u) => {
                self.state.loading = true;
                self.tx.send(Cmd::Navigate(u)).is_ok()
            }
            ToHelper::Text(t) => self.tx.send(Cmd::Text(t)).is_ok(),
            ToHelper::Key { down: true, keysym, .. } => self.tx.send(Cmd::Key { keysym }).is_ok(),
            ToHelper::Resize { w, h } => {
                self.size = (w, h);
                true
            }
            _ => true,
        }
    }
    fn pump(&mut self) {
        while let Ok(o) = self.rx.try_recv() {
            match o {
                Out::Script { id, ok, json } => self.scripts.push((id, ok, json)),
                Out::Page { url, title, text } => {
                    self.state.url = url;
                    self.state.title = title;
                    self.state.loading = false;
                    self.text = text;
                }
            }
        }
    }
    fn state(&self) -> PageState {
        self.state.clone()
    }
    fn take_script(&mut self, id: u32) -> Option<(bool, String)> {
        let i = self.scripts.iter().position(|s| s.0 == id)?;
        let (_, ok, j) = self.scripts.remove(i);
        Some((ok, j))
    }
    fn frame(&self) -> Option<Frame> {
        // a picture that depends on the page: the hash of its text picks the colours; stripes make it non-flat
        let mut hsh: u32 = 2166136261;
        for b in self.text.bytes().chain(self.state.url.bytes()) {
            hsh = (hsh ^ b as u32).wrapping_mul(16777619);
        }
        let (w, h) = self.size;
        let (r, g, b) = ((hsh >> 16) as u8 | 0x40, (hsh >> 8) as u8 | 0x40, hsh as u8 | 0x40);
        let rgba: Vec<u8> = (0..w * h).flat_map(|i| if (i / w) % 16 < 3 { [255 - r, 255 - g, 255 - b, 255] } else { [r, g, b, 255] }).collect();
        Some(Frame { w, h, rgba })
    }
    fn take_downloads(&mut self) -> Vec<(String, String)> {
        self.downloads.drain(..).collect()
    }
}

impl Drop for QaPage {
    fn drop(&mut self) {
        let _ = self.tx.send(Cmd::Quit);
    }
}

struct DoubleSession {
    stop: Arc<AtomicBool>,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl Drop for DoubleSession {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

/// Services one key's job queue with a [`QaPage`] until dropped.
pub fn start_double(spec: &EngineSpec, size: (u32, u32)) -> Result<EngineKeeper, String> {
    let jobs = spec.jobs.clone();
    let key = spec.key.clone();
    let stop = Arc::new(AtomicBool::new(false));
    let flag = stop.clone();
    let mut page = QaPage::start(size);
    let thread = std::thread::spawn(move || {
        let mut sv = Servicer::new();
        while !flag.load(Ordering::Relaxed) {
            let r = sv.service(&jobs, &key, &mut page);
            if r.close {
                break;
            }
            std::thread::sleep(Duration::from_millis(if r.busy { 1 } else { 5 }));
        }
        sv.abort(&jobs, &key, "the browser was closed");
    });
    Ok(Box::new(DoubleSession { stop, thread: Some(thread) }))
}

pub fn double_factory(size: (u32, u32)) -> EngineFactory {
    Arc::new(move |spec: &EngineSpec| start_double(spec, size))
}
