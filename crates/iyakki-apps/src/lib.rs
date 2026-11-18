//! The apps behind the launcher tiles (chat, browser, settings, IDE, ...) and the device helpers they use (input, display, camera,
//! touch, hostname ...). Split out of `iyakki-shell` (alpha.4, "the apps split") so that a change to an app rebuilds and tests this
//! crate only, and a change to the shell no longer runs the app tests. Rule: nothing here may depend on `iyakki-shell`; the shell
//! depends on this crate and re-exports every module under its old path (`iyakki_shell::apps::...`, `crate::camera`).
// with the `testing` feature (the shell's tests) the test seams are compiled but only the shell's tests call some of them
#![cfg_attr(all(feature = "testing", not(test)), allow(dead_code))]
#[cfg(any(test, feature = "testing"))]
mod testmem;
pub mod agent_handles_b2;
#[cfg(any(test, feature = "testing"))]
pub mod testkit;
/// The accounts live in their own crate (`iyakki-accounts`); this keeps every `crate::accounts::...` path as it was.
pub mod accounts {
    pub use iyakki_accounts::*;
}
pub use iyakki_uikit::access;
pub use iyakki_uikit::gfx;
pub use iyakki_uikit::fx;
pub use iyakki_uikit::shaping;
pub use iyakki_uikit::theme;
pub use iyakki_uikit::uisize;
pub use iyakki_uikit::links;
pub use iyakki_uikit::mdtext;
pub use iyakki_uikit::safeclamp;
pub use iyakki_uikit::textsel;
pub use iyakki_uikit::ctxmenu;
pub use iyakki_uikit::ui;
pub use iyakki_uikit::scrolleng;
pub use iyakki_uikit::scrolllist;
pub use iyakki_uikit::listwindow;
pub use iyakki_uikit::tooltip;
pub mod filepicker;
#[cfg(test)]
mod filepicker_tests;
pub mod pathpick;
pub mod apps;
pub mod inputprofile;
pub mod camera;
pub mod display;
pub mod samples;
pub mod splash;
pub mod external;
pub mod agent_login;
pub mod agent_login_scan;
pub mod scan_core;
pub mod batterybar;
pub mod btmodel;
pub mod devpop;
pub mod hostname;
pub mod input;
pub mod touchpad;
pub mod touchmap;
pub mod touchscreen;
pub mod fnkeys;
pub mod touchbar;
#[cfg(test)]
mod touchbar_tests;
pub mod osk;
pub mod panes;
pub mod memgate;

/// True in this crate's own tests and wherever the `testing` feature is on (the shell's tests): code that must never reach the
/// real machine, the network or a real drive takes its made-up version. (`cfg!(test)` alone is false for a crate that is only a
/// dependency of the test binary.)
pub const TESTING: bool = cfg!(any(test, feature = "testing"));

/// The process-wide handle on the agent's functions (generated facade, `iyakki-agent-api`). Call sites with a `cx` use `cx.agent`; code that has none
/// (state machines, helpers, closures) uses this. The program that composes the UI (the shell's constructor, which owns the agent) puts the real
/// implementation in with [`install_agent`] at start; iyakki-apps itself does not name the implementation. Without one, every call answers
/// "unavailable" (the fake's defaults); in this crate's own tests and with the `testing` feature (the shell's tests) the real implementation is the default, so the existing tests keep their behaviour.
/// A new reader of stories and chat answers (the agent's, behind the facet).
pub fn new_reader() -> std::sync::Arc<dyn iyakki_agent_api::StoryReader> {
    agent().story().new_reader()
}

pub fn agent() -> &'static dyn iyakki_agent_api::AgentApi {
    agent_cell().as_ref()
}

/// Put `api` in place. Only the first call before any use of [`agent`] takes effect; returns whether it did.
pub fn install_agent(api: std::sync::Arc<dyn iyakki_agent_api::AgentApi>) -> bool {
    AGENT.set(api).is_ok()
}

/// Whether the handle in use is the real one. The composition root (`iyakki-compose`) installs it before anything reads [`agent`]: the first read
/// fills the cell with the default (the fake in a production build), and a later [`install_agent`] cannot replace it (KI-devmode-off-2026-11-17).
pub fn ensure_real_agent() -> bool {
    iyakki_agent_api::AgentFacets::is_real(agent())
}

/// The agent handle as an `Arc` (the shell keeps one beside the process-wide reference).
pub fn agent_arc() -> std::sync::Arc<dyn iyakki_agent_api::AgentApi> {
    agent_cell().clone()
}

static AGENT: std::sync::OnceLock<std::sync::Arc<dyn iyakki_agent_api::AgentApi>> = std::sync::OnceLock::new();

fn agent_cell() -> &'static std::sync::Arc<dyn iyakki_agent_api::AgentApi> {
    AGENT.get_or_init(default_agent)
}

#[cfg(not(any(test, feature = "testing")))]
fn default_agent() -> std::sync::Arc<dyn iyakki_agent_api::AgentApi> {
    std::sync::Arc::new(iyakki_agent_api::FakeAgentApi::default())
}

#[cfg(any(test, feature = "testing"))]
fn default_agent() -> std::sync::Arc<dyn iyakki_agent_api::AgentApi> {
    testkit::kit().map_or_else(|| std::sync::Arc::new(iyakki_agent_api::FakeAgentApi::default()) as std::sync::Arc<dyn iyakki_agent_api::AgentApi>, |k| (k.agent)())
}

/// The process-wide machine facts (`iyakki_core_api::HostFacts`), for code with no `cx` in reach: installed by the shell like [`install_agent`]; without one
/// every answer is empty (in this crate's tests and with the `testing` feature it is the real local one).
pub fn host() -> &'static dyn iyakki_core_api::HostFacts {
    HOST.get_or_init(default_host).as_ref()
}

/// Put `h` in place; only the first call before any use of [`host`] takes effect.
pub fn install_host(h: std::sync::Arc<dyn iyakki_core_api::HostFacts>) -> bool {
    HOST.set(h).is_ok()
}

static HOST: std::sync::OnceLock<std::sync::Arc<dyn iyakki_core_api::HostFacts>> = std::sync::OnceLock::new();

#[cfg(not(any(test, feature = "testing")))]
fn default_host() -> std::sync::Arc<dyn iyakki_core_api::HostFacts> {
    std::sync::Arc::new(iyakki_core_api::NoHostFacts)
}

#[cfg(any(test, feature = "testing"))]
fn default_host() -> std::sync::Arc<dyn iyakki_core_api::HostFacts> {
    testkit::kit().map_or_else(|| std::sync::Arc::new(iyakki_core_api::NoHostFacts) as std::sync::Arc<dyn iyakki_core_api::HostFacts>, |k| (k.host)())
}

/// The process-wide file scope (`iyakki_core_api::FileScope`: policy checks, home folders, path resolving), for code with no `cx` in reach; installed by the
/// shell like [`install_agent`]. Without one every check refuses (in this crate's tests and with the `testing` feature it is the real local one).
pub fn file_scope() -> &'static dyn iyakki_core_api::FileScope {
    FILE_SCOPE.get_or_init(default_file_scope).as_ref()
}

/// Put `f` in place; only the first call before any use of [`file_scope`] takes effect.
pub fn install_file_scope(f: std::sync::Arc<dyn iyakki_core_api::FileScope>) -> bool {
    FILE_SCOPE.set(f).is_ok()
}

static FILE_SCOPE: std::sync::OnceLock<std::sync::Arc<dyn iyakki_core_api::FileScope>> = std::sync::OnceLock::new();

#[cfg(not(any(test, feature = "testing")))]
fn default_file_scope() -> std::sync::Arc<dyn iyakki_core_api::FileScope> {
    std::sync::Arc::new(iyakki_core_api::NoFileScope)
}

#[cfg(any(test, feature = "testing"))]
fn default_file_scope() -> std::sync::Arc<dyn iyakki_core_api::FileScope> {
    testkit::kit().map_or_else(|| std::sync::Arc::new(iyakki_core_api::NoFileScope) as std::sync::Arc<dyn iyakki_core_api::FileScope>, |k| (k.file_scope)())
}

/// The process-wide microphones and recordings (`iyakki_shared::voicedesk::Recorders`: find microphones, start a capture), for code with no `cx` in reach;
/// installed by the shell like [`install_agent`]. Without one there are no microphones (in this crate's tests and with the `testing` feature it is the real
/// system one).
pub fn recorders() -> std::sync::Arc<dyn iyakki_shared::voicedesk::Recorders> {
    RECORDERS.get_or_init(default_recorders).clone()
}

/// Put `r` in place; only the first call before any use of [`recorders`] takes effect.
pub fn install_recorders(r: std::sync::Arc<dyn iyakki_shared::voicedesk::Recorders>) -> bool {
    RECORDERS.set(r).is_ok()
}

static RECORDERS: std::sync::OnceLock<std::sync::Arc<dyn iyakki_shared::voicedesk::Recorders>> = std::sync::OnceLock::new();

/// A machine with no microphone and no way to record.
pub struct NoRecorders;

impl iyakki_shared::voicedesk::Recorders for NoRecorders {
    fn mics(&self) -> Vec<iyakki_shared::voicedesk::Mic> {
        Vec::new()
    }
    fn product(&self) -> String {
        String::new()
    }
    fn start(&self, _cmd: &str, _max_secs: usize) -> Result<Box<dyn iyakki_shared::voicedesk::Capture>, String> {
        Err("this core cannot record".into())
    }
}

#[cfg(not(any(test, feature = "testing")))]
fn default_recorders() -> std::sync::Arc<dyn iyakki_shared::voicedesk::Recorders> {
    std::sync::Arc::new(NoRecorders)
}

#[cfg(any(test, feature = "testing"))]
fn default_recorders() -> std::sync::Arc<dyn iyakki_shared::voicedesk::Recorders> {
    agent().helpers_b7().system_recorders().unwrap_or_else(|| std::sync::Arc::new(NoRecorders))
}

/// The process-wide speaker (`iyakki_shared::voicedesk::Players`), like [`recorders`]: installed by the shell; without one nothing plays (in this crate's tests and
/// with the `testing` feature it is the real system one).
pub fn players() -> std::sync::Arc<dyn iyakki_shared::voicedesk::Players> {
    PLAYERS.get_or_init(default_players).clone()
}

/// Put `p` in place; only the first call before any use of [`players`] takes effect.
pub fn install_players(p: std::sync::Arc<dyn iyakki_shared::voicedesk::Players>) -> bool {
    PLAYERS.set(p).is_ok()
}

static PLAYERS: std::sync::OnceLock<std::sync::Arc<dyn iyakki_shared::voicedesk::Players>> = std::sync::OnceLock::new();

/// A machine that plays nothing.
pub struct NoPlayers;

impl iyakki_shared::voicedesk::Players for NoPlayers {
    fn play(&self, _cmd: &str, _wav: &[u8]) -> Result<Box<dyn iyakki_shared::voicedesk::Playback>, String> {
        Err("this core cannot play sound".into())
    }
}

#[cfg(not(any(test, feature = "testing")))]
fn default_players() -> std::sync::Arc<dyn iyakki_shared::voicedesk::Players> {
    std::sync::Arc::new(NoPlayers)
}

#[cfg(any(test, feature = "testing"))]
fn default_players() -> std::sync::Arc<dyn iyakki_shared::voicedesk::Players> {
    agent().helpers_b7().system_players().unwrap_or_else(|| std::sync::Arc::new(NoPlayers))
}

#[cfg(test)]
mod agent_handle_tests {
    #[test]
    fn the_process_handle_is_the_real_facade_unless_something_was_installed_first() {
        use iyakki_agent_api::AgentOther;
        let url = "https://x.test/?token=abc";
        assert_eq!(super::agent().devtools_scrub_url(url), iyakki_shared::devtools::scrub_url(url));
        // a second install cannot replace a handle that is already in use
        assert!(!super::install_agent(std::sync::Arc::new(iyakki_agent_api::FakeAgentApi::default())));
    }

    /// KI-devmode-off-2026-11-17: the composition root makes the real handle current before anything reads it.
    #[test]
    fn ensure_real_agent_leaves_the_real_handle_in_place() {
        assert!(super::ensure_real_agent());
        assert!(iyakki_agent_api::AgentFacets::is_real(super::agent()));
        assert!(!iyakki_agent_api::AgentFacets::is_real(&iyakki_agent_api::FakeAgentApi::default()));
    }
}
