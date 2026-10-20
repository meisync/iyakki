//! Does the desktop window's web engine have speech synthesis (`window.speechSynthesis`)?
//!
//! Asked once, in the background, when somebody first wants to read a story aloud in window mode: the engine's helper is
//! started with no window, runs one line of script on a blank page and is closed again. WebKitGTK builds often come without
//! speech synthesis (it needs speech-dispatcher at build time); then the browser voice says so instead of failing.

use iyakki_webview::proto::VERSION;
use iyakki_webview::{Config, Event, Hello, ToHelper, WebView};
use std::path::PathBuf;
use std::time::{Duration, Instant};

/// The script that decides it.
pub const SCRIPT: &str = "typeof window.speechSynthesis === 'object' && typeof window.SpeechSynthesisUtterance === 'function'";

/// Start the helper `helper`, ask the page, quit. `Ok(true)` when the engine can speak, `Ok(false)` when it cannot, `Err`
/// when the helper could not be asked at all (it did not start, or did not answer within `limit`).
pub fn probe(helper: PathBuf, env: Option<Vec<(String, String)>>, profile: PathBuf, limit: Duration) -> Result<bool, String> {
    let _ = std::fs::create_dir_all(&profile);
    let hello = Hello { version: VERSION, w: 320, h: 240, scale_pct: 100, max_w: 320, max_h: 240, user_agent: String::new(), profile: profile.to_string_lossy().into_owned(), locale: String::new(), allow_hosts: vec!["*".into()] };
    let mut cfg = Config::new(helper, hello);
    cfg.env = env;
    cfg.cwd = Some(profile);
    let mut wv = WebView::start(cfg)?;
    let started = Instant::now();
    let mut asked = false;
    let result = loop {
        if started.elapsed() > limit {
            break Err("the web engine did not answer in time".to_string());
        }
        let mut answer: Option<Result<bool, String>> = None;
        for e in wv.poll() {
            match e {
                Event::Ready { .. } => {
                    wv.send(ToHelper::Navigate("about:blank".into()));
                }
                Event::Loading { loading: false, .. } if !asked => {
                    asked = true;
                    wv.send(ToHelper::Script { id: 1, js: SCRIPT.into(), timeout_ms: 5000 });
                }
                Event::ScriptResult { id: 1, ok: true, json } => answer = Some(Ok(json.trim() == "true")),
                Event::ScriptResult { id: 1, ok: false, json } => answer = Some(Err(format!("the web engine could not run the check: {json}"))),
                Event::Exited(why) | Event::Crashed(why) => answer = Some(Err(format!("the web engine stopped: {why}"))),
                _ => {}
            }
        }
        if let Some(r) = answer {
            break r;
        }
        std::thread::sleep(Duration::from_millis(40));
    };
    wv.send(ToHelper::Quit);
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Runs the real helper of this checkout (WebKitGTK through python3-gi and Xvfb): ignored by default because it needs them.
    #[test]
    #[ignore]
    fn the_real_engine_reports_whether_it_can_speak() {
        iyakki_agent::webengine::set_host_mode(true);
        let helper = iyakki_agent::webengine::embedded_helper().expect("a helper (set IYAKKI_WEBVIEW_HELPER)");
        let dir = std::env::temp_dir().join(format!("iyakki-speechprobe-{}", std::process::id()));
        let r = probe(helper, None, dir.clone(), Duration::from_secs(40));
        let _ = std::fs::remove_dir_all(&dir);
        eprintln!("speech synthesis in the real engine: {r:?}");
        assert!(r.is_ok(), "{r:?}");
    }
}
