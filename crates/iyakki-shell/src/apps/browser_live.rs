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
}

enum Cmd {
    Do(Interaction),
    Resize(iyakki_web::css::Viewport),
}

pub struct LiveSession {
    tx: Sender<Cmd>,
    rx: Receiver<Update>,
    pending: usize,
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
    pub fn start(mut live: LivePage, rebuild: Rebuild, first: iyakki_web::css::Viewport) -> Option<LiveSession> {
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
                                Update { doc: rebuild(&out.tree, &vp), notes, navigation: out.navigation, error: None }
                            }
                            Err(e) => Update { doc: rebuild(&live.tree(), &vp), notes: Vec::new(), navigation: None, error: Some(e.to_string()) },
                        },
                        // the window changed: the page's `resize` handlers and media-query lists hear about it, then it is built again
                        Cmd::Resize(new) => {
                            vp = new;
                            let error = live.resize(new).err().map(|e| e.to_string());
                            let navigation = live.take_navigation();
                            Update { doc: rebuild(&live.tree(), &vp), notes: Vec::new(), navigation, error }
                        }
                    };
                    if utx.send(u).is_err() {
                        break;
                    }
                }
            })
            .ok()?;
        Some(LiveSession { tx, rx, pending: 0 })
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
