// #[cfg(test)] -- this whole file is test code (it is included under #[cfg(test)] from flows/mod.rs); the marker keeps the BOUNDARY check from counting it.
//! Live-model checks of FLOWS (all `#[ignore = "live model"]`; see `crate::live_tests` for the environment): the open flow and its failure
//! reach a real model (F-FLOW-27), a real model proposes the repair of a failed step and the harness classifies and applies it
//! (F-FLOW-06), Autopilot decides what to do with that proposal (F-FLOW-34), and a real vision model is shown a camera picture
//! (F-FLOW-29). A local test server stands in for "the site"; the model is the real one.

use std::io::{Read, Write};
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use iyakki_flow::{AssistMode, AssistPolicy, HealMode, RunStatus, StartRequest};
use iyakki_sec::policy::{Policy, Role};
use serde_json::{json, Value};

use super::*;
use crate::live_tests::{attempts, head, model_name, Rec};

/// A little web server on loopback: /slow answers after 3 s, /new is fine, /old says it moved to /new, the rest is 404.
fn serve() -> String {
    let l = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = l.local_addr().unwrap().port();
    std::thread::spawn(move || {
        for s in l.incoming().flatten() {
            std::thread::spawn(move || {
                let mut s = s;
                let mut buf = [0u8; 2048];
                let n = s.read(&mut buf).unwrap_or(0);
                let first = String::from_utf8_lossy(&buf[..n]).lines().next().unwrap_or("").to_string();
                let path = first.split_whitespace().nth(1).unwrap_or("/").to_string();
                let (code, body) = match path.as_str() {
                    "/slow" => {
                        std::thread::sleep(Duration::from_secs(3));
                        (200, r#"{"ok":true}"#)
                    }
                    "/new" => (200, r#"{"ok":true,"status":"healthy"}"#),
                    "/old" => (404, r#"{"error":"this address moved","new_location":"/new"}"#),
                    _ => (404, r#"{"error":"not found"}"#),
                };
                let _ = write!(s, "HTTP/1.1 {code} X\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len());
            });
        }
    });
    format!("http://127.0.0.1:{port}")
}

struct W {
    env: Arc<FlowEnv>,
    dir: PathBuf,
    audit: Arc<std::sync::Mutex<Vec<String>>>,
}

impl Drop for W {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

fn world(tag: &str, rec: &Arc<Rec>) -> W {
    let dir = std::env::temp_dir().join(format!("iyakki-live-flow-{}-{tag}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let up: Arc<dyn iyakki_proxy::Upstream> = Arc::new(iyakki_proxy::https::UreqUpstream::with_timeout(Duration::from_secs(20)));
    let svc = Services::open(&dir.join("svc"), up.clone(), up).unwrap();
    let user = format!("livetester{}", tag.replace(|c: char| !c.is_ascii_alphanumeric(), ""));
    let mut p: Policy = Role::User.template();
    p.net_hosts = vec!["*".into()];
    let env = Arc::new(FlowEnv::build_in(&svc, &user, &p, false, &dir.join("home")));
    *env.host.backend.lock().unwrap() = Some(rec.clone());
    let audit = Arc::new(std::sync::Mutex::new(Vec::new()));
    let a2 = audit.clone();
    *env.host.audit_sink.lock().unwrap() = Some(Arc::new(move |_, action, detail| a2.lock().unwrap().push(format!("{action} {detail}"))));
    W { env, dir, audit }
}

fn publish(w: &W, f: iyakki_flow::Flow) -> String {
    let id = f.id.clone();
    w.env.create_draft(f, "user:t").unwrap();
    w.env.store.publish(&id, &iyakki_flow::store::PublishReq { by: "user:t", label: "", changelog: "v1", approved_by: Some("user:t") }).unwrap();
    id
}

// ------------------------------------------------------------------------------------------------ F-FLOW-27

#[test]
#[ignore = "live model"]
fn f_flow_27_a_real_model_given_the_open_flow_and_its_failure_explains_it_without_the_input_values() {
    attempts("F-FLOW-27", "the flows context (open flow, newest failed run, redacted) reaches a real model, which names the failing step and the cause and never sees or repeats the input value", |rec, note| {
        let w = world("ctx", rec);
        let token = format!("{}{}", "ghp_", "Z9y8X7w6V5u4T3s2R1q0P9o8N7m6L5k4J3i2");
        let spec = json!({"name": "Nightly check", "inputs": [{"name": "api", "type": "text", "required": true}], "nodes": [{"id": "call", "kind": "http", "name": "Call the site", "config": {"url": "http://127.0.0.1:1/x"}, "retry": {"attempts": 2}}], "edges": [{"from": "start", "to": "call"}], "triggers": [{"kind": "interval", "every_s": 3600}]});
        let flow = publish(&w, build_flow(&spec).unwrap());
        let mut req = StartRequest::new(&flow).trigger("manual", "me");
        req.inputs = json!({"api": token});
        let run = w.env.engine.start_run(req).unwrap().run_id;
        w.env.engine.wait_for(&run, Duration::from_secs(30), |r| r.status.is_terminal()).unwrap();
        let ctx = context_text(&w.env, &Open { flow: Some(flow.clone()), tab: "design".into(), ..Default::default() });
        note.say(format!("context: {} chars, names the failed run: {}", ctx.chars().count(), ctx.contains("NEWEST FAILED RUN")));
        if ctx.contains(&token) {
            return Err("the input value is in the context".into());
        }
        let sys = format!("{}\n{ctx}\nAnswer from the facts above. Be short.", crate::toolblock::COMPACT_PROMPT);
        let a = crate::chat::ChatBackend::complete(rec.as_ref(), &[crate::chat::Message::system(sys), crate::chat::Message::user("Why did my flow fail, and what should I check?")])?;
        note.say(format!("answer: {}", head(&a, 400)));
        let l = a.to_lowercase();
        if a.contains(&token) {
            return Err("the answer repeats the input value".into());
        }
        let cause = l.contains("refused") || l.contains("connect") || l.contains("reach");
        let place = l.contains("call") || l.contains("127.0.0.1");
        note.say(format!("names the step by name: {}; names the address: {}; names the cause: {cause}", l.contains("call the site") || l.contains("`call`"), l.contains("127.0.0.1")));
        if !(cause && place) {
            return Err("the answer does not name the cause and where it happened (the step or the address)".into());
        }
        Ok(())
    });
}

// ------------------------------------------------------------------------------------------------ F-FLOW-06

#[test]
#[ignore = "live model"]
fn f_flow_06_a_real_model_proposes_a_repair_the_harness_classifies_it_and_a_simple_one_is_applied_and_verified() {
    attempts("F-FLOW-06", "after the last attempt a real model proposes a structured patch for a step that times out; the harness classifies it; in auto-fix mode a simple one is applied to the draft, tried and the run goes on; the published version is untouched", |rec, note| {
        let base = serve();
        let w = world("heal", rec);
        let spec = json!({"name": "Slow check", "nodes": [{"id": "call", "kind": "http", "name": "Call the site", "timeout_ms": 800, "config": {"url": format!("{base}/slow")}}], "edges": [{"from": "start", "to": "call"}]});
        let mut f = build_flow(&spec).unwrap();
        f.defaults.heal.mode = HealMode::AutoFixSimple;
        f.defaults.heal.max_per_run = 2;
        let id = publish(&w, f);
        let o = w.env.engine.start_run(StartRequest::new(&id).trigger("manual", "me")).unwrap();
        let r = w.env.engine.wait_for(&o.run_id, Duration::from_secs(150), |r| r.status.is_terminal() || r.status == RunStatus::Blocked).unwrap();
        note.say(format!("run status: {:?}; heal cards: {}", r.status, r.heal.len()));
        for c in &r.heal {
            note.say(format!("card {}: state {:?}; diagnosis: {}; simple: {:?}; decided by `{}`; note: {}", c.id, c.state, head(&c.diagnosis, 160), c.class.as_ref().map(|k| k.is_simple()), c.decided_by, head(&c.note, 120)));
        }
        let Some(card) = r.heal.first() else { return Err("no repair card: the model was never asked or its answer was unusable".into()) };
        let patch = card.patch.as_ref().ok_or_else(|| format!("no usable patch in the model's answer: {}", head(&card.diagnosis, 200)))?;
        note.say(format!("patch: node {} ops {}", patch.node, head(&serde_json::to_string(&patch.ops).unwrap_or_default(), 200)));
        let class = card.class.as_ref().ok_or("the harness did not classify the patch")?;
        let audit = w.audit.lock().unwrap().clone();
        if !audit.iter().any(|l| l.contains("flow.heal.proposed")) {
            return Err(format!("no flow.heal.proposed audit line in {audit:?}"));
        }
        if w.env.store.published(&id).unwrap().node("call").unwrap().timeout_ms != Some(800) {
            return Err("the published version was changed".into());
        }
        if class.is_simple() {
            if r.status != RunStatus::Succeeded {
                return Err(format!("a simple fix was classified but the run is {:?}", r.status));
            }
            if card.decided_by != "agent:assistant" {
                return Err(format!("the simple fix was decided by `{}`", card.decided_by));
            }
            if w.env.store.get_draft(&id).unwrap().node("call").unwrap().timeout_ms.is_none_or(|t| t <= 800) {
                return Err("the draft's time limit was not raised".into());
            }
        } else {
            note.say("the model's patch was not a simple one: the harness asked instead of applying (also correct behaviour)");
        }
        Ok(())
    });
}

// ------------------------------------------------------------------------------------------------ F-FLOW-34

#[test]
#[ignore = "live model"]
fn f_flow_34_autopilot_with_a_real_model_publishes_a_guarded_fix_or_asks_with_reasons() {
    attempts("F-FLOW-34", "a failed run (a moved address) gets a real model's suggestion; the guards and a trial decide: an auto_publish autopilot publishes it as agent:assist, or asks with reasons; nothing is published unguarded", |rec, note| {
        let base = serve();
        let w = world("ap", rec);
        let spec = json!({"name": "Watch", "nodes": [{"id": "call", "kind": "http", "name": "Call the site", "config": {"url": format!("{base}/old")}}], "edges": [{"from": "start", "to": "call"}]});
        let mut f = build_flow(&spec).unwrap();
        f.defaults.assist = AssistPolicy { mode: AssistMode::Autopilot, auto_publish: true, ..Default::default() };
        let id = publish(&w, f);
        let o = w.env.engine.start_run(StartRequest::new(&id).trigger("manual", "me")).unwrap();
        let r = w.env.engine.wait_for(&o.run_id, Duration::from_secs(60), |r| r.status.is_terminal() || r.status == RunStatus::Blocked).unwrap();
        note.say(format!("run status: {:?}", r.status));
        if r.status == RunStatus::Succeeded {
            return Err("the run succeeded: the 404 did not fail the step, so the fix path was not exercised".into());
        }
        let lines = w.env.assist_tick();
        note.say(format!("assist lines: {:?}", lines.iter().map(|l| head(l, 160)).collect::<Vec<_>>()));
        let list = w.env.open_suggestions(&id);
        let published = w.env.store.published(&id).unwrap();
        let v = published.meta.version;
        let sugg = w.env.assist_store();
        if v > 1 {
            // published by itself
            let s = sugg.get(&id, "s1").ok_or("published, but no suggestion record")?;
            note.say(format!("autopilot published version {v}; by `{}`; guards: {:?}; trial: {:?}", s.by, s.guards.iter().map(|g| (g.0.clone(), g.1)).collect::<Vec<_>>(), s.trial));
            if s.guards.iter().any(|g| !g.1) {
                return Err("published although a guard failed".into());
            }
            let url = published.node("call").unwrap().config["url"].as_str().unwrap_or("").to_string();
            note.say(format!("the published address now ends with: {}", url.rsplit('/').next().unwrap_or("")));
            if !url.starts_with(&base) {
                return Err(format!("the fix moved the call to another host: {url}"));
            }
        } else {
            let s = list.first().ok_or("neither a published fix nor an open suggestion: the model's proposal was unusable")?;
            note.say(format!("asked instead: asks=`{}`; trial {:?}; guards failed: {:?}", head(&s.asks, 200), s.trial, s.guards.iter().filter(|g| !g.1).map(|g| g.0.clone()).collect::<Vec<_>>()));
            if s.asks.trim().is_empty() {
                return Err("an open suggestion without the reasons it asks".into());
            }
        }
        Ok(())
    });
}

// ------------------------------------------------------------------------------------------------ F-FLOW-29

/// A solid red 32x32 PNG (stored blocks, no compression), made here so nothing is shipped.
fn red_png() -> Vec<u8> {
    fn crc(data: &[u8]) -> u32 {
        let mut c = 0xFFFF_FFFFu32;
        for b in data {
            c ^= *b as u32;
            for _ in 0..8 {
                c = if c & 1 != 0 { 0xEDB8_8320 ^ (c >> 1) } else { c >> 1 };
            }
        }
        !c
    }
    fn chunk(kind: &[u8], data: &[u8]) -> Vec<u8> {
        let mut v = (data.len() as u32).to_be_bytes().to_vec();
        let mut body = kind.to_vec();
        body.extend_from_slice(data);
        v.extend_from_slice(&body);
        v.extend_from_slice(&crc(&body).to_be_bytes());
        v
    }
    let (w, h) = (32u32, 32u32);
    let mut raw = Vec::new();
    for _ in 0..h {
        raw.push(0u8);
        for _ in 0..w {
            raw.extend_from_slice(&[220, 20, 20]);
        }
    }
    let mut z = vec![0x78, 0x01, 0x01];
    z.extend_from_slice(&(raw.len() as u16).to_le_bytes());
    z.extend_from_slice(&(!(raw.len() as u16)).to_le_bytes());
    z.extend_from_slice(&raw);
    let (mut a, mut b) = (1u32, 0u32);
    for x in &raw {
        a = (a + *x as u32) % 65521;
        b = (b + a) % 65521;
    }
    z.extend_from_slice(&((b << 16) | a).to_be_bytes());
    let mut ihdr = w.to_be_bytes().to_vec();
    ihdr.extend_from_slice(&h.to_be_bytes());
    ihdr.extend_from_slice(&[8, 2, 0, 0, 0]);
    let mut png = vec![0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A];
    png.extend(chunk(b"IHDR", &ihdr));
    png.extend(chunk(b"IDAT", &z));
    png.extend(chunk(b"IEND", &[]));
    png
}

#[test]
#[ignore = "live model"]
fn f_flow_29_a_camera_picture_is_shown_to_a_real_vision_model_and_deleted_after_the_run() {
    use crate::camtools::{CameraBackend, CountryInfo, PassportOut};
    struct Cam {
        taken: std::sync::Mutex<Vec<PathBuf>>,
    }
    impl CameraBackend for Cam {
        fn cameras(&self) -> Vec<String> {
            vec!["video0: Test camera".into()]
        }
        fn photo(&self, dir: &Path) -> Result<PathBuf, String> {
            let p = dir.join("photo-1.png");
            std::fs::write(&p, red_png()).map_err(|e| e.to_string())?;
            self.taken.lock().unwrap().push(p.clone());
            Ok(p)
        }
        fn record_start(&self, _: &Path, _: u32) -> Result<PathBuf, String> {
            Err("no".into())
        }
        fn record_stop(&self) -> Result<(Vec<PathBuf>, u64), String> {
            Err("no".into())
        }
        fn recording(&self) -> Option<(PathBuf, f64)> {
            None
        }
        fn passport(&self, _: &Path, _: &str, _: &str) -> Result<PassportOut, String> {
            Err("no".into())
        }
        fn countries(&self) -> Vec<CountryInfo> {
            vec![]
        }
    }
    attempts("F-FLOW-29", "the camera node runs only when allowed, saves the picture in the run's private folder, a real vision model (Gemma-4-E2B) is shown it and answers in the flow's JSON, and the picture is deleted when the run ends. The camera is a fake that returns a solid red picture; no real camera was used", |rec, note| {
        let model = std::env::var("IYAKKI_LIVE_VISION_MODEL").unwrap_or_else(|_| "Gemma-4-E2B-it-GGUF".into());
        let _ = (rec, model_name());
        let vrec = Rec::new(&model);
        let w = world("cam", &vrec);
        let cam = Arc::new(Cam { taken: Default::default() });
        w.env.svc.camera.set_backend(Some(cam.clone()));
        crate::providers::note_vision_models(&[model.clone()]);
        let mut pc = crate::providers::ProviderCfg::new("p", crate::providers::Kind::Lemonade);
        pc.base_url = Some(std::env::var("IYAKKI_LIVE_LEMONADE_URL").map_err(|_| "set IYAKKI_LIVE_LEMONADE_URL")?);
        pc.key_ref = Some(format!("file:{}", std::env::var("IYAKKI_LIVE_LEMONADE_KEY_FILE").map_err(|_| "set IYAKKI_LIVE_LEMONADE_KEY_FILE")?));
        w.env.svc.settings.lock().unwrap().upsert_provider(pc).unwrap();
        w.env.svc.settings.lock().unwrap().update(|s| s.chat = Some(crate::settings::ModelChoice { provider: "p".into(), model: model.clone() })).unwrap();
        let t = iyakki_flow::templates::get("cam-sleep-check").unwrap();
        let mut f = iyakki_flow::templates::instantiate(&t, "cam-live", "");
        f.defaults.camera.allowed = true;
        let id = publish(&w, f);
        let o = w.env.engine.start_run(StartRequest::new(&id).trigger("manual", "test")).unwrap();
        let r = w.env.engine.wait_for(&o.run_id, Duration::from_secs(180), |r| r.status.is_terminal() || r.status == RunStatus::Blocked).unwrap();
        note.say(format!("run {:?}; node states: {:?}", r.status, r.nodes.iter().map(|(k, n)| (k.clone(), head(&n.error, 100))).collect::<Vec<_>>()));
        note.say(format!("vision model requests: {} ; log: {}", vrec.requests(), head(&serde_json::to_string(&*vrec.log.lock().unwrap()).unwrap_or_default(), 500)));
        if vrec.requests() == 0 {
            return Err("the vision model was never asked".into());
        }
        let taken = cam.taken.lock().unwrap().clone();
        if taken.len() != 1 || !taken[0].display().to_string().contains("/camera/") {
            return Err(format!("the picture was not taken into the private folder: {taken:?}"));
        }
        if taken[0].exists() {
            return Err("the picture is still there after the run".into());
        }
        let ask = r.nodes.get("ask").ok_or("no `ask` step")?;
        note.say(format!("the model's answer, parsed: {}", head(&ask.output.to_string(), 200)));
        if ask.output.get("present").is_none() {
            return Err(format!("the answer was not in the flow's JSON shape: {}", head(&ask.output.to_string(), 200)));
        }
        if r.status != RunStatus::Succeeded {
            return Err(format!("the run is {:?}: {}", r.status, head(&r.error, 200)));
        }
        Ok(())
    });
    let _ = Value::Null;
}
