//! Transient failures of `kubectl` (docs/K8S-TILE.md, "When the cluster does not answer"): a busy machine, an API server that
//! is starting or restarting, a timeout. They are told apart from the persistent ones (no kubectl, no kubeconfig, forbidden, a bad
//! context) so the views can retry quietly, keep the last good data on screen and show a banner only when it is really broken.
//!
//! * [`classify`] is a pure function: [`Verdict::Transient`] or [`Verdict::Persistent`] for one failed call.
//! * [`Retry`] runs one call with up to three quick retries (0.7, 1.5 and 3 s) and writes every attempt into the [`ClusterLog`].
//!   Its waits can be cancelled ([`Gate`]: Refresh) and its clock is a [`Timer`], so tests never sleep.
//! * [`Tracker`] is the feed's memory: how long the cluster has been failing, when it last worked, how long to wait before the next
//!   try (2, 4, 8, then 15 s) and whether the data on screen is stale or the banner is due.
//!
//! Nothing here runs kubectl by itself and nothing blocks a UI thread: the feeds call it from their own threads.

use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use crate::k8s::Kube;
use crate::wsexec::Out;

/// The quick retries inside one refresh.
pub const QUICK_RETRY_MS: [u64; 3] = [700, 1500, 3000];
/// A failing list keeps the old data on screen this long (the banner comes after it).
pub const STALE_WINDOW_MS: u64 = 25_000;
/// The same while the managed k3s is starting: the API server needs a minute or two, so the banner waits longer.
pub const STARTING_WINDOW_MS: u64 = 90_000;
/// After the managed cluster changed state, a refused connection is still expected for this long.
pub const CHANGE_GRACE_MS: u64 = 30_000;
/// The cluster log keeps this many events.
pub const LOG_EVENTS: usize = 50;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Verdict {
    /// Worth trying again: the cluster may answer in a moment.
    Transient,
    /// Trying again will not help until something is fixed.
    Persistent,
}

impl Verdict {
    pub fn word(self) -> &'static str {
        match self {
            Verdict::Transient => "transient",
            Verdict::Persistent => "persistent",
        }
    }
}

/// What the classifier may know about the situation.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Ctx {
    /// The managed k3s is starting, or its state changed a moment ago.
    pub starting: bool,
    /// This cluster answered before (so a refused connection now is a restart, not a wrong address).
    pub worked_before: bool,
}

/// Messages that mean the same thing every time they appear: fix something.
const PERSISTENT: &[&str] = &[
    "forbidden",
    "unauthorized",
    "no such host",
    "does not exist",
    "no configuration has been provided",
    "invalid configuration",
    "unable to read",
    "error loading config",
    "doesn't have a resource type",
    "unknown flag",
    "unknown command",
    "no such file or directory",
];

/// Messages of a cluster that is busy, starting or restarting: always worth another try.
const TRANSIENT: &[&str] = &[
    "timed out",
    "timeout",
    "deadline exceeded",
    "tls handshake timeout",
    "currently unable to handle the request",
    "etcdserver:",
    "connection reset",
    "broken pipe",
    "service unavailable",
    "too many requests",
    "server is not ready",
    "failed to download openapi",
    "an error on the server",
    "transport is closing",
];

/// Messages that need a reason to be transient: a refused connection while the managed k3s starts or after the cluster answered once.
const NEEDS_CONTEXT: &[&str] = &["connection refused", "was refused", "unable to connect to the server", "no route to host", "couldn't get current server", "dial tcp"];

/// Is this failed call worth retrying? Success is not a question here (callers ask only after a failure).
pub fn classify(o: &Out, ctx: Ctx) -> Verdict {
    if o.missing {
        return Verdict::Persistent;
    }
    if o.timed_out {
        return Verdict::Transient;
    }
    let e = format!("{}\n{}", o.stderr, if matches!(o.stdout.trim_start().chars().next(), Some('{' | '[')) { "" } else { o.stdout.as_str() }).to_lowercase();
    let said_nothing = e.trim().is_empty() || e.lines().map(str::trim).all(|l| l.is_empty() || matches!(l, "{" | "}" | "[" | "]"));
    if said_nothing {
        // killed (no exit code) or an exit with no message at all: a busy machine, not a configuration problem
        return Verdict::Transient;
    }
    if PERSISTENT.iter().any(|p| e.contains(p)) && !e.contains("etcdserver:") {
        // a DNS name that stopped resolving on a cluster that worked is a network blip
        let only_dns = e.contains("no such host") && !PERSISTENT.iter().filter(|p| **p != "no such host").any(|p| e.contains(p));
        return if only_dns && ctx.worked_before { Verdict::Transient } else { Verdict::Persistent };
    }
    if TRANSIENT.iter().any(|p| e.contains(p)) || e.split(|c: char| !c.is_alphanumeric()).any(|w| w == "eof") {
        return Verdict::Transient;
    }
    // an aggregated API (metrics.k8s.io) that answers 503 while it starts
    if e.contains("503") || e.contains("502") || e.contains("504") {
        return Verdict::Transient;
    }
    if NEEDS_CONTEXT.iter().any(|p| e.contains(p)) {
        return if ctx.starting || ctx.worked_before { Verdict::Transient } else { Verdict::Persistent };
    }
    Verdict::Persistent
}

// ------------------------------------------------------------------------------------------------------------ time

/// A cancellable wait: Refresh bumps it and every wait in progress ends at once.
#[derive(Default)]
pub struct Gate {
    epoch: AtomicU64,
}

impl Gate {
    pub fn bump(&self) {
        self.epoch.fetch_add(1, Ordering::SeqCst);
    }
    pub fn epoch(&self) -> u64 {
        self.epoch.load(Ordering::SeqCst)
    }
}

/// The clock and the waits of the retry layer, so tests can run it without sleeping.
pub trait Timer: Send + Sync {
    /// Milliseconds on a monotonic clock.
    fn now_ms(&self) -> u64;
    /// Seconds since the epoch (the log and the "last worked" text).
    fn wall_secs(&self) -> u64;
    /// Wait this long. Returns false when the gate was bumped meanwhile (the wait ended early).
    fn sleep(&self, ms: u64, gate: &Gate) -> bool;
}

pub struct SystemTimer {
    start: Instant,
}

impl SystemTimer {
    pub fn new() -> SystemTimer {
        SystemTimer { start: Instant::now() }
    }
}

impl Default for SystemTimer {
    fn default() -> Self {
        Self::new()
    }
}

impl Timer for SystemTimer {
    fn now_ms(&self) -> u64 {
        self.start.elapsed().as_millis() as u64
    }
    fn wall_secs(&self) -> u64 {
        std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs())
    }
    fn sleep(&self, ms: u64, gate: &Gate) -> bool {
        let (until, epoch) = (Instant::now() + Duration::from_millis(ms), gate.epoch());
        while Instant::now() < until {
            if gate.epoch() != epoch {
                return false;
            }
            std::thread::sleep(Duration::from_millis(20).min(until.saturating_duration_since(Instant::now())));
        }
        gate.epoch() == epoch
    }
}

/// How long to wait before the next background try after `fails` failed refreshes in a row: 2, 4, 8, then 15 s.
pub fn background_delay(fails: u32) -> Duration {
    Duration::from_secs(match fails {
        0 | 1 => 2,
        2 => 4,
        3 => 8,
        _ => 15,
    })
}

// ------------------------------------------------------------------------------------------------------------ hints

/// What the views tell the retry layer about the managed cluster (set from the UI thread, read from the feed thread).
#[derive(Default)]
pub struct Hints {
    starting: AtomicBool,
    changed_ms: AtomicU64,
    changed: AtomicBool,
    worked: AtomicBool,
    local: AtomicBool,
}

impl Hints {
    /// The managed k3s is starting (true) or not. A change in either direction starts the grace period.
    pub fn set_starting(&self, on: bool, now_ms: u64) {
        if self.starting.swap(on, Ordering::SeqCst) != on {
            self.changed_ms.store(now_ms, Ordering::SeqCst);
            self.changed.store(true, Ordering::SeqCst);
        }
    }
    /// The cluster is the managed k3s on this computer: a refused connection means it is (re)starting, never a wrong address.
    pub fn set_local(&self, on: bool) {
        self.local.store(on, Ordering::SeqCst);
    }
    pub fn worked(&self) {
        self.worked.store(true, Ordering::SeqCst);
    }
    /// Another cluster: what is known about the old one does not apply.
    pub fn forget(&self) {
        self.worked.store(false, Ordering::SeqCst);
    }
    pub fn starting_now(&self, now_ms: u64) -> bool {
        self.starting.load(Ordering::SeqCst) || (self.changed.load(Ordering::SeqCst) && now_ms.saturating_sub(self.changed_ms.load(Ordering::SeqCst)) < CHANGE_GRACE_MS)
    }
    pub fn ctx(&self, now_ms: u64) -> Ctx {
        Ctx { starting: self.starting_now(now_ms), worked_before: self.worked.load(Ordering::SeqCst) || self.local.load(Ordering::SeqCst) }
    }
}

// ------------------------------------------------------------------------------------------------------------ the log

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LogEvent {
    pub at: u64,
    /// The call without secrets: `get pods -A -o json`.
    pub call: String,
    pub ms: u64,
    pub code: Option<i32>,
    /// First line of what kubectl said (empty on success).
    pub said: String,
    /// None: the call worked.
    pub verdict: Option<Verdict>,
    /// 0 for the first try of a refresh, 1..3 for the quick retries.
    pub retry: u32,
}

impl LogEvent {
    pub fn line(&self) -> String {
        let (h, m, s) = ((self.at / 3600) % 24, (self.at / 60) % 60, self.at % 60);
        let code = self.code.map_or("killed".to_string(), |c| format!("exit {c}"));
        let result = match self.verdict {
            None => "ok".to_string(),
            Some(v) => format!("{}, try {}", v.word(), self.retry + 1),
        };
        let said = if self.said.is_empty() { String::new() } else { format!(" \"{}\"", self.said) };
        format!("{h:02}:{m:02}:{s:02} UTC  kubectl {}  {:.1} s  {code}  {result}{said}", self.call, self.ms as f64 / 1000.0)
    }
}

/// The last [`LOG_EVENTS`] kubectl failures (and the call that worked again), in memory only. Shown by the tile's "Show log".
#[derive(Default)]
pub struct ClusterLog {
    events: Mutex<VecDeque<LogEvent>>,
}

impl ClusterLog {
    /// Add an event. Returns true when it starts a new failure episode (the first failure since the last success).
    pub fn push(&self, e: LogEvent) -> bool {
        let mut v = self.events.lock().unwrap_or_else(|x| x.into_inner());
        let new_episode = e.verdict.is_some() && v.back().is_none_or(|l| l.verdict.is_none());
        if v.len() >= LOG_EVENTS {
            v.pop_front();
        }
        v.push_back(e);
        new_episode
    }
    pub fn events(&self) -> Vec<LogEvent> {
        self.events.lock().unwrap_or_else(|x| x.into_inner()).iter().cloned().collect()
    }
    /// The last call failed (a success right after is worth one line: "worked again").
    pub fn last_failed(&self) -> bool {
        self.events.lock().unwrap_or_else(|x| x.into_inner()).back().is_some_and(|l| l.verdict.is_some())
    }
    /// The log as text, newest last, for the screen and the clipboard.
    pub fn text(&self) -> String {
        let ev = self.events();
        if ev.is_empty() {
            return "No kubectl problems since this view opened.".to_string();
        }
        ev.iter().map(LogEvent::line).collect::<Vec<_>>().join("\n")
    }
}

/// The arguments of a call without anything private: the kubeconfig path, the context name and credentials are replaced.
pub fn summarize(args: &[String]) -> String {
    const HIDE: [&str; 9] = ["--kubeconfig", "--context", "--token", "--password", "--username", "--client-key", "--client-certificate", "--server", "--certificate-authority"];
    const SKIP: [&str; 1] = ["--request-timeout"];
    let mut out: Vec<String> = Vec::new();
    let mut it = args.iter();
    while let Some(a) = it.next() {
        let name = a.split('=').next().unwrap_or(a);
        if SKIP.contains(&name) {
            continue;
        }
        if HIDE.contains(&name) {
            if !a.contains('=') {
                it.next();
            }
            out.push(format!("{name}=\u{2026}"));
        } else {
            out.push(a.clone());
        }
    }
    out.join(" ")
}

/// The first line that says something, shortened, with credentials and the kubeconfig path taken out.
pub fn first_line(o: &Out, secrets: &[String]) -> String {
    let mut t: String = o.stderr.lines().map(str::trim).find(|l| !l.is_empty()).unwrap_or("").chars().take(240).collect();
    for s in secrets.iter().filter(|s| s.len() > 3) {
        t = t.replace(s.as_str(), "<kubeconfig>");
    }
    for key in ["bearer ", "token=", "password="] {
        if let Some(i) = t.to_lowercase().find(key) {
            let end = i + key.len();
            let rest = t[end..].find(char::is_whitespace).map_or(t.len(), |n| end + n);
            t.replace_range(end..rest, "\u{2026}");
        }
    }
    t
}

// ------------------------------------------------------------------------------------------------------------ retry

/// One call with its retries.
pub struct Attempt {
    pub out: Out,
    /// None when the call worked.
    pub verdict: Option<Verdict>,
    /// Quick retries used.
    pub retries: u32,
    /// Refresh ended a wait: the caller starts over at once.
    pub cancelled: bool,
}

/// The retry policy with its clock, its cancel gate, the managed-cluster hints and the log. Cheap to clone.
#[derive(Clone)]
pub struct Retry {
    pub quick: Vec<u64>,
    pub timer: Arc<dyn Timer>,
    pub gate: Arc<Gate>,
    pub hints: Arc<Hints>,
    pub log: Arc<ClusterLog>,
}

impl Retry {
    pub fn new(quick: &[u64], timer: Arc<dyn Timer>) -> Retry {
        Retry { quick: quick.to_vec(), timer, gate: Arc::new(Gate::default()), hints: Arc::new(Hints::default()), log: Arc::new(ClusterLog::default()) }
    }
    /// What the feeds use: three quick retries on the real clock.
    pub fn system() -> Retry {
        Retry::new(&QUICK_RETRY_MS, Arc::new(SystemTimer::new()))
    }
    /// No retries: one call, whatever happens (the plain functions of the engine).
    pub fn off() -> Retry {
        Retry::new(&[], Arc::new(SystemTimer::new()))
    }
    /// One retry after 0.7 s: what `fetch` did before for a killed kubectl, now for every transient failure.
    pub fn once() -> Retry {
        Retry::new(&QUICK_RETRY_MS[..1], Arc::new(SystemTimer::new()))
    }

    /// Run `args` through `kube`: on a transient failure wait and try again (at most `quick.len()` times). Every failure, and the
    /// call that worked again, goes into the log. A success makes exactly one call.
    pub fn run(&self, kube: &dyn Kube, args: &[String]) -> Attempt {
        let call = summarize(args);
        let secrets: Vec<String> = args.windows(2).filter(|w| w[0] == "--kubeconfig").map(|w| w[1].clone()).collect();
        let mut retry = 0u32;
        loop {
            let t0 = self.timer.now_ms();
            let out = kube.run(args);
            let ms = self.timer.now_ms().saturating_sub(t0);
            let now = self.timer.now_ms();
            if out.ok() {
                if self.log.last_failed() {
                    self.log.push(LogEvent { at: self.timer.wall_secs(), call: call.clone(), ms, code: out.code, said: String::new(), verdict: None, retry });
                }
                self.hints.worked();
                return Attempt { out, verdict: None, retries: retry, cancelled: false };
            }
            let v = classify(&out, self.hints.ctx(now));
            let new_episode = self.log.push(LogEvent { at: self.timer.wall_secs(), call: call.clone(), ms, code: out.code, said: first_line(&out, &secrets), verdict: Some(v), retry });
            if new_episode {
                log::warn!("k8s: kubectl {call} failed ({}): {}", v.word(), first_line(&out, &secrets));
            }
            if v == Verdict::Persistent || retry as usize >= self.quick.len() {
                return Attempt { out, verdict: Some(v), retries: retry, cancelled: false };
            }
            if !self.timer.sleep(self.quick[retry as usize], &self.gate) {
                return Attempt { out, verdict: Some(v), retries: retry, cancelled: true };
            }
            retry += 1;
        }
    }
}

// ------------------------------------------------------------------------------------------------------------ tracker

/// What a feed publishes about its failures for the views.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Status {
    /// The refresh is failing transiently and the data on screen is the last good one (or none yet).
    pub failing: bool,
    /// Seconds the cluster has been failing.
    pub failing_for_s: u64,
    /// Seconds since the last good answer (None: this view never got one).
    pub last_ok_age_s: Option<u64>,
    /// Seconds between background tries.
    pub retry_every_s: u64,
    /// Failed refreshes in a row.
    pub fails: u32,
    /// First line of what kubectl said last time.
    pub error: String,
    /// The managed k3s is starting: failures are expected.
    pub starting: bool,
    /// The window passed (or the failure is persistent): the view shows the banner.
    pub banner: bool,
}

#[derive(Debug, Default)]
pub struct Tracker {
    first_fail_ms: Option<u64>,
    last_ok_s: Option<u64>,
    fails: u32,
    error: String,
    banner: bool,
    starting: bool,
}

impl Tracker {
    pub fn ok(&mut self, now_s: u64) {
        *self = Tracker { last_ok_s: Some(now_s), ..Default::default() };
    }
    /// A refresh failed. Returns true when the old data may stay on screen (transient, inside the window).
    pub fn failed(&mut self, now_ms: u64, verdict: Verdict, error: &str, starting: bool) -> bool {
        self.fails += 1;
        self.error = error.to_string();
        self.starting = starting;
        let first = *self.first_fail_ms.get_or_insert(now_ms);
        let window = if starting { STARTING_WINDOW_MS } else { STALE_WINDOW_MS };
        self.banner = verdict == Verdict::Persistent || now_ms.saturating_sub(first) >= window;
        !self.banner
    }
    /// Refresh was pressed: the next wait is short again (the failure itself is still counted from its start).
    pub fn reset_backoff(&mut self) {
        self.fails = self.fails.min(1);
    }
    /// How long to wait before the next background call.
    pub fn next_delay(&self, interval: Duration) -> Duration {
        if self.fails == 0 {
            interval
        } else {
            background_delay(self.fails)
        }
    }
    pub fn failing(&self) -> bool {
        self.fails > 0
    }
    pub fn status(&self, now_ms: u64, now_s: u64) -> Status {
        if self.fails == 0 {
            return Status::default();
        }
        Status {
            failing: true,
            failing_for_s: now_ms.saturating_sub(self.first_fail_ms.unwrap_or(now_ms)) / 1000,
            last_ok_age_s: self.last_ok_s.map(|t| now_s.saturating_sub(t)),
            retry_every_s: background_delay(self.fails).as_secs(),
            fails: self.fails,
            error: self.error.clone(),
            starting: self.starting,
            banner: self.banner,
        }
    }
}

/// The dim line above stale data.
pub fn stale_line(st: &Status) -> String {
    if st.starting {
        return "k3s is starting. Showing the last data and trying again quietly.".to_string();
    }
    match st.last_ok_age_s {
        Some(a) => format!("Updating\u{2026} (the cluster did not answer, retrying) \u{b7} data is {a} s old"),
        None => "Updating\u{2026} (the cluster did not answer, retrying)".to_string(),
    }
}

/// What a banner adds once the window passed: when it last worked and what happens next.
pub fn banner_tail(st: &Status) -> String {
    let when = match st.last_ok_age_s {
        Some(a) => format!("Last worked {a} s ago."),
        None => "It has not answered since this view opened.".to_string(),
    };
    format!("{when} Retrying every {} s. Press Refresh to retry now.", st.retry_every_s.max(2))
}

/// A clock that only moves when a wait happens (or the test moves it), for tests and screenshots: waits cost no time.
pub struct FakeTimer {
    ms: AtomicU64,
    /// Every wait asked for, in milliseconds.
    pub slept: Mutex<Vec<u64>>,
    /// The next wait is ended by Refresh.
    pub cancel_next: AtomicBool,
}

impl FakeTimer {
    pub fn new() -> Arc<FakeTimer> {
        Arc::new(FakeTimer { ms: AtomicU64::new(1_000), slept: Mutex::new(vec![]), cancel_next: AtomicBool::new(false) })
    }
    pub fn advance(&self, ms: u64) {
        self.ms.fetch_add(ms, Ordering::SeqCst);
    }
}

impl Timer for FakeTimer {
    fn now_ms(&self) -> u64 {
        self.ms.load(Ordering::SeqCst)
    }
    fn wall_secs(&self) -> u64 {
        1_800_000_000 + self.now_ms() / 1000
    }
    fn sleep(&self, ms: u64, gate: &Gate) -> bool {
        self.slept.lock().unwrap().push(ms);
        if self.cancel_next.swap(false, Ordering::SeqCst) {
            gate.bump();
            return false;
        }
        self.advance(ms);
        true
    }
}

#[cfg(test)]
#[path = "k8sretry_tests.rs"]
mod tests;
