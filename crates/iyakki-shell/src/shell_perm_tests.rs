//! Permission requests through the real shell: a flow step that stops for want of the camera permission makes a popup on the open
//! screen and an entry in the INBOX; the person's own answer (Allow this time, Always allow, Not now, Block) is carried out by the
//! flows engine; nothing else can answer. The camera and the model are fakes: NO real camera and NO real vision model are used.

use super::tests::*;
use super::*;
use iyakki_agent::browserconsent::{Answer, ConsentKind};
use iyakki_agent::camtools::{CameraBackend, CountryInfo, PassportOut};
use iyakki_agent::flows::{build_flow, FlowEnv};
use serde_json::json;
use std::path::{Path, PathBuf};
use std::sync::Mutex as StdMutex;
use std::time::Duration;

const SIZES: [(u32, u32); 5] = [(1024, 768), (1280, 720), (1920, 1080), (2560, 1600), (3840, 2160)];

#[derive(Default)]
struct FakeCam {
    taken: StdMutex<usize>,
}
impl CameraBackend for FakeCam {
    fn cameras(&self) -> Vec<String> {
        vec!["video0: Test camera".into()]
    }
    fn photo(&self, dir: &Path) -> Result<PathBuf, String> {
        *self.taken.lock().unwrap() += 1;
        let p = dir.join("photo-1.jpg");
        std::fs::write(&p, [0xFF, 0xD8, 0xFF, 0xD9]).map_err(|e| e.to_string())?;
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

struct Rig {
    sh: Shell,
    env: Arc<FlowEnv>,
    cam: Arc<FakeCam>,
    user: String,
    _g: std::sync::MutexGuard<'static, ()>,
}

fn rig(tag: &str) -> Rig {
    static ONE_AT_A_TIME: StdMutex<()> = StdMutex::new(());
    let guard = ONE_AT_A_TIME.lock().unwrap_or_else(|e| e.into_inner());
    crate::apps::tests::isolated_home();
    let (e, _h) = env(tag);
    {
        let mut st = e.store.lock().unwrap();
        st.create_first_admin("root", "rootpass12").unwrap();
        let admin = st.authenticate("root", "rootpass12").unwrap();
        st.create_user(&admin, tag, "permpass1234", iyakki_sec::policy::Role::Admin).unwrap();
    }
    let mut sh = Shell::new(e);
    login(&mut sh, tag, "permpass1234");
    let policy = sh.session.as_ref().unwrap().policy.clone();
    let svc = sh.env.services.clone();
    let cam = Arc::new(FakeCam::default());
    svc.camera.set_backend(Some(cam.clone()));
    svc.settings.lock().unwrap().upsert_provider(iyakki_agent::providers::ProviderCfg::new("p", iyakki_agent::providers::Kind::Ollama)).unwrap();
    svc.settings.lock().unwrap().update(|s| s.chat = Some(iyakki_agent::settings::ModelChoice { provider: "p".into(), model: "qwen2.5vl:7b".into() })).unwrap();
    let env = FlowEnv::open(&svc, tag, &policy).unwrap();
    Rig { sh, env, cam, user: tag.to_string(), _g: guard }
}

impl Rig {
    /// A published flow with one camera step (not allowed), named like the sample.
    fn flow(&self, id: &str) -> String {
        let spec = json!({"name": format!("Camera: is a person there? {id}"), "nodes": [{"id": "shot", "kind": "cam_photo", "name": "Take the picture", "config": {}}], "defaults": {"heal": {"mode": "suggest"}}});
        let f = self.env.create_draft(build_flow(&spec).unwrap(), "user:root").unwrap();
        self.env.store.publish(&f.id, &iyakki_flow::store::PublishReq { by: "user:root", label: "", changelog: "v1", approved_by: Some("user:root") }).unwrap();
        f.id
    }
    fn run(&self, flow: &str, trigger: &str) -> String {
        let r = self.env.engine.start_run(iyakki_flow::StartRequest::new(flow).trigger(trigger, "test")).unwrap().run_id;
        self.env.engine.wait_for(&r, Duration::from_secs(20), |r| r.status.is_terminal() || r.status == iyakki_flow::RunStatus::Blocked).unwrap();
        r
    }
    fn pending(&self) -> Vec<iyakki_agent::browserconsent::ConsentRequest> {
        self.sh.env.services.browser.consent.pending(&self.user).into_iter().filter(|r| r.kind == ConsentKind::Permission).collect()
    }
    fn tick(&mut self) {
        for _ in 0..6 {
            self.sh.handle(&Event::Tick);
        }
    }
    fn answer(&mut self, a: Answer) -> Result<(), String> {
        let r = self.pending().into_iter().next().expect("a request is waiting");
        let cx = self.sh.cx().unwrap();
        crate::apps::browser_consent::ConsentDialog::new(r).answer(&cx, a)
    }
    fn wait(&self, run: &str, f: impl Fn(&iyakki_flow::Run) -> bool) -> iyakki_flow::Run {
        self.env.engine.wait_for(run, Duration::from_secs(20), |r| f(r)).unwrap()
    }
    fn notes(&self) -> usize {
        self.sh.env.services.inbox.list(&self.user).iter().filter(|n| n.title.contains("needs your permission")).count()
    }
}

#[test]
fn a_consent_failure_makes_a_popup_with_four_buttons_inside_the_window_at_every_size_and_an_inbox_entry() {
    let mut r = rig("perm-popup");
    let flow = r.flow("a");
    let run = r.run(&flow, "manual");
    r.tick();
    assert_eq!(r.pending().len(), 1, "one request");
    assert_eq!(r.notes(), 1, "one note in the INBOX");
    assert!(r.sh.permpop.is_some(), "the popup is up");
    let text = r.pending()[0].text.clone();
    assert!(text.contains(iyakki_flow::kinds::camera::ALLOW_TEXT) && text.contains("Take the picture") && text.contains("qwen2.5vl:7b") && text.contains("on this computer"), "{text}");
    for (w, h) in SIZES {
        let mut g = Gfx::new(w, h);
        r.sh.render(&mut g);
        let labels = ["Allow this time", "Always allow for this flow", "Not now", "Block"];
        for (i, l) in labels.iter().enumerate() {
            let hit = r.sh.hits().iter().find(|x| x.ev == Event::Key(Key::Char((b'1' + i as u8) as char))).unwrap_or_else(|| panic!("{w}x{h}: no button {l}")).clone();
            assert!(hit.rect.x >= 0.0 && hit.rect.y >= 0.0 && hit.rect.x + hit.rect.w <= w as f32 && hit.rect.y + hit.rect.h <= h as f32, "{w}x{h}: {l} is outside the window: {:?}", hit.rect);
            assert!(hit.rect.w > 40.0 && hit.rect.h > 14.0, "{w}x{h}: {l} is too small: {:?}", hit.rect);
        }
    }
    // "Not now" by clicking its button: the popup goes, the entry stays
    let mut g = Gfx::new(1280, 720);
    r.sh.render(&mut g);
    let b = r.sh.hits().iter().find(|x| x.ev == Event::Key(Key::Char('3'))).unwrap().rect;
    r.sh.handle(&Event::MouseMove((b.x + b.w / 2.0) as i32, (b.y + b.h / 2.0) as i32));
    r.sh.handle(&Event::MouseDown((b.x + b.w / 2.0) as i32, (b.y + b.h / 2.0) as i32));
    r.sh.handle(&Event::MouseUp((b.x + b.w / 2.0) as i32, (b.y + b.h / 2.0) as i32));
    assert!(r.sh.permpop.is_none(), "the popup went");
    assert_eq!(r.pending().len(), 1, "the INBOX entry stays");
    assert!(r.pending()[0].quiet);
    r.tick();
    assert!(r.sh.permpop.is_none(), "it does not come back");
    assert_eq!(r.env.engine.get_run(&run).unwrap().status, iyakki_flow::RunStatus::Blocked, "the run waits for you");
}

#[test]
fn the_entry_exists_with_nobody_signed_in_and_the_popup_waits_for_the_person() {
    let mut r = rig("perm-signedout");
    let flow = r.flow("b");
    r.sh.session = None;
    r.run(&flow, "manual");
    r.tick();
    assert!(r.sh.permpop.is_none(), "nobody is signed in: no popup");
    assert_eq!(r.pending().len(), 1);
    assert_eq!(r.notes(), 1, "the INBOX entry is there for the owner");
}

#[test]
fn allow_this_time_lets_exactly_that_run_through_and_the_next_run_asks_again() {
    let mut r = rig("perm-once");
    let flow = r.flow("c");
    let run = r.run(&flow, "manual");
    r.tick();
    assert_eq!(*r.cam.taken.lock().unwrap(), 0);
    r.answer(Answer::AllowOnce).unwrap();
    let done = r.wait(&run, |x| x.status.is_terminal());
    assert_eq!(done.status, iyakki_flow::RunStatus::Succeeded, "{} {:?}", done.error, done.nodes.get("shot").map(|n| n.error.clone()));
    assert_eq!(done.grants, vec!["camera".to_string()]);
    assert_eq!(*r.cam.taken.lock().unwrap(), 1, "one picture");
    assert!(!r.env.store.get_draft(&flow).unwrap().defaults.camera.allowed, "the flow itself is unchanged");
    assert!(!r.env.store.published(&flow).unwrap().defaults.camera.allowed);
    assert!(r.pending().is_empty());
    // a second run asks again
    let run2 = r.run(&flow, "manual");
    r.tick();
    assert_eq!(r.env.engine.get_run(&run2).unwrap().status, iyakki_flow::RunStatus::Blocked);
    assert_eq!(r.pending().len(), 1, "asked again");
    assert!(r.env.engine.get_run(&run2).unwrap().grants.is_empty());
}

#[test]
fn always_allow_switches_the_flow_setting_on_and_publishes_it_and_a_later_run_passes() {
    let mut r = rig("perm-always");
    let flow = r.flow("d");
    let run = r.run(&flow, "manual");
    r.tick();
    r.answer(Answer::AlwaysAllow).unwrap();
    assert!(r.env.store.get_draft(&flow).unwrap().defaults.camera.allowed, "the draft says allowed");
    let published = r.env.store.published(&flow).unwrap();
    assert!(published.defaults.camera.allowed && published.meta.version == 2, "published as a new version: {}", published.meta.version);
    assert_eq!(r.wait(&run, |x| x.status.is_terminal()).status, iyakki_flow::RunStatus::Succeeded);
    let later = r.run(&flow, "manual");
    assert_eq!(r.env.engine.get_run(&later).unwrap().status, iyakki_flow::RunStatus::Succeeded);
    assert!(r.pending().is_empty(), "no new request");
    // with something else waiting in the draft, only the draft changes
    let flow2 = r.flow("d2");
    let mut d = r.env.store.get_draft(&flow2).unwrap();
    d.description = "an unpublished edit".into();
    r.env.store.save_draft(&d, None).unwrap();
    r.run(&flow2, "manual");
    r.tick();
    r.answer(Answer::AlwaysAllow).unwrap();
    assert!(r.env.store.get_draft(&flow2).unwrap().defaults.camera.allowed);
    assert!(!r.env.store.published(&flow2).unwrap().defaults.camera.allowed, "published version untouched while other changes wait");
}

#[test]
fn block_stops_the_asking_for_that_flow_and_the_settings_undo_it() {
    let mut r = rig("perm-block");
    let flow = r.flow("e");
    r.run(&flow, "manual");
    r.tick();
    r.answer(Answer::Block).unwrap();
    assert!(r.env.store.get_draft(&flow).unwrap().defaults.camera.blocked && r.env.store.published(&flow).unwrap().defaults.camera.blocked);
    let run = r.run(&flow, "manual");
    r.tick();
    assert!(r.pending().is_empty(), "no new request");
    assert_eq!(r.notes(), 1, "and no new note");
    let e = r.env.engine.get_run(&run).unwrap().nodes["shot"].error.clone();
    assert!(e.contains("block camera use for this flow"), "{e}");
    assert_eq!(*r.cam.taken.lock().unwrap(), 0);
    // undo in the flow's settings (the person's own switch)
    let mut d = r.env.store.get_draft(&flow).unwrap();
    d.defaults.camera.blocked = false;
    r.env.store.save_draft(&d, None).unwrap();
    assert!(!r.env.store.get_draft(&flow).unwrap().defaults.camera.blocked);
}

#[test]
fn nothing_but_the_persons_own_answer_can_give_a_permission() {
    let mut r = rig("perm-nobody-else");
    let flow = r.flow("f");
    let run = r.run(&flow, "manual");
    r.tick();
    let req = r.pending().remove(0);
    // developer-mode input (a paired computer) is refused, by the popup and by the INBOX alike
    r.sh.env.services.browser.consent.set_synthetic(true);
    r.sh.handle(&Event::Key(Key::Char('2')));
    let e = r.answer(Answer::AlwaysAllow).unwrap_err();
    assert!(e.contains("on the device itself"), "{e}");
    r.sh.env.services.browser.consent.set_synthetic(false);
    assert_eq!(r.pending().len(), 1, "still waiting");
    assert!(!r.env.store.get_draft(&flow).unwrap().defaults.camera.allowed);
    assert!(r.env.engine.get_run(&run).unwrap().grants.is_empty());
    // another account cannot answer it either
    assert!(r.sh.env.services.browser.consent.resolve("someone-else", &req.id, Answer::AllowOnce).is_err());
    // the assistant has no tool for it, cannot set the flow's camera switch, and the fixing agent's patch is refused
    let tools = iyakki_agent::flows::tools::defs(&iyakki_sec::policy::Role::Admin.template());
    assert!(tools.iter().all(|t| !t.spec.name.contains("permission") && !t.spec.name.contains("consent")), "no tool answers a permission request");
    let e = r.env.call("flow_edit", &json!({"flow": flow, "ops": [{"op": "set_default", "camera": {"allowed": true}}]}), None).unwrap_err();
    assert!(e.contains("only the person"), "{e}");
    // an imported flow arrives without the setting
    let mut f = r.env.store.get_draft(&flow).unwrap();
    f.defaults.camera.allowed = true;
    f.defaults.camera.blocked = true;
    let imported = r.env.import_json(&serde_json::to_string(&f).unwrap(), "user:t").unwrap();
    assert!(!imported.defaults.camera.allowed);
}

#[test]
fn a_popup_older_than_ten_minutes_is_not_shown_but_the_entry_stays() {
    let mut r = rig("perm-expiry");
    let old = iyakki_agent::clock::now_unix() - iyakki_agent::flows::permission::POPUP_SECS - 5;
    r.sh.env.services.browser.consent.create_permission(&r.user, "some-flow", "Old / step needs your permission", "text", "r1|shot|camera|some-flow", old).unwrap();
    r.tick();
    assert!(r.sh.permpop.is_none(), "too old for a popup");
    assert_eq!(r.pending().len(), 1, "but still waiting in the INBOX");
    // a shown popup also goes when it turns old
    let fresh = iyakki_agent::clock::now_unix();
    r.sh.env.services.browser.consent.withdraw_permissions(&r.user, "r1");
    r.sh.env.services.browser.consent.create_permission(&r.user, "some-flow", "New / step needs your permission", "text", "r2|shot|camera|some-flow", fresh).unwrap();
    r.tick();
    assert!(r.sh.permpop.is_some());
    // withdrawing a dismissed run's requests removes them
    assert_eq!(r.sh.env.services.browser.consent.withdraw_permissions(&r.user, "r2"), 1);
    r.tick();
    assert!(r.sh.permpop.is_none() && r.pending().is_empty());
}

#[test]
fn repeated_failures_of_the_same_step_make_one_request_and_one_note() {
    let mut r = rig("perm-dedupe");
    let flow = r.flow("g");
    let run = r.run(&flow, "manual");
    // the step is retried by hand twice and stops each time
    for _ in 0..2 {
        r.env.engine.retry_node(&run, "shot").unwrap();
        r.wait(&run, |x| x.nodes["shot"].state == iyakki_flow::NodeState::Failed && x.status == iyakki_flow::RunStatus::Blocked);
        std::thread::sleep(Duration::from_millis(150));
    }
    r.tick();
    assert_eq!(r.pending().len(), 1, "one request");
    assert_eq!(r.notes(), 1, "one note");
}

#[test]
fn a_run_nobody_started_asks_for_the_separate_unattended_permission_with_its_own_text() {
    let mut r = rig("perm-unattended");
    let flow = r.flow("h");
    let mut d = r.env.store.get_draft(&flow).unwrap();
    d.defaults.camera.allowed = true;
    r.env.store.save_draft(&d, None).unwrap();
    r.env.store.publish(&flow, &iyakki_flow::store::PublishReq { by: "user:root", label: "", changelog: "allowed", approved_by: Some("user:root") }).unwrap();
    let run = r.run(&flow, "cron");
    r.tick();
    let req = r.pending().remove(0);
    assert!(req.payload.contains("|camera_unattended|"), "{}", req.payload);
    assert!(req.text.contains("Camera also when nobody is there") && req.text.contains("separate permission"), "{}", req.text);
    r.answer(Answer::AllowOnce).unwrap();
    let done = r.wait(&run, |x| x.status.is_terminal());
    assert_eq!(done.status, iyakki_flow::RunStatus::Succeeded, "{}", done.error);
    assert_eq!(done.grants, vec!["camera_unattended".to_string()]);
    assert!(!r.env.store.published(&flow).unwrap().defaults.camera.unattended);
}
