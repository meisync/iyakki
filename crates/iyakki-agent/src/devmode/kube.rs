//! `kube` scope: a READ-ONLY look at the dev cluster from a paired computer (`iyakki dev kube get|describe|logs|top|events ...`), without
//! a root shell. The device runs the managed `kubectl` (`k3s kubectl`, the kubeconfig stays root-only) for the computer, after
//! [`plan`] has checked the command line:
//!
//! * only these verbs run: `get`, `describe`, `logs`, `top`, `events`, `api-resources`, `version` and `auth can-i`; everything else
//!   (`exec`, `cp`, `port-forward`, `apply`, `delete`, `edit`, `patch`, `create`, `run`, `scale`, `rollout`, `drain`, ...) is refused by
//!   name, and an unknown verb is refused too;
//! * only flags from a fixed list: nothing that picks another cluster, user or kubeconfig (`--kubeconfig`, `--server`, `--token`,
//!   `--as`, `--context` ...), reads a file (`-f`), streams (`--watch`, `--follow`) or talks to the API server raw (`--raw`);
//! * Secrets are never shown: any resource kind that has "secret" in its name is refused, and so is structured output (`-o yaml`, `json`,
//!   `jsonpath`, `go-template`, `custom-columns`) of ConfigMaps (use `describe` for those);
//! * every line of the answer goes through the credential filter, the answer is at most [`MAX_OUT`] bytes and `kubectl` has 20 s;
//! * the audit record holds the verb and the resource kind only, never names, arguments or output.
//!
//! There is no write mode.

use serde_json::{json, Value};

/// Most bytes of an answer.
pub const MAX_OUT: usize = 64 * 1024;
/// Most arguments of one command and the longest one.
pub const MAX_ARGS: usize = 24;
pub const MAX_ARG_LEN: usize = 200;
/// The most `logs --tail` lines, and the number used when none is given.
pub const MAX_TAIL: u32 = 2000;
pub const DEFAULT_TAIL: u32 = 200;

const VERBS: [&str; 8] = ["get", "describe", "logs", "top", "events", "api-resources", "version", "auth"];

/// Verbs that change the cluster or reach into it: named in the refusal so the message is clear.
const WRITE_VERBS: [&str; 29] = [
    "exec", "cp", "port-forward", "apply", "delete", "edit", "patch", "create", "run", "scale", "rollout", "drain", "cordon", "uncordon", "taint", "label", "annotate", "set", "replace", "expose", "attach", "debug", "proxy", "config",
    "certificate", "wait", "kustomize", "diff", "autoscale",
];

/// Flags that take a value, and flags that do not (long form; the short forms are listed in [`short`]).
const VALUE_FLAGS: [&str; 16] = [
    "--namespace", "--selector", "--output", "--container", "--field-selector", "--tail", "--since", "--since-time", "--sort-by", "--limit", "--chunk-size", "--for", "--types", "--label-columns", "--limit-bytes", "--api-group",
];
const BOOL_FLAGS: [&str; 16] = [
    "--all-namespaces", "--previous", "--timestamps", "--no-headers", "--show-labels", "--show-kind", "--all-containers", "--containers", "--prefix", "--ignore-not-found", "--show-events", "--sum", "--namespaced", "--cached", "--client", "--list",
];

fn short(f: &str) -> Option<&'static str> {
    Some(match f {
        "-n" => "--namespace",
        "-l" => "--selector",
        "-o" => "--output",
        "-c" => "--container",
        "-L" => "--label-columns",
        "-A" => "--all-namespaces",
        "-p" => "--previous",
        _ => return None,
    })
}

/// A command line that passed the checks: what to run and what the audit log may say about it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Plan {
    pub verb: String,
    /// The resource kinds asked for (lower case, "pod", "node"...), for the audit record.
    pub kinds: Vec<String>,
    /// The arguments to give `kubectl`, in order.
    pub argv: Vec<String>,
}

impl Plan {
    /// `get pods`: all the audit log keeps of a command.
    pub fn audit(&self) -> String {
        let kinds = self.kinds.join(",");
        let t = if kinds.is_empty() { self.verb.clone() } else { format!("{} {}", self.verb, kinds) };
        t.chars().take(80).collect()
    }
}

fn kind_of(spec: &str) -> String {
    spec.split('/').next().unwrap_or("").split('.').next().unwrap_or("").to_ascii_lowercase()
}

fn is_secret_kind(k: &str) -> bool {
    k.contains("secret")
}

fn is_configmap(k: &str) -> bool {
    matches!(k, "cm" | "configmap" | "configmaps")
}

fn structured(o: &str) -> bool {
    let o = o.to_ascii_lowercase();
    ["yaml", "json", "jsonpath", "go-template", "template", "custom-columns"].iter().any(|p| o == *p || o.starts_with(&format!("{p}=")) || o.starts_with(&format!("{p}-")))
}

fn valid_output(o: &str) -> Result<(), String> {
    let l = o.to_ascii_lowercase();
    let base = l.split('=').next().unwrap_or("");
    if base.ends_with("-file") || base.ends_with("file") && base != "profile" {
        return Err("output templates read from a file are not allowed".into());
    }
    if ["wide", "name", "yaml", "json", "jsonpath", "jsonpath-as-json", "go-template", "template", "custom-columns"].contains(&base) {
        Ok(())
    } else {
        Err(format!("output format {base:?} is not available (wide, name, yaml, json, jsonpath=..., go-template=..., custom-columns=...)"))
    }
}

fn plain_chars(s: &str, allowed: &str) -> bool {
    s.chars().all(|c| c.is_ascii_alphanumeric() || allowed.contains(c))
}

fn check_value(flag: &str, v: &str) -> Result<(), String> {
    match flag {
        "--namespace" if !(!v.is_empty() && v.len() <= 63 && plain_chars(v, "-.")) => Err("a namespace is letters, digits, - and .".into()),
        "--selector" | "--field-selector" | "--label-columns" if !plain_chars(v, "_-.,=!()/ :") => Err(format!("{flag}: letters, digits and _ - . , = ! ( ) / : only")),
        "--container" if !(!v.is_empty() && plain_chars(v, "-._")) => Err("a container name is letters, digits, - _ .".into()),
        "--since" | "--since-time" | "--sort-by" | "--for" | "--types" | "--api-group" if v.is_empty() => Err(format!("{flag} needs a value")),
        "--tail" | "--limit" | "--chunk-size" | "--limit-bytes" if v.parse::<i64>().is_err() => Err(format!("{flag} takes a number")),
        "--output" => valid_output(v),
        _ => Ok(()),
    }
}

/// Check a command line (`args` is everything after `iyakki dev kube`, e.g. `["get", "pods", "-n", "kube-system"]`) and make the
/// arguments to run. Nothing is run here.
pub fn plan(args: &[String]) -> Result<Plan, String> {
    if args.is_empty() {
        return Err(usage());
    }
    if args.len() > MAX_ARGS {
        return Err(format!("at most {MAX_ARGS} arguments"));
    }
    for a in args {
        if a.len() > MAX_ARG_LEN || a.chars().any(|c| c.is_control()) {
            return Err(format!("an argument is 1 to {MAX_ARG_LEN} characters without control characters"));
        }
    }
    let verb = args[0].as_str();
    if verb.starts_with('-') {
        return Err("put the verb first: iyakki dev kube get|describe|logs|top|events ... (no flags before it: the cluster is chosen by the device)".into());
    }
    if WRITE_VERBS.contains(&verb) {
        return Err(format!("{verb} is not allowed: the kube scope is read-only (get, describe, logs, top, events, api-resources, version, auth can-i)"));
    }
    if !VERBS.contains(&verb) {
        return Err(format!("unknown command {verb:?}: get, describe, logs, top, events, api-resources, version or auth can-i"));
    }
    let mut pos: Vec<String> = Vec::new();
    let mut flags: Vec<(String, Option<String>)> = Vec::new();
    let mut it = args[1..].iter().peekable();
    while let Some(a) = it.next() {
        if a == "--" {
            return Err("-- is not allowed".into());
        }
        if a.starts_with('-') && a.len() > 1 {
            let (name, inline) = match a.split_once('=').filter(|_| a.starts_with("--")) {
                Some((k, v)) => (k.to_string(), Some(v.to_string())),
                None => (a.clone(), None),
            };
            let long = if name.starts_with("--") { name.clone() } else { short(&name).ok_or_else(|| format!("{name} is not an allowed flag"))?.to_string() };
            if VALUE_FLAGS.contains(&long.as_str()) {
                let v = match inline {
                    Some(v) => v,
                    None => it.next().cloned().ok_or_else(|| format!("{name} needs a value"))?,
                };
                check_value(&long, &v)?;
                flags.push((long, Some(v)));
            } else if BOOL_FLAGS.contains(&long.as_str()) {
                if inline.as_deref().is_some_and(|v| v != "true" && v != "false") {
                    return Err(format!("{long} takes no value"));
                }
                flags.push((long, None));
            } else {
                let why = match long.as_str() {
                    "--watch" | "-w" | "--follow" | "-f" => "streaming and files are not available",
                    "--kubeconfig" | "--server" | "-s" | "--token" | "--as" | "--as-group" | "--as-uid" | "--context" | "--cluster" | "--user" | "--certificate-authority" | "--client-certificate" | "--client-key" | "--insecure-skip-tls-verify" | "--username" | "--password" => "the device chooses the cluster and the identity",
                    "--raw" => "raw API requests are not available",
                    "--filename" | "--kustomize" | "-k" | "--recursive" | "-R" => "files are not available",
                    _ => "that flag is not on the list",
                };
                return Err(format!("{long} is not allowed: {why}"));
            }
        } else {
            pos.push(a.clone());
        }
    }
    let has = |f: &str| flags.iter().any(|(k, _)| k == f);
    let val = |f: &str| flags.iter().find(|(k, _)| k == f).and_then(|(_, v)| v.clone());
    let out_structured = val("--output").is_some_and(|o| structured(&o));
    let mut kinds: Vec<String> = Vec::new();
    let mut argv: Vec<String> = vec![verb.to_string()];
    match verb {
        "get" | "describe" => {
            let Some(first) = pos.first() else {
                if verb == "get" {
                    return Err("get what? e.g. iyakki dev kube get pods -A".into());
                }
                return Err("describe what? e.g. iyakki dev kube describe pod NAME -n NAMESPACE".into());
            };
            for part in first.split(',') {
                kinds.push(kind_of(part));
            }
            for later in pos.iter().skip(1).filter(|p| p.contains('/')) {
                kinds.push(kind_of(later));
            }
            kinds.retain(|k| !k.is_empty());
            kinds.dedup();
            if kinds.iter().any(|k| is_secret_kind(k)) {
                return Err("Secrets are never shown through developer mode: the kube scope blocks every resource with \"secret\" in its kind".into());
            }
            if out_structured && kinds.iter().any(|k| is_configmap(k)) {
                return Err("structured output of ConfigMaps is blocked (a ConfigMap can hold a secret): use plain `get configmaps` or `describe configmap NAME`".into());
            }
            if has("--show-managed-fields") {
                return Err("--show-managed-fields is not allowed".into());
            }
        }
        "logs" => {
            if pos.is_empty() && val("--selector").is_none() {
                return Err("logs of what? e.g. iyakki dev kube logs POD -n NAMESPACE [-c CONTAINER] [--previous] [--tail N]".into());
            }
            if let Some(f) = pos.first() {
                let k = kind_of(f);
                if is_secret_kind(&k) {
                    return Err("Secrets are never shown through developer mode".into());
                }
                kinds.push(if f.contains('/') { k } else { "pod".into() });
            } else {
                kinds.push("pod".into());
            }
            if pos.len() > 2 {
                return Err("logs takes one pod (or type/name)".into());
            }
            if let Some(t) = val("--tail") {
                let n: i64 = t.parse().unwrap_or(-1);
                if !(0..=MAX_TAIL as i64).contains(&n) {
                    return Err(format!("--tail is 0 to {MAX_TAIL} lines (200 when not given)"));
                }
            }
        }
        "top" => {
            let Some(k) = pos.first().map(|p| kind_of(p)) else { return Err("top what? iyakki dev kube top pods|nodes".into()) };
            if !matches!(k.as_str(), "pod" | "pods" | "po" | "node" | "nodes" | "no") {
                return Err("top shows pods or nodes".into());
            }
            kinds.push(k);
        }
        "events" => {
            if let Some(f) = val("--for") {
                let k = kind_of(&f);
                if is_secret_kind(&k) {
                    return Err("Secrets are never shown through developer mode".into());
                }
            }
            kinds.push("event".into());
            if !pos.is_empty() {
                return Err("events takes no name: use --for KIND/NAME".into());
            }
        }
        "api-resources" | "version" => {
            if !pos.is_empty() {
                return Err(format!("{verb} takes no name"));
            }
        }
        "auth" => {
            if pos.first().map(String::as_str) != Some("can-i") {
                return Err("auth: only `auth can-i VERB RESOURCE` is available".into());
            }
            if has("--list") && pos.len() > 1 {
                return Err("auth can-i --list takes no verb".into());
            }
            kinds.push("can-i".into());
        }
        _ => {}
    }
    // rebuild: the positionals first, then the flags in their long form
    argv.extend(pos.iter().cloned());
    for (k, v) in &flags {
        match v {
            Some(v) => argv.push(format!("{k}={v}")),
            None => argv.push(k.clone()),
        }
    }
    if verb == "logs" {
        if val("--tail").is_none() {
            argv.push(format!("--tail={DEFAULT_TAIL}"));
        }
        if val("--limit-bytes").is_none() {
            argv.push(format!("--limit-bytes={}", MAX_OUT - 4096));
        }
    }
    Ok(Plan { verb: verb.to_string(), kinds, argv })
}

pub fn usage() -> String {
    "usage: iyakki dev kube get|describe|logs|top|events|api-resources|version|auth can-i ...  (read-only; needs the kube permission)
  get pods -A | get nodes -o wide | get pod NAME -n NS -o yaml | describe pod NAME -n NS
  logs POD -n NS [-c CONTAINER] [--previous] [--tail N] | top pods -A | top nodes | events -A
  Secrets are blocked; exec, cp, port-forward, apply, delete, edit, patch, create, run, scale, rollout, drain and the rest are refused.".into()
}

/// What `kubectl` answered.
#[derive(Clone, Debug, Default)]
pub struct KubeOut {
    pub code: Option<i32>,
    pub stdout: String,
    pub stderr: String,
    pub timed_out: bool,
    pub missing: bool,
}

/// The device's way to run its managed `kubectl` (the real one is [`SvcKube`]; tests use a fake).
pub trait Kubectl {
    /// Is a cluster installed on this device at all?
    fn installed(&self) -> bool;
    fn run(&self, argv: &[String]) -> KubeOut;
}

/// The managed cluster of a device's services.
pub struct SvcKube<'a>(pub &'a crate::services::Services);

impl Kubectl for SvcKube<'_> {
    fn installed(&self) -> bool {
        self.0.k3sdev.installed().is_some()
    }

    fn run(&self, argv: &[String]) -> KubeOut {
        let refs: Vec<&str> = argv.iter().map(String::as_str).collect();
        let o = self.0.k3sdev.kubectl(&refs);
        KubeOut { code: o.code, stdout: o.stdout, stderr: o.stderr, timed_out: o.timed_out, missing: o.missing }
    }
}

/// Hide the lines that look like a credential; cut to [`MAX_OUT`] (at a line end); say so when it was cut.
pub fn clean_output(text: &str) -> (String, bool) {
    let mut out = String::new();
    let mut cut = false;
    for l in text.lines() {
        let l = super::debug::scrub(l);
        if out.len() + l.len() + 1 > MAX_OUT {
            cut = true;
            break;
        }
        out.push_str(&l);
        out.push('\n');
    }
    if cut {
        out.push_str(&format!("[... cut: the answer is limited to {} KiB; narrow it with -n, -l or --tail]\n", MAX_OUT / 1024));
    }
    (out, cut)
}

/// Run a checked plan. `Err` when `kubectl` could not run at all (no cluster, not installed); a `kubectl` that answered with an error
/// is an `Ok` with its exit code and message.
pub fn run(k: &dyn Kubectl, plan: &Plan) -> Result<Value, String> {
    if !k.installed() {
        return Err("no dev cluster is installed on this device (Settings, Linux / dev cluster)".into());
    }
    let o = k.run(&plan.argv);
    if o.missing {
        return Err("the cluster program is missing on this device".into());
    }
    if o.timed_out {
        return Err("kubectl took longer than 20 seconds (is the cluster starting?)".into());
    }
    let mut text = o.stdout;
    if !o.stderr.trim().is_empty() {
        if !text.is_empty() && !text.ends_with('\n') {
            text.push('\n');
        }
        text.push_str(&o.stderr);
    }
    let (text, truncated) = clean_output(&text);
    Ok(json!({ "text": text, "code": o.code, "truncated": truncated }))
}
