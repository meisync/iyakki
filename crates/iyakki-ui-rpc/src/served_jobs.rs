//! The platform calls that report progress while they run (F-SPLIT-03, D-SPLIT-03-APPS): deploy (`apply`, `apply_with`), upgrade, waiting for a
//! plan, repairing the private store. In the trait they take a `&mut dyn FnMut(&str)` the UI gives; over the socket the call STARTS a job on the
//! core (the core runs the real method on a thread with its own progress closure) and answers the object of that job; the UI polls it, hands each
//! line to its own closure and gets the method's own result at the end. A stop is a `cancel` of the job (what the core's method does with it is
//! the method's own: these calls end themselves, a cancel only stops the polling that waits for them).
//!
//! `apply_with` took an HTTP object of the UI for the registry: the UI is never given a way to make the core talk through an object of its own,
//! so the core's own transport is used and the object is dropped (D-SPLIT-03-ENV). The three calls that take the install's services as an
//! `Arc<dyn Any>` (`private_store_state`, `private_finish_local`, `private_start_job`) are served too (D-SPLIT-03-APPS2): the services are the
//! core's and cannot be named by the UI, so the wire carries the data arguments only and the core passes its own services
//! ([`crate::Host::with_services`]) and its own program for the background job; a host without services refuses by name.

use std::sync::Arc;
use std::time::Duration;

use iyakki_agent_api::PlatformApi;
use iyakki_shared::devcluster::ClusterProfile;
use iyakki_shared::platform_types::{Applied, Options, Plan, UpgradePlan, Wait};
use iyakki_shared::platform_types::ClusterRef;
use iyakki_shared::privatemanage::{Action, JobArgs, Report, StoreState, Why};
use serde::de::DeserializeOwned;
use serde::Serialize;
use serde_json::{json, Value};

use crate::served::Served;
use crate::served_apps::{drive, Jobs};
use crate::{de, ser, Obj, Registry, Rpc};

/// One job of the core as an object: poll it, cancel it, let go of it. The job starts with the object.
struct JobObj {
    jobs: Jobs<Value>,
    id: u64,
}

impl Served for JobObj {
    fn iface(&self) -> &'static str {
        "Job"
    }
    fn serve(&self, _reg: &Registry<'_>, method: &str, _a: Value) -> Result<Value, String> {
        match method {
            "poll" => ser(&self.jobs.poll(self.id)?),
            "cancel" => ser(&self.jobs.cancel(self.id)),
            other => Err(format!("Job has no call {other}")),
        }
    }
}

/// Run `f` as a job of the core and hand the UI its object. `f` says progress lines through the function it is given and returns the whole
/// answer of the method as a value.
pub(crate) fn start_job(reg: &Registry<'_>, f: impl FnOnce(&dyn Fn(&str) -> bool) -> Value + Send + 'static) -> Result<Value, String> {
    let jobs: Jobs<Value> = Jobs::new();
    let id = jobs.start(move |say, _| f(say))?;
    ser(&reg.add(Obj::Served(Arc::new(JobObj { jobs, id }))))
}

/// The UI's end: poll the job object `id` until the method has answered, hand every progress line to `progress`, read the answer as `T`.
pub(crate) fn finish_job<T: DeserializeOwned + crate::Lost>(rpc: &Rpc, id: Result<u64, String>, progress: &mut dyn FnMut(&str)) -> T {
    let mut run = |id: u64| -> Result<T, String> {
        let v = drive::<Value>(rpc, "Job", id, 0, &mut |l| {
            progress(l);
            true
        })?;
        serde_json::from_value(v).map_err(|e| format!("the answer of the job has the wrong shape: {e}"))
    };
    let out = id.and_then(|id| {
        let r = run(id);
        rpc.release(id);
        r
    });
    out.unwrap_or_else(|e| T::lost(&e))
}

/// `Applied` on the wire. A value to show once (the generated admin password) is a secret and never crosses the socket (D-SPLIT-03-SECRETS): the
/// UI gets what it is called and where it was kept, and the person reads the value in that vault.
#[derive(Serialize, serde::Deserialize)]
struct WApplied {
    done: Vec<String>,
    urls: Vec<iyakki_shared::platform_types::UrlInfo>,
    reveal: Vec<(String, String)>,
    ready: bool,
    how_to_use: String,
    instance: String,
}
impl From<Applied> for WApplied {
    fn from(a: Applied) -> WApplied {
        WApplied { done: a.done, urls: a.urls, reveal: a.reveal.iter().map(|r| (r.label.clone(), r.vault.clone())).collect(), ready: a.ready, how_to_use: a.how_to_use, instance: a.instance }
    }
}
impl From<WApplied> for Applied {
    fn from(a: WApplied) -> Applied {
        Applied { done: a.done, urls: a.urls, reveal: a.reveal.into_iter().map(|(label, vault)| iyakki_shared::platform_types::Reveal { label, vault, value: zeroize::Zeroizing::new(String::new()) }).collect(), ready: a.ready, how_to_use: a.how_to_use, instance: a.instance }
    }
}

fn wait_of(w: (u32, u64)) -> Wait {
    Wait { tries: w.0, pause: Duration::from_millis(w.1) }
}

// ---------------------------------------------------------------------------------------------------------------- the core's end (`s_`)

fn job<T: Serialize + Send + 'static>(reg: &Registry<'_>, f: impl FnOnce(&dyn Fn(&str) -> bool) -> T + Send + 'static) -> Result<Value, String> {
    start_job(reg, move |say| serde_json::to_value(f(say)).unwrap_or(Value::Null))
}

pub(crate) fn s_PlatformApi_apply(t: &Arc<dyn PlatformApi>, reg: &Registry<'_>, a: Value) -> Result<Value, String> {
    let (plan, approved): (Plan, bool) = de(a)?;
    let t = t.clone();
    job(reg, move |say| t.apply(&plan, approved, &mut |l| {
        say(l);
    }).map(WApplied::from))
}

/// The UI's registry object is dropped: the core's own transport pushes (D-SPLIT-03-ENV).
pub(crate) fn s_PlatformApi_apply_with(t: &Arc<dyn PlatformApi>, reg: &Registry<'_>, a: Value) -> Result<Value, String> {
    s_PlatformApi_apply(t, reg, a)
}

pub(crate) fn s_PlatformApi_upgrade(t: &Arc<dyn PlatformApi>, reg: &Registry<'_>, a: Value) -> Result<Value, String> {
    let (plan, approved): (UpgradePlan, bool) = de(a)?;
    let t = t.clone();
    job(reg, move |say| t.upgrade(&plan, approved, &mut |l| {
        say(l);
    }))
}

pub(crate) fn s_PlatformApi_plan_waiting(t: &Arc<dyn PlatformApi>, reg: &Registry<'_>, a: Value) -> Result<Value, String> {
    let (service, profile, given, wait): (String, ClusterProfile, Options, (u32, u64)) = de(a)?;
    let t = t.clone();
    job(reg, move |say| t.plan_waiting(&service, &profile, &given, wait_of(wait), &mut |l| {
        say(l);
    }))
}

pub(crate) fn s_PlatformApi_private_repair(t: &Arc<dyn PlatformApi>, reg: &Registry<'_>, a: Value) -> Result<Value, String> {
    let (rep, profile, opts): (Report, ClusterProfile, Options) = de(a)?;
    let t = t.clone();
    job(reg, move |say| t.private_repair(&rep, &profile, &opts, &mut |l| {
        say(l);
    }))
}

fn core_services(reg: &Registry<'_>) -> Result<Arc<dyn std::any::Any + Send + Sync>, String> {
    reg.services().ok_or_else(|| format!("{}: this core gave the socket no services of its own", crate::served::NOT_SERVED))
}

/// What state the private store is in. The UI says only whether it had services (`true`: the core reads its own STORE entries too); the services
/// are the core's.
pub(crate) fn s_PlatformApi_private_store_state(t: &Arc<dyn PlatformApi>, reg: &Registry<'_>, a: Value) -> Result<Value, String> {
    let (had_services, cluster, namespace): (bool, ClusterRef, String) = de(a)?;
    let services = if had_services { Some(core_services(reg)?) } else { None };
    let t = t.clone();
    job(reg, move |_| Ok::<Report, String>(t.private_store_state(services, &cluster, &namespace)))
}

/// The device side of a reset or removal; every line it says reaches the UI in order.
pub(crate) fn s_PlatformApi_private_finish_local(t: &Arc<dyn PlatformApi>, reg: &Registry<'_>, a: Value) -> Result<Value, String> {
    let (rep, action, registry): (Report, Action, Option<ClusterProfile>) = de(a)?;
    let services = core_services(reg)?;
    let t = t.clone();
    job(reg, move |say| {
        t.private_finish_local(services, &rep, &action, registry.as_ref(), &mut |l| {
            say(l);
        })
    })
}

/// Start the background job of a reset or removal: the core's own services and its own program (`None`), never one the UI names.
pub(crate) fn s_PlatformApi_private_start_job(t: &Arc<dyn PlatformApi>, reg: &Registry<'_>, a: Value) -> Result<Value, String> {
    let (args,): (JobArgs,) = de(a)?;
    let services = core_services(reg)?;
    ser(&t.private_start_job(services, None, &args))
}

// ---------------------------------------------------------------------------------------------------------------- the UI's end (`c_`)

type Remote = crate::RemotePlatformApi;

pub(crate) fn c_PlatformApi_apply(s: &Remote, plan: &Plan, approved: bool, progress: &mut dyn FnMut(&str)) -> Result<Applied, String> {
    finish_job::<Result<WApplied, String>>(&s.rpc, s.rpc.call::<u64>("PlatformApi", s.obj, "apply", json!([plan, approved])), progress).map(Applied::from)
}

pub(crate) fn c_PlatformApi_apply_with(s: &Remote, plan: &Plan, approved: bool, progress: &mut dyn FnMut(&str), _http: &dyn iyakki_shared::ocipush::RegistryHttp) -> Result<Applied, String> {
    finish_job::<Result<WApplied, String>>(&s.rpc, s.rpc.call::<u64>("PlatformApi", s.obj, "apply_with", json!([plan, approved])), progress).map(Applied::from)
}

pub(crate) fn c_PlatformApi_upgrade(s: &Remote, u: &UpgradePlan, approved: bool, progress: &mut dyn FnMut(&str)) -> Result<Vec<String>, String> {
    finish_job(&s.rpc, s.rpc.call::<u64>("PlatformApi", s.obj, "upgrade", json!([u, approved])), progress)
}

pub(crate) fn c_PlatformApi_plan_waiting(s: &Remote, service: &str, profile: &ClusterProfile, given: &Options, wait: Wait, say: &mut dyn FnMut(&str)) -> Result<Plan, String> {
    finish_job(&s.rpc, s.rpc.call::<u64>("PlatformApi", s.obj, "plan_waiting", json!([service, profile, given, (wait.tries, wait.pause.as_millis() as u64)])), say)
}

pub(crate) fn c_PlatformApi_private_repair(s: &Remote, rep: &Report, profile: &ClusterProfile, opts: &Options, progress: &mut dyn FnMut(&str)) -> Result<Vec<String>, String> {
    finish_job(&s.rpc, s.rpc.call::<u64>("PlatformApi", s.obj, "private_repair", json!([rep, profile, opts])), progress)
}

/// A report for a core that cannot be asked: the store is shown as unreachable and the reason is the line the person reads.
pub(crate) fn lost_report(cluster: &ClusterRef, namespace: &str, why: &str) -> Report {
    Report { state: StoreState::Broken { why: Why::ClusterDown, reason: why.to_string() }, cluster: cluster.clone(), namespace: namespace.to_string(), instance: String::new(), facts: Default::default(), record: None, elsewhere: None, source: None }
}

pub(crate) fn c_PlatformApi_private_store_state(s: &Remote, services: Option<Arc<dyn std::any::Any + Send + Sync>>, cluster: &ClusterRef, namespace: &str) -> Report {
    let id = s.rpc.call::<u64>("PlatformApi", s.obj, "private_store_state", json!([services.is_some(), cluster, namespace]));
    match finish_job::<Result<Report, String>>(&s.rpc, id, &mut |_| {}) {
        Ok(r) => r,
        Err(e) => lost_report(cluster, namespace, &e),
    }
}

pub(crate) fn c_PlatformApi_private_finish_local(s: &Remote, _services: Arc<dyn std::any::Any + Send + Sync>, rep: &Report, action: &Action, registry: Option<&ClusterProfile>, say: &mut dyn FnMut(&str)) -> Result<Vec<String>, String> {
    finish_job(&s.rpc, s.rpc.call::<u64>("PlatformApi", s.obj, "private_finish_local", json!([rep, action, registry])), say)
}

pub(crate) fn c_PlatformApi_private_start_job(s: &Remote, _services: Arc<dyn std::any::Any + Send + Sync>, _launch: Option<Arc<dyn std::any::Any + Send + Sync>>, args: &JobArgs) -> Result<iyakki_shared::jobs::Job, String> {
    s.rpc.call::<Result<iyakki_shared::jobs::Job, String>>("PlatformApi", s.obj, "private_start_job", json!([args]))?
}
