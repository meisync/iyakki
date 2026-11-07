//! Block ads and trackers, the person's side: the switch and the "allow on this site" list (one small file per person next to
//! their other browser choices), their filter lists (downloaded only when they click, see `iyakki_web::blocker_lists`), and the
//! glue to both engines: the reader's network is wrapped by `iyakki_web::blocker_state::FilteredFetch` (shell.rs), the full web
//! engine gets the same rules as WebKit's content-blocker JSON in its profile folder (`prepare_full_engine`).
//!
//! Default: ON, and harmless until the person has downloaded a list (no list, no rules, nothing blocked). Every failure leaves
//! pages loading as if there were no blocker.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use iyakki_web::blocker_lists::{FetchSource, ListStore, KNOWN};
use iyakki_web::blocker_state::Filter;

use super::AppCx;

/// The person's choices.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BlockConfig {
    pub enabled: bool,
    /// Sites where blocking is off (the person's "allow on this site").
    pub allow: BTreeSet<String>,
}

impl Default for BlockConfig {
    fn default() -> Self {
        BlockConfig { enabled: true, allow: BTreeSet::new() }
    }
}

fn who(cx: &AppCx) -> String {
    super::browser_config::who(cx)
}

fn config_path(cx: &AppCx) -> PathBuf {
    cx.services.dir().join(format!("browser-block-{}.json", who(cx)))
}

/// The folder of this person's lists.
pub fn lists_dir(cx: &AppCx) -> PathBuf {
    cx.services.dir().join("browser-lists").join(who(cx))
}

pub fn store(cx: &AppCx) -> ListStore {
    ListStore::open(lists_dir(cx))
}

fn clean_hosts(list: impl IntoIterator<Item = String>) -> BTreeSet<String> {
    list.into_iter().map(|h| h.to_ascii_lowercase()).filter(|h| !h.is_empty() && h.len() < 254 && h.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '.' | '_'))).take(500).collect()
}

pub fn load(cx: &AppCx) -> BlockConfig {
    let mut c = BlockConfig::default();
    if let Some(text) = super::browser_config::read_checked("browser blocker settings", &config_path(cx)) {
        if let Ok(v) = serde_json::from_str::<serde_json::Value>(&text) {
            c.enabled = v["enabled"].as_bool().unwrap_or(c.enabled);
            c.allow = clean_hosts(v["allow"].as_array().into_iter().flatten().filter_map(|h| h.as_str().map(str::to_string)));
        }
    }
    c
}

pub fn save(cx: &AppCx, c: &BlockConfig) {
    let json = serde_json::json!({ "enabled": c.enabled, "allow": c.allow });
    super::browser_config::write_checked("browser blocker settings", &config_path(cx), &json.to_string());
}

/// Which person's lists the shared filter holds, and what they looked like (the meta file's text).
static LOADED: Mutex<Option<(PathBuf, String)>> = Mutex::new(None);

fn signature(dir: &Path) -> String {
    std::fs::read_to_string(dir.join("lists.meta")).unwrap_or_default()
}

/// Put this person's choices and lists into the shared filter. Cheap when nothing changed; compiles the lists when they did.
pub fn apply(cx: &AppCx) {
    #[cfg(test)]
    {
        // a test that looks at the shared filter pins it to its own folder; other tests browsing meanwhile leave it alone
        if PIN.lock().ok().and_then(|p| p.clone()).is_some_and(|d| d != lists_dir(cx)) {
            return;
        }
    }
    let c = load(cx);
    let filter = Filter::global();
    filter.configure(c.enabled, &c.allow);
    if !c.enabled {
        return;
    }
    let dir = lists_dir(cx);
    let sig = signature(&dir);
    let Ok(mut loaded) = LOADED.lock() else { return };
    if loaded.as_ref().is_some_and(|(d, s)| *d == dir && *s == sig) {
        return;
    }
    filter.set_blocker(ListStore::open(&dir).compile());
    *loaded = Some((dir, sig));
}

/// Make the next `apply` read the lists again.
pub fn forget_loaded() {
    if let Ok(mut l) = LOADED.lock() {
        *l = None;
    }
}

/// Write (or remove) the rule list the full web engine loads, and return the variables that point the helper at it. Called
/// before the engine starts; unchanged rules keep their identifier, so WebKit's compiled copy is reused.
pub fn prepare_full_engine(cx: &AppCx, profile: &Path) -> Vec<(String, String)> {
    let c = load(cx);
    let json_path = profile.join("content-filter.json");
    let id_path = profile.join("content-filter.id");
    let remove = || {
        let _ = std::fs::remove_file(&json_path);
        let _ = std::fs::remove_file(&id_path);
    };
    if !c.enabled {
        remove();
        return Vec::new();
    }
    apply(cx);
    let blocker = Filter::global().blocker();
    if blocker.rule_count() == 0 && blocker.stats().cosmetic == 0 {
        remove();
        return Vec::new();
    }
    let allow: Vec<String> = c.allow.iter().cloned().collect();
    let out = iyakki_web::blocker_json::export_allowing(&blocker, iyakki_web::blocker_json::DEFAULT_LIMIT, true, &allow);
    if out.rules == 0 {
        remove();
        return Vec::new();
    }
    let id: String = iyakki_web::blocker_lists::sha256_hex(out.json.as_bytes()).chars().take(24).collect();
    let old = std::fs::read_to_string(&id_path).unwrap_or_default();
    if old.trim() != id || !json_path.is_file() {
        let tmp = profile.join("content-filter.json.tmp");
        if std::fs::write(&tmp, &out.json).is_err() || std::fs::rename(&tmp, &json_path).is_err() {
            remove();
            return Vec::new();
        }
        // the rules changed: the compiled copies of the old ones are of no use
        let _ = std::fs::remove_dir_all(profile.join("content-filter-store"));
        let _ = std::fs::write(&id_path, &id);
    }
    iyakki_agent::webengine::filter_env(profile)
}

/// What a background job (download, update) is doing, for Settings.
static JOB: Mutex<String> = Mutex::new(String::new());

pub fn job_status() -> String {
    JOB.lock().map(|j| j.clone()).unwrap_or_default()
}

fn set_job(s: &str) {
    if let Ok(mut j) = JOB.lock() {
        *j = s.to_string();
    }
}

pub fn busy() -> bool {
    !job_status().is_empty() && !job_status().starts_with("Done")
}

pub fn now_secs() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

/// What a download job does.
#[derive(Clone, Debug)]
pub enum Job {
    /// The lists EasyList and EasyPrivacy (the ones listed with their licences in Settings).
    DownloadStandard,
    /// A list from an address the person typed.
    AddUrl(String),
    /// A file the person chose.
    AddFile(PathBuf),
    /// Download again every list that has an address.
    UpdateAll,
}

/// Run a job on a thread of its own (the network can take long) with a source of list text. Returns false when one is running.
pub fn start_job(dir: PathBuf, src: Arc<dyn iyakki_web::blocker_lists::ListSource>, job: Job) -> bool {
    if busy() {
        return false;
    }
    set_job("Working…");
    let spawned = std::thread::Builder::new().name("iyakki-block-lists".into()).spawn(move || {
        let r = run_job(&ListStore::open(&dir), src.as_ref(), &job, now_secs());
        forget_loaded();
        set_job(&format!("Done: {r}"));
    });
    if spawned.is_err() {
        set_job("Done: could not start");
        return false;
    }
    true
}

/// The work of a job, in the calling thread (tests call this directly). One line for the status bar.
pub fn run_job(store: &ListStore, src: &dyn iyakki_web::blocker_lists::ListSource, job: &Job, now: u64) -> String {
    match job {
        Job::DownloadStandard => {
            let mut done = Vec::new();
            let mut failed = Vec::new();
            for k in KNOWN.iter().filter(|k| k.id == "easylist" || k.id == "easyprivacy") {
                match store.add_known(k, src, now) {
                    Ok(m) => done.push(format!("{} ({} rules)", m.name, m.rules)),
                    Err(e) => failed.push(format!("{}: {e}", k.name)),
                }
            }
            summary(&done, &failed)
        }
        Job::AddUrl(u) => match store.add_url(u, src, now) {
            Ok(m) => format!("added {} ({} rules)", m.name, m.rules),
            Err(e) => format!("not added: {e}"),
        },
        Job::AddFile(p) => match store.add_file(p, now) {
            Ok(m) => format!("added {} ({} rules)", m.name, m.rules),
            Err(e) => format!("not added: {e}"),
        },
        Job::UpdateAll => {
            let (mut done, mut failed) = (Vec::new(), Vec::new());
            for m in store.list().into_iter().filter(|m| !m.url.is_empty()) {
                match store.refresh(&m.id, src, now) {
                    Ok(n) => done.push(format!("{} ({} rules)", n.name, n.rules)),
                    Err(e) => failed.push(format!("{}: {e}", m.name)),
                }
            }
            if done.is_empty() && failed.is_empty() {
                "no list with an address to update".to_string()
            } else {
                summary(&done, &failed)
            }
        }
    }
}

fn summary(done: &[String], failed: &[String]) -> String {
    let mut s = String::new();
    if !done.is_empty() {
        s.push_str(&format!("updated {}", done.join(", ")));
    }
    if !failed.is_empty() {
        if !s.is_empty() {
            s.push_str("; ");
        }
        s.push_str(&format!("failed {}", failed.join("; ")));
    }
    s
}

/// The weekly refresh: at the first page load after start, lists with an address that are a week old are fetched again, on a
/// thread, once per run. Nothing happens for a person with no lists or with blocking off.
pub fn weekly_refresh(cx: &AppCx) {
    static DONE: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
    if load(cx).enabled && !DONE.swap(true, std::sync::atomic::Ordering::Relaxed) {
        let dir = lists_dir(cx);
        let web = cx.web.for_user(&cx.session.user).unwrap_or_else(|| cx.web.clone());
        let now = now_secs();
        if ListStore::open(&dir).list().iter().any(|m| !m.url.is_empty() && m.enabled && now.saturating_sub(m.updated) >= iyakki_web::blocker_lists::REFRESH_AFTER) {
            let _ = std::thread::Builder::new().name("iyakki-block-weekly".into()).spawn(move || {
                let _ = ListStore::open(&dir).refresh_due(&FetchSource(web), now);
                forget_loaded();
            });
        }
    }
}

/// The address of a list source for a real download: the browser's own network, as this person.
pub fn source(cx: &AppCx) -> Arc<dyn iyakki_web::blocker_lists::ListSource> {
    Arc::new(FetchSource(cx.web.for_user(&cx.session.user).unwrap_or_else(|| cx.web.clone())))
}

/// "7 days ago" for a time in seconds since 1970.
pub fn ago(then: u64, now: u64) -> String {
    let d = now.saturating_sub(then);
    match d {
        0..=59 => "just now".into(),
        60..=3599 => format!("{} min ago", d / 60),
        3600..=86399 => format!("{} h ago", d / 3600),
        _ => format!("{} days ago", d / 86400),
    }
}

/// Toggle "allow on this site" for `host` (which must be a plain host name); the new state.
pub fn toggle_allow(cx: &AppCx, host: &str) -> bool {
    let mut c = load(cx);
    let host = host.to_ascii_lowercase();
    let on = if c.allow.remove(&host) {
        false
    } else {
        c.allow.insert(host);
        true
    };
    c.allow = clean_hosts(c.allow);
    save(cx, &c);
    on
}

/// Tests share the one process-wide filter: they take turns.
#[cfg(test)]
pub(crate) static TEST_LOCK: Mutex<()> = Mutex::new(());

#[cfg(test)]
static PIN: Mutex<Option<PathBuf>> = Mutex::new(None);

/// While alive, `apply` only accepts the person whose lists folder this is (tests only).
#[cfg(test)]
pub(crate) struct Pin;

#[cfg(test)]
impl Pin {
    pub(crate) fn to(cx: &AppCx) -> Pin {
        if let Ok(mut p) = PIN.lock() {
            *p = Some(lists_dir(cx));
        }
        Pin
    }
}

#[cfg(test)]
impl Drop for Pin {
    fn drop(&mut self) {
        if let Ok(mut p) = PIN.lock() {
            *p = None;
        }
    }
}

#[cfg(test)]
#[path = "browser_block_tests.rs"]
mod tests;
