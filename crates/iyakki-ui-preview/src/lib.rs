//! A loopback HTTP server for one SDK app (see the crate manifest). Routes:
//!
//! * `GET /` the page (a canvas and the accessibility tree, drawn by `displaylist.js`)
//! * `GET /displaylist.js` the browser renderer (the same file the web service serves as `/ui/displaylist.js`)
//! * `GET /frame` the current frame as display list text
//! * `POST /event` input lines (see `iyakki_ui_sdk::wire`) in the body; answers the new frame, or 400 with the reason
//!
//! Safety: bound to 127.0.0.1; a request whose Host header is not that address or `localhost` is refused (DNS rebinding); `POST` needs the
//! header `X-Iyakki-Preview: 1` (a page of another site cannot add it without a pre-flight request, which is never answered); bodies are
//! at most 64 KB; one request is served at a time with a 5 second read limit.

use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc};
use std::time::Duration;

use iyakki_ui_sdk::app::UiApp;
use iyakki_ui_sdk::wire::Session;

pub const RENDERER: &str = include_str!("../../iyakki-agent/src/webservice/ui/displaylist.js");
const PAGE: &str = include_str!("page.html");
const MAX_BODY: usize = 64 * 1024;

struct Req {
    method: String,
    path: String,
    host: String,
    marked: bool,
    body: Vec<u8>,
}

fn read_request(s: &mut TcpStream) -> Result<Req, String> {
    s.set_read_timeout(Some(Duration::from_secs(5))).map_err(|e| e.to_string())?;
    let mut r = BufReader::new(s.try_clone().map_err(|e| e.to_string())?);
    let mut line = String::new();
    r.by_ref().take(4096).read_line(&mut line).map_err(|e| e.to_string())?;
    let mut it = line.split_whitespace();
    let (method, path) = (it.next().unwrap_or("").to_string(), it.next().unwrap_or("").to_string());
    let (mut host, mut marked, mut len) = (String::new(), false, 0usize);
    for _ in 0..64 {
        let mut h = String::new();
        r.by_ref().take(4096).read_line(&mut h).map_err(|e| e.to_string())?;
        let h = h.trim_end();
        if h.is_empty() {
            break;
        }
        if let Some((k, v)) = h.split_once(':') {
            match k.trim().to_ascii_lowercase().as_str() {
                "host" => host = v.trim().to_string(),
                "x-iyakki-preview" => marked = v.trim() == "1",
                "content-length" => len = v.trim().parse().map_err(|_| "bad length".to_string())?,
                _ => {}
            }
        }
    }
    if len > MAX_BODY {
        return Err("body too big".into());
    }
    let mut body = vec![0u8; len];
    r.read_exact(&mut body).map_err(|e| e.to_string())?;
    Ok(Req { method, path, host, marked, body })
}

type Answer = (u16, &'static str, Vec<u8>);

fn write_answer(s: &mut TcpStream, a: &Answer) {
    let (code, ctype, body) = a;
    let reason = match code {
        200 => "OK",
        400 => "Bad Request",
        403 => "Forbidden",
        404 => "Not Found",
        _ => "Error",
    };
    let head = format!("HTTP/1.1 {code} {reason}\r\nContent-Type: {ctype}\r\nContent-Length: {}\r\nCache-Control: no-store\r\nX-Content-Type-Options: nosniff\r\nConnection: close\r\n\r\n", body.len());
    let _ = s.write_all(head.as_bytes());
    let _ = s.write_all(body);
}

const TEXT: &str = "text/plain; charset=utf-8";

fn host_ok(host: &str, port: u16) -> bool {
    host == format!("127.0.0.1:{port}") || host == format!("localhost:{port}")
}

fn answer<A: UiApp>(req: Req, port: u16, session: &mut Session<A>) -> Answer {
    if !host_ok(&req.host, port) {
        return (403, TEXT, b"wrong host".to_vec());
    }
    // the query (the page's `?engine=wasm`) is for the page's script, not for routing
    match (req.method.as_str(), req.path.split('?').next().unwrap_or("")) {
        ("GET", "/") => (200, "text/html; charset=utf-8", PAGE.as_bytes().to_vec()),
        ("GET", "/displaylist.js") => (200, "text/javascript; charset=utf-8", RENDERER.as_bytes().to_vec()),
        // the client engine module and its glue, for the page's `?engine=wasm` mode (F-SDK-29)
        ("GET", "/engine.js") => (200, "text/javascript; charset=utf-8", iyakki_client_engine::GLUE_JS.as_bytes().to_vec()),
        ("GET", "/uidisplay.js") => (200, "text/javascript; charset=utf-8", iyakki_client_engine::UIDISPLAY_JS.as_bytes().to_vec()),
        ("GET", "/engine.wasm") => (200, "application/wasm", iyakki_client_engine::WASM.to_vec()),
        ("GET", "/frame") => (200, TEXT, session.frame().into_bytes()),
        ("POST", "/event") => {
            if !req.marked {
                return (403, TEXT, b"missing X-Iyakki-Preview header".to_vec());
            }
            match String::from_utf8(req.body).map_err(|_| "input is not UTF-8".to_string()).and_then(|t| session.apply(&t)) {
                Ok(frame) => (200, TEXT, frame.into_bytes()),
                Err(e) => (400, TEXT, e.into_bytes()),
            }
        }
        _ => (404, TEXT, b"not found".to_vec()),
    }
}

/// Serve `session` on `listener` (which must be bound to 127.0.0.1) until `stop` is set. Blocks. Every connection is read on its own
/// thread (a browser opens idle connections in advance), but the app is only ever touched here, one request at a time.
pub fn serve<A: UiApp>(listener: TcpListener, mut session: Session<A>, stop: Arc<AtomicBool>) -> std::io::Result<()> {
    let addr = listener.local_addr()?;
    if !addr.ip().is_loopback() {
        return Err(std::io::Error::new(std::io::ErrorKind::InvalidInput, "the preview only listens on the loopback address"));
    }
    listener.set_nonblocking(true)?;
    let (tx, rx) = mpsc::channel::<(Req, mpsc::Sender<Answer>)>();
    while !stop.load(Ordering::Relaxed) {
        match listener.accept() {
            Ok((mut s, _)) => {
                let _ = s.set_nonblocking(false);
                let tx = tx.clone();
                std::thread::spawn(move || match read_request(&mut s) {
                    Ok(req) => {
                        let (rtx, rrx) = mpsc::channel();
                        if tx.send((req, rtx)).is_ok() {
                            if let Ok(a) = rrx.recv_timeout(Duration::from_secs(10)) {
                                write_answer(&mut s, &a);
                            }
                        }
                    }
                    Err(e) => write_answer(&mut s, &(400, TEXT, e.into_bytes())),
                });
            }
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {}
            Err(e) => return Err(e),
        }
        while let Ok((req, rtx)) = rx.try_recv() {
            let _ = rtx.send(answer(req, addr.port(), &mut session));
        }
        std::thread::sleep(Duration::from_millis(2));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use iyakki_app_sample::SampleApp;
    use iyakki_ui_sdk::desktop::{Arrangement, MemDock, MemWallpaper};
    use iyakki_ui_sdk::host::Host;
    use iyakki_ui_sdk::look::Look;

    fn start() -> (u16, Arc<AtomicBool>, std::thread::JoinHandle<()>) {
        let l = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = l.local_addr().unwrap().port();
        let stop = Arc::new(AtomicBool::new(false));
        let s2 = stop.clone();
        // the app is made on the serving thread (an SDK app is not `Send`)
        (port, stop, std::thread::spawn(move || {
            let host = Host::new(SampleApp::new(), Look::default(), Arrangement::single(360, 740), Box::new(MemDock::knowing(&["sample"])), Box::new(MemWallpaper::default()));
            serve(l, Session::new(host), s2).unwrap()
        }))
    }

    fn http(port: u16, method: &str, path: &str, host: Option<&str>, marked: bool, body: &str) -> (u16, String) {
        let mut s = TcpStream::connect(("127.0.0.1", port)).unwrap();
        let h = host.map(str::to_string).unwrap_or(format!("127.0.0.1:{port}"));
        let m = if marked { "X-Iyakki-Preview: 1\r\n" } else { "" };
        // a refused big body may be answered and closed before it is all sent: a reset then counts as the refusal (code 0)
        let _ = write!(s, "{method} {path} HTTP/1.1\r\nHost: {h}\r\n{m}Content-Length: {}\r\n\r\n{body}", body.len());
        let mut out = String::new();
        let _ = s.read_to_string(&mut out);
        let code = out.split(' ').nth(1).and_then(|c| c.parse().ok()).unwrap_or(0);
        (code, out.split_once("\r\n\r\n").map(|x| x.1.to_string()).unwrap_or_default())
    }

    #[test]
    fn the_preview_serves_the_page_the_renderer_and_frames_and_applies_input_over_real_sockets() {
        let (port, stop, t) = start();
        let (c, page) = http(port, "GET", "/", None, false, "");
        assert!(c == 200 && page.contains("/displaylist.js") && page.contains("aria"), "{page}");
        let (c, js) = http(port, "GET", "/displaylist.js", None, false, "");
        assert!(c == 200 && js.contains("iyakki-display-list"));
        let (c, page2) = http(port, "GET", "/?engine=wasm", None, false, "");
        assert!(c == 200 && page2.contains("engine=wasm"), "a query on the page path is still the page");
        let (c, js) = http(port, "GET", "/engine.js", None, false, "");
        assert!(c == 200 && js.contains("class IyakkiEngine"));
        let (c, frame) = http(port, "GET", "/frame", None, false, "");
        assert!(c == 200 && frame.starts_with("iyakki-display-list 1 360 740\n") && frame.contains("Sample"));
        // a key press goes through the host: Enter on an empty field does nothing, text typed after a tap on the field is added
        let (c, f2) = http(port, "POST", "/event", None, true, "resize 800 480\n");
        assert!(c == 200 && f2.starts_with("iyakki-display-list 1 800 480\n"), "{f2}");
        let list = iyakki_ui_sdk::list::DisplayList::parse(&f2).unwrap();
        let (x, y) = list.ops.iter().find_map(|o| if let iyakki_ui_sdk::list::Op::Hit(r, h) = o { (h.name == "New note").then_some((r.x + r.w / 2.0, r.y + r.h / 2.0)) } else { None }).unwrap();
        let (c, _) = http(port, "POST", "/event", None, true, &format!("ptr 0 down {x} {y} 0\nptr 0 up {x} {y} 40\n"));
        assert_eq!(c, 200);
        let (_, f3) = http(port, "POST", "/event", None, true, "text buy%20milk\nkey enter\n");
        assert!(f3.contains("buy%20milk"), "{f3}");
        // refusals: wrong host (DNS rebinding), a POST without the marker, a bad batch, an unknown path, a body that is too big
        assert_eq!(http(port, "GET", "/frame", Some("evil.example:80"), false, "").0, 403);
        assert_eq!(http(port, "POST", "/event", None, false, "tick 1\n").0, 403);
        let (c, why) = http(port, "POST", "/event", None, true, "fly 1\n");
        assert!(c == 400 && why.contains("line 1") && why.contains("fly"), "{why}");
        assert_eq!(http(port, "GET", "/etc/passwd", None, false, "").0, 404);
        assert!(matches!(http(port, "POST", "/event", None, true, &"x".repeat(70_000)).0, 400 | 0));
        stop.store(true, Ordering::Relaxed);
        t.join().unwrap();
    }

    #[test]
    fn the_preview_refuses_to_listen_on_anything_but_the_loopback_address() {
        let l = TcpListener::bind("0.0.0.0:0").unwrap();
        let host = Host::new(SampleApp::new(), Look::default(), Arrangement::single(360, 740), Box::new(MemDock::default()), Box::new(MemWallpaper::default()));
        let e = serve(l, Session::new(host), Arc::new(AtomicBool::new(true))).unwrap_err();
        assert_eq!(e.kind(), std::io::ErrorKind::InvalidInput);
    }

    #[test]
    fn the_cameras_pane_over_the_wire_gives_the_browser_jpeg_pictures_only_after_the_card_is_allowed() {
        use iyakki_ui_sdk::cameras::{CamInfo, MemCameras};
        let m = MemCameras::new("sample");
        m.add(CamInfo::new("c0", "Front door"));
        m.push_solid("c0", 8, 6, [10, 20, 30]);
        let host = Host::new(SampleApp::new().with_cameras_for_the_wire(Box::new(m.clone())), Look::default(), Arrangement::single(360, 740), Box::new(MemDock::knowing(&["sample"])), Box::new(MemWallpaper::default()));
        let mut s = Session::new(host);
        // the Cameras tab, tick the camera, Show, a heartbeat or two: the card is on screen and nothing was pulled
        let f = s.apply("action 25\ntick 10\naction 50020\naction 50000\ntick 300\ntick 700\n").unwrap();
        assert!(f.contains("Allow") && !f.lines().any(|l| l.starts_with("image ")), "{f}");
        assert_eq!(m.pulled("c0"), 0);
        // Allow: the next heartbeats bring the picture, as a JPEG line a browser decodes
        let f = s.apply("action 50001\ntick 1000\ntick 1300\n").unwrap();
        let img: Vec<&str> = f.lines().filter(|l| l.starts_with("image ")).collect();
        assert_eq!(img.len(), 1, "{f}");
        assert!(img[0].contains(" jpeg 8 6 "), "{}", img[0]);
        assert!(iyakki_ui_sdk::list::DisplayList::parse(&f).is_ok());
        assert!(m.pulled("c0") >= 1);
    }
}
