//! The browser session facet of the core (F-SDK-60, docs/development/UI-SDK.md "Embedding the browser"). An SDK app that places a
//! `BrowserView` in its layout never touches a socket, a file or a device: the view asks the core for a *session* (one page, one history,
//! owned by a person and an app), and the core
//!
//! * judges every address before anything is sent (the known-bad site list, the person's tenant policy, the address rules) and logs it,
//! * loads the page with the engine the person's browser uses (the in-house reader engine, later the full engine),
//! * hands back the page as a *display list* (plain text of `DISPLAY_LIST_VERSION`), so the shell, a client and a WASM tile draw it the
//!   same way,
//! * takes the input back (a link, the wheel, keys, typed text) and
//! * routes what needs a person (a download, a permission) as a [`Prompt`] that is answered through the core, never by the page.
//!
//! This crate holds the types and the trait with default methods that say "unavailable"; `iyakki-core-fake` has the scripted fake, and
//! `iyakki-core-browser` the real session over the in-house engine. A person who never embeds a browser has no sessions and no cost.

use std::fmt;

/// The version of this facet (types and calls). Additive changes only; a breaking change is a new number.
pub const BROWSER_SESSION_VERSION: u32 = 1;
/// The most sessions one app of one person may hold open (a browser pane with tabs of a few views).
pub const MAX_SESSIONS_PER_APP: usize = 8;
/// The longest address or typed text a call takes.
pub const MAX_URL: usize = 2048;
pub const MAX_TEXT: usize = 4096;
/// The biggest page content an app may hand over (`Navigate::Html`).
pub const MAX_HTML: usize = 512 * 1024;

/// Which engine draws the page.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum EnginePref {
    /// The small in-house reader engine: no scripts, no plugins; fast and safe for documents, help pages and simple sites.
    #[default]
    InHouse,
    /// The full engine helper (scripts, video): the core says no when it is not installed or the person did not switch it on.
    Full,
    /// The full engine when it is available, else the in-house one; the state says which one was used.
    Auto,
}

impl EnginePref {
    pub const ALL: [EnginePref; 3] = [EnginePref::InHouse, EnginePref::Full, EnginePref::Auto];

    pub fn key(self) -> &'static str {
        match self {
            EnginePref::InHouse => "in-house",
            EnginePref::Full => "full",
            EnginePref::Auto => "auto",
        }
    }

    pub fn from_key(k: &str) -> Option<EnginePref> {
        EnginePref::ALL.iter().copied().find(|e| e.key() == k)
    }
}

/// The colours the page chrome is drawn in, from the app's Look (the core knows no Look: the view hands them over).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Palette {
    pub bg: [u8; 4],
    pub text: [u8; 4],
    pub muted: [u8; 4],
    pub link: [u8; 4],
    pub rule: [u8; 4],
    pub code_bg: [u8; 4],
    pub mark: [u8; 4],
}

impl Default for Palette {
    fn default() -> Palette {
        Palette { bg: [250, 250, 250, 255], text: [20, 20, 20, 255], muted: [100, 100, 100, 255], link: [20, 80, 200, 255], rule: [200, 200, 200, 255], code_bg: [236, 236, 236, 255], mark: [255, 224, 102, 255] }
    }
}

/// The colour scheme the page is asked to follow (`prefers-color-scheme`, and a forced-colours hint for high contrast).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Scheme {
    #[default]
    Dark,
    Light,
    /// High contrast: the reader draws thicker lines and underlines and no tinted marks; a full engine forces the colours.
    HighContrast,
}

/// The default font family of the page (a page's own choice wins in an engine that reads styles).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum PageFont {
    #[default]
    Sans,
    Mono,
}

/// What the app's theme and the person's settings say the page should default to.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PageDefaults {
    pub scheme: Scheme,
    /// The default text size in logical units before the text scale (15 normally).
    pub font_size: f32,
    pub font: PageFont,
    /// The person asked for reduced motion: nothing on the page may animate.
    pub reduced_motion: bool,
}

impl Default for PageDefaults {
    fn default() -> PageDefaults {
        PageDefaults { scheme: Scheme::Dark, font_size: 15.0, font: PageFont::Sans, reduced_motion: false }
    }
}

/// What an app asks for when it opens a session.
#[derive(Clone, Debug, PartialEq)]
pub struct OpenSpec {
    /// The size of the view in logical units; the frame is made for it (`Input::Resize` changes it).
    pub width: f32,
    pub height: f32,
    /// The person's text size factor (1.0 normal).
    pub text_scale: f32,
    pub engine: EnginePref,
    pub palette: Palette,
    pub defaults: PageDefaults,
}

impl OpenSpec {
    pub fn new(width: f32, height: f32) -> OpenSpec {
        OpenSpec { width, height, text_scale: 1.0, engine: EnginePref::InHouse, palette: Palette::default(), defaults: PageDefaults::default() }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct SessionId(pub u64);

/// A navigation. `Url` goes through the core's checks; `Html` is content the app brings (help text, a mail body): it is shown as a page
/// of its own address `base` (`about:` or an http address only for resolving links), never fetched, and its links are judged when followed.
#[derive(Clone, Debug, PartialEq)]
pub enum Navigate {
    Url(String),
    Html { base: String, html: String },
    Back,
    Forward,
    Reload,
    Stop,
}

/// Why the core did not do what was asked.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Refusal {
    /// The site is on the person's known-bad list (the host, why).
    KnownBad { host: String, reason: String },
    /// The person's account may not reach that host.
    Policy { host: String },
    /// Not an address the browser takes (a scheme other than http and https, too long, control characters).
    BadAddress(String),
    /// The person switched embedded browsers off, or this core has none.
    Off,
}

impl fmt::Display for Refusal {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Refusal::KnownBad { host, reason } if reason.is_empty() => write!(f, "{host} is on your known-bad site list"),
            Refusal::KnownBad { host, reason } => write!(f, "{host} is on your known-bad site list: {reason}"),
            Refusal::Policy { host } => write!(f, "your account may not open {host}"),
            Refusal::BadAddress(why) => write!(f, "that address cannot be opened: {why}"),
            Refusal::Off => write!(f, "embedded browsers are switched off"),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum BrowserError {
    /// This core has no browser sessions (a fake without them, a preview, a machine with the feature off), with the plain reason.
    Unavailable(String),
    Refused(Refusal),
    /// The session is closed, never existed, or belongs to another person or app (the answer is the same, so one cannot probe).
    NoSuchSession,
    /// The app holds [`MAX_SESSIONS_PER_APP`] already.
    TooMany,
    Invalid(String),
}

impl fmt::Display for BrowserError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            BrowserError::Unavailable(w) => write!(f, "{w}"),
            BrowserError::Refused(r) => write!(f, "{r}"),
            BrowserError::NoSuchSession => write!(f, "that browser view is closed"),
            BrowserError::TooMany => write!(f, "this app has too many browser views open"),
            BrowserError::Invalid(w) => write!(f, "{w}"),
        }
    }
}

impl std::error::Error for BrowserError {}

pub type BrowserResult<T> = Result<T, BrowserError>;

/// A key a page view understands (the SDK maps its own key type onto these).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PageKey {
    Up,
    Down,
    PageUp,
    PageDown,
    Home,
    End,
    Enter,
    Esc,
}

/// Input for a session.
#[derive(Clone, Debug, PartialEq)]
pub enum Input {
    /// A hit region of the last frame was used (its action number from the frame's `hit` lines): a link opens.
    Activate(u32),
    /// Positive = towards older content (up), like `UiEvent::Wheel`.
    Wheel(i32),
    Key(PageKey),
    /// Typed text; the in-house page has no text field yet, so it is ignored and counted nowhere.
    Text(String),
    /// The view was resized (logical units).
    Resize { width: f32, height: f32 },
}

/// The state of find in page.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct FindState {
    pub query: String,
    pub matches: usize,
    /// 1-based position of the match the view is at, 0 for none.
    pub current: usize,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PageState {
    /// The address of the page shown (after redirects); empty before the first load.
    pub url: String,
    pub title: String,
    pub loading: bool,
    pub can_back: bool,
    pub can_forward: bool,
    /// The plain reason the last load failed (the frame says it too).
    pub error: Option<String>,
    /// The engine that drew the page (never `Auto`).
    pub engine: EnginePref,
    pub find: FindState,
    /// Counts up whenever the frame changed; `frame(.., since)` returns nothing while it is the same.
    pub generation: u64,
    /// How far the page is scrolled, in thousandths of the way to the end.
    pub scroll_permille: u16,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PromptKind {
    /// The page leads to a file the reader cannot show: save it?
    Download,
    /// The page asks for a device or a right (camera, microphone, location, notifications): only the full engine raises these.
    Permission,
}

/// A question that needs a person. The answer comes back through `answer_prompt`; the page never sees who answered or how.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Prompt {
    pub id: u64,
    pub kind: PromptKind,
    /// The host that asks.
    pub origin: String,
    /// The question in plain words.
    pub text: String,
}

/// One frame: the display list text and the generation it belongs to.
#[derive(Clone, Debug, PartialEq)]
pub struct Frame {
    pub generation: u64,
    /// `iyakki-display-list` text (`iyakki_ui_sdk::list::DisplayList::parse` reads it); link regions are `hit` lines whose action is the
    /// number to send back in `Input::Activate`.
    pub list: String,
}

/// What an app may ask of the core for an embedded browser. Every call names the person and the app: the core checks both, so a view
/// of one app can never reach the session of another. Default methods say "unavailable", so a core without browsers needs no code.
pub trait BrowserSessions: Send + Sync {
    /// Whether this person may embed browsers (the person's switch). A core with no browser says false.
    fn enabled(&self, _user: &str) -> bool {
        false
    }
    /// The person's switch (the chat tool and Settings call it; switching off ends the person's open views).
    fn set_enabled(&self, _user: &str, _on: bool) -> BrowserResult<()> {
        Err(BrowserError::Unavailable("this core has no embedded browser".into()))
    }
    /// How many views the person has open now (for the status line).
    fn open_views(&self, _user: &str) -> usize {
        0
    }
    fn open(&self, _user: &str, _app: &str, _spec: &OpenSpec) -> BrowserResult<SessionId> {
        Err(BrowserError::Unavailable("this core has no embedded browser".into()))
    }
    /// Close a session (idempotent). A person's sessions also end at sign-out.
    fn close(&self, _user: &str, _app: &str, _id: SessionId) {}
    fn navigate(&self, _user: &str, _app: &str, _id: SessionId, _nav: &Navigate) -> BrowserResult<()> {
        Err(BrowserError::Unavailable("this core has no embedded browser".into()))
    }
    fn input(&self, _user: &str, _app: &str, _id: SessionId, _input: &Input) -> BrowserResult<()> {
        Err(BrowserError::Unavailable("this core has no embedded browser".into()))
    }
    /// Find in page. A new query starts at the first match; the same query again goes to the next (`forward`) or the previous one; an
    /// empty query clears the marks.
    fn find(&self, _user: &str, _app: &str, _id: SessionId, _query: &str, _forward: bool) -> BrowserResult<FindState> {
        Err(BrowserError::Unavailable("this core has no embedded browser".into()))
    }
    fn state(&self, _user: &str, _app: &str, _id: SessionId) -> BrowserResult<PageState> {
        Err(BrowserError::Unavailable("this core has no embedded browser".into()))
    }
    /// The frame, or `None` when its generation is still `since`.
    fn frame(&self, _user: &str, _app: &str, _id: SessionId, _since: u64) -> BrowserResult<Option<Frame>> {
        Err(BrowserError::Unavailable("this core has no embedded browser".into()))
    }
    fn prompts(&self, _user: &str, _app: &str, _id: SessionId) -> Vec<Prompt> {
        Vec::new()
    }
    fn answer_prompt(&self, _user: &str, _app: &str, _id: SessionId, _prompt: u64, _allow: bool) -> BrowserResult<()> {
        Err(BrowserError::NoSuchSession)
    }
}

/// The browser sessions of a core that has none.
pub struct NoBrowser;

impl BrowserSessions for NoBrowser {}

/// Check an address the way every implementation must before it asks anything else: length, control characters, and the scheme
/// (`http` and `https` only; `Html` content has its own rule). Returns the trimmed text.
pub fn check_address(raw: &str) -> Result<String, Refusal> {
    let t = raw.trim();
    if t.is_empty() {
        return Err(Refusal::BadAddress("empty".into()));
    }
    if t.len() > MAX_URL {
        return Err(Refusal::BadAddress("too long".into()));
    }
    if t.chars().any(char::is_control) {
        return Err(Refusal::BadAddress("control characters".into()));
    }
    if let Some((scheme, _)) = t.split_once("://") {
        if !scheme.eq_ignore_ascii_case("http") && !scheme.eq_ignore_ascii_case("https") {
            return Err(Refusal::BadAddress(format!("only http and https are opened, not {scheme}")));
        }
    } else if let Some((scheme, rest)) = t.split_once(':') {
        let looks_like_port = rest.chars().next().is_some_and(|c| c.is_ascii_digit());
        if !looks_like_port && scheme.chars().all(|c| c.is_ascii_alphabetic()) && scheme.len() < 12 {
            return Err(Refusal::BadAddress(format!("only http and https are opened, not {scheme}")));
        }
    }
    Ok(t.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_core_without_browsers_says_so_and_holds_nothing() {
        let c = NoBrowser;
        assert!(matches!(c.open("alice", "sample", &OpenSpec::new(300.0, 200.0)), Err(BrowserError::Unavailable(_))));
        assert!(c.prompts("alice", "sample", SessionId(1)).is_empty());
        assert_eq!(c.answer_prompt("alice", "sample", SessionId(1), 1, true), Err(BrowserError::NoSuchSession));
        assert!(matches!(c.frame("alice", "sample", SessionId(1), 0), Err(BrowserError::Unavailable(_))));
    }

    #[test]
    fn addresses_are_http_or_https_and_plain() {
        assert_eq!(check_address("  https://a.org/x ").unwrap(), "https://a.org/x");
        assert_eq!(check_address("a.org").unwrap(), "a.org");
        assert_eq!(check_address("localhost:8080/x").unwrap(), "localhost:8080/x");
        for bad in ["javascript:alert(1)", "file:///etc/passwd", "ftp://a.org/", "data:text/html,x", "", "https://a.org/\u{7}x"] {
            assert!(matches!(check_address(bad), Err(Refusal::BadAddress(_))), "{bad}");
        }
        assert!(check_address(&format!("https://a.org/{}", "x".repeat(MAX_URL))).is_err());
    }

    #[test]
    fn engine_keys_round_trip_and_the_text_of_a_refusal_names_the_host() {
        for e in EnginePref::ALL {
            assert_eq!(EnginePref::from_key(e.key()), Some(e));
        }
        assert_eq!(EnginePref::from_key("chrome"), None);
        assert!(Refusal::KnownBad { host: "bad.example".into(), reason: "phishing".into() }.to_string().contains("bad.example"));
    }
}
