//! What a click on a link in the transcript does. Nothing here runs by itself: a link is opened only by the person's click (or
//! Enter on a link the keyboard is on), never because a message contains it, and `javascript:`, `data:` and the like are never
//! opened at all. The pure decisions (what an address is, what is wrong with it) are in `crate::links`.

use super::*;
use crate::ctxmenu::Item;
use crate::links::{self, Kind};
use crate::mdtext::LinkRef;

/// How the person asked for the link to open.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OpenHow {
    /// A click or Enter: the natural place (the IDE's Browser tab in the IDE, a browser tile elsewhere).
    Click,
    /// A new tab of the BROWSER tile (Back returns here).
    NewTab,
    /// The IDE's Browser tab.
    Ide,
}

/// A place the chat cannot open by itself: the app around it does (the IDE, the shell).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LinkOut {
    /// The BROWSER tile, in a new tab; Back returns to the app that asked.
    Browser(String),
    /// The IDE's Browser tab.
    IdeBrowser(String),
    /// A file of the workspace, at a line.
    Editor { path: String, line: Option<usize> },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum CardAct {
    /// Only Copy and Close.
    CopyOnly,
    /// Also "Open in the system browser" (asked once per session).
    SystemOpen,
}

/// A small card over the chat: why a link is not opened, and Copy.
#[derive(Clone, Debug)]
pub struct LinkCard {
    pub title: String,
    pub body: String,
    pub url: String,
    act: CardAct,
}

#[derive(Default)]
pub struct LinkState {
    out: Option<LinkOut>,
    pub card: Option<LinkCard>,
    menu: Option<LinkRef>,
    req: Option<(String, String)>,
}

pub const CARD_BASE: usize = 70_000;

static SYSTEM_OK: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

thread_local! {
    /// Tests: pretend this is a desktop window, and catch the program that would run instead of starting it.
    static TEST_SYSTEM: std::cell::RefCell<Option<std::rc::Rc<std::cell::RefCell<Vec<Vec<String>>>>>> = const { std::cell::RefCell::new(None) };
}

#[cfg(test)]
pub fn test_system(on: bool) -> std::rc::Rc<std::cell::RefCell<Vec<Vec<String>>>> {
    let log = std::rc::Rc::new(std::cell::RefCell::new(Vec::new()));
    TEST_SYSTEM.with(|t| *t.borrow_mut() = on.then(|| log.clone()));
    SYSTEM_OK.store(false, std::sync::atomic::Ordering::Relaxed);
    log
}

fn system_available() -> bool {
    TEST_SYSTEM.with(|t| t.borrow().is_some()) || iyakki_agent::webengine::host_session()
}

fn run_system(url: &str) {
    let argv = vec!["xdg-open".to_string(), url.to_string()];
    if let Some(log) = TEST_SYSTEM.with(|t| t.borrow().clone()) {
        log.borrow_mut().push(argv);
        return;
    }
    // the desktop's own variables, so the program finds the person's session
    let env: Vec<(String, String)> = ["DISPLAY", "WAYLAND_DISPLAY", "XDG_RUNTIME_DIR", "DBUS_SESSION_BUS_ADDRESS", "XAUTHORITY", "XDG_CURRENT_DESKTOP"].iter().filter_map(|k| std::env::var(k).ok().map(|v| (k.to_string(), v))).collect();
    std::thread::spawn(move || {
        let _ = iyakki_agent::wsexec::run_argv(&std::env::temp_dir(), &argv, &env, std::time::Duration::from_secs(10), 4096);
    });
}

impl ChatApp {
    /// Add a message to the transcript (tests of the app around the chat).
    #[cfg(test)]
    pub fn push_for_test(&mut self, m: Message) {
        self.msgs.push(m);
    }

    /// The place the app around the chat should open (the IDE does this on its heartbeat).
    pub fn take_link_out(&mut self) -> Option<LinkOut> {
        self.links.out.take()
    }

    pub(super) fn take_link_request(&mut self) -> Option<(String, String)> {
        self.links.req.take()
    }

    fn in_ide(&self) -> bool {
        self.project.is_some()
    }

    fn standalone(&self) -> bool {
        self.project.is_none()
    }

    fn note(&mut self, t: impl Into<String>) {
        self.info = Some(t.into());
    }

    /// Hand a place to whoever shows it: the shell (standalone chat) or the app around the chat.
    fn send_out(&mut self, out: LinkOut) {
        if let (true, LinkOut::Browser(u)) = (self.standalone(), &out) {
            crate::apps::browser::request_open(u);
            self.links.req = Some(("browser".into(), u.clone()));
            return;
        }
        self.links.out = Some(out);
    }

    /// The person clicked a link (or pressed Enter on it, or chose Open in its menu).
    pub fn activate_link(&mut self, l: &LinkRef, how: OpenHow, cx: &AppCx) {
        let url = l.url.trim().to_string();
        cx.audit.record(&cx.session.user, "chat.link", &links::shown(&url, 120));
        self.links.card = None;
        match links::classify(&url) {
            Kind::Http { .. } => self.open_http(&url, l, how),
            Kind::Blank => self.note("about:blank is an empty page: nothing to open."),
            Kind::Mail(addr) => {
                crate::ui::clipboard::copy(&addr);
                self.note(format!("Copied {addr}: IYAKKI has no mail program to start a message with."));
            }
            Kind::Tel(n) => {
                crate::ui::clipboard::copy(&n);
                self.note(format!("Copied {n}: IYAKKI cannot place calls or send texts."));
            }
            Kind::Geo(_) => match links::geo_to_https(&url) {
                Some(h) => self.open_http(&h, l, how),
                None => self.error = Some("That location link has no usable coordinates.".into()),
            },
            Kind::File { path, line } => self.open_file_link(&path, line, cx),
            Kind::Never(s) => self.error = Some(format!("{s}: links are never opened (they can run code or hide what they are).")),
            Kind::Confirm(s) => {
                self.links.card = Some(LinkCard { title: format!("IYAKKI cannot open {s}:// links"), body: format!("This link uses {s}, a program IYAKKI does not have here. You can copy the address and use it elsewhere."), url, act: CardAct::CopyOnly });
            }
            Kind::Unknown(s) => {
                let what = if s.is_empty() { "This is not an address IYAKKI knows how to open".to_string() } else { format!("IYAKKI does not know the {s}: protocol") };
                self.links.card = Some(LinkCard { title: what, body: "Nothing was opened. You can copy the address.".into(), url, act: CardAct::CopyOnly });
            }
        }
    }

    fn open_http(&mut self, url: &str, l: &LinkRef, how: OpenHow) {
        if l.warn.insecure {
            self.note("This page is not encrypted (http): what you type there can be read on the way.");
        } else if l.warn.mismatch || l.warn.lookalike {
            self.note(format!("Careful: {}", l.warn.text(url)));
        }
        let out = match (how, self.in_ide()) {
            (OpenHow::NewTab, _) => LinkOut::Browser(url.to_string()),
            (_, true) => LinkOut::IdeBrowser(url.to_string()),
            _ => LinkOut::Browser(url.to_string()),
        };
        self.send_out(out);
    }

    fn open_file_link(&mut self, path: &str, line: Option<usize>, cx: &AppCx) {
        if path.split('/').any(|p| p == "..") {
            self.error = Some("That file link leaves the folder it is in, so it is not opened.".into());
            return;
        }
        if let Some(p) = &self.project {
            let root = p.root.clone();
            let rel = match path.strip_prefix('/') {
                Some(_) => match std::path::Path::new(path).strip_prefix(&root) {
                    Ok(r) => r.to_string_lossy().into_owned(),
                    Err(_) => {
                        self.error = Some(format!("{path} is outside this workspace, so it is not opened."));
                        return;
                    }
                },
                None => path.to_string(),
            };
            if !root.join(&rel).is_file() {
                self.error = Some(format!("There is no file {rel} in this workspace."));
                return;
            }
            self.links.out = Some(LinkOut::Editor { path: rel, line });
            return;
        }
        if self.dash.is_some() {
            crate::ui::clipboard::copy(path);
            self.note(format!("Copied {path}: open files from CHAT or the AI IDE."));
            return;
        }
        // elsewhere: the person's own folders, as the file tools see them
        let policy = &cx.session.policy;
        let roots = iyakki_agent::scope::FileRoots::new(policy, &cx.session.user, cx.services.dir());
        let cwd = self.session.as_ref().and_then(|s| cx.services.session_workdir(&cx.session.user, &s.id).ok()).unwrap_or_else(|| roots.home.clone());
        let real = match roots.resolve(&cwd, path) {
            Ok(r) if roots.read_ok(policy, &cx.session.user, &r) => r,
            Ok(r) => {
                self.error = Some(format!("{} is outside the folders your account may read, so it is not opened.", r.display()));
                return;
            }
            Err(e) => {
                self.error = Some(format!("Not opened: {e}"));
                return;
            }
        };
        if !real.is_file() {
            self.error = Some(format!("There is no file {}.", real.display()));
            return;
        }
        if !policy.allows_app(iyakki_sec::policy::App::Edit) {
            self.error = Some("Your account may not use the editor.".into());
            return;
        }
        if self.agent || self.run.is_some() {
            self.error = Some("Turn the agent off (Ctrl+G) to open the file in the editor.".into());
            return;
        }
        let mut ed = EditApp::new(cx);
        ed.embedded = true;
        match ed.open_path(cx, &real.to_string_lossy()) {
            Ok(()) => {
                if let Some(l) = line {
                    ed.goto_line(l);
                }
                self.editor = Some(ed);
                self.edit_focus = true;
            }
            Err(e) => self.error = Some(e),
        }
    }

    /// Open in the person's own desktop browser (window mode only; the first time asks).
    fn open_system(&mut self, url: &str) {
        if !system_available() {
            self.error = Some("The system browser can only be started from a desktop window.".into());
            return;
        }
        if !matches!(links::classify(url), Kind::Http { .. }) {
            self.error = Some("Only web addresses (http, https) are opened in the system browser.".into());
            return;
        }
        if SYSTEM_OK.load(std::sync::atomic::Ordering::Relaxed) {
            run_system(url);
            self.note("Opened in the system browser.");
        } else {
            self.links.card = Some(LinkCard { title: "Open in the system browser?".into(), body: "This leaves IYAKKI: the page opens in the browser of the computer this window runs on, with its cookies and no protection from IYAKKI's policy. Asked once per session.".into(), url: url.to_string(), act: CardAct::SystemOpen });
        }
    }

    // ------------------------------------------------------------------ the right-click menu

    pub fn link_menu_items(&mut self, x: i32, y: i32) -> Vec<Item> {
        let Some(l) = self.view.link_at(x as f32, y as f32) else {
            self.links.menu = None;
            return Vec::new();
        };
        let kind = links::classify(&l.url);
        let web = matches!(kind, Kind::Http { .. } | Kind::Geo(_));
        let mut v = vec![Item::new("link.open", "Open", "Enter")];
        if web {
            v.push(Item::new("link.tab", "Open in new browser tab", ""));
            if self.in_ide() {
                v.push(Item::new("link.ide", "Open in IDE browser", ""));
            }
            if system_available() && matches!(kind, Kind::Http { .. }) {
                v.push(Item::new("link.sys", "Open in the system browser", ""));
            }
        }
        v.push(Item::new("link.copy", "Copy link address", ""));
        v.push(Item::new("link.text", "Copy link text", ""));
        if links::is_map_link(&l.url) {
            v.push(Item::new("link.map", "Copy coordinates / address", "").enabled(links::map_text(&l.url).is_some()));
        }
        v.push(Item::separator());
        self.links.menu = Some(l);
        v
    }

    /// A choice of the link menu. False: not a link item.
    pub fn link_menu_pick(&mut self, id: &str, cx: &AppCx) -> bool {
        if !id.starts_with("link.") {
            return false;
        }
        let Some(l) = self.links.menu.take() else { return true };
        match id {
            "link.open" => self.activate_link(&l, OpenHow::Click, cx),
            "link.tab" => self.activate_link(&l, OpenHow::NewTab, cx),
            "link.ide" => self.activate_link(&l, OpenHow::Ide, cx),
            "link.sys" => {
                let u = if let Kind::Geo(_) = links::classify(&l.url) { links::geo_to_https(&l.url).unwrap_or_default() } else { l.url.clone() };
                self.open_system(&u)
            }
            "link.copy" => crate::ui::clipboard::copy(&l.url),
            "link.text" => crate::ui::clipboard::copy(&l.text),
            "link.map" => {
                if let Some(t) = links::map_text(&l.url) {
                    crate::ui::clipboard::copy(&t);
                }
            }
            _ => {}
        }
        true
    }

    // ------------------------------------------------------------------ the card

    /// Keys and clicks while the card is up. True when the event was for it.
    pub(super) fn link_card_event(&mut self, ev: &Event) -> bool {
        let Some(card) = self.links.card.clone() else { return false };
        let primary = card.act == CardAct::SystemOpen;
        match ev {
            Event::Tick | Event::MouseMove(..) | Event::MouseUp(..) => return false,
            Event::Tab(n) if *n == CARD_BASE && primary => self.card_open(&card),
            Event::Key(Key::Enter) if primary => self.card_open(&card),
            Event::Tab(n) if *n == CARD_BASE + 1 => {
                crate::ui::clipboard::copy(&card.url);
                self.links.card = None;
            }
            Event::Key(Key::Enter) | Event::Key(Key::Char('c')) => {
                crate::ui::clipboard::copy(&card.url);
                self.links.card = None;
            }
            _ => self.links.card = None,
        }
        true
    }

    fn card_open(&mut self, card: &LinkCard) {
        SYSTEM_OK.store(true, std::sync::atomic::Ordering::Relaxed);
        self.links.card = None;
        run_system(&card.url);
        self.note("Opened in the system browser.");
    }

    pub(super) fn draw_link_card(&mut self, g: &mut Gfx, full: R, s: f32) {
        let Some(card) = &self.links.card else { return };
        let w = (full.w - 40.0 * s).clamp(260.0 * s, 560.0 * s);
        let size = 14.0 * s;
        let body = g.wrap(size, &card.body, Face::Sans, w - 32.0 * s);
        let shown = links::shown(&card.url, 90);
        let h = (24.0 + 26.0 + 22.0 + 46.0) * s + body.len() as f32 * size * 1.35;
        let r = R::new(full.x + (full.w - w) / 2.0, full.y + (full.h - h) / 2.0, w, h);
        g.hit(full, Event::Tick, "backdrop");
        g.rect(full.x, full.y, full.w, full.h, [0, 0, 0, 120]);
        g.rrect(r.x, r.y, r.w, r.h, 12.0 * s, rgb(20, 28, 52));
        g.rrect_outline(r.x, r.y, r.w, r.h, 12.0 * s, 1.2, rgb(90, 110, 170));
        let mut y = r.y + 14.0 * s;
        g.text(r.x + 16.0 * s, y, 15.5 * s, TEXT, &card.title, Face::Bold);
        y += 28.0 * s;
        for l in &body {
            g.text(r.x + 16.0 * s, y, size, rgb(200, 210, 235), l, Face::Sans);
            y += size * 1.35;
        }
        y += 6.0 * s;
        g.text(r.x + 16.0 * s, y, 12.5 * s, MUTED, &shown, Face::Mono);
        y += 26.0 * s;
        let mut x = r.x + 16.0 * s;
        if card.act == CardAct::SystemOpen {
            x += g.button(x, y, 14.0 * s, "Open in the system browser", ACCENT, Event::Tab(CARD_BASE)) + 12.0 * s;
        }
        x += g.button(x, y, 14.0 * s, "Copy address", ACCENT, Event::Tab(CARD_BASE + 1)) + 12.0 * s;
        g.button(x, y, 14.0 * s, "Close", MUTED, Event::Tab(CARD_BASE + 2));
    }
}
