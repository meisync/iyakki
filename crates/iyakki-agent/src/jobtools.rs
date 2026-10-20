//! The assistant's tools for detached jobs (see `docs/JOBS.md`): `job_start`, `job_status`, `job_tail`, `job_list`, `job_wait` and
//! `job_stop`. They wrap the run's other tools (outermost), so an external coding agent that reaches IYAKKI's tools through the
//! policy bridge gets the same six.
//!
//! * `job_start` asks like `run_command` does: the person approves it, and in Auto it still asks for what `run_command` always
//!   asks for (destructive commands, anything outside the workspace, see `jobs::hard_ask_line`). The working folder must be inside
//!   the person's write scope (and inside the workspace in the IDE); an account that may not reach every host may not start one.
//! * Status, tail, list and wait only read: safe. Stop asks.
//! * A finished job is reported once to the assistant, at its next model turn (`take_notes`), and an Autopilot step that ends with
//!   a job running waits for it (`jobs_running`).

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use iyakki_sec::policy::{CliLevel, Policy};
use serde_json::{json, Value};

use crate::agent::{Risk, ToolDef, Tools};
use crate::chat::ToolSpec;
use crate::jobs::{fmt_duration, now_ms, Channel, Cmd, Job, Jobs, Spec, MAX_WAIT_SECS, STOP_GRACE};

pub const JOB_TOOLS: [&str; 6] = ["job_start", "job_status", "job_tail", "job_list", "job_wait", "job_stop"];

/// One paragraph for the assistant's system prompt (and, worded the same, for an external agent's instructions).
pub const PROMPT_RULE: &str = "\nJOBS: anything that may take longer than about 30 seconds (an SDK or emulator install, a big build, a download, a long test run, a server you start) goes through job_start, never through a foreground command: it runs detached, keeps running when this chat or the session ends, and the person sees it in the Jobs strip with its progress and Log and Stop buttons. After job_start do not poll with sleep or until-loops: call job_wait (up to 60 s, it returns early when the job ends) or job_status, and read the output with job_tail. You are told when a job finishes. Never start the same job twice: check job_list first. job_stop ends one (the person approves).";

/// The same advice as a line of an external agent's instructions (Claude Code and the others reach the tools as `mcp__iyakki__*`).
pub const EXTERNAL_RULE: &str = "For anything you expect to take more than about 30 seconds (SDK or emulator installs, big builds, downloads, long test runs, servers) call mcp__iyakki__job_start instead of running it in a foreground shell: a foreground command dies when this session ends, and the person cannot see it. A job runs detached, shows in the Jobs strip with progress and Log and Stop buttons, and survives this session. Do not poll with sleep or until-loops: use mcp__iyakki__job_wait (up to 60 s) or job_status, read output with job_tail, check job_list before starting the same job again.";

pub fn is_job_tool(n: &str) -> bool {
    JOB_TOOLS.contains(&n)
}

pub struct JobTools {
    inner: Arc<dyn Tools>,
    jobs: Jobs,
    user: String,
    policy: Policy,
    /// The folder a job without `cwd` runs in.
    cwd: PathBuf,
    /// (name, root) of the IDE workspace: jobs belong to it and stay inside it.
    workspace: Option<(String, PathBuf)>,
}

impl JobTools {
    pub fn new(inner: Arc<dyn Tools>, jobs: Jobs, user: &str, policy: Policy, cwd: &Path, workspace: Option<(&str, &Path)>) -> JobTools {
        JobTools { inner, jobs, user: user.into(), policy, cwd: cwd.into(), workspace: workspace.map(|(n, r)| (n.to_string(), r.to_path_buf())) }
    }

    /// Whether this account is offered the job tools at all (the same level as `run_command`).
    pub fn offered(p: &Policy) -> bool {
        p.cli == CliLevel::Full
    }

    fn ws_name(&self) -> &str {
        self.workspace.as_ref().map_or("", |w| w.0.as_str())
    }

    fn any_host(&self) -> bool {
        self.policy.net_hosts.iter().any(|h| h == "*")
    }

    /// What a start call asks for: the command and the folder, checked.
    fn plan(&self, args: &Value) -> Result<(Cmd, PathBuf, String), String> {
        let line = args.get("command").and_then(Value::as_str).map(str::trim).filter(|s| !s.is_empty());
        let argv: Vec<String> = args.get("argv").and_then(Value::as_array).map(|a| a.iter().filter_map(Value::as_str).map(str::to_string).collect()).unwrap_or_default();
        let cmd = match (line, argv.is_empty()) {
            (Some(l), true) => Cmd::Shell(l.to_string()),
            (None, false) => Cmd::Argv(argv),
            (Some(_), false) => return Err("give either `command` (a shell line) or `argv` (a program and its arguments), not both".into()),
            (None, true) => return Err("give `command` (a shell line) or `argv` (a program and its arguments)".into()),
        };
        let given = args.get("cwd").and_then(Value::as_str).map(str::trim).filter(|s| !s.is_empty());
        let cwd = match (&self.workspace, given) {
            (Some((_, root)), g) => crate::workspace::resolve_in(root, g.unwrap_or(""))?,
            (None, Some(g)) => crate::scope::resolve(&self.cwd, g)?,
            (None, None) => crate::scope::resolve(&self.cwd, ".")?,
        };
        crate::scope::check_write(&self.policy, &self.user, &cwd)?;
        let label = args.get("label").and_then(Value::as_str).unwrap_or("").trim().to_string();
        Ok((cmd, cwd, label))
    }

    fn line_of(j: &Job) -> String {
        let prog = match j.progress {
            Some(p) if j.running() => format!(" · {p:.0}%"),
            _ => String::new(),
        };
        let last = if j.last_line.is_empty() { String::new() } else { format!(" · last: {}", j.last_line) };
        format!("{} · {} · {}{prog} · {}{last}", j.id, j.label, j.state.label(), fmt_duration(j.elapsed_secs(now_ms())))
    }

    fn detail(&self, j: &Job) -> String {
        let mut s = format!("{}\ncommand: {}\nfolder: {}", Self::line_of(j), j.command, j.cwd);
        if j.running() {
            s.push_str(match j.progress {
                Some(_) => "\n(the percentage comes from the output)",
                None => "\n(no percentage in the output: progress unknown)",
            });
        }
        s
    }

    fn id_arg(args: &Value) -> Result<String, String> {
        args.get("id").and_then(Value::as_str).map(|s| s.trim().to_string()).filter(|s| !s.is_empty()).ok_or_else(|| "give the job `id` (from job_list)".into())
    }

    fn status_text(&self, args: &Value) -> Result<String, String> {
        match args.get("id").and_then(Value::as_str).filter(|s| !s.trim().is_empty()) {
            Some(id) => Ok(self.detail(&self.jobs.get(&self.user, id.trim())?)),
            None => self.list_text(),
        }
    }

    fn list_text(&self) -> Result<String, String> {
        let l = self.jobs.visible(&self.user, self.ws_name());
        if l.is_empty() {
            return Ok("No jobs.".into());
        }
        Ok(l.iter().take(30).map(Self::line_of).collect::<Vec<_>>().join("\n"))
    }
}

fn spec(name: &str, desc: &str, schema: Value, risk: Risk, untrusted: bool) -> ToolDef {
    ToolDef { spec: ToolSpec { name: name.into(), description: desc.into(), schema }, risk, untrusted }
}

impl Tools for JobTools {
    fn defs(&self) -> Vec<ToolDef> {
        let mut v = self.inner.defs();
        v.retain(|d| !is_job_tool(&d.spec.name));
        if !Self::offered(&self.policy) {
            return v;
        }
        let obj = |props: Value, req: &[&str]| json!({"type": "object", "properties": props, "required": req});
        v.push(spec(
            "job_start",
            "Start a long-running command as a detached JOB and return at once with its id. Use it for anything that may take more than about 30 seconds (SDK or emulator installs, big builds, downloads, long test runs, servers) instead of a foreground command: the job survives this chat, the session and a restart, its output goes to a log, and the person sees it in the Jobs strip (progress, Log, Stop). `command` is a shell line (pipes and && work), or `argv` is a program with separate arguments (no shell); `label` names it in the strip; `cwd` is the folder (default: the workspace or working folder; it must be inside it). Asks the person first. After starting, call job_wait or job_status, never a polling loop.",
            obj(json!({"command": {"type": "string"}, "argv": {"type": "array", "items": {"type": "string"}}, "label": {"type": "string"}, "cwd": {"type": "string"}}), &[]),
            Risk::Ask,
            false,
        ));
        v.push(spec("job_status", "State of a job: running, succeeded, failed (exit code), stopped or lost, how long it has run, its progress when the output shows a percentage, and the last line of output. Without `id` it lists the jobs.", obj(json!({"id": {"type": "string"}}), &[]), Risk::Safe, true));
        v.push(spec("job_tail", "The last lines of a job's output (secrets are removed). `id`; `lines` (default 40, at most 400).", obj(json!({"id": {"type": "string"}, "lines": {"type": "integer"}}), &["id"]), Risk::Safe, true));
        v.push(spec("job_list", "List the person's jobs in this workspace: id, label, state, progress, how long, last output line. Check it before starting a job so the same one is not started twice.", obj(json!({}), &[]), Risk::Safe, true));
        v.push(spec(
            "job_wait",
            &format!("Wait for a job to finish, at most `timeout_seconds` (default 30, never more than {MAX_WAIT_SECS}); returns at once when it ends, with its state and the last output. If it is still running, call it again or do other work: never write sleep or until-loops."),
            obj(json!({"id": {"type": "string"}, "timeout_seconds": {"type": "integer"}}), &["id"]),
            Risk::Safe,
            true,
        ));
        v.push(spec("job_stop", "Stop a job: TERM to its whole process group, then KILL if it does not end. Asks the person first.", obj(json!({"id": {"type": "string"}}), &["id"]), Risk::Ask, false));
        v
    }

    fn call(&self, name: &str, args: &Value) -> Result<String, String> {
        if is_job_tool(name) && !Self::offered(&self.policy) {
            return Err("your account may not run commands".into());
        }
        match name {
            "job_start" => {
                if let Some(e) = self.refusal(name, args) {
                    return Err(e);
                }
                let (cmd, cwd, label) = self.plan(args)?;
                let scope_policy = self.policy.clone();
                let user = self.user.clone();
                let j = self.jobs.start(&self.user, &Spec { label, cmd, cwd, workspace: self.ws_name().to_string() }, &move |p: &Path| crate::scope::check_write(&scope_policy, &user, p))?;
                Ok(format!("Started job {} “{}” (pid {}) in {}.\nIt runs detached: it keeps going if this chat or the session ends, and the person sees it in the Jobs strip with Log and Stop buttons. Do not poll in a loop: call job_wait (up to {MAX_WAIT_SECS} s, returns early when it ends) or job_status; read the output with job_tail. You will also be told when it finishes.", j.id, j.label, j.pid, j.cwd))
            }
            "job_status" => self.status_text(args),
            "job_list" => self.list_text(),
            "job_tail" => {
                let id = Self::id_arg(args)?;
                let lines = args.get("lines").and_then(Value::as_u64).unwrap_or(40) as usize;
                let j = self.jobs.get(&self.user, &id)?;
                let t = self.jobs.tail(&self.user, &id, lines)?;
                Ok(format!("{}\n--- last output ---\n{}", Self::line_of(&j), if t.is_empty() { "(nothing yet)" } else { &t }))
            }
            "job_wait" => {
                let id = Self::id_arg(args)?;
                let secs = args.get("timeout_seconds").and_then(Value::as_u64).unwrap_or(30).min(MAX_WAIT_SECS);
                let j = self.jobs.wait(&self.user, &id, Duration::from_secs(secs))?;
                if j.state.finished() {
                    // the assistant has the result now: it is not sent again as a note
                    self.jobs.mark_announced(&self.user, &id, Channel::Agent);
                    let t = self.jobs.tail(&self.user, &id, 15)?;
                    Ok(format!("{}\nFinished.\n--- last output ---\n{}", Self::line_of(&j), if t.is_empty() { "(no output)" } else { &t }))
                } else {
                    Ok(format!("{}\nStill running after {secs} s. Call job_wait again, or do something else meanwhile.", Self::line_of(&j)))
                }
            }
            "job_stop" => {
                let id = Self::id_arg(args)?;
                let j = self.jobs.stop(&self.user, &id, STOP_GRACE)?;
                self.jobs.mark_announced(&self.user, &id, Channel::Agent);
                Ok(format!("{}\nStopped (TERM to the group, KILL if it stayed).", Self::line_of(&j)))
            }
            _ => self.inner.call(name, args),
        }
    }

    fn describe(&self, name: &str, args: &Value) -> String {
        match name {
            "job_start" => match self.plan(args) {
                Ok((cmd, cwd, label)) => {
                    let text = cmd.text();
                    let one: String = text.chars().take(160).collect();
                    format!("start a detached job{} in {}: `{one}{}`", if label.is_empty() { String::new() } else { format!(" “{label}”") }, cwd.display(), if text.chars().count() > 160 { "…" } else { "" })
                }
                Err(e) => format!("job_start (invalid: {e})"),
            },
            "job_stop" => format!("stop job {}", args.get("id").and_then(Value::as_str).unwrap_or("?")),
            "job_status" | "job_tail" | "job_list" | "job_wait" => format!("{name} {}", args.get("id").and_then(Value::as_str).unwrap_or("")).trim().to_string(),
            _ => self.inner.describe(name, args),
        }
    }

    fn preview(&self, name: &str, args: &Value) -> Option<String> {
        if name == "job_start" {
            let (cmd, cwd, _) = self.plan(args).ok()?;
            return Some(format!("{}\nfolder: {}\nIt runs detached and keeps going when the chat ends; you can watch and stop it in the Jobs strip.", cmd.text(), cwd.display()));
        }
        if is_job_tool(name) { None } else { self.inner.preview(name, args) }
    }

    fn refusal(&self, name: &str, args: &Value) -> Option<String> {
        if name != "job_start" {
            return if is_job_tool(name) { None } else { self.inner.refusal(name, args) };
        }
        let (cmd, _, _) = match self.plan(args) {
            Ok(p) => p,
            Err(e) => return Some(e),
        };
        let connected = crate::devcluster::active();
        let window = crate::webengine::host_session();
        let guard = match &cmd {
            Cmd::Shell(l) => crate::devcluster::guard_line_with(window, connected.as_deref(), l),
            Cmd::Argv(a) => crate::devcluster::guard_with(window, connected.as_deref(), &a[0], &a[1..]),
        };
        if let Err(e) = guard {
            return Some(e);
        }
        if !self.any_host() {
            return Some("a job can reach any server, so only an account allowed to reach every host may start one".into());
        }
        let running = self.jobs.running_count(&self.user);
        if running >= crate::jobs::MAX_RUNNING {
            return Some(format!("{running} jobs are running already (at most {}): wait for one with job_wait or stop one", crate::jobs::MAX_RUNNING));
        }
        None
    }

    fn pre_approved(&self, name: &str) -> bool {
        !is_job_tool(name) && self.inner.pre_approved(name)
    }

    fn approved_always(&self, name: &str) -> bool {
        // every start and stop asks: "always" lasts only for the other tools
        !is_job_tool(name) && self.inner.approved_always(name)
    }

    fn grant_blocker(&self, name: &str, args: &Value) -> Option<String> {
        if is_job_tool(name) { crate::grants::blocker(name, args) } else { self.inner.grant_blocker(name, args) }
    }

    fn grant_scope(&self, name: &str, args: &Value) -> String {
        match name {
            // "allow for this session" covers the same command in the same folder, never another command (a shell line can be anything)
            "job_start" => match self.plan(args) {
                Ok((cmd, cwd, _)) => format!("{} @ {}", cmd.text(), cwd.display()),
                Err(_) => "invalid".into(),
            },
            "job_stop" => format!("job {}", args.get("id").and_then(Value::as_str).unwrap_or("?")),
            n if is_job_tool(n) => crate::grants::scope_of(n, args),
            _ => self.inner.grant_scope(name, args),
        }
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

    fn take_notes(&self) -> Vec<String> {
        let mut v = self.inner.take_notes();
        if Self::offered(&self.policy) {
            v.extend(self.jobs.take_finished(&self.user, self.ws_name(), Channel::Agent).iter().map(|j| self.jobs.finish_note(j)));
        }
        v
    }

    fn jobs_running(&self) -> Vec<String> {
        let mut v = self.inner.jobs_running();
        if Self::offered(&self.policy) {
            v.extend(self.jobs.visible(&self.user, self.ws_name()).iter().filter(|j| j.running()).map(|j| format!("{} ({})", j.label, j.id)));
        }
        v
    }
}
