//! The device core's embedded browser (F-SDK-63): the real [`iyakki_core_browser::LocalBrowserSessions`] with this machine's host. One
//! per data folder (the shell builds a few `AgentCore` handles over the same services, and a view must find its session through any
//! of them). The host judges with what the machine already keeps: the person's known-bad site list (`iyakki_web::badsites::Guard`, the
//! same list the Browser app uses, also checked at every redirect and by the transport itself), a per-person switch file, and the audit
//! sink the shell gives (`set_audit`, the hash-chained audit log). The host policy of the signed-in account (`net_hosts`) is a hook,
//! `set_policy`; until the shell sets it every host the known-bad list lets through is allowed.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};

use iyakki_core_browser::{BrowserHost, LocalBrowserSessions};
use iyakki_web::Fetch;

type AuditFn = Arc<dyn Fn(&str, &str, &str) + Send + Sync>;
type PolicyFn = Arc<dyn Fn(&str, &str) -> bool + Send + Sync>;

struct Shared {
    dir: PathBuf,
    fetch: OnceLock<Arc<dyn Fetch>>,
    audit: Mutex<Option<AuditFn>>,
    policy: Mutex<Option<PolicyFn>>,
}

/// The browser sessions of one machine and the switches around them.
pub struct Embedded {
    shared: Arc<Shared>,
    sessions: LocalBrowserSessions,
}

fn name_ok(n: &str) -> bool {
    !n.is_empty() && n.len() <= 48 && n.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_'))
}

fn lk<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// The browser's network for embedded views: the HTTP client behind connection limits, the ad and tracker filter outside it, and the
/// known-bad site list outside everything (a listed site is never asked). No page cache: a view is a window, not a second browser.
fn transport() -> Arc<dyn Fetch> {
    use iyakki_web::net::{LimitedFetch, Limiter, GLOBAL, PER_HOST};
    let limited: Arc<dyn Fetch> = Arc::new(LimitedFetch::new(iyakki_proxy::https::BrowserFetch::default(), Arc::new(Limiter::new(PER_HOST, GLOBAL))));
    Arc::new(iyakki_web::badsites::BadSiteFetch::new(Arc::new(iyakki_web::blocker_state::FilteredFetch::new(limited, iyakki_web::blocker_state::Filter::global())), iyakki_web::badsites::Guard::global()))
}

impl Shared {
    fn switch_path(&self, user: &str) -> Option<PathBuf> {
        name_ok(user).then(|| self.dir.join(format!("browser-embed-{user}.json")))
    }
}

impl BrowserHost for Shared {
    fn fetch(&self, _user: &str) -> Arc<dyn Fetch> {
        self.fetch.get_or_init(transport).clone()
    }
    fn known_bad(&self, host: &str) -> Option<(String, String)> {
        iyakki_web::badsites::Guard::global().check(host)
    }
    fn allows_host(&self, user: &str, host: &str) -> bool {
        match lk(&self.policy).clone() {
            Some(p) => p(user, host),
            None => true,
        }
    }
    fn enabled(&self, user: &str) -> bool {
        let Some(p) = self.switch_path(user) else { return false };
        std::fs::read_to_string(p).ok().and_then(|t| serde_json::from_str::<serde_json::Value>(&t).ok()).and_then(|v| v["enabled"].as_bool()).unwrap_or(true)
    }
    fn set_enabled(&self, user: &str, on: bool) -> Result<(), String> {
        let p = self.switch_path(user).ok_or("that person name is not allowed")?;
        iyakki_schema::write_atomic(&p, serde_json::json!({ "enabled": on }).to_string().as_bytes(), 0o600)
    }
    fn audit(&self, user: &str, app: &str, event: &str, detail: &str) {
        let line = format!("{app}: {detail}");
        match lk(&self.audit).clone() {
            Some(f) => f(user, event, &line),
            None => log::info!("{event} {user} {line}"),
        }
    }
}

impl Embedded {
    pub fn sessions(&self) -> &LocalBrowserSessions {
        &self.sessions
    }

    /// Where the shell's audit log gets a line: `(person, event, detail)`.
    pub fn set_audit(&self, f: AuditFn) {
        *lk(&self.shared.audit) = Some(f);
    }

    /// The account policy: `(person, host)` is allowed or not.
    pub fn set_policy(&self, f: PolicyFn) {
        *lk(&self.shared.policy) = Some(f);
    }
}

/// The embedded browser of the machine whose data folder this is.
pub fn for_dir(dir: &Path) -> Arc<Embedded> {
    static ALL: OnceLock<Mutex<HashMap<PathBuf, Arc<Embedded>>>> = OnceLock::new();
    let mut m = lk(ALL.get_or_init(|| Mutex::new(HashMap::new())));
    m.entry(dir.to_path_buf())
        .or_insert_with(|| {
            let shared = Arc::new(Shared { dir: dir.to_path_buf(), fetch: OnceLock::new(), audit: Mutex::new(None), policy: Mutex::new(None) });
            Arc::new(Embedded { sessions: LocalBrowserSessions::new(shared.clone()), shared })
        })
        .clone()
}

/// The device's embedded browser as a shared handle (`Arc<dyn BrowserSessions>`), for the web service's tile host: every call goes to the same
/// [`LocalBrowserSessions`] the shell's views use, so a tile's views are counted, switched and judged like any app's.
pub struct SharedSessions(pub Arc<Embedded>);

impl iyakki_core_api::BrowserSessions for SharedSessions {
    fn enabled(&self, user: &str) -> bool {
        self.0.sessions().enabled(user)
    }
    fn set_enabled(&self, user: &str, on: bool) -> iyakki_core_api::browsersession::BrowserResult<()> {
        self.0.sessions().set_enabled(user, on)
    }
    fn open_views(&self, user: &str) -> usize {
        self.0.sessions().open_views(user)
    }
    fn open(&self, user: &str, app: &str, spec: &iyakki_core_api::browsersession::OpenSpec) -> iyakki_core_api::browsersession::BrowserResult<iyakki_core_api::browsersession::SessionId> {
        self.0.sessions().open(user, app, spec)
    }
    fn close(&self, user: &str, app: &str, id: iyakki_core_api::browsersession::SessionId) {
        self.0.sessions().close(user, app, id)
    }
    fn navigate(&self, user: &str, app: &str, id: iyakki_core_api::browsersession::SessionId, nav: &iyakki_core_api::browsersession::Navigate) -> iyakki_core_api::browsersession::BrowserResult<()> {
        self.0.sessions().navigate(user, app, id, nav)
    }
    fn input(&self, user: &str, app: &str, id: iyakki_core_api::browsersession::SessionId, input: &iyakki_core_api::browsersession::Input) -> iyakki_core_api::browsersession::BrowserResult<()> {
        self.0.sessions().input(user, app, id, input)
    }
    fn find(&self, user: &str, app: &str, id: iyakki_core_api::browsersession::SessionId, query: &str, forward: bool) -> iyakki_core_api::browsersession::BrowserResult<iyakki_core_api::browsersession::FindState> {
        self.0.sessions().find(user, app, id, query, forward)
    }
    fn state(&self, user: &str, app: &str, id: iyakki_core_api::browsersession::SessionId) -> iyakki_core_api::browsersession::BrowserResult<iyakki_core_api::browsersession::PageState> {
        self.0.sessions().state(user, app, id)
    }
    fn frame(&self, user: &str, app: &str, id: iyakki_core_api::browsersession::SessionId, since: u64) -> iyakki_core_api::browsersession::BrowserResult<Option<iyakki_core_api::browsersession::Frame>> {
        self.0.sessions().frame(user, app, id, since)
    }
    fn prompts(&self, user: &str, app: &str, id: iyakki_core_api::browsersession::SessionId) -> Vec<iyakki_core_api::browsersession::Prompt> {
        self.0.sessions().prompts(user, app, id)
    }
    fn answer_prompt(&self, user: &str, app: &str, id: iyakki_core_api::browsersession::SessionId, prompt: u64, allow: bool) -> iyakki_core_api::browsersession::BrowserResult<()> {
        self.0.sessions().answer_prompt(user, app, id, prompt, allow)
    }
}
