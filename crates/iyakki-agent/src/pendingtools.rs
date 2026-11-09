//! The assistant's tools for pending work (F-AI-16, `docs/development/AGENT-WAIT-PLAN.md`): `wait_for`, `pending_list`,
//! `pending_cancel`, `pending_check`, `pending_settings`. They wrap the run's other tools (outermost, outside the job tools), so an
//! external coding agent that reaches IYAKKI's tools through the policy bridge gets the same five.
//!
//! * `wait_for` registers a ticket (a running job, a file that must appear, a loopback port that must open) and returns at once:
//!   the model ends its turn with "Waiting for: ..." and the waker starts a new turn when the ticket ends. With `block_seconds`
//!   it blocks (long-poll, for agents that cannot be woken). Safe: it only reads. A file is checked against the person's read
//!   scope, a port must be on this machine.
//! * `pending_list` is safe. `pending_cancel`, `pending_check` (an acceptance command run when a job ends) and `pending_settings`
//!   ask every time.
//! * The run sees the ledger through `jobs_running` (every source, not only jobs: Autopilot waits for all of it, and the
//!   end-of-run gate in `assistrun.rs` reads it) and `take_notes`.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use iyakki_sec::policy::{CliLevel, Policy};
use serde_json::{json, Value};

use crate::agent::{Risk, ToolDef, Tools};
use crate::chat::ToolSpec;
use crate::pending::{Feed, Ledger, Probe, Settings, Source, Ticket, MAX_WAIT_SECS};

pub const PENDING_TOOLS: [&str; 5] = ["wait_for", "pending_list", "pending_cancel", "pending_check", "pending_settings"];

/// The longest a single `wait_for` call blocks.
pub const MAX_BLOCK_SECS: u64 = 60;

pub const PROMPT_RULE: &str = "\nWAITING: you cannot wait; you answer once. Never end a turn with \"I'll wait\", \"I'll report back\" or \"I'll let you know\" unless something is registered to wake you: a job from job_start, a parallel task, or a `wait_for` ticket (a job id, a file that must appear, a local port that must open; give `on_done`, one line saying what you will do when it lands). After you register, end the turn with what you are waiting for; IYAKKI starts a new turn for you when it ends, with the result. A finished job is not a done goal: if you can name a check (a test command, a probe), set it with pending_check (the person approves) so the report says whether it passed. pending_list shows what is pending. If you cannot register anything, finish now and say what you did not verify.";

pub const EXTERNAL_RULE: &str = "You cannot be woken between turns. Never end a turn with \"I'll wait\" or \"I'll report back\": call mcp__iyakki__wait_for with block_seconds (up to 60, it returns the first pending work that ends) and call it again while work is pending, or finish and say what you did not verify.";

pub fn is_pending_tool(n: &str) -> bool {
    PENDING_TOOLS.contains(&n)
}

pub struct PendingTools {
    inner: Arc<dyn Tools>,
    ledger: Ledger,
    feed: Feed,
    policy: Policy,
    cwd: PathBuf,
    audit: Option<Arc<dyn Fn(&str, &str) + Send + Sync>>,
}

fn spec(name: &str, desc: &str, schema: Value, risk: Risk, untrusted: bool) -> ToolDef {
    ToolDef { spec: ToolSpec { name: name.into(), description: desc.into(), schema }, risk, untrusted }
}

impl PendingTools {
    pub fn new(inner: Arc<dyn Tools>, ledger: Ledger, feed: Feed, policy: Policy, cwd: &Path) -> PendingTools {
        PendingTools { inner, ledger, feed, policy, cwd: cwd.into(), audit: None }
    }

    pub fn with_audit(mut self, a: Arc<dyn Fn(&str, &str) + Send + Sync>) -> PendingTools {
        self.audit = Some(a);
        self
    }

    fn note(&self, what: &str, detail: &str) {
        if let Some(a) = &self.audit {
            a(what, detail);
        }
    }

    fn can_check(&self) -> bool {
        self.policy.cli == CliLevel::Full
    }

    fn jobs_pair(&self) -> Option<(&crate::jobs::Jobs, &str)> {
        self.feed.jobs.as_ref().map(|j| (j, self.feed.user.as_str()))
    }

    fn id_arg(args: &Value) -> Result<String, String> {
        args.get("id").and_then(Value::as_str).map(|s| s.trim().to_string()).filter(|s| !s.is_empty()).ok_or_else(|| "give the ticket `id` (from pending_list)".into())
    }

    fn deadline(args: &Value) -> u64 {
        let secs = args.get("timeout_seconds").and_then(Value::as_u64).unwrap_or(3600).clamp(5, MAX_WAIT_SECS);
        crate::jobs::now_ms() + secs * 1000
    }

    fn wait_for(&self, args: &Value) -> Result<String, String> {
        self.ledger.sync(&self.feed);
        let on_done = args.get("on_done").and_then(Value::as_str).unwrap_or("").trim().to_string();
        let label_arg = args.get("label").and_then(Value::as_str).unwrap_or("").trim().to_string();
        let ws = self.feed.workspace.clone();
        let t = if let Some(id) = args.get("job_id").and_then(Value::as_str).map(str::trim).filter(|s| !s.is_empty()) {
            let jobs = self.feed.jobs.as_ref().ok_or("jobs are not available here")?;
            let j = jobs.get(&self.feed.user, id)?;
            if j.state.finished() {
                return Ok(format!("Job {} is already over: {}. Read it with job_tail; nothing to wait for.", j.id, j.state.label()));
            }
            self.ledger.register(Ticket { source: Source::Job, key: j.id.clone(), label: j.label.clone(), workspace: ws, started_ms: j.started_ms, deadline_ms: Self::deadline(args), on_done, check_cwd: j.cwd.clone(), ..Ticket::default() })
        } else if let Some(p) = args.get("file").and_then(Value::as_str).map(str::trim).filter(|s| !s.is_empty()) {
            let path = crate::scope::resolve(&self.cwd, p)?;
            crate::scope::check_read(&self.policy, &self.feed.user, &path)?;
            let probe = Probe::File { path: path.display().to_string() };
            if probe.holds() {
                return Ok(format!("{} already: nothing to wait for.", probe.describe()));
            }
            let label = if label_arg.is_empty() { format!("wait for {}", probe.describe()) } else { label_arg };
            self.ledger.register(Ticket { source: Source::Probe, label, workspace: ws, deadline_ms: Self::deadline(args), on_done, probe: Some(probe), ..Ticket::default() })
        } else if let Some(port) = args.get("port").and_then(Value::as_u64) {
            let host = args.get("host").and_then(Value::as_str).unwrap_or("127.0.0.1").trim().to_string();
            if !matches!(host.as_str(), "127.0.0.1" | "localhost" | "::1") {
                return Err("only a port on this machine can be waited for (host 127.0.0.1 or localhost)".into());
            }
            let port = u16::try_from(port).map_err(|_| "`port` must be 1 to 65535".to_string())?;
            if port == 0 {
                return Err("`port` must be 1 to 65535".into());
            }
            let probe = Probe::Port { host, port };
            if probe.holds() {
                return Ok(format!("{} already: nothing to wait for.", probe.describe()));
            }
            let label = if label_arg.is_empty() { format!("wait for {}", probe.describe()) } else { label_arg };
            self.ledger.register(Ticket { source: Source::Probe, label, workspace: ws, deadline_ms: Self::deadline(args), on_done, probe: Some(probe), ..Ticket::default() })
        } else {
            return Err("give one of `job_id` (a running job), `file` (a path that must appear) or `port` (a local port that must accept connections)".into());
        };
        self.note("pending.register", &format!("{} {}", t.id, t.source.label()));
        let block = args.get("block_seconds").and_then(Value::as_u64).unwrap_or(0).min(MAX_BLOCK_SECS);
        if block > 0 {
            let t0 = Instant::now();
            loop {
                self.ledger.sync(&self.feed);
                let landed = self.ledger.take_landed(&self.feed.workspace, self.jobs_pair());
                if !landed.is_empty() {
                    self.note("pending.wake", &format!("{} landed (long-poll)", landed.len()));
                    return Ok(landed.join("\n\n"));
                }
                if t0.elapsed() >= Duration::from_secs(block) {
                    return Ok(format!("Ticket {} registered ({}). Still pending after {block} s. Call wait_for again with block_seconds, or finish and say what you are waiting for.", t.id, t.label));
                }
                std::thread::sleep(Duration::from_millis(250));
            }
        }
        Ok(format!("Ticket {} registered: {}. It is tracked by IYAKKI now: do not poll. End this turn by saying what you are waiting for (\"Waiting for: {}\"); you will be woken with the result when it ends (deadline {}).", t.id, t.label, t.label, crate::jobs::fmt_duration(t.deadline_ms.saturating_sub(crate::jobs::now_ms()) / 1000)))
    }

    fn list_text(&self) -> String {
        self.ledger.sync(&self.feed);
        let now = crate::jobs::now_ms();
        let l = self.ledger.list(&self.feed.workspace);
        let s = self.ledger.settings();
        let mut out = if l.is_empty() { "Nothing is pending.".to_string() } else { l.iter().rev().take(30).map(|t| t.line(now)).collect::<Vec<_>>().join("\n") };
        out.push_str(&format!("\n(auto-continue {}, {} wakes left today, longest wait {})", if s.auto_continue { "on" } else { "off" }, self.ledger.wakes_left(), crate::jobs::fmt_duration(s.max_wait_secs)));
        out
    }
}

impl Tools for PendingTools {
    fn defs(&self) -> Vec<ToolDef> {
        let mut v = self.inner.defs();
        v.retain(|d| !is_pending_tool(&d.spec.name));
        let obj = |props: Value, req: &[&str]| json!({"type": "object", "properties": props, "required": req});
        v.push(spec(
            "wait_for",
            "Register something to wait for and get woken when it ends: a running job (`job_id`), a file that must appear (`file`), or a port on this machine that must accept connections (`port`). Returns at once; end your turn saying \"Waiting for: ...\" and IYAKKI starts a new turn for you with the result. Never write \"I'll report back\" without a ticket. `on_done`: one line, what you will do when it lands. `timeout_seconds` (default 3600, at most 21600). `block_seconds` (at most 60) blocks and returns the first pending work that ended: only for agents that cannot be woken.",
            obj(json!({"job_id": {"type": "string"}, "file": {"type": "string"}, "port": {"type": "integer"}, "host": {"type": "string"}, "label": {"type": "string"}, "on_done": {"type": "string"}, "timeout_seconds": {"type": "integer"}, "block_seconds": {"type": "integer"}}), &[]),
            Risk::Safe,
            false,
        ));
        v.push(spec("pending_list", "List what is pending for this chat or workspace: running and ended tickets (jobs, parallel tasks, waits), their state, how long, the result line and whether the acceptance check passed; and the auto-continue settings.", obj(json!({}), &[]), Risk::Safe, true));
        v.push(spec("pending_cancel", "Stop waiting for a ticket (`id` from pending_list). A job ticket also stops the job. Asks the person first.", obj(json!({"id": {"type": "string"}}), &["id"]), Risk::Ask, false));
        if self.can_check() {
            v.push(spec(
                "pending_check",
                "Attach an acceptance check to a running job ticket (`id`, `command`): when the job ends with exit 0, IYAKKI runs the command in the job's folder and reports whether it passed, so \"finished\" means the goal was met, not only that the exit code was 0. Use a test command or a probe (`curl -fsS http://127.0.0.1:8080/health`, `test -f out.bin`). Asks the person first.",
                obj(json!({"id": {"type": "string"}, "command": {"type": "string"}}), &["id", "command"]),
                Risk::Ask,
                false,
            ));
        }
        v.push(spec(
            "pending_settings",
            "Change the waiting settings (`auto_continue` true/false, `max_wakes_per_day` 1-500, `max_wait_hours` 1-168). Without arguments it only shows them. Asks the person first.",
            obj(json!({"auto_continue": {"type": "boolean"}, "max_wakes_per_day": {"type": "integer"}, "max_wait_hours": {"type": "integer"}}), &[]),
            Risk::Ask,
            false,
        ));
        v
    }

    fn call(&self, name: &str, args: &Value) -> Result<String, String> {
        match name {
            "wait_for" => self.wait_for(args),
            "pending_list" => Ok(self.list_text()),
            "pending_cancel" => {
                let id = Self::id_arg(args)?;
                let t = self.ledger.cancel(&id)?;
                let mut extra = String::new();
                if t.source == Source::Job {
                    if let Some((j, user)) = self.jobs_pair() {
                        match j.stop(user, &t.key, crate::jobs::STOP_GRACE) {
                            Ok(_) => {
                                j.mark_announced(user, &t.key, crate::jobs::Channel::Agent);
                                extra = " The job was stopped too.".into();
                            }
                            Err(e) => extra = format!(" (the job could not be stopped: {e})"),
                        }
                    }
                } else if t.source == Source::Task {
                    extra = " The task itself keeps running: stop it in the Agents strip.".into();
                }
                self.note("pending.cancel", &id);
                Ok(format!("Stopped waiting for {} ({}).{extra}", t.id, t.label))
            }
            "pending_check" if self.can_check() => {
                let id = Self::id_arg(args)?;
                let cmd = args.get("command").and_then(Value::as_str).map(str::trim).filter(|s| !s.is_empty()).ok_or("give the check `command`")?;
                if let Some(why) = crate::jobs::hard_ask_line(cmd) {
                    return Err(format!("a check must only read: this command is refused ({why})"));
                }
                let t = self.ledger.get(&id).ok_or_else(|| format!("no ticket {id}"))?;
                let cwd = if t.check_cwd.is_empty() { self.cwd.display().to_string() } else { t.check_cwd.clone() };
                crate::scope::check_write(&self.policy, &self.feed.user, Path::new(&cwd))?;
                let t = self.ledger.set_check(&id, cmd, &cwd)?;
                self.note("pending.register", &format!("{} check", t.id));
                Ok(format!("Check set on {} ({}): `{cmd}` runs in {cwd} when it ends with exit 0, and the report says whether it passed.", t.id, t.label))
            }
            "pending_settings" => {
                let mut s: Settings = self.ledger.settings();
                if let Some(b) = args.get("auto_continue").and_then(Value::as_bool) {
                    s.auto_continue = b;
                }
                if let Some(n) = args.get("max_wakes_per_day").and_then(Value::as_u64) {
                    s.max_wakes_per_day = n.min(500) as u32;
                }
                if let Some(h) = args.get("max_wait_hours").and_then(Value::as_u64) {
                    s.max_wait_secs = h.clamp(1, 168) * 3600;
                }
                self.ledger.set_settings(s);
                let s = self.ledger.settings();
                self.note("pending.settings", &format!("auto_continue {}", s.auto_continue));
                Ok(format!("Waiting settings: auto-continue {}, at most {} wakes a day, longest wait {}.", if s.auto_continue { "on" } else { "off" }, s.max_wakes_per_day, crate::jobs::fmt_duration(s.max_wait_secs)))
            }
            _ => self.inner.call(name, args),
        }
    }

    fn describe(&self, name: &str, args: &Value) -> String {
        match name {
            "wait_for" => format!("wait for {}", ["job_id", "file", "port"].iter().find_map(|k| args.get(*k).map(|v| v.to_string().trim_matches('"').to_string())).unwrap_or_default()),
            "pending_list" => "list pending work".into(),
            "pending_cancel" => format!("stop waiting for {}", args.get("id").and_then(Value::as_str).unwrap_or("?")),
            "pending_check" => format!("attach a check to {}: `{}`", args.get("id").and_then(Value::as_str).unwrap_or("?"), args.get("command").and_then(Value::as_str).unwrap_or("").chars().take(160).collect::<String>()),
            "pending_settings" => "change the waiting settings".into(),
            _ => self.inner.describe(name, args),
        }
    }

    fn preview(&self, name: &str, args: &Value) -> Option<String> {
        match name {
            "pending_check" => Some(format!("Runs this command in the job's folder when the job ends with exit 0:\n{}\nIt must only read; its last output line is reported.", args.get("command").and_then(Value::as_str).unwrap_or(""))),
            "pending_settings" => Some(format!("Now: {:?}\nChange: {}", self.ledger.settings(), args)),
            n if is_pending_tool(n) => None,
            _ => self.inner.preview(name, args),
        }
    }

    fn refusal(&self, name: &str, args: &Value) -> Option<String> {
        if is_pending_tool(name) { None } else { self.inner.refusal(name, args) }
    }

    fn pre_approved(&self, name: &str) -> bool {
        !is_pending_tool(name) && self.inner.pre_approved(name)
    }

    fn approved_always(&self, name: &str) -> bool {
        !is_pending_tool(name) && self.inner.approved_always(name)
    }

    fn grant_blocker(&self, name: &str, args: &Value) -> Option<String> {
        if is_pending_tool(name) { crate::grants::blocker(name, args) } else { self.inner.grant_blocker(name, args) }
    }

    fn grant_scope(&self, name: &str, args: &Value) -> String {
        if is_pending_tool(name) { crate::grants::scope_of(name, args) } else { self.inner.grant_scope(name, args) }
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

    /// The job tools (inside) note a finished plain job first; then the ledger notes everything else that ended.
    fn take_notes(&self) -> Vec<String> {
        let mut v = self.inner.take_notes();
        self.ledger.sync(&self.feed);
        v.extend(self.ledger.take_notes(&self.feed.workspace));
        v
    }

    /// Running jobs (from the job tools) plus every other running ticket: tasks, waits, installs, pulls, flows, rollouts.
    fn jobs_running(&self) -> Vec<String> {
        let mut v = self.inner.jobs_running();
        self.ledger.sync(&self.feed);
        v.extend(self.ledger.running_labels(&self.feed.workspace, true));
        v
    }
}
