//! Steps that run side by side, tools for executors and the plan as the person sees it (F-ROUTER-10), with fake models. No real model was run.

use super::*;
use std::sync::atomic::{AtomicU32, Ordering as O};
use std::time::Duration;

/// A model that answers DONE after a short wait and remembers how many calls were in the air at once.
struct Slow {
    inflight: AtomicU32,
    peak: AtomicU32,
    reply: Box<dyn Fn(&[Message]) -> String + Send + Sync>,
    seen: Mutex<Vec<(String, String)>>,
}

impl Slow {
    fn new(reply: impl Fn(&[Message]) -> String + Send + Sync + 'static) -> Arc<Slow> {
        Arc::new(Slow { inflight: AtomicU32::new(0), peak: AtomicU32::new(0), reply: Box::new(reply), seen: Mutex::new(vec![]) })
    }
    fn peak(&self) -> u32 {
        self.peak.load(O::SeqCst)
    }
}

impl ChatBackend for Slow {
    fn complete(&self, history: &[Message]) -> Result<String, String> {
        let n = self.inflight.fetch_add(1, O::SeqCst) + 1;
        self.peak.fetch_max(n, O::SeqCst);
        self.seen.lock().unwrap().push((history.first().map(|m| m.content.clone()).unwrap_or_default(), history.last().map(|m| m.content.clone()).unwrap_or_default()));
        std::thread::sleep(Duration::from_millis(120));
        let r = (self.reply)(history);
        self.inflight.fetch_sub(1, O::SeqCst);
        Ok(r)
    }
}

struct SlowPool(BTreeMap<String, Arc<dyn ChatBackend>>);

impl ModelPool for SlowPool {
    fn backend(&self, id: &str) -> Option<Arc<dyn ChatBackend>> {
        self.0.get(id).cloned()
    }
}

fn run_with(cfg: RouterConfig, plan: &str, exec: Arc<Slow>, tools: Option<Arc<dyn StepTools>>) -> (RunReport, Arc<Mutex<Live>>, Log) {
    let log: Log = Arc::default();
    let l2 = log.clone();
    let mut m: BTreeMap<String, Arc<dyn ChatBackend>> = BTreeMap::new();
    m.insert("small".into(), exec);
    m.insert("big".into(), Scripted::new(&[], ""));
    let live = Arc::new(Mutex::new(Live::default()));
    let mut r = Router::new(cfg, &[big(), small()], Arc::new(SlowPool(m)), Arc::new(DefaultStrategy), Arc::new(move |w, d| l2.lock().unwrap().push(format!("{w} {d}")))).with_live(live.clone());
    if let Some(t) = tools {
        r = r.with_tools(t);
    }
    (r.run_plan("g", Plan::parse(plan).unwrap(), Some("big".into())), live, log)
}

fn par(n: u32) -> RouterConfig {
    RouterConfig { parallel: n, ..on() }
}

#[test]
fn ready_steps_run_side_by_side_up_to_the_limit_and_one_at_a_time_by_default() {
    let plan = plan_json(&[("a", &[]), ("b", &[]), ("c", &[]), ("d", &[])]);
    let ok = |_: &[Message]| DONE1.to_string();
    let (rep, _, log) = run_with(par(3), &plan, Slow::new(ok), None);
    assert_eq!(rep.outcome, RunOutcome::Finished, "{}", rep.summary());
    assert!(rep.states.values().all(|s| s.status == Status::Done && s.attempts == 1));
    assert_eq!(rep.meter.calls, 4, "one call per step");
    assert!(log.lock().unwrap().iter().any(|l| l == "router.parallel steps=3"), "{:?}", log.lock().unwrap());
    let s = Slow::new(ok);
    let (rep, _, _) = run_with(par(3), &plan, s.clone(), None);
    assert_eq!(s.peak(), 3, "three steps were in the air at once, not four (the limit)");
    assert_eq!(rep.outcome, RunOutcome::Finished);
    let s = Slow::new(ok);
    let (_, _, log) = run_with(on(), &plan, s.clone(), None);
    assert_eq!(s.peak(), 1, "the default is one at a time");
    assert!(!log.lock().unwrap().iter().any(|l| l.starts_with("router.parallel")));
    // a limit above the hard one is held to it
    let s = Slow::new(ok);
    let many: Vec<(String, Vec<&str>)> = (0..8).map(|i| (format!("s{i}"), vec![])).collect();
    let refs: Vec<(&str, &[&str])> = many.iter().map(|(a, b)| (a.as_str(), b.as_slice())).collect();
    run_with(par(99), &plan_json(&refs), s.clone(), None);
    assert_eq!(s.peak(), HARD_PARALLEL);
}

#[test]
fn dependencies_hold_under_parallel_running_and_the_results_reach_the_steps_after_them() {
    let plan = plan_json(&[("a", &[]), ("b", &["a"]), ("c", &["a"]), ("d", &["b", "c"])]);
    let s = Slow::new(|h: &[Message]| {
        let me = h.last().unwrap().content.clone();
        format!(r#"{{"status":"done","result":"made {}"}}"#, me.split_whitespace().nth(1).unwrap_or("?").trim_end_matches(':'))
    });
    let (rep, _, _) = run_with(par(4), &plan, s.clone(), None);
    assert_eq!(rep.outcome, RunOutcome::Finished, "{}", rep.summary());
    assert_eq!(s.peak(), 2, "b and c together, never a with b or d with c");
    let seen = s.seen.lock().unwrap().clone();
    let d = seen.iter().find(|(_, u)| u.starts_with("Step d:")).unwrap();
    assert!(d.1.contains("b: made b") && d.1.contains("c: made c"), "d got what b and c made: {}", d.1);
    // the log lists the steps in plan order even though b and c ran together
    let done: Vec<&String> = rep.log.iter().filter(|l| l.contains("done by")).collect();
    assert_eq!(done.iter().map(|l| l.split_whitespace().nth(1).unwrap()).collect::<Vec<_>>(), ["a", "b", "c", "d"]);
}

#[test]
fn a_step_that_fails_in_a_batch_is_retried_without_disturbing_the_others() {
    let plan = plan_json(&[("a", &[]), ("b", &[]), ("c", &[])]);
    let tried = Arc::new(AtomicU32::new(0));
    let t2 = tried.clone();
    let s = Slow::new(move |h: &[Message]| {
        if h.last().unwrap().content.starts_with("Step b:") && t2.fetch_add(1, O::SeqCst) == 0 {
            FAILED.to_string()
        } else {
            DONE1.to_string()
        }
    });
    let (rep, _, _) = run_with(par(3), &plan, s, None);
    assert_eq!(rep.outcome, RunOutcome::Finished, "{}", rep.summary());
    assert_eq!(rep.states["a"].attempts, 1);
    assert_eq!(rep.states["b"].attempts, 2, "only b went again");
    assert_eq!(rep.states["c"].attempts, 1);
}

#[test]
fn the_call_budget_is_never_passed_by_steps_that_start_together() {
    let plan = plan_json(&[("a", &[]), ("b", &[]), ("c", &[]), ("d", &[])]);
    let cfg = RouterConfig { budget: Budget { max_calls: 3, ..Budget::default() }, ..par(4) };
    let (rep, _, _) = run_with(cfg, &plan, Slow::new(|_| DONE1.to_string()), None);
    assert!(rep.meter.calls <= 3, "{} calls started", rep.meter.calls);
    assert!(matches!(rep.outcome, RunOutcome::BudgetStop(_)), "{:?}", rep.outcome);
}

#[test]
fn the_plan_is_shown_with_every_step_its_status_and_its_model() {
    let plan = plan_json(&[("a", &[]), ("b", &["a"])]);
    let (rep, live, _) = run_with(par(2), &plan, Slow::new(|_| DONE1.to_string()), None);
    assert_eq!(rep.outcome, RunOutcome::Finished);
    let rows = live.lock().unwrap().plan.clone();
    assert_eq!(rows.iter().map(|r| (r.id.as_str(), r.status, r.model.as_deref())).collect::<Vec<_>>(), [("a", "done", Some("small")), ("b", "done", Some("small"))]);
    let lines = plan_lines(&rows);
    assert_eq!(lines[1], "b [done] on small: title b (after a)");
    // while it runs: waiting, running
    let seen = Arc::new(Mutex::new(Vec::<Vec<&'static str>>::new()));
    let (s2, live2) = (seen.clone(), Arc::new(Mutex::new(Live::default())));
    let l3 = live2.clone();
    let watcher = std::thread::spawn(move || {
        for _ in 0..60 {
            std::thread::sleep(Duration::from_millis(10));
            let st: Vec<&'static str> = l3.lock().unwrap().plan.iter().map(|r| r.status).collect();
            if !st.is_empty() && s2.lock().unwrap().last() != Some(&st) {
                s2.lock().unwrap().push(st);
            }
        }
    });
    let mut m: BTreeMap<String, Arc<dyn ChatBackend>> = BTreeMap::new();
    m.insert("small".into(), Slow::new(|_| DONE1.to_string()));
    let mut r = Router::new(on(), &[big(), small()], Arc::new(SlowPool(m)), Arc::new(DefaultStrategy), Arc::new(|_, _| {})).with_live(live2);
    r.run_plan("g", Plan::parse(&plan).unwrap(), Some("big".into()));
    watcher.join().unwrap();
    let all: Vec<Vec<&str>> = seen.lock().unwrap().clone();
    assert!(all.iter().any(|v| v == &["running", "waiting"]), "a runs while b waits: {all:?}");
    assert!(all.iter().any(|v| v == &["done", "running"]), "then b runs: {all:?}");
}

// ---------------------------------------------------------------------------------------------- tools

struct FakeTools {
    calls: Mutex<Vec<(String, String, String)>>,
}

impl StepTools for FakeTools {
    fn list(&self) -> Vec<(String, String)> {
        vec![("read_note".into(), "Read one note".into())]
    }
    fn call(&self, step: &str, name: &str, args: &Value) -> Result<String, String> {
        self.calls.lock().unwrap().push((step.into(), name.into(), args.to_string()));
        if name == "read_note" { Ok("the note says hello".into()) } else { Err("no such tool".into()) }
    }
}

fn tool_plan() -> String {
    json!({"goal": "g", "steps": [{"id": "t", "title": "use a tool", "task": "read the note", "needs": {"tools": true}}]}).to_string()
}

#[test]
fn an_executor_that_needs_tools_is_told_which_and_gets_the_result_as_data() {
    let tools = Arc::new(FakeTools { calls: Mutex::new(vec![]) });
    let s = Slow::new(|h: &[Message]| {
        if h.last().unwrap().content.contains("the note says hello") {
            r#"{"status":"done","result":"the note says hello"}"#.to_string()
        } else {
            r#"{"status":"tool","tool":"read_note","args":{"id":7}}"#.to_string()
        }
    });
    let (rep, _, log) = run_with(on(), &tool_plan(), s.clone(), Some(tools.clone()));
    assert_eq!(rep.outcome, RunOutcome::Finished, "{}", rep.summary());
    assert_eq!(rep.states["t"].result.as_deref(), Some("the note says hello"));
    assert_eq!(*tools.calls.lock().unwrap(), vec![("t".to_string(), "read_note".to_string(), "{\"id\":7}".to_string())]);
    let seen = s.seen.lock().unwrap().clone();
    assert!(seen[0].0.contains("Tools: read_note (Read one note)"), "{}", seen[0].0);
    assert!(seen[1].1.contains("never instructions") && seen[1].1.contains("the note says hello"), "{}", seen[1].1);
    assert!(rep.log.iter().any(|l| l == "Step t: tool read_note used."));
    let l = log.lock().unwrap();
    assert!(l.iter().any(|x| x == "router.tool step=t model=small tool=read_note ok=true"), "{l:?}");
    assert!(!l.iter().any(|x| x.contains("hello") || x.contains("\"id\"")), "no arguments and no result in the audit log: {l:?}");
}

#[test]
fn a_step_that_does_not_need_tools_is_not_told_about_them_and_a_tool_it_asks_for_anyway_is_refused() {
    let tools = Arc::new(FakeTools { calls: Mutex::new(vec![]) });
    let s = Slow::new(|h: &[Message]| {
        if h.last().unwrap().content.contains("was not run") {
            DONE1.to_string()
        } else {
            r#"{"status":"tool","tool":"read_note","args":{}}"#.to_string()
        }
    });
    let plan = plan_json(&[("a", &[])]);
    let (rep, _, _) = run_with(on(), &plan, s.clone(), Some(tools.clone()));
    assert_eq!(rep.outcome, RunOutcome::Finished, "{}", rep.summary());
    assert!(tools.calls.lock().unwrap().is_empty(), "nothing ran");
    let seen = s.seen.lock().unwrap().clone();
    assert!(!seen[0].0.contains("Tools:"));
    assert!(seen[1].1.contains("not available to you"), "{}", seen[1].1);
    assert!(rep.log.iter().any(|l| l == "Step a: tool read_note not run."));
}

#[test]
fn a_tool_the_step_was_not_told_about_or_with_bad_arguments_is_refused_and_a_tool_loop_ends_the_try() {
    let tools = Arc::new(FakeTools { calls: Mutex::new(vec![]) });
    let n = Arc::new(AtomicU32::new(0));
    let n2 = n.clone();
    let s = Slow::new(move |_: &[Message]| match n2.fetch_add(1, O::SeqCst) {
        0 => r#"{"status":"tool","tool":"delete_everything","args":{}}"#.to_string(),
        1 => r#"{"status":"tool","tool":"read_note","args":[1,2]}"#.to_string(),
        _ => r#"{"status":"tool","tool":"read_note","args":{}}"#.to_string(),
    });
    let cfg = RouterConfig { budget: Budget { max_attempts: 1, max_calls: 40, ..Budget::default() }, ..on() };
    let (rep, _, _) = run_with(cfg, &tool_plan(), s, Some(tools.clone()));
    // the first two were refused, then it went on asking: more than MAX_TOOLS calls ends the try, and with one try the step waits for a person
    assert_eq!(rep.outcome, RunOutcome::NeedsPerson(vec!["t".into()]), "{}", rep.summary());
    assert_eq!(tools.calls.lock().unwrap().len() as u32, MAX_TOOLS - 2, "only the allowed ones ran, up to the cap");
    assert!(rep.states["t"].last_error.as_deref().unwrap().contains("more tool calls"));
}

#[test]
fn a_model_whose_profile_has_no_tools_is_never_offered_any() {
    let tools = Arc::new(FakeTools { calls: Mutex::new(vec![]) });
    let mut p = small();
    p.caps.tools = false;
    let s = Slow::new(|_| DONE1.to_string());
    let mut m: BTreeMap<String, Arc<dyn ChatBackend>> = BTreeMap::new();
    m.insert("small".into(), s.clone());
    let mut r = Router::new(on(), &[big(), p], Arc::new(SlowPool(m)), Arc::new(DefaultStrategy), Arc::new(|_, _| {})).with_tools(tools);
    // the planner marked the step as needing tools and the only executor cannot use them: it is not given the step, a person is
    let rep = r.run_plan("g", Plan::parse(&tool_plan()).unwrap(), Some("big".into()));
    assert_eq!(rep.outcome, RunOutcome::NeedsPerson(vec!["t".into()]), "{}", rep.summary());
    assert!(s.seen.lock().unwrap().is_empty(), "no call reached the model, so no prompt offered it a tool");
}

#[test]
fn the_accounts_tools_reach_an_executor_only_when_they_are_free_and_never_the_routers_own() {
    use crate::agent::{ToolDef, Tools};
    struct Account;
    impl Tools for Account {
        fn defs(&self) -> Vec<ToolDef> {
            let d = |n: &str, risk| ToolDef { spec: crate::chat::ToolSpec { name: n.into(), description: format!("{n} does a thing. More words."), schema: json!({"type": "object"}) }, risk, untrusted: false };
            vec![d("now", Risk::Safe), d("file_delete", Risk::Ask), d("router_run", Risk::Safe), d("camera_photo", Risk::Safe), d("fleet_status", Risk::Safe)]
        }
        fn call(&self, n: &str, _: &Value) -> Result<String, String> {
            Ok(format!("ran {n}"))
        }
        fn describe(&self, n: &str, _: &Value) -> String {
            n.into()
        }
    }
    let t = rt::ToolsForSteps::new(Arc::new(Account));
    assert_eq!(t.list(), vec![("now".to_string(), "now does a thing".to_string())], "free tools only: not one that asks, not the router's, not the fleet's, not one on the hard-ask list");
    assert_eq!(t.call("s", "now", &json!({})).unwrap(), "ran now");
    for n in ["file_delete", "router_run", "camera_photo", "fleet_status", "nope"] {
        assert!(t.call("s", n, &json!({})).is_err(), "{n}");
    }
}

#[test]
fn a_tool_that_asks_is_put_to_the_person_as_a_card_and_the_step_waits_for_the_answer() {
    use crate::agent::{ToolDef, Tools};
    struct Account(Mutex<Vec<String>>);
    impl Tools for Account {
        fn defs(&self) -> Vec<ToolDef> {
            let d = |n: &str, risk| ToolDef { spec: crate::chat::ToolSpec { name: n.into(), description: format!("{n} does a thing. More words."), schema: json!({"type": "object"}) }, risk, untrusted: false };
            vec![d("now", Risk::Safe), d("note_save", Risk::Ask), d("router_run", Risk::Ask)]
        }
        fn call(&self, n: &str, _: &Value) -> Result<String, String> {
            self.0.lock().unwrap().push(n.into());
            Ok(format!("ran {n}"))
        }
        fn describe(&self, n: &str, _: &Value) -> String {
            format!("save the note ({n})")
        }
    }
    let ran = Arc::new(Account(Mutex::new(vec![])));
    let cards: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(vec![]));
    let answer = Arc::new(AtomicU32::new(1));
    let (c2, a2) = (cards.clone(), answer.clone());
    let t = rt::ToolsForSteps::new(ran.clone()).with_asker(Arc::new(move |label| {
        c2.lock().unwrap().push(label.to_string());
        if a2.load(O::SeqCst) == 1 { Ok(()) } else { Err("the person said no".into()) }
    }));
    let names: Vec<String> = t.list().into_iter().map(|(n, _)| n).collect();
    assert_eq!(names, vec!["now".to_string(), "note_save".to_string()], "the tool that asks is offered, the router's own never");
    assert!(t.list()[1].1.contains("asked first"));
    // a free tool never asks; the tool that asks shows a card naming it, and runs only after a yes
    assert_eq!(t.call("s", "now", &json!({})).unwrap(), "ran now");
    assert!(cards.lock().unwrap().is_empty());
    assert_eq!(t.call("s", "note_save", &json!({})).unwrap(), "ran note_save");
    assert_eq!(cards.lock().unwrap().len(), 1);
    assert!(cards.lock().unwrap()[0].starts_with("step tool note_save: save the note"));
    // a no is the step's answer in words and nothing ran
    answer.store(0, O::SeqCst);
    assert_eq!(t.call("s", "note_save", &json!({})).unwrap_err(), "the person said no");
    assert_eq!(ran.0.lock().unwrap().iter().filter(|n| *n == "note_save").count(), 1);
    assert!(t.call("s", "router_run", &json!({})).is_err(), "the router's own tools are never reachable from a step");
}

// ---------------------------------------------------------------------------------------------- over the provider path

#[test]
fn through_the_providers_a_model_server_sees_two_steps_at_once_and_the_tool_round_trip() {
    use iyakki_proxy::{Upstream, UpstreamResponse};
    struct Mock {
        inflight: AtomicU32,
        peak: AtomicU32,
        bodies: Mutex<Vec<String>>,
    }
    impl Upstream for Mock {
        fn request(&self, _: &str, _: &str, _: &[(String, String)], body: &[u8]) -> Result<UpstreamResponse, String> {
            let b: Value = serde_json::from_slice(body).unwrap();
            let msgs = b["messages"].as_array().unwrap();
            let (sys, last) = (msgs[0]["content"].as_str().unwrap().to_string(), msgs.last().unwrap()["content"].as_str().unwrap().to_string());
            self.bodies.lock().unwrap().push(b.to_string());
            let n = self.inflight.fetch_add(1, O::SeqCst) + 1;
            self.peak.fetch_max(n, O::SeqCst);
            std::thread::sleep(Duration::from_millis(100));
            self.inflight.fetch_sub(1, O::SeqCst);
            let text = if b["model"] == "big-70b" {
                json!({"goal": "g", "steps": [
                    {"id": "x", "title": "first", "task": "read note x", "needs": {"tools": true}},
                    {"id": "y", "title": "second", "task": "read note y", "needs": {"tools": true}}]}).to_string()
            } else if last.contains("the note says hello") {
                r#"{"status":"done","result":"read it"}"#.to_string()
            } else if sys.contains("Tools: read_note") {
                r#"{"status":"tool","tool":"read_note","args":{}}"#.to_string()
            } else {
                FAILED.to_string()
            };
            Ok(UpstreamResponse { status: 200, body: json!({"choices": [{"message": {"content": text}}], "usage": {"prompt_tokens": 5, "completion_tokens": 3}}).to_string().into_bytes() })
        }
    }
    let up = Arc::new(Mock { inflight: AtomicU32::new(0), peak: AtomicU32::new(0), bodies: Mutex::new(vec![]) });
    let mut cfg = ProviderCfg::new("lan", Kind::Compat);
    cfg.base_url = Some("http://192.168.1.5/v1".into());
    cfg.models = vec!["big-70b".into(), "small-7b".into()];
    let mut over = BTreeMap::new();
    over.insert("lan/big-70b".to_string(), ProfileOverride { cost: Some(Cost::High), ..Default::default() });
    let profiles = build(&[cfg.clone()], &over);
    let (up2, c2) = (up.clone(), cfg.clone());
    let pool = Arc::new(crate::router_pool::ProviderPool::new(Arc::new(move |_| Ok(crate::providers::ProviderClient::new(c2.clone(), crate::vault::Secrets::new(None, up2.clone()), up2.clone())))));
    let tools = Arc::new(FakeTools { calls: Mutex::new(vec![]) });
    let mut r = Router::new(par(2), &profiles, pool, Arc::new(DefaultStrategy), Arc::new(|_, _| {})).with_tools(tools.clone());
    let rep = r.run("g");
    assert_eq!(rep.outcome, RunOutcome::Finished, "{}", rep.summary());
    assert_eq!(up.peak.load(O::SeqCst), 2, "the model server had two requests at once");
    assert_eq!(tools.calls.lock().unwrap().len(), 2, "each step used the tool once");
    assert_eq!(rep.meter.calls, 5, "plan + two steps with a tool round trip each");
    assert_eq!(rep.meter.input, 25);
}

#[test]
fn the_microphone_is_heard_only_after_the_core_says_yes_and_comes_back_as_words() {
    use crate::router::{GatedListener, Listener};
    let door = Arc::new(iyakki_core_fake::FakeRouter::default());
    let recorded = Arc::new(Mutex::new(vec![]));
    let r2 = recorded.clone();
    let l = GatedListener { gate: door.clone(), record: Arc::new(move |s| { r2.lock().unwrap().push(s); Ok("turn the lights off".to_string()) }) };
    assert_eq!(l.hear(5).unwrap(), "turn the lights off");
    assert_eq!(*recorded.lock().unwrap(), vec![5]);
    assert!(door.calls().iter().any(|c| c.starts_with("allow_listen(listen for 5 s")), "{:?}", door.calls());
    // seconds are kept inside 1 to 15
    l.hear(600).unwrap();
    assert_eq!(recorded.lock().unwrap()[1], 15);
    // a no from the core: nothing is recorded
    door.refuse(true);
    assert!(l.hear(5).is_err());
    assert_eq!(recorded.lock().unwrap().len(), 2, "a refused card means no recording");
}

#[test]
fn a_webassembly_strategy_decides_inside_the_contract_the_sample_is_accepted_and_a_bad_module_is_refused() {
    use crate::router::Strategy;
    use crate::router_wasm::{make, WasmStrategy, SAMPLE_FIRST_MODEL};
    // the sample passes the same trial run a script passes
    let s = make("sample", SAMPLE_FIRST_MODEL).unwrap();
    crate::router::try_strategy(s.as_ref()).unwrap();
    // the plan phase names the first listed model; the assign phase gives the first ready step to it
    let plan = json!({"v": 1, "phase": "plan", "goal_chars": 3, "models": [{"id": "lan/big-70b"}], "steps": [], "ready": []});
    assert_eq!(s.decide(&plan).unwrap(), json!({"v": 1, "planner": "lan/big-70b"}));
    let assign = json!({"v": 1, "phase": "assign", "goal_chars": 3, "models": [{"id": "lan/small-7b"}, {"id": "lan/big-70b"}], "steps": [], "ready": ["s1", "s2"]});
    assert_eq!(s.decide(&assign).unwrap(), json!({"v": 1, "assign": [{"step": "s1", "model": "lan/small-7b"}]}));
    let esc = json!({"v": 1, "phase": "escalate", "goal_chars": 3, "models": [], "steps": [], "ready": []});
    assert_eq!(s.decide(&esc).unwrap(), json!({"v": 1}));
    // the hexadecimal form of a module is the same strategy
    let wat = r#"(module (import "iyakki" "log" (func $l (param i32 i32))) (memory (export "memory") 1) (data (i32.const 0) "{\"v\":1}") (func (export "decide") (result i32) (call $l (i32.const 0) (i32.const 7)) (i32.const 0)))"#;
    let hex: String = wat::parse_str(wat).unwrap().iter().map(|b| format!("{b:02x}")).collect();
    let h = make("viahex", &format!("wasm-hex:{hex}")).unwrap();
    assert_eq!(h.decide(&json!({"phase": "x"})).unwrap(), json!({"v": 1}));
    // a module that loops forever, one without decide, one that traps, one that answers garbage and one that asks for a file are all refused
    let looping = r#"(module (memory (export "memory") 1) (func (export "decide") (result i32) (loop br 0) (i32.const 0)))"#;
    assert!(WasmStrategy::new("looper", looping).unwrap_err().starts_with("timeout"));
    assert!(WasmStrategy::new("nodecide", r#"(module (func (export "main")))"#).unwrap_err().contains("no decide"));
    assert!(WasmStrategy::new("trap", r#"(module (func (export "decide") (result i32) unreachable))"#).unwrap_err().starts_with("error"));
    let garbage = r#"(module (import "iyakki" "log" (func $l (param i32 i32))) (memory (export "memory") 1) (data (i32.const 0) "nope") (func (export "decide") (result i32) (call $l (i32.const 0) (i32.const 4)) (i32.const 0)))"#;
    assert!(WasmStrategy::new("garbage", garbage).unwrap_err().starts_with("garbage"));
    let reads_a_file = r#"(module (import "iyakki" "fs_read" (func $r (param i32 i32 i32 i32 i32) (result i32))) (memory (export "memory") 1) (data (i32.const 64) "/etc/passwd") (func (export "decide") (result i32) (call $r (i32.const 0) (i32.const 64) (i32.const 11) (i32.const 128) (i32.const 64)) drop (i32.const 0)))"#;
    let w = WasmStrategy::new("filer", reads_a_file);
    assert!(w.is_err() || { let s = w.unwrap(); s.decide(&json!({})).is_err() }, "no capability token, so the read can never succeed");
    // the name rules are the script's
    assert!(WasmStrategy::new("default", SAMPLE_FIRST_MODEL).is_err());
}
