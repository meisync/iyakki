//! The web page inside an app's page (F-SDK-14): what the tile screen draws for an `iyakki.embed` of kind `browser`. It is the page engine's
//! reader on its own, not the Browser tile: the page is read with `iyakki_web::fetch::load_request`, laid out with the browser's layout code
//! and drawn into the rectangle the app named. It keeps to the rules of the bridge:
//!
//! * it may visit only the hosts the app declared that the person's own host policy also allows (the intersection arrives as `hosts`); a link
//!   or a redirect to any other host is not followed, and the viewer says so in a strip with the host named;
//! * its cookies live in a jar of their own, named after the app (`embed:<person>:<app>`), in memory only: never the person's browser's jar,
//!   never written to a disk, shared with no other app;
//! * no scripts, no downloads, no forms sent, no pictures from other sites, nothing published to the assistant, no history entry: a page is
//!   read and its links followed, nothing more.
//!
//! The fetch runs on its own thread; the screen asks [`PageEmbed::poll`] each frame.

use crate::safeclamp::SafeClamp;
use std::collections::HashMap;
use std::sync::mpsc::{channel, Receiver, TryRecvError};
use std::sync::{Arc, Mutex, OnceLock};

use iyakki_web::cookies::CookieStore;
use iyakki_web::doc::Doc;
use iyakki_web::fetch::{Jar, Loaded, Request};
use iyakki_web::url::Url;

use super::browser_layout::{layout, Kind, Layout};
use super::AppCx;
use crate::gfx::{Face, Gfx};
use crate::ui::{DIM, ERR, MUTED, R, TEXT};

/// The height of the strip that names a refused site (before the scale).
const STRIP_H: f32 = 22.0;

/// The jars of the embedded pages, by profile name. In memory for the life of the process; a profile is one app of one person.
fn jars() -> &'static Mutex<HashMap<String, Arc<Mutex<CookieStore>>>> {
    static J: OnceLock<Mutex<HashMap<String, Arc<Mutex<CookieStore>>>>> = OnceLock::new();
    J.get_or_init(Default::default)
}

/// The jar of one profile (made empty the first time).
pub fn jar_of(profile: &str) -> Arc<Mutex<CookieStore>> {
    jars().lock().unwrap_or_else(|e| e.into_inner()).entry(profile.to_string()).or_default().clone()
}

/// Is this host one the app declared and the person's policy allowed? Exact names, no wildcards.
pub fn host_allowed(hosts: &[String], host: &str) -> bool {
    hosts.iter().any(|h| h.eq_ignore_ascii_case(host))
}

enum State {
    Idle,
    Loading(Receiver<Result<Loaded, String>>),
    Shown { url: Url, doc: Doc },
    Failed(String),
}

pub struct PageEmbed {
    hosts: Vec<String>,
    profile: String,
    state: State,
    layout: Option<(Layout, (i32, i32))>,
    /// How far the page is scrolled, in screen pixels.
    pub scroll: f32,
    content_h: f32,
    view_h: f32,
    /// The address a click or a redirect asked for that the viewer refused (shown in the strip).
    left: Option<String>,
    /// The links of the last frame: where each was drawn on the screen and its index in the page.
    links: Vec<(R, usize)>,
    /// The addresses of the pages shown before this one, for Back.
    history: Vec<Url>,
    /// The pages Back left, for Forward; a new page the person or a link chose empties it.
    ahead: Vec<Url>,
    /// The address being loaded or last shown, for Reload.
    wanted: Option<String>,
}

impl PageEmbed {
    pub fn new(hosts: Vec<String>, profile: &str) -> PageEmbed {
        PageEmbed { hosts, profile: profile.to_string(), state: State::Idle, layout: None, scroll: 0.0, content_h: 0.0, view_h: 0.0, left: None, links: Vec::new(), history: Vec::new(), ahead: Vec::new(), wanted: None }
    }

    /// The address of the page shown now.
    pub fn url(&self) -> Option<String> {
        match &self.state {
            State::Shown { url, .. } => Some(url.to_string()),
            _ => None,
        }
    }

    pub fn loading(&self) -> bool {
        matches!(self.state, State::Loading(_))
    }

    pub fn failure(&self) -> Option<&str> {
        match &self.state {
            State::Failed(e) => Some(e),
            _ => None,
        }
    }

    /// The host an earlier click or redirect was refused for, if the strip is up.
    pub fn refused(&self) -> Option<&str> {
        self.left.as_deref()
    }

    /// Load an address (an app's `options.url`, or a link). A host that is not allowed is refused in words before anything is requested.
    pub fn open(&mut self, address: &str, cx: &AppCx) {
        self.ahead.clear();
        self.load(address, cx);
    }

    fn load(&mut self, address: &str, cx: &AppCx) {
        let url = match Url::parse(address) {
            Ok(u) => u,
            Err(e) => {
                self.state = State::Failed(e);
                return;
            }
        };
        if !host_allowed(&self.hosts, &url.host) {
            self.left = Some(url.host.clone());
            if matches!(self.state, State::Idle) {
                self.state = State::Failed(format!("This app may not show {}. It may visit: {}.", url.host, self.hosts.join(", ")));
            }
            return;
        }
        self.left = None;
        self.wanted = Some(address.to_string());
        if let State::Shown { url: now, .. } = &self.state {
            self.history.push(now.clone());
            self.history.truncate(20);
        }
        let (tx, rx) = channel();
        let web = cx.web.for_user(&cx.session.user).unwrap_or_else(|| cx.web.clone());
        let (hosts, policy, jar) = (self.hosts.clone(), cx.session.policy.clone(), jar_of(&self.profile));
        let now = iyakki_clock::now_unix();
        std::thread::spawn(move || {
            let allow = |h: &str| host_allowed(&hosts, h) && policy.allows_host(h);
            // no scripts: the plain reader turns the HTML into the page; the jar is this app's own
            let plain = |_: &Url, html: &str| (iyakki_web::doc::from_html(html), Vec::new());
            let r = iyakki_web::fetch::load_request(&*web, Request::get(&url), &allow, &plain, Some(Jar { store: &jar, now }));
            let _ = tx.send(r);
        });
        self.state = State::Loading(rx);
        self.scroll = 0.0;
        self.layout = None;
    }

    /// Take the answer of the fetch, if it came. True when the picture changed.
    pub fn poll(&mut self) -> bool {
        let State::Loading(rx) = &self.state else { return false };
        match rx.try_recv() {
            Ok(Ok(Loaded::Page { url, doc, .. })) => {
                if !host_allowed(&self.hosts, &url.host) {
                    // a redirect the fetch already refused cannot end here, but a page that names itself elsewhere is checked again
                    self.left = Some(url.host);
                    self.state = State::Failed("The page sent the viewer to a site this app may not show.".into());
                } else {
                    self.state = State::Shown { url, doc };
                }
                true
            }
            Ok(Ok(Loaded::Unsupported { url, .. })) => {
                self.state = State::Failed(format!("{} is a file, not a page: the embedded viewer shows pages only and keeps no downloads.", url.host));
                true
            }
            Ok(Err(e)) => {
                if e.to_ascii_lowercase().contains("not allowed") || e.to_ascii_lowercase().contains("may not") {
                    self.left = Some(e.clone());
                }
                self.state = State::Failed(e);
                true
            }
            Err(TryRecvError::Empty) => false,
            Err(TryRecvError::Disconnected) => {
                self.state = State::Failed("The page could not be read.".into());
                true
            }
        }
    }

    /// Scroll by wheel steps (positive: up).
    pub fn scroll_by(&mut self, steps: i32) {
        self.scroll = (self.scroll - steps as f32 * 48.0).clamp_safe(0.0, (self.content_h - self.view_h).max(0.0));
    }

    /// Back to the page shown before this one.
    pub fn back(&mut self, cx: &AppCx) -> bool {
        match self.history.pop() {
            Some(u) => {
                let here = self.url().and_then(|a| Url::parse(&a).ok());
                let was = std::mem::take(&mut self.history);
                self.load(&u.to_string(), cx);
                self.history = was;
                if let Some(h) = here {
                    self.ahead.push(h);
                }
                true
            }
            None => false,
        }
    }

    /// Forward to the page Back left. False when there is none.
    pub fn forward(&mut self, cx: &AppCx) -> bool {
        match self.ahead.pop() {
            Some(u) => {
                let ahead = std::mem::take(&mut self.ahead);
                self.load(&u.to_string(), cx);
                self.ahead = ahead;
                true
            }
            None => false,
        }
    }

    /// Load the page again (the same address, no new entry of history). False when nothing was ever asked for.
    pub fn reload(&mut self, cx: &AppCx) -> bool {
        let Some(a) = self.wanted.clone() else { return false };
        let (was, ahead) = (std::mem::take(&mut self.history), std::mem::take(&mut self.ahead));
        // the page shown now is not pushed on to the history by loading itself again
        self.state = State::Idle;
        self.load(&a, cx);
        self.history = was;
        self.ahead = ahead;
        true
    }

    /// Stop a load that is under way (the page shown before stays gone; the viewer says so). False when nothing was loading.
    pub fn stop(&mut self) -> bool {
        if !self.loading() {
            return false;
        }
        self.state = State::Failed("Stopped.".into());
        true
    }

    /// Apply one `embed.call` of the app (`back`, `forward`, `reload`, `stop`); `focus` and `blur` need nothing here. True when the method is one of these and did something.
    pub fn apply_call(&mut self, method: &str, cx: &AppCx) -> bool {
        match method {
            "back" => self.back(cx),
            "forward" => self.forward(cx),
            "reload" => self.reload(cx),
            "stop" => self.stop(),
            _ => false,
        }
    }

    /// A press at (x, y) in screen pixels: a link is followed when its host is allowed, otherwise the strip names the host. True when it was a link.
    pub fn click(&mut self, x: f32, y: f32, cx: &AppCx) -> bool {
        let Some(i) = self.links.iter().rev().find(|(r, _)| r.contains(x as i32, y as i32)).map(|(_, i)| *i) else { return false };
        let State::Shown { url, doc } = &self.state else { return false };
        let Some(href) = doc.links.get(i) else { return false };
        match url.join(href) {
            Ok(next) => {
                let next = next.to_string();
                self.open(&next, cx);
                true
            }
            Err(_) => false,
        }
    }

    /// Draw the page inside `rect` (clipped by the caller to what is on screen).
    pub fn paint(&mut self, g: &mut Gfx, rect: R, s: f32) {
        g.rrect(rect.x, rect.y, rect.w, rect.h, 6.0 * s, crate::gfx::rgb(20, 26, 48));
        let strip = if self.left.is_some() { STRIP_H * s } else { 0.0 };
        let body = R::new(rect.x, rect.y, rect.w, (rect.h - strip).max(0.0));
        self.view_h = body.h;
        self.links.clear();
        let note = |g: &mut Gfx, t: &str, col: [u8; 4]| {
            for (i, l) in g.wrap(13.0 * s, t, Face::Sans, body.w - 16.0 * s).iter().take(6).enumerate() {
                g.text(body.x + 8.0 * s, body.y + 8.0 * s + i as f32 * 18.0 * s, 13.0 * s, col, l, Face::Sans);
            }
        };
        match &self.state {
            State::Idle => note(g, "Web page · nothing asked for yet", DIM),
            State::Loading(_) => note(g, "Loading the page…", MUTED),
            State::Failed(e) => note(g, e, ERR),
            State::Shown { doc, .. } => {
                let doc = doc.clone();
                let key = super::browser_layout::key_of(body.w, s);
                if self.layout.as_ref().is_none_or(|(_, k)| *k != key) {
                    self.layout = Some((layout(g, &doc, body.w, s, &[], 0), key));
                }
                let (l, _) = self.layout.as_ref().expect("laid out");
                if let Some(c) = l.canvas {
                    g.rect(body.x, body.y, body.w, body.h, c);
                }
                self.content_h = l.height;
                self.scroll = self.scroll.clamp_safe(0.0, (self.content_h - body.h).max(0.0));
                for p in &l.prims {
                    let (dx, dy) = (body.x + p.x, body.y + p.y - self.scroll);
                    if dy + p.h < body.y || dy > body.y + body.h {
                        continue;
                    }
                    match &p.kind {
                        Kind::Rect { color, radius } => {
                            if *radius > 0.0 {
                                g.rrect(dx, dy, p.w, p.h, *radius, *color);
                            } else {
                                g.rect(dx, dy, p.w, p.h, *color);
                            }
                        }
                        Kind::Text { text, size, face, color, link } => {
                            g.text(dx, dy, *size, *color, text, *face);
                            if let Some(i) = link {
                                self.links.push((R::new(dx, dy, p.w, p.h), *i));
                            }
                            let _ = link;
                        }
                        // pictures are not fetched here (no requests to other sites): the place is kept, empty
                        Kind::Image { .. } => g.rrect(dx, dy, p.w, p.h, 4.0 * s, [255, 255, 255, 16]),
                        _ => {}
                    }
                }
            }
        }
        if self.links.is_empty() {
            // a link the text prims do not name (a picture's, a button-like box): the first rectangle the layout kept for it
            if let Some((l, _)) = &self.layout {
                for (i, r) in l.links.iter().enumerate() {
                    if let Some(r) = r {
                        let at = R::new(body.x + r.x, body.y + r.y - self.scroll, r.w, r.h);
                        if at.y + at.h >= body.y && at.y <= body.y + body.h {
                            self.links.push((at, i));
                        }
                    }
                }
            }
        }
        if let Some(h) = &self.left {
            let strip_r = R::new(rect.x, rect.y + rect.h - strip, rect.w, strip);
            g.rect(strip_r.x, strip_r.y, strip_r.w, strip_r.h, crate::gfx::rgb(60, 30, 30));
            let t = g.fit(12.0 * s, &format!("Left the allowed sites: {h} is not shown here"), Face::Sans, strip_r.w - 12.0 * s);
            g.text(strip_r.x + 6.0 * s, strip_r.y + 4.0 * s, 12.0 * s, TEXT, &t, Face::Sans);
        }
        g.rrect_outline(rect.x, rect.y, rect.w, rect.h, 6.0 * s, 1.0, [255, 255, 255, 40]);
    }
}

#[cfg(test)]
#[path = "tile_embed_page_tests.rs"]
mod tests;
