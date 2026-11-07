//! Limits for flows and schedules, from the tenant configuration item `flows.quota` (a team may narrow the tenant's; the resolver
//! keeps the smaller number). Missing values take the safe defaults below.

use serde_json::Value;
use std::collections::BTreeMap;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Quota {
    /// Runs going at once in the whole tenant.
    pub max_runs_tenant: usize,
    /// Runs going at once for one person (the engine of that person).
    pub max_runs_person: usize,
    /// A run longer than this is stopped (seconds).
    pub max_run_secs: u64,
    /// Schedules (timed triggers) in the tenant.
    pub max_schedules: usize,
    /// Runs kept per flow, and for how many days.
    pub history_keep: usize,
    pub history_days: u64,
    /// A missed run older than this many seconds draws a notification.
    pub missed_notify_secs: u64,
}

impl Default for Quota {
    fn default() -> Quota {
        Quota { max_runs_tenant: 4, max_runs_person: 2, max_run_secs: 3600, max_schedules: 50, history_keep: 200, history_days: 30, missed_notify_secs: 900 }
    }
}

fn num(v: &BTreeMap<String, Value>, k: &str, lo: u64, hi: u64, d: u64) -> u64 {
    v.get(k).and_then(|x| x.as_u64()).map_or(d, |x| x.clamp(lo, hi))
}

impl Quota {
    pub fn from_values(v: &BTreeMap<String, Value>) -> Quota {
        let d = Quota::default();
        Quota {
            max_runs_tenant: num(v, "max_runs_tenant", 1, 1000, d.max_runs_tenant as u64) as usize,
            max_runs_person: num(v, "max_runs_person", 1, 1000, d.max_runs_person as u64) as usize,
            max_run_secs: num(v, "max_run_secs", 10, 7 * 86_400, d.max_run_secs),
            max_schedules: num(v, "max_schedules", 1, 10_000, d.max_schedules as u64) as usize,
            history_keep: num(v, "history_keep", 5, 10_000, d.history_keep as u64) as usize,
            history_days: num(v, "history_days", 1, 3650, d.history_days),
            missed_notify_secs: num(v, "missed_notify_secs", 60, 7 * 86_400, d.missed_notify_secs),
        }
    }

    /// The effective item of the tenant (or of a team), else the defaults.
    pub fn load(data: &std::path::Path, team: Option<&str>) -> Quota {
        use crate::tenantconfig::{resolve::resolve, schema, store::Store};
        let Some(spec) = schema::kind("flows.quota") else { return Quota::default() };
        let Ok(cfg) = Store::new(data).load() else { return Quota::default() };
        let entries = cfg.entries();
        let eff = resolve(spec, &cfg.layers(&entries, "flows.quota", team, "default"));
        if !eff.exists || !eff.enabled {
            return Quota::default();
        }
        Quota::from_values(&eff.values.into_iter().collect())
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Refusal {
    pub code: &'static str,
    pub reason: String,
}

/// May one more run start now?
pub fn admit_run(q: &Quota, tenant_running: usize, person_running: usize) -> Result<(), Refusal> {
    if tenant_running >= q.max_runs_tenant {
        return Err(Refusal { code: "tenant_runs", reason: format!("the tenant already has {tenant_running} runs going (limit {})", q.max_runs_tenant) });
    }
    if person_running >= q.max_runs_person {
        return Err(Refusal { code: "person_runs", reason: format!("this person already has {person_running} runs going (limit {})", q.max_runs_person) });
    }
    Ok(())
}

/// May one more schedule be created?
pub fn admit_schedule(q: &Quota, existing: usize) -> Result<(), Refusal> {
    if existing >= q.max_schedules {
        return Err(Refusal { code: "schedules", reason: format!("the tenant already has {existing} schedules (limit {})", q.max_schedules) });
    }
    Ok(())
}

/// Has a run used more time than allowed? (`flow_limit_s`: the flow's own, smaller limit.)
pub fn over_time(q: &Quota, started_ms: i64, now_ms: i64, flow_limit_s: Option<u64>) -> bool {
    if started_ms <= 0 {
        return false;
    }
    let limit = flow_limit_s.map_or(q.max_run_secs, |f| f.min(q.max_run_secs)) as i64 * 1000;
    now_ms - started_ms > limit
}
