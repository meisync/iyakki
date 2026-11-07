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
