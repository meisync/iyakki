// #[cfg(test)] -- this whole file is test code (it is included under #[cfg(test)] from flows/mod.rs); the marker keeps the BOUNDARY check from counting it.
//! Tests of the camera node and the sample flows that use it: allowed or not, attended or not, the picture's private folder and its
//! deletion, a model that cannot look at pictures, the vision model's JSON answer, the assistant and imports never switching the
//! permission on; and the data science samples (the toolkit check message, the report read back). A fake camera and a scripted model
//! stand in for the real ones: NO real camera and NO real Qwen-VL model are used here.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use iyakki_flow::{Flow, RunStatus, StartRequest};
use iyakki_sec::policy::{Policy, Role};
use serde_json::{json, Value};

use super::*;
use crate::camtools::{CameraBackend, CountryInfo, PassportOut};

struct Dead;
impl iyakki_proxy::Upstream for Dead {
    fn request(&self, _: &str, _: &str, _: &[(String, String)], _: &[u8]) -> Result<iyakki_proxy::UpstreamResponse, String> {
        Err("offline".into())
    }
}

/// A camera that writes a tiny JPEG and counts the pictures it was asked for.
#[derive(Default)]
struct FakeCam {
    taken: Mutex<Vec<PathBuf>>,
    none: bool,
}

impl CameraBackend for FakeCam {
    fn cameras(&self) -> Vec<String> {
        if self.none { vec![] } else { vec!["video0: Test camera".into()] }
    }
    fn photo(&self, dir: &Path) -> Result<PathBuf, String> {
        let p = dir.join(format!("photo-{}.jpg", self.taken.lock().unwrap().len() + 1));
        std::fs::write(&p, [0xFF, 0xD8, 0xFF, 0xD9]).map_err(|e| e.to_string())?;
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

/// A vision model with canned answers; it records what it was shown.
struct Scripted {
    answers: Mutex<Vec<String>>,
    seen: Mutex<Vec<crate::chat::Message>>,
}

impl Scripted {
    fn new(answers: &[&str]) -> Arc<Scripted> {
        Arc::new(Scripted { answers: Mutex::new(answers.iter().rev().map(|s| s.to_string()).collect()), seen: Mutex::new(vec![]) })
    }
}

impl crate::chat::ChatBackend for Scripted {
    fn complete(&self, h: &[crate::chat::Message]) -> Result<String, String> {
        self.seen.lock().unwrap().extend(h.iter().cloned());
        Ok(self.answers.lock().unwrap().pop().unwrap_or_else(|| "{}".into()))
    }
}

struct Cam {
    env: Arc<FlowEnv>,
    cam: Arc<FakeCam>,
    dir: PathBuf,
}

impl Drop for Cam {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

fn setup(tag: &str, cam: FakeCam, model: &str, kind: crate::providers::Kind) -> Cam {
    setup_at(tag, cam, model, kind, None)
}

fn setup_at(tag: &str, cam: FakeCam, model: &str, kind: crate::providers::Kind, base: Option<&str>) -> Cam {
    let dir = std::env::temp_dir().join(format!("iyakki-cam-{}-{tag}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let svc = Services::open(&dir.join("svc"), Arc::new(Dead), Arc::new(Dead)).unwrap();
    let user = format!("camtester{}", tag.replace(|c: char| !c.is_ascii_alphanumeric(), ""));
    let mut p: Policy = Role::User.template();
    p.net_hosts = vec!["*".into()];
    let env = Arc::new(FlowEnv::build_in(&svc, &user, &p, false, &dir.join("home")));
    let cam = Arc::new(cam);
    svc.camera.set_backend(Some(cam.clone()));
    let mut pc = crate::providers::ProviderCfg::new("p", kind);
    if let Some(b) = base {
        pc.base_url = Some(b.into());
    }
    if kind.needs_key() {
        pc.key_ref = Some("vault:p-key".into());
    }
    svc.settings.lock().unwrap().upsert_provider(pc).unwrap();
    svc.settings.lock().unwrap().update(|s| s.chat = Some(crate::settings::ModelChoice { provider: "p".into(), model: model.into() })).unwrap();
    Cam { env, cam, dir }
}

fn local(tag: &str) -> Cam {
    setup(tag, FakeCam::default(), "qwen2.5vl:7b", crate::providers::Kind::Ollama)
}

impl Cam {
    fn model(&self, answers: &[&str]) -> Arc<Scripted> {
        let m = Scripted::new(answers);
        *self.env.host.backend.lock().unwrap() = Some(m.clone());
        m
    }
    fn run(&self, f: Flow, trigger: &str) -> iyakki_flow::Run {
        let id = f.id.clone();
        self.env.create_draft(f, "user:t").unwrap();
        self.env.store.publish(&id, &iyakki_flow::store::PublishReq { by: "user:t", label: "", changelog: "x", approved_by: None }).unwrap();
        let o = self.env.engine.start_run(StartRequest::new(&id).trigger(trigger, "test")).unwrap();
        self.env.engine.wait_for(&o.run_id, Duration::from_secs(30), |r| r.status.is_terminal() || r.status == RunStatus::Blocked).unwrap()
    }
    fn sample(&self, id: &str, allowed: bool, unattended: bool) -> Flow {
        let t = iyakki_flow::templates::get(id).unwrap();
        let mut f = iyakki_flow::templates::instantiate(&t, &format!("{id}-t"), "");
        f.defaults.camera.allowed = allowed;
        f.defaults.camera.unattended = unattended;
        f
    }
    fn camera_files(&self) -> Vec<PathBuf> {
        fn walk(d: &Path, v: &mut Vec<PathBuf>) {
            for f in std::fs::read_dir(d).into_iter().flatten().flatten() {
                if f.path().is_dir() {
                    walk(&f.path(), v);
                } else {
                    v.push(f.path());
                }
            }
        }
        let mut v = vec![];
        walk(&self.env.data.join("camera"), &mut v);
        v
    }
}

const SLEEPING: &str = r#"{"present": true, "sleeping": true, "confidence": 0.9, "reason": "a person lies on a sofa with closed eyes"}"#;
const AWAKE: &str = r#"Here you go: {"present": true, "sleeping": false, "confidence": 0.8, "reason": "a person sits at a desk"}"#;
const EMPTY: &str = r#"{"present": false, "sleeping": null, "confidence": 0.95, "reason": "an empty room"}"#;

#[test]
fn a_flow_the_person_has_not_allowed_cannot_use_the_camera_and_no_picture_is_taken() {
    let c = local("notallowed");
    let m = c.model(&[SLEEPING]);
    let r = c.run(c.sample("cam-sleep-check", false, false), "manual");
    assert_ne!(r.status, RunStatus::Succeeded, "a failed step with nowhere to go blocks the run");
    // the agent is never asked to "fix" a permission: no question to the model, no attempt spent, no proposal, a card that says what to do
    assert!(m.seen.lock().unwrap().is_empty(), "the model was not asked anything");
    assert_eq!(r.heal_spent, 0);
    assert_eq!(r.heal.len(), 1);
    assert!(r.heal[0].patch.is_none() && r.heal[0].note == iyakki_flow::driver::CONSENT_NOTE, "{:?}", r.heal[0]);
    assert!(r.heal[0].diagnosis.contains("Camera allowed"), "{}", r.heal[0].diagnosis);
    assert!(r.nodes["shot"].error.contains("not allowed to use the camera"), "{}", r.nodes["shot"].error);
    assert!(c.cam.taken.lock().unwrap().is_empty(), "the camera was never asked");
    assert!(c.camera_files().is_empty());
}

/// A camera whose every call blocks, standing in for a device scan or a lock wait.
struct SlowCam(FakeCam);
impl CameraBackend for SlowCam {
    fn cameras(&self) -> Vec<String> {
        std::thread::sleep(Duration::from_secs(5));
        self.0.cameras()
    }
    fn photo(&self, dir: &Path) -> Result<PathBuf, String> {
        std::thread::sleep(Duration::from_secs(5));
        self.0.photo(dir)
    }
    fn record_start(&self, a: &Path, b: u32) -> Result<PathBuf, String> {
        self.0.record_start(a, b)
    }
    fn record_stop(&self) -> Result<(Vec<PathBuf>, u64), String> {
        self.0.record_stop()
    }
    fn recording(&self) -> Option<(PathBuf, f64)> {
        std::thread::sleep(Duration::from_secs(5));
        None
    }
    fn passport(&self, a: &Path, b: &str, c: &str) -> Result<PassportOut, String> {
        self.0.passport(a, b, c)
    }
    fn countries(&self) -> Vec<CountryInfo> {
        vec![]
    }
}

/// A model that takes five seconds to answer anything.
struct SlowModel;
impl crate::chat::ChatBackend for SlowModel {
    fn complete(&self, _: &[crate::chat::Message]) -> Result<String, String> {
        std::thread::sleep(Duration::from_secs(5));
        Ok("{}".into())
    }
}

#[test]
fn a_refusal_for_want_of_permission_comes_at_once_whatever_the_camera_or_the_model_are_doing() {
    let c = local("instant");
    c.env.svc.camera.set_backend(Some(Arc::new(SlowCam(FakeCam::default()))));
    *c.env.host.backend.lock().unwrap() = Some(Arc::new(SlowModel));
    for (tag, allowed, unattended, trigger, expect) in [("off", false, false, "manual", "not allowed to use the camera"), ("nobody", true, false, "cron", "only when you start it")] {
        let mut f = c.sample("cam-sleep-check", allowed, unattended);
        f.id = format!("instant-{tag}");
        let t = std::time::Instant::now();
        let r = c.run(f, trigger);
        let took = t.elapsed();
        assert_eq!(r.status, RunStatus::Blocked, "{tag}");
        assert!(r.nodes["shot"].error.contains(expect), "{}", r.nodes["shot"].error);
        assert!(took < Duration::from_millis(1500), "{tag}: the refusal took {took:?}");
        let n = &r.nodes["shot"];
        assert!(n.ended_ms > 0 && n.started_ms > 0 && n.ended_ms - n.started_ms < 1500, "{tag}: the step ran {} ms (started {} ended {})", n.ended_ms - n.started_ms, n.started_ms, n.ended_ms);
        assert!(n.attempts.len() == 1 && n.attempts[0].ended_ms > 0 && n.attempts[0].ended_ms - n.attempts[0].started_ms < 1500, "{tag}: {:?}", n.attempts);
        assert_eq!(r.heal[0].note, iyakki_flow::driver::CONSENT_NOTE);
    }
}

#[test]
fn the_sleeping_sample_takes_the_picture_in_a_private_folder_shows_it_to_the_model_and_deletes_it_after_the_run() {
    let c = local("sleeping");
    let m = c.model(&[SLEEPING]);
    let r = c.run(c.sample("cam-sleep-check", true, false), "manual");
    assert_eq!(r.status, RunStatus::Succeeded, "{} {:?}", r.error, r.nodes.iter().map(|(k, n)| (k.clone(), n.error.clone())).collect::<Vec<_>>());
    // a note went out because the model said present and sleeping
    assert_eq!(r.nodes["check"].port, "true");
    assert!(r.nodes["tell"].state.is_done());
    assert_eq!(r.nodes["ask"].output["sleeping"], true);
    // the picture was in the run's own folder under the flow data, not in Pictures, and the model got its bytes
    let taken = c.cam.taken.lock().unwrap().clone();
    assert_eq!(taken.len(), 1);
    let shown = taken[0].display().to_string();
    assert!(shown.contains("/camera/") && !shown.contains("Pictures"), "{shown}");
    assert!(r.nodes["shot"].output["path"].as_str().unwrap().contains("/camera/"));
    let seen = m.seen.lock().unwrap();
    let with_pic = seen.iter().find(|x| !x.images.is_empty()).expect("the model was shown the picture");
    assert_eq!(&with_pic.images[0][..2], &[0xFF, 0xD8]);
    assert!(with_pic.content.contains("untrusted data") && with_pic.content.contains("Do not identify who anyone is"), "{}", with_pic.content);
    // deleted when the run ended, folder and all
    assert!(!taken[0].exists(), "the picture is gone");
    assert!(c.camera_files().is_empty());
    assert!(!taken[0].parent().unwrap().exists(), "the run's folder is gone");
}

#[test]
fn the_vision_models_json_decides_the_branch() {
    let c = local("json");
    c.model(&[AWAKE, EMPTY, "I think someone is there"]);
    let r = c.run(c.sample("cam-sleep-check", true, false), "manual");
    assert_eq!(r.status, RunStatus::Succeeded);
    assert_eq!(r.nodes["check"].port, "false", "present but awake: nothing to report");
    assert_eq!(r.nodes["ask"].output["present"], true);
    let mut f = c.sample("cam-sleep-check", true, false);
    f.id = "second".into();
    let r = c.run(f, "manual");
    assert_eq!(r.nodes["ask"].output["sleeping"], Value::Null);
    assert_eq!(r.nodes["check"].port, "false", "nobody there: nothing to report");
    // an answer that is not JSON is a plain failure of the question step (the picture is still deleted)
    let mut f = c.sample("cam-sleep-check", true, false);
    f.id = "third".into();
    f.nodes.iter_mut().find(|n| n.id == "ask").unwrap().retry = None;
    let r = c.run(f, "manual");
    assert_ne!(r.status, RunStatus::Succeeded, "a failed step with nowhere to go blocks the run");
    assert!(r.nodes["ask"].error.contains("not JSON"), "{}", r.nodes["ask"].error);
    // the run is left waiting on the failed step, so its picture is still there; an hour later the sweep removes it
    assert_eq!(c.camera_files().len(), 1);
    assert_eq!(c.env.host.cam_sweep(Duration::from_secs(3600)), 0, "too young to go");
    assert_eq!(c.env.host.cam_sweep(Duration::from_secs(0)), 1);
    assert!(c.camera_files().is_empty());
    // the simpler sample
    c.model(&[EMPTY]);
    let r = c.run(c.sample("cam-person-check", true, false), "manual");
    assert_eq!(r.status, RunStatus::Succeeded);
    assert_eq!(r.nodes["check"].port, "false");
}

#[test]
fn keeping_the_picture_leaves_it_in_the_runs_private_folder() {
    let c = local("keep");
    c.model(&[EMPTY]);
    let mut f = c.sample("cam-person-check", true, false);
    f.nodes.iter_mut().find(|n| n.id == "shot").unwrap().config = json!({"keep": true});
    let r = c.run(f, "manual");
    assert_eq!(r.status, RunStatus::Succeeded);
    let kept = c.camera_files();
    assert_eq!(kept.len(), 1, "{kept:?}");
    assert!(kept[0].display().to_string().contains("/kept/"));
    assert_eq!(c.env.host.cam_sweep(Duration::from_secs(0)), 0, "a kept picture is never swept");
    assert!(kept[0].display().to_string().contains(&r.id.replace(|ch: char| !ch.is_ascii_alphanumeric() && ch != '-' && ch != '_', "_")));
}

#[test]
fn a_run_nobody_started_needs_the_unattended_switch() {
    let c = local("unattended");
    c.model(&[EMPTY, EMPTY]);
    let r = c.run(c.sample("cam-person-check", true, false), "cron");
    assert_ne!(r.status, RunStatus::Succeeded, "a failed step with nowhere to go blocks the run");
    assert!(r.nodes["shot"].error.contains("only when you start it"), "{}", r.nodes["shot"].error);
    assert!(c.cam.taken.lock().unwrap().is_empty());
    let mut f = c.sample("cam-person-check", true, true);
    f.id = "unatt2".into();
    let r = c.run(f, "cron");
    assert_eq!(r.status, RunStatus::Succeeded, "{}", r.error);
    assert_eq!(c.cam.taken.lock().unwrap().len(), 1);
}

#[test]
fn a_model_that_cannot_look_at_pictures_stops_the_flow_before_any_picture_is_taken() {
    let c = setup("blind", FakeCam::default(), "llama3:8b", crate::providers::Kind::Ollama);
    c.model(&[EMPTY]);
    let r = c.run(c.sample("cam-sleep-check", true, false), "manual");
    assert_ne!(r.status, RunStatus::Succeeded, "a failed step with nowhere to go blocks the run");
    let e = &r.nodes["shot"].error;
    assert!(e.contains("No vision model found") && e.contains("Install one") && e.contains("No picture was taken"), "{e}");
    assert!(c.cam.taken.lock().unwrap().is_empty());
}

#[test]
fn no_camera_or_a_busy_one_fails_with_a_plain_message() {
    let c = setup("nocam", FakeCam { none: true, ..Default::default() }, "qwen2.5vl:7b", crate::providers::Kind::Ollama);
    c.model(&[EMPTY]);
    let r = c.run(c.sample("cam-person-check", true, false), "manual");
    assert!(r.nodes["shot"].error.contains("No camera is connected"), "{}", r.nodes["shot"].error);
    let c = local("busy");
    c.model(&[EMPTY]);
    let _busy = c.env.svc.camera.begin("someone", "taking a photo");
    let mut f = c.sample("cam-person-check", true, false);
    f.nodes.iter_mut().find(|n| n.id == "shot").unwrap().retry = Some(iyakki_flow::RetryPolicy::fixed(1, 1));
    let r = c.run(f, "manual");
    assert!(r.nodes["shot"].error.contains("in use right now"), "{}", r.nodes["shot"].error);
    assert!(c.cam.taken.lock().unwrap().is_empty());
}

#[test]
fn a_cloud_model_gets_a_warning_in_the_node_log_and_a_local_one_does_not() {
    assert!(super::camera::provider_is_cloud(crate::providers::Kind::OpenAi, None));
    assert!(super::camera::provider_is_cloud(crate::providers::Kind::Anthropic, None));
    assert!(!super::camera::provider_is_cloud(crate::providers::Kind::Ollama, None));
    assert!(!super::camera::provider_is_cloud(crate::providers::Kind::Lemonade, None));
    assert!(!super::camera::provider_is_cloud(crate::providers::Kind::Compat, Some("http://192.168.1.20:8000/v1")));
    assert!(super::camera::provider_is_cloud(crate::providers::Kind::Compat, Some("https://models.example.org/v1")));
    assert!(super::camera::provider_is_cloud(crate::providers::Kind::Compat, None));
    let c = setup("cloud", FakeCam::default(), "gpt-4o", crate::providers::Kind::OpenAi);
    c.model(&[EMPTY]);
    let r = c.run(c.sample("cam-person-check", true, false), "manual");
    assert_eq!(r.status, RunStatus::Succeeded, "{}", r.error);
    assert!(r.nodes["shot"].attempts[0].logs.iter().any(|l| l.contains("cloud service")), "{:?}", r.nodes["shot"].attempts[0].logs);
    let c = local("localmodel");
    c.model(&[EMPTY]);
    let r = c.run(c.sample("cam-person-check", true, false), "manual");
    assert!(r.nodes["shot"].attempts[0].logs.iter().any(|l| l.contains("only to qwen2.5vl:7b on this computer")));
}

#[test]
fn the_assistant_and_imports_can_make_camera_steps_but_never_switch_the_permission_on() {
    let c = local("guard");
    let call = |name: &str, args: Value| c.env.call(name, &args, None);
    let made = call("flow_create", json!({"name": "Look", "nodes": [{"id": "shot", "kind": "cam_photo", "config": {}}]})).unwrap();
    assert!(made.contains("Made the draft `look`"), "{made}");
    assert!(!c.env.store.get_draft("look").unwrap().defaults.camera.allowed);
    let e = call("flow_edit", json!({"flow": "look", "ops": [{"op": "set_default", "camera": {"allowed": true}}]})).unwrap_err();
    assert!(e.contains("only the person can let a flow use the camera"), "{e}");
    let e = call("flow_create", json!({"name": "Sneaky", "nodes": [{"id": "s", "kind": "cam_photo", "config": {}}], "defaults": {"camera": {"allowed": true, "unattended": true}}})).unwrap_err();
    assert!(e.contains("only the person"), "{e}");
    let mut f = c.sample("cam-sleep-check", true, true);
    f.id = "imported".into();
    let e = c.env.import_json(&serde_json::to_string(&f).unwrap(), "user:t").unwrap();
    assert!(!e.defaults.camera.allowed && !e.defaults.camera.unattended, "an imported flow arrives without the permission");
    // a template from the Store arrives without it as well
    let mut t = iyakki_flow::templates::get("cam-sleep-check").unwrap();
    t.flow.defaults.camera.allowed = true;
    assert!(!iyakki_flow::templates::from_json(&serde_json::to_string(&t).unwrap()).unwrap().flow.defaults.camera.allowed);
}

#[test]
fn the_camera_samples_say_what_they_do_not_do_and_start_unallowed() {
    for id in ["cam-sleep-check", "cam-person-check"] {
        let t = iyakki_flow::templates::get(id).unwrap();
        assert!(!t.flow.defaults.camera.allowed && !t.flow.defaults.camera.unattended, "{id} starts unallowed");
        assert!(t.flow.description.contains("model that can look at pictures") && t.flow.description.contains("Camera allowed"), "{id}");
        let prompt = t.flow.node("ask").unwrap().config["prompt"].as_str().unwrap().to_string();
        assert!(prompt.contains("untrusted data") && prompt.contains("Describe only what is visible") && prompt.contains("Do not identify who anyone is"), "{id}");
        assert!(prompt.contains("JSON") && !prompt.contains("{{"), "{id}: {prompt}");
        assert_eq!(t.flow.node("ask").unwrap().config["images"], json!(["{{ nodes.shot.output.path }}"]));
    }
    let s = iyakki_flow::templates::get("cam-sleep-check").unwrap().flow.node("ask").unwrap().config["prompt"].as_str().unwrap().to_string();
    for k in ["present", "sleeping", "confidence", "reason"] {
        assert!(s.contains(&format!("\"{k}\"")), "{k}");
    }
}

// ------------------------------------------------------------------------------------------------ the data science samples

fn python_has(module: &str) -> bool {
    std::process::Command::new("python3").args(["-c", &format!("import {module}")]).output().is_ok_and(|o| o.status.success())
}

fn science(tag: &str) -> Cam {
    local(tag)
}

#[test]
fn the_data_science_samples_are_valid_flows_that_start_with_the_check_and_embed_the_unchanged_scripts() {
    let reg = iyakki_flow::Registry::builtin();
    for (id, script_file, modules) in [("sklearn-sample", "sklearn_sample.py", ["numpy", "sklearn"]), ("opencv-sample", "opencv_sample.py", ["numpy", "cv2"])] {
        let t = iyakki_flow::templates::get(id).unwrap();
        let bad: Vec<_> = iyakki_flow::validate::validate_flow(&t.flow, &reg).into_iter().filter(|i| i.level == iyakki_flow::kinds::Level::Error).collect();
        assert!(bad.is_empty(), "{id}: {bad:?}");
        assert_eq!(t.flow.edges[0].to, "check", "the first step is the toolkit check");
        let check = t.flow.node("check").unwrap().config["command"].as_array().unwrap().iter().map(|v| v.as_str().unwrap().to_string()).collect::<Vec<_>>();
        assert_eq!(&check[3..], &modules.map(String::from), "{id}");
        assert!(t.flow.edges.iter().any(|e| e.from == "check" && e.port == "error" && e.to == "missing"), "the failure is told plainly");
        let run = t.flow.node("run").unwrap().config["command"].as_array().unwrap().iter().map(|v| v.as_str().unwrap().to_string()).collect::<Vec<_>>();
        let on_disk = std::fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join("../../docs/samples/data-science").join(script_file)).unwrap();
        assert_eq!(run[2], on_disk, "{id} runs the script from docs/samples/data-science as it is");
        assert!(!serde_json::to_string(&t.flow).unwrap().contains("/home/"), "no home paths");
    }
}

#[test]
fn a_missing_toolkit_stops_the_flow_at_the_first_step_with_the_plain_message() {
    if !crate::wsexec::on_path("python3") {
        eprintln!("python3 is not on this PATH: the missing-module test is skipped");
        return;
    }
    let c = science("missing");
    let t = iyakki_flow::templates::get("sklearn-sample").unwrap();
    // a module that cannot exist stands in for "the toolkit is not installed"
    let mut f = iyakki_flow::templates::instantiate(&t, "sk-missing", "");
    let check = f.nodes.iter_mut().find(|n| n.id == "check").unwrap();
    check.config["command"] = json!(["python3", "-c", iyakki_flow::templates::TOOLKIT_CHECK, "numpy", "iyakki_no_such_module_xyz"]);
    let r = c.run(f, "manual");
    assert_eq!(r.status, RunStatus::Succeeded, "the error port leads to the plain note: {}", r.error);
    assert!(r.nodes["check"].error.contains(iyakki_flow::templates::TOOLKIT_MISSING), "{}", r.nodes["check"].error);
    assert!(r.nodes["check"].error.starts_with("`python3` exited with code 3"));
    assert!(r.nodes["missing"].state.is_done(), "the note was sent");
    assert!(!matches!(r.nodes.get("run").map(|n| n.state), Some(iyakki_flow::NodeState::Succeeded)), "the script never ran");
}

#[test]
fn the_samples_run_the_script_read_the_report_and_report_it_when_the_toolkit_is_there() {
    // needs a Python with the toolkit on PATH (a throwaway virtual environment); skipped, said aloud, when there is none
    for (id, module, expect) in [("sklearn-sample", "sklearn", "accuracy"), ("opencv-sample", "cv2", "shapes found")] {
        if !python_has(module) {
            eprintln!("python3 here has no {module}: the {id} end-to-end test is skipped");
            continue;
        }
        let c = science(id);
        let t = iyakki_flow::templates::get(id).unwrap();
        let f = iyakki_flow::templates::instantiate(&t, &format!("{id}-t"), "");
        let r = c.run(f, "manual");
        assert_eq!(r.status, RunStatus::Succeeded, "{} {:?}", r.error, r.nodes.iter().map(|(k, n)| (k.clone(), n.error.clone())).collect::<Vec<_>>());
        let report = r.outputs["report"].as_str().unwrap();
        assert!(report.contains(expect), "{report}");
        let work = c.env.data.join("work").join(format!("{id}-t")).join(id);
        assert!(work.join("report.txt").is_file());
        if id == "sklearn-sample" {
            assert!(work.join("test_set.csv").is_file());
        } else {
            assert!(work.join("shapes.png").is_file() && work.join("motion.png").is_file());
        }
    }
}

#[test]
fn where_the_picture_goes_is_said_from_the_providers_real_address_in_the_log_the_popup_text_and_never_this_computer_for_another() {
    use iyakki_flow::host::PermissionNeed;
    let classes = [
        ("http://127.0.0.1:13305", "on this computer", true),
        ("http://192.168.50.68:31435", "on another computer on your network (192.168.50.68)", false),
        ("http://nas.local:8000", "on another computer on your network (nas.local)", false),
        ("https://lemonade.example.test/api/v1", "a CLOUD service (lemonade.example.test)", false),
    ];
    for (i, (url, words, local)) in classes.iter().enumerate() {
        let c = setup_at(&format!("place{i}"), FakeCam::default(), "qwen2.5vl:7b", crate::providers::Kind::Compat, Some(url));
        c.model(&[EMPTY]);
        let r = c.run(c.sample("cam-person-check", true, false), "manual");
        assert_eq!(r.status, RunStatus::Succeeded, "{url}: {}", r.error);
        let logs = r.nodes["shot"].attempts[0].logs.join("\n");
        assert!(logs.contains(words), "{url}: {logs}");
        assert_eq!(logs.contains("this computer"), *local, "{url}: only a loopback address is this computer: {logs}");
        assert_eq!(logs.contains("Warning:") && logs.contains("CLOUD") || logs.contains("cloud service"), words.starts_with("a CLOUD"), "{url}: {logs}");
        let need = PermissionNeed { flow_id: "f".into(), flow_name: "Camera".into(), run_id: "r1".into(), node_id: "shot".into(), node_title: "Take the picture".into(), permission: "camera".into(), message: "m".into(), blocked: false };
        let popup = c.env.host.permission_text(&need);
        assert!(popup.contains("qwen2.5vl:7b") && popup.contains(words), "{url}: {popup}");
        let where_line = popup.lines().find(|l| l.starts_with("The picture would be shown to")).unwrap_or("");
        assert_eq!(where_line.contains("this computer"), *local, "{url}: {where_line}");
    }
}

#[test]
fn an_agent_step_that_shows_a_picture_with_no_vision_model_asks_for_a_choice_and_the_fix_agent_is_not_asked() {
    let c = setup("choice", FakeCam::default(), "llama3:8b", crate::providers::Kind::Ollama);
    let m = c.model(&[SLEEPING, SLEEPING]);
    let mut f = iyakki_flow::Flow::new("look", "Look");
    f.nodes.push(iyakki_flow::Node::new("start", "start", json!({})));
    f.nodes.push(iyakki_flow::Node::new("ask", "agent", json!({"prompt": "what is in it", "images": ["/nowhere/p.jpg"]})));
    f.edges.push(iyakki_flow::Edge::new("start", "ask"));
    f.defaults.heal = iyakki_flow::HealPolicy { mode: iyakki_flow::HealMode::AutoFixSimple, ..Default::default() };
    let r = c.run(f, "manual");
    assert_eq!(r.status, RunStatus::Blocked);
    assert!(m.seen.lock().unwrap().is_empty(), "neither the model nor the fixing agent was asked anything");
    assert_eq!(r.heal_spent, 0);
    assert_eq!(r.heal.len(), 1);
    let card = &r.heal[0];
    assert!(card.patch.is_none() && card.note == iyakki_flow::driver::CHOICE_NOTE, "{card:?}");
    assert!(card.diagnosis.starts_with("`ask` needs your choice: No vision model found on p."), "the card says what the engine says: {}", card.diagnosis);
    assert!(r.nodes["ask"].needs_person && r.nodes["ask"].permission == "choice");
    // no permission request is made for a choice
    assert!(c.env.svc.browser.consent.pending(&c.env.user).is_empty());
}
