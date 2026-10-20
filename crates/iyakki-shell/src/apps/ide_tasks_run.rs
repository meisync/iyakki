//! How the IDE runs a parallel task for real: the Autopilot machinery with the person's own tools, narrowed to the task's tree.
//! See `docs/PARALLEL-TASKS.md`. Tests put a fake runner in with [`use_runner`].

use std::path::Path;
use std::sync::{Arc, Mutex};

use iyakki_agent::agent::{RunLimits, UserTools};
use iyakki_agent::tasks::{autopilot_runner, Hooks, Manager, Runner, RunnerParts};

use super::AppCx;

static OVERRIDE: Mutex<Option<Runner>> = Mutex::new(None);

/// Tests and screenshots give the IDE a fake runner (no model, no vendor); `None` goes back to the real one.
pub fn use_runner(r: Option<Runner>) {
    *OVERRIDE.lock().unwrap_or_else(|e| e.into_inner()) = r;
}

/// The runner of a workspace's tasks.
pub fn make_runner(cx: &AppCx, root: &Path) -> Runner {
    if let Some(r) = OVERRIDE.lock().unwrap_or_else(|e| e.into_inner()).clone() {
        return r;
    }
    let (user, base, svc, host, mcp) = (cx.session.user.clone(), cx.session.policy.clone(), cx.services.clone(), cx.host.clone(), cx.mcp_servers.clone());
    let root_s = root.display().to_string();
    let price = cx.services.settings.lock().unwrap_or_else(|e| e.into_inner()).get().token_price_cents_per_mtok;
    let reminder = iyakki_agent::toolblock::ReminderCtx { agent_on: true, browser_policy: "off".into(), engine_installed: false, screen_available: false, loaded: None };
    let (cx2, audit) = (cx.clone(), cx.audit.clone());
    let user2 = user.clone();
    let tools_user = user.clone();
    let parts = RunnerParts {
        backend: cx.chat.clone(),
        tools: Arc::new(move |spec| {
            // the working folder and the file scope are the task's tree: nothing else (the main workspace is not readable)
            let tree = spec.tree.display().to_string();
            let mut p = base.clone();
            p.fs_read = if base.allows_read(&tools_user, &root_s) { vec![tree.clone()] } else { vec![] };
            p.fs_write = if base.allows_write(&tools_user, &root_s) { vec![tree] } else { vec![] };
            Arc::new(UserTools::new(&tools_user, p, svc.clone(), host.clone(), mcp.clone()).with_request(&spec.prompt).with_cwd(&spec.tree))
        }),
        limits: RunLimits::for_budget(&cx.session.policy.budget, price),
        assist: iyakki_agent::assistrun::Assist::standard(reminder),
        executor: Some(Arc::new(move |spec, tools| {
            let agent = spec.task.agent.as_str();
            (agent != "builtin" && !agent.is_empty()).then(|| super::chat::ext::task_executor(&cx2, agent, tools, spec.tree.clone(), spec.task.session.clone(), spec.task.model.clone()))
        })),
        history: None,
        audit: Some(Arc::new(move |k, d| audit.record(&user2, k, d))),
    };
    autopilot_runner(parts)
}

/// The hooks that put a task's events in the audit log and the inbox of the person.
pub fn hooks(cx: &AppCx, workspace: &str) -> Hooks {
    let (audit, user) = (cx.audit.clone(), cx.session.user.clone());
    let (svc, user2, ws) = (cx.services.clone(), cx.session.user.clone(), workspace.to_string());
    let (runs, user3) = (cx.services.runs.clone(), cx.session.user.clone());
    Hooks {
        audit: Some(Arc::new(move |k, d| audit.record(&user, k, d))),
        notify: Some(Arc::new(move |title, text| {
            let _ = svc.inbox.add(&user2, iyakki_agent::inbox::Kind::Agent, &format!("[{ws}] {title}"), text, "tasks", Some(iyakki_agent::inbox::Link::new("inbox", "")), iyakki_agent::clock::now_unix());
        })),
        cleanup: None,
        // every run of a task is kept as a run log with the task, row and agent, so RUNS lists and opens it
        record: Some(Arc::new(move |r| {
            if r.run_id.is_empty() {
                return;
            }
            let log = iyakki_agent::runs::RunLog { id: r.run_id, user: user3.clone(), started: r.started.max(0) as u64, prompt: format!("{}: {}", r.task.title, r.task.goal), status: r.status, events: r.events, session: Some(r.task.session.clone()), task_id: Some(r.task.id.clone()), row: r.task.rows.first().cloned(), agent: Some(r.agent) };
            let _ = runs.save(&log);
        })),
    }
}

/// A manager for a workspace, with its runner and hooks (the IDE owns it; the workspace chat gets a clone).
pub fn manager(cx: &AppCx, root: &Path, features: &Path, name: &str) -> Manager {
    let m = Manager::open(root, features);
    m.set_hooks(hooks(cx, name));
    m.set_runner(make_runner(cx, root));
    m
}

// ------------------------------------------------------------------------------------------------ the IDE's side of the tab

use super::ide::{Confirm, FormKind, IdeApp, MainTab, Modal};
use super::ide_layout::Pane;
use super::ide_tasks::TasksAct;
use super::widgets::Form;

impl IdeApp {
    /// The agents a task may use: (id, model), in the order of the New task form's choice.
    pub(super) fn task_agents(&self, cx: &AppCx) -> Vec<(String, String)> {
        let mut v = vec![("builtin".to_string(), String::new())];
        let cfg = cx.services.agents_cfg();
        for a in cfg.all() {
            v.push((a.id(), a.cfg_model()));
        }
        v
    }

    /// The runner is made again from the current model and policy whenever something starts, so a model chosen since the
    /// workspace opened is the one a new task gets.
    fn refresh_task_runner(&mut self, cx: &AppCx) {
        if let Some(w) = self.active_mut() {
            let root = w.root();
            w.tasks.mgr.set_runner(make_runner(cx, &root));
        }
    }

    /// Make tasks from the lead's or the person's request. The first error ends it (the ones made stay).
    pub(super) fn spawn_tasks(&mut self, list: Vec<iyakki_agent::tasks::NewTask>, cx: &AppCx) -> Result<Vec<String>, String> {
        if self.active_ws().is_some_and(|w| w.ro.is_some()) {
            return Err("Read-only: another session has this workspace open.".into());
        }
        if cx.session.policy.cli != iyakki_sec::policy::CliLevel::Full {
            return Err("Tasks run commands in their own folder: your account needs the full command-line permission.".into());
        }
        self.refresh_task_runner(cx);
        let mut ids = Vec::new();
        let Some(w) = self.active_mut() else { return Err("no workspace".into()) };
        for n in list {
            let id = w.tasks.mgr.spawn(n)?;
            ids.push(id);
        }
        if let Some(id) = ids.first() {
            w.tasks.select_id(id);
        }
        w.main_tab = MainTab::Tasks;
        w.set_focus(Pane::Main);
        w.tasks.status = Some((format!("{} task(s) started in their own folders", ids.len()), true));
        w.tree.refresh();
        self.redraw = true;
        Ok(ids)
    }

    pub(super) fn open_new_task_form(&mut self, cx: &AppCx, rows: &str, title: &str) {
        let agents = self.task_agents(cx);
        let labels: Vec<String> = {
            let cfg = cx.services.agents_cfg();
            std::iter::once("Built-in assistant".to_string()).chain(cfg.all().iter().map(|a| a.label())).collect()
        };
        let opts: Vec<&str> = labels.iter().map(String::as_str).collect();
        let _ = agents;
        let f = Form::new("New task (runs in its own copy of the workspace)")
            .text("Title", title, "a short name")
            .text("Goal", "", "what it must do, completely: it cannot see the chat")
            .text("Board rows", rows, "optional: F-A-01, F-A-02")
            .choice("Agent", &opts, 0)
            .text("Minutes", "", "optional, default 30")
            .text("Token budget", "", "optional, default 300,000")
            .text("Cost cap (cents)", "", "optional, default none");
        self.modal = Some(Modal::Form(FormKind::NewTask, f));
    }

    /// "Run as task" on the selected board row, or one task per open planned row.
    pub(super) fn run_rows_as_tasks(&mut self, all_planned: bool, cx: &AppCx) {
        let Some(w) = self.active_ws() else { return };
        let rows: Vec<iyakki_agent::featureboard::Feature> = if all_planned {
            w.board.filtered().into_iter().filter(|f| f.status == "planned" && !f.is_bug()).collect()
        } else {
            w.board.selected().into_iter().collect()
        };
        if rows.is_empty() {
            self.notice = Some(("Select a board row first (or there are no planned rows).".into(), false));
            return;
        }
        let cap = iyakki_agent::tasks::MAX_PER_CALL;
        let list: Vec<iyakki_agent::tasks::NewTask> = rows
            .iter()
            .take(cap)
            .map(|f| {
                let (agent, model) = ("builtin".to_string(), String::new());
                iyakki_agent::tasks::NewTask { title: format!("{} {}", f.id, f.title), goal: format!("Do board row {}: {}", f.id, f.title), rows: vec![f.id.clone()], agent, model, budgets: None, ..Default::default() }
            })
            .collect();
        let more = rows.len().saturating_sub(cap);
        match self.spawn_tasks(list, cx) {
            Ok(ids) => {
                if more > 0 {
                    self.notice = Some((format!("Started {} tasks; {more} more rows were left (at most {cap} at once)", ids.len()), true));
                }
            }
            Err(e) => self.notice = Some((e, false)),
        }
    }

    /// A control of an agent's panel on the board (stop, pause, resume, message, open the run).
    pub(super) fn agent_cmd(&mut self, key: &str, c: char) {
        let Some(w) = self.active_mut() else { return };
        if w.ro.is_some() && c != 'o' {
            w.say("Read-only: another session has this workspace open.", false);
            return;
        }
        if w.tasks.mgr.get(key).is_none() {
            w.say("This agent runs in the chat (Autopilot or an external agent): stop or pause it there.", false);
            return;
        }
        let r = match c {
            's' => w.tasks.mgr.stop(key).map(|()| "Stopping the agent.".to_string()),
            'p' => w.tasks.mgr.pause(key).map(|()| "The agent pauses at the end of its step.".to_string()),
            'r' => w.tasks.mgr.resume(key, "").map(|()| "Queued: the agent continues from its saved place.".to_string()),
            'm' => {
                let f = Form::new("Message for the agent").text("Message", "", "guidance: it pauses after its step and continues with this");
                self.modal = Some(Modal::Form(FormKind::GuideAgent(key.to_string()), f));
                self.redraw = true;
                return;
            }
            'o' => {
                // the run log of the task opens in RUNS (the IDE is kept aside and comes back with Back); without one, the Tasks tab
                let run = iyakki_agent::assign::load(&w.root()).get(key).map(|a| a.run_id.clone()).filter(|r| !r.is_empty());
                match run {
                    Some(id) => self.request = Some(("runs".into(), format!("run:{id}"))),
                    None => {
                        w.tasks.select_id(key);
                        self.show_main(MainTab::Tasks);
                    }
                }
                return;
            }
            _ => return,
        };
        match r {
            Ok(m) => w.say(m, true),
            Err(e) => w.say(e, false),
        }
        self.redraw = true;
    }

    /// Start an agent (a parallel task) on each of these rows. They are ready: every dependency is done. The board stays in front.
    pub(super) fn assign_rows(&mut self, rows: Vec<String>, cx: &AppCx) {
        let Some(w) = self.active_ws() else { return };
        let feats = w.board.file.as_ref().map(|f| f.board.features()).unwrap_or_default();
        let cap = iyakki_agent::tasks::MAX_PER_CALL;
        let list: Vec<iyakki_agent::tasks::NewTask> = rows
            .iter()
            .take(cap)
            .filter_map(|id| feats.iter().find(|f| &f.id == id))
            .map(|f| iyakki_agent::tasks::NewTask { title: format!("{} {}", f.id, f.title), goal: format!("Do board row {}: {}", f.id, f.title), rows: vec![f.id.clone()], ..Default::default() })
            .collect();
        if list.is_empty() {
            return;
        }
        let tab = self.active_ws().map(|w| w.main_tab);
        match self.spawn_tasks(list, cx) {
            Ok(ids) => {
                if let (Some(w), Some(t)) = (self.active_mut(), tab) {
                    w.main_tab = t;
                    w.say(format!("Started {} agent{} on the ready rows.", ids.len(), if ids.len() == 1 { "" } else { "s" }), true);
                }
            }
            Err(e) => self.notice = Some((e, false)),
        }
    }

    pub(super) fn tasks_act(&mut self, a: TasksAct, cx: &AppCx) {
        match a {
            TasksAct::None | TasksAct::Used | TasksAct::Back => {}
            TasksAct::New => self.open_new_task_form(cx, "", ""),
            TasksAct::Followup(id) => {
                let f = Form::new("Follow-up for the task").text("Message", "", "what to do next");
                self.modal = Some(Modal::Form(FormKind::TaskFollowup(id), f));
            }
            TasksAct::Discard(id) => {
                if self.active_ws().is_some_and(|w| w.ro.is_some()) {
                    return;
                }
                self.modal = Some(Modal::Confirm(Confirm::DiscardTask(id)));
            }
            TasksAct::Merge(id) => {
                let Some(w) = self.active_mut() else { return };
                if w.ro.is_some() {
                    w.tasks.status = Some(("Read-only: another session has this workspace open.".into(), false));
                    return;
                }
                match w.tasks.mgr.merge_check(&id) {
                    Ok(c) if c.clean && c.files > 0 => self.modal = Some(Modal::Confirm(Confirm::MergeTask(id, c.message))),
                    Ok(c) => {
                        let (title, text) = if c.conflicts.is_empty() && c.blocking.is_empty() { ("Nothing to merge".to_string(), c.message) } else { ("The merge would not go through".to_string(), format!("{} Nothing was changed. You can Rebase on the latest commit (b), open the files in the editor, or Discard the task.", c.message)) };
                        self.modal = Some(Modal::Info(title, text));
                    }
                    Err(e) => w.tasks.status = Some((e, false)),
                }
            }
            TasksAct::Open(rel) => self.open_file(&rel, None, cx),
        }
        self.redraw = true;
    }

    pub(super) fn merge_task(&mut self, id: &str, cx: &AppCx) {
        let Some(w) = self.active_mut() else { return };
        match w.tasks.mgr.merge(id) {
            Ok(m) => {
                w.tasks.status = Some((m.clone(), true));
                w.say(format!("Task merged: {m}"), true);
                w.tree.refresh();
                let p = w.features_path();
                w.editor.reload_clean(cx, &p);
                let _ = w.board.poll_disk();
            }
            Err(e) => w.tasks.status = Some((e, false)),
        }
    }
}
