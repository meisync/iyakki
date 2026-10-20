//! What the person has allowed the assistant to do in their browser, and how it asks (docs/BROWSER-AGENT.md).
//!
//! * **Looking** at a host (opening it, snapshots, screenshots) asks once per host per session: *Allow* / *Deny*.
//! * **Interacting** (click, type, select, submit) on a domain asks once per domain per session with exactly three answers:
//!   *Approve all actions on D for this session*, *Approve one by one*, *Deny*. "Approve all" lasts until sign-out, 30 minutes
//!   without use, the person pressing Stop or Take over, or the end of the run, and never reaches another domain.
//! * **Always asked, also under "approve all"**: payments, destructive account actions, and anything the injection heuristic
//!   flags. Passwords, one-time codes and captchas are never typed by the assistant at all.
//!
//! The requests are held here ([`ConsentStore`]); the INBOX (Approvals page) and the open chat show them and resolve them. The
//! waiting tool gets only the [`Answer`].

use std::collections::{HashMap, VecDeque};
use std::sync::atomic::{AtomicI64, AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::time::Duration;

use crate::browserjobs::BrowserJobs;

pub const MAX_PENDING: usize = 10;
/// A plain "Allow" to look at a host lasts this long.
pub const LOOK_SHORT_SECS: i64 = 5 * 60;
/// "Approve all" is dropped after this many seconds without an action.
pub const IDLE_SECS: i64 = 30 * 60;
/// Defaults of the per-run budget (the account policy can change them).
pub const MAX_ACTIONS: u32 = 60;
pub const MAX_MINUTES: u32 = 20;
/// At most this many actions in any 10 seconds.
pub const RATE_PER_10S: usize = 20;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ConsentKind {
    /// May the assistant look at this host in your browser?
    Look,
    /// May the assistant act on your behalf on this domain?
    Interact,
    /// One action to approve (one-by-one mode, payments, destructive actions, suspicious ones).
    Step,
    /// The assistant needs the person to do something (a code, a captcha) and press Continue.
    Handover,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Answer {
    Allow,
    /// Look at this host for the rest of the session (a plain `Allow` is for [`LOOK_SHORT_SECS`]).
    AllowSession,
    Deny,
    ApproveAll,
    OneByOne,
    Continue,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ConsentChoice {
    pub answer: Answer,
    pub label: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ConsentRequest {
    pub id: String,
    pub user: String,
    pub kind: ConsentKind,
    pub domain: String,
    /// The short question (the row in the Approvals list).
    pub title: String,
    /// The full text of the dialog.
    pub text: String,
    /// What exactly would happen (steps), may be empty.
    pub detail: String,
    pub choices: Vec<ConsentChoice>,
    pub since: i64,
}

#[derive(Default)]
struct CState {
    open: Vec<ConsentRequest>,
    done: HashMap<String, Answer>,
    next: u64,
}

/// The requests waiting for the person, like [`crate::secretreq::SecretRequests`].
#[derive(Default)]
pub struct ConsentStore {
    st: Mutex<CState>,
    cv: Condvar,
}

fn choice(answer: Answer, label: impl Into<String>) -> ConsentChoice {
    ConsentChoice { answer, label: label.into() }
}

/// The host as the unit of consent: lower case, without a leading `www.`.
pub fn domain_of(url_or_host: &str) -> String {
    let h = if url_or_host.contains("://") { iyakki_sec::url_host(url_or_host).unwrap_or_default() } else { url_or_host.to_string() };
    let h = h.trim().trim_end_matches('.').to_ascii_lowercase();
    h.strip_prefix("www.").map(str::to_string).unwrap_or(h)
}

/// The account a page says you are signed in as: an e-mail address in the page title, if there is one.
pub fn account_hint(title: &str) -> Option<String> {
    for w in title.split(|c: char| c.is_whitespace() || matches!(c, '(' | ')' | '<' | '>' | ',' | ';' | '|' | '"' | '\'')) {
        let w = w.trim_matches(|c: char| c == '.' || c == ':');
        if let Some((local, host)) = w.split_once('@') {
            if !local.is_empty() && host.contains('.') && !host.starts_with('.') && !host.ends_with('.') && w.len() <= 80 && w.chars().all(|c| c.is_ascii_alphanumeric() || "@._-+".contains(c)) {
                return Some(w.to_string());
            }
        }
    }
    None
}

/// The question that asks for willingness (exact text; the account line only when the page title shows one).
pub fn willingness_text(domain: &str, account: Option<&str>) -> String {
    let base = format!("Are you willing to let the assistant act on your behalf on {domain}? It will be able to click, type and submit in your signed-in browser session. You can stop it at any time.");
    match account {
        Some(a) => format!("{base}\nSigned in as {a}."),
        None => base,
    }
}

pub fn look_title(host: &str) -> String {
    format!("Let the assistant look at {host} in your browser?")
}

pub fn interact_choices(domain: &str) -> Vec<ConsentChoice> {
    vec![choice(Answer::ApproveAll, format!("Approve all actions on {domain} for this session")), choice(Answer::OneByOne, "Approve one by one"), choice(Answer::Deny, "Deny")]
}

impl ConsentStore {
    pub fn new() -> ConsentStore {
        ConsentStore::default()
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, CState> {
        self.st.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Register a question; returns its id.
    pub fn create(&self, user: &str, kind: ConsentKind, domain: &str, text: &str, detail: &str, now: i64) -> Result<String, String> {
        let mut g = self.lock();
        if g.open.iter().filter(|r| r.user == user).count() >= MAX_PENDING {
            return Err("too many browser questions are waiting; ask the person to answer them first".into());
        }
        g.next += 1;
        let id = format!("bc{}", g.next);
        let clean = |s: &str, n: usize| -> String { s.chars().filter(|c| !c.is_control() || *c == '\n').take(n).collect() };
        let (title, choices) = match kind {
            ConsentKind::Look => (look_title(domain), vec![choice(Answer::Allow, "Allow"), choice(Answer::AllowSession, "Allow for this session"), choice(Answer::Deny, "Deny")]),
            ConsentKind::Interact => (format!("Act on your behalf on {domain}?"), interact_choices(domain)),
            ConsentKind::Step => (format!("Approve this action on {domain}?"), vec![choice(Answer::Allow, "Allow this action"), choice(Answer::Deny, "Deny")]),
            ConsentKind::Handover => ("The assistant needs you".to_string(), vec![choice(Answer::Continue, "Continue"), choice(Answer::Deny, "Cancel")]),
        };
        g.open.push(ConsentRequest { id: id.clone(), user: user.into(), kind, domain: domain.into(), title, text: clean(text, 600), detail: clean(detail, 600), choices, since: now });
        Ok(id)
    }

    pub fn pending(&self, user: &str) -> Vec<ConsentRequest> {
        self.lock().open.iter().filter(|r| r.user == user).cloned().collect()
    }

    pub fn get(&self, user: &str, id: &str) -> Option<ConsentRequest> {
        self.pending(user).into_iter().find(|r| r.id == id)
    }

    /// Answer a question (only its owner, and only with one of its choices).
    pub fn resolve(&self, user: &str, id: &str, answer: Answer) -> Result<(), String> {
        let mut g = self.lock();
        let Some(pos) = g.open.iter().position(|r| r.id == id && r.user == user) else { return Err("that question is not waiting any more".into()) };
        if !g.open[pos].choices.iter().any(|c| c.answer == answer) {
            return Err("that is not one of the choices".into());
        }
        g.open.remove(pos);
        g.done.insert(id.to_string(), answer);
        self.cv.notify_all();
        Ok(())
    }

    /// Deny everything a person has waiting (Stop, Take over, sign-out).
    pub fn deny_all(&self, user: &str) {
        let mut g = self.lock();
        let ids: Vec<String> = g.open.iter().filter(|r| r.user == user).map(|r| r.id.clone()).collect();
        g.open.retain(|r| r.user != user);
        for id in ids {
            g.done.insert(id, Answer::Deny);
        }
        self.cv.notify_all();
    }

    /// Wait for the answer; `None` when the time ran out (the question is withdrawn).
    pub fn wait(&self, id: &str, timeout: Duration) -> Option<Answer> {
        let end = std::time::Instant::now() + timeout;
        let mut g = self.lock();
        loop {
            if let Some(a) = g.done.remove(id) {
                return Some(a);
            }
            let left = end.saturating_duration_since(std::time::Instant::now());
            if left.is_zero() {
                g.open.retain(|r| r.id != id);
                return None;
            }
            g = self.cv.wait_timeout(g, left.min(Duration::from_millis(100))).unwrap_or_else(|e| e.into_inner()).0;
        }
    }
}

// ------------------------------------------------------------------------------------------------ session state

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    /// Approved for the rest of the run (until `last` + idle limit).
    All { run: u64, last: i64 },
    OneByOne,
    Denied,
}

struct RunBudget {
    started: i64,
    actions: u32,
    max_actions: u32,
    max_minutes: u32,
}

#[derive(Default)]
struct UserBrowser {
    /// Hosts the assistant may look at, with the time the permission ends.
    looked: HashMap<String, i64>,
    interact: HashMap<String, Mode>,
    runs: HashMap<u64, RunBudget>,
    taken_over: bool,
    stopped: bool,
    recent: VecDeque<i64>,
    last_snapshot: String,
    /// The assistant is acting until this time (the banner shows while it is in the future).
    active_until: i64,
    doing: String,
    host: String,
}

/// What the BROWSER tab shows about the assistant.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ControlView {
    /// The assistant is controlling the tab right now.
    pub active: bool,
    pub doing: String,
    pub host: String,
    pub taken_over: bool,
    pub stopped: bool,
}

type AuditSink = Arc<dyn Fn(&str, &str, &str) + Send + Sync>;

/// Everything about the assistant's use of the browser: the job queue, the questions, the grants and the budgets.
pub struct BrowserState {
    pub jobs: Arc<BrowserJobs>,
    pub consent: ConsentStore,
    users: Mutex<HashMap<String, UserBrowser>>,
    audit: Mutex<Option<AuditSink>>,
    /// Moves the clock forward (tests of the 30 minute expiry).
    shift: AtomicI64,
    idle_secs: AtomicI64,
    run_ids: AtomicU64,
    /// How long a tool waits for the person to answer a question.
    wait_ms: AtomicU64,
}

impl Default for BrowserState {
    fn default() -> Self {
        BrowserState::new()
    }
}

impl BrowserState {
    pub fn new() -> BrowserState {
        BrowserState {
            jobs: Arc::new(BrowserJobs::new()),
            consent: ConsentStore::new(),
            users: Mutex::new(HashMap::new()),
            audit: Mutex::new(None),
            shift: AtomicI64::new(0),
            idle_secs: AtomicI64::new(IDLE_SECS),
            run_ids: AtomicU64::new(0),
            wait_ms: AtomicU64::new(600_000),
        }
    }

    fn users(&self) -> std::sync::MutexGuard<'_, HashMap<String, UserBrowser>> {
        self.users.lock().unwrap_or_else(|e| e.into_inner())
    }

    pub fn now(&self) -> i64 {
        crate::clock::now_unix() + self.shift.load(Ordering::Relaxed)
    }

    /// Tests: let time pass without waiting.
    pub fn advance_clock(&self, secs: i64) {
        self.shift.fetch_add(secs, Ordering::Relaxed);
    }

    pub fn set_idle_limit(&self, secs: i64) {
        self.idle_secs.store(secs, Ordering::Relaxed);
    }

    pub fn set_wait(&self, d: Duration) {
        self.wait_ms.store(d.as_millis() as u64, Ordering::Relaxed);
    }

    pub fn wait_time(&self) -> Duration {
        Duration::from_millis(self.wait_ms.load(Ordering::Relaxed))
    }

    pub fn set_audit(&self, sink: AuditSink) {
        *self.audit.lock().unwrap_or_else(|e| e.into_inner()) = Some(sink);
    }

    pub fn has_audit(&self) -> bool {
        self.audit.lock().unwrap_or_else(|e| e.into_inner()).is_some()
    }

    pub fn audit(&self, user: &str, event: &str, detail: &str) {
        if let Some(s) = self.audit.lock().unwrap_or_else(|e| e.into_inner()).as_ref() {
            s(user, event, detail);
        }
    }

    // ---- runs and budgets

    /// A run starts using the browser tools. A new run is the person's new request: Stop and Take over are over.
    pub fn begin_run(&self, user: &str, max_actions: u32, max_minutes: u32) -> u64 {
        let id = self.run_ids.fetch_add(1, Ordering::SeqCst) + 1;
        let now = self.now();
        let mut g = self.users();
        let u = g.entry(user.to_string()).or_default();
        u.taken_over = false;
        u.stopped = false;
        u.runs.insert(id, RunBudget { started: now, actions: 0, max_actions: max_actions.max(1), max_minutes: max_minutes.max(1) });
        id
    }

    /// The run is over: "approve all" of that run ends with it.
    pub fn end_run(&self, user: &str, run: u64) {
        let mut g = self.users();
        if let Some(u) = g.get_mut(user) {
            u.runs.remove(&run);
            u.interact.retain(|_, m| !matches!(m, Mode::All { run: r, .. } if *r == run));
            if u.runs.is_empty() {
                u.active_until = 0;
            }
        }
    }

    /// Count one action against the run and the rate limit; refuses when the person stopped the assistant, took over, or a budget is used up.
    pub fn charge(&self, user: &str, run: u64) -> Result<(), String> {
        let now = self.now();
        let mut g = self.users();
        let u = g.entry(user.to_string()).or_default();
        if u.stopped {
            return Err("The person pressed Stop. Do not continue; tell them what you did so far and wait for their next request.".into());
        }
        if u.taken_over {
            return Err("The person took over the browser tab. Do not continue; ask them to tell you when to carry on.".into());
        }
        let Some(b) = u.runs.get_mut(&run) else { return Err("this run is over".into()) };
        if b.actions >= b.max_actions {
            return Err(format!("Browser action limit reached ({} actions in this run). Stop here, summarise what you did and ask the person whether to continue in a new request.", b.max_actions));
        }
        if now - b.started > b.max_minutes as i64 * 60 {
            return Err(format!("Browser time limit reached ({} minutes in this run). Stop here and summarise.", b.max_minutes));
        }
        while u.recent.front().is_some_and(|t| now - *t >= 10) {
            u.recent.pop_front();
        }
        if u.recent.len() >= RATE_PER_10S {
            return Err("Too many browser actions in a few seconds; slow down and look at the page before acting.".into());
        }
        u.recent.push_back(now);
        b.actions += 1;
        Ok(())
    }

    pub fn actions_used(&self, user: &str, run: u64) -> u32 {
        self.users().get(user).and_then(|u| u.runs.get(&run)).map_or(0, |b| b.actions)
    }

    /// Fail fast (no counting) when the person stopped the assistant or took over.
    pub fn check_control(&self, user: &str) -> Result<(), String> {
        let g = self.users();
        match g.get(user) {
            Some(u) if u.stopped => Err("The person pressed Stop. Do not continue; wait for their next request.".into()),
            Some(u) if u.taken_over => Err("The person took over the browser tab. Do not continue; ask them to tell you when to carry on.".into()),
            _ => Ok(()),
        }
    }

    // ---- looking

    pub fn looked(&self, user: &str, host: &str) -> bool {
        let now = self.now();
        self.users().get(user).and_then(|u| u.looked.get(&domain_of(host)).copied()).is_some_and(|until| until > now)
    }

    /// Allow looking at a host for the rest of the session.
    pub fn allow_look(&self, user: &str, host: &str) {
        self.users().entry(user.to_string()).or_default().looked.insert(domain_of(host), i64::MAX);
    }

    /// Allow looking at a host for a while only.
    pub fn allow_look_for(&self, user: &str, host: &str, secs: i64) {
        let until = self.now() + secs;
        self.users().entry(user.to_string()).or_default().looked.insert(domain_of(host), until);
    }

    // ---- interacting

    /// The grant for a domain, with the 30 minute idle rule and the run rule applied.
    pub fn mode(&self, user: &str, domain: &str, run: u64) -> Option<Mode> {
        let now = self.now();
        let idle = self.idle_secs.load(Ordering::Relaxed);
        let mut g = self.users();
        let u = g.get_mut(user)?;
        let d = domain_of(domain);
        match u.interact.get(&d).copied() {
            Some(Mode::All { run: r, last }) => {
                if r != run || now - last > idle {
                    u.interact.remove(&d);
                    None
                } else {
                    Some(Mode::All { run: r, last })
                }
            }
            other => other,
        }
    }

    pub fn set_mode(&self, user: &str, domain: &str, mode: Mode) {
        self.users().entry(user.to_string()).or_default().interact.insert(domain_of(domain), mode);
    }

    /// Keep "approve all" alive: it is used now.
    pub fn touch(&self, user: &str, domain: &str) {
        let now = self.now();
        if let Some(u) = self.users().get_mut(user) {
            if let Some(Mode::All { last, .. }) = u.interact.get_mut(&domain_of(domain)) {
                *last = now;
            }
        }
    }

    /// For the CLI test runner: the person ran the spec themselves, so these domains are approved for it.
    pub fn preapprove(&self, user: &str, run: u64, domains: &[String]) {
        let now = self.now();
        let mut g = self.users();
        let u = g.entry(user.to_string()).or_default();
        for d in domains {
            let d = domain_of(d);
            u.looked.insert(d.clone(), i64::MAX);
            u.interact.insert(d, Mode::All { run, last: now });
        }
    }

    // ---- control

    /// Stop: the assistant must not go on; every grant and question is gone.
    pub fn stop(&self, user: &str) {
        {
            let mut g = self.users();
            let u = g.entry(user.to_string()).or_default();
            u.stopped = true;
            u.interact.retain(|_, m| !matches!(m, Mode::All { .. }));
            u.active_until = 0;
        }
        self.consent.deny_all(user);
        self.audit(user, "browser.agent.stop", "");
    }

    /// Take over: the person uses the tab; the assistant pauses and loses "approve all".
    pub fn take_over(&self, user: &str) {
        {
            let mut g = self.users();
            let u = g.entry(user.to_string()).or_default();
            u.taken_over = true;
            u.interact.retain(|_, m| !matches!(m, Mode::All { .. }));
            u.active_until = 0;
        }
        self.consent.deny_all(user);
        self.audit(user, "browser.agent.takeover", "");
    }

    /// Hand the tab back: the assistant may continue (its approvals are asked again).
    pub fn resume(&self, user: &str) {
        let mut g = self.users();
        let u = g.entry(user.to_string()).or_default();
        u.taken_over = false;
        u.stopped = false;
    }

    /// Sign-out: nothing survives.
    pub fn on_logout(&self, user: &str) {
        self.users().remove(user);
        self.consent.deny_all(user);
        self.jobs.fail_all(user, "the person signed out");
    }

    pub fn note_doing(&self, user: &str, doing: &str, host: &str) {
        let until = self.now() + 6;
        let mut g = self.users();
        let u = g.entry(user.to_string()).or_default();
        if !u.taken_over && !u.stopped {
            u.active_until = until;
        }
        u.doing = doing.chars().take(120).collect();
        u.host = host.to_string();
    }

    pub fn control_view(&self, user: &str) -> ControlView {
        let now = self.now();
        let g = self.users();
        match g.get(user) {
            Some(u) => ControlView { active: now < u.active_until && !u.taken_over && !u.stopped, doing: u.doing.clone(), host: u.host.clone(), taken_over: u.taken_over, stopped: u.stopped },
            None => ControlView::default(),
        }
    }

    pub fn set_last_snapshot(&self, user: &str, text: &str) {
        self.users().entry(user.to_string()).or_default().last_snapshot = text.to_string();
    }

    pub fn last_snapshot(&self, user: &str) -> String {
        self.users().get(user).map(|u| u.last_snapshot.clone()).unwrap_or_default()
    }

    /// Ask the person and wait. `Err` when nobody answered in time.
    pub fn ask(&self, user: &str, kind: ConsentKind, domain: &str, text: &str, detail: &str) -> Result<Answer, String> {
        let id = self.consent.create(user, kind, domain, text, detail, crate::clock::now_unix())?;
        self.consent.wait(&id, self.wait_time()).ok_or_else(|| match kind {
            ConsentKind::Look => format!("You (the person) did not answer the approval request for {domain} in time; nothing was opened. Ask them to approve and retry: they answer with the Allow / Deny buttons in the chat, Ctrl+Y / Ctrl+U / Ctrl+N, or the inbox bell; typing 'approve' in the chat does not approve anything."),
            _ => format!("You (the person) did not answer the approval request for {domain} in time; nothing was done. Ask them to approve and retry: they answer with the buttons in the chat, Ctrl+Y / Ctrl+U / Ctrl+N, or the inbox bell; typing 'approve' in the chat does not approve anything."),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn domains_are_hosts_without_www() {
        assert_eq!(domain_of("https://WWW.Example.org/a?b"), "example.org");
        assert_eq!(domain_of("gemini.google.com"), "gemini.google.com");
        assert_ne!(domain_of("https://accounts.google.com/"), domain_of("https://gemini.google.com/"), "subdomains are separate sites");
    }

    #[test]
    fn the_signed_in_account_comes_from_the_title_when_it_shows_one() {
        assert_eq!(account_hint("Inbox (3) - ann.lee@example.org - Mail"), Some("ann.lee@example.org".into()));
        assert_eq!(account_hint("Google Gemini"), None);
        assert_eq!(account_hint("contact us at @home"), None);
        assert!(willingness_text("a.org", Some("x@y.org")).ends_with("Signed in as x@y.org."));
    }

    #[test]
    fn the_willingness_text_and_choices_are_exact() {
        assert_eq!(willingness_text("gemini.google.com", None), "Are you willing to let the assistant act on your behalf on gemini.google.com? It will be able to click, type and submit in your signed-in browser session. You can stop it at any time.");
        let labels: Vec<String> = interact_choices("a.org").into_iter().map(|c| c.label).collect();
        assert_eq!(labels, vec!["Approve all actions on a.org for this session", "Approve one by one", "Deny"]);
        assert_eq!(look_title("a.org"), "Let the assistant look at a.org in your browser?");
    }

    #[test]
    fn only_a_listed_choice_can_answer_and_only_by_its_owner() {
        let c = ConsentStore::new();
        let id = c.create("ann", ConsentKind::Look, "a.org", "t", "", 1).unwrap();
        assert!(c.resolve("bob", &id, Answer::Allow).is_err());
        assert!(c.resolve("ann", &id, Answer::ApproveAll).is_err(), "Look has no approve-all");
        assert_eq!(c.get("ann", &id).unwrap().choices.len(), 3);
        c.resolve("ann", &id, Answer::Allow).unwrap();
        assert_eq!(c.wait(&id, Duration::from_millis(10)), Some(Answer::Allow));
        let id = c.create("ann", ConsentKind::Interact, "a.org", "t", "", 1).unwrap();
        c.deny_all("ann");
        assert_eq!(c.wait(&id, Duration::from_millis(10)), Some(Answer::Deny));
        let id = c.create("ann", ConsentKind::Step, "a.org", "t", "", 1).unwrap();
        assert_eq!(c.wait(&id, Duration::from_millis(10)), None);
        assert!(c.pending("ann").is_empty());
    }

    #[test]
    fn approve_all_is_per_domain_per_run_and_expires() {
        let b = BrowserState::new();
        let run = b.begin_run("ann", 60, 20);
        b.set_mode("ann", "a.org", Mode::All { run, last: b.now() });
        assert!(matches!(b.mode("ann", "https://www.a.org/x", run), Some(Mode::All { .. })));
        assert_eq!(b.mode("ann", "b.org", run), None, "never another domain");
        let other = b.begin_run("ann", 60, 20);
        assert_eq!(b.mode("ann", "a.org", other), None, "a grant belongs to its run");
        b.set_mode("ann", "a.org", Mode::All { run: other, last: b.now() });
        b.advance_clock(IDLE_SECS + 5);
        assert_eq!(b.mode("ann", "a.org", other), None, "30 minutes without use");
        b.set_mode("ann", "a.org", Mode::All { run: other, last: b.now() });
        b.end_run("ann", other);
        assert_eq!(b.mode("ann", "a.org", other), None, "the end of the run");
        b.set_mode("ann", "c.org", Mode::OneByOne);
        b.set_mode("ann", "d.org", Mode::Denied);
        b.on_logout("ann");
        assert_eq!(b.mode("ann", "c.org", 0), None);
        assert_eq!(b.mode("ann", "d.org", 0), None, "sign-out forgets everything");
    }

    #[test]
    fn stop_and_take_over_drop_approve_all_and_pause_the_assistant() {
        let b = BrowserState::new();
        let run = b.begin_run("ann", 60, 20);
        b.set_mode("ann", "a.org", Mode::All { run, last: b.now() });
        b.set_mode("ann", "k.org", Mode::OneByOne);
        let id = b.consent.create("ann", ConsentKind::Step, "a.org", "t", "", 1).unwrap();
        b.take_over("ann");
        assert_eq!(b.mode("ann", "a.org", run), None);
        assert_eq!(b.mode("ann", "k.org", run), Some(Mode::OneByOne), "one-by-one is a smaller grant and stays");
        assert_eq!(b.consent.wait(&id, Duration::from_millis(5)), Some(Answer::Deny));
        assert!(b.charge("ann", run).unwrap_err().contains("took over"));
        b.resume("ann");
        assert!(b.charge("ann", run).is_ok());
        b.stop("ann");
        assert!(b.charge("ann", run).unwrap_err().contains("Stop"));
        let run2 = b.begin_run("ann", 60, 20);
        assert!(b.charge("ann", run2).is_ok(), "a new request starts afresh");
    }

    #[test]
    fn budgets_count_actions_minutes_and_rate() {
        let b = BrowserState::new();
        let run = b.begin_run("ann", 3, 20);
        for _ in 0..3 {
            b.charge("ann", run).unwrap();
        }
        let e = b.charge("ann", run).unwrap_err();
        assert!(e.contains("action limit") && e.contains('3'), "{e}");
        let run = b.begin_run("ann", 60, 1);
        b.advance_clock(61);
        assert!(b.charge("ann", run).unwrap_err().contains("time limit"));
        let run = b.begin_run("ann", 500, 20);
        for _ in 0..RATE_PER_10S {
            b.charge("ann", run).unwrap();
        }
        assert!(b.charge("ann", run).unwrap_err().contains("Too many"));
        b.advance_clock(11);
        assert!(b.charge("ann", run).is_ok());
    }

    #[test]
    fn the_wait_for_an_answer_defaults_to_ten_minutes() {
        assert_eq!(BrowserState::new().wait_time(), Duration::from_secs(600));
    }

    #[test]
    fn the_banner_shows_only_while_acting() {
        let b = BrowserState::new();
        let run = b.begin_run("ann", 60, 20);
        assert!(!b.control_view("ann").active);
        b.note_doing("ann", "click button \"Send\"", "a.org");
        let v = b.control_view("ann");
        assert!(v.active && v.doing.contains("Send") && v.host == "a.org");
        b.advance_clock(10);
        assert!(!b.control_view("ann").active);
        b.note_doing("ann", "x", "a.org");
        b.take_over("ann");
        assert!(!b.control_view("ann").active && b.control_view("ann").taken_over);
        let _ = run;
    }
}
