//! The fleet tools and the one door (`FleetApi`) the chat and the Settings page share (F-FLEET-07, F-FLEET-09): reads are free, every change
//! asks the core first, a refusal changes nothing, no secret or join code reaches the assistant's status, and the tools are hard asks.
//! The core is the fake machine; the other system is a second hub over a real loopback socket.

use std::sync::{Arc, Mutex};

use serde_json::{json, Value};

use iyakki_core_api::Os as _;
use crate::agent::{hard_ask, Risk, Tools};
use crate::fleet::{FixedSource, Heartbeat};
use crate::fleet_hub::Hub;
use crate::fleettools::*;

struct NoInner;
impl Tools for NoInner {
    fn defs(&self) -> Vec<crate::agent::ToolDef> {
        vec![]
    }
    fn call(&self, n: &str, _: &Value) -> Result<String, String> {
        Err(format!("inner: {n}"))
    }
    fn describe(&self, _: &str, _: &Value) -> String {
        String::new()
    }
}

struct Side {
    api: FleetApi,
    fake: Arc<iyakki_core_fake::FakeOs>,
    lines: Arc<Mutex<Vec<String>>>,
    tools: FleetTools,
}

fn hb() -> Heartbeat {
    Heartbeat { disk_free_mb: 1, disk_total_mb: 2, cpu_load_pct: 3, cpus: 4, mem_used_pct: 5, mem_total_mb: 6, tasks_running: 0, models: vec![] }
}

fn side(tag: &str) -> Side {
    let hub = Arc::new(Hub::load(&crate::testdir::outside_project("fleettools", tag)).unwrap());
    hub.set_bind("127.0.0.1:0");
    let fake = Arc::new(iyakki_core_fake::FakeOs::new());
    let lines = Arc::new(Mutex::new(vec![]));
    let l2 = lines.clone();
    let api = FleetApi::new(hub, fake.fleet(), Arc::new(FixedSource(hb())), Arc::new(move |w, d| l2.lock().unwrap().push(format!("{w} {d}"))));
    let tools = FleetTools::new(Arc::new(NoInner), api.clone());
    Side { api, fake, lines, tools }
}

fn call(s: &Side, name: &str, a: Value) -> Result<String, String> {
    s.tools.call(name, &a)
}

#[test]
fn status_is_a_free_read_that_asks_only_the_read_door_and_never_shows_a_code_or_a_secret() {
    let a = side("st");
    assert!(call(&a, STATUS, json!({})).unwrap().contains("in no fleet"));
    assert_eq!(a.fake.fleet_fake().calls(), vec!["read(fleet members and numbers)"]);
    call(&a, PAIR, json!({"action": "create", "name": "lab", "my_name": "alpha"})).unwrap();
    let offer = call(&a, PAIR, json!({"action": "offer"})).unwrap();
    let code: String = offer.split("Join code ").nth(1).unwrap().chars().take(11).collect();
    assert_eq!(code.len(), 11, "{offer}");
    let status = call(&a, STATUS, json!({})).unwrap();
    assert!(!status.contains(&code) && !status.contains(&code.replace('-', "")), "the code is shown to the person who asked, not in the status: {status}");
    assert!(status.contains("Fleet lab") && status.contains("alpha"), "{status}");
    a.api.hub.stop_node();
}

#[test]
fn every_change_asks_the_core_with_the_right_capability_and_a_refusal_changes_nothing() {
    let a = side("ask");
    let f = a.fake.fleet_fake();
    f.refuse(true);
    assert!(call(&a, PAIR, json!({"action": "create", "name": "lab", "my_name": "alpha"})).unwrap_err().contains("refused"));
    assert!(a.api.hub.status().fleets.is_empty() && a.api.hub.listening().is_none(), "no fleet and no listener after a refusal");
    f.refuse(false);
    call(&a, PAIR, json!({"action": "create", "name": "lab", "my_name": "alpha"})).unwrap();
    f.refuse(true);
    for (t, args) in [
        (PAIR, json!({"action": "offer"})),
        (PAIR, json!({"action": "join", "address": "127.0.0.1:9", "code": "123456789", "my_name": "x"})),
        (PAIR, json!({"action": "remove", "member": "alpha"})),
        (PAIR, json!({"action": "admin", "member": "alpha", "on": true})),
        (LEAVE, json!({})),
        (ASSIGN, json!({"to": "alpha", "title": "t", "goal": "g"})),
        (RECEIVE, json!({"action": "mode", "mode": "never"})),
        (RECEIVE, json!({"action": "trust", "member": "alpha"})),
        (RECEIVE, json!({"action": "accept", "task": "abcdef"})),
    ] {
        let e = call(&a, t, args.clone()).unwrap_err();
        assert!(e.contains("refused") || e.contains("no such") || e.contains("not a member") || e.contains("that is this computer"), "{t} {args}: {e}");
    }
    assert_eq!(a.api.hub.status().fleets[0].members.len(), 1, "still there");
    assert_eq!(a.api.hub.status().accept, crate::fleet_hub::Accept::Ask);
    let calls = f.calls().join(" ");
    for cap in ["manage(", "join(", "assign(", "receive("] {
        assert!(calls.contains(cap), "{cap} not asked: {calls}");
    }
    a.api.hub.stop_node();
}

#[test]
fn the_tools_are_listed_six_five_of_them_ask_and_the_hard_ask_list_names_them_so_auto_mode_asks_too() {
    let a = side("defs");
    let defs = a.tools.defs();
    let ours: Vec<_> = defs.iter().filter(|d| is_fleet_tool(&d.spec.name)).collect();
    assert_eq!(ours.len(), 6);
    for d in &ours {
        assert_eq!(d.risk == Risk::Safe, d.spec.name == STATUS, "{}", d.spec.name);
        if d.spec.name != STATUS {
            assert!(hard_ask(&d.spec.name, &json!({})).is_some(), "{} is a hard ask", d.spec.name);
        }
    }
    assert!(hard_ask(STATUS, &json!({})).is_none());
    assert!(a.tools.preview(ASSIGN, &json!({"to": "b", "title": "T", "goal": "the goal text"})).unwrap().contains("the goal text"), "the card shows the whole goal");
    assert!(a.tools.refusal(ASSIGN, &json!({"title": "t", "goal": "my password is x"})).unwrap().contains("secret"));
    assert!(a.tools.refusal(ASSIGN, &json!({"title": "t", "goal": "fine"})).is_none());
    assert!(a.tools.describe(PAIR, &json!({"action": "create", "name": "lab"})).contains("lab"));
    assert!(PROMPT_RULE.contains("ask the person every time") && PROMPT_RULE.contains("never shows a join code"));
    assert_eq!(crate::toolblock::group_of("fleet_pair"), crate::toolblock::Group::Machine);
    assert!(crate::toolblock::intent_groups("pair my computers into a fleet").contains(&crate::toolblock::Group::Machine));
}

#[test]
fn two_assistants_pair_two_computers_and_send_work_by_the_tools_alone() {
    let (a, b) = (side("duoa"), side("duob"));
    call(&a, PAIR, json!({"action": "create", "name": "lab", "my_name": "alpha"})).unwrap();
    let offer = call(&a, PAIR, json!({"action": "offer"})).unwrap();
    let code: String = offer.split("Join code ").nth(1).unwrap().chars().take(11).collect();
    let addr = a.api.hub.listening().unwrap().to_string();
    // b needs its channel up to be reachable: joining starts it
    let r = call(&b, PAIR, json!({"action": "join", "address": addr, "code": code, "my_name": "bravo"})).unwrap();
    assert!(r.contains("Asked"), "{r}");
    let sa = call(&a, STATUS, json!({})).unwrap();
    assert!(sa.contains("Join request") && sa.contains("bravo"), "{sa}");
    assert!(a.tools.preview(PAIR, &json!({"action": "approve"})).unwrap().contains("fingerprint"));
    call(&a, PAIR, json!({"action": "approve"})).unwrap();
    let sb = call(&b, STATUS, json!({})).unwrap();
    assert!(sb.contains("offered") && sb.contains("Compare the fingerprints"), "{sb}");
    assert!(b.tools.preview(PAIR, &json!({"action": "accept"})).unwrap().contains("approved by alpha"));
    call(&b, PAIR, json!({"action": "accept"})).unwrap();
    a.api.hub.settle();
    a.api.hub.heartbeat_tick(&FixedSource(hb()));
    b.api.hub.heartbeat_tick(&FixedSource(hb()));
    let sa = call(&a, STATUS, json!({})).unwrap();
    assert!(sa.contains("bravo") && sa.contains("up: disk"), "numbers of bravo on alpha: {sa}");
    // work goes b -> a, a's person accepts
    let sent = call(&b, ASSIGN, json!({"to": "alpha", "title": "fix the parser", "goal": "profile it", "base_branch": "main"})).unwrap();
    assert!(sent.contains("waiting"), "{sent}");
    let sa = call(&a, STATUS, json!({})).unwrap();
    assert!(sa.contains("Work in:") && sa.contains("fix the parser") && sa.contains("[waiting]"), "{sa}");
    let id = a.api.hub.status().inbox[0].id.clone();
    let r = call(&a, RECEIVE, json!({"action": "accept", "task": id[..8]})).unwrap();
    assert!(r.contains("No IDE workspace is open"), "honest about what accepting does when no workspace is open: {r}");
    a.api.hub.settle();
    assert!(call(&b, STATUS, json!({})).unwrap().contains("[accepted]"));
    // the audit lines carry ids and numbers, never the goal or a code
    let all = format!("{:?}{:?}", a.lines.lock().unwrap(), b.lines.lock().unwrap());
    assert!(all.contains("fleet.create") && all.contains("fleet.join.approve") && all.contains("fleet.assign") && all.contains("fleet.inbox.decide"), "{all}");
    assert!(!all.contains("profile it") && !all.contains("fix the parser") && !all.contains(&code) && !all.contains(&code.replace('-', "")), "{all}");
    // leaving
    call(&b, LEAVE, json!({})).unwrap();
    b.api.hub.settle();
    assert!(call(&b, STATUS, json!({})).unwrap().contains("in no fleet"));
    a.api.hub.stop_node();
    b.api.hub.stop_node();
}

#[test]
fn bad_arguments_are_said_plainly() {
    let a = side("bad");
    assert!(call(&a, PAIR, json!({"action": "dance"})).unwrap_err().contains("action is one of"));
    assert!(call(&a, PAIR, json!({"action": "join"})).unwrap_err().contains("address"));
    assert!(call(&a, PAIR, json!({"action": "create", "name": ""})).is_err());
    assert!(call(&a, RECEIVE, json!({"action": "mode", "mode": "sometimes"})).unwrap_err().contains("ask, never or members"));
    assert!(call(&a, ASSIGN, json!({"to": "x", "title": "", "goal": "g"})).is_err());
    assert!(call(&a, "other_tool", json!({})).unwrap_err().contains("inner"));
}

fn side_tls(tag: &str) -> Side {
    let dir = crate::testdir::outside_project("fleettools-tls", tag);
    let hub = Arc::new(Hub::load(&dir).unwrap());
    hub.set_bind("127.0.0.1:0");
    let fake = Arc::new(iyakki_core_fake::FakeOs::new());
    let lines = Arc::new(Mutex::new(vec![]));
    let l2 = lines.clone();
    let api = FleetApi::new(hub, fake.fleet(), Arc::new(FixedSource(hb())), Arc::new(move |w, d| l2.lock().unwrap().push(format!("{w} {d}")))).with_dir(&dir);
    let tools = FleetTools::new(Arc::new(NoInner), api.clone());
    Side { api, fake, lines, tools }
}

#[test]
fn two_assistants_pair_over_tls_by_the_tools_alone_and_the_address_and_tls_changes_ask_first() {
    let (a, b) = (side_tls("tlsa"), side_tls("tlsb"));
    // turning TLS on asks the core; a refusal changes nothing
    a.fake.fleet_fake().refuse(true);
    assert!(call(&a, PAIR, json!({"action": "tls", "on": true})).unwrap_err().contains("refused"));
    assert!(!a.api.hub.tls_wanted() && a.api.hub.tls_fingerprint().is_none());
    a.fake.fleet_fake().refuse(false);
    let r = call(&a, PAIR, json!({"action": "tls", "on": true})).unwrap();
    let fp = a.api.hub.tls_fingerprint().unwrap();
    assert!(r.contains("uses TLS") && r.contains(&crate::fleet_hub::colon_fp(&fp)), "{r}");
    assert!(a.fake.fleet_fake().calls().iter().any(|c| c.contains("TLS")), "{:?}", a.fake.fleet_fake().calls());
    call(&b, PAIR, json!({"action": "tls", "on": true})).unwrap();
    call(&a, PAIR, json!({"action": "create", "name": "lab", "my_name": "alpha"})).unwrap();
    let offer = call(&a, PAIR, json!({"action": "offer"})).unwrap();
    let code: String = offer.split("Join code ").nth(1).unwrap().chars().take(11).collect();
    let addr = format!("tls://{}#{}", a.api.hub.listening().unwrap(), crate::fleet_hub::colon_fp(&fp));
    let r = call(&b, PAIR, json!({"action": "join", "address": addr, "code": code, "my_name": "bravo"})).unwrap();
    assert!(r.contains("Asked"), "{r}");
    call(&a, PAIR, json!({"action": "approve"})).unwrap();
    call(&b, PAIR, json!({"action": "accept"})).unwrap();
    a.api.hub.settle();
    a.api.hub.heartbeat_tick(&FixedSource(hb()));
    b.api.hub.heartbeat_tick(&FixedSource(hb()));
    let sa = call(&a, STATUS, json!({})).unwrap();
    assert!(sa.contains("bravo") && sa.contains("over TLS") && sa.contains("up: disk"), "{sa}");
    // work goes over the TLS channel
    let sent = call(&b, ASSIGN, json!({"to": "alpha", "title": "over tls", "goal": "g", "base_branch": "main"})).unwrap();
    assert!(sent.contains("waiting"), "{sent}");
    assert_eq!(a.api.hub.status().inbox.len(), 1);
    // an address is set with a card; a bad one is said plainly
    let who = a.api.hub.status().fleets[0].members.iter().find(|m| !m.me).unwrap().fingerprint.clone();
    a.fake.fleet_fake().refuse(true);
    assert!(call(&a, PAIR, json!({"action": "address", "member": who, "address": "tls://example.org:7671"})).unwrap_err().contains("refused"));
    a.fake.fleet_fake().refuse(false);
    assert!(call(&a, PAIR, json!({"action": "address", "member": who, "address": "tls://example.org:7671"})).unwrap_err().contains("fingerprint"));
    let ok = call(&a, PAIR, json!({"action": "address", "member": who, "address": format!("tls://localhost:7671#{}", crate::fleet_hub::colon_fp(&fp))})).unwrap();
    assert!(ok.contains("tls://localhost:7671#"), "{ok}");
    let all = format!("{:?}{:?}", a.lines.lock().unwrap(), b.lines.lock().unwrap());
    assert!(all.contains("fleet.tls.set") && all.contains("fleet.member.address") && !all.contains(&code) && !all.contains(&fp), "{all}");
    assert!(call(&a, PAIR, json!({"action": "tls", "on": false})).unwrap().contains("plain again"));
    assert!(a.api.hub.tls_fingerprint().is_none() && !a.api.hub.tls_wanted());
    a.api.hub.stop_node();
    b.api.hub.stop_node();
}
