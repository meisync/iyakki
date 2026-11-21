//! Scripted pages in the reader that react to the person (F-BJS-01, KI-716). The loader keeps the page's script engine alive
//! ([`iyakki_js::LivePage`]) on a thread of its own and hands this module its end of a channel: a click on a button, a check box
//! or a radio button of the drawn page is sent to the page's scripts, the changed page is built into a new document with the same style
//! sheets and window size as the first time, and the browser swaps it in where the old one was. The screen thread never waits for a
//! handler: it sends the interaction and looks for the answer on its next tick.
//!
//! One session per shown page. It lives while that page is the one in view; going to another page, back or forward, or loading again
//! drops it (and with it the engine). At most [`MAX_PENDING`] interactions wait at a time; more are refused with a note.
//!
//! Not done (KI-716): typing into text fields, links and other elements with handlers, a submit button inside a form (the reader
//! submits it itself), key events, timers that run while the page is looked at, and the full engine's video and canvas.

use std::sync::mpsc::{channel, Receiver, Sender, TryRecvError};
use std::sync::{Arc, Mutex};

use iyakki_js::{Interaction, LivePage};
use iyakki_web::html::Tree;
use iyakki_web::{Doc, Fetch, Url};

pub const MAX_PENDING: usize = 8;
/// The most live page engines at one time, across every tab (each is a thread with a script engine in it). The page that would be the
/// ninth is shown without its scripts reacting, with a note.
pub const MAX_LIVE: usize = 8;
static LIVE: Places = Places::new();

/// A count of places that is taken before the thing it counts starts, so two starting together cannot both get the last one.
pub struct Places(std::sync::atomic::AtomicUsize);

impl Places {
    pub const fn new() -> Places {
        Places(std::sync::atomic::AtomicUsize::new(0))
    }
    pub fn take(&self, max: usize) -> bool {
        if self.0.fetch_add(1, std::sync::atomic::Ordering::SeqCst) >= max {
            self.0.fetch_sub(1, std::sync::atomic::Ordering::SeqCst);
            return false;
        }
        true
    }
    pub fn give_back(&self) {
        self.0.fetch_sub(1, std::sync::atomic::Ordering::SeqCst);
    }
    pub fn used(&self) -> usize {
        self.0.load(std::sync::atomic::Ordering::SeqCst)
    }
}

/// How many live page engines run now.
pub fn live_count() -> usize {
    LIVE.used()
}

/// What the page says when its scripts ran once but no place was free to keep them running for clicks and keys.
pub fn no_place_note(max: usize) -> String {
    format!("{max} pages in your tabs already keep their scripts running, so this page was shown after its first run only: clicks and keys on it do not reach its scripts. Close a tab and open the page again to use them.")
}

/// How long a page is looked at between two timer ticks sent to its scripts.
pub const TICK_MS: u64 = 250;

/// A page tree built into what is drawn, with the style sheets and window size of the first build.
pub type Rebuild = Box<dyn Fn(&Tree, &iyakki_web::css::Viewport) -> Doc + Send>;

/// What a live page looks like after one interaction.
pub struct Update {
    pub doc: Doc,
    /// The page's console lines from this interaction, ready for the developer view.
    pub notes: Vec<String>,
    /// Where a script asked to go.
    pub navigation: Option<String>,
    /// The interaction could not be delivered (the page's engine refused it).
    pub error: Option<String>,
    /// A handler stopped the default action (a link is not followed, a key is not used by the reader).
    pub prevented: bool,
    /// A form the page sent that no `submit` handler stopped (by a submit button, Enter in a field, or `form.submit()`).
    pub submission: Option<iyakki_js::Submission>,
}

enum Cmd {
    Do(Interaction),
    Resize(iyakki_web::css::Viewport),
}

pub struct LiveSession {
    places: &'static Places,
    tx: Sender<Cmd>,
    rx: Receiver<Update>,
    pending: usize,
    last_tick: std::time::Instant,
}

impl Drop for LiveSession {
    fn drop(&mut self) {
        // the thread ends when its channel closes (this struct's `tx` goes with it): its place is free from now on
        self.places.give_back();
    }
}

/// A key of the reader as the page's `KeyboardEvent` has it: (`key`, `code`, modifier bits 1 ctrl, 2 shift, 4 alt, 8 meta).
pub fn dom_key(k: &crate::ui::Key) -> Option<(String, String, u8)> {
    use crate::ui::Key;
    let letter = |c: char, mods: u8| -> (String, String, u8) {
        let upper = c.is_ascii_uppercase();
        let code = if c.is_ascii_alphabetic() { format!("Key{}", c.to_ascii_uppercase()) } else if c.is_ascii_digit() { format!("Digit{c}") } else { String::new() };
        (c.to_string(), code, mods | if upper { 2 } else { 0 })
    };
    Some(match k {
        Key::Char(c) if !c.is_control() => letter(*c, 0),
        Key::Ctrl(c) => letter(c.to_ascii_lowercase(), 1 | if c.is_ascii_uppercase() { 2 } else { 0 }),
        Key::Alt(c) => letter(*c, 4),
        Key::Enter => ("Enter".into(), "Enter".into(), 0),
        Key::Backspace => ("Backspace".into(), "Backspace".into(), 0),
        Key::Delete => ("Delete".into(), "Delete".into(), 0),
        Key::Tab => ("Tab".into(), "Tab".into(), 0),
        Key::BackTab => ("Tab".into(), "Tab".into(), 2),
        Key::Esc => ("Escape".into(), "Escape".into(), 0),
        Key::Up => ("ArrowUp".into(), "ArrowUp".into(), 0),
        Key::Down => ("ArrowDown".into(), "ArrowDown".into(), 0),
        Key::Left => ("ArrowLeft".into(), "ArrowLeft".into(), 0),
        Key::Right => ("ArrowRight".into(), "ArrowRight".into(), 0),
        Key::Home => ("Home".into(), "Home".into(), 0),
        Key::End => ("End".into(), "End".into(), 0),
        Key::PageUp => ("PageUp".into(), "PageUp".into(), 0),
        Key::PageDown => ("PageDown".into(), "PageDown".into(), 0),
        Key::F(n) if (1..=12).contains(n) => (format!("F{n}"), format!("F{n}"), 0),
        _ => return None,
    })
}

/// A page built into a document for a window size: the page's style sheets are fetched through the same checks as the page, so the
/// same page can be built again for another window (F-BROW-28).
pub type Builder = Arc<dyn Fn(&Url, &Tree, &iyakki_web::css::Viewport) -> Doc + Send + Sync>;

/// What the loader leaves for the page it made: how to build it again, the window it was built for, whether its scripts ran, and the
/// engine that was kept for it (if any).
pub struct PageKit {
    pub built: Builder,
    /// The window size the page was last built for (and the person's motion and contrast choices, which do not change with it).
    pub vp: iyakki_web::css::Viewport,
    pub scripted: bool,
    pub session: Option<LiveSession>,
}

/// A place the loader thread puts the kit it made, for the browser to take once the page has arrived.
pub type Slot = Arc<Mutex<Option<PageKit>>>;

pub fn new_slot() -> Slot {
    Arc::new(Mutex::new(None))
}

/// Takes what the loader left in the slot.
pub fn take(slot: &Slot) -> Option<PageKit> {
    slot.lock().ok().and_then(|mut s| s.take())
}

impl LiveSession {
    /// Starts the thread that owns the page's engine.
    pub fn start(live: LivePage, rebuild: Rebuild, first: iyakki_web::css::Viewport) -> Option<LiveSession> {
        Self::start_in(&LIVE, MAX_LIVE, live, rebuild, first)
    }

    /// Like [`LiveSession::start`], but also the sentence for the page when no place was free (the page is then shown as it came out of
    /// its first run: its scripts do not hear clicks or keys until a tab is closed and the page is opened again).
    pub fn start_with_note(live: LivePage, rebuild: Rebuild, first: iyakki_web::css::Viewport) -> (Option<LiveSession>, Option<String>) {
        Self::start_noted(&LIVE, MAX_LIVE, live, rebuild, first)
    }

    fn start_noted(places: &'static Places, max: usize, live: LivePage, rebuild: Rebuild, first: iyakki_web::css::Viewport) -> (Option<LiveSession>, Option<String>) {
        match Self::start_in(places, max, live, rebuild, first) {
            Some(s) => (Some(s), None),
            None => (None, Some(no_place_note(max))),
        }
    }

    fn start_in(places: &'static Places, max: usize, mut live: LivePage, rebuild: Rebuild, first: iyakki_web::css::Viewport) -> Option<LiveSession> {
        // the place is taken first so two pages starting together cannot both take the last one
        if !places.take(max) {
            return None;
        }
        let (tx, irx) = channel::<Cmd>();
        let (utx, rx) = channel::<Update>();
        std::thread::Builder::new()
            .name("iyakki-live-page".into())
            .stack_size(32 << 20)
            .spawn(move || {
                // ends when the browser drops its end: the engine goes with it
                let mut vp = first;
                while let Ok(cmd) = irx.recv() {
                    let u = match cmd {
                        Cmd::Do(what) => match live.interact(&what) {
                            Ok(out) => {
                                let notes = out.console.iter().take(50).map(|l| format!("console.{}: {}", l.level, l.text.chars().take(300).collect::<String>())).collect();
                                Update { doc: rebuild(&out.tree, &vp), notes, navigation: out.navigation, error: None, prevented: out.prevented, submission: out.submission }
                            }
                            Err(e) => Update { doc: rebuild(&live.tree(), &vp), notes: Vec::new(), navigation: None, error: Some(e.to_string()), prevented: false, submission: None },
                        },
                        // the window changed: the page's `resize` handlers and media-query lists hear about it, then it is built again
                        Cmd::Resize(new) => {
                            vp = new;
                            let error = live.resize(new).err().map(|e| e.to_string());
                            let navigation = live.take_navigation();
                            Update { doc: rebuild(&live.tree(), &vp), notes: Vec::new(), navigation, error, prevented: false, submission: None }
                        }
                    };
                    if utx.send(u).is_err() {
                        break;
                    }
                }
            })
            .map_err(|_| places.give_back())
            .ok()?;
        Some(LiveSession { places, tx, rx, pending: 0, last_tick: std::time::Instant::now() })
    }

    /// Time passes for a page that is looked at: its timers run (a clock, a slide show, an animation). Sent at most every [`TICK_MS`] and
    /// never while the page is still busy, so a slow page does not pile work up. Returns whether a tick was sent.
    pub fn tick(&mut self, now: std::time::Instant) -> bool {
        let since = now.saturating_duration_since(self.last_tick);
        if self.pending > 0 || since < std::time::Duration::from_millis(TICK_MS) {
            return false;
        }
        let ms = since.as_millis().min(5_000) as u64;
        self.last_tick = now;
        self.send(Interaction::Tick(ms))
    }

    /// Sends one interaction; false when too many are waiting already.
    pub fn send(&mut self, what: Interaction) -> bool {
        if self.pending >= MAX_PENDING {
            return false;
        }
        if self.tx.send(Cmd::Do(what)).is_err() {
            return false;
        }
        self.pending += 1;
        true
    }

    /// An answer that has arrived, if any. `Err` when the thread is gone.
    pub fn poll(&mut self) -> Result<Option<Update>, ()> {
        match self.rx.try_recv() {
            Ok(u) => {
                self.pending = self.pending.saturating_sub(1);
                Ok(Some(u))
            }
            Err(TryRecvError::Empty) => Ok(None),
            Err(TryRecvError::Disconnected) => Err(()),
        }
    }

    /// Waits up to `within` for an answer (for a host that has nothing else to do; the screen thread uses [`LiveSession::poll`]).
    pub fn wait(&mut self, within: std::time::Duration) -> Result<Option<Update>, ()> {
        match self.rx.recv_timeout(within) {
            Ok(u) => {
                self.pending = self.pending.saturating_sub(1);
                Ok(Some(u))
            }
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => Ok(None),
            Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => Err(()),
        }
    }

    /// The window changed size: the page is told and built again for it.
    pub fn resize(&mut self, vp: iyakki_web::css::Viewport) -> bool {
        if self.pending >= MAX_PENDING || self.tx.send(Cmd::Resize(vp)).is_err() {
            return false;
        }
        self.pending += 1;
        true
    }

    pub fn pending(&self) -> usize {
        self.pending
    }
}

/// Builds a page tree into a document: the page's style sheets are fetched through the same checks as the page (the account's hosts,
/// cookies, a size limit, nothing unprotected on a secure page), or the plain reader view when styles are off.
#[allow(clippy::too_many_arguments)]
pub fn page_builder(web: Arc<dyn Fetch>, allow: impl Fn(&str) -> bool + Send + Sync + 'static, store: Arc<Mutex<iyakki_web::CookieStore>>, now: i64, secure_page: bool, styles_on: bool) -> Builder {
    use iyakki_web::{Jar, Request};
    let sheet = move |u: &Url| -> Option<String> {
        // a secure page does not load unprotected style sheets (anyone on the network could swap them): never asked for (F-BROW-98)
        if secure_page && !u.https {
            return None;
        }
        let (final_req, r) = iyakki_web::fetch_following(web.as_ref(), Request { priority: iyakki_web::net::BLOCKING, ..Request::get(u) }, &allow, Some(Jar { store: &store, now })).ok()?;
        if secure_page && !final_req.url.https {
            return None; // a redirect that ended unprotected
        }
        let ct = r.content_type.to_ascii_lowercase();
        (r.status < 400 && r.body.len() <= iyakki_web::style::MAX_SHEET_BYTES && (ct.is_empty() || ct.contains("css") || ct.contains("text/plain"))).then(|| String::from_utf8_lossy(&r.body).into_owned())
    };
    Arc::new(move |u: &Url, tree: &Tree, vp: &iyakki_web::css::Viewport| -> Doc {
        // the pictures are chosen for the window the page is built for
        iyakki_web::imgsel::with_window_dppx(vp.width, vp.dppx, || {
        if styles_on {
            let mut d = iyakki_web::style::styled_doc(tree, u, vp, &sheet);
            // a page that brings its own style sheets was drawn for a white page, as every browser shows it
            if d.canvas.is_none() && (tree.find("style").is_some() || tree.find("link").is_some()) {
                d.canvas = Some([255, 255, 255, 255]);
            }
            d
        } else {
            iyakki_web::doc::build(tree)
        }
        })
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ui::Key;

    fn session(html: &str) -> LiveSession {
        struct Nothing;
        impl iyakki_web::Fetch for Nothing {
            fn get(&self, _: &Url) -> Result<iyakki_web::Response, String> {
                Err("offline".into())
            }
        }
        let u = Url::parse("https://a.org/").unwrap();
        let live = LivePage::load(html, &u, Arc::new(Nothing), |_| true, &iyakki_js::SiteData::default(), iyakki_js::Limits::default(), Vec::new()).expect("engine");
        let vp = iyakki_web::css::Viewport::default();
        LiveSession::start(live, Box::new(|t, _| iyakki_web::doc::build(t)), vp).expect("a place was free")
    }

    fn page() -> LivePage {
        struct Nothing;
        impl iyakki_web::Fetch for Nothing {
            fn get(&self, _: &Url) -> Result<iyakki_web::Response, String> {
                Err("offline".into())
            }
        }
        let u = Url::parse("https://a.org/").unwrap();
        LivePage::load("<p id=c>0</p><script>1</script>", &u, Arc::new(Nothing), |_| true, &iyakki_js::SiteData::default(), iyakki_js::Limits::default(), Vec::new()).expect("engine")
    }

    #[test]
    fn a_page_that_finds_no_place_says_so_in_words_and_a_freed_place_is_used_again() {
        // a pool of its own: the real one is shared with every other test
        let pool: &'static Places = Box::leak(Box::new(Places::new()));
        let vp = iyakki_web::css::Viewport::default();
        let rebuild = || -> Rebuild { Box::new(|t, _| iyakki_web::doc::build(t)) };
        let (first, note) = LiveSession::start_noted(pool, 1, page(), rebuild(), vp);
        assert!(first.is_some() && note.is_none(), "the first page keeps its engine");
        let (second, note) = LiveSession::start_noted(pool, 1, page(), rebuild(), vp);
        assert!(second.is_none(), "the next page finds the one place taken");
        let note = note.expect("and is told");
        assert!(note.contains("1 pages") && note.contains("Close a tab") && note.contains("do not reach its scripts"), "{note}");
        assert_eq!(pool.used(), 1, "a refused page takes nothing");
        drop(first);
        assert_eq!(pool.used(), 0, "closing the page gives its place back");
        assert!(LiveSession::start_noted(pool, 1, page(), rebuild(), vp).0.is_some());
    }

    #[test]
    fn places_are_taken_before_the_thing_starts_and_given_back() {
        let p = Places::new();
        assert!((0..3).all(|_| p.take(3)));
        assert!(!p.take(3), "the fourth finds no place");
        assert_eq!(p.used(), 3);
        p.give_back();
        assert!(p.take(3));
    }

    #[test]
    fn a_session_answers_keys_submits_and_ticks_with_what_the_page_did() {
        let mut s = session("<form action=/go><input name=q value=x><button id=b>Go</button></form><p id=c>0</p><script>let n = 0; setInterval(() => { n++; document.getElementById('c').textContent = String(n); }, 250); document.addEventListener('keydown', e => { if (e.key === 'q') e.preventDefault(); });</script>");
        assert!(s.send(Interaction::Key { node: None, key: "q".into(), code: "KeyQ".into(), mods: 0, up: false }));
        let u = s.wait(std::time::Duration::from_secs(20)).unwrap().expect("an answer");
        assert!(u.prevented && u.submission.is_none());
        let t = std::time::Instant::now() + std::time::Duration::from_secs(2);
        assert!(s.tick(t), "a tick goes out once enough time has passed");
        assert!(!s.tick(t), "and not again while the page is busy");
        let u = s.wait(std::time::Duration::from_secs(20)).unwrap().expect("the tick answer");
        assert!(u.doc.blocks.iter().any(|b| matches!(b, iyakki_web::Block::Text(t) if t.spans.iter().any(|x| x.text.parse::<u32>().is_ok_and(|n| n >= 1)))), "the timer ran");
        assert!(s.send(Interaction::Key { node: None, key: "Enter".into(), code: "Enter".into(), mods: 0, up: false }));
        let _ = s.wait(std::time::Duration::from_secs(20));
    }

    #[test]
    fn reader_keys_become_the_keys_a_page_expects() {
        assert_eq!(dom_key(&Key::Char('a')), Some(("a".into(), "KeyA".into(), 0)));
        assert_eq!(dom_key(&Key::Char('A')), Some(("A".into(), "KeyA".into(), 2)));
        assert_eq!(dom_key(&Key::Char('7')), Some(("7".into(), "Digit7".into(), 0)));
        assert_eq!(dom_key(&Key::Ctrl('s')), Some(("s".into(), "KeyS".into(), 1)));
        assert_eq!(dom_key(&Key::Ctrl('S')), Some(("s".into(), "KeyS".into(), 3)));
        assert_eq!(dom_key(&Key::Alt('x')), Some(("x".into(), "KeyX".into(), 4)));
        assert_eq!(dom_key(&Key::Esc), Some(("Escape".into(), "Escape".into(), 0)));
        assert_eq!(dom_key(&Key::Right).unwrap().0, "ArrowRight");
        assert_eq!(dom_key(&Key::F(5)).unwrap().0, "F5");
        assert_eq!(dom_key(&Key::F(40)), None);
        assert_eq!(dom_key(&Key::Char('\u{7}')), None);
    }
}
