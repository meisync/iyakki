//! Recording and storing agent runs. Each run is a JSON transcript plus a hash-chained ledger
//! (`iyakki-ledger`) derived from it. Loading re-derives the chain from the transcript and compares it
//! with the stored ledger, so editing either file after the fact is detected.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use iyakki_ledger::{Event as LedgerEvent, Ledger};
use serde::{Deserialize, Serialize};

use crate::agent::{start, RunEvent, RunHandle, RunLimits, Tools, Verdict};
use crate::chat::{ChatBackend, Message};

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct RunLog {
    pub id: String,
    pub user: String,
    pub started: u64,
    pub prompt: String,
    /// `running`, `completed`, `failed` or `stopped`.
    pub status: String,
    pub events: Vec<RunEvent>,
    /// The chat this run belongs to, so deleting the chat can delete its runs.
    #[serde(default)]
    pub session: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RunMeta {
    pub id: String,
    pub started: u64,
    pub prompt: String,
    pub status: String,
    pub tool_calls: usize,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Integrity {
    Verified,
    Tampered(String),
}

pub fn new_run_id() -> String {
    let mut r = [0u8; 6];
    let _ = iyakki_sec::random_fill(&mut r);
    r.iter().map(|b| format!("{b:02x}")).collect()
}

fn now() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs())
}

/// The ledger that a given transcript must produce.
pub fn ledger_for(log: &RunLog) -> Ledger {
    let mut l = Ledger::new(b"iyakki-run-v1");
    l.append(LedgerEvent::Input(log.prompt.clone().into_bytes()));
    for e in &log.events {
        match e {
            RunEvent::Model { .. } | RunEvent::Approval { .. } | RunEvent::Usage { .. } | RunEvent::Final { .. } | RunEvent::Failed { .. } | RunEvent::Stopped { .. } => {
                l.append(LedgerEvent::ToolOutput { tool: "run".into(), data: serde_json::to_vec(e).unwrap_or_default() })
            }
            RunEvent::Decision { id, name, approved, by } => l.append(LedgerEvent::HostCall { name: format!("decision:{by}"), path: format!("{name}#{id}"), status: *approved as i32 }),
            RunEvent::ToolResult { id, name, ok, text } => l.append(LedgerEvent::ToolOutput { tool: format!("{name}#{id}#{}", if *ok { "ok" } else { "err" }), data: text.clone().into_bytes() }),
        }
    }
    l
}

/// A run that is going on now, for the SYSTEM tile.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LiveRun {
    pub id: String,
    pub user: String,
    /// The start of what was asked.
    pub prompt: String,
    pub started: i64,
    /// Tool calls waiting for an answer.
    pub pending: usize,
    /// Events so far.
    pub steps: usize,
    /// Tokens used so far, when the run has reported them.
    pub tokens: u64,
    /// What happened last, in a few words.
    pub last: String,
}

/// A tool call that is waiting for the person's answer, as the INBOX shows it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PendingApproval {
    pub run_id: String,
    pub user: String,
    /// The start of what the run was asked.
    pub prompt: String,
    pub call_id: String,
    pub tool: String,
    /// One line on what the call will do.
    pub summary: String,
    /// What exactly will change, when there is more to show.
    pub detail: String,
    /// `low`, `medium` or `high`.
    pub risk: &'static str,
    /// When the call started waiting.
    pub since: i64,
}

/// How risky a tool that asks first is: what it can change.
pub fn approval_risk(tool: &str) -> &'static str {
    let t = tool;
    if t.starts_with("mcp__") || ["run_command", "sandbox_run", "install_cli", "remove_cli", "write_file", "cluster_start", "cluster_stop", "http_request", "terminal"].contains(&t) || t.contains("delete") && !t.starts_with("calendar") && !t.starts_with("alarm") {
        "high"
    } else if t.starts_with("calendar_") || t.starts_with("alarm_") || (t.contains("media") && t != "media_download") || t.starts_with("task_") || t == "vault_request" {
        "low"
    } else {
        "medium"
    }
}

struct LiveEntry {
    run: LiveRun,
    cancel: Arc<std::sync::atomic::AtomicBool>,
    decisions: std::sync::mpsc::Sender<(String, Verdict)>,
    approvals: Vec<PendingApproval>,
}

type FinishHook = Box<dyn Fn(&RunLog) + Send + Sync>;

pub struct RunStore {
    dir: PathBuf,
    live: std::sync::Mutex<std::collections::BTreeMap<String, LiveEntry>>,
    on_finish: std::sync::Mutex<Option<FinishHook>>,
}

fn valid_id(id: &str) -> bool {
    id.len() == 12 && id.chars().all(|c| c.is_ascii_hexdigit())
}

impl RunStore {
    pub fn new(dir: &Path) -> RunStore {
        RunStore { dir: dir.into(), live: Default::default(), on_finish: Default::default() }
    }

    /// Called with the record of every run that ends (completed, failed or stopped), after it is saved.
    pub fn set_on_finish(&self, f: impl Fn(&RunLog) + Send + Sync + 'static) {
        *self.on_finish.lock().unwrap_or_else(|p| p.into_inner()) = Some(Box::new(f));
    }

    /// The tool calls waiting for an answer in the runs going on now, oldest first. `user` limits them to one person's
    /// runs; `None` lists everyone's (for an administrator).
    pub fn pending_approvals(&self, user: Option<&str>) -> Vec<PendingApproval> {
        let mut v: Vec<PendingApproval> = self.live_lock().values().filter(|e| user.is_none_or(|u| e.run.user == u)).flat_map(|e| e.approvals.clone()).collect();
        v.sort_by(|a, b| a.since.cmp(&b.since).then(a.call_id.cmp(&b.call_id)));
        v
    }

    /// Answer a waiting tool call from outside the run's own screen. Only the owner of the run may answer, unless
    /// `admin`. Fails when the call is not waiting (already answered, or the run ended).
    pub fn decide_pending(&self, user: &str, admin: bool, run_id: &str, call_id: &str, v: Verdict) -> Result<PendingApproval, String> {
        let mut g = self.live_lock();
        let e = g.get_mut(run_id).ok_or("that run is no longer going")?;
        if e.run.user != user && !admin {
            return Err("that approval belongs to someone else".into());
        }
        let i = e.approvals.iter().position(|a| a.call_id == call_id).ok_or("that request was already answered")?;
        e.decisions.send((call_id.to_string(), v)).map_err(|_| "the run has ended".to_string())?;
        let a = e.approvals.remove(i);
        e.run.pending = e.approvals.len();
        Ok(a)
    }

    fn live_lock(&self) -> std::sync::MutexGuard<'_, std::collections::BTreeMap<String, LiveEntry>> {
        self.live.lock().unwrap_or_else(|p| p.into_inner())
    }

    /// The runs going on now, oldest first.
    pub fn live(&self) -> Vec<LiveRun> {
        let mut v: Vec<LiveRun> = self.live_lock().values().map(|e| e.run.clone()).collect();
        v.sort_by(|a, b| a.started.cmp(&b.started).then(a.id.cmp(&b.id)));
        v
    }

    /// Stop one run; false when it is not running.
    pub fn stop_live(&self, id: &str) -> bool {
        match self.live_lock().get(id) {
            Some(e) => {
                e.cancel.store(true, std::sync::atomic::Ordering::SeqCst);
                true
            }
            None => false,
        }
    }

    /// Stop every run (or only those of `user`); returns how many were told to stop.
    pub fn stop_all_live(&self, user: Option<&str>) -> usize {
        let g = self.live_lock();
        let mut n = 0;
        for e in g.values().filter(|e| user.is_none_or(|u| e.run.user == u)) {
            e.cancel.store(true, std::sync::atomic::Ordering::SeqCst);
            n += 1;
        }
        n
    }

    fn live_update(&self, id: &str, f: impl FnOnce(&mut LiveRun)) {
        if let Some(e) = self.live_lock().get_mut(id) {
            f(&mut e.run);
        }
    }

    fn user_dir(&self, user: &str) -> Result<PathBuf, String> {
        if !iyakki_sec::policy::valid_username(user) {
            return Err("invalid user".into());
        }
        Ok(self.dir.join(user))
    }

    pub fn save(&self, log: &RunLog) -> Result<(), String> {
        use std::os::unix::fs::OpenOptionsExt;
        if !valid_id(&log.id) {
            return Err("invalid run id".into());
        }
        let d = self.user_dir(&log.user)?;
        std::fs::create_dir_all(&d).map_err(|e| e.to_string())?;
        let write = |name: String, bytes: Vec<u8>| -> Result<(), String> {
            let tmp = d.join(format!("{name}.tmp"));
            {
                use std::io::Write;
                let mut f = std::fs::OpenOptions::new().write(true).create(true).truncate(true).mode(0o600).open(&tmp).map_err(|e| e.to_string())?;
                f.write_all(&bytes).map_err(|e| e.to_string())?;
            }
            std::fs::rename(&tmp, d.join(&name)).map_err(|e| e.to_string())
        };
        write(format!("{}.json", log.id), serde_json::to_vec_pretty(log).map_err(|e| e.to_string())?)?;
        write(format!("{}.ledger", log.id), ledger_for(log).to_bytes())
    }

    /// Newest first.
    pub fn list(&self, user: &str) -> Vec<RunMeta> {
        let Ok(d) = self.user_dir(user) else { return vec![] };
        let mut v: Vec<RunMeta> = std::fs::read_dir(d)
            .into_iter()
            .flatten()
            .flatten()
            .filter(|e| e.path().extension().is_some_and(|x| x == "json"))
            .filter_map(|e| serde_json::from_slice::<RunLog>(&std::fs::read(e.path()).ok()?).ok())
            .filter(|l| valid_id(&l.id) && l.user == user)
            .map(|l| RunMeta { tool_calls: l.events.iter().filter(|e| matches!(e, RunEvent::ToolResult { .. })).count(), id: l.id, started: l.started, prompt: l.prompt, status: l.status })
            .collect();
        v.sort_by(|a, b| b.started.cmp(&a.started).then(b.id.cmp(&a.id)));
        v
    }

    pub fn load(&self, user: &str, id: &str) -> Result<(RunLog, Integrity), String> {
        if !valid_id(id) {
            return Err("invalid run id".into());
        }
        let d = self.user_dir(user)?;
        let log: RunLog = serde_json::from_slice(&std::fs::read(d.join(format!("{id}.json"))).map_err(|e| e.to_string())?).map_err(|e| format!("corrupt run file: {e}"))?;
        if log.user != user || log.id != id {
            return Ok((log, Integrity::Tampered("the transcript belongs to a different run or user".into())));
        }
        let integrity = match std::fs::read(d.join(format!("{id}.ledger"))) {
            Err(_) => Integrity::Tampered("the ledger file is missing".into()),
            Ok(bytes) => match Ledger::from_bytes(&bytes) {
                Err(e) => Integrity::Tampered(format!("the ledger is damaged: {e}")),
                Ok(stored) => {
                    let expected = ledger_for(&log);
                    if stored.entries.len() == expected.entries.len() && stored.entries.iter().zip(&expected.entries).all(|(a, b)| a.hash == b.hash) {
                        Integrity::Verified
                    } else {
                        Integrity::Tampered("the transcript was changed after the run".into())
                    }
                }
            },
        };
        Ok((log, integrity))
    }

    /// Deletes every recorded run that belongs to one chat. Returns how many.
    pub fn delete_for_session(&self, user: &str, session: &str) -> usize {
        let Ok(d) = self.user_dir(user) else { return 0 };
        let mut n = 0;
        for e in std::fs::read_dir(d).into_iter().flatten().flatten().filter(|e| e.path().extension().is_some_and(|x| x == "json")) {
            let Some(log) = std::fs::read(e.path()).ok().and_then(|b| serde_json::from_slice::<RunLog>(&b).ok()) else { continue };
            if log.user == user && log.session.as_deref() == Some(session) && valid_id(&log.id) && self.delete(user, &log.id).is_ok() {
                n += 1;
            }
        }
        n
    }

    pub fn delete(&self, user: &str, id: &str) -> Result<(), String> {
        if !valid_id(id) {
            return Err("invalid run id".into());
        }
        let d = self.user_dir(user)?;
        let _ = std::fs::remove_file(d.join(format!("{id}.ledger")));
        std::fs::remove_file(d.join(format!("{id}.json"))).map_err(|e| e.to_string())
    }
}

/// A run in progress, recording itself. Poll it from the UI loop.
pub struct RunSession {
    handle: RunHandle,
    log: RunLog,
    store: Option<Arc<RunStore>>,
    done: bool,
    /// Tool calls waiting for the person's answer.
    pending: std::collections::HashSet<String>,
}

impl RunSession {
    pub fn start(store: Option<Arc<RunStore>>, user: &str, prompt: &str, backend: Arc<dyn ChatBackend>, tools: Arc<dyn Tools>, history: Vec<Message>, limits: RunLimits) -> RunSession {
        let log = RunLog { id: new_run_id(), user: user.into(), started: now(), prompt: prompt.into(), status: "running".into(), events: vec![], session: None };
        let handle = start(backend, tools, history, limits);
        if let Some(s) = &store {
            let live = LiveRun { id: log.id.clone(), user: user.into(), prompt: prompt.chars().take(100).collect(), started: log.started as i64, pending: 0, steps: 0, tokens: 0, last: "started".into() };
            s.live_lock().insert(log.id.clone(), LiveEntry { run: live, cancel: handle.cancel_flag(), decisions: handle.decision_sender(), approvals: vec![] });
        }
        RunSession { handle, log, store, done: false, pending: Default::default() }
    }

    /// Marks the run as part of a chat.
    pub fn in_session(mut self, session: &str) -> RunSession {
        self.log.session = Some(session.to_string());
        self
    }

    pub fn id(&self) -> &str {
        &self.log.id
    }
    pub fn log(&self) -> &RunLog {
        &self.log
    }
    pub fn is_done(&self) -> bool {
        self.done
    }
    pub fn decide(&mut self, call_id: &str, v: Verdict) {
        self.handle.decide(call_id, v);
        self.pending.remove(call_id);
        if let Some(s) = &self.store {
            let n = self.pending.len();
            if let Some(e) = s.live_lock().get_mut(&self.log.id) {
                e.approvals.retain(|a| a.call_id != call_id);
                e.run.pending = n;
            }
        }
    }
    pub fn cancel(&self) {
        self.handle.cancel();
    }

    fn finish(&mut self, status: &str) {
        self.done = true;
        self.log.status = status.into();
        if let Some(s) = &self.store {
            s.live_lock().remove(&self.log.id);
        }
        if let Some(s) = &self.store {
            let _ = s.save(&self.log);
            if let Some(f) = s.on_finish.lock().unwrap_or_else(|p| p.into_inner()).as_ref() {
                f(&self.log);
            }
        }
    }

    /// New events since the last call (also recorded). The run is saved when it ends.
    pub fn poll(&mut self) -> Vec<RunEvent> {
        let mut out = Vec::new();
        while let Some(e) = self.handle.try_event() {
            let status = match &e {
                RunEvent::Final { .. } => Some("completed"),
                RunEvent::Failed { .. } => Some("failed"),
                RunEvent::Stopped { .. } => Some("stopped"),
                _ => None,
            };
            match &e {
                RunEvent::Approval { id, name, summary, detail } => {
                    self.pending.insert(id.clone());
                    if let Some(s) = &self.store {
                        let a = PendingApproval { run_id: self.log.id.clone(), user: self.log.user.clone(), prompt: self.log.prompt.chars().take(100).collect(), call_id: id.clone(), tool: name.clone(), summary: summary.clone(), detail: detail.clone(), risk: approval_risk(name), since: now() as i64 };
                        if let Some(en) = s.live_lock().get_mut(&self.log.id) {
                            en.approvals.retain(|x| x.call_id != *id);
                            en.approvals.push(a);
                        }
                    }
                }
                // answered (here, from the INBOX, or by timeout): no longer waiting
                RunEvent::Decision { id, .. } => {
                    self.pending.remove(id);
                    if let Some(s) = &self.store {
                        let n = self.pending.len();
                        if let Some(en) = s.live_lock().get_mut(&self.log.id) {
                            en.approvals.retain(|a| a.call_id != *id);
                            en.run.pending = n;
                        }
                    }
                }
                _ => {}
            }
            if let Some(store) = &self.store {
                let (pending, steps) = (self.pending.len(), self.log.events.len() + 1);
                let (last, tokens) = match &e {
                    RunEvent::Model { calls, .. } if !calls.is_empty() => ("asked for a tool".to_string(), None),
                    RunEvent::Model { .. } => ("thinking".to_string(), None),
                    RunEvent::Approval { name, .. } => (format!("waiting for approval: {name}"), None),
                    RunEvent::Decision { name, approved, .. } => (format!("{name} {}", if *approved { "approved" } else { "denied" }), None),
                    RunEvent::ToolResult { name, ok, .. } => (format!("{name} {}", if *ok { "done" } else { "failed" }), None),
                    RunEvent::Usage { tokens, .. } => ("working".to_string(), Some(*tokens)),
                    RunEvent::Final { .. } => ("finished".to_string(), None),
                    RunEvent::Failed { .. } => ("failed".to_string(), None),
                    RunEvent::Stopped { .. } => ("stopped".to_string(), None),
                };
                store.live_update(&self.log.id, |r| {
                    r.pending = pending;
                    r.steps = steps;
                    r.last = last;
                    if let Some(t) = tokens {
                        r.tokens = t;
                    }
                });
            }
            self.log.events.push(e.clone());
            out.push(e);
            if let Some(s) = status {
                self.finish(s);
                break;
            }
        }
        out
    }
}

impl Drop for RunSession {
    fn drop(&mut self) {
        if !self.done {
            // the user left mid-run: stop it and keep what happened so far
            self.handle.cancel();
            self.log.events.push(RunEvent::Stopped { reason: "the chat was closed while the run was active".into() });
            self.finish("stopped");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn tmp(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("iyakki-runs-{}-{}", std::process::id(), tag));
        let _ = std::fs::remove_dir_all(&d);
        d
    }

    fn sample(user: &str, id: &str, started: u64) -> RunLog {
        RunLog {
            id: id.into(),
            session: None,
            user: user.into(),
            started,
            prompt: "summarise notes.txt".into(),
            status: "completed".into(),
            events: vec![
                RunEvent::Model { text: "I'll read it.".into(), calls: vec![crate::agent::CallInfo { id: "c1".into(), name: "read_file".into(), args: json!({"path": "notes.txt"}) }] },
                RunEvent::Decision { id: "c1".into(), name: "read_file".into(), approved: true, by: "auto".into() },
                RunEvent::ToolResult { id: "c1".into(), name: "read_file".into(), ok: true, text: "meeting at 3pm".into() },
                RunEvent::Final { text: "There is a meeting at 3pm.".into() },
            ],
        }
    }

    #[test]
    fn save_list_load_and_verify() {
        let dir = tmp("a");
        let s = RunStore::new(&dir);
        s.save(&sample("amy", "aaaaaaaaaaaa", 100)).unwrap();
        s.save(&sample("amy", "bbbbbbbbbbbb", 200)).unwrap();
        s.save(&sample("bob", "cccccccccccc", 300)).unwrap();
        let l = s.list("amy");
        assert_eq!(l.iter().map(|m| m.id.as_str()).collect::<Vec<_>>(), vec!["bbbbbbbbbbbb", "aaaaaaaaaaaa"], "newest first, only amy's runs");
        assert_eq!((l[0].tool_calls, l[0].status.as_str()), (1, "completed"));
        let (log, ok) = s.load("amy", "aaaaaaaaaaaa").unwrap();
        assert_eq!((ok, log.events.len()), (Integrity::Verified, 4));
        assert!(s.list("nobody").is_empty());
        use std::os::unix::fs::PermissionsExt;
        let p = dir.join("amy/aaaaaaaaaaaa.json");
        assert_eq!(std::fs::metadata(p).unwrap().permissions().mode() & 0o777, 0o600);
        s.delete("amy", "aaaaaaaaaaaa").unwrap();
        assert_eq!(s.list("amy").len(), 1);
    }

    #[test]
    fn editing_the_transcript_or_the_ledger_is_detected() {
        let dir = tmp("b");
        let s = RunStore::new(&dir);
        s.save(&sample("amy", "aaaaaaaaaaaa", 1)).unwrap();
        // 1. change what the tool returned
        let p = dir.join("amy/aaaaaaaaaaaa.json");
        let text = std::fs::read_to_string(&p).unwrap().replace("meeting at 3pm", "wire money to eve");
        std::fs::write(&p, &text).unwrap();
        assert!(matches!(s.load("amy", "aaaaaaaaaaaa").unwrap().1, Integrity::Tampered(ref m) if m.contains("changed")));
        // 2. restore it, then corrupt the ledger
        s.save(&sample("amy", "aaaaaaaaaaaa", 1)).unwrap();
        let lp = dir.join("amy/aaaaaaaaaaaa.ledger");
        let mut b = std::fs::read(&lp).unwrap();
        let n = b.len();
        b[n / 2] ^= 0xff;
        std::fs::write(&lp, b).unwrap();
        assert!(matches!(s.load("amy", "aaaaaaaaaaaa").unwrap().1, Integrity::Tampered(_)));
        // 3. delete the ledger
        std::fs::remove_file(&lp).unwrap();
        assert!(matches!(s.load("amy", "aaaaaaaaaaaa").unwrap().1, Integrity::Tampered(ref m) if m.contains("missing")));
        // 4. a transcript copied into another user's folder is not accepted as theirs
        s.save(&sample("amy", "aaaaaaaaaaaa", 1)).unwrap();
        std::fs::create_dir_all(dir.join("eve")).unwrap();
        std::fs::copy(dir.join("amy/aaaaaaaaaaaa.json"), dir.join("eve/aaaaaaaaaaaa.json")).unwrap();
        assert!(matches!(s.load("eve", "aaaaaaaaaaaa").unwrap().1, Integrity::Tampered(_)));
        assert!(s.list("eve").is_empty(), "and it is not listed for them either");
    }

    #[test]
    fn ids_and_users_are_validated_against_path_tricks() {
        let s = RunStore::new(&tmp("c"));
        for bad in ["../../etc/passwd", "short", "zzzzzzzzzzzz", "aaaaaaaaaaaa/../x", ""] {
            assert!(s.load("amy", bad).is_err(), "{bad}");
            assert!(s.delete("amy", bad).is_err());
        }
        assert!(s.load("../root", "aaaaaaaaaaaa").is_err());
        let mut l = sample("amy", "aaaaaaaaaaaa", 1);
        l.user = "../x".into();
        assert!(s.save(&l).is_err());
        assert!(s.list("../x").is_empty());
    }

    #[test]
    fn ledger_is_deterministic_and_covers_every_event() {
        let l = sample("amy", "aaaaaaaaaaaa", 1);
        let (a, b) = (ledger_for(&l), ledger_for(&l));
        assert_eq!(a, b);
        assert_eq!(a.entries.len(), 1 + l.events.len());
        a.verify().unwrap();
        let mut changed = l.clone();
        changed.prompt = "other".into();
        assert_ne!(ledger_for(&changed).entries[0].hash, a.entries[0].hash);
    }

    // ------------------------------------------------------------------ runs going on now

    struct Stuck;
    impl crate::chat::ChatBackend for Stuck {
        fn complete(&self, _: &[crate::chat::Message]) -> Result<String, String> {
            Err("unused".into())
        }
        fn complete_tools(&self, _: &[crate::chat::Message], _: &[crate::chat::ToolSpec]) -> Result<crate::chat::ModelTurn, String> {
            Ok(crate::chat::ModelTurn { text: String::new(), calls: vec![crate::chat::ToolCall { id: "c1".into(), name: "poke".into(), args: json!({}) }] })
        }
    }
    struct PokeTools;
    impl crate::agent::Tools for PokeTools {
        fn defs(&self) -> Vec<crate::agent::ToolDef> {
            vec![crate::agent::ToolDef { spec: crate::chat::ToolSpec { name: "poke".into(), description: "poke".into(), schema: json!({"type": "object"}) }, risk: crate::agent::Risk::Ask, untrusted: false }]
        }
        fn call(&self, _: &str, _: &serde_json::Value) -> Result<String, String> {
            Ok("ok".into())
        }
        fn describe(&self, name: &str, _: &serde_json::Value) -> String {
            name.into()
        }
    }

    fn until(mut f: impl FnMut() -> bool) {
        for _ in 0..400 {
            if f() {
                return;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        panic!("it never happened");
    }

    #[test]
    fn a_running_run_is_listed_with_what_it_waits_for_and_can_be_stopped_from_outside() {
        let store = Arc::new(RunStore::new(&tmp("live")));
        let mut a = RunSession::start(Some(store.clone()), "ann", "tidy my notes please", Arc::new(Stuck), Arc::new(PokeTools), vec![], crate::agent::RunLimits::default());
        let mut b = RunSession::start(Some(store.clone()), "bob", "another one", Arc::new(Stuck), Arc::new(PokeTools), vec![], crate::agent::RunLimits::default());
        assert_eq!(store.live().len(), 2);
        // the run asks for approval and waits
        until(|| {
            a.poll();
            b.poll();
            store.live().iter().all(|r| r.pending == 1)
        });
        let live = store.live();
        let mine = live.iter().find(|r| r.user == "ann").unwrap();
        assert_eq!((mine.prompt.as_str(), mine.pending), ("tidy my notes please", 1));
        assert!(mine.last.contains("waiting for approval: poke") && mine.steps >= 2, "{mine:?}");
        // answering clears the count
        a.decide("c1", crate::agent::Verdict::Deny);
        assert_eq!(store.live().iter().find(|r| r.user == "ann").unwrap().pending, 0);
        // stop only bob's runs from outside
        assert_eq!(store.stop_all_live(Some("nobody")), 0);
        assert_eq!(store.stop_all_live(Some("bob")), 1);
        until(|| {
            b.poll();
            b.is_done()
        });
        assert_eq!(b.log().status, "stopped");
        assert_eq!(store.live().iter().map(|r| r.user.as_str()).collect::<Vec<_>>(), vec!["ann"], "a finished run leaves the list");
        assert!(store.stop_live(&store.live()[0].id));
        assert!(!store.stop_live("nope"));
        until(|| {
            a.poll();
            a.is_done()
        });
        assert!(store.live().is_empty());
        // dropping a run that is still going also removes it
        let c = RunSession::start(Some(store.clone()), "cy", "x", Arc::new(Stuck), Arc::new(PokeTools), vec![], crate::agent::RunLimits::default());
        assert_eq!(store.live().len(), 1);
        drop(c);
        assert!(store.live().is_empty());
    }

    // ------------------------------------------------------------------ approvals answered from outside

    /// First turn asks for the `poke` tool, every later turn answers.
    struct Script(std::sync::atomic::AtomicUsize);
    impl crate::chat::ChatBackend for Script {
        fn complete(&self, _: &[crate::chat::Message]) -> Result<String, String> {
            Err("unused".into())
        }
        fn complete_tools(&self, _: &[crate::chat::Message], _: &[crate::chat::ToolSpec]) -> Result<crate::chat::ModelTurn, String> {
            if self.0.fetch_add(1, std::sync::atomic::Ordering::SeqCst) == 0 {
                Ok(crate::chat::ModelTurn { text: String::new(), calls: vec![crate::chat::ToolCall { id: "c1".into(), name: "poke".into(), args: json!({}) }] })
            } else {
                Ok(crate::chat::ModelTurn { text: "all done".into(), calls: vec![] })
            }
        }
    }

    fn scripted(store: &Arc<RunStore>, user: &str) -> RunSession {
        RunSession::start(Some(store.clone()), user, "tidy my notes", Arc::new(Script(Default::default())), Arc::new(PokeTools), vec![], crate::agent::RunLimits::default())
    }

    #[test]
    fn a_waiting_approval_is_listed_and_can_be_approved_or_rejected_from_outside_the_run() {
        let store = Arc::new(RunStore::new(&tmp("appr")));
        let finished = Arc::new(std::sync::Mutex::new(Vec::<(String, String)>::new()));
        let f2 = finished.clone();
        store.set_on_finish(move |log| f2.lock().unwrap().push((log.user.clone(), log.status.clone())));
        let mut a = scripted(&store, "ann");
        let mut b = scripted(&store, "bob");
        let mut seen_a = vec![];
        until(|| {
            seen_a.extend(a.poll());
            b.poll();
            store.pending_approvals(None).len() == 2
        });
        // each person sees only their own; the read model says what, why, how risky and since when
        let mine = store.pending_approvals(Some("ann"));
        assert_eq!(mine.len(), 1);
        let p = &mine[0];
        assert_eq!((p.user.as_str(), p.tool.as_str(), p.call_id.as_str(), p.summary.as_str(), p.prompt.as_str(), p.risk), ("ann", "poke", "c1", "poke", "tidy my notes", "medium"));
        assert!(p.since > 1_600_000_000 && p.run_id == a.id());
        // somebody else cannot answer it; a made-up run or call fails cleanly
        assert!(store.decide_pending("bob", false, &p.run_id, "c1", crate::agent::Verdict::Approve).unwrap_err().contains("someone else"));
        assert!(store.decide_pending("ann", false, "nope", "c1", crate::agent::Verdict::Approve).is_err());
        assert!(store.decide_pending("ann", false, &p.run_id, "zzz", crate::agent::Verdict::Approve).is_err());
        assert_eq!(store.pending_approvals(Some("ann")).len(), 1, "failed attempts change nothing");
        // approving from outside: the run carries on and finishes, the owner's session sees the decision
        store.decide_pending("ann", false, &p.run_id, "c1", crate::agent::Verdict::Approve).unwrap();
        assert!(store.pending_approvals(Some("ann")).is_empty());
        assert!(store.decide_pending("ann", false, &p.run_id, "c1", crate::agent::Verdict::Approve).unwrap_err().contains("already answered"), "answering twice is refused");
        until(|| {
            seen_a.extend(a.poll());
            a.is_done()
        });
        assert!(seen_a.iter().any(|e| matches!(e, RunEvent::Decision { approved: true, by, .. } if by == "user")));
        assert!(seen_a.iter().any(|e| matches!(e, RunEvent::ToolResult { ok: true, .. })));
        assert_eq!(a.log().status, "completed");
        // rejecting bob's: the tool is not run
        let pb = store.pending_approvals(Some("bob"))[0].clone();
        store.decide_pending("bob", false, &pb.run_id, &pb.call_id, crate::agent::Verdict::Deny).unwrap();
        let mut seen_b = vec![];
        until(|| {
            seen_b.extend(b.poll());
            b.is_done()
        });
        assert!(seen_b.iter().any(|e| matches!(e, RunEvent::Decision { approved: false, .. })));
        assert!(!seen_b.iter().any(|e| matches!(e, RunEvent::ToolResult { ok: true, .. })));
        // an administrator may answer for another person
        let mut c = scripted(&store, "cy");
        until(|| {
            c.poll();
            store.pending_approvals(Some("cy")).len() == 1
        });
        let pc = store.pending_approvals(Some("cy"))[0].clone();
        store.decide_pending("root", true, &pc.run_id, &pc.call_id, crate::agent::Verdict::Deny).unwrap();
        until(|| {
            c.poll();
            c.is_done()
        });
        // a finished run is reported once, after it is saved
        let mut done = finished.lock().unwrap().clone();
        done.sort();
        assert_eq!(done, vec![("ann".to_string(), "completed".to_string()), ("bob".to_string(), "completed".to_string()), ("cy".to_string(), "completed".to_string())]);
        assert!(store.live().is_empty() && store.pending_approvals(None).is_empty());
    }

    #[test]
    fn answering_in_the_runs_own_session_clears_the_shared_list_too() {
        let store = Arc::new(RunStore::new(&tmp("appr2")));
        let mut a = scripted(&store, "ann");
        until(|| {
            a.poll();
            store.pending_approvals(Some("ann")).len() == 1
        });
        a.decide("c1", crate::agent::Verdict::Deny);
        assert!(store.pending_approvals(Some("ann")).is_empty());
        until(|| {
            a.poll();
            a.is_done()
        });
        assert_eq!(approval_risk("write_file"), "high");
        assert_eq!(approval_risk("run_command"), "high");
        assert_eq!(approval_risk("calendar_add"), "low");
        assert_eq!(approval_risk("pull_model"), "medium");
    }
}
