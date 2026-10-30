//! The browser session of a flow run (the QA browser nodes of `iyakki-flow::kinds::browser_qa`).
//!
//! * **One session per run**, made on the first browser node, closed when the run ends (finished, failed, stopped), and by
//!   [`BrowserSessions::drop_all`] when the program stops. The page engine is the one the IDE tab and the assistant use (the
//!   WebKit / WPE helper through [`crate::browserjobs::HeadlessSession`]); the actions are `browserqa::QaSession`, the same
//!   driver layer as the assistant's tools (`browsertools::Drv`, the job queue, the servicer).
//! * **Isolated by default**: a fresh profile folder that is thrown away. The person's own browser profile is used only when the
//!   flow says `profile: person`, and then the person is asked at every run, trusted flow or not (a scheduled run with nobody
//!   there to answer does not start).
//! * **Asked once per run** with the hosts the flow names, unless the flow is marked trusted. Hosts must pass the account's host
//!   policy (loopback addresses are allowed); after every page change the address is checked again, and a page that ends up on a
//!   host nobody approved is left (the tab goes blank) and the node fails.
//! * **Budgets**: the account's browser limits (or the flow's lower or higher settings up to 1000 actions and 180 minutes) per run.
//! * **Secrets**: values come in as text the engine resolved from the vault; every string the host returns is scrubbed of every
//!   secret the session has seen; passwords and token-like fields are removed from DOM dumps by the page script. Screenshots are
//!   pictures and can show secrets that were on the screen.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use iyakki_flow::host::*;
use iyakki_flow::retry::NodeError;
use iyakki_sec::policy::BrowserAgent;
use serde_json::{json, Value};

use super::host::AgentHost;
use crate::browserconsent::{domain_of, Answer, ConsentKind};
use crate::browserjobs::{BrowserJobs, HeadlessOptions, HeadlessSession};
use crate::browserqa::{QaSession, QaState};
use crate::qaimage::{self, Image};

/// Whatever keeps an engine alive; dropping it closes the engine.
pub type EngineKeeper = Box<dyn std::any::Any + Send>;

/// What an engine is started with.
pub struct EngineSpec {
    pub key: String,
    pub jobs: Arc<BrowserJobs>,
    pub profile: Option<PathBuf>,
    pub size: (u32, u32),
    /// Hosts the engine itself lets pages navigate to (`*` for any host).
    pub allow_hosts: Vec<String>,
}

pub type EngineFactory = Arc<dyn Fn(&EngineSpec) -> Result<EngineKeeper, String> + Send + Sync>;

const MAX_SESSIONS: usize = 3;
const MAX_ACTIONS: u32 = 1000;
const MAX_MINUTES: u32 = 180;
const INTERACTIVE: [&str; 9] = ["click", "type", "fill", "submit", "key", "hover", "select", "js", "storage"];

pub struct Session {
    key: String,
    run_id: String,
    profile: String,
    hosts: Vec<String>,
    trusted: bool,
    started: Instant,
    started_ms: i64,
    actions: AtomicU32,
    max_actions: u32,
    max_minutes: u32,
    dir: PathBuf,
    profile_dir: Option<PathBuf>,
    shots: AtomicU32,
    secrets: Mutex<Vec<String>>,
    last_url: Mutex<String>,
    preview_at: Mutex<Option<Instant>>,
    /// Nodes of parallel branches take turns on the one page.
    op_lock: Mutex<()>,
    keeper: Mutex<Option<EngineKeeper>>,
    engine: String,
    qa_state: QaState,
}

#[derive(Default)]
pub struct BrowserSessions {
    map: Mutex<HashMap<String, Arc<Session>>>,
    /// The factory for engines (None: the real helper).
    pub factory: Mutex<Option<EngineFactory>>,
    /// How long the person has to answer the question at the start of a run.
    pub ask_wait: Mutex<Option<Duration>>,
}

impl BrowserSessions {
    pub fn get(&self, run_id: &str) -> Option<Arc<Session>> {
        self.map.lock().unwrap_or_else(|e| e.into_inner()).get(run_id).cloned()
    }
    pub fn count(&self) -> usize {
        self.map.lock().unwrap_or_else(|e| e.into_inner()).len()
    }
    pub fn runs(&self) -> Vec<String> {
        self.map.lock().unwrap_or_else(|e| e.into_inner()).keys().cloned().collect()
    }
    /// Close every session (the program is stopping, or the account signed out).
    pub fn drop_all(&self) {
        let all: Vec<Arc<Session>> = self.map.lock().unwrap_or_else(|e| e.into_inner()).drain().map(|x| x.1).collect();
        for s in all {
            s.close();
        }
    }
}

impl Session {
    fn close(&self) {
        // the engine first (its thread and helper program end), then the thrown-away profile
        let k = self.keeper.lock().unwrap_or_else(|e| e.into_inner()).take();
        drop(k);
        if let Some(p) = &self.profile_dir {
            let _ = std::fs::remove_dir_all(p);
        }
    }
    pub fn dir(&self) -> &Path {
        &self.dir
    }
    pub fn actions(&self) -> u32 {
        self.actions.load(Ordering::SeqCst)
    }
}

fn loopback(host: &str) -> bool {
    matches!(host, "localhost" | "127.0.0.1" | "::1" | "[::1]") || host.ends_with(".localhost") || host.starts_with("127.")
}

fn safe_file(name: &str) -> String {
    let s: String = name.chars().map(|c| if c.is_ascii_alphanumeric() || c == '-' || c == '_' { c } else { '_' }).take(60).collect();
    if s.is_empty() { "unnamed".into() } else { s }
}

/// The address without its query and fragment (they can carry tokens).
fn plain_url(u: &str) -> String {
    u.split(['?', '#']).next().unwrap_or("").to_string()
}

fn host_of(url: &str) -> String {
    iyakki_flow::kinds::browser_qa::url_host(url)
}

fn cfg_err(m: impl Into<String>) -> NodeError {
    NodeError::config(m)
}

/// A refusal only the person can lift (trust, the host list, the account's browser setting): never handed to the fixing agent.
fn consent_err(m: impl Into<String>) -> NodeError {
    NodeError::consent(m)
}

/// An error text from the driver as a node error: timeouts and navigation trouble can be retried, the rest cannot.
fn driver_err(e: String) -> NodeError {
    if e == "cancelled" {
        return NodeError::cancelled();
    }
    let l = e.to_ascii_lowercase();
    NodeError { message: e.chars().take(600).collect(), timed_out: l.contains("timed out") || l.contains("did not answer"), navigation: l.contains("could not be opened") || l.contains("did not load") || l.contains("stayed on a blank page"), ..Default::default() }
}

fn str_arg<'a>(a: &'a Value, k: &str) -> &'a str {
    a.get(k).and_then(Value::as_str).unwrap_or("")
}

fn idx_arg(a: &Value) -> i64 {
    a.get("index").and_then(|v| v.as_i64().or_else(|| v.as_str().and_then(|s| s.trim().parse().ok()))).unwrap_or(0)
}

fn bool_arg(a: &Value, k: &str, d: bool) -> bool {
    a.get(k).and_then(Value::as_bool).unwrap_or(d)
}

fn num_arg(a: &Value, k: &str) -> Option<f64> {
    a.get(k).and_then(|v| v.as_f64().or_else(|| v.as_str().and_then(|s| s.trim().parse().ok())))
}

fn now_ms() -> i64 {
    crate::clock::now_unix() * 1000
}

impl AgentHost {
    pub fn browser_factory(&self, f: EngineFactory) {
        *self.browser_sessions.factory.lock().unwrap_or_else(|e| e.into_inner()) = Some(f);
    }

    /// Why the browser nodes cannot run for this account here (None: they can).
    fn qa_unavailable(&self) -> Option<String> {
        if self.policy.browser_agent == BrowserAgent::Off {
            return Some("browser control is off for your account (an administrator can change it in Settings, Users)".into());
        }
        let custom = self.browser_sessions.factory.lock().unwrap_or_else(|e| e.into_inner()).is_some();
        if !custom && !crate::webengine::embedded_available() {
            return Some(format!("{}: the browser nodes need the web engine's helper program", crate::webengine::NOT_INSTALLED));
        }
        None
    }

    fn flow_dir(&self, run_id: &str) -> PathBuf {
        self.data.join("artifacts").join(safe_file(run_id))
    }

    /// The question at the start of a run's browser session. Err when the person said no or did not answer.
    fn approve_session(&self, req: &BrowserOp, cancel: &Cancel) -> Result<&'static str, NodeError> {
        let person = req.profile == "person";
        if req.trusted && !person {
            return Ok("trusted");
        }
        let hosts = if req.hosts.is_empty() { "(addresses decided while the flow runs; each one must pass your host policy)".to_string() } else { req.hosts.join(", ") };
        let text = if person {
            format!("Let the flow \"{}\" use YOUR browser profile (your signed-in sessions) for this run? It may open: {hosts}. It can click, type and read pages there.", req.flow_id)
        } else {
            format!("Let the flow \"{}\" open a private browser session for this run? It may open: {hosts}. The session has no sign-ins of yours and is thrown away at the end.", req.flow_id)
        };
        let title = format!("Browser session for the flow \"{}\"", req.flow_id);
        let id = self.svc.browser.consent.create(&self.user, ConsentKind::Step, &format!("the flow {}", req.flow_id), &text, &format!("{title}. Sites: {hosts}"), crate::clock::now_unix()).map_err(|e| cfg_err(format!("the question could not be asked: {e}")))?;
        self.audit_to("flow.browser.ask", &format!("{} hosts {hosts}{}", req.run_id, if person { " (the person's profile)" } else { "" }));
        let wait = self.browser_sessions.ask_wait.lock().unwrap_or_else(|e| e.into_inner()).unwrap_or(Duration::from_secs(600));
        let svc = self.svc.clone();
        let (tx, rx) = std::sync::mpsc::channel();
        let id2 = id.clone();
        std::thread::spawn(move || {
            let _ = tx.send(svc.browser.consent.wait(&id2, wait));
        });
        let end = Instant::now() + wait + Duration::from_secs(2);
        loop {
            if cancel.is_cancelled() {
                let _ = self.svc.browser.consent.resolve(&self.user, &id, Answer::Deny);
                return Err(NodeError::cancelled());
            }
            match rx.recv_timeout(Duration::from_millis(100)) {
                Ok(Some(Answer::Allow)) | Ok(Some(Answer::AllowSession)) => {
                    self.audit_to("flow.browser.allowed", &req.run_id);
                    return Ok("asked");
                }
                Ok(Some(_)) => {
                    self.audit_to("flow.browser.denied", &req.run_id);
                    return Err(consent_err("You did not allow this flow to use a browser session, so the browser steps did not run. Mark the flow trusted in its settings to skip the question."));
                }
                Ok(None) => return Err(consent_err("Nobody answered the question about the browser session in time, so the browser steps did not run (a scheduled or webhook run needs a trusted flow). Approve it in the INBOX and run again.")),
                Err(std::sync::mpsc::RecvTimeoutError::Timeout) if Instant::now() < end => {}
                Err(_) => return Err(consent_err("the question about the browser session ended without an answer")),
            }
        }
    }

    fn start_session(&self, req: &BrowserOp, cancel: &Cancel) -> Result<Arc<Session>, NodeError> {
        if let Some(why) = self.qa_unavailable() {
            return Err(cfg_err(format!("The browser nodes cannot run: {why}.")));
        }
        {
            // sessions that outlived their run (they should not) are closed first
            let mut g = self.browser_sessions.map.lock().unwrap_or_else(|e| e.into_inner());
            let old: Vec<String> = g.iter().filter(|(_, s)| s.started.elapsed() > Duration::from_secs(MAX_MINUTES as u64 * 60 + 600)).map(|(k, _)| k.clone()).collect();
            for k in old {
                if let Some(s) = g.remove(&k) {
                    s.close();
                }
            }
            if g.len() >= MAX_SESSIONS {
                return Err(cfg_err(format!("{MAX_SESSIONS} browser sessions are open already (other flow runs): wait for one to end")));
            }
        }
        let how = self.approve_session(req, cancel)?;
        let key = format!("flowrun:{}:{}", self.user, safe_file(&req.run_id));
        let dir = self.flow_dir(&req.run_id);
        std::fs::create_dir_all(&dir).map_err(|e| cfg_err(format!("cannot make the run's picture folder: {e}")))?;
        let (profile_path, thrown_away) = if req.profile == "person" {
            (Some(crate::webengine::prepare_profile(self.svc.dir(), &self.user).map_err(cfg_err)?), None)
        } else {
            let p = self.data.join("browser-profiles").join(safe_file(&req.run_id));
            std::fs::create_dir_all(&p).map_err(|e| cfg_err(format!("cannot make a private profile: {e}")))?;
            (Some(p.clone()), Some(p))
        };
        let mut allow: Vec<String> = Vec::new();
        if req.trusted || req.hosts.is_empty() {
            if self.policy.net_hosts.iter().any(|h| h == "*") {
                allow.push("*".into());
            } else {
                allow.extend(self.policy.net_hosts.iter().cloned());
            }
        } else {
            allow.extend(req.hosts.iter().cloned());
        }
        allow.extend(["localhost".to_string(), "127.0.0.1".to_string()]);
        let spec = EngineSpec { key: key.clone(), jobs: self.svc.browser.jobs.clone(), profile: profile_path, size: (1280, 800), allow_hosts: allow };
        let factory = self.browser_sessions.factory.lock().unwrap_or_else(|e| e.into_inner()).clone();
        let (keeper, engine): (EngineKeeper, String) = match factory {
            Some(f) => (f(&spec).map_err(|e| NodeError::new(format!("the browser engine could not be started: {e}")))?, "test engine".into()),
            None => {
                let helper = crate::webengine::embedded_helper().ok_or_else(|| cfg_err(crate::webengine::NOT_INSTALLED))?;
                let env = spec.profile.as_ref().and_then(|p| crate::webengine::helper_env(p));
                let opts = HeadlessOptions { helper, profile: spec.profile.clone(), env, size: spec.size, allow_hosts: spec.allow_hosts.clone(), user_agent: crate::webengine::user_agent_suffix() };
                (Box::new(HeadlessSession::start(spec.jobs.clone(), &key, opts).map_err(|e| NodeError::new(format!("the browser engine could not be started: {e}")))?), "WebKit helper".into())
            }
        };
        let max_actions = if req.max_actions > 0 { req.max_actions.min(MAX_ACTIONS) } else { self.policy.browser_max_actions.max(1) };
        let max_minutes = if req.max_minutes > 0 { req.max_minutes.min(MAX_MINUTES) } else { self.policy.browser_max_minutes.max(1) };
        let s = Arc::new(Session {
            key,
            run_id: req.run_id.clone(),
            profile: if req.profile == "person" { "person".into() } else { "isolated".into() },
            hosts: req.hosts.clone(),
            trusted: req.trusted,
            started: Instant::now(),
            started_ms: now_ms(),
            actions: AtomicU32::new(0),
            max_actions,
            max_minutes,
            dir,
            profile_dir: thrown_away,
            shots: AtomicU32::new(0),
            secrets: Mutex::new(Vec::new()),
            last_url: Mutex::new(String::new()),
            preview_at: Mutex::new(None),
            op_lock: Mutex::new(()),
            keeper: Mutex::new(Some(keeper)),
            engine,
            qa_state: QaState::default(),
        });
        self.browser_sessions.map.lock().unwrap_or_else(|e| e.into_inner()).insert(req.run_id.clone(), s.clone());
        self.audit_to("flow.browser.start", &format!("{} {}/{} profile {} ({how}) hosts {}", req.run_id, req.flow_id, req.node_id, s.profile, if req.hosts.is_empty() { "-".to_string() } else { req.hosts.join(",") }));
        Ok(s)
    }

    /// May the session be on this address? Host policy first, then the hosts the person approved for the run.
    fn check_address(&self, s: &Session, url: &str) -> Result<(), NodeError> {
        let host = host_of(url);
        if host.is_empty() {
            return Err(cfg_err("that address has no host"));
        }
        if !loopback(&host) && !self.policy.allows_host(&host) {
            return Err(consent_err(format!("Your account may not open pages from {host} (host policy).")));
        }
        if !s.trusted && !loopback(&host) && !s.hosts.iter().any(|h| domain_of(h) == domain_of(&host)) {
            return Err(consent_err(format!("{host} is not one of the sites this run was approved for ({}). Mark the flow trusted, or give the address in an Open URL step.", if s.hosts.is_empty() { "none named".to_string() } else { s.hosts.join(", ") })));
        }
        Ok(())
    }

    fn redact(&self, s: &Session, extra: &[String]) -> impl Fn(&str) -> String {
        let mut all: Vec<String> = s.secrets.lock().unwrap_or_else(|e| e.into_inner()).clone();
        all.extend(extra.iter().cloned());
        all.retain(|x| x.chars().count() >= 3);
        all.sort_by_key(|x| std::cmp::Reverse(x.len()));
        move |t: &str| {
            let mut o = t.to_string();
            for x in &all {
                if o.contains(x.as_str()) {
                    o = o.replace(x.as_str(), "***");
                }
            }
            o
        }
    }

    fn save_png(&self, s: &Session, name: &str, img: &Image) -> Result<PathBuf, NodeError> {
        let n = s.shots.fetch_add(1, Ordering::SeqCst) + 1;
        let stamp = crate::clock::format(&crate::clock::Zone::system(), crate::clock::now_unix(), "%Y%m%d-%H%M%S");
        let path = s.dir.join(format!("{n:02}-{}-{stamp}.png", safe_file(name)));
        std::fs::write(&path, img.to_png()).map_err(|e| NodeError::new(format!("{}: {e}", path.display())))?;
        Ok(path)
    }

    /// The latest live picture of the page, for the run view (throttled).
    fn preview(&self, s: &Session, qa: &QaSession, force: bool) {
        {
            let mut g = s.preview_at.lock().unwrap_or_else(|e| e.into_inner());
            if !force && g.is_some_and(|t| t.elapsed() < Duration::from_millis(800)) {
                return;
            }
            *g = Some(Instant::now());
        }
        if let Ok(img) = qa.capture("", 0, false, &|| false) {
            let tmp = s.dir.join("preview.tmp");
            if std::fs::write(&tmp, img.to_png()).is_ok() {
                let _ = std::fs::rename(&tmp, s.dir.join("preview.png"));
            }
        }
    }

    fn baseline_path(&self, flow_id: &str, name: &str) -> PathBuf {
        self.flows_dir.join("baselines").join(safe_file(flow_id)).join(format!("{}.png", safe_file(name)))
    }

    pub fn browser_op_impl(&self, req: &BrowserOp, cancel: &Cancel) -> Result<BrowserOpResult, NodeError> {
        if cancel.is_cancelled() {
            return Err(NodeError::cancelled());
        }
        // what the account's browser setting allows
        if INTERACTIVE.contains(&req.op.as_str()) && self.policy.browser_agent != BrowserAgent::Interactive {
            return Err(consent_err(format!("`{}` acts on the page, and your account's browser setting is `{}`: an administrator can set it to `interactive` (Settings, Users).", req.op, self.policy.browser_agent.name())));
        }
        if req.op == "js" && !(self.policy.manage_settings && self.policy.browser_agent == BrowserAgent::Interactive) {
            return Err(consent_err("Run JavaScript is for administrators only (it needs the permission to change settings and an interactive browser setting)."));
        }
        let session = match self.browser_sessions.get(&req.run_id) {
            Some(s) => s,
            None => {
                if req.op != "open" && req.op != "viewport" {
                    // not an error to start with another op, but nothing is open: the node will say so
                }
                self.start_session(req, cancel)?
            }
        };
        let _turn = session.op_lock.lock().unwrap_or_else(|e| e.into_inner());
        if cancel.is_cancelled() {
            return Err(NodeError::cancelled());
        }
        // budgets
        let used = session.actions.fetch_add(1, Ordering::SeqCst) + 1;
        if used > session.max_actions {
            return Err(cfg_err(format!("The run used all {} browser actions it may have (the flow's browser settings, or your account's limit).", session.max_actions)));
        }
        if session.started.elapsed() > Duration::from_secs(session.max_minutes as u64 * 60) {
            return Err(cfg_err(format!("The run's browser session reached its time limit of {} minutes.", session.max_minutes)));
        }
        if !req.secrets.is_empty() {
            let mut g = session.secrets.lock().unwrap_or_else(|e| e.into_inner());
            for x in &req.secrets {
                if !g.contains(x) {
                    g.push(x.clone());
                }
            }
        }
        let red = self.redact(&session, &req.secrets);
        let qa = QaSession::new(&self.svc.browser, &session.key, &session.qa_state);
        let a = &req.args;
        let timeout = Duration::from_millis(req.timeout_ms.max(1000));
        let cancelled = || cancel.is_cancelled();
        let mut log: Vec<String> = Vec::new();
        let mut artifacts: Vec<String> = Vec::new();
        self.audit_to("flow.browser.op", &format!("{}/{} {} {}", req.run_id, req.node_id, req.op, if req.op == "open" { host_of(str_arg(a, "url")) } else { host_of(&session.last_url.lock().unwrap_or_else(|e| e.into_inner())) }));
        let value: Value = match req.op.as_str() {
            "open" => {
                let url = str_arg(a, "url").trim().to_string();
                if !(url.starts_with("http://") || url.starts_with("https://")) {
                    return Err(cfg_err("only http and https addresses can be opened"));
                }
                if url.len() > 4096 || url.chars().any(|c| c.is_control() || c.is_whitespace()) {
                    return Err(cfg_err("that address cannot be opened"));
                }
                let rest = url.split("://").nth(1).unwrap_or("");
                if rest.split(['/', '?', '#']).next().unwrap_or("").contains('@') {
                    return Err(cfg_err("no user name or password in an address: type them into the sign-in form (use vault:NAME)"));
                }
                self.check_address(&session, &url)?;
                let wait = if str_arg(a, "wait").is_empty() { "idle" } else { str_arg(a, "wait") };
                let v = qa.open(&url, wait, timeout, &cancelled).map_err(driver_err)?;
                // a redirect may have taken the page somewhere else
                let landed = v["url"].as_str().unwrap_or("").to_string();
                if let Err(e) = self.check_address(&session, &landed) {
                    let _ = qa.open("about:blank", "none", Duration::from_secs(3), &|| false);
                    return Err(NodeError::config(format!("The page redirected to {}: {}", host_of(&landed), e.message)));
                }
                *session.last_url.lock().unwrap_or_else(|e| e.into_inner()) = landed.clone();
                log.push(format!("opened {}", plain_url(&landed)));
                json!({"url": landed, "title": v["title"], "status": v["status"], "loaded": v["loaded"]})
            }
            "screenshot" => {
                let img = qa.capture(str_arg(a, "selector"), idx_arg(a), bool_arg(a, "full_page", false), &cancelled).map_err(driver_err)?;
                let name = if str_arg(a, "name").is_empty() { "screenshot" } else { str_arg(a, "name") };
                let path = self.save_png(&session, name, &img)?;
                artifacts.push(path.display().to_string());
                let url = qa.state().ok().and_then(|v| v["url"].as_str().map(plain_url)).unwrap_or_default();
                log.push(format!("picture {}x{} saved as {}", img.w, img.h, path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default()));
                json!({"path": path.display().to_string(), "name": name, "width": img.w, "height": img.h, "bytes": std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0), "taken_ms": now_ms(), "url": url, "note": "a screenshot shows whatever was on the screen, including text typed in clear"})
            }
            "dom" => {
                let max = num_arg(a, "max_bytes").unwrap_or(200_000.0).clamp(1000.0, 2_000_000.0) as u64;
                let args = json!({"mode": if str_arg(a, "mode").is_empty() { "page" } else { str_arg(a, "mode") }, "selector": str_arg(a, "selector"), "index": idx_arg(a), "max": max, "links": bool_arg(a, "links", false), "failures": bool_arg(a, "failures", false), "console": bool_arg(a, "console", true)});
                let mut v = qa.dom(&args, &red).map_err(driver_err)?;
                if let Value::Object(m) = &mut v {
                    let n = m.get("console").and_then(Value::as_array).map_or(0, Vec::len);
                    log.push(format!("read the page ({} console messages)", n));
                }
                v
            }
            "assert" => {
                let check = str_arg(a, "check");
                let expected = match a.get("expected") {
                    Some(Value::String(t)) => t.clone(),
                    Some(Value::Null) | None => String::new(),
                    Some(other) => other.to_string(),
                };
                let wait = Duration::from_millis(num_arg(a, "wait_ms").unwrap_or(2000.0).clamp(0.0, 60_000.0) as u64).min(timeout);
                let (passed, message) = qa.check(check, str_arg(a, "selector"), idx_arg(a), &expected, str_arg(a, "attribute"), wait, &cancelled).map_err(driver_err)?;
                json!({"check": check, "selector": str_arg(a, "selector"), "passed": passed, "message": red(&message)})
            }
            "click" => qa.click(str_arg(a, "selector"), idx_arg(a), bool_arg(a, "double", false), bool_arg(a, "native", false), &cancelled).map_err(driver_err)?,
            "type" => {
                let text = str_arg(a, "text");
                let v = qa.type_text(str_arg(a, "selector"), idx_arg(a), text, bool_arg(a, "clear", true), bool_arg(a, "native", false), bool_arg(a, "press_enter", false), &cancelled).map_err(|e| driver_err(red(&e)))?;
                log.push(format!("typed {} characters into {}", text.chars().count(), str_arg(a, "selector")));
                v
            }
            "fill" => {
                let Some(fields) = a.get("fields").and_then(Value::as_object) else { return Err(cfg_err("fields must be a map of selector to value")) };
                let file_ok = |p: &str| -> Result<(), String> {
                    if p.is_empty() {
                        return Err("a file input needs a path".into());
                    }
                    let path = Path::new(p);
                    crate::scope::check_read(&self.policy, &self.user, path)
                };
                let mut v = qa.fill(fields, &file_ok, &cancelled).map_err(|e| driver_err(red(&e)))?;
                log.push(format!("filled {} fields", fields.len()));
                let sub = str_arg(a, "submit");
                if !sub.is_empty() {
                    qa.submit(sub, 0, &cancelled).map_err(driver_err)?;
                    v["submitted"] = json!(sub);
                }
                v
            }
            "submit" => qa.submit(str_arg(a, "selector"), idx_arg(a), &cancelled).map_err(driver_err)?,
            "key" => qa.key(str_arg(a, "key"), str_arg(a, "selector"), idx_arg(a), &cancelled).map_err(driver_err)?,
            "hover" => qa.hover(str_arg(a, "selector"), idx_arg(a), &cancelled).map_err(driver_err)?,
            "scroll" => qa.scroll(&json!({"to": if str_arg(a, "to").is_empty() { "selector" } else { str_arg(a, "to") }, "selector": str_arg(a, "selector"), "index": idx_arg(a), "pixels": num_arg(a, "pixels")})).map_err(driver_err)?,
            "select" => qa.select(str_arg(a, "selector"), idx_arg(a), str_arg(a, "option"), if str_arg(a, "by").is_empty() { "auto" } else { str_arg(a, "by") }, &cancelled).map_err(driver_err)?,
            "wait" => {
                let what = if str_arg(a, "for").is_empty() { "selector" } else { str_arg(a, "for") };
                qa.wait_for(what, str_arg(a, "value"), num_arg(a, "ms").unwrap_or(0.0) as u64, timeout, &cancelled).map_err(driver_err)?
            }
            "extract" => {
                let mut args = json!({});
                if let Some(f) = a.get("fields").filter(|f| f.is_object()) {
                    args["fields"] = f.clone();
                }
                if let Some(r) = a.get("rows").filter(|f| f.is_object()) {
                    args["rows"] = r.clone();
                }
                let v = qa.extract(&args).map_err(driver_err)?;
                let data = v.get("data").cloned().unwrap_or(json!({}));
                let mut out = json!({"data": data});
                if let Some(r) = v.get("rows") {
                    out["rows"] = r.clone();
                }
                if let Value::Object(m) = &data {
                    // the extracted values are also top-level fields, so `{{ nodes.x.output.price }}` works
                    for (k, x) in m {
                        if !matches!(k.as_str(), "data" | "rows") {
                            out[k] = x.clone();
                        }
                    }
                }
                out
            }
            "js" => {
                let expr = str_arg(a, "expression");
                let v = qa.run_js(expr, a.get("args").unwrap_or(&Value::Null), timeout).map_err(|e| driver_err(red(&e)))?;
                let text = v.to_string();
                if text.len() > 200_000 {
                    return Err(NodeError::new("the expression returned more than 200 KB"));
                }
                json!({"value": v})
            }
            "frame" => qa.frame_action(if str_arg(a, "action").is_empty() { "frame" } else { str_arg(a, "action") }, str_arg(a, "selector")).map_err(driver_err)?,
            "viewport" => {
                let (w, h) = match str_arg(a, "preset") {
                    "phone 375x667" => (375, 667),
                    "tablet 768x1024" => (768, 1024),
                    "laptop 1280x800" => (1280, 800),
                    "desktop 1920x1080" => (1920, 1080),
                    _ => (num_arg(a, "width").unwrap_or(1280.0) as u32, num_arg(a, "height").unwrap_or(800.0) as u32),
                };
                if !(200..=1920).contains(&w) || !(200..=1200).contains(&h) {
                    return Err(cfg_err("the viewport must be 200 to 1920 pixels wide and 200 to 1200 high (the engine's picture buffer)"));
                }
                qa.set_viewport(w, h, &cancelled).map_err(driver_err)?
            }
            "storage" => {
                let kind = if str_arg(a, "kind").is_empty() { "cookie" } else { str_arg(a, "kind") };
                let path = if str_arg(a, "path").is_empty() { "/" } else { str_arg(a, "path") };
                qa.set_storage(kind, str_arg(a, "name"), str_arg(a, "value"), path).map_err(driver_err)?
            }
            "download" => {
                let want = str_arg(a, "name_contains").to_string();
                let end = Instant::now() + timeout;
                let found = loop {
                    if cancel.is_cancelled() {
                        return Err(NodeError::cancelled());
                    }
                    let list = qa.downloads().map_err(driver_err)?;
                    if let Some(d) = list.into_iter().find(|d| want.is_empty() || d.1.contains(&want)) {
                        break d;
                    }
                    if Instant::now() >= end {
                        return Err(NodeError::timeout(format!("no download started within {:.0} s{}", timeout.as_secs_f64(), if want.is_empty() { String::new() } else { format!(" with a name containing \"{want}\"") })));
                    }
                    std::thread::sleep(Duration::from_millis(100));
                };
                self.save_download(&session, req, &found.0, &found.1, str_arg(a, "save_in"), cancel, &mut artifacts, &mut log)?
            }
            "compare" => self.compare(&session, &qa, req, &mut artifacts, &mut log, &cancelled)?,
            other => return Err(cfg_err(format!("`{other}` is not a browser action"))),
        };
        // the address after the action: acting may have moved the page, and the host policy still holds
        if !matches!(req.op.as_str(), "open" | "viewport" | "frame") {
            if let Ok(st) = qa.state() {
                let u = st.get("url").and_then(Value::as_str).unwrap_or("").to_string();
                if !u.is_empty() && !u.starts_with("about:") {
                    if let Err(e) = self.check_address(&session, &u) {
                        let _ = qa.open("about:blank", "none", Duration::from_secs(3), &|| false);
                        return Err(NodeError::config(format!("The page moved to {}, which this run may not use: {}", host_of(&u), e.message)));
                    }
                    *session.last_url.lock().unwrap_or_else(|e| e.into_inner()) = u;
                }
            }
        }
        self.preview(&session, &qa, req.op == "open");
        let value = redact_json(&value, &red);
        Ok(BrowserOpResult { value, artifacts, log })
    }

    #[allow(clippy::too_many_arguments)]
    pub(super) fn save_download(&self, s: &Session, req: &BrowserOp, url: &str, name: &str, save_in: &str, cancel: &Cancel, artifacts: &mut Vec<String>, log: &mut Vec<String>) -> Result<Value, NodeError> {
        let host = host_of(url);
        if !loopback(&host) && !self.policy.allows_host(&host) {
            return Err(cfg_err(format!("Your account may not download from {host}.")));
        }
        let base = Path::new(name).file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        let base = if base.is_empty() || base.starts_with('.') { "download".to_string() } else { base };
        let folder = if save_in.is_empty() { self.data.join("work").join(safe_file(&req.flow_id)).join("downloads") } else { PathBuf::from(save_in) };
        let folder = if save_in.is_empty() { folder } else { crate::scope::resolve(&self.data, save_in).map_err(cfg_err)? };
        std::fs::create_dir_all(&folder).map_err(|e| cfg_err(format!("{}: {e}", folder.display())))?;
        let target = folder.join(&base);
        // downloads go only where the person may write (the flow's own work folder is always allowed)
        let inside_work = std::fs::canonicalize(&folder).ok().is_some_and(|f| std::fs::canonicalize(self.data.join("work")).ok().is_some_and(|w| f.starts_with(w)));
        if !inside_work {
            crate::scope::check_write(&self.policy, &self.user, &target).map_err(cfg_err)?;
        }
        let up = (self.upstream)(&iyakki_proxy::https::HttpOptions { timeout: Duration::from_secs(60), ..Default::default() }).map_err(cfg_err)?;
        if cancel.is_cancelled() {
            return Err(NodeError::cancelled());
        }
        let (resp, _) = up.request_full("GET", url, &[], &[]).map_err(|e| NodeError::new(format!("the download could not be fetched: {e}")))?;
        if resp.status >= 400 {
            return Err(NodeError::new(format!("the download answered {}", resp.status)));
        }
        if resp.body.len() > 100 << 20 {
            return Err(NodeError::new("the download is larger than 100 MB"));
        }
        std::fs::write(&target, &resp.body).map_err(|e| NodeError::new(format!("{}: {e}", target.display())))?;
        let _ = s;
        artifacts.push(target.display().to_string());
        log.push(format!("saved {} ({} bytes)", base, resp.body.len()));
        Ok(json!({"name": base, "path": target.display().to_string(), "bytes": resp.body.len(), "url": plain_url(url)}))
    }

    fn compare(&self, s: &Session, qa: &QaSession, req: &BrowserOp, artifacts: &mut Vec<String>, log: &mut Vec<String>, cancelled: &dyn Fn() -> bool) -> Result<Value, NodeError> {
        let a = &req.args;
        let name = str_arg(a, "name");
        let img = qa.capture(str_arg(a, "selector"), idx_arg(a), false, cancelled).map_err(driver_err)?;
        let base_path = self.baseline_path(&req.flow_id, name);
        let update = bool_arg(a, "update", false);
        let threshold = num_arg(a, "threshold").unwrap_or(0.5).clamp(0.0, 100.0);
        let tolerance = num_arg(a, "tolerance").unwrap_or(8.0).clamp(0.0, 255.0) as u8;
        let current = self.save_png(s, &format!("{name}-now"), &img)?;
        artifacts.push(current.display().to_string());
        if update || !base_path.is_file() {
            if let Some(p) = base_path.parent() {
                std::fs::create_dir_all(p).map_err(|e| NodeError::new(format!("{}: {e}", p.display())))?;
            }
            std::fs::write(&base_path, img.to_png()).map_err(|e| NodeError::new(format!("{}: {e}", base_path.display())))?;
            let msg = if update { format!("the baseline \"{name}\" was replaced ({}x{})", img.w, img.h) } else { format!("no baseline \"{name}\" yet: this picture ({}x{}) was saved as the baseline", img.w, img.h) };
            log.push(msg.clone());
            return Ok(json!({"check": "baseline", "passed": true, "message": msg, "baseline_created": true, "baseline": base_path.display().to_string(), "current": current.display().to_string(), "percent": 0.0, "changed": 0, "total": img.w as u64 * img.h as u64, "threshold": threshold}));
        }
        let base_bytes = std::fs::read(&base_path).map_err(|e| NodeError::new(format!("{}: {e}", base_path.display())))?;
        let base = qaimage::decode_png(&base_bytes).map_err(|e| cfg_err(format!("the baseline \"{name}\" cannot be read: {e}")))?;
        if base.w != img.w || base.h != img.h {
            let msg = format!("the picture is {}x{} but the baseline \"{name}\" is {}x{}: set the same viewport, or replace the baseline", img.w, img.h, base.w, base.h);
            return Ok(json!({"check": "baseline", "passed": false, "message": msg, "baseline": base_path.display().to_string(), "current": current.display().to_string(), "size_changed": true, "threshold": threshold}));
        }
        let d = qaimage::diff(&base, &img, tolerance).map_err(cfg_err)?;
        let passed = d.percent <= threshold;
        let mut diff_path = Value::Null;
        if d.changed > 0 {
            let p = self.save_png(s, &format!("{name}-diff"), &d.image)?;
            artifacts.push(p.display().to_string());
            diff_path = json!(p.display().to_string());
        }
        let msg = if passed { format!("\"{name}\" matches its baseline: {:.3}% of pixels changed (allowed {threshold}%)", d.percent) } else { format!("\"{name}\" differs from its baseline: {:.3}% of {} pixels changed (allowed {threshold}%)", d.percent, d.total) };
        log.push(msg.clone());
        Ok(json!({"check": "baseline", "passed": passed, "message": msg, "percent": d.percent, "changed": d.changed, "total": d.total, "threshold": threshold, "tolerance": tolerance, "baseline": base_path.display().to_string(), "current": current.display().to_string(), "diff": diff_path}))
    }

    pub fn browser_session_info_impl(&self, run_id: &str) -> Option<BrowserSessionInfo> {
        let s = self.browser_sessions.get(run_id)?;
        let preview = s.dir.join("preview.png");
        let url = plain_url(&s.last_url.lock().unwrap_or_else(|e| e.into_inner()));
        Some(BrowserSessionInfo {
            run_id: s.run_id.clone(),
            hosts: s.hosts.clone(),
            profile: s.profile.clone(),
            actions: s.actions(),
            started_ms: s.started_ms,
            url,
            preview: preview.is_file().then(|| preview.display().to_string()),
            engine: s.engine.clone(),
        })
    }

    pub fn browser_end_impl(&self, run_id: &str) {
        let s = self.browser_sessions.map.lock().unwrap_or_else(|e| e.into_inner()).remove(run_id);
        if let Some(s) = s {
            let secs = s.started.elapsed().as_secs();
            s.close();
            self.audit_to("flow.browser.end", &format!("{} {} actions, {} s", s.run_id, s.actions(), secs));
        }
    }
}

fn redact_json(v: &Value, f: &dyn Fn(&str) -> String) -> Value {
    match v {
        Value::String(s) => Value::String(f(s)),
        Value::Array(a) => Value::Array(a.iter().map(|x| redact_json(x, f)).collect()),
        Value::Object(o) => Value::Object(o.iter().map(|(k, x)| (k.clone(), redact_json(x, f))).collect()),
        other => other.clone(),
    }
}
