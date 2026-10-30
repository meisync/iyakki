mod common;

use std::sync::Arc;

use common::*;
use iyakki_flow::assist::*;
use iyakki_flow::assist_run::*;
use iyakki_flow::assist_store::*;
use iyakki_flow::heal::{Patch, PatchOp};
use iyakki_flow::host::{FakeHost, HealProposal, Host};
use iyakki_flow::store::{diff, PublishReq};
use iyakki_flow::*;
use serde_json::json;

// ------------------------------------------------------------------------------------------------ the setting

#[test]
fn a_flow_written_before_assist_loads_as_off_and_a_default_writes_nothing_new() {
    let f = linear("old");
    let text = f.to_json();
    assert!(!text.contains("assist"), "a flow with the default setting is written exactly as before: no new key");
    let back = Flow::from_json(&text).unwrap();
    assert_eq!(back.defaults.assist.mode, AssistMode::Off);
    assert!(!back.defaults.assist.auto_publish && back.defaults.assist.allow_fix && !back.defaults.assist.allow_upgrade);
    assert_eq!(back.content_hash(), f.content_hash(), "the content hash of existing versions does not change");
    // a published version written by alpha.1 still verifies
    let e = env("assist-old");
    e.publish(f);
    e.store.verify("old", 1).unwrap();
    // a flow that sets it keeps it
    let mut g = linear("new");
    g.defaults.assist = AssistPolicy { mode: AssistMode::Autopilot, auto_publish: true, ..Default::default() };
    let t = g.to_json();
    assert!(t.contains("\"autopilot\""));
    assert_eq!(Flow::from_json(&t).unwrap().defaults.assist.mode, AssistMode::Autopilot);
    // unknown or partial settings fall back to the defaults
    let part = Flow::from_json(r#"{"id":"p","name":"P","defaults":{"assist":{"mode":"copilot"}}}"#).unwrap();
    assert_eq!(part.defaults.assist.mode, AssistMode::Copilot);
    assert_eq!(part.defaults.assist.max_publish_per_day, 1);
    assert_eq!(AssistMode::parse("Auto-Pilot"), Some(AssistMode::Autopilot));
    assert_eq!(AssistMode::parse("sure"), None);
}

#[test]
fn an_imported_flow_or_store_template_never_arrives_with_assist_on() {
    let mut f = linear("imp");
    f.defaults.assist = AssistPolicy { mode: AssistMode::Autopilot, auto_publish: true, ..Default::default() };
    templates::sanitize_import(&mut f);
    assert_eq!(f.defaults.assist, AssistPolicy::default());
    assert_eq!(f.defaults.assist.mode, AssistMode::Off);
}

#[test]
fn the_assist_files_have_a_valid_schema_and_keep_what_a_newer_release_wrote() {
    ASSIST_SCHEMA.check().unwrap();
    let e = env("assist-schema");
    e.publish(linear("fl"));
    let root = e.dir.join("flows");
    let book = AssistStore::from_fn(&root, || 5);
    book.set_paused("fl", true).unwrap();
    let p = root.join("fl").join("assist.json");
    let mut v: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(&p).unwrap()).unwrap();
    assert_eq!(v["schema"], 1);
    v["future_field"] = json!({"x": 1});
    std::fs::write(&p, v.to_string()).unwrap();
    book.set_paused("fl", false).unwrap();
    let after = std::fs::read_to_string(&p).unwrap();
    assert!(after.contains("future_field"), "{after}");
    // a file from a newer schema is read but never written
    let mut v2: serde_json::Value = serde_json::from_str(&after).unwrap();
    v2["schema"] = json!(99);
    std::fs::write(&p, v2.to_string()).unwrap();
    assert!(book.set_paused("fl", true).is_err());
    assert!(std::fs::read_to_string(&p).unwrap().contains("\"schema\":99"));
}

// ------------------------------------------------------------------------------------------------ the matrix

fn all_pass() -> GuardReport {
    let mut r = GuardReport::default();
    for g in PUBLISH_GUARDS.iter().chain(DRAFT_GUARDS.iter()).chain(ROLLBACK_GUARDS.iter()) {
        r.set(*g, Ok(()));
    }
    r
}

const ALL_GUARDS: [Guard; 13] = [Guard::Qa, Guard::Trial, Guard::DiffStored, Guard::DiffSize, Guard::NodeKinds, Guard::Forbidden, Guard::NewHosts, Guard::BaseCurrent, Guard::FlowCap, Guard::DayCap, Guard::DraftCap, Guard::Budget, Guard::OwnVersionFailing];

fn reports() -> Vec<(String, GuardReport)> {
    let mut v = vec![("all pass".to_string(), all_pass()), ("empty".to_string(), GuardReport::default())];
    for g in ALL_GUARDS {
        let mut r = all_pass();
        r.set(g, Err("no".into()));
        v.push((format!("{g:?} fails"), r));
        let mut m = all_pass();
        m.results.retain(|(x, _)| *x != g);
        v.push((format!("{g:?} missing"), m));
    }
    v
}

fn expected(mode: AssistMode, p: &AssistPolicy, class: Class, of: Option<Class>, r: &GuardReport) -> &'static str {
    if mode == AssistMode::Off {
        return "deny";
    }
    match class {
        Class::Delete => "deny",
        Class::ApplyToRuns => "ask",
        Class::Fix | Class::Upgrade => {
            if !r.passes(&DRAFT_GUARDS) {
                "deny"
            } else if (class == Class::Fix && !p.allow_fix) || (class == Class::Upgrade && !p.allow_upgrade) {
                "ask"
            } else {
                "allow"
            }
        }
        Class::Publish => {
            if mode == AssistMode::Autopilot && of == Some(Class::Fix) && p.auto_publish && r.passes(&PUBLISH_GUARDS) {
                "allow"
            } else {
                "ask"
            }
        }
        Class::Rollback => {
            if mode == AssistMode::Autopilot && r.passes(&ROLLBACK_GUARDS) {
                "allow"
            } else {
                "ask"
            }
        }
    }
}

#[test]
fn the_matrix_is_exhaustive_every_mode_class_origin_setting_and_guard() {
    let mut n = 0;
    let mut allowed_publishes = 0;
    for mode in [AssistMode::Off, AssistMode::Copilot, AssistMode::Autopilot] {
        for auto in [false, true] {
            for fix in [false, true] {
                for up in [false, true] {
                    let p = AssistPolicy { mode, auto_publish: auto, allow_fix: fix, allow_upgrade: up, ..Default::default() };
                    for class in Class::ALL {
                        for of in [None, Some(Class::Fix), Some(Class::Upgrade)] {
                            for (name, r) in reports() {
                                let d = decide(mode, &p, class, of, &r);
                                let want = expected(mode, &p, class, of, &r);
                                let got = if d.is_allow() { "allow" } else if d.is_ask() { "ask" } else { "deny" };
                                assert_eq!(got, want, "{mode:?} auto={auto} fix={fix} up={up} {class:?} of={of:?} [{name}]: {d:?}");
                                n += 1;
                                // the invariants, stated on their own
                                if d.is_allow() {
                                    assert_ne!(mode, AssistMode::Off);
                                    assert!(!matches!(class, Class::Delete | Class::ApplyToRuns), "never automatic: {class:?}");
                                    if class == Class::Publish {
                                        allowed_publishes += 1;
                                        assert!(mode == AssistMode::Autopilot && auto && of == Some(Class::Fix), "{mode:?} {auto} {of:?}");
                                        for g in PUBLISH_GUARDS {
                                            assert!(r.results.iter().any(|(x, res)| *x == g && res.is_ok()), "{g:?} must have passed");
                                        }
                                    }
                                    if class == Class::Rollback {
                                        assert_eq!(mode, AssistMode::Autopilot);
                                    }
                                }
                                if !d.is_allow() {
                                    match &d {
                                        Decision::Ask(w) | Decision::Deny(w) => assert!(!w.is_empty(), "every refusal says why"),
                                        Decision::Allow => unreachable!(),
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
    }
    assert!(n > 5000, "{n} combinations");
    assert!(allowed_publishes > 0, "the one allowed path exists");
}

#[test]
fn the_effective_mode_follows_the_kill_switch_pause_and_cool_down() {
    let auto = AssistPolicy { mode: AssistMode::Autopilot, ..Default::default() };
    let cop = AssistPolicy { mode: AssistMode::Copilot, ..Default::default() };
    let off = AssistPolicy::default();
    for (p, s, want) in [
        (&auto, Situation::default(), AssistMode::Autopilot),
        (&auto, Situation { paused: true, ..Default::default() }, AssistMode::Copilot),
        (&auto, Situation { cooling: true, ..Default::default() }, AssistMode::Copilot),
        (&auto, Situation { kill: true, paused: true, cooling: true }, AssistMode::Off),
        (&cop, Situation { paused: true, cooling: true, kill: false }, AssistMode::Copilot),
        (&cop, Situation { kill: true, ..Default::default() }, AssistMode::Off),
        (&off, Situation::default(), AssistMode::Off),
        (&off, Situation { paused: true, ..Default::default() }, AssistMode::Off),
    ] {
        assert_eq!(effective_mode(p, s), want, "{:?} {s:?}", p.mode);
    }
}

// ------------------------------------------------------------------------------------------------ the guards

fn base() -> Flow {
    let mut f = linear("g");
    f.nodes[1].config = json!({"url": "https://api.example.com/a", "headers": {"Accept": "application/json"}});
    f
}

fn report_for(change: impl FnOnce(&mut Flow), facts: Facts) -> GuardReport {
    let b = base();
    let mut d = b.clone();
    change(&mut d);
    let df = diff(&b, &d);
    evaluate(&AssistPolicy::default(), &b, &d, &df, &facts)
}

fn failing(r: &GuardReport) -> Vec<Guard> {
    ALL_GUARDS.iter().copied().filter(|g| !r.passes(&[*g]) && *g != Guard::OwnVersionFailing).collect()
}

#[test]
fn a_small_change_to_the_failing_step_passes_every_guard() {
    let r = report_for(|f| f.nodes[1].config["url"] = json!("https://api.example.com/v2/a"), Facts::clean());
    assert!(r.passes(&PUBLISH_GUARDS), "{:?}", r.failures(&PUBLISH_GUARDS));
}

#[test]
fn each_guard_fails_for_its_own_reason() {
    // QA, trial, stored diff, base
    let url = |f: &mut Flow| f.nodes[1].config["url"] = json!("https://api.example.com/v2/a");
    let mut facts = Facts::clean();
    facts.qa = Err("bad".into());
    assert_eq!(failing(&report_for(url, facts)), vec![Guard::Qa]);
    let mut facts = Facts::clean();
    facts.trial = TrialState::NotRun("effects".into());
    assert_eq!(failing(&report_for(url, facts)), vec![Guard::Trial]);
    let mut facts = Facts::clean();
    facts.trial = TrialState::Failed("404".into());
    assert_eq!(failing(&report_for(url, facts)), vec![Guard::Trial]);
    let mut facts = Facts::clean();
    facts.diff_stored = false;
    assert_eq!(failing(&report_for(url, facts)), vec![Guard::DiffStored]);
    let mut facts = Facts::clean();
    facts.base_current = Err("moved".into());
    assert_eq!(failing(&report_for(url, facts)), vec![Guard::BaseCurrent]);
    // caps and budget
    let mut facts = Facts::clean();
    facts.published_today_flow = 1;
    assert_eq!(failing(&report_for(url, facts)), vec![Guard::FlowCap]);
    let mut facts = Facts::clean();
    facts.published_today_all = 3;
    assert_eq!(failing(&report_for(url, facts)), vec![Guard::DayCap]);
    let mut facts = Facts::clean();
    facts.drafts_today = 4;
    assert_eq!(failing(&report_for(url, facts)), vec![Guard::DraftCap]);
    let mut facts = Facts::clean();
    facts.cents_today = 50;
    assert_eq!(failing(&report_for(url, facts)), vec![Guard::Budget]);
    let mut facts = Facts::clean();
    facts.cents_this_fix = 11;
    assert_eq!(failing(&report_for(url, facts)), vec![Guard::Budget]);
    // size: three steps changed
    let r = report_for(
        |f| {
            f.nodes[1].config["url"] = json!("https://api.example.com/v2/a");
            f.nodes[0].name = "x".into();
            f.nodes[2].name = "y".into();
        },
        Facts::clean(),
    );
    assert_eq!(failing(&r), vec![Guard::DiffSize]);
    // too many settings in one step
    let r = report_for(
        |f| {
            for i in 0..11 {
                f.nodes[1].config[format!("k{i}")] = json!(i);
            }
        },
        Facts::clean(),
    );
    assert_eq!(failing(&r), vec![Guard::DiffSize]);
}

#[test]
fn nothing_protected_may_change_in_an_automatic_publish() {
    let cases: Vec<(&str, Box<dyn Fn(&mut Flow)>)> = vec![
        ("camera permission", Box::new(|f| f.defaults.camera.allowed = true)),
        ("browser trust", Box::new(|f| f.defaults.browser.trusted = true)),
        ("assist setting itself", Box::new(|f| f.defaults.assist.mode = AssistMode::Autopilot)),
        ("schedule", Box::new(|f| f.triggers.push(Trigger::new("t", TriggerKind::Manual)))),
        ("flow runner", Box::new(|f| f.defaults.runner = RunnerKind::K3sJob)),
        ("step runner", Box::new(|f| f.nodes[1].runner = Some(RunnerKind::K3sJob))),
        ("heal policy", Box::new(|f| f.defaults.heal.mode = HealMode::AutoFixSimple)),
        ("production flag", Box::new(|f| f.production = true)),
        ("credential header", Box::new(|f| f.nodes[1].config["headers"]["Authorization"] = json!("Bearer abc"))),
        ("secret reference", Box::new(|f| f.nodes[1].config["headers"]["X-Trace"] = json!("{{secret.vault.key}}"))),
        ("allow list", Box::new(|f| f.nodes[1].config["allow_domains"] = json!(["*"]))),
        ("kind", Box::new(|f| f.nodes[1].kind = "cli".into())),
        ("lib reference", Box::new(|f| f.nodes[1].lib = Some(LibRef { id: "x".into(), version: 2 }))),
        ("a step removed", Box::new(|f| {
            f.nodes.remove(1);
        })),
        ("a connection added", Box::new(|f| f.edges.push(Edge::new("s", "e")))),
        ("inputs", Box::new(|f| f.inputs.push(Param { name: "x".into(), ..Default::default() }))),
    ];
    for (what, change) in cases {
        let r = report_for(&*change, Facts::clean());
        assert!(!r.passes(&[Guard::Forbidden]), "{what} must be forbidden");
    }
    // and the person's own settings are refused outright when a suggestion is made, in every mode
    for (what, change) in [("camera", Box::new(|f: &mut Flow| f.defaults.camera.allowed = true) as Box<dyn Fn(&mut Flow)>), ("assist", Box::new(|f| f.defaults.assist.auto_publish = true)), ("trust", Box::new(|f| f.defaults.browser.trusted = true)), ("heal", Box::new(|f| f.defaults.heal.auto_publish_simple = true))] {
        let b = base();
        let mut d = b.clone();
        change(&mut d);
        assert!(!hard_forbidden(&diff(&b, &d)).is_empty(), "{what}");
    }
    let b = base();
    let mut d = b.clone();
    d.nodes[1].config["url"] = json!("https://api.example.com/v2/a");
    assert!(hard_forbidden(&diff(&b, &d)).is_empty() && forbidden_changes(&diff(&b, &d)).is_empty());
}

#[test]
fn a_new_effect_or_a_new_host_stops_an_automatic_publish() {
    let r = report_for(|f| f.nodes[1].config["url"] = json!("https://evil.example/a"), Facts::clean());
    assert_eq!(failing(&r), vec![Guard::NewHosts]);
    let r = report_for(|f| f.nodes[1].config["headers"]["Accept"] = json!("https://other.example/ref"), Facts::clean());
    assert!(failing(&r).contains(&Guard::NewHosts));
    let r = report_for(|f| f.nodes[1].config["method"] = json!("POST"), Facts::clean());
    assert!(failing(&r).contains(&Guard::NodeKinds), "a read became a write");
    let r = report_for(|f| f.nodes.push(Node::new("n", "notify", json!({"title": "hi"}))), Facts::clean());
    assert!(failing(&r).contains(&Guard::NodeKinds));
    let r = report_for(|f| f.nodes.push(Node::new("d", "delay", json!({"seconds": 1}))), Facts::clean());
    assert!(!failing(&r).contains(&Guard::NodeKinds), "a pure step reaches nowhere");
    assert!(effectful("cli", &json!({})) && effectful("cam_photo", &json!({})) && effectful("http", &json!({"method": "delete"})) && !effectful("http", &json!({"method": "get"})) && !effectful("condition", &json!({})));
    for k in ["browser", "br_click", "cam_photo", "notify", "agent"] {
        assert!(never_in_trial(k), "{k}");
    }
    assert!(!never_in_trial("http"));
}

#[test]
fn the_watch_rolls_back_on_one_failure_among_the_first_runs_and_ends_after_enough_good_ones() {
    let p = AssistPolicy { watch_runs: 3, ..Default::default() };
    assert_eq!(watch_verdict(&p, &[]), Verdict::Pending);
    assert_eq!(watch_verdict(&p, &[true, true]), Verdict::Pending);
    assert_eq!(watch_verdict(&p, &[true, true, true]), Verdict::Healthy);
    assert_eq!(watch_verdict(&p, &[true, false]), Verdict::Failing);
    assert_eq!(watch_verdict(&p, &[false]), Verdict::Failing);
    assert_eq!(watch_verdict(&p, &[true, true, true, false]), Verdict::Healthy, "only the first runs count");
}

// ------------------------------------------------------------------------------------------------ the work

struct FakeTrial(TrialState);
impl Trial for FakeTrial {
    fn trial(&self, _f: &Flow, _r: &Run, _c: &[String]) -> TrialState {
        self.0.clone()
    }
}

struct World {
    e: Env,
    book: AssistStore,
}

impl World {
    fn new(tag: &str, p: AssistPolicy) -> World {
        let e = env(tag);
        // the site: the old address is gone, the new one works
        e.host.on_http(|req| if req.url.contains("/v2/") { ok_json("{}") } else { Ok(host::ok_response(404, "gone")) });
        let mut f = linear("fl");
        f.defaults.assist = p;
        e.publish(f);
        let c = e.clock.clone();
        let book = AssistStore::from_fn(&e.dir.join("flows"), move || c.now_ms());
        World { e, book }
    }

    fn run(&self) -> Run {
        self.e.run("fl")
    }

    fn propose_url(&self) {
        self.e.host.propose(Ok(HealProposal { diagnosis: "the address moved to /v2".into(), patch: Some(Patch { node: "a".into(), ops: vec![PatchOp::SetConfig { path: "url".into(), value: json!("https://api.example.com/v2/a") }], reason: "moved".into(), claimed_risk: "low".into(), touches: vec![] }), cents: 2 }));
    }

    fn tick_with(&self, trial: &dyn Trial) -> Vec<String> {
        let reg = self.e.engine.registry();
        let a = Assistant { store: &self.e.store, book: &self.book, host: &*self.e.host as &dyn Host, reg: &reg, trial };
        let engine = self.e.engine.clone();
        a.tick(&self.e.engine.summaries(), &move |id| engine.get_run(id))
    }

    fn tick(&self) -> Vec<String> {
        let reg = self.e.engine.registry();
        let t = EngineTrial { engine: &self.e.engine, reg: &reg };
        self.tick_with(&t)
    }

    fn assistant<'a>(&'a self, reg: &'a Registry, t: &'a dyn Trial) -> Assistant<'a> {
        Assistant { store: &self.e.store, book: &self.book, host: &*self.e.host as &dyn Host, reg, trial: t }
    }

    fn published(&self) -> Flow {
        self.e.store.published("fl").unwrap()
    }
}

fn copilot() -> AssistPolicy {
    AssistPolicy { mode: AssistMode::Copilot, ..Default::default() }
}

fn autopilot(auto: bool) -> AssistPolicy {
    AssistPolicy { mode: AssistMode::Autopilot, auto_publish: auto, watch_runs: 2, ..Default::default() }
}

#[test]
fn off_does_nothing_even_when_a_run_fails() {
    let w = World::new("assist-off", AssistPolicy::default());
    let r = w.run();
    assert_eq!(r.status, RunStatus::Failed);
    w.propose_url();
    let out = w.tick();
    assert!(out.is_empty(), "{out:?}");
    assert!(w.e.host.heal_requests.lock().unwrap().is_empty(), "the model is not asked");
    assert!(w.book.list("fl").is_empty());
}

#[test]
fn copilot_turns_a_failed_run_into_a_stored_suggestion_and_changes_nothing_else() {
    let w = World::new("assist-copilot", copilot());
    let before_draft = w.e.store.get_draft("fl").unwrap();
    let r = w.run();
    assert_eq!(r.status, RunStatus::Failed);
    w.propose_url();
    let out = w.tick();
    assert_eq!(out.len(), 1, "{out:?}");
    let list = w.book.list("fl");
    assert_eq!(list.len(), 1);
    let s = &list[0];
    assert_eq!((s.status, s.class, s.node.as_str(), s.base_version, s.run_id.as_str()), (Status::Pending, Class::Fix, "a", 1, r.id.as_str()));
    assert!(s.diff.iter().any(|l| l.contains("config.url") && l.contains("/v2/a")), "{:?}", s.diff);
    assert!(s.reason.contains("moved to /v2") && s.summary.contains("url"));
    assert!(matches!(s.trial, TrialState::Passed(_)), "{:?}", s.trial);
    assert!(s.qa.iter().any(|l| l.contains("valid")));
    assert!(s.asks.contains("copilot"), "{}", s.asks);
    assert_eq!(s.proposed.node("a").unwrap().config["url"], "https://api.example.com/v2/a");
    assert_eq!(s.cents, 2);
    // nothing was published or edited
    assert_eq!(w.published().meta.version, 1);
    assert_eq!(w.e.store.versions("fl").len(), 1);
    assert_eq!(w.e.store.get_draft("fl").unwrap(), before_draft);
    // the person is told (needs you), the audit says what happened, the same failure is not answered twice
    let n: Vec<_> = w.e.host.notices.lock().unwrap().iter().filter(|n| n.title.contains("fix is ready")).cloned().collect();
    assert!(n.len() == 1 && n[0].needs_you && n[0].flow_id == "fl" && n[0].run_id == r.id, "{n:?}");
    let a = w.e.host.audit_lines();
    assert!(a.iter().any(|l| l.starts_with("flow.assist.suggested fl s1 fix")), "{a:?}");
    assert!(w.tick().is_empty());
    assert_eq!(w.book.list("fl").len(), 1);
    // the spend and the draft are counted
    assert_eq!((w.book.drafts_today("fl"), w.book.cents_today("fl")), (1, 2));
    // the model was asked once, with the failing step and no input value
    assert_eq!(w.e.host.heal_requests.lock().unwrap().len(), 1);
}

#[test]
fn the_model_is_never_given_an_input_value() {
    let w = World::new("assist-secret", copilot());
    let token = format!("{}{}", "tok_", "Q7w8E9r0T1y2U3i4");
    let o = w.e.engine.start_run(StartRequest::new("fl").inputs(json!({"api": token}))).unwrap();
    w.e.finish(&o.run_id);
    w.e.host.propose(Ok(HealProposal { diagnosis: format!("the token {token} is refused"), patch: None, cents: 0 }));
    w.tick();
    let ctx = w.e.host.heal_requests.lock().unwrap()[0].context.clone();
    assert!(!ctx.contains(&token));
    let all = std::fs::read_dir(w.e.dir.join("flows").join("fl")).unwrap().count();
    assert!(all > 0);
    let audits = w.e.host.audit_lines().join("\n");
    assert!(!audits.contains(&token), "{audits}");
}

#[test]
fn approving_puts_the_change_in_the_draft_and_the_ordinary_publish_makes_it_a_version() {
    let w = World::new("assist-approve", copilot());
    w.run();
    w.propose_url();
    w.tick();
    let reg = w.e.engine.registry();
    let t = FakeTrial(TrialState::Passed("x".into()));
    let a = w.assistant(&reg, &t);
    // the person's own unpublished edit is never overwritten
    let mut d = w.e.store.get_draft("fl").unwrap();
    d.description = "my edit".into();
    w.e.store.save_draft(&d, None).unwrap();
    let e = a.adopt("fl", "s1").unwrap_err();
    assert!(e.contains("not published yet"), "{e}");
    d.description = String::new();
    w.e.store.save_draft(&d, None).unwrap();
    let s = a.adopt("fl", "s1").unwrap();
    assert_eq!(s.status, Status::Loaded);
    let draft = w.e.store.get_draft("fl").unwrap();
    assert_eq!(draft.node("a").unwrap().config["url"], "https://api.example.com/v2/a");
    assert!(draft.meta.modified_by.iter().any(|c| c.by_agent()), "the draft carries the agent badge");
    // the store still refuses to publish the agent's change without a person
    let e = w.e.store.publish("fl", &PublishReq { by: "user:me", label: "", changelog: "x", approved_by: None }).unwrap_err();
    assert!(e.contains("approve"), "{e}");
    let v = w.e.store.publish("fl", &PublishReq { by: "user:me", label: "", changelog: "take the fix", approved_by: Some("user:me") }).unwrap();
    assert_eq!(v, 2);
    a.after_publish("fl", v, "user:me");
    let s = w.book.get("fl", "s1").unwrap();
    assert_eq!((s.status, s.published_version), (Status::Approved, 2));
    assert!(w.e.host.audit_lines().iter().any(|l| l.starts_with("flow.assist.approved fl s1 v2")));
    // the next run works
    assert_eq!(w.run().status, RunStatus::Succeeded);
}

#[test]
fn rejecting_takes_the_loaded_change_out_of_the_draft_again() {
    let w = World::new("assist-reject", copilot());
    w.run();
    w.propose_url();
    w.tick();
    let reg = w.e.engine.registry();
    let t = FakeTrial(TrialState::Passed("x".into()));
    let a = w.assistant(&reg, &t);
    a.adopt("fl", "s1").unwrap();
    a.reject("fl", "s1", "user:me").unwrap();
    assert_eq!(w.book.get("fl", "s1").unwrap().status, Status::Rejected);
    assert_eq!(w.e.store.get_draft("fl").unwrap().node("a").unwrap().config["url"], "https://api.example.com/a");
    assert!(a.adopt("fl", "s1").is_err(), "a rejected suggestion cannot be approved");
    assert!(a.reject("fl", "s1", "user:me").is_err());
    // a change published another way makes the waiting suggestion out of date
    w.run();
    w.propose_url();
    w.tick();
    let s2 = w.book.open_list("fl");
    assert_eq!(s2.len(), 1);
    let mut d = w.e.store.get_draft("fl").unwrap();
    d.description = "other".into();
    w.e.store.save_draft(&d, None).unwrap();
    let v = w.e.store.publish("fl", &PublishReq { by: "user:me", label: "", changelog: "other", approved_by: Some("user:me") }).unwrap();
    a.after_publish("fl", v, "user:me");
    assert_eq!(w.book.get("fl", &s2[0].id).unwrap().status, Status::Superseded);
    assert!(a.adopt("fl", &s2[0].id).is_err());
}

#[test]
fn autopilot_without_auto_publish_drafts_and_tests_but_asks() {
    let w = World::new("assist-ask", autopilot(false));
    w.run();
    w.propose_url();
    w.tick();
    let s = &w.book.list("fl")[0];
    assert_eq!(s.status, Status::Pending);
    assert!(s.asks.contains("not switched on"), "{}", s.asks);
    assert!(matches!(s.trial, TrialState::Passed(_)));
    assert_eq!(w.published().meta.version, 1);
}

#[test]
fn autopilot_with_auto_publish_publishes_a_tested_fix_audits_notifies_and_watches() {
    let w = World::new("assist-auto", autopilot(true));
    w.run();
    w.propose_url();
    let out = w.tick();
    assert!(out[0].contains("published as v2"), "{out:?} {:?}", w.book.list("fl").iter().map(|s| (s.asks.clone(), s.guards.clone())).collect::<Vec<_>>());
    let p = w.published();
    assert_eq!(p.meta.version, 2);
    assert_eq!(p.node("a").unwrap().config["url"], "https://api.example.com/v2/a");
    let ve = w.e.store.versions("fl");
    assert_eq!((ve[1].author.as_str(), ve[1].approved_by.as_str()), (ASSIST_AGENT, POLICY_APPROVER), "history says who and under what authority");
    assert!(ve[1].changelog.starts_with("autopilot fix:"));
    let s = w.book.get("fl", "s1").unwrap();
    assert_eq!((s.status, s.published_version), (Status::AutoPublished, 2));
    assert!(s.diff.iter().any(|l| l.contains("config.url")), "the diff is stored");
    assert!(s.guards.iter().all(|g| g.1), "{:?}", s.guards);
    assert!(w.book.watch("fl").is_some_and(|x| x.version == 2 && x.previous == 1));
    assert_eq!((w.book.publishes_today("fl"), w.book.publishes_today_all()), (1, 1));
    let a = w.e.host.audit_lines();
    assert!(a.iter().any(|l| l.starts_with("flow.assist.published fl v2 from v1 s1")), "{a:?}");
    let n = w.e.host.notices.lock().unwrap().clone();
    assert!(n.iter().any(|x| x.title.contains("Autopilot published version 2") && !x.needs_you), "{n:?}");
    // the person's draft is the published version, ready for their own edits
    assert!(diff(&w.published(), &w.e.store.get_draft("fl").unwrap()).is_empty());
    assert_eq!(w.run().status, RunStatus::Succeeded);
}

#[test]
fn every_failed_guard_turns_the_publish_into_a_question_with_the_reason() {
    // trial could not run
    let w = World::new("assist-g-trial", autopilot(true));
    w.run();
    w.propose_url();
    w.tick_with(&FakeTrial(TrialState::NotRun("it has effects".into())));
    let s = &w.book.list("fl")[0];
    assert_eq!(s.status, Status::Pending);
    assert!(s.asks.contains("trial") && s.asks.contains("it has effects"), "{}", s.asks);
    assert_eq!(w.published().meta.version, 1);
    assert!(w.e.host.notices.lock().unwrap().iter().any(|n| n.needs_you));
    // a trial that failed
    let w = World::new("assist-g-trial2", autopilot(true));
    w.run();
    w.propose_url();
    w.tick_with(&FakeTrial(TrialState::Failed("404".into())));
    assert!(w.book.list("fl")[0].asks.contains("failed: 404"));
    // the size limit
    let mut p = autopilot(true);
    p.max_diff_fields = 0;
    let w = World::new("assist-g-size", p);
    w.run();
    w.propose_url();
    w.tick();
    assert!(w.book.list("fl")[0].asks.contains("settings changed"), "{}", w.book.list("fl")[0].asks);
    // the account's kill switch
    let w = World::new("assist-g-kill", autopilot(true));
    w.run();
    w.book.set_enabled(false).unwrap();
    w.propose_url();
    let out = w.tick();
    assert!(out.is_empty() && w.book.list("fl").is_empty() && w.e.host.heal_requests.lock().unwrap().is_empty());
    // a fix that writes (read became write) is not a simple fix and gains an effect
    let w = World::new("assist-g-effect", autopilot(true));
    w.run();
    w.e.host.propose(Ok(HealProposal { diagnosis: "use POST".into(), patch: Some(Patch { node: "a".into(), ops: vec![PatchOp::SetConfig { path: "method".into(), value: json!("POST") }], reason: "x".into(), claimed_risk: "none".into(), touches: vec![] }), cents: 0 }));
    w.tick();
    let s = &w.book.list("fl")[0];
    assert_eq!(s.status, Status::Pending);
    assert!(s.asks.contains("no new external effects") || s.asks.contains("QA"), "{}", s.asks);
}

#[test]
fn a_proposal_that_touches_what_only_the_person_may_change_is_refused_and_nothing_is_stored() {
    let w = World::new("assist-refuse", copilot());
    w.run();
    // a patch for another step than the failing one
    w.e.host.propose(Ok(HealProposal { diagnosis: "x".into(), patch: Some(Patch { node: "e".into(), ops: vec![PatchOp::SetTimeout { ms: 5 }], reason: "".into(), claimed_risk: "".into(), touches: vec![] }), cents: 0 }));
    let out = w.tick();
    assert!(out[0].contains("another step"), "{out:?}");
    assert!(w.book.list("fl").is_empty());
    // a credential change is refused by the classifier
    let w = World::new("assist-refuse2", copilot());
    w.run();
    w.e.host.propose(Ok(HealProposal { diagnosis: "x".into(), patch: Some(Patch { node: "a".into(), ops: vec![PatchOp::SetConfig { path: "headers.Authorization".into(), value: json!("Bearer x") }], reason: "".into(), claimed_risk: "".into(), touches: vec![] }), cents: 0 }));
    let out = w.tick();
    assert!(w.book.list("fl").iter().all(|s| s.status != Status::AutoPublished), "{out:?}");
    // the model could not answer: no suggestion, an honest line
    let w = World::new("assist-refuse3", copilot());
    w.run();
    let out = w.tick();
    assert!(out[0].contains("could not look"), "{out:?}");
    assert!(w.book.list("fl").is_empty());
}

#[test]
fn a_failure_in_the_first_runs_of_a_published_fix_rolls_it_back_pauses_autopilot_and_asks() {
    let w = World::new("assist-rollback", autopilot(true));
    w.run();
    w.propose_url();
    w.tick();
    assert_eq!(w.published().meta.version, 2);
    // the site changes again: version 2 now fails
    w.e.host.on_http(|_| Ok(host::ok_response(500, "down")));
    let r = w.run();
    assert_eq!((r.status, r.flow.meta.version), (RunStatus::Failed, 2));
    w.e.host.notices.lock().unwrap().clear();
    w.e.host.propose(Ok(HealProposal { diagnosis: "again".into(), patch: None, cents: 0 }));
    let out = w.tick();
    assert!(out.iter().any(|l| l.contains("rolled back v2 to v1 as v3")), "{out:?}");
    // the content of v1 is published again, the history shows who did it
    assert_eq!(w.published().meta.version, 3);
    assert_eq!(w.published().node("a").unwrap().config["url"], "https://api.example.com/a");
    assert_eq!(w.e.store.versions("fl")[2].author, ASSIST_AGENT);
    assert_eq!(w.book.get("fl", "s1").unwrap().status, Status::RolledBack);
    assert!(w.book.watch("fl").is_none());
    // autopilot is paused: the cool-down makes it copilot, which asks
    assert!(w.book.cooling("fl"));
    let reg = w.e.engine.registry();
    let t = FakeTrial(TrialState::Passed("x".into()));
    assert_eq!(w.assistant(&reg, &t).mode_of("fl"), AssistMode::Copilot);
    let n = w.e.host.notices.lock().unwrap().clone();
    assert!(n.iter().any(|x| x.level == "warn" && x.needs_you && x.title.contains("rolled back")), "{n:?}");
    let a = w.e.host.audit_lines();
    assert!(a.iter().any(|l| l.starts_with("flow.assist.rollback fl v2 failed its first runs")), "{a:?}");
    // after the cool-down, autopilot is back
    w.e.clock.advance(25 * 3_600_000);
    assert!(!w.book.cooling("fl"));
    assert_eq!(w.assistant(&reg, &t).mode_of("fl"), AssistMode::Autopilot);
}

#[test]
fn a_cool_down_after_a_rollback_stops_automatic_publishing_and_a_pause_does_too() {
    let w = World::new("assist-cool", autopilot(true));
    w.book.start_cooldown("fl", 24).unwrap();
    w.run();
    w.propose_url();
    w.tick();
    assert_eq!(w.book.list("fl")[0].status, Status::Pending, "cooling: asks instead of publishing");
    assert_eq!(w.published().meta.version, 1);
    // a pause
    let w = World::new("assist-pause", autopilot(true));
    w.book.set_paused("fl", true).unwrap();
    w.run();
    w.propose_url();
    w.tick();
    assert_eq!(w.book.list("fl")[0].status, Status::Pending);
    assert!(w.book.paused("fl"));
    w.book.set_paused("fl", false).unwrap();
    assert!(!w.book.paused("fl"));
}

#[test]
fn a_published_fix_that_runs_well_ends_its_watch() {
    let w = World::new("assist-healthy", autopilot(true));
    w.run();
    w.propose_url();
    w.tick();
    assert!(w.book.watch("fl").is_some());
    w.run();
    w.tick();
    assert!(w.book.watch("fl").is_some(), "one good run of two so far");
    w.run();
    let out = w.tick();
    assert!(out.iter().any(|l| l.contains("healthy")), "{out:?}");
    assert!(w.book.watch("fl").is_none());
    assert_eq!(w.published().meta.version, 2, "no rollback");
}

#[test]
fn caps_per_flow_per_account_and_per_day_hold_and_reset() {
    // one publish per flow per day
    let w = World::new("assist-cap", autopilot(true));
    w.run();
    w.propose_url();
    w.tick();
    assert_eq!(w.published().meta.version, 2);
    w.e.host.on_http(|_| Ok(host::ok_response(500, "down")));
    // the watch must not roll this back for this test: end it, then fail again
    w.book.set_watch("fl", None).unwrap();
    w.run();
    w.e.host.propose(Ok(HealProposal { diagnosis: "again".into(), patch: Some(Patch { node: "a".into(), ops: vec![PatchOp::SetConfig { path: "url".into(), value: json!("https://api.example.com/v3/a") }], reason: "x".into(), claimed_risk: "".into(), touches: vec![] }), cents: 1 }));
    w.tick_with(&FakeTrial(TrialState::Passed("x".into())));
    let open = w.book.open_list("fl");
    assert_eq!(open.len(), 1, "the second change waits: {:?}", w.book.list("fl").iter().map(|s| (s.id.clone(), s.status, s.asks.clone())).collect::<Vec<_>>());
    assert!(open[0].asks.contains("publishes today"), "{}", open[0].asks);
    assert_eq!(w.published().meta.version, 2);
    // a day later the cap is free again
    w.e.clock.advance(25 * 3_600_000);
    assert_eq!(w.book.publishes_today("fl"), 0);
    // the account cap
    let w = World::new("assist-cap-all", autopilot(true));
    w.book.set_global_cap(0).unwrap();
    w.run();
    w.propose_url();
    w.tick();
    assert!(w.book.list("fl")[0].asks.contains("across flows"), "{}", w.book.list("fl")[0].asks);
    // the per-flow draft cap and the budget stop the model being asked at all
    let mut p = autopilot(true);
    p.max_drafts_per_day = 0;
    let w = World::new("assist-cap-drafts", p);
    w.run();
    let out = w.tick();
    assert!(out[0].contains("drafts today") && w.e.host.heal_requests.lock().unwrap().is_empty(), "{out:?}");
    let mut p = copilot();
    p.max_cents_per_day = 0;
    let w = World::new("assist-cap-budget", p);
    w.run();
    let out = w.tick();
    assert!(out[0].contains("cents spent today") && w.e.host.heal_requests.lock().unwrap().is_empty(), "{out:?}");
}

#[test]
fn a_fix_of_a_fix_is_bounded() {
    let w = World::new("assist-loop", autopilot(true));
    // two versions in a row by the agent, then a failing run of the second
    for v in 0..2 {
        let mut d = w.published();
        d.nodes[1].config["url"] = json!(format!("https://api.example.com/v{}/a", v + 5));
        d.meta.modified_by.push(Change { who: ASSIST_AGENT.into(), node: "a".into(), summary: "fix".into(), at_ms: 1, simple: true });
        w.e.store.save_draft(&d, None).unwrap();
        w.e.store.publish("fl", &PublishReq { by: ASSIST_AGENT, label: "", changelog: "auto", approved_by: Some(POLICY_APPROVER) }).unwrap();
    }
    w.e.host.on_http(|_| Ok(host::ok_response(500, "down")));
    let r = w.run();
    assert_eq!(r.flow.meta.version, 3);
    let reg = w.e.engine.registry();
    let t = FakeTrial(TrialState::Passed("x".into()));
    w.propose_url();
    let o = w.assistant(&reg, &t).suggest_for_run(&r, false).unwrap();
    assert!(matches!(&o, Outcome::Skipped(w) if w.contains("in a row")), "{o:?}");
    assert!(w.e.host.heal_requests.lock().unwrap().is_empty());
    // a person's publish ends the chain
    let mut d = w.published();
    d.description = "mine".into();
    w.e.store.save_draft(&d, None).unwrap();
    w.e.store.publish("fl", &PublishReq { by: "user:me", label: "", changelog: "mine", approved_by: Some("user:me") }).unwrap();
    assert_eq!(w.assistant(&reg, &t).agent_chain("fl"), 0);
}

#[test]
fn a_production_flow_is_never_fixed_by_itself() {
    let e = env("assist-prod");
    e.host.on_http(|_| Ok(host::ok_response(404, "gone")));
    let mut f = linear("fl");
    f.production = true;
    f.defaults.assist = autopilot(true);
    e.publish(f);
    let r = e.run("fl");
    let c = e.clock.clone();
    let book = AssistStore::from_fn(&e.dir.join("flows"), move || c.now_ms());
    let reg = e.engine.registry();
    let t = FakeTrial(TrialState::Passed("x".into()));
    let a = Assistant { store: &e.store, book: &book, host: &*e.host as &dyn Host, reg: &reg, trial: &t };
    assert!(matches!(a.suggest_for_run(&r, true).unwrap(), Outcome::Skipped(w) if w.contains("production")));
}

// ------------------------------------------------------------------------------------------------ upgrades

fn lib_node(url: &str) -> Node {
    Node::new("lib", "http", json!({"url": url, "extract": {"n": "count"}}))
}

fn upgrade_world(tag: &str, p: AssistPolicy) -> (World, Arc<FakeHost>) {
    let e = env(tag);
    e.store.lib_publish("check", &lib_node("https://api.example.com/v1"), "user:me").unwrap();
    e.store.lib_publish("check", &lib_node("https://api.example.com/v2"), "user:me").unwrap();
    let mut f = Flow::new("fl", "fl");
    f.nodes.push(Node::new("s", "start", json!({})));
    let mut n = Node::new("a", "http", json!({"url": "https://api.example.com/v1", "extract": {"n": "count"}}));
    n.lib = Some(LibRef { id: "check".into(), version: 1 });
    f.nodes.push(n);
    f.nodes.push(Node::new("e", "end", json!({})));
    f.edges = vec![Edge::new("s", "a"), Edge::new("a", "e")];
    f.defaults.assist = p;
    e.publish(f);
    let c = e.clock.clone();
    let book = AssistStore::from_fn(&e.dir.join("flows"), move || c.now_ms());
    let host = e.host.clone();
    (World { e, book }, host)
}

#[test]
fn a_newer_compatible_library_step_becomes_an_upgrade_suggestion_that_is_never_published_by_itself() {
    let mut p = autopilot(true);
    p.allow_upgrade = true;
    let (w, _h) = upgrade_world("assist-up", p);
    let reg = w.e.engine.registry();
    let t = FakeTrial(TrialState::Passed("x".into()));
    let out = w.assistant(&reg, &t).suggest_upgrades("fl").unwrap();
    assert!(out[0].contains("suggestion s1"), "{out:?}");
    let s = &w.book.list("fl")[0];
    assert_eq!((s.class, s.status), (Class::Upgrade, Status::Pending));
    assert!(s.diff.iter().any(|l| l.contains("lib.version") && l.contains("1 -> 2")), "{:?}", s.diff);
    assert!(s.asks.contains("upgrade is published only after"), "{}", s.asks);
    assert_eq!(w.published().meta.version, 1, "no automatic publish");
    // asking again does not pile up another
    let out = w.assistant(&reg, &t).suggest_upgrades("fl").unwrap();
    assert!(out.is_empty(), "{out:?}");
    assert_eq!(w.book.list("fl").len(), 1);
    // the person can approve it like any suggestion
    let a = w.assistant(&reg, &t);
    a.adopt("fl", "s1").unwrap();
    assert_eq!(w.e.store.get_draft("fl").unwrap().node("a").unwrap().lib.as_ref().unwrap().version, 2);
}

#[test]
fn upgrades_wait_for_the_switch_and_an_incompatible_one_is_only_reported() {
    // class not switched on: found, reported, nothing made
    let (w, _) = upgrade_world("assist-up-off", copilot());
    let reg = w.e.engine.registry();
    let t = FakeTrial(TrialState::Passed("x".into()));
    let out = w.assistant(&reg, &t).suggest_upgrades("fl").unwrap();
    assert!(out[0].contains("not switched on") && w.book.list("fl").is_empty(), "{out:?}");
    // off: nothing
    let (w, _) = upgrade_world("assist-up-off2", AssistPolicy::default());
    let out = w.assistant(&reg, &t).suggest_upgrades("fl").unwrap();
    assert!(out[0].contains("off"));
    // an incompatible newer version (other output names)
    let (w, _) = upgrade_world("assist-up-incompat", AssistPolicy { mode: AssistMode::Copilot, allow_upgrade: true, ..Default::default() });
    w.e.store.lib_publish("check", &Node::new("lib", "http", json!({"url": "https://api.example.com/v3", "extract": {"other": "x"}})), "user:me").unwrap();
    let out = w.assistant(&reg, &t).suggest_upgrades("fl").unwrap();
    assert!(out[0].contains("changes its ports or outputs"), "{out:?}");
    assert!(w.book.list("fl").is_empty());
}

// ------------------------------------------------------------------------------------------------ the real trial

#[test]
fn the_real_trial_runs_read_only_steps_and_refuses_the_rest() {
    let e = env("assist-trial");
    e.host.on_http(|req| if req.url.contains("/v2/") { ok_json("{}") } else { Ok(host::ok_response(404, "gone")) });
    let reg = e.engine.registry();
    let t = EngineTrial { engine: &e.engine, reg: &reg };
    let mut f = linear("fl");
    e.publish(f.clone());
    let run = e.run("fl");
    // a changed read-only step that now works
    f.nodes[1].config["url"] = json!("https://api.example.com/v2/a");
    assert!(matches!(t.trial(&f, &run, &["a".to_string()]), TrialState::Passed(_)));
    // one that still fails
    f.nodes[1].config["url"] = json!("https://api.example.com/other");
    assert!(matches!(t.trial(&f, &run, &["a".to_string()]), TrialState::Failed(w) if w.contains("a")));
    // nothing changed: nothing was tried
    assert!(matches!(t.trial(&f, &run, &[]), TrialState::NotRun(_)));
    // a write is never tried, and neither is a browser, a camera or a message
    f.nodes[1].config = json!({"url": "https://api.example.com/v2/a", "method": "POST"});
    let calls = e.host.http_calls.lock().unwrap().len();
    e.host.notices.lock().unwrap().clear();
    assert!(matches!(t.trial(&f, &run, &["a".to_string()]), TrialState::NotRun(_)));
    f.nodes.push(Node::new("n", "notify", json!({"title": "x"})));
    f.nodes.push(Node::new("c", "cam_photo", json!({})));
    f.nodes.push(Node::new("b", "br_open", json!({"url": "https://x.example"})));
    for id in ["n", "c", "b"] {
        assert!(matches!(t.trial(&f, &run, &[id.to_string()]), TrialState::NotRun(w) if w.contains("never run")), "{id}");
    }
    assert_eq!(e.host.http_calls.lock().unwrap().len(), calls, "no real call was made for any of them");
    assert!(e.host.notices.lock().unwrap().is_empty() && e.host.cam_calls.lock().unwrap().is_empty() && e.host.browser_ops.lock().unwrap().is_empty());
}

#[test]
fn the_files_hold_no_input_value_and_no_secret() {
    let w = World::new("assist-files", autopilot(true));
    let token = format!("{}{}", "ghp_", "Z9y8X7w6V5u4T3s2R1q0P9o8N7m6");
    let o = w.e.engine.start_run(StartRequest::new("fl").inputs(json!({"api": token}))).unwrap();
    w.e.finish(&o.run_id);
    w.e.host.propose(Ok(HealProposal { diagnosis: format!("refused {token}"), patch: Some(Patch { node: "a".into(), ops: vec![PatchOp::SetConfig { path: "url".into(), value: json!("https://api.example.com/v2/a") }], reason: "x".into(), claimed_risk: "".into(), touches: vec![] }), cents: 0 }));
    w.tick();
    fn walk(p: &std::path::Path, out: &mut String) {
        if p.is_dir() {
            for e in std::fs::read_dir(p).unwrap().flatten() {
                if e.file_name() != "runs" {
                    walk(&e.path(), out);
                }
            }
        } else if let Ok(t) = std::fs::read_to_string(p) {
            out.push_str(&t);
        }
    }
    let mut all = String::new();
    walk(&w.e.dir.join("flows"), &mut all);
    assert!(all.contains("flow.json") || all.contains("suggestions") || all.contains("proposed"));
    assert!(!all.contains(&token), "no input value is written by the assist files");
}

#[allow(dead_code)]
fn _unused(_: Arc<FakeHost>) {}
