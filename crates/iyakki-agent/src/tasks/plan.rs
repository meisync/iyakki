//! Plan fan-out, the memory-aware cap and the lead summary of parallel tasks (see `docs/PARALLEL-TASKS.md`).
//! A child module of `tasks`: it uses the manager's private state, and holds no model and no UI.

use super::*;

/// What one task is expected to need, in MB, until the person sets another value (`task_memory_mb` in `.iyakki/tasks/config.json`).
pub const DEFAULT_TASK_MB: u64 = 1500;
/// Memory kept free for the system and the person's own programs, in MB.
pub const RESERVE_MB: u64 = 2048;

/// How many tasks may run at once: the person's maximum (and [`HARD_CAP`]), lowered when the free memory cannot hold more.
/// `running` tasks already use their memory, so `avail` (free now) pays for the extra ones only. One task may always run.
/// Unknown free memory: at most 2.
pub fn memory_cap(setting: usize, running: usize, avail: Option<u64>, task_mb: u64) -> usize {
    let max = setting.clamp(1, HARD_CAP);
    let Some(a) = avail else { return max.min(2) };
    let usable = a.saturating_sub(RESERVE_MB * 1024 * 1024);
    let extra = (usable / (task_mb.max(1) * 1024 * 1024)) as usize;
    (running + extra).clamp(1, max)
}

pub(super) fn gb(b: Option<u64>) -> String {
    b.map_or("unknown memory".into(), |b| format!("{:.1} GB", b as f64 / (1u64 << 30) as f64))
}

/// `MemAvailable` of this machine in bytes.
pub fn meminfo_available() -> Option<u64> {
    crate::sysinfo::mem_available_bytes()
}

// ------------------------------------------------------------------------------------------------ the split

/// One step of a plan the lead wants to fan out.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct PlanStep {
    pub title: String,
    pub goal: String,
    /// The files or folders (`src/a.rs`, `docs/`) the step may change: its scope.
    pub files: Vec<String>,
    /// Steps (0-based) whose result this step needs: it starts after they are merged.
    pub depends_on: Vec<usize>,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Split {
    /// Step numbers (0-based) per wave: a wave starts when the one before it is merged.
    pub waves: Vec<Vec<usize>>,
    /// Why a step is not in the first wave.
    pub notes: Vec<String>,
}

fn norm(p: &str) -> String {
    p.trim().trim_start_matches("./").trim_end_matches('/').to_string()
}

/// Two scopes meet when they are the same path or one is a folder that holds the other.
fn meets(a: &str, b: &str) -> bool {
    let (a, b) = (norm(a), norm(b));
    !a.is_empty() && !b.is_empty() && (a == b || a.starts_with(&format!("{b}/")) || b.starts_with(&format!("{a}/")))
}

fn scopes_meet(a: &[String], b: &[String]) -> Option<String> {
    a.iter().find_map(|x| b.iter().find(|y| meets(x, y)).map(|y| norm(y)))
}

/// Order a plan: a step with no dependencies and a scope of its own goes in the first wave; a step that needs another waits for
/// the wave after it; two steps whose scopes meet never share a wave (the later one waits), so tasks never edit the same file.
pub fn split_plan(steps: &[PlanStep]) -> Result<Split, String> {
    if steps.is_empty() {
        return Err("the plan has no steps".into());
    }
    if steps.len() > MAX_OPEN {
        return Err(format!("a plan has at most {MAX_OPEN} steps"));
    }
    for (i, s) in steps.iter().enumerate() {
        let name = if s.title.trim().is_empty() { format!("step {}", i + 1) } else { s.title.clone() };
        if s.title.trim().is_empty() && s.goal.trim().is_empty() {
            return Err(format!("step {} needs a title or a goal", i + 1));
        }
        if s.files.iter().all(|f| norm(f).is_empty()) {
            return Err(format!("{name}: list the files or folders the step changes (its scope), so the split can keep tasks apart"));
        }
        for d in &s.depends_on {
            if *d >= steps.len() || *d == i {
                return Err(format!("{name}: depends on step {} which does not exist or is itself", d + 1));
            }
        }
    }
    let n = steps.len();
    let mut wave = vec![0usize; n];
    let mut why: Vec<Option<String>> = vec![None; n];
    // a fixed point; a cycle never settles and is refused after n rounds of growth
    for round in 0..=(n * n + 1) {
        let mut changed = false;
        for i in 0..n {
            for &d in &steps[i].depends_on {
                if wave[i] < wave[d] + 1 {
                    wave[i] = wave[d] + 1;
                    why[i] = Some(format!("needs step {} first", d + 1));
                    changed = true;
                }
            }
            for j in 0..i {
                if wave[j] == wave[i] {
                    if let Some(f) = scopes_meet(&steps[i].files, &steps[j].files) {
                        wave[i] += 1;
                        why[i] = Some(format!("shares {f} with step {}, so it waits for it", j + 1));
                        changed = true;
                    }
                }
            }
        }
        if !changed {
            break;
        }
        if round > n * n {
            return Err("the steps depend on each other in a circle".into());
        }
    }
    let last = wave.iter().copied().max().unwrap_or(0);
    let waves: Vec<Vec<usize>> = (0..=last).map(|w| (0..n).filter(|i| wave[*i] == w).collect::<Vec<_>>()).filter(|v| !v.is_empty()).collect();
    let notes = (0..n).filter(|i| wave[*i] > 0).map(|i| format!("{}: {}", if steps[i].title.trim().is_empty() { format!("step {}", i + 1) } else { steps[i].title.clone() }, why[i].clone().unwrap_or_default())).collect();
    Ok(Split { waves, notes })
}

// ------------------------------------------------------------------------------------------------ the running plan

/// A fanned-out plan: the wave that runs now and the ones that follow once it is merged.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct PlanRun {
    pub steps: Vec<PlanStep>,
    pub waves: Vec<Vec<usize>>,
    /// The next wave to start.
    pub next: usize,
    /// The tasks of the wave that runs now.
    pub current: Vec<String>,
    pub agent: String,
    pub model: String,
    pub stopped: String,
}

impl PlanRun {
    pub(super) fn load(root: &Path) -> Option<PlanRun> {
        let p: PlanRun = serde_json::from_str(&std::fs::read_to_string(root.join(".iyakki/tasks/plan.json")).ok()?).ok()?;
        (p.next < p.waves.len()).then_some(p)
    }

    fn save(&self, root: &Path) {
        let dir = root.join(".iyakki/tasks");
        if std::fs::create_dir_all(&dir).is_ok() {
            if self.next >= self.waves.len() {
                let _ = std::fs::remove_file(dir.join("plan.json"));
            } else if let Ok(t) = serde_json::to_string(self) {
                let _ = std::fs::write(dir.join("plan.json"), t);
            }
        }
    }

    /// Steps that have not started.
    pub fn waiting(&self) -> usize {
        self.waves.iter().skip(self.next).map(Vec::len).sum()
    }
}

fn step_task(s: &PlanStep, agent: &str, model: &str) -> NewTask {
    let scope = s.files.iter().map(|f| norm(f)).filter(|f| !f.is_empty()).collect::<Vec<_>>().join(", ");
    NewTask {
        title: s.title.chars().take(100).collect(),
        goal: format!("{}\n\nYour scope: change only {scope}. Other tasks work on the rest in parallel; files outside your scope will conflict when merged.", s.goal.trim()),
        agent: agent.into(),
        model: model.into(),
        ..Default::default()
    }
}

impl Manager {
    /// The free memory the cap sees.
    pub fn free_memory(&self) -> Option<u64> {
        let f = self.inner.mem_source.lock().unwrap_or_else(|e| e.into_inner()).clone();
        // unit tests of other parts must not depend on how full the developer's machine is
        #[cfg(test)]
        let fallback = || Some(64u64 << 30);
        #[cfg(not(test))]
        let fallback = meminfo_available;
        f.map_or_else(fallback, |f| f())
    }

    /// Tests: where the free memory comes from.
    pub fn set_memory_source(&self, f: Option<Arc<dyn Fn() -> Option<u64> + Send + Sync>>) {
        *self.inner.mem_source.lock().unwrap_or_else(|e| e.into_inner()) = f;
        self.pump();
    }

    /// What one task is expected to need, in MB (kept for this workspace).
    pub fn set_task_memory_mb(&self, mb: u64) {
        let mb = mb.clamp(100, 64_000);
        self.st().task_mb = mb;
        self.write_config("task_memory_mb", serde_json::json!(mb));
        self.pump();
    }

    pub fn task_memory_mb(&self) -> u64 {
        self.st().task_mb
    }

    /// How many tasks may run at once right now: the maximum, lowered by the free memory.
    pub fn effective_concurrency(&self) -> usize {
        let avail = self.free_memory();
        let s = self.st();
        let running = s.entries.iter().filter(|e| e.task.status.active()).count();
        memory_cap(s.max_concurrency, running, avail, s.task_mb)
    }

    /// Say why queued tasks wait (only those that wait for a place, not for rows).
    pub(super) fn hold_note(&self, msg: &str) {
        let mut s = self.st();
        for e in s.entries.iter_mut().filter(|e| e.task.status == Status::Queued && (e.task.last.starts_with("waiting for a free place") || e.task.last.starts_with("waiting for memory"))) {
            e.task.last = msg.to_string();
        }
    }

    /// A slow nudge for the UI: start waiting tasks once memory is free again (at most every 5 seconds).
    pub fn poll(&self) {
        {
            let mut l = self.inner.last_poll.lock().unwrap_or_else(|e| e.into_inner());
            if l.is_some_and(|t| t.elapsed() < Duration::from_secs(5)) {
                return;
            }
            *l = Some(Instant::now());
        }
        if self.st().entries.iter().any(|e| e.task.status == Status::Queued) {
            self.pump();
        }
    }

    // ---- fan-out

    /// The plan that is running, if later waves are still to start.
    pub fn plan(&self) -> Option<PlanRun> {
        self.inner.plan.lock().unwrap_or_else(|e| e.into_inner()).clone()
    }

    /// Split a plan and start its first wave, each step in its own worktree. Later waves start by themselves once the wave before
    /// them is merged (a discarded task stops the plan). Returns the split and the ids made.
    pub fn fan_out(&self, steps: Vec<PlanStep>, agent: &str, model: &str) -> Result<(Split, Vec<String>), String> {
        let split = split_plan(&steps)?;
        if self.plan().is_some() {
            return Err("a plan is already running: merge or discard its tasks first".into());
        }
        if self.st().entries.iter().filter(|e| !e.task.status.finished()).count() + split.waves[0].len() > MAX_OPEN {
            return Err(format!("that would make more than {MAX_OPEN} unfinished tasks: merge or discard some first"));
        }
        let mut ids = Vec::new();
        for &i in &split.waves[0] {
            ids.push(self.spawn(step_task(&steps[i], agent, model)).map_err(|e| format!("{e} (started: {})", ids.join(", ")))?);
        }
        let run = PlanRun { steps, waves: split.waves.clone(), next: 1, current: ids.clone(), agent: agent.into(), model: model.into(), stopped: String::new() };
        run.save(&self.inner.root);
        self.audit("task.plan", &format!("{} step(s) in {} wave(s): {}", run.steps.len(), run.waves.len(), ids.join(",")));
        *self.inner.plan.lock().unwrap_or_else(|e| e.into_inner()) = (run.next < run.waves.len()).then_some(run);
        Ok((split, ids))
    }

    /// Start the next wave when every task of the one before is merged; stop the plan when one was discarded.
    pub(super) fn advance_plan(&self) {
        let Some(mut run) = self.inner.plan.lock().unwrap_or_else(|e| e.into_inner()).take() else { return };
        let states: Vec<Option<Status>> = run.current.iter().map(|id| self.get(id).map(|t| t.status)).collect();
        if states.iter().any(|s| matches!(s, Some(Status::Discarded) | None)) {
            run.stopped = "a task of the earlier wave was discarded".into();
            self.notify("Plan stopped", &format!("{} step(s) were not started: a task they depend on was discarded.", run.waiting()));
            run.next = run.waves.len();
            run.save(&self.inner.root);
            return;
        }
        if !states.iter().all(|s| *s == Some(Status::Merged)) {
            *self.inner.plan.lock().unwrap_or_else(|e| e.into_inner()) = Some(run);
            return;
        }
        let wave = run.waves[run.next].clone();
        let mut ids = Vec::new();
        let mut failed = String::new();
        for i in wave {
            match self.spawn(step_task(&run.steps[i], &run.agent, &run.model)) {
                Ok(id) => ids.push(id),
                Err(e) => {
                    failed = e;
                    break;
                }
            }
        }
        if !failed.is_empty() {
            self.notify("Plan stopped", &format!("the next wave could not start: {failed}"));
            run.next = run.waves.len();
        } else {
            run.next += 1;
            run.current = ids;
        }
        run.save(&self.inner.root);
        if run.next < run.waves.len() {
            *self.inner.plan.lock().unwrap_or_else(|e| e.into_inner()) = Some(run);
        }
    }

    // ---- the lead summary

    /// What each finished task did, the conflicts and what to merge; `None` while something still runs or waits to run, or when
    /// no task has a result to report.
    pub fn lead_summary(&self) -> Option<String> {
        let list = self.list();
        if list.iter().any(|t| matches!(t.status, Status::Running | Status::NeedsYou | Status::Queued)) {
            return None;
        }
        let mut reports = Vec::new();
        for t in list.iter().filter(|t| matches!(t.status, Status::Done | Status::Failed)) {
            let files: Vec<String> = self.files(&t.id).map(|f| f.into_iter().map(|x| x.1).collect()).unwrap_or_default();
            let check = if t.status == Status::Done { self.merge_check(&t.id).ok() } else { None };
            reports.push(Report { id: t.id.clone(), title: t.title.clone(), status: t.status, files, conflicts: check.as_ref().map(|c| c.conflicts.clone()).unwrap_or_default(), blocking: check.as_ref().map(|c| c.blocking.clone()).unwrap_or_default(), tests: t.tests.clone(), said: t.summary.clone(), error: t.error.clone() });
        }
        if reports.is_empty() {
            return None;
        }
        Some(summary_text(&reports, self.plan().map_or(0, |p| p.waiting())))
    }

    /// The last summary written when all tasks stopped (empty before the first).
    pub fn last_summary(&self) -> String {
        self.inner.summary.lock().unwrap_or_else(|e| e.into_inner()).1.clone()
    }

    /// When nothing runs or waits any more and there is something new to report, write the summary once and tell the person.
    pub(super) fn maybe_summarize(&self) {
        let key = {
            let s = self.st();
            if s.entries.iter().any(|e| matches!(e.task.status, Status::Running | Status::NeedsYou | Status::Queued)) {
                return;
            }
            let k: Vec<String> = s.entries.iter().filter(|e| matches!(e.task.status, Status::Done | Status::Failed)).map(|e| format!("{}:{}:{}", e.task.id, e.task.status.label(), e.task.ended)).collect();
            if k.is_empty() {
                return;
            }
            k.join(",")
        };
        {
            let mut l = self.inner.summary.lock().unwrap_or_else(|e| e.into_inner());
            if l.0 == key {
                return;
            }
            l.0 = key.clone();
        }
        let me = self.clone();
        std::thread::spawn(move || {
            if let Some(text) = me.lead_summary() {
                {
                    let mut l = me.inner.summary.lock().unwrap_or_else(|e| e.into_inner());
                    if l.0 != key {
                        return;
                    }
                    l.1 = text.clone();
                }
                me.notify("Parallel tasks finished: what to merge", &text);
            }
        });
    }
}

/// What the summary needs to know about one finished task.
#[derive(Clone, Debug, PartialEq)]
pub struct Report {
    pub id: String,
    pub title: String,
    pub status: Status,
    pub files: Vec<String>,
    /// Files that conflict with the main branch.
    pub conflicts: Vec<String>,
    /// Files the person has uncommitted changes in.
    pub blocking: Vec<String>,
    pub tests: String,
    pub said: String,
    pub error: String,
}

/// The lead's summary as plain text: one line per task, the conflicts, the files two tasks both changed, and a merge order.
/// `waiting` is the number of plan steps that start after a merge.
pub fn summary_text(reports: &[Report], waiting: usize) -> String {
    let done = reports.iter().filter(|r| r.status == Status::Done).count();
    let failed = reports.len() - done;
    let mut out = format!("{} task(s) finished: {done} done{}.\n", reports.len(), if failed > 0 { format!(", {failed} failed") } else { String::new() });
    for r in reports {
        let files = match r.files.len() {
            0 => "changed nothing".to_string(),
            n => format!("changed {n} file(s): {}{}", r.files.iter().take(5).cloned().collect::<Vec<_>>().join(", "), if n > 5 { ", ..." } else { "" }),
        };
        if r.status == Status::Failed {
            out.push_str(&format!("- {} \"{}\" FAILED: {}; {files}.\n", r.id, r.title, crate::agent::truncate(&r.error, 120)));
        } else {
            let tests = if r.tests.is_empty() { "tests not run".to_string() } else { r.tests.clone() };
            let said = r.said.lines().find(|l| !l.trim().is_empty()).map(|l| format!(" It reported: {}", crate::agent::truncate(l.trim(), 140))).unwrap_or_default();
            out.push_str(&format!("- {} \"{}\" done: {files}; {tests}.{said}\n", r.id, r.title));
        }
    }
    // files more than one task changed
    let mut overlaps: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for r in reports {
        for f in &r.files {
            overlaps.entry(f.clone()).or_default().push(r.id.clone());
        }
    }
    overlaps.retain(|_, v| v.len() > 1);
    for (f, ids) in &overlaps {
        out.push_str(&format!("Overlap: {f} was changed by {}; merge one, then rebase the other.\n", ids.join(" and ")));
    }
    for r in reports.iter().filter(|r| !r.conflicts.is_empty()) {
        out.push_str(&format!("Conflict: {} \"{}\" conflicts with the main branch in {}.\n", r.id, r.title, r.conflicts.join(", ")));
    }
    for r in reports.iter().filter(|r| !r.blocking.is_empty()) {
        out.push_str(&format!("Blocked: {} touches {} where you have uncommitted changes.\n", r.id, r.blocking.join(", ")));
    }
    // clean first, then the ones that overlap or conflict
    let touches_overlap = |r: &Report| r.files.iter().any(|f| overlaps.contains_key(f));
    let mut order: Vec<&Report> = reports.iter().filter(|r| r.status == Status::Done && !r.files.is_empty()).collect();
    order.sort_by_key(|r| (!r.conflicts.is_empty() || !r.blocking.is_empty(), touches_overlap(r), r.files.len()));
    if order.is_empty() {
        out.push_str("Nothing to merge.\n");
    } else {
        let list: Vec<String> = order.iter().map(|r| if !r.conflicts.is_empty() { format!("{} (rebase first)", r.id) } else if touches_overlap(r) { format!("{} (check the overlap)", r.id) } else { r.id.clone() }).collect();
        out.push_str(&format!("To merge, in this order: {}.\n", list.join(", ")));
    }
    if waiting > 0 {
        out.push_str(&format!("{waiting} more plan step(s) start by themselves once these are merged.\n"));
    }
    out
}
