//! What a user has chosen for the browser: scripts (everywhere, or only for sites they allow), pictures, style sheets and
//! cookies. Kept in one small file per user next to their other settings; the browser reads it before every page, so a
//! change made in Settings applies at once.

use std::collections::BTreeSet;

use iyakki_web::SearchEngine;

use super::AppCx;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BrowserConfig {
    /// Run scripts on every site (when off, only on `js_sites`). On by default.
    pub js_all: bool,
    /// Hosts the user has allowed scripts for.
    pub js_sites: BTreeSet<String>,
    /// Fetch and show pictures.
    pub images: bool,
    /// Fetch and apply style sheets.
    pub styles: bool,
    /// Accept cookies (off: none are sent or kept).
    pub cookies: bool,
    /// Where words typed into the address bar are searched.
    pub search: SearchEngine,
    /// The program that plays video and audio files, with its arguments (the address is added last). Empty: look for
    /// mpv, VLC, ffplay or mplayer.
    pub player: String,
    /// How much memory and disk the page cache may use (Settings -> Browser, "Speed and memory").
    pub cache: iyakki_web::net::CacheSize,
    /// "Smooth pages": the full web engine draws at half the width and height (a quarter of the pixels) and the shell stretches the
    /// picture: much smoother scrolling and video on a computer without a graphics driver for its engine, with softer text.
    pub smooth: bool,
    /// The tallest video the full engine is asked for, in pixels (0 = no limit): a cap keeps a slow computer smooth (F-BROX-18).
    pub video_max_height: u32,
    /// May the full engine use the audio and video add-on's decoders when it is installed (F-BROX-20). On by default; off means the pages
    /// that need H.264 or AAC are told the engine cannot play them.
    pub codecs: bool,
    /// The person's own short words for the address bar: word -> address with `%s` where the search words go.
    pub short_words: std::collections::BTreeMap<String, String>,
}

/// A short word is one to twelve lowercase letters or digits; the address must be https (or http) and hold `%s`. Anything else is refused.
pub fn valid_short_word(word: &str, template: &str) -> Result<(), String> {
    crate::agent().browsershort_valid(word, template)
}

impl Default for BrowserConfig {
    fn default() -> Self {
        // everything is on, scripts included (owner decision 2026-10-28: video, maps, mail and most recorders need them); the person can
        // block scripts everywhere, or allow them only for chosen sites, in Settings -> Browser
        BrowserConfig { js_all: true, js_sites: BTreeSet::new(), images: true, styles: true, cookies: true, search: SearchEngine::default(), player: String::new(), cache: iyakki_web::net::CacheSize::Auto, smooth: false, video_max_height: 0, codecs: true, short_words: Default::default() }
    }
}

impl BrowserConfig {
    pub fn scripts_for(&self, host: &str) -> bool {
        self.js_all || self.js_sites.contains(host)
    }
}

thread_local! {
    /// The "smooth pages" choice of the person whose settings were last read or saved on this thread (the shell draws and handles
    /// keys on one thread): the browser asks it on every heartbeat, which a file read could not afford.
    static SMOOTH: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// The "smooth pages" choice as last read or saved (see [`BrowserConfig::smooth`]).
pub fn smooth_now() -> bool {
    SMOOTH.with(|s| s.get())
}

pub(super) fn who(cx: &AppCx) -> String {
    cx.session.user.chars().filter(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.')).collect()
}

pub(crate) fn path(cx: &AppCx) -> std::path::PathBuf {
    cx.data_dir().join(format!("browser-{}.json", who(cx)))
}

/// The text of a person's browser file. A file that is not valid JSON is moved to `<file>.corrupt-<time>` first (at most three are
/// kept, an audit line says so) and then reads as missing, so the defaults are used and the damaged bytes are never overwritten.
pub(super) fn read_checked(what: &str, p: &std::path::Path) -> Option<String> {
    let text = std::fs::read_to_string(p).ok()?;
    if serde_json::from_str::<serde_json::Value>(&text).is_err() {
        let _ = iyakki_schema::read_plain::<serde_json::Value>(what, p, iyakki_schema::now_secs());
        return None;
    }
    Some(text)
}

/// Write a person's browser file atomically, unless a damaged file that could not be moved aside is in the way.
pub(super) fn write_checked(what: &str, p: &std::path::Path, text: &str) {
    if let Err(e) = iyakki_schema::plain_writable(what, p) {
        iyakki_schema::report(e);
        return;
    }
    let tmp = p.with_extension("tmp");
    if std::fs::write(&tmp, text).is_ok() {
        let _ = std::fs::rename(&tmp, p);
    }
}

fn old_sites_path(cx: &AppCx) -> std::path::PathBuf {
    cx.data_dir().join(format!("browser-js-{}.json", who(cx)))
}

fn clean_hosts<I: IntoIterator<Item = String>>(list: I) -> BTreeSet<String> {
    list.into_iter().filter(|h| !h.is_empty() && h.len() < 254 && h.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '.' | '_'))).take(500).collect()
}

/// The video heights a person can choose (0 = no limit).
pub const VIDEO_CAPS: [u32; 5] = [0, 1080, 720, 480, 360];

/// The words of a video cap.
pub fn cap_words(h: u32) -> String {
    if h == 0 { "Automatic (no limit)".into() } else { format!("up to {h} pixels tall") }
}

pub fn load(cx: &AppCx) -> BrowserConfig {
    let mut c = BrowserConfig::default();
    if let Some(text) = read_checked("browser settings", &path(cx)) {
        if let Ok(v) = serde_json::from_str::<serde_json::Value>(&text) {
            c.js_all = v["js_all"].as_bool().unwrap_or(c.js_all);
            c.images = v["images"].as_bool().unwrap_or(c.images);
            c.styles = v["styles"].as_bool().unwrap_or(c.styles);
            c.cookies = v["cookies"].as_bool().unwrap_or(c.cookies);
            c.search = v["search"].as_str().and_then(SearchEngine::from_id).unwrap_or(c.search);
            c.player = v["player"].as_str().filter(|p| cx.agent.voice_validate_command(p).is_ok()).unwrap_or("").to_string();
            c.cache = v["cache"].as_str().map(iyakki_web::net::CacheSize::from_id).unwrap_or(c.cache);
            c.smooth = v["smooth"].as_bool().unwrap_or(c.smooth);
            c.video_max_height = v["video_max_height"].as_u64().map(|h| h as u32).filter(|h| VIDEO_CAPS.contains(h)).unwrap_or(0);
            c.codecs = v["codecs"].as_bool().unwrap_or(c.codecs);
            if let Some(m) = v["short_words"].as_object() {
                c.short_words = m.iter().filter_map(|(k, t)| t.as_str().filter(|t| valid_short_word(k, t).is_ok()).map(|t| (k.clone(), t.to_string()))).take(50).collect();
            }
            c.js_sites = clean_hosts(v["js_sites"].as_array().into_iter().flatten().filter_map(|h| h.as_str().map(str::to_string)));
        }
    } else if let Ok(text) = std::fs::read_to_string(old_sites_path(cx)) {
        // the first version kept only the list of sites
        let list: Vec<String> = serde_json::from_str(&text).unwrap_or_default();
        c.js_sites = clean_hosts(list);
    }
    SMOOTH.with(|s| s.set(c.smooth));
    c
}

pub fn save(cx: &AppCx, c: &BrowserConfig) {
    let p = path(cx);
    let json = serde_json::json!({ "js_all": c.js_all, "js_sites": c.js_sites, "images": c.images, "styles": c.styles, "cookies": c.cookies, "search": c.search.id(), "player": c.player, "cache": c.cache.id(), "smooth": c.smooth, "video_max_height": c.video_max_height, "codecs": c.codecs, "short_words": c.short_words });
    SMOOTH.with(|s| s.set(c.smooth));
    write_checked("browser settings", &p, &json.to_string());
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::apps::tests::cx_with_admin_store;

    #[test]
    fn the_defaults_have_everything_on_scripts_included() {
        let c = BrowserConfig::default();
        assert!(c.js_all && c.images && c.styles && c.cookies);
        assert!(c.scripts_for("a.org"));
    }

    #[test]
    fn choices_are_saved_per_user_and_read_back_and_bad_files_fall_back_to_defaults() {
        let cx = cx_with_admin_store("bcfg1");
        assert_eq!(load(&cx), BrowserConfig::default());
        let mut c = BrowserConfig { js_all: true, images: false, search: SearchEngine::DuckDuckGo, ..BrowserConfig::default() };
        c.js_sites.insert("a.org".into());
        save(&cx, &c);
        assert_eq!(load(&cx), c);
        assert!(c.scripts_for("anything.example"), "all sites");
        // another user has their own
        let mut other = cx.clone();
        other.session.user = "amy".into();
        assert_eq!(load(&other), BrowserConfig::default());
        // a damaged file does no harm
        std::fs::write(super::path(&cx), "{ not json").unwrap();
        assert_eq!(load(&cx), BrowserConfig::default());
        std::fs::write(super::path(&cx), r#"{"js_sites":["ok.org","bad host!","../x"],"images":"yes"}"#).unwrap();
        let d = load(&cx);
        assert_eq!(d.js_sites.iter().cloned().collect::<Vec<_>>(), vec!["ok.org".to_string()], "only valid host names");
        assert!(d.images, "a value of the wrong type is ignored");
    }

    #[test]
    fn the_first_versions_site_list_is_carried_over() {
        let cx = cx_with_admin_store("bcfg2");
        std::fs::write(super::old_sites_path(&cx), r#"["old.org"]"#).unwrap();
        let c = load(&cx);
        assert!(c.scripts_for("old.org"));
        assert!(c.js_sites.contains("old.org"), "the old list is carried into the allowed sites");
        assert!(c.images && c.styles);
    }
}

/// The decoders choice that reaches the engine: the person's own, unless the tenant's `media.codecs` denies them (it can only narrow).
pub fn effective_codecs(person: bool, tenant: Option<bool>) -> bool {
    person && tenant != Some(false)
}

#[cfg(test)]
mod codecs_tests {
    use super::effective_codecs;

    #[test]
    fn the_tenant_can_turn_the_decoders_off_but_never_on() {
        assert!(effective_codecs(true, None) && effective_codecs(true, Some(true)));
        assert!(!effective_codecs(true, Some(false)));
        assert!(!effective_codecs(false, None) && !effective_codecs(false, Some(true)) && !effective_codecs(false, Some(false)));
    }
}
