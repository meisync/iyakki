//! Types moved out of `iyakki_agent::webengine` (F-SPLIT-03, scripted by dist/split/move_types.py); the agent re-exports them.



pub const NOT_INSTALLED: &str = "The full web engine is not installed in this image (see docs/FULL-BROWSER.md)";

/// The help the banner's button opens: an HTML page (kept short; the long version is docs/FULL-BROWSER.md).
pub const FULL_ENGINE_HELP_HTML: &str = "<html><head><title>The full web engine</title></head><body>\
<h1>The full web engine</h1>\
<p>The built-in reader shows documents and runs small scripts. YouTube, Gmail, Google Docs, Netflix, Slack, Notion, X, Facebook, Instagram and Maps need a full web engine (fast JavaScript, video, WebGL). This image does not have one.</p>\
<h2>Ways to get it</h2>\
<ul>\
<li>Boot the <b>web</b> image (any PC) or the <b>macweb</b> image (Intel Macs such as the Mac mini 2014): the engine is WPE WebKit in a helper program, and the BROWSER tile uses it by itself or when you press F.</li>\
<li>Use <b>window mode</b> on a Linux desktop (./run.sh window): the host's WebKitGTK is the engine. On a Mac the window uses the built-in reader only.</li>\
</ul>\
<h2>Good to know</h2>\
<ul>\
<li>YouTube plays VP9 and Opus on the web images without H.264. For H.264 and AAC (TikTok and similar) the BROWSER offers the optional audio and video add-on, downloaded on your click.</li>\
<li>Sign-in and video need the clock to be right: a wrong date breaks HTTPS.</li>\
<li>Details: docs/FULL-BROWSER.md and docs/YOUTUBE-ON-IYAKKI.md in the source tree.</li>\
</ul></body></html>";

/// A folder name made only of letters, digits, `-`, `_` and `.` (never `.` alone or `..`), so a user name
/// cannot reach outside the profile directory.
pub fn safe_name(user: &str) -> String {
    let s: String = user.chars().map(|c| if c.is_ascii_alphanumeric() || c == '-' || c == '_' { c } else { '_' }).take(64).collect();
    if s.is_empty() { "_".into() } else { s }
}

/// The cage compositor and the cog browser of the `web` image.
pub const CAGE: &str = "/usr/bin/cage";
pub const COG: &str = "/usr/bin/cog";

/// Is the web engine of the `web` image in this image?
pub fn available() -> bool {
    std::path::Path::new(CAGE).is_file() && std::path::Path::new(COG).is_file()
}

/// Set by the desktop window: it runs on a development machine's desktop.
pub static HOST_MODE: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// The desktop window (`iyakki-desktop`) says it runs on a development machine's desktop, so the helper may be the
/// WebKitGTK one of this repository and runs with the desktop's own environment.
pub fn set_host_mode(on: bool) {
    HOST_MODE.store(on, std::sync::atomic::Ordering::Relaxed);
}

/// A desktop session (window mode on a development machine, not the `web` image).
pub fn host_session() -> bool {
    HOST_MODE.load(std::sync::atomic::Ordering::Relaxed) && !available()
}
