//! The lead assistant's tools for parallel tasks (see `docs/PARALLEL-TASKS.md`): `task_spawn` starts isolated tasks (Ask: the person
//! approves every spawn) and `task_status` reads them. There is no merge or discard tool: those are the person's buttons.
//! Tasks themselves are not offered these tools, so a task cannot start more tasks.

use std::sync::Arc;

use serde_json::{json, Value};

use crate::agent::{Risk, ToolDef, Tools};
use crate::chat::ToolSpec;
use crate::tasks::{split_plan, Manager, NewTask, PlanStep, Split, MAX_PER_CALL};

/// One line for the lead assistant's system prompt.
pub const PROMPT_RULE: &str = "\nPARALLEL TASKS: for independent pieces of work (\"do these three things in parallel\") call task_spawn: each task runs in its own isolated copy of the workspace and the person reviews and merges the result in the Tasks tab. Tasks cannot see this chat, so write each goal completely. task_status shows how they are doing and, when all have finished, what each did, the conflicts and what to merge. For a plan with several steps call task_plan with each step's title, goal, the files it changes and which steps it depends on: independent steps with separate files run in parallel, the rest follow after a merge. You cannot merge or discard them.";

pub const TASK_TOOLS: [&str; 3] = ["task_spawn", "task_status", "task_plan"];

pub struct TaskTools {
    inner: Arc<dyn Tools>,
    mgr: Manager,
    /// The agent a spawn without `agent` uses (the chat's current choice).
    default_agent: String,
    default_model: String,
}

fn is_task_tool(n: &str) -> bool {
    TASK_TOOLS.contains(&n)
}

fn clean_id(s: &str) -> String {
    s.chars().filter(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | ':' | '-')).take(60).collect()
}

impl TaskTools {
    pub fn new(inner: Arc<dyn Tools>, mgr: Manager, default_agent: &str, default_model: &str) -> TaskTools {
        TaskTools { inner, mgr, default_agent: default_agent.into(), default_model: default_model.into() }
    }

    /// The tasks a call asks for: `tasks: [...]`, or the one in the arguments themselves.
    pub fn requested(&self, args: &Value) -> Result<Vec<NewTask>, String> {
        let list: Vec<Value> = match args.get("tasks").and_then(Value::as_array) {
            Some(a) => a.clone(),
            None => vec![args.clone()],
        };
        if list.is_empty() {
            return Err("give at least one task".into());
        }
        if list.len() > MAX_PER_CALL {
            return Err(format!("at most {MAX_PER_CALL} tasks per call"));
        }
        let mut out = Vec::new();
        for v in &list {
            let s = |k: &str| v.get(k).and_then(Value::as_str).unwrap_or("").trim().to_string();
            let rows: Vec<String> = v.get("rows").and_then(Value::as_array).map(|a| a.iter().filter_map(Value::as_str).map(|r| r.trim().to_string()).filter(|r| !r.is_empty()).collect()).unwrap_or_default();
            let (title, goal) = (s("title"), s("goal"));
            if title.is_empty() && goal.is_empty() {
                return Err("each task needs a title or a goal".into());
            }
            let agent = clean_id(&s("agent"));
            let model = clean_id(&s("model"));
            out.push(NewTask {
                title: title.chars().take(100).collect(),
                goal: goal.chars().take(4000).collect(),
                rows,
                agent: if agent.is_empty() { self.default_agent.clone() } else { agent },
                model: if model.is_empty() && agent_is_default(&s("agent")) { self.default_model.clone() } else { model },
                budgets: None,
                override_deps: false,
            });
        }
        Ok(out)
    }

    /// The steps of a `task_plan` call.
    pub fn plan_steps(&self, args: &Value) -> Result<Vec<PlanStep>, String> {
        let list = args.get("steps").and_then(Value::as_array).ok_or("give `steps`: a list of steps")?;
        let mut out = Vec::new();
        for v in list {
            let s = |k: &str| v.get(k).and_then(Value::as_str).unwrap_or("").trim().to_string();
            let files = v.get("files").and_then(Value::as_array).map(|a| a.iter().filter_map(Value::as_str).map(|r| r.trim().chars().take(200).collect::<String>()).filter(|r| !r.is_empty() && !r.starts_with('/') && !r.contains("..")).collect()).unwrap_or_default();
            // step numbers in the call start at 1
            let depends_on = v.get("depends_on").and_then(Value::as_array).map(|a| a.iter().filter_map(Value::as_u64).map(|n| (n as usize).wrapping_sub(1)).collect()).unwrap_or_default();
            out.push(PlanStep { title: s("title").chars().take(100).collect(), goal: s("goal").chars().take(4000).collect(), files, depends_on });
        }
        Ok(out)
    }

    fn plan_text(steps: &[PlanStep], split: &Split) -> String {
        let name = |i: usize| if steps[i].title.is_empty() { format!("step {}", i + 1) } else { steps[i].title.clone() };
        let mut out = split.waves.iter().enumerate().map(|(w, v)| format!("{}: {}", if w == 0 { "Now, in parallel".to_string() } else { format!("After the merge of wave {w}") }, v.iter().map(|i| name(*i)).collect::<Vec<_>>().join("; "))).collect::<Vec<_>>().join("\n");
        for n in &split.notes {
            out.push_str(&format!("\n  note: {n}"));
        }
        out
    }

    fn status_text(&self) -> String {
        let now = crate::clock::now_unix();
        let l = self.mgr.list();
        if l.is_empty() {
            return "No tasks yet.".into();
        }
        let mut out = l.iter().filter(|t| !t.status.finished() || now - t.ended < 3600).map(|t| format!("{} · {} · {} · {} · {}", t.id, t.title, t.status.label(), t.progress_text(), crate::agent::truncate(&t.last, 80))).collect::<Vec<_>>().join("\n");
        if let Some(sum) = self.mgr.lead_summary() {
            out.push_str("\n\n");
            out.push_str(&sum);
        }
        out
    }
}

fn agent_is_default(a: &str) -> bool {
    a.trim().is_empty()
}

impl Tools for TaskTools {
    fn defs(&self) -> Vec<ToolDef> {
        let mut v = self.inner.defs();
        v.retain(|d| !is_task_tool(&d.spec.name));
        let item = json!({"type":"object","properties":{
            "title":{"type":"string","description":"Short name of the task."},
            "goal":{"type":"string","description":"What the task must do, with everything it needs to know: it cannot see this chat."},
            "rows":{"type":"array","items":{"type":"string"},"description":"Board row ids (F-...) the task works on; a row can be in one task at a time."},
            "agent":{"type":"string","description":"builtin, claude-code, copilot or a custom agent id; default: the current one."},
            "model":{"type":"string","description":"Model name; default: the agent's."}},
            "required":["title","goal"]});
        v.push(ToolDef {
            spec: ToolSpec {
                name: "task_spawn".into(),
                description: format!("Start isolated tasks that run in parallel, each in its own copy of the workspace (a git worktree), for independent pieces of work (\"do these 3 things in parallel\"). Give either one task (title, goal, rows?, agent?, model?) or `tasks`: a list of at most {MAX_PER_CALL}. Tasks cannot see this chat, so each goal must be complete. They never change the workspace themselves: the person reviews the diff and merges. Do not use it for work that depends on another task's result."),
                schema: json!({"type":"object","properties":{
                    "title": item["properties"]["title"], "goal": item["properties"]["goal"], "rows": item["properties"]["rows"], "agent": item["properties"]["agent"], "model": item["properties"]["model"],
                    "tasks":{"type":"array","items":item,"description":"Several tasks at once."}}}),
            },
            risk: Risk::Ask,
            untrusted: false,
        });
        let step = json!({"type":"object","properties":{
            "title":{"type":"string"},
            "goal":{"type":"string","description":"Everything the step needs to know: it cannot see this chat."},
            "files":{"type":"array","items":{"type":"string"},"description":"Files or folders (relative) the step changes. Required: steps whose files meet never run at the same time."},
            "depends_on":{"type":"array","items":{"type":"integer"},"description":"Numbers (from 1) of the steps whose result this one needs; it starts after they are merged."}},
            "required":["title","goal","files"]});
        v.push(ToolDef {
            spec: ToolSpec {
                name: "task_plan".into(),
                description: "Split a plan into tasks and start the independent ones in parallel, each in its own worktree. Steps that depend on others, or that change the same files, start after the earlier ones are merged by the person. The person approves the split once.".into(),
                schema: json!({"type":"object","properties":{"steps":{"type":"array","items":step,"description":"The steps of the plan, at most 12."}},"required":["steps"]}),
            },
            risk: Risk::Ask,
            untrusted: false,
        });
        v.push(ToolDef { spec: ToolSpec { name: "task_status".into(), description: "List the parallel tasks with their status, progress and last event.".into(), schema: json!({"type":"object","properties":{}}) }, risk: Risk::Safe, untrusted: false });
        v
    }

    fn call(&self, name: &str, args: &Value) -> Result<String, String> {
        match name {
            "task_spawn" => {
                let list = self.requested(args)?;
                let mut made = Vec::new();
                for n in list {
                    let title = n.title.clone();
                    match self.mgr.spawn(n) {
                        Ok(id) => made.push(format!("{id}: {title}")),
                        Err(e) => {
                            made.push(format!("not started ({title}): {e}"));
                            break;
                        }
                    }
                }
                Ok(format!("{}\nThe tasks run in the background; the person reviews and merges them in the Tasks tab. Read progress with task_status.", made.join("\n")))
            }
            "task_plan" => {
                let steps = self.plan_steps(args)?;
                let (split, ids) = self.mgr.fan_out(steps.clone(), &self.default_agent, &self.default_model)?;
                Ok(format!("Started {} task(s): {}\n{}\nLater waves start by themselves after the person merges the wave before. Read progress with task_status.", ids.len(), ids.join(", "), Self::plan_text(&steps, &split)))
            }
            "task_status" => Ok(self.status_text()),
            _ => self.inner.call(name, args),
        }
    }

    fn describe(&self, name: &str, args: &Value) -> String {
        if name == "task_spawn" {
            return match self.requested(args) {
                Ok(l) => format!("Start {} parallel task(s): {}", l.len(), l.iter().map(|t| crate::agent::truncate(if t.title.is_empty() { &t.goal } else { &t.title }, 50)).collect::<Vec<_>>().join("; ")),
                Err(e) => format!("task_spawn (invalid: {e})"),
            };
        }
        if name == "task_plan" {
            return match self.plan_steps(args) {
                Ok(l) => format!("Fan out a plan of {} step(s) into parallel tasks", l.len()),
                Err(e) => format!("task_plan (invalid: {e})"),
            };
        }
        if name == "task_status" {
            return "List the parallel tasks".into();
        }
        self.inner.describe(name, args)
    }

    fn preview(&self, name: &str, args: &Value) -> Option<String> {
        if name == "task_spawn" {
            let l = self.requested(args).ok()?;
            return Some(l.iter().map(|t| format!("• {} [{}]{}\n  {}", t.title, t.agent, if t.rows.is_empty() { String::new() } else { format!(" rows {}", t.rows.join(", ")) }, crate::agent::truncate(&t.goal, 300))).collect::<Vec<_>>().join("\n"));
        }
        if name == "task_plan" {
            let steps = self.plan_steps(args).ok()?;
            let split = split_plan(&steps).ok()?;
            return Some(Self::plan_text(&steps, &split));
        }
        if is_task_tool(name) { None } else { self.inner.preview(name, args) }
    }

    fn refusal(&self, name: &str, args: &Value) -> Option<String> {
        if name == "task_spawn" {
            return match self.requested(args) {
                Ok(_) if self.mgr.open_count() >= crate::tasks::MAX_OPEN => Some(format!("there are already {} unfinished tasks: ask the person to merge or discard some first", crate::tasks::MAX_OPEN)),
                Ok(_) => None,
                Err(e) => Some(e),
            };
        }
        if name == "task_plan" {
            return match self.plan_steps(args).and_then(|s| split_plan(&s).map(|_| ())) {
                Ok(()) if self.mgr.plan().is_some() => Some("a plan is already running: its tasks must be merged or discarded first".into()),
                Ok(()) if self.mgr.open_count() >= crate::tasks::MAX_OPEN => Some(format!("there are already {} unfinished tasks: ask the person to merge or discard some first", crate::tasks::MAX_OPEN)),
                Ok(()) => None,
                Err(e) => Some(e),
            };
        }
        if is_task_tool(name) { None } else { self.inner.refusal(name, args) }
    }

    fn pre_approved(&self, name: &str) -> bool {
        !is_task_tool(name) && self.inner.pre_approved(name)
    }

    fn approved_always(&self, name: &str) -> bool {
        // every spawn asks: "always" lasts only for the other tools
        !is_task_tool(name) && self.inner.approved_always(name)
    }

    fn grant_blocker(&self, name: &str, args: &Value) -> Option<String> {
        if is_task_tool(name) { Some("starting parallel tasks asks every time".into()) } else { self.inner.grant_blocker(name, args) }
    }
    fn grant_scope(&self, name: &str, args: &Value) -> String {
        self.inner.grant_scope(name, args)
    }

    fn unresolved_work(&self) -> Vec<String> {
        self.inner.unresolved_work()
    }

    fn begin_step(&self) {
        self.inner.begin_step()
    }

    fn should_end_run(&self) -> Option<String> {
        self.inner.should_end_run()
    }

    fn take_diagnosis(&self) -> Option<(String, String)> {
        self.inner.take_diagnosis()
    }

    fn note_external(&self, name: &str, args: &Value, ok: bool) {
        self.inner.note_external(name, args, ok)
    }
}
