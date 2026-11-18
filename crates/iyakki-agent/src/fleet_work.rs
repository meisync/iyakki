//! Work that comes from another member (F-FLEET-03, F-FLEET-05, F-ROUTER-08; docs/development/FLEET-AND-MODEL-ROUTER-PLAN.md section 3.3).
//!
//! * **The worker.** An assignment the person here accepted (or that a trusted member sent) is made an ordinary parallel task of this
//!   computer's task manager (`tasks::Manager::spawn`): its own tree and branch, its own run, its own approvals and budgets. Nothing the
//!   planner sent carries an approval or a capability; a question that needs a person is shown here and the planner is only told the task
//!   is `needs_you`.
//! * **The hand-back.** When the task is done, its branch since the base commit is made into a Git bundle by the real `git`, and the bundle,
//!   the head commit and the test result this computer saw go back to the planner over the fleet channel (`fleet_hub/work.rs`). A failure
//!   or a discarded task goes back as a message without a bundle.
//! * **Choosing a member.** [`pick_member`] reads the heartbeats (models, load, memory, disk, tasks running) and says which member fits.
//!   [`FleetApi::requeue_lost`](crate::fleettools::FleetApi) uses it to send a silent member's tasks to another one.

use std::collections::BTreeMap;
use std::path::Path;
use std::sync::{Arc, Mutex};

use crate::fleet::Heartbeat;
use crate::fleet_hub::{Hub, InboxItem, MemberView, Status as HubStatus};
use crate::tasks::{git_ok, git_run, Budgets, Manager, NewTask, Status as TaskStatus};

/// What the worker does on a computer: the hub it hears from and the task manager that runs the work.
pub struct Worker {
    hub: Arc<Hub>,
    mgr: Arc<Manager>,
    /// The workspace's own test command, run in the task's tree when the task's run did not leave a test result.
    test: Mutex<Option<(String, Vec<String>)>>,
    busy: Mutex<()>,
    /// The models of this computer as profiles, so a task's model class picks one here.
    models: Mutex<Option<Arc<dyn Fn() -> Vec<crate::router_registry::Profile> + Send + Sync>>>,
    said: Mutex<BTreeMap<String, String>>,
}

impl Worker {
    /// How many tasks of the attached workspace are running or waiting for their person (the figure the heartbeat shares).
    pub fn running(&self) -> u32 {
        let (running, needs_you, _) = self.mgr.counts();
        (running + needs_you) as u32
    }

    pub fn new(hub: Arc<Hub>, mgr: Arc<Manager>) -> Arc<Worker> {
        Arc::new(Worker { hub, mgr, test: Mutex::new(None), busy: Mutex::new(()), models: Mutex::new(None), said: Mutex::new(BTreeMap::new()) })
    }

    /// Make the hub call [`Worker::tick`] when work arrives or is decided, and after every heartbeat round.
    pub fn install(self: &Arc<Self>) {
        let w = self.clone();
        self.hub.set_work_hook(Arc::new(move || {
            w.tick();
        }));
    }

    /// Tell the worker where this computer's models come from (the router registry), so a task that asks for a model class (`vision`,
    /// `audio`) runs on a model here that has it instead of the default.
    pub fn set_models(&self, f: Arc<dyn Fn() -> Vec<crate::router_registry::Profile> + Send + Sync>) {
        *self.models.lock().unwrap_or_else(std::sync::PoisonError::into_inner) = Some(f);
    }

    fn model_for(&self, class: &str) -> String {
        let f = self.models.lock().unwrap_or_else(std::sync::PoisonError::into_inner).clone();
        f.and_then(|f| pick_model(&f(), class)).unwrap_or_default()
    }

    pub fn set_test_command(&self, program: &str, args: &[String]) {
        *self.test.lock().unwrap_or_else(std::sync::PoisonError::into_inner) = Some((program.to_string(), args.to_vec()));
    }

    /// One pass: start what was accepted, tell the planner how running tasks are doing, hand back what is done. Returns plain lines of what
    /// happened (for the log and the tests). Two passes never run at once.
    pub fn tick(&self) -> Vec<String> {
        let Ok(_g) = self.busy.try_lock() else { return vec![] };
        let mut out = vec![];
        for item in self.hub.status().inbox {
            match (item.state.as_str(), item.task.is_empty()) {
                ("accepted", true) => out.push(self.start(&item)),
                ("running", false) => {
                    if let Some(l) = self.follow(&item) {
                        out.push(l);
                    }
                }
                _ => {}
            }
        }
        out
    }

    fn start(&self, item: &InboxItem) -> String {
        let b = &item.body;
        let goal = format!("{}\n\n(Sent by the member {} of a fleet. The work is yours to do under your own approvals; the result goes back as a branch.)", b.goal, item.from_name);
        let mut budgets = Budgets::default();
        if b.budget_tokens > 0 {
            budgets.max_tokens = b.budget_tokens;
        }
        // a follow-up starts from the planner's integration branch, which this computer does not have: the planner sent it as a bundle first
        self.fetch_base(&b.base_commit);
        let new = |rows: Vec<String>| NewTask { title: b.title.clone(), goal: goal.clone(), rows, model: self.model_for(&b.model_class), from_member: item.from_name.clone(), budgets: Some(budgets.clone()), base_commit: b.base_commit.clone(), ..Default::default() };
        let (id, note) = match self.mgr.spawn(new(b.rows.clone())) {
            Ok(id) => (Ok(id), String::new()),
            // a row the planner named that this computer's board does not have, or one that is taken here: the task still runs, without the rows
            Err(e) if !b.rows.is_empty() && (e.contains("no board row") || e.contains("already")) => (self.mgr.spawn(new(vec![])), format!("rows not used here: {e}")),
            Err(e) => (Err(e), String::new()),
        };
        match id {
            Ok(id) => {
                self.hub.inbox_set(&item.id, "running", &id, &note);
                self.mgr.hooks_audit("task.fleet", &format!("{id} from member {} task {}", item.from_name, &item.id[..8]));
                format!("started {} as task {id}", &item.id[..8])
            }
            Err(e) => {
                self.hub.inbox_set(&item.id, "failed", "", &e);
                let _ = self.hub.send_handback(item, "failed", "", "", "", "", &format!("could not start the task here: {e}"), None);
                format!("could not start {}: {e}", &item.id[..8])
            }
        }
    }

    /// Make a base commit available in the workspace: when it is not there and the planner sent its bundle, fetch the bundle's branches into
    /// `refs/iyakki-fleet-base/` (they stay out of the person's own branches).
    fn fetch_base(&self, commit: &str) {
        if commit.is_empty() {
            return;
        }
        let root = self.mgr.root().to_path_buf();
        if git_run(&root, &["cat-file", "-e", &format!("{commit}^{{commit}}")]).0 {
            return;
        }
        if let Some(bundle) = self.hub.base_path(commit) {
            let b = bundle.to_string_lossy().into_owned();
            let _ = git_run(&root, &["-c", "core.hooksPath=/dev/null", "fetch", "-q", "--no-tags", "--force", &b, "refs/heads/*:refs/iyakki-fleet-base/*"]);
        }
    }

    fn follow(&self, item: &InboxItem) -> Option<String> {
        let t = self.mgr.get(&item.task)?;
        let said = |state: &str, detail: &str| {
            let mut m = self.said.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
            if m.get(&item.id).map(String::as_str) == Some(state) {
                return;
            }
            if self.hub.send_status(item, state, detail).is_ok() {
                m.insert(item.id.clone(), state.to_string());
            }
        };
        // the planner answered the question the task asked: it goes to the running task as guidance, once
        if let Some(a) = self.hub.take_answer(&item.id) {
            let _ = self.mgr.guide(&t.id, &format!("The member that sent you this task answers your question: {a}"));
        }
        match t.status {
            TaskStatus::Running => {
                said("running", &t.progress_text());
                None
            }
            TaskStatus::NeedsYou => {
                // the question itself goes with the status, so the planner can answer it (`fleet_work` action `answer`)
                let q: String = t.last.chars().filter(|c| !c.is_control()).take(120).collect();
                said("needs_you", &if q.trim().is_empty() { "a question or approval waits for the person on that computer".to_string() } else { format!("asks: {q}") });
                None
            }
            TaskStatus::Paused | TaskStatus::Interrupted => {
                said("paused", t.status.label());
                None
            }
            TaskStatus::Queued => None,
            TaskStatus::Failed => Some(self.finish(item, "failed", &t.error.clone(), false)),
            TaskStatus::Discarded => Some(self.finish(item, "failed", "the person on that computer discarded the task", false)),
            TaskStatus::Done | TaskStatus::Merged => Some(self.finish(item, "done", &t.summary.clone(), true)),
        }
    }

    fn finish(&self, item: &InboxItem, status: &str, why: &str, with_bundle: bool) -> String {
        let Some(t) = self.mgr.get(&item.task) else { return "the task is gone".into() };
        let mut tests = t.tests.clone();
        if tests.is_empty() && with_bundle && t.tree.exists() {
            let cmd = self.test.lock().unwrap_or_else(std::sync::PoisonError::into_inner).clone();
            if let Some((p, a)) = cmd {
                tests = self.mgr.run_tests(&t.id, &p, &a).unwrap_or_else(|e| e);
            }
        }
        if tests.is_empty() {
            tests = "no test run was seen".into();
        }
        let summary = if why.trim().is_empty() { format!("{} ({})", t.title, t.progress_text()) } else { why.to_string() };
        let (mut head, mut bundle) = (String::new(), None);
        if with_bundle && t.tree.exists() {
            head = git_ok(&t.tree, &["rev-parse", "HEAD"]).map(|s| s.trim().to_string()).unwrap_or_default();
            let path = t.tree.parent().unwrap_or(&t.tree).join("handback.bundle");
            let range = format!("{}..{}", t.base, t.branch);
            let (ok, _, err) = git_run(&t.tree, &["bundle", "create", &path.to_string_lossy(), &range]);
            if ok {
                // what the task wrote must not carry a credential to another computer (the fleet's rule: no secrets travel)
                if let Some(what) = secret_in_changes(&t.tree, &range) {
                    let _ = std::fs::remove_file(&path);
                    let why = format!("not sent: the changes hold something that looks like a credential ({what}); put the value in the vault and name it instead");
                    self.hub.audit_line("fleet.handback.secret", &format!("task={} what={what}", &item.id[..8]));
                    let sent = self.hub.send_handback(item, "failed", "", "", "", "", &why, None);
                    if sent.is_ok() {
                        self.hub.inbox_set(&item.id, "failed", "", &why);
                    }
                    return format!("kept {} back: {why}", &item.id[..8]);
                }
                bundle = Some(path);
            } else if !err.contains("empty bundle") {
                self.hub.inbox_set(&item.id, "running", "", &format!("could not make the bundle: {}", err.trim()));
                return format!("could not make the bundle of {}: {}", &item.id[..8], err.trim());
            }
        }
        let summary = if with_bundle && bundle.is_none() { format!("{summary} (the task changed nothing)") } else { summary };
        let sent = self.hub.send_handback(item, status, &head, &t.base, &t.branch, &tests, &summary, bundle.as_deref());
        if let Some(p) = &bundle {
            let _ = std::fs::remove_file(p);
        }
        match sent {
            Ok(()) => {
                self.hub.inbox_set(&item.id, if status == "done" { "handed_back" } else { "failed" }, "", "");
                format!("handed back {} ({status})", &item.id[..8])
            }
            // the planner is not reachable right now: the task stays, and the next pass tries again
            Err(e) => format!("could not hand back {} yet: {e}", &item.id[..8]),
        }
    }
}

impl Manager {
    /// An access-log line written by the fleet bridge in the manager's own audit hook.
    pub fn hooks_audit(&self, what: &str, detail: &str) {
        if let Some(a) = self.hooks().audit {
            a(what, detail);
        }
    }
}

/// The workspace a fleet works with on this computer: the worker, the shared Context and the test command.
pub struct Attached {
    pub worker: Arc<Worker>,
    pub ctx: Arc<crate::fleet_context::ContextSync>,
    pub root: std::path::PathBuf,
    pub test: Option<(String, Vec<String>)>,
}

/// Make `mgr` (the task manager of an open IDE workspace) the place work from other members runs, and the workspace's Context the one that is
/// shared. The hub calls the worker when work arrives and after each heartbeat round.
pub fn attach(hub: &Arc<Hub>, mgr: Arc<Manager>, root: &Path, test: &str, ctx_dir: &Path) -> Arc<Attached> {
    let worker = Worker::new(hub.clone(), mgr);
    let mut parts = test.split_whitespace().map(str::to_string);
    let test = parts.next().map(|p| (p, parts.collect::<Vec<_>>()));
    if let Some((p, a)) = &test {
        worker.set_test_command(p, a);
    }
    worker.install();
    let ctx = Arc::new(crate::fleet_context::ContextSync::new(root, ctx_dir));
    let c2 = ctx.clone();
    hub.set_context_handler(Arc::new(move |fleet, _from, name, msg| c2.incoming(fleet, name, msg)));
    let a = Arc::new(Attached { worker, ctx, root: root.to_path_buf(), test });
    hub.set_attached(a.clone());
    a
}

// ------------------------------------------------------------------------------------------------ choosing a member

/// What a step or task needs from a member.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Want {
    pub vision: bool,
    pub audio: bool,
    /// Members that must not be picked (the one that was lost, the one that already refused).
    pub not: Vec<String>,
}

/// The member that fits, from the heartbeats: it must be up, hold a model with the abilities wanted, have disk and memory to spare and fewer tasks
/// running than its processors can carry; among those the least loaded wins. `None` when nobody fits (the caller keeps the work, or asks the
/// person). The model abilities are hints from the model names (the same hints the router registry uses), not tested facts.
pub fn pick_member(st: &HubStatus, fleet: &str, want: &Want) -> Option<MemberView> {
    let f = st.fleets.iter().find(|f| f.id == fleet)?;
    f.members
        .iter()
        .filter(|m| !m.me && m.state == "up" && !want.not.contains(&m.id))
        .filter_map(|m| m.heartbeat.as_ref().map(|h| (m, h)))
        .filter(|(_, h)| fits(h, want))
        .min_by_key(|(m, h)| (score(h), m.id.clone()))
        .map(|(m, _)| m.clone())
}

/// What the added lines of a task's changes look like when one of them is a credential: `Some(kind, file)` for the first, else `None`.
pub fn secret_in_changes(tree: &Path, range: &str) -> Option<String> {
    let (ok, out, _) = git_run(tree, &["diff", "--no-color", "-U0", "--text", range]);
    if !ok {
        return None;
    }
    let mut file = String::new();
    let mut added = String::new();
    let mut owner: Vec<(usize, String)> = vec![];
    for l in out.lines() {
        if let Some(f) = l.strip_prefix("+++ b/") {
            file = f.to_string();
        } else if let Some(a) = l.strip_prefix('+') {
            owner.push((added.len(), file.clone()));
            added.push_str(a);
            added.push('\n');
            if added.len() > 8 * 1024 * 1024 {
                break;
            }
        }
    }
    let f = crate::credguard::detect(&added)?;
    let at = added.find(&f.value).unwrap_or(0);
    let name = owner.iter().rev().find(|(o, _)| *o <= at).map(|(_, n)| n.clone()).unwrap_or_default();
    Some(format!("{} in {}", f.what, name))
}

/// The model of this computer for a task's model class: an enabled model that can carry out a task and has the ability the class names
/// (`vision`, `audio`), one on this computer first, then the cheapest. Empty class or no such model: `None` (the default model runs it).
pub fn pick_model(profiles: &[crate::router_registry::Profile], class: &str) -> Option<String> {
    if class.is_empty() {
        return None;
    }
    let mut ok: Vec<&crate::router_registry::Profile> = profiles
        .iter()
        .filter(|p| p.enabled && p.role.can_execute() && match class {
            "vision" => p.caps.vision,
            "audio" => p.caps.audio,
            _ => false,
        })
        .collect();
    ok.sort_by_key(|p| (!p.local, p.cost, p.id.clone()));
    ok.first().map(|p| p.model.clone())
}

fn fits(h: &Heartbeat, w: &Want) -> bool {
    let ok_vision = !w.vision || h.models.iter().any(|m| crate::providers::model_sees_pictures(m));
    let ok_audio = !w.audio || h.models.iter().any(|m| m.to_ascii_lowercase().contains("audio") || m.to_ascii_lowercase().contains("whisper"));
    ok_vision && ok_audio && h.mem_used_pct < 90 && h.disk_free_mb >= 2000 && (h.tasks_running as usize) < member_cap(h) && h.cpu_load_pct < 150 && !h.models.is_empty()
}

/// How many tasks a member can run at once: the same memory-aware rule its own task manager uses (`tasks::memory_cap`), worked out from the
/// figures of its heartbeat (processors as the person's maximum, the memory not used as the free memory, the usual size of a task).
pub fn member_cap(h: &Heartbeat) -> usize {
    let free_mb = h.mem_total_mb.saturating_mul(100u64.saturating_sub(h.mem_used_pct as u64)) / 100;
    crate::tasks::memory_cap(h.cpus.max(1) as usize, h.tasks_running as usize, Some(free_mb * 1024 * 1024), crate::tasks::DEFAULT_TASK_MB)
}

fn score(h: &Heartbeat) -> u32 {
    h.cpu_load_pct + h.mem_used_pct + 40 * h.tasks_running
}

// ------------------------------------------------------------------------------------------------ the router with the fleet (F-ROUTER-08)

/// Lets the model router hand a step to the fleet member whose models and load fit (`router::FleetRunner`). The step goes as an ordinary
/// fleet task (so the other computer's person or trust setting decides, its own approvals and budgets apply), the answer is the worker's
/// hand-back. A member that refused, failed or did not answer in time is not asked again for the same run.
pub struct FleetDispatch {
    hub: Arc<Hub>,
    gate: Arc<dyn iyakki_core_api::fleet::Fleet>,
    fleet: String,
    wait: std::time::Duration,
    tried: Mutex<Vec<String>>,
}

impl FleetDispatch {
    pub fn new(hub: Arc<Hub>, gate: Arc<dyn iyakki_core_api::fleet::Fleet>, fleet: &str, wait: std::time::Duration) -> FleetDispatch {
        FleetDispatch { hub, gate, fleet: fleet.to_string(), wait, tried: Mutex::new(vec![]) }
    }
}

impl crate::router::FleetRunner for FleetDispatch {
    fn members(&self) -> Vec<crate::router::MemberInfo> {
        let not = self.tried.lock().unwrap_or_else(std::sync::PoisonError::into_inner).clone();
        let st = self.hub.status();
        st.fleets
            .iter()
            .filter(|f| self.fleet.is_empty() || f.id == self.fleet)
            .flat_map(|f| f.members.iter())
            .filter(|m| !m.me && m.state == "up" && !not.contains(&m.id))
            .filter_map(|m| m.heartbeat.as_ref().map(|h| (m, h)))
            .filter(|(_, h)| fits(h, &Want::default()))
            .map(|(m, h)| crate::router::MemberInfo {
                name: m.name.clone(),
                models: h.models.iter().take(8).cloned().collect(),
                cpu_load_pct: h.cpu_load_pct,
                mem_used_pct: h.mem_used_pct,
                tasks_running: h.tasks_running,
                cpus: h.cpus,
                vision: h.models.iter().any(|x| crate::providers::model_sees_pictures(x)),
                audio: h.models.iter().any(|x| x.to_ascii_lowercase().contains("audio") || x.to_ascii_lowercase().contains("whisper")),
            })
            .collect()
    }

    fn run_on(&self, step: &crate::router::Step, goal: &str, member: &str) -> Option<(String, Result<String, String>)> {
        let st = self.hub.status();
        let all: Vec<&MemberView> = st.fleets.iter().filter(|f| self.fleet.is_empty() || f.id == self.fleet).flat_map(|f| f.members.iter()).collect();
        let id = all.iter().find(|m| m.name == member && !m.me)?.id.clone();
        // every other member is excluded, so the normal picker can only choose the named one (and only when it fits the step)
        let others: Vec<String> = all.iter().filter(|m| m.id != id).map(|m| m.id.clone()).collect();
        self.dispatch(step, goal, others)
    }

    fn run_remote(&self, step: &crate::router::Step, goal: &str) -> Option<(String, Result<String, String>)> {
        self.dispatch(step, goal, vec![])
    }
}

impl FleetDispatch {
    fn dispatch(&self, step: &crate::router::Step, goal: &str, extra_not: Vec<String>) -> Option<(String, Result<String, String>)> {
        let mut not = self.tried.lock().unwrap_or_else(std::sync::PoisonError::into_inner).clone();
        not.extend(extra_not);
        let want = Want { vision: step.needs.vision, audio: step.needs.audio, not };
        let st = self.hub.status();
        // one fleet named: that one; none named: the fits of every fleet of this computer compared, the lightest member wins
        let (fleet, m) = st
            .fleets
            .iter()
            .filter(|f| self.fleet.is_empty() || f.id == self.fleet)
            .filter_map(|f| pick_member(&st, &f.id, &want).map(|m| (f.id.clone(), m)))
            .min_by_key(|(_, m)| (m.heartbeat.as_ref().map_or(u32::MAX, score), m.id.clone()))?;
        let fail = |e: String| {
            self.tried.lock().unwrap_or_else(std::sync::PoisonError::into_inner).push(m.id.clone());
            Some((m.name.clone(), Err(e)))
        };
        let id = match crate::cloudkeys::random::<16>() {
            Ok(r) => crate::cloudkeys::hex(&r),
            Err(e) => return fail(e),
        };
        let check = if step.check.is_empty() { String::new() } else { format!("\nHow it is judged: {}", step.check) };
        let body = crate::fleet_hub::AssignBody {
            id: id.clone(),
            title: crate::fleet::clean(&format!("Step {}: {}", step.id, if step.title.is_empty() { &step.task } else { &step.title }), 120),
            goal: format!("{}{check}\n(One step of a larger goal of {} characters.)", step.task, goal.chars().count()),
            base_branch: String::new(),
            base_commit: String::new(),
            rows: vec![],
            context_digest: String::new(),
            budget_tokens: 0,
            model_class: if step.needs.vision { "vision".into() } else if step.needs.audio { "audio".into() } else { String::new() },
            after: vec![],
        };
        if let Err(e) = crate::routertools::with_press(|| self.gate.assign(&format!("run the router step {} on {}", step.id, m.name))) {
            return fail(e);
        }
        if let Err(e) = self.hub.assign(&fleet, &m.id, &body) {
            return fail(e);
        }
        let t = std::time::Instant::now();
        loop {
            let state = self.hub.status().outbox.into_iter().find(|o| o.id == id).map(|o| o.state).unwrap_or_default();
            match state.as_str() {
                "returned" => {
                    let r = self.hub.returned(&fleet).into_iter().find(|r| r.id == id);
                    return Some((m.name.clone(), Ok(r.map(|r| format!("{} [tests: {}]", r.summary, r.tests.lines().next().unwrap_or(""))).unwrap_or_default())));
                }
                "declined" | "question" | "requeued" => return fail(format!("the member {} answered {state}", m.name)),
                x if x.starts_with("failed") => return fail(format!("the member {} said: {x}", m.name)),
                _ if t.elapsed() > self.wait => return fail(format!("the member {} did not answer in {} s", m.name, self.wait.as_secs())),
                _ => std::thread::sleep(std::time::Duration::from_millis(50)),
            }
        }
    }
}

#[cfg(test)]
#[path = "fleet_work_tests.rs"]
mod tests;
