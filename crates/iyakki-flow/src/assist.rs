//! FLOWS assist: how far a configured agent may go on a flow by itself. This file is pure code (no files, no model, no clock):
//! the permission matrix (mode x action class x guards), the guards that decide whether a change may be published without asking,
//! and the read of a diff that the guards need (which settings it touches, which hosts it adds, whether a step gains an external
//! effect). The suggestions and counters live in [`crate::assist_store`], the work itself in [`crate::assist_run`].
//!
//! The rules in one place (docs/development/FLOW-AUTOPILOT-PLAN.md):
//!
//! * `Off`: the agent does nothing by itself. `Copilot`: it drafts and tests; every publish asks. `Autopilot`: it drafts and tests;
//!   it publishes by itself only for a FIX, only when the person switched `auto_publish` on for that flow, and only when every
//!   guard passes; a failed guard turns the publish into a question that carries the reasons.
//! * A guard that was not evaluated counts as failed. Nothing here trusts the model: the classes and guards are decided by the
//!   harness from the diff.
//! * APPLY-TO-RUNS always asks, DELETE is never done by the agent, ROLLBACK is automatic only for a version the agent itself
//!   published whose first runs failed.

use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::model::*;
use crate::store::FlowDiff;

/// What kind of work. The first two make a draft, the rest act on published versions.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Class {
    /// Patch the failing step's settings after a failed run.
    Fix,
    /// Newer library versions, changed names, dependency bumps.
    Upgrade,
    /// Promote a tested draft to the published version.
    Publish,
    /// Publish an older version again.
    Rollback,
    /// Hand running and scheduled tasks a newer step.
    ApplyToRuns,
    /// Delete a flow or a version.
    Delete,
}

impl Class {
    pub const ALL: [Class; 6] = [Class::Fix, Class::Upgrade, Class::Publish, Class::Rollback, Class::ApplyToRuns, Class::Delete];

    pub fn label(self) -> &'static str {
        match self {
            Class::Fix => "fix",
            Class::Upgrade => "upgrade",
            Class::Publish => "publish",
            Class::Rollback => "rollback",
            Class::ApplyToRuns => "apply to runs",
            Class::Delete => "delete",
        }
    }
}

/// One check. All of them are decided from data the harness has, never from what the model says about its own change.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Guard {
    /// The draft is valid and the patch is one the classifier accepts (the flow's own checks).
    Qa,
    /// Every changed step was tried with the recorded inputs without side effects.
    Trial,
    /// The diff is stored with the suggestion.
    DiffStored,
    /// At most `max_diff_nodes` steps and `max_diff_fields` settings changed.
    DiffSize,
    /// No step gained an external effect.
    NodeKinds,
    /// No change to credentials, trust flags, permissions, schedule, trigger, runner or the assist setting itself.
    Forbidden,
    /// No host that the published version did not already name.
    NewHosts,
    /// The published version is still the one the change was made from, and the person's draft holds nothing else.
    BaseCurrent,
    /// Publishes of this flow today are under its cap.
    FlowCap,
    /// Publishes of every flow today are under the account's cap.
    DayCap,
    /// Drafts of this flow today are under the cap.
    DraftCap,
    /// Spending today and for this fix is under the budgets.
    Budget,
    /// The version the agent published is failing in its first runs (the condition for an automatic rollback).
    OwnVersionFailing,
}

impl Guard {
    pub fn label(self) -> &'static str {
        match self {
            Guard::Qa => "QA checks",
            Guard::Trial => "trial without side effects",
            Guard::DiffStored => "diff stored",
            Guard::DiffSize => "size of the change",
            Guard::NodeKinds => "no new external effects",
            Guard::Forbidden => "nothing protected touched",
            Guard::NewHosts => "no new hosts",
            Guard::BaseCurrent => "based on the published version",
            Guard::FlowCap => "publishes per flow today",
            Guard::DayCap => "publishes per account today",
            Guard::DraftCap => "drafts per flow today",
            Guard::Budget => "budget",
            Guard::OwnVersionFailing => "its own version is failing",
        }
    }
}

/// What making a draft needs.
pub const DRAFT_GUARDS: [Guard; 2] = [Guard::DraftCap, Guard::Budget];
/// What publishing without asking needs: every one of these.
pub const PUBLISH_GUARDS: [Guard; 11] = [Guard::Qa, Guard::Trial, Guard::DiffStored, Guard::DiffSize, Guard::NodeKinds, Guard::Forbidden, Guard::NewHosts, Guard::BaseCurrent, Guard::FlowCap, Guard::DayCap, Guard::Budget];
/// What an automatic rollback needs.
pub const ROLLBACK_GUARDS: [Guard; 1] = [Guard::OwnVersionFailing];

/// Guard results. A guard that is not in the report did not run, and counts as failed.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct GuardReport {
    pub results: Vec<(Guard, Result<(), String>)>,
}

impl GuardReport {
    pub fn set(&mut self, g: Guard, r: Result<(), String>) {
        self.results.retain(|(x, _)| *x != g);
        self.results.push((g, r));
    }

    /// The reasons of the failed guards among `need` (a missing one says it was not checked).
    pub fn failures(&self, need: &[Guard]) -> Vec<String> {
        need.iter()
            .filter_map(|g| match self.results.iter().find(|(x, _)| x == g) {
                Some((_, Ok(()))) => None,
                Some((_, Err(why))) => Some(format!("{}: {why}", g.label())),
                None => Some(format!("{}: not checked", g.label())),
            })
            .collect()
    }

    pub fn passes(&self, need: &[Guard]) -> bool {
        self.failures(need).is_empty()
    }

    /// One line per guard for the screen and the audit (no values, only the reason).
    pub fn lines(&self) -> Vec<(String, bool, String)> {
        self.results.iter().map(|(g, r)| (g.label().to_string(), r.is_ok(), r.clone().err().unwrap_or_default())).collect()
    }
}

/// The part of the situation that changes what the setting means.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Situation {
    /// The person paused autopilot for this flow.
    pub paused: bool,
    /// A rollback happened less than `cooldown_hours` ago.
    pub cooling: bool,
    /// The account-wide switch is off.
    pub kill: bool,
}

/// The mode that counts right now: the kill switch turns everything off; a pause or a cool-down turns autopilot into copilot.
pub fn effective_mode(policy: &AssistPolicy, s: Situation) -> AssistMode {
    if s.kill {
        return AssistMode::Off;
    }
    match policy.mode {
        AssistMode::Autopilot if s.paused || s.cooling => AssistMode::Copilot,
        m => m,
    }
}

/// What the harness does with a request.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Decision {
    /// Go ahead without asking.
    Allow,
    /// Raise an approval request (the reasons go with it).
    Ask(String),
    /// Not done at all (nothing is created).
    Deny(String),
}

impl Decision {
    pub fn is_allow(&self) -> bool {
        matches!(self, Decision::Allow)
    }
    pub fn is_ask(&self) -> bool {
        matches!(self, Decision::Ask(_))
    }
    pub fn is_deny(&self) -> bool {
        matches!(self, Decision::Deny(_))
    }
}

fn joined(v: Vec<String>) -> String {
    v.join("; ")
}

/// The permission matrix. `mode` is the [`effective_mode`]. For `Publish`, `of` is the class the draft came from (`Fix` or
/// `Upgrade`; `None` counts as an upgrade, the stricter one).
pub fn decide(mode: AssistMode, policy: &AssistPolicy, class: Class, of: Option<Class>, report: &GuardReport) -> Decision {
    if mode == AssistMode::Off {
        return Decision::Deny("assist is off for this flow".into());
    }
    match class {
        Class::Delete => Decision::Deny("the agent never deletes a flow or a version".into()),
        Class::ApplyToRuns => Decision::Ask("changing running and scheduled tasks always asks".into()),
        Class::Fix | Class::Upgrade => {
            let f = report.failures(&DRAFT_GUARDS);
            if !f.is_empty() {
                return Decision::Deny(joined(f));
            }
            let allowed = if class == Class::Fix { policy.allow_fix } else { policy.allow_upgrade };
            if !allowed {
                return Decision::Ask(format!("{} is not switched on for this flow", class.label()));
            }
            Decision::Allow
        }
        Class::Publish => {
            if mode == AssistMode::Copilot {
                return Decision::Ask("in copilot the person approves every publish".into());
            }
            if of != Some(Class::Fix) {
                return Decision::Ask("an upgrade is published only after the person approves it".into());
            }
            if !policy.auto_publish {
                return Decision::Ask("automatic publishing is not switched on for this flow".into());
            }
            let f = report.failures(&PUBLISH_GUARDS);
            if f.is_empty() {
                Decision::Allow
            } else {
                Decision::Ask(joined(f))
            }
        }
        Class::Rollback => {
            if mode == AssistMode::Autopilot && report.passes(&ROLLBACK_GUARDS) {
                Decision::Allow
            } else if mode == AssistMode::Autopilot {
                Decision::Ask(joined(report.failures(&ROLLBACK_GUARDS)))
            } else {
                Decision::Ask("in copilot the person approves every rollback".into())
            }
        }
    }
}

// ------------------------------------------------------------------------------------------------ reading a diff

/// Kinds that never reach outside the run.
const PURE_KINDS: [&str; 9] = ["start", "end", "condition", "switch", "parallel", "join", "delay", "approval", "script"];

/// Whether a step does something to the world (the network beyond a read, a command, a browser, the camera, a message, a model, a
/// remote call). Reading over HTTP is not an effect; everything not known to be pure is.
pub fn effectful(kind: &str, config: &Value) -> bool {
    if PURE_KINDS.contains(&kind) {
        return false;
    }
    if kind == "http" {
        let m = config.get("method").and_then(Value::as_str).unwrap_or("").to_ascii_uppercase();
        return !matches!(m.as_str(), "" | "GET" | "HEAD" | "OPTIONS");
    }
    true
}

/// Kinds a trial must never run, whatever they say about themselves: the camera, a browser, and messages.
pub fn never_in_trial(kind: &str) -> bool {
    kind == "browser" || kind.starts_with("br_") || kind == "cam_photo" || kind == "notify" || kind == "remote" || kind == "agent"
}

const CREDENTIAL_WORDS: [&str; 10] = ["authorization", "cookie", "token", "secret", "password", "passwd", "apikey", "api-key", "api_key", "credential"];
const PROTECTED_KEYS: [&str; 12] = ["auth", "secret", "allow_domains", "allow_hosts", "tools", "net", "level", "image", "runner", "target", "profile", "trusted"];

fn credential_name(n: &str) -> bool {
    let l = n.to_ascii_lowercase();
    CREDENTIAL_WORDS.iter().any(|w| l.contains(w)) || l == "x-api-key" || l.ends_with("-key")
}

fn holds_secret_ref(v: &Value) -> bool {
    let t = v.to_string();
    t.contains("{{secret") || t.contains("vault:")
}

fn segments(path: &str) -> Vec<String> {
    path.split(['.', '[', ']']).filter(|s| !s.is_empty()).map(|s| s.to_ascii_lowercase()).collect()
}

/// Settings of the flow that no agent change may touch at all, in any mode: the agent's own permission, the camera, the browser's
/// trust, production and the healing policy. A suggestion that changes one is refused when it is made.
pub fn hard_forbidden(d: &FlowDiff) -> Vec<String> {
    let mut out = Vec::new();
    for c in &d.other {
        let p = c.path.as_str();
        if p.starts_with("defaults.assist") || p.starts_with("defaults.camera") || p.starts_with("defaults.browser") || p.starts_with("defaults.heal") || p == "production" {
            out.push(format!("`{p}` is the person's to change"));
        }
    }
    for n in &d.nodes_changed {
        for c in &n.changes {
            if c.path.starts_with("heal") {
                out.push(format!("step `{}`: `{}` is the person's to change", n.id, c.path));
            }
        }
    }
    out
}

/// Everything an automatic publish must not contain (a superset of [`hard_forbidden`]).
pub fn forbidden_changes(d: &FlowDiff) -> Vec<String> {
    let mut out = hard_forbidden(d);
    for c in &d.other {
        let p = c.path.as_str();
        let ok = p == "name" || p == "description" || p == "defaults.timeout_ms" || p.starts_with("defaults.retry") || p.starts_with("vars");
        if !ok && !out.iter().any(|o| o.contains(&format!("`{p}`"))) {
            out.push(format!("`{p}` (the flow's settings, schedule and inputs) is not changed by itself"));
        }
        if holds_secret_ref(&c.new) && !holds_secret_ref(&c.old) {
            out.push(format!("`{p}` would name a credential"));
        }
    }
    if !d.nodes_removed.is_empty() {
        out.push(format!("steps removed: {}", d.nodes_removed.join(", ")));
    }
    if !d.edges_added.is_empty() || !d.edges_removed.is_empty() {
        out.push("connections changed".into());
    }
    for n in &d.nodes_changed {
        for c in &n.changes {
            let seg = segments(&c.path);
            let first = seg.first().map(String::as_str).unwrap_or("");
            if matches!(first, "kind" | "runner" | "lib" | "track" | "on_error") {
                out.push(format!("step `{}`: `{}` is not changed by itself", n.id, c.path));
                continue;
            }
            if seg.iter().any(|s| PROTECTED_KEYS.contains(&s.as_str())) {
                out.push(format!("step `{}`: `{}` is a permission or credential setting", n.id, c.path));
                continue;
            }
            if seg.len() >= 3 && seg[0] == "config" && seg[1] == "headers" && credential_name(&seg[2]) {
                out.push(format!("step `{}`: header `{}` is a credential", n.id, seg[2]));
                continue;
            }
            if seg.len() >= 2 && seg[0] == "config" && seg[1] == "command" && c.path.contains("[0]") {
                out.push(format!("step `{}`: it would run another program", n.id));
                continue;
            }
            if holds_secret_ref(&c.new) && !holds_secret_ref(&c.old) {
                out.push(format!("step `{}`: `{}` would name a credential", n.id, c.path));
            }
        }
    }
    out
}

fn collect_hosts(v: &Value, out: &mut BTreeSet<String>) {
    match v {
        Value::String(s) => {
            let mut rest = s.as_str();
            while let Some(i) = rest.find("://") {
                let scheme_start = rest[..i].rfind(|c: char| !c.is_ascii_alphanumeric()).map_or(0, |p| p + 1);
                let scheme = rest[scheme_start..i].to_ascii_lowercase();
                let after = &rest[i + 3..];
                let host = after.split(['/', '?', '#', ' ', '"', '\'']).next().unwrap_or("");
                let host = host.rsplit_once('@').map_or(host, |x| x.1);
                if !host.is_empty() && !scheme.is_empty() {
                    out.insert(format!("{scheme}://{}", host.to_ascii_lowercase()));
                }
                rest = after;
            }
        }
        Value::Array(a) => a.iter().for_each(|x| collect_hosts(x, out)),
        Value::Object(o) => o.values().for_each(|x| collect_hosts(x, out)),
        _ => {}
    }
}

/// The scheme-and-host of every address the flow's steps and variables name.
pub fn hosts_of(f: &Flow) -> BTreeSet<String> {
    let mut out = BTreeSet::new();
    for n in &f.nodes {
        collect_hosts(&n.config, &mut out);
    }
    for v in f.vars.values() {
        collect_hosts(v, &mut out);
    }
    out
}

/// Hosts the draft names that the base did not.
pub fn new_hosts(base: &Flow, draft: &Flow) -> Vec<String> {
    let old = hosts_of(base);
    hosts_of(draft).into_iter().filter(|h| !old.contains(h)).collect()
}

/// Steps that gained an external effect: added with one, changed to another kind, or an HTTP step that went from reading to writing.
pub fn gained_effects(base: &Flow, draft: &Flow, d: &FlowDiff) -> Vec<String> {
    let mut out = Vec::new();
    for id in &d.nodes_added {
        if let Some(n) = draft.node(id) {
            if effectful(&n.kind, &n.config) {
                out.push(format!("new step `{id}` ({}) reaches outside", n.kind));
            }
        }
    }
    for n in &d.nodes_changed {
        if let (Some(o), Some(new)) = (base.node(&n.id), draft.node(&n.id)) {
            if o.kind != new.kind {
                out.push(format!("step `{}` changed kind", n.id));
            } else if !effectful(&o.kind, &o.config) && effectful(&new.kind, &new.config) {
                out.push(format!("step `{}` went from reading to writing", n.id));
            }
        }
    }
    out
}

/// Number of steps touched and of settings changed.
pub fn diff_size(d: &FlowDiff) -> (usize, usize) {
    let nodes = d.nodes_added.len() + d.nodes_removed.len() + d.nodes_changed.len();
    let fields = d.nodes_changed.iter().map(|n| n.changes.len()).sum::<usize>() + d.other.len() + d.edges_added.len() + d.edges_removed.len() + d.nodes_added.len() + d.nodes_removed.len();
    (nodes, fields)
}

/// How a trial of the changed steps went.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "state", content = "note", rename_all = "snake_case")]
pub enum TrialState {
    Passed(String),
    Failed(String),
    /// A changed step could not be tried without side effects (or nothing was tried).
    NotRun(String),
}

impl Default for TrialState {
    fn default() -> TrialState {
        TrialState::NotRun("not tried".into())
    }
}

/// What the guards need that is not in the two flows.
#[derive(Clone, Debug)]
pub struct Facts {
    /// The draft is valid and the patch was accepted by the classifier (reasons when not).
    pub qa: Result<(), String>,
    pub trial: TrialState,
    pub diff_stored: bool,
    /// The published version is still the base, and the person's own draft is the published version.
    pub base_current: Result<(), String>,
    pub published_today_flow: u32,
    pub published_today_all: u32,
    pub max_publish_per_day_all: u32,
    pub drafts_today: u32,
    pub cents_today: u64,
    pub cents_this_fix: u64,
    pub own_version_failing: bool,
}

impl Facts {
    /// Everything fine and nothing spent: the starting point of tests and of a first look.
    pub fn clean() -> Facts {
        Facts { qa: Ok(()), trial: TrialState::Passed("tried".into()), diff_stored: true, base_current: Ok(()), published_today_flow: 0, published_today_all: 0, max_publish_per_day_all: 3, drafts_today: 0, cents_today: 0, cents_this_fix: 0, own_version_failing: false }
    }
}

/// Evaluate every guard for a change from `base` (the published version) to `draft`.
pub fn evaluate(policy: &AssistPolicy, base: &Flow, draft: &Flow, d: &FlowDiff, f: &Facts) -> GuardReport {
    let mut r = GuardReport::default();
    r.set(Guard::Qa, f.qa.clone());
    r.set(
        Guard::Trial,
        match &f.trial {
            TrialState::Passed(_) => Ok(()),
            TrialState::Failed(w) => Err(format!("failed: {w}")),
            TrialState::NotRun(w) => Err(format!("not run: {w}")),
        },
    );
    r.set(Guard::DiffStored, if f.diff_stored && !d.is_empty() { Ok(()) } else { Err("no diff is stored".into()) });
    let (nodes, fields) = diff_size(d);
    r.set(
        Guard::DiffSize,
        if nodes as u32 > policy.max_diff_nodes {
            Err(format!("{nodes} steps changed (the limit is {})", policy.max_diff_nodes))
        } else if fields as u32 > policy.max_diff_fields {
            Err(format!("{fields} settings changed (the limit is {})", policy.max_diff_fields))
        } else {
            Ok(())
        },
    );
    let eff = gained_effects(base, draft, d);
    r.set(Guard::NodeKinds, if eff.is_empty() { Ok(()) } else { Err(eff.join("; ")) });
    let forb = forbidden_changes(d);
    r.set(Guard::Forbidden, if forb.is_empty() { Ok(()) } else { Err(forb.join("; ")) });
    let nh = new_hosts(base, draft);
    r.set(Guard::NewHosts, if nh.is_empty() { Ok(()) } else { Err(format!("new: {}", nh.join(", "))) });
    r.set(Guard::BaseCurrent, f.base_current.clone());
    r.set(
        Guard::FlowCap,
        if f.published_today_flow >= policy.max_publish_per_day { Err(format!("{} publishes today (the limit is {})", f.published_today_flow, policy.max_publish_per_day)) } else { Ok(()) },
    );
    r.set(
        Guard::DayCap,
        if f.published_today_all >= f.max_publish_per_day_all { Err(format!("{} publishes today across flows (the limit is {})", f.published_today_all, f.max_publish_per_day_all)) } else { Ok(()) },
    );
    r.set(
        Guard::DraftCap,
        if f.drafts_today >= policy.max_drafts_per_day { Err(format!("{} drafts today (the limit is {})", f.drafts_today, policy.max_drafts_per_day)) } else { Ok(()) },
    );
    r.set(
        Guard::Budget,
        if f.cents_today >= policy.max_cents_per_day {
            Err(format!("{} of {} cents spent today", f.cents_today, policy.max_cents_per_day))
        } else if f.cents_this_fix > policy.max_cents_per_fix {
            Err(format!("this fix cost {} cents (the limit is {})", f.cents_this_fix, policy.max_cents_per_fix))
        } else {
            Ok(())
        },
    );
    r.set(Guard::OwnVersionFailing, if f.own_version_failing { Ok(()) } else { Err("it is not failing".into()) });
    r
}

/// Result of watching a version the agent published.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Verdict {
    /// Fewer than `watch_runs` runs have finished and none failed.
    Pending,
    /// `watch_runs` runs finished without a failure: the watch ends.
    Healthy,
    /// One of the first runs failed.
    Failing,
}

/// `outcomes` are the finished runs of that version in order (`true`: succeeded). A cancelled run is left out by the caller.
pub fn watch_verdict(policy: &AssistPolicy, outcomes: &[bool]) -> Verdict {
    let n = policy.watch_runs.max(1) as usize;
    if outcomes.iter().take(n).any(|ok| !ok) {
        Verdict::Failing
    } else if outcomes.len() >= n {
        Verdict::Healthy
    } else {
        Verdict::Pending
    }
}

/// An approver recorded on a version that nobody approved by hand: the person's own setting allowed it.
pub const POLICY_APPROVER: &str = "policy:autopilot";
/// Who the agent is in the history.
pub const ASSIST_AGENT: &str = "agent:assist";
/// A fix of a fix: the agent that published `MAX_SELF_FIX` versions in a row stops and asks.
pub const MAX_SELF_FIX: u32 = 2;
