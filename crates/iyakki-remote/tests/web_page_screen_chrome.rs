//! The served web page (`page.rs` + `decoder.js`) in a real headless Chrome against the real `Server` (a real TCP listener, the real
//! sign-in, the real WebSocket and the real version 2 picture encoder; only the screen behind it is a fake with a made-up picture):
//! the picture arrives and is drawn by the decoder in a Worker on an OffscreenCanvas and, with workers switched off, on the main thread
//! (red left half, blue right half, read back from what Chrome drew); then, through Chrome's own touch and key events, a tap is a click, a
//! long press a right click, a two-finger drag a scroll, a key press a key, and Ctrl+Alt+S shows the statistics overlay.
//!
//! Skipped (passes without checking) when `google-chrome` or `node` (22 or newer) is missing. No real phone, screen or network.

use iyakki_remote::input::Input;
use iyakki_remote::screen::{Frame, Gate, LoginError, Pixels, Screen};
use iyakki_remote::server::{Config, Server};
use std::process::{Command, Stdio};
use std::sync::{Arc, Mutex};
use std::time::Duration;


struct Fake {
    log: Arc<Mutex<Vec<String>>>,
    armed: bool,
    sent: bool,
    size: (u32, u32),
}

impl Screen for Fake {
    fn user(&self) -> String {
        "ann".into()
    }
    fn input(&mut self, i: Input) {
        let s = format!("{i:?}");
        match i {
            Input::Full => self.sent = false,
            Input::Size(w, h) if (w, h) != self.size => {
                self.size = (w, h);
                self.sent = false;
            }
            _ => {}
        }
        self.log.lock().unwrap().push(s);
    }
    fn viewing(&mut self, _: bool) {}
    fn step(&mut self) -> Option<Frame> {
        std::thread::sleep(Duration::from_millis(5));
        None
    }
    fn take_copy(&mut self) -> Option<String> {
        None
    }
    fn set_stream(&mut self, _on: bool) {}
    fn arm(&mut self) {
        self.armed = true;
    }
    fn take_pixels(&mut self) -> Option<Pixels> {
        if self.armed && !self.sent {
            self.armed = false;
            self.sent = true;
            let (w, h) = self.size;
            let mut rgba = Vec::with_capacity((w * h * 4) as usize);
            for _y in 0..h {
                for x in 0..w {
                    rgba.extend_from_slice(if x < w / 2 { &[220, 20, 20, 255] } else { &[20, 20, 220, 255] });
                }
            }
            return Some(Pixels { w, h, rgba });
        }
        None
    }
    fn finished(&self) -> bool {
        false
    }
}

struct G(Arc<Mutex<Vec<String>>>);
impl Gate for G {
    fn login(&self, user: &str, password: &str) -> Result<Box<dyn Screen>, LoginError> {
        if user == "ann" && password == "right" {
            Ok(Box::new(Fake { log: self.0.clone(), armed: false, sent: false, size: (600, 400) }))
        } else {
            Err(LoginError::Invalid)
        }
    }
}

const DRIVER: &str = r##"const [BASE, DIR, PORT] = process.argv.slice(2);
import { spawn } from "node:child_process";
const chrome = spawn("google-chrome", ["--headless=new", "--no-sandbox", "--disable-gpu", "--window-size=1000,700", "--remote-debugging-port=" + PORT, "--user-data-dir=" + DIR + "/prof", "about:blank"], { stdio: "ignore" });
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
const result = { logs: [], errors: [] };
const done = (code) => { try { chrome.kill(); } catch {} console.log("RESULT " + JSON.stringify(result)); setTimeout(() => process.exit(code), 100); };
process.on("uncaughtException", (e) => { result.errors.push("driver: " + e.message); done(1); });
let tabs; for (let i = 0; i < 60; i++) { try { tabs = await (await fetch("http://127.0.0.1:" + PORT + "/json")).json(); if (tabs.length) break; } catch (e) {} await sleep(200); }
const ws = new WebSocket(tabs.find((t) => t.type === "page").webSocketDebuggerUrl);
await new Promise((r) => (ws.onopen = r));
let id = 0; const pending = new Map();
ws.onmessage = (m) => {
  const j = JSON.parse(m.data);
  if (j.id && pending.has(j.id)) return pending.get(j.id)(j);
  if (j.method === "Runtime.exceptionThrown") result.logs.push("exception: " + JSON.stringify(j.params.exceptionDetails.exception?.description ?? j.params.exceptionDetails.text));
};
const call = (method, params = {}) => new Promise((r) => { const i = ++id; pending.set(i, r); ws.send(JSON.stringify({ id: i, method, params })); });
const ev = async (expr) => { const r = await call("Runtime.evaluate", { expression: expr, returnByValue: true, awaitPromise: true }); if (r.result.exceptionDetails) throw new Error("eval: " + JSON.stringify(r.result.exceptionDetails.exception?.description)); return r.result.result.value; };
const waitFor = async (expr, what, ms = 20000) => { const t0 = Date.now(); while (Date.now() - t0 < ms) { try { if (await ev(expr)) return; } catch (e) {} await sleep(100); } throw new Error("timed out waiting for " + what); };
// what Chrome really drew: a screenshot, loaded back into a canvas, sampled at fractions of the screen canvas
const colours = async () => {
  const rect = await ev("(() => { const r = document.getElementById('screen').getBoundingClientRect(); return [r.x, r.y, r.width, r.height]; })()");
  const shot = await call("Page.captureScreenshot", { format: "png" });
  return await ev("(async () => { const im = new Image(); im.src = 'data:image/png;base64," + shot.result.data + "'; await im.decode(); const c = document.createElement('canvas'); c.width = im.width; c.height = im.height; const g = c.getContext('2d'); g.drawImage(im, 0, 0); const [x, y, w, h] = " + JSON.stringify(rect) + "; const at = (fx, fy) => Array.from(g.getImageData(Math.round(x + w * fx), Math.round(y + h * fy), 1, 1).data).slice(0, 3); return { left: at(0.25, 0.5), right: at(0.75, 0.5), w, h }; })()");
};
const centre = async () => ev("(() => { const r = document.getElementById('screen').getBoundingClientRect(); return [r.x + r.width / 2, r.y + r.height / 2]; })()");
const signIn = async () => {
  await waitFor("!!document.getElementById('user') && document.readyState === 'complete'", "the sign-in form");
  await sleep(300);
  await ev("document.getElementById('user').value = 'ann'; document.getElementById('pass').value = 'right'; document.getElementById('form').requestSubmit(); true");
  await waitFor("!document.getElementById('gear').hidden", "signed in");
};
try {
  await call("Runtime.enable"); await call("Page.enable");
  await call("Emulation.setTouchEmulationEnabled", { enabled: true, maxTouchPoints: 5 });
  await call("Page.navigate", { url: BASE + "/" });
  await signIn();
  // 1. the worker path
  await waitFor("typeof painter !== 'undefined' && painter && painter.worker === true", "the worker painter");
  result.workerMode = await ev("painter.worker === true && typeof OffscreenCanvas === 'function'");
  let c; for (let i = 0; i < 60; i++) { c = await colours(); if (c.left[0] > 150 && c.right[2] > 150) break; await sleep(200); }
  result.workerColours = c;
  // 2. touch: tap, long press, two-finger scroll
  const [cx, cy] = await centre();
  const touch = (type, pts) => call("Input.dispatchTouchEvent", { type, touchPoints: pts.map((p, i) => ({ x: p[0], y: p[1], id: i + 1 })) });
  await touch("touchStart", [[cx, cy]]); await sleep(60); await touch("touchEnd", []); await sleep(400);
  result.afterTap = "tap";
  console.log("TAP_DONE"); await sleep(600);
  await touch("touchStart", [[cx - 100, cy]]); await sleep(900); await touch("touchEnd", []); await sleep(400);
  console.log("LONG_DONE"); await sleep(600);
  await touch("touchStart", [[cx - 50, cy - 60], [cx + 50, cy - 60]]); await sleep(60);
  for (let k = 1; k <= 10; k++) { await touch("touchMove", [[cx - 50, cy - 60 + k * 15], [cx + 50, cy - 60 + k * 15]]); await sleep(30); }
  await touch("touchEnd", []); await sleep(400);
  console.log("SCROLL_DONE"); await sleep(600);
  // 3. a key and the statistics overlay
  await ev("document.getElementById('screen').focus(); true");
  await call("Input.dispatchKeyEvent", { type: "keyDown", key: "q", code: "KeyQ", text: "q", windowsVirtualKeyCode: 81 });
  await call("Input.dispatchKeyEvent", { type: "keyUp", key: "q", code: "KeyQ", windowsVirtualKeyCode: 81 });
  await sleep(300);
  console.log("KEY_DONE"); await sleep(600);
  result.statsBefore = await ev("document.getElementById('stats').hidden");
  await call("Input.dispatchKeyEvent", { type: "rawKeyDown", key: "s", code: "KeyS", modifiers: 3, windowsVirtualKeyCode: 83 });
  await call("Input.dispatchKeyEvent", { type: "keyUp", key: "s", code: "KeyS", modifiers: 3, windowsVirtualKeyCode: 83 });
  await waitFor("!document.getElementById('stats').hidden && document.getElementById('stats').textContent.length > 0", "the statistics");
  result.statsText = await ev("document.getElementById('stats').textContent");
  // pinch: two fingers spread apart zoom the picture; the reset button takes it back
  await touch("touchStart", [[cx - 40, cy], [cx + 40, cy]]); await sleep(60);
  for (let k = 1; k <= 8; k++) { await touch("touchMove", [[cx - 40 - k * 20, cy], [cx + 40 + k * 20, cy]]); await sleep(30); }
  await touch("touchEnd", []); await sleep(300);
  result.zoomed = await ev("zoom");
  result.transform = await ev("document.getElementById('screen').style.transform");
  await ev("document.getElementById('rz').click(); true"); await sleep(200);
  result.zoomAfterReset = await ev("zoom");
  // the display settings: the quality choice and the data saver reach the server
  await ev("(() => { const q = document.getElementById('q'); q.value = 'low'; q.dispatchEvent(new Event('change')); return true; })()"); await sleep(300);
  await sleep(1500); result.statsLow = await ev("document.getElementById('stats').textContent");
  console.log("QUALITY_DONE"); await sleep(600);
  await ev("(() => { const s = document.getElementById('saver'); s.checked = true; s.dispatchEvent(new Event('change')); return true; })()"); await sleep(300);
  await sleep(1500); result.statsSaver = await ev("document.getElementById('stats').textContent");
  console.log("SAVER_DONE"); await sleep(600);
  // the keyboard button puts the focus in the typing field
  await ev("document.getElementById('kbd').click(); true"); await sleep(200);
  result.kbdFocus = await ev("document.activeElement && document.activeElement.id");
  // 4. the main-thread fallback: workers off, reload, sign in again
  await ev("localStorage.setItem('iyakki.noworker', '1'); true");
  await call("Page.navigate", { url: BASE + "/" });
  await sleep(500);
  await signIn();
  await waitFor("typeof painter !== 'undefined' && painter && painter.worker === false", "the main-thread painter");
  for (let i = 0; i < 60; i++) { c = await colours(); if (c.left[0] > 150 && c.right[2] > 150) break; await sleep(200); }
  result.mainColours = c;
  result.mainWorker = await ev("painter.worker");
  // the canvas itself, read back on the main thread
  result.mainPixels = await ev("(() => { const g = document.getElementById('screen').getContext('2d'); const w = document.getElementById('screen').width, h = document.getElementById('screen').height; return { w, h, left: Array.from(g.getImageData(Math.floor(w * 0.25), Math.floor(h / 2), 1, 1).data), right: Array.from(g.getImageData(Math.floor(w * 0.75), Math.floor(h / 2), 1, 1).data) }; })()");
  done(0);
} catch (e) { result.errors.push(String(e.message || e)); done(1); }
"##;

fn have(tool: &str) -> bool {
    Command::new(tool).arg("--version").stdout(Stdio::null()).stderr(Stdio::null()).status().map(|s| s.success()).unwrap_or(false)
}

#[test]
fn the_page_draws_the_picture_in_a_worker_and_on_the_main_thread_and_turns_touch_and_keys_into_input() {
    if !have("google-chrome") || !have("node") {
        eprintln!("skipped: google-chrome or node is missing");
        return;
    }
    let log = Arc::new(Mutex::new(Vec::<String>::new()));
    let srv = Server::start(Config { addr: "127.0.0.1:0".into(), tls: None }, Arc::new(G(log.clone()))).unwrap();
    let addr = srv.local_addr();
    let dir = std::env::temp_dir().join(format!("iyakki-screen-chrome-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("cdp.mjs"), DRIVER).unwrap();
    let port = 9500 + (std::process::id() % 200) as u16;
    let mut child = Command::new("node")
        .arg(dir.join("cdp.mjs"))
        .args([&format!("http://{addr}"), dir.to_string_lossy().as_ref(), &port.to_string()])
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .spawn()
        .expect("node");
    // mark where each gesture ended in the input log
    let stdout = child.stdout.take().unwrap();
    let marks = Arc::new(Mutex::new(Vec::<(String, usize)>::new()));
    let (m2, l2) = (marks.clone(), log.clone());
    let reader = std::thread::spawn(move || {
        use std::io::{BufRead, BufReader};
        let mut result = String::new();
        for l in BufReader::new(stdout).lines().map_while(Result::ok) {
            if let Some(r) = l.strip_prefix("RESULT ") {
                result = r.to_string();
            } else {
                let n = l2.lock().unwrap().len();
                m2.lock().unwrap().push((l, n));
            }
        }
        result
    });
    let result = reader.join().unwrap();
    let _ = child.wait();
    let v: serde_json::Value = serde_json::from_str(&result).unwrap_or_else(|e| panic!("no result ({e}): {result}"));
    let _ = std::fs::remove_dir_all(&dir);
    assert_eq!(v["errors"], serde_json::json!([]), "{v}");
    assert_eq!(v["logs"], serde_json::json!([]), "page exceptions: {v}");
    let near = |c: &serde_json::Value, want: [i64; 3]| (0..3).all(|i| (c[i].as_i64().unwrap() - want[i]).abs() <= 12);
    assert_eq!(v["workerMode"], true, "{v}");
    assert!(near(&v["workerColours"]["left"], [220, 20, 20]) && near(&v["workerColours"]["right"], [20, 20, 220]), "worker: {v}");
    assert_eq!(v["mainWorker"], false, "{v}");
    assert!(near(&v["mainColours"]["left"], [220, 20, 20]) && near(&v["mainColours"]["right"], [20, 20, 220]), "main thread (screenshot): {v}");
    let px = &v["mainPixels"];
    assert!(near(&px["left"], [220, 20, 20]) && near(&px["right"], [20, 20, 220]), "main thread (canvas): {v}");
    assert_eq!(v["statsBefore"], true, "{v}");
    assert!(v["statsText"].as_str().unwrap().len() > 5, "{v}");
    let log = log.lock().unwrap().clone();
    let marks = marks.lock().unwrap().clone();
    let at = |name: &str| marks.iter().find(|(l, _)| l == name).map(|(_, n)| *n).unwrap_or_else(|| panic!("no mark {name}: {marks:?}"));
    let between = |a: usize, b: usize| log[a..b].iter().filter(|l| !l.starts_with("Speech") && !l.starts_with("Size")).cloned().collect::<Vec<_>>();
    let first_size = log.iter().position(|l| l.starts_with("Size")).unwrap() + 1;
    // a tap: move, then the left button down and up at the middle of the picture (1000 x 557 here)
    let tap = between(first_size, at("TAP_DONE"));
    assert!(tap.iter().any(|l| l.starts_with("Event(MouseDown(")) && tap.iter().any(|l| l.starts_with("Event(MouseUp(")), "a tap is a click: {tap:?}");
    assert!(tap.iter().any(|l| l.contains("MouseDown(500, 278)") || l.contains("MouseDown(500, 279)")), "at the middle of the picture: {tap:?}");
    assert!(!tap.iter().any(|l| l.contains("MouseRight")), "{tap:?}");
    let long = between(at("TAP_DONE"), at("LONG_DONE"));
    assert!(long.iter().any(|l| l.starts_with("Event(MouseRight(400,")), "a long press is a right click 100 px left of the middle: {long:?}");
    assert!(!long.iter().any(|l| l.starts_with("Event(MouseDown(")), "{long:?}");
    let scroll = between(at("LONG_DONE"), at("SCROLL_DONE"));
    assert!(scroll.iter().filter(|l| *l == "Event(Scroll(1))").count() >= 3, "a two-finger drag down scrolls: {scroll:?}");
    assert!(v["zoomed"].as_f64().unwrap() > 1.5, "a pinch zooms: {v}");
    assert!(v["transform"].as_str().unwrap().contains("scale("), "{v}");
    assert_eq!(v["zoomAfterReset"].as_f64().unwrap(), 1.0, "{v}");
    assert_eq!(v["kbdFocus"], "ime", "{v}");
    // the server answers a quality choice in its statistics message, which the page shows
    assert!(v["statsText"].as_str().unwrap().contains("Balanced (auto)"), "{v}");
    assert!(v["statsLow"].as_str().unwrap().contains("Low bandwidth") && !v["statsLow"].as_str().unwrap().contains("(auto)"), "the chosen level: {v}");
    assert!(v["statsSaver"].as_str().unwrap().to_lowercase().contains("saver"), "the data saver: {v}");
    let key = between(at("SCROLL_DONE"), at("KEY_DONE"));
    assert!(key.iter().any(|l| l == "Event(Key(Char('q')))"), "{key:?}");
}
