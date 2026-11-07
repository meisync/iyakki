//! Tests of the cloud flows pieces, with a fake clock (every function takes the time) and fake engines.

use super::approval::{OnTimeout, Resolution, Resolver, Waits};
use super::executor::{Decision, Executor, InProcess, K3sJob, RunRequest};
use super::identity::*;
use super::job::{bundle_text, registry_of, JobSpec};
use crate::sandbox::policy::SandboxPolicy;
use super::lease::{Acquire, Lease};
use super::notify::*;
use crate::notifyhub::service::{Hub, HubEnv};
use super::quota::*;
use super::service::*;
use super::tz;
use crate::cloudroles::{NsRole, Roles, TenantRole};
use crate::tenantconfig::api::{Caller, ConfigAudit, Env};
use crate::tenantconfig::approvals;
use crate::tenantconfig::schema::SecretRef;
use crate::tenantconfig::vault::{MemVault, SecretState, SecretStore};
use serde_json::json;
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

const T0: u64 = 1_800_000_000; // a Friday-ish instant; only differences matter

fn dir(tag: &str) -> PathBuf {
    let d = crate::testdir::unique("cloudflows", tag);
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

/// olga owner, adam tenant admin, mia admin of web, max editor of web, vic admin of data, zed a member outside web.
fn roles() -> Roles {
    let mut r = Roles::new("olga").unwrap();
    r.add_tenant_member("olga", "adam", TenantRole::Admin).unwrap();
    for m in ["mia", "max", "vic", "zed"] {
        r.add_tenant_member("olga", m, TenantRole::Member).unwrap();
    }
    r.create_namespace("adam", "web").unwrap();
    r.add_namespace_member("adam", "web", "mia", NsRole::Admin).unwrap();
    r.add_namespace_member("mia", "web", "max", NsRole::Editor).unwrap();
    r.create_namespace("adam", "data").unwrap();
    r.add_namespace_member("adam", "data", "vic", NsRole::Admin).unwrap();
    r.namespaces.get_mut("web").unwrap().remove("adam");
    r.namespaces.get_mut("data").unwrap().remove("adam");
    r
}

#[derive(Default)]
struct Rec(Mutex<Vec<String>>);
impl ConfigAudit for Rec {
    fn record(&self, who: &str, event: &str, detail: &str) {
        self.0.lock().unwrap().push(format!("{who} {event} {detail}"));
    }
}
impl Rec {
    fn all(&self) -> String {
        self.0.lock().unwrap().join("\n")
    }
}

/// The tenant's real notification hub over a fake environment: what the scheduler emits lands in people's INBOX files.
struct HEnv(Roles);
impl HubEnv for HEnv {
    fn roles(&self) -> Option<Roles> {
        Some(self.0.clone())
    }
    fn channels(&self) -> Vec<crate::notifyhub::channel::Channel> {
        vec![]
    }
    fn rules(&self, _: Option<&str>) -> crate::notifyhub::rules::Rules {
        crate::notifyhub::rules::Rules::default()
    }
    fn secret(&self, _: &str) -> Option<String> {
        None
    }
    fn approvals(&self) -> Vec<crate::tenantconfig::approvals::Pending> {
        vec![]
    }
}

struct Sink {
    hub: Arc<Hub>,
}
impl NotifySink for Sink {
    fn emit(&self, n: &Notification) -> Result<Receipt, String> {
        self.hub.emit(n)
    }
}
const PEOPLE: [&str; 7] = ["adam", "olga", "mia", "max", "vic", "zed", "solo"];
impl Sink {
    fn new(d: &PathBuf) -> Arc<Sink> {
        Arc::new(Sink { hub: Arc::new(Hub::new(d, "acme", Box::new(HEnv(roles())), "test")) })
    }
    /// The titles in one person's INBOX.
    fn to(&self, who: &str) -> Vec<String> {
        self.hub.inbox.list(who).iter().map(|n| n.title.clone()).collect()
    }
    fn titles(&self) -> Vec<String> {
        PEOPLE.iter().flat_map(|p| self.to(p).into_iter().map(move |t| format!("{p}:{t}"))).collect()
    }
    fn count(&self, who: &str, title: &str) -> usize {
        self.to(who).iter().filter(|t| t.starts_with(title)).count()
    }
}

#[derive(Default)]
struct FakeBackend {
    accounts: Vec<String>,
    runs: Mutex<Vec<(String, RunInfo)>>,
    started: Mutex<Vec<String>>,
    cancelled: Mutex<Vec<String>>,
    paused: Mutex<Vec<(String, String, bool)>>,
    stopped: AtomicUsize,
}
impl Backend for FakeBackend {
    fn accounts(&self) -> Vec<String> {
        self.accounts.clone()
    }
    fn start(&self, user: &str) -> Result<(), String> {
        self.started.lock().unwrap().push(user.to_string());
        Ok(())
    }
    fn stop_all(&self) {
        self.stopped.fetch_add(1, Ordering::SeqCst);
    }
    fn runs(&self, user: &str) -> Vec<RunInfo> {
        self.runs.lock().unwrap().iter().filter(|(u, _)| u == user).map(|(_, r)| r.clone()).collect()
    }
    fn cancel(&self, _u: &str, run: &str) -> Result<(), String> {
        self.cancelled.lock().unwrap().push(run.to_string());
        Ok(())
    }
    fn pause(&self, u: &str, f: &str, p: bool) {
        self.paused.lock().unwrap().push((u.into(), f.into(), p));
    }
}

fn run(id: &str, flow: &str, status: &str) -> RunInfo {
    RunInfo { id: id.into(), flow: flow.into(), status: status.into(), created_ms: T0 as i64 * 1000, started_ms: T0 as i64 * 1000, due_ms: None, scheduled: true, missed: false, error: String::new(), blocked: String::new() }
}

fn svc(d: &PathBuf, holder: &str, sink: &Arc<Sink>) -> TenantScheduler {
    TenantScheduler::new(d, "acme", holder, 30, 5, sink.clone())
}

// ------------------------------------------------------------------------------------------------------------------ lease

#[test]
fn lease_is_taken_over_only_after_it_expires() {
    let d = dir("lease1");
    let (a, b) = (Lease::new(&d, "pod-a", 30), Lease::new(&d, "pod-b", 30));
    assert_eq!(a.try_acquire(T0), Acquire::Held(1));
    assert!(matches!(b.try_acquire(T0 + 10), Acquire::Busy { ref holder, expires } if holder == "pod-a" && expires == T0 + 30));
    assert_eq!(a.heartbeat(T0 + 20), Ok(1));
    assert!(matches!(b.try_acquire(T0 + 40), Acquire::Busy { .. }), "the heartbeat moved the end to T0+50");
    assert_eq!(b.try_acquire(T0 + 51), Acquire::Held(2), "expired: taken over with a higher epoch");
    assert!(a.heartbeat(T0 + 52).is_err(), "the old holder finds out at its next heartbeat");
}

#[test]
fn lease_release_lets_the_next_replica_in_at_once() {
    let d = dir("lease2");
    let (a, b) = (Lease::new(&d, "pod-a", 30), Lease::new(&d, "pod-b", 30));
    a.try_acquire(T0);
    a.release();
    assert_eq!(b.try_acquire(T0 + 1), Acquire::Held(2));
}

#[test]
fn two_replicas_never_both_lead_and_the_loser_stops_its_engines() {
    let d = dir("lease3");
    let sink = Sink::new(&d);
    let (mut a, mut b) = (svc(&d, "pod-a", &sink), svc(&d, "pod-b", &sink));
    let (ba, bb) = (FakeBackend { accounts: vec!["max".into()], ..Default::default() }, FakeBackend { accounts: vec!["max".into()], ..Default::default() });
    let (r, q) = (roles(), Quota::default());
    let mut now = T0;
    for _ in 0..6 {
        a.step(now, &ba, &r, &q);
        b.step(now, &bb, &r, &q);
        assert!(!(a.is_leader() && b.is_leader()), "double leader at {now}");
        now += 5;
    }
    assert!(a.is_leader() && !b.is_leader());
    assert_eq!(ba.started.lock().unwrap().len(), 1);
    assert!(bb.started.lock().unwrap().is_empty(), "the standby started nothing, so nothing can run twice");
    // pod-a stops stepping (frozen, rescheduled); after the lease time pod-b takes over and starts its engines
    now += 40;
    b.step(now, &bb, &r, &q);
    assert!(b.is_leader());
    assert_eq!(bb.started.lock().unwrap().as_slice(), ["max"]);
    // pod-a wakes up: its heartbeat finds another name, it stops every engine
    let rep = a.step(now + 1, &ba, &r, &q);
    assert!(matches!(rep.role, Role::Standby { ref holder } if holder == "pod-b"));
    assert_eq!(ba.stopped.load(Ordering::SeqCst), 1);
    assert!(!a.is_leader());
}

#[test]
fn a_clean_stop_stops_engines_and_hands_the_lease_over() {
    let d = dir("lease4");
    let sink = Sink::new(&d);
    let mut a = svc(&d, "pod-a", &sink);
    let ba = FakeBackend { accounts: vec!["max".into()], ..Default::default() };
    a.step(T0, &ba, &roles(), &Quota::default());
    a.shutdown(&ba);
    assert_eq!(ba.stopped.load(Ordering::SeqCst), 1);
    assert_eq!(Lease::new(&super::dir(&d), "pod-b", 30).try_acquire(T0 + 1), Acquire::Held(2));
}

// ------------------------------------------------------------------------------------------------------------------ service

#[test]
fn schedules_start_with_no_person_signed_in() {
    // nothing here knows a session: the backend lists accounts that have flows and the service starts them
    let d = dir("nosession");
    let sink = Sink::new(&d);
    let mut s = svc(&d, "pod-a", &sink);
    let b = FakeBackend { accounts: vec!["max".into(), "mia".into()], ..Default::default() };
    let rep = s.step(T0, &b, &roles(), &Quota::default());
    assert_eq!(rep.started, ["max", "mia"]);
    assert_eq!(rep.role, Role::Leader);
    // a second pass starts nothing again
    assert!(s.step(T0 + 5, &b, &roles(), &Quota::default()).started.is_empty());
}

#[test]
fn an_owner_who_left_the_team_pauses_their_schedules_and_the_admins_are_told() {
    let d = dir("owner-gone");
    let sink = Sink::new(&d);
    let mut s = svc(&d, "pod-a", &sink);
    let mut r = roles();
    s.registry.upsert(make_schedule(&r, "max", "nightly", "web", "dev", RunAs::Owner, T0).unwrap()).unwrap();
    let b = FakeBackend { accounts: vec!["max".into()], ..Default::default() };
    assert!(s.step(T0, &b, &r, &Quota::default()).paused.is_empty());
    r.remove_namespace_member("mia", "web", "max").unwrap();
    let rep = s.step(T0 + 5, &b, &r, &Quota::default());
    assert_eq!(rep.paused, ["max/nightly"]);
    assert_eq!(b.paused.lock().unwrap().as_slice(), [("max".to_string(), "nightly".to_string(), true)]);
    assert!(s.registry.get("max", "nightly").unwrap().paused_reason.unwrap().contains("no longer in the team"));
    let told = sink.titles();
    assert!(told.contains(&"mia:Schedule paused: nightly".to_string()) && told.contains(&"adam:Schedule paused: nightly".to_string()), "{told:?}");
    // not again at the next pass
    assert!(s.step(T0 + 10, &b, &r, &Quota::default()).paused.is_empty());
}

#[test]
fn a_lowered_role_pauses_and_a_raised_role_does_not_raise_the_snapshot() {
    let mut r = roles();
    let sched = make_schedule(&r, "max", "f", "web", "dev", RunAs::Owner, T0).unwrap();
    assert_eq!(sched.role_snapshot, "editor");
    assert_eq!(check(&r, &sched), OwnerCheck::Ok);
    r.add_namespace_member("mia", "web", "max", NsRole::Admin).unwrap();
    assert_eq!(check(&r, &sched), OwnerCheck::Ok);
    assert_eq!(sched.role_snapshot, "editor", "the snapshot stays what it was");
    let admin = make_schedule(&r, "max", "g", "web", "dev", RunAs::Owner, T0).unwrap();
    r.add_namespace_member("mia", "web", "max", NsRole::Viewer).unwrap();
    assert!(matches!(check(&r, &admin), OwnerCheck::Pause(w) if w.contains("lower")));
    assert!(matches!(check(&r, &sched), OwnerCheck::Pause(_)), "a viewer may not own a schedule at all");
    assert!(make_schedule(&r, "max", "h", "web", "dev", RunAs::Owner, T0).is_err());
}

#[test]
fn the_team_identity_needs_an_admin_and_follows_that_admin() {
    let mut r = roles();
    assert!(make_schedule(&r, "max", "f", "web", "dev", RunAs::TeamService, T0).is_err(), "an editor may not choose the team identity");
    let s = make_schedule(&r, "mia", "f", "web", "dev", RunAs::TeamService, T0).unwrap();
    assert_eq!(check(&r, &s), OwnerCheck::Ok);
    r.namespaces.get_mut("web").unwrap().insert("mia".into(), NsRole::Editor);
    assert!(matches!(check(&r, &s), OwnerCheck::Pause(w) if w.contains("no longer an admin")));
}

#[test]
fn failed_scheduled_runs_notify_once_per_run_and_stop_after_the_daily_cap() {
    let d = dir("failed");
    let sink = Sink::new(&d);
    let mut s = svc(&d, "pod-a", &sink);
    let r = roles();
    s.registry.upsert(make_schedule(&r, "max", "nightly", "web", "dev", RunAs::Owner, T0).unwrap()).unwrap();
    let b = FakeBackend { accounts: vec!["max".into()], ..Default::default() };
    for i in 0..8 {
        let mut f = run(&format!("r{i}"), "nightly", "failed");
        f.error = "http 500".into();
        b.runs.lock().unwrap().push(("max".into(), f));
    }
    s.step(T0, &b, &r, &Quota::default());
    assert_eq!(sink.count("max", "Scheduled run failed") as u32, FAIL_NOTES_PER_DAY, "the cap holds");
    assert_eq!(sink.count("mia", "Scheduled run failed") as u32, FAIL_NOTES_PER_DAY, "the team admin hears too");
    let before = sink.titles().len();
    s.step(T0 + 5, &b, &r, &Quota::default());
    assert_eq!(sink.titles().len(), before, "the same runs are not reported again");
}

#[test]
fn a_run_over_its_time_limit_is_stopped_and_reported() {
    let d = dir("stuck");
    let sink = Sink::new(&d);
    let mut s = svc(&d, "pod-a", &sink);
    let r = roles();
    s.registry.upsert(make_schedule(&r, "max", "slow", "web", "dev", RunAs::Owner, T0).unwrap()).unwrap();
    let b = FakeBackend { accounts: vec!["max".into()], ..Default::default() };
    b.runs.lock().unwrap().push(("max".into(), run("r1", "slow", "running")));
    let q = Quota { max_run_secs: 600, ..Quota::default() };
    assert!(s.step(T0 + 500, &b, &r, &q).cancelled.is_empty());
    let rep = s.step(T0 + 700, &b, &r, &q);
    assert_eq!(rep.cancelled, ["r1"]);
    assert!(sink.titles().contains(&"max:Run stopped: slow".to_string()));
    assert_eq!(s.status(T0 + 700)["stuck_cancelled"], 1);
}

#[test]
fn a_quota_refusal_and_a_long_missed_run_and_a_blocked_secret_all_notify() {
    let d = dir("others");
    let sink = Sink::new(&d);
    let mut s = svc(&d, "pod-a", &sink);
    let r = roles();
    s.registry.upsert(make_schedule(&r, "max", "f", "web", "dev", RunAs::Owner, T0).unwrap()).unwrap();
    let b = FakeBackend { accounts: vec!["max".into()], ..Default::default() };
    let mut missed = run("r1", "f", "succeeded");
    missed.missed = true;
    missed.due_ms = Some((T0 as i64 - 3600) * 1000);
    let mut blocked = run("r2", "f", "blocked");
    blocked.blocked = "needs secret: the tenant vault is locked".into();
    b.runs.lock().unwrap().extend([("max".to_string(), missed), ("max".to_string(), blocked)]);
    s.counters.quota_refused.fetch_add(3, Ordering::Relaxed);
    s.step(T0, &b, &r, &Quota::default());
    let t = sink.titles();
    assert!(t.contains(&"max:Missed run: f".to_string()), "{t:?}");
    assert!(t.contains(&"max:Run blocked: f".to_string()), "{t:?}");
    assert!(t.contains(&"adam:Runs were held back by the quota".to_string()), "{t:?}");
    let st = s.status(T0);
    assert_eq!((st["quota_refused"].clone(), st["missed_notified"].clone(), st["runs"]["blocked"].clone()), (json!(3), json!(1), json!(1)));
}

#[test]
fn the_status_has_counters_and_no_names() {
    let d = dir("status");
    let sink = Sink::new(&d);
    let mut s = svc(&d, "pod-a", &sink);
    let b = FakeBackend { accounts: vec!["max".into()], ..Default::default() };
    b.runs.lock().unwrap().push(("max".into(), run("r1", "secretflowname", "running")));
    s.step(T0, &b, &roles(), &Quota::default());
    let st = s.status(T0 + 3).to_string();
    assert!(st.contains("\"leader\":true") && st.contains("\"running\":1") && st.contains("since_last_step_secs"));
    assert!(!st.contains("secretflowname") && !st.contains("max"));
}

// ------------------------------------------------------------------------------------------------------------------ identity

struct LockedVault;
impl SecretStore for LockedVault {
    fn state(&self, _r: &SecretRef) -> SecretState {
        SecretState::Locked
    }
    fn put(&self, _r: &SecretRef, _v: &str) -> Result<(), String> {
        Err("locked".into())
    }
    fn remove(&self, _r: &SecretRef) -> Result<(), String> {
        Err("locked".into())
    }
    fn use_secret(&self, _r: &SecretRef, _a: &str, _f: &mut dyn FnMut(&str) -> Result<String, String>) -> Result<String, String> {
        Err("locked".into())
    }
}

fn write_config(d: &PathBuf, items: serde_json::Value) {
    let c = d.join("cloud").join("config");
    std::fs::create_dir_all(&c).unwrap();
    std::fs::write(c.join("tenant-config.json"), json!({ "items": items }).to_string()).unwrap();
}

#[test]
fn an_unattended_run_may_use_only_a_marked_team_secret_and_only_when_the_vault_is_open() {
    let d = dir("unattended");
    let vault = MemVault::new();
    let tok = SecretRef::parse("vault:team/web/api-token").unwrap();
    vault.put(&tok, "s3cr3t").unwrap();
    // not marked: blocked, whatever the vault holds
    assert!(matches!(secret_gate(&d, vault.as_ref(), "web", "api-token"), SecretUse::Blocked(w) if w.contains("not marked")));
    write_config(&d, json!([{"kind": "flows.unattended", "namespace": "web", "name": "api-token", "enabled": true, "values": {"secret": "vault:team/web/api-token", "terms_ack": true}}]));
    assert_eq!(secret_gate(&d, vault.as_ref(), "web", "api-token"), SecretUse::Allowed);
    // another team's flow cannot use web's marked secret
    assert!(matches!(secret_gate(&d, vault.as_ref(), "data", "api-token"), SecretUse::Blocked(_)));
    // marked but missing, and marked with the vault locked: blocked with the reason
    assert!(matches!(secret_gate(&d, vault.as_ref(), "web", "other"), SecretUse::Blocked(_)));
    assert!(matches!(secret_gate(&d, &LockedVault, "web", "api-token"), SecretUse::Blocked(w) if w.starts_with("needs secret") && w.contains("locked")));
}

#[test]
fn the_unattended_mark_is_a_registered_kind_that_requires_the_acknowledgement() {
    let k = crate::tenantconfig::schema::kind("flows.unattended").unwrap();
    assert!(k.field("secret").unwrap().required && k.field("terms_ack").unwrap().required);
    assert!(k.team_level);
    let item = crate::tenantconfig::Item { kind: "flows.unattended".into(), name: "tok".into(), namespace: Some("web".into()), enabled: true, managed: false, values: [("secret".to_string(), json!("vault:team/web/tok"))].into_iter().collect() };
    assert!(!crate::tenantconfig::schema::validate(&item, &[]).is_empty(), "no acknowledgement, no mark");
}

#[test]
fn a_flow_that_needs_a_locked_secret_goes_blocked_and_the_owner_is_told() {
    // the service side of "blocked: needs secret": the run's blocked text reaches the owner and the team admins once
    let d = dir("locked-secret");
    let sink = Sink::new(&d);
    let mut s = svc(&d, "pod-a", &sink);
    let r = roles();
    s.registry.upsert(make_schedule(&r, "max", "f", "web", "dev", RunAs::Owner, T0).unwrap()).unwrap();
    let b = FakeBackend { accounts: vec!["max".into()], ..Default::default() };
    let mut blocked = run("r9", "f", "blocked");
    blocked.blocked = match secret_gate(&d, &LockedVault, "web", "tok") {
        SecretUse::Blocked(w) => w,
        SecretUse::Allowed => panic!(),
    };
    b.runs.lock().unwrap().push(("max".into(), blocked));
    s.step(T0, &b, &r, &Quota::default());
    s.step(T0 + 5, &b, &r, &Quota::default());
    assert_eq!(sink.count("max", "Run blocked: f"), 1);
    assert_eq!(sink.count("mia", "Run blocked: f"), 1);
}

fn node(id: &str, kind: &str, cfg: serde_json::Value) -> iyakki_flow::model::Node {
    iyakki_flow::model::Node::new(id, kind, cfg)
}

#[test]
fn personal_subscription_logins_are_refused_for_schedules_and_api_keys_are_not() {
    let mut f = iyakki_flow::model::Flow::new("f", "F");
    f.nodes.push(node("a", "agent", json!({"agent": "claude"})));
    f.nodes.push(node("b", "agent", json!({"agent": "copilot"})));
    f.nodes.push(node("c", "agent", json!({"agent": "claude", "credential": "vault:team/web/claude-key"})));
    f.nodes.push(node("d", "agent", json!({"agent": ""})));
    let p = personal_logins(&f);
    assert_eq!(p.len(), 2, "{p:?}");
    assert!(p[0].contains("Claude") && p[1].contains("Copilot") && p.iter().all(|x| x.contains("nobody is signed in")));
}

#[test]
fn secrets_a_flow_names_are_found_and_unmarked_ones_stop_the_save() {
    let d = dir("save-check");
    let vault = MemVault::new();
    let mut f = iyakki_flow::model::Flow::new("f", "F");
    f.nodes.push(node("h", "http", json!({"url": "https://x.example.com", "headers": {"Authorization": "Bearer {{secret.api-token}}"}})));
    assert_eq!(secrets_in(&f), ["api-token"]);
    let problems = unattended_problems(&f, &d, vault.as_ref(), "web");
    assert_eq!(problems.len(), 1);
    assert!(problems[0].contains("not marked"));
}

#[test]
fn a_run_token_is_scoped_to_one_run_expires_and_is_never_written_to_disk() {
    let d = dir("tokens");
    let sink = Sink::new(&d);
    let tokens = RunTokens::default();
    let t = tokens.issue("r1", T0, 60).unwrap();
    assert!(tokens.verify(&t, "r1", T0 + 10));
    assert!(!tokens.verify(&t, "r2", T0 + 10), "another run");
    assert!(!tokens.verify(&t, "r1", T0 + 61), "expired");
    assert!(!tokens.verify("rt-forged", "r1", T0));
    tokens.revoke_run("r1");
    assert!(!tokens.verify(&t, "r1", T0 + 1));
    // run a full pass that writes every state file, then look for the token or a session in them
    let t2 = tokens.issue("r2", T0, 60).unwrap();
    let mut s = svc(&d, "pod-a", &sink);
    let r = roles();
    s.registry.upsert(make_schedule(&r, "max", "f", "web", "dev", RunAs::Owner, T0).unwrap()).unwrap();
    let b = FakeBackend { accounts: vec!["max".into()], ..Default::default() };
    b.runs.lock().unwrap().push(("max".into(), run("r3", "f", "failed")));
    s.step(T0, &b, &r, &Quota::default());
    let mut seen = 0;
    for e in std::fs::read_dir(super::dir(&d)).unwrap().flatten() {
        if e.path().is_file() {
            seen += 1;
            let text = std::fs::read_to_string(e.path()).unwrap().to_ascii_lowercase();
            assert!(!text.contains(&t2.to_ascii_lowercase()) && !text.contains("rt-") && !text.contains("bearer") && !text.contains("session"), "{}", e.path().display());
        }
    }
    assert!(seen >= 3, "lease, schedules and service state files were written: {seen}");
    let hub_state = std::fs::read_to_string(d.join("cloud/notify/hub.json")).unwrap_or_default().to_ascii_lowercase();
    assert!(!hub_state.contains(&t2.to_ascii_lowercase()) && !hub_state.contains("rt-"));
}

// ------------------------------------------------------------------------------------------------------------------ notify

fn ctx<'a>() -> FlowNote<'a> {
    FlowNote { tenant: "acme", team: "web", flow: "nightly", run: "r7", owner: "max" }
}

#[test]
fn a_notify_node_becomes_a_hub_notification_with_its_recipients_severity_and_source() {
    let mut v = BTreeMap::new();
    v.insert("flow".to_string(), "nightly".to_string());
    let n = from_node(&ctx(), &json!({"to": "role:admin", "severity": "warn", "title": "{{flow}} failed", "body": "see the run"}), &v).unwrap();
    assert_eq!((n.title.as_str(), n.severity, n.source.as_str(), n.team.as_deref()), ("nightly failed", Severity::High, "flow:nightly:r7", Some("web")));
    assert_eq!(n.recipients, Recipients::Roles(vec![RoleSel::TeamAdmin]));
    assert_eq!(n.link.as_ref().map(|l| (l.app.as_str(), l.id.as_str())), Some(("flows", "r7")));
    assert!(n.dedupe_key.starts_with("flow:nightly:r7:"));
    let to = |x: serde_json::Value| recipients_from_node(&x, "max").unwrap();
    assert_eq!(to(json!("me")), Recipients::People(vec!["max".into()]));
    assert_eq!(to(json!("owner")), Recipients::People(vec!["max".into()]));
    assert_eq!(to(json!(null)), Recipients::People(vec!["max".into()]));
    assert_eq!(to(json!("team")), Recipients::Team);
    assert_eq!(to(json!("role:tenant_admin")), Recipients::Roles(vec![RoleSel::TenantAdmin]));
    assert_eq!(to(json!("role:approver:prod")), Recipients::Roles(vec![RoleSel::Approver("prod".into())]));
    assert_eq!(to(json!("role:group:ops")), Recipients::Roles(vec![RoleSel::Group("ops".into())]));
    assert_eq!(to(json!(["mia", "vic"])), Recipients::People(vec!["mia".into(), "vic".into()]));
    for bad in [json!("role:root"), json!("a b"), json!(["role:admin"]), json!(7)] {
        assert!(recipients_from_node(&bad, "max").is_err(), "{bad}");
    }
    assert!(from_node(&ctx(), &json!({"title": ""}), &v).is_err());
    assert!(from_node(&ctx(), &json!({"title": "key {{secret.x}}"}), &v).is_err(), "secrets never enter a text");
    assert_eq!((severity_of("info"), severity_of("error"), severity_of("critical"), severity_of("low")), (Severity::Normal, Severity::High, Severity::Critical, Severity::Low));
}

#[test]
fn recipients_reach_the_right_people_through_the_hub_and_never_leave_the_tenant() {
    let d = dir("hub-recipients");
    let sink = Sink::new(&d);
    let send = |to: serde_json::Value, key: &str| {
        let n = from_node(&ctx(), &json!({"to": to, "title": format!("hello {key}")}), &BTreeMap::new()).unwrap();
        sink.emit(&n).unwrap()
    };
    send(json!("role:admin"), "a");
    send(json!("team"), "b");
    send(json!("role:tenant_admin"), "c");
    send(json!(["max", "vic", "nobody"]), "d");
    assert_eq!(sink.count("mia", "hello a"), 1);
    assert_eq!(sink.count("max", "hello a"), 0, "an editor is not an admin");
    assert_eq!((sink.count("max", "hello b"), sink.count("mia", "hello b"), sink.count("vic", "hello b")), (1, 1, 0), "the team, not another team");
    assert_eq!((sink.count("adam", "hello c"), sink.count("olga", "hello c")), (1, 1));
    assert_eq!((sink.count("max", "hello d"), sink.count("vic", "hello d"), sink.count("zed", "hello d")), (1, 1, 0), "named people who are tenant members get it; a name that is nobody is dropped");
    // a notification for another tenant is refused by this tenant's hub
    let mut other = from_node(&ctx(), &json!({"title": "x"}), &BTreeMap::new()).unwrap();
    other.tenant = "someone-else".into();
    assert!(sink.emit(&other).is_err());
}

#[test]
fn the_same_key_is_delivered_once_and_a_notification_is_in_the_inbox_after_a_restart() {
    let d = dir("hub-dedupe");
    let sink = Sink::new(&d);
    let n = from_node(&ctx(), &json!({"to": "me", "title": "disk is full"}), &BTreeMap::new()).unwrap();
    assert!(!sink.emit(&n).unwrap().deduped);
    assert!(sink.emit(&n).unwrap().deduped, "the second one is a repeat");
    assert_eq!(sink.count("max", "disk is full"), 1);
    // the hub is made again over the same volume (a new pod): the note is there for the person's next sign-in
    let again = Sink::new(&d);
    assert_eq!(again.count("max", "disk is full"), 1);
    assert!(again.emit(&n).unwrap().deduped, "and the repeat is still known");
}

#[test]
fn templates_fill_run_data_and_refuse_secrets() {
    let mut v = BTreeMap::new();
    v.insert("flow".to_string(), "nightly".to_string());
    v.insert("status".to_string(), "failed".to_string());
    v.insert("leak".to_string(), "vault:team/web/x".to_string());
    assert_eq!(render("{{flow}} {{ status }}", &v).unwrap(), "nightly failed");
    for bad in ["token {{secret.api}}", "{{ secret.api }}", "see vault:team/web/x", "{{vault.x}}", "{{uvault:me}}", "{{leak}}", "{{unknown}}", "open {{flow"] {
        assert!(render(bad, &v).is_err(), "{bad}");
    }
}

// ------------------------------------------------------------------------------------------------------------------ approvals

#[derive(Default)]
struct Engine {
    decided: Mutex<Vec<(String, String, bool, String)>>,
    failed: Mutex<Vec<(String, String)>>,
}
impl Resolver for Engine {
    fn decide(&self, _u: &str, run: &str, node: &str, approved: bool, by: &str, _n: &str) -> Result<(), String> {
        self.decided.lock().unwrap().push((run.into(), node.into(), approved, by.into()));
        Ok(())
    }
    fn fail(&self, _u: &str, run: &str, node: &str, _r: &str) -> Result<(), String> {
        self.failed.lock().unwrap().push((run.into(), node.into()));
        Ok(())
    }
}

struct Fix {
    d: PathBuf,
    rec: Arc<Rec>,
    vault: Arc<MemVault>,
    sink: Arc<Sink>,
    roles: Roles,
}
fn fix(tag: &str) -> Fix {
    let d = dir(tag);
    let sink = Sink::new(&d);
    Fix { d, rec: Arc::new(Rec::default()), vault: MemVault::new(), sink, roles: roles() }
}
impl Fix {
    fn env(&self) -> Env<'_> {
        Env { dir: &self.d, vault: self.vault.clone(), audit: self.rec.as_ref() }
    }
    fn waits(&self) -> Waits {
        Waits::new(&super::dir(&self.d))
    }
    fn file(&self, w: &Waits, user: &str, run: &str, timeout: Option<u64>, on_timeout: &str, now: u64) -> Result<super::approval::Wait, String> {
        w.file_wait(&self.env(), &self.roles, "acme", "web", user, "deploy", run, "gate", "ship it?", timeout, on_timeout, now)
    }
    fn poll(&self, w: &Waits, e: &Engine, now: u64) -> Vec<String> {
        w.poll(&self.env(), &self.roles, self.sink.as_ref(), e, now)
    }
    fn decide(&self, who: &str, id: &str, yes: bool, now: u64) -> Result<serde_json::Value, crate::tenantconfig::api::ApiErr> {
        approvals::decide(&self.env(), &Caller { user: who, roles: Some(&self.roles) }, id, yes, now)
    }
}

#[test]
fn a_flow_approval_goes_to_the_teams_admin_never_to_the_requester() {
    let x = fix("ap-route");
    let w = x.waits();
    let a = x.file(&w, "max", "r1", None, "reject", T0).unwrap();
    assert_eq!(approvals::approvers_of(&x.env(), &x.roles, &a.approval_id), ["mia"], "max is an editor; the team's admin decides");
    assert!(x.sink.titles().is_empty(), "the hub announces a waiting approval from the approvals file; this module adds nothing");
    assert!(x.decide("max", &a.approval_id, true, T0 + 1).is_err(), "nobody approves their own request");
    assert!(x.decide("vic", &a.approval_id, true, T0 + 1).is_err(), "another team's admin is not eligible");
    assert!(x.rec.all().contains("tenant.approval.request") && x.rec.all().contains("kind=flow.approval"), "{}", x.rec.all());
}

#[test]
fn when_the_only_approver_is_the_requester_it_escalates_to_a_tenant_admin() {
    let x = fix("ap-escalate");
    let w = x.waits();
    let a = x.file(&w, "mia", "r1", None, "reject", T0).unwrap();
    let mut who = approvals::approvers_of(&x.env(), &x.roles, &a.approval_id);
    who.sort();
    assert_eq!(who, ["adam", "olga"]);
    assert!(x.rec.all().contains("escalated=true"));
    assert!(x.decide("adam", &a.approval_id, true, T0 + 1).is_ok());
}

#[test]
fn a_step_nobody_can_approve_is_refused_at_once() {
    let mut x = fix("ap-nobody");
    x.roles = Roles::new("solo").unwrap();
    x.roles.create_namespace("solo", "web").unwrap();
    let w = x.waits();
    let e = w.file_wait(&x.env(), &x.roles, "acme", "web", "solo", "deploy", "r1", "gate", "m", None, "approve", T0);
    assert!(e.unwrap_err().contains("nobody is eligible"), "default deny, even with on_timeout approve");
}

#[test]
fn a_decision_from_another_session_resolves_the_waiting_node_once() {
    let x = fix("ap-decide");
    let (w, e) = (x.waits(), Engine::default());
    let a = x.file(&w, "max", "r1", None, "reject", T0).unwrap();
    assert!(x.poll(&w, &e, T0 + 5).is_empty(), "still pending: the run is blocked and holds nothing");
    assert!(e.decided.lock().unwrap().is_empty());
    x.decide("mia", &a.approval_id, true, T0 + 100).unwrap(); // the approver's own session, hours later in real life
    let lines = x.poll(&w, &e, T0 + 110);
    assert_eq!(lines.len(), 1);
    assert_eq!(e.decided.lock().unwrap().as_slice(), [("r1".into(), "gate".into(), true, "mia".into())]);
    assert!(x.poll(&w, &e, T0 + 120).is_empty());
    assert_eq!(e.decided.lock().unwrap().len(), 1, "exactly once");
    assert!(w.waiting().is_empty());
}

#[test]
fn a_denial_rejects_the_node() {
    let x = fix("ap-deny");
    let (w, e) = (x.waits(), Engine::default());
    let a = x.file(&w, "max", "r1", None, "approve", T0).unwrap();
    x.decide("mia", &a.approval_id, false, T0 + 1).unwrap();
    x.poll(&w, &e, T0 + 2);
    assert_eq!(e.decided.lock().unwrap()[0].2, false);
}

#[test]
fn two_paths_deciding_at_the_same_time_resolve_the_node_exactly_once() {
    let x = fix("ap-race");
    let w = Arc::new(x.waits());
    let e = Arc::new(Engine::default());
    x.file(&w, "max", "r1", None, "reject", T0).unwrap();
    let applied = Arc::new(AtomicUsize::new(0));
    let hs: Vec<_> = (0..8)
        .map(|i| {
            let (w, e, applied) = (w.clone(), e.clone(), applied.clone());
            std::thread::spawn(move || {
                // the desktop inbox path and the API path both come here
                if w.resolve_once("max", "r1", "gate", i % 2 == 0, &format!("p{i}"), "", e.as_ref(), T0 + 1) == Resolution::Applied {
                    applied.fetch_add(1, Ordering::SeqCst);
                }
            })
        })
        .collect();
    for h in hs {
        h.join().unwrap();
    }
    assert_eq!(applied.load(Ordering::SeqCst), 1);
    assert_eq!(e.decided.lock().unwrap().len(), 1);
}

#[test]
fn a_timeout_applies_on_timeout_approve_reject_or_fail() {
    for (word, approved, failed) in [("approve", true, 0), ("reject", false, 0), ("fail", false, 1), ("", false, 0), ("nonsense", false, 0)] {
        let x = fix(&format!("ap-timeout-{word}"));
        let (w, e) = (x.waits(), Engine::default());
        let a = x.file(&w, "max", "r1", Some(600), word, T0).unwrap();
        assert!(x.poll(&w, &e, T0 + 599).is_empty());
        let lines = x.poll(&w, &e, T0 + 600);
        assert_eq!(lines.len(), 1, "{word}");
        if failed == 1 {
            assert_eq!(e.failed.lock().unwrap().len(), 1);
            assert!(e.decided.lock().unwrap().is_empty());
        } else {
            assert_eq!(e.decided.lock().unwrap()[0].2, approved, "{word}");
        }
        // the request left the approvers' list, and a late click is refused
        assert!(approvals::list(&x.env(), &Caller { user: "mia", roles: Some(&x.roles) }, T0 + 601).unwrap()["approvals"].as_array().unwrap().is_empty());
        assert!(x.decide("mia", &a.approval_id, true, T0 + 602).is_err());
        assert!(x.sink.titles().contains(&"mia:Approval expired: deploy".to_string()), "{:?}", x.sink.titles());
        assert_eq!(x.sink.count("max", "Approval expired"), 1, "the requester is told too");
    }
    assert_eq!(OnTimeout::parse(""), OnTimeout::Reject);
}

#[test]
fn the_routers_expiry_applies_on_timeout_when_the_node_has_no_timeout_of_its_own() {
    let x = fix("ap-expiry");
    let (w, e) = (x.waits(), Engine::default());
    x.file(&w, "max", "r1", None, "approve", T0).unwrap();
    assert!(x.poll(&w, &e, T0 + approvals::ttl_secs() - 1).iter().all(|l| !l.contains("timeout")));
    let lines = x.poll(&w, &e, T0 + approvals::ttl_secs());
    assert!(lines.iter().any(|l| l.contains("timeout: approve")), "{lines:?}");
    assert_eq!(e.decided.lock().unwrap()[0].3, "timeout");
}

#[test]
fn a_pod_restart_keeps_pending_approvals_and_finishes_a_half_applied_decision() {
    let x = fix("ap-restart");
    let w = x.waits();
    let a = x.file(&w, "max", "r1", None, "reject", T0).unwrap();
    drop(w);
    // a new process over the same volume sees the pending request and still routes it
    let (w2, e) = (x.waits(), Engine::default());
    assert_eq!(w2.waiting().len(), 1);
    assert_eq!(approvals::list(&x.env(), &Caller { user: "mia", roles: Some(&x.roles) }, T0 + 5).unwrap()["approvals"].as_array().unwrap().len(), 1);
    x.decide("mia", &a.approval_id, true, T0 + 6).unwrap();
    // the process dies after taking the claim and before telling the engine: simulate with a resolver that fails once
    struct Flaky(Mutex<u32>, Engine);
    impl Resolver for Flaky {
        fn decide(&self, u: &str, r: &str, n: &str, a: bool, b: &str, note: &str) -> Result<(), String> {
            let mut c = self.0.lock().unwrap();
            *c += 1;
            if *c == 1 { Err("engine busy".into()) } else { self.1.decide(u, r, n, a, b, note) }
        }
        fn fail(&self, u: &str, r: &str, n: &str, m: &str) -> Result<(), String> {
            self.1.fail(u, r, n, m)
        }
    }
    let f = Flaky(Mutex::new(0), Engine::default());
    w2.poll(&x.env(), &x.roles, x.sink.as_ref(), &f, T0 + 7);
    assert!(f.1.decided.lock().unwrap().is_empty());
    let w3 = x.waits();
    w3.poll(&x.env(), &x.roles, x.sink.as_ref(), &f, T0 + 12);
    assert_eq!(f.1.decided.lock().unwrap().len(), 1, "finished at the next pass, from the claim on the volume");
    assert_eq!(f.1.decided.lock().unwrap()[0].2, true);
    let _ = e;
}

// ------------------------------------------------------------------------------------------------------------------ quota, tz

#[test]
fn quota_defaults_clamps_and_refusals() {
    let q = Quota::from_values(&BTreeMap::new());
    assert_eq!(q, Quota::default());
    let mut v = BTreeMap::new();
    v.insert("max_runs_tenant".to_string(), json!(0));
    v.insert("max_run_secs".to_string(), json!(99_999_999u64));
    v.insert("max_runs_person".to_string(), json!("many"));
    let q = Quota::from_values(&v);
    assert_eq!((q.max_runs_tenant, q.max_run_secs, q.max_runs_person), (1, 7 * 86_400, 2));
    let q = Quota { max_runs_tenant: 3, max_runs_person: 2, ..Quota::default() };
    assert!(admit_run(&q, 2, 1).is_ok());
    assert_eq!(admit_run(&q, 3, 0).unwrap_err().code, "tenant_runs");
    assert_eq!(admit_run(&q, 1, 2).unwrap_err().code, "person_runs");
    assert_eq!(admit_schedule(&q, 50).unwrap_err().code, "schedules");
    assert!(!over_time(&q, 0, 10_000_000, None), "not started");
    assert!(over_time(&q, 1000, 1000 + 3_600_001, None));
    assert!(over_time(&q, 1000, 1000 + 61_000, Some(60)), "the flow's own smaller limit");
}

#[test]
fn quota_is_read_from_the_tenant_configuration_and_a_team_can_only_narrow_it() {
    let d = dir("quota-cfg");
    assert_eq!(Quota::load(&d, None), Quota::default());
    write_config(&d, json!([
        {"kind": "flows.quota", "name": "default", "enabled": true, "values": {"max_runs_tenant": 6, "max_run_secs": 900}},
        {"kind": "flows.quota", "name": "default", "namespace": "web", "enabled": true, "values": {"max_runs_tenant": 9, "max_run_secs": 300}}
    ]));
    let q = Quota::load(&d, None);
    assert_eq!((q.max_runs_tenant, q.max_run_secs), (6, 900));
    let w = Quota::load(&d, Some("web"));
    assert_eq!((w.max_runs_tenant, w.max_run_secs), (6, 300), "the team may lower a limit, not raise it");
}

#[test]
fn the_tenants_zone_decides_when_a_local_cron_fires() {
    use iyakki_flow::sched::Cron;
    let cron = Cron::parse("0 9 * * *").unwrap();
    let day = 1_800_000_000i64 - 1_800_000_000i64.rem_euclid(86_400); // midnight UTC
    for (setting, offset_h) in [("UTC", 0.0), ("", 0.0), ("+05:30", 5.5), ("-0800", -8.0)] {
        let r = tz::tenant_resolver(setting, None);
        let z = r("local").unwrap();
        let fire = cron.next_after(day, z.as_ref()).unwrap();
        assert_eq!(fire, day + (9.0 * 3600.0 - offset_h * 3600.0) as i64, "{setting}");
    }
    // a trigger that names its own zone keeps it
    let r = tz::tenant_resolver("+05:30", None);
    assert_eq!(r("-0800").unwrap().offset_secs(0), -8 * 3600);
}

#[test]
fn an_unknown_zone_name_is_utc_and_says_so_where_there_are_no_zone_files() {
    let none = None;
    assert_eq!(tz::mode_of("Europe/Berlin", &none), tz::Mode::FellBackToUtc("Europe/Berlin".into()));
    assert!(tz::setting_ok("Europe/Berlin", &none).is_err());
    assert!(tz::setting_ok("+01:00", &none).is_ok() && tz::setting_ok("UTC", &none).is_ok());
    let r = tz::tenant_resolver("Europe/Berlin", None);
    assert_eq!(r("local").unwrap().offset_secs(T0 as i64), 0, "UTC, not the pod's clock");
    assert_eq!(tz::mode_of("../../etc/passwd", &Some(PathBuf::from("/usr/share/zoneinfo"))), tz::Mode::FellBackToUtc("../../etc/passwd".into()));
}

#[test]
fn daylight_saving_gaps_and_overlaps_follow_the_zones_offsets() {
    use iyakki_flow::sched::{Cron, Tz};
    // a zone that is +1 h before the instant S and +2 h from it on (clocks go forward at 02:00 local, i.e. 01:00 UTC)
    struct Spring(i64);
    impl Tz for Spring {
        fn offset_secs(&self, u: i64) -> i32 {
            if u < self.0 { 3600 } else { 7200 }
        }
    }
    let s = 1_805_000_000i64 - 1_805_000_000i64.rem_euclid(86_400) + 3600; // 01:00 UTC on some day
    let z = Spring(s);
    let day0 = s - 3600;
    // 02:30 local does not exist that day; it fires at the first valid time after the gap (03:30 local = 01:30 UTC)
    let cron = Cron::parse("30 2 * * *").unwrap();
    let fire = cron.next_after(day0 - 1, &z).unwrap();
    assert!(fire >= s && fire <= s + 3600 + 1800, "{}", fire - day0);
    // 09:00 local the same day is 07:00 UTC after the change
    let nine = Cron::parse("0 9 * * *").unwrap().next_after(day0, &z).unwrap();
    assert_eq!(nine, day0 + 7 * 3600);
}

// ------------------------------------------------------------------------------------------------------------------ job, executor

fn spec() -> JobSpec {
    JobSpec { name: "run-nightly-r12".into(), namespace: "web-prod".into(), tenant: "acme".into(), team: "web".into(), environment: "prod".into(), flow: "nightly".into(), run: "r12".into(), image: "registry.example.com/web/tool:1.2".into(), argv: vec!["/app/tool".into(), "--once".into()], env: vec![("IYAKKI_FLOW".into(), "nightly".into())], secret_env: vec![], cpu_m: 500, mem_mib: 256, deadline_secs: 900, ttl_after_finish_secs: 3600 }
}

fn sandbox_values() -> serde_json::Value {
    json!({"tiers": ["job"], "allowed_registries": ["registry.example.com"], "require_digest": false, "max_cpu_m": 1000, "max_mem_mib": 512, "max_pids": 128, "max_seconds": 3600,
        "max_workspace_mib": 256, "network_modes": ["none", "proxy"], "runtime_class": "gvisor", "cli_role": "editor", "approval_patterns": [], "per_person_concurrency": 2,
        "per_team_concurrency": 4, "queue_timeout_secs": 5, "audit_retention_days": 90, "k3s_environment": "dev", "cli_image": "registry.example.com/tools/cli:1.0"})
}

fn sandbox_policy() -> SandboxPolicy {
    let map: BTreeMap<String, serde_json::Value> = sandbox_values().as_object().unwrap().iter().map(|(k, v)| (k.clone(), v.clone())).collect();
    SandboxPolicy::from_values(true, &map)
}

#[test]
fn the_job_documents_are_exactly_this() {
    let got = bundle_text(&spec(), "mia", "editor", &sandbox_policy(), "web-prod").unwrap();
    let want = include_str!("job_golden.json");
    assert_eq!(got, want, "--- got ---\n{got}");
}

#[test]
fn the_job_comes_from_the_one_sandbox_builder_hardened_with_its_network_policy_and_no_secret_value() {
    let m = bundle_text(&spec(), "mia", "editor", &sandbox_policy(), "web-prod").unwrap();
    let docs: serde_json::Value = serde_json::from_str(&m).unwrap();
    let kinds: Vec<&str> = docs["items"].as_array().unwrap().iter().map(|d| d["kind"].as_str().unwrap()).collect();
    assert!(kinds.contains(&"Job") && kinds.contains(&"NetworkPolicy"), "{kinds:?}");
    for must in ["runAsNonRoot", "readOnlyRootFilesystem", "allowPrivilegeEscalation", "RuntimeDefault", "automountServiceAccountToken", "activeDeadlineSeconds", "ttlSecondsAfterFinished", "gvisor"] {
        assert!(m.contains(must), "{must}");
    }
    assert!(!m.contains("privileged\": true") && !m.contains("hostNetwork\": true") && !m.contains("hostPath"));
    // a Kubernetes Secret reference cannot be carried; the policy and the registry are enforced by the sandbox's admission
    let mut with_secret = spec();
    with_secret.secret_env = vec![("API_TOKEN".into(), "web-prod-api".into(), "token".into())];
    assert!(bundle_text(&with_secret, "mia", "editor", &sandbox_policy(), "web-prod").unwrap_err().contains("vault reference"));
    let mut other = spec();
    other.image = "evil.example.net/x:1".into();
    assert!(bundle_text(&other, "mia", "editor", &sandbox_policy(), "web-prod").unwrap_err().contains("registry"));
    assert!(bundle_text(&spec(), "max", "viewer", &sandbox_policy(), "web-prod").is_err(), "the policy's role");
    assert!(bundle_text(&spec(), "mia", "editor", &SandboxPolicy::off(), "web-prod").unwrap_err().contains("not switched on"));
    assert_eq!(registry_of("registry.example.com/web/tool:1"), "registry.example.com");
    assert_eq!(registry_of("busybox"), "docker.io");
}

#[test]
fn the_job_documents_are_read_by_kubectl_when_it_is_here() {
    // `kubectl create --dry-run=client --validate=false` parses the file; skipped where kubectl is missing
    let Some(kubectl) = ["/usr/local/bin/kubectl", "/usr/bin/kubectl"].into_iter().find(|p| std::path::Path::new(p).exists()) else { return };
    let d = dir("kubectl");
    std::fs::write(d.join("job.json"), bundle_text(&spec(), "mia", "editor", &sandbox_policy(), "web-prod").unwrap()).unwrap();
    let out = std::process::Command::new(kubectl).args(["create", "--dry-run=client", "--validate=false", "-o", "name", "-f"]).arg(d.join("job.json")).env("KUBECONFIG", d.join("none")).output().unwrap();
    let text = format!("{}{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr));
    // without a cluster kubectl may fail to discover the API; what must not appear is a complaint about the file itself
    assert!(!text.contains("error parsing") && !text.contains("yaml:") && !text.contains("unknown field") && !text.contains("cannot unmarshal") && !text.contains("invalid character"), "{text}");
}

fn req(env: &str, owner: &str, tenant_running: usize, person_running: usize) -> RunRequest {
    RunRequest { tenant: "acme".into(), team: "web".into(), environment: env.into(), owner: owner.into(), flow: "nightly".into(), run: "r12".into(), image: "registry.example.com/web/tool:1.2".into(), argv: vec!["/app/tool".into()], tenant_running, person_running, promoted_from: match env { "preprod" => Some("dev".into()), "prod" => Some("preprod".into()), _ => None } }
}

#[test]
fn the_in_process_executor_enforces_the_quota() {
    let e = InProcess { quota: Quota { max_runs_tenant: 2, max_runs_person: 1, max_run_secs: 120, ..Quota::default() } };
    assert_eq!(e.plan(&req("dev", "max", 0, 0), T0), Decision::InProcess { deadline_secs: 120 });
    assert!(matches!(e.plan(&req("dev", "max", 2, 0), T0), Decision::Refused { code, .. } if code == "tenant_runs"));
    assert!(matches!(e.plan(&req("dev", "max", 0, 1), T0), Decision::Refused { code, .. } if code == "person_runs"));
}

fn bindings_without_sandbox(x: &Fix) {
    bindings_with(x, false);
}

fn bindings(x: &Fix) {
    bindings_with(x, true);
}

fn bindings_with(x: &Fix, sandbox: bool) {
    let mut items = vec![];
    for e in ["dev", "preprod", "prod"] {
        items.push(json!({"kind": "k3s.cluster", "namespace": "web", "name": e, "enabled": true, "values": {"api_server": format!("https://web-{e}.k3s.example.com:6443"), "credential": format!("vault:k3s.cluster/web-{e}-credential"), "allowed_registries": ["registry.example.com"], "deploy_role": if e == "dev" { "editor" } else { "admin" }}}));
        x.vault.put(&SecretRef::parse(&format!("vault:k3s.cluster/web-{e}-credential")).unwrap(), "kubeconfig").unwrap();
    }
    if sandbox {
        items.push(json!({"kind": "sandbox.policy", "name": "default", "enabled": true, "values": sandbox_values()}));
    }
    write_config(&x.d, json!(items));
}

#[test]
fn a_scheduled_run_in_dev_becomes_a_job_through_the_teams_binding() {
    let x = fix("k3s-dev");
    bindings(&x);
    let e = K3sJob { env: &x.env(), roles: &x.roles, quota: Quota::default(), cpu_m: 500, mem_mib: 256 };
    let Decision::Job { namespace, manifest } = e.plan(&req("dev", "max", 0, 0), T0) else { panic!("{:?}", e.plan(&req("dev", "max", 0, 0), T0)) };
    assert_eq!(namespace, "web-dev");
    assert!(manifest.contains("\"kind\": \"Job\"") && manifest.contains("\"namespace\": \"web-dev\"") && manifest.contains("NetworkPolicy"));
    assert!(x.rec.all().contains("tenant.k3s.deploy") && x.rec.all().contains("decision=allow"));
    // quota refusal comes first
    assert!(matches!(e.plan(&req("dev", "max", 4, 0), T0), Decision::Refused { code, .. } if code == "tenant_runs"));
    // the sandbox policy is the second gate: switched off, the same request is refused with that reason
    std::fs::remove_file(x.d.join("cloud/config/tenant-config.json")).unwrap();
    bindings_without_sandbox(&x);
    assert!(matches!(e.plan(&req("dev", "max", 0, 0), T0), Decision::Refused { code, reason } if code == "sandbox" && reason.contains("not switched on")));
    bindings(&x);
    // a person outside the team is refused by the binding rules
    assert!(matches!(e.plan(&req("dev", "zed", 0, 0), T0), Decision::Refused { code, .. } if code == "denied"));
}

#[test]
fn a_scheduled_run_in_prod_waits_as_a_pending_approval_and_runs_once_approved() {
    let x = fix("k3s-prod");
    bindings(&x);
    let e = K3sJob { env: &x.env(), roles: &x.roles, quota: Quota::default(), cpu_m: 500, mem_mib: 256 };
    // mia owns the schedule and is the only admin of web: the request escalates to the tenant admins
    let d = e.plan(&req("prod", "mia", 0, 0), T0);
    let Decision::Wait { approval_id, approver_role } = d else { panic!("{d:?}") };
    assert_eq!(approver_role, "tenant_admin");
    // the next look finds it still pending: the same request, no second one
    assert!(matches!(e.plan(&req("prod", "mia", 0, 0), T0 + 30), Decision::Wait { approval_id: id2, .. } if id2 == approval_id));
    assert!(x.decide("mia", &approval_id, true, T0 + 31).is_err());
    x.decide("adam", &approval_id, true, T0 + 60).unwrap();
    let Decision::Job { namespace, .. } = e.plan(&req("prod", "mia", 0, 0), T0 + 90) else { panic!("approved: it runs") };
    assert_eq!(namespace, "web-prod");
    // the approval was used once: the next scheduled run asks again
    assert!(matches!(e.plan(&req("prod", "mia", 0, 0), T0 + 120), Decision::Wait { .. }));
    // promotion order still holds
    let mut skip = req("prod", "mia", 0, 0);
    skip.promoted_from = Some("dev".into());
    assert!(matches!(e.plan(&skip, T0 + 130), Decision::Refused { reason, .. } if reason.contains("promotion order")));
}

// ------------------------------------------------------------------------------------------------------------------ isolation

#[test]
fn one_tenants_state_is_invisible_to_another() {
    let (a, b) = (dir("tenant-a"), dir("tenant-b"));
    let (sink_a, sink_b) = (Sink::new(&a), Sink::new(&b));
    let (sa, sb) = (svc(&a, "pod-a", &sink_a), svc(&b, "pod-b", &sink_b));
    let r = roles();
    sa.registry.upsert(make_schedule(&r, "max", "f", "web", "dev", RunAs::Owner, T0).unwrap()).unwrap();
    assert_eq!(sa.registry.list().len(), 1);
    assert!(sb.registry.list().is_empty());
    assert_eq!(Lease::new(&super::dir(&b), "x", 30).try_acquire(T0), Acquire::Held(1), "its own lease");
    // approvals and waits live under each tenant's own volume
    let (xa, xb) = (Fix { d: a.clone(), ..fix("tenant-a2") }, Fix { d: b.clone(), ..fix("tenant-b2") });
    let w = xa.waits();
    xa.file(&w, "max", "r1", None, "reject", T0).unwrap();
    assert!(xb.waits().waiting().is_empty());
    assert!(approvals::list(&xb.env(), &Caller { user: "mia", roles: Some(&xb.roles) }, T0 + 1).unwrap()["approvals"].as_array().unwrap().is_empty());
    // and each tenant's notification hub keeps its own inbox
    sink_a.emit(&from_node(&ctx(), &json!({"to": "me", "title": "only for A"}), &BTreeMap::new()).unwrap()).unwrap();
    assert_eq!(sink_a.count("max", "only for A"), 1);
    assert_eq!(sink_b.count("max", "only for A"), 0);
    // a schedule of tenant A is not found by tenant B's check of the same owner name
    let mut r2 = Roles::new("olga").unwrap();
    r2.create_namespace("olga", "web").unwrap();
    assert!(matches!(check(&r2, &sa.registry.list()[0]), OwnerCheck::Pause(_)), "max is nobody in tenant B");
}
