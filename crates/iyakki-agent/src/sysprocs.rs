//! Which Quick install service or pod a process belongs to, for the System tile's Processes tab and the `system_processes` and
//! `system_pod_busy` chat tools.
//!
//! * [`parse_cgroup`] (pure): the kernel's control-group text of a process (the text of the process's `cgroup` file, read by
//!   [`crate::sysinfo`], the one reader) to the pod uid and container id of containerd/k3s, in the cgroup v2 systemd style
//!   (`kubepods-burstable-pod<uid with underscores>.slice/cri-containerd-<id>.scope`) and the v1 or cgroupfs style
//!   (`/kubepods/burstable/pod<uid>/<id>`).
//! * [`PodTable`]: the pod uid to namespace, pod, service and requests/limits table, from `kubectl get pods -A -o json` (read only,
//!   through the managed cluster's kubectl with its usual timeouts and size caps). With the cluster off or unreachable it is empty
//!   and every label is absent; nothing is reported as wrong.
//! * [`PodCache`]: the table kept fresh in the background for the tile.
//! * [`rows`], [`groups`], [`near_limit`]: what the tab and the tool show.
//! * [`busy_text`]: the recent log tail (redacted, capped) and `kubectl top` numbers of one pod. Nothing is guessed.

pub use iyakki_shared::sysprocs::*;
use crate::dash::cluster::Kubectl;
use std::sync::Arc;
use serde_json::Value;

// ------------------------------------------------------------------------------------------------------------ chat wiring

use crate::agent::{Risk, ToolDef};
use crate::chat::ToolSpec;
use crate::services::Services;
use iyakki_sec::policy::{CliLevel, Policy};
use serde_json::json;

pub const TOOLS: [&str; 2] = ["system_processes", "system_pod_busy"];

pub fn is_tool(name: &str) -> bool {
    TOOLS.contains(&name)
}

fn allowed(p: &Policy) -> bool {
    p.cli != CliLevel::Off && p.manage_settings
}

/// Both tools only read, so neither asks. Administrators only, like the Processes tab.
pub fn defs(p: &Policy) -> Vec<ToolDef> {
    if !allowed(p) {
        return Vec::new();
    }
    let t = |name: &str, description: &str, schema: Value| ToolDef { spec: ToolSpec { name: name.into(), description: description.into(), schema }, risk: Risk::Safe, untrusted: true };
    vec![
        t(
            "system_processes",
            "The busiest processes on this machine, each with the Quick install service or pod it belongs to (for example clickhouse in iyakki-data) and the pod's memory and processor requests and limits, with a NEAR ITS LIMIT marker. Read only. `sort`: cpu (default) or memory. `limit`: how many (default 15, at most 50). `group`: true adds up the processes of each service. Command lines are shown with secrets hidden.",
            json!({"type": "object", "properties": {"sort": {"type": "string", "enum": ["cpu", "memory"]}, "limit": {"type": "integer"}, "group": {"type": "boolean"}}, "required": []}),
        ),
        t(
            "system_pod_busy",
            "What a pod is busy with: its last log lines (secrets hidden, capped) and `kubectl top` numbers. Use it for a pod at high processor use (take the pod name from system_processes). Read only; it shows facts and does not guess a cause.",
            json!({"type": "object", "properties": {"pod": {"type": "string"}, "namespace": {"type": "string"}}, "required": ["pod"]}),
        ),
    ]
}

pub fn describe(name: &str, args: &Value) -> String {
    match name {
        "system_pod_busy" => format!("look at the log and usage of pod {}", args.get("pod").and_then(Value::as_str).unwrap_or("?")),
        _ => "list the busiest processes".into(),
    }
}

fn running(svc: &Services) -> bool {
    svc.k3sdev.state() == crate::k3sdev::Svc::Running
}

pub fn call(name: &str, args: &Value, svc: &Services, p: &Policy) -> Result<String, String> {
    if !allowed(p) {
        return Err("only administrators may look at the processes".into());
    }
    let kube: Arc<dyn Kubectl> = svc.k3sdev.clone();
    call_with(name, args, &crate::sysinfo::SysRoots::real(), &kube, running(svc))
}

/// [`call`] with the machine and the cluster given (the tests give fakes).
pub fn call_with(name: &str, args: &Value, roots: &crate::sysinfo::SysRoots, kube: &Arc<dyn Kubectl>, cluster_up: bool) -> Result<String, String> {
    let table = if cluster_up { fetch_table(&**kube) } else { PodTable::empty() };
    match name {
        "system_processes" => {
            let by_memory = args.get("sort").and_then(Value::as_str) == Some("memory");
            let limit = args.get("limit").and_then(Value::as_u64).unwrap_or(15) as usize;
            let group = args.get("group").and_then(Value::as_bool).unwrap_or(false);
            let before = crate::sysinfo::processes(roots);
            std::thread::sleep(std::time::Duration::from_millis(if cfg!(test) { 5 } else { 700 }));
            let after = crate::sysinfo::processes(roots);
            let cpu = crate::sysinfo::proc_cpu(&before, &after, if cfg!(test) { 1.0 } else { 0.7 });
            Ok(top_text(&rows(&after, &cpu, &table), by_memory, limit, group, !table.is_empty()))
        }
        "system_pod_busy" => {
            let pod = args.get("pod").and_then(Value::as_str).unwrap_or("").trim();
            let ns = args.get("namespace").and_then(Value::as_str).unwrap_or("").trim();
            busy_for(&**kube, &table, ns, pod)
        }
        _ => Err(format!("unknown tool {name}")),
    }
}

#[cfg(test)]
#[path = "sysprocs_tests.rs"]
mod tests;

#[cfg(test)]
use std::collections::HashMap;
