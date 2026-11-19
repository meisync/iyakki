//! The known-bad site list, the person's side (F-BROW-98): the list text and the switch live next to the person's other browser
//! choices; `apply` puts them into the shared [`iyakki_web::badsites::Guard`] that the browser's network consults before any request
//! (the transport is wrapped in shell.rs). Changing the list is done by the chat tools (each asks first); a list is downloaded only
//! when the person asks, from an address they give or from a suggested one, and is never shipped.
//!
//! Default: ON, and harmless while the list is empty. Every failure leaves pages loading as if there were no list.

use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use iyakki_web::badsites::{BadSites, Guard, Parsed, MAX_BYTES};
use iyakki_web::blocker_lists::ListSource;

use super::AppCx;

fn who(cx: &AppCx) -> String {
    super::browser_config::who(cx)
}

pub fn list_path(cx: &AppCx) -> PathBuf {
    cx.data_dir().join(format!("browser-badsites-{}.txt", who(cx)))
}

fn config_path(cx: &AppCx) -> PathBuf {
    cx.data_dir().join(format!("browser-badsites-{}.json", who(cx)))
}

pub fn enabled(cx: &AppCx) -> bool {
    super::browser_config::read_checked("known-bad site settings", &config_path(cx)).and_then(|t| serde_json::from_str::<serde_json::Value>(&t).ok()).and_then(|v| v["enabled"].as_bool()).unwrap_or(true)
}

pub fn set_enabled(cx: &AppCx, on: bool) {
    super::browser_config::write_checked("known-bad site settings", &config_path(cx), &serde_json::json!({ "enabled": on }).to_string());
    apply(cx);
}

pub fn load_list(cx: &AppCx) -> BadSites {
    let mut b = BadSites::default();
    if let Ok(text) = std::fs::read_to_string(list_path(cx)) {
        if text.len() <= MAX_BYTES {
            b.read_text(&text);
        }
    }
    b
}

pub fn save_list(cx: &AppCx, b: &BadSites) {
    let p = list_path(cx);
    let tmp = p.with_extension("tmp");
    if std::fs::write(&tmp, b.to_text()).is_ok() {
        let _ = std::fs::rename(&tmp, &p);
    }
}

/// Which person's list the shared guard holds (path, enabled, length of the text), so an unchanged list is not read again.
static LOADED: Mutex<Option<(PathBuf, bool, u64, u64)>> = Mutex::new(None);

fn stamp(p: &std::path::Path) -> (u64, u64) {
    std::fs::metadata(p).map(|m| (m.len(), m.modified().ok().and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok()).map_or(0, |d| d.as_nanos() as u64))).unwrap_or((0, 0))
}

/// Put this person's list and switch into the shared guard. Cheap when nothing changed.
pub fn apply(cx: &AppCx) {
    #[cfg(any(test, feature = "testing"))]
    {
        // a test that looks at the shared guard pins it to its own list; other tests browsing meanwhile leave it alone
        if PIN.lock().ok().and_then(|p| p.clone()).is_some_and(|d| d != list_path(cx)) {
            return;
        }
    }
    let (path, on) = (list_path(cx), enabled(cx));
    let (len, at) = stamp(&path);
    let Ok(mut loaded) = LOADED.lock() else { return };
    if loaded.as_ref().is_some_and(|l| l.0 == path && l.1 == on && l.2 == len && l.3 == at) {
        return;
    }
    Guard::global().set(load_list(cx), on);
    *loaded = Some((path, on, len, at));
}

/// Make the next `apply` read the list again.
pub fn forget_loaded() {
    if let Ok(mut l) = LOADED.lock() {
        *l = None;
    }
}

pub fn add(cx: &AppCx, host: &str, reason: &str) -> Result<String, String> {
    let mut b = load_list(cx);
    let h = b.add(host, reason)?;
    save_list(cx, &b);
    forget_loaded();
    apply(cx);
    Ok(h)
}

pub fn remove(cx: &AppCx, host: &str) -> bool {
    let mut b = load_list(cx);
    let gone = b.remove(host);
    if gone {
        save_list(cx, &b);
        forget_loaded();
        apply(cx);
    }
    gone
}

pub fn clear(cx: &AppCx) -> usize {
    let n = load_list(cx).len();
    let _ = std::fs::remove_file(list_path(cx));
    forget_loaded();
    apply(cx);
    n
}

/// Adds the sites of a list text to the person's list.
pub fn import_text(path: &std::path::Path, text: &str) -> Result<Parsed, String> {
    let mut b = BadSites::default();
    if let Ok(old) = std::fs::read_to_string(path) {
        b.read_text(&old);
    }
    let p = b.read_text(text);
    let tmp = path.with_extension("tmp");
    std::fs::write(&tmp, b.to_text()).map_err(|e| format!("the list could not be saved ({e})"))?;
    std::fs::rename(&tmp, path).map_err(|e| format!("the list could not be saved ({e})"))?;
    Ok(p)
}

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
    let j = job_status();
    !j.is_empty() && !j.starts_with("Done")
}

/// Downloads a list from `url` in the background and adds its sites. False when a download is running.
pub fn start_download(path: PathBuf, src: Arc<dyn ListSource>, url: String) -> bool {
    if busy() {
        return false;
    }
    set_job("Working…");
    let spawned = std::thread::Builder::new().name("iyakki-badsite-list".into()).spawn(move || {
        let r = src.get(&url, MAX_BYTES).and_then(|body| import_text(&path, &String::from_utf8_lossy(&body)));
        forget_loaded();
        set_job(&match r {
            Ok(p) => format!("Done: {} sites read{}{}", p.added, if p.skipped > 0 { format!(", {} lines skipped", p.skipped) } else { String::new() }, if p.truncated { ", the list was cut at its limit" } else { "" }),
            Err(e) => format!("Done: failed ({e})"),
        });
    });
    if spawned.is_err() {
        set_job("Done: could not start");
        return false;
    }
    true
}

pub fn source(cx: &AppCx) -> Arc<dyn ListSource> {
    super::browser_block::source(cx)
}

#[cfg(any(test, feature = "testing"))]
static PIN: Mutex<Option<PathBuf>> = Mutex::new(None);

/// One test at a time looks at the shared guard (tests only).
#[cfg(any(test, feature = "testing"))]
pub(crate) static TEST_LOCK: Mutex<()> = Mutex::new(());

/// While alive, `apply` only accepts the person whose list this is (tests only).
#[cfg(any(test, feature = "testing"))]
pub(crate) struct Pin;

#[cfg(any(test, feature = "testing"))]
impl Pin {
    pub(crate) fn to(cx: &AppCx) -> Pin {
        if let Ok(mut p) = PIN.lock() {
            *p = Some(list_path(cx));
        }
        Pin
    }
}

#[cfg(any(test, feature = "testing"))]
impl Drop for Pin {
    fn drop(&mut self) {
        if let Ok(mut p) = PIN.lock() {
            *p = None;
        }
    }
}
