//! The served web page (`page.rs` + `media.js`) in a real headless Chrome with Chrome's fake camera and microphone, against the real
//! `Server` and the real `BrowserMedia` (fake screen only): Start sound, the microphone and the camera switched by trusted clicks (CDP mouse
//! events), a script's click refused, the "Play sound on" choice, a program's ask answered only by a real click, a denying policy shown in the
//! page and nothing stored, and everything off when the page goes away.
//!
//! Ignored by default because it needs `google-chrome` and `node` (22 or newer). No real speaker, microphone or camera, no person.
//! Run: cargo test -p iyakki-remote --test web_page_devices_chrome -- --ignored --nocapture

use iyakki_agent::browsermedia::{BrowserMedia, Kind, Policy, Requested, SinkMode};
use iyakki_remote::input::Input;
use iyakki_remote::screen::{Frame, Gate, LoginError, Screen};
use iyakki_remote::server::{Config, Server};
use std::io::{BufRead, BufReader};
use std::process::{Command, Stdio};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

struct Fake {
    media: Arc<BrowserMedia>,
}

impl Screen for Fake {
    fn user(&self) -> String {
        "ann".into()
    }
    fn input(&mut self, _: Input) {}
    fn viewing(&mut self, _: bool) {}
    fn step(&mut self) -> Option<Frame> {
        std::thread::sleep(Duration::from_millis(5));
        None
    }
    fn take_copy(&mut self) -> Option<String> {
        None
    }
    fn media(&self) -> Option<Arc<BrowserMedia>> {
        Some(self.media.clone())
    }
    fn finished(&self) -> bool {
        false
    }
}

struct G(Arc<BrowserMedia>);
impl Gate for G {
    fn login(&self, user: &str, password: &str) -> Result<Box<dyn Screen>, LoginError> {
        if user == "ann" && password == "right" {
            Ok(Box::new(Fake { media: self.0.clone() }))
        } else {
            Err(LoginError::Invalid)
        }
    }
}

const DRIVER: &str = r##"const [BASE, SCENARIO, DIR, PORT] = process.argv.slice(2);
import { spawn } from "node:child_process";
const chrome = spawn("google-chrome", ["--headless=new", "--no-sandbox", "--disable-gpu", "--window-size=1100,900", "--autoplay-policy=no-user-gesture-required", "--use-fake-ui-for-media-stream", "--use-fake-device-for-media-stream", "--remote-debugging-port=" + PORT, "--user-data-dir=" + DIR + "/prof", "about:blank"], { stdio: "ignore" });
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
const result = { requests: [], logs: [], errors: [] };
const done = (code) => { try { chrome.kill(); } catch {} console.log("RESULT " + JSON.stringify(result)); setTimeout(() => process.exit(code), 100); };
process.on("uncaughtException", (e) => { result.errors.push("driver: " + e.message); done(1); });
let tabs; for (let i = 0; i < 60; i++) { try { tabs = await (await fetch("http://127.0.0.1:" + PORT + "/json")).json(); if (tabs.length) break; } catch (e) {} await sleep(200); }
const ws = new WebSocket(tabs.find((t) => t.type === "page").webSocketDebuggerUrl);
await new Promise((r) => (ws.onopen = r));
let id = 0; const pending = new Map();
ws.onmessage = (m) => {
  const j = JSON.parse(m.data);
  if (j.id && pending.has(j.id)) return pending.get(j.id)(j);
  if (j.method === "Network.requestWillBeSent") result.requests.push(j.params.request.url);
  if (j.method === "Log.entryAdded") result.logs.push(j.params.entry.level + ": " + j.params.entry.text);
  if (j.method === "Runtime.consoleAPICalled") result.logs.push("console." + j.params.type + ": " + j.params.args.map((a) => a.value ?? a.description ?? "").join(" "));
  if (j.method === "Runtime.exceptionThrown") result.logs.push("exception: " + JSON.stringify(j.params.exceptionDetails.exception?.description ?? j.params.exceptionDetails.text));
};
const call = (method, params = {}) => new Promise((r) => { const i = ++id; pending.set(i, r); ws.send(JSON.stringify({ id: i, method, params })); });
const ev = async (expr) => { const r = await call("Runtime.evaluate", { expression: expr, returnByValue: true, awaitPromise: true }); if (r.result.exceptionDetails) throw new Error("eval: " + JSON.stringify(r.result.exceptionDetails.exception?.description)); return r.result.result.value; };
const waitFor = async (expr, what, ms = 20000) => { const t0 = Date.now(); while (Date.now() - t0 < ms) { try { if (await ev(expr)) return; } catch (e) {} await sleep(100); } throw new Error("timed out waiting for " + what + "; note=" + (await ev("document.getElementById('dnote').textContent").catch(() => "?"))); };
// a real click: the browser's own mouse events, so isTrusted is true
const real = async (eid) => {
  const [x, y] = await ev("(() => { const r = document.getElementById('" + eid + "').getBoundingClientRect(); return [r.x + r.width / 2, r.y + r.height / 2]; })()");
  await call("Input.dispatchMouseEvent", { type: "mouseMoved", x, y });
  await call("Input.dispatchMouseEvent", { type: "mousePressed", x, y, button: "left", clickCount: 1 });
  await call("Input.dispatchMouseEvent", { type: "mouseReleased", x, y, button: "left", clickCount: 1 });
};
const fake = (eid) => ev("document.getElementById('" + eid + "').click(); true"); // a script's click: isTrusted false
const text = (eid) => ev("document.getElementById('" + eid + "').textContent");
try {
  await call("Network.enable"); await call("Log.enable"); await call("Runtime.enable"); await call("Page.enable");
  await call("Page.navigate", { url: BASE + "/" });
  await waitFor("!!document.getElementById('user') && typeof media !== 'undefined' && document.readyState === 'complete'", "the sign-in form and the page's script");
  await sleep(300);
  await ev("document.getElementById('user').value = 'ann'; document.getElementById('pass').value = 'right'; document.getElementById('form').requestSubmit(); true");
  await waitFor("!document.getElementById('gear').hidden", "the gear (signed in)");
  await real("gear");
  await waitFor("!document.getElementById('panel').hidden", "the devices panel");
  await waitFor("document.getElementById('dmic').textContent.length > 0 && !/needs https/.test(document.getElementById('dmic').textContent)", "the page to know the policy");
  result.micLabel = await text("dmic");
  result.camLabel = await text("dcam");
  result.micDisabled = await ev("document.getElementById('dmic').disabled");
  if (SCENARIO === "deny") {
    // a denying policy: the buttons say so, a click does nothing and the server keeps nothing
    await fake("dmic"); await fake("dcam");
    await sleep(800);
    result.afterMicLabel = await text("dmic");
    console.log("DENY_DONE");
    await sleep(500);
    done(0);
  } else {
    // 1. sound: a script's click does not start it, a real click does
    await fake("dsound"); await sleep(600);
    result.soundAfterFake = await text("dsound");
    console.log("SOUND_FAKE_DONE");
    await real("dsound");
    await waitFor("document.getElementById('dsound').textContent === 'Stop sound'", "sound to start");
    result.soundCtx = await ev("media.player.ctx ? media.player.ctx.state : 'none'");
    result.volumeShown = await ev("!document.getElementById('dvolrow').hidden");
    console.log("SOUND_ON");
    await sleep(1200);
    // the route: The Mac only sends nothing to the page; back to the browser sends again
    await ev("(() => { const s = document.getElementById('dsink'); s.value = 'mac'; s.dispatchEvent(new Event('change')); return true; })()");
    console.log("ROUTE_MAC"); await sleep(800);
    await ev("(() => { const s = document.getElementById('dsink'); s.value = 'browser'; s.dispatchEvent(new Event('change')); return true; })()");
    console.log("ROUTE_BROWSER"); await sleep(800);
    // 2. microphone: a script's click cannot switch it on
    await fake("dmic"); await sleep(1000);
    result.micAfterFake = await text("dmic");
    console.log("MIC_FAKE_DONE");
    await real("dmic");
    await waitFor("document.getElementById('rec').textContent.includes('microphone')", "the recording indicator for the microphone");
    result.rec1 = await text("rec");
    console.log("MIC_ON");
    await sleep(2500);
    // 3. camera
    await real("dcam");
    await waitFor("document.getElementById('rec').textContent.includes('camera')", "the recording indicator for the camera");
    console.log("CAM_ON");
    await sleep(2500);
    await real("dcam");
    await waitFor("!document.getElementById('rec').textContent.includes('camera')", "the camera to stop");
    console.log("CAM_OFF");
    await real("dmic");
    await waitFor("document.getElementById('rec').hidden", "the microphone to stop");
    console.log("MIC_OFF");
    // 4. a program asks for the microphone: the page offers a button, a script's click does nothing, a real click switches it on
    console.log("READY_FOR_ASK");
    await waitFor("!document.getElementById('askdev').hidden", "the ask button");
    result.askText = await text("askdev");
    await fake("askdev"); await sleep(800);
    console.log("ASK_FAKE_DONE");
    // the page drops the ask after a script's click (it said so in its note); the program asks again, and now a real click answers
    await sleep(1000);
    await waitFor("!document.getElementById('askdev').hidden", "the second ask");
    await real("askdev");
    await waitFor("document.getElementById('rec').textContent.includes('microphone')", "the microphone after the real click on the ask");
    console.log("ASK_ANSWERED");
    await real("dsound");
    await waitFor("document.getElementById('dsound').textContent === 'Start sound'", "sound to stop");
    console.log("SOUND_OFF");
    await sleep(500);
    // leave the page with the microphone still on: the server must switch it off
    await call("Page.navigate", { url: "about:blank" });
    await sleep(1000);
    done(0);
  }
} catch (e) { result.errors.push(String(e.message || e)); done(1); }
"##;

fn run(policy: Policy, scenario: &str, drive: impl Fn(&Arc<BrowserMedia>, &Arc<Mutex<Vec<String>>>, &str)) -> serde_json::Value {
    let audit = Arc::new(Mutex::new(Vec::<String>::new()));
    let a2 = audit.clone();
    let media = BrowserMedia::new(policy, Arc::new(move |a, d| a2.lock().unwrap().push(format!("{a} {d}"))), None);
    let srv = Server::start(Config { addr: "127.0.0.1:0".into(), tls: None }, Arc::new(G(media.clone()))).unwrap();
    let addr = srv.local_addr();
    let dir = std::env::temp_dir().join(format!("iyakki-webdev-chrome-{}-{scenario}", std::process::id()));
    let driver = std::env::var("WEBDEV_DRIVER_FILE").ok().and_then(|f| std::fs::read_to_string(f).ok()).unwrap_or_else(|| DRIVER.to_string());
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("cdp.mjs"), driver).unwrap();
    let port = 9300 + (std::process::id() % 100) as u16 * 2 + if scenario == "deny" { 1 } else { 0 };
    let mut child = Command::new("node")
        .arg(dir.join("cdp.mjs"))
        .args([&format!("http://{addr}"), scenario, dir.to_string_lossy().as_ref(), &port.to_string()])
        .stdout(Stdio::piped())
        .stderr(std::fs::File::create(dir.join("node-stderr.txt")).unwrap())
        .spawn()
        .expect("node");
    let stdout = child.stdout.take().unwrap();
    let (tx, rx) = std::sync::mpsc::channel::<String>();
    std::thread::spawn(move || {
        for l in BufReader::new(stdout).lines().map_while(Result::ok) {
            let _ = tx.send(l);
        }
    });
    let t0 = Instant::now();
    let mut result = None;
    let mut seen: Vec<String> = Vec::new();
    while result.is_none() && t0.elapsed() < Duration::from_secs(90) {
        match rx.recv_timeout(Duration::from_millis(10)) {
            Ok(l) if l.starts_with("RESULT ") => result = Some(l[7..].to_string()),
            Ok(l) => {
                drive(&media, &audit, &l);
                seen.push(l);
            }
            _ => drive(&media, &audit, ""),
        }
        // what the mixer would give the tee: a tone, every 20 ms, only taken while a page wants it
        media.push_audio(48_000, 2, &(0..1920).map(|i| (((i / 2) % 100) as i16 - 50) * 100).collect::<Vec<i16>>());
    }
    let _ = child.kill();
    let _ = child.wait();
    let err = std::fs::read_to_string(dir.join("node-stderr.txt")).unwrap_or_default();
    let _ = Command::new("pkill").args(["-f", &format!("user-data-dir={}", dir.display())]).status();
    let text = result.unwrap_or_else(|| panic!("no result from the browser driver; stdout {seen:?}; stderr: {err}"));
    eprintln!("{text}");
    let v: serde_json::Value = serde_json::from_str(&text).unwrap();
    assert_eq!(v["errors"], serde_json::json!([]), "{text}");
    let mut out = v;
    out["seen"] = serde_json::json!(seen);
    out["audit"] = serde_json::json!(audit.lock().unwrap().clone());
    out["finalMic"] = serde_json::json!(media.is_on(Kind::Mic));
    out["finalCamera"] = serde_json::json!(media.is_on(Kind::Camera));
    out["finalSound"] = serde_json::json!(media.sources().audio_out);
    out["finalCameraFrame"] = serde_json::json!(media.camera().is_some());
    let _ = std::fs::remove_dir_all(&dir);
    drop(srv);
    out
}

fn need_tools() {
    for tool in ["google-chrome", "node"] {
        if Command::new(tool).arg("--version").output().is_err() {
            panic!("{tool} is needed for this test");
        }
    }
}

#[test]
#[ignore]
fn the_served_page_starts_sound_microphone_and_camera_only_by_real_clicks_and_everything_is_off_when_it_goes_in_a_real_chrome() {
    need_tools();
    let mic_bytes = Mutex::new(0usize);
    let cam_frames = Mutex::new(0u64);
    let jpeg_ok = Mutex::new(true);
    let sound_chunks_checked = Mutex::new(Vec::<(String, bool)>::new());
    let v = run(Policy::allow_all(), "allow", |m, _audit, l| {
        // called for every line the driver prints; poll the server's state too
        *mic_bytes.lock().unwrap() += m.take_mic().len();
        if let Some(c) = m.camera() {
            if c.len() < 4 || c[0] != 0xFF || c[1] != 0xD8 {
                *jpeg_ok.lock().unwrap() = false;
            }
        }
        {
            let mut g = cam_frames.lock().unwrap();
            *g = (*g).max(m.camera_frames());
        }
        match l {
            "SOUND_FAKE_DONE" => assert!(!m.sources().audio_out, "a script's click started the sound"),
            "SOUND_ON" => {
                let t = Instant::now();
                while !m.sources().audio_out && t.elapsed() < Duration::from_secs(5) {
                    std::thread::sleep(Duration::from_millis(20));
                }
                assert!(m.sources().audio_out, "the real click did not reach the server");
            }
            "ROUTE_MAC" => {
                let t = Instant::now();
                while m.sink_mode() != SinkMode::Mac && t.elapsed() < Duration::from_secs(5) {
                    std::thread::sleep(Duration::from_millis(20));
                }
                assert_eq!(m.sink_mode(), SinkMode::Mac);
                sound_chunks_checked.lock().unwrap().push(("mac".into(), !m.wants_audio()));
            }
            "ROUTE_BROWSER" => {
                let t = Instant::now();
                while m.sink_mode() != SinkMode::Browser && t.elapsed() < Duration::from_secs(5) {
                    std::thread::sleep(Duration::from_millis(20));
                }
                assert_eq!(m.sink_mode(), SinkMode::Browser);
                sound_chunks_checked.lock().unwrap().push(("browser".into(), m.wants_audio()));
            }
            "MIC_FAKE_DONE" => assert!(!m.is_on(Kind::Mic), "a script's click switched the microphone on"),
            "MIC_ON" => {
                let t = Instant::now();
                while !m.is_on(Kind::Mic) && t.elapsed() < Duration::from_secs(5) {
                    std::thread::sleep(Duration::from_millis(20));
                }
                assert!(m.is_on(Kind::Mic), "the real click did not switch the microphone on");
            }
            "CAM_ON" => {
                let t = Instant::now();
                while !m.is_on(Kind::Camera) && t.elapsed() < Duration::from_secs(5) {
                    std::thread::sleep(Duration::from_millis(20));
                }
                assert!(m.is_on(Kind::Camera));
            }
            "CAM_OFF" => {
                let t = Instant::now();
                while m.is_on(Kind::Camera) && t.elapsed() < Duration::from_secs(5) {
                    std::thread::sleep(Duration::from_millis(20));
                }
                assert!(!m.is_on(Kind::Camera) && m.camera().is_none(), "the camera frame is kept after the camera went off");
            }
            "MIC_OFF" => {
                let t = Instant::now();
                while m.is_on(Kind::Mic) && t.elapsed() < Duration::from_secs(5) {
                    std::thread::sleep(Duration::from_millis(20));
                }
                assert!(!m.is_on(Kind::Mic));
                // a program (a tile) asks for the microphone: asking does not switch it on
                assert_eq!(m.request(Kind::Mic), Requested::Asked);
                assert!(!m.is_on(Kind::Mic));
            }
            "ASK_FAKE_DONE" => {
                assert!(!m.is_on(Kind::Mic), "a script's click answered the program's ask");
                assert_eq!(m.request(Kind::Mic), Requested::Asked);
            }
            "ASK_ANSWERED" => assert!(m.is_on(Kind::Mic), "the real click on the ask did not switch the microphone on"),
            _ => {}
        }
    });
    // page side
    assert!(v["micLabel"].as_str().unwrap().contains("off") && v["camLabel"].as_str().unwrap().contains("off"), "{v}");
    assert_eq!(v["soundAfterFake"], "Start sound", "a script's click started the sound in the page");
    assert_eq!(v["soundCtx"], "running", "{v}");
    assert_eq!(v["volumeShown"], true);
    assert!(v["micAfterFake"].as_str().unwrap().contains("off"), "{v}");
    assert!(v["rec1"].as_str().unwrap().contains("Recording: microphone"), "the red indicator: {v}");
    assert!(v["askText"].as_str().unwrap().contains("microphone"), "{v}");
    // server side: real data from Chrome's fake devices reached the session
    assert!(*mic_bytes.lock().unwrap() > 8_000, "microphone sound bytes taken: {}", mic_bytes.lock().unwrap());
    assert!(*cam_frames.lock().unwrap() >= 3, "camera frames: {}", cam_frames.lock().unwrap());
    assert!(*jpeg_ok.lock().unwrap(), "a camera frame was not a JPEG");
    let checks = sound_chunks_checked.lock().unwrap().clone();
    assert!(checks.iter().all(|(_, ok)| *ok) && checks.len() == 2, "the route was not honoured: {checks:?}");
    // leaving the page switches everything off and forgets the frame
    assert_eq!(v["finalMic"], false, "the microphone is still on after the page left");
    assert_eq!(v["finalCamera"], false);
    assert_eq!(v["finalSound"], false);
    assert_eq!(v["finalCameraFrame"], false);
    let audit: Vec<String> = serde_json::from_value(v["audit"].clone()).unwrap();
    for want in ["browser.media.on mic", "browser.media.on camera", "browser.media.off camera", "browser.media.off mic", "browser.media.route mac", "browser.media.route browser"] {
        assert!(audit.iter().any(|l| l == want), "{want} missing in {audit:?}");
    }
    assert!(audit.iter().all(|l| l.starts_with("browser.")), "kind only, no content: {audit:?}");
    // nothing left the machine; no console error
    for u in v["requests"].as_array().unwrap() {
        let u = u.as_str().unwrap();
        assert!(u.contains("127.0.0.1") || u.starts_with("about:") || u.starts_with("data:") || u.starts_with("blob:"), "a request to another address: {u}");
    }
    let logs = v["logs"].to_string();
    assert!(!logs.contains("Content Security Policy") && !logs.contains("Refused to") && !logs.contains("exception:"), "{logs}");
}

#[test]
#[ignore]
fn a_page_whose_policy_denies_the_devices_says_so_and_the_server_keeps_nothing_in_a_real_chrome() {
    need_tools();
    let v = run(Policy::deny_all(), "deny", |_, _, _| {});
    assert!(v["micLabel"].as_str().unwrap().contains("not allowed") && v["camLabel"].as_str().unwrap().contains("not allowed"), "{v}");
    assert_eq!(v["micDisabled"], true);
    assert!(v["afterMicLabel"].as_str().unwrap().contains("not allowed"), "{v}");
    assert_eq!(v["finalMic"], false);
    assert_eq!(v["finalCamera"], false);
    assert_eq!(v["finalCameraFrame"], false);
}
