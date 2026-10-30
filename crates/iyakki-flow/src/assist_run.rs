//! FLOWS assist at work: on a failed run (or on demand) the configured agent proposes a fix, the harness classifies and tests it, and
//! the result is a stored suggestion (a proposed version, its diff, reason, QA and trial result). In copilot the person approves it
//! in the Flows screen; in autopilot, for a fix only and only when the person switched `auto_publish` on and every guard passes, it is
//! published by itself, announced, and watched: a failure in its first runs rolls it back and pauses autopilot.
//!
//! Everything the world does goes through the [`Host`] trait (the model, the audit log, the bell), so this runs the same with the
//! fake host in tests. The model is asked through `Host::propose_patch`; whatever it answers is only a request: [`heal::classify`],
//! [`assist::decide`] and the guards decide, never the answer's own claim.

use serde_json::Value;

use crate::assist::*;
use crate::assist_store::*;
use crate::heal;
use crate::host::{Host, HealRequest, Notice, Redactor};
use crate::kinds::Registry;
use crate::model::*;
use crate::store::{compatible, diff, FlowStore, PublishReq};

/// Tries the changed steps of a proposed flow with the failed run's recorded input, without side effects.
pub trait Trial: Send + Sync {
    fn trial(&self, flow: &Flow, run: &Run, changed: &[String]) -> TrialState;
}

/// The real trial: only steps that are read-only (and are not a camera, a browser, a message, a model or a remote call) are run, once,
/// through the engine; anything else is `NotRun`, which stops an automatic publish.
pub struct EngineTrial<'a> {
    pub engine: &'a crate::Engine,
    pub reg: &'a Registry,
}

impl Trial for EngineTrial<'_> {
    fn trial(&self, flow: &Flow, run: &Run, changed: &[String]) -> TrialState {
        if changed.is_empty() {
            return TrialState::NotRun("no step changed, so there is nothing to try".into());
        }
        for id in changed {
            let Some(node) = flow.node(id) else { return TrialState::NotRun(format!("step `{id}` is gone")) };
            if never_in_trial(&node.kind) {
                return TrialState::NotRun(format!("step `{id}` ({}) is never run in a trial", node.kind));
            }
            let Some(kind) = self.reg.get(&node.kind) else { return TrialState::NotRun(format!("`{}` is not a step kind", node.kind)) };
            if !kind.idempotent(node) {
                return TrialState::NotRun(format!("step `{id}` ({}) has effects and is not run in a trial", node.kind));
            }
        }
        for id in changed {
            let input = run.nodes.get(id).map(|n| n.input.clone()).unwrap_or(Value::Null);
            match self.engine.test_node(flow, id, run.inputs.clone(), input) {
                Ok(t) if t.ok => {}
                Ok(t) => return TrialState::Failed(format!("step `{id}`: {}", t.error)),
                Err(e) => return TrialState::Failed(format!("step `{id}`: {e}")),
            }
        }
        TrialState::Passed(format!("{} step(s) tried read-only with the recorded input", changed.len()))
    }
}

/// What a request for a suggestion came to.
#[derive(Clone, Debug, PartialEq)]
pub enum Outcome {
    /// A suggestion was made; `published` is the version when autopilot published it by itself, `asks` why it waits for the person.
    Suggested { id: String, published: Option<u32>, asks: String },
    /// The agent looked and found no fix (its diagnosis).
    NoFix(String),
    /// Nothing was done, and why (off, paused, budget, already answered ...).
    Skipped(String),
}

pub struct Assistant<'a> {
    pub store: &'a FlowStore,
    pub book: &'a AssistStore,
    pub host: &'a dyn Host,
    pub reg: &'a Registry,
    pub trial: &'a dyn Trial,
}

fn clip(s: &str, n: usize) -> String {
    let t: String = s.lines().find(|l| !l.trim().is_empty()).unwrap_or("").trim().chars().take(n).collect();
    t
}

fn strings_of(v: &Value, out: &mut Vec<String>) {
    match v {
        Value::String(s) if s.len() >= 4 => out.push(s.clone()),
        Value::Array(a) => a.iter().for_each(|x| strings_of(x, out)),
        Value::Object(o) => o.values().for_each(|x| strings_of(x, out)),
        _ => {}
    }
}

/// The redactor for a run: every input value, so none reaches the model, a stored reason or the audit log.
fn redactor_for(run: &Run) -> Redactor {
    let r = Redactor::new();
    let mut v = Vec::new();
    strings_of(&run.inputs, &mut v);
    for s in v {
        r.add(&s);
    }
    r
}

impl Assistant<'_> {
    /// The person's setting for the flow: the one in the published version, which is the one that runs. Turning assist on is
    /// therefore itself a published (and reviewed) change.
    pub fn policy_of(&self, flow_id: &str) -> Option<AssistPolicy> {
        self.store.published(flow_id).map(|f| f.defaults.assist)
    }

    pub fn situation(&self, flow_id: &str) -> Situation {
        Situation { paused: self.book.paused(flow_id), cooling: self.book.cooling(flow_id), kill: !self.book.global().enabled }
    }

    pub fn mode_of(&self, flow_id: &str) -> AssistMode {
        self.policy_of(flow_id).map(|p| effective_mode(&p, self.situation(flow_id))).unwrap_or_default()
    }

    fn notice(&self, flow_id: &str, run_id: &str, title: &str, body: &str, level: &str, needs_you: bool) {
        let _ = self.host.notify(&Notice { title: clip(title, 120), body: body.chars().take(500).collect(), level: level.into(), flow_id: flow_id.into(), run_id: run_id.into(), needs_you });
    }

    fn base_current(&self, flow_id: &str, base_version: u32) -> Result<(), String> {
        let Some(p) = self.store.published(flow_id) else { return Err("the flow has no published version".into()) };
        if p.meta.version != base_version {
            return Err(format!("version {} was published since (the change was made from {base_version})", p.meta.version));
        }
        match self.store.get_draft(flow_id) {
            Some(d) if diff(&p, &d).is_empty() => Ok(()),
            Some(_) => Err("your draft holds changes that are not published yet".into()),
            None => Err("the flow has no draft".into()),
        }
    }

    /// How many versions in a row the agent published (a fix of its own fix is bounded).
    pub fn agent_chain(&self, flow_id: &str) -> u32 {
        self.store.versions(flow_id).iter().rev().take_while(|v| v.author.starts_with("agent:")).count() as u32
    }

    fn facts(&self, flow_id: &str, base_version: u32, qa: Result<(), String>, trial: TrialState, cents: u64, stored: bool) -> Facts {
        Facts {
            qa,
            trial,
            diff_stored: stored,
            base_current: self.base_current(flow_id, base_version),
            published_today_flow: self.book.publishes_today(flow_id),
            published_today_all: self.book.publishes_today_all(),
            max_publish_per_day_all: self.book.global().max_publish_per_day,
            drafts_today: self.book.drafts_today(flow_id),
            cents_today: self.book.cents_today(flow_id),
            cents_this_fix: cents,
            own_version_failing: false,
        }
    }

    // ------------------------------------------------------------------------------------------ a fix after a failed run

    /// Ask the agent to fix the failed step of `run`, test the answer and keep it as a suggestion (publishing it itself only when
    /// autopilot, `auto_publish` and every guard allow). `on_demand`: the person asked (the same limits apply).
    pub fn suggest_for_run(&self, run: &Run, on_demand: bool) -> Result<Outcome, String> {
        let flow_id = run.flow_id.as_str();
        if run.status != RunStatus::Failed {
            return Ok(Outcome::Skipped("the run did not fail".into()));
        }
        let Some(published) = self.store.published(flow_id) else { return Ok(Outcome::Skipped("the flow has no published version".into())) };
        let policy = published.defaults.assist.clone();
        let mode = effective_mode(&policy, self.situation(flow_id));
        if mode == AssistMode::Off {
            return Ok(Outcome::Skipped(if policy.mode == AssistMode::Off { "assist is off for this flow".into() } else { "assist is switched off for this account".into() }));
        }
        if run.version() != published.meta.version {
            return Ok(Outcome::Skipped(format!("the run was of version {}, the published one is {}", run.version(), published.meta.version)));
        }
        if let Some(s) = self.book.for_run(flow_id, &run.id) {
            return Ok(Outcome::Suggested { id: s.id, published: (s.published_version > 0).then_some(s.published_version), asks: s.asks });
        }
        if published.production {
            return Ok(Outcome::Skipped("a production flow never gets an automatic fix".into()));
        }
        if self.agent_chain(flow_id) >= MAX_SELF_FIX && run.flow.meta.author.starts_with("agent:") {
            return Ok(Outcome::Skipped(format!("the agent already published {MAX_SELF_FIX} versions in a row; it stops and waits for you")));
        }
        // the failing step
        let node_id = run.flow.nodes.iter().find(|n| run.nodes.get(&n.id).is_some_and(|x| matches!(x.state, NodeState::Failed | NodeState::Interrupted))).map(|n| n.id.clone());
        let Some(node_id) = node_id else { return Ok(Outcome::Skipped("the run failed without a failing step".into())) };
        // may a draft be made at all
        let mut pre = GuardReport::default();
        pre.set(Guard::DraftCap, if self.book.drafts_today(flow_id) >= policy.max_drafts_per_day { Err(format!("{} drafts today (the limit is {})", self.book.drafts_today(flow_id), policy.max_drafts_per_day)) } else { Ok(()) });
        pre.set(Guard::Budget, if self.book.cents_today(flow_id) >= policy.max_cents_per_day { Err(format!("{} of {} cents spent today", self.book.cents_today(flow_id), policy.max_cents_per_day)) } else { Ok(()) });
        match decide(mode, &policy, Class::Fix, None, &pre) {
            Decision::Deny(why) => return Ok(Outcome::Skipped(why)),
            Decision::Ask(why) => return Ok(Outcome::Skipped(format!("{why}; ask for it by hand to go on"))),
            Decision::Allow => {}
        }
        let red = redactor_for(run);
        let clis = {
            let (cli, _) = heal::required_cli(run.flow.node(&node_id).unwrap_or(&Node::new("x", "x", Value::Null)));
            if cli.is_empty() { vec![] } else { vec![(cli.clone(), self.host.cli_info(&cli).map(|i| i.version).unwrap_or_default())] }
        };
        let ctx = red.text(&heal::failure_context(run, &node_id, &clis));
        let req = HealRequest { run_id: run.id.clone(), flow_id: flow_id.to_string(), node_id: node_id.clone(), context: format!("{}\n\n{ctx}", heal::HEAL_INSTRUCTIONS), agent: String::new(), max_cents: policy.max_cents_per_fix };
        self.book.record(flow_id, "draft", 0, &format!("{} asked about run {}", if on_demand { "person" } else { "failure" }, run.id))?;
        let proposal = self.host.propose_patch(&req, &crate::Cancel::new());
        let proposal = match proposal {
            Ok(p) => p,
            Err(e) => {
                let e = red.text(&e);
                self.host.audit("flow.assist.failed", &format!("{flow_id} run {}: no proposal ({})", run.id, clip(&e, 160)));
                return Ok(Outcome::NoFix(format!("the agent could not look at this: {}", clip(&e, 160))));
            }
        };
        if proposal.cents > 0 {
            self.book.record(flow_id, "spend", proposal.cents, "model")?;
        }
        let diagnosis = clip(&red.text(&proposal.diagnosis), 400);
        let Some(patch) = proposal.patch else {
            self.host.audit("flow.assist.nofix", &format!("{flow_id} run {}: {}", run.id, clip(&diagnosis, 160)));
            return Ok(Outcome::NoFix(if diagnosis.is_empty() { "the agent found nothing to change".into() } else { diagnosis }));
        };
        if patch.node != node_id {
            return Ok(Outcome::NoFix("the proposal changes another step than the failing one".into()));
        }
        let cl = heal::classify(&patch, &run.flow, &node_id, &heal::ClassifyEnv { in_catalog: &|c| self.host.cli_known(c) });
        if cl.class == heal::Class::Refused {
            self.host.audit("flow.assist.refused", &format!("{flow_id} run {}: {}", run.id, clip(&cl.reasons.join("; "), 200)));
            return Ok(Outcome::NoFix(format!("the proposal was refused: {}", clip(&cl.reasons.join("; "), 200))));
        }
        // the change is made on the published version, never on the person's draft
        let mut proposed = published.clone();
        let summary = heal::apply(&mut proposed, &patch, ASSIST_AGENT, cl.is_simple(), self.book.now()).map_err(|e| format!("the proposal cannot be applied: {e}"))?;
        let qa = if cl.is_simple() { Ok(()) } else { Err(format!("not a simple fix: {}", clip(&cl.reasons.join("; "), 200))) };
        self.finish_suggestion(Class::Fix, &published, proposed, Some(run), &node_id, &diagnosis, &red.text(&summary), qa, proposal.cents, mode, &policy)
    }

    /// The common end of every suggestion: checks, trial, guards, storing, and (autopilot, fix) possibly publishing.
    #[allow(clippy::too_many_arguments)]
    fn finish_suggestion(&self, class: Class, base: &Flow, proposed: Flow, run: Option<&Run>, node: &str, reason: &str, summary: &str, classified: Result<(), String>, cents: u64, mode: AssistMode, policy: &AssistPolicy) -> Result<Outcome, String> {
        let flow_id = base.id.as_str();
        let d = diff(base, &proposed);
        if d.is_empty() {
            return Ok(Outcome::NoFix("the proposal changes nothing".into()));
        }
        let hard = hard_forbidden(&d);
        if !hard.is_empty() {
            self.host.audit("flow.assist.refused", &format!("{flow_id}: {}", clip(&hard.join("; "), 200)));
            return Ok(Outcome::NoFix(format!("the proposal touches settings only you may change: {}", clip(&hard.join("; "), 200))));
        }
        // QA: the flow is valid as the engine will see it, and the patch is of a kind the classifier accepts
        let issues = crate::validate::validate_flow(&self.store.resolve_for_check(&proposed), self.reg);
        let errs: Vec<String> = crate::validate::errors(&issues).iter().map(|i| format!("{}{}", i.node.as_ref().map(|n| format!("{n}: ")).unwrap_or_default(), i.msg)).collect();
        let mut qa_notes = vec![];
        let qa = if !errs.is_empty() {
            qa_notes.push(format!("the proposed flow has problems: {}", clip(&errs.join("; "), 200)));
            Err(qa_notes[0].clone())
        } else {
            qa_notes.push("the proposed flow is valid".into());
            match &classified {
                Ok(()) => qa_notes.push("the change is one of the simple, safe kinds".into()),
                Err(e) => qa_notes.push(e.clone()),
            }
            classified
        };
        let changed: Vec<String> = d.nodes_changed.iter().map(|n| n.id.clone()).collect();
        let trial = match run {
            Some(r) => self.trial.trial(&proposed, r, &changed),
            None => TrialState::NotRun("an upgrade is tried by the person before it is published".into()),
        };
        let facts = self.facts(flow_id, base.meta.version, qa, trial.clone(), cents, true);
        let mut report = evaluate(policy, base, &proposed, &d, &facts);
        let decision = decide(mode, policy, Class::Publish, Some(class), &report);
        // the rollback condition has nothing to say about a publish
        report.results.retain(|(g, _)| *g != Guard::OwnVersionFailing);
        let (asks, auto) = match &decision {
            Decision::Allow => (String::new(), true),
            Decision::Ask(w) => (w.clone(), false),
            Decision::Deny(w) => (w.clone(), false),
        };
        let sugg = Suggestion {
            schema: 1,
            extra: Default::default(),
            id: String::new(),
            flow_id: flow_id.to_string(),
            class,
            status: Status::Pending,
            created_ms: self.book.now(),
            decided_ms: 0,
            base_version: base.meta.version,
            run_id: run.map(|r| r.id.clone()).unwrap_or_default(),
            node: node.to_string(),
            reason: reason.to_string(),
            summary: summary.to_string(),
            diff: d.text().lines().take(60).map(str::to_string).collect(),
            hash: proposed.content_hash(),
            proposed,
            qa: qa_notes,
            trial: trial.clone(),
            guards: report.lines(),
            asks: asks.clone(),
            by: ASSIST_AGENT.into(),
            cents,
            published_version: 0,
        };
        let s = self.book.add(sugg)?;
        self.host.audit("flow.assist.suggested", &format!("{flow_id} {} {} run {}: {}", s.id, class.label(), if s.run_id.is_empty() { "-" } else { &s.run_id }, clip(summary, 160)));
        if auto {
            return match self.publish_by_agent(flow_id, &s.id) {
                Ok(v) => Ok(Outcome::Suggested { id: s.id, published: Some(v), asks: String::new() }),
                Err(e) => {
                    // a guard that failed at the last moment is a question like any other
                    let _ = self.book.update(flow_id, &s.id, |x| x.asks = e.clone());
                    self.ask_person(&s, &e);
                    Ok(Outcome::Suggested { id: s.id, published: None, asks: e })
                }
            };
        }
        self.ask_person(&s, &asks);
        Ok(Outcome::Suggested { id: s.id, published: None, asks })
    }

    fn ask_person(&self, s: &Suggestion, why: &str) {
        let title = format!("A fix is ready to look at: {}", clip(&s.summary, 80));
        let body = format!("{} {}", clip(&s.reason, 200), if why.is_empty() { String::new() } else { format!("It waits for you because: {}", clip(why, 200)) });
        self.notice(&s.flow_id, &s.run_id, &title, &body, "info", true);
    }

    // ------------------------------------------------------------------------------------------ publishing by the agent

    /// Publish a suggestion by itself. Every guard is evaluated again right now; the person's setting must still allow it.
    pub fn publish_by_agent(&self, flow_id: &str, sid: &str) -> Result<u32, String> {
        let s = self.book.get(flow_id, sid).ok_or_else(|| format!("there is no suggestion `{sid}`"))?;
        if s.status != Status::Pending {
            return Err("the suggestion is no longer waiting".into());
        }
        let base = self.store.published(flow_id).ok_or("the flow has no published version")?;
        let policy = base.defaults.assist.clone();
        let mode = effective_mode(&policy, self.situation(flow_id));
        let d = diff(&base, &s.proposed);
        let qa = if s.qa.iter().any(|l| l.contains("problems") || l.starts_with("not a simple")) { Err("the QA checks did not pass".to_string()) } else { Ok(()) };
        let facts = self.facts(flow_id, s.base_version, qa, s.trial.clone(), s.cents, !s.diff.is_empty());
        let report = evaluate(&policy, &base, &s.proposed, &d, &facts);
        match decide(mode, &policy, Class::Publish, Some(s.class), &report) {
            Decision::Allow => {}
            Decision::Ask(w) | Decision::Deny(w) => return Err(w),
        }
        if s.proposed.content_hash() != s.hash {
            return Err("the stored change was altered".into());
        }
        self.store.save_draft(&s.proposed, None)?;
        let changelog = format!("autopilot fix: {}", clip(&s.summary, 160));
        let v = match self.store.publish(flow_id, &PublishReq { by: ASSIST_AGENT, label: "", changelog: &changelog, approved_by: Some(POLICY_APPROVER) }) {
            Ok(v) => v,
            Err(e) => {
                // put the person's draft back as it was
                let _ = self.store.save_draft(&base, None);
                return Err(e);
            }
        };
        self.book.record_publish(flow_id, &format!("v{v} {}", s.id))?;
        self.book.set_watch(flow_id, Some(Watch { version: v, previous: base.meta.version, since_ms: self.book.now(), suggestion: s.id.clone() }))?;
        let _ = self.book.update(flow_id, &s.id, |x| {
            x.status = Status::AutoPublished;
            x.published_version = v;
            x.decided_ms = x.created_ms;
            x.asks.clear();
        });
        self.host.audit("flow.assist.published", &format!("{flow_id} v{v} from v{} {} by autopilot: {}", base.meta.version, s.id, clip(&s.summary, 160)));
        self.notice(flow_id, &s.run_id, &format!("Autopilot published version {v} of {}", base.name), &format!("{} It is watched for its first {} runs and rolled back if one fails. You can roll it back or pause autopilot in FLOWS.", clip(&s.summary, 200), policy.watch_runs), "info", false);
        Ok(v)
    }

    // ------------------------------------------------------------------------------------------ the person's side

    /// The person pressed Approve: the change goes into the draft (which must hold nothing else) and the ordinary publish question
    /// follows in the screen. The suggestion becomes `Loaded`.
    pub fn adopt(&self, flow_id: &str, sid: &str) -> Result<Suggestion, String> {
        let s = self.book.get(flow_id, sid).ok_or_else(|| format!("there is no suggestion `{sid}`"))?;
        if s.status != Status::Pending {
            return Err(format!("this suggestion is {}", s.status.label()));
        }
        if let Err(e) = self.base_current(flow_id, s.base_version) {
            if self.store.published(flow_id).is_some_and(|p| p.meta.version != s.base_version) {
                let _ = self.book.set_status(flow_id, sid, Status::Superseded);
            }
            return Err(e);
        }
        if s.proposed.content_hash() != s.hash {
            return Err("the stored change was altered; it is not used".into());
        }
        self.store.save_draft(&s.proposed, None)?;
        self.host.audit("flow.assist.loaded", &format!("{flow_id} {sid} put in the draft for publishing"));
        self.book.set_status(flow_id, sid, Status::Loaded)
    }

    /// The person said no. A change already put in the draft is taken back out when nothing else was edited in it.
    pub fn reject(&self, flow_id: &str, sid: &str, by: &str) -> Result<Suggestion, String> {
        let s = self.book.get(flow_id, sid).ok_or_else(|| format!("there is no suggestion `{sid}`"))?;
        if !s.status.open() {
            return Err(format!("this suggestion is {}", s.status.label()));
        }
        if s.status == Status::Loaded {
            if let (Some(d), Some(p)) = (self.store.get_draft(flow_id), self.store.published(flow_id)) {
                if diff(&s.proposed, &d).is_empty() {
                    let _ = self.store.save_draft(&p, None);
                }
            }
        }
        self.host.audit("flow.assist.rejected", &format!("{flow_id} {sid} by {by}"));
        self.book.set_status(flow_id, sid, Status::Rejected)
    }

    /// A version was published (by the person, from the screen or the assistant). A loaded suggestion that it contains is marked
    /// approved; open suggestions made from an older version are out of date.
    pub fn after_publish(&self, flow_id: &str, version: u32, by: &str) {
        if let Some(p) = self.store.get_version(flow_id, version) {
            for s in self.book.open_list(flow_id) {
                if s.status == Status::Loaded && diff(&s.proposed, &p).is_empty() {
                    let _ = self.book.update(flow_id, &s.id, |x| {
                        x.status = Status::Approved;
                        x.published_version = version;
                    });
                    self.host.audit("flow.assist.approved", &format!("{flow_id} {} v{version} by {by}", s.id));
                }
            }
        }
        self.book.supersede_older(flow_id, version);
    }

    // ------------------------------------------------------------------------------------------ the watcher

    /// Look at the runs of every version the agent published: a failure among the first runs rolls it back (autopilot only), starts the
    /// cool-down and asks the person; enough good runs end the watch. Returns what happened, one line each.
    pub fn watch_tick(&self, runs: &[RunSummary]) -> Vec<String> {
        let mut out = vec![];
        let ids = self.store.ids();
        for (flow_id, w) in self.book.flows_watched(&ids) {
            let Some(policy) = self.policy_of(&flow_id) else { continue };
            let mut mine: Vec<&RunSummary> = runs.iter().filter(|r| r.flow_id == flow_id && r.version == w.version && r.parent.is_none() && matches!(r.status, RunStatus::Succeeded | RunStatus::Failed)).collect();
            mine.sort_by_key(|r| (r.created_ms, r.id.clone()));
            let outcomes: Vec<bool> = mine.iter().map(|r| r.status == RunStatus::Succeeded).collect();
            match watch_verdict(&policy, &outcomes) {
                Verdict::Pending => {}
                Verdict::Healthy => {
                    let _ = self.book.set_watch(&flow_id, None);
                    let _ = self.book.record(&flow_id, "note", 0, &format!("v{} passed its watch", w.version));
                    self.host.audit("flow.assist.watch.ok", &format!("{flow_id} v{} ran well {} times", w.version, outcomes.len()));
                    out.push(format!("{flow_id}: version {} is healthy", w.version));
                }
                Verdict::Failing => {
                    let mode = effective_mode(&policy, self.situation(&flow_id));
                    let mut r = GuardReport::default();
                    r.set(Guard::OwnVersionFailing, Ok(()));
                    let _ = self.book.set_watch(&flow_id, None);
                    match decide(mode, &policy, Class::Rollback, None, &r) {
                        Decision::Allow => match self.store.rollback(&flow_id, w.previous, ASSIST_AGENT) {
                            Ok(nv) => {
                                let _ = self.book.start_cooldown(&flow_id, policy.cooldown_hours);
                                let _ = self.book.record(&flow_id, "rollback", 0, &format!("v{} -> v{nv}", w.version));
                                let _ = self.book.update(&flow_id, &w.suggestion, |x| x.status = Status::RolledBack);
                                self.host.audit("flow.assist.rollback", &format!("{flow_id} v{} failed its first runs; v{} published again as v{nv}; autopilot pauses {} h", w.version, w.previous, policy.cooldown_hours));
                                self.notice(&flow_id, "", &format!("Autopilot rolled back {flow_id}"), &format!("Version {} failed one of its first runs, so version {} is published again (as {nv}). Autopilot asks you before it publishes anything for the next {} hours.", w.version, w.previous, policy.cooldown_hours), "warn", true);
                                out.push(format!("{flow_id}: rolled back v{} to v{} as v{nv}", w.version, w.previous));
                            }
                            Err(e) => {
                                self.host.audit("flow.assist.rollback.failed", &format!("{flow_id}: {}", clip(&e, 160)));
                                self.notice(&flow_id, "", &format!("{flow_id} needs a rollback"), &format!("Version {} is failing and the automatic rollback did not work: {}", w.version, clip(&e, 160)), "error", true);
                            }
                        },
                        Decision::Ask(_) | Decision::Deny(_) => {
                            self.notice(&flow_id, "", &format!("{flow_id}: a published fix is failing"), &format!("Version {} (published by the agent) failed one of its first runs. Roll it back from the history if you want the earlier behaviour.", w.version), "warn", true);
                            out.push(format!("{flow_id}: v{} failing, asked the person", w.version));
                        }
                    }
                }
            }
        }
        out
    }

    // ------------------------------------------------------------------------------------------ the periodic pass

    /// One pass: the watcher first, then each flow whose newest finished run failed and has not been answered yet. `get_run` reads a
    /// run's full record. At most one new suggestion per flow per pass, and none while one is already waiting.
    pub fn tick(&self, runs: &[RunSummary], get_run: &dyn Fn(&str) -> Option<Run>) -> Vec<String> {
        let mut out = self.watch_tick(runs);
        for id in self.store.ids() {
            if self.mode_of(&id) == AssistMode::Off {
                continue;
            }
            let newest = runs.iter().filter(|r| r.flow_id == id && r.parent.is_none() && r.status.is_terminal() && r.status != RunStatus::Cancelled).max_by_key(|r| (r.created_ms, r.id.clone()));
            let Some(newest) = newest else { continue };
            if newest.status != RunStatus::Failed || self.book.book(&id).seen_runs.contains(&newest.id) {
                continue;
            }
            let rid = newest.id.clone();
            let _ = self.book.with_book(&id, |b| b.seen_runs.push(rid.clone()));
            if !self.book.open_list(&id).is_empty() {
                continue;
            }
            let Some(run) = get_run(&rid) else { continue };
            match self.suggest_for_run(&run, false) {
                Ok(Outcome::Suggested { id: sid, published, .. }) => out.push(format!("{id}: suggestion {sid}{}", published.map(|v| format!(" published as v{v}")).unwrap_or_default())),
                Ok(Outcome::NoFix(w)) | Ok(Outcome::Skipped(w)) => out.push(format!("{id}: {}", clip(&w, 120))),
                Err(e) => out.push(format!("{id}: {}", clip(&e, 120))),
            }
        }
        out
    }

    // ------------------------------------------------------------------------------------------ upgrades

    /// Look for newer, compatible library versions of the steps of the published flow and keep each as a suggestion. Detection and
    /// suggestion only: an upgrade is never published by itself in this release. Returns lines about what was found.
    pub fn suggest_upgrades(&self, flow_id: &str) -> Result<Vec<String>, String> {
        let Some(base) = self.store.published(flow_id) else { return Ok(vec!["the flow has no published version".into()]) };
        let policy = base.defaults.assist.clone();
        let mode = effective_mode(&policy, self.situation(flow_id));
        if mode == AssistMode::Off {
            return Ok(vec!["assist is off for this flow".into()]);
        }
        let mut out = vec![];
        for n in &base.nodes {
            let Some(l) = &n.lib else { continue };
            if n.track != Track::Pinned || l.version == 0 {
                continue;
            }
            let Some((latest, new)) = self.store.lib_latest(&l.id) else { continue };
            if latest <= l.version {
                continue;
            }
            let Some(cur) = self.store.lib_get(&l.id, l.version) else { continue };
            if !compatible(&cur, &new, self.reg) {
                out.push(format!("step `{}`: library node `{}` v{latest} is newer but changes its ports or outputs; look at it by hand", n.id, l.id));
                continue;
            }
            if self.book.open_list(flow_id).iter().any(|s| s.class == Class::Upgrade && s.node == n.id && s.summary.contains(&format!("v{latest}"))) {
                continue;
            }
            let mut pre = GuardReport::default();
            pre.set(Guard::DraftCap, if self.book.drafts_today(flow_id) >= policy.max_drafts_per_day { Err("drafts per day used".into()) } else { Ok(()) });
            pre.set(Guard::Budget, if self.book.cents_today(flow_id) >= policy.max_cents_per_day { Err("budget used".into()) } else { Ok(()) });
            match decide(mode, &policy, Class::Upgrade, None, &pre) {
                Decision::Allow => {}
                Decision::Ask(w) | Decision::Deny(w) => {
                    out.push(format!("step `{}`: v{latest} of `{}` is available ({w})", n.id, l.id));
                    continue;
                }
            }
            self.book.record(flow_id, "draft", 0, &format!("upgrade of {}", n.id))?;
            let mut proposed = base.clone();
            let summary = format!("use library node `{}` v{latest} (was v{}) in step `{}`", l.id, l.version, n.id);
            if let Some(m) = proposed.node_mut(&n.id) {
                m.lib = Some(LibRef { id: l.id.clone(), version: latest });
                let ch = Change { who: ASSIST_AGENT.into(), node: n.id.clone(), summary: summary.clone(), at_ms: self.book.now(), simple: false };
                m.modified_by = Some(ch.clone());
                proposed.meta.modified_by.push(ch);
            }
            match self.finish_suggestion(Class::Upgrade, &base, proposed, None, &n.id, &format!("A newer version of the shared step `{}` is available.", l.id), &summary, Ok(()), 0, mode, &policy)? {
                Outcome::Suggested { id, .. } => out.push(format!("step `{}`: suggestion {id}: {summary}", n.id)),
                Outcome::NoFix(w) | Outcome::Skipped(w) => out.push(format!("step `{}`: {w}", n.id)),
            }
        }
        Ok(out)
    }
}
