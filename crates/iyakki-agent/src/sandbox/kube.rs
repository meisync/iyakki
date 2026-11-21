//! The cluster door of the job executor, behind a trait so every test uses a fake. The real one runs `kubectl` (the program the
//! cluster tools already use: no shell, fixed arguments, manifests on standard input so a secret never touches a file or a command line).

use serde_json::Value;
use std::io::{Read, Write};
use iyakki_core_local::starters_launch::{Launch, Stream};
use std::time::{Duration, Instant};

/// What the cluster says about a run's Job and pod, boiled down.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct JobState {
    pub succeeded: bool,
    pub failed: bool,
    /// `DeadlineExceeded` (activeDeadlineSeconds).
    pub deadline_exceeded: bool,
    pub oom_killed: bool,
    pub exit_code: Option<i32>,
    pub started: bool,
    /// `ErrImagePull`, `ImagePullBackOff`, `InvalidImageName`, `CreateContainerConfigError`, ...
    pub waiting_reason: Option<String>,
    pub max_mem_mib: u32,
}

pub trait KubeApi: Send + Sync {
    /// Create or replace one object (`kubectl apply -f -`, the manifest on standard input).
    fn apply(&self, manifest: &Value) -> Result<(), String>;
    fn state(&self, ns: &str, run: &str) -> Result<JobState, String>;
    /// What the container printed (stdout and stderr together: the cluster does not separate them), at most `limit` bytes.
    fn logs(&self, ns: &str, run: &str, limit: usize) -> Result<Vec<u8>, String>;
    /// Files the run left in `/workspace/out` (planned for the real API: see the plan, section "Results").
    fn artifacts(&self, _ns: &str, _run: &str) -> Result<Vec<(String, Vec<u8>)>, String> {
        Ok(vec![])
    }
    /// The door to the team's own cluster, when this one has none of its own: the binding of (team, environment) with its credential from
    /// the vault (`binding.rs`). `None`: use this door as it is (fakes, and a `Kubectl` given a kubeconfig of its own).
    fn for_binding(&self, _env: &crate::tenantconfig::api::Env, _team: &str, _environment: &str) -> Option<std::sync::Arc<dyn KubeApi>> {
        None
    }
    /// Delete everything the run made (Job, NetworkPolicy, ConfigMap, Secret), found by the run label.
    fn delete_run(&self, ns: &str, run: &str) -> Result<(), String>;
}

/// `kubectl` against one kubeconfig.
pub struct Kubectl {
    pub program: String,
    pub kubeconfig: Option<String>,
    pub timeout: Duration,
}

impl Kubectl {
    pub fn new(kubeconfig: Option<String>) -> Kubectl {
        Kubectl { program: "kubectl".into(), kubeconfig, timeout: Duration::from_secs(60) }
    }

    pub(crate) fn run(&self, args: &[&str], stdin: Option<&[u8]>, cap: usize) -> Result<Vec<u8>, String> {
        let mut c = Launch::new(&self.program);
        if let Some(k) = &self.kubeconfig {
            c = c.arg("--kubeconfig").arg(k);
        }
        let c = c.args(args).stdin(if stdin.is_some() { Stream::Piped } else { Stream::Null }).stdout(Stream::Piped).stderr(Stream::Piped).env_clear().pass_env(["PATH"]);
        let mut child = c.spawn().map_err(|e| format!("cannot start {}: {e}", self.program))?;
        // what the cluster is asked for a person (an editor script, a test run) is hers in "My processes" while it runs (KI-131)
        let _claim = crate::procowner::current_user().map(|u| crate::procowner::claim(child.id(), &u, "kubectl"));
        if let (Some(data), Some(mut si)) = (stdin, child.stdin.take()) {
            si.write_all(data).map_err(|e| e.to_string())?;
        }
        let mut out = child.stdout.take().ok_or("no output pipe")?;
        let mut err = child.stderr.take().ok_or("no error pipe")?;
        let reader = std::thread::spawn(move || {
            let mut b = Vec::new();
            let _ = (&mut out).take(cap as u64).read_to_end(&mut b);
            b
        });
        let ereader = std::thread::spawn(move || {
            let mut b = Vec::new();
            let _ = (&mut err).take(8192).read_to_end(&mut b);
            b
        });
        let end = Instant::now() + self.timeout;
        let state = loop {
            match iyakki_core_api::child_status(&mut child) {
                s if s.is_over() => break s,
                _ if Instant::now() > end => {
                    let _ = child.kill();
                    let _ = child.wait();
                    return Err("kubectl did not answer in time".into());
                }
                _ => std::thread::sleep(Duration::from_millis(20)),
            }
        };
        let o = reader.join().unwrap_or_default();
        let e = ereader.join().unwrap_or_default();
        // a status taken by the PID 1 reaper: the answer counts when kubectl printed one and no error
        let gone_ok = state == iyakki_core_api::ChildState::Gone && !o.is_empty() && e.iter().all(|b| b.is_ascii_whitespace());
        if state.success() || gone_ok {
            Ok(o)
        } else {
            Err(String::from_utf8_lossy(&e).lines().next().unwrap_or("kubectl failed").chars().take(300).collect())
        }
    }
}

impl KubeApi for Kubectl {
    fn for_binding(&self, env: &crate::tenantconfig::api::Env, team: &str, environment: &str) -> Option<std::sync::Arc<dyn KubeApi>> {
        if self.kubeconfig.is_some() {
            return None;
        }
        let dir = std::env::temp_dir().join("iyakki-sandbox-kc");
        super::binding::BindingKube::from_binding(env, team, environment, &dir).map(|k| std::sync::Arc::new(k) as std::sync::Arc<dyn KubeApi>)
    }
    fn apply(&self, manifest: &Value) -> Result<(), String> {
        self.run(&["apply", "-f", "-"], Some(manifest.to_string().as_bytes()), 4096).map(|_| ())
    }
    fn state(&self, ns: &str, run: &str) -> Result<JobState, String> {
        let job = self.run(&["get", "job", run, "-n", ns, "-o", "json"], None, 1 << 20)?;
        let pods = self.run(&["get", "pods", "-n", ns, "-l", &format!("{}={run}", super::job::RUN_LABEL), "-o", "json"], None, 1 << 20)?;
        let j: Value = serde_json::from_slice(&job).map_err(|e| e.to_string())?;
        let p: Value = serde_json::from_slice(&pods).map_err(|e| e.to_string())?;
        Ok(parse_state(&j, &p))
    }
    fn logs(&self, ns: &str, run: &str, limit: usize) -> Result<Vec<u8>, String> {
        self.run(&["logs", &format!("job/{run}"), "-n", ns, &format!("--limit-bytes={limit}")], None, limit)
    }
    fn delete_run(&self, ns: &str, run: &str) -> Result<(), String> {
        self.run(&["delete", "job,networkpolicy,configmap,secret", "-n", ns, "-l", &format!("iyakki.dev/run={run}"), "--ignore-not-found", "--wait=false"], None, 4096).map(|_| ())
    }
}

/// Read a Job and its pods (as `kubectl get -o json` gives them).
pub fn parse_state(job: &Value, pods: &Value) -> JobState {
    let mut s = JobState::default();
    s.succeeded = job.pointer("/status/succeeded").and_then(Value::as_u64).unwrap_or(0) >= 1;
    s.failed = job.pointer("/status/failed").and_then(Value::as_u64).unwrap_or(0) >= 1;
    s.deadline_exceeded = job.pointer("/status/conditions").and_then(Value::as_array).is_some_and(|c| c.iter().any(|x| x.get("reason").and_then(Value::as_str) == Some("DeadlineExceeded")));
    if let Some(c) = pods.pointer("/items/0/status/containerStatuses/0") {
        s.waiting_reason = c.pointer("/state/waiting/reason").and_then(Value::as_str).map(String::from);
        s.started = c.get("started").and_then(Value::as_bool).unwrap_or(false) || c.pointer("/state/running").is_some() || c.pointer("/state/terminated").is_some();
        let term = c.pointer("/state/terminated").or_else(|| c.pointer("/lastState/terminated"));
        if let Some(t) = term {
            s.exit_code = t.get("exitCode").and_then(Value::as_i64).map(|x| x as i32);
            s.oom_killed = t.get("reason").and_then(Value::as_str) == Some("OOMKilled");
        }
    }
    s
}
