//! The tenant scheduler service: one per tenant volume, run by the replica that holds the lease, with nobody signed in.
//!
//! [`TenantScheduler::step`] is called every few seconds by a thread (see `real.rs`) and does, in order: take or renew the lease (a
//! replica that lost it stops every engine at once); start the engine of every account that has flows (the engine applies each trigger's
//! missed-run policy at its first tick, from the scheduler state kept on the volume); pause schedules whose owner lost access and tell
//! the team's admins; stop runs that exceeded their time limit; report failed scheduled runs, long-missed runs and quota refusals with
//! deduplicated notifications; refresh the counters behind the status route.
//!
//! Pure over the [`Backend`] trait and a number for the time, so the tests use fakes and a fake clock.

use super::identity::{check, OwnerCheck, Registry};
use super::lease::{Acquire, Lease};
use super::notify::{say, NotifySink, RoleSel, Severity};
use super::quota::{over_time, Quota};
use crate::cloudroles::Roles;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RunInfo {
    pub id: String,
    pub flow: String,
    /// `running`, `queued`, `blocked`, `failed`, `succeeded`, `cancelled`, ...
    pub status: String,
    pub created_ms: i64,
    pub started_ms: i64,
    /// A scheduled run: the tick it was for, and whether it was a missed one.
    pub due_ms: Option<i64>,
    pub scheduled: bool,
    pub missed: bool,
    pub error: String,
    pub blocked: String,
}

/// The accounts' engines, as the service needs them.
pub trait Backend: Send + Sync {
    fn accounts(&self) -> Vec<String>;
    /// Make sure the account's engine is going (idempotent).
    fn start(&self, user: &str) -> Result<(), String>;
    /// Stop every engine; runs stay on disk and are picked up by the next leader.
    fn stop_all(&self);
    fn runs(&self, user: &str) -> Vec<RunInfo>;
    fn cancel(&self, user: &str, run: &str) -> Result<(), String>;
    fn pause(&self, user: &str, flow: &str, paused: bool);
}

/// Counters behind the status route; the quota gate (inside the engines) adds to `quota_refused`.
#[derive(Default)]
pub struct Counters {
    pub quota_refused: AtomicU64,
    pub stuck_cancelled: AtomicU64,
    pub owner_paused: AtomicU64,
    pub failures_notified: AtomicU64,
    pub missed_notified: AtomicU64,
    pub leader_changes: AtomicU64,
    pub steps: AtomicU64,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
struct State {
    reported: BTreeSet<String>,
    /// flow -> (day, notifications sent today)
    fail_day: BTreeMap<String, (u64, u32)>,
    refused_seen: u64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Role {
    Leader,
    Standby { holder: String },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StepReport {
    pub role: Role,
    pub started: Vec<String>,
    pub paused: Vec<String>,
    pub cancelled: Vec<String>,
    pub notified: usize,
}

#[derive(Clone, Debug, Default)]
struct Gauges {
    running: usize,
    queued: usize,
    blocked: usize,
    failed: usize,
    schedules: usize,
    paused_by_service: usize,
    last_step: u64,
    lag_secs: u64,
}

pub struct TenantScheduler {
    dir: std::path::PathBuf,
    tenant: String,
    pub lease: Lease,
    pub sink: Arc<dyn NotifySink>,
    pub registry: Registry,
    pub counters: Arc<Counters>,
    started: BTreeSet<String>,
    leading: bool,
    epoch: u64,
    gauges: Gauges,
    interval: u64,
}

/// Fail notifications per flow per day before only the digest counts.
pub const FAIL_NOTES_PER_DAY: u32 = 5;

impl TenantScheduler {
    pub fn new(data: &std::path::Path, tenant: &str, holder: &str, lease_secs: u64, interval_secs: u64, sink: Arc<dyn NotifySink>) -> TenantScheduler {
        let dir = super::dir(data);
        let _ = std::fs::create_dir_all(&dir);
        TenantScheduler { lease: Lease::new(&dir, holder, lease_secs), registry: Registry::new(&dir), dir, tenant: tenant.into(), sink, counters: Arc::new(Counters::default()), started: BTreeSet::new(), leading: false, epoch: 0, gauges: Gauges::default(), interval: interval_secs.max(1) }
    }

    pub fn is_leader(&self) -> bool {
        self.leading
    }

    fn state_file(&self) -> std::path::PathBuf {
        self.dir.join("service.json")
    }

    /// Tell people and roles of a team; true when the hub took at least one notification.
    #[allow(clippy::too_many_arguments)]
    fn tell(&self, team: &str, people: Vec<String>, roles: Vec<RoleSel>, sev: Severity, title: String, body: String, link: Option<(&str, &str)>, key: String, source: String) -> bool {
        say(self.sink.as_ref(), &self.tenant, team, people, roles, sev, &title, &body, link, &key, &source) > 0
    }

    pub fn step(&mut self, now: u64, b: &dyn Backend, roles: &Roles, q: &Quota) -> StepReport {
        let mut rep = StepReport { role: Role::Leader, started: vec![], paused: vec![], cancelled: vec![], notified: 0 };
        self.counters.steps.fetch_add(1, Ordering::Relaxed);
        let lag = if self.gauges.last_step > 0 { now.saturating_sub(self.gauges.last_step).saturating_sub(self.interval) } else { 0 };
        // 1. the lease
        let held = if self.leading {
            match self.lease.heartbeat(now) {
                Ok(e) => Some(e),
                Err(_) => None,
            }
        } else {
            None
        };
        let epoch = match held {
            Some(e) => Some(e),
            None => match self.lease.try_acquire(now) {
                Acquire::Held(e) => Some(e),
                Acquire::Busy { holder, .. } => {
                    if self.leading {
                        b.stop_all();
                        self.started.clear();
                        self.leading = false;
                        self.counters.leader_changes.fetch_add(1, Ordering::Relaxed);
                    }
                    rep.role = Role::Standby { holder };
                    self.gauges.last_step = now;
                    return rep;
                }
            },
        };
        if !self.leading {
            self.leading = true;
            self.counters.leader_changes.fetch_add(1, Ordering::Relaxed);
        }
        self.epoch = epoch.unwrap_or(self.epoch);
        self.gauges.lag_secs = lag;
        self.gauges.last_step = now;
        let mut st: State = super::load_json(&self.state_file());
        let day = now / 86_400;

        // 2. engines for every account with flows
        let accounts = b.accounts();
        for u in &accounts {
            if !self.started.contains(u) && b.start(u).is_ok() {
                self.started.insert(u.clone());
                rep.started.push(u.clone());
            }
        }

        // 3. owners that lost access
        let schedules = self.registry.list();
        self.gauges.schedules = schedules.len();
        self.gauges.paused_by_service = schedules.iter().filter(|s| s.paused_reason.is_some()).count();
        for s in &schedules {
            if let OwnerCheck::Pause(why) = check(roles, s) {
                if s.paused_reason.is_none() {
                    b.pause(&s.owner, &s.flow, true);
                    let _ = self.registry.set_paused(&s.owner, &s.flow, Some(why.clone()));
                    self.counters.owner_paused.fetch_add(1, Ordering::Relaxed);
                    rep.paused.push(s.key());
                    let sent = self.tell(&s.team, vec![], vec![RoleSel::TeamAdmin, RoleSel::TenantAdmin], Severity::High, format!("Schedule paused: {}", s.flow), format!("{why}. The schedule is paused until an admin looks at it."), Some(("flows", &s.flow)), format!("owner-gone:{}", s.key()), "scheduler".into());
                    rep.notified += sent as usize;
                }
            }
        }

        // 4. runs: time limits, failures, missed ticks
        let (mut running, mut queued, mut blocked, mut failed) = (0, 0, 0, 0);
        for u in &accounts {
            for r in b.runs(u) {
                match r.status.as_str() {
                    "running" | "retrying" | "healing" => {
                        running += 1;
                        if over_time(q, r.started_ms, now as i64 * 1000, None) && b.cancel(u, &r.id).is_ok() {
                            self.counters.stuck_cancelled.fetch_add(1, Ordering::Relaxed);
                            rep.cancelled.push(r.id.clone());
                            let team = schedules.iter().find(|s| s.owner == *u && s.flow == r.flow).map(|s| s.team.clone()).unwrap_or_default();
                            let sent = self.tell(&team, vec![u.clone()], vec![RoleSel::TeamAdmin], Severity::High, format!("Run stopped: {}", r.flow), format!("It ran longer than {} s and was stopped.", q.max_run_secs), Some(("flows", &r.id)), format!("stuck:{}", r.id), format!("flow:{}:{}", r.flow, r.id));
                            rep.notified += sent as usize;
                        }
                    }
                    "queued" | "scheduled" => queued += 1,
                    "blocked" => {
                        blocked += 1;
                        if r.blocked.contains("needs secret") && !st.reported.contains(&format!("secret:{}", r.id)) {
                            st.reported.insert(format!("secret:{}", r.id));
                            let team = schedules.iter().find(|s| s.owner == *u && s.flow == r.flow).map(|s| s.team.clone()).unwrap_or_default();
                            let sent = self.tell(&team, vec![u.clone()], vec![RoleSel::TeamAdmin], Severity::High, format!("Run blocked: {}", r.flow), r.blocked.clone(), Some(("flows", &r.id)), format!("blocked:{}", r.id), format!("flow:{}:{}", r.flow, r.id));
                            rep.notified += sent as usize;
                        }
                    }
                    "failed" => {
                        failed += 1;
                        if r.scheduled && st.reported.insert(r.id.clone()) {
                            let team = schedules.iter().find(|s| s.owner == *u && s.flow == r.flow).map(|s| s.team.clone()).unwrap_or_default();
                            let e = st.fail_day.entry(r.flow.clone()).or_insert((day, 0));
                            if e.0 != day {
                                *e = (day, 0);
                            }
                            e.1 += 1;
                            if e.1 <= FAIL_NOTES_PER_DAY {
                                let sent = self.tell(&team, vec![u.clone()], vec![RoleSel::TeamAdmin], Severity::High, format!("Scheduled run failed: {}", r.flow), r.error.chars().take(300).collect(), Some(("flows", &r.id)), format!("run-failed:{}", r.id), format!("flow:{}:{}", r.flow, r.id));
                                self.counters.failures_notified.fetch_add(1, Ordering::Relaxed);
                                rep.notified += sent as usize;
                            }
                        }
                    }
                    _ => {}
                }
                if r.scheduled && r.missed && r.due_ms.is_some_and(|d| r.created_ms - d > q.missed_notify_secs as i64 * 1000) && st.reported.insert(format!("missed:{}", r.id)) {
                    let team = schedules.iter().find(|s| s.owner == *u && s.flow == r.flow).map(|s| s.team.clone()).unwrap_or_default();
                    self.counters.missed_notified.fetch_add(1, Ordering::Relaxed);
                    let sent = self.tell(&team, vec![u.clone()], vec![RoleSel::TeamAdmin], Severity::High, format!("Missed run: {}", r.flow), "The scheduler was not running when this run was due; the schedule's missed-run policy was applied.".into(), Some(("flows", &r.id)), format!("missed:{}", r.id), format!("flow:{}:{}", r.flow, r.id));
                    rep.notified += sent as usize;
                }
            }
        }
        self.gauges.running = running;
        self.gauges.queued = queued;
        self.gauges.blocked = blocked;
        self.gauges.failed = failed;

        // 5. quota refusals (the gate counts; one notification a day)
        let refused = self.counters.quota_refused.load(Ordering::Relaxed);
        if refused > st.refused_seen {
            st.refused_seen = refused;
            let sent = self.tell("", vec![], vec![RoleSel::TenantAdmin], Severity::High, "Runs were held back by the quota".into(), format!("{refused} runs have waited because the tenant or a person was at its limit (flows.quota)."), None, format!("quota:{day}"), "scheduler".into());
            rep.notified += sent as usize;
        }
        if st.reported.len() > 5000 {
            st.reported.clear();
        }
        let _ = super::save_json(&self.state_file(), &st);
        rep
    }

    /// Clean stop (SIGTERM): stop the engines (runs stay on disk) and hand the lease over.
    pub fn shutdown(&mut self, b: &dyn Backend) {
        b.stop_all();
        self.started.clear();
        if self.leading {
            self.lease.release();
            self.leading = false;
        }
    }

    /// The status route: counters and gauges, no names of people or flows.
    pub fn status(&self, now: u64) -> serde_json::Value {
        let c = &self.counters;
        let g = &self.gauges;
        serde_json::json!({
            "leader": self.leading, "holder": self.lease.holder_name(), "epoch": self.epoch,
            "schedules": g.schedules, "paused_by_service": g.paused_by_service,
            "runs": {"running": g.running, "queued": g.queued, "blocked": g.blocked, "failed": g.failed},
            "quota_refused": c.quota_refused.load(Ordering::Relaxed), "stuck_cancelled": c.stuck_cancelled.load(Ordering::Relaxed),
            "owner_paused": c.owner_paused.load(Ordering::Relaxed), "failures_notified": c.failures_notified.load(Ordering::Relaxed),
            "missed_notified": c.missed_notified.load(Ordering::Relaxed), "leader_changes": c.leader_changes.load(Ordering::Relaxed),
            "steps": c.steps.load(Ordering::Relaxed), "lag_secs": g.lag_secs, "since_last_step_secs": now.saturating_sub(g.last_step),
        })
    }
}
