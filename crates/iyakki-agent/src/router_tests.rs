//! Tests of the model router with FAKE models: scripted planner and executor replies. No real model was run by any of these.

use std::collections::{BTreeMap, VecDeque};
use std::sync::{Arc, Mutex};

use serde_json::{json, Value};

use crate::agent::{hard_ask, Risk, Tools};
use crate::chat::{ChatBackend, Message};
use crate::providers::{Kind, ProviderCfg};
use crate::router::*;
use crate::router_js::JsStrategy;
use crate::router_registry::{build, Caps, Cost, Profile, ProfileOverride, Role, Zone};
use crate::routertools::{self as rt, Backend, RouterTools};

/// A model that answers from a script; when the script runs out it repeats `rest`. It remembers what it was asked.
struct Scripted {
    replies: Mutex<VecDeque<Result<String, String>>>,
    rest: String,
    seen: Mutex<Vec<String>>,
    /// How many pictures the last message of each call carried, and the system prompt of each call.
    pictures: Mutex<Vec<usize>>,
    systems: Mutex<Vec<String>>,
}

impl Scripted {
    fn new(replies: &[&str], rest: &str) -> Arc<Scripted> {
        Arc::new(Scripted { replies: Mutex::new(replies.iter().map(|s| Ok(s.to_string())).collect()), rest: rest.into(), seen: Mutex::new(vec![]), pictures: Mutex::new(vec![]), systems: Mutex::new(vec![]) })
    }
    fn asked(&self) -> Vec<String> {
        self.seen.lock().unwrap().clone()
    }
}

impl ChatBackend for Scripted {
    fn complete(&self, history: &[Message]) -> Result<String, String> {
        self.seen.lock().unwrap().push(history.last().map(|m| m.content.clone()).unwrap_or_default());
        self.pictures.lock().unwrap().push(history.last().map_or(0, |m| m.images.len()));
        self.systems.lock().unwrap().push(history.first().map(|m| m.content.clone()).unwrap_or_default());
        self.replies.lock().unwrap().pop_front().unwrap_or_else(|| Ok(self.rest.clone()))
    }
}

struct Pool(BTreeMap<String, Arc<Scripted>>);

impl ModelPool for Pool {
    fn backend(&self, id: &str) -> Option<Arc<dyn ChatBackend>> {
        self.0.get(id).map(|b| b.clone() as Arc<dyn ChatBackend>)
    }
}

fn prof(id: &str, role: Role, cost: Cost, zone: Zone) -> Profile {
    Profile { id: id.into(), provider: "t".into(), model: id.into(), role, caps: Caps { tools: true, vision: false, audio: false, context: 8192 }, cost, zone, local: zone != Zone::Internet, enabled: true }
}

fn big() -> Profile {
    prof("big", Role::Both, Cost::High, Zone::Computer)
}
fn small() -> Profile {
    prof("small", Role::Executor, Cost::Free, Zone::Computer)
}

const DONE1: &str = r#"{"status":"done","result":"result one"}"#;
const FAILED: &str = r#"{"status":"failed","error":"the test still fails"}"#;

fn plan_json(steps: &[(&str, &[&str])]) -> String {
    let v: Vec<Value> = steps.iter().map(|(id, after)| json!({"id": id, "title": format!("title {id}"), "task": format!("do {id}"), "after": after, "check": "it works"})).collect();
    json!({"goal": "g", "steps": v}).to_string()
}

type Log = Arc<Mutex<Vec<String>>>;

fn make(cfg: RouterConfig, models: &[Profile], pool: Vec<(&str, Arc<Scripted>)>, strategy: Arc<dyn Strategy>) -> (Router, Log) {
    let log: Log = Arc::default();
    let l2 = log.clone();
    let audit: Arc<dyn Fn(&str, &str) + Send + Sync> = Arc::new(move |w, d| l2.lock().unwrap().push(format!("{w} {d}")));
    let pool = Arc::new(Pool(pool.into_iter().map(|(k, v)| (k.to_string(), v)).collect()));
    (Router::new(cfg, models, pool, strategy, audit), log)
}

fn on() -> RouterConfig {
    RouterConfig { enabled: true, ..RouterConfig::default() }
}

// ---------------------------------------------------------------------------------------------- the registry

#[test]
fn the_registry_is_built_from_the_provider_settings_and_a_correction_wins() {
    let mut local = ProviderCfg::new("local", Kind::Ollama);
    local.models = vec!["qwen:7b".into(), "bge-embed".into()];
    local.model_kinds.insert("bge-embed".into(), "embeddings".into());
    let mut cloud = ProviderCfg::new("cloud", Kind::OpenAi);
    cloud.models = vec!["gpt-large".into(), "gpt-mini".into()];
    let mut off = ProviderCfg::new("old", Kind::OpenRouter);
    off.models = vec!["x".into()];
    off.enabled = false;
    let mut over = BTreeMap::new();
    let all = build(&[local.clone(), cloud.clone(), off.clone()], &over);
    let get = |id: &str| all.iter().find(|p| p.id == id).unwrap().clone();
    assert!(all.iter().all(|p| p.id != "local/bge-embed"), "a model the server labelled as embeddings is not a chat model");
    let q = get("local/qwen:7b");
    assert_eq!((q.role, q.cost, q.zone, q.local), (Role::Executor, Cost::Free, Zone::Computer, true));
    let l = get("cloud/gpt-large");
    assert_eq!((l.role, l.cost, l.zone, l.local), (Role::Both, Cost::High, Zone::Internet, false));
    let m = get("cloud/gpt-mini");
    assert_eq!((m.role, m.cost), (Role::Executor, Cost::Low));
    assert!(!get("old/x").enabled, "a provider that is off gives models that are off");
    over.insert("local/qwen:7b".to_string(), ProfileOverride { role: Some(Role::Both), context: Some(32768), vision: Some(true), ..Default::default() });
    let q2 = build(&[local], &over).into_iter().find(|p| p.id == "local/qwen:7b").unwrap();
    assert_eq!((q2.role, q2.caps.context, q2.caps.vision), (Role::Both, 32768, true));
    let w = serde_json::to_string(&q2).unwrap();
    assert!(!w.contains("key") && !w.contains("http"), "a profile carries no key and no address: {w}");
}

#[test]
fn a_network_server_is_on_the_network_zone_and_an_unreadable_address_counts_as_the_internet() {
    let mut p = ProviderCfg::new("lan", Kind::Compat);
    p.base_url = Some("http://192.168.1.20:8080/v1".into());
    p.models = vec!["m".into()];
    assert_eq!(build(&[p.clone()], &BTreeMap::new())[0].zone, Zone::Network);
    p.base_url = Some("nonsense".into());
    assert_eq!(build(&[p], &BTreeMap::new())[0].zone, Zone::Internet);
}

// ---------------------------------------------------------------------------------------------- the plan

#[test]
fn a_plan_is_checked_for_ids_dependencies_and_cycles_and_may_sit_in_a_code_fence() {
    let ok = format!("Here you go:\n```json\n{}\n```", plan_json(&[("a", &[]), ("b", &["a"])]));
    assert_eq!(Plan::parse(&ok).unwrap().steps.len(), 2);
    assert!(Plan::parse(&plan_json(&[("a", &["b"]), ("b", &["a"])])).unwrap_err().contains("circle"));
    assert!(Plan::parse(&plan_json(&[("a", &["a"])])).unwrap_err().contains("itself"));
    assert!(Plan::parse(&plan_json(&[("a", &["zz"])])).unwrap_err().contains("not a step"));
    assert!(Plan::parse(&plan_json(&[("a", &[]), ("a", &[])])).unwrap_err().contains("twice"));
    assert!(Plan::parse(&plan_json(&[("A B", &[])])).unwrap_err().contains("step id"));
    assert!(Plan::parse("no json here").is_err());
    assert!(Plan::parse("{\"steps\": []}").unwrap_err().contains("no steps"));
}

#[test]
fn a_plan_with_dependencies_runs_in_order_on_the_cheapest_able_model_and_the_planner_only_plans() {
    let planner = Scripted::new(&[&plan_json(&[("s1", &[]), ("s2", &["s1"]), ("s3", &["s1", "s2"])])], "");
    let exec = Scripted::new(&[DONE1, r#"{"status":"done","result":"result two"}"#, r#"{"status":"done","result":"result three"}"#], "");
    let (mut r, log) = make(on(), &[big(), small()], vec![("big", planner.clone()), ("small", exec.clone())], Arc::new(DefaultStrategy));
    let rep = r.run("SECRET-GOAL-TEXT build the thing");
    assert_eq!(rep.outcome, RunOutcome::Finished, "{}", rep.summary());
    let asked = exec.asked();
    assert_eq!(asked.len(), 3);
    assert!(asked[0].starts_with("Step s1"), "{}", asked[0]);
    assert!(asked[1].starts_with("Step s2") && asked[1].contains("s1: result one"), "{}", asked[1]);
    assert!(asked[2].contains("s1: result one") && asked[2].contains("s2: result two"), "{}", asked[2]);
    assert_eq!(planner.asked().len(), 1, "the bigger model only planned");
    assert!(rep.states.values().all(|s| s.status == Status::Done && s.model.as_deref() == Some("small")));
    let lines = log.lock().unwrap().clone();
    assert!(lines.iter().any(|l| l == "router.plan planner=big steps=3"), "{lines:?}");
    assert!(lines.iter().any(|l| l == "router.assign step=s2 model=small"), "{lines:?}");
    assert!(lines.first().unwrap().starts_with("router.run.start") && lines.last().unwrap().starts_with("router.run.end outcome=finished"), "{lines:?}");
    assert!(lines.iter().all(|l| !l.contains("SECRET") && !l.contains("result one")), "no content in the access log: {lines:?}");
    assert_eq!(rep.meter.calls, 4);
    assert_eq!(rep.meter.per_model["small"].2, 3);
}

#[test]
fn a_planner_that_answers_nonsense_is_asked_once_more_and_then_the_run_has_no_plan() {
    let planner = Scripted::new(&["no idea", "still none"], "");
    let (mut r, _) = make(on(), &[big(), small()], vec![("big", planner.clone()), ("small", Scripted::new(&[], DONE1))], Arc::new(DefaultStrategy));
    let rep = r.run("g");
    assert!(matches!(rep.outcome, RunOutcome::NoPlan(_)));
    assert_eq!(planner.asked().len(), 2);
    assert!(planner.asked()[1].contains("refused"));
}

#[test]
fn a_step_that_needs_vision_goes_to_a_model_that_has_it_and_with_none_it_waits_for_a_person_and_its_dependents_are_skipped() {
    let mut eye = prof("eye", Role::Executor, Cost::Medium, Zone::Computer);
    eye.caps.vision = true;
    let plan = Plan { goal: "g".into(), steps: vec![
        Step { id: "look".into(), title: "".into(), task: "look at the picture".into(), after: vec![], role: Role::Executor, needs: Needs { vision: true, ..Default::default() }, check: "".into() },
        Step { id: "then".into(), title: "".into(), task: "use it".into(), after: vec!["look".into()], role: Role::Executor, needs: Needs::default(), check: "".into() },
    ] };
    let s = Scripted::new(&[], DONE1);
    let e = Scripted::new(&[], DONE1);
    let (mut r, _) = make(on(), &[big(), small(), eye.clone()], vec![("small", s.clone()), ("eye", e.clone()), ("big", Scripted::new(&[], ""))], Arc::new(DefaultStrategy));
    let rep = r.run_plan("g", plan.clone(), None);
    assert_eq!(rep.outcome, RunOutcome::Finished);
    assert_eq!(rep.states["look"].model.as_deref(), Some("eye"), "the cheaper model has no vision");
    assert_eq!(rep.states["then"].model.as_deref(), Some("small"));
    let (mut r, log) = make(on(), &[big(), small()], vec![("small", s), ("big", Scripted::new(&[], ""))], Arc::new(DefaultStrategy));
    let rep = r.run_plan("g", plan, None);
    assert_eq!(rep.outcome, RunOutcome::NeedsPerson(vec!["look".into()]));
    assert_eq!(rep.states["then"].status, Status::Skipped);
    assert!(log.lock().unwrap().iter().any(|l| l == "router.person step=look why=strategy"));
}

// ---------------------------------------------------------------------------------------------- escalation

#[test]
fn an_executor_that_fails_twice_asks_the_planner_gets_a_suggestion_and_then_succeeds() {
    let planner = Scripted::new(&[&plan_json(&[("s1", &[])]), "use the other flag"], "");
    let exec = Scripted::new(&[FAILED, FAILED, DONE1], "");
    let (mut r, log) = make(on(), &[big(), small()], vec![("big", planner.clone()), ("small", exec.clone())], Arc::new(DefaultStrategy));
    let rep = r.run("g");
    assert_eq!(rep.outcome, RunOutcome::Finished, "{}", rep.summary());
    let st = &rep.states["s1"];
    assert_eq!((st.attempts, st.escalations, st.status), (3, 1, Status::Done));
    assert!(exec.asked()[2].contains("Advice from the planner: use the other flag"));
    let q = &planner.asked()[1];
    assert!(q.contains("the test still fails") && q.contains("tried 2 time(s)"), "the planner is told the step, the error and the tries: {q}");
    let lines = log.lock().unwrap().clone();
    assert!(lines.iter().any(|l| l == "router.escalate step=s1 to=big round=1"), "{lines:?}");
    assert!(rep.log.iter().any(|l| l.contains("Asking big about step s1")));
}

#[test]
fn a_blocked_executor_asks_the_planner_at_once_and_a_revised_step_replaces_the_task() {
    let planner = Scripted::new(&[&plan_json(&[("s1", &[])]), r#"{"advice":"read the manual first","task":"do s1 the new way"}"#], "");
    let exec = Scripted::new(&[r#"{"status":"blocked","error":"I do not know the tool"}"#, DONE1], "");
    let (mut r, _) = make(on(), &[big(), small()], vec![("big", planner), ("small", exec.clone())], Arc::new(DefaultStrategy));
    let rep = r.run("g");
    assert_eq!(rep.outcome, RunOutcome::Finished);
    assert_eq!(rep.states["s1"].attempts, 2);
    assert!(exec.asked()[1].starts_with("Step s1: do s1 the new way") && exec.asked()[1].contains("read the manual first"), "{}", exec.asked()[1]);
}

#[test]
fn the_retry_bounds_stop_a_runaway_and_hand_the_step_to_a_person() {
    // 4 tries, one round with the planner in the middle
    let planner = Scripted::new(&[&plan_json(&[("s1", &[])])], "try harder");
    let exec = Scripted::new(&[], FAILED);
    let (mut r, log) = make(on(), &[big(), small()], vec![("big", planner.clone()), ("small", exec.clone())], Arc::new(DefaultStrategy));
    let rep = r.run("g");
    assert_eq!(rep.outcome, RunOutcome::NeedsPerson(vec!["s1".into()]));
    assert_eq!(exec.asked().len(), 4, "the default allows 4 tries");
    assert_eq!(planner.asked().len(), 2, "the plan and one round of advice");
    assert!(log.lock().unwrap().iter().any(|l| l == "router.person step=s1 why=attempts"));
    // fewer rounds with the planner than the tries would call for
    let cfg = RouterConfig { budget: Budget { max_attempts: 10, max_escalations: 1, ..Budget::default() }, ..on() };
    let planner = Scripted::new(&[&plan_json(&[("s1", &[])])], "try harder");
    let exec = Scripted::new(&[], FAILED);
    let (mut r, log) = make(cfg, &[big(), small()], vec![("big", planner.clone()), ("small", exec.clone())], Arc::new(DefaultStrategy));
    let rep = r.run("g");
    assert_eq!(rep.outcome, RunOutcome::NeedsPerson(vec!["s1".into()]));
    assert_eq!(planner.asked().len(), 2, "the planner is asked once for the step");
    assert_eq!(exec.asked().len(), 4);
    assert!(log.lock().unwrap().iter().any(|l| l == "router.person step=s1 why=rounds"));
}

#[test]
fn an_executor_that_does_not_answer_in_the_agreed_form_or_fails_the_check_is_a_failed_try() {
    struct Refuse;
    impl Verifier for Refuse {
        fn verify(&self, _: &Step, _: &str) -> Result<(), String> {
            Err("the tests did not pass".into())
        }
    }
    let planner = Scripted::new(&[&plan_json(&[("s1", &[])])], "advice");
    let exec = Scripted::new(&["I did it!"], FAILED);
    let (mut r, _) = make(on(), &[big(), small()], vec![("big", planner.clone()), ("small", exec.clone())], Arc::new(DefaultStrategy));
    let rep = r.run("g");
    assert_eq!(rep.states["s1"].attempts, 4, "the first answer was a failed try, not a success");
    let planner = Scripted::new(&[&plan_json(&[("s1", &[])])], "advice");
    let exec = Scripted::new(&[], DONE1);
    let (r, _) = make(on(), &[big(), small()], vec![("big", planner), ("small", exec)], Arc::new(DefaultStrategy));
    let mut r = r.with_verifier(Arc::new(Refuse));
    let rep = r.run("g");
    assert_eq!(rep.outcome, RunOutcome::NeedsPerson(vec!["s1".into()]));
    assert!(rep.states["s1"].last_error.as_deref().unwrap().contains("the check refused it"));
}

// ---------------------------------------------------------------------------------------------- the budget

#[test]
fn the_token_and_call_budgets_stop_the_run_before_the_next_call() {
    let steps = plan_json(&[("s1", &[]), ("s2", &["s1"])]);
    let cfg = RouterConfig { budget: Budget { max_tokens: 20, ..Budget::default() }, ..on() };
    let exec = Scripted::new(&[], DONE1);
    let (mut r, log) = make(cfg, &[big(), small()], vec![("big", Scripted::new(&[&steps], "")), ("small", exec.clone())], Arc::new(DefaultStrategy));
    let rep = r.run("g");
    assert!(matches!(rep.outcome, RunOutcome::BudgetStop(_)), "{:?}", rep.outcome);
    assert!(exec.asked().is_empty(), "no executor call after the plan used the budget");
    assert!(log.lock().unwrap().iter().any(|l| l.starts_with("router.budget tokens=")));
    let cfg = RouterConfig { budget: Budget { max_calls: 2, ..Budget::default() }, ..on() };
    let exec = Scripted::new(&[], DONE1);
    let (mut r, _) = make(cfg, &[big(), small()], vec![("big", Scripted::new(&[&steps], "")), ("small", exec.clone())], Arc::new(DefaultStrategy));
    let rep = r.run("g");
    assert!(matches!(rep.outcome, RunOutcome::BudgetStop(_)));
    assert_eq!(rep.meter.calls, 2);
    assert_eq!(rep.states["s1"].status, Status::Done);
    assert_eq!(rep.states["s2"].status, Status::Pending);
    assert!(rep.log.iter().any(|l| l.starts_with("Stopped: the call budget (2)")));
}

#[test]
fn a_budget_above_the_hard_caps_is_refused() {
    assert!(Budget { max_tokens: HARD_TOKENS + 1, ..Budget::default() }.validate().is_err());
    assert!(Budget { max_attempts: 0, ..Budget::default() }.validate().is_err());
    assert!(Budget::default().validate().is_ok());
}

#[test]
fn a_strategy_cannot_raise_the_rounds_or_tries_the_core_allows() {
    // a strategy that always asks the planner and never gives up
    let always = StrategyFn::new("eager", |i| {
        let phase = i["phase"].as_str().unwrap_or("");
        let models = i["models"].as_array().unwrap();
        let big = models.iter().find(|m| m["role"] == "both").unwrap()["id"].clone();
        let small = models.iter().find(|m| m["role"] == "executor").unwrap()["id"].clone();
        Ok(match phase {
            "plan" => json!({"v": 1, "planner": big}),
            "assign" => json!({"v": 1, "assign": [{"step": i["ready"][0], "model": small}]}),
            _ => json!({"v": 1, "escalate": {"step": i["blocked"]["step"], "model": big}}),
        })
    });
    let planner = Scripted::new(&[&plan_json(&[("s1", &[])])], "try");
    let exec = Scripted::new(&[], FAILED);
    let (mut r, _) = make(on(), &[big(), small()], vec![("big", planner.clone()), ("small", exec.clone())], Arc::new(always));
    let rep = r.run("g");
    assert_eq!(rep.outcome, RunOutcome::NeedsPerson(vec!["s1".into()]));
    assert!(exec.asked().len() <= 4 && planner.asked().len() <= 3, "tries {} planner calls {}", exec.asked().len(), planner.asked().len());
}

/// A strategy made of a Rust closure, to try the contract without a script.
struct StrategyFn {
    name: String,
    f: Box<dyn Fn(&Value) -> Result<Value, String> + Send + Sync>,
}

impl StrategyFn {
    fn new(name: &str, f: impl Fn(&Value) -> Result<Value, String> + Send + Sync + 'static) -> StrategyFn {
        StrategyFn { name: name.into(), f: Box::new(f) }
    }
}

impl Strategy for StrategyFn {
    fn name(&self) -> String {
        self.name.clone()
    }
    fn decide(&self, i: &Value) -> Result<Value, String> {
        (self.f)(i)
    }
}

// ---------------------------------------------------------------------------------------------- the strategy plugin

fn one_step_run(strategy: Arc<dyn Strategy>, models: &[Profile]) -> (RunReport, Log, Arc<Scripted>, Arc<Scripted>) {
    let planner = Scripted::new(&[&plan_json(&[("s1", &[])])], DONE1);
    let exec = Scripted::new(&[], DONE1);
    let (mut r, log) = make(on(), models, vec![("big", planner.clone()), ("small", exec.clone()), ("cloud", exec.clone())], strategy);
    (r.run("g"), log, planner, exec)
}

#[test]
fn a_script_that_loops_forever_is_stopped_and_the_default_takes_over_with_one_plain_line() {
    let js = JsStrategy::new("spin", "function decide(i) { while (true) {} }").unwrap();
    let t0 = std::time::Instant::now();
    let (rep, log, _, exec) = one_step_run(Arc::new(js), &[big(), small()]);
    assert!(t0.elapsed() < std::time::Duration::from_secs(20), "the time limit held");
    assert_eq!(rep.outcome, RunOutcome::Finished, "the default strategy carried the run: {}", rep.summary());
    assert_eq!(exec.asked().len(), 1);
    let lines: Vec<_> = rep.log.iter().filter(|l| l.contains("was stopped")).collect();
    assert_eq!(lines.len(), 1, "{:?}", rep.log);
    assert!(lines[0].contains("spin") && lines[0].contains("timeout") && lines[0].contains("default strategy takes over"), "{}", lines[0]);
    assert_eq!(rep.strategy, "default");
    assert!(log.lock().unwrap().iter().any(|l| l == "router.fallback strategy=spin why=timeout"));
}

#[test]
fn a_script_that_answers_garbage_throws_or_asks_for_a_forbidden_model_falls_back_with_the_reason_category() {
    let cases: [(&str, &str, &str); 7] = [
        ("hello", "function decide(i) { return 'hello'; }", "garbage"),
        ("undef", "function decide(i) { }", "garbage"),
        ("wrongv", "function decide(i) { return {v: 7, planner: 'big'}; }", "garbage"),
        ("throws", "function decide(i) { throw new Error('boom'); }", "error"),
        ("nodecide", "var x = 1;", "error"),
        ("ghost", "function decide(i) { return {v: 1, planner: 'ghost'}; }", "refused"),
        ("huge", "function decide(i) { return {v: 1, stop: 'x'.repeat(40000)}; }", "size"),
    ];
    for (name, src, want) in cases {
        let js = JsStrategy::new(name, src).unwrap();
        let (rep, log, _, _) = one_step_run(Arc::new(js), &[big(), small()]);
        assert_eq!(rep.outcome, RunOutcome::Finished, "{name}: {}", rep.summary());
        let lines: Vec<_> = rep.log.iter().filter(|l| l.contains("was stopped")).collect();
        assert_eq!(lines.len(), 1, "{name}: {:?}", rep.log);
        assert!(lines[0].contains(&format!("({want})")), "{name}: {}", lines[0]);
        assert!(log.lock().unwrap().iter().any(|l| l == &format!("router.fallback strategy={name} why={want}")), "{name}");
    }
}

#[test]
fn a_script_that_eats_memory_is_stopped_by_the_sandbox() {
    let js = JsStrategy::new("hog", "function decide(i) { var a = []; while (true) { a.push(new Array(1000000).fill(1)); } }").unwrap();
    let e = js.decide(&sample_input()).unwrap_err();
    assert!(e.starts_with("error") || e.starts_with("timeout"), "{e}");
}

#[test]
fn a_script_sees_only_the_language_no_files_network_programs_storage_or_timers() {
    let src = "function decide(i) { return {v: 1, stop: [typeof fetch, typeof XMLHttpRequest, typeof require, typeof process, typeof localStorage, typeof setTimeout, typeof console, typeof document, typeof WebAssembly, typeof std, typeof os].join(',')}; }";
    let out = JsStrategy::new("probe", src).unwrap().decide(&sample_input()).unwrap();
    let stop = out["stop"].as_str().unwrap();
    assert!(stop.split(',').all(|t| t == "undefined"), "the sandbox has nothing but the language: {stop}");
}

#[test]
fn a_working_script_decides_and_a_new_call_remembers_nothing_of_the_last() {
    let src = "var calls = 0; function decide(i) { calls++; if (i.phase === 'plan') return {v: 1, planner: i.models[0].id}; var ready = i.ready[0]; var best = i.models.filter(function (m) { return m.role !== 'planner'; }).sort(function (a, b) { return a.id < b.id ? -1 : 1; })[0]; return {v: 1, assign: [{step: ready, model: best.id}], stop: calls > 1 ? 'state leaked' : undefined}; }";
    let js = JsStrategy::new("alpha", src).unwrap();
    // the models in order: big, small: the script picks the first by id, which is `big`
    let (rep, _, planner, exec) = one_step_run(Arc::new(js), &[big(), small()]);
    assert_eq!(rep.outcome, RunOutcome::Finished, "{}", rep.summary());
    assert_eq!(rep.strategy, "alpha", "the script was not stopped");
    assert_eq!(rep.states["s1"].model.as_deref(), Some("big"));
    assert_eq!(planner.asked().len(), 2, "the plan, then the step the script gave to the bigger model");
    assert!(exec.asked().is_empty());
}

#[test]
fn a_model_outside_the_privacy_zone_is_never_offered_and_a_script_that_picks_it_is_refused() {
    let cloud = prof("cloud", Role::Executor, Cost::Free, Zone::Internet);
    let cfg = RouterConfig { max_zone: Zone::Network, ..on() };
    let seen: Arc<Mutex<Vec<String>>> = Arc::default();
    let s2 = seen.clone();
    let greedy = StrategyFn::new("greedy", move |i| {
        *s2.lock().unwrap() = i["models"].as_array().unwrap().iter().map(|m| m["id"].as_str().unwrap().to_string()).collect();
        Ok(match i["phase"].as_str().unwrap() {
            "plan" => json!({"v": 1, "planner": "big"}),
            _ => json!({"v": 1, "assign": [{"step": i["ready"][0], "model": "cloud"}]}),
        })
    });
    let planner = Scripted::new(&[&plan_json(&[("s1", &[])])], "");
    let local = Scripted::new(&[], DONE1);
    let remote = Scripted::new(&[], DONE1);
    let (mut r, log) = make(cfg, &[big(), small(), cloud], vec![("big", planner), ("small", local.clone()), ("cloud", remote.clone())], Arc::new(greedy));
    let rep = r.run("g");
    assert_eq!(rep.outcome, RunOutcome::Finished);
    assert!(!seen.lock().unwrap().contains(&"cloud".to_string()), "the strategy never saw the model outside the zone: {:?}", seen.lock().unwrap());
    assert!(remote.asked().is_empty(), "the internet model was never called");
    assert_eq!(local.asked().len(), 1);
    assert!(log.lock().unwrap().iter().any(|l| l == "router.fallback strategy=greedy why=refused"));
}

#[test]
fn a_disabled_model_is_not_offered_and_with_no_planner_the_run_says_so() {
    let mut off = big();
    off.enabled = false;
    let (mut r, _) = make(on(), &[off, small()], vec![("small", Scripted::new(&[], DONE1))], Arc::new(DefaultStrategy));
    match r.run("g").outcome {
        RunOutcome::NoPlan(e) => assert!(e.contains("no allowed model can plan"), "{e}"),
        o => panic!("{o:?}"),
    }
}

#[test]
fn the_default_strategy_answers_inside_the_contract_for_every_phase() {
    let i: DecideInput = serde_json::from_value(sample_input()).unwrap();
    try_strategy(&DefaultStrategy).unwrap();
    let mut p = i.clone();
    p.phase = "plan".into();
    let d: Decision = serde_json::from_value(DefaultStrategy.decide(&json!(p)).unwrap()).unwrap();
    assert_eq!(d.planner.as_deref(), Some("big"));
    let mut e = i;
    e.phase = "escalate".into();
    e.blocked = Some(Blocked { step: "s1".into(), kind: "failed".into(), error: "x".into(), attempts: 1, escalations: 0, since_advice: 1 });
    let d: Decision = serde_json::from_value(DefaultStrategy.decide(&json!(e.clone())).unwrap()).unwrap();
    assert!(d.escalate.is_none(), "one failure is retried");
    e.blocked.as_mut().unwrap().since_advice = 2;
    let d: Decision = serde_json::from_value(DefaultStrategy.decide(&json!(e)).unwrap()).unwrap();
    assert_eq!(d.escalate.unwrap().model, "big", "the second failure asks the planner");
}

// ---------------------------------------------------------------------------------------------- the ledger

#[test]
fn every_call_of_a_run_is_written_to_the_usage_ledger_under_the_router_source() {
    let dir = crate::testdir::outside_project("router", "ledger");
    let log = Arc::new(crate::usage::UsageLog::open(&dir.join("usage.jsonl")));
    let planner = Scripted::new(&[&plan_json(&[("s1", &[])])], "");
    let exec = Scripted::new(&[], DONE1);
    let (r, _) = make(on(), &[big(), small()], vec![("big", planner), ("small", exec)], Arc::new(DefaultStrategy));
    let mut r = r.with_sink(Arc::new(LedgerSink { log: log.clone(), user: "ann".into() }));
    let rep = r.run("g");
    let e = log.entries(Some("ann"));
    assert_eq!(e.iter().map(|x| x.calls).sum::<u64>(), 2);
    assert!(e.iter().all(|x| x.src == SRC_ROUTER));
    assert_eq!(e.iter().map(|x| x.input + x.output).sum::<u64>(), rep.meter.tokens());
    assert!(rep.meter.tokens() > 0);
}

// ---------------------------------------------------------------------------------------------- the tools

#[derive(Default)]
struct Mem {
    cfg: Mutex<RouterConfig>,
    files: Mutex<BTreeMap<String, String>>,
    audit: Mutex<Vec<String>>,
    pool: Mutex<Option<Arc<dyn ModelPool>>>,
    slot: Arc<rt::RunSlot>,
    last: Mutex<Option<String>>,
    refuse: Mutex<bool>,
    verifier: Mutex<Option<Arc<dyn Verifier>>>,
    used_today: Mutex<u64>,
}

impl Backend for Mem {
    fn providers(&self) -> Vec<ProviderCfg> {
        let mut l = ProviderCfg::new("local", Kind::Ollama);
        l.models = vec!["qwen:7b".into()];
        let mut c = ProviderCfg::new("cloud", Kind::OpenAi);
        c.models = vec!["gpt-large".into()];
        vec![l, c]
    }
    fn config(&self) -> RouterConfig {
        self.cfg.lock().unwrap().clone()
    }
    fn save_config(&self, c: &RouterConfig) -> Result<(), String> {
        *self.cfg.lock().unwrap() = c.clone();
        Ok(())
    }
    fn strategies(&self) -> Vec<(String, usize)> {
        self.files.lock().unwrap().iter().map(|(k, v)| (k.clone(), v.len())).collect()
    }
    fn strategy_source(&self, n: &str) -> Option<String> {
        self.files.lock().unwrap().get(n).cloned()
    }
    fn save_strategy(&self, n: &str, s: &str) -> Result<(), String> {
        self.files.lock().unwrap().insert(n.into(), s.into());
        Ok(())
    }
    fn remove_strategy(&self, n: &str) -> Result<(), String> {
        self.files.lock().unwrap().remove(n);
        Ok(())
    }
    fn last_run(&self) -> Option<String> {
        self.last.lock().unwrap().clone()
    }
    fn audit(&self, what: &str, detail: &str) {
        self.audit.lock().unwrap().push(format!("{what} {detail}"));
    }
    fn pool(&self) -> Option<Arc<dyn ModelPool>> {
        self.pool.lock().unwrap().clone()
    }
    fn verifier(&self) -> Arc<dyn Verifier> {
        self.verifier.lock().unwrap().clone().unwrap_or_else(|| Arc::new(AcceptClaim))
    }
    fn permit_run(&self, _: &str) -> Result<(), String> {
        if *self.refuse.lock().unwrap() { Err("card_declined".into()) } else { Ok(()) }
    }
    fn permit_strategy(&self, _: &str) -> Result<(), String> {
        if *self.refuse.lock().unwrap() { Err("card_declined".into()) } else { Ok(()) }
    }
    fn slot(&self) -> Arc<rt::RunSlot> {
        self.slot.clone()
    }
    fn tokens_last_day(&self) -> u64 {
        *self.used_today.lock().unwrap()
    }
    fn set_last_run(&self, t: &str) {
        *self.last.lock().unwrap() = Some(t.into());
    }
}

struct Nothing;
impl Tools for Nothing {
    fn defs(&self) -> Vec<crate::agent::ToolDef> {
        vec![]
    }
    fn call(&self, n: &str, _: &Value) -> Result<String, String> {
        Err(format!("unknown {n}"))
    }
    fn describe(&self, n: &str, _: &Value) -> String {
        n.into()
    }
}

fn tools() -> (RouterTools, Arc<Mem>) {
    let be = Arc::new(Mem::default());
    (RouterTools::new(Arc::new(Nothing), be.clone()), be)
}

#[test]
fn the_reads_are_free_and_every_change_is_a_hard_ask_that_no_session_grant_covers() {
    let (t, _) = tools();
    let defs = t.defs();
    for n in [rt::STATUS, rt::MODELS, rt::STRATEGY_LIST] {
        let d = defs.iter().find(|d| d.spec.name == n).unwrap();
        assert!(matches!(d.risk, Risk::Safe), "{n}");
        assert!(hard_ask(n, &json!({})).is_none(), "{n}");
    }
    for n in [rt::SET, rt::STRATEGY_SET, rt::STRATEGY_ADD, rt::STRATEGY_REMOVE] {
        let d = defs.iter().find(|d| d.spec.name == n).unwrap();
        assert!(matches!(d.risk, Risk::Ask), "{n}");
        assert!(hard_ask(n, &json!({"enabled": true})).is_some(), "{n} always asks, even in Auto mode");
        assert!(t.grant_blocker(n, &json!({})).is_some(), "{n}");
        assert!(!t.approved_always(n), "{n}");
    }
}

#[test]
fn router_set_validates_saves_and_writes_one_audit_line_without_content() {
    let (t, be) = tools();
    assert!(t.call(rt::SET, &json!({})).unwrap_err().contains("nothing to change"));
    assert!(t.call(rt::SET, &json!({"max_attempts": 99})).unwrap_err().contains("too large"));
    assert!(t.call(rt::SET, &json!({"planner": "local/qwen:7b"})).unwrap_err().contains("not a model that can plan"));
    assert!(t.call(rt::SET, &json!({"max_zone": "moon"})).is_err());
    assert!(t.call(rt::SET, &json!({"model": {"id": "ghost/x", "role": "both"}})).unwrap_err().contains("not a known model"));
    assert!(be.audit.lock().unwrap().is_empty(), "a refused change leaves no line");
    let _ = &be.refuse;
    let preview = t.preview(rt::SET, &json!({"enabled": true, "max_tokens": 5000, "max_zone": "network"})).unwrap();
    assert!(preview.contains("turned on") && preview.contains("5000 tokens") && preview.contains("local network"), "{preview}");
    assert!(!be.config().enabled, "a preview changes nothing");
    let out = t.call(rt::SET, &json!({"enabled": true, "max_tokens": 5000, "max_zone": "network", "planner": "cloud/gpt-large", "model": {"id": "local/qwen:7b", "role": "both", "context": 16384}})).unwrap();
    assert!(out.contains("cloud/gpt-large is the planner"), "{out}");
    let c = be.config();
    assert!(c.enabled && c.budget.max_tokens == 5000 && c.max_zone == Zone::Network && c.planner.as_deref() == Some("cloud/gpt-large"));
    assert_eq!(c.overrides["local/qwen:7b"].role, Some(Role::Both));
    assert_eq!(be.audit.lock().unwrap().as_slice(), ["router.set 5 change(s)"]);
    t.call(rt::SET, &json!({"model": {"id": "local/qwen:7b", "reset": true}})).unwrap();
    assert!(be.config().overrides.is_empty());
    let models = t.call(rt::MODELS, &json!({})).unwrap();
    assert!(models.contains("cloud/gpt-large") && models.contains("outside the zone you allow") && models.contains("local/qwen:7b"), "{models}");
    assert!(t.call(rt::STATUS, &json!({})).unwrap().contains("The router is on"));
}

#[test]
fn a_strategy_script_is_tried_before_it_is_added_shown_in_full_on_the_card_chosen_and_removed() {
    let (t, be) = tools();
    let bad = t.call(rt::STRATEGY_ADD, &json!({"name": "oops", "source": "function decide(i) { return 5; }"})).unwrap_err();
    assert!(bad.contains("did not pass the trial run"), "{bad}");
    assert!(t.refusal(rt::STRATEGY_ADD, &json!({"name": "default", "source": "x"})).is_some());
    assert!(t.refusal(rt::STRATEGY_ADD, &json!({"name": "Bad Name", "source": "x"})).is_some());
    let good = "function decide(i) { if (i.phase === 'plan') return {v: 1, planner: i.models[0].id}; return {v: 1, assign: [{step: i.ready[0], model: i.models[i.models.length - 1].id}]}; }";
    let card = t.preview(rt::STRATEGY_ADD, &json!({"name": "mine", "source": good})).unwrap();
    assert!(card.contains(good) && card.contains("no file, network or program access"), "{card}");
    t.call(rt::STRATEGY_ADD, &json!({"name": "mine", "source": good})).unwrap();
    assert!(t.call(rt::STRATEGY_SET, &json!({"name": "ghost"})).is_err());
    t.call(rt::STRATEGY_SET, &json!({"name": "mine"})).unwrap();
    assert_eq!(be.config().strategy, "mine");
    assert_eq!(rt::strategy_of(be.as_ref()).name(), "mine");
    let list = t.call(rt::STRATEGY_LIST, &json!({})).unwrap();
    assert!(list.contains("default (built in)") && list.contains("mine (your script") && list.contains("[in use]"), "{list}");
    t.call(rt::STRATEGY_REMOVE, &json!({"name": "mine"})).unwrap();
    assert_eq!(be.config().strategy, "default", "removing the strategy in use goes back to the default");
    assert_eq!(rt::strategy_of(be.as_ref()).name(), "default");
    let log = be.audit.lock().unwrap().clone();
    assert_eq!(log, ["router.strategy.add name=mine bytes=".to_string() + &good.len().to_string(), "router.strategy.set name=mine".into(), "router.strategy.remove name=mine".into()], "{log:?}");
}

#[test]
fn the_files_of_the_real_backend_round_trip_and_are_private() {
    use std::os::unix::fs::PermissionsExt;
    let dir = crate::testdir::outside_project("router", "files");
    let audit: Log = Arc::default();
    let a2 = audit.clone();
    let be = rt::FileBackend::new(dir.join("r"), Arc::new(Vec::new), Arc::new(move |w, d| a2.lock().unwrap().push(format!("{w} {d}"))));
    assert_eq!(be.config(), RouterConfig::default());
    let mut c = on();
    c.strategy = "mine".into();
    be.save_config(&c).unwrap();
    assert_eq!(be.config(), c);
    be.save_strategy("mine", "function decide(i) {}").unwrap();
    assert_eq!(be.strategies(), vec![("mine".to_string(), "function decide(i) {}".len())]);
    assert_eq!(be.strategy_source("mine").unwrap(), "function decide(i) {}");
    assert!(be.strategy_source("../etc").is_none() && be.save_strategy("../x", "y").is_err());
    let mode = std::fs::metadata(dir.join("r").join("config.json")).unwrap().permissions().mode() & 0o777;
    assert_eq!(mode, 0o600);
    be.remove_strategy("mine").unwrap();
    assert!(be.strategies().is_empty());
    let planner = Scripted::new(&[&plan_json(&[("s1", &[])])], "");
    let (mut r, _) = make(on(), &[big(), small()], vec![("big", planner), ("small", Scripted::new(&[], DONE1))], Arc::new(DefaultStrategy));
    be.record_run(&r.run("g"));
    assert!(be.last_run().unwrap().starts_with("Outcome: finished"));
}

// ---------------------------------------------------------------------------------------------- slice 2: runs, the core, the pool

/// A Mem whose providers are the two scripted fakes `big` (plans, may also run steps) and `small`.
fn run_rig(planner: Arc<Scripted>, exec: Arc<Scripted>) -> (RouterTools, Arc<Mem>) {
    let (t, be) = tools();
    let pool = Pool([("local/qwen:7b".to_string(), exec), ("cloud/gpt-large".to_string(), planner)].into_iter().collect());
    *be.pool.lock().unwrap() = Some(Arc::new(pool));
    (t, be)
}

fn rig_plan(steps: &[(&str, &[&str])]) -> Value {
    serde_json::from_str::<Value>(&plan_json(steps)).unwrap()
}

#[test]
fn router_run_asks_first_shows_the_plan_on_the_card_runs_in_the_background_and_leaves_one_summary() {
    let (t, be) = run_rig(Scripted::new(&[&plan_json(&[("s1", &[])])], ""), Scripted::new(&[], DONE1));
    assert!(hard_ask(rt::RUN, &json!({"goal": "x"})).is_some() && hard_ask(rt::CANCEL, &json!({})).is_none());
    assert!(t.grant_blocker(rt::RUN, &json!({})).is_some() && !t.approved_always(rt::RUN));
    let a = json!({"goal": "tidy the folder", "plan": rig_plan(&[("a", &[]), ("b", &["a"])])});
    let card = t.preview(rt::RUN, &a).unwrap();
    assert!(card.contains("Goal: tidy the folder") && card.contains("The plan:") && card.contains("- b (after a): do b [check: it works]") && card.contains("At most 200000 tokens"), "{card}");
    assert!(t.preview(rt::RUN, &json!({"goal": "x"})).unwrap().contains("planner will write the plan during the run"));
    assert!(t.refusal(rt::RUN, &json!({"goal": " "})).is_some());
    assert!(t.refusal(rt::RUN, &json!({"goal": "x", "plan": rig_plan(&[("a", &["b"]), ("b", &["a"])])})).unwrap().contains("circle"));
    // off, until the person turns it on
    assert!(t.call(rt::RUN, &json!({"goal": "x"})).unwrap_err().contains("router is off"));
    t.call(rt::SET, &json!({"enabled": true, "planner": "cloud/gpt-large"})).unwrap();
    // the profiles of the two providers: the local one only executes, the cloud one plans (and executes)
    t.call(rt::SET, &json!({"model": {"id": "cloud/gpt-large", "role": "planner"}})).unwrap();
    let out = t.call(rt::RUN, &json!({"goal": "SECRET-GOAL make it"})).unwrap();
    assert!(out.contains("has started"), "{out}");
    be.slot.join();
    let last = be.last_run().unwrap();
    assert!(last.starts_with("Outcome: finished") && last.contains("planned 1 steps"), "{last}");
    assert!(!last.contains("SECRET"), "the summary holds no goal text");
    let log = be.audit.lock().unwrap().clone();
    assert!(log.iter().any(|l| l.starts_with("router.run.permit")) && log.iter().any(|l| l.starts_with("router.run.end outcome=finished")), "{log:?}");
    assert!(log.iter().all(|l| !l.contains("SECRET")), "{log:?}");
    assert!(t.call(rt::STATUS, &json!({})).unwrap().contains("Latest run:\nOutcome: finished"));
    assert!(!be.slot.running());
}

#[test]
fn the_core_is_asked_before_a_run_and_a_strategy_change_and_its_refusal_stops_both() {
    let (t, be) = run_rig(Scripted::new(&[&plan_json(&[("s1", &[])])], ""), Scripted::new(&[], DONE1));
    t.call(rt::SET, &json!({"enabled": true, "model": {"id": "cloud/gpt-large", "role": "planner"}})).unwrap();
    *be.refuse.lock().unwrap() = true;
    assert_eq!(t.call(rt::RUN, &json!({"goal": "x"})).unwrap_err(), "card_declined");
    assert_eq!(t.call(rt::STRATEGY_SET, &json!({"name": "default"})).unwrap_err(), "card_declined");
    assert!(!be.slot.running() && be.last_run().is_none(), "nothing ran");
    assert!(be.audit.lock().unwrap().iter().all(|l| !l.starts_with("router.run")));
}

/// A model that waits until it is let go.
struct Held {
    open: Mutex<bool>,
    cv: std::sync::Condvar,
    reply: String,
    /// Set when a call has reached this model (it is now waiting to be let go).
    entered: std::sync::atomic::AtomicBool,
}

impl ChatBackend for Held {
    fn complete(&self, _: &[Message]) -> Result<String, String> {
        self.entered.store(true, std::sync::atomic::Ordering::SeqCst);
        let mut g = self.open.lock().unwrap();
        while !*g {
            g = self.cv.wait(g).unwrap();
        }
        Ok(self.reply.clone())
    }
}

struct HeldPool(Arc<Held>, Arc<Held>);
impl ModelPool for HeldPool {
    fn backend(&self, id: &str) -> Option<Arc<dyn ChatBackend>> {
        match id {
            "local/qwen:7b" => Some(self.0.clone()),
            "cloud/gpt-large" => Some(self.1.clone()),
            _ => None,
        }
    }
}

#[test]
fn a_running_plan_shows_in_the_status_a_second_run_is_refused_and_cancel_stops_it_after_the_call_that_is_running() {
    let (t, be) = tools();
    let exec = Arc::new(Held { open: Mutex::new(false), cv: Default::default(), reply: DONE1.into(), entered: Default::default() });
    let planner = Arc::new(Held { open: Mutex::new(true), cv: Default::default(), reply: "x".into(), entered: Default::default() });
    *be.pool.lock().unwrap() = Some(Arc::new(HeldPool(exec.clone(), planner)));
    t.call(rt::SET, &json!({"enabled": true, "model": {"id": "cloud/gpt-large", "role": "planner"}})).unwrap();
    t.call(rt::RUN, &json!({"goal": "g", "plan": rig_plan(&[("s1", &[]), ("s2", &["s1"])])})).unwrap();
    let mut shown = String::new();
    // wait until the first step's call is really in flight (held by `exec`): cancelling before it started would stop the run with 0 done
    let t0 = std::time::Instant::now();
    while t0.elapsed() < std::time::Duration::from_secs(30) {
        shown = t.call(rt::STATUS, &json!({})).unwrap();
        if shown.contains("0 of 2 steps done") && exec.entered.load(std::sync::atomic::Ordering::SeqCst) {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    assert!(shown.contains("A run is in progress: 0 of 2 steps done"), "{shown}");
    assert!(t.call(rt::RUN, &json!({"goal": "again"})).unwrap_err().contains("already going on"));
    assert!(t.call(rt::CANCEL, &json!({})).unwrap().contains("will stop"));
    *exec.open.lock().unwrap() = true;
    exec.cv.notify_all();
    be.slot.join();
    let last = be.last_run().unwrap();
    assert!(last.starts_with("Outcome: cancelled") && last.contains("1 done"), "{last}");
    assert!(t.call(rt::CANCEL, &json!({})).unwrap().contains("No run"));
}

#[test]
fn a_cancel_flag_set_before_the_run_ends_it_at_once_and_a_failing_check_from_the_ide_makes_a_failed_try() {
    let flag = Arc::new(std::sync::atomic::AtomicBool::new(true));
    let (r, _) = make(on(), &[big(), small()], vec![("big", Scripted::new(&[], "")), ("small", Scripted::new(&[], DONE1))], Arc::new(DefaultStrategy));
    let rep = r.with_cancel(flag).run("g");
    assert_eq!(rep.outcome, RunOutcome::Cancelled);
    assert_eq!(rep.meter.calls, 0, "no model call after the cancel");
    struct Cmd;
    impl AcceptanceCheck for Cmd {
        fn applies(&self, s: &Step) -> bool {
            s.check.starts_with("cmd:")
        }
        fn check(&self, s: &Step, result: &str) -> Result<(), String> {
            if result.contains("green") { Ok(()) } else { Err(format!("{} did not pass", s.check)) }
        }
    }
    let mut plan: Plan = serde_json::from_str(&plan_json(&[("s1", &[]), ("s2", &["s1"])])).unwrap();
    plan.steps[0].check = "cmd:cargo test".into();
    let exec = Scripted::new(&[r#"{"status":"done","result":"red"}"#, r#"{"status":"done","result":"green"}"#], DONE1);
    let (r, _) = make(on(), &[big(), small()], vec![("big", Scripted::new(&[], "")), ("small", exec.clone())], Arc::new(DefaultStrategy));
    let mut r = r.with_verifier(Arc::new(CheckedVerifier(vec![Arc::new(Cmd)])));
    let rep = r.run_plan("g", plan, None);
    assert_eq!(rep.outcome, RunOutcome::Finished);
    assert_eq!(rep.states["s1"].attempts, 2, "the first result failed the IDE's check");
    assert_eq!(rep.states["s2"].attempts, 1, "a step no check applies to keeps the executor's claim");
    assert_eq!(exec.asked().len(), 3);
}

#[test]
fn the_real_models_are_the_configured_providers_chat_only_and_the_run_meets_every_bound_on_scripted_provider_replies() {
    use iyakki_proxy::{Upstream, UpstreamResponse};
    struct Mock {
        calls: Mutex<Vec<(String, String)>>,
        plan: String,
    }
    impl Upstream for Mock {
        fn request(&self, _: &str, url: &str, _: &[(String, String)], body: &[u8]) -> Result<UpstreamResponse, String> {
            let b: Value = serde_json::from_slice(body).unwrap();
            let model = b["model"].as_str().unwrap().to_string();
            self.calls.lock().unwrap().push((model.clone(), b.to_string()));
            assert!(url.starts_with("http://192.168.1.5/v1"), "{url}");
            let text = if model == "big-70b" { self.plan.clone() } else { DONE1.to_string() };
            Ok(UpstreamResponse { status: 200, body: json!({"choices": [{"message": {"content": text}}], "usage": {"prompt_tokens": 11, "completion_tokens": 7}}).to_string().into_bytes() })
        }
    }
    let up = Arc::new(Mock { calls: Default::default(), plan: plan_json(&[("s1", &[]), ("s2", &["s1"])]) });
    let mut cfg = ProviderCfg::new("lan", Kind::Compat);
    cfg.base_url = Some("http://192.168.1.5/v1".into());
    cfg.models = vec!["big-70b".into(), "small-7b".into()];
    let mut over = BTreeMap::new();
    over.insert("lan/big-70b".to_string(), ProfileOverride { cost: Some(Cost::High), ..Default::default() });
    let profiles = build(&[cfg.clone()], &over);
    let up2 = up.clone();
    let c2 = cfg.clone();
    let pool = Arc::new(crate::router_pool::ProviderPool::new(Arc::new(move |p| {
        assert_eq!(p, "lan");
        Ok(crate::providers::ProviderClient::new(c2.clone(), crate::vault::Secrets::new(None, up2.clone()), up2.clone()))
    })));
    let log: Log = Arc::default();
    let l2 = log.clone();
    let mut r = Router::new(on(), &profiles, pool, Arc::new(DefaultStrategy), Arc::new(move |w, d| l2.lock().unwrap().push(format!("{w} {d}"))));
    let rep = r.run("g");
    assert_eq!(rep.outcome, RunOutcome::Finished, "{}", rep.summary());
    let calls = up.calls.lock().unwrap().clone();
    assert_eq!(calls.iter().map(|c| c.0.as_str()).collect::<Vec<_>>(), ["big-70b", "small-7b", "small-7b"], "the big model planned, the cheaper one ran both steps");
    assert!(calls.iter().all(|c| !c.1.contains("\"tools\"")), "no tools are offered to any model");
    assert_eq!(rep.meter.input, 33, "the counts the provider reported are used, not an estimate");
    assert_eq!(rep.meter.output, 21);
    assert!(log.lock().unwrap().iter().any(|l| l == "router.plan planner=lan/big-70b steps=2"));
}

#[test]
fn the_audit_lines_also_go_to_the_cores_access_log_and_the_core_decides_a_run() {
    use iyakki_core_api::Os;
    let _one = PRESS_TESTS.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    let fake = iyakki_core_fake::FakeOs::new();
    let dir = crate::testdir::outside_project("router", "gate");
    let audit: Log = Arc::default();
    let a2 = audit.clone();
    let be = rt::FileBackend::new(dir.join("r"), Arc::new(Vec::new), Arc::new(move |w, d| a2.lock().unwrap().push(format!("{w} {d}")))).with_gate(fake.router());
    be.audit("router.assign", "step=s1 model=small");
    assert_eq!(audit.lock().unwrap().len(), 1);
    assert_eq!(fake.router_fake().calls(), ["record(router.assign step=s1 model=small)"]);
    assert!(be.permit_run("1 steps").is_ok() && be.permit_strategy("mine").is_ok());
    fake.router_fake().refuse(true);
    assert!(be.permit_run("1 steps").is_err() && be.permit_strategy("mine").is_err());
}

/// The kept press is one slot for the whole process: the tests that use it take turns.
static PRESS_TESTS: Mutex<()> = Mutex::new(());

#[test]
fn a_run_approved_on_the_screens_thread_is_not_refused_on_the_tool_thread_for_lack_of_a_gesture_but_only_once_and_never_without_a_press() {
    use iyakki_core_api::broker::{Caller, CallerKind};
    use iyakki_core_api::guarded::{GestureCard, GuardedOs};
    use iyakki_core_api::Os;
    let _one = PRESS_TESTS.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    let dir = crate::testdir::outside_project("router", "press");
    let card = Arc::new(GestureCard::new(std::time::Duration::from_secs(1)));
    let (broker, access) = crate::accessbroker::local(&dir, Some(card.clone()));
    let fake = Arc::new(iyakki_core_fake::FakeOs::new());
    let g = GuardedOs::new(fake.clone(), broker, Caller { app: "chat".into(), kind: CallerKind::Builtin, person: "ann".into(), device: "d".into() });
    let be = Arc::new(rt::FileBackend::new(dir.join("r"), Arc::new(Vec::new), Arc::new(|_, _| {})).with_gate(g.router()));
    let ask = |be: &Arc<rt::FileBackend>| {
        let b = be.clone();
        std::thread::spawn(move || b.permit_run("1 steps, strategy default")).join().unwrap()
    };
    // a run nobody pressed for is refused by the core, as it must be
    let _ = rt::with_press(|| ());
    assert!(ask(&be).unwrap_err().contains("no_card"));
    // the person's press on the screen's thread approves the chat's card; the tool runs on another thread
    card.dispatch(rt::note_press);
    assert!(ask(&be).is_ok(), "the run was refused for lack of a gesture");
    // the press is spent: the next run needs its own
    assert!(ask(&be).unwrap_err().contains("no_card"));
    card.dispatch(rt::note_press);
    let b2 = be.clone();
    assert!(std::thread::spawn(move || b2.permit_strategy("mine")).join().unwrap().is_ok(), "the same for a strategy change");
    let _ = (&access, &fake);
}

// ---------------------------------------------------------------------------------------------- executors with eyes and ears (F-ROUTER-03)

/// A camera and a microphone that count what they were asked for (the real ones are the core's, with its approvals).
struct FakeSenses {
    photos: Mutex<Vec<String>>,
    sounds: Mutex<u32>,
    deny: bool,
}

impl FakeSenses {
    fn new(deny: bool) -> Arc<FakeSenses> {
        Arc::new(FakeSenses { photos: Mutex::new(vec![]), sounds: Mutex::new(0), deny })
    }
}

impl Senses for FakeSenses {
    fn available(&self) -> (bool, bool) {
        (true, true)
    }
    fn photo(&self, camera: &str) -> Result<Vec<u8>, String> {
        self.photos.lock().unwrap().push(camera.to_string());
        if self.deny { Err("the person said no".into()) } else { Ok(vec![0xff, 0xd8, 1, 2, 3]) }
    }
    fn sound(&self, _: u32) -> Result<String, String> {
        *self.sounds.lock().unwrap() += 1;
        if self.deny { Err("the person said no".into()) } else { Ok("a dog barks twice".into()) }
    }
}

fn eyes(id: &str, vision: bool, audio: bool) -> Profile {
    let mut p = prof(id, Role::Executor, Cost::Free, Zone::Computer);
    p.caps.vision = vision;
    p.caps.audio = audio;
    p
}

const NEED_PHOTO: &str = r#"{"status":"need","sense":"photo","camera":"usb:video0"}"#;
const NEED_SOUND: &str = r#"{"status":"need","sense":"sound"}"#;

fn sense_run(models: &[Profile], exec: Arc<Scripted>, senses: Arc<FakeSenses>) -> (RunReport, Log) {
    let (r, log) = make(on(), models, vec![("eye", exec)], Arc::new(DefaultStrategy));
    let mut r = r.with_senses(senses);
    let plan = Plan::parse(&plan_json(&[("s1", &[])])).unwrap();
    (r.run_plan("look at the room", plan, None), log)
}

#[test]
fn an_executor_that_can_see_takes_a_photo_through_the_senses_and_the_picture_reaches_only_that_model() {
    let exec = Scripted::new(&[NEED_PHOTO, r#"{"status":"done","result":"a lamp is on"}"#], DONE1);
    let senses = FakeSenses::new(false);
    let (rep, log) = sense_run(&[eyes("eye", true, false)], exec.clone(), senses.clone());
    assert_eq!(rep.outcome, RunOutcome::Finished, "{}", rep.summary());
    assert_eq!(rep.states["s1"].result.as_deref(), Some("a lamp is on"));
    assert_eq!(*senses.photos.lock().unwrap(), vec!["usb:video0".to_string()]);
    assert_eq!(*exec.pictures.lock().unwrap(), vec![0, 1], "the second call carried the picture");
    assert!(exec.systems.lock().unwrap()[0].contains("photo"), "the model is told it may ask");
    let l = log.lock().unwrap().join("\n");
    assert!(l.contains("router.sense step=s1 model=eye kind=photo ok=true"), "{l}");
    assert!(!l.contains("0xff") && !l.contains("lamp") && !l.contains("dog"), "the log holds no picture or words");
}

#[test]
fn a_sound_is_given_as_text_to_a_model_that_hears_and_refused_to_one_that_does_not() {
    let hears = Scripted::new(&[NEED_SOUND, r#"{"status":"done","result":"a dog"}"#], DONE1);
    let senses = FakeSenses::new(false);
    let (rep, _) = sense_run(&[eyes("eye", false, true)], hears.clone(), senses.clone());
    assert_eq!(rep.outcome, RunOutcome::Finished);
    assert_eq!(*senses.sounds.lock().unwrap(), 1);
    assert!(hears.asked()[1].contains("a dog barks twice"));
    // a model without audio asks anyway: the microphone is not opened and it is told so
    let deaf = Scripted::new(&[NEED_SOUND, r#"{"status":"blocked","error":"cannot hear"}"#], DONE1);
    let senses = FakeSenses::new(false);
    let (rep, log) = sense_run(&[eyes("eye", true, false)], deaf.clone(), senses.clone());
    assert_eq!(*senses.sounds.lock().unwrap(), 0, "the core was never asked");
    assert!(deaf.asked()[1].contains("not possible"), "{:?}", deaf.asked());
    assert!(log.lock().unwrap().iter().any(|l| l.contains("kind=sound ok=false")));
    assert_ne!(rep.outcome, RunOutcome::Finished);
}

#[test]
fn a_refusal_by_the_person_is_passed_on_and_a_step_may_ask_only_so_often() {
    let exec = Scripted::new(&[NEED_PHOTO, r#"{"status":"done","result":"worked without it"}"#], DONE1);
    let senses = FakeSenses::new(true);
    let (rep, _) = sense_run(&[eyes("eye", true, false)], exec.clone(), senses);
    assert_eq!(rep.outcome, RunOutcome::Finished);
    assert!(exec.asked()[1].contains("the person said no"));
    assert_eq!(*exec.pictures.lock().unwrap(), vec![0, 0], "no picture came");
    // a model that keeps asking is stopped after MAX_SENSES
    let greedy = Scripted::new(&[], NEED_PHOTO);
    let senses = FakeSenses::new(false);
    let (rep, _) = sense_run(&[eyes("eye", true, false)], greedy, senses.clone());
    let asked = senses.photos.lock().unwrap().len() as u32;
    assert!(asked >= MAX_SENSES && asked % MAX_SENSES == 0, "at most MAX_SENSES per try: {asked}");
    assert_ne!(rep.outcome, RunOutcome::Finished);
    // with no camera on the computer the model is not even told it may ask
    let plain = Scripted::new(&[], DONE1);
    let (r, _) = make(on(), &[eyes("eye", true, true)], vec![("eye", plain.clone())], Arc::new(DefaultStrategy));
    let mut r = r;
    let _ = r.run_plan("g", Plan::parse(&plan_json(&[("s1", &[])])).unwrap(), None);
    assert!(!plain.systems.lock().unwrap()[0].contains("photo"));
}

#[test]
fn a_step_whose_check_says_tests_is_judged_by_the_projects_own_test_command() {
    use crate::router::ProjectTestCheck;
    let dir = crate::testdir::outside_project("router-check", "tests");
    let step = |check: &str| Step { id: "s1".into(), title: "t".into(), task: "t".into(), after: vec![], role: Role::Executor, needs: Default::default(), check: check.into() };
    let pass = ProjectTestCheck { root: dir.clone(), program: "true".into(), args: vec![] };
    let fail = ProjectTestCheck { root: dir.clone(), program: "false".into(), args: vec![] };
    assert!(pass.applies(&step("tests pass")) && pass.applies(&step("Tests")) && pass.applies(&step("test: cargo test")));
    assert!(!pass.applies(&step("the file exists")) && !pass.applies(&step("")), "other checks are not its business");
    assert!(pass.check(&step("tests pass"), "done").is_ok());
    let e = fail.check(&step("tests pass"), "done").unwrap_err();
    assert!(e.contains("did not pass"), "{e}");
    // a project with no test command has no such check
    assert!(ProjectTestCheck::for_project(&dir).is_none());
    // through the router: a failing project test makes the step a failed try
    let v = CheckedVerifier(vec![Arc::new(fail)]);
    assert!(v.verify(&step("tests pass"), "done").is_err());
    assert!(v.verify(&step("looks fine"), "done").is_ok());
}

#[test]
fn the_strategy_is_told_how_fast_each_allowed_model_has_been_and_the_ledger_lines_are_averaged() {
    struct Spy(Arc<std::sync::Mutex<Vec<serde_json::Value>>>);
    impl Strategy for Spy {
        fn name(&self) -> String {
            "spy".into()
        }
        fn decide(&self, input: &serde_json::Value) -> Result<serde_json::Value, String> {
            self.0.lock().unwrap().push(input.clone());
            DefaultStrategy.decide(input)
        }
    }
    let seen = Arc::new(std::sync::Mutex::new(vec![]));
    let (r, _) = make(on(), &[big(), small()], vec![("big", Scripted::new(&[], "")), ("small", Scripted::new(&[], DONE1))], Arc::new(Spy(seen.clone())));
    let mut speeds = std::collections::BTreeMap::new();
    speeds.insert("big".to_string(), 900u64);
    speeds.insert("not-a-model-here".to_string(), 5u64);
    let plan: Plan = serde_json::from_str(&plan_json(&[("s1", &[])])).unwrap();
    let rep = r.with_speeds(speeds).run_plan("g", plan, None);
    assert_eq!(rep.outcome, RunOutcome::Finished);
    let first = seen.lock().unwrap()[0].clone();
    assert_eq!(first["speeds"]["big"], 900);
    assert!(first["speeds"].get("not-a-model-here").is_none(), "only models the core allows are described");
    // ledger lines: only calls that worked and have times count; errors and untimed lines do not
    use crate::usage::Entry;
    let e = |model: &str, calls: u64, errors: u64, ms: u64, cat: &str| Entry { provider: "p".into(), model: model.into(), calls, errors, ms, cat: cat.into(), ..Default::default() };
    let m = crate::routertools::speeds_of(&[e("m1", 4, 1, 3000, ""), e("m1", 2, 0, 1000, "llm"), e("m2", 3, 0, 0, ""), e("m3", 5, 0, 500, "tts")]);
    assert_eq!(m.get("p/m1"), Some(&(5, 4000)), "3 worked of the first line and 2 of the second");
    assert!(m.get("p/m2").is_none() && m.get("p/m3").is_none());
}

struct TwoCameras(Mutex<Vec<String>>);

impl Senses for TwoCameras {
    fn available(&self) -> (bool, bool) {
        (true, false)
    }
    fn cameras(&self) -> Vec<(String, String)> {
        vec![("usb:0".into(), "Front door".into()), ("usb:1".into(), "Garden".into())]
    }
    fn photo(&self, camera: &str) -> Result<Vec<u8>, String> {
        self.0.lock().unwrap().push(camera.to_string());
        Ok(vec![0xff, 0xd8, 9])
    }
}

#[test]
fn with_several_cameras_the_executor_is_told_their_ids_and_names_and_can_choose_one() {
    let exec = Scripted::new(&[r#"{"status":"need","sense":"photo","camera":"Garden"}"#, r#"{"status":"done","result":"the garden is dark"}"#], DONE1);
    let cams = Arc::new(TwoCameras(Mutex::new(vec![])));
    let (r, _) = make(on(), &[eyes("eye", true, false)], vec![("eye", exec.clone())], Arc::new(DefaultStrategy));
    let mut r = r.with_senses(cams.clone());
    let rep = r.run_plan("look", Plan::parse(&plan_json(&[("s1", &[])])).unwrap(), None);
    assert_eq!(rep.outcome, RunOutcome::Finished, "{}", rep.summary());
    let sys = exec.systems.lock().unwrap()[0].clone();
    assert!(sys.contains("usb:0 = Front door") && sys.contains("usb:1 = Garden"), "{sys}");
    assert_eq!(*cams.0.lock().unwrap(), vec!["Garden".to_string()], "the choice reached the camera door as the executor wrote it");
}

#[test]
fn the_router_keeps_to_a_daily_token_limit_the_person_sets_and_a_run_that_would_pass_it_does_not_start() {
    let (t, be) = tools();
    t.call(rt::SET, &json!({"enabled": true, "max_tokens_per_day": 1000})).unwrap();
    assert_eq!(be.config().daily_tokens, 1000);
    *be.pool.lock().unwrap() = Some(Arc::new(NoPool));
    *be.used_today.lock().unwrap() = 1000;
    let e = t.call(rt::RUN, &json!({"goal": "do the thing"})).unwrap_err();
    assert!(e.contains("last 24 hours") && e.contains("1000"), "{e}");
    assert!(be.slot.running() == false, "nothing started");
    // 0 takes the limit away again
    t.call(rt::SET, &json!({"max_tokens_per_day": 0})).unwrap();
    assert_eq!(be.config().daily_tokens, 0);
    assert!(!rt::start_run(&(be.clone() as Arc<dyn Backend>), "g", None).err().unwrap_or_default().contains("last 24 hours"));
}

struct NoPool;
impl ModelPool for NoPool {
    fn backend(&self, _: &str) -> Option<Arc<dyn ChatBackend>> {
        None
    }
}

#[test]
fn a_strategy_can_be_chosen_for_one_workspace_or_fleet_and_the_rest_keep_the_computers_own() {
    let (t, be) = tools();
    let good = "function decide(i) { if (i.phase === 'plan') return {v: 1, planner: i.models[0].id}; return {v: 1, assign: [{step: i.ready[0], model: i.models[i.models.length - 1].id}]}; }";
    t.call(rt::STRATEGY_ADD, &json!({"name": "mine", "source": good})).unwrap();
    assert!(t.call(rt::STRATEGY_SET, &json!({"name": "mine", "scope": "everything"})).unwrap_err().contains("workspace:NAME"));
    let out = t.call(rt::STRATEGY_SET, &json!({"name": "mine", "scope": "workspace:alpha"})).unwrap();
    assert!(out.contains("workspace:alpha"), "{out}");
    t.call(rt::STRATEGY_SET, &json!({"name": "mine", "scope": "fleet:lab"})).unwrap();
    assert_eq!(be.config().strategy, "default", "the computer's own strategy did not change");
    assert_eq!(rt::strategy_for(be.as_ref(), "workspace:alpha").name(), "mine");
    assert_eq!(rt::strategy_for(be.as_ref(), "fleet:lab").name(), "mine");
    assert_eq!(rt::strategy_for(be.as_ref(), "workspace:beta").name(), "default");
    assert_eq!(rt::strategy_for(be.as_ref(), "").name(), "default");
    assert!(be.audit.lock().unwrap().iter().any(|l| l.contains("router.strategy.set") && l.contains("scope=workspace:alpha")));
    assert!(rt::scope_ok("workspace:a b") && !rt::scope_ok("workspace:") && !rt::scope_ok("moon:x"));
}

#[path = "router_parallel_tests.rs"]
mod parallel;
