//! The core's embedded browser sessions (F-SDK-62): the real [`iyakki_core_api::BrowserSessions`] over the in-house reader engine.
//! A session is one page and its history, owned by a person and an app. The core (this crate, through its [`BrowserHost`]) judges every
//! address before anything is sent, loads the page on a worker thread, lays it out ([`page`]) and answers with a display list; it never
//! shows an app anything but that list, a state and prompts. The host (the device's agent, or a test) supplies the transport, the
//! known-bad list, the person's host policy, the audit line and the place downloads go.

pub mod page;
mod session;

use std::sync::Arc;

use iyakki_web::Fetch;

pub use session::LocalBrowserSessions;

/// What the sessions need from the machine they run on. Every method but `fetch` has a safe default (nothing listed, everything allowed,
/// no audit, downloads refused), so a test names only what it uses.
pub trait BrowserHost: Send + Sync {
    /// The transport for this person (the person's own cache, cookies and the transport-level known-bad guard live behind it).
    fn fetch(&self, user: &str) -> Arc<dyn Fetch>;
    /// The known-bad list entry covering `host`: the listed site and why. Asked before a request and for every redirect.
    fn known_bad(&self, _host: &str) -> Option<(String, String)> {
        None
    }
    /// Whether the person's account may reach this host (the tenant and role policy).
    fn allows_host(&self, _user: &str, _host: &str) -> bool {
        true
    }
    /// The person's switch for embedded browsers (default on).
    fn enabled(&self, _user: &str) -> bool {
        true
    }
    /// Switch embedded browsers on or off for the person (the host keeps the switch). The sessions end themselves when it goes off.
    fn set_enabled(&self, _user: &str, _on: bool) -> Result<(), String> {
        Err("this machine keeps no switch for embedded browsers".into())
    }
    /// Whether the full engine helper is installed and the person allowed it.
    fn full_engine(&self, _user: &str) -> bool {
        false
    }
    /// One audit line: `event` is a dotted name (`browser.embed.open`), `detail` is short and holds no page content.
    fn audit(&self, _user: &str, _app: &str, _event: &str, _detail: &str) {}
    /// A download the person allowed: the core saves the file and says where (a sentence), or says why not.
    fn save_download(&self, _user: &str, _app: &str, _url: &str) -> Result<String, String> {
        Err("downloads are not switched on for embedded browsers".into())
    }
}

#[cfg(test)]
mod tests;
