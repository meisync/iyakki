//! The cloud tenant's hooks in the engine: a replica without the lease starts nothing on a timer and a replica that takes over reads
//! the scheduler state the previous leader wrote (no tick is fired twice); an admission gate holds a queued run back; `local` in a cron
//! trigger means the tenant's zone.

mod common;
use common::*;
use iyakki_flow::engine::Gate;
use iyakki_flow::host::{FakeClock, FakeHost};
use iyakki_flow::*;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;

fn engine_with(dir: &std::path::Path, clock: &Arc<FakeClock>, host: &Arc<FakeHost>, gate: Option<Gate>, tz: Option<&str>) -> (Engine, Arc<FlowStore>) {
    let c2 = clock.clone();
    let store = Arc::new(FlowStore::new(&dir.join("flows"), Arc::new(move || c2.now_ms())));
    let mut cfg = EngineCfg::new(&dir.join("engine"));
    cfg.gate = gate;
    cfg.local_tz = tz.map(String::from);
    (Engine::new(cfg, host.clone(), Arc::new(Registry::builtin()), clock.clone(), store.clone()), store)
}

fn publish(store: &FlowStore, f: Flow) {
    use iyakki_flow::store::PublishReq;
    let id = f.id.clone();
    store.create(f, "user:test").unwrap();
    store.publish(&id, &PublishReq { by: "user:test", label: "", changelog: "t", approved_by: None }).unwrap();
}

fn every_minute() -> Flow {
    let mut f = linear("tick");
    f.triggers.push(Trigger::new("t", TriggerKind::Interval { every_s: 60, anchor_ms: None }));
    f
}

#[test]
fn a_replica_without_the_lease_starts_nothing_on_a_timer_and_a_new_leader_does_not_repeat_a_tick() {
    let dir = tmp("gate-lead");
    let (clock, host) = (FakeClock::new(1_000_000), FakeHost::new());
    host.on_http(|_| ok_json("{}"));
    let (lead_a, lead_b) = (Arc::new(AtomicBool::new(true)), Arc::new(AtomicBool::new(false)));
    let gate = |l: &Arc<AtomicBool>| Gate { lead: { let l = l.clone(); Arc::new(move || l.load(Ordering::SeqCst)) }, admit: Arc::new(|_, _| true) };
    let (a, store_a) = engine_with(&dir, &clock, &host, Some(gate(&lead_a)), None);
    let (b, _store_b) = engine_with(&dir, &clock, &host, Some(gate(&lead_b)), None);
    publish(&store_a, every_minute());
    a.tick();
    b.tick();
    clock.advance(61_000);
    a.tick();
    b.tick();
    assert_eq!(a.runs_of("tick").len(), 1, "the leader fired");
    assert_eq!(b.runs_of("tick").len(), 0, "the standby did not");
    assert!(a.wait_idle(Duration::from_secs(5)));
    // the lease moves: b leads now and must not fire the tick a already fired, though b's own memory is old
    lead_a.store(false, Ordering::SeqCst);
    lead_b.store(true, Ordering::SeqCst);
    b.tick();
    assert_eq!(b.runs_of("tick").len(), 0, "the tick of 61 s was a's; b read the state a saved");
    clock.advance(60_000);
    b.tick();
    a.tick();
    assert_eq!(b.runs_of("tick").len(), 1, "the next tick belongs to the new leader");
    assert_eq!(a.runs_of("tick").len(), 1, "and the old one does nothing more");
}

#[test]
fn a_missed_tick_while_nobody_led_is_handled_by_the_missed_policy_at_takeover() {
    let dir = tmp("gate-missed");
    let (clock, host) = (FakeClock::new(1_000_000), FakeHost::new());
    host.on_http(|_| ok_json("{}"));
    let lead = Arc::new(AtomicBool::new(true));
    let gate = Gate { lead: { let l = lead.clone(); Arc::new(move || l.load(Ordering::SeqCst)) }, admit: Arc::new(|_, _| true) };
    let (e, store) = engine_with(&dir, &clock, &host, Some(gate), None);
    publish(&store, every_minute());
    e.tick();
    lead.store(false, Ordering::SeqCst);
    clock.advance(10 * 60_000); // ten minutes with no leader: a pod restart
    e.tick();
    assert_eq!(e.runs_of("tick").len(), 0);
    lead.store(true, Ordering::SeqCst);
    e.tick();
    assert_eq!(e.runs_of("tick").len(), 1, "the default policy runs once for the missed ticks");
    assert!(e.get_run(&e.runs_of("tick")[0].id).unwrap().trigger.detail.contains("missed"));
}

#[test]
fn the_admission_gate_holds_a_queued_run_back_until_it_says_yes() {
    let dir = tmp("gate-admit");
    let (clock, host) = (FakeClock::new(1_000_000), FakeHost::new());
    host.on_http(|_| ok_json("{}"));
    let yes = Arc::new(AtomicBool::new(false));
    let asked = Arc::new(AtomicUsize::new(0));
    let gate = Gate { lead: Arc::new(|| true), admit: { let (y, a) = (yes.clone(), asked.clone()); Arc::new(move |_, _| { a.fetch_add(1, Ordering::SeqCst); y.load(Ordering::SeqCst) }) } };
    let (e, store) = engine_with(&dir, &clock, &host, Some(gate), None);
    publish(&store, linear("job"));
    let id = e.start_run(StartRequest::new("job")).unwrap().run_id;
    std::thread::sleep(Duration::from_millis(100));
    assert_eq!(e.get_run(&id).unwrap().status, RunStatus::Queued, "held back, not failed");
    assert!(asked.load(Ordering::SeqCst) >= 1);
    yes.store(true, Ordering::SeqCst);
    e.pump();
    let r = e.wait_for(&id, Duration::from_secs(5), |r| r.status.is_terminal()).unwrap();
    assert_eq!(r.status, RunStatus::Succeeded);
}

#[test]
fn local_in_a_cron_trigger_means_the_tenants_zone() {
    let dir = tmp("gate-tz");
    let (clock, host) = (FakeClock::new(1_000_000), FakeHost::new());
    let (e, store) = engine_with(&dir, &clock, &host, None, Some("+05:30"));
    let mut f = linear("morning");
    f.triggers.push(Trigger::new("t", TriggerKind::Cron { expr: "0 9 * * *".into(), tz: "local".into() }));
    publish(&store, f);
    let fl = store.published("morning").unwrap();
    let next = e.next_runs(&fl, 1)[0].1;
    // 09:00 at +05:30 is 03:30 UTC
    assert_eq!((next / 1000).rem_euclid(86_400), 3 * 3600 + 30 * 60);
}

#[test]
fn the_tenants_history_rule_lists_first_then_removes_and_the_tick_applies_it_once_an_hour() {
    use iyakki_flow::engine::RetentionSource;
    use iyakki_flow::runstore::Retention;
    let dir = tmp("gate-retention");
    let (clock, host) = (FakeClock::new(1_000_000_000), FakeHost::new());
    host.on_http(|_| ok_json("{}"));
    let c2 = clock.clone();
    let store = Arc::new(FlowStore::new(&dir.join("flows"), Arc::new(move || c2.now_ms())));
    let rule = Arc::new(std::sync::Mutex::new(Retention { keep: 100, days: 0 }));
    let mut cfg = EngineCfg::new(&dir.join("engine"));
    cfg.retention = Some(RetentionSource({ let r = rule.clone(); Arc::new(move || *r.lock().unwrap()) }));
    let e = Engine::new(cfg, host.clone(), Arc::new(Registry::builtin()), clock.clone(), store.clone());
    publish(&store, linear("job"));
    let mut ids = Vec::new();
    for _ in 0..4 {
        let id = e.start_run(StartRequest::new("job")).unwrap().run_id;
        e.wait_for(&id, Duration::from_secs(5), |r| r.status.is_terminal()).unwrap();
        ids.push(id);
        clock.advance(1000);
    }
    // nothing to remove under the generous rule
    assert!(e.retention_plan(None).is_empty());
    *rule.lock().unwrap() = Retention { keep: 2, days: 0 };
    // the dry run lists the two oldest and deletes nothing
    let plan = e.retention_plan(None);
    let listed: Vec<&str> = plan.iter().map(|p| p.id.as_str()).collect();
    assert_eq!(listed.len(), 2);
    assert!(listed.contains(&ids[0].as_str()) && listed.contains(&ids[1].as_str()), "{listed:?}");
    for id in &ids {
        assert!(dir.join("engine/runs").join(id).join("run.json").is_file(), "a dry run keeps every folder");
    }
    // the first tick applies the rule by itself; the new rule at the next hour
    e.tick();
    assert!(!dir.join("engine/runs").join(&ids[0]).exists() && !dir.join("engine/runs").join(&ids[1]).exists());
    assert!(dir.join("engine/runs").join(&ids[2]).exists() && dir.join("engine/runs").join(&ids[3]).exists());
    assert!(e.get_run(&ids[0]).is_none());
    assert!(host.audit_lines().iter().any(|a| a.contains("flow.run.prune") && a.contains(&ids[0])));
    *rule.lock().unwrap() = Retention { keep: 1, days: 0 };
    clock.advance(10 * 60_000);
    e.tick();
    assert!(dir.join("engine/runs").join(&ids[2]).exists(), "not asked again within the hour");
    clock.advance(60 * 60_000);
    e.tick();
    assert!(!dir.join("engine/runs").join(&ids[2]).exists() && dir.join("engine/runs").join(&ids[3]).exists());
}

#[test]
fn the_engine_asks_the_host_for_every_secret_on_behalf_of_the_run_and_a_refusal_stops_the_step() {
    let dir = tmp("secret-gate");
    let (clock, host) = (FakeClock::new(1_000_000), FakeHost::new());
    host.on_http(|_| ok_json("{}"));
    host.secret_is("tok", "s3cret");
    // the host's rule: only a run a person started may read; anything else is refused the way a cloud tenant does
    host.secret_rule(|run, name| if run.attended() { None } else { Some(Err(format!("needs secret: `{name}` is not marked for scheduled runs"))) });
    let (e, store) = engine_with(&dir, &clock, &host, None, None);
    let mut f = linear("job");
    f.nodes[1].config = serde_json::json!({"url": "https://api.example.com/a", "headers": {"Authorization": "Bearer {{secret.tok}}"}});
    publish(&store, f);
    // a person starts it: the secret is read, and the host was told who and what
    let id = e.start_run(StartRequest::new("job").trigger("manual", "user:ann")).unwrap().run_id;
    let r = e.wait_for(&id, Duration::from_secs(5), |r| r.status.is_terminal()).unwrap();
    assert_eq!(r.status, RunStatus::Succeeded);
    assert_eq!(host.http_calls.lock().unwrap().last().unwrap().headers.iter().find(|h| h.0 == "Authorization").unwrap().1, "Bearer s3cret");
    // a timer starts it: refused, the request never leaves, and the reason is the run's error
    let id2 = e.start_run(StartRequest::new("job").trigger("interval", "trigger t")).unwrap().run_id;
    let r2 = e.wait_for(&id2, Duration::from_secs(5), |r| r.status.is_terminal() || r.status == RunStatus::Blocked).unwrap();
    assert_ne!(r2.status, RunStatus::Succeeded);
    assert_eq!(host.http_calls.lock().unwrap().len(), 1, "no request was made with a refused secret");
    let blob = serde_json::to_string(&r2).unwrap();
    assert!(blob.contains("needs secret") && !blob.contains("s3cret"), "{blob}");
    let asks = host.secret_asks.lock().unwrap().clone();
    assert!(asks.iter().any(|(c, n)| c.trigger == "manual" && c.flow_id == "job" && c.run_id == id && n == "tok"));
    assert!(asks.iter().any(|(c, _)| c.trigger == "interval" && c.run_id == id2));
}

#[test]
fn a_notify_step_hands_its_to_to_the_host_and_a_desktop_host_still_makes_one_note() {
    let dir = tmp("notify-to");
    let (clock, host) = (FakeClock::new(1_000_000), FakeHost::new());
    let (e, store) = engine_with(&dir, &clock, &host, None, None);
    let mut f = Flow::new("tell", "tell");
    f.nodes.push(Node::new("s", "start", serde_json::json!({})));
    f.nodes.push(Node::new("n", "notify", serde_json::json!({"title": "disk full", "to": ["mia", "vic"]})));
    f.nodes.push(Node::new("m", "notify", serde_json::json!({"title": "no to"})));
    f.edges.push(Edge::new("s", "n"));
    f.edges.push(Edge::new("n", "m"));
    publish(&store, f);
    let id = e.start_run(StartRequest::new("tell")).unwrap().run_id;
    let r = e.wait_for(&id, Duration::from_secs(5), |r| r.status.is_terminal()).unwrap();
    assert_eq!(r.status, RunStatus::Succeeded);
    let tos = host.notify_tos.lock().unwrap().clone();
    assert_eq!(tos, vec![serde_json::json!(["mia", "vic"]), serde_json::Value::Null]);
    let titles: Vec<String> = host.notices.lock().unwrap().iter().filter(|n| !n.needs_you).map(|n| n.title.clone()).collect();
    assert!(titles.contains(&"disk full".to_string()) && titles.contains(&"no to".to_string()), "{titles:?}");
}
